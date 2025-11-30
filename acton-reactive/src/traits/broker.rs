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

use async_trait::async_trait;

use crate::message::BrokerRequest;
use crate::prelude::ActonMessage;
use crate::traits::ActorHandleInterface; // Needed for broadcast_sync default impl

/// Defines the capability to broadcast messages to subscribers via the system broker.
///
/// This trait is typically implemented by types that have access to the central
/// [`Broker`](crate::common::Broker), such as [`ActorHandle`](crate::common::ActorHandle).
/// It provides methods for sending messages to the broker for distribution to all
/// actors subscribed to that message type.
#[async_trait]
pub trait Broadcaster: Clone + Debug + Default + Send + Sync + 'static {
    /// Asynchronously sends a message to the broker for broadcasting.
    ///
    /// The implementor should wrap the `message` in a [`BrokerRequest`] and send it
    /// to the central `Broker`.
    ///
    /// # Arguments
    ///
    /// * `message`: The message payload (must implement [`ActonMessage`]) to broadcast.
    ///
    /// # Returns
    ///
    /// A `Future` that completes once the broadcast request has been sent to the broker.
    /// Completion does not guarantee delivery to subscribers.
    fn broadcast(&self, message: impl ActonMessage) -> impl Future<Output = ()> + Send + Sync + '_;

    /// Synchronously sends a message to the broker for broadcasting.
    ///
    /// **Warning:** This default implementation relies on [`OutboundEnvelope::reply`],
    /// which internally spawns a blocking task and creates a new Tokio runtime.
    /// This is generally **discouraged** and can lead to performance issues or deadlocks,
    /// especially if called from within an existing asynchronous context. Prefer using
    /// the asynchronous [`Broker::broadcast`] method where possible.
    ///
    /// This method requires the implementing type (`Self`) to also implement
    /// [`ActorHandleInterface`] to use its `create_envelope` method.
    ///
    /// # Arguments
    ///
    /// * `message`: The message payload (must implement [`ActonMessage`]) to broadcast.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or failure of initiating the synchronous send.
    /// It relies on the behavior of [`OutboundEnvelope::reply`], which might not
    /// propagate all underlying errors from the actual send operation.
    fn broadcast_sync(&self, message: impl ActonMessage) -> anyhow::Result<()>
    where
        Self: ActorHandleInterface + Sized, // Require ActorHandleInterface for default impl
    {
        // Create an envelope targeting the broker (using self's address, assuming self *is* the broker handle or has access)
        // The recipient here is implicitly the broker itself when using reply on an envelope created from the broker handle.
        let envelope = self.create_envelope(Some(self.reply_address()));
        // Wrap the user message in a BrokerRequest before sending.
        envelope.reply(BrokerRequest::new(message))?; // Uses the potentially problematic OutboundEnvelope::reply
        Ok(())
    }
}
