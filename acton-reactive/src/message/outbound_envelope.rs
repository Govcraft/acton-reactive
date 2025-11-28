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
use std::sync::Arc;

use tokio::runtime::Runtime; // Used in reply
use tracing::{error, instrument, trace};

use crate::common::{Envelope, MessageError};
use crate::message::message_address::MessageAddress;
use crate::traits::ActonMessage;

/// Represents a message prepared for sending, including sender and optional recipient addresses.
///
/// An `OutboundEnvelope` is typically created by an agent (using methods like
/// [`AgentHandle::create_envelope`](crate::common::AgentHandle::create_envelope))
/// before sending a message. It holds the [`MessageAddress`] of the sender (`return_address`)
/// and optionally the [`MessageAddress`] of the recipient (`recipient_address`).
///
/// The primary methods for dispatching the message are [`OutboundEnvelope::send`] (asynchronous)
/// and [`OutboundEnvelope::reply`] (synchronous wrapper).
///
/// Equality and hashing are based solely on the `return_address`.
#[derive(Clone, Debug)]
pub struct OutboundEnvelope {
    /// The address of the agent sending the message.
    pub(crate) return_address: MessageAddress,
    /// The address of the intended recipient agent, if specified directly.
    /// If `None`, the recipient might be implied (e.g., sending back to `return_address`).
    pub(crate) recipient_address: Option<MessageAddress>,
    /// The cancellation token for the sending agent.
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
    /// * `return_address`: The [`MessageAddress`] of the agent creating this envelope (the sender).
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
    pub fn reply_to(&self) -> MessageAddress {
        self.return_address.clone()
    }

    /// Returns a reference to the optional recipient's [`MessageAddress`].
    #[inline]
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

    /// Sends a message using this envelope, blocking the current thread until sent.
    ///
    /// **Warning:** This method spawns a blocking Tokio task and creates a new Tokio runtime
    /// internally to execute the asynchronous `send` operation. This is generally **discouraged**
    /// within an existing asynchronous context as it can lead to performance issues or deadlocks.
    /// Prefer using the asynchronous [`OutboundEnvelope::send`] method whenever possible.
    ///
    /// This method is primarily intended for scenarios where an asynchronous context is not readily
    /// available, but its use should be carefully considered.
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
        // Consider changing return type if errors aren't propagated.
        let envelope = self.clone();
        let message_arc = Arc::new(message); // Arc the message once

        // Spawn a blocking task to handle the async send without blocking the caller's async runtime (if any).
        // Note: Creating a new Runtime per call is inefficient.
        tokio::task::spawn_blocking(move || {
            trace!(sender = %envelope.return_address.sender, recipient = ?envelope.recipient_address.as_ref().map(|r| r.sender.to_string()), "Replying synchronously (blocking task)");
            // Consider using Handle::current().block_on if already in a runtime context,
            // but creating a new one avoids potential deadlocks if called from non-Tokio thread.
            let rt = Runtime::new().unwrap();
            rt.block_on(async move {
                // Use the internal async send logic.
                envelope.send_message_inner(message_arc).await;
            });
        });
        Ok(()) // Currently doesn't propagate errors from send_message_inner
    }

    /// Crate-internal: Asynchronously sends the message payload to the recipient.
    /// Handles channel reservation and error logging.
    #[instrument(skip(self, message), level = "debug", fields(message_type = ?message.type_id()))]
    async fn send_message_inner(&self, message: Arc<dyn ActonMessage + Send + Sync>) {
        // Determine the target address: recipient if Some, otherwise return_address.
        let target_address = self
            .recipient_address
            .as_ref()
            .unwrap_or(&self.return_address);
        let target_id = &target_address.sender;
        let channel_sender = target_address.address.clone(); // Keep the owned clone

        trace!(sender = %self.return_address.sender, recipient = %target_id, "Attempting to send message");

        if channel_sender.is_closed() {
            // Channel was already closed.
            error!(sender = %self.return_address.sender, recipient = %target_id, "Recipient channel is closed");
        } else {
            // Cancellation-aware reservation of a send permit
            let cancellation = self.cancellation_token.clone();
            tokio::select! {
                () = cancellation.cancelled() => {
                    error!(sender = %self.return_address.sender, recipient = %target_id, "Send aborted: cancellation_token triggered");
                    return;
                }
                permit_result = channel_sender.reserve() => {
                    match permit_result {
                        Ok(permit) => {
                            // Create the internal Envelope to put on the channel.
                            let internal_envelope = Envelope::new(
                                message, // Pass the Arc'd message
                                self.return_address.clone(),
                                target_address.clone(),
                            );
                            trace!(sender = %self.return_address.sender, recipient = %target_id, "Sending message via permit");
                            permit.send(internal_envelope); // Send using the permit
                        }
                        Err(e) => {
                            // Error reserving capacity (likely channel closed).
                            error!(sender = %self.return_address.sender, recipient = %target_id, error = %e, "Failed to reserve channel capacity");
                        }
                    }
                }
            }
        }
    }

    /// Sends a message asynchronously using this envelope.
    ///
    /// This method takes the message payload, wraps it in an `Arc`, and calls the
    /// internal `send_message_inner` to dispatch it to the recipient's channel.
    /// The recipient is determined by `recipient_address` if `Some`, otherwise it
    /// defaults to `return_address`.
    ///
    /// This is the preferred method for sending messages from within an asynchronous context.
    ///
    /// # Arguments
    ///
    /// * `message`: The message payload to send. Must implement [`ActonMessage`] and be `'static`.
    #[instrument(skip(self, message), level = "trace", fields(message_type = std::any::type_name_of_val(&message)))]
    pub async fn send(&self, message: impl ActonMessage + 'static) {
        // Arc the message and call the internal async sender.
        self.send_message_inner(Arc::new(message)).await;
    }
}
