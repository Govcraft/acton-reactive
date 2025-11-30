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

use rand::Rng;
use tracing::{instrument, trace};

use acton_reactive::prelude::*;

// Import message types used by this actor.
use crate::PriceResponse;
use crate::{CartItem, ItemScanned};

// Constants for configuration and simulation.
const PRICE_SERVICE_ROOT: &str = "price_service"; // Base name for the actor's ERN.
const MOCK_DELAY_MS: u64 = 100; // Simulated delay for price lookup.
const PRICE_MIN: i32 = 100; // Minimum random price in cents.
const PRICE_MAX: i32 = 250; // Maximum random price in cents.

/// Represents the state (model) for the `PriceService` actor.
/// This actor is responsible for looking up (or simulating) the price of items.
// The `#[acton_actor]` macro derives `Default`, `Clone`, and implements `Debug`.
#[acton_actor]
pub struct PriceService;

impl PriceService {
    /// Creates, configures, and starts a new `PriceService` actor.
    /// Returns a handle to the started actor.
    #[instrument(skip(runtime))] // Instrument for tracing, skip the runtime param.
    pub(crate) async fn create(runtime: &mut ActorRuntime) -> anyhow::Result<ActorHandle> {
        // Configure the actor's identity (ERN).
        let config = ActorConfig::new(Ern::with_root(PRICE_SERVICE_ROOT).unwrap(), None, None)?;
        // Create the actor builder using the runtime and configuration.
        let mut price_service_builder = runtime.new_actor_with_config::<Self>(config);

        // Configure the actor's message handler for `ItemScanned` messages.
        price_service_builder.mutate_on::<ItemScanned>(|actor, envelope| {
            // Clone the item from the incoming message envelope.
            let item = envelope.message().0.clone();
            // Clone the actor's state (PriceService is a unit struct, but this pattern is common).
            // Cloning the model allows moving it into the async block if needed, though not strictly necessary here.
            let model = actor.model.clone();
            // Get a handle to the message broker for broadcasting the response.
            let broker_handle = actor.broker().clone();

            // Use ActorReply::from_async to handle the asynchronous price lookup and broadcast.
            ActorReply::from_async(async move {
                let mut item = item;
                // Simulate getting the price (includes an artificial delay).
                // Calls the `get_price` method on the cloned model state.
                item.set_cost(model.get_price(item.clone()).await);
                // Create the response message containing the updated item (now with cost).
                let response_message = PriceResponse { item };
                // Broadcast the response message via the broker. Any actor subscribed
                // to `PriceResponse` will receive it (e.g., the Register actor).
                broker_handle.broadcast(response_message).await;
            })
        });

        // Start the actor and return its handle.
        Ok(price_service_builder.start().await)
    }

    /// Simulates looking up the price for a given `CartItem`.
    /// Includes an artificial delay to mimic real-world latency.
    async fn get_price(&self, item: CartItem) -> i32 {
        trace!("Getting price for {}", item.name());
        // Simulate network/database latency.
        tokio::time::sleep(Duration::from_millis(MOCK_DELAY_MS)).await;
        // Generate a random price within the defined range (in cents).
        rand::rng().random_range(PRICE_MIN..=PRICE_MAX)
    }
}
