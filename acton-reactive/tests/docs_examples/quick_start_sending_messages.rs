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

//! Tests for examples from docs/quick-start/sending-messages/page.md
//!
//! This module verifies that the code examples from the "Sending Messages" quick start
//! documentation page compile and run correctly.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// The complete request-response example from the docs.
///
/// From: docs/quick-start/sending-messages/page.md - "A Complete Example"
#[acton_test]
async fn test_request_response_example() -> anyhow::Result<()> {
    // The service actor that responds to queries
    #[acton_actor]
    struct Counter {
        count: i32,
    }

    // The client actor that requests data
    #[acton_actor]
    #[derive(Default)]
    struct Client {
        counter: Option<ActorHandle>,
        received_count: Option<i32>,
    }

    // Messages
    #[acton_message]
    struct Increment;

    #[acton_message]
    struct GetCount;

    #[acton_message]
    struct CountResponse(i32);

    #[acton_message]
    struct RequestCount;

    let final_received = Arc::new(AtomicI32::new(-1));
    let final_received_clone = final_received.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Create the counter service
    let mut counter = runtime.new_actor::<Counter>();

    counter
        .mutate_on::<Increment>(|actor, _envelope| {
            actor.model.count += 1;
            Reply::ready()
        })
        .act_on::<GetCount>(|actor, envelope| {
            let count = actor.model.count;
            let reply_envelope = envelope.reply_envelope();

            Reply::pending(async move {
                reply_envelope.send(CountResponse(count)).await;
            })
        });

    let counter_handle = counter.start().await;

    // Create the client that will request data
    let mut client = runtime.new_actor::<Client>();
    client.model.counter = Some(counter_handle.clone());

    client
        .mutate_on::<RequestCount>(|actor, envelope| {
            let counter = actor.model.counter.clone().unwrap();
            let request_envelope = envelope.new_envelope(&counter.reply_address());

            Reply::pending(async move {
                request_envelope.send(GetCount).await;
            })
        })
        .mutate_on::<CountResponse>(move |actor, envelope| {
            let count = envelope.message().0;
            actor.model.received_count = Some(count);
            Reply::ready()
        })
        .after_stop(move |actor| {
            if let Some(count) = actor.model.received_count {
                final_received_clone.store(count, Ordering::SeqCst);
            }
            Reply::ready()
        });

    let client_handle = client.start().await;

    // Increment the counter a few times
    counter_handle.send(Increment).await;
    counter_handle.send(Increment).await;
    counter_handle.send(Increment).await;

    // Ask for the count via the client
    client_handle.send(RequestCount).await;

    // Give time for async messages to process
    tokio::time::sleep(Duration::from_millis(100)).await;

    runtime.shutdown_all().await?;

    // Verify the client received the correct count
    assert_eq!(final_received.load(Ordering::SeqCst), 3);

    Ok(())
}

/// Tests accessing message data through the envelope.
///
/// From: docs/quick-start/sending-messages/page.md - "Accessing Message Data"
#[acton_test]
async fn test_accessing_message_data() -> anyhow::Result<()> {
    #[acton_actor]
    struct Counter {
        count: i32,
    }

    #[acton_message]
    struct IncrementBy {
        amount: i32,
    }

    let final_count = Arc::new(AtomicI32::new(0));
    let final_count_clone = final_count.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut counter = runtime.new_actor::<Counter>();

    counter
        // In handler, access message through envelope.message()
        .mutate_on::<IncrementBy>(|actor, envelope| {
            let amount = envelope.message().amount;
            actor.model.count += amount;
            Reply::ready()
        })
        .after_stop(move |actor| {
            final_count_clone.store(actor.model.count, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = counter.start().await;

    handle.send(IncrementBy { amount: 5 }).await;
    handle.send(IncrementBy { amount: 7 }).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert_eq!(final_count.load(Ordering::SeqCst), 12);

    Ok(())
}

/// Tests Reply::ready() for synchronous completion.
///
/// From: docs/quick-start/sending-messages/page.md - "Reply::ready()"
#[acton_test]
async fn test_reply_ready() -> anyhow::Result<()> {
    #[acton_actor]
    struct Counter {
        count: i32,
    }

    #[acton_message]
    struct Increment;

    let final_count = Arc::new(AtomicI32::new(0));
    let final_count_clone = final_count.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut counter = runtime.new_actor::<Counter>();

    counter
        .mutate_on::<Increment>(|actor, _envelope| {
            actor.model.count += 1;
            Reply::ready() // Synchronous completion
        })
        .after_stop(move |actor| {
            final_count_clone.store(actor.model.count, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = counter.start().await;

    handle.send(Increment).await;
    handle.send(Increment).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert_eq!(final_count.load(Ordering::SeqCst), 2);

    Ok(())
}

/// Tests Reply::pending() for async operations.
///
/// From: docs/quick-start/sending-messages/page.md - "Reply::pending(future)"
#[acton_test]
async fn test_reply_pending() -> anyhow::Result<()> {
    #[acton_actor]
    struct Counter {
        count: i32,
    }

    #[acton_message]
    struct GetCount;

    #[acton_message]
    struct CountResponse(i32);

    let mut runtime = ActonApp::launch_async().await;

    // Create a counter to receive responses
    let mut receiver = runtime.new_actor::<Counter>();
    let received = Arc::new(AtomicI32::new(-1));
    let received_clone = received.clone();

    receiver
        .mutate_on::<CountResponse>(move |_actor, envelope| {
            received_clone.store(envelope.message().0, Ordering::SeqCst);
            Reply::ready()
        });

    let receiver_handle = receiver.start().await;

    // Create the counter service
    let mut counter = runtime.new_actor::<Counter>();
    counter.model.count = 42;

    counter.act_on::<GetCount>(|actor, envelope| {
        let count = actor.model.count;
        let reply_envelope = envelope.reply_envelope();

        // Reply::pending for async work
        Reply::pending(async move {
            reply_envelope.send(CountResponse(count)).await;
        })
    });

    let counter_handle = counter.start().await;

    // Create envelope to send GetCount with reply address pointing to receiver
    let request_envelope = counter_handle.create_envelope(Some(receiver_handle.reply_address()));
    request_envelope.send(GetCount).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    assert_eq!(received.load(Ordering::SeqCst), 42);

    Ok(())
}
