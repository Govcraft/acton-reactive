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

//! IPC Streaming Client Example
//!
//! This example demonstrates the client side of streaming IPC communication,
//! sending a single request and receiving multiple response frames.
//!
//! # Features
//!
//! - **Request-Stream**: Sends messages with `expects_stream: true` and reads
//!   multiple `IpcStreamFrame` responses until the final frame.
//! - **Countdown Demo**: Demonstrates streaming countdown ticks.
//! - **Paginated List Demo**: Shows streaming pages of items.
//!
//! # Running This Example
//!
//! First, start the server:
//! ```bash
//! cargo run --example ipc_streaming_server --features ipc
//! ```
//!
//! Then, in another terminal, run this client:
//! ```bash
//! cargo run --example ipc_streaming_client --features ipc
//! ```

use std::path::PathBuf;
use std::time::Duration;

use acton_reactive::ipc::protocol::{is_stream, read_frame, write_envelope, MAX_FRAME_SIZE};
use acton_reactive::ipc::{socket_exists, socket_is_alive, IpcConfig, IpcEnvelope, IpcStreamFrame};
use tokio::net::UnixStream;
use tokio::time::timeout;

/// Default server name to connect to.
const DEFAULT_SERVER: &str = "ipc_streaming_server";

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

    // Default: connect to ipc_streaming server
    let mut config = IpcConfig::load();
    config.socket.app_name = Some(DEFAULT_SERVER.to_string());
    config.socket_path()
}

/// Sends a stream request and processes all response frames.
async fn send_stream_request(
    reader: &mut tokio::net::unix::OwnedReadHalf,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    target: &str,
    message_type: &str,
    payload: serde_json::Value,
    timeout_ms: u64,
) -> Result<Vec<IpcStreamFrame>, Box<dyn std::error::Error>> {
    // Create envelope with expects_stream = true
    let envelope = IpcEnvelope::new_stream_request_with_timeout(target, message_type, payload, timeout_ms);

    println!("📤 Stream request to {target}::{message_type}");
    println!("   correlation_id: {}", envelope.correlation_id);
    println!("   expects_stream: {}", envelope.expects_stream);
    println!("   timeout: {}ms", envelope.response_timeout_ms);
    println!();

    write_envelope(writer, &envelope).await?;

    // Collect all stream frames
    let mut frames = Vec::new();
    let overall_timeout = Duration::from_millis(timeout_ms + 5000);
    let frame_timeout = Duration::from_millis(timeout_ms.max(1000));

    let start = std::time::Instant::now();

    loop {
        // Check overall timeout
        if start.elapsed() > overall_timeout {
            return Err("Stream timeout: no final frame received".into());
        }

        // Read next frame with timeout
        let result = timeout(frame_timeout, read_frame(reader, MAX_FRAME_SIZE)).await;

        match result {
            Ok(Ok((msg_type, _format, payload))) => {
                if !is_stream(msg_type) {
                    println!("⚠️  Unexpected message type: 0x{msg_type:02X}");
                    continue;
                }

                // Deserialize the stream frame
                let frame: IpcStreamFrame = serde_json::from_slice(&payload)?;

                // Display the frame
                display_stream_frame(&frame);

                let is_final = frame.is_final;
                frames.push(frame);

                if is_final {
                    println!("\n   ✅ Stream complete ({} frames received)", frames.len());
                    break;
                }
            }
            Ok(Err(e)) => {
                return Err(format!("Error reading frame: {e}").into());
            }
            Err(_) => {
                return Err("Frame timeout: no response received".into());
            }
        }
    }

    Ok(frames)
}

/// Displays a single stream frame.
fn display_stream_frame(frame: &IpcStreamFrame) {
    if let Some(error) = &frame.error {
        println!("   📥 Frame #{}: ERROR - {}", frame.sequence, error);
    } else if let Some(payload) = &frame.payload {
        let final_marker = if frame.is_final { " [FINAL]" } else { "" };

        // Try to pretty-print the payload
        if let Some(obj) = payload.as_object() {
            println!("   📥 Frame #{}:{}", frame.sequence, final_marker);
            for (key, value) in obj {
                println!("      {key}: {value}");
            }
        } else {
            println!(
                "   📥 Frame #{}: {}{}",
                frame.sequence,
                serde_json::to_string(payload).unwrap_or_default(),
                final_marker
            );
        }
    } else {
        println!("   📥 Frame #{}: (empty payload)", frame.sequence);
    }
}

/// Demonstrates countdown streaming.
async fn demo_countdown(
    reader: &mut tokio::net::unix::OwnedReadHalf,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║                   Countdown Stream Demo                      ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    // Test: Countdown from 5 with 500ms delays
    println!("⏱️  Test 1: Countdown from 5 (500ms delay)");
    let _frames = send_stream_request(
        reader,
        writer,
        "countdown",
        "CountdownRequest",
        serde_json::json!({ "start": 5, "delay_ms": 500 }),
        10000,
    )
    .await?;

    println!();

    // Test: Quick countdown from 3
    println!("⏱️  Test 2: Quick countdown from 3 (100ms delay)");
    let _frames = send_stream_request(
        reader,
        writer,
        "countdown",
        "CountdownRequest",
        serde_json::json!({ "start": 3, "delay_ms": 100 }),
        5000,
    )
    .await?;

    Ok(())
}

/// Demonstrates paginated list streaming.
async fn demo_paginated_list(
    reader: &mut tokio::net::unix::OwnedReadHalf,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║                   Paginated List Stream Demo                 ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    // Test: List items with page size of 3
    println!("📋 Test 1: List items (page size: 3)");
    let _frames = send_stream_request(
        reader,
        writer,
        "list_service",
        "ListItemsRequest",
        serde_json::json!({ "page_size": 3 }),
        10000,
    )
    .await?;

    println!();

    // Test: List items with page size of 5
    println!("📋 Test 2: List items (page size: 5)");
    let _frames = send_stream_request(
        reader,
        writer,
        "list_service",
        "ListItemsRequest",
        serde_json::json!({ "page_size": 5 }),
        10000,
    )
    .await?;

    Ok(())
}

/// Demonstrates error handling in streaming.
async fn demo_error_handling(
    reader: &mut tokio::net::unix::OwnedReadHalf,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║                  Error Handling Demo                          ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    // Test: Send stream request to non-existent agent
    println!("❌ Test: Stream request to non-existent agent 'nonexistent'");
    let result = send_stream_request(
        reader,
        writer,
        "nonexistent",
        "SomeMessage",
        serde_json::json!({ "test": "data" }),
        5000,
    )
    .await;

    match result {
        Ok(frames) => {
            if let Some(frame) = frames.first() {
                if frame.error.is_some() {
                    println!("   Expected error received");
                }
            }
        }
        Err(e) => {
            println!("   Error: {e}");
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║       IPC Streaming Response Example (Client)                ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    // Resolve socket path
    let socket_path = resolve_socket_path();
    println!("🔌 Socket path: {}", socket_path.display());

    // Check if socket is available
    if !socket_exists(&socket_path) {
        eprintln!("\n❌ Error: Socket does not exist at {}", socket_path.display());
        eprintln!("   Make sure the ipc_streaming server is running:");
        eprintln!("   cargo run --example ipc_streaming_server --features ipc");
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
    println!();

    let (mut reader, mut writer) = stream.into_split();

    // Run demonstrations
    demo_countdown(&mut reader, &mut writer).await?;
    demo_paginated_list(&mut reader, &mut writer).await?;
    demo_error_handling(&mut reader, &mut writer).await?;

    println!("\n════════════════════════════════════════════════════════════════");
    println!("  All streaming IPC tests completed!");
    println!("════════════════════════════════════════════════════════════════");

    Ok(())
}
