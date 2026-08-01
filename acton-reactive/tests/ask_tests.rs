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

//! Defends the two promises `ask` makes: it answers, and it always finishes.
//!
//! The second is the load-bearing one. `ask` exists so that callers stop synchronising
//! with `tokio::time::sleep`, and a request/reply primitive that can wedge a task would
//! be worse than the sleeps it replaces. So every way a reply can fail to arrive gets a
//! test here that *provokes* it and asserts a returned value.
//!
//! Two rules follow from that, and both are deliberate:
//!
//! * **Nothing here sleeps to synchronise.** `ask` is the replacement for that idiom;
//!   testing it with the idiom it replaces would prove nothing about ordering. Where a
//!   test needs to know the actor has reached a certain point, the actor says so —
//!   either by answering, or through an explicit signal from inside the handler.
//!   Assuming a spawned request has already been enqueued is *not* good enough; that
//!   assumption made two of these tests pass for the wrong reason until a mutation
//!   exposed it.
//! * **Every await is wrapped in [`PATIENCE`].** A hang must surface as a failed
//!   assertion naming the invariant, not as a suite that never returns.
//!
//! Handle equality is never asserted on: `ActorHandle`'s `PartialEq` compares only the
//! `Ern`, so a stale handle pointing at a dead mailbox compares equal to a live one.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Long enough to be decisive, short enough that a hang is not a break.
const PATIENCE: Duration = Duration::from_secs(5);

#[acton_actor]
struct Counter {
    count: usize,
    /// A reply envelope the actor deliberately holds instead of answering, used to
    /// model a deferred reply that never comes.
    deferred: Option<OutboundEnvelope>,
}

#[acton_message]
struct Increment;

#[acton_message]
struct GetCount;

#[acton_message]
#[derive(PartialEq, Eq)]
struct Count {
    value: usize,
}

impl Request for GetCount {
    type Response = Count;
}

/// Answered by a handler that returns without replying.
#[acton_message]
struct Ignored;

impl Request for Ignored {
    type Response = Count;
}

/// Answered by a handler that panics before replying.
#[acton_message]
struct Panics;

impl Request for Panics {
    type Response = Count;
}

/// Answered by a handler that replies with a type the request does not declare.
#[acton_message]
struct Confused;

impl Request for Confused {
    type Response = Count;
}

/// The handler stores the reply envelope rather than answering, so the reply arrives
/// only if the actor is still alive to send it later.
#[acton_message]
struct Deferred;

impl Request for Deferred {
    type Response = Count;
}

/// Sent by the confused handler: a perfectly good message, just not a `Count`.
#[acton_message]
struct NotACount;

/// Carries a distinct token so a concurrent caller can prove it got *its own* reply
/// rather than merely *a* reply.
#[acton_message]
struct Echo {
    token: usize,
}

#[acton_message]
#[derive(PartialEq, Eq)]
struct Echoed {
    token: usize,
}

impl Request for Echo {
    type Response = Echoed;
}

/// Builds an actor answering every request type this file uses.
///
/// `stored_request` is notified once the `Deferred` handler has stored its reply
/// envelope. Tests wait on it rather than assuming a spawned `ask` has already reached
/// the inbox: `tokio::spawn` does not run the task before the spawning code continues,
/// so ordering between a spawned request and a later one is not guaranteed by the
/// inbox's FIFO. Waiting for the actor to say it got there is what makes it certain.
async fn start_counter(
    runtime: &mut ActorRuntime,
    stored_request: Arc<Notify>,
) -> ActorHandle {
    let mut actor = runtime.new_actor::<Counter>();

    actor
        .mutate_on::<Increment>(|actor, _ctx| {
            actor.model.count += 1;
            Reply::ready()
        })
        .mutate_on::<GetCount>(|actor, ctx| {
            let reply = ctx.reply_envelope();
            let value = actor.model.count;
            Reply::pending(async move {
                reply.send(Count { value }).await;
            })
        })
        // Legal, and by far the most common handler shape: it does its work and says
        // nothing. `ask` must cope with this rather than wait on it.
        .mutate_on::<Ignored>(|actor, _ctx| {
            actor.model.count += 1;
            Reply::ready()
        })
        .mutate_on::<Panics>(|_actor, _ctx| {
            panic!("this handler panics before it can reply");
        })
        .mutate_on::<Confused>(|_actor, ctx| {
            let reply = ctx.reply_envelope();
            Reply::pending(async move {
                reply.send(NotACount).await;
            })
        })
        .mutate_on::<Deferred>(move |actor, ctx| {
            actor.model.deferred = Some(ctx.reply_envelope());
            // Announced only after the envelope is stored, so a waiter that sees this
            // knows the request has genuinely arrived and is being held.
            stored_request.notify_one();
            Reply::ready()
        })
        .mutate_on::<Echo>(|_actor, ctx| {
            let reply = ctx.reply_envelope();
            let token = ctx.message().token;
            Reply::pending(async move {
                reply.send(Echoed { token }).await;
            })
        });

    actor.start().await
}

/// A notifier for tests that do not care when the `Deferred` handler runs.
fn unused_signal() -> Arc<Notify> {
    Arc::new(Notify::new())
}

/// The happy path: the caller gets the handler's answer back, typed.
#[acton_test]
async fn ask_returns_the_reply_the_handler_sent() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let handle = start_counter(&mut runtime, unused_signal()).await;

    handle.send(Increment).await;
    handle.send(Increment).await;

    let count = tokio::time::timeout(PATIENCE, handle.ask(GetCount))
        .await
        .expect("ask must resolve, not hang")?;

    assert_eq!(count.value, 2, "the reply should carry the actor's state");

    runtime.shutdown_all().await?;
    Ok(())
}

/// The reason `ask` exists. Messages sent before an `ask` are guaranteed processed by
/// the time it resolves, because the actor's inbox is a FIFO and the reply is sent from
/// the handler. That is the happens-before the examples currently fake with a sleep.
///
/// Deliberately many messages: a single one could be processed in time by luck.
#[acton_test]
async fn ask_proves_every_earlier_message_was_processed() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let handle = start_counter(&mut runtime, unused_signal()).await;

    for _ in 0..500 {
        handle.send(Increment).await;
    }

    // No sleep anywhere. If `ask` did not order behind the increments, this count would
    // come back short.
    let count = tokio::time::timeout(PATIENCE, handle.ask(GetCount))
        .await
        .expect("ask must resolve, not hang")?;

    assert_eq!(
        count.value, 500,
        "ask must not resolve until everything queued ahead of it has been processed"
    );

    runtime.shutdown_all().await?;
    Ok(())
}

/// A handler that never replies is legal. The caller must be told so, not left waiting.
#[acton_test]
async fn ask_reports_no_reply_when_the_handler_does_not_answer() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let handle = start_counter(&mut runtime, unused_signal()).await;

    let outcome = tokio::time::timeout(PATIENCE, handle.ask(Ignored))
        .await
        .expect("a handler that never replies must not hang the caller");

    assert_eq!(outcome, Err(AskError::NoReply));

    // The actor is unharmed and still answering: a silent handler is not an error.
    let count = tokio::time::timeout(PATIENCE, handle.ask(GetCount))
        .await
        .expect("ask must resolve, not hang")?;
    assert_eq!(count.value, 1, "the silent handler still did its work");

    runtime.shutdown_all().await?;
    Ok(())
}

/// A panicking handler cannot reply. Under the default `catch-handler-panics` the actor
/// survives; without it the actor dies. Either way the caller must be released, so this
/// test is deliberately not gated on that feature.
///
/// Uses `#[tokio::test]` rather than `#[acton_test]`, following the convention in
/// `panic_recovery_tests.rs`: the `acton_test` macro's panic detection would fail the
/// test on the very panic it is trying to provoke.
#[tokio::test(flavor = "multi_thread")]
async fn ask_reports_no_reply_when_the_handler_panics() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let handle = start_counter(&mut runtime, unused_signal()).await;

    let outcome = tokio::time::timeout(PATIENCE, handle.ask(Panics))
        .await
        .expect("a panicking handler must not hang the caller");

    assert_eq!(outcome, Err(AskError::NoReply));

    runtime.shutdown_all().await?;
    Ok(())
}

/// An actor that goes away while holding a request owes a reply it can no longer send.
/// Dropping the actor drops the stored envelope, which closes the reply channel — the
/// same mechanism that covers a request discarded during shutdown, and a request
/// abandoned when the supervision engine restarts an actor onto a fresh mailbox.
#[acton_test]
async fn ask_reports_no_reply_when_the_actor_stops_holding_the_request() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let stored = Arc::new(Notify::new());
    let handle = start_counter(&mut runtime, Arc::clone(&stored)).await;

    let asking = tokio::spawn({
        let handle = handle.clone();
        async move { handle.ask(Deferred).await }
    });

    // The actor tells us the request arrived and is being held. Waiting on a second
    // `ask` instead would prove nothing: the spawned task above may not have reached
    // its send yet, in which case the actor would be stopped before the request ever
    // arrived and this would pass as `Undeliverable` — a different failure wearing this
    // test's name.
    tokio::time::timeout(PATIENCE, stored.notified())
        .await
        .expect("the actor must receive and store the request");

    handle.stop().await?;

    let outcome = tokio::time::timeout(PATIENCE, asking)
        .await
        .expect("a stopped actor must release anyone waiting on it")?;

    assert_eq!(outcome, Err(AskError::NoReply));

    runtime.shutdown_all().await?;
    Ok(())
}

/// Asking an actor that has already stopped fails before delivery, and says so. This is
/// distinct from `NoReply`: nothing ran, so nothing was left half-done.
#[acton_test]
async fn ask_reports_undeliverable_when_the_actor_has_already_stopped() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let handle = start_counter(&mut runtime, unused_signal()).await;

    handle.stop().await?;

    let outcome = tokio::time::timeout(PATIENCE, handle.ask(GetCount))
        .await
        .expect("asking a stopped actor must fail promptly, not hang");

    assert_eq!(outcome, Err(AskError::Undeliverable));

    runtime.shutdown_all().await?;
    Ok(())
}

/// A handler answering with the wrong type is a bug in the actor. It must be reported
/// as such rather than looking like a lost reply.
#[acton_test]
async fn ask_reports_unexpected_reply_when_the_handler_answers_with_another_type(
) -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let handle = start_counter(&mut runtime, unused_signal()).await;

    let outcome = tokio::time::timeout(PATIENCE, handle.ask(Confused))
        .await
        .expect("ask must resolve, not hang");

    match outcome {
        Err(AskError::UnexpectedReply { expected, received }) => {
            assert!(
                expected.ends_with("Count"),
                "expected type should name the declared reply, got `{expected}`"
            );
            assert!(
                received.contains("NotACount"),
                "the rendering should identify what was actually sent, got `{received}`"
            );
        }
        other => panic!("expected UnexpectedReply, got {other:?}"),
    }

    runtime.shutdown_all().await?;
    Ok(())
}

/// The deadline is the backstop for the one case channel closure cannot see: the actor
/// is alive and still holds a usable reply address, it just never answers. Closure
/// cannot fire, so only a clock ends this.
#[acton_test]
async fn ask_times_out_when_the_actor_holds_the_request_and_never_answers() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let handle = start_counter(&mut runtime, unused_signal()).await;

    // The handler stores the reply envelope, so a sender stays alive indefinitely.
    let outcome = tokio::time::timeout(
        PATIENCE,
        handle.ask_with_timeout(Deferred, Duration::from_millis(100)),
    )
    .await
    .expect("the deadline must release the caller well inside PATIENCE");

    assert_eq!(
        outcome,
        Err(AskError::TimedOut {
            after: Duration::from_millis(100)
        }),
        "a held-but-unanswered request must time out, not resolve as NoReply"
    );

    runtime.shutdown_all().await?;
    Ok(())
}

/// The deadline must not pre-empt the ordinary failures. A dropped reply address is
/// reported immediately and specifically, rather than stalling until the timer expires
/// — that is the whole point of layering closure underneath the deadline.
#[acton_test]
async fn a_dropped_reply_address_is_reported_without_waiting_for_the_deadline(
) -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let handle = start_counter(&mut runtime, unused_signal()).await;

    // The real default deadline, six times longer than this test's patience: if `ask`
    // waited it out instead of noticing closure, PATIENCE would fire first and fail
    // this test rather than letting it pass slowly.
    let outcome = tokio::time::timeout(
        PATIENCE,
        handle.ask_with_timeout(Ignored, DEFAULT_ASK_TIMEOUT),
    )
    .await
    .expect("closure must report the failure without waiting out the deadline");

    assert!(
        PATIENCE < DEFAULT_ASK_TIMEOUT,
        "this test only means something while the deadline outlasts our patience"
    );

    assert_eq!(outcome, Err(AskError::NoReply));

    runtime.shutdown_all().await?;
    Ok(())
}

/// Concurrent asks must not be confused with one another. Each call owns a private reply
/// channel, so correlation is structural rather than by identifier.
///
/// Every request carries a distinct token that the handler echoes back, and each caller
/// asserts it received *its own* token. That is the property at issue: a test that only
/// checked "some reply arrived" would pass even if replies were handed to the wrong
/// waiters.
#[acton_test]
async fn concurrent_asks_each_receive_their_own_reply() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let handle = start_counter(&mut runtime, unused_signal()).await;

    let mut asks = Vec::new();
    for token in 0..25 {
        let handle = handle.clone();
        asks.push(tokio::spawn(async move {
            (token, handle.ask(Echo { token }).await)
        }));
    }

    for ask in asks {
        let (token, reply) = tokio::time::timeout(PATIENCE, ask)
            .await
            .expect("every concurrent ask must resolve")?;
        let echoed = reply?;
        assert_eq!(
            echoed.token, token,
            "each caller must receive the reply to its own request"
        );
    }

    runtime.shutdown_all().await?;
    Ok(())
}
