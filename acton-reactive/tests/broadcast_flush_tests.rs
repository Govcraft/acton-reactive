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

//! Defends the broadcast barrier: [`FlushBroadcasts`] and the flush that
//! [`ActorRuntime::shutdown_all`] performs before it signals anything.
//!
//! What is actually being claimed here is an ordering property, so every test states it
//! as a count that is wrong if the ordering does not hold. Nothing sleeps: a test for a
//! barrier that synchronised with a sleep would pass whether or not the barrier worked,
//! which is the exact defect this whole area of the code was fixed to remove.
//!
//! The load-bearing test is [`shutdown_all_delivers_a_broadcast_issued_just_before_it`].
//! Before `shutdown_all` flushed the broker it failed intermittently rather than never,
//! so a single green run proves little - it is written to fan out widely enough that a
//! lost message is very likely rather than merely possible.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Long enough to be decisive, short enough that a hang is not a break.
const PATIENCE: Duration = Duration::from_secs(5);

#[acton_actor]
struct Listener {
    seen: u32,
}

#[acton_message]
struct Event;

/// Asks a listener how many events it has handled.
#[acton_message]
struct HowMany;

#[acton_message]
struct Seen(u32);

impl Request for HowMany {
    type Response = Seen;
}

/// Builds a started listener subscribed to [`Event`], counting into `counter`.
async fn spawn_listener(runtime: &mut ActorRuntime, counter: &Arc<AtomicU32>) -> ActorHandle {
    let counter = counter.clone();
    let mut listener = runtime.new_actor::<Listener>();

    listener
        .mutate_on::<Event>(move |actor, _ctx| {
            actor.model.seen += 1;
            counter.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        })
        .mutate_on::<HowMany>(|actor, ctx| {
            let reply = ctx.reply_envelope();
            let seen = actor.model.seen;
            Reply::pending(async move {
                reply.send(Seen(seen)).await;
            })
        });

    listener.handle().subscribe::<Event>().await;
    listener.start().await
}

/// The broker answers the request at all, and does so promptly.
#[acton_test]
async fn the_broker_answers_a_flush() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let broker = runtime.broker();

    let answered = tokio::time::timeout(PATIENCE, broker.ask(FlushBroadcasts))
        .await
        .expect("flushing an idle broker should not hang")?;

    assert_eq!(answered, BroadcastsFlushed);

    runtime.shutdown_all().await?;
    Ok(())
}

/// A flush with no subscribers at all still answers, rather than waiting for an audience
/// that will never exist.
#[acton_test]
async fn a_flush_with_no_subscribers_still_answers() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let broker = runtime.broker();

    broker.broadcast(Event).await;

    tokio::time::timeout(PATIENCE, broker.ask(FlushBroadcasts))
        .await
        .expect("a broadcast nobody subscribed to should not stall the flush")?;

    runtime.shutdown_all().await?;
    Ok(())
}

/// The property the barrier exists for: once the flush answers, every earlier broadcast
/// is in the subscriber's inbox, so a question asked afterwards queues behind them all.
///
/// Without the flush the `ask` below could reach the listener before the broker had
/// handed over any of the events, and the count would be short.
#[acton_test]
async fn a_flush_puts_every_earlier_broadcast_ahead_of_a_later_question() -> anyhow::Result<()> {
    const EVENTS: u32 = 25;

    let counter = Arc::new(AtomicU32::new(0));
    let mut runtime = ActonApp::launch_async().await;
    let broker = runtime.broker();

    let listener = spawn_listener(&mut runtime, &counter).await;

    for _ in 0..EVENTS {
        broker.broadcast(Event).await;
    }
    broker.ask(FlushBroadcasts).await?;

    let seen: Seen = tokio::time::timeout(PATIENCE, listener.ask(HowMany))
        .await
        .expect("the listener should answer")?;

    assert_eq!(
        seen.0, EVENTS,
        "the flush should have placed all {EVENTS} events ahead of the question"
    );

    runtime.shutdown_all().await?;
    Ok(())
}

/// The reason `shutdown_all` flushes: a broadcast issued immediately before it must not
/// be lost to the `Terminate` signals that follow.
///
/// Several listeners and several events, because the failure this guards against was
/// intermittent - a wider fan-out makes a regression show up as a failure rather than as
/// a lucky pass.
#[acton_test]
async fn shutdown_all_delivers_a_broadcast_issued_just_before_it() -> anyhow::Result<()> {
    const LISTENERS: u32 = 5;
    const EVENTS: u32 = 10;

    let counter = Arc::new(AtomicU32::new(0));
    let mut runtime = ActonApp::launch_async().await;
    let broker = runtime.broker();

    for _ in 0..LISTENERS {
        let _handle = spawn_listener(&mut runtime, &counter).await;
    }

    for _ in 0..EVENTS {
        broker.broadcast(Event).await;
    }

    // No barrier of our own, and no sleep. This is the plain thing a user writes, and
    // `shutdown_all` is responsible for making it correct.
    runtime.shutdown_all().await?;

    assert_eq!(
        counter.load(Ordering::SeqCst),
        LISTENERS * EVENTS,
        "every listener should have handled every event before shutdown completed"
    );

    Ok(())
}

/// Delivery is not completion, and the documentation says so. This pins the weaker of
/// the two claims: after a flush the messages are *queued*, and it is the subscriber's
/// own FIFO inbox that turns that into "handled" when you ask it something.
#[acton_test]
async fn a_flush_is_delivery_and_the_inbox_turns_it_into_completion() -> anyhow::Result<()> {
    let counter = Arc::new(AtomicU32::new(0));
    let mut runtime = ActonApp::launch_async().await;
    let broker = runtime.broker();

    let listener = spawn_listener(&mut runtime, &counter).await;

    broker.broadcast(Event).await;
    broker.ask(FlushBroadcasts).await?;

    // The reply is the proof: it cannot be produced until the handler ahead of it in the
    // same inbox has run.
    let seen: Seen = listener.ask(HowMany).await?;
    assert_eq!(seen.0, 1);
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    runtime.shutdown_all().await?;
    Ok(())
}
