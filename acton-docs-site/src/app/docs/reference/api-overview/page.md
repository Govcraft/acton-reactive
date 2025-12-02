---
title: API Overview
description: Quick reference for core types, traits, and macros.
---

All the key types, traits, and macros in one place. For complete API documentation, see [docs.rs/acton-reactive](https://docs.rs/acton-reactive).

## Core Types

### ActonApp

The actor system runtime.

```rust
let runtime = ActonApp::launch_async().await;
```

| Method | Description |
|--------|-------------|
| `launch_async().await` | Start a new actor system |
| `new_actor::<T>()` | Create an actor builder |
| `new_actor_with_name::<T>(name)` | Create a named actor builder |
| `shutdown_all().await` | Graceful shutdown |
| `broker()` | Access the message broker |
| `ipc_registry()` | Access IPC type registry |
| `ipc_expose(name, handle)` | Expose actor for IPC |
| `start_ipc_listener().await` | Start IPC listener |

### Actor Builder

Configures actors before spawning.

```rust
let handle = runtime
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
| `before_start(hook)` | Lifecycle hook before start |
| `after_stop(hook)` | Lifecycle hook after stop |
| `start().await` | Spawn the actor |
| `handle()` | Get handle before starting |

### ActorHandle

Reference to a running actor.

```rust
handle.send(Message).await;
handle.stop().await.ok();
```

| Method | Description |
|--------|-------------|
| `send(msg).await` | Fire-and-forget message |
| `stop().await` | Stop the actor |
| `subscribe::<M>().await` | Subscribe to broadcast messages |
| `reply_address()` | Get address for replies |
| `create_envelope(target)` | Create envelope for sending |
| `supervise(child).await` | Start and supervise a child |
| `id()` | Get actor's identifier |

### Reply

Handler return type.

| Method | Description |
|--------|-------------|
| `Reply::ready()` | Synchronous completion |
| `Reply::pending(future)` | Async completion |
| `Reply::try_ok(value)` | For fallible handlers |

### Envelope

Message wrapper with routing information.

| Method | Description |
|--------|-------------|
| `message()` | Access the message data |
| `reply_envelope()` | Create envelope back to sender |
| `new_envelope(&address)` | Create envelope to different recipient |
| `send(msg).await` | Send via this envelope |

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

#[acton_message(ipc)]  // Enable IPC serialization
struct GetValue;
```

### #[acton_main]

Sets up the async runtime.

```rust
#[acton_main]
async fn main() {
    let runtime = ActonApp::launch_async().await;
    // ...
}
```

---

## Prelude

Import everything:

```rust
use acton_reactive::prelude::*;
```

Includes: `ActonApp`, `ActorHandle`, `Reply`, all macros.
