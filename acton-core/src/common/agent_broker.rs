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

use std::any::TypeId;
use std::collections::HashSet;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use acton_ern::Ern;
use dashmap::DashMap;
use futures::channel::oneshot::Canceled;
use futures::future::join_all;
use tokio_util::sync::CancellationToken;
use tracing::*;

use crate::actor::{AgentConfig, Idle, ManagedAgent};
use crate::common::{AgentHandle, AgentRuntime, BrokerRef};
use crate::message::{BrokerRequest, BrokerRequestEnvelope, SubscribeBroker};
use crate::traits::AgentHandleInterface;

/// Manages message subscriptions and broadcasts messages to interested subscribers.
///
/// The `AgentBroker` acts as a central publish-subscribe hub within the Acton system.
/// Agents can subscribe to specific message types using the [`Subscribable`](crate::traits::Subscribable)
/// trait (typically via their [`AgentHandle`]). When a message is sent to the broker
/// (usually wrapped in a [`BrokerRequest`]), the broker identifies all agents subscribed
/// to that message's type and forwards the message to them concurrently.
///
/// Internally, the `AgentBroker` runs as a specialized [`ManagedAgent`] that handles
/// [`SubscribeBroker`] messages to manage its subscription list and [`BrokerRequest`]
/// messages to trigger broadcasts.
///
/// It also dereferences ([`Deref`] and [`DerefMut`]) to its underlying [`AgentHandle`],
/// allowing direct use of handle methods where appropriate.
#[derive(Default, Debug, Clone)]
pub struct AgentBroker {
    /// A thread-safe map storing subscribers keyed by message `TypeId`.
    /// The value is a set of tuples containing the subscriber's ID (`Ern`) and its `AgentHandle`.
    subscribers: Subscribers,
    /// The underlying handle for the broker agent itself.
    agent_handle: AgentHandle,
}

/// Type alias for the internal storage of subscribers.
/// `TypeId` maps to a `HashSet` of `(Ern, AgentHandle)` tuples.
type Subscribers = Arc<DashMap<TypeId, HashSet<(Ern, AgentHandle)>>>;

/// Allows immutable access to the underlying [`AgentHandle`] of the `AgentBroker`.
///
/// This enables calling methods from [`AgentHandleInterface`] directly on an `AgentBroker` instance.
impl Deref for AgentBroker {
    type Target = AgentHandle;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.agent_handle
    }
}

/// Allows mutable access to the underlying [`AgentHandle`] of the `AgentBroker`.
///
/// This enables modifying the internal state of the broker's `AgentHandle`. Use with caution,
/// as direct mutable access might bypass intended broker logic if not used carefully.
impl DerefMut for AgentBroker {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.agent_handle
    }
}

impl AgentBroker {
    /// Initializes the broker agent and starts its processing loop.
    ///
    /// This internal function creates the `ManagedAgent` for the broker, configures
    /// its message handlers for `BrokerRequest` (triggering `broadcast`) and
    /// `SubscribeBroker` (adding subscribers), and starts the agent.
    ///
    /// Returns the `AgentHandle` of the initialized broker agent.
    #[instrument]
    pub(crate) async fn initialize(runtime: AgentRuntime) -> BrokerRef {
        let actor_config = AgentConfig::new(Ern::with_root("broker_main").unwrap(), None, None)
            .expect("Couldn't create initial broker config");

        // Assert that the cancellation_token in the runtime is not cancelled before agent creation.
        assert!(
            !runtime.0.cancellation_token.is_cancelled(),
            "ActonInner cancellation_token must be present and active before creating ManagedAgent in AgentBroker::initialize"
        );

        // Create the ManagedAgent for the broker. The model state *is* the AgentBroker itself.
        let mut broker_agent: ManagedAgent<Idle, AgentBroker> =
            ManagedAgent::new(&Some(runtime), Some(actor_config)).await;

        // Configure the broker agent's message handlers.
        broker_agent
            .act_on::<BrokerRequest>(|agent, event| {
                // Handler for broadcast requests.
                trace!(message_type = ?event.message.message_type_id, "Broker received BrokerRequest");
                let subscribers = agent.model.subscribers.clone(); // Clone Arc<DashMap>
                let message_to_broadcast = event.message.clone(); // Clone the BrokerRequest

                Box::pin(async move {
                    // Call the static broadcast method.
                    AgentBroker::broadcast(subscribers, message_to_broadcast).await;
                })
            })
            .act_on::<SubscribeBroker>(|agent, event| {
                // Handler for subscription requests.
                let subscription_msg = event.message.clone();
                let type_id = subscription_msg.message_type_id;
                let subscriber_handle = subscription_msg.subscriber_context.clone();
                let subscriber_id = subscription_msg.subscriber_id.clone();
                trace!(subscriber = %subscriber_id, message_type = ?type_id, "Broker received SubscribeBroker");

                let subscribers_map = agent.model.subscribers.clone(); // Clone Arc<DashMap>
                Box::pin(async move {
                    let subscriber_id_for_insert = subscriber_id.clone(); // Clone before moving
                    // Insert the subscriber into the set for the given message TypeId.
                    subscribers_map
                        .entry(type_id)
                        .or_default() // Get the HashSet or create a new one
                        .insert((subscriber_id_for_insert, subscriber_handle)); // Insert the clone
                    trace!(subscriber = %subscriber_id, message_type = ?type_id, "Subscription added"); // Use original subscriber_id here
                })
            });

        trace!("Starting the AgentBroker agent...");
        let mut broker_handle = broker_agent.start().await;
        // The broker needs a reference to itself to function correctly via its handle.
        broker_handle.broker = Box::from(Some(broker_handle.clone()));
        trace!("AgentBroker started with handle ID: {}", broker_handle.id());
        broker_handle
    }

    /// Broadcasts a message contained within a [`BrokerRequest`] to all relevant subscribers.
    ///
    /// This static method performs the core broadcast logic. It looks up the message type's `TypeId`
    /// in the provided `subscribers` map and asynchronously sends a [`BrokerRequestEnvelope`]
    /// containing the original message payload to each registered subscriber's handle.
    ///
    /// # Arguments
    ///
    /// * `subscribers`: An `Arc<DashMap<TypeId, HashSet<(Ern, AgentHandle)>>>` containing the
    ///   current subscription state.
    /// * `request`: The [`BrokerRequest`] containing the message payload and its `TypeId`.
    pub async fn broadcast(
        subscribers: Subscribers, // Takes the Arc<DashMap>
        request: BrokerRequest,
    ) {
        let message_type_id = request.message_type_id; // Get TypeId from the request
        trace!(message_type = ?message_type_id, "Broadcasting message");

        // Check if there are any subscribers for this message type.
        if let Some(subscribers_set) = subscribers.get(&message_type_id) {
            let num_subscribers = subscribers_set.len();
            trace!(count = num_subscribers, message_type = ?message_type_id, "Found subscribers");

            // Create futures to send the message to each subscriber concurrently.
            let send_futures = subscribers_set.value().iter().map(|(_, subscriber_handle)| {
                let handle = subscriber_handle.clone();
                // Wrap the original message payload in a BrokerRequestEnvelope for sending.
                let envelope_to_send: BrokerRequestEnvelope = request.clone().into();
                async move {
                    trace!(subscriber = %handle.id(), message_type = ?message_type_id, "Sending broadcast");
                    // Send the envelope to the subscriber's handle.
                    // Ignore potential send errors (e.g., closed channel).
                    let _ = handle.send(envelope_to_send).await;
                }
            });

            // Wait for all send operations to complete.
            join_all(send_futures).await;
            trace!(count = num_subscribers, message_type = ?message_type_id, "Broadcast sends completed");
        } else {
            trace!(message_type = ?message_type_id, "No subscribers found for message type");
        }
    }
}
