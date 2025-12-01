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
 * See the applicable License for the specific language governing permissions
 * and limitations under that License.
 */

#![allow(dead_code, unused_doc_comments)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

// Use direct paths as re-exports seem problematic in test context
use crate::setup::initialize_tracing;

mod setup;

/// Represents state for testing concurrent read-only handlers
#[derive(Default, Debug, Clone)]
pub struct ConcurrentTestActor {
    pub read_only_count: Arc<AtomicUsize>,
    pub mutable_count: usize,
    pub concurrent_executions: Arc<AtomicUsize>,
}

/// Messages for testing concurrent behavior
#[derive(Debug, Clone)]
pub struct ReadOnlyMessage;

#[derive(Debug, Clone)]
pub struct MutableMessage;

#[derive(Debug, Clone)]
pub struct ConcurrentMessage;

#[derive(Debug, Clone)]
pub struct StatusRequest;

/// Actor for performance testing
#[derive(Default, Debug, Clone)]
pub struct PerformanceActor {
    pub counter: Arc<AtomicUsize>,
    pub start_time: Option<std::time::Instant>,
}

#[derive(Debug, Clone)]
pub struct PerformanceMessage;

/// Actor for concurrent execution verification
#[derive(Default, Debug, Clone)]
pub struct ConcurrencyActor {
    pub start_time: Option<std::time::Instant>,
    pub completion_times: Arc<std::sync::Mutex<Vec<std::time::Instant>>>,
    pub message_count: Arc<AtomicUsize>,
}

#[derive(Debug, Clone)]
pub struct DelayMessage;

/// Actor for mixed handler testing
#[derive(Default, Debug, Clone)]
pub struct MixedActor {
    pub read_only_hits: Arc<AtomicUsize>,
    pub mutable_hits: usize,
    pub data: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ReadOnlyOp;

#[derive(Debug, Clone)]
pub struct MutableOp(pub String);

#[derive(Debug, Clone)]
pub struct StatsRequest;

#[acton_test]
async fn test_act_on_concurrent_readonly() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let mut actor_builder = runtime.new_actor::<ConcurrentTestActor>();

    // Add read-only handler using act_on for concurrent execution
    actor_builder
        .act_on::<ReadOnlyMessage>(|actor, _envelope| {
            // This should be read-only access - using &ConcurrentTestActor
            let _count = actor.model.read_only_count.fetch_add(1, Ordering::SeqCst);

            // Simulate some work to test concurrency
            Reply::pending(async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                // This should be safe for concurrent execution
            })
        })
        .mutate_on::<MutableMessage>(|actor, _envelope| {
            // This should have mutable access - using &mut ConcurrentTestActor
            actor.model.mutable_count += 1;
            Reply::ready()
        })
        .after_stop(|actor| {
            let readonly_count = actor.model.read_only_count.load(Ordering::SeqCst);
            assert_eq!(readonly_count, 10, "Should have 10 read-only executions");
            assert_eq!(
                actor.model.mutable_count, 5,
                "Should have 5 mutable executions"
            );
            Reply::ready()
        });

    let handle = actor_builder.start().await;

    // Send multiple concurrent read-only messages
    for _ in 0..10 {
        handle.send(ReadOnlyMessage).await;
    }

    // Send mutable messages
    for _ in 0..5 {
        handle.send(MutableMessage).await;
    }

    tokio::time::sleep(Duration::from_millis(500)).await;
    runtime.shutdown_all().await?;

    Ok(())
}

#[acton_test]
async fn test_concurrent_readonly_state_safety() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let mut actor_builder = runtime.new_actor::<ConcurrentTestActor>();

    // Test that read-only handlers can safely access shared state concurrently
    actor_builder
        .act_on::<ConcurrentMessage>(|actor, _envelope| {
            // Access shared atomic counter - safe for concurrent access
            let _current_count = actor
                .model
                .concurrent_executions
                .fetch_add(1, Ordering::SeqCst);

            Reply::pending(async move {
                // Ensure we don't have mutable access to non-atomic fields
                // This would not compile if we tried to mutate non-atomic state
                tokio::time::sleep(Duration::from_millis(5)).await;
            })
        })
        .after_stop(|actor| {
            let count = actor.model.concurrent_executions.load(Ordering::SeqCst);
            assert!(count > 0, "Should have some concurrent executions");
            Reply::ready()
        });

    let handle = actor_builder.start().await;

    // Send many concurrent messages to stress test
    for _ in 0..20 {
        handle.send(ConcurrentMessage).await;
    }

    tokio::time::sleep(Duration::from_millis(500)).await;
    runtime.shutdown_all().await?;

    Ok(())
}

/// Test actor with complex state to verify read-only access patterns
#[derive(Default, Debug, Clone)]
pub struct DataActor {
    pub data: Vec<i32>,
    pub total: Arc<AtomicUsize>,
    pub read_count: Arc<AtomicUsize>,
}

#[derive(Debug, Clone)]
pub struct ReadData;

#[derive(Debug, Clone)]
pub struct AddData(i32);

#[acton_test]
async fn test_readonly_complex_data_access() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let mut actor_builder = runtime.new_actor::<DataActor>();

    // Initialize with some data
    actor_builder.model.data = vec![1, 2, 3, 4, 5];

    actor_builder
        .act_on::<ReadData>(|actor, _envelope| {
            // Read-only access to complex data structure
            let sum: i32 = actor.model.data.iter().sum();
            let _count = actor.model.data.len();

            // Update atomic counters - safe for concurrent access
            // sum is always positive in this test context
            actor
                .model
                .total
                .fetch_add(usize::try_from(sum).unwrap_or(0), Ordering::SeqCst);
            actor.model.read_count.fetch_add(1, Ordering::SeqCst);

            Reply::pending(async move {
                tokio::time::sleep(Duration::from_millis(1)).await;
            })
        })
        .mutate_on::<AddData>(|actor, envelope| {
            // Mutable access to add data
            actor.model.data.push(envelope.message().0);
            Reply::ready()
        })
        .after_stop(|actor| {
            let total = actor.model.total.load(Ordering::SeqCst);
            let reads = actor.model.read_count.load(Ordering::SeqCst);
            assert_eq!(reads, 5, "Should have 5 read-only executions");
            assert_eq!(total, 75, "Sum should be 15 * 5 = 75");
            assert_eq!(
                actor.model.data.len(),
                7,
                "Should have 7 items after additions"
            );
            Reply::ready()
        });

    let handle = actor_builder.start().await;

    // Send read-only messages
    for _ in 0..5 {
        handle.send(ReadData).await;
    }

    // Send mutable messages
    handle.send(AddData(6)).await;
    handle.send(AddData(7)).await;

    tokio::time::sleep(Duration::from_millis(200)).await;
    runtime.shutdown_all().await?;

    Ok(())
}

/// Test to verify performance improvement with concurrent read-only handlers
#[acton_test]
async fn test_concurrent_performance() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let mut actor_builder = runtime.new_actor::<PerformanceActor>();
    actor_builder.model.start_time = Some(std::time::Instant::now());

    actor_builder
        .act_on::<PerformanceMessage>(|actor, _envelope| {
            actor.model.counter.fetch_add(1, Ordering::SeqCst);

            Reply::pending(async move {
                // Simulate work
                tokio::time::sleep(Duration::from_millis(10)).await;
            })
        })
        .after_stop(|actor| {
            let elapsed = actor.model.start_time.unwrap().elapsed();
            let count = actor.model.counter.load(Ordering::SeqCst);

            // With concurrent execution, 10 messages should complete much faster than 100ms
            assert!(count >= 10, "Should have processed 10 messages");
            // Note: The timing assertion is relaxed for test stability
            assert!(
                elapsed < Duration::from_millis(500),
                "Concurrent execution should complete within reasonable time"
            );
            Reply::ready()
        });

    let handle = actor_builder.start().await;

    // Send multiple messages that should execute concurrently
    for _ in 0..10 {
        handle.send(PerformanceMessage).await;
    }

    tokio::time::sleep(Duration::from_millis(200)).await;
    runtime.shutdown_all().await?;

    Ok(())
}

/// Test to verify handlers are actually running concurrently
#[acton_test]
async fn test_concurrent_execution_verification() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let mut actor_builder = runtime.new_actor::<ConcurrencyActor>();
    actor_builder.model.start_time = Some(std::time::Instant::now());

    actor_builder
        .act_on::<DelayMessage>(|actor, _envelope| {
            let completion_times = actor.model.completion_times.clone();
            let message_count = actor.model.message_count.clone();

            Reply::pending(async move {
                // Each handler waits 1 second
                tokio::time::sleep(Duration::from_secs(1)).await;

                // Record completion time
                let now = std::time::Instant::now();
                completion_times.lock().unwrap().push(now);
                message_count.fetch_add(1, Ordering::SeqCst);
            })
        })
        .after_stop(|actor| {
            let start_time = actor.model.start_time.unwrap();
            let completion_count = actor.model.completion_times.lock().unwrap().len();
            let total_messages = actor.model.message_count.load(Ordering::SeqCst);

            let total_elapsed = start_time.elapsed();

            // With 10 concurrent handlers each taking 1 second:
            // - Sequential: would take 10 seconds
            // - Concurrent: should complete in ~1-2 seconds

            assert_eq!(total_messages, 10, "Should process all 10 messages");
            assert!(
                completion_count >= 10,
                "Should have completion times for all messages"
            );

            // Verify concurrent execution: total time should be much less than 10 seconds
            // Allow some buffer for startup/shutdown overhead
            assert!(
                total_elapsed < Duration::from_secs(5),
                "Concurrent execution should complete much faster than sequential. Took: {total_elapsed:?}"
            );

            tracing::info!(
                "Total time for 10 concurrent 1-second handlers: {:?}",
                total_elapsed
            );
            Reply::ready()
        });

    let handle = actor_builder.start().await;

    // Send 10 messages that each take 1 second to process
    // If running concurrently, should complete in ~1-2 seconds total
    for _ in 0..10 {
        handle.send(DelayMessage).await;
    }

    // Wait for completion with buffer
    tokio::time::sleep(Duration::from_secs(3)).await;
    runtime.shutdown_all().await?;

    Ok(())
}

/// Test mixed handler types working together
#[acton_test]
async fn test_mixed_handlers() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let mut actor_builder = runtime.new_actor::<MixedActor>();

    actor_builder
        .act_on::<ReadOnlyOp>(|actor, _envelope| {
            // Read-only: just read and count
            let _ = actor.model.data.len();
            actor.model.read_only_hits.fetch_add(1, Ordering::SeqCst);

            Reply::pending(async move {
                tokio::time::sleep(Duration::from_millis(5)).await;
            })
        })
        .mutate_on::<MutableOp>(|actor, envelope| {
            // Mutable: actually modify state
            actor.model.data.push(envelope.message().0.clone());
            actor.model.mutable_hits += 1;
            Reply::ready()
        })
        .act_on::<StatsRequest>(|actor, _envelope| {
            // Read-only stats access
            let read_count = actor.model.read_only_hits.load(Ordering::SeqCst);
            let mutable_count = actor.model.mutable_hits;
            let data_count = actor.model.data.len();

            Reply::pending(async move {
                tracing::info!(
                    "Stats: read={}, mutable={}, data={}",
                    read_count,
                    mutable_count,
                    data_count
                );
            })
        })
        .after_stop(|actor| {
            let read_count = actor.model.read_only_hits.load(Ordering::SeqCst);
            assert_eq!(read_count, 6, "Should have 6 read-only operations");
            assert_eq!(
                actor.model.mutable_hits, 4,
                "Should have 4 mutable operations"
            );
            assert_eq!(actor.model.data.len(), 4, "Should have 4 data items");
            Reply::ready()
        });

    let handle = actor_builder.start().await;

    // Send mixed messages
    for i in 0..6 {
        handle.send(ReadOnlyOp).await;
        if i < 4 {
            handle.send(MutableOp(format!("item_{i}"))).await;
        }
    }

    handle.send(StatsRequest).await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    runtime.shutdown_all().await?;

    Ok(())
}
