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

//! IPC Fruit Market Example - Keyboard Client
//!
//! Captures keyboard input and sends commands to the server via IPC.
//!
//! # Features
//!
//! - **Raw Keyboard Input**: Uses crossterm for raw terminal mode
//! - **Fire-and-Forget**: Sends commands without waiting for response
//! - **Random Item Selection**: Simulates scanning random fruits
//!
//! # Controls
//!
//! - `s` - Scan a random fruit item
//! - `?` - Toggle help display
//! - `q` or `ESC` - Quit
//!
//! # Running This Example
//!
//! ```bash
//! cargo run --example ipc_fruit_market_keyboard --features ipc
//! ```

use std::io::{stdout, Write};
use std::path::PathBuf;

use acton_reactive::ipc::protocol::write_envelope;
use acton_reactive::ipc::{socket_exists, socket_is_alive, IpcConfig, IpcEnvelope};
use acton_reactive::prelude::acton_main;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use rand::prelude::*;
use serde::{Deserialize, Serialize};
use tokio::net::UnixStream;

/// Default server name to connect to.
const DEFAULT_SERVER: &str = "ipc_fruit_market_server";

/// List of possible fruit items.
const FRUIT_ITEMS: &[&str] = &[
    "Apple",
    "Banana",
    "Cantaloupe",
    "Orange",
    "Grapes",
    "Mango",
    "Pineapple",
    "Strawberry",
    "Watermelon",
    "Kiwi",
];

/// Minimum quantity for random selection.
const QUANTITY_MIN: i32 = 1;

/// Maximum quantity for random selection.
const QUANTITY_MAX: i32 = 6;

// ============================================================================
// Message Types (must match server definitions)
// ============================================================================

/// Command to scan a fruit item.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ScanItem {
    fruit_name: String,
    quantity: i32,
}

/// Command to toggle help display.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct ToggleHelp;

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

    // Default: connect to ipc_fruit_market server
    let mut config = IpcConfig::load();
    config.socket.app_name = Some(DEFAULT_SERVER.to_string());
    config.socket_path()
}

/// Sends a scan item command to the server (fire-and-forget).
async fn send_scan_item(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    fruit_name: &str,
    quantity: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let command = ScanItem {
        fruit_name: fruit_name.to_string(),
        quantity,
    };

    // Fire-and-forget: expects_reply = false (default)
    let envelope = IpcEnvelope::new(
        "price_service",
        "ScanItem",
        serde_json::to_value(&command)?,
    );

    write_envelope(writer, &envelope).await?;

    Ok(())
}

/// Sends a toggle help command to the server (fire-and-forget).
async fn send_toggle_help(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> Result<(), Box<dyn std::error::Error>> {
    let command = ToggleHelp;

    let envelope = IpcEnvelope::new(
        "price_service",
        "ToggleHelp",
        serde_json::to_value(&command)?,
    );

    write_envelope(writer, &envelope).await?;

    Ok(())
}

/// Selects a random fruit and quantity.
fn random_fruit() -> (&'static str, i32) {
    let mut rng = rand::rng();
    let fruit = FRUIT_ITEMS.choose(&mut rng).unwrap_or(&"Apple");
    let quantity = rng.random_range(QUANTITY_MIN..=QUANTITY_MAX);
    (fruit, quantity)
}

// ============================================================================
// Main
// ============================================================================

#[acton_main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("====================================================================");
    println!("       IPC Fruit Market - Keyboard Client");
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
        eprintln!("Make sure the fruit market server is running:");
        eprintln!("  cargo run --example ipc_fruit_market_server --features ipc");
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

    let (_reader, mut writer) = stream.into_split();

    println!();
    println!("====================================================================");
    println!("  Keyboard Client is ready!");
    println!();
    println!("  Controls:");
    println!("    s     - Scan a random fruit item");
    println!("    ?     - Toggle help display");
    println!("    q/ESC - Quit");
    println!("====================================================================");
    println!();

    // Enable raw mode for capturing individual keystrokes
    enable_raw_mode()?;

    // Ensure we restore terminal state on exit
    let _guard = scopeguard::guard((), |()| {
        let _ = disable_raw_mode();
    });

    let mut items_scanned: u64 = 0;
    let mut stdout = stdout();

    loop {
        // Poll for keyboard events
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key_event) = event::read()? {
                // Only process key press events (not release or repeat)
                if key_event.kind != KeyEventKind::Press {
                    continue;
                }

                // Check for quit keys
                if key_event.code == KeyCode::Esc || key_event.code == KeyCode::Char('q') {
                    break;
                }

                // Check for Ctrl+C
                if key_event.modifiers.contains(KeyModifiers::CONTROL)
                    && key_event.code == KeyCode::Char('c')
                {
                    break;
                }

                match key_event.code {
                    // Scan item
                    KeyCode::Char('s') => {
                        let (fruit, quantity) = random_fruit();
                        if let Err(e) = send_scan_item(&mut writer, fruit, quantity).await {
                            let _ = disable_raw_mode();
                            eprintln!("\nError sending scan command: {e}");
                            break;
                        }
                        items_scanned += 1;
                        write!(
                            stdout,
                            "  Scanned: {fruit} x{quantity} (#{items_scanned} items)\r\n"
                        )?;
                        stdout.flush()?;
                    }
                    // Toggle help
                    KeyCode::Char('?') => {
                        if let Err(e) = send_toggle_help(&mut writer).await {
                            let _ = disable_raw_mode();
                            eprintln!("\nError sending help toggle: {e}");
                            break;
                        }
                        write!(stdout, "  Toggled help display\r\n")?;
                        stdout.flush()?;
                    }
                    _ => {}
                }
            }
        }
    }

    // Raw mode is disabled by the scopeguard

    println!();
    println!();
    println!("====================================================================");
    println!("  Keyboard Client shutting down.");
    println!("  Scanned {items_scanned} items.");
    println!("====================================================================");

    Ok(())
}
