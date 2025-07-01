# Acton Reactive: Build Fast, Concurrent Rust Apps Easily

Welcome to Acton Reactive! This framework helps you build fast, concurrent Rust applications without getting tangled in complex threading or locking code.

Think of your application's logic broken down into independent workers called **Agents**. Each agent manages its own state and communicates with others by sending **Messages**. Acton Reactive handles the tricky parts of making these agents run concurrently and talk to each other efficiently, letting you focus on your application's features. It's built on top of [Tokio](https://tokio.rs/), Rust's powerful asynchronous runtime.

## Why Acton Reactive?

*   **Simplified Concurrency:** Forget manual thread management and complex locking. Agents run independently, managing their own data. Acton ensures messages are processed safely, making concurrent programming more approachable.
*   **Asynchronous & Performant:** Leverages Rust's `async/await` and Tokio for high-performance, non-blocking operations. Your application stays responsive, even under load.
*   **Organized & Maintainable Code:** Encourages breaking down complex problems into smaller, self-contained agents. This makes your codebase easier to understand, test, and maintain.
*   **Type-Safe Communication:** Define clear message types. Rust's compiler helps ensure you're sending and receiving the right kinds of messages, catching errors before runtime.
*   **Context-Aware Error Handling:** Register specific error handlers for fallible operations. When an error occurs, your handler receives both the error and the context of the message that caused it, enabling robust recovery patterns.
*   **Built-in Observability:** Integrates with the `tracing` crate, providing insights into your application's behavior for easier debugging and performance monitoring.

## Core Concepts Explained Simply

Before diving into code, let's understand the main building blocks:

1.  **Agent:** The fundamental unit. It's a Rust struct (that implements `Default` and `Debug`) which holds some internal state (its `model`) and reacts to incoming messages. Think of it as an independent worker or service.
2.  **Message:** A simple Rust struct (that implements `Debug` and `Clone`) used for communication. Agents send messages to other agents (or themselves) to trigger actions or share information. The `#[acton_message]` macro helps derive the required traits easily.
3.  **Handler (`act_on`, `act_on_fallible`):** A piece of code you define for an agent that specifies *how* it should react to a message.
    *   `act_on`: For operations that cannot fail.
    *   `act_on_fallible`: For operations that return a `Result`. If it returns an `Err`, Acton will look for a corresponding error handler.
4.  **Error Handler (`on_error`):** A handler that executes when a specific `(Message, Error)` pair occurs in a fallible handler. It receives the original message context and the concrete error, allowing you to handle failures with full context.
5.  **Handle (`AgentHandle`):** An inexpensive, cloneable reference to an agent. You use an agent's handle to send messages *to* it from outside, or from other agents.
6.  **Runtime (`ActonApp` / `AgentRuntime`):** The Acton system environment. You launch it using `ActonApp::launch()`. It manages the agents, their communication channels, and the central message broker.

## Getting Started: A Basic Example

Let's build a simple counter agent that can also handle errors.

1.  **Add Acton Reactive to your `Cargo.toml`:**

    ```toml
    [dependencies]
    acton_reactive = { version = "1.1.0-beta.1" } # Use the latest version
    tokio = { version = "1", features = ["full"] } # Acton requires a Tokio runtime
    anyhow = "1" # Useful for error handling in main
    thiserror = "1" # For creating custom error types
    ```

2.  **Write the code (`src/main.rs`):**

    ```rust
    use acton_reactive::prelude::*;
    use std::time::Duration;
    use anyhow::Result;
    use thiserror::Error;

    // 1. Define the Agent's state
    #[derive(Debug, Default)]
    struct CounterAgent {
        count: i32,
        error_count: i32,
    }

    // 2. Define Messages
    #[acton_message]
    struct IncrementMsg;

    #[acton_message]
    struct FailMsg;

    // 3. Define a custom Error type
    #[derive(Error, Debug, Clone)]
    #[error("A deliberate failure has occurred!")]
    struct DeliberateError;


    // 4. The main async function
    #[tokio::main]
    async fn main() -> Result<()> {
        println!("Launching Acton application...");

        // 5. Launch the Acton Runtime
        let mut app = ActonApp::launch();

        // 6. Create an Agent Builder
        let mut counter_builder = app.new_agent::<CounterAgent>().await;
        println!("Created agent builder for: {}", counter_builder.id());

        // 7. Define Message and Error Handlers
        counter_builder
            // --- Handler for successful increments ---
            .act_on_fallible::<IncrementMsg, (), DeliberateError>(|agent, _context| {
                agent.model.count += 1;
                println!("Agent {}: Incremented count to {}", agent.id(), agent.model.count);
                // This operation succeeds, so we return Ok.
                Box::pin(async { Ok(()) })
            })
            // --- Handler for messages that will fail ---
            .act_on_fallible::<FailMsg, (), DeliberateError>(|agent, _context| {
                println!("Agent {}: Received FailMsg, preparing to fail...", agent.id());
                // This operation fails, so we return an Err.
                Box::pin(async { Err(DeliberateError) })
            })
            // --- Error handler for when FailMsg results in DeliberateError ---
            .on_error::<FailMsg, DeliberateError>(|agent, context, error| {
                // This code runs only when a FailMsg handler returns a DeliberateError.
                // We have the original message context and the specific error!
                agent.model.error_count += 1;
                println!(
                    "Agent {}: Handled error '{}' for message {:?}. Total errors: {}",
                    agent.id(),
                    error,
                    context.message,
                    agent.model.error_count
                );
                AgentReply::immediate()
            })
            // Optional: Define lifecycle hooks
            .after_stop(|agent| {
                 println!(
                    "Agent {}: Final count is {}. Total errors handled: {}. Stopping.",
                    agent.id(),
                    agent.model.count,
                    agent.model.error_count
                );
                 AgentReply::immediate()
            });

        // 8. Start the Agent
        let counter_handle = counter_builder.start().await;
        println!("Started agent: {}", counter_handle.id());

        // 9. Send Messages
        println!("\nSending IncrementMsg (will succeed)...");
        counter_handle.send(IncrementMsg).await;

        println!("\nSending FailMsg (will trigger error handler)...");
        counter_handle.send(FailMsg).await;

        println!("\nSending another IncrementMsg (will succeed)...");
        counter_handle.send(IncrementMsg).await;

        // Give the agent a moment to process everything
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 10. Shut down the application gracefully
        println!("\nShutting down application...");
        app.shutdown_all().await?;
        println!("Application shut down.");

        Ok(())
    }
    ```

3.  **Run it:** `cargo run`

You'll see the agent process both successful and failing messages, with the specific error handler being triggered only for the `FailMsg`.

## Common Patterns

While the example above covers the basics, Acton Reactive supports more patterns:

*   **Replying to Messages:** Inside a handler, use `context.reply_envelope()` to get an envelope addressed back to the original sender, then use `.send(YourReplyMessage).await`.
*   **Sending to Specific Agents:** If an agent has the `AgentHandle` of another agent, it can create a new envelope using `context.new_envelope(&target_handle.reply_address())` and then `.send(YourMessage).await`.
*   **Asynchronous Operations:** As shown in the example, you can perform non-blocking tasks (like I/O) within your handlers by returning a pinned `Future`.
*   **Lifecycle Hooks:** Use `.before_start()`, `.after_start()`, `.before_stop()`, and `.after_stop()` on the agent builder to run code during agent initialization or shutdown.
*   **Publish/Subscribe (Broadcasting):** Agents can subscribe to specific message types using `agent_handle.subscribe::<MyMessageType>().await`. Anyone (often the central `AgentBroker` obtained via `app.broker()` or `agent.broker()`) can then `broadcast(MyMessageType)` to notify all subscribers. This is great for system-wide events.
*   **Supervision (Parent/Child Agents):** Agents can create and manage child agents using `agent_handle.supervise(child_builder).await`. Stopping the parent will automatically stop its children.

## Explore More Examples

For more detailed examples demonstrating patterns like broadcasting, replies, and agent lifecycles, check out the `acton-reactive/examples/` directory in this repository.

## Contributing

Contributions are welcome! Feel free to submit issues, fork the repository, and send pull requests. Let's make Acton Reactive even better together!

## License

This project is licensed under either of:

*   Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
*   MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
