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
//! This module verifies that the code examples from the "Your First Actor" main
//! documentation page compile and run correctly.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// The complete counter example from the main your-first-actor page.
///
/// From: docs/quick-start/your-first-actor/page.md - "The Complete Example"
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

    // `shutdown_all` enqueues `Terminate` behind the three sends above, and an actor
    // drains everything ahead of it, so no wait is needed here.
    runtime.shutdown_all().await?;

    // Verify: Count is now: 1, then 3, then 6
    assert_eq!(final_count.load(Ordering::SeqCst), 6);

    Ok(())
}

/// Tests adding a query handler with `act_on`.
///
/// From: docs/quick-start/your-first-actor/page.md - "Adding a Query Handler"
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

    impl Request for GetCount {
        type Response = CountResponse;
    }

    let mut runtime = ActonApp::launch_async().await;

    // Create the main counter (responder)
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

    let counter_handle = counter.start().await;

    // Increment counter
    counter_handle.send(Increment(5)).await;
    counter_handle.send(Increment(3)).await;

    // The query waits for its answer, and the answer cannot be produced until both
    // increments ahead of it in the inbox have been applied.
    let response: CountResponse = counter_handle.ask(GetCount).await?;
    assert_eq!(response.0, 8);

    runtime.shutdown_all().await?;

    Ok(())
}
