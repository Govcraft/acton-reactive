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
use std::time::{Duration, Instant};

use futures::future::join_all;
use futures::stream::{FuturesUnordered, StreamExt};
use tracing::{instrument, trace}; // Removed unused error import

use crate::actor::ManagedAgent;
use crate::common::{Envelope, OutboundEnvelope, ReactorItem, ReactorMap};
use crate::common::config::CONFIG;
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
        self.cancellation_token.clone().map(|cancellation_token| {
            OutboundEnvelope::new(
                MessageAddress::new(self.handle.outbox.clone(), self.id.clone()),
                cancellation_token,
            )
        })
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
        // Only construct if both parent and cancellation_token exist
        let cancellation_token = self.cancellation_token.clone()?;
        self.parent.as_ref().map(|parent_handle| {
            OutboundEnvelope::new_with_recipient(
                MessageAddress::new(self.handle.outbox.clone(), self.id.clone()), // Self is sender
                parent_handle.reply_address(), // Parent is recipient
                cancellation_token,
            )
        })
    }

    // wake() and terminate() are internal implementation details (`pub(crate)` or private)
    // and do not require public documentation.
    #[instrument(skip(mutable_reactors, read_only_reactors, self))]
    pub(crate) async fn wake(
        &mut self,
        mutable_reactors: ReactorMap<Agent>,
        read_only_reactors: ReactorMap<Agent>,
    ) {
        (self.after_start)(self).await;
        // Assert that cancellation_token always exists; it must never be missing.
        assert!(
            self.cancellation_token.is_some(),
            "ManagedAgent in Started state must always have a cancellation_token"
        );
        let cancel_token = self.cancellation_token.clone().unwrap();
        let mut cancel = Box::pin(cancel_token.cancelled());

        let mut _terminate_signal_received = false;

        use crate::common::config::CONFIG;
        // Concurrent execution buffer for read-only handlers
        let mut read_only_futures = FuturesUnordered::new();
        let high_water_mark = CONFIG.limits.concurrent_handlers_high_water_mark;
        let max_wait_duration = Duration::from_millis(CONFIG.timeouts.read_only_handler_flush);
        let mut last_flush_time = Instant::now();

        loop {
            tokio::select! {
                // React immediately to cancellation
                () = &mut cancel => {
                    trace!("Forceful cancellation triggered for agent: {}", self.id());
                    // Flush any remaining read-only futures before breaking
                    if !read_only_futures.is_empty() {
                        trace!("Flushing {} remaining read-only futures on cancellation", read_only_futures.len());
                        while read_only_futures.next().await.is_some() {}
                    }
                    break; // Immediate break on forceful cancellation.
                }

                // Check if we should flush read-only futures due to time limit
                () = tokio::time::sleep_until((last_flush_time + max_wait_duration).into()), if !read_only_futures.is_empty() => {
                    trace!("Flushing {} read-only futures due to time limit", read_only_futures.len());
                    while read_only_futures.next().await.is_some() {}
                    last_flush_time = Instant::now();
                }

                incoming_opt = self.inbox.recv() => {
                    let Some(incoming_envelope) = incoming_opt else { break; };
                    let type_id;
                    let mut envelope;
                    trace!(
                        "Received envelope from: {}",
                        incoming_envelope.reply_to.sender.root
                    );
                    trace!(
                        "Message type: {}",
                        type_name_of_val(&incoming_envelope.message)
                    );

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
                    if let Some(reactor) = mutable_reactors.get(&type_id) {
                        // Flush any pending read-only futures before processing mutable handlers
                        if !read_only_futures.is_empty() {
                            trace!("Flushing {} read-only futures before mutable handler", read_only_futures.len());
                            while read_only_futures.next().await.is_some() {}
                            last_flush_time = Instant::now();
                        }

                        match reactor.value() {
                            ReactorItem::Mutable(fut) => {
                                // Legacy handler: await, always Ok
                                fut(self, &mut envelope).await;
                            }
                            ReactorItem::MutableFallible(fut) => {
                                // New Result-based handler: await and trigger error handler on Err
                                let result = fut(self, &mut envelope).await;
                                if let Err((err, error_type_id)) = result {
                                    let message_type_id = envelope.message.as_any().type_id();
                                    if let Some(handler) =
                                        self.error_handler_map.remove(&(message_type_id, error_type_id))
                                    {
                                        let fut = handler(self, &mut envelope, err.as_ref());
                                        fut.await;
                                        self.error_handler_map.insert((message_type_id, error_type_id), handler);
                                    } else {
                                        tracing::error!(
                                            "Unhandled error from message handler in agent {}: {:?}",
                                            self.id(),
                                            err
                                        );
                                    }
                                }
                            }
                            ReactorItem::ReadOnly(_) => {
                                // This should not happen - read-only handlers should be in read_only_reactors
                                tracing::warn!("Found read-only handler in mutable_reactors map");
                            }
                            ReactorItem::ReadOnlyFallible(_) => {
                                // This should not happen - read-only handlers should be in read_only_reactors
                                tracing::warn!("Found read-only Result handler in mutable_reactors map");
                            }
                        }
                    } else if let Some(reactor) = read_only_reactors.get(&type_id) {
                        match reactor.value() {
                            ReactorItem::ReadOnly(fut) => {
                                // Concurrent read-only handler: add to buffer
                                let fut = fut(self, &mut envelope);
                                read_only_futures.push(tokio::spawn(async move {
                                    fut.await;
                                }));

                                // Check if we've hit the high water mark
                                if read_only_futures.len() >= high_water_mark {
                                    trace!("Flushing {} read-only futures due to high water mark", read_only_futures.len());
                                    while read_only_futures.next().await.is_some() {}
                                    last_flush_time = Instant::now();
                                }
                            }
                            ReactorItem::ReadOnlyFallible(fut) => {
                                // Concurrent read-only handler with error handling: add to buffer
                                let fut = fut(self, &mut envelope);
                                read_only_futures.push(tokio::spawn(async move {
                                    if let Err((err, _error_type_id)) = fut.await {
                                        // Note: Error handling for read-only handlers is not supported
                                        // as we don't have access to the agent for error handlers
                                        tracing::error!(
                                            "Unhandled error from read-only message handler: {:?}",
                                            err
                                        );
                                    }
                                }));

                                // Check if we've hit the high water mark
                                if read_only_futures.len() >= high_water_mark {
                                    trace!("Flushing {} read-only Result futures due to high water mark", read_only_futures.len());
                                    while read_only_futures.next().await.is_some() {}
                                    last_flush_time = Instant::now();
                                }
                            }
                            _ => {
                                // This should not happen - mutable handlers should be in mutable_reactors
                                tracing::warn!("Found mutable handler in read_only_reactors map");
                            }
                        }
                    } else if matches!(envelope.message.as_any().downcast_ref::<SystemSignal>(), Some(SystemSignal::Terminate))
                    {
                        // Flush any remaining read-only futures before processing terminate
                        if !read_only_futures.is_empty() {
                            trace!("Flushing {} read-only futures before terminate", read_only_futures.len());
                            while read_only_futures.next().await.is_some() {}
                        }

                        trace!("Terminate signal received for agent: {}. Closing inbox.", self.id());
                        _terminate_signal_received = true; // Set flag
                        (self.before_stop)(self).await; // Execute before_stop hook
                        self.inbox.close(); // Close inbox to stop receiving new messages.
                        // Do NOT break here. Allow loop to drain existing messages.
                    } else {
                        trace!(
                            "No handler found for message type {:?} for agent {}",
                            type_id,
                            self.id()
                        );
                        // Optionally log or handle unknown message types
                    }
                }
            }
        }
        // After loop breaks (either gracefully or forcefully), perform final termination steps.
        trace!(
            "Message loop finished for agent: {}. Initiating final termination.",
            self.id()
        );

        // Flush any remaining read-only futures before final termination
        if !read_only_futures.is_empty() {
            trace!(
                "Flushing {} remaining read-only futures before final termination",
                read_only_futures.len()
            );
            while read_only_futures.next().await.is_some() {}
        }

        self.terminate().await; // Stop children and other cleanup.
        (self.after_stop)(self).await;
        trace!("Agent {} stopped.", self.id());
    }

    #[instrument(skip(self))]
    async fn terminate(&mut self) {
        trace!("Terminating children for agent: {}", self.id());
        // Stop all child agents concurrently.
        use std::time::Duration;
        use tokio::time::timeout as tokio_timeout;

        let timeout_ms = CONFIG.timeouts.agent_shutdown;

        let stop_futures: Vec<_> = self
            .handle
            .children()
            .iter()
            .map(|item| {
                let child_handle = item.value().clone();
                async move {
                    trace!("Sending stop signal to child: {}", child_handle.id());
                    let stop_res =
                        tokio_timeout(Duration::from_millis(timeout_ms), child_handle.stop()).await;
                    match stop_res {
                        Ok(Ok(())) => {
                            trace!(
                                "Stop signal sent to and child {} shut down successfully.",
                                child_handle.id()
                            );
                        }
                        Ok(Err(e)) => {
                            tracing::error!(
                                "Stop signal to child {} returned error: {:?}",
                                child_handle.id(),
                                e
                            );
                        }
                        Err(_) => {
                            tracing::error!(
                                "Shutdown timeout for child {} after {} ms",
                                child_handle.id(),
                                timeout_ms
                            );
                        }
                    }
                }
            })
            .collect();

        join_all(stop_futures).await; // Wait for all stop signals to be sent/processed.

        trace!("All children stopped for agent: {}.", self.id());
    }
}
