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

//! IPC Bidirectional Client Example
//!
//! This example demonstrates the client side of bidirectional IPC communication,
//! sending request-response messages to actors and receiving their replies.
//!
//! # Features
//!
//! - **Request-Response**: Sends messages with `expects_reply: true` and waits
//!   for actor responses.
//! - **Fire-and-Forget**: Shows contrast with regular one-way messages.
//! - **Custom Timeouts**: Demonstrates configurable response timeouts.
//!
//! # Running This Example
//!
//! First, start the server:
//! ```bash
//! cargo run --example ipc_bidirectional_server --features ipc
//! ```
//!
//! Then, in another terminal, run this client:
//! ```bash
//! cargo run --example ipc_bidirectional_client --features ipc
//! ```
//!
//! See the README.md in this directory for more details.

use std::path::PathBuf;
use std::time::Duration;

use acton_reactive::ipc::protocol::{read_response, write_envelope, MAX_FRAME_SIZE};
use acton_reactive::ipc::{socket_exists, socket_is_alive, IpcConfig, IpcEnvelope, IpcResponse};
use acton_reactive::prelude::acton_main;
use tokio::net::UnixStream;
use tokio::time::timeout;

/// Default server name to connect to.
const DEFAULT_SERVER: &str = "ipc_bidirectional_server";

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

    // Check for direct path argument (not starting with --)
    if let Some(arg) = args.get(1) {
        if !arg.starts_with("--") {
            return PathBuf::from(arg);
        }
    }

    // Default: connect to ipc_bidirectional server
    let mut config = IpcConfig::load();
    config.socket.app_name = Some(DEFAULT_SERVER.to_string());
    config.socket_path()
}

/// Sends a request and waits for a response.
async fn send_request(
    reader: &mut tokio::net::unix::OwnedReadHalf,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    target: &str,
    message_type: &str,
    payload: serde_json::Value,
    timeout_ms: u64,
) -> Result<IpcResponse, Box<dyn std::error::Error>> {
    // Create envelope with expects_reply = true
    let envelope = IpcEnvelope::new_request_with_timeout(target, message_type, payload, timeout_ms);

    println!("📤 Request to {target}::{message_type}");
    println!("   correlation_id: {}", envelope.correlation_id);
    println!("   expects_reply: {}", envelope.expects_reply);
    println!("   timeout: {}ms", envelope.response_timeout_ms);

    write_envelope(writer, &envelope).await?;

    let response = timeout(
        Duration::from_millis(timeout_ms + 1000), // Add buffer for network
        read_response(reader, MAX_FRAME_SIZE),
    )
    .await??;

    Ok(response)
}

/// Displays a response in a formatted way.
fn display_response(response: &IpcResponse) {
    if response.success {
        println!("📥 Response (success):");
        if let Some(payload) = &response.payload {
            // Try to pretty-print the payload
            if let Some(obj) = payload.as_object() {
                for (key, value) in obj {
                    println!("   {key}: {value}");
                }
            } else {
                println!(
                    "   {}",
                    serde_json::to_string_pretty(payload).unwrap_or_default()
                );
            }
        }
    } else {
        println!("📥 Response (error):");
        println!("   error_code: {:?}", response.error_code);
        println!("   error: {:?}", response.error);
    }
}

/// Demonstrates calculator operations with request-response.
async fn demo_calculator(
    reader: &mut tokio::net::unix::OwnedReadHalf,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║                    Calculator Service Demo                    ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Test: Add two numbers
    println!("🧮 Test 1: Addition (5 + 3)");
    let response = send_request(
        reader,
        writer,
        "calculator",
        "AddRequest",
        serde_json::json!({ "a": 5, "b": 3 }),
        5000,
    )
    .await?;
    display_response(&response);

    println!();

    // Test: Multiply two numbers
    println!("🧮 Test 2: Multiplication (7 × 6)");
    let response = send_request(
        reader,
        writer,
        "calculator",
        "MultiplyRequest",
        serde_json::json!({ "a": 7, "b": 6 }),
        5000,
    )
    .await?;
    display_response(&response);

    println!();

    // Test: More complex calculation
    println!("🧮 Test 3: Large numbers (1000000 + 2000000)");
    let response = send_request(
        reader,
        writer,
        "calculator",
        "AddRequest",
        serde_json::json!({ "a": 1_000_000, "b": 2_000_000 }),
        5000,
    )
    .await?;
    display_response(&response);

    Ok(())
}

/// Demonstrates key-value store operations with request-response.
async fn demo_kv_store(
    reader: &mut tokio::net::unix::OwnedReadHalf,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║                   Key-Value Store Demo                        ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Test: Set a value
    println!("📦 Test 1: Set key 'username' = 'alice'");
    let response = send_request(
        reader,
        writer,
        "kv_store",
        "SetValue",
        serde_json::json!({ "key": "username", "value": "alice" }),
        5000,
    )
    .await?;
    display_response(&response);

    println!();

    // Test: Get the value back
    println!("📦 Test 2: Get key 'username'");
    let response = send_request(
        reader,
        writer,
        "kv_store",
        "GetValue",
        serde_json::json!({ "key": "username" }),
        5000,
    )
    .await?;
    display_response(&response);

    println!();

    // Test: Get a non-existent key
    println!("📦 Test 3: Get non-existent key 'nonexistent'");
    let response = send_request(
        reader,
        writer,
        "kv_store",
        "GetValue",
        serde_json::json!({ "key": "nonexistent" }),
        5000,
    )
    .await?;
    display_response(&response);

    println!();

    // Test: Set another value
    println!("📦 Test 4: Set key 'email' = 'alice@example.com'");
    let response = send_request(
        reader,
        writer,
        "kv_store",
        "SetValue",
        serde_json::json!({ "key": "email", "value": "alice@example.com" }),
        5000,
    )
    .await?;
    display_response(&response);

    println!();

    // Test: Get the email back
    println!("📦 Test 5: Get key 'email'");
    let response = send_request(
        reader,
        writer,
        "kv_store",
        "GetValue",
        serde_json::json!({ "key": "email" }),
        5000,
    )
    .await?;
    display_response(&response);

    Ok(())
}

/// Demonstrates error handling scenarios.
async fn demo_error_handling(
    reader: &mut tokio::net::unix::OwnedReadHalf,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║                  Error Handling Demo                          ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Test: Send to non-existent actor
    println!("❌ Test: Send to non-existent actor 'nonexistent'");
    let response = send_request(
        reader,
        writer,
        "nonexistent",
        "SomeMessage",
        serde_json::json!({ "test": "data" }),
        5000,
    )
    .await?;
    display_response(&response);

    Ok(())
}

#[acton_main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║    IPC Bidirectional Communication Example (Client)          ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    // Resolve socket path
    let socket_path = resolve_socket_path();
    println!("🔌 Socket path: {}", socket_path.display());

    // Check if socket is available
    if !socket_exists(&socket_path) {
        eprintln!(
            "\n❌ Error: Socket does not exist at {}",
            socket_path.display()
        );
        eprintln!("   Make sure the ipc_bidirectional server is running:");
        eprintln!("   cargo run --example ipc_bidirectional_server --features ipc");
        std::process::exit(1);
    }

    if !socket_is_alive(&socket_path).await {
        eprintln!("\n❌ Error: Socket exists but is not responding");
        eprintln!("   The server may have crashed. Try restarting it.");
        std::process::exit(1);
    }

    // Connect to the socket
    println!("🔗 Connecting to server...");
    let stream = UnixStream::connect(&socket_path).await?;
    println!("✅ Connected successfully!");

    let (mut reader, mut writer) = stream.into_split();

    // Run demonstrations
    demo_calculator(&mut reader, &mut writer).await?;
    demo_kv_store(&mut reader, &mut writer).await?;
    demo_error_handling(&mut reader, &mut writer).await?;

    println!("\n════════════════════════════════════════════════════════════════");
    println!("  All bidirectional IPC tests completed!");
    println!("════════════════════════════════════════════════════════════════");

    Ok(())
}
