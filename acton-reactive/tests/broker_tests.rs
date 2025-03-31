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
use std::sync::Arc;

use tracing::*;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

use crate::setup::*;

mod setup;

/// Tests the basic publish-subscribe functionality of the `AgentBroker`.
///
/// **Scenario:**
/// 1. Launch the runtime and get the central broker handle.
/// 2. Create two agents: `Comedian` and `Counter`. The `Counter` is configured with the broker handle.
/// 3. Configure handlers:
///    - `Counter` handles `Pong` (increments count). Asserts count is 1 on stop.
///    - `Comedian` handles `Ping` and `Pong` (increments `funny` count). Asserts `funny` is 2 on stop.
/// 4. Subscribe agents to message types using their handles:
///    - `Counter` subscribes to `Pong`.
///    - `Comedian` subscribes to `Ping` and `Pong`.
/// 5. Start both agents.
/// 6. Broadcast `Ping` and `Pong` messages using the broker handle obtained from the started `Comedian` agent.
/// 7. Shut down the runtime (which stops all agents and triggers `after_stop` handlers).
///
/// **Verification:**
/// - `Comedian` receives `Ping` (funny=1) and `Pong` (funny=2). `after_stop` asserts `funny == 2`.
/// - `Counter` receives `Pong` (count=1). `after_stop` asserts `count == 1`.
#[acton_test]
async fn test_broker() -> anyhow::Result<()> {
    initialize_tracing();
    // Launch the runtime environment.
    let mut runtime: AgentRuntime = ActonApp::launch();
    // Get a handle to the central message broker.
    let broker_handle = runtime.broker();

    // --- Comedian Agent ---
    // Create the agent builder. No broker needed in config as it gets it automatically.
    let mut comedian_agent_builder = runtime.new_agent::<Comedian>().await;

    // --- Counter Agent ---
    // Configure the counter agent, explicitly providing the broker handle.
    let counter_config = AgentConfig::new(
        Ern::with_root("counter").unwrap(),
        None,
        Some(broker_handle.clone()), // Provide broker handle for potential direct use (though subscribe uses handle's broker)
    )?;
    let mut counter_agent_builder = runtime.new_agent_with_config::<Counter>(counter_config).await;

    // Configure Counter agent's handlers.
    counter_agent_builder.act_on::<Pong>(|agent, _envelope| {
        info!("Also SUCCESS! PONG!");
        agent.model.count += 1;
        AgentReply::immediate()
    }).after_stop(|agent| {
        assert_eq!(agent.model.count, 1, "count should be 1");
        AgentReply::immediate()
    });

    // Configure Comedian agent's handlers.
    comedian_agent_builder
        .act_on::<Ping>(|agent, _envelope| {
            info!("SUCCESS! PING!");
            agent.model.funny += 1;
            AgentReply::immediate()
        })
        .act_on::<Pong>(|agent, _envelope| {
            agent.model.funny += 1;
            info!("SUCCESS! PONG!");
            AgentReply::immediate()
        })
        .after_stop(|agent| {
            assert_eq!(agent.model.funny, 2, "funny count should be 2");
            AgentReply::immediate()
        });

    // Subscribe agents to messages *before* starting them.
    // The `subscribe` method uses the broker reference stored within the agent's handle.
    counter_agent_builder.handle().subscribe::<Pong>().await;
    comedian_agent_builder.handle().subscribe::<Ping>().await;
    comedian_agent_builder.handle().subscribe::<Pong>().await;

    // Start the agents.
    let comedian_handle = comedian_agent_builder.start().await;
    let _counter_handle = counter_agent_builder.start().await; // Handle not used directly after start

    // Broadcast messages using the broker reference held by the comedian's handle.
    // Any agent subscribed to these message types will receive them.
    if let Some(broker_ref) = comedian_handle.broker.as_ref() {
        broker_ref.broadcast(Ping).await; // Should be received by Comedian
        broker_ref.broadcast(Pong).await; // Should be received by Comedian and Counter
    }

    // Shut down the runtime, stopping all agents and running `after_stop` handlers.
    runtime.shutdown_all().await?;

    Ok(())
}

#[acton_test]
async fn test_broker_from_handler() -> anyhow::Result<()> {
    /// Tests broadcasting a message from within an agent's message handler.
    ///
    /// **Scenario:**
    /// 1. Launch runtime, get broker handle.
    /// 2. Create `Comedian` and `Counter` agents.
    /// 3. Configure handlers:
    ///    - `Counter` handles `Pong` (increments count). Asserts count is 1 on stop.
    ///    - `Comedian` handles `Ping`:
    ///        - Gets its own broker handle using `agent.broker()`.
    ///        - Asynchronously broadcasts a `Pong` message.
    /// 4. Subscribe agents:
    ///    - `Counter` subscribes to `Pong`.
    ///    - `Comedian` subscribes to `Ping`.
    /// 5. Start both agents.
    /// 6. Broadcast an initial `Ping` message using the main broker handle.
    /// 7. Shut down the runtime.
    ///
    /// **Verification:**
    /// - Initial `Ping` broadcast is received by `Comedian`.
    /// - `Comedian`'s `Ping` handler broadcasts `Pong`.
    /// - `Pong` broadcast is received by `Counter` (count=1).
    /// - `Counter`'s `after_stop` handler asserts `count == 1`.
    initialize_tracing();
    let mut runtime: AgentRuntime = ActonApp::launch();
    let broker_handle = runtime.broker();

    // --- Comedian Agent ---
    let mut comedian_agent_builder = runtime.new_agent::<Comedian>().await;

    // --- Counter Agent ---
    let counter_config = AgentConfig::new(
        Ern::with_root("counter").unwrap(),
        None,
        Some(broker_handle.clone()),
    )?;
    let mut counter_agent_builder = runtime.new_agent_with_config::<Counter>(counter_config).await;

    // Configure Counter handler.
    counter_agent_builder.act_on::<Pong>(|agent, _envelope| {
        info!("Also SUCCESS! PONG!");
        agent.model.count += 1;
        AgentReply::immediate()
    }).after_stop(|agent| {
        assert_eq!(agent.model.count, 1, "count should be 1");
        AgentReply::immediate()
    });

    // Configure Comedian handler to broadcast from within.
    comedian_agent_builder
        .act_on::<Ping>(|agent, _envelope| {
            // Get the broker handle associated with this agent.
            let agent_broker_handle = agent.broker().clone();
            // Return an async block to perform the broadcast.
            Box::pin(async move {
                // Broadcast Pong using the handle obtained within the agent context.
                agent_broker_handle.broadcast(Pong).await;
            })
        });

    // Subscribe agents.
    counter_agent_builder.handle().subscribe::<Pong>().await; // Counter listens for Pong
    comedian_agent_builder.handle().subscribe::<Ping>().await; // Comedian listens for Ping

    // Start agents.
    let _comedian_handle = comedian_agent_builder.start().await;
    let _counter_handle = counter_agent_builder.start().await;

    // Initiate the sequence by broadcasting Ping using the main runtime broker handle.
    // This will trigger the Comedian's Ping handler, which will then broadcast Pong.
    broker_handle.broadcast(Ping).await;

    // Shut down the runtime.
    runtime.shutdown_all().await?;

    Ok(())
}
