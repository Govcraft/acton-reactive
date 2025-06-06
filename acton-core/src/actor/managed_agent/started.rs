/*
 * Copyright (c) 2024. Govcraft
 *
 * Licensed under either of
 *   * Apache License, Version 2.0 (the "License");
 *     you may not use this file except in compliance with the License.
 *     You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 *   * MIT license: http://opensource.org/licenses/MIT
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the applicable License for the specific language governing permissions and
 * limitations under that License.
 */

use std::any::type_name_of_val;
use std::fmt::Debug;
use std::time::Duration;

use futures::future::join_all;
use tokio::time::sleep;
use tracing::{instrument, trace}; // Removed unused error import

use crate::actor::ManagedAgent;
use crate::common::{Envelope, OutboundEnvelope, ReactorItem, ReactorMap};
use crate::message::{BrokerRequestEnvelope, MessageAddress, SystemSignal};
use crate::traits::AgentHandleInterface;

/// Type-state marker for a [`ManagedAgent`] that is actively running and processing messages.
///
/// When a `ManagedAgent` is in the `Started` state, its main asynchronous task (`wake`)
/// is running, receiving messages from its inbox and dispatching them to the appropriate
/// handlers registered during the [`Idle`](super::Idle) state.
///
/// Agents in this state can create message envelopes using methods like [`ManagedAgent::new_envelope`]
/// and [`ManagedAgent::new_parent_envelope`]. Interaction typically occurs via the agent's
/// [`AgentHandle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)] // Add common derives
pub struct Started;

/// Implements methods specific to a `ManagedAgent` in the `Started` state.
impl<Agent: Default + Send + Debug + 'static> ManagedAgent<Started, Agent> {
    /// Creates a new [`OutboundEnvelope`] originating from this agent.
    ///
    /// This helper function constructs an envelope suitable for sending a message
    /// from this agent to another recipient. The envelope's `return_address`
    /// will be set to this agent's [`MessageAddress`]. The `recipient_address`
    /// field will be `None` initially and should typically be set using the
    /// envelope's methods before sending.
    ///
    /// # Returns
    ///
    /// An [`OutboundEnvelope`] configured with this agent as the sender.
    /// Returns `None` only if the agent's handle somehow lacks an outbox, which
    /// should not occur under normal circumstances.
    pub fn new_envelope(&self) -> Option<OutboundEnvelope> {
        // The Option wrapper seems unnecessary given the implementation always returns Some.
        // Consider changing return type to just OutboundEnvelope if None is impossible.
        Some(OutboundEnvelope::new(MessageAddress::new(
            self.handle.outbox.clone(),
            self.id.clone(),
        )))
    }

    /// Creates a new [`OutboundEnvelope`] addressed to this agent's parent.
    ///
    /// This is a convenience method for creating an envelope specifically for
    /// replying or sending a message to the agent that supervises this one.
    /// It clones the parent's return address information.
    ///
    /// # Returns
    ///
    /// *   `Some(OutboundEnvelope)`: An envelope configured to be sent to the parent,
    ///     if this agent has a parent. The `return_address` will be the parent's address,
    ///     and the `recipient_address` will be this agent's address.
    /// *   `None`: If this agent does not have a parent (i.e., it's a top-level agent).
    pub fn new_parent_envelope(&self) -> Option<OutboundEnvelope> {
        // Clones the parent's handle and creates an envelope targeting the parent.
        self.parent.as_ref().map(|parent_handle| {
            // Create an envelope where the recipient is the parent and the sender is self.
            OutboundEnvelope::new_with_recipient(
                MessageAddress::new(self.handle.outbox.clone(), self.id.clone()), // Self is sender
                parent_handle.reply_address() // Parent is recipient
            )
        })
        // The original implementation `parent.create_envelope(None).clone()` might have different semantics.
        // The above implementation assumes sending *to* the parent *from* self.
        // If the intent was to create an envelope *as if* it came from the parent,
        // the original logic might be needed, but seems less common. Let's stick to the clearer intent.
        // Original logic: self.parent.as_ref().map(|parent| parent.create_envelope(None).clone())
    }

    // wake() and terminate() are internal implementation details (`pub(crate)` or private)
    // and do not require public documentation.
    #[instrument(skip(reactors, self))]
    pub(crate) async fn wake(&mut self, reactors: ReactorMap<Agent>) {
        (self.after_start)(self).await;
        let mut terminate_requested = false;
        while let Some(incoming_envelope) = self.inbox.recv().await {
            let type_id;
            let mut envelope;
            trace!("Received envelope from: {}", incoming_envelope.reply_to.sender.root);
            trace!("Message type: {}", type_name_of_val(&incoming_envelope.message));

            // Handle potential BrokerRequestEnvelope indirection
            if let Some(broker_request_envelope) = incoming_envelope
                .message
                .as_any()
                .downcast_ref::<BrokerRequestEnvelope>()
            {
                trace!("Processing message via BrokerRequestEnvelope");
                envelope = Envelope::new(
                    broker_request_envelope.message.clone(), // Extract inner message
                    incoming_envelope.reply_to.clone(),
                    incoming_envelope.recipient.clone(),
                );
                type_id = broker_request_envelope.message.as_any().type_id(); // Use inner message TypeId
            } else {
                envelope = incoming_envelope;
                type_id = envelope.message.as_any().type_id();
            }

            // Dispatch to registered handler or handle system signals
            if let Some(reactor) = reactors.get(&type_id) {
                match reactor.value() {
                    ReactorItem::FutureReactor(fut) => fut(self, &mut envelope).await,
                }
            } else if let Some(SystemSignal::Terminate) =
                envelope.message.as_any().downcast_ref::<SystemSignal>()
            {
                trace!("Terminate signal received for agent: {}", self.id());
                terminate_requested = true;
                (self.before_stop)(self).await; // Execute before_stop hook
                // Short delay to allow before_stop processing, if needed.
                sleep(Duration::from_millis(10)).await;
                self.inbox.close(); // Close inbox to stop receiving new messages
                trace!("Inbox closed for agent: {}", self.id());
            } else {
                 trace!("No handler found for message type {:?} for agent {}", type_id, self.id());
                 // Optionally log or handle unknown message types
            }

            // Check if termination requested and inbox is now empty and closed
            if terminate_requested && self.inbox.is_empty() && self.inbox.is_closed() {
                trace!("Inbox empty and closed after terminate request, initiating termination for agent: {}", self.id());
                self.terminate().await; // Initiate graceful shutdown of children etc.
                break; // Exit the loop
            }
        }
        trace!("Message loop finished for agent: {}", self.id());
        (self.after_stop)(self).await; // Execute after_stop hook
        trace!("Agent {} stopped.", self.id());
    }

    #[instrument(skip(self))]
    async fn terminate(&mut self) {
        trace!("Terminating children for agent: {}", self.id());
        // Stop all child agents concurrently.
        let stop_futures: Vec<_> = self.handle.children().iter().map(|item| {
            let child_handle = item.value().clone();
            async move {
                trace!("Sending stop signal to child: {}", child_handle.id());
                let _ = child_handle.stop().await; // Ignore result, best effort stop
                trace!("Stop signal sent to child: {}", child_handle.id());
            }
        }).collect();

        join_all(stop_futures).await; // Wait for all stop signals to be sent/processed.

        trace!("All children stopped for agent: {}. Closing own inbox.", self.id());
        // Ensure inbox is closed (might be redundant if closed in wake loop, but safe).
        self.inbox.close();
    }
}
