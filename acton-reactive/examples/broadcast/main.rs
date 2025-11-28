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
use std::io::stdout;
use std::io::Stdout;

use crossterm::{
    execute,
    style::{Color, Print, ResetColor, SetForegroundColor},
};

use acton_reactive::prelude::*;
// Import the macro for agent state structs
use acton_macro::acton_actor;

/// State for the `DataCollector` agent.
// The `#[acton_actor]` macro derives `Default`, `Clone`, and implements `Debug`.
#[acton_actor]
struct DataCollector {
    /// Stores received data points.
    data_points: Vec<i32>,
}

/// State for the Aggregator agent.
// The `#[acton_actor]` macro derives `Default`, `Clone`, and implements `Debug`.
#[acton_actor]
struct Aggregator {
    /// Stores the running sum of received data.
    sum: i32,
}

/// State for the Printer agent.
// Note: Manual Default impl needed because `Stdout` doesn't impl Default.
// Cannot use `#[acton_actor]` because it attempts to derive Default and Clone,
// which `Stdout` does not implement. We derive Debug manually.
#[derive(Debug)]
struct Printer {
    /// Handle to standard output for printing.
    out: Stdout,
}

// Manual Default implementation for Printer state.
impl Default for Printer {
    fn default() -> Self {
        Self {
            out: stdout()
        }
    }
}

// --- Messages ---

/// Message broadcast when new data is available.
#[derive(Clone, Debug)]
struct NewData(i32);

// Type aliases for clarity in StatusUpdate message.
type From = String;
type Status = String;

/// Message broadcast to report agent status or results.
#[derive(Clone, Debug)]
enum StatusUpdate {
    Ready(From, Status),
    Updated(From, i32),
    Done(i32),
}

// --- Main Application Logic ---

#[tokio::main]
async fn main() {
    // 1. Launch the Acton runtime environment.
    let mut runtime = ActonApp::launch();
    // Get a handle to the central message broker provided by the runtime.
    let broker_handle = runtime.broker();

    // 2. Create agent builders.
    //    These agents will communicate indirectly via the broker.
    let mut data_collector_builder = runtime.new_agent::<DataCollector>();
    let mut aggregator_builder = runtime.new_agent::<Aggregator>();
    let mut printer_builder = runtime.new_agent::<Printer>();

    // 3. Configure the DataCollector agent.
    data_collector_builder
        // Handler for `NewData` messages.
        .mutate_on::<NewData>(|agent, envelope| {
            // Add the received data point to internal state.
            agent.model.data_points.push(envelope.message().0);

            // Broadcast a status update via the broker.
            let broker_handle = agent.broker().clone();
            let message = envelope.message().0;
            Box::pin(async move { broker_handle.broadcast(StatusUpdate::Updated("DataCollector".to_string(), message)).await })
        })
        // After starting, broadcast its readiness.
        .after_start(|agent| {
            let broker_handle = agent.broker().clone();
            Box::pin(async move {
                broker_handle.broadcast(StatusUpdate::Ready("DataCollector".to_string(), "ready to collect data".to_string())).await;
            })
        });

    // 4. Configure the Aggregator agent.
    aggregator_builder
        // Handler for `NewData` messages.
        .mutate_on::<NewData>(|agent, envelope| {
            // Add the received data to the running sum.
            agent.model.sum += envelope.message().0;

            // Broadcast an update with the current sum.
            let broker_handle = agent.broker().clone();
            let sum = agent.model.sum;
            Box::pin(async move { broker_handle.broadcast(StatusUpdate::Updated("Aggregator".to_string(), sum)).await })
        })
        // After starting, broadcast its readiness.
        .after_start(|agent| {
            let broker_handle = agent.broker().clone();
            Box::pin(async move {
                broker_handle.broadcast(StatusUpdate::Ready("Aggregator".to_string(), "ready to sum data".to_string())).await;
            })
        })
        // Before stopping, broadcast the final sum.
        .before_stop(|agent| {
            let broker_handle = agent.broker().clone();
            let sum = agent.model.sum;
            Box::pin(async move {
                broker_handle.broadcast(StatusUpdate::Done(sum)).await;
            })
        });

    // 5. Configure the Printer agent.
    printer_builder
        // Handler for `StatusUpdate` messages (broadcast by other agents).
        .mutate_on::<StatusUpdate>(|agent, envelope| {
            // Use crossterm to print formatted/colored output based on the message variant.
            match &envelope.message() {
                StatusUpdate::Ready(who, what) => {
                    let _ = execute!(
                        &mut agent.model.out, // Use the Stdout handle from agent state (needs mut ref).
                        SetForegroundColor(Color::DarkYellow),
                        Print("\u{2713}  "),
                        SetForegroundColor(Color::Yellow),
                        Print(who.to_string()),
                        SetForegroundColor(Color::DarkYellow),
                        Print(format!(" is {what}!\n")),
                        ResetColor
                    );
                }
                StatusUpdate::Updated(who, data_value) => {
                    let _ = execute!(
                        &mut agent.model.out,
                        SetForegroundColor(Color::DarkCyan),
                        Print("\u{2139}  "),
                        SetForegroundColor(Color::Cyan),
                        Print(who.to_string()),
                        SetForegroundColor(Color::DarkCyan),
                        Print(format!(" updated value: {data_value}!\n")),
                        ResetColor
                    );
                }
                StatusUpdate::Done(sum) => {
                    let _ = execute!(
                        &mut agent.model.out,
                        SetForegroundColor(Color::DarkMagenta),
                        Print("\u{1F680} "),
                        SetForegroundColor(Color::Red),
                        Print(format!("Final sum {sum}!\n\n")),
                        ResetColor
                    );
                }
            }

            AgentReply::immediate()
        })
        // After starting, broadcast its readiness.
        .after_start(|agent| {
            // Note: We broadcast a message here instead of printing directly because
            // the `agent` reference in lifecycle hooks is immutable, and `execute!`
            // requires a mutable reference to `agent.model.out`.
            let broker_handle = agent.broker().clone();
            Box::pin(async move { broker_handle.broadcast(StatusUpdate::Ready("Printer".to_string(), "ready to display messages".to_string())).await })
        });

    // 6. Subscribe agents to the message types they care about *before* starting them.
    //    Agents only receive broadcast messages they are subscribed to.
    data_collector_builder.handle().subscribe::<NewData>().await;
    aggregator_builder.handle().subscribe::<NewData>().await;
    printer_builder.handle().subscribe::<StatusUpdate>().await;

    // 7. Start the agents.
    let _data_collector_handle = data_collector_builder.start().await;
    let _aggregator_handle = aggregator_builder.start().await;
    let _printer_handle = printer_builder.start().await;

    // 8. Broadcast `NewData` messages using the main broker handle.
    //    Both DataCollector and Aggregator are subscribed and will receive these.
    broker_handle.broadcast(NewData(5)).await;
    broker_handle.broadcast(NewData(10)).await;

    // 9. Shut down the runtime.
    //    This triggers the Aggregator's `before_stop` handler, which broadcasts
    //    the final `StatusUpdate::Done` message, received by the Printer.
    runtime.shutdown_all().await.expect("Failed to shutdown system");
}
