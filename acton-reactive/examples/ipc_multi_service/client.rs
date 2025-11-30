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

//! IPC Client Example
//!
//! This example demonstrates how to build a standalone IPC client that connects
//! to an acton-reactive IPC server from an external process. It showcases:
//!
//! - Connecting to a Unix Domain Socket
//! - Sending messages using the wire protocol
//! - Receiving and handling responses
//! - Heartbeat/ping for connection health checks
//! - Error handling for various failure scenarios
//!
//! # Running This Example
//!
//! First, start the IPC server:
//! ```bash
//! cargo run --example ipc_multi_service_server --features ipc
//! ```
//!
//! Then, in another terminal, run this client:
//! ```bash
//! cargo run --example ipc_multi_service_client --features ipc
//! ```
//!
//! By default, this client connects to the `ipc_multi_service_server` server.
//! You can specify a different server name or custom socket path:
//! ```bash
//! # Connect to a different server by name
//! cargo run --example ipc_multi_service_client --features ipc -- --server ipc_basic
//!
//! # Or specify a full socket path
//! cargo run --example ipc_multi_service_client --features ipc -- /path/to/socket.sock
//! ```
//!
//! See the README.md in this directory for more details.

use std::path::{Path, PathBuf};
use std::time::Duration;

use acton_reactive::ipc::protocol::{
    read_frame, read_response, write_envelope, write_heartbeat, is_heartbeat, MAX_FRAME_SIZE,
};
use acton_reactive::ipc::{socket_exists, socket_is_alive, IpcConfig, IpcEnvelope};
use tokio::net::UnixStream;
use tokio::time::timeout;

/// Displays connection status information.
async fn display_connection_status(socket_path: &Path) {
    println!("Socket path: {}", socket_path.display());
    println!("Socket exists: {}", socket_exists(socket_path));
    println!("Socket is alive: {}", socket_is_alive(socket_path).await);
}

/// Sends a heartbeat and waits for response.
async fn send_heartbeat(
    reader: &mut tokio::net::unix::OwnedReadHalf,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n--- Heartbeat Test ---");
    write_heartbeat(writer).await?;
    println!("Sent heartbeat...");

    let (msg_type, _format, _payload) = timeout(Duration::from_secs(5), read_frame(reader, MAX_FRAME_SIZE)).await??;

    if is_heartbeat(msg_type) {
        println!("Received heartbeat response - connection is healthy!");
    } else {
        println!("Unexpected response type: {msg_type:#04x}");
    }

    Ok(())
}

/// Sends a message to a target agent and displays the response.
async fn send_message(
    reader: &mut tokio::net::unix::OwnedReadHalf,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    target: &str,
    message_type: &str,
    payload: serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let envelope = IpcEnvelope::new(target, message_type, payload.clone());

    println!("\n--- Sending Message ---");
    println!("Target: {target}");
    println!("Type: {message_type}");
    println!("Correlation ID: {}", envelope.correlation_id);
    println!("Payload: {}", serde_json::to_string_pretty(&payload)?);

    write_envelope(writer, &envelope).await?;

    let response = timeout(Duration::from_secs(10), read_response(reader, MAX_FRAME_SIZE)).await??;

    println!("\n--- Response ---");
    println!("Success: {}", response.success);
    println!("Correlation ID: {}", response.correlation_id);
    if let Some(err) = &response.error {
        println!("Error: {err}");
    }
    if let Some(payload) = &response.payload {
        println!("Payload: {}", serde_json::to_string_pretty(payload)?);
    }

    Ok(())
}

/// Demonstrates error handling by sending to a non-existent agent.
async fn demonstrate_error_handling(
    reader: &mut tokio::net::unix::OwnedReadHalf,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== Error Handling Demonstration ===");

    // Try to send to a non-existent agent
    send_message(
        reader,
        writer,
        "nonexistent_agent",
        "SomeMessage",
        serde_json::json!({"test": "data"}),
    )
    .await?;

    Ok(())
}

/// Default server name to connect to.
const DEFAULT_SERVER: &str = "ipc_multi_service_server";

/// Resolves the socket path from command line arguments.
///
/// Supports:
/// - `--server <name>` to connect to a named server
/// - A direct path to a socket file
/// - Defaults to `ipc_multi_service_server` if no arguments
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

    // Default: connect to ipc_multi_service_server
    let mut config = IpcConfig::load();
    config.socket.app_name = Some(DEFAULT_SERVER.to_string());
    config.socket_path()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== IPC Client Example ===\n");

    // Get socket path from command line or default to ipc_multi_service_server
    let socket_path = resolve_socket_path();

    // Display connection status
    display_connection_status(&socket_path).await;

    // Check if socket is available
    if !socket_exists(&socket_path) {
        eprintln!("\nError: Socket does not exist at {}", socket_path.display());
        eprintln!("Make sure an IPC server is running (e.g., ipc_multi_service_server example).");
        std::process::exit(1);
    }

    if !socket_is_alive(&socket_path).await {
        eprintln!("\nError: Socket exists but is not responding at {}", socket_path.display());
        eprintln!("The IPC server may have crashed. Try removing the stale socket and restarting.");
        std::process::exit(1);
    }

    // Connect to the socket
    println!("\nConnecting to IPC server...");
    let stream = UnixStream::connect(&socket_path).await?;
    println!("Connected successfully!");

    let (mut reader, mut writer) = stream.into_split();

    // Test heartbeat
    send_heartbeat(&mut reader, &mut writer).await?;

    // Send messages to different services (assuming ipc_multi_service_server is running)
    println!("\n=== Sending Messages to Services ===");

    // Counter increment
    send_message(
        &mut reader,
        &mut writer,
        "counter",
        "Increment",
        serde_json::json!({ "amount": 5 }),
    )
    .await?;

    // Another counter increment
    send_message(
        &mut reader,
        &mut writer,
        "counter",
        "Increment",
        serde_json::json!({ "amount": 10 }),
    )
    .await?;

    // Logger message
    send_message(
        &mut reader,
        &mut writer,
        "logger",
        "LogMessage",
        serde_json::json!({
            "level": "INFO",
            "message": "Hello from external IPC client!"
        }),
    )
    .await?;

    // Config update
    send_message(
        &mut reader,
        &mut writer,
        "config",
        "SetConfig",
        serde_json::json!({
            "key": "app.theme",
            "value": "dark"
        }),
    )
    .await?;

    // Demonstrate error handling
    demonstrate_error_handling(&mut reader, &mut writer).await?;

    println!("\n=== IPC Client Example Complete ===");

    Ok(())
}
