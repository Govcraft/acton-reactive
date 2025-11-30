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

//! IPC Streaming Response Example (Server)
//!
//! This example demonstrates the request-stream pattern in acton-reactive IPC,
//! where a single request triggers multiple streaming responses from an agent.
//!
//! # Features
//!
//! - **Request-Stream Pattern**: Client sends one request, receives multiple
//!   response frames over time.
//! - **Countdown Service**: Streams countdown numbers with delays.
//! - **Paginated Results**: Streams items from a collection page by page.
//!
//! # How It Works
//!
//! 1. IPC client sends `IpcEnvelope` with `expects_stream: true`
//! 2. IPC listener creates a temporary MPSC channel (proxy)
//! 3. Message is sent to agent with proxy as `reply_to` address
//! 4. Agent handler sends multiple responses via `envelope.reply_envelope().send()`
//! 5. Listener receives each response and serializes it as an `IpcStreamFrame`
//! 6. When the agent finishes, listener sends a final frame with `is_final: true`
//!
//! # Running This Example
//!
//! Start the server:
//! ```bash
//! cargo run --example ipc_streaming_server --features ipc
//! ```
//!
//! Then connect with the client (in another terminal):
//! ```bash
//! cargo run --example ipc_streaming_client --features ipc
//! ```

use std::time::Duration;

use acton_macro::acton_actor;
use acton_reactive::ipc::{socket_exists, IpcConfig};
use acton_reactive::prelude::*;
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

// ============================================================================
// Message Definitions - Countdown Service
// ============================================================================

/// Request to start a countdown from a given number.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CountdownRequest {
    /// Starting number for the countdown.
    start: u32,
    /// Delay between each number in milliseconds.
    delay_ms: u64,
}

/// A single countdown tick sent as a streaming response.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CountdownTick {
    /// Current number in the countdown.
    number: u32,
    /// Whether this is the final tick (number == 0).
    is_final: bool,
}

// ============================================================================
// Message Definitions - Paginated List Service
// ============================================================================

/// Request to list items with pagination.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ListItemsRequest {
    /// Number of items per page.
    page_size: usize,
}

/// A page of items sent as a streaming response.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ItemPage {
    /// Page number (1-indexed).
    page: usize,
    /// Items on this page.
    items: Vec<String>,
    /// Whether there are more pages.
    has_more: bool,
}

// ============================================================================
// Agent States
// ============================================================================

/// Countdown service - streams countdown numbers.
#[acton_actor]
struct CountdownState {
    countdowns_started: usize,
}

/// List service - streams paginated results from a collection.
#[acton_actor]
#[derive(Default)]
struct ListServiceState {
    #[allow(dead_code)]
    request_count: usize,
}

/// Sample items for the list service.
const SAMPLE_ITEMS: &[&str] = &[
    "Apple",
    "Banana",
    "Cherry",
    "Date",
    "Elderberry",
    "Fig",
    "Grape",
    "Honeydew",
    "Kiwi",
    "Lemon",
];

// ============================================================================
// Agent Creation Functions
// ============================================================================

/// Creates the countdown service agent that streams countdown ticks.
async fn create_countdown_agent(runtime: &mut AgentRuntime) -> AgentHandle {
    let mut countdown = runtime.new_agent_with_name::<CountdownState>("countdown".to_string());

    // Handle countdown requests - send multiple responses over time
    countdown.mutate_on::<CountdownRequest>(|agent, envelope| {
        let msg = envelope.message();
        let start = msg.start;
        let delay_ms = msg.delay_ms;
        agent.model.countdowns_started += 1;

        println!(
            "  [Countdown] Starting countdown from {} with {}ms delay (#{})...",
            start, delay_ms, agent.model.countdowns_started
        );

        // Get the reply envelope to send multiple responses
        let reply_envelope = envelope.reply_envelope();

        Box::pin(async move {
            for i in (0..=start).rev() {
                let tick = CountdownTick {
                    number: i,
                    is_final: i == 0,
                };
                println!("  [Countdown] Sending tick: {i}");
                reply_envelope.send(tick).await;

                if i > 0 {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            }
            println!("  [Countdown] Stream complete");
        })
    });

    countdown.start().await
}

/// Creates the list service agent that streams paginated items.
async fn create_list_agent(runtime: &mut AgentRuntime) -> AgentHandle {
    let mut list_service = runtime.new_agent_with_name::<ListServiceState>("list_service".to_string());

    // Handle list requests - stream pages of items
    list_service.act_on::<ListItemsRequest>(|_agent, envelope| {
        let msg = envelope.message();
        let page_size = msg.page_size.max(1); // At least 1 item per page

        // Use the constant sample items
        let items: Vec<String> = SAMPLE_ITEMS.iter().map(|s| (*s).to_string()).collect();

        println!(
            "  [ListService] Streaming {} items in pages of {}...",
            items.len(),
            page_size
        );

        let reply_envelope = envelope.reply_envelope();

        Box::pin(async move {
            let chunks: Vec<_> = items.chunks(page_size).collect();
            let total_pages = chunks.len();

            for (idx, chunk) in chunks.iter().enumerate() {
                let page_num = idx + 1;
                let page = ItemPage {
                    page: page_num,
                    items: chunk.to_vec(),
                    has_more: page_num < total_pages,
                };

                println!(
                    "  [ListService] Sending page {}/{} with {} items",
                    page_num,
                    total_pages,
                    page.items.len()
                );
                reply_envelope.send(page).await;

                // Small delay between pages to demonstrate streaming
                if page_num < total_pages {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
            println!("  [ListService] Stream complete");
        })
    });

    list_service.start().await
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("acton=info".parse()?))
        .init();

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║       IPC Streaming Response Example (Server)                ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    let mut runtime = ActonApp::launch();

    // Register all IPC message types (both requests and responses)
    let registry = runtime.ipc_registry();

    // Countdown messages
    registry.register::<CountdownRequest>("CountdownRequest");
    registry.register::<CountdownTick>("CountdownTick");

    // List service messages
    registry.register::<ListItemsRequest>("ListItemsRequest");
    registry.register::<ItemPage>("ItemPage");

    println!("📝 Registered {} IPC message types", registry.len());

    // Create service agents
    let countdown = create_countdown_agent(&mut runtime).await;
    println!("⏱️  Countdown service started");

    let list_service = create_list_agent(&mut runtime).await;
    println!("📋 List service started");

    // Expose agents for IPC access
    runtime.ipc_expose("countdown", countdown.clone());
    runtime.ipc_expose("list_service", list_service.clone());
    println!("🔗 Exposed agents: countdown, list_service");

    // Start the IPC listener
    let ipc_config = IpcConfig::load();
    let socket_path = ipc_config.socket_path();

    let listener_handle = runtime.start_ipc_listener().await?;
    println!("🚀 IPC listener started");

    // Verify socket is ready
    tokio::time::sleep(Duration::from_millis(50)).await;
    if socket_exists(&socket_path) {
        println!("📡 Socket ready: {}", socket_path.display());
    }

    println!();
    println!("════════════════════════════════════════════════════════════════");
    println!("  Server is ready for streaming IPC communication!");
    println!("  Run the client example in another terminal:");
    println!("  cargo run --example ipc_streaming_client --features ipc");
    println!("════════════════════════════════════════════════════════════════");
    println!();
    println!("Press Ctrl+C to shutdown...");
    println!();

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;

    println!();
    println!("Shutting down...");

    // Stop the listener
    listener_handle.stop();

    // Brief delay for cleanup
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Shutdown the runtime
    runtime.shutdown_all().await?;

    println!("Server shutdown complete.");

    Ok(())
}
