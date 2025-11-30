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

use tracing::{info, instrument, trace};

use acton_reactive::prelude::*;
use acton_test::prelude::*;

// Use direct paths as re-exports seem problematic in test context
use crate::setup::{
    initialize_tracing,
    messages::{Ping, Pong},
};

mod setup;

// Represents the internal state (model) for the Shopping Cart actor.
#[derive(Default, Debug, Clone)]
pub(crate) struct ShoppingCart {
    // Note: Storing the actor's own handle in its state is generally discouraged.
    // It's usually accessed via `actor.handle()` within handlers.
    // This might be leftover from an older pattern or for a specific reason not obvious here.
    // We keep it for now as refactoring it out is beyond the scope of renaming/commenting.
    actor_handle: ActorHandle,
    // Handle to the PriceService actor needed for communication.
    pub(crate) price_service_handle: ActorHandle,
}

/// Tests direct message passing and reply mechanism between two actors.
///
/// **Scenario:**
/// 1. A `PriceService` actor is created and started. It handles `Pong` messages by replying with `PongResponse(100)`.
/// 2. A `ShoppingCart` actor is created and started. It holds a handle to the `PriceService`.
/// 3. The `ShoppingCart` actor handles `Ping` messages by sending a `Pong` message directly to the `PriceService`.
/// 4. The `ShoppingCart` actor handles `PongResponse` messages (replies from `PriceService`) by asserting the received price is 100.
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

    // Create and start the PriceService actor. The `new` function returns a context struct holding the handle.
    let price_service_context = PriceService::new(&mut runtime).await;
    // Clone the handle for the ShoppingCart.
    let price_service_handle = price_service_context.actor_handle.clone();
    // Create and start the ShoppingCart actor, providing the PriceService handle.
    let shopping_cart_context = ShoppingCart::new(price_service_handle, &mut runtime).await;

    // Trigger the interaction multiple times. Each trigger sends Ping -> Pong -> PongResponse.
    shopping_cart_context.trigger().await;
    shopping_cart_context.trigger().await;
    shopping_cart_context.trigger().await;

    // Shut down the system and all actors
    runtime
        .shutdown_all()
        .await
        .expect("Failed to shut down system");
    Ok(())
}

/// Response message sent by `PriceService` back to the requester.
#[derive(Default, Debug, Clone)]
pub struct PongResponse(i8);

/// Message used internally by the test's `trigger` method to initiate the sequence.
#[derive(Default, Debug, Clone)]
pub struct Trigger;

// Represents the internal state (model) for the Price Service actor.
#[derive(Default, Debug, Clone)]
pub(crate) struct PriceService {
    // See note in ShoppingCart struct about storing own handle.
    pub(crate) actor_handle: ActorHandle,
}

impl PriceService {
    /// Creates, configures, and starts a new `PriceService` actor.
    /// Returns a struct containing the handle to the started actor.
    pub(crate) async fn new(runtime: &mut ActorRuntime) -> Self {
        let config = ActorConfig::new(Ern::with_root("price_service").unwrap(), None, None)
            .expect("Failed to create actor config");
        // Create the actor builder.
        let mut price_service_builder = runtime.new_actor_with_config::<Self>(config);
        // Configure the actor's behavior.
        price_service_builder
            // Handler for `Pong` messages (sent by ShoppingCart).
            .mutate_on::<Pong>(|_actor, envelope| {
                trace!("Received Pong");
                // Get an envelope pre-addressed to reply to the sender of the incoming `Pong` message.
                let reply_envelope = envelope.reply_envelope();
                // Perform the reply asynchronously.
                ActorReply::from_async(async move {
                    trace!("Sending PriceResponse");
                    // Send the PongResponse back to the original sender (ShoppingCart).
                    reply_envelope.send(PongResponse(100)).await;
                })
            });

        // Start the actor and get its handle.
        let actor_handle = price_service_builder.start().await;
        // Return the context struct containing the handle.
        Self { actor_handle }
    }
}

impl ShoppingCart {
    /// Creates, configures, and starts a new `ShoppingCart` actor.
    /// Returns a struct containing the handle to the started actor and the `PriceService` handle.
    pub(crate) async fn new(price_service_handle: ActorHandle, runtime: &mut ActorRuntime) -> Self {
        let config = ActorConfig::new(Ern::with_root("shopping_cart").unwrap(), None, None)
            .expect("Failed to create actor config");
        // Create the actor builder.
        let mut shopping_cart_builder = runtime.new_actor_with_config::<Self>(config);
        // Configure actor behavior
        shopping_cart_builder
            // Handler for `Ping` messages (sent by the `trigger` method).
            .mutate_on::<Ping>(|actor, envelope| {
                // Get a reference to the PriceService handle stored in the actor's state.
                let price_service_handle_ref = &actor.model.price_service_handle;
                // Create a new envelope specifically addressed to the PriceService actor.
                // `reply_address()` provides the necessary `MessageAddress`.
                let request_envelope =
                    envelope.new_envelope(&price_service_handle_ref.reply_address());
                let target_name = price_service_handle_ref.name();
                // Send the `Pong` message asynchronously.
                ActorReply::from_async(async move {
                    trace!("Sending Pong to price_service id: {:?}", target_name);
                    // Send the Pong message directly to the PriceService.
                    request_envelope.send(Pong).await;
                })
            })
            // Handler for `PongResponse` messages (replies from PriceService).
            .mutate_on::<PongResponse>(|_actor, envelope| {
                // Extract the price from the message content.
                let price = envelope.message().0;
                // Assert that the received price is correct.
                assert_eq!(price, 100);
                info!("fin. price: {}", price);
                ActorReply::immediate()
            });

        // Set the PriceService handle in the actor's state before starting.
        shopping_cart_builder.model.price_service_handle = price_service_handle.clone();

        // Start the actor and get its handle.
        let shopping_cart_handle = shopping_cart_builder.start().await;

        // Return the context struct containing the handles.
        Self {
            actor_handle: shopping_cart_handle, // Store own handle (see note above)
            price_service_handle,
        }
    }

    /// Sends a `Ping` message to this `ShoppingCart` actor itself to initiate the
    /// price request sequence.
    #[instrument(skip(self), level = "debug")]
    pub(crate) async fn trigger(&self) {
        // Note: No need for &mut self because internal state mutation happens
        // via message handlers, not direct struct modification here.
        trace!(
            "actor_handle id is {}",
            self.actor_handle.id().root.to_string()
        );
        trace!(
            "and pricing_service id is {}",
            self.price_service_handle.id().root.to_string()
        );
        // Send Ping to self to kick off the process defined in the Ping handler.
        self.actor_handle.send(Ping).await;
    }
}
