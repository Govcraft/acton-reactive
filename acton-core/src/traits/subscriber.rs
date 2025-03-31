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

use crate::common::BrokerRef;

/// Trait implemented by entities that need access to the system message broker.
///
/// This trait provides a single method, [`get_broker`](Subscriber::get_broker), which allows
/// retrieving an optional handle ([`BrokerRef`]) to the central message broker.
/// This is essential for components that need to interact with the broker, primarily
/// for subscribing or unsubscribing from message types via the [`Subscribable`] trait.
///
/// It is typically implemented by [`AgentHandle`](crate::common::AgentHandle).
pub trait Subscriber {
    /// Retrieves an optional handle to the message broker associated with this entity.
    ///
    /// Implementations should return `Some(BrokerRef)` if the entity is configured
    /// with a connection to the system broker, and `None` otherwise. The [`BrokerRef`]
    /// is an alias for [`AgentHandle`](crate::common::AgentHandle).
    ///
    /// # Returns
    ///
    /// * `Some(BrokerRef)`: A handle to the associated message broker.
    /// * `None`: If no broker is associated with this entity.
    fn get_broker(&self) -> Option<BrokerRef>;
}
