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

use std::time::Duration;

use tracing::*;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

// Use direct paths as re-exports seem problematic in test context
use crate::setup::{
    actors::{comedian::Comedian, counter::Counter, parent_child::Parent},
    initialize_tracing,
    messages::{Ping, Pong},
};

mod setup;

/// Tests spawning actors using `spawn_actor_with_setup_fn`, including spawning a child actor
/// from within the parent's setup function.
///
/// **Scenario:**
/// 1. Launch runtime and get broker handle.
/// 2. Define a parent actor config.
/// 3. Use `runtime.spawn_actor_with_setup_fn` to create the parent actor:
///    - Inside the parent's setup closure:
///        - Define a child actor config.
///        - Clone the runtime handle (`actor.runtime()`).
///        - Use the cloned runtime handle to `spawn_actor_with_setup_fn` for the child actor.
///        - Inside the child's setup closure:
///            - Configure a `Pong` handler.
///            - Subscribe the child to `Pong`.
///            - Start the child actor (`child_builder.start().await`).
///        - Configure `Ping` and `Pong` handlers for the parent. The `Pong` handler uses an async block with a delay.
///        - Subscribe the parent to `Ping` and `Pong`.
///        - Start the parent actor (`parent_builder.start().await`).
/// 4. Broadcast `Ping` and `Pong` messages using the main broker handle.
/// 5. Shut down the runtime.
///
/// **Verification:**
/// - Parent receives `Ping`.
/// - Child receives `Pong`.
/// - Parent receives `Pong` (after delay).
/// - Tracing logs confirm the message flow and handler execution.
#[acton_test]
async fn test_launch_passing_acton() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let broker_handle = runtime.broker();

    // Configuration for the parent actor.
    let parent_config = ActorConfig::new(Ern::with_root("parent")?, None, None)?;

    // Clone broker handle for broadcasting later.
    let broker_handle_clone = broker_handle.clone();

    // Spawn the parent actor using a setup function. This allows complex initialization logic,
    // including spawning other actors, before the parent actor starts its main loop.
    // The function returns the handle of the started actor.
    let _parent_handle = runtime
        .spawn_actor_with_setup_fn::<Parent>(parent_config, |mut parent_builder| {
            // This async block is the setup function for the parent actor.
            // It receives the `ManagedActor` builder (`parent_builder`) in its `Idle` state.
            Box::pin(async move {
                // Configuration for the child actor.
                let child_config = ActorConfig::new(
                    Ern::with_root("child").expect("Could not create child ARN root"),
                    None,
                    None,
                )
                .expect("Couldn't create child config");

                // Get a clone of the runtime handle from the parent builder's context.
                // This is needed to spawn the child actor.
                let mut runtime_clone = parent_builder.runtime().clone();

                // Spawn the child actor using the cloned runtime handle and another setup function.
                let _child_handle = runtime_clone
                    .spawn_actor_with_setup_fn::<Parent>(child_config, |mut child_builder| {
                        // This async block is the setup function for the child actor.
                        Box::pin(async move {
                            // Configure child's Pong handler.
                            child_builder.mutate_on::<Pong>(|_actor, _envelope| {
                                info!("CHILD SUCCESS! PONG!");
                                Reply::ready()
                            });

                            // Subscribe child to Pong messages using its builder handle.
                            let child_builder_handle = &child_builder.handle().clone();
                            child_builder_handle.subscribe::<Pong>().await;
                            // Start the child actor from within its setup function.
                            child_builder.start().await
                        })
                    })
                    .await
                    .expect("Couldn't create child actor");

                // Configure parent's handlers.
                parent_builder
                    .mutate_on::<Ping>(|_actor, _envelope| {
                        info!("SUCCESS! PING!");
                        Reply::ready()
                    })
                    // Pong handler includes an async delay.
                    .mutate_on::<Pong>(|_actor, _envelope| Reply::pending(wait_and_respond()));

                // Subscribe parent to messages using its builder handle.
                let parent_builder_handle = &parent_builder.handle().clone();
                parent_builder_handle.subscribe::<Ping>().await;
                parent_builder_handle.subscribe::<Pong>().await;

                // Start the parent actor from within its setup function.
                parent_builder.start().await
            })
        })
        .await?;

    // Broadcast messages using the cloned broker handle.
    broker_handle_clone.broadcast(Ping).await; // Should trigger parent's Ping handler
    broker_handle_clone.broadcast(Pong).await; // Should trigger child's and parent's Pong handlers

    // Shut down the runtime.
    runtime.shutdown_all().await?;
    Ok(())
}

async fn wait_and_respond() {
    tokio::time::sleep(Duration::from_secs(1)).await;
    info!("Waited, then...SUCCESS! PONG!");
}

/// Tests spawning actors using the simpler `spawn_actor` method, which takes a setup closure
/// but doesn't require explicit actor configuration beforehand (uses defaults).
///
/// **Scenario:**
/// 1. Launch runtime and get broker handle.
/// 2. Use `runtime.spawn_actor` for a `Comedian` actor:
///    - Inside the setup closure:
///        - Configure `Ping` and `Pong` handlers.
///        - Subscribe to `Ping` and `Pong`.
///        - Start the actor (`actor_builder.start().await`).
/// 3. Use `runtime.spawn_actor` for a `Counter` actor:
///    - Inside the setup closure:
///        - Configure a `Pong` handler.
///        - Subscribe to `Pong`.
///        - Start the actor (`actor_builder.start().await`).
/// 4. Assert that the runtime now has more than 0 actors (implicitly, the broker + 2 spawned).
/// 5. Broadcast `Ping` and `Pong` messages.
/// 6. Shut down the runtime.
///
/// **Verification:**
/// - `Comedian` receives `Ping` and `Pong`.
/// - `Counter` receives `Pong`.
/// - `actor_count` assertion passes.
/// - Tracing logs confirm message flow.
#[acton_test]
async fn test_launchpad() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let broker_handle = runtime.broker();

    // Spawn Comedian using default config + setup function.
    let _comedian_handle = runtime
        .spawn_actor::<Comedian>(|mut actor_builder| {
            // Setup closure for Comedian.
            Box::pin(async move {
                actor_builder
                    .mutate_on::<Ping>(|_actor, _envelope| {
                        info!("SUCCESS! PING!");
                        Reply::ready()
                    })
                    .mutate_on::<Pong>(|_actor, _envelope| {
                        Reply::pending(async move {
                            info!("SUCCESS! PONG!");
                        })
                    });

                actor_builder.handle().subscribe::<Ping>().await;
                actor_builder.handle().subscribe::<Pong>().await;
                actor_builder.start().await // Start the actor within the setup closure
            })
        })
        .await?;

    // Spawn Counter using default config + setup function.
    let _counter_handle = runtime
        .spawn_actor::<Counter>(|mut actor_builder| {
            // Setup closure for Counter.
            Box::pin(async move {
                actor_builder.mutate_on::<Pong>(|_actor, _envelope| {
                    Reply::pending(async move {
                        info!("SUCCESS! PONG!");
                    })
                });

                actor_builder.handle().subscribe::<Pong>().await;
                actor_builder.start().await // Start the actor within the setup closure
            })
        })
        .await?;

    // Verify actors were actually spawned (broker counts as 1, so > 1 means success).
    assert!(
        runtime.actor_count() > 1,
        "Expected more than 1 actor (broker + spawned)"
    );

    // Broadcast messages.
    broker_handle.broadcast(Ping).await; // To Comedian
    broker_handle.broadcast(Pong).await; // To Comedian and Counter

    // Shut down.
    runtime.shutdown_all().await?;
    Ok(())
}
