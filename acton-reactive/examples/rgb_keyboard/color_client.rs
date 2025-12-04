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

//! RGB Keyboard Example - Color Client
//!
//! This client handles one color component (R, G, or B). Run three instances,
//! one for each component.
//!
//! # Features
//!
//! - **Subscribes to `ColorRequest`**: Receives broadcast color requests from server
//! - **Generates Random Values**: Creates random 0-255 value for its component
//! - **Sends `ColorResponse`**: Returns the value to the server via request-response
//!
//! # Running This Example
//!
//! Run three instances, one for each color component:
//! ```bash
//! cargo run --example rgb_keyboard_color_client --features ipc -- --component R
//! cargo run --example rgb_keyboard_color_client --features ipc -- --component G
//! cargo run --example rgb_keyboard_color_client --features ipc -- --component B
//! ```

use std::path::PathBuf;
use std::time::Duration;

use acton_reactive::ipc::protocol::{
    read_frame, write_envelope, write_frame, Format, MAX_FRAME_SIZE, MSG_TYPE_PUSH,
    MSG_TYPE_RESPONSE, MSG_TYPE_SUBSCRIBE,
};
use acton_reactive::ipc::{
    socket_exists, socket_is_alive, IpcConfig, IpcEnvelope, IpcPushNotification,
    IpcSubscribeRequest, IpcSubscriptionResponse,
};
use acton_reactive::prelude::acton_main;
use rand::Rng;
use serde::{Deserialize, Serialize};
use tokio::net::UnixStream;
use tokio::time::timeout;

/// Default server name to connect to.
const DEFAULT_SERVER: &str = "rgb_keyboard_server";

// ============================================================================
// Message Types (must match server definitions)
// ============================================================================

/// Request sent to color clients to generate a color component.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ColorRequest {
    correlation_id: String,
    character: char,
}

/// Response from a color client with their component value.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ColorResponse {
    correlation_id: String,
    component: String,
    value: u8,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Parses command line arguments for the color component.
fn parse_component() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();

    if let Some(pos) = args.iter().position(|a| a == "--component") {
        if let Some(component) = args.get(pos + 1) {
            let c = component.to_uppercase();
            if c == "R" || c == "G" || c == "B" {
                return Some(c);
            }
        }
    }

    None
}

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

    // Default: connect to rgb_keyboard server
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
    write_frame(writer, MSG_TYPE_SUBSCRIBE, Format::Json, &payload).await?;

    Ok(correlation_id)
}

/// Sends a color response to the server.
async fn send_color_response(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    correlation_id: &str,
    component: &str,
    value: u8,
) -> Result<(), Box<dyn std::error::Error>> {
    let response = ColorResponse {
        correlation_id: correlation_id.to_string(),
        component: component.to_string(),
        value,
    };

    let envelope = IpcEnvelope::new(
        "keystroke_processor",
        "ColorResponse",
        serde_json::to_value(&response)?,
    );

    write_envelope(writer, &envelope).await?;

    Ok(())
}

/// Returns ANSI color code for the component.
fn component_color(component: &str) -> &'static str {
    match component {
        "R" => "\x1b[31m", // Red
        "G" => "\x1b[32m", // Green
        "B" => "\x1b[34m", // Blue
        _ => "\x1b[0m",    // Reset
    }
}

// ============================================================================
// Main
// ============================================================================

#[acton_main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse component from command line
    let Some(component) = parse_component() else {
        eprintln!("Error: Must specify color component with --component R|G|B");
        eprintln!();
        eprintln!("Usage:");
        eprintln!(
            "  cargo run --example rgb_keyboard_color_client --features ipc -- --component R"
        );
        eprintln!(
            "  cargo run --example rgb_keyboard_color_client --features ipc -- --component G"
        );
        eprintln!(
            "  cargo run --example rgb_keyboard_color_client --features ipc -- --component B"
        );
        std::process::exit(1);
    };

    let color = component_color(&component);
    let reset = "\x1b[0m";

    println!("====================================================================");
    println!("       RGB Keyboard - {color}{component}{reset} Color Client");
    println!("====================================================================");
    println!();

    // Resolve socket path
    let socket_path = resolve_socket_path();
    println!("Socket path: {}", socket_path.display());

    // Check if socket is available
    if !socket_exists(&socket_path) {
        eprintln!(
            "\nError: Socket does not exist at {}",
            socket_path.display()
        );
        eprintln!("Make sure the rgb_keyboard server is running:");
        eprintln!("  cargo run --example rgb_keyboard_server --features ipc");
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

    // Subscribe to ColorRequest broadcasts
    println!("\nSubscribing to ColorRequest...");
    let _corr_id = send_subscribe(&mut writer, vec!["ColorRequest".to_string()]).await?;

    // Wait for subscription response
    let (msg_type, _format, payload) =
        timeout(Duration::from_secs(5), read_frame(&mut reader, MAX_FRAME_SIZE)).await??;

    if msg_type == MSG_TYPE_RESPONSE {
        let response: IpcSubscriptionResponse = serde_json::from_slice(&payload)?;
        if response.success {
            println!("  Subscribed to: {:?}", response.subscribed_types);
        } else {
            eprintln!(
                "  Subscription failed: {}",
                response.error.as_deref().unwrap_or("Unknown error")
            );
            std::process::exit(1);
        }
    }

    println!();
    println!("====================================================================");
    println!("  {color}{component}{reset} Color Client is ready!");
    println!("  Waiting for ColorRequest broadcasts...");
    println!("  Press Ctrl+C to quit.");
    println!("====================================================================");
    println!();

    // Create random number generator
    let mut rng = rand::rng();
    let mut requests_handled: u64 = 0;

    // Main loop: receive ColorRequest, respond with random value
    loop {
        match timeout(Duration::from_secs(30), read_frame(&mut reader, MAX_FRAME_SIZE)).await {
            Ok(Ok((msg_type, _format, payload))) => {
                if msg_type == MSG_TYPE_PUSH {
                    let notification: IpcPushNotification = serde_json::from_slice(&payload)?;

                    if notification.message_type == "ColorRequest" {
                        if let Ok(request) =
                            serde_json::from_value::<ColorRequest>(notification.payload)
                        {
                            // Generate random value 0-255
                            let value: u8 = rng.random();
                            requests_handled += 1;

                            let character = request.character;
                            let correlation_id = &request.correlation_id;
                            println!("  {color}[{component}]{reset} '{character}' -> {value} (#{requests_handled}) [{correlation_id}]");

                            // Send response back to server
                            send_color_response(
                                &mut writer,
                                &request.correlation_id,
                                &component,
                                value,
                            )
                            .await?;
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                eprintln!("Error reading frame: {e}");
                break;
            }
            Err(_) => {
                // Timeout - just continue waiting
            }
        }
    }

    println!();
    println!("{color}{component}{reset} Color Client shutting down. Handled {requests_handled} requests.");

    Ok(())
}
