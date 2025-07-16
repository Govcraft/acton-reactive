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
use std::future::Future;

use async_trait::async_trait;
use tracing::*;

use crate::message::{SubscribeBroker, UnsubscribeBroker};
use crate::traits::{ActonMessage, AgentHandleInterface};
use crate::traits::subscriber::Subscriber;

/// Enables an entity (typically an agent handle) to manage its subscriptions to message types via the system broker.
///
/// This trait provides methods to asynchronously subscribe and unsubscribe from specific
/// message types ([`ActonMessage`]). Implementors, usually [`AgentHandle`](crate::common::AgentHandle),
/// interact with the central [`AgentBroker`](crate::common::AgentBroker) to register or
/// deregister interest in receiving broadcast messages.
#[async_trait]
pub trait Subscribable: Send + Sync + 'static { // Added Send + Sync + 'static bounds
    /// Asynchronously subscribes the agent associated with this handle to messages of type `M`.
    ///
    /// After subscribing, the agent will receive copies of messages of type `M` that are
    /// broadcast via the [`Broker`](super::Broker) trait.
    ///
    /// # Type Parameters
    ///
    /// * `M`: The concrete message type to subscribe to. Must implement [`ActonMessage`]
    ///   and be `Send + Sync + 'static`.
    ///
    /// # Returns
    ///
    /// A `Future` that completes once the subscription request has been sent to the broker.
    /// Completion does not guarantee the subscription is immediately active.
    ///
    /// # Requirements
    ///
    /// The implementing type `Self` must also implement [`AgentHandleInterface`] and [`Subscriber`].
    fn subscribe<M: ActonMessage + Send + Sync + 'static>(
        &self,
    ) -> impl Future<Output = ()> + Send + Sync + '_
    where
        // These bounds are requirements for *calling* the method, enforced by the blanket impl.
        Self: AgentHandleInterface + Subscriber;

    /// Sends a request to unsubscribe the agent associated with this handle from messages of type `M`.
    ///
    /// After unsubscribing, the agent will no longer receive broadcast messages of type `M`.
    ///
    /// Note: The default blanket implementation currently spawns a Tokio task to send the
    /// unsubscribe request asynchronously. The `UnsubscribeBroker` message itself might be incomplete
    /// in the current implementation (commented-out fields).
    ///
    /// # Type Parameters
    ///
    /// * `M`: The concrete message type to unsubscribe from. Must implement [`ActonMessage`].
    ///
    /// # Requirements
    ///
    /// The implementing type `Self` must also implement [`AgentHandleInterface`] and [`Subscriber`].
    fn unsubscribe<M: ActonMessage>(&self)
    where
        // These bounds are requirements for *calling* the method, enforced by the blanket impl.
        Self: AgentHandleInterface + Subscriber;
}

/// Blanket implementation of `Subscribable` for types implementing necessary traits.
///
/// This implementation provides the `subscribe` and `unsubscribe` methods for any type `T`
/// that implements [`AgentHandleInterface`] and [`Subscriber`]. It works by sending the
/// appropriate internal messages ([`SubscribeBroker`] or [`UnsubscribeBroker`]) to the
/// broker obtained via the [`Subscriber::get_broker`] method.
#[async_trait]
impl<T> Subscribable for T
where
    // Corrected bounds based on usage within the methods.
    T: AgentHandleInterface + Subscriber + Send + Sync + 'static,
{
    /// Sends a [`SubscribeBroker`] message to the broker.
    #[instrument(skip(self), fields(message_type = std::any::type_name::<M>(), subscriber = %self.id()))]
    fn subscribe<M: ActonMessage + Send + Sync + 'static>(
        &self,
    ) -> impl Future<Output = ()> + Send + Sync + '_
    // No need for where clause here as it's enforced by the impl block's bounds
    {
        let subscriber_id = self.id();
        let message_type_id = TypeId::of::<M>();
        let message_type_name = std::any::type_name::<M>().to_string();
        // Create the subscription message with the agent's handle as context.
        let subscription = SubscribeBroker {
            subscriber_id: subscriber_id.clone(), // Clone Ern for the message
            message_type_id,
            subscriber_context: self.clone_ref(), // Clone the handle
        };
        let broker_option = self.get_broker(); // Get Option<BrokerRef>

        async move {
            trace!( type_id=?message_type_id, subscriber = %subscriber_id, "Sending subscription request");
            if let Some(broker_handle) = broker_option {
                trace!(broker = %broker_handle.id(), "Sending SubscribeBroker message");
                // Send the subscription message to the broker.
                broker_handle.send(subscription).await;
            } else {
                // Log an error if no broker is available.
                error!(subscriber = %subscriber_id, message_type = %message_type_name, "Cannot subscribe: No broker found.");
            }
        }
    }

    /// Spawns a task to send an [`UnsubscribeBroker`] message to the broker.
    /// Note: The current `UnsubscribeBroker` message structure might be incomplete.
    #[instrument(skip(self), fields(message_type = std::any::type_name::<M>(), subscriber = %self.id()))]
    fn unsubscribe<M: ActonMessage>(&self)
    // No need for where clause here as it's enforced by the impl block's bounds
    {
        let type_id = TypeId::of::<M>();
        let type_name = std::any::type_name::<M>();
        let subscriber_id = self.id(); // Get subscriber ID
        let broker_option = self.get_broker(); // Get Option<BrokerRef>

        trace!(type_id = ?type_id, subscriber = %subscriber_id, "Initiating unsubscribe request for type {}", type_name);

        if let Some(broker_handle) = broker_option {
            // Create the unsubscribe message (currently seems incomplete based on commented fields).
            let unsubscription = UnsubscribeBroker {
                // ern: subscriber_id, // Assuming these fields will be added later
                // message_type_id: type_id,
                // subscriber_ref: self.clone_ref(),
            };
            // Spawn a task to send the message asynchronously.
            tokio::spawn(async move {
                trace!(broker = %broker_handle.id(), type_id = ?type_id, "Sending UnsubscribeBroker message");
                broker_handle.send(unsubscription).await;
            });
        } else {
            error!(subscriber = %subscriber_id, message_type = %type_name, "Cannot unsubscribe: No broker found.");
        }
    }
}
