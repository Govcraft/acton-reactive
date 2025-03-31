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

use std::time::Duration;

use tracing::*;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

// Use direct paths as re-exports seem problematic in test context
use crate::setup::{
    actors::{comedian::Comedian, counter::Counter, parent_child::Parent},
    messages::{Ping, Pong},
    initialize_tracing,
};

mod setup;

/// Tests spawning agents using `spawn_agent_with_setup_fn`, including spawning a child agent
/// from within the parent's setup function.
///
/// **Scenario:**
/// 1. Launch runtime and get broker handle.
/// 2. Define a parent agent config.
/// 3. Use `runtime.spawn_agent_with_setup_fn` to create the parent agent:
///    - Inside the parent's setup closure:
///        - Define a child agent config.
///        - Clone the runtime handle (`agent.runtime()`).
///        - Use the cloned runtime handle to `spawn_agent_with_setup_fn` for the child agent.
///        - Inside the child's setup closure:
///            - Configure a `Pong` handler.
///            - Subscribe the child to `Pong`.
///            - Start the child agent (`child_builder.start().await`).
///        - Configure `Ping` and `Pong` handlers for the parent. The `Pong` handler uses an async block with a delay.
///        - Subscribe the parent to `Ping` and `Pong`.
///        - Start the parent agent (`parent_builder.start().await`).
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
    let mut runtime: AgentRuntime = ActonApp::launch();
    let broker_handle = runtime.broker();

    // Configuration for the parent agent.
    let parent_config = AgentConfig::new(Ern::with_root("parent")?, None, None)?;

    // Clone broker handle for broadcasting later.
    let broker_handle_clone = broker_handle.clone();

    // Spawn the parent agent using a setup function. This allows complex initialization logic,
    // including spawning other agents, before the parent agent starts its main loop.
    // The function returns the handle of the started agent.
    let _parent_handle = runtime
        .spawn_agent_with_setup_fn::<Parent>(parent_config, |mut parent_builder| {
            // This async block is the setup function for the parent agent.
            // It receives the `ManagedAgent` builder (`parent_builder`) in its `Idle` state.
            Box::pin(async move {
                // Configuration for the child agent.
                let child_config = AgentConfig::new(
                    Ern::with_root("child").expect("Could not create child ARN root"),
                    None,
                    None,
                )
                     .expect("Couldn't create child config");

                // Get a clone of the runtime handle from the parent builder's context.
                // This is needed to spawn the child agent.
                let mut runtime_clone = parent_builder.runtime().clone();

                // Spawn the child agent using the cloned runtime handle and another setup function.
                let _child_handle = runtime_clone
                    .spawn_agent_with_setup_fn::<Parent>(child_config, |mut child_builder| {
                        // This async block is the setup function for the child agent.
                        Box::pin(async move {
                            // Configure child's Pong handler.
                            child_builder.act_on::<Pong>(|_agent, _envelope| {
                                info!("CHILD SUCCESS! PONG!");
                                AgentReply::immediate()
                            });

                            // Subscribe child to Pong messages using its builder handle.
                            let child_builder_handle = &child_builder.handle().clone();
                            child_builder_handle.subscribe::<Pong>().await;
                            // Start the child agent from within its setup function.
                            child_builder.start().await
                        })
                    })
                    .await
                    .expect("Couldn't create child actor");
                
                // Configure parent's handlers.
                parent_builder
                    .act_on::<Ping>(|_agent, _envelope| {
                        info!("SUCCESS! PING!");
                        AgentReply::immediate()
                    })
                    // Pong handler includes an async delay.
                    .act_on::<Pong>(|_agent, _envelope| AgentReply::from_async(wait_and_respond()));

                // Subscribe parent to messages using its builder handle.
                let parent_builder_handle = &parent_builder.handle().clone();
                parent_builder_handle.subscribe::<Ping>().await;
                parent_builder_handle.subscribe::<Pong>().await;

                // Start the parent agent from within its setup function.
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

/// Tests spawning agents using the simpler `spawn_agent` method, which takes a setup closure
/// but doesn't require explicit agent configuration beforehand (uses defaults).
///
/// **Scenario:**
/// 1. Launch runtime and get broker handle.
/// 2. Use `runtime.spawn_agent` for a `Comedian` agent:
///    - Inside the setup closure:
///        - Configure `Ping` and `Pong` handlers.
///        - Subscribe to `Ping` and `Pong`.
///        - Start the agent (`agent_builder.start().await`).
/// 3. Use `runtime.spawn_agent` for a `Counter` agent:
///    - Inside the setup closure:
///        - Configure a `Pong` handler.
///        - Subscribe to `Pong`.
///        - Start the agent (`agent_builder.start().await`).
/// 4. Assert that the runtime now has more than 0 agents (implicitly, the broker + 2 spawned).
/// 5. Broadcast `Ping` and `Pong` messages.
/// 6. Shut down the runtime.
///
/// **Verification:**
/// - `Comedian` receives `Ping` and `Pong`.
/// - `Counter` receives `Pong`.
/// - `agent_count` assertion passes.
/// - Tracing logs confirm message flow.
#[acton_test]
async fn test_launchpad() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: AgentRuntime = ActonApp::launch();
    let broker_handle = runtime.broker();

    // Spawn Comedian using default config + setup function.
    let _comedian_handle = runtime
        .spawn_agent::<Comedian>(|mut agent_builder| {
            // Setup closure for Comedian.
            Box::pin(async move {
                agent_builder
                    .act_on::<Ping>(|_agent, _envelope| {
                        info!("SUCCESS! PING!");
                        AgentReply::immediate()
                    })
                    .act_on::<Pong>(|_agent, _envelope| {
                        Box::pin(async move {
                            info!("SUCCESS! PONG!");
                        })
                    });

                agent_builder.handle().subscribe::<Ping>().await;
                agent_builder.handle().subscribe::<Pong>().await;
                agent_builder.start().await // Start the agent within the setup closure
            })
        })
        .await?;

    // Spawn Counter using default config + setup function.
    let _counter_handle = runtime
        .spawn_agent::<Counter>(|mut agent_builder| {
            // Setup closure for Counter.
            Box::pin(async move {
                agent_builder.act_on::<Pong>(|_agent, _envelope| {
                    Box::pin(async move {
                        info!("SUCCESS! PONG!");
                    })
                });

                agent_builder.handle().subscribe::<Pong>().await;
                agent_builder.start().await // Start the agent within the setup closure
            })
        })
        .await?;

    // Verify agents were actually spawned (broker counts as 1, so > 1 means success).
    assert!(runtime.agent_count() > 1, "Expected more than 1 agent (broker + spawned)");

    // Broadcast messages.
    broker_handle.broadcast(Ping).await; // To Comedian
    broker_handle.broadcast(Pong).await; // To Comedian and Counter

    // Shut down.
    runtime.shutdown_all().await?;
    Ok(())
}
