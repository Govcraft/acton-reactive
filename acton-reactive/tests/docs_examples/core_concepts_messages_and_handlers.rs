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

//! Tests for examples from docs/core-concepts/messages-and-handlers/page.md
//!
//! This module verifies that the code examples from the core concepts
//! "Messages and Handlers" documentation page compile and run correctly.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Tests defining messages with the acton_message attribute.
///
/// From: docs/core-concepts/messages-and-handlers/page.md - "Defining Messages"
#[acton_test]
async fn test_defining_messages() -> anyhow::Result<()> {
    #[acton_message]
    struct AddItem {
        name: String,
        quantity: u32,
    }

    #[acton_message]
    struct GetTotal;

    #[acton_actor]
    struct Inventory {
        items: Vec<String>,
        total: u32,
    }

    let final_total = Arc::new(AtomicU32::new(0));
    let final_clone = final_total.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<Inventory>();

    actor
        .mutate_on::<AddItem>(|actor, ctx| {
            let msg = ctx.message();
            actor.model.items.push(msg.name.clone());
            actor.model.total += msg.quantity;
            Reply::ready()
        })
        .after_stop(move |actor| {
            final_clone.store(actor.model.total, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = actor.start().await;

    handle
        .send(AddItem {
            name: "Widget".to_string(),
            quantity: 5,
        })
        .await;
    handle
        .send(AddItem {
            name: "Gadget".to_string(),
            quantity: 3,
        })
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert_eq!(final_total.load(Ordering::SeqCst), 8);

    Ok(())
}

/// Tests mutate_on for sequential state changes.
///
/// From: docs/core-concepts/messages-and-handlers/page.md - "mutate_on: Sequential State Changes"
#[acton_test]
async fn test_mutate_on_sequential() -> anyhow::Result<()> {
    #[acton_actor]
    struct Store {
        items: Vec<String>,
        total: u32,
    }

    #[acton_message]
    struct AddItem {
        name: String,
        quantity: u32,
    }

    let items_verified = Arc::new(AtomicBool::new(false));
    let items_clone = items_verified.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut builder = runtime.new_actor::<Store>();

    builder
        .mutate_on::<AddItem>(|actor, envelope| {
            let msg = envelope.message();
            actor.model.items.push(msg.name.clone());
            actor.model.total += msg.quantity;
            Reply::ready()
        })
        .after_stop(move |actor| {
            items_clone.store(
                actor.model.items.len() == 2 && actor.model.total == 7,
                Ordering::SeqCst,
            );
            Reply::ready()
        });

    let handle = builder.start().await;

    handle
        .send(AddItem {
            name: "A".to_string(),
            quantity: 3,
        })
        .await;
    handle
        .send(AddItem {
            name: "B".to_string(),
            quantity: 4,
        })
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert!(items_verified.load(Ordering::SeqCst));

    Ok(())
}

/// Tests act_on for concurrent read-only operations.
///
/// From: docs/core-concepts/messages-and-handlers/page.md - "act_on: Concurrent Read-Only Operations"
#[acton_test]
async fn test_act_on_concurrent() -> anyhow::Result<()> {
    #[acton_actor]
    struct Store {
        total: u32,
    }

    #[acton_message]
    struct GetTotal;

    #[acton_message]
    struct TotalResponse(u32);

    let received_total = Arc::new(AtomicU32::new(0));
    let received_clone = received_total.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Create receiver
    let mut receiver = runtime.new_actor::<Store>();
    receiver.mutate_on::<TotalResponse>(move |_actor, ctx| {
        received_clone.store(ctx.message().0, Ordering::SeqCst);
        Reply::ready()
    });
    let receiver_handle = receiver.start().await;

    // Create store with initial value
    let mut store = runtime.new_actor::<Store>();
    store.model.total = 42;

    store.act_on::<GetTotal>(|actor, envelope| {
        let total = actor.model.total;
        let reply_envelope = envelope.reply_envelope();

        Reply::pending(async move {
            reply_envelope.send(TotalResponse(total)).await;
        })
    });

    let store_handle = store.start().await;

    // Query the store
    let envelope = store_handle.create_envelope(Some(receiver_handle.reply_address()));
    envelope.send(GetTotal).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    assert_eq!(received_total.load(Ordering::SeqCst), 42);

    Ok(())
}

/// Tests why the mutate_on vs act_on distinction matters.
///
/// From: docs/core-concepts/messages-and-handlers/page.md - "Why This Distinction Matters"
#[acton_test]
async fn test_handler_distinction() -> anyhow::Result<()> {
    #[acton_actor]
    struct Counter {
        counter: i32,
    }

    #[acton_message]
    struct Increment;

    let final_count = Arc::new(std::sync::atomic::AtomicI32::new(0));
    let final_clone = final_count.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut builder = runtime.new_actor::<Counter>();

    // CORRECT: Use mutate_on for mutations
    builder
        .mutate_on::<Increment>(|actor, _envelope| {
            actor.model.counter += 1; // Works because we have mutable access
            Reply::ready()
        })
        .after_stop(move |actor| {
            final_clone.store(actor.model.counter, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = builder.start().await;

    handle.send(Increment).await;
    handle.send(Increment).await;
    handle.send(Increment).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert_eq!(final_count.load(Ordering::SeqCst), 3);

    Ok(())
}

/// Tests working with message data.
///
/// From: docs/core-concepts/messages-and-handlers/page.md - "Working with Message Data"
#[acton_test]
async fn test_working_with_message_data() -> anyhow::Result<()> {
    #[acton_actor]
    struct Store {
        items: Vec<String>,
    }

    #[acton_message]
    struct AddItem {
        name: String,
    }

    let items_stored = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let items_clone = items_stored.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut builder = runtime.new_actor::<Store>();

    builder
        .mutate_on::<AddItem>(|actor, envelope| {
            let msg = envelope.message(); // Get &AddItem
            actor.model.items.push(msg.name.clone());
            Reply::ready()
        })
        .after_stop(move |actor| {
            *items_clone.lock().unwrap() = actor.model.items.clone();
            Reply::ready()
        });

    let handle = builder.start().await;

    handle
        .send(AddItem {
            name: "Item1".to_string(),
        })
        .await;
    handle
        .send(AddItem {
            name: "Item2".to_string(),
        })
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    let items = items_stored.lock().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0], "Item1");
    assert_eq!(items[1], "Item2");

    Ok(())
}

/// Tests ignoring envelope for messages without data.
///
/// From: docs/core-concepts/messages-and-handlers/page.md - "For messages without data"
#[acton_test]
async fn test_messages_without_data() -> anyhow::Result<()> {
    #[acton_actor]
    struct Counter {
        count: i32,
    }

    #[acton_message]
    struct Increment;

    let final_count = Arc::new(std::sync::atomic::AtomicI32::new(0));
    let final_clone = final_count.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut builder = runtime.new_actor::<Counter>();

    builder
        .mutate_on::<Increment>(|actor, _envelope| {
            // For messages without data, you can ignore the envelope
            actor.model.count += 1;
            Reply::ready()
        })
        .after_stop(move |actor| {
            final_clone.store(actor.model.count, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = builder.start().await;

    handle.send(Increment).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert_eq!(final_count.load(Ordering::SeqCst), 1);

    Ok(())
}

/// Tests replying to messages - no response needed.
///
/// From: docs/core-concepts/messages-and-handlers/page.md - "No Response Needed"
#[acton_test]
async fn test_no_response_needed() -> anyhow::Result<()> {
    #[acton_actor]
    struct Logger {
        events: Vec<LogEvent>,
    }

    #[acton_message]
    struct LogEvent {
        message: String,
    }

    let event_count = Arc::new(AtomicU32::new(0));
    let count_clone = event_count.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut builder = runtime.new_actor::<Logger>();

    builder
        .mutate_on::<LogEvent>(|actor, envelope| {
            let msg = envelope.message();
            actor.model.events.push(msg.clone());
            Reply::ready() // Done, no response
        })
        .after_stop(move |actor| {
            count_clone.store(actor.model.events.len() as u32, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = builder.start().await;

    handle
        .send(LogEvent {
            message: "test".to_string(),
        })
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert_eq!(event_count.load(Ordering::SeqCst), 1);

    Ok(())
}

/// Tests sending a response using reply envelope.
///
/// From: docs/core-concepts/messages-and-handlers/page.md - "Sending a Response"
#[acton_test]
async fn test_sending_response() -> anyhow::Result<()> {
    #[acton_actor]
    struct Counter {
        count: i32,
    }

    #[acton_message]
    struct GetCount;

    #[acton_message]
    struct CountResponse(i32);

    let received_count = Arc::new(std::sync::atomic::AtomicI32::new(-1));
    let received_clone = received_count.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Create receiver
    let mut receiver = runtime.new_actor::<Counter>();
    receiver.mutate_on::<CountResponse>(move |_actor, ctx| {
        received_clone.store(ctx.message().0, Ordering::SeqCst);
        Reply::ready()
    });
    let receiver_handle = receiver.start().await;

    // Create counter
    let mut counter = runtime.new_actor::<Counter>();
    counter.model.count = 42;

    counter.act_on::<GetCount>(|actor, envelope| {
        let count = actor.model.count;
        let reply_envelope = envelope.reply_envelope();

        Reply::pending(async move {
            reply_envelope.send(CountResponse(count)).await;
        })
    });

    let counter_handle = counter.start().await;

    // Query counter
    let envelope = counter_handle.create_envelope(Some(receiver_handle.reply_address()));
    envelope.send(GetCount).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    assert_eq!(received_count.load(Ordering::SeqCst), 42);

    Ok(())
}
