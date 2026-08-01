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

//! A child stopped by its parent's shutdown reports `ParentShutdown`, not
//! `Normal`.
//!
//! Both stops reach a child as a message, and until they were told apart both
//! recorded `TerminationReason::Normal`. That is the reason
//! [`RestartPolicy::should_restart`] consumes, and `Permanent` warrants a
//! restart on a `Normal` termination — so a supervisor shutting down would ask
//! for a restart of every child it was in the middle of stopping. Nothing
//! restarts automatically yet, which is the only thing keeping that latent.
//!
//! # Why the observer is a third actor
//!
//! A child reports termination to its configured parent, and a shutting-down
//! parent has already closed its inbox by the time it stops its children, so a
//! notification sent back to it is dropped. The reason therefore cannot be read
//! from the parent performing the cascade.
//!
//! The two relationships are separate, which is what makes this observable: the
//! notification target comes from `ActorConfig`, while the set of children to
//! cascade to comes from the supervisor's registry and `handle.children()`. So
//! each child below is configured to report to a live observer while a
//! different actor supervises it. The cascade under test is the real one.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Collects the termination notifications the test wants to inspect.
///
/// A `Mutex` here is test scaffolding for reading actor state from the test
/// body, not shared domain state, matching `panic_termination_tests`.
type Captured = Arc<Mutex<Vec<ChildTerminated>>>;

#[acton_actor]
struct Observer {
    seen: Captured,
}

#[acton_actor]
struct Supervisor;

#[acton_actor]
struct Worker;

/// Starts an actor that records every `ChildTerminated` it is sent.
async fn start_observer(runtime: &mut ActorRuntime, seen: &Captured) -> ActorHandle {
    let mut builder = runtime.new_actor::<Observer>();
    builder.model.seen = Arc::clone(seen);
    builder.mutate_on::<ChildTerminated>(|actor, ctx| {
        let seen = Arc::clone(&actor.model.seen);
        let notification = ctx.message().clone();
        Box::pin(async move {
            seen.lock()
                .expect("capture mutex poisoned")
                .push(notification);
        })
    });
    builder.start().await
}

/// Builds a worker that reports its termination to `observer`.
fn worker_reporting_to(
    runtime: &mut ActorRuntime,
    observer: &ActorHandle,
    name: &str,
) -> anyhow::Result<ManagedActor<Idle, Worker>> {
    let config = ActorConfig::for_supervised_child(name, observer.clone(), None)?;
    Ok(runtime.new_actor_with_config::<Worker>(config))
}

/// Polls until a notification for `child` arrives, so a passing test is fast and
/// a failing one is still decisive.
async fn await_reason(seen: &Captured, child: &Ern) -> TerminationReason {
    for _ in 0..200 {
        let found = seen
            .lock()
            .expect("capture mutex poisoned")
            .iter()
            .find(|notification| &notification.child_id == child)
            .map(|notification| notification.reason.clone());
        if let Some(reason) = found {
            return reason;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no ChildTerminated notification ever arrived for {child}");
}

/// A child stopped because its supervisor is shutting down reports
/// `ParentShutdown`.
#[acton_test]
async fn a_child_stopped_by_its_parents_shutdown_reports_parent_shutdown() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let seen: Captured = Arc::new(Mutex::new(Vec::new()));
    let observer = start_observer(&mut runtime, &seen).await;
    let supervisor = runtime.new_actor::<Supervisor>().start().await;

    let worker = worker_reporting_to(&mut runtime, &observer, "cascade-worker")?;
    let worker_handle = supervisor.supervise(worker).await?;

    // Let the registration reach the supervisor's task.
    tokio::time::sleep(Duration::from_millis(50)).await;

    supervisor.stop().await?;

    assert_eq!(
        await_reason(&seen, &worker_handle.id()).await,
        TerminationReason::ParentShutdown,
        "a child stopped by its parent's shutdown must not record a reason a \
         restart policy would act on"
    );

    Ok(())
}

/// A child stopped directly by a caller still reports `Normal`.
///
/// The regression guard that gives the test above its meaning. Marking every
/// stop as `ParentShutdown` would satisfy that test and silently suppress every
/// legitimate restart, so this child is built and supervised exactly like the
/// one above and differs only in who stops it.
#[acton_test]
async fn a_child_stopped_directly_still_reports_normal() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let seen: Captured = Arc::new(Mutex::new(Vec::new()));
    let observer = start_observer(&mut runtime, &seen).await;
    let supervisor = runtime.new_actor::<Supervisor>().start().await;

    let worker = worker_reporting_to(&mut runtime, &observer, "directly-stopped-worker")?;
    let worker_handle = supervisor.supervise(worker).await?;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // The supervisor stays up: this is a user stopping one child, which a
    // `Permanent` policy legitimately restarts from.
    worker_handle.stop().await?;

    assert_eq!(
        await_reason(&seen, &worker_handle.id()).await,
        TerminationReason::Normal,
        "a directly stopped child must still report Normal, or every restart \
         policy is silently suppressed"
    );

    runtime.shutdown_all().await?;
    Ok(())
}

/// Cascading shutdown marks every generation, not just the first.
///
/// The grandchild is stopped by a parent that was itself stopped by a cascade
/// rather than by a caller, so it covers the case where the reason has to
/// survive being passed down a level.
#[acton_test]
async fn a_cascade_marks_every_generation_as_parent_shutdown() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let seen: Captured = Arc::new(Mutex::new(Vec::new()));
    let observer = start_observer(&mut runtime, &seen).await;
    let supervisor = runtime.new_actor::<Supervisor>().start().await;

    let middle = worker_reporting_to(&mut runtime, &observer, "middle-worker")?;
    let middle_handle = supervisor.supervise(middle).await?;

    let leaf = worker_reporting_to(&mut runtime, &observer, "leaf-worker")?;
    let leaf_handle = middle_handle.supervise(leaf).await?;

    // Let both registrations reach their supervisors' tasks.
    tokio::time::sleep(Duration::from_millis(100)).await;

    supervisor.stop().await?;

    assert_eq!(
        await_reason(&seen, &middle_handle.id()).await,
        TerminationReason::ParentShutdown,
        "the directly cascaded child should report ParentShutdown"
    );
    assert_eq!(
        await_reason(&seen, &leaf_handle.id()).await,
        TerminationReason::ParentShutdown,
        "the grandchild should report ParentShutdown too: a cascade that only \
         marks its first generation still restarts everything below it"
    );

    Ok(())
}

/// The consequence, pinned directly: a `Permanent` child terminated by its
/// parent's shutdown is not restarted.
///
/// The label is only a proxy. This asserts the decision the restart engine will
/// actually make, using the reason observed from a real cascade rather than one
/// constructed by hand.
#[acton_test]
async fn a_permanent_child_is_not_restarted_after_a_parent_shutdown() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let seen: Captured = Arc::new(Mutex::new(Vec::new()));
    let observer = start_observer(&mut runtime, &seen).await;
    let supervisor = runtime.new_actor::<Supervisor>().start().await;

    let worker = worker_reporting_to(&mut runtime, &observer, "permanent-worker")?;
    let worker_handle = supervisor.supervise(worker).await?;

    tokio::time::sleep(Duration::from_millis(50)).await;

    supervisor.stop().await?;

    let reason = await_reason(&seen, &worker_handle.id()).await;
    assert!(
        !RestartPolicy::Permanent.should_restart(&reason),
        "a Permanent child terminated by its parent's shutdown was reported as \
         {reason:?}, which should_restart accepts: a supervisor shutting down \
         would restart the very children it is stopping"
    );

    Ok(())
}
