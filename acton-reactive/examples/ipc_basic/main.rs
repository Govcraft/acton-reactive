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

//! IPC (Inter-Process Communication) Example
//!
//! This example demonstrates the complete IPC infrastructure for acton-reactive:
//!
//! 1. **Phase 1 - Serialization Foundation**: Type registration, message envelopes,
//!    and actor exposure via logical names.
//!
//! 2. **Phase 2 - UDS Listener**: Unix Domain Socket communication with external
//!    processes using length-prefixed wire protocol.
//!
//! # Key Concepts Demonstrated
//!
//! - **Type Registration**: Registering message types with `IpcTypeRegistry`
//! - **Actor Exposure**: Exposing actors via logical names for IPC routing
//! - **UDS Listener**: Starting the IPC listener with `runtime.start_ipc_listener()`
//! - **Wire Protocol**: Using `protocol::write_envelope` and `protocol::read_response`
//!   for client-side communication
//! - **Socket Utilities**: Using `socket_exists` and `socket_is_alive` for
//!   socket state checking
//!
//! # Running This Example
//!
//! ```bash
//! cargo run --example ipc_basic --features ipc
//! ```

use std::sync::Arc;

use acton_macro::{acton_actor, acton_message};
use acton_reactive::ipc::protocol::{read_response, write_envelope};
use acton_reactive::prelude::*;
use tokio::net::UnixStream;

// ============================================================================
// Message Definitions
// ============================================================================

/// A price update message that can be sent via IPC.
#[acton_message(ipc)]
struct PriceUpdate {
    symbol: String,
    price: f64,
    timestamp: u64,
}

/// A request to get the current price of a symbol.
#[acton_message(ipc)]
struct GetPrice {
    symbol: String,
}

/// Response containing the current price.
#[acton_message(ipc)]
struct PriceResponse {
    symbol: String,
    price: f64,
    found: bool,
}

/// A message to print text to the console.
#[acton_message]
struct Print(String);

/// A message to print a section header.
#[acton_message]
struct PrintSection(String);

/// Message to initialize the price service with a printer handle.
#[acton_message]
struct InitPrinter(ActorHandle);

// ============================================================================
// Actor States
// ============================================================================

/// State for the console printer actor.
/// All console output goes through this actor to ensure proper ordering.
#[acton_actor]
struct PrinterState;

/// State for our price tracking actor.
#[acton_actor]
struct PriceServiceState {
    prices: std::collections::HashMap<String, f64>,
    update_count: usize,
    printer: Option<ActorHandle>,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Creates and starts the printer actor for coordinated console output.
async fn create_printer_actor(runtime: &mut ActorRuntime) -> ActorHandle {
    let mut printer_actor = runtime.new_actor::<PrinterState>();
    printer_actor
        .act_on::<Print>(|_actor, envelope| {
            println!("{}", envelope.message().0);
            ActorReply::immediate()
        })
        .act_on::<PrintSection>(|_actor, envelope| {
            println!("\n--- {} ---\n", envelope.message().0);
            ActorReply::immediate()
        });
    printer_actor.start().await
}

/// Creates and starts the price service actor.
async fn create_price_service(runtime: &mut ActorRuntime, printer: &ActorHandle) -> ActorHandle {
    let mut price_service =
        runtime.new_actor_with_name::<PriceServiceState>("price_service".to_string());

    price_service
        .mutate_on::<InitPrinter>(|actor, envelope| {
            actor.model.printer = Some(envelope.message().0.clone());
            ActorReply::immediate()
        })
        .mutate_on::<PriceUpdate>(|actor, envelope| {
            let msg = envelope.message().clone();
            let printer = actor.model.printer.clone();
            actor.model.prices.insert(msg.symbol.clone(), msg.price);
            actor.model.update_count += 1;

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
        .mutate_on::<GetPrice>(|actor, envelope| {
            let msg = envelope.message().clone();
            let printer = actor.model.printer.clone();
            let price = actor.model.prices.get(&msg.symbol).copied();
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
        .after_stop(|actor| {
            let printer = actor.model.printer.clone();
            let update_count = actor.model.update_count;
            let prices = actor.model.prices.clone();

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

/// Processes an incoming IPC envelope and routes it to the target actor.
async fn process_ipc_envelope(
    runtime: &ActorRuntime,
    registry: &Arc<IpcTypeRegistry>,
    printer: &ActorHandle,
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
            .send(Print(format!("Found target actor: {}", target_handle.id())))
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
            &IpcError::ActorNotFound(envelope.target.clone()),
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
async fn demonstrate_envelope_creation(printer: &ActorHandle) {
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
    runtime: &ActorRuntime,
    registry: &Arc<IpcTypeRegistry>,
    printer: &ActorHandle,
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

    // Actor not found
    if runtime.ipc_lookup("nonexistent").is_none() {
        let err = IpcError::ActorNotFound("nonexistent".to_string());
        printer
            .send(Print(format!("\nError looking up actor: {err}")))
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

/// Demonstrates the UDS listener and client-side protocol functions.
async fn demonstrate_uds_communication(
    runtime: &ActorRuntime,
    printer: &ActorHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    use acton_reactive::ipc::{socket_exists, socket_is_alive};

    // Create a custom IPC config with a unique socket path for this example
    let config = IpcConfig::load();
    let socket_path = config.socket_path();

    printer
        .send(Print(format!("Socket path: {}", socket_path.display())))
        .await;

    // Check socket state before starting
    printer
        .send(Print(format!(
            "Socket exists before start: {}",
            socket_exists(&socket_path)
        )))
        .await;

    // Start the IPC listener
    let listener_handle = runtime.start_ipc_listener_with_config(config).await?;

    printer
        .send(Print("IPC listener started successfully!".to_string()))
        .await;

    // Small delay to ensure listener is ready
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Check socket state after starting
    printer
        .send(Print(format!(
            "Socket exists after start: {}",
            socket_exists(&socket_path)
        )))
        .await;
    printer
        .send(Print(format!(
            "Socket is alive: {}",
            socket_is_alive(&socket_path).await
        )))
        .await;

    // Connect as a client and send a message
    printer
        .send(Print("\nConnecting as IPC client...".to_string()))
        .await;

    let stream = UnixStream::connect(&socket_path).await?;
    let (mut reader, mut writer) = stream.into_split();

    // Create and send an envelope using the wire protocol
    let envelope = IpcEnvelope::new(
        "prices",
        "PriceUpdate",
        serde_json::json!({
            "symbol": "NVDA",
            "price": 875.50,
            "timestamp": 1_700_000_010
        }),
    );

    printer
        .send(Print(format!(
            "Sending envelope via UDS: correlation_id={}",
            envelope.correlation_id
        )))
        .await;

    // Use the wire protocol to send the envelope
    write_envelope(&mut writer, &envelope).await?;

    // Read the response using the wire protocol
    let response = read_response(&mut reader, 1024 * 1024).await?;

    printer
        .send(Print(format!(
            "Received response: success={}, correlation_id={}",
            response.success, response.correlation_id
        )))
        .await;
    printer
        .send(Print(format!(
            "Response payload: {}",
            serde_json::to_string_pretty(&response.payload).unwrap_or_default()
        )))
        .await;

    // Show listener statistics
    let stats = &listener_handle.stats;
    printer
        .send(Print(format!(
            "\nListener stats:\n  Connections accepted: {}\n  Messages received: {}\n  Messages routed: {}",
            stats.connections_accepted(),
            stats.messages_received(),
            stats.messages_routed()
        )))
        .await;

    // Stop the listener
    listener_handle.stop();
    printer
        .send(Print("IPC listener stopped.".to_string()))
        .await;

    // Small delay for cleanup
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Verify socket is cleaned up
    printer
        .send(Print(format!(
            "Socket exists after stop: {}",
            socket_exists(&socket_path)
        )))
        .await;

    Ok(())
}

/// Demonstrates hiding an actor from IPC access.
async fn demonstrate_ipc_hiding(runtime: &ActorRuntime, printer: &ActorHandle) {
    let hidden = runtime.ipc_hide("prices");
    printer
        .send(Print(format!(
            "Removed 'prices' from IPC: {}",
            hidden.map_or_else(|| "not found".to_string(), |h| h.id().to_string())
        )))
        .await;
    printer
        .send(Print(format!(
            "Exposed actors remaining: {}",
            runtime.ipc_actor_count()
        )))
        .await;
}

// ============================================================================
// Main
// ============================================================================

#[acton_main]
async fn main() {
    let mut runtime = ActonApp::launch();

    // Create printer actor for coordinated console output
    let printer = create_printer_actor(&mut runtime).await;
    printer
        .send(Print(
            "=== IPC Example (Phase 1 + Phase 2) ===\n".to_string(),
        ))
        .await;

    // Register IPC message types
    let registry = runtime.ipc_registry();
    registry.register::<PriceUpdate>("PriceUpdate");
    registry.register::<GetPrice>("GetPrice");
    registry.register::<PriceResponse>("PriceResponse");

    printer
        .send(Print(format!(
            "Registered {} IPC message types:",
            registry.len()
        )))
        .await;
    for type_name in registry.type_names() {
        printer.send(Print(format!("  - {type_name}"))).await;
    }

    // Create and expose price service
    let price_handle = create_price_service(&mut runtime, &printer).await;
    runtime.ipc_expose("prices", price_handle.clone());

    printer
        .send(Print(format!(
            "\nExposed {} actor(s) for IPC:",
            runtime.ipc_actor_count()
        )))
        .await;
    printer
        .send(Print(format!("  - 'prices' -> {}", price_handle.id())))
        .await;

    // Phase 1: Simulate IPC message flow (in-process)
    printer
        .send(PrintSection(
            "Phase 1: In-Process IPC Simulation".to_string(),
        ))
        .await;
    let incoming_json = r#"{
        "correlation_id": "req_001",
        "target": "prices",
        "message_type": "PriceUpdate",
        "payload": { "symbol": "AAPL", "price": 178.25, "timestamp": 1700000000 }
    }"#;
    process_ipc_envelope(&runtime, &registry, &printer, incoming_json).await;

    // Send more updates directly
    printer
        .send(PrintSection("Direct Actor Updates".to_string()))
        .await;
    for (symbol, price, ts) in [
        ("GOOGL", 141.80, 1_700_000_001),
        ("MSFT", 378.91, 1_700_000_002),
        ("AAPL", 179.50, 1_700_000_003),
    ] {
        price_handle
            .send(PriceUpdate {
                symbol: symbol.to_string(),
                price,
                timestamp: ts,
            })
            .await;
    }

    // Demonstrate envelope creation
    printer
        .send(PrintSection(
            "Creating IpcEnvelope Programmatically".to_string(),
        ))
        .await;
    demonstrate_envelope_creation(&printer).await;

    // Demonstrate error handling
    printer
        .send(PrintSection("Error Handling Examples".to_string()))
        .await;
    demonstrate_error_handling(&runtime, &registry, &printer).await;

    // Phase 2: UDS listener demonstration
    printer
        .send(PrintSection(
            "Phase 2: Unix Domain Socket Communication".to_string(),
        ))
        .await;
    if let Err(e) = demonstrate_uds_communication(&runtime, &printer).await {
        printer
            .send(Print(format!("UDS demonstration error: {e}")))
            .await;
    }

    // Allow time for async message processing
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Demonstrate hiding actors
    printer
        .send(PrintSection("Hiding Actor from IPC".to_string()))
        .await;
    demonstrate_ipc_hiding(&runtime, &printer).await;

    // Shutdown - actor framework ensures all messages are processed
    runtime
        .shutdown_all()
        .await
        .expect("Failed to shut down system");
    println!("\n=== Example Complete ===");
}
