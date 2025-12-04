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

//! IPC Fruit Market Example - Display Client
//!
//! Subscribes to server broadcasts and renders the point-of-sale terminal UI.
//!
//! # Features
//!
//! - **Subscribes to Updates**: Receives `ItemScanned`, `PriceUpdate`, and `HelpToggled`
//! - **Local State Management**: Maintains cart state for UI rendering
//! - **Terminal UI**: Colorful receipt-style display
//!
//! # Running This Example
//!
//! ```bash
//! cargo run --example ipc_fruit_market_display --features ipc
//! ```

use std::collections::HashMap;
use std::io::{stdout, Stdout, Write};
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
use ansi_term::Color::RGB;
use crossterm::{
    cursor, execute, queue,
    style::Print,
    terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate},
};
use serde::{Deserialize, Serialize};
use tokio::net::UnixStream;
use tokio::time::timeout;

/// Default server name to connect to.
const DEFAULT_SERVER: &str = "ipc_fruit_market_server";

// ============================================================================
// UI Constants
// ============================================================================

const COLS: u16 = 40;
const PAD_LEFT: u16 = 3;
const PAD_TOP: u16 = 2;
const HEADER_HEIGHT: u16 = 4;
const TRANSACTION_RECEIPT: &str = "Transaction Receipt";
const CHECKMARK: &str = "\u{2713}";
const HELP_TEXT: &str = "s: scan item, q: quit, ?: toggle help";
const HELP_TEXT_SHORT: &str = "?: toggle help";
const START_HELP: &str = "Press 's' to scan an item.";
const SUBTOTAL_LABEL: &str = "Subtotal";
const TAX_LABEL: &str = "Tax";
const DUE_LABEL: &str = "Due";

// RGB color constants
const CHECK_MARK_COLOR: (u8, u8, u8) = (113, 208, 131);
const TOTAL_DUE_COLOR_NOT_LOADED: (u8, u8, u8) = (255, 255, 255);
const COLOR_DARK_GREY: (u8, u8, u8) = (58, 58, 58);
const COLOR_LIGHT_BLUE: (u8, u8, u8) = (194, 234, 255);
const COLOR_MEDIUM_BLUE: (u8, u8, u8) = (117, 199, 240);
const COLOR_GREEN: (u8, u8, u8) = (194, 240, 194);
const COLOR_LOADER: (u8, u8, u8) = (73, 71, 78);
const COLOR_HELP_TEXT: (u8, u8, u8) = (96, 96, 96);

/// Tax rate (7%)
const TAX_RATE_PERCENT: i32 = 7;

// ============================================================================
// Message Types (must match server definitions)
// ============================================================================

/// Notification that an item has been scanned.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ItemScanned {
    item_id: String,
    name: String,
    quantity: i32,
}

/// Notification that a price has been retrieved.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PriceUpdate {
    item_id: String,
    unit_price: i32,
    total_price: i32,
}

// Note: HelpToggled message is just a signal - we only check the message type string
// and don't need to deserialize the payload.

// ============================================================================
// Display State
// ============================================================================

/// A cart item with its display state.
#[derive(Clone, Debug)]
struct CartItem {
    item_id: String,
    name: String,
    quantity: i32,
    unit_price: Option<i32>,
    total_price: Option<i32>,
}

impl CartItem {
    const fn new(item_id: String, name: String, quantity: i32) -> Self {
        Self {
            item_id,
            name,
            quantity,
            unit_price: None,
            total_price: None,
        }
    }

    const fn is_priced(&self) -> bool {
        self.unit_price.is_some()
    }

    const fn set_price(&mut self, unit_price: i32, total_price: i32) {
        self.unit_price = Some(unit_price);
        self.total_price = Some(total_price);
    }
}

/// Display state for the UI.
#[derive(Default)]
struct DisplayState {
    /// Cart items indexed by `item_id`.
    items: HashMap<String, CartItem>,
    /// Order of items (for consistent display).
    item_order: Vec<String>,
    /// Whether to show full help text.
    show_help: bool,
}

impl DisplayState {
    fn add_item(&mut self, item: CartItem) {
        let item_id = item.item_id.clone();
        self.items.insert(item_id.clone(), item);
        self.item_order.push(item_id);
    }

    fn update_price(&mut self, item_id: &str, unit_price: i32, total_price: i32) {
        if let Some(item) = self.items.get_mut(item_id) {
            item.set_price(unit_price, total_price);
        }
    }

    const fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    fn all_priced(&self) -> bool {
        self.items.values().all(CartItem::is_priced)
    }

    fn subtotal(&self) -> i32 {
        self.items
            .values()
            .filter_map(|item| item.total_price)
            .sum()
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Resolves the socket path from command line arguments or defaults.
fn resolve_socket_path() -> PathBuf {
    let args: Vec<String> = std::env::args().collect();

    if let Some(pos) = args.iter().position(|a| a == "--server") {
        if let Some(server_name) = args.get(pos + 1) {
            let mut config = IpcConfig::load();
            config.socket.app_name = Some(server_name.clone());
            return config.socket_path();
        }
    }

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

/// Formats cents as $X.YY.
fn format_money(cents: i32) -> String {
    if cents <= 0 {
        format!("${: >3}{:>3}", "", "-")
    } else {
        format!("${: >3}.{:0>2}", cents / 100, cents % 100)
    }
}

// ============================================================================
// UI Rendering
// ============================================================================

fn print_header(stdout: &mut Stdout) -> anyhow::Result<()> {
    execute!(stdout, BeginSynchronizedUpdate)?;
    let top = PAD_TOP;

    // Top border
    let header_border = RGB(COLOR_DARK_GREY.0, COLOR_DARK_GREY.1, COLOR_DARK_GREY.2)
        .paint("─".repeat(COLS as usize + 1))
        .to_string();
    queue!(stdout, cursor::MoveTo(PAD_LEFT, top))?;
    queue!(stdout, Print(&header_border))?;

    // Centered title
    let padding = (COLS as usize).saturating_sub(TRANSACTION_RECEIPT.len()) / 2;
    let centered_text = format!("{}{}", " ".repeat(padding), TRANSACTION_RECEIPT);
    queue!(stdout, cursor::MoveTo(PAD_LEFT, top + 1))?;
    queue!(stdout, Print(&centered_text))?;

    // Separator
    let separator = RGB(COLOR_DARK_GREY.0, COLOR_DARK_GREY.1, COLOR_DARK_GREY.2)
        .paint(format!(
            "{}{}{}",
            "─".repeat((COLS - 11) as usize),
            "┬",
            "─".repeat(11)
        ))
        .to_string();
    queue!(stdout, cursor::MoveTo(PAD_LEFT, top + 2))?;
    queue!(stdout, Print(&separator))?;

    execute!(stdout, EndSynchronizedUpdate)?;
    Ok(())
}

fn print_items(stdout: &mut Stdout, state: &DisplayState) -> anyhow::Result<()> {
    execute!(stdout, BeginSynchronizedUpdate)?;

    let top = PAD_TOP + HEADER_HEIGHT;

    // Clear previous items area
    for i in 0..15 {
        queue!(stdout, cursor::MoveTo(PAD_LEFT, top + i))?;
        queue!(stdout, Clear(ClearType::CurrentLine))?;
    }

    if state.items.is_empty() {
        // Display initial help message
        let help_len = u16::try_from(START_HELP.len()).unwrap_or(u16::MAX);
        let start_col = PAD_LEFT + ((COLS - help_len) / 2);
        queue!(stdout, cursor::MoveTo(start_col, top))?;
        queue!(stdout, Print(START_HELP))?;
        execute!(stdout, EndSynchronizedUpdate)?;
        return Ok(());
    }

    // Print each item in order
    for (i, item_id) in state.item_order.iter().enumerate() {
        if let Some(item) = state.items.get(item_id) {
            let formatted = if item.is_priced() {
                let unit_price = item.unit_price.unwrap();
                let total_price = item.total_price.unwrap();
                format!(
                    "{}({}) @ {} {} {:>5}",
                    RGB(COLOR_LIGHT_BLUE.0, COLOR_LIGHT_BLUE.1, COLOR_LIGHT_BLUE.2).paint(&item.name),
                    RGB(COLOR_MEDIUM_BLUE.0, COLOR_MEDIUM_BLUE.1, COLOR_MEDIUM_BLUE.2)
                        .paint(item.quantity.to_string()),
                    format_money(unit_price),
                    RGB(COLOR_DARK_GREY.0, COLOR_DARK_GREY.1, COLOR_DARK_GREY.2).paint("│"),
                    RGB(COLOR_GREEN.0, COLOR_GREEN.1, COLOR_GREEN.2)
                        .paint(format_money(total_price))
                )
            } else {
                format!(
                    "{}( ) @ {} {} {:>5}",
                    RGB(COLOR_LOADER.0, COLOR_LOADER.1, COLOR_LOADER.2).paint(&item.name),
                    format_money(0),
                    RGB(COLOR_DARK_GREY.0, COLOR_DARK_GREY.1, COLOR_DARK_GREY.2).paint("│"),
                    format_money(0)
                )
            };

            // Calculate display length without ANSI codes for alignment
            let plain_text = if item.is_priced() {
                let unit_price = item.unit_price.unwrap();
                let total_price = item.total_price.unwrap();
                format!(
                    "{}({}) @ {} | {}",
                    item.name,
                    item.quantity,
                    format_money(unit_price),
                    format_money(total_price)
                )
            } else {
                format!(
                    "{}( ) @ {} | {}",
                    item.name,
                    format_money(0),
                    format_money(0)
                )
            };

            let display_len = u16::try_from(plain_text.len()).unwrap_or(u16::MAX);
            let start_col = PAD_LEFT + COLS.saturating_sub(display_len);
            let row = u16::try_from(i).unwrap_or(u16::MAX);

            queue!(stdout, cursor::MoveTo(start_col, top + row))?;
            queue!(stdout, Print(formatted))?;
        }
    }

    execute!(stdout, EndSynchronizedUpdate)?;
    Ok(())
}

fn print_totals(stdout: &mut Stdout, state: &DisplayState) -> anyhow::Result<()> {
    if state.items.is_empty() {
        return Ok(());
    }

    execute!(stdout, BeginSynchronizedUpdate)?;

    let items_count = u16::try_from(state.items.len()).unwrap_or(u16::MAX);
    let top = PAD_TOP + HEADER_HEIGHT + items_count;

    // Separator
    let separator = RGB(COLOR_DARK_GREY.0, COLOR_DARK_GREY.1, COLOR_DARK_GREY.2)
        .paint(format!(
            "{}{}{}",
            "─".repeat((COLS - 11) as usize),
            "┴",
            "─".repeat(11)
        ))
        .to_string();

    queue!(stdout, cursor::MoveTo(PAD_LEFT, top))?;
    queue!(stdout, Clear(ClearType::CurrentLine))?;
    queue!(stdout, Print(&separator))?;

    // Calculate totals
    let subtotal = state.subtotal();
    let tax = subtotal * TAX_RATE_PERCENT / 100;
    let total_due = subtotal + tax;
    let all_loaded = state.all_priced();

    // Format strings
    let subtotal_str = format!("{:<11}{}", SUBTOTAL_LABEL, format_money(subtotal));
    let tax_str = format!("{:<11}{}", TAX_LABEL, format_money(tax));

    let total_due_str = if all_loaded {
        format!(
            "{:<11}{} {}",
            DUE_LABEL,
            RGB(CHECK_MARK_COLOR.0, CHECK_MARK_COLOR.1, CHECK_MARK_COLOR.2)
                .paint(format_money(total_due)),
            RGB(CHECK_MARK_COLOR.0, CHECK_MARK_COLOR.1, CHECK_MARK_COLOR.2).paint(CHECKMARK)
        )
    } else {
        format!(
            "{:<11}{}",
            DUE_LABEL,
            RGB(
                TOTAL_DUE_COLOR_NOT_LOADED.0,
                TOTAL_DUE_COLOR_NOT_LOADED.1,
                TOTAL_DUE_COLOR_NOT_LOADED.2
            )
            .paint(format_money(total_due))
        )
    };

    // Calculate alignment
    let subtotal_plain = format!("{:<11}{}", SUBTOTAL_LABEL, format_money(subtotal));
    let tax_plain = format!("{:<11}{}", TAX_LABEL, format_money(tax));
    let total_due_plain = if all_loaded {
        format!(
            "{:<11}{} {}",
            DUE_LABEL,
            format_money(total_due),
            CHECKMARK
        )
    } else {
        format!("{:<11}{}", DUE_LABEL, format_money(total_due))
    };

    let subtotal_width = u16::try_from(subtotal_plain.len()).unwrap_or(u16::MAX);
    let tax_width = u16::try_from(tax_plain.len()).unwrap_or(u16::MAX);
    let total_due_width = u16::try_from(total_due_plain.len()).unwrap_or(u16::MAX);

    let subtotal_start_col = PAD_LEFT + COLS.saturating_sub(subtotal_width);
    let tax_start_col = PAD_LEFT + COLS.saturating_sub(tax_width);
    let total_due_start_col = PAD_LEFT + COLS.saturating_sub(total_due_width);

    // Print totals
    queue!(stdout, cursor::MoveTo(subtotal_start_col, top + 1))?;
    queue!(stdout, Clear(ClearType::CurrentLine))?;
    queue!(stdout, Print(&subtotal_str))?;

    queue!(stdout, cursor::MoveTo(tax_start_col, top + 2))?;
    queue!(stdout, Clear(ClearType::CurrentLine))?;
    queue!(stdout, Print(&tax_str))?;

    queue!(stdout, cursor::MoveTo(total_due_start_col, top + 3))?;
    queue!(stdout, Clear(ClearType::CurrentLine))?;
    write!(stdout, "{total_due_str}")?;

    execute!(stdout, EndSynchronizedUpdate)?;
    Ok(())
}

fn print_help(stdout: &mut Stdout, state: &DisplayState) -> anyhow::Result<()> {
    execute!(stdout, BeginSynchronizedUpdate)?;

    let help_msg = if state.show_help {
        HELP_TEXT
    } else {
        HELP_TEXT_SHORT
    };

    let items_count = u16::try_from(state.items.len()).unwrap_or(u16::MAX);
    let top = PAD_TOP + HEADER_HEIGHT + items_count + 4;
    let help_len = u16::try_from(help_msg.len()).unwrap_or(u16::MAX);
    let start_col = PAD_LEFT + COLS.saturating_sub(help_len);

    queue!(stdout, cursor::MoveTo(0, top))?;
    queue!(stdout, Clear(ClearType::FromCursorDown))?;
    queue!(stdout, cursor::MoveTo(start_col, top))?;

    queue!(
        stdout,
        Print(
            RGB(COLOR_HELP_TEXT.0, COLOR_HELP_TEXT.1, COLOR_HELP_TEXT.2)
                .paint(help_msg)
                .to_string()
        )
    )?;

    execute!(stdout, EndSynchronizedUpdate)?;
    Ok(())
}

fn repaint(stdout: &mut Stdout, state: &DisplayState) -> anyhow::Result<()> {
    print_header(stdout)?;
    print_items(stdout, state)?;
    print_totals(stdout, state)?;
    print_help(stdout, state)?;
    stdout.flush()?;
    Ok(())
}

// ============================================================================
// Main
// ============================================================================

#[acton_main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Resolve socket path
    let socket_path = resolve_socket_path();

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
    let stream = UnixStream::connect(&socket_path).await?;
    let (mut reader, mut writer) = stream.into_split();

    // Subscribe to all relevant message types
    let message_types = vec![
        "ItemScanned".to_string(),
        "PriceUpdate".to_string(),
        "HelpToggled".to_string(),
    ];
    let _corr_id = send_subscribe(&mut writer, message_types).await?;

    // Wait for subscription response
    let (msg_type, _format, payload) =
        timeout(Duration::from_secs(5), read_frame(&mut reader, MAX_FRAME_SIZE)).await??;

    if msg_type == MSG_TYPE_RESPONSE {
        let response: IpcSubscriptionResponse = serde_json::from_slice(&payload)?;
        if !response.success {
            eprintln!(
                "Subscription failed: {}",
                response.error.as_deref().unwrap_or("Unknown error")
            );
            std::process::exit(1);
        }
    }

    // Initialize display
    let mut stdout = stdout();
    execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0))?;
    execute!(stdout, cursor::Hide)?;

    // Ensure we restore cursor on exit
    let _cursor_guard = scopeguard::guard((), |()| {
        let mut out = std::io::stdout();
        let _ = execute!(out, cursor::Show);
    });

    // Initialize state
    let mut state = DisplayState::default();

    // Initial paint
    repaint(&mut stdout, &state)?;

    // Main loop: receive and process messages
    loop {
        match timeout(Duration::from_secs(60), read_frame(&mut reader, MAX_FRAME_SIZE)).await {
            Ok(Ok((msg_type, _format, payload))) => {
                if msg_type == MSG_TYPE_PUSH {
                    let notification: IpcPushNotification = serde_json::from_slice(&payload)?;

                    match notification.message_type.as_str() {
                        "ItemScanned" => {
                            if let Ok(item) =
                                serde_json::from_value::<ItemScanned>(notification.payload)
                            {
                                state.add_item(CartItem::new(
                                    item.item_id,
                                    item.name,
                                    item.quantity,
                                ));
                                repaint(&mut stdout, &state)?;
                            }
                        }
                        "PriceUpdate" => {
                            if let Ok(update) =
                                serde_json::from_value::<PriceUpdate>(notification.payload)
                            {
                                state.update_price(
                                    &update.item_id,
                                    update.unit_price,
                                    update.total_price,
                                );
                                repaint(&mut stdout, &state)?;
                            }
                        }
                        "HelpToggled" => {
                            state.toggle_help();
                            repaint(&mut stdout, &state)?;
                        }
                        _ => {}
                    }
                }
            }
            Ok(Err(e)) => {
                // Clear screen and show error
                execute!(stdout, cursor::Show)?;
                execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0))?;
                eprintln!("Connection error: {e}");
                break;
            }
            Err(_) => {
                // Timeout - continue waiting (no-op heartbeat)
            }
        }
    }

    Ok(())
}
