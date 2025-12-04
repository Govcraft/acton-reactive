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

//! RGB Keyboard Example - Output Client
//!
//! Subscribes to colored character broadcasts and displays them with true RGB colors.
//!
//! # Features
//!
//! - **Subscribes to `ColoredCharacter`**: Receives completed colored characters
//! - **True RGB Colors**: Uses ANSI 24-bit color codes for accurate display
//! - **Real-time Display**: Shows characters as they arrive
//!
//! # Running This Example
//!
//! ```bash
//! cargo run --example rgb_keyboard_output_client --features ipc
//! ```

use std::io::{stdout, Write};
use std::path::PathBuf;
use std::time::Duration;

use acton_reactive::ipc::protocol::{
    read_frame, write_frame, Format, MAX_FRAME_SIZE, MSG_TYPE_PUSH, MSG_TYPE_RESPONSE,
    MSG_TYPE_SUBSCRIBE,
};
use acton_reactive::ipc::{
    socket_exists, socket_is_alive, IpcConfig, IpcPushNotification, IpcSubscribeRequest,
    IpcSubscriptionResponse,
};
use acton_reactive::prelude::acton_main;
use serde::{Deserialize, Serialize};
use tokio::net::UnixStream;
use tokio::time::timeout;

/// Default server name to connect to.
const DEFAULT_SERVER: &str = "rgb_keyboard_server";

// ============================================================================
// Message Types (must match server definitions)
// ============================================================================

/// A completed colored character from the server.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ColoredCharacter {
    character: char,
    red: u8,
    green: u8,
    blue: u8,
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

/// Returns ANSI 24-bit color escape sequence for foreground.
fn rgb_color(r: u8, g: u8, b: u8) -> String {
    format!("\x1b[38;2;{r};{g};{b}m")
}

/// Returns ANSI reset escape sequence.
const RESET: &str = "\x1b[0m";

/// Displays a colored character.
fn display_colored_char(colored: &ColoredCharacter, count: u64) {
    let color = rgb_color(colored.red, colored.green, colored.blue);

    // Handle special characters for display
    let display = match colored.character {
        '\n' => "\\n".to_string(),
        '\t' => "\\t".to_string(),
        '\x08' => "\\b".to_string(),
        '\x7f' => "DEL".to_string(),
        c if c.is_control() => format!("^{}", (c as u8 + 64) as char),
        c => c.to_string(),
    };

    let red = colored.red;
    let green = colored.green;
    let blue = colored.blue;
    println!("  {color}{display}{RESET} RGB({red:>3}, {green:>3}, {blue:>3}) #{count}");
    let _ = stdout().flush();
}

// ============================================================================
// Main
// ============================================================================

#[acton_main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("====================================================================");
    println!("       RGB Keyboard - Output Client");
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

    // Subscribe to ColoredCharacter broadcasts
    println!("\nSubscribing to ColoredCharacter...");
    let _corr_id = send_subscribe(&mut writer, vec!["ColoredCharacter".to_string()]).await?;

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
    println!("  Output Client is ready!");
    println!();
    println!("  Waiting for colored characters from server...");
    println!("  Characters will appear with randomly generated RGB colors.");
    println!();
    println!("  Press Ctrl+C to quit.");
    println!("====================================================================");
    println!();
    println!("  Received characters:");
    println!();

    let mut characters_received: u64 = 0;

    // Main loop: receive and display colored characters
    loop {
        match timeout(Duration::from_secs(60), read_frame(&mut reader, MAX_FRAME_SIZE)).await {
            Ok(Ok((msg_type, _format, payload))) => {
                if msg_type == MSG_TYPE_PUSH {
                    let notification: IpcPushNotification = serde_json::from_slice(&payload)?;

                    if notification.message_type == "ColoredCharacter" {
                        if let Ok(colored) =
                            serde_json::from_value::<ColoredCharacter>(notification.payload)
                        {
                            characters_received += 1;
                            display_colored_char(&colored, characters_received);
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                eprintln!("\nError reading frame: {e}");
                break;
            }
            Err(_) => {
                // Timeout - continue waiting (just heartbeat)
                print!(".");
                let _ = stdout().flush();
            }
        }
    }

    println!();
    println!("====================================================================");
    println!("  Output Client shutting down.");
    println!("  Received {characters_received} colored characters.");
    println!("====================================================================");

    Ok(())
}
