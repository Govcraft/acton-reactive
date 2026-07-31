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

//! The framework bringing a failed child back by itself.
//!
//! Most of these fail by **hanging** rather than by asserting — a restart that
//! never happens leaves a caller waiting on a status that never changes — so
//! every wait is wrapped in an explicit timeout that fails loudly instead.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Long enough to be decisive, short enough that a hang is not a coffee break.
const PATIENCE: Duration = Duration::from_secs(5);

#[acton_actor]
struct Parent;

#[acton_actor]
struct Worker;

/// Asks a supervisor to take on a child, from inside its own handler.
#[acton_message]
struct HireWorker;

/// Asks a supervisor to prove it is still taking messages.
#[acton_message]
struct Ping;

type Registrations = (
    tokio::sync::mpsc::UnboundedSender<Result<SupervisedChild, SupervisionError>>,
    tokio::sync::mpsc::UnboundedReceiver<Result<SupervisedChild, SupervisionError>>,
);

/// Restart settings that make a test quick and its arithmetic obvious.
///
/// A 10ms initial backoff keeps the waiting invisible; the multiplier is 1.0 so
/// a test asserting *which* incarnation is running does not also have to model
/// exponential growth.
const fn brisk_limiter(max_restarts: u32) -> RestartLimiterConfig {
    RestartLimiterConfig {
        enabled: true,
        max_restarts,
        window_secs: 60,
        initial_backoff_ms: 10,
        max_backoff_ms: 50,
        backoff_multiplier: 1.0,
    }
}

/// A blueprint whose children count how many of them have been built.
///
/// The count is the proof a *new* incarnation exists rather than the old one
/// having been revived: nothing else distinguishes them, because a restart
/// deliberately keeps the child's identifier.
fn counting_blueprint(
    builds: &Arc<AtomicUsize>,
) -> impl Fn(&mut ManagedActor<Idle, Worker>) + Clone + Send + Sync + 'static {
    let builds = Arc::clone(builds);
    move |actor: &mut ManagedActor<Idle, Worker>| {
        builds.fetch_add(1, Ordering::SeqCst);
        actor.mutate_on::<Ping>(|_actor, _ctx| Reply::ready());
    }
}

/// A supervisor that hires one child when asked, and reports what happened.
///
/// `supervise_deferred` is the path a handler can use: it records the child and
/// queues its start, and the message loop launches it on its next turn.
fn supervising_parent(
    runtime: &mut ActorRuntime,
    registered: tokio::sync::mpsc::UnboundedSender<Result<SupervisedChild, SupervisionError>>,
    blueprint: impl Fn(&mut ManagedActor<Idle, Worker>) + Clone + Send + Sync + 'static,
    limiter: Option<RestartLimiterConfig>,
) -> ManagedActor<Idle, Parent> {
    let mut parent = runtime.new_actor::<Parent>();
    parent.mutate_on::<HireWorker>(move |actor, _ctx| {
        let mut config = ActorConfig::for_supervised_child("worker", actor.handle().clone(), None)
            .expect("a name plus a live parent is a valid child configuration")
            .with_restart_policy(RestartPolicy::Permanent);
        if let Some(limiter) = limiter.clone() {
            config = config.with_restart_limiter(limiter);
        }
        let _ = registered.send(actor.supervise_deferred(config, blueprint.clone()));
        Reply::ready()
    });
    parent.mutate_on::<Ping>(|_actor, _ctx| Reply::ready());
    parent
}

/// Hires the one child and hands back the caller's view of it.
async fn hire(
    parent: &ActorHandle,
    registrations: &mut tokio::sync::mpsc::UnboundedReceiver<
        Result<SupervisedChild, SupervisionError>,
    >,
) -> SupervisedChild {
    parent.send(HireWorker).await;
    tokio::time::timeout(PATIENCE, registrations.recv())
        .await
        .expect("the supervisor must answer a hire")
        .expect("the channel is open")
        .expect("the first child of a name is accepted")
}

fn channel() -> Registrations {
    tokio::sync::mpsc::unbounded_channel()
}

#[acton_test]
async fn a_permanent_child_that_dies_is_brought_back_by_the_framework() -> anyhow::Result<()> {
    // **Fails by hanging** without the restart engine: the child stops, its
    // slot sits at `Running` with a dead handle, and nothing ever publishes
    // generation 1. The timeout is the assertion.
    let mut runtime = ActonApp::launch_async().await;
    let (registered, mut registrations) = channel();
    let builds = Arc::new(AtomicUsize::new(0));
    let parent = supervising_parent(
        &mut runtime,
        registered,
        counting_blueprint(&builds),
        Some(brisk_limiter(5)),
    )
    .start()
    .await;

    let mut child = hire(&parent, &mut registrations).await;
    let first = tokio::time::timeout(PATIENCE, child.wait_running())
        .await
        .expect("the first start must land")?;
    assert_eq!(builds.load(Ordering::SeqCst), 1);

    // Stop the child out from under its supervisor. A `Permanent` child is
    // restarted from a normal termination, which is what makes an explicit
    // stop a usable stand-in for a crash here.
    first.stop().await?;

    let second = tokio::time::timeout(PATIENCE, child.wait_generation(RestartGeneration::FIRST.next()))
        .await
        .expect("the framework must bring a Permanent child back")?;

    assert_eq!(
        builds.load(Ordering::SeqCst),
        2,
        "the blueprint ran again, so this is a new incarnation and not the old one"
    );
    assert_eq!(
        second.id(),
        first.id(),
        "a restart keeps the child's identity"
    );
    assert_eq!(child.status().generation(), RestartGeneration::FIRST.next());
    assert_eq!(child.status().state(), SupervisionState::Running);

    // And the replacement is a real, reachable actor rather than a record.
    tokio::time::timeout(PATIENCE, second.send(Ping))
        .await
        .expect("the new incarnation takes messages");

    runtime.shutdown_all().await?;
    Ok(())
}

#[acton_test]
async fn a_supervisor_keeps_taking_messages_while_a_child_is_backing_off() -> anyhow::Result<()> {
    // The reason the backoff is a timer and not a sleep. Waiting it out on the
    // supervisor's own task would stop it taking messages — including its own
    // `Terminate` — for as long as the backoff lasts, and the default ceiling
    // on that is 30 seconds.
    let mut runtime = ActonApp::launch_async().await;
    let (registered, mut registrations) = channel();
    let builds = Arc::new(AtomicUsize::new(0));
    let parent = supervising_parent(
        &mut runtime,
        registered,
        counting_blueprint(&builds),
        Some(RestartLimiterConfig {
            initial_backoff_ms: 1_500,
            max_backoff_ms: 1_500,
            ..brisk_limiter(5)
        }),
    )
    .start()
    .await;

    let mut child = hire(&parent, &mut registrations).await;
    let first = tokio::time::timeout(PATIENCE, child.wait_running())
        .await
        .expect("the first start must land")?;
    first.stop().await?;

    // Establish that the backoff is actually running before timing anything
    // against it. Neither of the two calls above orders this: `stop()` returns
    // when the child's task ends, which is before its `ChildTerminated` has
    // been taken off the supervisor's inbox, and `send` only queues. Asserting
    // the state directly here therefore raced the decision and read `Running`.
    tokio::time::timeout(
        PATIENCE,
        child.wait_for(|s| s.state() == SupervisionState::RestartPending),
    )
    .await
    .expect("the child must reach its backoff")?;

    // Mid-backoff, the supervisor answers. The 1500ms backoff was armed a
    // moment ago, so an answer that arrives inside 500ms is unambiguously
    // during it — which is what makes the assertion below safe rather than
    // lucky.
    tokio::time::timeout(Duration::from_millis(500), parent.send(Ping))
        .await
        .expect("a supervisor waiting out a backoff must still take messages");
    assert_eq!(
        child.status().state(),
        SupervisionState::RestartPending,
        "and it answered during the backoff rather than after the restart"
    );

    // Then the restart still happens.
    tokio::time::timeout(PATIENCE, child.wait_generation(RestartGeneration::FIRST.next()))
        .await
        .expect("the armed timer must still fire")?;

    runtime.shutdown_all().await?;
    Ok(())
}

#[acton_test]
async fn a_child_that_exhausts_its_allowance_is_escalated_rather_than_left_pending()
-> anyhow::Result<()> {
    // **Fails by hanging** if `Escalate` is not handled: the slot stays in
    // `AwaitingBackoff` for a restart that will never be arranged, and every
    // caller waiting on this child waits on a status that cannot change again.
    let mut runtime = ActonApp::launch_async().await;
    let (registered, mut registrations) = channel();
    let builds = Arc::new(AtomicUsize::new(0));
    let parent = supervising_parent(
        &mut runtime,
        registered,
        counting_blueprint(&builds),
        Some(brisk_limiter(2)),
    )
    .start()
    .await;

    let mut child = hire(&parent, &mut registrations).await;
    let mut generation = RestartGeneration::FIRST;
    let mut handle = tokio::time::timeout(PATIENCE, child.wait_running())
        .await
        .expect("the first start must land")?;

    // Two restarts is the whole allowance; the third failure has nothing left.
    for _ in 0..2 {
        handle.stop().await?;
        generation = generation.next();
        handle = tokio::time::timeout(PATIENCE, child.wait_generation(generation))
            .await
            .expect("a restart within the allowance must happen")?;
    }
    handle.stop().await?;

    let status = tokio::time::timeout(PATIENCE, child.wait_for(|s| s.state().is_terminal()))
        .await
        .expect("an exhausted child must reach a terminal state, not sit pending")?;

    assert_eq!(status.state(), SupervisionState::Escalated);
    assert!(
        matches!(
            status.failure(),
            Some(SupervisionError::RestartLimit { .. })
        ),
        "the caller learns why it gave up: {:?}",
        status.failure()
    );
    assert_eq!(
        builds.load(Ordering::SeqCst),
        3,
        "one first start plus the two restarts the allowance covered"
    );

    // And a `wait_running` from here returns rather than waiting forever.
    let error = tokio::time::timeout(PATIENCE, child.wait_running())
        .await
        .expect("a terminal state must end the wait")
        .expect_err("the child is not coming back");
    assert!(
        matches!(error, SupervisionError::RestartLimit { .. }),
        "unexpected error: {error}"
    );

    runtime.shutdown_all().await?;
    Ok(())
}

#[acton_test]
async fn a_child_whose_policy_forbids_a_restart_is_recorded_down() -> anyhow::Result<()> {
    // The other terminal outcome, and the one that must *not* consume restart
    // allowance: a `Temporary` child is never restarted, so the decision layer
    // returns before the limiter is touched.
    let mut runtime = ActonApp::launch_async().await;
    let (registered, mut registrations) = channel();
    let builds = Arc::new(AtomicUsize::new(0));
    let blueprint = counting_blueprint(&builds);

    let mut parent = runtime.new_actor::<Parent>();
    parent.mutate_on::<HireWorker>(move |actor, _ctx| {
        let config = ActorConfig::for_supervised_child("worker", actor.handle().clone(), None)
            .expect("a name plus a live parent is a valid child configuration")
            .with_restart_policy(RestartPolicy::Temporary);
        let _ = registered.send(actor.supervise_deferred(config, blueprint.clone()));
        Reply::ready()
    });
    let parent = parent.start().await;

    let mut child = hire(&parent, &mut registrations).await;
    let handle = tokio::time::timeout(PATIENCE, child.wait_running())
        .await
        .expect("the first start must land")?;
    handle.stop().await?;

    let status = tokio::time::timeout(PATIENCE, child.wait_for(|s| s.state().is_terminal()))
        .await
        .expect("a child that will not be restarted must say so")?;

    assert_eq!(status.state(), SupervisionState::Down);
    assert_eq!(status.generation(), RestartGeneration::FIRST);
    assert_eq!(
        builds.load(Ordering::SeqCst),
        1,
        "nothing was rebuilt for a Temporary child"
    );

    runtime.shutdown_all().await?;
    Ok(())
}

#[acton_test]
async fn a_user_handler_for_child_terminated_still_runs() -> anyhow::Result<()> {
    // The one supervision message that is intercepted *additively*. Every other
    // one `continue`s past handler dispatch, so the locally consistent thing
    // here would be wrong: `ChildTerminated` is public, is in the prelude, and
    // until this release the deprecated config setters told people to handle it.
    let mut runtime = ActonApp::launch_async().await;
    let (registered, mut registrations) = channel();
    let builds = Arc::new(AtomicUsize::new(0));
    let blueprint = counting_blueprint(&builds);
    let noticed = Arc::new(AtomicUsize::new(0));

    let mut parent = runtime.new_actor::<Parent>();
    parent.mutate_on::<HireWorker>(move |actor, _ctx| {
        let config = ActorConfig::for_supervised_child("worker", actor.handle().clone(), None)
            .expect("a name plus a live parent is a valid child configuration")
            .with_restart_policy(RestartPolicy::Permanent)
            .with_restart_limiter(brisk_limiter(5));
        let _ = registered.send(actor.supervise_deferred(config, blueprint.clone()));
        Reply::ready()
    });
    let counter = Arc::clone(&noticed);
    parent.mutate_on::<ChildTerminated>(move |_actor, _ctx| {
        counter.fetch_add(1, Ordering::SeqCst);
        Reply::ready()
    });
    let parent = parent.start().await;

    let mut child = hire(&parent, &mut registrations).await;
    let first = tokio::time::timeout(PATIENCE, child.wait_running())
        .await
        .expect("the first start must land")?;
    first.stop().await?;

    // The engine restarted it...
    tokio::time::timeout(PATIENCE, child.wait_generation(RestartGeneration::FIRST.next()))
        .await
        .expect("the framework must still restart the child")?;
    // ...and the user's handler saw the same notification.
    assert_eq!(
        noticed.load(Ordering::SeqCst),
        1,
        "the engine's bookkeeping must not swallow a public message"
    );

    runtime.shutdown_all().await?;
    Ok(())
}

#[acton_test]
async fn a_supervisor_stopping_mid_backoff_settles_the_child_it_will_not_restart()
-> anyhow::Result<()> {
    // A caller blocked on a restart that can no longer happen must be told, not
    // left on `RestartPending`.
    //
    // This pins the *outcome* rather than the mechanism, and it cannot tell you
    // which mechanism delivered it: when the supervisor's task ends it drops the
    // status sender, so `wait_running` reports `SupervisorStopped` from the
    // channel closing whether or not the slot was settled first. The settling
    // itself is pinned where it is actually observable, by
    // `cancelling_settles_a_child_that_was_waiting_out_a_backoff` in the
    // registry's own tests. Measured: removing `AwaitingBackoff` from
    // `cancel_unfinished_starts` leaves this test green and that one red.
    let mut runtime = ActonApp::launch_async().await;
    let (registered, mut registrations) = channel();
    let builds = Arc::new(AtomicUsize::new(0));
    let parent = supervising_parent(
        &mut runtime,
        registered,
        counting_blueprint(&builds),
        Some(RestartLimiterConfig {
            initial_backoff_ms: 2_000,
            max_backoff_ms: 2_000,
            ..brisk_limiter(5)
        }),
    )
    .start()
    .await;

    let mut child = hire(&parent, &mut registrations).await;
    let first = tokio::time::timeout(PATIENCE, child.wait_running())
        .await
        .expect("the first start must land")?;
    first.stop().await?;

    tokio::time::timeout(PATIENCE, child.wait_for(|s| {
        s.state() == SupervisionState::RestartPending
    }))
    .await
    .expect("the child must be waiting out its backoff")?;

    parent.stop().await?;

    let error = tokio::time::timeout(PATIENCE, child.wait_running())
        .await
        .expect("a supervisor stopping mid-backoff must end the wait")
        .expect_err("the restart it was waiting for cannot happen now");
    assert!(
        matches!(error, SupervisionError::SupervisorStopped { .. }),
        "unexpected error: {error}"
    );

    runtime.shutdown_all().await?;
    Ok(())
}

#[acton_test]
async fn a_child_adopted_through_the_legacy_path_is_never_restarted() -> anyhow::Result<()> {
    // The double-restart firewall, end to end. A child adopted through
    // `supervise()` registers with no blueprint, so its supervisor has no
    // recipe for rebuilding it and never tries. That is what makes the restart
    // engine safe to ship into programs that hand-rolled their own restarts:
    // it can only reach children registered through `supervise_with` and
    // `supervise_deferred`, and neither has ever been released.
    //
    // # What this test is, honestly
    //
    // A characterization test, not a mutation-backed one, and the difference is
    // worth stating rather than leaving for someone to discover.
    //
    // The firewall has two layers. `evaluate` returns `Forget` for a
    // blueprint-less slot, and `begin_start` independently skips any slot with
    // no spawner. The second shadows the first, so disabling `evaluate`'s check
    // leaves end-to-end behaviour identical and **this test stays green**. It
    // therefore cannot be shown to catch that regression, and it is not
    // offered as if it could.
    //
    // What it does do is lock the user-visible promise, which no unit test
    // states: adopt a child the old way, kill it, and nothing brings it back.
    // The layer that *is* mutation-backed is
    // `a_child_with_no_blueprint_is_left_down_without_spending_an_allowance`,
    // in the engine's own tests — and note that it asserts the untouched
    // **limiter**, because with `evaluate`'s check disabled the slot still
    // reaches `Down`; only the wasted restart charge gives the bug away.
    let mut runtime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;

    // Built through `for_supervised_child` deliberately. `ChildTerminated` goes
    // to the child's *configured* parent, not to whoever called `supervise()`,
    // so a child configured without one would never reach the engine at all and
    // this test would prove nothing.
    let config = ActorConfig::for_supervised_child("legacy", parent.clone(), None)
        .expect("a name plus a live parent is a valid child configuration")
        .with_restart_policy(RestartPolicy::Permanent);
    let child_id = config.id();

    let builds = Arc::new(AtomicUsize::new(0));
    let mut child = runtime.new_actor_with_config::<Worker>(config);
    {
        let builds = Arc::clone(&builds);
        child.before_start(move |_actor| {
            let builds = Arc::clone(&builds);
            async move {
                builds.fetch_add(1, Ordering::SeqCst);
            }
        });
    }
    child.mutate_on::<Ping>(|_actor, _ctx| Reply::ready());

    let child_handle = parent.supervise(child).await?;
    assert_eq!(builds.load(Ordering::SeqCst), 1, "the child came up once");
    assert_eq!(child_handle.id(), child_id);

    // `Permanent` plus a normal termination is precisely the combination that
    // would warrant a restart for a child with a blueprint.
    child_handle.stop().await?;

    // Give the engine every opportunity to do the thing it must not do. The
    // default backoff is 100ms, so this is comfortably longer than a restart
    // would have taken.
    tokio::time::sleep(Duration::from_millis(400)).await;

    assert_eq!(
        builds.load(Ordering::SeqCst),
        1,
        "nothing rebuilt a child the supervisor has no blueprint for"
    );

    // And the supervisor is still healthy rather than stuck on a decision it
    // could not carry out.
    tokio::time::timeout(PATIENCE, parent.send(Ping))
        .await
        .expect("the supervisor is still taking messages");

    runtime.shutdown_all().await?;
    Ok(())
}
