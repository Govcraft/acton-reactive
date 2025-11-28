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

use std::fmt::Debug; // Import Debug explicitly
use std::sync::Arc;

use tracing::trace;

use crate::message::BrokerRequest;
use crate::traits::ActonMessage;

/// A specialized envelope used by the [`AgentBroker`](crate::common::AgentBroker) to distribute broadcast messages.
///
/// When the broker receives a [`BrokerRequest`], it extracts the message payload
/// (`Arc<dyn ActonMessage>`) and wraps it in this `BrokerRequestEnvelope` before
/// sending it to each relevant subscriber. This simplifies the message handling logic
/// for subscribers receiving broadcast messages, as they receive the payload directly
/// without the extra metadata contained in the original `BrokerRequest`.
///
/// This type is primarily used internally by the broker and message dispatch mechanisms.
#[derive(Debug, Clone)]
pub struct BrokerRequestEnvelope {
    /// The original message payload being broadcast, shared via an `Arc`.
    pub message: Arc<dyn ActonMessage + Send + Sync + 'static>,
}

/// Enables converting a [`BrokerRequest`] directly into a [`BrokerRequestEnvelope`].
impl From<BrokerRequest> for BrokerRequestEnvelope {
    /// Extracts the message payload from a `BrokerRequest`.
    ///
    /// # Arguments
    ///
    /// * `value`: The `BrokerRequest` to convert.
    ///
    /// # Returns
    ///
    /// A new `BrokerRequestEnvelope` containing only the message payload (`Arc`)
    /// from the original request.
    #[inline]
    fn from(value: BrokerRequest) -> Self {
        trace!("Converting BrokerRequest to BrokerRequestEnvelope for message type: {}", value.message_type_name);
        Self {
            message: value.message, // Move the Arc from the request
        }
    }
}

impl BrokerRequestEnvelope {
    /// Creates a new `BrokerRequestEnvelope` wrapping the given message.
    ///
    /// This constructor takes a concrete message `M`, wraps it in an `Arc`, and
    /// stores it in the envelope. This is typically used internally or in testing
    /// scenarios where a `BrokerRequestEnvelope` needs to be created directly.
    ///
    /// # Type Parameters
    ///
    /// * `M`: The concrete type of the message. Must implement [`ActonMessage`]
    ///   and be `Send + Sync + 'static`.
    ///
    /// # Arguments
    ///
    /// * `message`: The message instance to wrap.
    ///
    /// # Returns
    ///
    /// A new `BrokerRequestEnvelope`.
    pub fn new<M: ActonMessage + Send + Sync + 'static>(message: M) -> Self {
        let message_arc = Arc::new(message);
        trace!(message_type = %std::any::type_name::<M>(), "Creating new BrokerRequestEnvelope");
        Self { message: message_arc }
    }
}
