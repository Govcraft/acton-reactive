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
    actors::{comedian::Comedian, counter::Counter},
    initialize_tracing,
    messages::{Ping, Pong},
};

mod setup;

/// Tests the basic publish-subscribe functionality of the `ActorBroker`.
///
/// **Scenario:**
/// 1. Launch the runtime and get the central broker handle.
/// 2. Create two actors: `Comedian` and `Counter`. The `Counter` is configured with the broker handle.
/// 3. Configure handlers:
///    - `Counter` handles `Pong` (increments count). Asserts count is 1 on stop.
///    - `Comedian` handles `Ping` and `Pong` (increments `funny` count). Asserts `funny` is 2 on stop.
/// 4. Subscribe actors to message types using their handles:
///    - `Counter` subscribes to `Pong`.
///    - `Comedian` subscribes to `Ping` and `Pong`.
/// 5. Start both actors.
/// 6. Broadcast `Ping` and `Pong` messages using the broker handle obtained from the started `Comedian` actor.
/// 7. Shut down the runtime (which stops all actors and triggers `after_stop` handlers).
///
/// **Verification:**
/// - `Comedian` receives `Ping` (funny=1) and `Pong` (funny=2). `after_stop` asserts `funny == 2`.
/// - `Counter` receives `Pong` (count=1). `after_stop` asserts `count == 1`.
#[acton_test]
async fn test_broker() -> anyhow::Result<()> {
    initialize_tracing();
    // Launch the runtime environment.
    let mut runtime: ActorRuntime = ActonApp::launch();
    // Get a handle to the central message broker.
    let broker_handle = runtime.broker();

    // --- Comedian Actor ---
    // Create the actor builder. No broker needed in config as it gets it automatically.
    let mut comedian_actor_builder = runtime.new_actor::<Comedian>();

    // --- Counter Actor ---
    // Configure the counter actor, explicitly providing the broker handle.
    let counter_config = ActorConfig::new(
        Ern::with_root("counter").unwrap(),
        None,
        Some(broker_handle.clone()), // Provide broker handle for potential direct use (though subscribe uses handle's broker)
    )?;
    let mut counter_actor_builder = runtime.new_actor_with_config::<Counter>(counter_config);

    // Configure Counter actor's handlers.
    counter_actor_builder
        .mutate_on::<Pong>(|actor, _envelope| {
            info!("Also SUCCESS! PONG!");
            actor.model.count += 1;
            ActorReply::immediate()
        })
        .after_stop(|actor| {
            assert_eq!(actor.model.count, 1, "count should be 1");
            ActorReply::immediate()
        });

    // Configure Comedian actor's handlers.
    comedian_actor_builder
        .mutate_on::<Ping>(|actor, _envelope| {
            info!("SUCCESS! PING!");
            actor.model.funny += 1;
            ActorReply::immediate()
        })
        .mutate_on::<Pong>(|actor, _envelope| {
            actor.model.funny += 1;
            info!("SUCCESS! PONG!");
            ActorReply::immediate()
        })
        .after_stop(|actor| {
            assert_eq!(actor.model.funny, 2, "funny count should be 2");
            ActorReply::immediate()
        });

    // Subscribe actors to messages *before* starting them.
    // The `subscribe` method uses the broker reference stored within the actor's handle.
    counter_actor_builder.handle().subscribe::<Pong>().await;
    comedian_actor_builder.handle().subscribe::<Ping>().await;
    comedian_actor_builder.handle().subscribe::<Pong>().await;

    // Start the actors.
    let comedian_handle = comedian_actor_builder.start().await;
    let _counter_handle = counter_actor_builder.start().await; // Handle not used directly after start

    // Broadcast messages using the broker reference held by the comedian's handle.
    // Any actor subscribed to these message types will receive them.
    if let Some(broker_ref) = comedian_handle.broker.as_ref() {
        broker_ref.broadcast(Ping).await; // Should be received by Comedian
        broker_ref.broadcast(Pong).await; // Should be received by Comedian and Counter
    }

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Shut down the runtime, stopping all actors and running `after_stop` handlers.
    runtime.shutdown_all().await?;

    Ok(())
}

#[acton_test]
async fn test_broker_from_handler() -> anyhow::Result<()> {
    /// Tests broadcasting a message from within an actor's message handler.
    ///
    /// **Scenario:**
    /// 1. Launch runtime, get broker handle.
    /// 2. Create `Comedian` and `Counter` actors.
    /// 3. Configure handlers:
    ///    - `Counter` handles `Pong` (increments count). Asserts count is 1 on stop.
    ///    - `Comedian` handles `Ping`:
    ///        - Gets its own broker handle using `actor.broker()`.
    ///        - Asynchronously broadcasts a `Pong` message.
    /// 4. Subscribe actors:
    ///    - `Counter` subscribes to `Pong`.
    ///    - `Comedian` subscribes to `Ping`.
    /// 5. Start both actors.
    /// 6. Broadcast an initial `Ping` message using the main broker handle.
    /// 7. Shut down the runtime.
    ///
    /// **Verification:**
    /// - Initial `Ping` broadcast is received by `Comedian`.
    /// - `Comedian`'s `Ping` handler broadcasts `Pong`.
    /// - `Pong` broadcast is received by `Counter` (count=1).
    /// - `Counter`'s `after_stop` handler asserts `count == 1`.
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch();
    let broker_handle = runtime.broker();

    // --- Comedian Actor ---
    let mut comedian_actor_builder = runtime.new_actor::<Comedian>();

    // --- Counter Actor ---
    let counter_config = ActorConfig::new(
        Ern::with_root("counter").unwrap(),
        None,
        Some(broker_handle.clone()),
    )?;
    let mut counter_actor_builder = runtime.new_actor_with_config::<Counter>(counter_config);

    // Configure Counter handler.
    counter_actor_builder
        .mutate_on::<Pong>(|actor, _envelope| {
            info!("Also SUCCESS! PONG!");
            actor.model.count += 1;
            ActorReply::immediate()
        })
        .after_stop(|actor| {
            assert_eq!(actor.model.count, 1, "count should be 1");
            ActorReply::immediate()
        });

    // Configure Comedian handler to broadcast from within.
    comedian_actor_builder.mutate_on::<Ping>(|actor, _envelope| {
        // Get the broker handle associated with this actor.
        let actor_broker_handle = actor.broker().clone();
        // Return an async block to perform the broadcast.
        Box::pin(async move {
            // Broadcast Pong using the handle obtained within the actor context.
            actor_broker_handle.broadcast(Pong).await;
        })
    });

    // Subscribe actors.
    counter_actor_builder.handle().subscribe::<Pong>().await; // Counter listens for Pong
    comedian_actor_builder.handle().subscribe::<Ping>().await; // Comedian listens for Ping

    // Start actors.
    let _comedian_handle = comedian_actor_builder.start().await;
    let _counter_handle = counter_actor_builder.start().await;

    // Initiate the sequence by broadcasting Ping using the main runtime broker handle.
    // This will trigger the Comedian's Ping handler, which will then broadcast Pong.
    broker_handle.broadcast(Ping).await;

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Shut down the runtime.
    runtime.shutdown_all().await?;

    Ok(())
}
