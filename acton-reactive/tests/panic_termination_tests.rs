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

//! Integration tests for panic-driven actor termination.
//!
//! These tests verify that when the `catch-handler-panics` feature is DISABLED,
//! a panicking message handler terminates the actor cleanly and notifies its
//! parent with `ChildTerminated { reason: TerminationReason::Panic(..), .. }`.
//!
//! Run with: `cargo nextest run -p acton-reactive --no-default-features`
#![cfg(not(feature = "catch-handler-panics"))]
//!
//! Note: These tests use `#[tokio::test]` instead of `#[acton_test]` because
//! the `acton_test` macro's panic detection would fail the test when we
//! intentionally trigger panics to verify the termination contract.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use acton_reactive::prelude::*;
use tokio::time::Duration;

/// Polls the captured notification slot until it is set, failing after a
/// generous deadline so the tests stay deterministic on loaded runners.
async fn await_notification(
    captured: &Arc<Mutex<Option<ChildTerminated>>>,
) -> ChildTerminated {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let current = captured
            .lock()
            .expect("notification mutex poisoned")
            .clone();
        if let Some(notification) = current {
            return notification;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Timed out waiting for the ChildTerminated notification"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Message whose handler panics with a `&'static str` payload.
#[acton_message]
struct Boom;

/// Message whose handler panics with a formatted `String` payload.
#[acton_message]
struct BoomWithMessage {
    message: String,
}

/// Child actor state whose handlers panic on demand.
#[derive(Default, Debug)]
struct PanickingChild;

/// Parent actor state that records the last `ChildTerminated` notification.
#[derive(Default, Debug)]
struct SupervisorActor {
    last_notification: Arc<Mutex<Option<ChildTerminated>>>,
}

/// Starts a supervisor actor that captures `ChildTerminated` notifications.
async fn start_supervisor(
    app: &mut ActorRuntime,
    name: &str,
    captured: Arc<Mutex<Option<ChildTerminated>>>,
) -> anyhow::Result<ActorHandle> {
    let parent_config = ActorConfig::new(Ern::with_root(name)?, None, None)?;

    let mut parent = app.new_actor_with_config::<SupervisorActor>(parent_config);
    parent.model.last_notification = captured;

    parent.mutate_on::<ChildTerminated>(|actor, ctx| {
        let captured = actor.model.last_notification.clone();
        let notification = ctx.message().clone();

        Box::pin(async move {
            *captured.lock().expect("notification mutex poisoned") = Some(notification);
        })
    });

    Ok(parent.start().await)
}

/// Tests that a panicking handler terminates the child and the parent receives
/// `ChildTerminated` with a `Panic` reason carrying the panic message.
#[tokio::test]
async fn test_parent_notified_of_panic_termination() -> anyhow::Result<()> {
    let mut app = ActonApp::launch_async().await;

    let captured = Arc::new(Mutex::new(None));
    let parent_handle =
        start_supervisor(&mut app, "panic-supervisor", captured.clone()).await?;

    // Create child actor whose Boom handler panics with a &str payload
    let child_config = ActorConfig::new(
        Ern::with_root("panicking-child")?,
        Some(parent_handle.clone()),
        None,
    )?;

    let mut child = app.new_actor_with_config::<PanickingChild>(child_config);
    child.mutate_on::<Boom>(|_actor, _ctx| {
        panic!("intentional test panic");
    });

    let child_handle = child.start().await;

    // Trigger the panic and wait for the termination notification
    child_handle.send(Boom).await;
    let notification = await_notification(&captured).await;

    assert_eq!(
        notification.reason,
        TerminationReason::Panic("intentional test panic".to_string()),
        "Termination reason should be Panic carrying the panic message"
    );
    assert_eq!(
        notification.child_id,
        child_handle.id(),
        "Notification should identify the panicked child"
    );

    parent_handle.stop().await?;
    app.shutdown_all().await?;
    Ok(())
}

/// Tests that a formatted (`String`) panic payload is preserved in the reason
/// and that a Transient restart policy treats the panic as restart-worthy.
#[tokio::test]
async fn test_panic_reason_preserves_string_payload_and_policy() -> anyhow::Result<()> {
    let mut app = ActonApp::launch_async().await;

    let captured = Arc::new(Mutex::new(None));
    let parent_handle =
        start_supervisor(&mut app, "transient-panic-supervisor", captured.clone()).await?;

    // Create child with Transient policy whose handler panics with a String payload
    let child_config = ActorConfig::new(
        Ern::with_root("transient-panicking-child")?,
        Some(parent_handle.clone()),
        None,
    )?
    .with_restart_policy(RestartPolicy::Transient);

    let mut child = app.new_actor_with_config::<PanickingChild>(child_config);
    child.mutate_on::<BoomWithMessage>(|_actor, ctx| {
        let msg = ctx.message().message.clone();
        panic!("worker failed: {msg}");
    });

    let child_handle = child.start().await;

    // Trigger the panic and wait for the termination notification
    child_handle
        .send(BoomWithMessage {
            message: "disk full".to_string(),
        })
        .await;
    let notification = await_notification(&captured).await;

    assert_eq!(
        notification.reason,
        TerminationReason::Panic("worker failed: disk full".to_string()),
        "Formatted panic message should be preserved in the termination reason"
    );
    assert_eq!(
        notification.restart_policy,
        RestartPolicy::Transient,
        "Notification should carry the child's configured restart policy"
    );
    assert!(
        notification
            .restart_policy
            .should_restart(&notification.reason),
        "Transient policy should restart on panic termination"
    );

    parent_handle.stop().await?;
    app.shutdown_all().await?;
    Ok(())
}

/// Tests that the parent notification survives a second panic during shutdown
/// cleanup: when the `after_stop` hook panics after the handler already
/// panicked, `ChildTerminated` still arrives and preserves the FIRST panic's
/// message.
#[tokio::test]
async fn test_notification_survives_after_stop_panic() -> anyhow::Result<()> {
    let mut app = ActonApp::launch_async().await;

    let captured = Arc::new(Mutex::new(None));
    let parent_handle =
        start_supervisor(&mut app, "cleanup-panic-supervisor", captured.clone()).await?;

    let child_config = ActorConfig::new(
        Ern::with_root("double-panicking-child")?,
        Some(parent_handle.clone()),
        None,
    )?;

    let mut child = app.new_actor_with_config::<PanickingChild>(child_config);
    child
        .mutate_on::<Boom>(|_actor, _ctx| {
            panic!("original handler panic");
        })
        .after_stop(|_actor| {
            Reply::pending(async move {
                panic!("panic during shutdown cleanup");
            })
        });

    let child_handle = child.start().await;
    child_handle.send(Boom).await;

    let notification = await_notification(&captured).await;
    assert_eq!(
        notification.reason,
        TerminationReason::Panic("original handler panic".to_string()),
        "The first panic's message must be preserved even when after_stop panics too"
    );

    parent_handle.stop().await?;
    app.shutdown_all().await?;
    Ok(())
}

/// Tests that a panic in `after_stop` during an otherwise clean termination is
/// itself reported as `Panic`, rather than being swallowed.
#[tokio::test]
async fn test_after_stop_panic_reported_on_clean_termination() -> anyhow::Result<()> {
    let mut app = ActonApp::launch_async().await;

    let captured = Arc::new(Mutex::new(None));
    let parent_handle =
        start_supervisor(&mut app, "cleanup-only-panic-supervisor", captured.clone()).await?;

    let child_config = ActorConfig::new(
        Ern::with_root("cleanup-panicking-child")?,
        Some(parent_handle.clone()),
        None,
    )?;

    let mut child = app.new_actor_with_config::<PanickingChild>(child_config);
    child.after_stop(|_actor| {
        Reply::pending(async move {
            panic!("cleanup-only panic");
        })
    });

    let child_handle = child.start().await;
    child_handle.stop().await?;

    let notification = await_notification(&captured).await;
    assert_eq!(
        notification.reason,
        TerminationReason::Panic("cleanup-only panic".to_string()),
        "A cleanup panic during clean termination should be reported as Panic"
    );

    parent_handle.stop().await?;
    app.shutdown_all().await?;
    Ok(())
}

/// Child actor state that counts received broadcasts.
#[derive(Default, Debug)]
struct Recorder {
    pongs: Arc<AtomicUsize>,
}

/// Broadcast message used to observe broker subscription state.
#[acton_message]
struct Pong;

/// Tests that a PANICKED actor's broker subscriptions are removed, exactly like
/// a gracefully stopped actor's: a replacement actor registered under the same
/// `Ern` can subscribe and receive broadcasts. The broker's subscriber set is
/// keyed by the subscriber's `Ern`, so if the panicked incarnation's entry
/// leaked, the replacement's subscription insert would be blocked by the stale
/// equal-key entry and the broadcast would never arrive — this test fails
/// without the panic-path cleanup.
///
/// Both incarnations are built WITHOUT a parent: a parented child's effective
/// `Ern` is derived from the parent's id and does not reproducibly collide, so
/// the same-key premise only holds for top-level actors sharing the root `Ern`.
/// Synchronization does not rely on a parent notification either: the panicked
/// actor queues its `RemoveAllSubscriptions` at the broker BEFORE its task
/// exits, and `stop().await` on the dead handle returns only after the task
/// has exited (it waits on the task tracker), so a subscribe issued afterwards
/// is ordered behind the removal in the broker's FIFO inbox.
#[tokio::test]
async fn test_panicked_actor_is_unsubscribed_from_broker() -> anyhow::Result<()> {
    let mut app = ActonApp::launch_async().await;
    let broker_handle = app.broker();

    let recorder_id = Ern::with_root("panic-recorder")?;

    // --- First incarnation: subscribe, then panic ---
    let first_config =
        ActorConfig::new(recorder_id.clone(), None, Some(broker_handle.clone()))?;
    let mut first = app.new_actor_with_config::<Recorder>(first_config);
    let first_pongs = first.model.pongs.clone();
    first
        .mutate_on::<Pong>(|actor, _ctx| {
            actor.model.pongs.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        })
        .mutate_on::<Boom>(|_actor, _ctx| {
            panic!("subscriber panic");
        });
    first.handle().subscribe::<Pong>().await;
    let first_id = first.handle().id();
    let first_handle = first.start().await;

    first_handle.send(Boom).await;
    // Wait for the panicked actor's task to exit; by then its
    // RemoveAllSubscriptions message is already queued at the broker.
    first_handle.stop().await?;

    // --- Second incarnation: same root Ern, fresh subscription ---
    let second_config = ActorConfig::new(recorder_id, None, Some(broker_handle.clone()))?;
    let mut second = app.new_actor_with_config::<Recorder>(second_config);
    let second_pongs = second.model.pongs.clone();
    second.mutate_on::<Pong>(|actor, _ctx| {
        actor.model.pongs.fetch_add(1, Ordering::SeqCst);
        Reply::ready()
    });
    assert_eq!(
        second.handle().id(),
        first_id,
        "test precondition: both incarnations must share the same Ern so their \
         broker subscription entries genuinely collide"
    );
    second.handle().subscribe::<Pong>().await;
    let _second_handle = second.start().await;

    broker_handle.broadcast(Pong).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while second_pongs.load(Ordering::SeqCst) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the replacement actor should receive broadcasts, proving the panicked \
             actor's subscription entry was removed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(
        first_pongs.load(Ordering::SeqCst),
        0,
        "the panicked actor should not have received any broadcasts"
    );

    app.shutdown_all().await?;
    Ok(())
}
