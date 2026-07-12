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
use std::fmt::Debug;

use acton_ern::Ern;

use crate::common::ActorHandle;

/// Internal message sent to the broker to remove a single subscription.
///
/// Carries the `TypeId` of the message type being unsubscribed along with the
/// identity and handle of the unsubscribing actor, mirroring the data carried
/// by [`SubscribeBroker`](crate::message::SubscribeBroker). The broker uses
/// this information to remove the matching entry from its subscriber map.
///
/// This message supersedes the crate's former field-less `UnsubscribeBroker`
/// placeholder, which the broker had no handler for and which therefore made
/// unsubscribing a silent no-op.
#[derive(Debug, Clone)]
pub struct RemoveSubscription {
    pub(crate) subscriber_id: Ern,
    pub(crate) message_type_id: TypeId,
    pub(crate) subscriber_context: ActorHandle,
}

/// Internal message sent to the broker to remove an actor from all subscriptions.
///
/// Sent automatically during actor shutdown so that a stopped actor's handle
/// does not linger in the broker's subscriber map and continue to receive
/// broadcasts addressed to its closed inbox.
#[derive(Debug, Clone)]
pub struct RemoveAllSubscriptions {
    pub(crate) subscriber_id: Ern,
}
