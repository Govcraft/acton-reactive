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

//! IPC Serialization Foundation Example
//!
//! This example demonstrates the IPC (Inter-Process Communication) serialization
//! infrastructure that enables external processes to communicate with acton-reactive
//! agents.
//!
//! # Key Concepts Demonstrated
//!
//! 1. **Type Registration**: Registering message types with `IpcTypeRegistry` so they
//!    can be deserialized from external JSON data.
//!
//! 2. **Agent Exposure**: Exposing agents via logical names for IPC routing using
//!    `runtime.ipc_expose()`.
//!
//! 3. **Message Envelopes**: Using `IpcEnvelope` format for incoming messages with
//!    correlation IDs, targets, and type information.
//!
//! 4. **Response Handling**: Creating `IpcResponse` envelopes for success/error
//!    responses back to external callers.
//!
//! # Running This Example
//!
//! ```bash
//! cargo run --example ipc_basic --features ipc
//! ```

use std::sync::Arc;

use acton_macro::acton_actor;
use acton_reactive::prelude::*;
use serde::{Deserialize, Serialize};

// ============================================================================
// Message Definitions
// ============================================================================

/// A price update message that can be sent via IPC.
///
/// This message includes serde derives so it can be serialized/deserialized
/// for cross-process communication.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PriceUpdate {
    symbol: String,
    price: f64,
    timestamp: u64,
}

/// A request to get the current price of a symbol.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct GetPrice {
    symbol: String,
}

/// Response containing the current price.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PriceResponse {
    symbol: String,
    price: f64,
    found: bool,
}

// ============================================================================
// Agent State
// ============================================================================

/// State for our price tracking agent.
#[acton_actor]
struct PriceServiceState {
    /// Tracks the latest prices by symbol.
    prices: std::collections::HashMap<String, f64>,
    /// Count of updates received.
    update_count: usize,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Registers message types with the IPC type registry.
fn register_ipc_types(registry: &Arc<IpcTypeRegistry>) {
    registry.register::<PriceUpdate>("PriceUpdate");
    registry.register::<GetPrice>("GetPrice");
    registry.register::<PriceResponse>("PriceResponse");

    println!("Registered {} IPC message types:", registry.len());
    for type_name in registry.type_names() {
        println!("  - {type_name}");
    }
    println!();
}

/// Processes an incoming IPC envelope and routes it to the appropriate agent.
async fn process_ipc_envelope(
    runtime: &AgentRuntime,
    registry: &Arc<IpcTypeRegistry>,
    incoming_json: &str,
) {
    println!("Received IPC envelope:\n{incoming_json}\n");

    // Parse the envelope
    let envelope: IpcEnvelope = serde_json::from_str(incoming_json).expect("Invalid JSON");
    println!(
        "Parsed envelope: correlation_id={}, target={}, type={}",
        envelope.correlation_id, envelope.target, envelope.message_type
    );

    // Look up the target agent
    if let Some(target_handle) = runtime.ipc_lookup(&envelope.target) {
        println!("Found target agent: {}\n", target_handle.id());

        // Deserialize the payload using the type registry
        match registry.deserialize_value(&envelope.message_type, &envelope.payload) {
            Ok(message) => {
                // Send the deserialized message to the agent
                // Note: In production, you'd use the boxed message directly.
                // Here we demonstrate the flow by re-creating the message.
                let price_update: &PriceUpdate = (*message)
                    .as_any()
                    .downcast_ref()
                    .expect("Type mismatch");

                target_handle.send(price_update.clone()).await;

                // Create success response
                let response = IpcResponse::success(
                    &envelope.correlation_id,
                    Some(serde_json::json!({"status": "delivered"})),
                );
                println!(
                    "Response: {}\n",
                    serde_json::to_string_pretty(&response).unwrap()
                );
            }
            Err(e) => {
                let response = IpcResponse::error(&envelope.correlation_id, &e);
                println!(
                    "Error response: {}\n",
                    serde_json::to_string_pretty(&response).unwrap()
                );
            }
        }
    } else {
        let response = IpcResponse::error(
            &envelope.correlation_id,
            &IpcError::AgentNotFound(envelope.target.clone()),
        );
        println!(
            "Error: {}\n",
            serde_json::to_string_pretty(&response).unwrap()
        );
    }
}

/// Sends multiple price updates to demonstrate internal messaging.
async fn send_price_updates(price_handle: &AgentHandle) {
    println!("--- Sending More Updates ---\n");

    let updates = vec![
        PriceUpdate {
            symbol: "GOOGL".to_string(),
            price: 141.80,
            timestamp: 1_700_000_001,
        },
        PriceUpdate {
            symbol: "MSFT".to_string(),
            price: 378.91,
            timestamp: 1_700_000_002,
        },
        PriceUpdate {
            symbol: "AAPL".to_string(),
            price: 179.50,
            timestamp: 1_700_000_003,
        },
    ];

    for update in updates {
        price_handle.send(update).await;
    }
}

/// Demonstrates programmatic envelope creation.
fn demonstrate_envelope_creation() {
    println!("\n--- Creating IpcEnvelope Programmatically ---\n");

    let new_envelope = IpcEnvelope::new(
        "prices",
        "PriceUpdate",
        serde_json::json!({
            "symbol": "AMZN",
            "price": 178.25,
            "timestamp": 1_700_000_004
        }),
    );

    println!("Created envelope with auto-generated correlation_id:");
    println!("{}\n", serde_json::to_string_pretty(&new_envelope).unwrap());
}

/// Demonstrates error handling for various IPC failure scenarios.
fn demonstrate_error_handling(runtime: &AgentRuntime, registry: &Arc<IpcTypeRegistry>) {
    println!("--- Error Handling Examples ---\n");

    // Unknown message type
    let unknown_result = registry.deserialize("UnknownType", b"{}");
    if let Err(e) = unknown_result {
        println!("Error deserializing unknown type: {e}");
        let response = IpcResponse::error("req_err_1", &e);
        println!(
            "Response: {}\n",
            serde_json::to_string_pretty(&response).unwrap()
        );
    }

    // Agent not found
    let missing_agent = runtime.ipc_lookup("nonexistent");
    if missing_agent.is_none() {
        let err = IpcError::AgentNotFound("nonexistent".to_string());
        println!("Error looking up agent: {err}");
        let response = IpcResponse::error("req_err_2", &err);
        println!(
            "Response: {}\n",
            serde_json::to_string_pretty(&response).unwrap()
        );
    }
}

/// Demonstrates hiding an agent from IPC access.
fn demonstrate_ipc_hiding(runtime: &AgentRuntime) {
    println!("--- Hiding Agent from IPC ---\n");
    let hidden = runtime.ipc_hide("prices");
    println!(
        "Removed 'prices' from IPC: {}",
        hidden.map_or_else(|| "not found".to_string(), |h| h.id().to_string())
    );
    println!("Exposed agents remaining: {}\n", runtime.ipc_agent_count());
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() {
    println!("=== IPC Serialization Foundation Example ===\n");

    // 1. Launch the Acton runtime
    let mut runtime = ActonApp::launch();

    // 2. Register message types with the IPC type registry
    let registry = runtime.ipc_registry();
    register_ipc_types(&registry);

    // 3. Create and configure the price service agent
    let mut price_service =
        runtime.new_agent_with_name::<PriceServiceState>("price_service".to_string());

    price_service
        // Handle price updates
        .mutate_on::<PriceUpdate>(|agent, envelope| {
            let msg = envelope.message().clone();
            println!(
                "[PriceService] Received update: {} = ${:.2} (ts: {})",
                msg.symbol, msg.price, msg.timestamp
            );

            // Update our price map
            agent.model.prices.insert(msg.symbol, msg.price);
            agent.model.update_count += 1;

            AgentReply::immediate()
        })
        // Handle price queries
        .mutate_on::<GetPrice>(|agent, envelope| {
            let msg = envelope.message().clone();
            println!("[PriceService] Query for: {}", msg.symbol);

            let price = agent.model.prices.get(&msg.symbol).copied();
            let reply_envelope = envelope.reply_envelope();

            Box::pin(async move {
                let response = PriceResponse {
                    symbol: msg.symbol.clone(),
                    price: price.unwrap_or(0.0),
                    found: price.is_some(),
                };
                reply_envelope.send(response).await;
            })
        })
        .after_stop(|agent| {
            println!(
                "\n[PriceService] Shutting down. Processed {} updates.",
                agent.model.update_count
            );
            println!("Final prices: {:?}", agent.model.prices);
            AgentReply::immediate()
        });

    // 4. Start the agent and get its handle
    let price_handle = price_service.start().await;

    // 5. Expose the agent for IPC access using a logical name
    runtime.ipc_expose("prices", price_handle.clone());

    println!("Exposed {} agent(s) for IPC:", runtime.ipc_agent_count());
    println!("  - 'prices' -> {}\n", price_handle.id());

    // ========================================================================
    // Simulate IPC Message Processing
    // ========================================================================

    println!("--- Simulating IPC Message Flow ---\n");

    // Simulate an external process sending JSON messages
    let incoming_json = r#"{
        "correlation_id": "req_001",
        "target": "prices",
        "message_type": "PriceUpdate",
        "payload": {
            "symbol": "AAPL",
            "price": 178.25,
            "timestamp": 1700000000
        }
    }"#;

    process_ipc_envelope(&runtime, &registry, incoming_json).await;

    // Give the agent time to process the IPC message
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Send more price updates
    send_price_updates(&price_handle).await;

    // Give the agent time to process all updates
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Demonstrate envelope creation
    demonstrate_envelope_creation();

    // Demonstrate error handling
    demonstrate_error_handling(&runtime, &registry);

    // Demonstrate hiding agents from IPC
    demonstrate_ipc_hiding(&runtime);

    // Shutdown
    runtime
        .shutdown_all()
        .await
        .expect("Failed to shut down system");

    println!("\n=== Example Complete ===");
}
