# Acton Reactive: Build Fast, Concurrent Rust Apps Easily

Welcome to Acton Reactive! This framework helps you build fast, concurrent Rust applications without getting tangled in complex threading or locking code.

Think of your application's logic broken down into independent workers called **Agents**. Each agent manages its own state and communicates with others by sending **Messages**. Acton Reactive handles the tricky parts of making these agents run concurrently and talk to each other efficiently, letting you focus on your application's features. It's built on top of [Tokio](https://tokio.rs/), Rust's powerful asynchronous runtime.

## Why Acton Reactive?

*   **Simplified Concurrency:** Forget manual thread management and complex locking. Agents run independently, managing their own data. Acton ensures messages are processed safely, making concurrent programming more approachable.
*   **Asynchronous & Performant:** Leverages Rust's `async/await` and Tokio for high-performance, non-blocking operations. Your application stays responsive, even under load.
*   **Organized & Maintainable Code:** Encourages breaking down complex problems into smaller, self-contained agents. This makes your codebase easier to understand, test, and maintain.
*   **Type-Safe Communication:** Define clear message types. Rust's compiler helps ensure you're sending and receiving the right kinds of messages, catching errors before runtime.
*   **Built-in Observability:** Integrates with the `tracing` crate, providing insights into your application's behavior for easier debugging and performance monitoring.

## Core Concepts Explained Simply

Before diving into code, let's understand the main building blocks:

1.  **Agent:** The fundamental unit. It's a Rust struct (that implements `Default` and `Debug`) which holds some internal state (its `model`) and reacts to incoming messages. Think of it as an independent worker or service.
2.  **Message:** A simple Rust struct (that implements `Debug` and `Clone`) used for communication. Agents send messages to other agents (or themselves) to trigger actions or share information. The `#[acton_message]` macro helps derive the required traits easily.
3.  **Handler:** A piece of code you define for an agent that specifies *how* it should react when it receives a particular type of message. There are two types:
    *   **`mutate_on`**: For operations that need to modify the agent's internal state (`&mut agent.model`). These run sequentially to ensure state consistency.
    *   **`act_on`**: For read-only operations that only need to inspect the agent's state (`&agent.model`). These run concurrently for better performance.
    Both types return an `AgentReply`.
4.  **Handle (`AgentHandle`):** An inexpensive, cloneable reference to an agent. You use an agent's handle to send messages *to* it from outside, or from other agents.
5.  **Runtime (`ActonApp` / `AgentRuntime`):** The Acton system environment. You launch it using `ActonApp::launch()`. It manages the agents, their communication channels, and the central message broker.

## Version 5.0 API Changes

**Acton Reactive v5.0 introduces breaking changes** to better support concurrent operations while maintaining state safety:

- **`act_on` is now for read-only concurrent handlers** - operates on `&agent.model` (immutable)
- **`mutate_on` is now for mutable sequential handlers** - operates on `&mut agent.model` (mutable)

### Migration Guide from v4.x

**Before (v4.x):**
```rust
builder.act_on::<MyMessage>(|agent, _| {
    agent.model.value += 1;  // mutation
    AgentReply::immediate()
});
```

**After (v5.0):**
```rust
// For mutations
builder.mutate_on::<MyMessage>(|agent, _| {
    agent.model.value += 1;  // mutation
    AgentReply::immediate()
});

// For read-only operations
builder.act_on::<QueryMessage>(|agent, _| {
    let value = agent.model.value;  // read-only
    AgentReply::immediate()
});
```

## Getting Started: A Basic Example

Let's build a simple counter agent.

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

    // 1. Define the Agent's state (must be Default + Debug)
    #[derive(Debug, Default)]
    struct CounterAgent {
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

        // 5. Create an Agent Builder
        // This prepares an agent but doesn't start its processing loop yet.
        let mut counter_builder = app.new_agent::<CounterAgent>().await;
        println!("Created agent builder for: {}", counter_builder.id());

        // 6. Define Message Handlers
        // For state mutations, use mutate_on (sequential execution)
        counter_builder
            .mutate_on::<IncrementMsg>(|agent, _context| {
                // This code runs when the agent receives an IncrementMsg.
                // We can safely mutate the agent's internal state (`model`).
                agent.model.count += 1;
                println!("Agent {}: Incremented count to {}", agent.id(), agent.model.count);
                // No async work needed here, return immediately.
                AgentReply::immediate()
            })
            // For read-only operations, use act_on (concurrent execution)
            .act_on::<PrintMsg>(|agent, _context| {
                // This code runs when the agent receives a PrintMsg.
                // This is a read-only operation - we only read the state
                println!("Agent {}: Current count is {}", agent.id(), agent.model.count);
                // We can also perform async operations within a handler.
                AgentReply::from_async(async move {
                    // Example: Simulate some async work
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    println!("Agent {}: Finished async work in PrintMsg handler.", agent.id());
                    // No need to explicitly send anything back here for this example.
                })
            })
            // Optional: Define lifecycle hooks
            .after_stop(|agent| {
                 println!("Agent {}: Final count is {}. Stopping.", agent.id(), agent.model.count);
                 AgentReply::immediate()
            });

        // 7. Start the Agent
        // This spawns the agent's task and returns a handle for sending messages.
        let counter_handle = counter_builder.start().await;
        println!("Started agent: {}", counter_handle.id());

        // 8. Send Messages using the Handle
        println!("Sending IncrementMsg...");
        counter_handle.send(IncrementMsg).await;

        println!("Sending PrintMsg...");
        counter_handle.send(PrintMsg).await;

        println!("Sending another IncrementMsg...");
        counter_handle.send(IncrementMsg).await;

        // Give the agent a moment to process the last message and its async handler
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 9. Shut down the application gracefully
        // This stops all agents and waits for them to finish.
        println!("Shutting down application...");
        app.shutdown_all().await?;
        println!("Application shut down.");

        Ok(())
    }
    ```

3.  **Run it:** `cargo run`

You should see output showing the agent being created, handling messages, incrementing its count, and finally stopping.

## Common Patterns

While the example above covers the basics, Acton Reactive supports more patterns:

*   **Replying to Messages:** Inside a handler, use `context.reply_envelope()` to get an envelope addressed back to the original sender, then use `.send(YourReplyMessage).await`.
*   **Sending to Specific Agents:** If an agent has the `AgentHandle` of another agent, it can create a new envelope using `context.new_envelope(&target_handle.reply_address())` and then `.send(YourMessage).await`.
*   **Asynchronous Operations:** As shown in the `PrintMsg` handler, use `AgentReply::from_async(async move { ... })` to perform non-blocking tasks (like I/O) within your handlers.
*   **Lifecycle Hooks:** Use `.before_start()`, `.after_start()`, `.before_stop()`, and `.after_stop()` on the agent builder to run code during agent initialization or shutdown.
*   **Publish/Subscribe (Broadcasting):** Agents can subscribe to specific message types using `agent_handle.subscribe::<MyMessageType>().await`. Anyone (often the central `AgentBroker` obtained via `app.broker()` or `agent.broker()`) can then `broadcast(MyMessageType)` to notify all subscribers. This is great for system-wide events.
*   **Supervision (Parent/Child Agents):** Agents can create and manage child agents using `agent_handle.supervise(child_builder).await`. Stopping the parent will automatically stop its children.

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
   agent_shutdown = 5000
   system_shutdown = 15000

   [limits]
   agent_inbox_capacity = 512
   concurrent_handlers_high_water_mark = 50

   [tracing]
   debug = "info"
   ```

3. **Verify configuration**:
   ```bash
   cargo run --example configuration
   ```

### Configuration Categories

- **Timeouts**: Agent and system shutdown timeouts
- **Limits**: Buffer sizes and capacity limits
- **Defaults**: Default agent names and identifiers
- **Tracing**: Logging levels and configuration
- **Paths**: Custom directory locations
- **Behavior**: Feature toggles and settings

See `docs/CONFIGURATION.md` for the complete configuration guide.

## Explore More Examples

For more detailed examples demonstrating patterns like broadcasting, replies, agent lifecycles, and configuration usage, check out the `acton-reactive/examples/` directory in this repository.

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
