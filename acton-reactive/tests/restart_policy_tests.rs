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

//! Integration tests for restart policy configuration and child termination notifications.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use acton_reactive::prelude::*;
use tokio::time::Duration;

/// A message for normal operations.
#[acton_message]
struct Ping;

/// Test actor state.
#[derive(Default, Debug)]
struct TestActor {
    receive_count: usize,
}

/// Parent actor state that tracks child terminations.
#[derive(Default, Debug)]
struct SupervisorActor {
    child_terminated_count: Arc<AtomicUsize>,
    should_restart: Arc<AtomicBool>,
}

/// Tests that the restart policy unit logic works correctly.
#[tokio::test]
async fn test_restart_policy_decision_logic() {
    // Permanent restarts on everything except parent shutdown
    assert!(RestartPolicy::Permanent.should_restart(&TerminationReason::Normal));
    assert!(RestartPolicy::Permanent.should_restart(&TerminationReason::Panic("test".into())));
    assert!(RestartPolicy::Permanent.should_restart(&TerminationReason::InboxClosed));
    assert!(!RestartPolicy::Permanent.should_restart(&TerminationReason::ParentShutdown));

    // Temporary never restarts
    assert!(!RestartPolicy::Temporary.should_restart(&TerminationReason::Normal));
    assert!(!RestartPolicy::Temporary.should_restart(&TerminationReason::Panic("test".into())));
    assert!(!RestartPolicy::Temporary.should_restart(&TerminationReason::InboxClosed));
    assert!(!RestartPolicy::Temporary.should_restart(&TerminationReason::ParentShutdown));

    // Transient only restarts on abnormal termination
    assert!(!RestartPolicy::Transient.should_restart(&TerminationReason::Normal));
    assert!(RestartPolicy::Transient.should_restart(&TerminationReason::Panic("test".into())));
    assert!(RestartPolicy::Transient.should_restart(&TerminationReason::InboxClosed));
    assert!(!RestartPolicy::Transient.should_restart(&TerminationReason::ParentShutdown));
}

/// Tests that a parent receives `ChildTerminated` message when a child stops.
#[tokio::test]
async fn test_parent_receives_child_terminated_notification() -> anyhow::Result<()> {
    let mut app = ActonApp::launch_async().await;

    let child_terminated_count = Arc::new(AtomicUsize::new(0));
    let should_restart = Arc::new(AtomicBool::new(false));

    let count_clone = child_terminated_count.clone();
    let restart_clone = should_restart.clone();

    // Create parent actor that handles ChildTerminated
    let parent_config = ActorConfig::new(
        Ern::with_root("supervisor")?,
        None,
        None,
    )?;

    let mut parent = app.new_actor_with_config::<SupervisorActor>(parent_config);
    parent.model.child_terminated_count = count_clone.clone();
    parent.model.should_restart = restart_clone.clone();

    parent.mutate_on::<ChildTerminated>(|actor, ctx| {
        let count = actor.model.child_terminated_count.clone();
        let should_restart_arc = actor.model.should_restart.clone();
        let reason = ctx.message().reason.clone();
        let policy = ctx.message().restart_policy;

        Box::pin(async move {
            count.fetch_add(1, Ordering::SeqCst);
            should_restart_arc.store(policy.should_restart(&reason), Ordering::SeqCst);
        })
    });

    let parent_handle = parent.start().await;

    // Create child actor with parent
    let child_config = ActorConfig::new(
        Ern::with_root("child")?,
        Some(parent_handle.clone()),
        None,
    )?;

    let mut child = app.new_actor_with_config::<TestActor>(child_config);
    child.mutate_on::<Ping>(|actor, _ctx| {
        actor.model.receive_count += 1;
        Reply::ready()
    });

    let child_handle = child.start().await;

    // Send a message to ensure child is running
    child_handle.send(Ping).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Stop the child gracefully
    child_handle.stop().await?;

    // Wait for the notification to be processed
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Parent should have received the ChildTerminated notification
    assert_eq!(
        child_terminated_count.load(Ordering::SeqCst),
        1,
        "Parent should receive exactly one ChildTerminated notification"
    );

    // Default policy is Permanent, and for Normal termination with Permanent policy,
    // it should restart
    assert!(
        should_restart.load(Ordering::SeqCst),
        "Permanent policy should restart on normal termination"
    );

    parent_handle.stop().await?;
    app.shutdown_all().await?;
    Ok(())
}

/// Tests that a child with Transient policy and normal shutdown does NOT trigger restart.
#[tokio::test]
async fn test_transient_policy_normal_shutdown_no_restart() -> anyhow::Result<()> {
    let mut app = ActonApp::launch_async().await;

    let should_restart = Arc::new(AtomicBool::new(true)); // Start as true to verify it becomes false
    let restart_clone = should_restart.clone();

    // Create parent actor
    let parent_config = ActorConfig::new(
        Ern::with_root("transient-supervisor")?,
        None,
        None,
    )?;

    let mut parent = app.new_actor_with_config::<SupervisorActor>(parent_config);
    parent.model.should_restart = restart_clone.clone();

    parent.mutate_on::<ChildTerminated>(|actor, ctx| {
        let should_restart_arc = actor.model.should_restart.clone();
        let reason = ctx.message().reason.clone();
        let policy = ctx.message().restart_policy;

        Box::pin(async move {
            should_restart_arc.store(policy.should_restart(&reason), Ordering::SeqCst);
        })
    });

    let parent_handle = parent.start().await;

    // Create child actor with Transient policy
    let child_config = ActorConfig::new(
        Ern::with_root("transient-child")?,
        Some(parent_handle.clone()),
        None,
    )?
    .with_restart_policy(RestartPolicy::Transient);

    let child = app.new_actor_with_config::<TestActor>(child_config);
    let child_handle = child.start().await;

    // Stop the child gracefully (Normal termination)
    child_handle.stop().await?;

    // Wait for notification
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Transient policy should NOT restart on Normal termination
    assert!(
        !should_restart.load(Ordering::SeqCst),
        "Transient policy should NOT restart on normal termination"
    );

    parent_handle.stop().await?;
    app.shutdown_all().await?;
    Ok(())
}

/// Tests that a child with Temporary policy never triggers restart.
#[tokio::test]
async fn test_temporary_policy_never_restarts() -> anyhow::Result<()> {
    let mut app = ActonApp::launch_async().await;

    let should_restart = Arc::new(AtomicBool::new(true)); // Start as true to verify it becomes false
    let restart_clone = should_restart.clone();

    // Create parent actor
    let parent_config = ActorConfig::new(
        Ern::with_root("temp-supervisor")?,
        None,
        None,
    )?;

    let mut parent = app.new_actor_with_config::<SupervisorActor>(parent_config);
    parent.model.should_restart = restart_clone.clone();

    parent.mutate_on::<ChildTerminated>(|actor, ctx| {
        let should_restart_arc = actor.model.should_restart.clone();
        let reason = ctx.message().reason.clone();
        let policy = ctx.message().restart_policy;

        Box::pin(async move {
            should_restart_arc.store(policy.should_restart(&reason), Ordering::SeqCst);
        })
    });

    let parent_handle = parent.start().await;

    // Create child with Temporary policy
    let child_config = ActorConfig::new(
        Ern::with_root("temp-child")?,
        Some(parent_handle.clone()),
        None,
    )?
    .with_restart_policy(RestartPolicy::Temporary);

    let child = app.new_actor_with_config::<TestActor>(child_config);
    let child_handle = child.start().await;

    // Stop the child
    child_handle.stop().await?;

    // Wait for notification
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Temporary policy should NEVER restart
    assert!(
        !should_restart.load(Ordering::SeqCst),
        "Temporary policy should NEVER restart"
    );

    parent_handle.stop().await?;
    app.shutdown_all().await?;
    Ok(())
}
