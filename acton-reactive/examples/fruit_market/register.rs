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

use rand::prelude::*;

use tracing::trace;

use acton_reactive::prelude::*;

// Import necessary components from other modules within the example.
use crate::{price_service::PriceService, CartItem, ItemScanned};
use crate::printer::{Printer, ToggleHelp};

// --- Constants ---
// List of possible grocery items for random selection.
const GROCERY_ITEMS: &[&str] = &[
    "Apple", "Banana", "Cantaloupe", "Orange", "Grapes", "Mango",
    "Pineapple", "Strawberry", "Milk", "Bread",
];
// Range for random quantity generation.
const QUANTITY_MIN: i32 = 1;
const QUANTITY_MAX: i32 = 6;
// Error message for item selection failure.
const ITEM_SELECTION_ERROR: &str = "Failed to select an item";

/// Represents the main coordinator for the fruit market transaction.
/// Holds handles to the necessary service agents (Printer, PriceService).
/// This struct itself is not an agent state but orchestrates interactions.
#[derive(Clone)]
pub struct Register {
    /// Handle to the PriceService agent.
    pub(crate) price_service: AgentHandle,
    /// Handle to the Printer agent.
    pub(crate) printer: AgentHandle,
}

impl Register {
    /// Creates a new transaction context by initializing and starting
    /// the required Printer and PriceService agents.
    /// Returns a `Register` instance holding handles to these agents.
    pub async fn new_transaction(runtime: &mut AgentRuntime) -> anyhow::Result<Self> {
        Ok(Register {
            // Start the Printer agent.
            printer: Printer::power_on(runtime).await?,
            // Start the PriceService agent.
            price_service: PriceService::new(runtime).await?,
        })
    }

    /// Sends a message to the Printer agent to toggle the help display.
    pub async fn toggle_help(&self) -> anyhow::Result<()> {
        trace!("Register::toggle_help");
        // Use the stored printer handle to send the ToggleHelp message.
        self.printer.send(ToggleHelp).await;
        Ok(())
    }

    /// Simulates scanning a random item with a random quantity.
    /// Sends an `ItemScanned` message to the PriceService agent.
    pub async fn scan(&self) -> anyhow::Result<()> {
        // Use a seeded RNG for potentially reproducible results if needed, otherwise `from_entropy` is fine.
        let mut rng = StdRng::from_os_rng();

        // Choose a random item name from the list.
        let item_name = GROCERY_ITEMS
            .choose(&mut rng)
            .expect(ITEM_SELECTION_ERROR)
            .to_string();

        // Generate a random quantity.
        let quantity = rng.random_range(QUANTITY_MIN..=QUANTITY_MAX);

        // Create a new CartItem and wrap it in an ItemScanned message.
        // Send the message to the PriceService agent using its stored handle.
        self.price_service
            .send(ItemScanned(CartItem::new(item_name, quantity)))
            .await;
        Ok(())
    }
}
