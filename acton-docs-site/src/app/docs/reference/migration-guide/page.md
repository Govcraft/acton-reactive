---
title: Migration Guide
description: Upgrading Acton Reactive, and coming to it from other actor frameworks.
---

Two kinds of migration live here: [upgrading between Acton versions](#upgrading-acton-reactive), and mapping concepts from another actor framework onto Acton.

## Upgrading Acton Reactive

### 8.x → 9.0

A major release. Most of it is additive, but there are compile errors to fix and **two silent behaviour changes** that no compiler will point at. Start with those.

#### Silent behaviour changes: read these first

**1. `unsupervise` now stops the child it releases.**

It previously retired the supervisor's record and left the actor running, contradicting its own documentation. The signature is unchanged, so nothing will fail to compile.

```rust
// Was: retires the record, child keeps running
// Now: retires the record AND stops the child
parent_handle.unsupervise(&child_ern).await?;

// If you relied on the child surviving, this is the replacement.
// It hands back the still-running child's handle.
let child = parent_handle.release(&child_ern).await?;
```

`unsupervise` also drops the child's IPC names, whichever way that child was registered. A name that resolves to a mailbox nobody is reading is the precise failure the IPC registry exists to prevent, so the names go with the actor.

**2. Cascading shutdown now reaches every supervised child.**

A child supervised through a **handle clone obtained after the parent started** used to be invisible to the parent's own task and simply outlived it. `ActorHandle` stores its children in a map that is deep-copied on clone. Such a child is now stopped with its parent.

If a child genuinely should outlive its supervisor, start it as a root actor instead of supervising it.

There is a narrower consequence worth checking. Children stopped by a cascading shutdown terminate with `TerminationReason::ParentShutdown`, and `RestartPolicy::Permanent` warrants a restart on a normal termination. The framework suppresses its own restart decisions during shutdown; **a hand-rolled `ChildTerminated` handler does not**. If you restart children from your own handler, check the reason:

```rust
supervisor.mutate_on::<ChildTerminated>(|actor, ctx| {
    if matches!(ctx.message().reason, TerminationReason::ParentShutdown) {
        return Reply::ready();   // we are on the way down
    }
    // ...
});
```

**3. A graceful stop drains its backlog again.**

An actor receiving `Terminate` now runs `before_stop`, closes its inbox, and works off the messages already queued behind the signal before stopping. This restores documented behaviour that was silently lost in 7.0.0. It means **more of your messages get handled during shutdown than in 8.x**, which is almost always what you wanted, but it is a change in observable behaviour. The drain is bounded: the inbox is closed first, so the backlog can only shrink.

#### Compile errors, and how to fix each

**`ActorConfig::new` no longer takes a parent, and no longer returns a `Result`.**

```rust
// Was
ActorConfig::new(id, None, broker)?                              // root
ActorConfig::new(Ern::with_root(name)?, Some(parent), broker)?   // child

// Now
ActorConfig::new(id, broker)                                     // root, infallible
ActorConfig::for_supervised_child(name, parent, broker)?         // child
```

Migration is mechanical: for a root, drop the `None` and the `?`; for a child, pass the plain **name** where the `Ern` used to be built, and keep the `?`.

The parent branch is deleted rather than patched, because that is where a real defect lived: children named `alpha` and `beta` came out with **identical** identifiers, and differed in practice only because each call happened to draw a fresh random suffix. See "identifiers" below.

**`ActorRuntime::ipc_expose` returns `Result<(), IpcNameInUse>`.**

Handle or `expect()` the result. It no longer silently replaces an existing registration, because overwriting redirected traffic away from an actor that was already serving, with no way for it to learn it had been displaced.

```rust
runtime.ipc_expose("prices", handle)?;
```

Release a name with `ipc_hide` if you intend to reuse it. `ipc_rebind` still overwrites, deliberately: that is the supervision engine repointing a name it already owns at a restarted incarnation.

`expose_for_ipc()` remains infallible and still returns `&mut Self`; a conflict is logged at `error!` with both actors named, and the actor starts but is not reachable under that name.

**`IpcError` is now `#[non_exhaustive]`.**

A `match` listing every variant stops compiling. Add a wildcard arm. This is a one-time cost: later variants are additive and will not break it again. The same applies to `SystemSignal`, and to the new `AskError`.

**`SubscriptionManager::register_connection` takes a third argument**, `peer: Option<PeerCredentials>`. Pass `None` if you do not need the identity of the connecting process.

#### Identifiers and IPC names changed

**`expose_for_ipc()` now registers the name you chose.** The old name embedded a `UUIDv7` regenerated on every process start, so it differed on every run and no client, config file or script could ever have named it. **No working program can have depended on the old value.**

| Actor | Was | Now |
|---|---|---|
| `new_actor_with_name("prices")` | `prices_01kyww2gfb…` | `prices` |
| child `"alpha"` of `prices` | `prices_01kyww2gfb…` | `prices/alpha` |
| child `"beta"` of `prices` | `prices_01kyww2gfb…` | `prices/beta` |

The middle column is not a typo. Every child of one parent registered under the same name and each silently replaced the last, along with the parent itself.

**`create_child` now keeps the name you gave it.** Its `Ern` is `<parent-ern>/<name>`, and the same parent and name always produce the same identifier. Previously the child's name contributed nothing at all.

**A supervision chain is limited to `MAX_SUPERVISION_DEPTH` (10) levels.** Exceeding it now names the child that was refused instead of surfacing a generic identifier error.

#### The framework now restarts supervised children

This is the headline feature, and it **cannot affect any existing program**. A supervisor can only rebuild a child it holds a blueprint for, and blueprints reach the registry only through `supervise_with` and `supervise_deferred`, neither of which has appeared in a released version. Children adopted through `supervise()` have no blueprint and are left down exactly as before.

**That firewall stops applying the moment you migrate a child.** When you do, **delete your hand-rolled restart for that child**, or it will come back twice: once from your handler and once from the framework.

```rust
// Before: adopt an already-built actor. Never restarted.
let child_handle = parent_handle.supervise(child).await?;

// After: register a blueprint. Restarted from it on failure.
let config = ActorConfig::for_supervised_child("worker", parent_handle.clone(), None)?
    .with_restart_policy(RestartPolicy::Permanent);

let child = parent_handle
    .supervise_with::<Worker>(&runtime, config, |actor| {
        actor.mutate_on::<Task>(handle_task);
    })
    .await?;
```

Note what `supervise_with` returns: a **`SupervisedChild`**, not an `ActorHandle`. A handle names one incarnation and goes stale across a restart, silently. Store the `SupervisedChild` and call `current()` at the point of use.

Inside a handler, use `supervise_deferred`, which records the child and queues its start rather than awaiting it on the supervisor's message loop.

**`with_supervision_strategy` and `with_restart_limiter` are no longer deprecated**, because they are now read. Their deprecation notices used to advise hand-rolling a `ChildTerminated` handler, which would now cause exactly the double-restart described above.

`OneForAll` and `RestForOne` are carried out rather than planned and ignored. `ActorConfig::with_escalation` makes `Escalation` reachable; it decides what a supervisor does once a child exhausts its restart allowance. See [Supervision Basics](/docs/core-concepts/supervision-basics).

#### New: ask

`send` returns `()`, so a caller had no way to learn a message had been processed. Awaiting a result meant hand-rolling a channel per call site, or sleeping and hoping.

```rust
// Before: a client actor, a hand-addressed envelope, a response handler, a sleep
let query = client_handle.create_envelope(Some(counter_handle.reply_address()));
query.send(GetCount).await;
tokio::time::sleep(Duration::from_millis(100)).await;

// After
let count = counter_handle.ask(GetCount).await?;
```

Make a message askable by implementing `Request`:

```rust
impl Request for GetCount {
    type Response = Count;
}
```

Handlers are unchanged: they answer through the reply envelope exactly as before, and an actor cannot tell an `ask` from a `send`. This is purely additive, so nothing forces you to migrate, but it is the fix for most sleeps in test and startup code: **a completed `ask` proves every message sent to that actor beforehand was processed.**

**Do not `ask` from inside a `mutate_on` handler.** Mutable handlers are awaited inline on the message loop, so waiting for a reply stops the actor from processing the message that would produce it. The 30-second default deadline turns that mistake into a prompt `AskError::TimedOut` rather than a hang.

`IpcClient::actor` gives the same call across a process boundary, with the added bounds a wire form requires:

```rust
let count: Count = handle.ask(GetCount).await?;                  // local
let count: Count = client.actor("counter").ask(GetCount).await?; // remote
```

#### New: FlushBroadcasts

`broadcast` completes when the broker has the message, not when subscribers do, and a broadcast cannot answer for itself. `broker.ask(FlushBroadcasts).await` answers once every earlier broadcast is sitting in every subscriber's inbox.

**`shutdown_all` now does this for you** before signalling anything, so broadcasting and then shutting down is no longer a race. You need an explicit flush only when asserting before shutdown, or when the broadcast has not been issued yet.

#### Defaults and configuration

- **`max_connections` now defaults to 1024**, up from 100. If you were relying on the old default as a resource ceiling, set `limits.max_connections` explicitly.
- **`IpcConfig::load()` prefers `$XDG_CONFIG_HOME/acton/<app_name>/ipc.toml`**, falling back to the shared `$XDG_CONFIG_HOME/acton/ipc.toml`. The shared path still loads, so no action is required.
- **A refused connection now says why.** A server at its connection limit writes a typed error before closing, and the client reports `IpcError::ConnectionLimitReached { limit }` instead of `Broken pipe` on the first write. Nothing changes on the wire.

#### Also new

`BrokerRef`, `ParentRef` and `SystemSignal` are exported from the prelude. All three were already `pub` but lived in private modules, so they were unnameable from outside the crate even though public signatures referred to them.

`SupervisedChild`, `SupervisionStatus`, `SupervisionState`, `SupervisionError`, `Escalation`, `PeerCredentials`, `IpcNameInUse`, `ConfigSource`, and the `ChildSupervised` / `ChildRestarted` / `SupervisionEscalated` broker events are all new. See the [changelog](https://github.com/Govcraft/acton-reactive/blob/main/acton-reactive/CHANGELOG.md) for the full list.

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
