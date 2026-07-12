---
title: API Overview
description: Quick reference for core types, traits, and macros.
---

All the key types, traits, and macros in one place. For complete API documentation, see [docs.rs/acton-reactive](https://docs.rs/acton-reactive).

## Core Types

### ActonApp

The entry point. Its only job is to boot the system and hand you an `ActorRuntime`.

```rust
let mut runtime = ActonApp::launch_async().await;
```

| Method | Description |
|--------|-------------|
| `launch_async().await` | Start a new actor system (use this in async contexts) |
| `launch()` | Start from a **sync** context. Panics if called inside a Tokio runtime. |

### ActorRuntime

What `launch_async()` returns. This is where actor creation, the broker, shutdown, and IPC live.

| Method | Description |
|--------|-------------|
| `new_actor::<T>()` | Create an actor builder (default name) |
| `new_actor_with_name::<T>(name)` | Create a named actor builder |
| `new_actor_with_config::<T>(config)` | Create a builder from an `ActorConfig` |
| `spawn_actor::<T>(config, setup)` | Create, configure, and start in one call |
| `spawn_actor_with_setup_fn::<T>(config, setup_fn)` | As above, with an async setup closure |
| `broker()` | Access the message broker |
| `actor_count()` | Number of top-level actors |
| `shutdown_all().await` | Graceful shutdown of the whole system |
| `ipc_registry()` | Access IPC type registry |
| `ipc_expose(name, handle)` | Expose actor for IPC |
| `ipc_hide(name)` | Remove IPC exposure |
| `ipc_lookup(name)` | Find an IPC-exposed actor |
| `start_ipc_listener().await` | Start IPC listener (default config) |
| `start_ipc_listener_with_config(cfg).await` | Start IPC listener with custom config |

IPC methods require the `ipc` feature.

### Actor Builder

`ManagedActor<Idle, T>` — configures an actor before spawning.

Handler-registration methods take `&mut self` and return `&mut Self`, while `start()` **consumes** the builder. So configure first, then start — you can't chain straight through into `.start()`:

```rust
let mut builder = runtime.new_actor::<Counter>();
builder
    .mutate_on::<Increment>(handler)
    .act_on::<GetCount>(handler);

let handle = builder.start().await;
```

| Method | Description |
|--------|-------------|
| `mutate_on::<M>(handler)` | Register async state-changing handler |
| `mutate_on_sync::<M>(handler)` | Register sync state-changing handler (no future allocation) |
| `act_on::<M>(handler)` | Register async read-only handler |
| `act_on_sync::<M>(handler)` | Register sync read-only handler (no future allocation) |
| `try_mutate_on::<M, T, E>(handler)` | Async state-changing handler returning `Result<T, E>` |
| `try_act_on::<M, T, E>(handler)` | Async read-only handler returning `Result<T, E>` |
| `on_error::<M, E>(handler)` | Handle error `E` returned by a `try_*` handler for message `M` |
| `before_start(hook)` | Lifecycle hook before the message loop starts |
| `after_start(hook)` | Lifecycle hook after the message loop starts |
| `before_stop(hook)` | Lifecycle hook before shutdown begins |
| `after_stop(hook)` | Lifecycle hook after the message loop ends |
| `create_child(name)` | Build a child actor under this one (**`Idle` only** — not available inside a handler) |
| `expose_for_ipc()` | Expose actor for IPC using its ERN root name |
| `start().await` | Spawn the actor, consuming the builder; returns its `ActorHandle` |
| `handle()` | Get the handle before starting |

### ActorHandle

Reference to a running actor.

```rust
handle.send(Message).await;
handle.stop().await.ok();
```

| Method | Description |
|--------|-------------|
| `send(msg).await` | Fire-and-forget message |
| `broadcast(msg).await` | Publish to the broker for all subscribers |
| `stop().await` | Stop the actor |
| `subscribe::<M>().await` | Subscribe to broadcast messages |
| `unsubscribe::<M>()` | Unsubscribe from broadcast messages (fire-and-forget) |
| `unsubscribe_async::<M>().await` | Unsubscribe, awaiting delivery of the request to the broker |
| `reply_address()` | Get this actor's address, for use as a return address |
| `create_envelope(recipient)` | Create an envelope **from** this actor **to** `recipient` |
| `supervise(child).await` | Start a child and register it under this actor |
| `children()` | The map of supervised children |
| `find_child(&ern)` | Look up a direct child by ERN |
| `id()` | Get actor's identifier (`Ern`) |
| `name()` | Get actor's root name |

### Reply

Builds the future a handler must return.

| Method | Used with | Description |
|--------|-----------|-------------|
| `Reply::ready()` | `mutate_on`, `act_on` | Complete immediately, no async work |
| `Reply::pending(future)` | `mutate_on`, `act_on` | Wrap an async block |
| `Reply::try_pending(future)` | `try_mutate_on`, `try_act_on` | Wrap an async block returning `Result` |
| `Reply::try_ok(value)` | `try_mutate_on`, `try_act_on` | Immediate success |
| `Reply::try_err(error)` | `try_mutate_on`, `try_act_on` | Immediate failure |

{% callout type="warning" title="Handler futures must be Send + Sync" %}
`Reply::pending` produces a `Pin<Box<dyn Future<Output = ()> + Send + Sync + 'static>>`. The `Sync` bound catches people out: anything held across an `.await` inside the block must be `Sync`, not merely `Send`. For work whose future isn't `Sync` (many HTTP and DB clients), `tokio::spawn` it and message the result back to the actor instead. See [Integration](/docs/advanced/integration).
{% /callout %}

### MessageContext

The second argument every handler receives — the message plus its routing information. Written `ctx` or `envelope` in examples.

| Method | Description |
|--------|-------------|
| `message()` | Reference to the message payload (an accessor, not a field) |
| `reply_envelope()` | `OutboundEnvelope` addressed back to the sender |
| `origin_envelope()` | `OutboundEnvelope` representing where the message came from |
| `new_envelope(&address)` | `OutboundEnvelope` to a different recipient, keeping this actor as the return address |

`MessageContext` is **not** exported from the prelude — you don't normally need to name it, since handlers are closures and the type is inferred.

### OutboundEnvelope

A message prepared for sending. This is what `reply_envelope()` and `create_envelope()` hand you.

| Method | Description |
|--------|-------------|
| `send(msg).await` | Send a message via this envelope |
| `try_send(msg).await` | Send, returning `Err` instead of waiting if the inbox is full |
| `reply(msg)` | Synchronous send (discouraged — prefer `send`) |
| `reply_to()` | The return address |
| `recipient()` | The recipient address, if any |

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

| Category | Items |
|----------|-------|
| Macros | `acton_actor`, `acton_message`, `acton_main` |
| System | `ActonApp`, `ActorRuntime`, `ActorConfig`, `Broker` |
| Actors | `ManagedActor`, `Idle`, `Started`, `ActorHandle` |
| Handlers | `Reply` |
| Messages | `BrokerRequest`, `BrokerRequestEnvelope`, `MessageAddress`, `OutboundEnvelope`, `ChildTerminated` |
| Supervision | `RestartPolicy`, `TerminationReason`, `SupervisionStrategy`, `SupervisionDecision`, `RestartLimiter`, `RestartLimiterConfig`, `RestartLimitExceeded`, `RestartStats` |
| Traits | `ActonMessage`, `ActorHandleInterface`, `Broadcaster`, `Subscribable`, `Subscriber` |
| Re-exports | everything from `acton_ern`, plus `async_trait` and `tokio` |
| IPC (`ipc` feature) | `IpcClient`, `IpcClientConfig`, `IpcConfig`, `IpcEnvelope`, `IpcError`, `IpcResponse`, `IpcListenerHandle`, `IpcListenerStats`, `IpcTypeRegistry`, `ShutdownResult` |

Note that `MessageContext` is deliberately **not** in the prelude; handlers receive it as an inferred closure parameter.
