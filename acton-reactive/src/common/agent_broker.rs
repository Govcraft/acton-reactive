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
use futures::future::join_all;
use tracing::{instrument, trace};

use crate::actor::{AgentConfig, Idle, ManagedAgent};
use crate::common::{AgentHandle, AgentRuntime, BrokerRef};
use crate::message::{BrokerRequest, BrokerRequestEnvelope, SubscribeBroker};
use crate::traits::AgentHandleInterface;

#[cfg(feature = "ipc")]
use parking_lot::RwLock;
#[cfg(feature = "ipc")]
use crate::common::ipc::{IpcPushNotification, IpcTypeRegistry, SubscriptionManager};

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
    /// Reference to IPC subscription manager for forwarding broadcasts to external clients.
    #[cfg(feature = "ipc")]
    ipc_subscription_manager: Arc<RwLock<Option<Arc<SubscriptionManager>>>>,
    /// Reference to IPC type registry for looking up type names.
    #[cfg(feature = "ipc")]
    ipc_type_registry: Arc<IpcTypeRegistry>,
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
        let mut broker_agent: ManagedAgent<Idle, Self> =
            ManagedAgent::new(Some(&runtime), Some(&actor_config));

        // Set IPC-related fields on the broker model
        #[cfg(feature = "ipc")]
        {
            broker_agent.model.ipc_subscription_manager =
                runtime.0.ipc_subscription_manager.clone();
            broker_agent.model.ipc_type_registry = runtime.0.ipc_type_registry.clone();
        }

        // Configure the broker agent's message handlers.
        broker_agent
            .mutate_on::<BrokerRequest>(|agent, event| {
                // Handler for broadcast requests.
                trace!(message_type = ?event.message.message_type_id, "Broker received BrokerRequest");
                let subscribers = agent.model.subscribers.clone(); // Clone Arc<DashMap>
                let message_to_broadcast = event.message.clone(); // Clone the BrokerRequest

                // Clone IPC-related references for async block
                #[cfg(feature = "ipc")]
                let ipc_sub_mgr = agent.model.ipc_subscription_manager.clone();
                #[cfg(feature = "ipc")]
                let ipc_type_reg = agent.model.ipc_type_registry.clone();

                Box::pin(async move {
                    // Call the static broadcast method.
                    Self::broadcast(subscribers, message_to_broadcast.clone()).await;

                    // Forward to IPC subscribers if available
                    #[cfg(feature = "ipc")]
                    Self::forward_to_ipc(ipc_sub_mgr, ipc_type_reg, &message_to_broadcast);
                })
            })
            .act_on::<SubscribeBroker>(|agent, event| {
                // Handler for subscription requests.
                let subscription_msg = event.message.clone();
                let type_id = subscription_msg.message_type_id;
                let subscriber_handle = subscription_msg.subscriber_context.clone();
                let subscriber_id = subscription_msg.subscriber_id;
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
                    let () = handle.send(envelope_to_send).await;
                }
            });

            // Wait for all send operations to complete.
            join_all(send_futures).await;
            trace!(count = num_subscribers, message_type = ?message_type_id, "Broadcast sends completed");
        } else {
            trace!(message_type = ?message_type_id, "No subscribers found for message type");
        }
    }

    /// Forwards a broadcast message to IPC clients subscribed to this message type.
    ///
    /// This method checks if there's an active IPC subscription manager and, if so,
    /// looks up the message type name from the IPC type registry and forwards the
    /// message to all subscribed external clients.
    ///
    /// Only available when the `ipc` feature is enabled.
    #[cfg(feature = "ipc")]
    fn forward_to_ipc(
        ipc_sub_mgr: Arc<RwLock<Option<Arc<SubscriptionManager>>>>,
        ipc_type_reg: Arc<IpcTypeRegistry>,
        request: &BrokerRequest,
    ) {
        // Check if there's an active subscription manager
        let sub_mgr = {
            let guard = ipc_sub_mgr.read();
            guard.clone()
        };

        let Some(sub_mgr) = sub_mgr else {
            trace!("No IPC subscription manager active, skipping IPC forward");
            return;
        };

        // Look up the type name in the IPC registry
        let Some(type_name) = ipc_type_reg.get_type_name_by_id(&request.message_type_id) else {
            trace!(type_id = ?request.message_type_id, "Type not registered for IPC, skipping forward");
            return;
        };

        // Serialize the message payload to JSON for IPC transmission
        let payload_json = match ipc_type_reg.serialize_by_type_id(
            &request.message_type_id,
            request.message.as_ref(),
        ) {
            Ok(json) => json,
            Err(e) => {
                trace!(type_name, error = %e, "Failed to serialize payload for IPC forward");
                return;
            }
        };

        // Create and forward the push notification
        let notification = IpcPushNotification::new(type_name.clone(), None, payload_json);
        sub_mgr.forward_to_subscribers(&notification);
        trace!(type_name, "Forwarded broadcast to IPC subscribers");
    }
}
