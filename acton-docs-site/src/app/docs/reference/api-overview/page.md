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
| `ipc_expose(name, handle)` | Expose actor for IPC. Returns `Result<(), IpcNameInUse>`; it no longer replaces an existing registration |
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
| `ask(request).await` | Send a request and wait for the reply |
| `ask_with_timeout(request, dur).await` | `ask` with an explicit deadline |
| `broadcast(msg).await` | Publish to the broker for all subscribers |
| `stop().await` | Stop the actor |
| `subscribe::<M>().await` | Subscribe to broadcast messages |
| `unsubscribe::<M>()` | Unsubscribe from broadcast messages (fire-and-forget) |
| `unsubscribe_async::<M>().await` | Unsubscribe, awaiting delivery of the request to the broker |
| `reply_address()` | Get this actor's address, for use as a return address |
| `create_envelope(recipient)` | Create an envelope **from** this actor **to** `recipient` |
| `supervise_with::<S>(&runtime, config, blueprint).await` | Register a child with a blueprint; returns `SupervisedChild`. **The framework restarts it** |
| `supervise(child).await` | Adopt an already-built actor for cascading shutdown. No blueprint, so **never restarted** |
| `unsupervise(&ern).await` | Retire the record and **stop** the child, dropping its IPC names |
| `release(&ern).await` | Retire the record and hand the child back **still running** |
| `children()` | The local view of children supervised through *this handle clone*; handles go stale across a restart |
| `find_child(&ern)` | Look up a direct child by ERN, with the same caveat |
| `id()` | Get actor's identifier (`Ern`) |
| `name()` | Get actor's root name |

### ask and Request

`ask` sends a request and waits for the reply. A message becomes askable by implementing `Request`, which names the reply through an associated type:

```rust
impl Request for GetCount {
    type Response = Count;
}

let count = handle.ask(GetCount).await?;
```

Because inboxes are FIFO, a completed `ask` also proves every message sent to that actor beforehand has been processed. Handlers are unchanged: they answer through the reply envelope, and an actor cannot tell an `ask` from a `send`.

`ask` always finishes, backed by a 30-second `DEFAULT_ASK_TIMEOUT`. Outcomes come back as `AskError`, which is `#[non_exhaustive]`:

| Variant | Meaning |
|---|---|
| `NoReply` | Delivered, but no answer is coming |
| `TimedOut { after }` | The reply address is still live; the deadline expired |
| `Undeliverable` | The inbox was already closed |
| `Cancelled` | Delivery was abandoned during shutdown |
| `UnexpectedReply` | The handler answered with a type the request does not declare |
| `PeerRejected { code, detail }` | *(IPC)* The peer refused before dispatch; a retry is safe |
| `TransportFailed { detail }` | *(IPC)* The connection failed; whether it was processed is unknown |

**Do not `ask` from inside a `mutate_on` handler.** Mutable handlers are awaited inline on the message loop, so waiting for a reply stops the actor from processing the message that would produce it.

### SupervisedChild

A reference to a supervised child that survives its restarts. `ActorHandle` names one incarnation and goes stale; `SupervisedChild` reads a status channel its supervisor publishes to.

| Method | Description |
|--------|-------------|
| `current()` | Handle for the incarnation running right now |
| `wait_running().await` | Block until an incarnation is up; errors once escalated |
| `wait_generation(gen).await` | Block until a specific incarnation is up |
| `status()` | The published `SupervisionStatus`: state, generation, last reason |

`SupervisionState` is `Starting`, `Running`, `RestartPending`, `Restarting`, `Down` (will not come back), or `Escalated` (allowance exhausted).

### FlushBroadcasts

A barrier for broadcasts. `broadcast` completes when the broker has the message, not when subscribers do, and a broadcast carries no reply address for a subscriber to answer.

```rust
broker.ask(FlushBroadcasts).await?;   // answers BroadcastsFlushed
```

The reply cannot arrive until every earlier broadcast is sitting in every subscriber's inbox. That is **delivery, not completion**: to know a particular subscriber has handled it, `ask` that subscriber afterwards.

`ActorRuntime::shutdown_all` flushes the broker before signalling anything, so broadcasting and then shutting down is not a race.

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
| System | `ActonApp`, `ActorRuntime`, `ActorConfig`, `Broker`, `BrokerRef`, `ParentRef` |
| Actors | `ManagedActor`, `Idle`, `Started`, `ActorHandle` |
| Handlers | `Reply` |
| Request/reply | `AskError`, `DEFAULT_ASK_TIMEOUT` |
| Messages | `BrokerRequest`, `BrokerRequestEnvelope`, `MessageAddress`, `OutboundEnvelope`, `ChildTerminated`, `SystemSignal`, `FlushBroadcasts`, `BroadcastsFlushed` |
| Supervision | `SupervisedChild`, `SupervisionStatus`, `SupervisionState`, `SupervisionError`, `Escalation`, `RestartPolicy`, `TerminationReason`, `SupervisionStrategy`, `SupervisionDecision`, `RestartLimiter`, `RestartLimiterConfig`, `RestartLimitExceeded`, `RestartStats`, `RestartGeneration`, `ChildIndex`, `BackoffDelay`, `MAX_SUPERVISION_DEPTH` |
| Supervision events | `ChildSupervised`, `ChildRestarted`, `SupervisionEscalated` |
| Traits | `ActonMessage`, `ActorHandleInterface`, `Broadcaster`, `Request`, `Subscribable`, `Subscriber` |
| Re-exports | everything from `acton_ern`, plus `async_trait` and `tokio` |
| IPC (`ipc` feature) | `IpcClient`, `IpcClientConfig`, `IpcConfig`, `IpcEnvelope`, `IpcError`, `IpcResponse`, `IpcListenerHandle`, `IpcListenerStats`, `IpcTypeRegistry`, `RemoteActorRef`, `ShutdownResult`, and the `RemoteRequest` trait |

Note that `MessageContext` is deliberately **not** in the prelude; handlers receive it as an inferred closure parameter.
