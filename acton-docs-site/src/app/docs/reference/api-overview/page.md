---
title: API Overview
description: Quick reference for core types, traits, and macros.
---

All the key types, traits, and macros in one place. For complete API documentation, see [docs.rs/acton-reactive](https://docs.rs/acton-reactive).

## Core Types

### ActonApp

The actor system runtime.

```rust
let app = ActonApp::launch();
```

| Method | Description |
|--------|-------------|
| `launch()` | Start a new actor system |
| `new_actor::<T>()` | Create an AgentBuilder |
| `shutdown_all()` | Graceful shutdown |
| `get_broker()` | Access the message broker |

### AgentBuilder

Configures actors before spawning.

```rust
let handle = app
    .new_actor::<Counter>()
    .mutate_on::<Increment>(handler)
    .act_on::<GetCount>(handler)
    .start()
    .await;
```

| Method | Description |
|--------|-------------|
| `mutate_on::<M>(handler)` | Register state-changing handler |
| `act_on::<M>(handler)` | Register read-only handler |
| `after_start(hook)` | Lifecycle hook |
| `start()` | Spawn the actor |

### AgentHandle

Reference to a running actor.

```rust
handle.send(Message).await;
let result = handle.ask(Query).await;
```

| Method | Description |
|--------|-------------|
| `send(msg)` | Fire-and-forget |
| `ask(msg)` | Request-response |
| `stop()` | Stop the actor |
| `subscribe::<M>()` | Subscribe to broadcast messages |

### Reply

Handler return type.

| Method | Description |
|--------|-------------|
| `Reply::ready()` | No response |
| `Reply::with(value)` | Return a value |
| `Reply::pending(future)` | Async response |

---

## Macros

### #[acton_actor]

Marks a struct as actor state.

```rust
#[acton_actor]
struct Counter {
    count: i32,
}
```

### #[acton_message]

Marks a struct as a message.

```rust
#[acton_message]
struct Increment;

#[acton_message(ipc)]  // Enable IPC
struct GetValue;
```

### #[acton_main]

Sets up the async runtime.

```rust
#[acton_main]
async fn main() {
    let app = ActonApp::launch();
    // ...
}
```

---

## Prelude

Import everything:

```rust
use acton_reactive::prelude::*;
```

Includes: `ActonApp`, `AgentBuilder`, `AgentHandle`, `Reply`, all macros.
