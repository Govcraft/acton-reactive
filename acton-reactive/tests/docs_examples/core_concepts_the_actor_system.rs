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

//! Tests for examples from docs/core-concepts/the-actor-system/page.md
//!
//! This module verifies that the code examples from the core concepts
//! "The Actor System" documentation page compile and run correctly.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Tests creating the runtime.
///
/// From: docs/core-concepts/the-actor-system/page.md - "Creating the Runtime"
#[acton_test]
async fn test_creating_runtime() -> anyhow::Result<()> {
    #[acton_actor]
    struct MyActor;

    // Every Acton application starts by launching the runtime
    let mut runtime = ActonApp::launch_async().await;

    // Create actors, send messages...
    let actor = runtime.new_actor::<MyActor>();
    let _handle = actor.start().await;

    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests creating actors with the runtime.
///
/// From: docs/core-concepts/the-actor-system/page.md - "Creating Actors"
#[acton_test]
async fn test_creating_actors() -> anyhow::Result<()> {
    #[acton_actor]
    struct Counter {
        count: u32,
    }

    #[acton_message]
    struct Increment;

    #[acton_message]
    struct GetCount;

    let mut runtime = ActonApp::launch_async().await;

    let mut counter = runtime.new_actor::<Counter>();

    counter
        .mutate_on::<Increment>(|actor, _ctx| {
            actor.model.count += 1;
            Reply::ready()
        })
        .act_on::<GetCount>(|_actor, _ctx| {
            Reply::ready()
        });

    let handle = counter.start().await;

    // Verify handle is usable
    handle.send(Increment).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests named actors for easier debugging.
///
/// From: docs/core-concepts/the-actor-system/page.md - "Named Actors"
#[acton_test]
async fn test_named_actors() -> anyhow::Result<()> {
    #[acton_actor]
    struct Counter;

    let mut runtime = ActonApp::launch_async().await;

    // Give actors meaningful names for easier debugging
    let counter = runtime.new_actor_with_name::<Counter>("user-counter".to_string());

    let handle = counter.start().await;

    // Verify the actor name (Ern removes hyphens and adds a suffix)
    assert!(handle.name().starts_with("usercounter"));

    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests working with handles.
///
/// From: docs/core-concepts/the-actor-system/page.md - "Working with Handles"
#[acton_test]
async fn test_working_with_handles() -> anyhow::Result<()> {
    #[acton_actor]
    struct Counter {
        count: u32,
    }

    #[acton_message]
    struct Increment;

    let count1 = Arc::new(AtomicU32::new(0));
    let c1_clone = count1.clone();

    let mut runtime = ActonApp::launch_async().await;

    let mut counter = runtime.new_actor::<Counter>();
    counter
        .mutate_on::<Increment>(|actor, _ctx| {
            actor.model.count += 1;
            Reply::ready()
        })
        .after_stop(move |actor| {
            c1_clone.store(actor.model.count, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = counter.start().await;

    // Fire and forget
    handle.send(Increment).await;

    // Handles are cheap to clone
    let handle_for_web = handle.clone();
    let handle_for_background = handle.clone();

    // Both clones can send messages
    handle_for_web.send(Increment).await;
    handle_for_background.send(Increment).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert_eq!(count1.load(Ordering::SeqCst), 3);

    Ok(())
}

/// Tests the Broker for pub/sub.
///
/// From: docs/core-concepts/the-actor-system/page.md - "The Broker: Pub/Sub"
#[acton_test]
async fn test_broker_pubsub() -> anyhow::Result<()> {
    #[acton_actor]
    struct EventListener {
        received: bool,
    }

    #[acton_message]
    struct UserLoggedIn {
        user_id: u64,
    }

    let received = Arc::new(AtomicBool::new(false));
    let received_clone = received.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Getting the Broker
    let broker = runtime.broker();

    // Create a listener
    let mut listener = runtime.new_actor::<EventListener>();

    listener.mutate_on::<UserLoggedIn>(move |actor, ctx| {
        let msg = ctx.message();
        actor.model.received = true;
        assert_eq!(msg.user_id, 123);
        received_clone.store(true, Ordering::SeqCst);
        Reply::ready()
    });

    // Subscribe before starting
    listener.handle().subscribe::<UserLoggedIn>().await;

    let _handle = listener.start().await;

    // Publishing Events
    broker.broadcast(UserLoggedIn { user_id: 123 }).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    assert!(received.load(Ordering::SeqCst));

    Ok(())
}

/// Tests proper shutdown.
///
/// From: docs/core-concepts/the-actor-system/page.md - "Shutdown"
#[acton_test]
async fn test_shutdown() -> anyhow::Result<()> {
    #[acton_actor]
    struct MyActor {
        stopped: bool,
    }

    let stopped = Arc::new(AtomicBool::new(false));
    let stopped_clone = stopped.clone();

    let mut runtime = ActonApp::launch_async().await;

    let mut actor = runtime.new_actor::<MyActor>();
    actor.after_stop(move |_actor| {
        // after_stop receives immutable reference, just verify the hook runs
        stopped_clone.store(true, Ordering::SeqCst);
        Reply::ready()
    });

    let _handle = actor.start().await;

    // Proper shutdown ensures messages aren't lost
    runtime.shutdown_all().await?;

    // Verify the actor was properly stopped
    assert!(stopped.load(Ordering::SeqCst));

    Ok(())
}

/// Tests multiple subscribers to broker.
///
/// From: docs/core-concepts/the-actor-system/page.md - broker vs direct messaging
#[acton_test]
async fn test_multiple_subscribers() -> anyhow::Result<()> {
    #[acton_actor]
    struct Listener {
        received_count: u32,
    }

    #[acton_message]
    struct Event;

    let total_received = Arc::new(AtomicU32::new(0));

    let mut runtime = ActonApp::launch_async().await;
    let broker = runtime.broker();

    // Create multiple listeners
    let mut handles = Vec::new();
    for _ in 0..3 {
        let counter = total_received.clone();
        let mut listener = runtime.new_actor::<Listener>();

        listener.mutate_on::<Event>(move |actor, _ctx| {
            actor.model.received_count += 1;
            counter.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        });

        listener.handle().subscribe::<Event>().await;
        handles.push(listener.start().await);
    }

    // Broadcast reaches all subscribers
    broker.broadcast(Event).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    // All 3 listeners should have received the event
    assert_eq!(total_received.load(Ordering::SeqCst), 3);

    Ok(())
}

/// Tests direct messaging vs broker.
///
/// From: docs/core-concepts/the-actor-system/page.md - "Broker vs Direct Messaging"
#[acton_test]
async fn test_direct_vs_broker() -> anyhow::Result<()> {
    #[acton_actor]
    struct Worker {
        direct_count: u32,
        broadcast_count: u32,
    }

    #[acton_message]
    struct DirectMessage;

    #[acton_message]
    struct BroadcastMessage;

    let direct = Arc::new(AtomicU32::new(0));
    let broadcast = Arc::new(AtomicU32::new(0));
    let d_clone = direct.clone();
    let b_clone = broadcast.clone();

    let mut runtime = ActonApp::launch_async().await;
    let broker = runtime.broker();

    let mut worker = runtime.new_actor::<Worker>();

    worker
        .mutate_on::<DirectMessage>(move |actor, _ctx| {
            actor.model.direct_count += 1;
            d_clone.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        })
        .mutate_on::<BroadcastMessage>(move |actor, _ctx| {
            actor.model.broadcast_count += 1;
            b_clone.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        });

    // Subscribe to broadcasts
    worker.handle().subscribe::<BroadcastMessage>().await;

    let handle = worker.start().await;

    // Use handle for direct messaging - when you know exactly which actor
    handle.send(DirectMessage).await;
    handle.send(DirectMessage).await;

    // Use broker for broadcasts - when multiple actors should react
    broker.broadcast(BroadcastMessage).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    assert_eq!(direct.load(Ordering::SeqCst), 2);
    assert_eq!(broadcast.load(Ordering::SeqCst), 1);

    Ok(())
}
