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

//! Tests for examples from docs/supervision/page.md
//!
//! This module verifies that the code examples from the "Supervision & Children"
//! documentation page compile and run correctly.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Tests creating child actors via parent's handle.
///
/// From: docs/supervision/page.md - "Creating Child Actors"
#[acton_test]
async fn test_creating_child_actors() -> anyhow::Result<()> {
    #[acton_actor]
    struct Parent;

    #[acton_actor]
    struct Child {
        received_task: bool,
    }

    #[acton_message]
    struct Task;

    let child_ran = Arc::new(AtomicBool::new(false));
    let child_ran_clone = child_ran.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Create parent
    let parent = runtime.new_actor::<Parent>();
    let parent_handle = parent.start().await;

    // Create child and configure it
    let mut child = runtime.new_actor::<Child>();
    child
        .mutate_on::<Task>(|actor, _ctx| {
            actor.model.received_task = true;
            Reply::ready()
        })
        .after_stop(move |actor| {
            child_ran_clone.store(actor.model.received_task, Ordering::SeqCst);
            Reply::ready()
        });

    // Parent supervises the child
    let child_handle = parent_handle.supervise(child).await?;

    // Verify parent has child
    assert_eq!(parent_handle.children().len(), 1);

    // Send task to child
    child_handle.send(Task).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    assert!(child_ran.load(Ordering::SeqCst));

    Ok(())
}

/// Tests ERN (Entity Resource Names) hierarchy.
///
/// From: docs/supervision/page.md - "ERN (Entity Resource Names)"
#[acton_test]
async fn test_ern_hierarchy() -> anyhow::Result<()> {
    #[acton_actor]
    struct ServiceState;

    #[acton_actor]
    struct WorkerState;

    let mut runtime = ActonApp::launch_async().await;

    // Root actor with name
    let service = runtime.new_actor_with_name::<ServiceState>("payment-service".to_string());

    // Create child with config
    let child_config = ActorConfig::new(Ern::with_root("worker-1").unwrap(), None, None)?;
    let child = runtime.new_actor_with_config::<WorkerState>(child_config);

    let service_handle = service.start().await;
    let child_handle = service_handle.supervise(child).await?;

    // Verify names - `Ern` transforms the name (removes hyphens, adds unique suffix)
    // so we check that the name starts with the expected prefix
    assert!(service_handle.name().starts_with("paymentservice"));
    assert!(child_handle.name().starts_with("worker"));

    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests cascading shutdown.
///
/// From: docs/supervision/page.md - "Cascading Shutdown"
#[acton_test]
async fn test_cascading_shutdown() -> anyhow::Result<()> {
    #[acton_actor]
    struct Parent;

    #[acton_actor]
    struct Child;

    let child_stopped = Arc::new(AtomicBool::new(false));
    let child_clone = child_stopped.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Create parent
    let parent = runtime.new_actor::<Parent>();
    let parent_handle = parent.start().await;

    // Create child
    let mut child = runtime.new_actor::<Child>();
    child.after_stop(move |_actor| {
        child_clone.store(true, Ordering::SeqCst);
        Reply::ready()
    });

    // Parent supervises child
    let _child_handle = parent_handle.supervise(child).await?;

    // Verify hierarchy
    assert_eq!(parent_handle.children().len(), 1);

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Stop parent - should cascade to child
    runtime.shutdown_all().await?;

    // Child should have stopped
    assert!(child_stopped.load(Ordering::SeqCst));

    Ok(())
}

/// Tests worker pool pattern.
///
/// From: docs/supervision/page.md - "Worker Pool Pattern"
#[acton_test]
async fn test_worker_pool_pattern() -> anyhow::Result<()> {
    #[acton_actor]
    struct Supervisor;

    #[acton_actor]
    struct Worker {
        task_count: u32,
    }

    #[acton_message]
    struct Task;

    let total_tasks = Arc::new(AtomicU32::new(0));

    let mut runtime = ActonApp::launch_async().await;
    let supervisor = runtime.new_actor::<Supervisor>();
    let supervisor_handle = supervisor.start().await;

    // Create worker pool
    let mut worker_handles = Vec::new();
    for i in 0..3 {
        let counter = total_tasks.clone();
        let config =
            ActorConfig::new(Ern::with_root(format!("worker-{i}")).unwrap(), None, None)?;

        let mut worker = runtime.new_actor_with_config::<Worker>(config);

        worker
            .mutate_on::<Task>(|actor, _ctx| {
                actor.model.task_count += 1;
                Reply::ready()
            })
            .after_stop(move |actor| {
                counter.fetch_add(actor.model.task_count, Ordering::SeqCst);
                Reply::ready()
            });

        let handle = supervisor_handle.supervise(worker).await?;
        worker_handles.push(handle);
    }

    // Send tasks round-robin
    for i in 0..9 {
        let worker = &worker_handles[i % 3];
        worker.send(Task).await;
    }

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    // Each worker should have processed 3 tasks (9 tasks / 3 workers)
    assert_eq!(total_tasks.load(Ordering::SeqCst), 9);

    Ok(())
}

/// Tests child lifecycle hooks.
///
/// From: docs/supervision/page.md - "Child Lifecycle Hooks"
#[acton_test]
async fn test_child_lifecycle_hooks() -> anyhow::Result<()> {
    #[acton_actor]
    struct Parent;

    #[acton_actor]
    struct Child;

    let child_started = Arc::new(AtomicBool::new(false));
    let child_started_clone = child_started.clone();
    let child_stopped = Arc::new(AtomicBool::new(false));
    let child_stopped_clone = child_stopped.clone();

    let mut runtime = ActonApp::launch_async().await;

    let parent = runtime.new_actor::<Parent>();
    let parent_handle = parent.start().await;

    let mut child = runtime.new_actor::<Child>();

    child
        .after_start(move |_actor| {
            child_started_clone.store(true, Ordering::SeqCst);
            Reply::ready()
        })
        .after_stop(move |_actor| {
            child_stopped_clone.store(true, Ordering::SeqCst);
            Reply::ready()
        });

    let _child_handle = parent_handle.supervise(child).await?;

    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(child_started.load(Ordering::SeqCst));

    runtime.shutdown_all().await?;

    assert!(child_stopped.load(Ordering::SeqCst));

    Ok(())
}

/// Tests finding a child by ID.
///
/// From: docs/supervision/page.md - "Finding Children"
#[acton_test]
async fn test_find_child() -> anyhow::Result<()> {
    #[acton_actor]
    struct Parent;

    #[acton_actor]
    struct Child;

    let mut runtime = ActonApp::launch_async().await;

    let parent = runtime.new_actor::<Parent>();
    let parent_handle = parent.start().await;

    let config = ActorConfig::new(Ern::with_root("my-child").unwrap(), None, None)?;
    let child = runtime.new_actor_with_config::<Child>(config);
    let child_id = child.id().clone();

    let _child_handle = parent_handle.supervise(child).await?;

    // Find child by ID
    let found = parent_handle.find_child(&child_id);
    assert!(found.is_some());

    runtime.shutdown_all().await?;

    Ok(())
}
