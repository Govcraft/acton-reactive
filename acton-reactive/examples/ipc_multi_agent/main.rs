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

//! IPC Multi-Agent Example with Dashboard UI
//!
//! This example demonstrates exposing multiple agents via IPC, each providing
//! a different service, with a real-time dashboard that updates in place.
//!
//! # Features
//!
//! - Multiple agents with different responsibilities
//! - IPC routing to different services by logical name
//! - Real-time dashboard with in-place updates
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
use std::io::{stdout, Write};
use std::sync::Arc;
use std::time::Duration;

use acton_macro::acton_actor;
use acton_reactive::ipc::{socket_exists, IpcConfig, IpcListenerStats};
use acton_reactive::prelude::*;
use crossterm::{
    cursor::{Hide, Show},
    event::{Event, EventStream, KeyCode, KeyModifiers},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

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

// ============================================================================
// Dashboard State
// ============================================================================

/// State for a single service displayed in the dashboard.
#[derive(Clone, Debug, Default)]
struct ServiceState {
    name: String,
    running: bool,
    col1: String,
    col2: String,
}

/// Activity log entry.
#[derive(Clone, Debug)]
struct ActivityEntry {
    timestamp: String,
    message: String,
}

/// Complete dashboard state synchronized via watch channel.
#[derive(Clone, Debug, Default)]
struct DashboardState {
    services: Vec<ServiceState>,
    connections_active: usize,
    connections_total: usize,
    messages_received: usize,
    messages_routed: usize,
    errors: usize,
    activity_log: Vec<ActivityEntry>,
    socket_path: String,
}

impl DashboardState {
    fn new() -> Self {
        Self {
            services: vec![
                ServiceState {
                    name: "Counter".to_string(),
                    running: true,
                    col1: "0 value".to_string(),
                    col2: "0 ops".to_string(),
                },
                ServiceState {
                    name: "Logger".to_string(),
                    running: true,
                    col1: "0 entries".to_string(),
                    col2: String::new(),
                },
                ServiceState {
                    name: "Config".to_string(),
                    running: true,
                    col1: "0 keys".to_string(),
                    col2: String::new(),
                },
            ],
            ..Default::default()
        }
    }

    fn add_activity(&mut self, message: String) {
        let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
        self.activity_log.push(ActivityEntry { timestamp, message });
        // Keep only last 10 entries
        if self.activity_log.len() > 10 {
            self.activity_log.remove(0);
        }
    }

    fn update_counter(&mut self, value: i64, ops: usize) {
        if let Some(service) = self.services.iter_mut().find(|s| s.name == "Counter") {
            service.col1 = format!("{value} value");
            service.col2 = format!("{ops} ops");
        }
    }

    fn update_logger(&mut self, entries: usize) {
        if let Some(service) = self.services.iter_mut().find(|s| s.name == "Logger") {
            service.col1 = format!("{entries} entries");
        }
    }

    fn update_config(&mut self, keys: usize) {
        if let Some(service) = self.services.iter_mut().find(|s| s.name == "Config") {
            service.col1 = format!("{keys} keys");
        }
    }
}

// ============================================================================
// Dashboard UI
// ============================================================================

/// Render helper functions for dashboard sections.
mod render {
    use crossterm::{
        cursor::MoveTo,
        execute,
        style::{Color, Print, ResetColor, SetForegroundColor},
    };

    use super::{ActivityEntry, DashboardState, ServiceState};

    pub fn header(stdout: &mut std::io::Stdout, socket_path: &str) -> std::io::Result<()> {
        execute!(
            stdout,
            MoveTo(0, 0),
            SetForegroundColor(Color::Cyan),
            Print("═══════════════════════════════════════════════════════════\r\n"),
            Print("  IPC Multi-Agent Server Dashboard\r\n"),
            Print("═══════════════════════════════════════════════════════════\r\n"),
            ResetColor,
            SetForegroundColor(Color::DarkGrey),
            Print(format!("  Socket: {socket_path}\r\n\r\n")),
            ResetColor
        )
    }

    pub fn services(
        stdout: &mut std::io::Stdout,
        services: &[ServiceState],
    ) -> std::io::Result<()> {
        execute!(
            stdout,
            SetForegroundColor(Color::Yellow),
            Print("┌─ Services ─────────────────────────────────────────────────┐\r\n"),
            ResetColor
        )?;

        for service in services {
            let (status_color, status_char) = if service.running {
                (Color::Green, "●")
            } else {
                (Color::Red, "○")
            };
            execute!(
                stdout,
                Print("│"),
                SetForegroundColor(status_color),
                Print(format!("{:<8}{:<2}", "", status_char)),
                ResetColor,
                Print(format!(
                    "{:<17}{:<17}{:<16}",
                    service.name, service.col1, service.col2
                )),
                Print("│\r\n")
            )?;
        }

        execute!(
            stdout,
            SetForegroundColor(Color::Yellow),
            Print("└────────────────────────────────────────────────────────────┘\r\n\r\n"),
            ResetColor
        )
    }

    pub fn statistics(stdout: &mut std::io::Stdout, state: &DashboardState) -> std::io::Result<()> {
        let row1 = format!(
            " {:>8} active    {:>8} total",
            state.connections_active, state.connections_total
        );
        let row2 = format!(
            " {:>8} received  {:>8} routed  {:>8} errors",
            state.messages_received, state.messages_routed, state.errors
        );
        execute!(
            stdout,
            SetForegroundColor(Color::Blue),
            Print("┌─ IPC Statistics ───────────────────────────────────────────┐\r\n"),
            ResetColor,
            Print(format!("│{row1:<60}│\r\n")),
            Print(format!("│{row2:<60}│\r\n")),
            SetForegroundColor(Color::Blue),
            Print("└────────────────────────────────────────────────────────────┘\r\n\r\n"),
            ResetColor
        )
    }

    pub fn activity_log(
        stdout: &mut std::io::Stdout,
        log: &[ActivityEntry],
    ) -> std::io::Result<()> {
        execute!(
            stdout,
            SetForegroundColor(Color::Magenta),
            Print("┌─ Activity Log ─────────────────────────────────────────────┐\r\n"),
            ResetColor
        )?;

        for i in 0..8 {
            if let Some(entry) = log.get(log.len().saturating_sub(8) + i) {
                let truncated = if entry.message.len() > 48 {
                    format!("{}...", &entry.message[..45])
                } else {
                    entry.message.clone()
                };
                execute!(
                    stdout,
                    Print("│ "),
                    SetForegroundColor(Color::DarkGrey),
                    Print(format!("[{}] ", entry.timestamp)),
                    ResetColor,
                    Print(format!("{truncated:<48}")),
                    Print("│\r\n")
                )?;
            } else {
                execute!(
                    stdout,
                    Print("│                                                            │\r\n")
                )?;
            }
        }

        execute!(
            stdout,
            SetForegroundColor(Color::Magenta),
            Print("└────────────────────────────────────────────────────────────┘\r\n\r\n"),
            ResetColor
        )
    }

    pub fn footer(stdout: &mut std::io::Stdout) -> std::io::Result<()> {
        execute!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print("  Press Ctrl+C to shutdown gracefully\r\n"),
            ResetColor
        )
    }
}

/// Dashboard manages the terminal UI with alternate screen buffer.
struct Dashboard {
    state_rx: watch::Receiver<DashboardState>,
}

impl Dashboard {
    const fn new(state_rx: watch::Receiver<DashboardState>) -> Self {
        Self { state_rx }
    }

    /// Enter alternate screen and enable raw mode.
    fn enter() -> std::io::Result<()> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen, Hide, Clear(ClearType::All))?;
        Ok(())
    }

    /// Render the dashboard to the terminal.
    fn render(state: &DashboardState) -> std::io::Result<()> {
        let mut stdout = stdout();
        render::header(&mut stdout, &state.socket_path)?;
        render::services(&mut stdout, &state.services)?;
        render::statistics(&mut stdout, state)?;
        render::activity_log(&mut stdout, &state.activity_log)?;
        render::footer(&mut stdout)?;
        stdout.flush()
    }

    /// Run the render loop until shutdown.
    async fn run(&self, mut shutdown_rx: watch::Receiver<bool>) {
        let mut event_stream = EventStream::new();
        let mut render_interval = tokio::time::interval(Duration::from_millis(100));

        loop {
            tokio::select! {
                // Check for shutdown signal
                result = shutdown_rx.changed() => {
                    if result.is_ok() && *shutdown_rx.borrow() {
                        break;
                    }
                }

                // Rate-limited render
                _ = render_interval.tick() => {
                    let state = self.state_rx.borrow().clone();
                    if let Err(e) = Self::render(&state) {
                        eprintln!("Render error: {e}");
                    }
                }

                // Handle keyboard events
                event = event_stream.next() => {
                    if let Some(Ok(Event::Key(key_event))) = event {
                        if key_event.code == KeyCode::Char('c')
                            && key_event.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            break;
                        }
                    }
                }
            }
        }
    }
}

// ============================================================================
// Agent States
// ============================================================================

/// Counter service agent state.
#[acton_actor]
struct CounterState {
    value: i64,
    operations: usize,
    state_tx: Option<watch::Sender<DashboardState>>,
}

/// Logger service agent state.
#[acton_actor]
struct LoggerState {
    entries: Vec<(String, String)>,
    state_tx: Option<watch::Sender<DashboardState>>,
}

/// Config service agent state.
#[acton_actor]
struct ConfigState {
    values: HashMap<String, String>,
    state_tx: Option<watch::Sender<DashboardState>>,
}

// ============================================================================
// Agent Creation Functions
// ============================================================================

/// Creates the counter service agent.
async fn create_counter_agent(
    runtime: &mut AgentRuntime,
    state_tx: watch::Sender<DashboardState>,
) -> AgentHandle {
    let mut counter = runtime.new_agent_with_name::<CounterState>("counter".to_string());

    // Store the sender in the agent state
    counter.model.state_tx = Some(state_tx.clone());

    counter
        .mutate_on::<Increment>(move |agent, envelope| {
            let amount = envelope.message().amount;
            agent.model.value += amount;
            agent.model.operations += 1;
            let value = agent.model.value;
            let ops = agent.model.operations;

            if let Some(tx) = &agent.model.state_tx {
                tx.send_modify(|state| {
                    state.update_counter(value, ops);
                    state.add_activity(format!("Counter +{amount} → {value}"));
                });
            }

            Box::pin(async {})
        })
        .mutate_on::<Decrement>(move |agent, envelope| {
            let amount = envelope.message().amount;
            agent.model.value -= amount;
            agent.model.operations += 1;
            let value = agent.model.value;
            let ops = agent.model.operations;

            if let Some(tx) = &agent.model.state_tx {
                tx.send_modify(|state| {
                    state.update_counter(value, ops);
                    state.add_activity(format!("Counter -{amount} → {value}"));
                });
            }

            Box::pin(async {})
        });

    counter.start().await
}

/// Creates the logger service agent.
async fn create_logger_agent(
    runtime: &mut AgentRuntime,
    state_tx: watch::Sender<DashboardState>,
) -> AgentHandle {
    let mut logger = runtime.new_agent_with_name::<LoggerState>("logger".to_string());

    logger.model.state_tx = Some(state_tx.clone());

    logger.mutate_on::<LogMessage>(move |agent, envelope| {
        let level = envelope.message().level.clone();
        let message = envelope.message().message.clone();
        agent.model.entries.push((level.clone(), message.clone()));
        let count = agent.model.entries.len();

        if let Some(tx) = &agent.model.state_tx {
            tx.send_modify(|state| {
                state.update_logger(count);
                state.add_activity(format!("[{level}] {message}"));
            });
        }

        Box::pin(async {})
    });

    logger.start().await
}

/// Creates the config service agent.
async fn create_config_agent(
    runtime: &mut AgentRuntime,
    state_tx: watch::Sender<DashboardState>,
) -> AgentHandle {
    let mut config = runtime.new_agent_with_name::<ConfigState>("config".to_string());

    config.model.state_tx = Some(state_tx.clone());

    config
        .mutate_on::<SetConfig>(move |agent, envelope| {
            let key = envelope.message().key.clone();
            let value = envelope.message().value.clone();
            agent.model.values.insert(key.clone(), value.clone());
            let count = agent.model.values.len();

            if let Some(tx) = &agent.model.state_tx {
                tx.send_modify(|state| {
                    state.update_config(count);
                    state.add_activity(format!("Config: {key}={value}"));
                });
            }

            Box::pin(async {})
        })
        .mutate_on::<GetConfig>(move |agent, envelope| {
            let key = envelope.message().key.clone();
            let value = agent.model.values.get(&key).cloned();

            if let Some(tx) = &agent.model.state_tx {
                let msg = value.map_or_else(
                    || format!("Config get: {key} (not found)"),
                    |v| format!("Config get: {key}={v}"),
                );
                tx.send_modify(|state| {
                    state.add_activity(msg);
                });
            }

            Box::pin(async {})
        });

    config.start().await
}

/// Periodically updates IPC statistics in the dashboard state.
async fn stats_update_loop(
    stats: Arc<IpcListenerStats>,
    state_tx: watch::Sender<DashboardState>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(Duration::from_millis(250));

    loop {
        tokio::select! {
            result = shutdown_rx.changed() => {
                if result.is_ok() && *shutdown_rx.borrow() {
                    break;
                }
            }
            _ = interval.tick() => {
                state_tx.send_modify(|state| {
                    state.connections_active = stats.connections_active();
                    state.connections_total = stats.connections_accepted();
                    state.messages_received = stats.messages_received();
                    state.messages_routed = stats.messages_routed();
                    state.errors = stats.errors();
                });
            }
        }
    }
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create dashboard state channel
    let (state_tx, state_rx) = watch::channel(DashboardState::new());
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Create dashboard
    let dashboard = Dashboard::new(state_rx);

    // Enter alternate screen
    Dashboard::enter()?;

    // Ensure we cleanup on panic or early exit
    let cleanup_guard = scopeguard::guard((), |()| {
        let _ = execute!(stdout(), Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    });

    let mut runtime = ActonApp::launch();

    // Register all IPC message types
    let registry = runtime.ipc_registry();
    registry.register::<Increment>("Increment");
    registry.register::<Decrement>("Decrement");
    registry.register::<LogMessage>("LogMessage");
    registry.register::<SetConfig>("SetConfig");
    registry.register::<GetConfig>("GetConfig");

    state_tx.send_modify(|state| {
        state.add_activity(format!("Registered {} IPC message types", registry.len()));
    });

    // Create service agents
    let counter = create_counter_agent(&mut runtime, state_tx.clone()).await;
    let logger = create_logger_agent(&mut runtime, state_tx.clone()).await;
    let config = create_config_agent(&mut runtime, state_tx.clone()).await;

    // Expose agents for IPC
    runtime.ipc_expose("counter", counter.clone());
    runtime.ipc_expose("logger", logger.clone());
    runtime.ipc_expose("config", config.clone());

    state_tx.send_modify(|state| {
        state.add_activity("Exposed agents: counter, logger, config".to_string());
    });

    // Start the IPC listener
    let ipc_config = IpcConfig::load();
    let socket_path = ipc_config.socket_path();

    state_tx.send_modify(|state| {
        state.socket_path = socket_path.display().to_string();
    });

    let listener_handle = runtime.start_ipc_listener().await?;

    state_tx.send_modify(|state| {
        state.add_activity("IPC listener started".to_string());
    });

    // Verify socket is ready
    tokio::time::sleep(Duration::from_millis(50)).await;
    if socket_exists(&socket_path) {
        state_tx.send_modify(|state| {
            state.add_activity("Socket ready for connections".to_string());
        });
    }

    // Start stats update loop
    let stats_clone = listener_handle.stats.clone();
    let stats_shutdown_rx = shutdown_rx.clone();
    let stats_state_tx = state_tx.clone();
    tokio::spawn(async move {
        stats_update_loop(stats_clone, stats_state_tx, stats_shutdown_rx).await;
    });

    // Run dashboard until Ctrl+C
    let dashboard_shutdown_rx = shutdown_rx.clone();
    let dashboard_task = tokio::spawn(async move {
        dashboard.run(dashboard_shutdown_rx).await;
    });

    // Wait for Ctrl+C or dashboard exit
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = dashboard_task => {}
    }

    // Signal shutdown
    let _ = shutdown_tx.send(true);

    // Stop the listener
    listener_handle.stop();

    // Allow time for async cleanup
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Shutdown the runtime
    runtime.shutdown_all().await?;

    // Leave alternate screen (cleanup guard handles this, but be explicit)
    drop(cleanup_guard);

    println!("\n=== Server Shutdown Complete ===");

    Ok(())
}
