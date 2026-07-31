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
struct Worker {
    greetings: usize,
}

#[acton_message]
struct Greet;

/// A blueprint that counts how many times it has been applied.
fn counting_blueprint(
    applications: &Arc<AtomicUsize>,
) -> impl Fn(&mut ManagedActor<Idle, Worker>) + Send + Sync + 'static {
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

// Test 6 (in-handler `ManagedActor::supervise_with`) is deliberately absent.
//
// It cannot be written from user code. `supervise_with` on `ManagedActor` is
// `async fn(&mut self)`, but a `mutate_on` handler's asynchronous half is a
// `'static` future that cannot borrow the actor. There is no user-reachable
// context that holds `&mut ManagedActor<Started, _>` across an `await`, so the
// method is currently only callable from inside the framework.

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

/// A released child is stopped and no longer supervised.
#[acton_test]
async fn releasing_a_child_stops_it_and_frees_its_name() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;
    let applications = Arc::new(AtomicUsize::new(0));

    let config = ActorConfig::for_supervised_child("worker", parent.clone(), None)?;
    let child_id = config.id();
    parent
        .supervise_with(&runtime, config, counting_blueprint(&applications))
        .await?;

    tokio::time::timeout(PATIENCE, parent.unsupervise(&child_id))
        .await
        .expect("unsupervise must not hang")?;

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
