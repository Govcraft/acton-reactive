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

//! A barrier for broadcasts: [`FlushBroadcasts`] and its reply [`BroadcastsFlushed`].

use crate::traits::Request;

/// Asks the broker to confirm that every broadcast sent before it has been delivered.
///
/// # Why this exists
///
/// [`broadcast`](crate::traits::Broadcaster::broadcast) is fire-and-forget. Awaiting it
/// means the message is in the *broker's* inbox, which says nothing about whether any
/// subscriber has received it, let alone handled it. That gap is why broadcasting and
/// then immediately shutting down loses messages, and why so much pub/sub code reaches
/// for a sleep — a guess that is only ever probably long enough.
///
/// Unlike a direct message, a broadcast cannot answer for itself: the broker hands
/// subscribers the payload alone, with no reply address, so there is nothing for a
/// subscriber to reply to. The broker is the only participant that can speak for a
/// broadcast, which is why the barrier lives here rather than on the subscriber.
///
/// # What it guarantees
///
/// The broker is an ordinary actor with a FIFO inbox, and its broadcast handler is a
/// mutable one — it awaits fan-out to every subscriber before it dequeues the next
/// message. So when the broker answers this request, every broadcast enqueued ahead of
/// it is already sitting in every subscriber's inbox.
///
/// That is delivery, not completion. Subscriber inboxes are also FIFO, so the messages
/// will be handled in the order they arrived and ahead of anything sent later — which
/// is exactly what makes a subsequent
/// [`shutdown_all`](crate::common::ActorRuntime::shutdown_all) safe, since `Terminate`
/// then queues behind work that has already landed. If you need to know a *particular*
/// subscriber has finished handling it, `ask` that subscriber afterwards: the reply
/// proves it, because the broadcast was already ahead of your request in its inbox.
///
/// # Example
///
/// ```
/// use acton_reactive::prelude::*;
///
/// # #[acton_message]
/// # struct PriceChanged { price: f64 }
/// # async fn example(runtime: &mut ActorRuntime) -> anyhow::Result<()> {
/// let broker = runtime.broker();
///
/// broker.broadcast(PriceChanged { price: 42.0 }).await;
///
/// // Every subscriber now has the message queued. Shutting down is safe: `Terminate`
/// // lands behind it, and an actor drains its backlog before it stops.
/// broker.ask(FlushBroadcasts).await?;
/// runtime.shutdown_all().await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FlushBroadcasts;

/// The broker's answer to [`FlushBroadcasts`]: every earlier broadcast has been
/// delivered to every subscriber.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BroadcastsFlushed;

impl Request for FlushBroadcasts {
    type Response = BroadcastsFlushed;
}
