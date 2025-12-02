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

//! Tests for examples from docs/core-concepts/what-are-actors/page.md
//!
//! This module verifies that the code examples from the "What Are Actors?"
//! documentation page compile and run correctly.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Tests basic actor mutation - messages processed one at a time.
///
/// From: docs/core-concepts/what-are-actors/page.md - "Actor model - messages processed one at a time"
#[acton_test]
async fn test_actor_sequential_processing() -> anyhow::Result<()> {
    #[acton_actor]
    struct Counter {
        count: i32,
    }

    #[acton_message]
    struct Increment;

    let final_count = Arc::new(std::sync::atomic::AtomicI32::new(0));
    let final_clone = final_count.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut counter = runtime.new_actor::<Counter>();

    // Actor model - messages processed one at a time
    counter
        .mutate_on::<Increment>(|actor, _envelope| {
            actor.model.count += 1; // Always safe
            Reply::ready()
        })
        .after_stop(move |actor| {
            final_clone.store(actor.model.count, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = counter.start().await;

    // When two Increment messages arrive, they're processed sequentially. No locks, no races.
    handle.send(Increment).await;
    handle.send(Increment).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert_eq!(final_count.load(Ordering::SeqCst), 2);

    Ok(())
}

/// Tests creating many actors - actors are lightweight.
///
/// From: docs/core-concepts/what-are-actors/page.md - "Actors Are Lightweight"
#[acton_test]
async fn test_actors_are_lightweight() -> anyhow::Result<()> {
    #[acton_actor]
    struct Session {
        id: usize,
    }

    #[acton_message]
    struct Request;

    let mut runtime = ActonApp::launch_async().await;

    // You can create many actors (though we'll do fewer for test speed)
    let actor_count = 100; // In production you could do 10_000+
    let mut handles = Vec::new();

    for i in 0..actor_count {
        let mut actor = runtime.new_actor_with_name::<Session>(format!("session-{}", i));
        actor.model.id = i;
        actor.mutate_on::<Request>(|actor, _ctx| {
            let _ = actor.model.id; // Use the id
            Reply::ready()
        });
        let handle = actor.start().await;
        handles.push(handle);
    }

    // All actors are running
    assert_eq!(handles.len(), actor_count);

    // Send a message to each
    for handle in &handles {
        handle.send(Request).await;
    }

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests actor isolation - failures don't crash the system.
///
/// From: docs/core-concepts/what-are-actors/page.md - "Actors Are Isolated"
#[acton_test]
async fn test_actors_are_isolated() -> anyhow::Result<()> {
    #[acton_actor]
    struct Worker {
        task_count: u32,
    }

    #[acton_message]
    struct Task;

    let worker1_count = Arc::new(AtomicU32::new(0));
    let worker2_count = Arc::new(AtomicU32::new(0));
    let w1_clone = worker1_count.clone();
    let w2_clone = worker2_count.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Create two independent workers
    let mut worker1 = runtime.new_actor::<Worker>();
    worker1
        .mutate_on::<Task>(|actor, _ctx| {
            actor.model.task_count += 1;
            Reply::ready()
        })
        .after_stop(move |actor| {
            w1_clone.store(actor.model.task_count, Ordering::SeqCst);
            Reply::ready()
        });
    let handle1 = worker1.start().await;

    let mut worker2 = runtime.new_actor::<Worker>();
    worker2
        .mutate_on::<Task>(|actor, _ctx| {
            actor.model.task_count += 1;
            Reply::ready()
        })
        .after_stop(move |actor| {
            w2_clone.store(actor.model.task_count, Ordering::SeqCst);
            Reply::ready()
        });
    let handle2 = worker2.start().await;

    // Each actor runs independently
    handle1.send(Task).await;
    handle1.send(Task).await;
    handle2.send(Task).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    // They don't share state
    assert_eq!(worker1_count.load(Ordering::SeqCst), 2);
    assert_eq!(worker2_count.load(Ordering::SeqCst), 1);

    Ok(())
}

/// Tests mutate_on vs act_on distinction.
///
/// From: docs/core-concepts/what-are-actors/page.md - "mutate_on vs act_on distinction"
#[acton_test]
async fn test_mutate_vs_act_distinction() -> anyhow::Result<()> {
    #[acton_actor]
    struct DataStore {
        data: String,
    }

    #[acton_message]
    struct Update {
        new_data: String,
    }

    #[acton_message]
    struct Query;

    #[acton_message]
    struct QueryResponse(String);

    let query_response = Arc::new(std::sync::Mutex::new(String::new()));
    let response_clone = query_response.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Create receiver for query responses
    let mut receiver = runtime.new_actor::<DataStore>();
    receiver.mutate_on::<QueryResponse>(move |_actor, ctx| {
        *response_clone.lock().unwrap() = ctx.message().0.clone();
        Reply::ready()
    });
    let receiver_handle = receiver.start().await;

    // Create data store
    let mut store = runtime.new_actor::<DataStore>();

    // mutate_on for handlers that change state (sequential)
    store.mutate_on::<Update>(|actor, ctx| {
        actor.model.data = ctx.message().new_data.clone();
        Reply::ready()
    });

    // act_on for handlers that only read (can be concurrent)
    store.act_on::<Query>(|actor, ctx| {
        let data = actor.model.data.clone();
        let reply = ctx.reply_envelope();

        Reply::pending(async move {
            reply.send(QueryResponse(data)).await;
        })
    });

    let store_handle = store.start().await;

    // Update then query
    store_handle
        .send(Update {
            new_data: "updated value".to_string(),
        })
        .await;

    let envelope = store_handle.create_envelope(Some(receiver_handle.reply_address()));
    envelope.send(Query).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    assert_eq!(*query_response.lock().unwrap(), "updated value");

    Ok(())
}
