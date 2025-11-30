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
use std::io::{stdout, Write};
use std::sync::Once;

use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyModifiers},
    execute, queue,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType},
};
use futures::StreamExt; // Required for reader.next().await
use tokio::sync::oneshot;
use tracing::{error, info, Level};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt::format::FmtSpan, EnvFilter, FmtSubscriber};

// Import necessary components from other modules within the example and the framework.
use acton_reactive::prelude::*;
use cart_item::CartItem;
use register::Register;

// Import the acton_message macro for message type definitions.
use acton_macro::acton_message;

// Declare the modules composing this example.
mod cart_item;
mod price_service;
mod printer;
mod register;

// Constants for error messages and configuration.
const FAILED_TO_ENABLE_RAW_MODE: &str = "Failed to enable raw mode";
const FAILED_TO_DISABLE_RAW_MODE: &str = "Failed to disable raw mode";
const ERROR_READING_EVENT: &str = "Error reading terminal event:";
const LOG_DIRECTORY: &str = "logs"; // Directory for log files.
const LOG_FILENAME: &str = "fruit_market.log"; // Base name for log files.

// --- Messages ---
// Messages are the primary way actors communicate.
// The `#[acton_message]` macro derives `Clone` and `Debug`.

/// Message sent when an item is successfully scanned (likely includes price info).
#[acton_message]
struct ItemScanned(CartItem);

/// Message sent to request the price of a specific item.
#[acton_message]
struct PriceRequest(CartItem);

/// Message containing the price response for a requested item.
#[acton_message]
struct PriceResponse {
    item: CartItem,
}

/// Message specific to controlling the Printer actor.
#[acton_message]
enum PrinterMessage {
    /// Instructs the printer to repaint the display.
    Repaint,
}

// --- Terminal Raw Mode Handling ---

/// A guard struct to ensure terminal raw mode is disabled when dropped.
/// This is crucial for restoring the terminal state even if the application panics or exits unexpectedly.
struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        // Attempt to disable raw mode when the guard goes out of scope.
        disable_raw_mode().expect(FAILED_TO_DISABLE_RAW_MODE);
    }
}

// --- Main Application Logic ---

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging first.
    initialize_tracing();
    info!("** Fruit Market Example Startup **");

    // Enable terminal raw mode to capture key presses directly without line buffering.
    enable_raw_mode().expect(FAILED_TO_ENABLE_RAW_MODE);
    // Create the guard. When `_raw_mode_guard` goes out of scope (at the end of main or on panic),
    // its `drop` implementation will disable raw mode.
    let _raw_mode_guard = RawModeGuard;
    // Get a handle to standard output.
    let mut stdout = stdout();
    // Clear the terminal screen and move the cursor to the top-left corner.
    execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0))?;

    // 1. Launch the Acton runtime environment.
    let mut runtime = ActonApp::launch();

    // 2. Create the main 'Register' actor.
    //    The `Register::new_transaction` function likely sets up the Register actor
    //    and potentially spawns its dependencies (like PriceService, Printer).
    let register_handle = Register::new_transaction(&mut runtime).await?;

    // 3. Set up graceful shutdown mechanism using a oneshot channel.
    //    The input handling task will send a signal on this channel when 'q' or Ctrl+C is pressed.
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    // 4. Spawn a separate Tokio task to handle user input events asynchronously.
    //    This prevents blocking the main thread while waiting for input.
    tokio::spawn(async move {
        // Create a stream for reading terminal events.
        let mut reader = event::EventStream::new();
        // Loop reading terminal events.
        while let Some(event_result) = reader.next().await {
            match event_result {
                // Handle key press events.
                Ok(Event::Key(key_event)) => match key_event.code {
                    // Ctrl+C for shutdown.
                    KeyCode::Char('c') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                        // Send the shutdown signal and break the loop.
                        let _ = shutdown_tx.send(());
                        break;
                    }
                    // 'q' for quit/shutdown.
                    KeyCode::Char('q') => {
                        // Send the shutdown signal and break the loop.
                        let _ = shutdown_tx.send(());
                        break;
                    }
                    // 's' to trigger scanning an item (sends message to Register actor).
                    KeyCode::Char('s') => {
                        // Use the captured register handle to interact with the actor.
                        if let Err(e) = register_handle.scan().await {
                            error!("Failed to trigger scan: {}", e);
                        }
                    }
                    // '?' to toggle help display (sends message to Register actor).
                    KeyCode::Char('?') => {
                        if let Err(e) = register_handle.toggle_help().await {
                            error!("Failed to toggle help: {}", e);
                        }
                    }
                    // Ignore other key presses.
                    _ => {}
                },
                Err(e) => {
                    // Log errors and break the loop on read error.
                    error!("{} {}", ERROR_READING_EVENT, e);
                    let _ = shutdown_tx.send(()); // Also trigger shutdown on error
                    break;
                }
                // Ignore other event types (like mouse events).
                _ => {}
            }
        }
    });

    // 5. Wait for the shutdown signal from the input handling task.
    //    The `.await` here will pause execution until the signal is received.
    let _ = shutdown_rx.await; // Result can be ignored if we just need the signal

    // 6. Clean up the terminal before exiting.
    //    Queue commands for efficiency, then flush.
    queue!(stdout, cursor::MoveTo(0, 0))?;
    queue!(stdout, Clear(ClearType::FromCursorDown))?;
    stdout.write_all(b"Shutting down...\n")?;
    queue!(stdout, cursor::Show)?; // Ensure cursor is visible
    stdout.flush()?;
    queue!(stdout, cursor::MoveTo(0, 1))?; // Move cursor down one line

    // 7. Gracefully shut down all actors managed by the runtime.
    runtime.shutdown_all().await?;
    info!("Shutdown complete.");

    Ok(())
}

// --- Tracing Initialization ---

// Ensures tracing initialization happens only once.
static INIT: Once = Once::new();

/// Initializes the global tracing subscriber for the `fruit_market` example.
/// Configures filtering and formats logs to a rolling file.
pub fn initialize_tracing() {
    INIT.call_once(|| {
        // Configure log filtering using environment variables or directives.
        let filter = EnvFilter::new("")
            // Set specific levels for modules within this example.
            .add_directive("fruit_market=debug".parse().unwrap()) // Log debug for this example
            .add_directive("fruit_market::printer=debug".parse().unwrap())
            // Silence potentially noisy core components unless needed for debugging.
            .add_directive("acton_reactive::common::actor_handle=off".parse().unwrap())
            .add_directive("acton_reactive::common::actor_broker=off".parse().unwrap())
            .add_directive(
                "acton_reactive::actor::managed_actor::idle[start]=off"
                    .parse()
                    .unwrap(),
            )
            .add_directive(
                "acton_reactive::actor::managed_actor::started[wake]=off"
                    .parse()
                    .unwrap(),
            )
            .add_directive(
                "acton_reactive::traits::actor[send_message]=off"
                    .parse()
                    .unwrap(),
            )
            // Silence test modules if running the example directly.
            .add_directive("supervisor_tests=off".parse().unwrap())
            .add_directive("broker_tests=off".parse().unwrap())
            .add_directive("launchpad_tests=off".parse().unwrap())
            .add_directive("lifecycle_tests=off".parse().unwrap())
            .add_directive("actor_tests=off".parse().unwrap())
            .add_directive("load_balancer_tests=off".parse().unwrap())
            .add_directive("acton::tests::setup::actor::pool_item=off".parse().unwrap());

        // Set up a rolling file appender to write logs to files in the LOG_DIRECTORY.
        // Rotates daily.
        let file_appender = RollingFileAppender::new(Rotation::DAILY, LOG_DIRECTORY, LOG_FILENAME);

        // Build the subscriber.
        let subscriber = FmtSubscriber::builder()
            .with_span_events(FmtSpan::NONE) // Don't log span entry/exit.
            .with_max_level(Level::TRACE) // Process all levels up to TRACE.
            .compact() // Use a more compact output format.
            .with_line_number(false) // Don't include line numbers.
            .with_target(false) // Don't include the module path target.
            .without_time() // Don't include timestamps.
            .with_env_filter(filter) // Apply the filter defined above.
            .with_writer(file_appender) // Write logs to the rolling file.
            .finish();

        // Set the subscriber as the global default.
        tracing_subscriber::util::SubscriberInitExt::init(subscriber);
    });
}
