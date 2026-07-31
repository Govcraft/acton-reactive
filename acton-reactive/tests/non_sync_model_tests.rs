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

//! Proof that an actor's model does not have to be [`Sync`].
//!
//! # Why this file exists at all
//!
//! This crate promises `Model: Default + Send + Debug + 'static` and
//! deliberately **not** `Sync`. That promise is easy to break by accident and
//! impossible for the rest of the suite to notice, because every other model in
//! it happens to be `Sync`.
//!
//! The way it breaks is specific. An `async fn(&self)` produces a future
//! holding `&Self`, and `&T` is `Send` only when `T: Sync`. So the moment
//! anything on the actor takes `&self` across an `await`, the message loop's
//! future silently acquires a `Model: Sync` bound. Against a `Sync` model it
//! compiles and passes; against a user's non-`Sync` model it does not compile at
//! all. The suite goes green and the user gets a type error in someone else's
//! crate.
//!
//! That is why `terminate_children` and `await_start_tasks` are free functions
//! over individually-borrowed `Sync` fields rather than methods, and why the
//! registration methods take `&mut self` rather than `&self`.
//!
//! # What this catches, and what the compiler already catches
//!
//! Worth being precise, because the two were measured and they are not the
//! same.
//!
//! Turning `await_start_tasks` back into an `async fn(&self)` method does
//! **not** need this file: the message loop is spawned from a generic `impl`,
//! so the bound is checked against the type parameter and **the library itself
//! stops compiling**. The compiler was already guarding that one.
//!
//! What it does not guard is a *public signature* quietly acquiring the bound.
//! Adding `Sync` to `supervise_deferred`'s `where` clause leaves the library
//! compiling perfectly — the bound is simply part of the contract now — and
//! nothing fails until somebody instantiates it with a non-`Sync` model. Every
//! other model in this suite is `Sync`, so nothing here would. That is a
//! silently narrowed public API, shipped green, and it is what this file exists
//! to fail on. Measured: that mutation builds the lib cleanly and breaks *this
//! file*, naming `Cell<bool>` and the offending bound.
//!
//! # How it is checked
//!
//! **These are compile-time assertions wearing tests' clothing.** The bodies
//! barely matter; a regression here fails to *build*, and a build failure is
//! the report. The runtime assertions only confirm the actor genuinely ran.
//!
//! `NotSync` holds a [`Cell`], which is [`Send`] but not [`Sync`] — the exact
//! shape a user reaches for when they want cheap interior mutability inside an
//! actor that owns its own state, which is the normal case in this crate.

use std::cell::Cell;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

const PATIENCE: Duration = Duration::from_secs(5);

/// A model that is [`Send`] but deliberately **not** [`Sync`].
///
/// `#[acton_actor]` is not used here: the point is to control this type
/// precisely, and to keep the non-`Sync` field visible at the definition rather
/// than behind a macro.
#[derive(Debug, Default)]
struct NotSync {
    counter: Cell<u32>,
}

/// A second one, for the supervised child.
#[derive(Debug, Default)]
struct AlsoNotSync {
    _marker: Cell<bool>,
}

#[acton_message]
struct Bump;

#[acton_message]
struct HireWorker;

// The premise. If `Cell` ever became `Sync` these tests would still pass while
// proving nothing, so the premise is asserted rather than assumed.
static_assertions::assert_impl_all!(NotSync: Send);
static_assertions::assert_not_impl_any!(NotSync: Sync);
static_assertions::assert_impl_all!(AlsoNotSync: Send);
static_assertions::assert_not_impl_any!(AlsoNotSync: Sync);

#[acton_test]
async fn an_actor_with_a_non_sync_model_starts_and_handles_messages() -> anyhow::Result<()> {
    // Starting an actor spawns its message loop with `tokio::spawn`, which
    // requires the whole loop future to be `Send`. With a non-`Sync` model that
    // holds only while nothing in the loop takes `&self` across an `await`.
    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<NotSync>();

    actor.mutate_on::<Bump>(|actor, _ctx| {
        actor.model.counter.set(actor.model.counter.get() + 1);
        Reply::ready()
    });

    // Every lifecycle hook takes `&ManagedActor<Started, _>`, so registering all
    // four is what keeps the hook plumbing honest as well as the loop.
    actor.before_start(|_actor| async move {});
    actor.after_start(|_actor| async move {});
    actor.before_stop(|_actor| async move {});
    actor.after_stop(|_actor| async move {});

    let handle = actor.start().await;
    tokio::time::timeout(PATIENCE, handle.send(Bump))
        .await
        .expect("a non-Sync actor takes messages");

    runtime.shutdown_all().await?;
    Ok(())
}

#[acton_test]
async fn a_non_sync_supervisor_can_supervise_and_restart_a_non_sync_child() -> anyhow::Result<()> {
    // The path this file was added for. The restart engine put new `async fn`s
    // near the actor — the shutdown IPC sweep, the start-task wait, cascading
    // termination — and any one of them taking `&self` across an `await` would
    // impose `Model: Sync` on the loop.
    //
    // Supervisor and child are both non-`Sync`, so the bound cannot be
    // satisfied incidentally by either half.
    let mut runtime = ActonApp::launch_async().await;
    let (registered, mut registrations) = tokio::sync::mpsc::unbounded_channel();

    let mut parent = runtime.new_actor::<NotSync>();
    parent.mutate_on::<HireWorker>(move |actor, _ctx| {
        let config = ActorConfig::for_supervised_child("worker", actor.handle().clone(), None)
            .expect("a name plus a live parent is a valid child configuration")
            .with_restart_policy(RestartPolicy::Permanent)
            .with_restart_limiter(RestartLimiterConfig {
                initial_backoff_ms: 10,
                max_backoff_ms: 50,
                backoff_multiplier: 1.0,
                ..RestartLimiterConfig::default()
            });
        let _ = registered.send(
            actor.supervise_deferred(config, |child: &mut ManagedActor<Idle, AlsoNotSync>| {
                child.mutate_on::<Bump>(|_actor, _ctx| Reply::ready());
            }),
        );
        Reply::ready()
    });

    let parent = parent.start().await;
    parent.send(HireWorker).await;

    let mut child = tokio::time::timeout(PATIENCE, registrations.recv())
        .await
        .expect("the supervisor must answer")
        .expect("the channel is open")
        .expect("the first child of a name is accepted");

    let first = tokio::time::timeout(PATIENCE, child.wait_running())
        .await
        .expect("the first start must land")?;

    // And drive a full restart, so the engine's own paths are exercised under a
    // non-Sync model rather than merely compiled.
    first.stop().await?;
    let second = tokio::time::timeout(
        PATIENCE,
        child.wait_generation(RestartGeneration::FIRST.next()),
    )
    .await
    .expect("a non-Sync child must be restarted like any other")?;

    assert_eq!(second.id(), first.id(), "a restart keeps the child's identity");

    runtime.shutdown_all().await?;
    Ok(())
}
