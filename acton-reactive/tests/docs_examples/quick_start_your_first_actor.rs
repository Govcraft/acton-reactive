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

//! Tests for examples from docs/quick-start/your-first-actor/page.md
//!
//! This module verifies that the code examples from the "Your First Actor" quick start
//! documentation page compile and run correctly.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// The complete counter example from the quick start page.
///
/// From: docs/quick-start/your-first-actor/page.md - "The Complete Example"
#[acton_test]
async fn test_complete_counter_example() -> anyhow::Result<()> {
    // 1. Define your actor's data
    #[acton_actor]
    struct Counter {
        count: i32,
    }

    // 2. Define messages it can receive
    #[acton_message]
    struct Increment;

    #[acton_message]
    struct PrintCount;

    // Track the final count for verification
    let final_count = Arc::new(AtomicI32::new(0));
    let final_count_clone = final_count.clone();

    // Start the actor system
    let mut runtime = ActonApp::launch_async().await;

    // Create and configure the actor
    let mut counter = runtime.new_actor::<Counter>();

    counter
        .mutate_on::<Increment>(|actor, _envelope| {
            actor.model.count += 1;
            Reply::ready()
        })
        .act_on::<PrintCount>(|actor, _envelope| {
            // Instead of println, we verify the count
            assert_eq!(actor.model.count, 3);
            Reply::ready()
        })
        .after_stop(move |actor| {
            final_count_clone.store(actor.model.count, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = counter.start().await;

    // Send some messages
    handle.send(Increment).await;
    handle.send(Increment).await;
    handle.send(Increment).await;
    handle.send(PrintCount).await;

    // Wait for messages to be processed
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Clean shutdown
    runtime.shutdown_all().await?;

    // Verify final count
    assert_eq!(final_count.load(Ordering::SeqCst), 3);

    Ok(())
}

/// Tests message with data - the IncrementBy example from the docs.
///
/// From: docs/quick-start/your-first-actor/page.md - "Messages can also include fields"
#[acton_test]
async fn test_message_with_data() -> anyhow::Result<()> {
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
        .mutate_on::<IncrementBy>(|actor, envelope| {
            actor.model.count += envelope.message().amount;
            Reply::ready()
        })
        .after_stop(move |actor| {
            final_count_clone.store(actor.model.count, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = counter.start().await;

    handle.send(IncrementBy { amount: 5 }).await;
    handle.send(IncrementBy { amount: 10 }).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert_eq!(final_count.load(Ordering::SeqCst), 15);

    Ok(())
}

/// Tests the mutate_on vs act_on distinction from the docs.
///
/// From: docs/quick-start/your-first-actor/page.md - "mutate_on vs act_on"
#[acton_test]
async fn test_mutate_on_vs_act_on() -> anyhow::Result<()> {
    #[acton_actor]
    struct Counter {
        count: i32,
    }

    #[acton_message]
    struct Increment;

    #[acton_message]
    struct GetCount;

    let final_count = Arc::new(AtomicI32::new(0));
    let final_count_clone = final_count.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut counter = runtime.new_actor::<Counter>();

    counter
        // mutate_on for messages that change the actor's state
        .mutate_on::<Increment>(|actor, _envelope| {
            actor.model.count += 1;
            Reply::ready()
        })
        // act_on for messages that only read the state
        .act_on::<GetCount>(|actor, _envelope| {
            // Verify count is readable
            let _count = actor.model.count;
            Reply::ready()
        })
        .after_stop(move |actor| {
            final_count_clone.store(actor.model.count, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = counter.start().await;

    handle.send(Increment).await;
    handle.send(Increment).await;
    handle.send(GetCount).await;
    handle.send(Increment).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert_eq!(final_count.load(Ordering::SeqCst), 3);

    Ok(())
}
