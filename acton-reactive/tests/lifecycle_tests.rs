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
use acton_reactive::prelude::*;
use acton_test::prelude::*;

use crate::setup::*;

mod setup;

/// Tests the basic agent lifecycle event handlers: `after_start` and `after_stop`.
///
/// **Scenario:**
/// 1. Launch the runtime.
/// 2. Create a `PoolItem` agent builder.
/// 3. Register an `after_start` handler that logs a message with the agent's ID.
/// 4. Register an `after_stop` handler that logs a message with the agent's ID.
/// 5. Start the agent.
/// 6. Immediately stop the agent.
///
/// **Verification:**
/// - Relies on observing the tracing output to confirm that the log messages from
///   both `after_start` and `after_stop` handlers are printed, indicating they were executed.
#[acton_test]
async fn test_actor_lifecycle_events() -> anyhow::Result<()> {
    initialize_tracing();
    // Launch the runtime environment.
    let mut runtime: AgentRuntime = ActonApp::launch();
    // Create an agent builder for the PoolItem state.
    let mut pool_item_agent_builder = runtime.new_agent::<PoolItem>().await;

    // Configure the lifecycle handlers.
    pool_item_agent_builder
        // Register a function to run immediately after the agent's task starts.
        .after_start(|agent| {
            tracing::info!("Agent started with ID: {}", agent.id());
            AgentReply::immediate()
        })
        // Register a function to run after the agent has processed all messages
        // following a stop signal and its task is about to terminate.
        .after_stop(|agent| {
            tracing::info!("Agent stopping with ID: {}", agent.id());
            AgentReply::immediate()
        });

    // Start the agent, spawning its task and returning a handle.
    let agent_handle = pool_item_agent_builder.start().await;
    // Immediately send a stop signal to the agent and wait for it to shut down.
    // This will trigger the `after_stop` handler.
    agent_handle.stop().await?;
    Ok(())
}
