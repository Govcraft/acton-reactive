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
#![allow(dead_code, unused_doc_comments)]

use acton_reactive::prelude::*;
use acton_test::prelude::*;

// Use direct paths as re-exports seem problematic in test context
use crate::setup::{actors::pool_item::PoolItem, initialize_tracing};

mod setup;

/// Tests the basic actor lifecycle event handlers: `after_start` and `after_stop`.
///
/// **Scenario:**
/// 1. Launch the runtime.
/// 2. Create a `PoolItem` actor builder.
/// 3. Register an `after_start` handler that logs a message with the actor's ID.
/// 4. Register an `after_stop` handler that logs a message with the actor's ID.
/// 5. Start the actor.
/// 6. Immediately stop the actor.
///
/// **Verification:**
/// - Relies on observing the tracing output to confirm that the log messages from
///   both `after_start` and `after_stop` handlers are printed, indicating they were executed.
#[acton_test]
async fn test_actor_lifecycle_events() -> anyhow::Result<()> {
    initialize_tracing();
    // Launch the runtime environment.
    let mut runtime: ActorRuntime = ActonApp::launch();
    // Create an actor builder for the PoolItem state.
    let mut pool_item_actor_builder = runtime.new_actor::<PoolItem>();

    // Configure the lifecycle handlers.
    pool_item_actor_builder
        // Register a function to run immediately after the actor's task starts.
        .after_start(|actor| {
            tracing::info!("Actor started with ID: {}", actor.id());
            Reply::ready()
        })
        // Register a function to run after the actor has processed all messages
        // following a stop signal and its task is about to terminate.
        .after_stop(|actor| {
            tracing::info!("Actor stopping with ID: {}", actor.id());
            Reply::ready()
        });

    // Start the actor, spawning its task and returning a handle.
    let actor_handle = pool_item_actor_builder.start().await;
    // Immediately send a stop signal to the actor and wait for it to shut down.
    // This will trigger the `after_stop` handler.
    actor_handle.stop().await?;
    Ok(())
}
