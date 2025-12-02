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

//! Tests for examples from docs/your-first-actor/page.md
//!
//! This module verifies that the code examples from the "Your First Actor" main
//! documentation page compile and run correctly.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// The complete counter example from the main your-first-actor page.
///
/// From: docs/your-first-actor/page.md - "The Complete Example"
#[acton_test]
async fn test_counter_state_example() -> anyhow::Result<()> {
    // This is our actor's "desk" - its private data
    #[acton_actor]
    struct CounterState {
        count: u32,
    }

    // This is a message - a memo telling the counter what to do
    #[acton_message]
    struct Increment(u32);

    let final_count = Arc::new(AtomicU32::new(0));
    let final_count_clone = final_count.clone();

    // Start the "office" (runtime)
    let mut runtime = ActonApp::launch_async().await;

    // Hire a counter actor
    let mut counter = runtime.new_actor::<CounterState>();

    // Tell the actor: "When you get an Increment memo, add to your count"
    counter
        .mutate_on::<Increment>(|actor, ctx| {
            actor.model.count += ctx.message().0;
            Reply::ready()
        })
        .after_stop(move |actor| {
            final_count_clone.store(actor.model.count, Ordering::SeqCst);
            Reply::ready()
        });

    // The actor starts working
    let handle = counter.start().await;

    // Send some memos!
    handle.send(Increment(1)).await;
    handle.send(Increment(2)).await;
    handle.send(Increment(3)).await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Close up shop
    runtime.shutdown_all().await?;

    // Verify: Count is now: 1, then 3, then 6
    assert_eq!(final_count.load(Ordering::SeqCst), 6);

    Ok(())
}

/// Tests adding a query handler with act_on.
///
/// From: docs/your-first-actor/page.md - "Adding a Query Handler"
#[acton_test]
async fn test_query_handler_example() -> anyhow::Result<()> {
    #[acton_actor]
    struct CounterState {
        count: u32,
    }

    #[acton_message]
    struct Increment(u32);

    #[acton_message]
    struct GetCount;

    #[acton_message]
    struct CountResponse(u32);

    let received = Arc::new(AtomicU32::new(0));
    let received_clone = received.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Create a receiver actor to get the response
    let mut receiver = runtime.new_actor::<CounterState>();
    receiver.mutate_on::<CountResponse>(move |_actor, ctx| {
        received_clone.store(ctx.message().0, Ordering::SeqCst);
        Reply::ready()
    });
    let receiver_handle = receiver.start().await;

    // Create the main counter
    let mut counter = runtime.new_actor::<CounterState>();

    // Handler for mutations
    counter.mutate_on::<Increment>(|actor, ctx| {
        actor.model.count += ctx.message().0;
        Reply::ready()
    });

    // Handler for queries (read-only)
    counter.act_on::<GetCount>(|actor, ctx| {
        let count = actor.model.count;
        let reply = ctx.reply_envelope();

        Reply::pending(async move {
            reply.send(CountResponse(count)).await;
        })
    });

    let handle = counter.start().await;

    handle.send(Increment(5)).await;
    handle.send(Increment(3)).await;

    // Send GetCount with receiver's address as reply destination
    let envelope = handle.create_envelope(Some(receiver_handle.reply_address()));
    envelope.send(GetCount).await;

    // Give time for the reply to process
    tokio::time::sleep(Duration::from_millis(100)).await;

    runtime.shutdown_all().await?;

    // Verify the response was received
    assert_eq!(received.load(Ordering::SeqCst), 8);

    Ok(())
}
