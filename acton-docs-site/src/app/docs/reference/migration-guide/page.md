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
| `ActorRef` | `AgentHandle` |
| `tell` (!) | `handle.send(msg).await` |
| `ask` (?) | `handle.ask(msg).await` |
| `Props` | `AgentBuilder` |
| `context.spawn` | `actor.new_child::<T>()` |
| `PoisonPill` | `handle.stop()` |
| `EventBus` | `app.get_broker()` |

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

builder.mutate_on::<Event>(|actor, msg| {
    match actor.model.state {
        State::Idle => { /* idle behavior */ }
        State::Processing => { /* processing behavior */ }
        State::Done => { /* done behavior */ }
    }
    Reply::ready()
});
```

**Supervision is simpler**: No complex supervision strategies. Parents are notified of child failures and decide how to respond.

---

## From Actix (Rust)

| Actix | Acton Reactive |
|-------|----------------|
| `Actor` trait | `#[acton_actor]` struct |
| `Handler<M>` impl | `mutate_on::<M>` / `act_on::<M>` |
| `Addr<A>` | `AgentHandle` |
| `do_send` | `handle.send(msg).await` |
| `send().await` | `handle.ask(msg).await` |
| `Context` | `ManagedAgent` (in handler) |
| `Arbiter` | Tokio runtime (implicit) |
| `System::new()` | `ActonApp::launch()` |

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
builder.mutate_on::<Increment>(|actor, _| {
    actor.model.count += 1;
    Reply::ready()
});
```

**Async handlers are explicit**: Use `Reply::pending` for async work:

```rust
builder.act_on::<Query>(|actor, msg| {
    Reply::pending(async move {
        fetch_data().await
    })
});
```

---

## From Tokio Actors (manual implementation)

If you've built actors manually with Tokio channels:

| Manual | Acton Reactive |
|--------|----------------|
| `mpsc::channel` | Built into framework |
| `tokio::spawn` + loop | `AgentBuilder.start()` |
| Match on message enum | Typed handlers |
| Manual state management | `actor.model` |
| Manual shutdown logic | `app.shutdown_all()` |

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
    .mutate_on::<Increment>(|actor, _| {
        actor.model.count += 1;
        Reply::ready()
    })
    .act_on::<GetCount>(|actor, _| {
        Reply::with(actor.model.count)
    });
```

**Built-in request-response**: No need to include oneshot channels in messages:

```rust
// Manual: include response channel
struct GetCount(oneshot::Sender<i32>);

// Acton: just use ask
let count: i32 = handle.ask(GetCount).await;
```

---

## From Orleans (.NET)

| Orleans | Acton Reactive |
|---------|----------------|
| Grain | Actor |
| `IGrain` interface | `#[acton_actor]` struct |
| `GrainClient` | `AgentHandle` |
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
| `pid` | `AgentHandle` |
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
5. **Simplify supervision**: Start with default behavior, add custom logic as needed

---

## Need Help?

If you're stuck migrating from a specific framework, [open an issue](https://github.com/acton-lang/acton-reactive/issues) with your use case.

