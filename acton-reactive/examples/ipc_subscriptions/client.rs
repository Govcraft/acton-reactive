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

//! IPC Subscriptions Example - Client
//!
//! This example demonstrates the client side of subscription-based IPC,
//! subscribing to message types and receiving push notifications.
//!
//! # Features
//!
//! - **Subscribe to Message Types**: Client sends subscription requests
//! - **Receive Push Notifications**: Real-time updates for subscribed types
//! - **Selective Subscription**: Subscribe to specific message types
//! - **Unsubscribe**: Stop receiving notifications for specific types
//!
//! # Running This Example
//!
//! First, start the server:
//! ```bash
//! cargo run --example ipc_subscriptions_server --features ipc
//! ```
//!
//! Then, in another terminal, run this client:
//! ```bash
//! cargo run --example ipc_subscriptions_client --features ipc
//! ```

use std::path::PathBuf;
use std::time::Duration;

use acton_reactive::ipc::protocol::{
    read_frame, write_frame, MAX_FRAME_SIZE, MSG_TYPE_PUSH, MSG_TYPE_RESPONSE,
    MSG_TYPE_SUBSCRIBE, MSG_TYPE_UNSUBSCRIBE,
};
use acton_reactive::ipc::{
    socket_exists, socket_is_alive, IpcConfig, IpcPushNotification, IpcSubscribeRequest,
    IpcSubscriptionResponse, IpcUnsubscribeRequest,
};
use serde::{Deserialize, Serialize};
use tokio::net::UnixStream;
use tokio::time::timeout;

/// Default server name to connect to.
const DEFAULT_SERVER: &str = "ipc_subscriptions_server";

// ============================================================================
// Message Types (must match server definitions for deserialization)
// ============================================================================

/// Stock price update.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PriceUpdate {
    symbol: String,
    price: f64,
    change: f64,
    timestamp_ms: u64,
}

/// Trade execution event.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct TradeExecuted {
    trade_id: String,
    symbol: String,
    quantity: u32,
    price: f64,
    side: String,
}

/// System status notification.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SystemStatus {
    status: String,
    message: String,
    timestamp_ms: u64,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Resolves the socket path from command line arguments or defaults.
fn resolve_socket_path() -> PathBuf {
    let args: Vec<String> = std::env::args().collect();

    // Check for --server argument
    if let Some(pos) = args.iter().position(|a| a == "--server") {
        if let Some(server_name) = args.get(pos + 1) {
            let mut config = IpcConfig::load();
            config.socket.app_name = Some(server_name.clone());
            return config.socket_path();
        }
    }

    // Check for direct path argument
    if let Some(arg) = args.get(1) {
        if !arg.starts_with("--") {
            return PathBuf::from(arg);
        }
    }

    // Default: connect to ipc_subscriptions server
    let mut config = IpcConfig::load();
    config.socket.app_name = Some(DEFAULT_SERVER.to_string());
    config.socket_path()
}

/// Sends a subscribe request.
async fn send_subscribe(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    message_types: Vec<String>,
) -> Result<String, Box<dyn std::error::Error>> {
    let request = IpcSubscribeRequest::new(message_types);
    let correlation_id = request.correlation_id.clone();

    let payload = serde_json::to_vec(&request)?;
    write_frame(writer, MSG_TYPE_SUBSCRIBE, &payload).await?;

    Ok(correlation_id)
}

/// Sends an unsubscribe request.
async fn send_unsubscribe(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    message_types: Vec<String>,
) -> Result<String, Box<dyn std::error::Error>> {
    let request = IpcUnsubscribeRequest::new(message_types);
    let correlation_id = request.correlation_id.clone();

    let payload = serde_json::to_vec(&request)?;
    write_frame(writer, MSG_TYPE_UNSUBSCRIBE, &payload).await?;

    Ok(correlation_id)
}

/// Displays a push notification.
fn display_push_notification(notification: &IpcPushNotification) {
    match notification.message_type.as_str() {
        "PriceUpdate" => {
            if let Ok(update) = serde_json::from_value::<PriceUpdate>(notification.payload.clone())
            {
                let arrow = if update.change >= 0.0 { "^" } else { "v" };
                let color = if update.change >= 0.0 { "32" } else { "31" }; // Green/Red
                println!(
                    "  \x1b[{}m{} {} ${:.2} ({:+.2})\x1b[0m",
                    color, arrow, update.symbol, update.price, update.change
                );
            }
        }
        "TradeExecuted" => {
            if let Ok(trade) = serde_json::from_value::<TradeExecuted>(notification.payload.clone())
            {
                let color = if trade.side == "BUY" { "36" } else { "35" }; // Cyan/Magenta
                println!(
                    "  \x1b[{}m[TRADE] {} {} {} @ ${:.2} ({})\x1b[0m",
                    color, trade.trade_id, trade.side, trade.symbol, trade.price, trade.quantity
                );
            }
        }
        "SystemStatus" => {
            if let Ok(status) = serde_json::from_value::<SystemStatus>(notification.payload.clone())
            {
                println!(
                    "  \x1b[33m[SYSTEM] {}: {}\x1b[0m",
                    status.status, status.message
                );
            }
        }
        _ => {
            println!(
                "  [{}] {:?}",
                notification.message_type, notification.payload
            );
        }
    }
}

/// Handles a subscription response.
fn handle_subscription_response(response: &IpcSubscriptionResponse) {
    if response.success {
        println!("  Subscribed to: {:?}", response.subscribed_types);
    } else {
        println!(
            "  Subscription failed: {}",
            response.error.as_deref().unwrap_or("Unknown error")
        );
    }
}

// ============================================================================
// Demo Scenarios
// ============================================================================

/// Demo 1: Subscribe to all message types and receive notifications.
async fn demo_subscribe_all(
    reader: &mut tokio::net::unix::OwnedReadHalf,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> Result<(), Box<dyn std::error::Error>> {
    println!();
    println!("====================================================================");
    println!("  Demo 1: Subscribe to ALL Message Types");
    println!("====================================================================");

    // Subscribe to all types
    let types = vec![
        "PriceUpdate".to_string(),
        "TradeExecuted".to_string(),
        "SystemStatus".to_string(),
    ];
    println!("\nSubscribing to: {types:?}");

    let _corr_id = send_subscribe(writer, types).await?;

    // Wait for subscription response
    let (msg_type, payload) = timeout(Duration::from_secs(5), read_frame(reader, MAX_FRAME_SIZE))
        .await??;

    if msg_type == MSG_TYPE_RESPONSE {
        let response: IpcSubscriptionResponse = serde_json::from_slice(&payload)?;
        handle_subscription_response(&response);
    }

    println!("\nReceiving notifications for 15 seconds...\n");

    // Receive push notifications for 15 seconds
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_secs(1), read_frame(reader, MAX_FRAME_SIZE)).await {
            Ok(Ok((msg_type, payload))) => {
                if msg_type == MSG_TYPE_PUSH {
                    let notification: IpcPushNotification = serde_json::from_slice(&payload)?;
                    display_push_notification(&notification);
                }
            }
            Ok(Err(e)) => {
                println!("Error reading frame: {e}");
                break;
            }
            Err(_) => {
                // Timeout - continue waiting
            }
        }
    }

    Ok(())
}

/// Demo 2: Subscribe only to price updates.
async fn demo_prices_only(
    reader: &mut tokio::net::unix::OwnedReadHalf,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> Result<(), Box<dyn std::error::Error>> {
    println!();
    println!("====================================================================");
    println!("  Demo 2: Subscribe to Price Updates Only");
    println!("====================================================================");

    // Unsubscribe from all first
    println!("\nUnsubscribing from all types...");
    let _corr_id = send_unsubscribe(writer, vec![]).await?;

    // Read unsubscribe response
    let (msg_type, payload) = timeout(Duration::from_secs(5), read_frame(reader, MAX_FRAME_SIZE))
        .await??;

    if msg_type == MSG_TYPE_RESPONSE {
        let response: IpcSubscriptionResponse = serde_json::from_slice(&payload)?;
        println!("  After unsubscribe: {:?}", response.subscribed_types);
    }

    // Subscribe only to prices
    println!("\nSubscribing to PriceUpdate only...");
    let _corr_id = send_subscribe(writer, vec!["PriceUpdate".to_string()]).await?;

    // Wait for subscription response
    let (msg_type, payload) = timeout(Duration::from_secs(5), read_frame(reader, MAX_FRAME_SIZE))
        .await??;

    if msg_type == MSG_TYPE_RESPONSE {
        let response: IpcSubscriptionResponse = serde_json::from_slice(&payload)?;
        handle_subscription_response(&response);
    }

    println!("\nReceiving ONLY price updates for 10 seconds...\n");
    println!("(Note: You should NOT see trade or system events)\n");

    // Receive push notifications for 10 seconds
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_secs(1), read_frame(reader, MAX_FRAME_SIZE)).await {
            Ok(Ok((msg_type, payload))) => {
                if msg_type == MSG_TYPE_PUSH {
                    let notification: IpcPushNotification = serde_json::from_slice(&payload)?;
                    display_push_notification(&notification);
                }
            }
            Ok(Err(e)) => {
                println!("Error reading frame: {e}");
                break;
            }
            Err(_) => {
                // Timeout - continue waiting
            }
        }
    }

    Ok(())
}

/// Demo 3: Subscribe to trades only.
async fn demo_trades_only(
    reader: &mut tokio::net::unix::OwnedReadHalf,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> Result<(), Box<dyn std::error::Error>> {
    println!();
    println!("====================================================================");
    println!("  Demo 3: Subscribe to Trades Only");
    println!("====================================================================");

    // Unsubscribe from prices
    println!("\nUnsubscribing from PriceUpdate...");
    let _corr_id = send_unsubscribe(writer, vec!["PriceUpdate".to_string()]).await?;

    // Read unsubscribe response
    let (msg_type, payload) = timeout(Duration::from_secs(5), read_frame(reader, MAX_FRAME_SIZE))
        .await??;

    if msg_type == MSG_TYPE_RESPONSE {
        let response: IpcSubscriptionResponse = serde_json::from_slice(&payload)?;
        println!("  After unsubscribe: {:?}", response.subscribed_types);
    }

    // Subscribe to trades
    println!("\nSubscribing to TradeExecuted...");
    let _corr_id = send_subscribe(writer, vec!["TradeExecuted".to_string()]).await?;

    // Wait for subscription response
    let (msg_type, payload) = timeout(Duration::from_secs(5), read_frame(reader, MAX_FRAME_SIZE))
        .await??;

    if msg_type == MSG_TYPE_RESPONSE {
        let response: IpcSubscriptionResponse = serde_json::from_slice(&payload)?;
        handle_subscription_response(&response);
    }

    println!("\nReceiving ONLY trade events for 12 seconds...\n");
    println!("(Trades occur every 5 seconds)\n");

    // Receive push notifications for 12 seconds
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_secs(1), read_frame(reader, MAX_FRAME_SIZE)).await {
            Ok(Ok((msg_type, payload))) => {
                if msg_type == MSG_TYPE_PUSH {
                    let notification: IpcPushNotification = serde_json::from_slice(&payload)?;
                    display_push_notification(&notification);
                }
            }
            Ok(Err(e)) => {
                println!("Error reading frame: {e}");
                break;
            }
            Err(_) => {
                // Timeout - continue waiting
            }
        }
    }

    Ok(())
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("====================================================================");
    println!("       IPC Subscriptions Example (Client)");
    println!("====================================================================");
    println!();

    // Resolve socket path
    let socket_path = resolve_socket_path();
    println!("Socket path: {}", socket_path.display());

    // Check if socket is available
    if !socket_exists(&socket_path) {
        eprintln!("\nError: Socket does not exist at {}", socket_path.display());
        eprintln!("Make sure the ipc_subscriptions server is running:");
        eprintln!("  cargo run --example ipc_subscriptions_server --features ipc");
        std::process::exit(1);
    }

    if !socket_is_alive(&socket_path).await {
        eprintln!("\nError: Socket exists but is not responding");
        eprintln!("The server may have crashed. Try restarting it.");
        std::process::exit(1);
    }

    // Connect to the socket
    println!("Connecting to server...");
    let stream = UnixStream::connect(&socket_path).await?;
    println!("Connected successfully!");

    let (mut reader, mut writer) = stream.into_split();

    // Run demonstrations
    demo_subscribe_all(&mut reader, &mut writer).await?;
    demo_prices_only(&mut reader, &mut writer).await?;
    demo_trades_only(&mut reader, &mut writer).await?;

    // Final unsubscribe
    println!();
    println!("====================================================================");
    println!("  Cleanup: Unsubscribing from all");
    println!("====================================================================");

    let _corr_id = send_unsubscribe(&mut writer, vec![]).await?;
    let (msg_type, payload) = timeout(Duration::from_secs(5), read_frame(&mut reader, MAX_FRAME_SIZE))
        .await??;

    if msg_type == MSG_TYPE_RESPONSE {
        let response: IpcSubscriptionResponse = serde_json::from_slice(&payload)?;
        println!("  Final subscriptions: {:?}", response.subscribed_types);
    }

    println!();
    println!("====================================================================");
    println!("  All subscription demos completed!");
    println!("====================================================================");

    Ok(())
}
