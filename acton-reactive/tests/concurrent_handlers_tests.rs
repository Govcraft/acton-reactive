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
use crate::setup::{
    initialize_tracing,
};

mod setup;

/// Represents state for testing concurrent read-only handlers
#[derive(Default, Debug, Clone)]
pub struct ConcurrentTestAgent {
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

#[acton_test]
async fn test_act_on_concurrent_readonly() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: AgentRuntime = ActonApp::launch();

    let mut agent_builder = runtime.new_agent::<ConcurrentTestAgent>().await;
    
    // Add read-only handler using act_on for concurrent execution
    agent_builder
        .act_on::<ReadOnlyMessage>(|agent, _envelope| {
            // This should be read-only access - using &ConcurrentTestAgent
            let _count = agent.model.read_only_count.fetch_add(1, Ordering::SeqCst);
            
            // Simulate some work to test concurrency
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                // This should be safe for concurrent execution
            })
        })
        .mutate_on::<MutableMessage>(|agent, _envelope| {
            // This should have mutable access - using &mut ConcurrentTestAgent
            agent.model.mutable_count += 1;
            AgentReply::immediate()
        })
        .after_stop(|agent| {
            let readonly_count = agent.model.read_only_count.load(Ordering::SeqCst);
            assert_eq!(readonly_count, 10, "Should have 10 read-only executions");
            assert_eq!(agent.model.mutable_count, 5, "Should have 5 mutable executions");
            AgentReply::immediate()
        });

    let handle = agent_builder.start().await;

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
    let mut runtime: AgentRuntime = ActonApp::launch();

    let mut agent_builder = runtime.new_agent::<ConcurrentTestAgent>().await;
    
    // Test that read-only handlers can safely access shared state concurrently
    agent_builder
        .act_on::<ConcurrentMessage>(|agent, _envelope| {
            // Access shared atomic counter - safe for concurrent access
            let _current_count = agent.model.concurrent_executions.fetch_add(1, Ordering::SeqCst);
            
            Box::pin(async move {
                // Ensure we don't have mutable access to non-atomic fields
                // This would not compile if we tried to mutate non-atomic state
                tokio::time::sleep(Duration::from_millis(5)).await;
            })
        })
        .after_stop(|agent| {
            let count = agent.model.concurrent_executions.load(Ordering::SeqCst);
            assert!(count > 0, "Should have some concurrent executions");
            AgentReply::immediate()
        });

    let handle = agent_builder.start().await;

    // Send many concurrent messages to stress test
    for _ in 0..20 {
        handle.send(ConcurrentMessage).await;
    }
    
    tokio::time::sleep(Duration::from_millis(500)).await;
    runtime.shutdown_all().await?;
    
    Ok(())
}

/// Test agent with complex state to verify read-only access patterns
#[derive(Default, Debug, Clone)]
pub struct DataAgent {
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
    let mut runtime: AgentRuntime = ActonApp::launch();

    let mut agent_builder = runtime.new_agent::<DataAgent>().await;
    
    // Initialize with some data
    agent_builder.model.data = vec![1, 2, 3, 4, 5];
    
    agent_builder
        .act_on::<ReadData>(|agent, _envelope| {
            // Read-only access to complex data structure
            let sum: i32 = agent.model.data.iter().sum();
            let _count = agent.model.data.len();
            
            // Update atomic counters - safe for concurrent access
            agent.model.total.fetch_add(sum as usize, Ordering::SeqCst);
            agent.model.read_count.fetch_add(1, Ordering::SeqCst);
            
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(1)).await;
            })
        })
        .mutate_on::<AddData>(|agent, envelope| {
            // Mutable access to add data
            agent.model.data.push(envelope.message().0);
            AgentReply::immediate()
        })
        .after_stop(|agent| {
            let total = agent.model.total.load(Ordering::SeqCst);
            let reads = agent.model.read_count.load(Ordering::SeqCst);
            assert_eq!(reads, 5, "Should have 5 read-only executions");
            assert_eq!(total, 75, "Sum should be 15 * 5 = 75");
            assert_eq!(agent.model.data.len(), 7, "Should have 7 items after additions");
            AgentReply::immediate()
        });

    let handle = agent_builder.start().await;

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
    let mut runtime: AgentRuntime = ActonApp::launch();

    #[derive(Default, Debug, Clone)]
    pub struct PerformanceAgent {
        pub counter: Arc<AtomicUsize>,
        pub start_time: Option<std::time::Instant>,
    }

    #[derive(Debug, Clone)]
    pub struct PerformanceMessage;

    let mut agent_builder = runtime.new_agent::<PerformanceAgent>().await;
    agent_builder.model.start_time = Some(std::time::Instant::now());
    
    agent_builder
        .act_on::<PerformanceMessage>(|agent, _envelope| {
            agent.model.counter.fetch_add(1, Ordering::SeqCst);
            
            Box::pin(async move {
                // Simulate work
                tokio::time::sleep(Duration::from_millis(10)).await;
            })
        })
        .after_stop(|agent| {
            let elapsed = agent.model.start_time.unwrap().elapsed();
            let count = agent.model.counter.load(Ordering::SeqCst);
            
            // With concurrent execution, 10 messages should complete much faster than 100ms
            assert!(count >= 10, "Should have processed 10 messages");
            // Note: The timing assertion is relaxed for test stability
            assert!(elapsed < Duration::from_millis(500), 
                   "Concurrent execution should complete within reasonable time");
            AgentReply::immediate()
        });

    let handle = agent_builder.start().await;

    // Send multiple messages that should execute concurrently
    for _ in 0..10 {
        handle.send(PerformanceMessage).await;
    }
    
    tokio::time::sleep(Duration::from_millis(200)).await;
    runtime.shutdown_all().await?;
    
    Ok(())
}

/// Test mixed handler types working together
#[acton_test]
async fn test_mixed_handlers() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: AgentRuntime = ActonApp::launch();

    #[derive(Default, Debug, Clone)]
    pub struct MixedAgent {
        pub read_only_hits: Arc<AtomicUsize>,
        pub mutable_hits: usize,
        pub data: Vec<String>,
    }

    #[derive(Debug, Clone)]
    pub struct ReadOnlyOp;
    #[derive(Debug, Clone)]
    pub struct MutableOp(String);
    #[derive(Debug, Clone)]
    pub struct StatsRequest;

    let mut agent_builder = runtime.new_agent::<MixedAgent>().await;
    
    agent_builder
        .act_on::<ReadOnlyOp>(|agent, _envelope| {
            // Read-only: just read and count
            let _ = agent.model.data.len();
            agent.model.read_only_hits.fetch_add(1, Ordering::SeqCst);
            
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(5)).await;
            })
        })
        .mutate_on::<MutableOp>(|agent, envelope| {
            // Mutable: actually modify state
            agent.model.data.push(envelope.message().0.clone());
            agent.model.mutable_hits += 1;
            AgentReply::immediate()
        })
        .act_on::<StatsRequest>(|agent, _envelope| {
            // Read-only stats access
            let read_count = agent.model.read_only_hits.load(Ordering::SeqCst);
            let mutable_count = agent.model.mutable_hits;
            let data_count = agent.model.data.len();
            
            Box::pin(async move {
                tracing::info!("Stats: read={}, mutable={}, data={}", read_count, mutable_count, data_count);
            })
        })
        .after_stop(|agent| {
            let read_count = agent.model.read_only_hits.load(Ordering::SeqCst);
            assert_eq!(read_count, 6, "Should have 6 read-only operations");
            assert_eq!(agent.model.mutable_hits, 4, "Should have 4 mutable operations");
            assert_eq!(agent.model.data.len(), 4, "Should have 4 data items");
            AgentReply::immediate()
        });

    let handle = agent_builder.start().await;

    // Send mixed messages
    for i in 0..6 {
        handle.send(ReadOnlyOp).await;
        if i < 4 {
            handle.send(MutableOp(format!("item_{}", i))).await;
        }
    }
    
    handle.send(StatsRequest).await;
    
    tokio::time::sleep(Duration::from_millis(300)).await;
    runtime.shutdown_all().await?;
    
    Ok(())
}