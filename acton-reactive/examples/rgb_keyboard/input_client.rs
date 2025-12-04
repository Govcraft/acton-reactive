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

//! RGB Keyboard Example - Input Client
//!
//! Captures raw keyboard input and streams it to the server.
//!
//! # Features
//!
//! - **Raw Keyboard Input**: Uses crossterm for raw terminal mode
//! - **Fire-and-Forget**: Sends keystrokes without waiting for response
//! - **Escape to Exit**: Press Esc key to quit
//!
//! # Running This Example
//!
//! ```bash
//! cargo run --example rgb_keyboard_input_client --features ipc
//! ```

use std::io::{stdout, Write};
use std::path::PathBuf;

use acton_reactive::ipc::protocol::write_envelope;
use acton_reactive::ipc::{socket_exists, socket_is_alive, IpcConfig, IpcEnvelope};
use acton_reactive::prelude::acton_main;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use serde::{Deserialize, Serialize};
use tokio::net::UnixStream;

/// Default server name to connect to.
const DEFAULT_SERVER: &str = "rgb_keyboard_server";

// ============================================================================
// Message Types (must match server definitions)
// ============================================================================

/// A keystroke to send to the server.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Keystroke {
    character: char,
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

/// Sends a keystroke to the server (fire-and-forget).
async fn send_keystroke(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    character: char,
) -> Result<(), Box<dyn std::error::Error>> {
    let keystroke = Keystroke { character };

    // Fire-and-forget: expects_reply = false (default)
    let envelope = IpcEnvelope::new(
        "keystroke_processor",
        "Keystroke",
        serde_json::to_value(&keystroke)?,
    );

    write_envelope(writer, &envelope).await?;

    Ok(())
}

// ============================================================================
// Main
// ============================================================================

#[acton_main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("====================================================================");
    println!("       RGB Keyboard - Input Client");
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

    let (_reader, mut writer) = stream.into_split();

    println!();
    println!("====================================================================");
    println!("  Input Client is ready!");
    println!();
    println!("  Type characters to send them to the server.");
    println!("  Each keystroke will be colored by the R/G/B clients.");
    println!();
    println!("  Press ESC to quit.");
    println!("====================================================================");
    println!();

    // Enable raw mode for capturing individual keystrokes
    enable_raw_mode()?;

    // Ensure we restore terminal state on exit
    let _guard = scopeguard::guard((), |()| {
        let _ = disable_raw_mode();
    });

    let mut keystrokes_sent: u64 = 0;

    // Use stdout for printing while in raw mode
    let mut stdout = stdout();

    loop {
        // Poll for keyboard events
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key_event) = event::read()? {
                // Only process key press events (not release or repeat)
                if key_event.kind != KeyEventKind::Press {
                    continue;
                }

                // Check for Ctrl+C or Escape to exit
                if key_event.code == KeyCode::Esc {
                    break;
                }

                if key_event.modifiers.contains(KeyModifiers::CONTROL)
                    && key_event.code == KeyCode::Char('c')
                {
                    break;
                }

                // Extract character from key event
                let character = match key_event.code {
                    KeyCode::Char(c) => Some(c),
                    KeyCode::Enter => Some('\n'),
                    KeyCode::Tab => Some('\t'),
                    KeyCode::Backspace => Some('\x08'),
                    KeyCode::Delete => Some('\x7f'),
                    _ => None,
                };

                if let Some(c) = character {
                    // Send keystroke to server
                    if let Err(e) = send_keystroke(&mut writer, c).await {
                        // Disable raw mode before printing error
                        let _ = disable_raw_mode();
                        eprintln!("\nError sending keystroke: {e}");
                        break;
                    }

                    keystrokes_sent += 1;

                    // Print feedback (in raw mode we need \r\n)
                    let display_char = match c {
                        '\n' => "\\n".to_string(),
                        '\t' => "\\t".to_string(),
                        '\x08' => "\\b".to_string(),
                        '\x7f' => "DEL".to_string(),
                        c if c.is_control() => format!("^{}", (c as u8 + 64) as char),
                        c => c.to_string(),
                    };

                    write!(stdout, "  Sent: '{display_char}' (#{keystrokes_sent} keystrokes)\r\n")?;
                    stdout.flush()?;
                }
            }
        }
    }

    // Raw mode is disabled by the scopeguard

    println!();
    println!();
    println!("====================================================================");
    println!("  Input Client shutting down.");
    println!("  Sent {keystrokes_sent} keystrokes to server.");
    println!("====================================================================");

    Ok(())
}
