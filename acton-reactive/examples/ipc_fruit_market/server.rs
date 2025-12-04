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

//! IPC Fruit Market Example - Server
//!
//! Central server that coordinates the fruit market point-of-sale system.
//! Each actor runs in this process while clients connect via IPC.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                        SERVER PROCESS                          │
//! │                                                                 │
//! │  ┌─────────────────┐         ┌─────────────────────────────┐   │
//! │  │  PriceService   │         │       Message Broker        │   │
//! │  │    Actor        │────────>│  (broadcasts to clients)    │   │
//! │  └─────────────────┘         └─────────────────────────────┘   │
//! │          ▲                              │                      │
//! │          │                              ▼                      │
//! │  ┌───────┴───────┐           ┌─────────────────────┐           │
//! │  │ IPC Listener  │◄─────────►│ Subscription Mgr    │           │
//! │  └───────────────┘           └─────────────────────┘           │
//! └─────────┬───────────────────────────────┬───────────────────────┘
//!           │                               │
//!     ┌─────┴─────┐                   ┌─────┴─────┐
//!     │ Keyboard  │                   │ Display   │
//!     │ Client    │                   │ Client    │
//!     └───────────┘                   └───────────┘
//! ```
//!
//! # Running This Example
//!
//! ```bash
//! cargo run --example ipc_fruit_market_server --features ipc
//! ```

use std::time::Duration;

use acton_reactive::ipc::{socket_exists, IpcConfig};
use acton_reactive::prelude::*;
use rand::Rng;
use tracing_subscriber::EnvFilter;

// ============================================================================
// Message Definitions
// ============================================================================

/// Command from keyboard client to scan an item.
#[acton_message(ipc)]
struct ScanItem {
    /// The name of the fruit to scan.
    fruit_name: String,
    /// Quantity being purchased.
    quantity: i32,
}

/// Command from keyboard client to toggle help display.
#[acton_message(ipc)]
#[derive(Default)]
struct ToggleHelp;

/// Notification that an item has been scanned (price pending).
/// Broadcast to display clients.
#[acton_message(ipc)]
struct ItemScanned {
    /// Unique ID for this item instance.
    item_id: String,
    /// Name of the fruit.
    name: String,
    /// Quantity purchased.
    quantity: i32,
}

/// Notification that a price has been retrieved.
/// Broadcast to display clients.
#[acton_message(ipc)]
struct PriceUpdate {
    /// ID of the item being priced.
    item_id: String,
    /// Price per unit in cents.
    unit_price: i32,
    /// Total price (`unit_price` * quantity) in cents.
    total_price: i32,
}

/// Notification that help display should be toggled.
/// Broadcast to display clients.
#[acton_message(ipc)]
#[derive(Default)]
struct HelpToggled;

// ============================================================================
// Actor State
// ============================================================================

/// Price service actor state.
#[derive(Debug, Clone, Default)]
struct PriceServiceState {
    /// Count of items processed.
    items_processed: u64,
}

// ============================================================================
// Constants
// ============================================================================

/// Simulated price lookup delay in milliseconds.
const PRICE_LOOKUP_DELAY_MS: u64 = 100;

/// Minimum random price in cents.
const PRICE_MIN: i32 = 100;

/// Maximum random price in cents.
const PRICE_MAX: i32 = 350;

// ============================================================================
// Actor Creation
// ============================================================================

/// Creates the price service actor.
async fn create_price_service(runtime: &mut ActorRuntime) -> ActorHandle {
    let mut price_service =
        runtime.new_actor_with_name::<PriceServiceState>("price_service".to_string());

    // Handle scan item commands
    price_service.mutate_on::<ScanItem>(|actor, envelope| {
        let msg = envelope.message();
        let fruit_name = msg.fruit_name.clone();
        let quantity = msg.quantity;

        actor.model.items_processed += 1;
        let count = actor.model.items_processed;

        // Generate unique item ID
        let item_id = format!("item_{count}_{}", fruit_name.to_lowercase().replace(' ', "_"));

        println!("  [PriceService] Scanning: {fruit_name} x{quantity} ({item_id})");

        // First broadcast that item was scanned (price pending)
        let broker = actor.broker().clone();

        Reply::pending(async move {
            // Broadcast scan notification immediately
            broker
                .broadcast(ItemScanned {
                    item_id: item_id.clone(),
                    name: fruit_name,
                    quantity,
                })
                .await;

            // Simulate price lookup delay
            tokio::time::sleep(Duration::from_millis(PRICE_LOOKUP_DELAY_MS)).await;

            // Generate random price
            let unit_price = rand::rng().random_range(PRICE_MIN..=PRICE_MAX);
            let total_price = unit_price * quantity;

            println!(
                "  [PriceService] Price found: ${:.2}/unit, ${:.2} total",
                f64::from(unit_price) / 100.0,
                f64::from(total_price) / 100.0
            );

            // Broadcast price update
            broker
                .broadcast(PriceUpdate {
                    item_id,
                    unit_price,
                    total_price,
                })
                .await;
        })
    });

    // Handle toggle help commands
    price_service.act_on::<ToggleHelp>(|actor, _envelope| {
        println!("  [PriceService] Help toggled");
        let broker = actor.broker().clone();
        Reply::pending(async move {
            broker.broadcast(HelpToggled).await;
        })
    });

    price_service.start().await
}

// ============================================================================
// Main
// ============================================================================

#[acton_main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("acton=info".parse()?))
        .init();

    println!("====================================================================");
    println!("       IPC Fruit Market Example (Server)");
    println!("====================================================================");
    println!();

    let mut runtime = ActonApp::launch_async().await;

    // Register IPC message types
    let registry = runtime.ipc_registry();

    // Command messages (from keyboard client)
    registry.register::<ScanItem>("ScanItem");
    registry.register::<ToggleHelp>("ToggleHelp");

    // Broadcast messages (to display client)
    registry.register::<ItemScanned>("ItemScanned");
    registry.register::<PriceUpdate>("PriceUpdate");
    registry.register::<HelpToggled>("HelpToggled");

    println!("Registered {} IPC message types:", registry.len());
    println!("  Commands:");
    println!("    - ScanItem: Scan a fruit item");
    println!("    - ToggleHelp: Toggle help display");
    println!("  Broadcasts:");
    println!("    - ItemScanned: Item scanned (price pending)");
    println!("    - PriceUpdate: Price retrieved for item");
    println!("    - HelpToggled: Help display toggled");

    // Create the price service actor
    let price_service = create_price_service(&mut runtime).await;
    println!("\nPriceService actor started");

    // Expose actor for IPC access
    runtime.ipc_expose("price_service", price_service.clone());
    println!("Exposed actors: price_service");

    // Start the IPC listener
    let ipc_config = IpcConfig::load();
    let socket_path = ipc_config.socket_path();

    let listener_handle = runtime.start_ipc_listener().await?;
    println!("IPC listener started");

    // Verify socket is ready
    tokio::time::sleep(Duration::from_millis(50)).await;
    if socket_exists(&socket_path) {
        println!("Socket ready: {}", socket_path.display());
    }

    println!();
    println!("====================================================================");
    println!("  Fruit Market Server is ready!");
    println!();
    println!("  Start the clients in separate terminals:");
    println!("    1. cargo run --example ipc_fruit_market_display --features ipc");
    println!("    2. cargo run --example ipc_fruit_market_keyboard --features ipc");
    println!();
    println!("  Or use the launcher script:");
    println!("    ./examples/ipc_fruit_market/start.sh");
    println!("====================================================================");
    println!();
    println!("Press Ctrl+C to shutdown...");
    println!();

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;

    println!();
    println!("Shutting down...");

    // Stop the listener
    listener_handle.stop();

    // Brief delay for cleanup
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Shutdown the runtime
    runtime.shutdown_all().await?;

    println!("Server shutdown complete.");

    Ok(())
}
