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

//! IPC Bidirectional Communication Example
//!
//! This example demonstrates the request-response pattern in acton-reactive IPC,
//! where external clients can send queries and receive responses from agents.
//!
//! # Features
//!
//! - **Request-Response Pattern**: External process sends a query, agent processes it,
//!   and returns a response via the IPC proxy channel.
//! - **Calculator Service**: A simple arithmetic service that demonstrates
//!   synchronous response generation.
//! - **Key-Value Store**: A stateful service demonstrating queries that may or may not
//!   have results.
//!
//! # How It Works
//!
//! 1. IPC client sends `IpcEnvelope` with `expects_reply: true`
//! 2. IPC listener creates a temporary MPSC channel (proxy)
//! 3. Message is sent to agent with proxy as `reply_to` address
//! 4. Agent handler uses `envelope.reply_envelope().send(response)` to reply
//! 5. Listener receives response on proxy channel and serializes it back to client
//!
//! # Running This Example
//!
//! Start the server:
//! ```bash
//! cargo run --example ipc_bidirectional_server --features ipc
//! ```
//!
//! Then connect with the client (in another terminal):
//! ```bash
//! cargo run --example ipc_bidirectional_client --features ipc
//! ```
//!
//! See the README.md in this directory for more details.

use std::collections::HashMap;
use std::time::Duration;

use acton_macro::acton_actor;
use acton_reactive::ipc::{socket_exists, IpcConfig};
use acton_reactive::prelude::*;
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

// ============================================================================
// Message Definitions - Calculator Service
// ============================================================================

/// Request to add two numbers.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct AddRequest {
    a: i64,
    b: i64,
}

/// Request to multiply two numbers.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct MultiplyRequest {
    a: i64,
    b: i64,
}

/// Response containing a calculation result.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CalculationResult {
    result: i64,
    operation: String,
}

// ============================================================================
// Message Definitions - Key-Value Store
// ============================================================================

/// Request to set a key-value pair.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SetValue {
    key: String,
    value: String,
}

/// Request to get a value by key.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct GetValue {
    key: String,
}

/// Response containing the retrieved value (or None if not found).
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ValueResponse {
    key: String,
    value: Option<String>,
    found: bool,
}

/// Acknowledgment response for set operations.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SetAcknowledgment {
    key: String,
    success: bool,
}

// ============================================================================
// Agent States
// ============================================================================

/// Calculator service - stateless arithmetic operations.
#[acton_actor]
struct CalculatorState {
    operations_performed: usize,
}

/// Key-Value store service - stateful storage.
#[acton_actor]
struct KeyValueState {
    store: HashMap<String, String>,
}

// ============================================================================
// Agent Creation Functions
// ============================================================================

/// Creates the calculator service agent that responds to arithmetic queries.
async fn create_calculator_agent(runtime: &mut AgentRuntime) -> AgentHandle {
    let mut calculator = runtime.new_agent_with_name::<CalculatorState>("calculator".to_string());

    // Handle addition requests and reply with result
    calculator.mutate_on::<AddRequest>(|agent, envelope| {
        let msg = envelope.message();
        let result = msg.a + msg.b;
        agent.model.operations_performed += 1;

        let response = CalculationResult {
            result,
            operation: format!("{} + {}", msg.a, msg.b),
        };

        println!(
            "  [Calculator] Add: {} + {} = {} (op #{})",
            msg.a, msg.b, result, agent.model.operations_performed
        );

        // Send the response back to the IPC client via reply_envelope
        let reply_envelope = envelope.reply_envelope();
        Box::pin(async move {
            reply_envelope.send(response).await;
        })
    });

    // Handle multiplication requests and reply with result
    calculator.mutate_on::<MultiplyRequest>(|agent, envelope| {
        let msg = envelope.message();
        let result = msg.a * msg.b;
        agent.model.operations_performed += 1;

        let response = CalculationResult {
            result,
            operation: format!("{} × {}", msg.a, msg.b),
        };

        println!(
            "  [Calculator] Multiply: {} × {} = {} (op #{})",
            msg.a, msg.b, result, agent.model.operations_performed
        );

        // Send the response back to the IPC client
        let reply_envelope = envelope.reply_envelope();
        Box::pin(async move {
            reply_envelope.send(response).await;
        })
    });

    calculator.start().await
}

/// Creates the key-value store agent that responds to get/set queries.
async fn create_kv_store_agent(runtime: &mut AgentRuntime) -> AgentHandle {
    let mut kv_store = runtime.new_agent_with_name::<KeyValueState>("kv_store".to_string());

    // Handle set requests - stores value and acknowledges
    kv_store.mutate_on::<SetValue>(|agent, envelope| {
        let msg = envelope.message();
        let key = msg.key.clone();
        let value = msg.value.clone();

        agent.model.store.insert(key.clone(), value.clone());

        let response = SetAcknowledgment {
            key: key.clone(),
            success: true,
        };

        println!("  [KV Store] Set: {} = \"{}\"", key, value);

        let reply_envelope = envelope.reply_envelope();
        Box::pin(async move {
            reply_envelope.send(response).await;
        })
    });

    // Handle get requests - retrieves value and responds
    kv_store.mutate_on::<GetValue>(|agent, envelope| {
        let msg = envelope.message();
        let key = msg.key.clone();
        let value = agent.model.store.get(&key).cloned();

        let response = ValueResponse {
            key: key.clone(),
            value: value.clone(),
            found: value.is_some(),
        };

        println!(
            "  [KV Store] Get: {} = {:?}",
            key,
            value.as_deref().unwrap_or("(not found)")
        );

        let reply_envelope = envelope.reply_envelope();
        Box::pin(async move {
            reply_envelope.send(response).await;
        })
    });

    kv_store.start().await
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

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║     IPC Bidirectional Communication Example (Server)        ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    let mut runtime = ActonApp::launch();

    // Register all IPC message types (both requests and responses)
    let registry = runtime.ipc_registry();

    // Calculator messages
    registry.register::<AddRequest>("AddRequest");
    registry.register::<MultiplyRequest>("MultiplyRequest");
    registry.register::<CalculationResult>("CalculationResult");

    // Key-Value store messages
    registry.register::<SetValue>("SetValue");
    registry.register::<GetValue>("GetValue");
    registry.register::<ValueResponse>("ValueResponse");
    registry.register::<SetAcknowledgment>("SetAcknowledgment");

    println!("📝 Registered {} IPC message types", registry.len());

    // Create service agents
    let calculator = create_calculator_agent(&mut runtime).await;
    println!("🧮 Calculator service started");

    let kv_store = create_kv_store_agent(&mut runtime).await;
    println!("📦 Key-Value store service started");

    // Expose agents for IPC access
    runtime.ipc_expose("calculator", calculator.clone());
    runtime.ipc_expose("kv_store", kv_store.clone());
    println!("🔗 Exposed agents: calculator, kv_store");

    // Start the IPC listener
    let ipc_config = IpcConfig::load();
    let socket_path = ipc_config.socket_path();

    let listener_handle = runtime.start_ipc_listener().await?;
    println!("🚀 IPC listener started");

    // Verify socket is ready
    tokio::time::sleep(Duration::from_millis(50)).await;
    if socket_exists(&socket_path) {
        println!("📡 Socket ready: {}", socket_path.display());
    }

    println!();
    println!("════════════════════════════════════════════════════════════════");
    println!("  Server is ready for bidirectional IPC communication!");
    println!("  Run the client example in another terminal:");
    println!("  cargo run --example ipc_bidirectional_client --features ipc");
    println!("════════════════════════════════════════════════════════════════");
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
