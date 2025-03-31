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
use std::sync::Once;

use tracing::Level;
use tracing_subscriber::{EnvFilter, FmtSubscriber};
use tracing_subscriber::fmt::format::FmtSpan;

// Re-export actors and messages for easy access within tests.
pub use actors::*;
pub use messages::*;

// Declare the submodules.
mod actors;
mod messages;

// Ensures tracing initialization happens only once across all tests.
static INIT: Once = Once::new();

/// Initializes the global tracing subscriber for tests.
///
/// This function sets up a `tracing_subscriber::FmtSubscriber` with a
/// specific `EnvFilter` configuration to control log levels for different
/// modules and targets during test execution. It uses `std::sync::Once`
/// to ensure that this initialization logic runs only once, even if
/// `initialize_tracing` is called multiple times (e.g., from different tests).
pub fn initialize_tracing() {
    INIT.call_once(|| {
        // Configure the log filter using `EnvFilter`. This allows fine-grained control
        // over which log levels are shown for different parts of the codebase (targets).
        // Directives are typically in the format "target=level" or "target[span]=level".
        // This setup aims to reduce noise from core components while enabling more
        // detailed logging for specific test modules or areas under investigation.
        let filter = EnvFilter::new("")
            // Example: Reduce noise from the agent's main loop unless it's a warning.
            .add_directive("acton_core::actor::managed_agent::started[wake]=warn".parse().unwrap())
            // Enable debug logs for actor references (handles).
            .add_directive("acton_core::common::actor_ref=debug".parse().unwrap())
            // Set various core components to only show errors by default.
            .add_directive("acton_core::common::acton=error".parse().unwrap())
            .add_directive("acton_core::pool=error".parse().unwrap())
            .add_directive("acton_core::pool::builder=error".parse().unwrap())
            .add_directive("acton_core::common::system=error".parse().unwrap())
            .add_directive("acton_core::common::supervisor=error".parse().unwrap())
            .add_directive("acton_core::actor::actor=error".parse().unwrap())
            .add_directive("acton_core::common::broker=error".parse().unwrap())
            .add_directive(
                "acton_core::common::broker[broadcast]=error"
                    .parse()
                    .unwrap(),
            )
            .add_directive("acton_core::actor::managed_actor=info".parse().unwrap())
            .add_directive("acton_core::actor::actor[wake]=error".parse().unwrap())
            .add_directive(
                "acton_core::actor::actor[terminate_actor]=off"
                    .parse()
                    .unwrap(),
            )
            .add_directive(
                "acton_core::actor::actor[handle_message]=trace"
                    .parse()
                    .unwrap(),
            )
            .add_directive(
                "acton_core::actor::actor[suspend_self]=off"
                    .parse()
                    .unwrap(),
            )
            .add_directive("acton_core::actor::actor[new]=error".parse().unwrap())
            .add_directive("acton_core::actor::actor[init]=error".parse().unwrap())
            .add_directive("acton_core::actor::actor[activate]=error".parse().unwrap())
            .add_directive("acton_core::actor::idle=off".parse().unwrap())
            .add_directive("acton_core::actor::idle[act_on]=warn".parse().unwrap())
            .add_directive(
                "acton_core::message::message_envelope=trace"
                    .parse()
                    .unwrap(),
            )
            .add_directive(
                "acton_core::message::outbound_envelope=debug"
                    .parse()
                    .unwrap(),
            )
            .add_directive(
                "acton_core::message::broadcast_envelope=off"
                    .parse()
                    .unwrap(),
            )
            .add_directive("acton_core::traits::broker_context=error".parse().unwrap())
            .add_directive("acton_core::traits::actor_context=error".parse().unwrap())
            .add_directive("acton_core::traits::subscribable=error".parse().unwrap())
            // Set specific log levels for different test files/modules.
            // .add_directive("supervisor_tests=info".parse().unwrap()) // Removed as this test file doesn't exist
            .add_directive("broker_tests=trace".parse().unwrap())
            .add_directive("launchpad_tests=info".parse().unwrap())
            .add_directive("lifecycle_tests=info".parse().unwrap())
            .add_directive("actor_tests=debug".parse().unwrap())
            .add_directive("direct_messaging_tests=debug".parse().unwrap())
            .add_directive("load_balancer_tests=info".parse().unwrap()) // Keep even if file doesn't exist yet
            .add_directive(
                "acton::tests::setup::actor::pool_item=info"
                    .parse()
                    .unwrap(),
            )
            .add_directive(
                "acton::tests=info" // General level for tests if not specified above
                    .parse()
                    .unwrap(),
            )
            .add_directive("messaging_tests=info".parse().unwrap())
            // Default level for everything else.
            .add_directive(tracing_subscriber::filter::LevelFilter::ERROR.into());

        // Build the FmtSubscriber with desired formatting options.
        let subscriber = FmtSubscriber::builder()
            // Disable span events (enter/exit) for less verbose logs.
            .with_span_events(FmtSpan::NONE)
            // Set the maximum level processed by this subscriber (TRACE includes everything).
            .with_max_level(Level::TRACE)
            .compact() // Use a more compact output format.
            .with_line_number(true) // Include line numbers in logs.
            .without_time() // Don't include timestamps.
            .with_target(true) // Include the log target (module path).
            // Apply the configured filter.
            .with_env_filter(filter)
            .finish();

        // Set the configured subscriber as the global default for the application.
        tracing::subscriber::set_global_default(subscriber)
            .expect("setting default subscriber failed");
    });
}
