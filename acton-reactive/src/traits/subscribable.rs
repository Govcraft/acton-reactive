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
use tracing::{error, instrument, trace};

use crate::message::{RemoveSubscription, SubscribeBroker};
use crate::traits::subscriber::Subscriber;
use crate::traits::{ActonMessage, ActorHandleInterface};

/// Enables an entity (typically an actor handle) to manage its subscriptions to message types via the system broker.
///
/// This trait provides methods to asynchronously subscribe and unsubscribe from specific
/// message types ([`ActonMessage`]). Implementors, usually [`ActorHandle`](crate::common::ActorHandle),
/// interact with the central [`Broker`](crate::common::Broker) to register or
/// deregister interest in receiving broadcast messages.
#[async_trait]
pub trait Subscribable: Send + Sync + 'static {
    // Added Send + Sync + 'static bounds
    /// Asynchronously subscribes the actor associated with this handle to messages of type `M`.
    ///
    /// After subscribing, the actor will receive copies of messages of type `M` that are
    /// broadcast via the [`Broadcaster`](super::Broadcaster) trait.
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
    /// The implementing type `Self` must also implement [`ActorHandleInterface`] and [`Subscriber`].
    fn subscribe<M: ActonMessage + Send + Sync + 'static>(
        &self,
    ) -> impl Future<Output = ()> + Send + Sync + '_
    where
        // These bounds are requirements for *calling* the method, enforced by the blanket impl.
        Self: ActorHandleInterface + Subscriber;

    /// Sends a request to unsubscribe the actor associated with this handle from messages of type `M`.
    ///
    /// After the broker processes the request, the actor no longer receives broadcast
    /// messages of type `M`. Other subscriptions held by the actor are unaffected. Note
    /// that actors are also automatically unsubscribed from all message types when they
    /// stop — on every termination path, including panic terminations when the
    /// `catch-handler-panics` feature is disabled.
    ///
    /// This method is fire-and-forget: it queues the removal request on a background
    /// task and returns immediately. When subsequent operations must observe the
    /// removal (for example, broadcasting again right away), use
    /// [`unsubscribe_async`](Subscribable::unsubscribe_async) instead.
    ///
    /// # Type Parameters
    ///
    /// * `M`: The concrete message type to unsubscribe from. Must implement [`ActonMessage`].
    ///
    /// # Requirements
    ///
    /// The implementing type `Self` must also implement [`ActorHandleInterface`] and [`Subscriber`].
    fn unsubscribe<M: ActonMessage>(&self)
    where
        // These bounds are requirements for *calling* the method, enforced by the blanket impl.
        Self: ActorHandleInterface + Subscriber;

    /// Asynchronously unsubscribes the actor associated with this handle from messages of type `M`.
    ///
    /// Behaves like [`unsubscribe`](Subscribable::unsubscribe) but returns a `Future`
    /// (mirroring [`subscribe`](Subscribable::subscribe)) so callers can await delivery
    /// of the removal request to the broker. Because the broker processes its inbox in
    /// order, a broadcast sent after this future completes can no longer be delivered
    /// under the removed subscription.
    ///
    /// # Type Parameters
    ///
    /// * `M`: The concrete message type to unsubscribe from. Must implement [`ActonMessage`]
    ///   and be `Send + Sync + 'static`.
    ///
    /// # Returns
    ///
    /// A `Future` that completes once the unsubscribe request has been sent to the broker.
    ///
    /// # Requirements
    ///
    /// The implementing type `Self` must also implement [`ActorHandleInterface`] and [`Subscriber`].
    fn unsubscribe_async<M: ActonMessage + Send + Sync + 'static>(
        &self,
    ) -> impl Future<Output = ()> + Send + Sync + '_
    where
        // These bounds are requirements for *calling* the method, enforced by the blanket impl.
        Self: ActorHandleInterface + Subscriber;
}

/// Blanket implementation of `Subscribable` for types implementing necessary traits.
///
/// This implementation provides the `subscribe` and `unsubscribe` methods for any type `T`
/// that implements [`ActorHandleInterface`] and [`Subscriber`]. It works by sending the
/// appropriate internal messages (`SubscribeBroker` or `RemoveSubscription`) to the
/// broker obtained via the [`Subscriber::get_broker`] method.
#[async_trait]
impl<T> Subscribable for T
where
    // Corrected bounds based on usage within the methods.
    T: ActorHandleInterface + Subscriber + Send + Sync + 'static,
{
    /// Sends the broker a crate-internal subscription request carrying this actor's
    /// handle and the `TypeId` of `M`, which the broker records so that later
    /// broadcasts of `M` are forwarded to this actor.
    #[instrument(skip(self), fields(message_type = std::any::type_name::<M>(), subscriber = %self.id()))]
    fn subscribe<M: ActonMessage + Send + Sync + 'static>(
        &self,
    ) -> impl Future<Output = ()> + Send + Sync + '_
// No need for where clause here as it's enforced by the impl block's bounds
    {
        let subscriber_id = self.id();
        let message_type_id = TypeId::of::<M>();
        let message_type_name = std::any::type_name::<M>().to_string();
        // Create the subscription message with the actor's handle as context.
        let subscription = SubscribeBroker {
            subscriber_id,
            message_type_id,
            subscriber_context: self.clone_ref(), // Clone the handle
        };
        let broker_option = self.get_broker(); // Get Option<BrokerRef>

        async move {
            trace!( type_id=?message_type_id, subscriber = %subscription.subscriber_id, "Sending subscription request");
            if let Some(broker_handle) = broker_option {
                trace!(broker = %broker_handle.id(), "Sending SubscribeBroker message");
                // Send the subscription message to the broker.
                broker_handle.send(subscription).await;
            } else {
                // Log an error if no broker is available.
                error!(subscriber = %subscription.subscriber_id, message_type = %message_type_name, "Cannot subscribe: No broker found.");
            }
        }
    }

    /// Spawns a task to send a `RemoveSubscription` message to the broker.
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
            // Create the removal message with the actor's handle as context.
            let unsubscription = RemoveSubscription {
                subscriber_id,
                message_type_id: type_id,
                subscriber_context: self.clone_ref(),
            };
            // Spawn a task to send the message asynchronously.
            tokio::spawn(async move {
                trace!(broker = %broker_handle.id(), type_id = ?type_id, "Sending RemoveSubscription message");
                broker_handle.send(unsubscription).await;
            });
        } else {
            error!(subscriber = %subscriber_id, message_type = %type_name, "Cannot unsubscribe: No broker found.");
        }
    }

    /// Sends a `RemoveSubscription` message to the broker.
    #[instrument(skip(self), fields(message_type = std::any::type_name::<M>(), subscriber = %self.id()))]
    fn unsubscribe_async<M: ActonMessage + Send + Sync + 'static>(
        &self,
    ) -> impl Future<Output = ()> + Send + Sync + '_
// No need for where clause here as it's enforced by the impl block's bounds
    {
        let subscriber_id = self.id();
        let message_type_id = TypeId::of::<M>();
        let message_type_name = std::any::type_name::<M>().to_string();
        // Create the removal message with the actor's handle as context.
        let unsubscription = RemoveSubscription {
            subscriber_id,
            message_type_id,
            subscriber_context: self.clone_ref(), // Clone the handle
        };
        let broker_option = self.get_broker(); // Get Option<BrokerRef>

        async move {
            trace!(type_id = ?message_type_id, subscriber = %unsubscription.subscriber_id, "Sending unsubscribe request");
            if let Some(broker_handle) = broker_option {
                trace!(broker = %broker_handle.id(), "Sending RemoveSubscription message");
                // Send the removal message to the broker.
                broker_handle.send(unsubscription).await;
            } else {
                // Log an error if no broker is available.
                error!(subscriber = %unsubscription.subscriber_id, message_type = %message_type_name, "Cannot unsubscribe: No broker found.");
            }
        }
    }
}
