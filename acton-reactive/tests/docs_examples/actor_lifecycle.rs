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

//! Tests for examples from docs/actor-lifecycle/page.md
//!
//! This module verifies that the code examples from the "Actor Lifecycle"
//! documentation page compile and run correctly.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Tests all four lifecycle hooks and their execution order.
///
/// From: docs/actor-lifecycle/page.md - "Lifecycle Hooks"
#[acton_test]
async fn test_lifecycle_hooks() -> anyhow::Result<()> {
    #[acton_actor]
    struct MyState {
        hook_order: Vec<String>,
    }

    let hook_order = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let before_start_order = hook_order.clone();
    let after_start_order = hook_order.clone();
    let before_stop_order = hook_order.clone();
    let after_stop_order = hook_order.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<MyState>();

    actor
        .before_start(move |_actor| {
            before_start_order
                .lock()
                .unwrap()
                .push("before_start".to_string());
            Reply::ready()
        })
        .after_start(move |_actor| {
            after_start_order
                .lock()
                .unwrap()
                .push("after_start".to_string());
            Reply::ready()
        })
        .before_stop(move |_actor| {
            before_stop_order
                .lock()
                .unwrap()
                .push("before_stop".to_string());
            Reply::ready()
        })
        .after_stop(move |_actor| {
            after_stop_order
                .lock()
                .unwrap()
                .push("after_stop".to_string());
            Reply::ready()
        });

    let _handle = actor.start().await;

    // Give time for after_start to complete
    tokio::time::sleep(Duration::from_millis(50)).await;

    runtime.shutdown_all().await?;

    let order = hook_order.lock().unwrap();
    assert_eq!(order.len(), 4);
    assert_eq!(order[0], "before_start");
    assert_eq!(order[1], "after_start");
    assert_eq!(order[2], "before_stop");
    assert_eq!(order[3], "after_stop");
    drop(order);

    Ok(())
}

/// Tests `before_start` hook.
///
/// From: docs/actor-lifecycle/page.md - "`before_start`"
#[acton_test]
async fn test_before_start_hook() -> anyhow::Result<()> {
    #[acton_actor]
    struct MyState {
        some_field: String,
    }

    let before_start_ran = Arc::new(AtomicBool::new(false));
    let before_start_clone = before_start_ran.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<MyState>();

    actor.before_start(move |actor| {
        // Synchronous validation or logging
        if actor.model.some_field.is_empty() {
            // Log warning (in tests we just verify it ran)
        }
        before_start_clone.store(true, Ordering::SeqCst);
        Reply::ready()
    });

    let _handle = actor.start().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert!(before_start_ran.load(Ordering::SeqCst));

    Ok(())
}

/// Tests `after_start` hook with self-messaging.
///
/// From: docs/actor-lifecycle/page.md - "`after_start`"
#[acton_test]
async fn test_after_start_hook() -> anyhow::Result<()> {
    #[acton_actor]
    struct MyState {
        initialized: bool,
    }

    #[acton_message]
    struct Heartbeat;

    let heartbeat_received = Arc::new(AtomicBool::new(false));
    let heartbeat_clone = heartbeat_received.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<MyState>();

    actor
        .after_start(|actor| {
            let self_handle = actor.handle().clone();

            Reply::pending(async move {
                // Send a heartbeat message after start
                self_handle.send(Heartbeat).await;
            })
        })
        .mutate_on::<Heartbeat>(move |actor, _ctx| {
            actor.model.initialized = true;
            heartbeat_clone.store(true, Ordering::SeqCst);
            Reply::ready()
        });

    let _handle = actor.start().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    assert!(heartbeat_received.load(Ordering::SeqCst));

    Ok(())
}

/// Tests `before_stop` hook for cleanup.
///
/// From: docs/actor-lifecycle/page.md - "`before_stop`"
#[acton_test]
async fn test_before_stop_hook() -> anyhow::Result<()> {
    #[acton_actor]
    struct MyState {
        active: bool,
    }

    let cleanup_ran = Arc::new(AtomicBool::new(false));
    let cleanup_clone = cleanup_ran.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<MyState>();
    actor.model.active = true;

    actor.before_stop(move |_actor| {
        // before_stop is read-only, just verify the hook runs
        cleanup_clone.store(true, Ordering::SeqCst);
        Reply::ready()
    });

    let _handle = actor.start().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert!(cleanup_ran.load(Ordering::SeqCst));

    Ok(())
}

/// Tests `after_stop` hook for final assertions.
///
/// From: docs/actor-lifecycle/page.md - "`after_stop`"
#[acton_test]
async fn test_after_stop_hook() -> anyhow::Result<()> {
    #[acton_actor]
    struct Counter {
        count: u32,
    }

    #[acton_message]
    struct Increment;

    let final_count = Arc::new(AtomicU32::new(0));
    let final_count_clone = final_count.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<Counter>();

    actor
        .mutate_on::<Increment>(|actor, _ctx| {
            actor.model.count += 1;
            Reply::ready()
        })
        .after_stop(move |actor| {
            // Great for test assertions
            final_count_clone.store(actor.model.count, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = actor.start().await;

    handle.send(Increment).await;
    handle.send(Increment).await;
    handle.send(Increment).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    // after_stop assertion runs during shutdown
    assert_eq!(final_count.load(Ordering::SeqCst), 3);

    Ok(())
}

/// Tests async initialization pattern with `after_start`.
///
/// From: docs/actor-lifecycle/page.md - "Async Initialization Pattern"
#[acton_test]
async fn test_async_initialization_pattern() -> anyhow::Result<()> {
    // Simulated connection
    #[derive(Clone, Debug, Default)]
    struct Connection {
        connected: bool,
    }

    #[acton_actor]
    struct DatabaseActor {
        connection: Option<Connection>,
    }

    #[acton_message]
    struct SetConnection(Connection);

    #[acton_message]
    struct CheckConnection;

    let connection_set = Arc::new(AtomicBool::new(false));
    let connection_clone = connection_set.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<DatabaseActor>();

    actor
        .after_start(|actor| {
            let self_handle = actor.handle().clone();
            Reply::pending(async move {
                // Simulate async database connection
                tokio::time::sleep(Duration::from_millis(10)).await;
                let conn = Connection { connected: true };
                self_handle.send(SetConnection(conn)).await;
            })
        })
        .mutate_on::<SetConnection>(|actor, ctx| {
            actor.model.connection = Some(ctx.message().0.clone());
            Reply::ready()
        })
        .act_on::<CheckConnection>(move |actor, _ctx| {
            if let Some(conn) = &actor.model.connection {
                connection_clone.store(conn.connected, Ordering::SeqCst);
            }
            Reply::ready()
        });

    let handle = actor.start().await;

    // Wait for async initialization
    tokio::time::sleep(Duration::from_millis(100)).await;

    handle.send(CheckConnection).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert!(connection_set.load(Ordering::SeqCst));

    Ok(())
}

/// Tests testing lifecycle with assertions.
///
/// From: docs/actor-lifecycle/page.md - "Testing Lifecycle"
#[acton_test]
async fn test_lifecycle_with_assertions() -> anyhow::Result<()> {
    #[acton_actor]
    struct Counter {
        count: u32,
    }

    #[acton_message]
    struct Increment;

    let test_passed = Arc::new(AtomicBool::new(false));
    let test_passed_clone = test_passed.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<Counter>();

    actor
        .mutate_on::<Increment>(|actor, _ctx| {
            actor.model.count += 1;
            Reply::ready()
        })
        .after_stop(move |actor| {
            // Verify final state
            if actor.model.count == 3 {
                test_passed_clone.store(true, Ordering::SeqCst);
            }
            Reply::ready()
        });

    let handle = actor.start().await;

    handle.send(Increment).await;
    handle.send(Increment).await;
    handle.send(Increment).await;

    // Give time to process
    tokio::time::sleep(Duration::from_millis(50)).await;

    runtime.shutdown_all().await?;
    // after_stop assertion runs during shutdown

    assert!(test_passed.load(Ordering::SeqCst));

    Ok(())
}
