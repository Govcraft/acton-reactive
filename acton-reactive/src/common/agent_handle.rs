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
use std::fmt::Debug;
use std::future::Future;
use std::hash::{Hash, Hasher};

use acton_ern::Ern;
use async_trait::async_trait;
use dashmap::DashMap;
use tokio::sync::mpsc;
use tokio_util::task::TaskTracker;
use tracing::{error, instrument, trace, warn}; // warn seems unused

use crate::actor::{Idle, ManagedAgent};
use crate::common::{AgentSender, BrokerRef, OutboundEnvelope, ParentRef};
use crate::message::{BrokerRequest, MessageAddress, SystemSignal};
use crate::prelude::ActonMessage;
use crate::traits::{AgentHandleInterface, Broker, Subscriber};

/// A clonable handle for interacting with an agent.
///
/// `AgentHandle` provides the primary mechanism for communicating with and managing
/// an agent from outside its own execution context. It encapsulates the necessary
/// information to send messages to the agent's inbox (`outbox`), identify the agent (`id`),
/// manage its lifecycle (`stop`), track its tasks (`tracker`), and navigate the
/// supervision hierarchy (`parent`, `children`, `supervise`).
///
/// Handles can be cloned freely, allowing multiple parts of the system to hold references
/// to the same agent. Sending messages through the handle is asynchronous.
///
/// Key functionalities are exposed through implemented traits:
/// *   [`AgentHandleInterface`]: Core methods for interaction (sending messages, stopping, etc.).
/// *   [`Broker`]: Methods for broadcasting messages via the system broker.
/// *   [`Subscriber`]: Method for accessing the system broker handle.
///
/// Equality and hashing are based solely on the agent's unique identifier (`id`).
#[derive(Debug, Clone)]
pub struct AgentHandle {
    /// The unique identifier (`Ern`) for the agent this handle refers to.
    pub(crate) id: Ern,
    /// The sender part of the MPSC channel connected to the agent's inbox.
    pub(crate) outbox: AgentSender,
    /// Tracks the agent's main task and potentially other associated tasks.
    tracker: TaskTracker,
    /// Optional reference to the parent (supervisor) agent's handle.
    /// `None` if this is a top-level agent. Boxed to manage `AgentHandle` size.
    pub parent: Option<Box<ParentRef>>,
    /// Optional reference to the system message broker's handle.
    /// Boxed to manage `AgentHandle` size.
    pub broker: Box<Option<BrokerRef>>,
    /// A map holding handles to the direct children supervised by this agent.
    /// Keys are the string representation of the child agent's `Ern`.
    children: DashMap<String, AgentHandle>,
    /// The agent's cancellation token (clone).
    pub(crate) cancellation_token: tokio_util::sync::CancellationToken,
}

impl Default for AgentHandle {
    /// Creates a default, placeholder `AgentHandle`.
    ///
    /// This handle is typically initialized with a default `Ern`, a closed channel,
    /// and no parent, broker, or children. It's primarily used as a starting point
    /// before being properly configured when a `ManagedAgent` is created.
    fn default() -> Self {
        use crate::common::config::CONFIG;
        
        let dummy_channel_size = CONFIG.limits.dummy_channel_size;
        let (outbox, _) = mpsc::channel(dummy_channel_size); // Create a dummy channel
        Self {
            id: Ern::default(),
            outbox,
            tracker: TaskTracker::new(),
            parent: None,
            broker: Box::new(None),
            children: DashMap::new(),
            cancellation_token: tokio_util::sync::CancellationToken::new(),
        }
    }
}

/// Implements the `Subscriber` trait, allowing access to the broker.
impl Subscriber for AgentHandle {
    /// Returns a clone of the optional broker handle associated with this agent.
    ///
    /// Returns `None` if the agent was not configured with a broker reference.
    fn get_broker(&self) -> Option<BrokerRef> {
        *self.broker.clone() // Clone the Option<BrokerRef> inside the Box
    }
}

/// Implements equality comparison based on the agent's unique ID (`Ern`).
impl PartialEq for AgentHandle {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

/// Derives `Eq` based on the `PartialEq` implementation.
impl Eq for AgentHandle {}

/// Implements hashing based on the agent's unique ID (`Ern`).
impl Hash for AgentHandle {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl AgentHandle {
    /// Starts a child agent and registers it under this agent's supervision.
    ///
    /// This method takes a `ManagedAgent` configured in the [`Idle`] state,
    /// starts its execution by calling its `start` method, and then stores
    /// the resulting child `AgentHandle` in this parent handle's `children` map.
    ///
    /// # Type Parameters
    ///
    /// *   `State`: The user-defined state type of the child agent. Must implement
    ///     `Default`, `Send`, `Debug`, and be `'static`.
    ///
    /// # Arguments
    ///
    /// *   `child`: The [`ManagedAgent<Idle, State>`] instance representing the child agent
    ///     to be started and supervised.
    ///
    /// # Returns
    ///
    /// A `Result` containing:
    /// *   `Ok(AgentHandle)`: The handle of the successfully started and registered child agent.
    /// *   `Err(anyhow::Error)`: If starting the child agent fails.
    #[instrument(skip(self, child))] // Skip child in instrument
    pub async fn supervise<State: Default + Send + Debug + 'static>(
        // Add 'static bound
        &self,
        child: ManagedAgent<Idle, State>,
    ) -> anyhow::Result<Self> {
        let child_id = child.id().clone(); // Get ID before consuming child
        trace!("Supervising child agent with id: {}", child_id);
        let handle = child.start().await; // Start the child agent
        trace!(
            "Child agent {} started, adding to parent {} children map",
            child_id,
            self.id
        );
        self.children.insert(handle.id.to_string(), handle.clone()); // Store child handle
        Ok(handle)
    }
}

/// Implements the `Broker` trait, allowing broadcasting via the associated broker.
impl Broker for AgentHandle {
    /// Sends a message to the associated system broker for broadcasting.
    ///
    /// This method wraps the provided `message` in a [`BrokerRequest`] and sends it
    /// to the broker handle stored within this `AgentHandle`. If no broker handle
    /// is configured, an error is logged.
    ///
    /// # Arguments
    ///
    /// * `message`: The message payload (must implement `ActonMessage`) to be broadcast.
    fn broadcast(&self, message: impl ActonMessage) -> impl Future<Output = ()> + Send + Sync + '_ {
        trace!("Attempting broadcast via handle: {}", self.id);
        async move {
            if let Some(broker_handle) = self.broker.as_ref() {
                trace!("Broker found for handle {}, sending BrokerRequest", self.id);
                // Send the BrokerRequest to the actual broker agent.
                broker_handle.send(BrokerRequest::new(message)).await;
            } else {
                // Log an error if no broker is configured for this agent handle.
                error!(
                    "No broker configured for agent handle {}, cannot broadcast.",
                    self.id
                );
            }
        }
    }
}

/// Implements the core interface for interacting with an agent.
#[async_trait]
impl AgentHandleInterface for AgentHandle {
    /// Returns the [`MessageAddress`] for this agent, used for sending replies.
    #[inline]
    fn reply_address(&self) -> MessageAddress {
        MessageAddress::new(self.outbox.clone(), self.id.clone())
    }

    /// Creates an [`OutboundEnvelope`] for sending a message from this agent.
    ///
    /// # Arguments
    ///
    /// * `recipient_address`: An optional [`MessageAddress`] specifying the recipient.
    ///   If `None`, the envelope is created without a specific recipient (e.g., for broadcasting
    ///   or when the recipient is set later).
    ///
    /// # Returns
    ///
    /// An [`OutboundEnvelope`] with the `return_address` set to this agent's address.
    #[instrument(skip(self))]
    fn create_envelope(&self, recipient_address: Option<MessageAddress>) -> OutboundEnvelope {
        let return_address = self.reply_address();
        trace!(sender = %return_address.sender.root, recipient = ?recipient_address.as_ref().map(|r| r.sender.root.as_str()), "Creating envelope");
        if let Some(recipient) = recipient_address {
            OutboundEnvelope::new_with_recipient(
                return_address,
                recipient,
                self.cancellation_token.clone(),
            )
        } else {
            OutboundEnvelope::new(return_address, self.cancellation_token.clone())
        }
    }

    /// Returns a clone of the internal map containing handles to the agent's direct children.
    ///
    /// Provides a snapshot of the currently supervised children. Modifications to the
    /// returned map will not affect the agent's actual children list.
    #[inline]
    fn children(&self) -> DashMap<String, AgentHandle> {
        self.children.clone()
    }

    /// Searches for a direct child agent by its unique identifier (`Ern`).
    ///
    /// # Arguments
    ///
    /// * `ern`: The [`Ern`] of the child agent to find.
    ///
    /// # Returns
    ///
    /// * `Some(AgentHandle)`: If a direct child with the matching `Ern` is found.
    /// * `None`: If no direct child with the specified `Ern` exists.
    #[instrument(skip(self))]
    fn find_child(&self, ern: &Ern) -> Option<Self> {
        trace!("Searching for child with ERN: {}", ern);
        // Access the DashMap using the ERN's string representation as the key.
        self.children.get(&ern.to_string()).map(
            |entry| entry.value().clone(), // Clone the handle if found
        )
    }

    /// Returns a clone of the agent's task tracker.
    ///
    /// The tracker can be used to monitor the agent's main task.
    #[inline]
    fn tracker(&self) -> TaskTracker {
        self.tracker.clone()
    }

    /// Returns a clone of the agent's unique Entity Resource Name (`Ern`).
    #[inline]
    fn id(&self) -> Ern {
        self.id.clone()
    }

    /// Returns the agent's root name (the first part of its `Ern`) as a String.
    #[inline]
    fn name(&self) -> String {
        self.id.root.to_string()
    }

    /// Returns a clone of this `AgentHandle`.
    #[inline]
    fn clone_ref(&self) -> AgentHandle {
        self.clone()
    }

    /// Sends a [`SystemSignal::Terminate`] message to the agent and waits for its task to complete.
    ///
    /// This initiates a graceful shutdown of the agent. It sends the `Terminate` signal
    /// to the agent's inbox and then waits on the agent's `TaskTracker` until the main
    /// task (and potentially associated tasks) have finished execution.
    ///
    /// The agent's `wake` loop is responsible for handling the `Terminate` signal,
    /// potentially running `before_stop` and `after_stop` hooks, and stopping child agents.
    ///
    /// # Returns
    ///
    /// An `anyhow::Result<()>` indicating success or failure. Failure typically occurs
    /// if sending the `Terminate` signal to the agent's inbox fails (e.g., if the channel
    /// is already closed).
    #[allow(clippy::manual_async_fn)] // Keep async_trait style
    #[instrument(skip(self))]
    fn stop(&self) -> impl Future<Output = anyhow::Result<()>> + Send + Sync + '_ {
        async move {
            let tracker = self.tracker();

            // Create an envelope to send the signal from self to self.
            let self_envelope = self.create_envelope(Some(self.reply_address()));

            trace!(actor = %self.id, "Sending Terminate signal");
            // Send the Terminate signal to initiate graceful shutdown.
            self_envelope.send(SystemSignal::Terminate).await;

            // Wait for the agent's main task and any tracked tasks to finish.
            tracker.wait().await;

            trace!(actor = %self.id, "Agent terminated successfully.");
            Ok(())
        }
    }

    /// Sends a boxed message to the agent.
    ///
    /// This method accepts a boxed trait object, converts it to an Arc, and sends it
    /// to the agent's inbox. This is primarily used for IPC scenarios where messages
    /// are deserialized into trait objects at runtime.
    ///
    /// # Arguments
    ///
    /// * `message`: A boxed message implementing `ActonMessage + Send + Sync`.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the message was successfully queued for delivery, or an error
    /// if the send failed (e.g., channel closed).
    #[cfg(feature = "ipc")]
    #[instrument(skip(self, message))]
    fn send_boxed(
        &self,
        message: Box<dyn ActonMessage + Send + Sync>,
    ) -> impl Future<Output = anyhow::Result<()>> + Send + Sync + '_ {
        use std::sync::Arc;
        async move {
            let envelope = self.create_envelope(Some(self.reply_address()));
            trace!(recipient = %self.id, "Sending boxed message via IPC");
            // Convert Box to Arc for the internal send mechanism
            let arc_message: Arc<dyn ActonMessage + Send + Sync> = Arc::from(message);
            envelope.send_arc(arc_message).await;
            Ok(())
        }
    }

    /// Sends a boxed message to the agent with a custom reply-to address.
    ///
    /// This method is used for IPC request-response patterns where responses
    /// should be routed back to a temporary IPC proxy channel rather than
    /// another agent.
    ///
    /// When the target agent calls `reply_envelope.send(response)`, the response
    /// will be delivered to the specified `reply_to` address.
    ///
    /// # Arguments
    ///
    /// * `message`: A boxed message implementing `ActonMessage + Send + Sync`.
    /// * `reply_to`: The [`MessageAddress`] where responses should be sent.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the message was successfully queued for delivery, or an error
    /// if the send failed (e.g., channel closed).
    #[cfg(feature = "ipc")]
    #[instrument(skip(self, message, reply_to))]
    fn send_boxed_with_reply_to(
        &self,
        message: Box<dyn ActonMessage + Send + Sync>,
        reply_to: MessageAddress,
    ) -> impl Future<Output = anyhow::Result<()>> + Send + Sync + '_ {
        use std::sync::Arc;
        async move {
            // Create envelope with the custom reply-to address as sender
            // and self (target agent) as recipient
            let envelope = OutboundEnvelope::new_with_recipient(
                reply_to,                  // The reply-to address (IPC proxy)
                self.reply_address(),       // The recipient (target agent)
                self.cancellation_token.clone(),
            );
            trace!(
                recipient = %self.id,
                reply_to = ?envelope.return_address.sender.root.as_str(),
                "Sending boxed message with custom reply-to via IPC"
            );
            // Convert Box to Arc for the internal send mechanism
            let arc_message: Arc<dyn ActonMessage + Send + Sync> = Arc::from(message);
            envelope.send_arc(arc_message).await;
            Ok(())
        }
    }

    /// Tries to send a boxed message without blocking (backpressure-aware).
    ///
    /// This method attempts to send a message but returns immediately with an error
    /// if the target agent's inbox is full.
    ///
    /// # Arguments
    ///
    /// * `message`: A boxed message implementing `ActonMessage + Send + Sync`.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the message was successfully queued, or an `IpcError` if
    /// the agent's inbox is full (`TargetBusy`) or the channel is closed.
    #[cfg(feature = "ipc")]
    #[instrument(skip(self, message))]
    fn try_send_boxed(
        &self,
        message: Box<dyn ActonMessage + Send + Sync>,
    ) -> Result<(), crate::common::ipc::IpcError> {
        use std::sync::Arc;
        let envelope = self.create_envelope(Some(self.reply_address()));
        trace!(recipient = %self.id, "Trying to send boxed message via IPC (backpressure-aware)");
        // Convert Box to Arc for the internal send mechanism
        let arc_message: Arc<dyn ActonMessage + Send + Sync> = Arc::from(message);
        envelope.try_send_arc(arc_message)
    }

    /// Tries to send a boxed message with a custom reply-to address without blocking.
    ///
    /// This method is the backpressure-aware variant of `send_boxed_with_reply_to`.
    /// It returns immediately with an error if the target agent's inbox is full.
    ///
    /// # Arguments
    ///
    /// * `message`: A boxed message implementing `ActonMessage + Send + Sync`.
    /// * `reply_to`: The [`MessageAddress`] where responses should be sent.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the message was successfully queued, or an `IpcError` if
    /// the agent's inbox is full (`TargetBusy`) or the channel is closed.
    #[cfg(feature = "ipc")]
    #[instrument(skip(self, message, reply_to))]
    fn try_send_boxed_with_reply_to(
        &self,
        message: Box<dyn ActonMessage + Send + Sync>,
        reply_to: MessageAddress,
    ) -> Result<(), crate::common::ipc::IpcError> {
        use std::sync::Arc;
        // Create envelope with the custom reply-to address as sender
        // and self (target agent) as recipient
        let envelope = OutboundEnvelope::new_with_recipient(
            reply_to,                  // The reply-to address (IPC proxy)
            self.reply_address(),       // The recipient (target agent)
            self.cancellation_token.clone(),
        );
        trace!(
            recipient = %self.id,
            reply_to = ?envelope.return_address.sender.root.as_str(),
            "Trying to send boxed message with custom reply-to via IPC (backpressure-aware)"
        );
        // Convert Box to Arc for the internal send mechanism
        let arc_message: Arc<dyn ActonMessage + Send + Sync> = Arc::from(message);
        envelope.try_send_arc(arc_message)
    }
}
