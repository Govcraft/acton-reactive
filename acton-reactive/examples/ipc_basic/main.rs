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

/// A message to print text to the console.
#[derive(Clone, Debug)]
struct Print(String);

/// A message to print a section header.
#[derive(Clone, Debug)]
struct PrintSection(String);

/// Message to initialize the price service with a printer handle.
#[derive(Clone, Debug)]
struct InitPrinter(AgentHandle);

// ============================================================================
// Agent States
// ============================================================================

/// State for the console printer agent.
/// All console output goes through this agent to ensure proper ordering.
#[acton_actor]
struct PrinterState;

/// State for our price tracking agent.
#[acton_actor]
struct PriceServiceState {
    prices: std::collections::HashMap<String, f64>,
    update_count: usize,
    printer: Option<AgentHandle>,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Creates and starts the printer agent for coordinated console output.
async fn create_printer_agent(runtime: &mut AgentRuntime) -> AgentHandle {
    let mut printer_agent = runtime.new_agent::<PrinterState>();
    printer_agent
        .act_on::<Print>(|_agent, envelope| {
            println!("{}", envelope.message().0);
            AgentReply::immediate()
        })
        .act_on::<PrintSection>(|_agent, envelope| {
            println!("\n--- {} ---\n", envelope.message().0);
            AgentReply::immediate()
        });
    printer_agent.start().await
}

/// Creates and starts the price service agent.
async fn create_price_service(
    runtime: &mut AgentRuntime,
    printer: &AgentHandle,
) -> AgentHandle {
    let mut price_service =
        runtime.new_agent_with_name::<PriceServiceState>("price_service".to_string());

    price_service
        .mutate_on::<InitPrinter>(|agent, envelope| {
            agent.model.printer = Some(envelope.message().0.clone());
            AgentReply::immediate()
        })
        .mutate_on::<PriceUpdate>(|agent, envelope| {
            let msg = envelope.message().clone();
            let printer = agent.model.printer.clone();
            agent.model.prices.insert(msg.symbol.clone(), msg.price);
            agent.model.update_count += 1;

            Box::pin(async move {
                if let Some(p) = printer {
                    p.send(Print(format!(
                        "[PriceService] Received update: {} = ${:.2} (ts: {})",
                        msg.symbol, msg.price, msg.timestamp
                    )))
                    .await;
                }
            })
        })
        .mutate_on::<GetPrice>(|agent, envelope| {
            let msg = envelope.message().clone();
            let printer = agent.model.printer.clone();
            let price = agent.model.prices.get(&msg.symbol).copied();
            let reply_envelope = envelope.reply_envelope();

            Box::pin(async move {
                if let Some(p) = &printer {
                    p.send(Print(format!("[PriceService] Query for: {}", msg.symbol)))
                        .await;
                }
                let response = PriceResponse {
                    symbol: msg.symbol.clone(),
                    price: price.unwrap_or(0.0),
                    found: price.is_some(),
                };
                reply_envelope.send(response).await;
            })
        })
        .after_stop(|agent| {
            let printer = agent.model.printer.clone();
            let update_count = agent.model.update_count;
            let prices = agent.model.prices.clone();

            Box::pin(async move {
                if let Some(p) = printer {
                    p.send(Print(format!(
                        "\n[PriceService] Shutting down. Processed {update_count} updates."
                    )))
                    .await;
                    p.send(Print(format!("Final prices: {prices:?}"))).await;
                }
            })
        });

    let handle = price_service.start().await;
    handle.send(InitPrinter(printer.clone())).await;
    handle
}

/// Processes an incoming IPC envelope and routes it to the target agent.
async fn process_ipc_envelope(
    runtime: &AgentRuntime,
    registry: &Arc<IpcTypeRegistry>,
    printer: &AgentHandle,
    incoming_json: &str,
) {
    printer
        .send(Print(format!("Received IPC envelope:\n{incoming_json}")))
        .await;

    let envelope: IpcEnvelope = serde_json::from_str(incoming_json).expect("Invalid JSON");
    printer
        .send(Print(format!(
            "\nParsed envelope: correlation_id={}, target={}, type={}",
            envelope.correlation_id, envelope.target, envelope.message_type
        )))
        .await;

    if let Some(target_handle) = runtime.ipc_lookup(&envelope.target) {
        printer
            .send(Print(format!("Found target agent: {}", target_handle.id())))
            .await;

        match registry.deserialize_value(&envelope.message_type, &envelope.payload) {
            Ok(message) => {
                let price_update: &PriceUpdate =
                    (*message).as_any().downcast_ref().expect("Type mismatch");
                target_handle.send(price_update.clone()).await;

                let response = IpcResponse::success(
                    &envelope.correlation_id,
                    Some(serde_json::json!({"status": "delivered"})),
                );
                printer
                    .send(Print(format!(
                        "\nResponse: {}",
                        serde_json::to_string_pretty(&response).unwrap()
                    )))
                    .await;
            }
            Err(e) => {
                let response = IpcResponse::error(&envelope.correlation_id, &e);
                printer
                    .send(Print(format!(
                        "\nError response: {}",
                        serde_json::to_string_pretty(&response).unwrap()
                    )))
                    .await;
            }
        }
    } else {
        let response = IpcResponse::error(
            &envelope.correlation_id,
            &IpcError::AgentNotFound(envelope.target.clone()),
        );
        printer
            .send(Print(format!(
                "\nError: {}",
                serde_json::to_string_pretty(&response).unwrap()
            )))
            .await;
    }
}

/// Demonstrates programmatic envelope creation with MTI-generated correlation ID.
async fn demonstrate_envelope_creation(printer: &AgentHandle) {
    let new_envelope = IpcEnvelope::new(
        "prices",
        "PriceUpdate",
        serde_json::json!({
            "symbol": "AMZN",
            "price": 178.25,
            "timestamp": 1_700_000_004
        }),
    );

    printer
        .send(Print(
            "Created envelope with auto-generated correlation_id:".to_string(),
        ))
        .await;
    printer
        .send(Print(serde_json::to_string_pretty(&new_envelope).unwrap()))
        .await;
}

/// Demonstrates error handling for various IPC failure scenarios.
async fn demonstrate_error_handling(
    runtime: &AgentRuntime,
    registry: &Arc<IpcTypeRegistry>,
    printer: &AgentHandle,
) {
    // Unknown message type
    let unknown_result = registry.deserialize("UnknownType", b"{}");
    if let Err(e) = unknown_result {
        printer
            .send(Print(format!("Error deserializing unknown type: {e}")))
            .await;
        let response = IpcResponse::error("req_err_1", &e);
        printer
            .send(Print(format!(
                "Response: {}",
                serde_json::to_string_pretty(&response).unwrap()
            )))
            .await;
    }

    // Agent not found
    if runtime.ipc_lookup("nonexistent").is_none() {
        let err = IpcError::AgentNotFound("nonexistent".to_string());
        printer
            .send(Print(format!("\nError looking up agent: {err}")))
            .await;
        let response = IpcResponse::error("req_err_2", &err);
        printer
            .send(Print(format!(
                "Response: {}",
                serde_json::to_string_pretty(&response).unwrap()
            )))
            .await;
    }
}

/// Demonstrates hiding an agent from IPC access.
async fn demonstrate_ipc_hiding(runtime: &AgentRuntime, printer: &AgentHandle) {
    let hidden = runtime.ipc_hide("prices");
    printer
        .send(Print(format!(
            "Removed 'prices' from IPC: {}",
            hidden.map_or_else(|| "not found".to_string(), |h| h.id().to_string())
        )))
        .await;
    printer
        .send(Print(format!(
            "Exposed agents remaining: {}",
            runtime.ipc_agent_count()
        )))
        .await;
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() {
    let mut runtime = ActonApp::launch();

    // Create printer agent for coordinated console output
    let printer = create_printer_agent(&mut runtime).await;
    printer
        .send(Print("=== IPC Serialization Foundation Example ===\n".to_string()))
        .await;

    // Register IPC message types
    let registry = runtime.ipc_registry();
    registry.register::<PriceUpdate>("PriceUpdate");
    registry.register::<GetPrice>("GetPrice");
    registry.register::<PriceResponse>("PriceResponse");

    printer
        .send(Print(format!("Registered {} IPC message types:", registry.len())))
        .await;
    for type_name in registry.type_names() {
        printer.send(Print(format!("  - {type_name}"))).await;
    }

    // Create and expose price service
    let price_handle = create_price_service(&mut runtime, &printer).await;
    runtime.ipc_expose("prices", price_handle.clone());

    printer
        .send(Print(format!("\nExposed {} agent(s) for IPC:", runtime.ipc_agent_count())))
        .await;
    printer
        .send(Print(format!("  - 'prices' -> {}", price_handle.id())))
        .await;

    // Simulate IPC message flow
    printer.send(PrintSection("Simulating IPC Message Flow".to_string())).await;
    let incoming_json = r#"{
        "correlation_id": "req_001",
        "target": "prices",
        "message_type": "PriceUpdate",
        "payload": { "symbol": "AAPL", "price": 178.25, "timestamp": 1700000000 }
    }"#;
    process_ipc_envelope(&runtime, &registry, &printer, incoming_json).await;

    // Send more updates
    printer.send(PrintSection("Sending More Updates".to_string())).await;
    for (symbol, price, ts) in [("GOOGL", 141.80, 1_700_000_001), ("MSFT", 378.91, 1_700_000_002), ("AAPL", 179.50, 1_700_000_003)] {
        price_handle
            .send(PriceUpdate { symbol: symbol.to_string(), price, timestamp: ts })
            .await;
    }

    // Demonstrate envelope creation
    printer.send(PrintSection("Creating IpcEnvelope Programmatically".to_string())).await;
    demonstrate_envelope_creation(&printer).await;

    // Demonstrate error handling
    printer.send(PrintSection("Error Handling Examples".to_string())).await;
    demonstrate_error_handling(&runtime, &registry, &printer).await;

    // Demonstrate hiding agents
    printer.send(PrintSection("Hiding Agent from IPC".to_string())).await;
    demonstrate_ipc_hiding(&runtime, &printer).await;

    // Shutdown - actor framework ensures all messages are processed
    runtime.shutdown_all().await.expect("Failed to shut down system");
    println!("\n=== Example Complete ===");
}
