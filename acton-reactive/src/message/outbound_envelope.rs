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

use std::cmp::PartialEq;
use std::fmt::Debug; // Import Debug
use std::hash::{Hash, Hasher}; // Import Hash and Hasher
use std::sync::{Arc, OnceLock};

use tokio::runtime::{Handle, Runtime};
use tracing::{debug, error, instrument, trace, warn};

/// Shared runtime for synchronous `reply()` calls made outside of a Tokio context.
///
/// This runtime is created lazily on first use and persists for the process lifetime.
/// Using a shared runtime avoids the expensive overhead of creating a new runtime
/// (and associated thread pools) for each `reply()` call from non-async code.
///
/// The runtime uses the multi-threaded flavor with a single worker thread. This allows
/// `spawn()` to work without blocking the caller (preserving fire-and-forget semantics),
/// while keeping resource usage minimal since reply operations are lightweight channel sends.
static SYNC_REPLY_RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Gets or creates the shared fallback runtime for synchronous `reply()` calls.
///
/// This function is called when `reply()` is invoked outside of any Tokio runtime
/// context. Rather than creating a new runtime per call (expensive), we lazily
/// initialize a single shared runtime that handles all sync reply operations.
fn sync_reply_runtime() -> &'static Runtime {
    SYNC_REPLY_RUNTIME.get_or_init(|| {
        debug!("Creating shared fallback runtime for sync reply() calls");
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("acton-sync-reply")
            .build()
            .expect("Failed to create fallback Tokio runtime for sync reply()")
    })
}

use crate::common::{Envelope, MessageError};
use crate::message::message_address::MessageAddress;
use crate::traits::ActonMessage;

/// Represents a message prepared for sending, including sender and optional recipient addresses.
///
/// An `OutboundEnvelope` is typically created by an actor (using methods like
/// [`ActorHandle::create_envelope`](crate::common::ActorHandle::create_envelope))
/// before sending a message. It holds the [`MessageAddress`] of the sender (`return_address`)
/// and optionally the [`MessageAddress`] of the recipient (`recipient_address`).
///
/// The primary methods for dispatching the message are [`OutboundEnvelope::send`] (asynchronous)
/// and [`OutboundEnvelope::reply`] (synchronous wrapper).
///
/// Equality and hashing are based solely on the `return_address`.
#[derive(Clone, Debug)]
pub struct OutboundEnvelope {
    /// The address of the actor sending the message.
    pub(crate) return_address: MessageAddress,
    /// The address of the intended recipient actor, if specified directly.
    /// If `None`, the recipient might be implied (e.g., sending back to `return_address`).
    pub(crate) recipient_address: Option<MessageAddress>,
    /// The cancellation token for the sending actor.
    pub(crate) cancellation_token: tokio_util::sync::CancellationToken,
}

// Note: The PartialEq impl for MessageAddress is defined here, but ideally should be
// in message_address.rs if it's generally applicable. Assuming it's needed here for now.
/// Implements equality comparison for `MessageAddress` based on the sender's `Ern`.
impl PartialEq for MessageAddress {
    fn eq(&self, other: &Self) -> bool {
        self.sender == other.sender // Compare based on Ern
    }
}

/// Implements equality comparison for `OutboundEnvelope` based on the `return_address`.
impl PartialEq for OutboundEnvelope {
    fn eq(&self, other: &Self) -> bool {
        self.return_address == other.return_address
    }
}

/// Derives `Eq` based on the `PartialEq` implementation.
impl Eq for OutboundEnvelope {}

/// Implements hashing for `OutboundEnvelope` based on the `return_address`.
impl Hash for OutboundEnvelope {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Hash only based on the return address's sender Ern, consistent with PartialEq.
        self.return_address.sender.hash(state);
    }
}

impl OutboundEnvelope {
    /// Creates a new `OutboundEnvelope` with only a return address specified.
    ///
    /// The recipient address is initially set to `None`. Use [`OutboundEnvelope::send`]
    /// or [`OutboundEnvelope::reply`] to send the message, typically back to the
    /// `return_address` if no recipient is set later (though `send_message_inner` logic defaults to `return_address` if `recipient_address` is `None`).
    ///
    /// # Arguments
    ///
    /// * `return_address`: The [`MessageAddress`] of the actor creating this envelope (the sender).
    ///
    /// # Returns
    ///
    /// A new `OutboundEnvelope` instance.
    #[instrument(skip(return_address))]
    pub fn new(
        return_address: MessageAddress,
        cancellation_token: tokio_util::sync::CancellationToken,
    ) -> Self {
        trace!(sender = %return_address.sender, "Creating new OutboundEnvelope");
        Self {
            return_address,
            recipient_address: None,
            cancellation_token,
        }
    }

    /// Returns a clone of the sender's [`MessageAddress`].
    #[inline]
    #[must_use]
    pub fn reply_to(&self) -> MessageAddress {
        self.return_address.clone()
    }

    /// Returns a reference to the optional recipient's [`MessageAddress`].
    #[inline]
    #[must_use]
    pub const fn recipient(&self) -> &Option<MessageAddress> {
        &self.recipient_address
    }

    /// Crate-internal constructor: Creates a new `OutboundEnvelope` with specified sender and recipient.
    #[instrument(skip(return_address, recipient_address))]
    pub(crate) fn new_with_recipient(
        return_address: MessageAddress,
        recipient_address: MessageAddress,
        cancellation_token: tokio_util::sync::CancellationToken,
    ) -> Self {
        trace!(sender = %return_address.sender, recipient = %recipient_address.sender, "Creating new OutboundEnvelope with recipient");
        Self {
            return_address,
            recipient_address: Some(recipient_address),
            cancellation_token,
        }
    }

    /// Sends a message using this envelope synchronously.
    ///
    /// This method attempts to send a message without requiring an async context.
    /// It uses the following strategy:
    ///
    /// 1. If called from within a Tokio runtime context, it spawns the send operation
    ///    on the existing runtime (most efficient).
    /// 2. If called from outside any Tokio context, it creates a minimal runtime
    ///    to execute the send (fallback for non-async code paths).
    ///
    /// **Recommendation:** Prefer using the asynchronous [`OutboundEnvelope::send`] method
    /// whenever possible, as it integrates better with async workflows.
    ///
    /// # Arguments
    ///
    /// * `message`: The message payload to send. Must implement [`ActonMessage`] and be `'static`.
    ///
    /// # Returns
    ///
    /// * `Ok(())`: If the message was successfully scheduled to be sent (actual delivery depends on the recipient).
    /// * `Err(MessageError)`: Currently, this implementation always returns `Ok(())`, but the signature
    ///   allows for future error handling. Potential errors (like closed channels) are logged internally.
    #[instrument(skip(self, message), fields(message_type = std::any::type_name_of_val(&message)))]
    pub fn reply(&self, message: impl ActonMessage + 'static) -> Result<(), MessageError> {
        let envelope = self.clone();
        let message_arc = Arc::new(message);

        // Try to use the existing runtime if we're already in a Tokio context.
        // This avoids the overhead of creating a new runtime per call.
        if let Ok(handle) = Handle::try_current() {
            // We're inside a Tokio runtime - spawn on the existing runtime
            trace!(
                sender = %envelope.return_address.sender,
                recipient = ?envelope.recipient_address.as_ref().map(|r| r.sender.to_string()),
                "Replying via existing runtime handle"
            );
            // Spawn a boxed future to reduce stack usage from large tokio::select! in send_message_inner
            Self::spawn_reply_task(&handle, envelope, message_arc);
        } else {
            // We're outside any Tokio context - use the shared fallback runtime.
            warn!(
                sender = %envelope.return_address.sender,
                "reply() called outside Tokio context; using shared fallback runtime"
            );
            Self::spawn_reply_on_fallback(envelope, message_arc);
        }
        Ok(())
    }

    /// Helper to spawn reply task on existing runtime, using boxed future.
    fn spawn_reply_task(
        handle: &Handle,
        envelope: Self,
        message: Arc<dyn ActonMessage + Send + Sync>,
    ) {
        handle.spawn(Box::pin(async move {
            envelope.send_message_inner(message).await;
        }));
    }

    /// Spawns a reply task on the shared fallback runtime.
    ///
    /// This is much more efficient than the previous approach of creating a new
    /// `std::thread` and `Runtime` per call. The shared runtime amortizes the
    /// cost across all sync `reply()` calls.
    fn spawn_reply_on_fallback(envelope: Self, message: Arc<dyn ActonMessage + Send + Sync>) {
        sync_reply_runtime().spawn(async move {
            envelope.send_message_inner(message).await;
        });
    }

    /// Crate-internal: Asynchronously sends the message payload to the recipient.
    /// Handles channel reservation and error logging.
    ///
    /// # Performance
    ///
    /// This method uses a fast-path optimization: it first attempts `try_reserve()` which
    /// is non-blocking and avoids async overhead when the channel has capacity (common case).
    /// Only when the channel is full does it fall back to the async `reserve()` path.
    ///
    /// Clones are minimized on the fast path: `sender`/`recipient` identifiers are borrowed
    /// from the address structs for logging rather than cloned upfront.
    async fn send_message_inner(&self, message: Arc<dyn ActonMessage + Send + Sync>) {
        let target_address = self
            .recipient_address
            .as_ref()
            .unwrap_or(&self.return_address)
            .clone();
        let return_address = self.return_address.clone();
        let channel_sender = target_address.address.clone();

        // Check if cancelled before attempting send
        if self.cancellation_token.is_cancelled() {
            error!(sender = %return_address.sender, recipient = %target_address.sender, "Send aborted: cancellation_token triggered");
            return;
        }

        // Fast path: try non-blocking reserve first (common case when channel has capacity)
        match channel_sender.try_reserve() {
            Ok(permit) => {
                permit.send(Envelope::new(message, return_address, target_address));
                return;
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(())) => {
                error!(sender = %return_address.sender, recipient = %target_address.sender, "Recipient channel is closed");
                return;
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(())) => {
                // Channel is full, fall through to slow path
            }
        }

        // Slow path: channel is full, need to wait for capacity
        match channel_sender.reserve().await {
            Ok(permit) => {
                permit.send(Envelope::new(message, return_address, target_address));
            }
            Err(e) => {
                error!(sender = %return_address.sender, recipient = %target_address.sender, error = %e, "Failed to reserve channel capacity");
            }
        };
    }

    /// Sends a message asynchronously using this envelope.
    ///
    /// This method takes the message payload, wraps it in an `Arc`, and calls the
    /// internal `send_message_inner` to dispatch it to the recipient's channel.
    /// The recipient is determined by `recipient_address` if `Some`, otherwise it
    /// defaults to `return_address`.
    ///
    /// This is the preferred method for sending messages from within an asynchronous context.
    /// For fire-and-forget scenarios where errors can be ignored, this method logs errors
    /// internally. For explicit error handling, use [`try_send`](OutboundEnvelope::try_send).
    ///
    /// # Arguments
    ///
    /// * `message`: The message payload to send. Must implement [`ActonMessage`] and be `'static`.
    #[instrument(skip(self, message), level = "trace", fields(message_type = std::any::type_name_of_val(&message)))]
    pub async fn send(&self, message: impl ActonMessage + 'static) {
        // Arc the message and call the internal async sender.
        self.send_message_inner(Arc::new(message)).await;
    }

    /// Sends a message asynchronously with explicit error handling.
    ///
    /// This method is similar to [`send`](OutboundEnvelope::send), but returns a `Result`
    /// indicating whether the message was successfully delivered to the recipient's channel.
    /// Use this when you need to handle delivery failures explicitly rather than relying
    /// on internal logging.
    ///
    /// # Arguments
    ///
    /// * `message`: The message payload to send. Must implement [`ActonMessage`] and be `'static`.
    ///
    /// # Returns
    ///
    /// * `Ok(())` - The message was successfully queued in the recipient's channel
    /// * `Err(MessageError)` - The message could not be delivered
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The recipient's channel is closed (`MessageError::ChannelClosed`)
    /// - The operation was cancelled (`MessageError::Cancelled`)
    /// - The channel capacity could not be reserved (`MessageError::SendFailed`)
    ///
    /// # Performance
    ///
    /// Uses a fast-path with `try_reserve()` for the common case when the channel has capacity.
    #[instrument(skip(self, message), level = "trace", fields(message_type = std::any::type_name_of_val(&message)))]
    pub async fn try_send(&self, message: impl ActonMessage + 'static) -> Result<(), MessageError> {
        let message = Arc::new(message);

        // Determine the target address: recipient if Some, otherwise return_address.
        let target_address = self
            .recipient_address
            .as_ref()
            .unwrap_or(&self.return_address);

        // Check cancellation synchronously first
        if self.cancellation_token.is_cancelled() {
            return Err(MessageError::Cancelled);
        }

        let channel_sender = &target_address.address;

        // Fast path: try non-blocking reserve first
        match channel_sender.try_reserve() {
            Ok(permit) => {
                let internal_envelope =
                    Envelope::new(message, self.return_address.clone(), target_address.clone());
                permit.send(internal_envelope);
                return Ok(());
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(())) => {
                return Err(MessageError::ChannelClosed);
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(())) => {
                // Fall through to slow path
            }
        }

        // Slow path: channel is full, need to wait for capacity
        let channel_sender = channel_sender.clone();
        let cancellation = self.cancellation_token.clone();
        let return_addr = self.return_address.clone();
        let target_addr = target_address.clone();

        Box::pin(async move {
            tokio::select! {
                () = cancellation.cancelled() => {
                    Err(MessageError::Cancelled)
                }
                permit_result = channel_sender.reserve() => {
                    match permit_result {
                        Ok(permit) => {
                            let internal_envelope = Envelope::new(message, return_addr, target_addr);
                            permit.send(internal_envelope);
                            Ok(())
                        }
                        Err(e) => {
                            Err(MessageError::SendFailed(e.to_string()))
                        }
                    }
                }
            }
        })
        .await
    }

    /// Sends an Arc-wrapped message asynchronously using this envelope.
    ///
    /// This method is similar to [`send`](OutboundEnvelope::send), but accepts an
    /// already-Arc'd message. This is useful when the message is already wrapped
    /// in an Arc (e.g., from IPC deserialization where Box is converted to Arc).
    ///
    /// # Arguments
    ///
    /// * `message`: An Arc-wrapped message payload to send.
    #[cfg(feature = "ipc")]
    #[instrument(skip(self, message), level = "trace")]
    pub async fn send_arc(&self, message: Arc<dyn ActonMessage + Send + Sync>) {
        self.send_message_inner(message).await;
    }

    /// Tries to send an Arc-wrapped message without blocking.
    ///
    /// This method is similar to [`send_arc`](OutboundEnvelope::send_arc), but uses
    /// `try_reserve()` instead of `reserve()`. It returns immediately with an error
    /// if the recipient's channel is full, rather than waiting for capacity.
    ///
    /// This is useful for IPC scenarios where backpressure feedback is needed
    /// rather than blocking the IPC listener.
    ///
    /// # Arguments
    ///
    /// * `message`: An Arc-wrapped message payload to send.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The recipient's channel is closed
    /// - The recipient's channel is full (backpressure)
    #[cfg(feature = "ipc")]
    #[instrument(skip(self, message), level = "trace")]
    pub fn try_send_arc(
        &self,
        message: Arc<dyn ActonMessage + Send + Sync>,
    ) -> Result<(), crate::common::ipc::IpcError> {
        use crate::common::ipc::IpcError;
        use tokio::sync::mpsc::error::TrySendError;

        // Determine the target address
        let target_address = self
            .recipient_address
            .as_ref()
            .unwrap_or(&self.return_address);
        let target_id = &target_address.sender;
        let channel_sender = target_address.address.clone();

        trace!(sender = %self.return_address.sender, recipient = %target_id, "Attempting try_send_arc");

        if channel_sender.is_closed() {
            tracing::error!(sender = %self.return_address.sender, recipient = %target_id, "Recipient channel is closed");
            return Err(IpcError::IoError("Recipient channel is closed".to_string()));
        }

        // Try to reserve a send permit without blocking
        let permit = match channel_sender.try_reserve() {
            Ok(permit) => permit,
            Err(TrySendError::Full(())) => {
                tracing::warn!(sender = %self.return_address.sender, recipient = %target_id, "Target actor inbox is full");
                return Err(IpcError::TargetBusy);
            }
            Err(TrySendError::Closed(())) => {
                tracing::error!(sender = %self.return_address.sender, recipient = %target_id, "Recipient channel is closed");
                return Err(IpcError::IoError("Recipient channel is closed".to_string()));
            }
        };

        let internal_envelope =
            Envelope::new(message, self.return_address.clone(), target_address.clone());
        trace!(sender = %self.return_address.sender, recipient = %target_id, "Sending message via try_reserve permit");
        permit.send(internal_envelope);
        Ok(())
    }
}
