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

//! IPC Multi-Agent Example
//!
//! This example demonstrates exposing multiple agents via IPC, each providing
//! a different service. It showcases:
//!
//! - Multiple agents with different responsibilities
//! - IPC routing to different services by logical name
//! - Listener statistics monitoring
//! - Graceful shutdown with Ctrl+C handling
//!
//! # Services Provided
//!
//! - **counter**: A simple counter that tracks increments
//! - **logger**: A logging service that accepts log messages
//! - **config**: A configuration store for key-value pairs
//!
//! # Running This Example
//!
//! ```bash
//! cargo run --example ipc_multi_agent --features ipc
//! ```
//!
//! Then connect with the `ipc_client` example:
//! ```bash
//! cargo run --example ipc_client --features ipc
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use acton_macro::acton_actor;
use acton_reactive::ipc::{socket_exists, IpcConfig, IpcListenerStats};
use acton_reactive::prelude::*;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

// ============================================================================
// Message Definitions
// ============================================================================

/// Increment the counter by an amount.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Increment {
    amount: i64,
}

/// Decrement the counter by an amount.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Decrement {
    amount: i64,
}

/// Log a message at a specified level.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct LogMessage {
    level: String,
    message: String,
}

/// Set a configuration value.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SetConfig {
    key: String,
    value: String,
}

/// Get a configuration value.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct GetConfig {
    key: String,
}

/// A message to print text to the console.
#[derive(Clone, Debug)]
struct Print(String);

/// A message to print a section header.
#[derive(Clone, Debug)]
struct PrintSection(String);

// ============================================================================
// Agent States
// ============================================================================

/// Console printer agent state for coordinated output.
#[acton_actor]
struct PrinterState;

/// Counter service agent state.
#[acton_actor]
struct CounterState {
    value: i64,
    operations: usize,
}

/// Logger service agent state.
#[acton_actor]
struct LoggerState {
    entries: Vec<(String, String)>,
}

/// Config service agent state.
#[acton_actor]
struct ConfigState {
    values: HashMap<String, String>,
}

// ============================================================================
// Agent Creation Functions
// ============================================================================

/// Creates the printer agent for coordinated console output.
async fn create_printer_agent(runtime: &mut AgentRuntime) -> AgentHandle {
    let mut printer = runtime.new_agent::<PrinterState>();
    printer
        .act_on::<Print>(|_agent, envelope| {
            println!("{}", envelope.message().0);
            AgentReply::immediate()
        })
        .act_on::<PrintSection>(|_agent, envelope| {
            println!("\n=== {} ===\n", envelope.message().0);
            AgentReply::immediate()
        });
    printer.start().await
}

/// Creates the counter service agent.
async fn create_counter_agent(
    runtime: &mut AgentRuntime,
    printer: &AgentHandle,
) -> AgentHandle {
    let mut counter = runtime.new_agent_with_name::<CounterState>("counter".to_string());
    let printer_clone = printer.clone();

    counter
        .mutate_on::<Increment>(move |agent, envelope| {
            let amount = envelope.message().amount;
            agent.model.value += amount;
            agent.model.operations += 1;
            let p = printer_clone.clone();
            let value = agent.model.value;
            let ops = agent.model.operations;

            Box::pin(async move {
                p.send(Print(format!(
                    "[Counter] Incremented by {amount} -> value={value}, ops={ops}"
                )))
                .await;
            })
        })
        .mutate_on::<Decrement>({
            let p = printer.clone();
            move |agent, envelope| {
                let amount = envelope.message().amount;
                agent.model.value -= amount;
                agent.model.operations += 1;
                let p = p.clone();
                let value = agent.model.value;
                let ops = agent.model.operations;

                Box::pin(async move {
                    p.send(Print(format!(
                        "[Counter] Decremented by {amount} -> value={value}, ops={ops}"
                    )))
                    .await;
                })
            }
        })
        .after_stop({
            let p = printer.clone();
            move |agent| {
                let p = p.clone();
                let value = agent.model.value;
                let ops = agent.model.operations;

                Box::pin(async move {
                    p.send(Print(format!(
                        "[Counter] Shutdown - final value={value}, total ops={ops}"
                    )))
                    .await;
                })
            }
        });

    counter.start().await
}

/// Creates the logger service agent.
async fn create_logger_agent(
    runtime: &mut AgentRuntime,
    printer: &AgentHandle,
) -> AgentHandle {
    let mut logger = runtime.new_agent_with_name::<LoggerState>("logger".to_string());

    logger
        .mutate_on::<LogMessage>({
            let p = printer.clone();
            move |agent, envelope| {
                let level = envelope.message().level.clone();
                let message = envelope.message().message.clone();
                agent.model.entries.push((level.clone(), message.clone()));
                let p = p.clone();
                let count = agent.model.entries.len();

                Box::pin(async move {
                    p.send(Print(format!(
                        "[Logger] [{level}] {message} (entry #{count})"
                    )))
                    .await;
                })
            }
        })
        .after_stop({
            let p = printer.clone();
            move |agent| {
                let p = p.clone();
                let entries = agent.model.entries.clone();

                Box::pin(async move {
                    p.send(Print(format!(
                        "[Logger] Shutdown - {} total entries logged",
                        entries.len()
                    )))
                    .await;
                })
            }
        });

    logger.start().await
}

/// Creates the config service agent.
async fn create_config_agent(
    runtime: &mut AgentRuntime,
    printer: &AgentHandle,
) -> AgentHandle {
    let mut config = runtime.new_agent_with_name::<ConfigState>("config".to_string());

    config
        .mutate_on::<SetConfig>({
            let p = printer.clone();
            move |agent, envelope| {
                let key = envelope.message().key.clone();
                let value = envelope.message().value.clone();
                agent.model.values.insert(key.clone(), value.clone());
                let p = p.clone();
                let count = agent.model.values.len();

                Box::pin(async move {
                    p.send(Print(format!(
                        "[Config] Set {key}={value} ({count} total keys)"
                    )))
                    .await;
                })
            }
        })
        .mutate_on::<GetConfig>({
            let p = printer.clone();
            move |agent, envelope| {
                let key = envelope.message().key.clone();
                let value = agent.model.values.get(&key).cloned();
                let p = p.clone();

                Box::pin(async move {
                    match value {
                        Some(v) => {
                            p.send(Print(format!("[Config] Get {key}={v}"))).await;
                        }
                        None => {
                            p.send(Print(format!("[Config] Get {key}=<not found>"))).await;
                        }
                    }
                })
            }
        })
        .after_stop({
            let p = printer.clone();
            move |agent| {
                let p = p.clone();
                let values = agent.model.values.clone();

                Box::pin(async move {
                    p.send(Print(format!(
                        "[Config] Shutdown - {} keys stored",
                        values.len()
                    )))
                    .await;
                    for (k, v) in &values {
                        p.send(Print(format!("  {k}={v}"))).await;
                    }
                })
            }
        });

    config.start().await
}

/// Periodically displays listener statistics.
async fn display_stats_loop(
    stats: Arc<IpcListenerStats>,
    printer: AgentHandle,
    shutdown: Arc<Notify>,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.tick().await; // Skip first immediate tick

    loop {
        tokio::select! {
            () = shutdown.notified() => break,
            _ = interval.tick() => {
                printer.send(Print(format!(
                    "[Stats] Connections: {} accepted, {} active | Messages: {} received, {} routed | Errors: {}",
                    stats.connections_accepted(),
                    stats.connections_active(),
                    stats.messages_received(),
                    stats.messages_routed(),
                    stats.errors()
                ))).await;
            }
        }
    }
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut runtime = ActonApp::launch();

    // Create printer for coordinated output
    let printer = create_printer_agent(&mut runtime).await;
    printer.send(PrintSection("IPC Multi-Agent Server".to_string())).await;

    // Register all IPC message types
    let registry = runtime.ipc_registry();
    registry.register::<Increment>("Increment");
    registry.register::<Decrement>("Decrement");
    registry.register::<LogMessage>("LogMessage");
    registry.register::<SetConfig>("SetConfig");
    registry.register::<GetConfig>("GetConfig");

    printer
        .send(Print(format!("Registered {} IPC message types", registry.len())))
        .await;

    // Create service agents
    let counter = create_counter_agent(&mut runtime, &printer).await;
    let logger = create_logger_agent(&mut runtime, &printer).await;
    let config = create_config_agent(&mut runtime, &printer).await;

    // Expose agents for IPC
    runtime.ipc_expose("counter", counter.clone());
    runtime.ipc_expose("logger", logger.clone());
    runtime.ipc_expose("config", config.clone());

    printer
        .send(Print(format!(
            "Exposed {} agents for IPC: counter, logger, config",
            runtime.ipc_agent_count()
        )))
        .await;

    // Start the IPC listener
    let ipc_config = IpcConfig::load();
    let socket_path = ipc_config.socket_path();
    printer
        .send(Print(format!("Socket path: {}", socket_path.display())))
        .await;

    let listener_handle = runtime.start_ipc_listener().await?;
    printer.send(Print("IPC listener started!".to_string())).await;

    // Verify socket is ready
    tokio::time::sleep(Duration::from_millis(50)).await;
    if socket_exists(&socket_path) {
        printer
            .send(Print("Socket is ready for connections".to_string()))
            .await;
    }

    printer
        .send(Print("\nPress Ctrl+C to shutdown...".to_string()))
        .await;

    // Set up shutdown notification
    let shutdown = Arc::new(Notify::new());
    let shutdown_clone = shutdown.clone();

    // Start stats display task
    let stats_clone = listener_handle.stats.clone();
    let stats_printer = printer.clone();
    let stats_shutdown = shutdown.clone();
    tokio::spawn(async move {
        display_stats_loop(stats_clone, stats_printer, stats_shutdown).await;
    });

    // Wait for Ctrl+C
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            printer.send(PrintSection("Shutdown Initiated".to_string())).await;
        }
    }

    // Signal shutdown to stats loop
    shutdown_clone.notify_waiters();

    // Display final stats
    let stats = &listener_handle.stats;
    printer
        .send(Print(format!(
            "\nFinal Statistics:\n  Connections accepted: {}\n  Messages received: {}\n  Messages routed: {}\n  Errors: {}",
            stats.connections_accepted(),
            stats.messages_received(),
            stats.messages_routed(),
            stats.errors()
        )))
        .await;

    // Stop the listener
    listener_handle.stop();
    printer.send(Print("IPC listener stopped".to_string())).await;

    // Allow time for async cleanup
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Shutdown the runtime
    runtime.shutdown_all().await?;
    println!("\n=== Server Shutdown Complete ===");

    Ok(())
}
