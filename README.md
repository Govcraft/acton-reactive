# Acton Reactive: Build Fast, Concurrent Rust Apps Easily

Welcome to Acton Reactive! This framework helps you build fast, concurrent Rust applications without getting tangled in complex threading or locking code.

Think of your application's logic broken down into independent workers called **Actors**. Each actor manages its own state and communicates with others by sending **Messages**. Acton Reactive handles the tricky parts of making these actors run concurrently and talk to each other efficiently, letting you focus on your application's features. It's built on top of [Tokio](https://tokio.rs/), Rust's powerful asynchronous runtime.

## Why Acton Reactive?

*   **Simplified Concurrency:** Forget manual thread management and complex locking. Actors run independently, managing their own data. Acton ensures messages are processed safely, making concurrent programming more approachable.
*   **Asynchronous & Performant:** Leverages Rust's `async/await` and Tokio for high-performance, non-blocking operations. Your application stays responsive, even under load.
*   **Organized & Maintainable Code:** Encourages breaking down complex problems into smaller, self-contained actors. This makes your codebase easier to understand, test, and maintain.
*   **Type-Safe Communication:** Define clear message types. Rust's compiler helps ensure you're sending and receiving the right kinds of messages, catching errors before runtime.
*   **Built-in Observability:** Integrates with the `tracing` crate, providing insights into your application's behavior for easier debugging and performance monitoring.

## Core Concepts Explained Simply

Before diving into code, let's understand the main building blocks:

1.  **Actor:** The fundamental unit. It's a Rust struct (that implements `Default` and `Debug`) which holds some internal state (its `model`) and reacts to incoming messages. Think of it as an independent worker or service.
2.  **Message:** A simple Rust struct (that implements `Debug` and `Clone`) used for communication. Actors send messages to other actors (or themselves) to trigger actions or share information. The `#[acton_message]` macro helps derive the required traits easily.
3.  **Handler:** A piece of code you define for an actor that specifies *how* it should react when it receives a particular type of message. There are two types:
    *   **`mutate_on`**: For operations that need to modify the actor's internal state (`&mut actor.model`). These run sequentially to ensure state consistency.
    *   **`act_on`**: For read-only operations that only need to inspect the actor's state (`&actor.model`). These run concurrently for better performance.
    Both types return an `ActorReply`.
4.  **Handle (`ActorHandle`):** An inexpensive, cloneable reference to an actor. You use an actor's handle to send messages *to* it from outside, or from other actors.
5.  **Runtime (`ActonApp` / `ActorRuntime`):** The Acton system environment. You launch it using `ActonApp::launch()`. It manages the actors, their communication channels, and the central message broker.

## Version 5.0 API Changes

**Acton Reactive v5.0 introduces breaking changes** to better support concurrent operations while maintaining state safety:

- **`act_on` is now for read-only concurrent handlers** - operates on `&actor.model` (immutable)
- **`mutate_on` is now for mutable sequential handlers** - operates on `&mut actor.model` (mutable)

### Migration Guide from v4.x

**Before (v4.x):**
```rust
builder.act_on::<MyMessage>(|actor, _| {
    actor.model.value += 1;  // mutation
    ActorReply::immediate()
});
```

**After (v5.0):**
```rust
// For mutations
builder.mutate_on::<MyMessage>(|actor, _| {
    actor.model.value += 1;  // mutation
    ActorReply::immediate()
});

// For read-only operations
builder.act_on::<QueryMessage>(|actor, _| {
    let value = actor.model.value;  // read-only
    ActorReply::immediate()
});
```

## Getting Started: A Basic Example

Let's build a simple counter actor.

1.  **Add Acton Reactive to your `Cargo.toml`:**

    ```toml
    [dependencies]
    acton_reactive = { version = "1.1.0-beta.1" } # Use the latest version
    tokio = { version = "1", features = ["full"] } # Acton requires a Tokio runtime
    anyhow = "1" # Useful for error handling in main
    ```

2.  **Write the code (`src/main.rs`):**

    ```rust
    use acton_reactive::prelude::*;
    use std::time::Duration;
    use anyhow::Result;

    // 1. Define the Actor's state (must be Default + Debug)
    #[derive(Debug, Default)]
    struct CounterActor {
        count: i32,
    }

    // 2. Define Messages (must be Debug + Clone)
    // Use the macro for convenience!
    #[acton_message]
    struct IncrementMsg;

    #[acton_message]
    struct PrintMsg;

    // 3. The main async function (requires a Tokio runtime)
    #[tokio::main]
    async fn main() -> Result<()> {
        println!("Launching Acton application...");

        // 4. Launch the Acton Runtime
        let mut app = ActonApp::launch();

        // 5. Create an Actor Builder
        // This prepares an actor but doesn't start its processing loop yet.
        let mut counter_builder = app.new_actor::<CounterActor>();
        println!("Created actor builder for: {}", counter_builder.id());

        // 6. Define Message Handlers
        // For state mutations, use mutate_on (sequential execution)
        counter_builder
            .mutate_on::<IncrementMsg>(|actor, _context| {
                // This code runs when the actor receives an IncrementMsg.
                // We can safely mutate the actor's internal state (`model`).
                actor.model.count += 1;
                println!("Actor {}: Incremented count to {}", actor.id(), actor.model.count);
                // No async work needed here, return immediately.
                ActorReply::immediate()
            })
            // For read-only operations, use act_on (concurrent execution)
            .act_on::<PrintMsg>(|actor, _context| {
                // This code runs when the actor receives a PrintMsg.
                // This is a read-only operation - we only read the state
                println!("Actor {}: Current count is {}", actor.id(), actor.model.count);
                // We can also perform async operations within a handler.
                ActorReply::from_async(async move {
                    // Example: Simulate some async work
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    println!("Actor {}: Finished async work in PrintMsg handler.", actor.id());
                    // No need to explicitly send anything back here for this example.
                })
            })
            // Optional: Define lifecycle hooks
            .after_stop(|actor| {
                 println!("Actor {}: Final count is {}. Stopping.", actor.id(), actor.model.count);
                 ActorReply::immediate()
            });

        // 7. Start the Actor
        // This spawns the actor's task and returns a handle for sending messages.
        let counter_handle = counter_builder.start().await;
        println!("Started actor: {}", counter_handle.id());

        // 8. Send Messages using the Handle
        println!("Sending IncrementMsg...");
        counter_handle.send(IncrementMsg).await;

        println!("Sending PrintMsg...");
        counter_handle.send(PrintMsg).await;

        println!("Sending another IncrementMsg...");
        counter_handle.send(IncrementMsg).await;

        // Give the actor a moment to process the last message and its async handler
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 9. Shut down the application gracefully
        // This stops all actors and waits for them to finish.
        println!("Shutting down application...");
        app.shutdown_all().await?;
        println!("Application shut down.");

        Ok(())
    }
    ```

3.  **Run it:** `cargo run`

You should see output showing the actor being created, handling messages, incrementing its count, and finally stopping.

## Common Patterns

While the example above covers the basics, Acton Reactive supports more patterns:

*   **Replying to Messages:** Inside a handler, use `context.reply_envelope()` to get an envelope addressed back to the original sender, then use `.send(YourReplyMessage).await`.
*   **Sending to Specific Actors:** If an actor has the `ActorHandle` of another actor, it can create a new envelope using `context.new_envelope(&target_handle.reply_address())` and then `.send(YourMessage).await`.
*   **Asynchronous Operations:** As shown in the `PrintMsg` handler, use `ActorReply::from_async(async move { ... })` to perform non-blocking tasks (like I/O) within your handlers.
*   **Lifecycle Hooks:** Use `.before_start()`, `.after_start()`, `.before_stop()`, and `.after_stop()` on the actor builder to run code during actor initialization or shutdown.
*   **Publish/Subscribe (Broadcasting):** Actors can subscribe to specific message types using `actor_handle.subscribe::<MyMessageType>().await`. Anyone (often the central `ActorBroker` obtained via `app.broker()` or `actor.broker()`) can then `broadcast(MyMessageType)` to notify all subscribers. This is great for system-wide events.
*   **Supervision (Parent/Child Actors):** Actors can create and manage child actors using `actor_handle.supervise(child_builder).await`. Stopping the parent will automatically stop its children.

## Configuration

Acton Reactive supports configuration via TOML files using the XDG Base Directory Specification:

- **Linux**: `~/.config/acton/config.toml`
- **macOS**: `~/Library/Application Support/acton/config.toml`
- **Windows**: `%APPDATA%\acton\config.toml`

### Quick Configuration Setup

1. **Copy example configuration**:
   ```bash
   cp examples/config.toml ~/.config/acton/config.toml
   ```

2. **Customize settings**:
   ```toml
   [timeouts]
   actor_shutdown = 5000
   system_shutdown = 15000

   [limits]
   actor_inbox_capacity = 512
   concurrent_handlers_high_water_mark = 50

   [tracing]
   debug = "info"
   ```

3. **Verify configuration**:
   ```bash
   cargo run --example configuration
   ```

### Configuration Categories

- **Timeouts**: Actor and system shutdown timeouts
- **Limits**: Buffer sizes and capacity limits
- **Defaults**: Default actor names and identifiers
- **Tracing**: Logging levels and configuration
- **Paths**: Custom directory locations
- **Behavior**: Feature toggles and settings

See `docs/CONFIGURATION.md` for the complete configuration guide.

## Explore More Examples

For more detailed examples demonstrating patterns like broadcasting, replies, actor lifecycles, and configuration usage, check out the `acton-reactive/examples/` directory in this repository.

## Contributing

Contributions are welcome! Feel free to submit issues, fork the repository, and send pull requests. Let's make Acton Reactive even better together!

## License

This project is licensed under either of:

*   Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
*   MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
## Sponsor

Govcraft is a one-person shop—no corporate backing, no investors, just me building useful tools. If this project helps you, [sponsoring](https://github.com/sponsors/Govcraft) keeps the work going.

[![Sponsor on GitHub](https://img.shields.io/badge/Sponsor-%E2%9D%A4-%23db61a2?logo=GitHub)](https://github.com/sponsors/Govcraft)
