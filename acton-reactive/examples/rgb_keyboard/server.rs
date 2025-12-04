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

//! RGB Keyboard Example - Server
//!
//! Central hub that coordinates keystroke processing and color generation.
//!
//! # Features
//!
//! - **Keystroke Processing**: Receives keystrokes from input client
//! - **Color Request Broadcasting**: Broadcasts to R/G/B color clients
//! - **Response Correlation**: Collects and correlates color responses by ID
//! - **Colored Output Broadcasting**: Sends completed colored characters to output
//!
//! # Running This Example
//!
//! ```bash
//! cargo run --example rgb_keyboard_server --features ipc
//! ```

use std::collections::HashMap;
use std::time::{Duration, Instant};

use acton_reactive::ipc::{socket_exists, IpcConfig};
use acton_reactive::prelude::*;
use mti::prelude::*;
use tracing_subscriber::EnvFilter;

// ============================================================================
// Message Definitions
// ============================================================================

/// A keystroke received from the input client.
#[acton_message(ipc)]
struct Keystroke {
    /// The character that was typed.
    character: char,
}

/// Request sent to color clients to generate a color component.
/// Broadcast to all R/G/B clients via push notification.
#[acton_message(ipc)]
struct ColorRequest {
    /// Correlation ID to track which keystroke this is for.
    correlation_id: String,
    /// The character being colored.
    character: char,
}

/// Response from a color client with their component value.
#[acton_message(ipc)]
struct ColorResponse {
    /// Correlation ID matching the original request.
    correlation_id: String,
    /// Which component this is: "R", "G", or "B".
    component: String,
    /// The random value (0-255) for this component.
    value: u8,
}

/// A completed colored character, broadcast to output client.
#[acton_message(ipc)]
struct ColoredCharacter {
    /// The original character.
    character: char,
    /// Red component (0-255).
    red: u8,
    /// Green component (0-255).
    green: u8,
    /// Blue component (0-255).
    blue: u8,
}

// ============================================================================
// Internal Messages
// ============================================================================

/// Internal message to clean up expired pending requests.
#[acton_message]
#[derive(Default)]
struct CleanupExpired;

// ============================================================================
// Actor State
// ============================================================================

/// Tracks a pending color request awaiting R/G/B responses.
#[derive(Debug, Clone)]
struct PendingColorRequest {
    /// The original character being colored.
    character: char,
    /// Red component (if received).
    red: Option<u8>,
    /// Green component (if received).
    green: Option<u8>,
    /// Blue component (if received).
    blue: Option<u8>,
    /// When this request was created.
    created_at: Instant,
}

impl PendingColorRequest {
    fn new(character: char) -> Self {
        Self {
            character,
            red: None,
            green: None,
            blue: None,
            created_at: Instant::now(),
        }
    }

    const fn is_complete(&self) -> bool {
        self.red.is_some() && self.green.is_some() && self.blue.is_some()
    }

    fn is_expired(&self, timeout: Duration) -> bool {
        self.created_at.elapsed() > timeout
    }
}

/// Keystroke processor actor state.
#[derive(Debug, Clone, Default)]
struct KeystrokeProcessorState {
    /// Pending color requests indexed by correlation ID.
    pending: HashMap<String, PendingColorRequest>,
    /// Count of keystrokes processed.
    keystroke_count: u64,
    /// Count of completed colored characters.
    completed_count: u64,
}

// ============================================================================
// Constants
// ============================================================================

/// Timeout for pending requests (5 seconds).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Interval for cleanup task (1 second).
const CLEANUP_INTERVAL: Duration = Duration::from_secs(1);

// ============================================================================
// Actor Creation
// ============================================================================

/// Creates the keystroke processor actor.
async fn create_keystroke_processor(runtime: &mut ActorRuntime) -> ActorHandle {
    let mut processor =
        runtime.new_actor_with_name::<KeystrokeProcessorState>("keystroke_processor".to_string());

    // Handle incoming keystrokes from input client
    processor.mutate_on::<Keystroke>(|actor, envelope| {
        let character = envelope.message().character;
        actor.model.keystroke_count += 1;

        // Generate correlation ID for this keystroke
        let correlation_id = "color".create_type_id::<V7>().to_string();

        let count = actor.model.keystroke_count;
        println!("  [Processor] Received keystroke '{character}' (#{count}) -> {correlation_id}");

        // Track pending request
        actor
            .model
            .pending
            .insert(correlation_id.clone(), PendingColorRequest::new(character));

        // Broadcast color request to all color clients
        let broker = actor.broker().clone();
        Reply::pending(async move {
            broker
                .broadcast(ColorRequest {
                    correlation_id,
                    character,
                })
                .await;
        })
    });

    // Handle color responses from R/G/B clients
    processor.mutate_on::<ColorResponse>(|actor, envelope| {
        let msg = envelope.message();
        let correlation_id = msg.correlation_id.clone();
        let component = msg.component.clone();
        let value = msg.value;

        println!("  [Processor] Received {component} = {value} for {correlation_id}");

        if let Some(pending) = actor.model.pending.get_mut(&correlation_id) {
            // Store the color component
            match component.as_str() {
                "R" => pending.red = Some(value),
                "G" => pending.green = Some(value),
                "B" => pending.blue = Some(value),
                _ => {
                    println!("  [Processor] Unknown component: {component}");
                    return Reply::ready();
                }
            }

            // Check if we have all three components
            if pending.is_complete() {
                let character = pending.character;
                let red = pending.red.unwrap();
                let green = pending.green.unwrap();
                let blue = pending.blue.unwrap();

                actor.model.pending.remove(&correlation_id);
                actor.model.completed_count += 1;

                println!(
                    "  [Processor] Complete! '{}' -> RGB({}, {}, {}) [#{}]",
                    character, red, green, blue, actor.model.completed_count
                );

                // Broadcast completed colored character
                let broker = actor.broker().clone();
                return Reply::pending(async move {
                    broker
                        .broadcast(ColoredCharacter {
                            character,
                            red,
                            green,
                            blue,
                        })
                        .await;
                });
            }
        } else {
            println!("  [Processor] No pending request for correlation_id: {correlation_id}");
        }

        Reply::ready()
    });

    // Handle cleanup of expired requests
    processor.mutate_on::<CleanupExpired>(|actor, _envelope| {
        let before = actor.model.pending.len();
        actor
            .model
            .pending
            .retain(|_id, req| !req.is_expired(REQUEST_TIMEOUT));
        let after = actor.model.pending.len();

        if before != after {
            let cleaned = before - after;
            println!("  [Processor] Cleaned up {cleaned} expired requests ({after} remaining)");
        }

        Reply::ready()
    });

    processor.start().await
}

// ============================================================================
// Main
// ============================================================================

#[acton_main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("acton=info".parse()?))
        .init();

    println!("====================================================================");
    println!("       RGB Keyboard Example (Server)");
    println!("====================================================================");
    println!();

    let mut runtime = ActonApp::launch_async().await;

    // Register IPC message types
    let registry = runtime.ipc_registry();

    // Input messages
    registry.register::<Keystroke>("Keystroke");

    // Color coordination messages
    registry.register::<ColorRequest>("ColorRequest");
    registry.register::<ColorResponse>("ColorResponse");

    // Output messages
    registry.register::<ColoredCharacter>("ColoredCharacter");

    println!("Registered {} IPC message types:", registry.len());
    println!("  - Keystroke: Input from keyboard client");
    println!("  - ColorRequest: Broadcast to R/G/B clients");
    println!("  - ColorResponse: Responses from R/G/B clients");
    println!("  - ColoredCharacter: Broadcast to output client");

    // Create the keystroke processor actor
    let processor = create_keystroke_processor(&mut runtime).await;
    println!("\nKeystroke processor started");

    // Expose actor for IPC access
    runtime.ipc_expose("keystroke_processor", processor.clone());
    println!("Exposed actors: keystroke_processor");

    // Start the IPC listener
    let ipc_config = IpcConfig::load();
    let socket_path = ipc_config.socket_path();

    let listener_handle = runtime.start_ipc_listener().await?;
    println!("IPC listener started");

    // Verify socket is ready
    tokio::time::sleep(Duration::from_millis(50)).await;
    if socket_exists(&socket_path) {
        println!("Socket ready: {}", socket_path.display());
    }

    println!();
    println!("====================================================================");
    println!("  RGB Keyboard Server is ready!");
    println!();
    println!("  Start the clients in separate terminals:");
    println!("    1. cargo run --example rgb_keyboard_color_client --features ipc -- --component R");
    println!("    2. cargo run --example rgb_keyboard_color_client --features ipc -- --component G");
    println!("    3. cargo run --example rgb_keyboard_color_client --features ipc -- --component B");
    println!("    4. cargo run --example rgb_keyboard_output_client --features ipc");
    println!("    5. cargo run --example rgb_keyboard_input_client --features ipc");
    println!("====================================================================");
    println!();
    println!("Press Ctrl+C to shutdown...");
    println!();

    // Start cleanup task
    let processor_handle = processor.clone();
    let cleanup_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(CLEANUP_INTERVAL);
        loop {
            interval.tick().await;
            processor_handle.send(CleanupExpired).await;
        }
    });

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;

    println!();
    println!("Shutting down...");

    // Cancel cleanup task
    cleanup_task.abort();

    // Stop the listener
    listener_handle.stop();

    // Brief delay for cleanup
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Shutdown the runtime
    runtime.shutdown_all().await?;

    println!("Server shutdown complete.");

    Ok(())
}
