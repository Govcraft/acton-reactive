# Acton Reactive

`acton-reactive` is the main crate of the [Acton Reactive Application Framework](https://github.com/Govcraft/acton-reactive), designed for building reactive, event-driven, and asynchronous systems in Rust.

## Key Features

- **Agent-Based Architecture**: Build systems using agents (actors) that communicate via message passing.
- **Async First**: Built on top of Tokio for highly concurrent and efficient applications.
- **Type-Safe Lifecycle**: Type-state pattern ensures agents follow proper lifecycle transitions.
- **Pub/Sub Messaging**: Built-in broker for topic-based message distribution.
- **Supervision**: Hierarchical agent supervision for fault-tolerant systems.

## Quick Start

```rust
use acton_reactive::prelude::*;

#[acton_message]
struct Greeting {
    name: String,
}

#[acton_actor]
#[derive(Default)]
struct Greeter {
    greetings: usize,
}
```

## Documentation

For more details on how the Acton framework works and examples of how to build reactive applications, refer to the [Acton Reactive documentation](https://github.com/Govcraft/acton-reactive/blob/main/README.md).

## License

This project is licensed under both the MIT and Apache-2.0 licenses. See the `LICENSE-MIT` and `LICENSE-APACHE` files for details.
