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
use std::fmt::Debug; // Import Debug explicitly
use std::sync::Arc;

use tracing::{trace, Callsite};

use crate::traits::ActonMessage;

/// Wraps a message intended for broadcast via the system message broker.
///
/// When an agent wants to publish a message to all interested subscribers, it sends
/// a `BrokerRequest` containing the message payload to the [`AgentBroker`](crate::common::AgentBroker).
/// This struct includes the message itself (as a type-erased `Arc<dyn ActonMessage>`)
/// along with its `TypeId` and type name for efficient routing and debugging by the broker.
///
/// Using `Arc` allows the message payload to be shared efficiently among potentially
/// many subscribers without multiple clones of the underlying data.
#[derive(Debug, Clone)]
pub struct BrokerRequest {
    /// The message payload to be broadcast.
    /// Stored as a dynamically-typed, atomically reference-counted trait object.
    pub message: Arc<dyn ActonMessage + Send + Sync + 'static>,
    /// The string representation of the message's type name. Primarily for logging/debugging.
    pub message_type_name: String,
    /// The `TypeId` of the original concrete message type. Used by the broker for routing.
    pub message_type_id: TypeId,
}

impl BrokerRequest {
    /// Creates a new `BrokerRequest` wrapping the given message.
    ///
    /// This constructor takes a concrete message `M` that implements [`ActonMessage`],
    /// captures its `TypeId` and type name, wraps the message in an `Arc`, and
    /// stores them in a new `BrokerRequest` instance.
    ///
    /// # Type Parameters
    ///
    /// * `M`: The concrete type of the message being sent. Must implement [`ActonMessage`]
    ///   and be `Send + Sync + 'static`.
    ///
    /// # Arguments
    ///
    /// * `message`: The message instance to be broadcast.
    ///
    /// # Returns
    ///
    /// A new `BrokerRequest` ready to be sent to the `AgentBroker`.
    pub fn new<M: ActonMessage + Send + Sync + 'static>(message: M) -> Self {
        let message_type_name = std::any::type_name_of_val(&message).to_string();
        let message_type_id = message.type_id();
        // Wrap the message in Arc for efficient sharing.
        let message_arc = Arc::new(message);
        trace!(
            message_type = %message_type_name,
            type_id = ?message_type_id,
            "Creating BrokerRequest"
        );
        Self {
            message: message_arc, // Store the Arc'd message
            message_type_id,
            message_type_name,
        }
    }
}
