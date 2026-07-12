---
title: Migration Guide
description: Upgrading Acton Reactive, and coming to it from other actor frameworks.
---

Two kinds of migration live here: [upgrading between Acton versions](#upgrading-acton-reactive), and mapping concepts from another actor framework onto Acton.

## Upgrading Acton Reactive

### 8.0 → 8.1

An additive release — existing code keeps working.

**Channel-based `IpcClient`.** IPC clients no longer need to hand-roll the wire protocol. `IpcClient` splits the socket into reader and writer tasks and gives you `send`, `request`, `subscribe`, `discover`, and `disconnect` over channels:

```rust
use acton_reactive::ipc::{IpcClient, IpcConfig, IpcEnvelope};

let client = IpcClient::connect(IpcConfig::load().socket_path()).await?;
let response = client.request(IpcEnvelope::new_request(
    "prices", "GetPrice", serde_json::json!({ "symbol": "ACTON" }),
)).await?;
```

The lower-level `acton_reactive::ipc::protocol` functions still exist; you just rarely need them now.

**`read_timeout_ms = 0` means "no timeout".** Zero is a sentinel in both `read_timeout_ms` and `subscription_read_timeout_ms` — an idle connection is never reaped. `subscription_read_timeout_ms` already defaults to `0`, so long-lived subscriber clients stay connected. See [Troubleshooting](/docs/reference/troubleshooting).

**MessagePack serialization is named.** The `ipc-messagepack` feature now uses named field serialization, which makes it interoperate correctly with `skip_serializing_if`.

### 7.x → 8.0

**Sync handler variants.** `mutate_on_sync` and `act_on_sync` register handlers that return `()` instead of a boxed future, skipping the `Box::pin(async move {})` allocation on every dispatch. If a handler has no `.await` in it, switch:

```rust
// Before
builder.mutate_on::<Increment>(|actor, _ctx| {
    actor.model.count += 1;
    Reply::ready()
});

// After — same behavior, no future allocated
builder.mutate_on_sync::<Increment>(|actor, _ctx| {
    actor.model.count += 1;
});
```

The async variants are unchanged and still correct — this is an optimization, not a required migration.

**Handler panics are now caught by default.** The new `catch-handler-panics` feature is **on by default**. A panicking handler is caught, logged, and the actor keeps running. If you relied on a panic taking an actor down, that no longer happens; opt out with:

```toml
[dependencies]
acton-reactive = { version = "8", default-features = false }
```

**`acton-ern` 2.0.** ERNs come from `acton-ern` 2.x. If you depend on `acton-ern` directly, bump it to `2` to avoid two incompatible copies in your tree. It's re-exported from the prelude, so most code needs no change.

**Internal reactor maps moved from `DashMap` to `HashMap`.** No API change — handler registration happens before start, so the lock-free map bought nothing. You may notice slightly lower dispatch overhead.

---

## From Akka (Scala/Java)

| Akka | Acton Reactive |
|------|----------------|
| `ActorSystem` | `ActonApp` |
| `Actor` trait | `#[acton_actor]` struct |
| `receive` | `mutate_on` / `act_on` handlers |
| `ActorRef` | `ActorHandle` |
| `tell` (!) | `handle.send(msg).await` |
| `ask` (?) | Reply envelope pattern |
| `Props` | Actor builder |
| `context.spawn` | `runtime.new_actor_with_config()` + `parent_handle.supervise()` |
| `PoisonPill` | `handle.stop()` |
| `EventBus` | `runtime.broker()` |

### Key Differences

**No behavior switching**: Acton actors don't change their message handlers at runtime. Use state enums instead:

```rust
#[acton_actor]
struct StateMachine {
    state: State,
}

enum State {
    Idle,
    Processing,
    Done,
}

builder.mutate_on::<Event>(|actor, envelope| {
    match actor.model.state {
        State::Idle => { /* idle behavior */ }
        State::Processing => { /* processing behavior */ }
        State::Done => { /* done behavior */ }
    }
    Reply::ready()
});
```

**Supervision is explicit, not automatic**: Acton has the same strategy vocabulary as Akka — `OneForOne`, `OneForAll`, `RestForOne` — plus `Permanent` / `Temporary` / `Transient` restart policies. The difference is who acts on them. Akka's supervisor restarts children for you; in Acton the framework sends the parent a `ChildTerminated` message and `SupervisionStrategy::decide()` tells you what *should* happen, but **your handler carries it out**. See [Supervision Basics](/docs/core-concepts/supervision-basics).

**Reply envelope pattern**: Instead of `ask`, use reply envelopes for request-response.

---

## From Actix (Rust)

| Actix | Acton Reactive |
|-------|----------------|
| `Actor` trait | `#[acton_actor]` struct |
| `Handler<M>` impl | `mutate_on::<M>` / `act_on::<M>` |
| `Addr<A>` | `ActorHandle` |
| `do_send` | `handle.send(msg).await` |
| `send().await` | Reply envelope pattern |
| `Context` | `ManagedActor` + `MessageContext` (the two handler args) |
| `Arbiter` | Tokio runtime (implicit) |
| `System::new()` | `ActonApp::launch_async().await` |

### Key Differences

**Builder pattern**: Actix uses trait implementations; Acton uses a builder:

```rust
// Actix
impl Handler<Increment> for Counter {
    type Result = ();
    fn handle(&mut self, msg: Increment, ctx: &mut Context<Self>) {
        self.count += 1;
    }
}

// Acton
builder.mutate_on::<Increment>(|actor, _envelope| {
    actor.model.count += 1;
    Reply::ready()
});
```

**Envelope-based handlers**: Handlers receive envelopes, not raw messages:

```rust
builder.mutate_on::<MyMessage>(|actor, envelope| {
    let msg = envelope.message();  // Access the message
    Reply::ready()
});
```

**Async handlers are explicit**: Use `Reply::pending` for async work:

```rust
builder.act_on::<Query>(|actor, envelope| {
    let reply = envelope.reply_envelope();
    Reply::pending(async move {
        let data = fetch_data().await;
        reply.send(QueryResponse(data)).await;
    })
});
```

---

## From Tokio Actors (manual implementation)

If you've built actors manually with Tokio channels:

| Manual | Acton Reactive |
|--------|----------------|
| `mpsc::channel` | Built into framework |
| `tokio::spawn` + loop | `builder.start().await` |
| Match on message enum | Typed handlers |
| Manual state management | `actor.model` |
| Manual shutdown logic | `runtime.shutdown_all()` |

### Key Differences

**No message enum matching**: Each message type gets its own handler:

```rust
// Manual Tokio
loop {
    match rx.recv().await {
        Some(Msg::Increment) => count += 1,
        Some(Msg::GetCount(tx)) => tx.send(count),
        None => break,
    }
}

// Acton
builder
    .mutate_on::<Increment>(|actor, _envelope| {
        actor.model.count += 1;
        Reply::ready()
    })
    .act_on::<GetCount>(|actor, envelope| {
        let count = actor.model.count;
        let reply = envelope.reply_envelope();
        Reply::pending(async move {
            reply.send(CountResponse(count)).await;
        })
    });
```

**Reply envelope pattern**: Use envelopes instead of oneshot channels:

```rust
// Manual: include response channel
struct GetCount(oneshot::Sender<i32>);

// Acton: use reply envelope
builder.act_on::<GetCount>(|actor, envelope| {
    let count = actor.model.count;          // copy out first
    let reply = envelope.reply_envelope();
    Reply::pending(async move {
        reply.send(CountResponse(count)).await;
    })
});
```

{% callout type="warning" title="Copy state out before the async block" %}
`Reply::pending` produces a `'static` boxed future, so you cannot borrow `actor` inside it — `reply.send(CountResponse(actor.model.count))` won't compile. Read what you need into a local *before* the `async move`, as above.
{% /callout %}

---

## From Orleans (.NET)

| Orleans | Acton Reactive |
|---------|----------------|
| Grain | Actor |
| `IGrain` interface | `#[acton_actor]` struct |
| `GrainClient` | `ActorHandle` |
| Silo | `ActonApp` (single process) |
| Virtual actors | Not supported (explicit spawn) |
| Grain persistence | Manual (store in state) |

### Key Differences

**Not virtual actors**: Acton actors must be explicitly created. There's no automatic activation on first call.

**Local only**: Acton is designed for single-process concurrency. For distribution, build your own layer on top.

---

## From Erlang/Elixir

| Erlang/Elixir | Acton Reactive |
|---------------|----------------|
| `spawn` | `builder.start().await` |
| `pid` | `ActorHandle` |
| `send` (!) | `handle.send(msg).await` |
| `receive` | Handler closures |
| `GenServer` | Actor with handlers |
| Supervisor | Parent actor |
| OTP Application | `ActonApp` |

### Key Differences

**Typed messages**: No pattern matching on arbitrary terms. Each message is a typed struct:

```rust
#[acton_message]
struct Ping;

#[acton_message]
struct SetValue { value: i32 }
```

**No hot code reloading**: Rust is compiled. Actors can't change code at runtime.

**The OTP vocabulary is there, but you drive it**: Acton has `OneForOne`, `OneForAll`, and `RestForOne` strategies and `Permanent` / `Temporary` / `Transient` restart policies — the concepts port over directly. What doesn't port over is the supervisor process doing the restarting. Acton delivers a `ChildTerminated` message to the parent and gives you `SupervisionStrategy::decide()` and `RestartLimiter` as helpers; recreating the child is your handler's job.

**No links/monitors**: There's no `link`/`monitor` distinction. A child notifies its parent — and only its parent — provided it was created with a parent reference in its `ActorConfig`.

---

## General Migration Tips

1. **Start small**: Port one actor at a time
2. **Map your messages**: Create `#[acton_message]` structs for each message type
3. **Identify mutation**: Separate read-only handlers (`act_on`) from state-changing ones (`mutate_on`)
4. **Skip the future when you can**: Handlers with no `.await` should use `mutate_on_sync` / `act_on_sync`
5. **Handle async differently**: Use `Reply::pending` for async work — and remember the future must be `Send + Sync`
6. **Use envelope pattern**: Replace `ask` with reply envelopes
7. **Write your supervision**: Handle `ChildTerminated` in the parent; nothing restarts automatically

---

## Need Help?

If you're stuck migrating from a specific framework, [open an issue](https://github.com/Govcraft/acton-reactive/issues) with your use case.
