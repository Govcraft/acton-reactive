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
    messages::Ping,
    initialize_tracing,
};

mod setup;

/// Tests basic message sending and handling for a `PoolItem` agent.
///
/// **Scenario:**
/// 1. Launch runtime.
/// 2. Create a `PoolItem` agent builder.
/// 3. Configure a handler for `Ping` messages that increments the agent's `receive_count`.
/// 4. Configure an `after_stop` handler to log the final count.
/// 5. Start the agent.
/// 6. Send one `Ping` message to the agent.
/// 7. Stop the agent.
///
/// **Verification:**
/// - Tracing logs show the "Received in sync handler" message.
/// - Tracing logs from `after_stop` show "Processed 1 Pings".
#[acton_test]
async fn test_messaging_behavior() -> anyhow::Result<()> {
    initialize_tracing();
    // Launch the runtime environment.
    let mut runtime: AgentRuntime = ActonApp::launch();
    // Create an agent builder for PoolItem state.
    let mut pool_item_agent_builder = runtime.new_agent::<PoolItem>().await;

    // Configure the agent's behavior.
    pool_item_agent_builder
        // Handler for `Ping` messages.
        .mutate_on::<Ping>(|agent, _envelope| {
            let type_name = std::any::type_name::<Ping>();
            info!(type_name = type_name, "Received in sync handler");
            // Mutate the agent's internal state.
            agent.model.receive_count += 1;
            AgentReply::immediate()
        })
        // Handler executed after the agent stops.
        .after_stop(|agent| {
            assert_eq!(agent.model.receive_count, 1, "expected one Ping");
            info!("Processed {} Pings", agent.model.receive_count);
            // Implicitly verifies count is 1 based on the log message.
            AgentReply::immediate()
        });

    // Start the agent and get its handle.
    let agent_handle = pool_item_agent_builder.start().await;
    // Send a message to the running agent.
    agent_handle.send(Ping).await;
    // Stop the agent.
    agent_handle.stop().await?;
    Ok(())
}

/// Tests basic message sending and handling for a `Messenger` agent (no internal state).
///
/// **Scenario:**
/// 1. Launch runtime.
/// 2. Create a `Messenger` agent builder.
/// 3. Configure a handler for `Ping` messages that logs receipt.
/// 4. Configure a simple `after_stop` handler.
/// 5. Start the agent.
/// 6. Send one `Ping` message.
/// 7. Stop the agent.
///
/// **Verification:**
/// - Tracing logs show the "Received in Messenger handler" message.
#[acton_test]
async fn test_basic_messenger() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: AgentRuntime = ActonApp::launch();
    // Create an agent builder for Messenger state (which is likely empty or minimal).
    let mut messenger_agent_builder = runtime.new_agent::<Messenger>().await;

    // Configure the agent's behavior.
    messenger_agent_builder
        // Handler for `Ping` messages.
        .mutate_on::<Ping>(|_agent, _envelope| {
            let type_name = std::any::type_name::<Ping>();
            info!(type_name = type_name, "Received in Messenger handler");
            AgentReply::immediate()
        })
        // Handler executed after the agent stops.
        .after_stop(|_agent| {
            info!("Stopping");
            AgentReply::immediate()
        });

    // Start the agent.
    let agent_handle = messenger_agent_builder.start().await;
    // Send a message.
    agent_handle.send(Ping).await;
    // Stop the agent.
    agent_handle.stop().await?;
    Ok(())
}

/// Tests basic message handling, seemingly intended to test async handlers but uses `AgentReply::immediate`.
/// Functionally similar to `test_messaging_behavior`.
///
/// **Scenario:**
/// 1. Launch runtime.
/// 2. Create a `PoolItem` agent builder.
/// 3. Configure a handler for `Ping` messages that increments `receive_count`.
/// 4. Configure an `after_stop` handler to log the final count.
/// 5. Start the agent.
/// 6. Send one `Ping` message.
/// 7. Stop the agent.
///
/// **Verification:**
/// - Tracing logs show the "Received in async handler" message.
/// - Tracing logs from `after_stop` show "Processed 1 Pings".
#[acton_test]
async fn test_async_messaging_behavior() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: AgentRuntime = ActonApp::launch();
    let mut pool_item_agent_builder = runtime.new_agent::<PoolItem>().await;

    pool_item_agent_builder
        // Handler for `Ping` messages. Although the test name suggests async,
        // this handler currently returns `AgentReply::immediate()`.
        .mutate_on::<Ping>(|agent, _envelope| {
            let type_name = std::any::type_name::<Ping>();
            info!(type_name = type_name, "Received in async handler");
            agent.model.receive_count += 1;
            AgentReply::immediate()
        })
        .after_stop(|agent| {
            assert_eq!(agent.model.receive_count, 1, "expected one Ping");
            info!("Processed {} Pings", agent.model.receive_count);
            // Implicitly verifies count is 1.
            AgentReply::immediate()
        });

    // Start the agent.
    let agent_handle = pool_item_agent_builder.start().await;
    // Send a message.
    agent_handle.send(Ping).await;
    // Stop the agent.
    agent_handle.stop().await?;
    Ok(())
}
