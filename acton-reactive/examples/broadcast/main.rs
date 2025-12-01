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

/// State for the `DataCollector` actor.
// The `#[acton_actor]` macro derives `Default`, `Clone`, and implements `Debug`.
#[acton_actor]
struct DataCollector {
    /// Stores received data points.
    data_points: Vec<i32>,
}

/// State for the Aggregator actor.
// The `#[acton_actor]` macro derives `Default`, `Clone`, and implements `Debug`.
#[acton_actor]
struct Aggregator {
    /// Stores the running sum of received data.
    sum: i32,
}

/// State for the Printer actor.
// Note: `Stdout` doesn't implement `Default`.
// Use `#[acton_actor(no_default)]` to skip that derive while still
// getting Debug and compile-time Send + 'static checks.
#[acton_actor(no_default)]
struct Printer {
    /// Handle to standard output for printing.
    out: Stdout,
}

// Manual Default implementation for Printer state.
impl Default for Printer {
    fn default() -> Self {
        Self { out: stdout() }
    }
}

// --- Messages ---

/// Message broadcast when new data is available.
#[acton_message]
struct NewData(i32);

// Type aliases for clarity in StatusUpdate message.
type From = String;
type Status = String;

/// Message broadcast to report actor status or results.
#[acton_message]
enum StatusUpdate {
    Ready(From, Status),
    Updated(From, i32),
    Done(i32),
}

// --- Main Application Logic ---

#[acton_main]
#[allow(clippy::too_many_lines)]
async fn main() {
    // 1. Launch the Acton runtime environment.
    let mut runtime = ActonApp::launch_async().await;
    // Get a handle to the central message broker provided by the runtime.
    let broker_handle = runtime.broker();

    // 2. Create actor builders.
    //    These actors will communicate indirectly via the broker.
    let mut data_collector_builder = runtime.new_actor::<DataCollector>();
    let mut aggregator_builder = runtime.new_actor::<Aggregator>();
    let mut printer_builder = runtime.new_actor::<Printer>();

    // 3. Configure the DataCollector actor.
    data_collector_builder
        // Handler for `NewData` messages.
        .mutate_on::<NewData>(|actor, envelope| {
            // Add the received data point to internal state.
            actor.model.data_points.push(envelope.message().0);

            // Broadcast a status update via the broker.
            let broker_handle = actor.broker().clone();
            let message = envelope.message().0;
            Reply::pending(async move {
                broker_handle
                    .broadcast(StatusUpdate::Updated("DataCollector".to_string(), message))
                    .await;
            })
        })
        // After starting, broadcast its readiness.
        .after_start(|actor| {
            let broker_handle = actor.broker().clone();
            Reply::pending(async move {
                broker_handle
                    .broadcast(StatusUpdate::Ready(
                        "DataCollector".to_string(),
                        "ready to collect data".to_string(),
                    ))
                    .await;
            })
        });

    // 4. Configure the Aggregator actor.
    aggregator_builder
        // Handler for `NewData` messages.
        .mutate_on::<NewData>(|actor, envelope| {
            // Add the received data to the running sum.
            actor.model.sum += envelope.message().0;

            // Broadcast an update with the current sum.
            let broker_handle = actor.broker().clone();
            let sum = actor.model.sum;
            Reply::pending(async move {
                broker_handle
                    .broadcast(StatusUpdate::Updated("Aggregator".to_string(), sum))
                    .await;
            })
        })
        // After starting, broadcast its readiness.
        .after_start(|actor| {
            let broker_handle = actor.broker().clone();
            Reply::pending(async move {
                broker_handle
                    .broadcast(StatusUpdate::Ready(
                        "Aggregator".to_string(),
                        "ready to sum data".to_string(),
                    ))
                    .await;
            })
        })
        // Before stopping, broadcast the final sum.
        .before_stop(|actor| {
            let broker_handle = actor.broker().clone();
            let sum = actor.model.sum;
            Reply::pending(async move {
                broker_handle.broadcast(StatusUpdate::Done(sum)).await;
            })
        });

    // 5. Configure the Printer actor.
    printer_builder
        // Handler for `StatusUpdate` messages (broadcast by other actors).
        .mutate_on::<StatusUpdate>(|actor, envelope| {
            // Use crossterm to print formatted/colored output based on the message variant.
            match &envelope.message() {
                StatusUpdate::Ready(who, what) => {
                    let _ = execute!(
                        &mut actor.model.out, // Use the Stdout handle from actor state (needs mut ref).
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
                        &mut actor.model.out,
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
                        &mut actor.model.out,
                        SetForegroundColor(Color::DarkMagenta),
                        Print("\u{1F680} "),
                        SetForegroundColor(Color::Red),
                        Print(format!("Final sum {sum}!\n\n")),
                        ResetColor
                    );
                }
            }

            Reply::ready()
        })
        // After starting, broadcast its readiness.
        .after_start(|actor| {
            // Note: We broadcast a message here instead of printing directly because
            // the `actor` reference in lifecycle hooks is immutable, and `execute!`
            // requires a mutable reference to `actor.model.out`.
            let broker_handle = actor.broker().clone();
            Reply::pending(async move {
                broker_handle
                    .broadcast(StatusUpdate::Ready(
                        "Printer".to_string(),
                        "ready to display messages".to_string(),
                    ))
                    .await;
            })
        });

    // 6. Subscribe actors to the message types they care about *before* starting them.
    //    Actors only receive broadcast messages they are subscribed to.
    data_collector_builder.handle().subscribe::<NewData>().await;
    aggregator_builder.handle().subscribe::<NewData>().await;
    printer_builder.handle().subscribe::<StatusUpdate>().await;

    // 7. Start the actors.
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
    runtime
        .shutdown_all()
        .await
        .expect("Failed to shutdown system");
}
