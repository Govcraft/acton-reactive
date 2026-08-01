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

//! What a graceful stop does, and does not, deliver.
//!
//! `SystemSignal::Terminate` is an ordinary message in a FIFO inbox. The
//! contract that falls out of that has two halves, and both are load-bearing:
//! the backlog queued *ahead* of the signal is drained, and work that arrives
//! *behind* it is not. Each half is pinned separately below, because they fail
//! for opposite reasons and a test that conflated them would be satisfied by
//! doing nothing at all.
//!
//! The drain half shipped through v0.6.0, was dropped in 79aeb80c (v7.0.0) as a
//! side effect of an unrelated change, and is restored here.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;

/// Upper bound on a wait that normally completes in microseconds.
const PATIENCE: Duration = Duration::from_secs(5);

#[acton_actor]
struct Counter {
    handled: usize,
}

#[acton_message]
struct Tick;

/// Sends a follow-up only after holding the actor's loop, so the follow-up is
/// provably enqueued behind the stop signal rather than racing it.
#[acton_message]
struct StartChain;

#[acton_message]
struct ChainLink;

/// A message enqueued *behind* the stop signal is still drained.
///
/// This is the half of the contract that regressed, and the only arrangement
/// that actually discriminates. Simply sending a burst and then stopping proves
/// nothing: the burst sits ahead of the stop signal in a FIFO queue, so it is
/// handled whether or not the loop drains. The message has to land *behind* the
/// signal for the drain to be what carries it.
///
/// The ordering is forced rather than raced. The handler holds the actor's loop
/// inside its `Reply::pending` block long enough for `shutdown_all` to have
/// enqueued the stop signal, and only then sends the follow-up - so the
/// follow-up is provably behind the signal. Without the hold this is exactly
/// the `examples/basic` race, which fails around a quarter of the time rather
/// than always.
///
/// Mutation that turns this red: restore the `break` in the stop-signal arm of
/// `run_message_loop` (`started.rs`), which is what v7.0.0 through v8.2.0 did.
/// `handled` is then 0, because the actor stops at the signal and never reaches
/// the message behind it.
#[tokio::test]
async fn a_message_enqueued_behind_the_stop_signal_is_still_drained() {
    let mut runtime = ActonApp::launch_async().await;
    let mut builder = runtime.new_actor::<Counter>();

    let handled = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&handled);

    builder
        .mutate_on::<StartChain>(|actor, _| {
            let self_handle = actor.handle().clone();
            Reply::pending(async move {
                // Mutable handlers are awaited inline, so this holds the actor's
                // loop. `shutdown_all` is concurrently enqueuing the stop
                // signal, which therefore lands first, and the `ChainLink`
                // below lands behind it.
                tokio::time::sleep(Duration::from_millis(250)).await;
                self_handle.send(ChainLink).await;
            })
        })
        .mutate_on::<ChainLink>(move |actor, _| {
            actor.model.handled += 1;
            counted.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = builder.start().await;
    handle.send(StartChain).await;

    tokio::time::timeout(PATIENCE, runtime.shutdown_all())
        .await
        .expect("shutdown_all must terminate, not drain forever")
        .expect("shutdown_all should succeed");

    assert_eq!(
        handled.load(Ordering::SeqCst),
        1,
        "a message enqueued behind the stop signal should still be drained"
    );
}

/// Once an actor has stopped, sends to it are not delivered.
///
/// This is the other half of the contract and the bound that makes the drain
/// terminate: the stop signal closes the inbox, so nothing can be added to the
/// backlog while it is being worked off. It is also the reason a drain cannot
/// be relied on to finish a chain of messages - a handler that runs *during*
/// the drain can no longer enqueue anything.
///
/// Mutation that turns this red: drop the `self.inbox.close()` from the
/// stop-signal arm of `run_message_loop`. The late message is then accepted and
/// `handled` becomes 4 - and the drain simultaneously loses its termination
/// guarantee, which is the trade this assertion documents.
#[tokio::test]
async fn sends_after_an_actor_has_stopped_are_not_delivered() {
    let mut runtime = ActonApp::launch_async().await;
    let mut builder = runtime.new_actor::<Counter>();

    // Counted in the handler itself rather than in `after_stop`, so a late
    // delivery is still observed after the stop hooks have run.
    let handled = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&handled);

    builder.mutate_on::<Tick>(move |actor, _| {
        actor.model.handled += 1;
        counted.fetch_add(1, Ordering::SeqCst);
        Reply::ready()
    });

    let handle = builder.start().await;
    for _ in 0..3 {
        handle.send(Tick).await;
    }

    // Returns only once the actor's task has finished, so the inbox is closed
    // and the drain is complete. Nothing after this point is racing anything.
    tokio::time::timeout(PATIENCE, handle.stop())
        .await
        .expect("stop must terminate")
        .expect("stop should succeed");

    assert_eq!(
        handled.load(Ordering::SeqCst),
        3,
        "the three queued messages should have been drained by the stop"
    );

    // The actor is gone; this send has nowhere to land.
    handle.send(Tick).await;

    tokio::time::timeout(PATIENCE, runtime.shutdown_all())
        .await
        .expect("shutdown_all must terminate")
        .expect("shutdown_all should succeed");

    assert_eq!(
        handled.load(Ordering::SeqCst),
        3,
        "a message sent after the actor stopped should not be delivered"
    );
}

/// The drain terminates even when every drained handler tries to enqueue more.
///
/// The bound is not a deadline: it is that a closed inbox rejects the sends,
/// so the backlog can only shrink. Without that, this test hangs and the
/// `PATIENCE` timeout fails it rather than letting it run forever.
#[tokio::test]
async fn draining_terminates_when_handlers_keep_sending() {
    let mut runtime = ActonApp::launch_async().await;
    let mut builder = runtime.new_actor::<Counter>();

    builder.mutate_on::<Tick>(|actor, _| {
        actor.model.handled += 1;
        let self_handle = actor.handle().clone();
        // Every drained message tries to feed the loop another one.
        Reply::pending(async move {
            self_handle.send(Tick).await;
        })
    });

    let handle = builder.start().await;
    for _ in 0..20 {
        handle.send(Tick).await;
    }

    tokio::time::timeout(PATIENCE, runtime.shutdown_all())
        .await
        .expect("a drain whose handlers keep sending must still terminate")
        .expect("shutdown_all should succeed");
}
