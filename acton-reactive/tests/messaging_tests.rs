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

use tracing::*;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

// Use direct paths as re-exports seem problematic in test context
use crate::setup::{
    actors::{messenger::Messenger, pool_item::PoolItem},
    initialize_tracing,
    messages::Ping,
};

mod setup;

/// Tests basic message sending and handling for a `PoolItem` actor.
///
/// **Scenario:**
/// 1. Launch runtime.
/// 2. Create a `PoolItem` actor builder.
/// 3. Configure a handler for `Ping` messages that increments the actor's `receive_count`.
/// 4. Configure an `after_stop` handler to log the final count.
/// 5. Start the actor.
/// 6. Send one `Ping` message to the actor.
/// 7. Stop the actor.
///
/// **Verification:**
/// - Tracing logs show the "Received in sync handler" message.
/// - Tracing logs from `after_stop` show "Processed 1 Pings".
#[acton_test]
async fn test_messaging_behavior() -> anyhow::Result<()> {
    initialize_tracing();
    // Launch the runtime environment.
    let mut runtime: ActorRuntime = ActonApp::launch();
    // Create an actor builder for PoolItem state.
    let mut pool_item_actor_builder = runtime.new_actor::<PoolItem>();

    // Configure the actor's behavior.
    pool_item_actor_builder
        // Handler for `Ping` messages.
        .mutate_on::<Ping>(|actor, _envelope| {
            let type_name = std::any::type_name::<Ping>();
            info!(type_name = type_name, "Received in sync handler");
            // Mutate the actor's internal state.
            actor.model.receive_count += 1;
            Reply::ready()
        })
        // Handler executed after the actor stops.
        .after_stop(|actor| {
            assert_eq!(actor.model.receive_count, 1, "expected one Ping");
            info!("Processed {} Pings", actor.model.receive_count);
            // Implicitly verifies count is 1 based on the log message.
            Reply::ready()
        });

    // Start the actor and get its handle.
    let actor_handle = pool_item_actor_builder.start().await;
    // Send a message to the running actor.
    actor_handle.send(Ping).await;
    // Stop the actor.
    actor_handle.stop().await?;
    Ok(())
}

/// Tests basic message sending and handling for a `Messenger` actor (no internal state).
///
/// **Scenario:**
/// 1. Launch runtime.
/// 2. Create a `Messenger` actor builder.
/// 3. Configure a handler for `Ping` messages that logs receipt.
/// 4. Configure a simple `after_stop` handler.
/// 5. Start the actor.
/// 6. Send one `Ping` message.
/// 7. Stop the actor.
///
/// **Verification:**
/// - Tracing logs show the "Received in Messenger handler" message.
#[acton_test]
async fn test_basic_messenger() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch();
    // Create an actor builder for Messenger state (which is likely empty or minimal).
    let mut messenger_actor_builder = runtime.new_actor::<Messenger>();

    // Configure the actor's behavior.
    messenger_actor_builder
        // Handler for `Ping` messages.
        .mutate_on::<Ping>(|_actor, _envelope| {
            let type_name = std::any::type_name::<Ping>();
            info!(type_name = type_name, "Received in Messenger handler");
            Reply::ready()
        })
        // Handler executed after the actor stops.
        .after_stop(|_actor| {
            info!("Stopping");
            Reply::ready()
        });

    // Start the actor.
    let actor_handle = messenger_actor_builder.start().await;
    // Send a message.
    actor_handle.send(Ping).await;
    // Stop the actor.
    actor_handle.stop().await?;
    Ok(())
}

/// Tests basic message handling, seemingly intended to test async handlers but uses `Reply::immediate`.
/// Functionally similar to `test_messaging_behavior`.
///
/// **Scenario:**
/// 1. Launch runtime.
/// 2. Create a `PoolItem` actor builder.
/// 3. Configure a handler for `Ping` messages that increments `receive_count`.
/// 4. Configure an `after_stop` handler to log the final count.
/// 5. Start the actor.
/// 6. Send one `Ping` message.
/// 7. Stop the actor.
///
/// **Verification:**
/// - Tracing logs show the "Received in async handler" message.
/// - Tracing logs from `after_stop` show "Processed 1 Pings".
#[acton_test]
async fn test_async_messaging_behavior() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch();
    let mut pool_item_actor_builder = runtime.new_actor::<PoolItem>();

    pool_item_actor_builder
        // Handler for `Ping` messages. Although the test name suggests async,
        // this handler currently returns `Reply::ready()`.
        .mutate_on::<Ping>(|actor, _envelope| {
            let type_name = std::any::type_name::<Ping>();
            info!(type_name = type_name, "Received in async handler");
            actor.model.receive_count += 1;
            Reply::ready()
        })
        .after_stop(|actor| {
            assert_eq!(actor.model.receive_count, 1, "expected one Ping");
            info!("Processed {} Pings", actor.model.receive_count);
            // Implicitly verifies count is 1.
            Reply::ready()
        });

    // Start the actor.
    let actor_handle = pool_item_actor_builder.start().await;
    // Send a message.
    actor_handle.send(Ping).await;
    // Stop the actor.
    actor_handle.stop().await?;
    Ok(())
}
