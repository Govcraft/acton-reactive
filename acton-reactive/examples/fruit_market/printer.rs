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
use std::fmt::{self, Debug, Display};
use std::io::{stdout, Stdout, Write};

use ansi_term::Color::RGB;
use crossterm::{
    cursor,
    execute,
    queue,
    style::Print, // Added Print
    terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate},
};
use dashmap::DashMap;
use mti::prelude::MagicTypeId;
use tracing::{error, trace};

use acton_reactive::prelude::*;

// Import message types and data structures used by this actor.
// Removed ToggleHelp as it's defined locally in this file.
use crate::cart_item::CartItem;
use crate::{PriceRequest, PriceResponse, PrinterMessage};

// --- Constants for UI Layout and Styling ---
const COLS: u16 = 40; // Width of the receipt area
const PAD_LEFT: u16 = 3; // Left padding for the receipt
const PAD_TOP: u16 = 2; // Top padding for the receipt content
const HEADER_HEIGHT: u16 = 4; // Height reserved for the header section
const TRANSACTION_RECEIPT: &str = "Transaction Receipt"; // Header title
const STARTED: &str = "\u{2713}"; // Checkmark symbol
const HELP_TEXT: &str = "s: scan item, q: quit, ?: toggle help"; // Full help text
const HELP_TEXT_SHORT: &str = "?: toggle help"; // Short help text
const START_HELP: &str = "Press 's' to scan an item."; // Initial prompt
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

/// Represents the state (model) for the Printer actor.
/// Responsible for rendering the fruit market UI to the terminal.
// Note: Manual Default impl needed because `Stdout` doesn't impl Default.
// Cannot use `#[acton_actor]` because it attempts to derive Default and Clone,
// which `Stdout` does not implement. We derive Debug manually.
#[derive(Debug)] // Only derive Debug
pub struct Printer {
    /// Flag indicating if all requested item prices have been loaded.
    loaded: bool,
    /// Flag to control whether the full help text is shown.
    show_help: bool,
    /// Stores the items to be displayed, using a concurrent `DashMap` for thread-safe access.
    /// Keyed by item UPC (`MagicTypeId`), Value is the `DisplayItem` state.
    items: DashMap<MagicTypeId, DisplayItem>,
}

// Manual Default implementation for Printer state.
impl Default for Printer {
    fn default() -> Self {
        Self {
            loaded: true,          // Start assuming loaded until a loader is added
            show_help: false,      // Start with short help
            items: DashMap::new(), // Initialize empty map
        }
    }
}

/// Message to toggle the help display in the `Printer` UI.
#[acton_message]
pub struct ToggleHelp;

/// Represents the different states an item can be in for display purposes.
#[derive(Clone, Debug, Default)]
enum DisplayItem {
    /// The item's details are fully loaded (including price).
    Item(CartItem),
    /// The item has been requested but the price is still loading. Stores the item name.
    Loader(String),
    /// Initial state before any items are processed.
    #[default]
    Startup,
}

impl Display for DisplayItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Item(item) => {
                write!(
                    f,
                    "{}({}) @ {} │ {:>5}",
                    item.name(),
                    item.quantity(),
                    MoneyFmt(**item.cost()), // Use MoneyFmt for cost
                    MoneyFmt(item.price())   // Use MoneyFmt for total price
                )
            }
            Self::Loader(what) => write!(
                f,
                "{}( ) @ {} │ {:>5}",
                what,
                MoneyFmt(0), // Show zero cost/price for loader
                MoneyFmt(0)
            ),
            Self::Startup => write!(f, ""), // Empty display for startup state
        }
    }
}

/// A helper struct to hold the pre-formatted, colored string representation of a `DisplayItem`.
#[derive(Clone, Debug, Default)]
struct FormattedItem(String);

/// Converts a `DisplayItem` into a `FormattedItem` with ANSI color codes using `ansi_term`.
impl From<DisplayItem> for FormattedItem {
    fn from(value: DisplayItem) -> Self {
        match value {
            DisplayItem::Item(item) => {
                let name = item.name().clone();
                Self(format!(
                    "{}({}) @ {} {} {:>5}",
                    RGB(COLOR_LIGHT_BLUE.0, COLOR_LIGHT_BLUE.1, COLOR_LIGHT_BLUE.2).paint(name),
                    RGB(
                        COLOR_MEDIUM_BLUE.0,
                        COLOR_MEDIUM_BLUE.1,
                        COLOR_MEDIUM_BLUE.2,
                    )
                    .paint(item.quantity().to_string()),
                    MoneyFmt(**item.cost()), // Format cost
                    RGB(COLOR_DARK_GREY.0, COLOR_DARK_GREY.1, COLOR_DARK_GREY.2).paint("│"),
                    RGB(COLOR_GREEN.0, COLOR_GREEN.1, COLOR_GREEN.2)
                        .paint(MoneyFmt(item.price()).to_string()) // Format total price
                ))
            }
            DisplayItem::Loader(what) => Self(format!(
                "{}( ) @ {} {} {:>5}",
                RGB(COLOR_LOADER.0, COLOR_LOADER.1, COLOR_LOADER.2).paint(what),
                MoneyFmt(0), // Format zero cost
                RGB(COLOR_DARK_GREY.0, COLOR_DARK_GREY.1, COLOR_DARK_GREY.2).paint("│"),
                MoneyFmt(0) // Format zero price
            )),
            DisplayItem::Startup => Self(String::default()), // Empty for startup
        }
    }
}

impl Display for FormattedItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0) // Write the pre-formatted string
    }
}

/// Helper struct for formatting currency values (stored in cents).
#[derive(Clone, Debug, Default)]
struct MoneyFmt(i32);

/// Implements `Display` for `MoneyFmt` to show cents as $X.YY.
impl Display for MoneyFmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cents = self.0;
        if cents <= 0 {
            // Special formatting for zero or negative values
            write!(f, "${: >3}{:>3}", "", "-")?;
        } else {
            // Format positive values as dollars and cents.
            write!(f, "${: >3}.{:0>2}", cents / 100, cents % 100)?;
        }
        Ok(())
    }
}

impl Printer {
    /// Creates, configures, subscribes, and starts the Printer actor.
    pub async fn power_on(runtime: &mut ActorRuntime) -> anyhow::Result<ActorHandle> {
        // Create the actor builder with a specific name.
        let mut printer_builder = runtime.new_actor_with_name::<Self>("printer".to_string());

        // Configure message handlers.
        printer_builder
            // Handler for `PriceRequest`: Adds a loader entry for the item if not present.
            .mutate_on::<PriceRequest>(|actor, context| {
                let item = context.message().0.clone();
                trace!("PriceRequest: {}", item.name());

                // Check if an item with the same name already exists (regardless of state).
                let item_exists = actor.model.items.iter().any(|entry| match entry.value() {
                    DisplayItem::Item(existing_item) => existing_item.name() == item.name(),
                    DisplayItem::Loader(existing_name) => existing_name.as_str() == item.name().as_str(),
                    DisplayItem::Startup => false,
                });

                if item_exists {
                    trace!("Item already exists or is loading: {}", item.name());
                    ActorReply::immediate() // No change, no repaint needed from here
                } else {
                    // If item doesn't exist, insert a Loader entry.
                    actor.model.items.insert(
                        item.id().clone(),
                        DisplayItem::Loader(item.name().to_string()),
                    );
                    // Mark the display as not fully loaded.
                    actor.model.loaded = false;
                    // Trigger a repaint asynchronously by sending a message to self.
                    let self_handle = actor.handle().clone();
                    ActorReply::from_async(async move {
                        // Send repaint message instead of calling directly
                        self_handle.send(PrinterMessage::Repaint).await;
                    })
                }
            })
            // Handler for `PriceResponse`: Updates or inserts the item with its price.
            .mutate_on::<PriceResponse>(|actor, context| {
                let new_item = context.message().item.clone();
                trace!("PriceResponse: {}", new_item.name());

                // Check if an item with the same name already exists as a fully loaded item.
                let existing_key = actor.model.items.iter().find_map(|item| {
                    if let DisplayItem::Item(existing_item) = item.value() {
                        if existing_item.name() == new_item.name() {
                            Some(item.key().clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                });

                if let Some(key) = existing_key {
                    // If it exists as an Item, update its quantity.
                    if let Some(mut old_item) = actor.model.items.get_mut(&key) {
                        if let DisplayItem::Item(existing_item) = old_item.value_mut() {
                            existing_item
                                .set_quantity(existing_item.quantity() + new_item.quantity());
                            trace!("Updated item quantity: {:?}", existing_item);
                        }
                    }
                } else {
                    // If it doesn't exist as an Item (might be a Loader or not present),
                    // insert/replace it with the new fully loaded Item.
                    actor
                        .model
                        .items
                        .insert(new_item.id().clone(), DisplayItem::Item(new_item.clone()));
                    trace!("Inserted/Replaced item: {:?}", new_item.id());
                }

                // Check if all items are now fully loaded (not Loaders).
                actor.model.loaded = actor
                    .model
                    .items
                    .iter()
                    .all(|item| matches!(item.value(), DisplayItem::Item(_)));

                // Trigger repaint after update.
                let self_handle = actor.handle().clone();
                ActorReply::from_async(async move {
                    self_handle.send(PrinterMessage::Repaint).await;
                })
            })
            // Handler for `ToggleHelp`: Flips the show_help flag and repaints.
            .mutate_on::<ToggleHelp>(|actor, _context| {
                actor.model.show_help = !actor.model.show_help;
                // Trigger repaint
                let self_handle = actor.handle().clone();
                ActorReply::from_async(async move {
                    self_handle.send(PrinterMessage::Repaint).await;
                })
            })
            // Handler for `PrinterMessage::Repaint`: Performs the actual repaint.
            .mutate_on::<PrinterMessage>(|actor, _context| {
                // Perform the repaint logic. This requires mutable access to stdout.
                // Since handlers only get immutable access to the actor state (model),
                // we cannot directly call `Self::repaint` which needs `&mut actor.model.out`.
                // This highlights a limitation or a need for a different pattern (e.g.,
                // dedicated UI task, message passing to main thread, or unsafe code).
                // For this example, we'll call repaint directly, but this relies on
                // getting a fresh stdout handle inside repaint, which might not be ideal
                // for performance or complex scenarios.
                trace!("Received Repaint message. Repainting...");
                if let Err(e) = Self::repaint(&actor.model) {
                    error!("Failed to repaint UI: {}", e);
                }
                ActorReply::immediate()
            })
            // After start: Hide cursor and send initial repaint message.
            .after_start(|actor| {
                let mut stdout = stdout();
                let _ = execute!(stdout, cursor::Hide);
                // Send initial repaint message to self.
                let self_handle = actor.handle().clone();
                ActorReply::from_async(async move {
                    self_handle.send(PrinterMessage::Repaint).await;
                })
            });

        // Subscribe the actor to relevant message types via the broker.
        printer_builder.handle().subscribe::<PriceResponse>().await;
        printer_builder.handle().subscribe::<PrinterMessage>().await;
        printer_builder.handle().subscribe::<PriceRequest>().await;
        printer_builder.handle().subscribe::<ToggleHelp>().await;

        // Start the actor and return its handle.
        Ok(printer_builder.start().await)
    }

    /// Repaints the entire terminal UI by calling helper functions.
    /// NOTE: This function requires mutable access to stdout, which is difficult
    /// to achieve safely from within actor message handlers due to borrowing rules.
    /// This example currently triggers repaints via messages, and this function
    /// gets a new stdout handle each time, which might not be ideal.
    fn repaint(printer: &Self) -> anyhow::Result<()> {
        // It's problematic to get `&mut stdout` here if called from actor context.
        // For the example's sake, we get a new handle, but this isn't ideal.
        let mut stdout = stdout();
        Self::print_header(&mut stdout)?;
        Self::print_items(&mut stdout, printer)?;
        Self::print_totals(&mut stdout, printer)?;
        Self::print_help(&mut stdout, printer)?;
        stdout.flush()?; // Ensure all queued commands are executed
        Ok(())
    }

    /// Prints the static header section of the UI.
    fn print_header(stdout: &mut Stdout) -> anyhow::Result<()> {
        // Use synchronized updates for smoother rendering, reducing flicker.
        execute!(stdout, BeginSynchronizedUpdate)?;
        let top = PAD_TOP;

        // Draw top border.
        let header_border = RGB(COLOR_DARK_GREY.0, COLOR_DARK_GREY.1, COLOR_DARK_GREY.2)
            .paint("─".repeat(COLS as usize + 1))
            .to_string();
        queue!(stdout, cursor::MoveTo(PAD_LEFT, top))?;
        queue!(stdout, Print(&header_border))?; // Use Print command

        let padding = (COLS as usize).saturating_sub(TRANSACTION_RECEIPT.len()) / 2;
        let centered_text = format!("{}{}", " ".repeat(padding), TRANSACTION_RECEIPT);

        queue!(stdout, cursor::MoveTo(PAD_LEFT, top + 1))?;
        queue!(stdout, Print(&centered_text))?; // Use Print command

        // Draw separator line below header text.
        let separator = RGB(COLOR_DARK_GREY.0, COLOR_DARK_GREY.1, COLOR_DARK_GREY.2)
            .paint(format!(
                "{}{}{}",
                "─".repeat((COLS - 11) as usize),
                "┬",
                "─".repeat(11)
            ))
            .to_string();
        queue!(stdout, cursor::MoveTo(PAD_LEFT, top + 2))?;
        queue!(stdout, Print(&separator))?; // Use Print command

        execute!(stdout, EndSynchronizedUpdate)?;
        // Changes are buffered until EndSynchronizedUpdate is executed.
        Ok(())
    }

    /// Prints the list of cart items (or loaders).
    fn print_items(stdout: &mut Stdout, printer: &Self) -> anyhow::Result<()> {
        execute!(stdout, BeginSynchronizedUpdate)?;

        let top = PAD_TOP + HEADER_HEIGHT;

        // Clear previous items area
        for i in 0..15 {
            // Assuming max 15 items for clearing purposes
            queue!(stdout, cursor::MoveTo(PAD_LEFT, top + i))?;
            queue!(stdout, Clear(ClearType::CurrentLine))?;
        }

        if printer.items.is_empty() {
            // Display initial help message if cart is empty.
            let help_len = u16::try_from(START_HELP.len()).unwrap_or(u16::MAX);
            let start_col = PAD_LEFT + ((COLS - help_len) / 2); // Center prompt
            queue!(stdout, cursor::MoveTo(start_col, top))?;
            queue!(stdout, Print(START_HELP))?;
            execute!(stdout, EndSynchronizedUpdate)?;
            return Ok(());
        }

        // Sort items by key (UPC) for consistent display order.
        // Collect into Vec<(key, value)> to avoid holding DashMap iterators across the loop.
        let mut sorted_items: Vec<_> = printer
            .items
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        sorted_items.sort_by_key(|(key, _)| key.clone());

        // Iterate and print each item.
        for (i, (_key, display_item)) in sorted_items.iter().enumerate() {
            // Convert DisplayItem to FormattedItem for colored output.
            let formatted: FormattedItem = display_item.clone().into();
            // Estimate display length *without* ANSI codes for alignment
            let line_item_display_len = match display_item {
                DisplayItem::Item(cart_item) => format!(
                    "{}({}) @ {} | {}",
                    cart_item.name(),
                    cart_item.quantity(),
                    MoneyFmt(**cart_item.cost()),
                    MoneyFmt(cart_item.price())
                )
                .len(),
                DisplayItem::Loader(name) => {
                    format!("{}( ) @ {} | {}", name, MoneyFmt(0), MoneyFmt(0)).len()
                }
                DisplayItem::Startup => 0,
            };

            let display_len = u16::try_from(line_item_display_len).unwrap_or(u16::MAX);
            let start_col = PAD_LEFT + COLS.saturating_sub(display_len); // Align right

            let row = u16::try_from(i).unwrap_or(u16::MAX);
            queue!(stdout, cursor::MoveTo(start_col, top + row))?;
            queue!(stdout, Print(formatted.to_string()))?; // Use Print command
        }

        execute!(stdout, EndSynchronizedUpdate)?;
        Ok(())
    }

    /// Prints the subtotal, tax, and total due section.
    fn print_totals(stdout: &mut Stdout, printer: &Self) -> anyhow::Result<()> {
        // Don't print totals if there are no items.
        if printer.items.is_empty() {
            // Clear the totals area if items were removed
            let top = PAD_TOP + HEADER_HEIGHT;
            for i in 0..5 {
                // Clear separator + 3 totals lines + help line space
                queue!(stdout, cursor::MoveTo(PAD_LEFT, top + i))?;
                queue!(stdout, Clear(ClearType::CurrentLine))?;
            }
            return Ok(());
        }

        execute!(stdout, BeginSynchronizedUpdate)?;

        let items_count = u16::try_from(printer.items.len()).unwrap_or(u16::MAX);
        let top = PAD_TOP + HEADER_HEIGHT + items_count; // Position below items
                                                         // Draw separator line above totals.
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

        // Calculate totals.
        let subtotal = printer
            .items
            .iter()
            .map(|kv| match kv.value() {
                DisplayItem::Item(item) => item.price(),
                _ => 0,
            })
            .sum::<i32>();
        // Tax calculation using integer arithmetic: tax = subtotal * 7 / 100 (for 7% tax rate)
        // This avoids float truncation concerns while maintaining precision
        let tax = subtotal * 7 / 100;
        let total_due = subtotal + tax;

        // Format total strings.
        let subtotal_str = format!("{:<11}{}", SUBTOTAL_LABEL, MoneyFmt(subtotal));
        let tax_str = format!("{:<11}{}", TAX_LABEL, MoneyFmt(tax));

        // Format total due, potentially adding a checkmark if all items are loaded.
        let total_due_str = if printer.loaded {
            format!(
                "{:<11}{} {}",
                DUE_LABEL,
                RGB(CHECK_MARK_COLOR.0, CHECK_MARK_COLOR.1, CHECK_MARK_COLOR.2,)
                    .paint(MoneyFmt(total_due).to_string()),
                RGB(CHECK_MARK_COLOR.0, CHECK_MARK_COLOR.1, CHECK_MARK_COLOR.2,).paint(STARTED)
            )
        } else {
            format!(
                "{:<11}{}",
                DUE_LABEL,
                RGB(
                    TOTAL_DUE_COLOR_NOT_LOADED.0,
                    TOTAL_DUE_COLOR_NOT_LOADED.1,
                    TOTAL_DUE_COLOR_NOT_LOADED.2,
                )
                .paint(MoneyFmt(total_due).to_string())
            )
        };

        // Align totals to the right, just before the right edge (COLS)
        // Need to account for potential ANSI codes in total_due_str for alignment
        let subtotal_len = subtotal_str.chars().count(); // Length without ANSI codes
        let tax_len = tax_str.chars().count();
        let total_due_len = if printer.loaded {
            // Estimate length without ANSI codes
            format!("{:<11}{} {}", DUE_LABEL, MoneyFmt(total_due), STARTED)
                .chars()
                .count()
        } else {
            format!("{:<11}{}", DUE_LABEL, MoneyFmt(total_due))
                .chars()
                .count()
        };

        let subtotal_width = u16::try_from(subtotal_len).unwrap_or(u16::MAX);
        let tax_width = u16::try_from(tax_len).unwrap_or(u16::MAX);
        let total_due_width = u16::try_from(total_due_len).unwrap_or(u16::MAX);
        let subtotal_start_col = PAD_LEFT + COLS.saturating_sub(subtotal_width);
        let tax_start_col = PAD_LEFT + COLS.saturating_sub(tax_width);
        let total_due_start_col = PAD_LEFT + COLS.saturating_sub(total_due_width);

        // Queue commands to print totals.
        queue!(stdout, cursor::MoveTo(subtotal_start_col, top + 1))?;
        queue!(stdout, Clear(ClearType::CurrentLine))?;
        queue!(stdout, Print(&subtotal_str))?;

        queue!(stdout, cursor::MoveTo(tax_start_col, top + 2))?;
        queue!(stdout, Clear(ClearType::CurrentLine))?;
        queue!(stdout, Print(&tax_str))?;

        queue!(stdout, cursor::MoveTo(total_due_start_col, top + 3))?;
        queue!(stdout, Clear(ClearType::CurrentLine))?;
        // Use write! macro for potentially colored string
        write!(stdout, "{total_due_str}")?;

        execute!(stdout, EndSynchronizedUpdate)?;
        Ok(())
    }

    /// Prints the help text at the bottom of the screen.
    fn print_help(stdout: &mut Stdout, printer: &Self) -> anyhow::Result<()> {
        execute!(stdout, BeginSynchronizedUpdate)?;
        // Choose between short and long help text based on state.
        let help_msg = if printer.show_help {
            HELP_TEXT
        } else {
            HELP_TEXT_SHORT
        };

        // Calculate vertical position based on number of items and totals height.
        let items_count = u16::try_from(printer.items.len()).unwrap_or(u16::MAX);
        let top = PAD_TOP + HEADER_HEIGHT + items_count + 4; // Header + Items + Separator + 3 Totals lines
        let help_len = u16::try_from(help_msg.len()).unwrap_or(u16::MAX);
        let start_col = PAD_LEFT + COLS.saturating_sub(help_len); // Align right

        queue!(stdout, cursor::MoveTo(0, top))?; // Move below totals
        queue!(stdout, Clear(ClearType::FromCursorDown))?; // Clear area below totals
        queue!(stdout, cursor::MoveTo(start_col, top))?; // Position for help text

        // Print the help text with specific color.
        queue!(
            stdout,
            Print(
                RGB(COLOR_HELP_TEXT.0, COLOR_HELP_TEXT.1, COLOR_HELP_TEXT.2,)
                    .paint(help_msg)
                    .to_string()
            )
        )?;

        execute!(stdout, EndSynchronizedUpdate)?;
        Ok(())
    }
}
