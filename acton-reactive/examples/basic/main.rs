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
// Import the macro for agent state structs
use acton_macro::acton_actor;

/// Defines the internal state (the "model") for our basic agent.
// The `#[acton_actor]` macro derives `Default`, `Clone`, and implements `Debug`.
#[acton_actor]
struct BasicAgentState {
    /// Simple counter state modified by message handlers.
    some_state: usize,
}

/// A message used to initiate interaction with the agent.
// Messages need to be Clone and Debug to satisfy the ActonMessage trait bounds.
#[derive(Debug, Clone)]
struct PingMsg;

/// A reply message sent back by the agent after receiving PingMsg.
// Note: We rely on the blanket `impl<T> ActonMessage for T` and derive traits manually.
#[derive(Debug, Clone)]
struct PongMsg;

/// A final message sent by the agent before stopping.
// Note: We rely on the blanket `impl<T> ActonMessage for T` and derive traits manually.
#[derive(Debug, Clone)]
struct BuhByeMsg;

#[tokio::main]
async fn main() {
    // 1. Launch the Acton runtime environment.
    let mut runtime = ActonApp::launch();

    // 2. Create an agent builder.
    //    `new_agent` takes the *state type* (model) as a generic parameter.
    //    It returns a builder (`ManagedAgent` in the `Idle` state) which we use to configure the agent.
    let mut agent_builder = runtime.new_agent::<BasicAgentState>().await;

    // 3. Configure the agent's behavior by defining message handlers using `act_on`.
    agent_builder
        // Define a handler for `PingMsg`.
        // The closure receives the `ManagedAgent` (giving access to `model` and `handle`)
        // and the incoming message `envelope`.
        .act_on::<PingMsg>(|agent, envelope| {
            println!("Pinged. You can mutate me!");
            // Access and modify the agent's internal state (`model`).
            agent.model.some_state += 1;

            // Get an envelope pre-addressed to reply to the sender of the incoming `PingMsg`.
            let reply_envelope = envelope.reply_envelope();

            // Handlers must return a future (specifically `AgentReply`, which wraps a `Pin<Box<dyn Future>>`).
            // This allows handlers to perform asynchronous operations.
            Box::pin(async move {
                // Send the `PongMsg` back to the original sender.
                reply_envelope.send(PongMsg).await;
            })
        })
        // Define a handler for `PongMsg`.
        .act_on::<PongMsg>(|agent, _envelope| {
            println!("I got ponged!");
            agent.model.some_state += 1;

            // Get a clone of the agent's own handle to send a message to itself.
            // Cloning is necessary to move the handle into the async block.
            let self_handle = agent.handle().clone();

            // `AgentReply::from_async` is a helper to wrap a future in the required type.
            AgentReply::from_async(async move {
                // Send `BuhByeMsg` to self.
                self_handle.send(BuhByeMsg).await;
            })
        })
        // Define a handler for `BuhByeMsg`.
        .act_on::<BuhByeMsg>(|_agent, _envelope| {
            println!("Thanks for all the fish! Buh Bye!");
            // If a handler doesn't need to perform async work or reply,
            // `AgentReply::immediate()` signifies immediate completion.
            AgentReply::immediate()
        })
        // Define a callback that runs after the agent stops.
        .after_stop(|agent| {
            println!("Agent stopped with state value: {}", agent.model.some_state);
            // Assert the final state after all messages are processed.
            debug_assert_eq!(agent.model.some_state, 2);
            AgentReply::immediate()
        });

    // 4. Start the agent.
    //    This transitions the agent to the `Started` state, spawns its task,
    //    and returns an `AgentHandle` for interaction.
    let agent_handle = agent_builder.start().await;

    // 5. Send the initial `PingMsg` to the running agent using its handle.
    agent_handle.send(PingMsg).await;

    // 6. Shut down the runtime.
    //    This gracefully stops all agents managed by the runtime, allowing them
    //    to finish processing messages and run their `after_stop` handlers.
    runtime.shutdown_all().await.expect("Failed to shut down system");
}
