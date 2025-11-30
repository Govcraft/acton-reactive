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

//! IPC Subscriptions Example - Server
//!
//! This example demonstrates the **push notification pattern** in acton-reactive IPC,
//! where external clients can subscribe to specific message types and receive
//! push notifications when those messages are broadcast internally.
//!
//! # Features
//!
//! - **Subscription-Based Push**: Clients subscribe to message types and receive
//!   real-time push notifications when messages are broadcast.
//! - **Price Feed Service**: A market data service that periodically broadcasts
//!   stock price updates.
//! - **System Events**: Demonstrates broadcasting system status notifications.
//!
//! # How It Works
//!
//! 1. Server starts with a price feed actor that periodically broadcasts prices
//! 2. IPC client connects and sends a `MSG_TYPE_SUBSCRIBE` frame with message types
//! 3. Server registers the subscription with the `SubscriptionManager`
//! 4. When internal messages are broadcast, the IPC listener forwards them as
//!    `IpcPushNotification` frames to subscribed clients
//! 5. Client can unsubscribe at any time with `MSG_TYPE_UNSUBSCRIBE`
//!
//! # Running This Example
//!
//! Start the server:
//! ```bash
//! cargo run --example ipc_subscriptions_server --features ipc
//! ```
//!
//! Then connect with the client (in another terminal):
//! ```bash
//! cargo run --example ipc_subscriptions_client --features ipc
//! ```

use std::time::Duration;

use acton_macro::acton_message;
use acton_reactive::ipc::{socket_exists, IpcConfig};
use acton_reactive::prelude::*;
use tracing_subscriber::EnvFilter;

// ============================================================================
// Message Definitions - Market Data
// ============================================================================

/// A stock price update broadcast by the price feed service.
///
/// Clients can subscribe to `PriceUpdate` to receive these notifications.
#[acton_message(ipc)]
struct PriceUpdate {
    /// Stock symbol (e.g., "AAPL", "GOOGL").
    symbol: String,
    /// Current price.
    price: f64,
    /// Price change from previous update.
    change: f64,
    /// Timestamp of the update (Unix ms).
    timestamp_ms: u64,
}

/// A trade execution event.
///
/// Clients can subscribe to `TradeExecuted` to receive these notifications.
#[acton_message(ipc)]
struct TradeExecuted {
    /// Trade ID.
    trade_id: String,
    /// Stock symbol.
    symbol: String,
    /// Trade quantity.
    quantity: u32,
    /// Execution price.
    price: f64,
    /// Trade side (buy/sell).
    side: String,
}

/// System status notification.
///
/// Clients can subscribe to `SystemStatus` to receive these notifications.
#[acton_message(ipc)]
struct SystemStatus {
    /// Status type (e.g., `market_open`, `market_close`, `maintenance`).
    status: String,
    /// Human-readable message.
    message: String,
    /// Timestamp.
    timestamp_ms: u64,
}

// ============================================================================
// Internal Control Messages (for triggering broadcasts)
// ============================================================================

/// Internal message to trigger a price broadcast.
#[acton_message]
#[derive(Default)]
struct BroadcastPrices;

/// Internal message to trigger a trade broadcast.
#[acton_message]
struct BroadcastTrade {
    symbol: String,
    quantity: u32,
    price: f64,
    side: String,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Returns current Unix timestamp in milliseconds, saturating to `u64::MAX` if overflow.
fn timestamp_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

// ============================================================================
// Actor States
// ============================================================================

/// Price feed service state - generates and broadcasts market data.
/// Note: We don't use `#[acton_actor]` because we need a custom `Default` impl.
#[derive(Debug, Clone)]
struct PriceFeedState {
    /// Current simulated prices for each symbol.
    prices: std::collections::HashMap<String, f64>,
    /// Update counter for generating trade IDs.
    update_count: u64,
}

impl Default for PriceFeedState {
    fn default() -> Self {
        let mut prices = std::collections::HashMap::new();
        prices.insert("AAPL".to_string(), 175.50);
        prices.insert("GOOGL".to_string(), 142.30);
        prices.insert("MSFT".to_string(), 378.90);
        prices.insert("AMZN".to_string(), 178.25);

        Self {
            prices,
            update_count: 0,
        }
    }
}

// ============================================================================
// Actor Creation
// ============================================================================

/// Creates the price feed actor that broadcasts market data.
async fn create_price_feed_actor(runtime: &mut ActorRuntime) -> ActorHandle {
    let mut price_feed = runtime.new_actor_with_name::<PriceFeedState>("price_feed".to_string());

    // Handle broadcast price requests
    price_feed.mutate_on::<BroadcastPrices>(|actor, _envelope| {
        let broker = actor.broker().clone();
        actor.model.update_count += 1;

        // Generate price updates with small random-ish changes
        let seed = actor.model.update_count;
        let updates: Vec<PriceUpdate> = actor
            .model
            .prices
            .iter_mut()
            .enumerate()
            .map(|(i, (symbol, price))| {
                // Deterministic "random" change based on seed and symbol index
                let combined = seed.wrapping_add(i as u64);
                let direction = if combined.is_multiple_of(3) {
                    -1.0
                } else {
                    1.0
                };
                let magnitude = f64::from((combined % 100) as u32) / 100.0;
                let change = direction * magnitude;
                *price += change;

                PriceUpdate {
                    symbol: symbol.clone(),
                    price: (*price * 100.0).round() / 100.0,
                    change: (change * 100.0).round() / 100.0,
                    timestamp_ms: timestamp_millis(),
                }
            })
            .collect();

        Box::pin(async move {
            for update in updates {
                println!(
                    "  [PriceFeed] Broadcasting: {} @ ${:.2} ({:+.2})",
                    update.symbol, update.price, update.change
                );
                broker.broadcast(update).await;
            }
        })
    });

    // Handle trade broadcast requests
    price_feed.mutate_on::<BroadcastTrade>(|actor, envelope| {
        let broker = actor.broker().clone();
        actor.model.update_count += 1;

        let msg = envelope.message();
        let trade = TradeExecuted {
            trade_id: format!("TRD{:06}", actor.model.update_count),
            symbol: msg.symbol.clone(),
            quantity: msg.quantity,
            price: msg.price,
            side: msg.side.clone(),
        };

        println!(
            "  [PriceFeed] Broadcasting trade: {} {} {} @ ${:.2}",
            trade.side, trade.quantity, trade.symbol, trade.price
        );

        Box::pin(async move {
            broker.broadcast(trade).await;
        })
    });

    price_feed.start().await
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("acton=info".parse()?))
        .init();

    println!("====================================================================");
    println!("       IPC Subscriptions Example (Server)");
    println!("====================================================================");
    println!();

    let mut runtime = ActonApp::launch();

    // Register IPC message types for subscription forwarding
    let registry = runtime.ipc_registry();

    // Market data messages (broadcast types that clients can subscribe to)
    registry.register::<PriceUpdate>("PriceUpdate");
    registry.register::<TradeExecuted>("TradeExecuted");
    registry.register::<SystemStatus>("SystemStatus");

    println!(
        "Registered {} IPC message types for subscriptions",
        registry.len()
    );
    println!("  - PriceUpdate: Stock price changes");
    println!("  - TradeExecuted: Trade execution events");
    println!("  - SystemStatus: System status notifications");

    // Create the price feed actor
    let price_feed = create_price_feed_actor(&mut runtime).await;
    println!("\nPrice feed service started");

    // Expose actors for IPC access
    runtime.ipc_expose("price_feed", price_feed.clone());
    println!("Exposed actors: price_feed");

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

    // Broadcast initial system status
    let broker = runtime.broker();
    broker
        .broadcast(SystemStatus {
            status: "market_open".to_string(),
            message: "Market is now open for trading".to_string(),
            timestamp_ms: timestamp_millis(),
        })
        .await;

    println!();
    println!("====================================================================");
    println!("  Server is ready for subscription-based IPC!");
    println!();
    println!("  Subscribable message types:");
    println!("    - PriceUpdate     : Stock price updates (every 2 seconds)");
    println!("    - TradeExecuted   : Trade execution events (every 5 seconds)");
    println!("    - SystemStatus    : System status changes");
    println!();
    println!("  Run the client in another terminal:");
    println!("    cargo run --example ipc_subscriptions_client --features ipc");
    println!("====================================================================");
    println!();
    println!("Press Ctrl+C to shutdown...");
    println!();

    // Start background tasks for periodic broadcasts
    let price_feed_handle = price_feed.clone();
    let price_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            interval.tick().await;
            price_feed_handle.send(BroadcastPrices).await;
        }
    });

    let price_feed_handle2 = price_feed.clone();
    let trade_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        let symbols = ["AAPL", "GOOGL", "MSFT", "AMZN"];
        let mut idx: u32 = 0;
        loop {
            interval.tick().await;
            let symbol = symbols[(idx as usize) % symbols.len()];
            idx += 1;

            price_feed_handle2
                .send(BroadcastTrade {
                    symbol: symbol.to_string(),
                    quantity: idx * 100,
                    price: f64::from(idx).mul_add(0.5, 150.0),
                    side: if idx.is_multiple_of(2) { "BUY" } else { "SELL" }.to_string(),
                })
                .await;
        }
    });

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;

    println!();
    println!("Shutting down...");

    // Cancel background tasks
    price_task.abort();
    trade_task.abort();

    // Broadcast shutdown status
    broker
        .broadcast(SystemStatus {
            status: "market_close".to_string(),
            message: "Market is now closed".to_string(),
            timestamp_ms: timestamp_millis(),
        })
        .await;

    // Give time for final broadcast to reach clients
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stop the listener
    listener_handle.stop();

    // Brief delay for cleanup
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Shutdown the runtime
    runtime.shutdown_all().await?;

    println!("Server shutdown complete.");

    Ok(())
}
