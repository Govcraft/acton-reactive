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

//! IPC Client Libraries Example Server
//!
//! A comprehensive example server that demonstrates all IPC features
//! for testing with the Python and Node.js client libraries.
//!
//! # Features
//!
//! - **Calculator Service**: Request-response arithmetic operations
//! - **Search Service**: Streaming search results
//! - **Logger Service**: Fire-and-forget logging
//! - **Price Publisher**: Push notifications via broker subscriptions
//!
//! # Running This Example
//!
//! Start the server:
//! ```bash
//! cargo run --example ipc_client_libraries_server --features ipc
//! ```
//!
//! Then test with the client libraries:
//! ```bash
//! # Python
//! cd examples/ipc_client_libraries/python
//! python example_client.py
//!
//! # Node.js
//! cd examples/ipc_client_libraries/nodejs
//! npm install && npm run example
//! ```

use std::time::Duration;

use acton_macro::{acton_actor, acton_message};
use acton_reactive::ipc::{socket_exists, IpcConfig};
use acton_reactive::prelude::*;
use tracing_subscriber::EnvFilter;

// ============================================================================
// Message Definitions - Calculator Service
// ============================================================================

/// Request to add two numbers.
#[acton_message(ipc)]
struct Add {
    a: i64,
    b: i64,
}

/// Request to multiply two numbers.
#[acton_message(ipc)]
struct Multiply {
    a: i64,
    b: i64,
}

/// Response containing a calculation result.
#[acton_message(ipc)]
struct CalcResult {
    result: i64,
    operation: String,
}

// ============================================================================
// Message Definitions - Search Service
// ============================================================================

/// Request to search with streaming results.
#[acton_message(ipc)]
struct SearchQuery {
    query: String,
    limit: usize,
}

/// A single search result in the stream.
#[acton_message(ipc)]
struct SearchResult {
    id: usize,
    title: String,
    score: f64,
}

// ============================================================================
// Message Definitions - Logger Service
// ============================================================================

/// Log event (fire-and-forget).
#[acton_message(ipc)]
struct LogEvent {
    level: String,
    message: String,
}

// ============================================================================
// Message Definitions - Price Publisher (Push Notifications)
// ============================================================================

/// Price update notification (pushed to subscribers).
#[acton_message(ipc)]
struct PriceUpdate {
    symbol: String,
    price: f64,
    change: f64,
}

/// Status change notification (pushed to subscribers).
#[acton_message(ipc)]
struct StatusChange {
    service: String,
    status: String,
    timestamp_ms: u64,
}

// ============================================================================
// Actor States
// ============================================================================

/// Calculator service - stateless arithmetic operations.
#[acton_actor]
struct CalculatorState {
    operations_performed: usize,
}

/// Search service - simulated search with streaming results.
#[acton_actor]
struct SearchState {
    searches_performed: usize,
}

/// Logger service - collects log events.
#[acton_actor]
struct LoggerState {
    log_count: usize,
}

/// Price publisher - periodically publishes price updates.
#[acton_actor]
struct PricePublisherState {
    tick_count: usize,
}

/// Internal message to trigger price publication.
#[acton_message]
struct PublishTick;

// ============================================================================
// Actor Creation Functions
// ============================================================================

/// Creates the calculator service actor.
async fn create_calculator_actor(runtime: &mut ActorRuntime) -> ActorHandle {
    let mut calculator = runtime.new_actor_with_name::<CalculatorState>("calculator".to_string());

    // Handle addition requests
    calculator.mutate_on::<Add>(|actor, envelope| {
        let msg = envelope.message();
        let result = msg.a + msg.b;
        actor.model.operations_performed += 1;

        let response = CalcResult {
            result,
            operation: format!("{} + {}", msg.a, msg.b),
        };

        println!(
            "  [Calculator] Add: {} + {} = {} (op #{})",
            msg.a, msg.b, result, actor.model.operations_performed
        );

        let reply_envelope = envelope.reply_envelope();
        Reply::pending(async move {
            reply_envelope.send(response).await;
        })
    });

    // Handle multiplication requests
    calculator.mutate_on::<Multiply>(|actor, envelope| {
        let msg = envelope.message();
        let result = msg.a * msg.b;
        actor.model.operations_performed += 1;

        let response = CalcResult {
            result,
            operation: format!("{} × {}", msg.a, msg.b),
        };

        println!(
            "  [Calculator] Multiply: {} × {} = {} (op #{})",
            msg.a, msg.b, result, actor.model.operations_performed
        );

        let reply_envelope = envelope.reply_envelope();
        Reply::pending(async move {
            reply_envelope.send(response).await;
        })
    });

    calculator.start().await
}

/// Creates the search service actor with streaming results.
async fn create_search_actor(runtime: &mut ActorRuntime) -> ActorHandle {
    let mut search = runtime.new_actor_with_name::<SearchState>("search".to_string());

    // Handle search requests - stream multiple results
    search.mutate_on::<SearchQuery>(|actor, envelope| {
        let msg = envelope.message();
        let query = msg.query.clone();
        let limit = msg.limit.clamp(1, 10); // 1-10 results
        actor.model.searches_performed += 1;

        println!(
            "  [Search] Query: \"{}\" (limit: {}, search #{})",
            query, limit, actor.model.searches_performed
        );

        let reply_envelope = envelope.reply_envelope();

        Reply::pending(async move {
            // Simulate search results
            let sample_items = [
                "Getting Started Guide",
                "API Reference",
                "Tutorial: Building Actors",
                "Configuration Options",
                "Performance Tuning",
                "Troubleshooting FAQ",
                "Architecture Overview",
                "Migration Guide",
                "Security Best Practices",
                "Release Notes",
            ];

            for (idx, title) in sample_items.iter().take(limit).enumerate() {
                #[allow(clippy::cast_precision_loss)]
                let score = (idx as f64).mul_add(-0.1, 1.0);
                let result = SearchResult {
                    id: idx + 1,
                    title: format!("{title} (matches: {query})"),
                    score,
                };

                println!("  [Search] Sending result {}/{}", idx + 1, limit);
                reply_envelope.send(result).await;

                // Small delay to demonstrate streaming
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            println!("  [Search] Stream complete");
        })
    });

    search.start().await
}

/// Creates the logger service actor (fire-and-forget).
async fn create_logger_actor(runtime: &mut ActorRuntime) -> ActorHandle {
    let mut logger = runtime.new_actor_with_name::<LoggerState>("logger".to_string());

    // Handle log events - no response needed
    logger.mutate_on::<LogEvent>(|actor, envelope| {
        let msg = envelope.message();
        actor.model.log_count += 1;

        println!(
            "  [Logger] #{} [{}] {}",
            actor.model.log_count,
            msg.level.to_uppercase(),
            msg.message
        );

        Reply::ready()
    });

    logger.start().await
}

/// Creates the price publisher actor that publishes notifications.
async fn create_price_publisher(runtime: &mut ActorRuntime) -> ActorHandle {
    let mut publisher =
        runtime.new_actor_with_name::<PricePublisherState>("price_publisher".to_string());

    // Handle publish ticks - publish price updates
    publisher.mutate_on::<PublishTick>(|actor, _envelope| {
        actor.model.tick_count += 1;
        let tick = actor.model.tick_count;

        // Simulate price movements
        let symbols = ["AAPL", "GOOGL", "MSFT", "AMZN"];
        let symbol = symbols[tick % symbols.len()];
        let base_price = match symbol {
            "AAPL" => 150.0,
            "GOOGL" => 140.0,
            "MSFT" => 370.0,
            "AMZN" => 180.0,
            _ => 100.0,
        };
        #[allow(clippy::cast_precision_loss)]
        let change = f64::mul_add((tick as f64 * 0.7).sin(), 2.0, 0.0);
        let change = (change * 100.0).round() / 100.0;
        let price = base_price + change;

        let update = PriceUpdate {
            symbol: symbol.to_string(),
            price,
            change,
        };

        println!("  [Publisher] Publishing: {symbol} @ ${price:.2} ({change:+.2})");

        let broker = actor.broker().clone();
        Reply::pending(async move {
            broker.broadcast(update).await;
        })
    });

    publisher.start().await
}

/// Spawns a task that periodically triggers price publication.
fn spawn_price_ticker(publisher: ActorHandle) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));

        loop {
            interval.tick().await;
            publisher.send(PublishTick).await;
        }
    });
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

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║      IPC Client Libraries Example Server                     ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    let mut runtime = ActonApp::launch();

    // Register all IPC message types
    let registry = runtime.ipc_registry();

    // Calculator messages
    registry.register::<Add>("Add");
    registry.register::<Multiply>("Multiply");
    registry.register::<CalcResult>("CalcResult");

    // Search messages
    registry.register::<SearchQuery>("SearchQuery");
    registry.register::<SearchResult>("SearchResult");

    // Logger messages
    registry.register::<LogEvent>("LogEvent");

    // Push notification messages
    registry.register::<PriceUpdate>("PriceUpdate");
    registry.register::<StatusChange>("StatusChange");

    println!("📝 Registered {} IPC message types", registry.len());

    // Create service actors
    let calculator = create_calculator_actor(&mut runtime).await;
    println!("🧮 Calculator service started");

    let search = create_search_actor(&mut runtime).await;
    println!("🔍 Search service started");

    let logger = create_logger_actor(&mut runtime).await;
    println!("📋 Logger service started");

    let price_publisher = create_price_publisher(&mut runtime).await;
    println!("💰 Price publisher started");

    // Expose actors for IPC access
    runtime.ipc_expose("calculator", calculator.clone());
    runtime.ipc_expose("search", search.clone());
    runtime.ipc_expose("logger", logger.clone());
    runtime.ipc_expose("price_publisher", price_publisher.clone());
    println!("🔗 Exposed actors: calculator, search, logger, price_publisher");

    // Start price ticker for push notifications
    spawn_price_ticker(price_publisher);
    println!("⏰ Price ticker started (every 5 seconds)");

    // Configure IPC for this example
    let mut ipc_config = IpcConfig::load();
    ipc_config.socket.app_name = Some("ipc_client_example".to_string());
    let socket_path = ipc_config.socket_path();

    // Ensure parent directory exists
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Start the IPC listener
    let listener_handle = runtime.start_ipc_listener_with_config(ipc_config).await?;
    println!("🚀 IPC listener started");

    // Verify socket is ready
    tokio::time::sleep(Duration::from_millis(50)).await;
    if socket_exists(&socket_path) {
        println!("📡 Socket ready: {}", socket_path.display());
    }

    println!();
    println!("════════════════════════════════════════════════════════════════");
    println!("  Server is ready! Test with the client libraries:");
    println!();
    println!("  Python:");
    println!("    cd examples/ipc_client_libraries/python");
    println!("    python example_client.py");
    println!();
    println!("  Node.js:");
    println!("    cd examples/ipc_client_libraries/nodejs");
    println!("    npm install && npx ts-node src/example-client.ts");
    println!();
    println!("  Available services:");
    println!("    - calculator: Add {{ a, b }}, Multiply {{ a, b }}");
    println!("    - search: SearchQuery {{ query, limit }} (streaming)");
    println!("    - logger: LogEvent {{ level, message }} (fire-and-forget)");
    println!("    - Subscribe to: PriceUpdate, StatusChange");
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
