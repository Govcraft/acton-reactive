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
use crate::traits::request::Request;

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

    /// Returns a reference to the map of children supervised **through this
    /// handle**.
    ///
    /// This is a local view, not the supervisor's roster. `ActorHandle` holds
    /// its children in a `DashMap` that is deep-copied on clone, so each clone
    /// accumulates only what was supervised through it. A child adopted through
    /// a different clone of the same actor's handle will not appear here, and
    /// neither will one adopted from inside the actor's own message handler.
    ///
    /// The handles stored here name one incarnation. If a child is restarted,
    /// the handle kept here goes stale. Use
    /// [`SupervisedChild`](crate::actor::SupervisedChild) for a reference that
    /// follows restarts.
    ///
    /// Use `.len()`, `.iter()`, `.get()`, or `.contains_key()` to query without
    /// cloning the map.
    fn children(&self) -> &DashMap<String, ActorHandle>;

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

    /// Sends a request and waits for the actor's reply.
    ///
    /// This is the counterpart to [`send`](Self::send), which is fire-and-forget and
    /// tells you nothing about whether the message was processed. `ask` resolves only
    /// once the actor has answered, so it establishes the happens-before that
    /// `send`-then-continue does not:
    ///
    /// ```ignore
    /// let count = handle.ask(GetCount).await?;
    /// ```
    ///
    /// Reach for it wherever code would otherwise sleep and hope. Because an actor
    /// processes its inbox in order, a completed `ask` also proves every message you
    /// sent that actor beforehand has been processed.
    ///
    /// The message must implement [`Request`], which names the reply type. The handler
    /// answers through its reply envelope exactly as it always has; `ask` places no new
    /// obligation on the handler side, and an actor cannot tell an `ask` from a `send`.
    ///
    /// # Deadlock
    ///
    /// **Do not `ask` from inside a mutable (`mutate_on`) handler.** Mutable handlers
    /// are awaited inline on the actor's message loop, so a handler that waits for a
    /// reply stops its own actor from processing anything — including the very message
    /// that would produce the reply. Asking your *own* handle this way can never
    /// succeed, and asking another actor deadlocks as soon as the two wait on each
    /// other.
    ///
    /// This mirrors the restriction on
    /// [`supervise_with`](crate::common::ActorHandle::supervise_with), and the ways out
    /// are the same shape:
    ///
    /// * Send instead of asking, and let the reply arrive as an ordinary message — the
    ///   handler is already given a reply envelope for exactly this.
    /// * Move the exchange off the message loop: clone the handle into a
    ///   `Reply::pending(async move { .. })` future, or a spawned task, so the loop is
    ///   free to keep processing.
    /// * Ask from outside the actor — from a task, a test, or `main`.
    ///
    /// A caller that deadlocks anyway is released by the deadline rather than stuck
    /// forever, and gets [`AskError::TimedOut`] to say so.
    ///
    /// # How this resolves when no reply comes
    ///
    /// `ask` always finishes. Two mechanisms cover different cases:
    ///
    /// * It holds no reply address of its own while waiting, so the moment the actor
    ///   lets go of the request — returning without replying, stopping, panicking, or
    ///   being restarted — the reply channel closes and the call returns
    ///   [`AskError::NoReply`], in microseconds and naming the situation.
    /// * [`DEFAULT_ASK_TIMEOUT`] backstops the cases that closure cannot see, where the
    ///   reply address is still alive but no answer is coming: a wedged actor, a
    ///   forgotten reply envelope, a self-inflicted deadlock.
    ///
    /// Use [`ask_with_timeout`](Self::ask_with_timeout) for a different bound.
    ///
    /// # Scope
    ///
    /// **One actor, in-process.**
    ///
    /// * *Not the broker.* [`broadcast`](crate::traits::Broadcaster::broadcast) has no
    ///   single replier — zero or many subscribers may answer — so request/reply has no
    ///   meaning over it. `ask` addresses the actor this handle names, and nothing else.
    /// * *Not across a process boundary yet.* The reply address is an in-process
    ///   channel, so `ask` cannot presently reach an actor over IPC. Typed request/reply
    ///   for IPC is being added separately, on top of the correlated transport
    ///   `IpcClient::request` already provides; until it lands, use that directly.
    ///
    /// # Errors
    ///
    /// * [`AskError::Undeliverable`] — the actor's inbox was already closed.
    /// * [`AskError::Cancelled`] — delivery was abandoned during shutdown.
    /// * [`AskError::NoReply`] — delivered, but no reply will arrive.
    /// * [`AskError::TimedOut`] — the deadline expired with the reply outstanding.
    /// * [`AskError::UnexpectedReply`] — the handler replied with a different type.
    ///
    /// [`Request`]: crate::traits::Request
    /// [`AskError`]: crate::common::AskError
    /// [`AskError::Undeliverable`]: crate::common::AskError::Undeliverable
    /// [`AskError::Cancelled`]: crate::common::AskError::Cancelled
    /// [`AskError::NoReply`]: crate::common::AskError::NoReply
    /// [`AskError::TimedOut`]: crate::common::AskError::TimedOut
    /// [`AskError::UnexpectedReply`]: crate::common::AskError::UnexpectedReply
    /// [`DEFAULT_ASK_TIMEOUT`]: crate::common::DEFAULT_ASK_TIMEOUT
    fn ask<R: Request>(
        &self,
        request: R,
    ) -> impl Future<Output = Result<R::Response, crate::common::AskError>> + Send + '_ {
        self.ask_with_timeout(request, crate::common::DEFAULT_ASK_TIMEOUT)
    }

    /// Sends a request and waits for the reply, giving up after `timeout`.
    ///
    /// [`ask`](Self::ask) with an explicit deadline instead of
    /// [`DEFAULT_ASK_TIMEOUT`](crate::common::DEFAULT_ASK_TIMEOUT). Pairs with `ask` the
    /// way `IpcClient::request_with_timeout` pairs with `IpcClient::request`, so local
    /// and remote requests are bounded the same way.
    ///
    /// The deadline covers the whole exchange, delivery included: a full inbox makes
    /// delivery itself wait, and a caller asking for a bound wants it on the operation
    /// rather than on one phase of it.
    ///
    /// Every caveat on [`ask`](Self::ask) applies unchanged — in particular the
    /// `# Deadlock` section, which a shorter deadline turns from a hang into a prompt
    /// [`AskError::TimedOut`](crate::common::AskError::TimedOut) but does not fix.
    ///
    /// # Errors
    ///
    /// As [`ask`](Self::ask).
    #[instrument(skip(self, request), fields(request_type = std::any::type_name::<R>()))]
    fn ask_with_timeout<R: Request>(
        &self,
        request: R,
        timeout: std::time::Duration,
    ) -> impl Future<Output = Result<R::Response, crate::common::AskError>> + Send + '_ {
        // The envelope is built only to borrow the cancellation token that governs
        // sends to this actor; the request itself travels in an envelope whose return
        // address is the private reply channel, not this actor.
        let cancellation_token = self.create_envelope(None).cancellation_token;
        let recipient = self.reply_address();
        trace!(recipient = %self.id(), "Asking actor and awaiting its reply");
        crate::common::ask::send_request(recipient, cancellation_token, request, timeout)
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
    /// This method should send a termination signal (e.g.,
    /// [`SystemSignal::Terminate`](crate::message::SystemSignal::Terminate))
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
    /// Returns [`IpcError::TargetBusy`](crate::ipc::IpcError::TargetBusy) if the actor's
    /// inbox is full, or [`IpcError::IoError`](crate::ipc::IpcError::IoError) if the
    /// channel is closed.
    #[cfg(feature = "ipc")]
    fn try_send_boxed(
        &self,
        message: Box<dyn ActonMessage + Send + Sync>,
    ) -> Result<(), crate::common::ipc::IpcError>;

    /// Tries to send a boxed message with a custom reply-to address without blocking.
    ///
    /// This method is the backpressure-aware variant of
    /// [`send_boxed_with_reply_to`](ActorHandleInterface::send_boxed_with_reply_to).
    /// It returns immediately with an error if the target actor's inbox is full.
    ///
    /// # Arguments
    ///
    /// * `message`: A boxed message payload to send.
    /// * `reply_to`: The [`MessageAddress`] where responses should be sent.
    ///
    /// # Errors
    ///
    /// Returns [`IpcError::TargetBusy`](crate::ipc::IpcError::TargetBusy) if the actor's
    /// inbox is full, or [`IpcError::IoError`](crate::ipc::IpcError::IoError) if the
    /// channel is closed.
    #[cfg(feature = "ipc")]
    fn try_send_boxed_with_reply_to(
        &self,
        message: Box<dyn ActonMessage + Send + Sync>,
        reply_to: MessageAddress,
    ) -> Result<(), crate::common::ipc::IpcError>;
}
