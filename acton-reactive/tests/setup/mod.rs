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
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::{EnvFilter, FmtSubscriber};

// Re-export actors and messages for easy access within tests.

// Declare the submodules.
pub mod actors;
pub mod messages;

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
        // Ensure logs directory exists
        std::fs::create_dir_all("logs").expect("could not create logs dir");

        // Set up file appender (no rotation, file is logs/actor_tests.log)
        let file_appender = RollingFileAppender::new(Rotation::NEVER, "logs", "actor_tests.txt");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        // Leak the guard so the non-blocking writer is not dropped before process exit
        Box::leak(Box::new(guard));

        let filter = EnvFilter::new("trace")
            .add_directive(
                "acton_reactive::actor::managed_actor::started[wake]=trace"
                    .parse()
                    .unwrap(),
            )
            .add_directive(
                "acton_reactive::actor::managed_actor::started=trace"
                    .parse()
                    .unwrap(),
            )
            .add_directive("acton_reactive::common::actor_ref=trace".parse().unwrap())
            .add_directive(
                "acton_reactive::message::outbound_envelope=trace"
                    .parse()
                    .unwrap(),
            )
            .add_directive(
                "acton_reactive::message::message_envelope=trace"
                    .parse()
                    .unwrap(),
            )
            .add_directive("broker_tests=trace".parse().unwrap())
            .add_directive("actor_tests=trace".parse().unwrap())
            .add_directive("tokio=trace".parse().unwrap())
            .add_directive(tracing_subscriber::filter::LevelFilter::TRACE.into());

        let subscriber = FmtSubscriber::builder()
            .with_span_events(FmtSpan::NONE)
            .with_max_level(Level::TRACE)
            .compact()
            .with_line_number(true)
            .without_time()
            .with_target(true)
            .with_env_filter(filter)
            .with_writer(non_blocking)
            .finish();

        tracing::subscriber::set_global_default(subscriber)
            .expect("setting default subscriber failed");
        // _guard can be leaked to ensure process flushes logs on exit if necessary
    });
}
