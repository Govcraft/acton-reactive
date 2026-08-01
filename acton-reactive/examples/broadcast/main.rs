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

//! Broadcasting to many actors through the broker, and knowing when they are done.
//!
//! The data path is pub/sub: `main` broadcasts `NewData`, and both the `DataCollector`
//! and the `Aggregator` receive it without `main` holding either handle. Each then
//! reports through the broker again, to the `Printer`.
//!
//! # Knowing when a broadcast pipeline has finished
//!
//! Broadcasting is fire-and-forget. `broadcast` returns once the message is in the
//! broker's inbox, which says nothing about whether any subscriber has run — so shutting
//! down straight after a broadcast races the work. Sleeping "long enough" is not a fix;
//! it is the same race with a longer fuse.
//!
//! Three properties of the framework combine into a fix that has no race at all:
//!
//! 1. **The broker is an ordinary actor with a FIFO inbox**, and it fans a message out to
//!    every subscriber before it dequeues the next one. So broadcasts are delivered to
//!    subscribers in the order they were broadcast.
//! 2. **A `mutate_on` handler awaits its `Reply::pending` future before the actor takes
//!    its next message.** An actor that broadcasts from inside a handler has therefore
//!    finished that broadcast before it handles anything else.
//! 3. **A handler may keep the reply envelope and answer later.** That is what makes the
//!    final `ask` below immune to ordering: it does not matter whether the request
//!    arrives before or after the work finishes.
//!
//! Together those give an end-to-end happens-before chain. `main` broadcasts the two
//! `NewData` messages and then a `Finalize`; (1) puts `Finalize` behind the data at every
//! subscriber, and (2) means each collector has already broadcast its updates by the time
//! it sees `Finalize`. Each collector answers `Finalize` with a `Finished` marker, so the
//! `Printer` — which receives all of it in broker order — can tell exactly when the last
//! report has been printed. `main` learns that through `ask`, then shuts down.
//!
//! The pattern to copy: **end a broadcast pipeline with a marker message, and `ask` the
//! actor at the end of the pipeline whether the marker has arrived.** Never shut down on
//! a timer.

use std::io::stdout;
use std::io::Stdout;

use crossterm::{
    execute,
    style::{Color, Print, ResetColor, SetForegroundColor},
};

use acton_reactive::prelude::*;

/// How many actors report a `Finished` marker in response to `Finalize`.
///
/// The `Printer` counts these to know the pipeline has quiesced.
const COLLECTOR_COUNT: usize = 2;

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
    /// The final sum, once the `Aggregator` has reported it.
    final_sum: Option<i32>,
    /// How many collectors have announced they are finished.
    collectors_finished: usize,
    /// A caller waiting to be told the run is complete.
    ///
    /// Holding the reply envelope is what lets the `Printer` answer an `AwaitReport`
    /// that arrived *before* the work finished, instead of answering too early.
    waiting: Option<OutboundEnvelope>,
}

// Manual Default implementation for Printer state.
impl Default for Printer {
    fn default() -> Self {
        Self {
            out: stdout(),
            final_sum: None,
            collectors_finished: 0,
            waiting: None,
        }
    }
}

/// Reports whether every collector has finished and the final sum has arrived.
///
/// A pure predicate over the printer's state, so the completion rule is one obvious
/// expression rather than a condition spread across two handlers.
const fn run_complete(printer: &Printer) -> bool {
    printer.final_sum.is_some() && printer.collectors_finished == COLLECTOR_COUNT
}

// --- Messages ---

/// Message broadcast when new data is available.
#[acton_message]
struct NewData(i32);

/// Broadcast once all data has been sent, asking each collector to report and finish.
///
/// Because the broker preserves broadcast order, every subscriber sees this *after* the
/// `NewData` messages that preceded it.
#[acton_message]
struct Finalize;

// Type aliases for clarity in StatusUpdate message.
type From = String;
type Status = String;

/// Message broadcast to report actor status or results.
#[acton_message]
enum StatusUpdate {
    Ready(From, Status),
    Updated(From, i32),
    Done(i32),
    /// Sent by a collector once it has reported everything it is going to report.
    ///
    /// Carries nothing: the `Printer` only needs to count these, not tell them apart.
    Finished,
}

/// Asks the `Printer` to report once the whole pipeline has finished.
///
/// Sent directly to the `Printer` rather than broadcast, because a reply has to come
/// back to a specific caller.
#[acton_message]
struct AwaitReport;

/// The `Printer`'s answer to [`AwaitReport`]: everything has been printed.
#[acton_message]
struct FinalReport {
    sum: i32,
}

impl Request for AwaitReport {
    type Response = FinalReport;
}

// --- Actor configuration ---

/// Configures the `DataCollector`: records each data point and reports it.
fn configure_data_collector(builder: &mut ManagedActor<Idle, DataCollector>) {
    builder
        // Handler for `NewData` messages.
        .mutate_on::<NewData>(|actor, envelope| {
            // Add the received data point to internal state.
            actor.model.data_points.push(envelope.message().0);

            // Broadcast a status update via the broker. Awaited before this actor takes
            // its next message, so the update is queued at the broker ahead of anything
            // this actor broadcasts later.
            let broker_handle = actor.broker().clone();
            let message = envelope.message().0;
            Reply::pending(async move {
                broker_handle
                    .broadcast(StatusUpdate::Updated("DataCollector".to_string(), message))
                    .await;
            })
        })
        // Nothing left to report: announce that this collector is finished.
        .mutate_on::<Finalize>(|actor, _envelope| {
            let broker_handle = actor.broker().clone();
            Reply::pending(async move {
                broker_handle.broadcast(StatusUpdate::Finished).await;
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
}

/// Configures the `Aggregator`: keeps a running sum and reports the total.
fn configure_aggregator(builder: &mut ManagedActor<Idle, Aggregator>) {
    builder
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
        // Report the final sum, then announce that this collector is finished.
        //
        // This is deliberately *not* a `before_stop` hook. A hook that broadcasts during
        // shutdown races the shutdown itself: the message has to cross the broker to
        // reach the Printer, and by then the Printer may already have closed its inbox.
        // Doing the work before shutdown starts removes the race rather than narrowing
        // it.
        .mutate_on::<Finalize>(|actor, _envelope| {
            let broker_handle = actor.broker().clone();
            let sum = actor.model.sum;
            Reply::pending(async move {
                broker_handle.broadcast(StatusUpdate::Done(sum)).await;
                broker_handle.broadcast(StatusUpdate::Finished).await;
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
        });
}

/// Prints one status update, using the `Stdout` handle held in the actor's state.
fn print_status(out: &mut Stdout, update: &StatusUpdate) {
    match update {
        StatusUpdate::Ready(who, what) => {
            let _ = execute!(
                out,
                SetForegroundColor(Color::DarkYellow),
                Print("\u{2713}  "),
                SetForegroundColor(Color::Yellow),
                Print(who.clone()),
                SetForegroundColor(Color::DarkYellow),
                Print(format!(" is {what}!\n")),
                ResetColor
            );
        }
        StatusUpdate::Updated(who, data_value) => {
            let _ = execute!(
                out,
                SetForegroundColor(Color::DarkCyan),
                Print("\u{2139}  "),
                SetForegroundColor(Color::Cyan),
                Print(who.clone()),
                SetForegroundColor(Color::DarkCyan),
                Print(format!(" updated value: {data_value}!\n")),
                ResetColor
            );
        }
        StatusUpdate::Done(sum) => {
            let _ = execute!(
                out,
                SetForegroundColor(Color::DarkMagenta),
                Print("\u{1F680} "),
                SetForegroundColor(Color::Red),
                Print(format!("Final sum {sum}!\n\n")),
                ResetColor
            );
        }
        // A bookkeeping marker rather than something worth showing the user.
        StatusUpdate::Finished => {}
    }
}

/// Configures the `Printer`: displays every update and reports when the run is complete.
fn configure_printer(builder: &mut ManagedActor<Idle, Printer>) {
    builder
        // Handler for `StatusUpdate` messages (broadcast by other actors).
        .mutate_on::<StatusUpdate>(|actor, envelope| {
            let update = envelope.message();
            print_status(&mut actor.model.out, update);

            // Track progress towards completion.
            match update {
                StatusUpdate::Done(sum) => actor.model.final_sum = Some(*sum),
                StatusUpdate::Finished => actor.model.collectors_finished += 1,
                StatusUpdate::Ready(..) | StatusUpdate::Updated(..) => {}
            }

            // If that was the last thing we were waiting for, answer whoever asked.
            if run_complete(&actor.model) {
                if let Some(reply) = actor.model.waiting.take() {
                    let sum = actor.model.final_sum.unwrap_or_default();
                    return Reply::pending(async move {
                        reply.send(FinalReport { sum }).await;
                    });
                }
            }

            Reply::ready()
        })
        // Report completion — now if the run has already finished, later if it has not.
        //
        // Storing the envelope is what makes the caller's `ask` insensitive to timing:
        // the request may arrive before the pipeline drains, and it still gets a correct
        // answer at the correct moment instead of an early one.
        .mutate_on::<AwaitReport>(|actor, envelope| {
            if run_complete(&actor.model) {
                let reply = envelope.reply_envelope();
                let sum = actor.model.final_sum.unwrap_or_default();
                return Reply::pending(async move {
                    reply.send(FinalReport { sum }).await;
                });
            }

            actor.model.waiting = Some(envelope.reply_envelope());
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
}

// --- Main Application Logic ---

#[acton_main]
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

    // 3. Configure each actor's handlers.
    configure_data_collector(&mut data_collector_builder);
    configure_aggregator(&mut aggregator_builder);
    configure_printer(&mut printer_builder);

    // 4. Subscribe actors to the message types they care about *before* starting them.
    //    Actors only receive broadcast messages they are subscribed to.
    data_collector_builder.handle().subscribe::<NewData>().await;
    data_collector_builder
        .handle()
        .subscribe::<Finalize>()
        .await;
    aggregator_builder.handle().subscribe::<NewData>().await;
    aggregator_builder.handle().subscribe::<Finalize>().await;
    printer_builder.handle().subscribe::<StatusUpdate>().await;

    // 5. Start the actors.
    let _data_collector_handle = data_collector_builder.start().await;
    let _aggregator_handle = aggregator_builder.start().await;
    let printer_handle = printer_builder.start().await;

    // 6. Broadcast `NewData` messages using the main broker handle.
    //    Both DataCollector and Aggregator are subscribed and will receive these.
    broker_handle.broadcast(NewData(5)).await;
    broker_handle.broadcast(NewData(10)).await;

    // 7. Close the pipeline. The broker delivers this after the data above, so each
    //    collector has already broadcast its updates by the time it sees this.
    broker_handle.broadcast(Finalize).await;

    // 8. Wait for the work to actually finish, rather than guessing with a sleep.
    //    The Printer answers once it has printed the final sum and both collectors have
    //    reported in.
    let report: FinalReport = printer_handle
        .ask(AwaitReport)
        .await
        .expect("the Printer should report once the pipeline has drained");

    // 9. Shut down the runtime. Everything the actors had to say has already been said,
    //    so nothing is lost to the shutdown.
    runtime
        .shutdown_all()
        .await
        .expect("Failed to shutdown system");

    println!("Run complete. Final sum reported by the Printer: {}", report.sum);
}
