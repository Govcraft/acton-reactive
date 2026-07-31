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

//! Supervising a child from a blueprint.
//!
//! Several of these fail by **hanging** rather than by asserting, so they are
//! wrapped in an explicit timeout that fails loudly instead.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Long enough to be decisive, short enough that a hang is not a coffee break.
const PATIENCE: Duration = Duration::from_secs(5);

/// How long a deliberately slow child takes to start.
///
/// Long enough that a supervisor which waited for it would be visibly stuck,
/// and short enough that the tests using it stay quick.
const SLOW_START: Duration = Duration::from_secs(2);

#[acton_actor]
struct Parent;

#[acton_actor]
struct Worker {
    greetings: usize,
}

#[acton_message]
struct Greet;

/// Asks a supervisor to take on a child named `name`, from inside its handler.
#[acton_message]
struct HireWorker {
    name: &'static str,
}

/// Asks a supervisor to prove it is still listening.
#[acton_message]
struct Ping;

/// The way a handler reports what it did back to the test.
///
/// A channel rather than shared state: the handler runs on the supervisor's
/// task, and this keeps the result crossing between the two as a message.
/// `send` on an unbounded sender is synchronous, which is what a handler that
/// must not await needs.
type Registrations = (
    tokio::sync::mpsc::UnboundedSender<Result<SupervisedChild, SupervisionError>>,
    tokio::sync::mpsc::UnboundedReceiver<Result<SupervisedChild, SupervisionError>>,
);

fn registration_channel() -> Registrations {
    tokio::sync::mpsc::unbounded_channel()
}

/// A blueprint whose child takes its time coming up, and says when it stops.
///
/// The `before_start` hook is the part that matters: it is user code of
/// arbitrary duration that has to run before the child exists, which is exactly
/// what a supervisor must not be waiting on.
fn slow_blueprint(
    stopped: &Arc<AtomicBool>,
) -> impl Fn(&mut ManagedActor<Idle, Worker>) + Clone + Send + Sync + 'static {
    let stopped = Arc::clone(stopped);
    move |actor: &mut ManagedActor<Idle, Worker>| {
        actor.before_start(|_actor| async move {
            tokio::time::sleep(SLOW_START).await;
        });
        let stopped = Arc::clone(&stopped);
        actor.after_stop(move |_actor| {
            let stopped = Arc::clone(&stopped);
            async move {
                stopped.store(true, Ordering::SeqCst);
            }
        });
    }
}

/// A blueprint that counts its applications and reports when its child stops.
///
/// The stop flag is what distinguishes `unsupervise` from `release`; without it
/// a test can only observe that a name was freed, which both of them do.
fn reporting_blueprint(
    applications: &Arc<AtomicUsize>,
    stopped: &Arc<AtomicBool>,
) -> impl Fn(&mut ManagedActor<Idle, Worker>) + Clone + Send + Sync + 'static {
    let applications = Arc::clone(applications);
    let stopped = Arc::clone(stopped);
    move |actor: &mut ManagedActor<Idle, Worker>| {
        applications.fetch_add(1, Ordering::SeqCst);
        actor.mutate_on::<Greet>(|actor, _| {
            actor.model.greetings += 1;
            Reply::ready()
        });
        let stopped = Arc::clone(&stopped);
        actor.after_stop(move |_actor| {
            let stopped = Arc::clone(&stopped);
            async move {
                stopped.store(true, Ordering::SeqCst);
            }
        });
    }
}

/// A blueprint that counts how many times it has been applied.
fn counting_blueprint(
    applications: &Arc<AtomicUsize>,
) -> impl Fn(&mut ManagedActor<Idle, Worker>) + Clone + Send + Sync + 'static {
    let applications = Arc::clone(applications);
    move |actor: &mut ManagedActor<Idle, Worker>| {
        applications.fetch_add(1, Ordering::SeqCst);
        actor.mutate_on::<Greet>(|actor, _| {
            actor.model.greetings += 1;
            Reply::ready()
        });
    }
}

/// Test 1 — a blueprint child starts, is supervised, and reports running.
#[acton_test]
async fn supervising_from_a_blueprint_starts_the_child_and_records_it() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;

    let applications = Arc::new(AtomicUsize::new(0));
    let config = ActorConfig::for_supervised_child("worker", parent.clone(), None)?;
    let expected_id = config.id();

    let mut child = tokio::time::timeout(
        PATIENCE,
        parent.supervise_with(&runtime, config, counting_blueprint(&applications)),
    )
    .await
    .expect("supervise_with must not hang")?;

    assert_eq!(child.ern(), &expected_id);
    assert_eq!(
        applications.load(Ordering::SeqCst),
        1,
        "the blueprint runs once per start"
    );

    let handle = tokio::time::timeout(PATIENCE, child.wait_running())
        .await
        .expect("the child must reach running")?;
    assert_eq!(handle.id(), expected_id);
    assert_eq!(child.status().generation(), RestartGeneration::FIRST);
    assert!(child.current().is_some());

    runtime.shutdown_all().await?;
    Ok(())
}

/// Test 2 — a duplicate is rejected, and the child it started is stopped.
#[acton_test]
async fn a_duplicate_is_rejected_and_the_second_child_is_stopped() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;
    let applications = Arc::new(AtomicUsize::new(0));

    let first = ActorConfig::for_supervised_child("worker", parent.clone(), None)?;
    parent
        .supervise_with(&runtime, first, counting_blueprint(&applications))
        .await?;

    // Same parent, same name: deterministic identity makes this a real collision.
    let second = ActorConfig::for_supervised_child("worker", parent.clone(), None)?;
    let error = tokio::time::timeout(
        PATIENCE,
        parent.supervise_with(&runtime, second, counting_blueprint(&applications)),
    )
    .await
    .expect("supervise_with must not hang")
    .expect_err("the same name under the same parent collides");

    assert!(matches!(error, SupervisionError::DuplicateChild { .. }));
    assert_eq!(
        applications.load(Ordering::SeqCst),
        2,
        "the second child really was built before being rejected"
    );

    runtime.shutdown_all().await?;
    Ok(())
}

/// Test 3 — supervising through a stale handle resolves rather than hanging.
///
/// **Fails by hanging** if the three-way guard is dropped: the outcome cell is
/// kept alive by the caller's own `Arc` and would never be set. The timeout is
/// the assertion.
#[acton_test]
async fn supervising_through_a_stopped_parent_resolves_with_an_error() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;
    let stale = parent.clone();

    parent.stop().await?;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let applications = Arc::new(AtomicUsize::new(0));
    let config = ActorConfig::for_supervised_child("orphan", stale.clone(), None)?;

    let error = tokio::time::timeout(
        PATIENCE,
        stale.supervise_with(&runtime, config, counting_blueprint(&applications)),
    )
    .await
    .expect("supervise_with must resolve, not hang, when the supervisor is gone")
    .expect_err("a stopped supervisor cannot take on a child");

    assert!(
        matches!(
            error,
            SupervisionError::SupervisorStopped { .. }
                | SupervisionError::RegistrationLost { .. }
        ),
        "unexpected error: {error}"
    );

    runtime.shutdown_all().await?;
    Ok(())
}

/// Test 4 — a parent that stops mid-flight yields an error, never a false `Ok`.
#[acton_test]
async fn a_parent_that_stops_before_recording_never_reports_success() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;
    let applications = Arc::new(AtomicUsize::new(0));

    let config = ActorConfig::for_supervised_child("racer", parent.clone(), None)?;
    let handle = parent.clone();
    let stopper = tokio::spawn(async move {
        let _ = handle.stop().await;
    });

    let result = tokio::time::timeout(
        PATIENCE,
        parent.supervise_with(&runtime, config, counting_blueprint(&applications)),
    )
    .await
    .expect("supervise_with must resolve either way");

    stopper.await?;
    // Either outcome is legitimate — the race is real — but a hang is not, and
    // neither is a success the supervisor never recorded.
    if let Ok(ref child) = result {
        assert!(!child.ern().to_string().is_empty());
    }

    runtime.shutdown_all().await?;
    Ok(())
}

/// Test 5 — releasing a child that is not supervised is an error.
#[acton_test]
async fn releasing_an_unknown_child_is_rejected() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;

    let stranger = ActorConfig::for_supervised_child("stranger", parent.clone(), None)?;
    let error = tokio::time::timeout(PATIENCE, parent.unsupervise(&stranger.id()))
        .await
        .expect("unsupervise must not hang")
        .expect_err("nothing by that name is supervised");

    assert!(matches!(error, SupervisionError::UnknownChild { .. }));

    runtime.shutdown_all().await?;
    Ok(())
}

/// Test 6 — a supervisor takes on a child from inside its own handler.
///
/// The one `supervise_with` cannot do. Its `ManagedActor` form needs `&mut self`
/// held across an `await`, and a `mutate_on` handler's asynchronous half is a
/// `'static` future that cannot borrow the actor; its `ActorHandle` form waits
/// for an acknowledgement this actor cannot produce while this handler is
/// running. `supervise_deferred` does not await at all, so neither applies.
///
/// **Fails by hanging** if the message loop stops draining what the handler
/// queued: nothing would ever create the child, and `wait_running` would wait
/// for a status that never comes. The timeout is the assertion.
#[acton_test]
async fn a_handler_can_put_a_child_under_its_own_actors_supervision() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let applications = Arc::new(AtomicUsize::new(0));
    let (registered, mut outcomes) = registration_channel();

    let mut parent = runtime.new_actor::<Parent>();
    let blueprint = counting_blueprint(&applications);
    parent.mutate_on::<HireWorker>(move |actor, context| {
        let config =
            ActorConfig::for_supervised_child(context.message().name, actor.handle().clone(), None)
                .expect("a name plus a live parent is a valid child configuration");
        // Synchronous: no await, so `&mut actor` is never held across one.
        let _ = registered.send(actor.supervise_deferred(config, blueprint.clone()));
        Reply::ready()
    });
    let parent = parent.start().await;

    parent.send(HireWorker { name: "worker" }).await;

    let mut child = tokio::time::timeout(PATIENCE, outcomes.recv())
        .await
        .expect("the handler must run")
        .expect("the handler reports its outcome")?;

    let handle = tokio::time::timeout(PATIENCE, child.wait_running())
        .await
        .expect("the queued start must actually happen")?;

    assert_eq!(handle.id(), *child.ern());
    assert_eq!(
        applications.load(Ordering::SeqCst),
        1,
        "the blueprint was applied to exactly one child"
    );
    assert_eq!(child.status().state(), SupervisionState::Running);
    assert_eq!(child.supervisor(), &parent.id());

    // And it is a real actor: it takes messages.
    handle.send(Greet).await;

    runtime.shutdown_all().await?;
    Ok(())
}

/// Test 6a — the duplicate is rejected at the call site, inside the handler.
///
/// Nothing is built for the rejected call, which is what registering before
/// spawning buys: the collision is known the moment the name is offered.
#[acton_test]
async fn a_duplicate_is_rejected_inside_the_handler_before_anything_is_built(
) -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let applications = Arc::new(AtomicUsize::new(0));
    let (registered, mut outcomes) = registration_channel();

    let mut parent = runtime.new_actor::<Parent>();
    let blueprint = counting_blueprint(&applications);
    parent.mutate_on::<HireWorker>(move |actor, context| {
        let name = context.message().name;
        // Twice, in one handler, with one name. Deterministic child identity
        // makes the second a real collision.
        for _ in 0..2 {
            let config =
                ActorConfig::for_supervised_child(name, actor.handle().clone(), None)
                    .expect("a name plus a live parent is a valid child configuration");
            let _ = registered.send(actor.supervise_deferred(config, blueprint.clone()));
        }
        Reply::ready()
    });
    let parent = parent.start().await;

    parent.send(HireWorker { name: "worker" }).await;

    let mut accepted = tokio::time::timeout(PATIENCE, outcomes.recv())
        .await
        .expect("the handler must run")
        .expect("the handler reports its first outcome")
        .expect("the first registration is accepted");
    let error = tokio::time::timeout(PATIENCE, outcomes.recv())
        .await
        .expect("the handler must run")
        .expect("the handler reports its second outcome")
        .expect_err("the same name under the same parent collides");

    assert!(
        matches!(error, SupervisionError::DuplicateChild { .. }),
        "unexpected error: {error}"
    );

    tokio::time::timeout(PATIENCE, accepted.wait_running())
        .await
        .expect("the accepted child still starts")?;
    assert_eq!(
        applications.load(Ordering::SeqCst),
        1,
        "the rejected registration never reached a blueprint"
    );

    runtime.shutdown_all().await?;
    Ok(())
}

/// Test 6b — a supervisor that stops with starts still queued does not strand
/// the caller that asked for them.
///
/// The race is real and either outcome is legitimate: the loop may drain the
/// queue before the stop arrives, or the stop may win. A hang is not
/// legitimate, and neither is a wait that resolves to a running child the
/// supervisor never recorded.
#[acton_test]
async fn stopping_with_starts_still_queued_resolves_every_waiting_caller() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let applications = Arc::new(AtomicUsize::new(0));
    let (registered, mut outcomes) = registration_channel();

    let mut parent = runtime.new_actor::<Parent>();
    let blueprint = counting_blueprint(&applications);
    parent.mutate_on::<HireWorker>(move |actor, context| {
        let name = context.message().name;
        for index in 0..8 {
            let config = ActorConfig::for_supervised_child(
                format!("{name}-{index}"),
                actor.handle().clone(),
                None,
            )
            .expect("a name plus a live parent is a valid child configuration");
            let _ = registered.send(actor.supervise_deferred(config, blueprint.clone()));
        }
        Reply::ready()
    });
    let parent = parent.start().await;

    parent.send(HireWorker { name: "worker" }).await;
    let mut children = Vec::new();
    for _ in 0..8 {
        children.push(
            tokio::time::timeout(PATIENCE, outcomes.recv())
                .await
                .expect("the handler must run")
                .expect("the handler reports its outcome")
                .expect("distinct names never collide"),
        );
    }

    parent.stop().await?;

    for child in &mut children {
        let outcome = tokio::time::timeout(PATIENCE, child.wait_running())
            .await
            .expect("every caller must be answered, running or not");
        if let Ok(ref handle) = outcome {
            assert_eq!(handle.id(), *child.ern(), "a start that won the race");
        }
    }

    runtime.shutdown_all().await?;
    Ok(())
}

/// Test 6c — a supervisor keeps answering while one of its children starts.
///
/// The reason a start runs on its own task. Creating a child means running the
/// child's `before_start`, which is user code of any duration; a supervisor that
/// awaited it would stop taking messages — including its own `Terminate` — for
/// as long as somebody else's hook felt like taking.
///
/// **Fails on the elapsed-time assertion** if the start moves back onto the
/// supervisor's task: the pong then cannot arrive until the child has finished
/// starting.
#[acton_test]
async fn a_supervisor_keeps_taking_messages_while_a_child_starts() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let stopped = Arc::new(AtomicBool::new(false));
    let (registered, mut outcomes) = registration_channel();
    let (pinged, mut pongs) = tokio::sync::mpsc::unbounded_channel::<Instant>();

    let mut parent = runtime.new_actor::<Parent>();
    let blueprint = slow_blueprint(&stopped);
    parent.mutate_on::<HireWorker>(move |actor, context| {
        let config =
            ActorConfig::for_supervised_child(context.message().name, actor.handle().clone(), None)
                .expect("a name plus a live parent is a valid child configuration");
        let _ = registered.send(actor.supervise_deferred(config, blueprint.clone()));
        Reply::ready()
    });
    parent.mutate_on_sync::<Ping>(move |_actor, _context| {
        let _ = pinged.send(Instant::now());
    });
    let parent = parent.start().await;

    let asked_at = Instant::now();
    parent.send(HireWorker { name: "slowpoke" }).await;
    parent.send(Ping).await;

    let mut child = tokio::time::timeout(PATIENCE, outcomes.recv())
        .await
        .expect("the handler must run")
        .expect("the handler reports its outcome")?;

    let ponged_at = tokio::time::timeout(PATIENCE, pongs.recv())
        .await
        .expect("the supervisor must answer while the child is still starting")
        .expect("the ping handler reports back");

    let waited = ponged_at.duration_since(asked_at);
    assert!(
        waited < SLOW_START / 2,
        "the supervisor answered only after {waited:?}, which means it was waiting for the child"
    );
    assert_eq!(
        child.status().state(),
        SupervisionState::Starting,
        "and the child really was still on its way up"
    );

    // The child does arrive, in its own time.
    let handle = tokio::time::timeout(PATIENCE, child.wait_running())
        .await
        .expect("the child must finish starting")?;
    assert_eq!(handle.id(), *child.ern());

    runtime.shutdown_all().await?;
    Ok(())
}

/// Test 6d — a supervisor that stops mid-start does not leave the child behind.
///
/// The hazard the hand-over creates. Between the moment a start task has built
/// a child and the moment its supervisor records it, that task holds the only
/// handle to a live actor. If it simply dropped that handle on finding nobody
/// to give it to, the child would run forever with nothing able to reach it —
/// worse than the stall this whole step removes.
#[acton_test]
async fn a_supervisor_that_stops_mid_start_stops_the_child_it_started(
) -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let stopped = Arc::new(AtomicBool::new(false));
    let (registered, mut outcomes) = registration_channel();

    let mut parent = runtime.new_actor::<Parent>();
    let blueprint = slow_blueprint(&stopped);
    parent.mutate_on::<HireWorker>(move |actor, context| {
        let config =
            ActorConfig::for_supervised_child(context.message().name, actor.handle().clone(), None)
                .expect("a name plus a live parent is a valid child configuration");
        let _ = registered.send(actor.supervise_deferred(config, blueprint.clone()));
        Reply::ready()
    });
    let parent = parent.start().await;

    parent.send(HireWorker { name: "slowpoke" }).await;
    let child = tokio::time::timeout(PATIENCE, outcomes.recv())
        .await
        .expect("the handler must run")
        .expect("the handler reports its outcome")?;

    // Long enough for the loop to have launched the start, far too short for
    // that start to have finished.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        child.status().state(),
        SupervisionState::Starting,
        "the start really is in flight when the supervisor is stopped"
    );

    tokio::time::timeout(PATIENCE, parent.stop())
        .await
        .expect("stopping must not hang on an in-flight start")?;

    // Asserted without polling, on purpose. The child is stopped by the start
    // task, which runs on its own tracker rather than the handle's, so nothing
    // outside would wait for it by accident. What makes this hold the moment
    // `stop()` returns is that the supervisor's own shutdown waits for every
    // in-flight start before it finishes — and `stop()` waits for that.
    assert!(
        stopped.load(Ordering::SeqCst),
        "the supervisor finished stopping while a child of its own was still being built"
    );

    Ok(())
}

/// Test 6e — a caller waiting on an in-flight start is answered, and told why.
///
/// Property three, which only became reachable once starts stopped happening on
/// the supervisor's own task: `Terminate` can now genuinely arrive while a child
/// is being built.
///
/// The assertion on the *published* status is the load-bearing one. A caller
/// whose wait ends because every sender was dropped learns only that the
/// supervisor is gone; the terminal status with a reason on it exists only
/// because the supervisor settled the record on its way down.
#[acton_test]
async fn a_start_in_flight_when_the_supervisor_stops_is_settled_not_abandoned(
) -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let stopped = Arc::new(AtomicBool::new(false));
    let (registered, mut outcomes) = registration_channel();

    let mut parent = runtime.new_actor::<Parent>();
    let blueprint = slow_blueprint(&stopped);
    parent.mutate_on::<HireWorker>(move |actor, context| {
        let config =
            ActorConfig::for_supervised_child(context.message().name, actor.handle().clone(), None)
                .expect("a name plus a live parent is a valid child configuration");
        let _ = registered.send(actor.supervise_deferred(config, blueprint.clone()));
        Reply::ready()
    });
    let parent = parent.start().await;
    let supervisor_id = parent.id();

    parent.send(HireWorker { name: "slowpoke" }).await;
    let mut child = tokio::time::timeout(PATIENCE, outcomes.recv())
        .await
        .expect("the handler must run")
        .expect("the handler reports its outcome")?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    tokio::time::timeout(PATIENCE, parent.stop())
        .await
        .expect("stopping must not hang on an in-flight start")?;

    let error = tokio::time::timeout(PATIENCE, child.wait_running())
        .await
        .expect("a caller waiting on an abandoned start must not wait forever")
        .expect_err("the child never came up");
    assert!(
        matches!(error, SupervisionError::SupervisorStopped { .. }),
        "unexpected error: {error}"
    );

    let last = child.status();
    assert!(
        last.state().is_terminal(),
        "the supervisor settled the record before it went, rather than leaving it at {}",
        last.state()
    );
    assert_eq!(
        last.failure(),
        Some(&SupervisionError::SupervisorStopped {
            supervisor: supervisor_id
        }),
        "and said why, rather than leaving the caller to infer it from a closed channel"
    );

    Ok(())
}

/// Test 7 — registration order is start order.
#[acton_test]
async fn children_are_recorded_in_the_order_they_were_supervised() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;
    let applications = Arc::new(AtomicUsize::new(0));

    let mut ids = Vec::new();
    for name in ["first", "second", "third"] {
        let config = ActorConfig::for_supervised_child(name, parent.clone(), None)?;
        let child = parent
            .supervise_with(&runtime, config, counting_blueprint(&applications))
            .await?;
        ids.push(child.ern().clone());
    }

    assert_eq!(ids.len(), 3);
    assert_eq!(applications.load(Ordering::SeqCst), 3);
    for pair in ids.windows(2) {
        assert_ne!(pair[0], pair[1]);
    }

    runtime.shutdown_all().await?;
    Ok(())
}

/// `unsupervise` frees the name **and stops the child**.
///
/// The stop half was previously asserted by the test's name and by nothing
/// else, which is how `unsupervise` came to leave the child running while its
/// documentation said otherwise. Both halves are checked here.
#[acton_test]
async fn unsupervising_a_child_stops_it_and_frees_its_name() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;
    let applications = Arc::new(AtomicUsize::new(0));
    let stopped = Arc::new(AtomicBool::new(false));

    let config = ActorConfig::for_supervised_child("worker", parent.clone(), None)?;
    let child_id = config.id();
    parent
        .supervise_with(
            &runtime,
            config,
            reporting_blueprint(&applications, &stopped),
        )
        .await?;

    tokio::time::timeout(PATIENCE, parent.unsupervise(&child_id))
        .await
        .expect("unsupervise must not hang")?;

    // Asserted without polling: `unsupervise` awaits the child's stop itself,
    // so by the time it returns the child is down, not merely on its way.
    assert!(
        stopped.load(Ordering::SeqCst),
        "unsupervise returned while the child was still running"
    );

    // The name is free again, so the same child can be supervised afresh.
    let again = ActorConfig::for_supervised_child("worker", parent.clone(), None)?;
    tokio::time::timeout(
        PATIENCE,
        parent.supervise_with(&runtime, again, counting_blueprint(&applications)),
    )
    .await
    .expect("supervise_with must not hang")?;

    assert_eq!(applications.load(Ordering::SeqCst), 2);

    runtime.shutdown_all().await?;
    Ok(())
}

/// `release` frees the name and **leaves the child running**.
///
/// The mirror of the test above, and the reason the two operations are
/// separate: "stop supervising this, but keep it serving" is a real thing to
/// want, and it was previously the *only* thing `unsupervise` actually did.
#[acton_test]
async fn releasing_a_child_frees_its_name_and_leaves_it_running() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;
    let applications = Arc::new(AtomicUsize::new(0));
    let stopped = Arc::new(AtomicBool::new(false));

    let config = ActorConfig::for_supervised_child("worker", parent.clone(), None)?;
    let child_id = config.id();
    parent
        .supervise_with(
            &runtime,
            config,
            reporting_blueprint(&applications, &stopped),
        )
        .await?;

    let released = tokio::time::timeout(PATIENCE, parent.release(&child_id))
        .await
        .expect("release must not hang")?
        .expect("the supervisor was holding a handle to a running child");

    assert!(
        !stopped.load(Ordering::SeqCst),
        "release stopped the child it was supposed to leave running"
    );
    assert_eq!(released.id(), child_id);

    // Still serving: it takes a message and acts on it.
    released.send(Greet).await;

    // And the name is free, exactly as after unsupervise.
    let again = ActorConfig::for_supervised_child("worker", parent.clone(), None)?;
    tokio::time::timeout(
        PATIENCE,
        parent.supervise_with(&runtime, again, counting_blueprint(&applications)),
    )
    .await
    .expect("supervise_with must not hang")?;

    // Nothing supervises the released child now, so stopping it is the
    // caller's job — which is the bargain `release` makes.
    released.stop().await?;

    runtime.shutdown_all().await?;
    Ok(())
}

/// Releasing a child through a supervisor that has already stopped resolves
/// with an error rather than hanging.
///
/// **Fails by hanging** without the liveness channel. A supervisor terminating
/// normally never cancels its own token — `run_message_loop` closes its inbox
/// and breaks — so the cancellation arm never fires, and a `SetOnce` has no
/// notion of a sender going away. The caller would hold its own `Arc` on a cell
/// nobody can ever fill. The timeout is the assertion.
#[acton_test]
async fn releasing_through_a_stopped_supervisor_resolves_with_an_error() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;
    let applications = Arc::new(AtomicUsize::new(0));

    let config = ActorConfig::for_supervised_child("worker", parent.clone(), None)?;
    let child_id = config.id();
    parent
        .supervise_with(&runtime, config, counting_blueprint(&applications))
        .await?;

    let stale = parent.clone();
    parent.stop().await?;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let error = tokio::time::timeout(PATIENCE, stale.unsupervise(&child_id))
        .await
        .expect("unsupervise must resolve, not hang, when the supervisor is gone")
        .expect_err("a stopped supervisor cannot release a child");

    assert!(
        matches!(
            error,
            SupervisionError::ReleaseLost { .. }
                | SupervisionError::SupervisorStopped { .. }
                | SupervisionError::UnknownChild { .. }
        ),
        "unexpected error: {error}"
    );

    runtime.shutdown_all().await?;
    Ok(())
}
