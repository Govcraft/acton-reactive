---
title: Migration Guide
description: Coming to Acton Reactive from other actor frameworks.
---

If you've used actor systems before, this guide maps familiar concepts to Acton Reactive.

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
| `context.spawn` | `actor.create_child()` + `supervise()` |
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

**Supervision is simpler**: No complex supervision strategies. Parents are notified of child failures and decide how to respond.

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
| `Context` | `ManagedAgent` (in handler) |
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
    let reply = envelope.reply_envelope();
    Reply::pending(async move {
        reply.send(CountResponse(actor.model.count)).await;
    })
});
```

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

**Less supervision complexity**: No one-for-one, rest-for-one strategies. Implement custom logic in parent actors.

---

## General Migration Tips

1. **Start small**: Port one actor at a time
2. **Map your messages**: Create `#[acton_message]` structs for each message type
3. **Identify mutation**: Separate read-only handlers (`act_on`) from state-changing ones (`mutate_on`)
4. **Handle async differently**: Use `Reply::pending` for async work
5. **Use envelope pattern**: Replace `ask` with reply envelopes
6. **Simplify supervision**: Start with default behavior, add custom logic as needed

---

## Need Help?

If you're stuck migrating from a specific framework, [open an issue](https://github.com/Govcraft/acton-reactive/issues) with your use case.
