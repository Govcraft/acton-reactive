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

use tracing::{debug, info, instrument, trace};

use acton_reactive::prelude::*;
use acton_test::prelude::*;

use crate::setup::*;

mod setup;

// Represents the internal state (model) for the Shopping Cart agent.
#[derive(Default, Debug, Clone)]
pub(crate) struct ShoppingCart {
    // Note: Storing the agent's own handle in its state is generally discouraged.
    // It's usually accessed via `agent.handle()` within handlers.
    // This might be leftover from an older pattern or for a specific reason not obvious here.
    // We keep it for now as refactoring it out is beyond the scope of renaming/commenting.
    agent_handle: AgentHandle,
    // Handle to the PriceService agent needed for communication.
    pub(crate) price_service_handle: AgentHandle,
}


/// Tests direct message passing and reply mechanism between two agents.
///
/// **Scenario:**
/// 1. A `PriceService` agent is created and started. It handles `Pong` messages by replying with `PongResponse(100)`.
/// 2. A `ShoppingCart` agent is created and started. It holds a handle to the `PriceService`.
/// 3. The `ShoppingCart` agent handles `Ping` messages by sending a `Pong` message directly to the `PriceService`.
/// 4. The `ShoppingCart` agent handles `PongResponse` messages (replies from `PriceService`) by asserting the received price is 100.
/// 5. The test triggers the interaction by calling `shopping_cart_context.trigger()` three times, which sends `Ping` messages from the `ShoppingCart` to itself.
/// 6. The runtime is shut down.
///
/// **Verification:**
/// - `ShoppingCart` sends `Pong` to `PriceService`.
/// - `PriceService` receives `Pong` and replies with `PongResponse(100)`.
/// - `ShoppingCart` receives `PongResponse` and the assertion `assert_eq!(price, 100)` passes.
#[acton_test]
async fn test_reply() -> anyhow::Result<()> {
    initialize_tracing();
    // Launch the runtime environment.
    let mut runtime = ActonApp::launch();

    // Create and start the PriceService agent. The `new` function returns a context struct holding the handle.
    let price_service_context = PriceService::new(&mut runtime).await;
    // Clone the handle for the ShoppingCart.
    let price_service_handle = price_service_context.agent_handle.clone();
    // Create and start the ShoppingCart agent, providing the PriceService handle.
    let shopping_cart_context = ShoppingCart::new(price_service_handle, &mut runtime).await;

    // Trigger the interaction multiple times. Each trigger sends Ping -> Pong -> PongResponse.
    shopping_cart_context.trigger().await;
    shopping_cart_context.trigger().await;
    shopping_cart_context.trigger().await;

    // Shut down the system and all agents
    runtime.shutdown_all().await.expect("Failed to shut down system");
    Ok(())
}

/// Response message sent by PriceService back to the requester.
#[derive(Default, Debug, Clone)]
pub struct PongResponse(i8);

/// Message used internally by the test's `trigger` method to initiate the sequence.
#[derive(Default, Debug, Clone)]
pub struct Trigger;

// Represents the internal state (model) for the Price Service agent.
#[derive(Default, Debug, Clone)]
pub(crate) struct PriceService {
    // See note in ShoppingCart struct about storing own handle.
    pub(crate) agent_handle: AgentHandle,
}

impl PriceService {
    /// Creates, configures, and starts a new PriceService agent.
    /// Returns a struct containing the handle to the started agent.
    pub(crate) async fn new(runtime: &mut AgentRuntime) -> Self {
        let config = AgentConfig::new(Ern::with_root("price_service").unwrap(), None, None).expect("Failed to create actor config");
        // Create the agent builder.
        let mut price_service_builder = runtime.new_agent_with_config::<PriceService>(config).await;
        // Configure the agent's behavior.
        price_service_builder
            // Handler for `Pong` messages (sent by ShoppingCart).
            .act_on::<Pong>(|_agent, envelope| {
                trace!("Received Pong");
                // Get an envelope pre-addressed to reply to the sender of the incoming `Pong` message.
                let reply_envelope = envelope.reply_envelope();
                // Perform the reply asynchronously.
                AgentReply::from_async(
                    async move {
                        trace!("Sending PriceResponse");
                        // Send the PongResponse back to the original sender (ShoppingCart).
                        reply_envelope.send(PongResponse(100)).await;
                    }
                )
            });

        // Start the agent and get its handle.
        let agent_handle = price_service_builder.start().await;
        // Return the context struct containing the handle.
        PriceService {
            agent_handle,
        }
    }
}

impl ShoppingCart {
    /// Creates, configures, and starts a new ShoppingCart agent.
    /// Returns a struct containing the handle to the started agent and the PriceService handle.
    pub(crate) async fn new(price_service_handle: AgentHandle, runtime: &mut AgentRuntime) -> Self {
        let config = AgentConfig::new(Ern::with_root("shopping_cart").unwrap(), None, None).expect("Failed to create actor config");
        // Create the agent builder.
        let mut shopping_cart_builder = runtime.new_agent_with_config::<ShoppingCart>(config).await;
        // Configure agent behavior
        shopping_cart_builder
            // Handler for `Ping` messages (sent by the `trigger` method).
            .act_on::<Ping>(|agent, envelope| {
                // Get a reference to the PriceService handle stored in the agent's state.
                let price_service_handle_ref = &agent.model.price_service_handle;
                // Create a new envelope specifically addressed to the PriceService agent.
                // `reply_address()` provides the necessary `MessageAddress`.
                let request_envelope = envelope.new_envelope(&price_service_handle_ref.reply_address());
                let target_name = price_service_handle_ref.name();
                // Send the `Pong` message asynchronously.
                AgentReply::from_async(async move {
                    trace!( "Sending Pong to price_service id: {:?}", target_name);
                    // Send the Pong message directly to the PriceService.
                    request_envelope.send(Pong).await;
                })
            })
            // Handler for `PongResponse` messages (replies from PriceService).
            .act_on::<PongResponse>(|_agent, envelope| {
                // Extract the price from the message content.
                let price = envelope.message().0;
                // Assert that the received price is correct.
                assert_eq!(price, 100);
                info!("fin. price: {}", price);
                AgentReply::immediate()
            });

        // Set the PriceService handle in the agent's state before starting.
        shopping_cart_builder.model.price_service_handle = price_service_handle.clone();

        // Start the agent and get its handle.
        let shopping_cart_handle = shopping_cart_builder.start().await;

        // Return the context struct containing the handles.
        ShoppingCart {
            agent_handle: shopping_cart_handle, // Store own handle (see note above)
            price_service_handle,
        }
    }

    /// Sends a `Ping` message to this ShoppingCart agent itself to initiate the
    /// price request sequence.
    #[instrument(skip(self), level = "debug")]
    pub(crate) async fn trigger(&self) {
        // Note: No need for &mut self because internal state mutation happens
        // via message handlers, not direct struct modification here.
        trace!("agent_handle id is {}", self.agent_handle.id().root.to_string());
        trace!("and pricing_service id is {}", self.price_service_handle.id().root.to_string());
        // Send Ping to self to kick off the process defined in the Ping handler.
        self.agent_handle.send(Ping).await;
    }
}
