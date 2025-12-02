# Acton Reactive

[![Crates.io](https://img.shields.io/crates/v/acton-reactive.svg)](https://crates.io/crates/acton-reactive)
[![Documentation](https://docs.rs/acton-reactive/badge.svg)](https://docs.rs/acton-reactive)
[![License](https://img.shields.io/crates/l/acton-reactive.svg)](LICENSE-MIT)

**Concurrent Rust made simple.** Build fast, responsive applications using actors—independent workers that manage their own state and communicate through messages.

```rust
use acton_reactive::prelude::*;

#[acton_actor]
struct Counter { count: i32 }

#[acton_message]
struct Increment;

#[acton_main]
async fn main() {
    let mut app = ActonApp::launch_async().await;
    let mut counter = app.new_actor::<Counter>();

    counter.mutate_on::<Increment>(|actor, _| {
        actor.model.count += 1;
        println!("Count: {}", actor.model.count);
        Reply::ready()
    });

    let handle = counter.start().await;
    handle.send(Increment).await;

    app.shutdown_all().await.unwrap();
}
```

No locks. No shared state. No data races.

---

## Why Actors?

Concurrent Rust is powerful but demanding. The actor model offers a simpler mental model:

- **Isolated state** — Each actor owns its data exclusively. No `Arc<Mutex<T>>`, no data races.
- **Message passing** — Actors communicate by sending messages. The framework handles delivery and ordering.
- **Natural concurrency** — Actors run independently. Scale by adding more actors, not by managing threads.

Acton Reactive makes this pattern accessible with derive macros that eliminate boilerplate while Rust's type system enforces safety at compile time.

---

## Features

- **Derive macros** — `#[acton_actor]` and `#[acton_message]` generate the required trait implementations
- **Two handler types** — `mutate_on` for state changes (sequential), `act_on` for reads (concurrent)
- **Async-native** — Built on Tokio for efficient, non-blocking operations
- **Lifecycle hooks** — `before_start`, `after_stop` and more for resource management
- **Pub/sub messaging** — Actors can subscribe to message types and receive broadcasts
- **Request/reply** — Send a message and get a response using reply envelopes
- **Supervision** — Parent actors can manage child actors for fault tolerance

---

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
acton-reactive = "0.7"
tokio = { version = "1", features = ["full"] }
```

---

## Getting Started

Here's a complete example showing the core patterns:

```rust
use acton_reactive::prelude::*;

// Define actor state with the macro
#[acton_actor]
struct Greeter {
    greet_count: usize,
}

// Define messages with the macro
#[acton_message]
struct Greet { name: String }

#[acton_message]
struct GetCount;

#[acton_main]
async fn main() {
    // Launch the runtime
    let mut app = ActonApp::launch_async().await;

    // Create and configure the actor
    let mut greeter = app.new_actor::<Greeter>();

    greeter
        // mutate_on: for handlers that modify state (runs sequentially)
        .mutate_on::<Greet>(|actor, envelope| {
            actor.model.greet_count += 1;
            println!("Hello, {}! (greeting #{})",
                envelope.message.name,
                actor.model.greet_count);
            Reply::ready()
        })
        // act_on: for read-only handlers (can run concurrently)
        .act_on::<GetCount>(|actor, _| {
            println!("Total greetings: {}", actor.model.greet_count);
            Reply::ready()
        });

    // Start the actor and get a handle for sending messages
    let handle = greeter.start().await;

    // Send messages
    handle.send(Greet { name: "World".into() }).await;
    handle.send(Greet { name: "Rust".into() }).await;
    handle.send(GetCount).await;

    // Clean shutdown
    app.shutdown_all().await.expect("shutdown failed");
}
```

**Output:**
```
Hello, World! (greeting #1)
Hello, Rust! (greeting #2)
Total greetings: 2
```

---

## Core Concepts

| Concept | Description |
|---------|-------------|
| **Actor** | A struct decorated with `#[acton_actor]` that holds state (`model`) and responds to messages |
| **Message** | A struct decorated with `#[acton_message]` that actors send to communicate |
| **Handler** | Code that runs when an actor receives a specific message type |
| **Handle** | A lightweight, cloneable reference used to send messages to an actor |
| **Runtime** | The `ActonApp` that manages actors and their communication |

### Handler Types

- **`mutate_on<M>`** — Use when the handler needs to modify `actor.model`. Runs sequentially to ensure state consistency.
- **`act_on<M>`** — Use for read-only operations on `actor.model`. Can run concurrently for better performance.

---

## When to Use Acton

**Good fit:**
- Stateful services with complex internal logic
- Event-driven systems with clear component boundaries
- Applications modeling entities (users, sessions, devices, game objects)
- Systems needing fault isolation between components

**Consider alternatives:**
- Pure computation without state → async functions
- Simple shared counters → `Arc<AtomicUsize>`
- Request-response without state → direct function calls

---

## Learn More

- **[API Documentation](https://docs.rs/acton-reactive)** — Complete reference
- **[Examples](acton-reactive/examples/)** — Working code for common patterns
- **[Configuration Guide](docs/CONFIGURATION.md)** — Customize timeouts, limits, and tracing

---

## Contributing

Contributions welcome! Open an issue to discuss changes, then submit a pull request.

---

## Sponsor

Govcraft is a one-person shop—no corporate backing, no investors, just me building useful tools. If this project helps you, [sponsoring](https://github.com/sponsors/Govcraft) keeps the work going.

[![Sponsor on GitHub](https://img.shields.io/badge/Sponsor-%E2%9D%A4-%23db61a2?logo=GitHub)](https://github.com/sponsors/Govcraft)

---

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
