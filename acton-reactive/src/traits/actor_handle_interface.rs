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

use std::fmt::Debug; // Import Debug
use std::future::Future;

use acton_ern::Ern;
use async_trait::async_trait;
use dashmap::DashMap;
use tokio_util::task::TaskTracker;
use tracing::{instrument, trace}; // Removed error, warn as they weren't used in defaults

use crate::common::{ActorHandle, OutboundEnvelope}; // Keep wildcard import if necessary, or specify types
use crate::message::{BrokerRequest, MessageAddress}; // BrokerRequest used in send_sync default
use crate::traits::acton_message::ActonMessage;

/// Defines the core asynchronous interface for interacting with an actor via its handle.
///
/// This trait specifies the fundamental operations that can be performed on an actor's handle,
/// such as sending messages, managing its lifecycle, accessing identity information,
/// and navigating the supervision hierarchy. It is typically implemented by [`ActorHandle`].
///
/// Implementors of this trait provide the concrete mechanisms for these operations.
#[async_trait]
pub trait ActorHandleInterface: Send + Sync + Debug + Clone + 'static {
    // Added bounds
    /// Returns the [`MessageAddress`] associated with this actor handle.
    ///
    /// This address contains the actor's unique ID (`Ern`) and the sender channel
    /// connected to its inbox, allowing others to send messages directly to it or
    /// use it as a return address.
    fn reply_address(&self) -> MessageAddress;

    /// Creates an [`OutboundEnvelope`] suitable for sending a message from this actor.
    ///
    /// The envelope's `return_address` is set to this actor's address.
    ///
    /// # Arguments
    ///
    /// * `recipient_address`: An optional [`MessageAddress`] for the intended recipient.
    ///   If `None`, the envelope is created without a specific recipient.
    fn create_envelope(&self, recipient_address: Option<MessageAddress>) -> OutboundEnvelope;

    /// Returns a clone of the map containing handles to the actor's direct children.
    ///
    /// Provides a snapshot of the currently supervised children. Modifications to the
    /// returned map do not affect the actor's actual children list.
    fn children(&self) -> DashMap<String, ActorHandle>;

    /// Attempts to find a direct child actor supervised by this actor, identified by its `Ern`.
    ///
    /// # Arguments
    ///
    /// * `id`: The unique [`Ern`] of the child actor to locate.
    ///
    /// # Returns
    ///
    /// * `Some(ActorHandle)`: If a direct child with the matching `Ern` is found.
    /// * `None`: If no direct child with the specified `Ern` exists.
    fn find_child(&self, id: &Ern) -> Option<ActorHandle>;

    /// Returns a clone of the actor's [`TaskTracker`].
    ///
    /// The tracker can be used to monitor the actor's main task and potentially
    /// other associated asynchronous operations.
    fn tracker(&self) -> TaskTracker;

    /// Returns a clone of the actor's unique identifier ([`Ern`]).
    fn id(&self) -> Ern;

    /// Returns the actor's root name (the first segment of its [`Ern`]) as a `String`.
    fn name(&self) -> String;

    /// Creates and returns a clone of this actor handle.
    fn clone_ref(&self) -> ActorHandle; // Consider renaming to `clone_handle` or just relying on `Clone`

    /// Sends a message asynchronously to this actor handle's associated actor.
    ///
    /// This default implementation creates an envelope with no specific recipient
    /// (implying the message is sent to the actor represented by `self`) and uses
    /// the envelope's `send` method.
    ///
    /// # Arguments
    ///
    /// * `message`: The message payload to send. Must implement [`ActonMessage`].
    #[instrument(skip(self, message), fields(message_type = std::any::type_name_of_val(&message)))]
    fn send(&self, message: impl ActonMessage) -> impl Future<Output = ()> + Send + Sync + '_ {
        async move {
            // Creates an envelope targeting self.
            let envelope = self.create_envelope(Some(self.reply_address()));
            trace!(sender = %self.id(), recipient = %self.id(), "Default send implementation");
            envelope.send(message).await;
        }
    }

    /// Sends a message synchronously to a specified recipient actor.
    ///
    /// **Warning:** This default implementation uses [`OutboundEnvelope::reply`], which internally
    /// spawns a blocking task and creates a new Tokio runtime. This is generally **discouraged**
    /// and can lead to performance issues or deadlocks, especially if called from within an
    /// existing asynchronous context. Prefer using asynchronous methods like [`ActorHandleInterface::send`]
    /// or [`OutboundEnvelope::send`] where possible.
    ///
    /// # Arguments
    ///
    /// * `message`: The message payload to send. Must implement [`ActonMessage`].
    /// * `recipient`: A reference to the [`ActorHandle`] of the recipient actor.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or failure. Currently, it relies on the behavior of
    /// [`OutboundEnvelope::reply`], which might not propagate all underlying errors.
    fn send_sync(&self, message: impl ActonMessage, recipient: &ActorHandle) -> anyhow::Result<()>
    where
        Self: Sized, // Required for calling create_envelope on self
    {
        trace!(sender = %self.id(), recipient = %recipient.id(), "Sending message synchronously");
        let envelope = self.create_envelope(Some(recipient.reply_address()));
        envelope.reply(BrokerRequest::new(message))?; // Uses the potentially problematic OutboundEnvelope::reply
        Ok(())
    }

    /// Initiates a graceful shutdown of the actor associated with this handle.
    ///
    /// This method should send a termination signal (e.g., [`SystemSignal::Terminate`])
    /// to the actor and wait for its main task and associated tasks (tracked by `tracker`)
    /// to complete.
    ///
    /// # Returns
    ///
    /// A `Future` that resolves to `Ok(())` upon successful termination, or an `Err`
    /// if sending the termination signal or waiting for completion fails.
    fn stop(&self) -> impl Future<Output = anyhow::Result<()>> + Send + Sync + '_;

    /// Sends a boxed message asynchronously to this actor handle's associated actor.
    ///
    /// This method is similar to [`send`](ActorHandleInterface::send), but accepts a
    /// boxed trait object instead of a generic message type. This is useful for IPC
    /// scenarios where messages are deserialized into trait objects at runtime.
    ///
    /// # Arguments
    ///
    /// * `message`: A boxed message payload to send. Must implement [`ActonMessage`].
    ///
    /// # Errors
    ///
    /// Returns an error if the message could not be sent (e.g., if the channel is closed).
    #[cfg(feature = "ipc")]
    fn send_boxed(
        &self,
        message: Box<dyn ActonMessage + Send + Sync>,
    ) -> impl Future<Output = anyhow::Result<()>> + Send + Sync + '_;

    /// Sends a boxed message asynchronously with a custom reply-to address.
    ///
    /// This method is used for IPC request-response patterns where responses
    /// should be routed back to a temporary IPC proxy channel rather than
    /// another actor. When the target actor calls `reply_envelope.send(response)`,
    /// the response will be delivered to the specified `reply_to` address.
    ///
    /// # Arguments
    ///
    /// * `message`: A boxed message payload to send. Must implement [`ActonMessage`].
    /// * `reply_to`: The [`MessageAddress`] where responses should be sent.
    ///
    /// # Errors
    ///
    /// Returns an error if the message could not be sent (e.g., if the channel is closed).
    #[cfg(feature = "ipc")]
    fn send_boxed_with_reply_to(
        &self,
        message: Box<dyn ActonMessage + Send + Sync>,
        reply_to: MessageAddress,
    ) -> impl Future<Output = anyhow::Result<()>> + Send + Sync + '_;

    /// Tries to send a boxed message without blocking (backpressure-aware).
    ///
    /// This method attempts to send a message but returns immediately with an error
    /// if the target actor's inbox is full. This is useful for IPC scenarios where
    /// backpressure feedback is needed rather than blocking.
    ///
    /// # Arguments
    ///
    /// * `message`: A boxed message payload to send.
    ///
    /// # Errors
    ///
    /// Returns [`IpcError::TargetBusy`] if the actor's inbox is full, or
    /// [`IpcError::IoError`] if the channel is closed.
    #[cfg(feature = "ipc")]
    fn try_send_boxed(
        &self,
        message: Box<dyn ActonMessage + Send + Sync>,
    ) -> Result<(), crate::common::ipc::IpcError>;

    /// Tries to send a boxed message with a custom reply-to address without blocking.
    ///
    /// This method is the backpressure-aware variant of [`send_boxed_with_reply_to`].
    /// It returns immediately with an error if the target actor's inbox is full.
    ///
    /// # Arguments
    ///
    /// * `message`: A boxed message payload to send.
    /// * `reply_to`: The [`MessageAddress`] where responses should be sent.
    ///
    /// # Errors
    ///
    /// Returns [`IpcError::TargetBusy`] if the actor's inbox is full, or
    /// [`IpcError::IoError`] if the channel is closed.
    #[cfg(feature = "ipc")]
    fn try_send_boxed_with_reply_to(
        &self,
        message: Box<dyn ActonMessage + Send + Sync>,
        reply_to: MessageAddress,
    ) -> Result<(), crate::common::ipc::IpcError>;
}
