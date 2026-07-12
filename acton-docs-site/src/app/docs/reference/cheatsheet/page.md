---
title: Cheatsheet
description: Copy-paste patterns for common actor tasks.
---

Quick reference for common patterns. Copy, paste, and adapt.

## Basic Actor Setup

```rust
use acton_reactive::prelude::*;

#[acton_actor]
struct MyActor {
    // your state here
}

#[acton_message]
struct MyMessage;

#[acton_main]
async fn main() {
    let mut runtime = ActonApp::launch_async().await;

    let mut builder = runtime.new_actor::<MyActor>();
    builder.mutate_on::<MyMessage>(|actor, _envelope| {
        // handle message
        Reply::ready()
    });

    let handle = builder.start().await;

    handle.send(MyMessage).await;
    runtime.shutdown_all().await.ok();
}
```

---

## Message Patterns

### Fire-and-Forget

```rust
handle.send(DoSomething).await;
```

### Request-Response (Reply Envelope)

```rust
// Server actor
builder.act_on::<GetValue>(|actor, envelope| {
    let value = actor.model.value;
    let reply = envelope.reply_envelope();
    Reply::pending(async move {
        reply.send(ValueResponse(value)).await;
    })
});

// Client actor receives response
client.mutate_on::<ValueResponse>(|actor, envelope| {
    let value = envelope.message().0;
    println!("Got: {}", value);
    Reply::ready()
});
```

### Broadcast

```rust
// Publisher
let broker = runtime.broker();
broker.broadcast(Event { data: "hello".into() }).await;

// Subscriber (before starting)
builder.mutate_on::<Event>(|actor, envelope| {
    println!("Got: {}", envelope.message().data);
    Reply::ready()
});
builder.handle().subscribe::<Event>().await;
let handle = builder.start().await;
```

---

## Handler Patterns

### Mutate State

```rust
builder.mutate_on::<Increment>(|actor, _envelope| {
    actor.model.count += 1;
    Reply::ready()
});
```

### Mutate State (Sync — No Future Allocation)

```rust
builder.mutate_on_sync::<Increment>(|actor, _envelope| {
    actor.model.count += 1;
});
```

### Read State with Reply

```rust
builder.act_on::<GetCount>(|actor, envelope| {
    let count = actor.model.count;
    let reply = envelope.reply_envelope();
    Reply::pending(async move {
        reply.send(CountResponse(count)).await;
    })
});
```

### Async Handler

```rust
builder.act_on::<Compute>(|actor, envelope| {
    let input = envelope.message().input;
    let reply = envelope.reply_envelope();
    Reply::pending(async move {
        let result = compute(input).await;
        reply.send(ComputeResponse { result }).await;
    })
});
```

{% callout type="warning" title="Handler futures must be Send + Sync" %}
The async block you pass to `Reply::pending` must be `Send + **Sync**`. That's stricter than the usual `Send`, and it rules out plenty of third-party futures (many HTTP and database clients). If the compiler says *"future created by async block is not `Sync`"*, use the spawn-and-report-back pattern below.
{% /callout %}

### Async Handler (Non-`Sync` Future — HTTP, DB, …)

Spawn the work with `tokio::spawn` (which only needs `Send`) and message the result back:

```rust
builder.mutate_on::<FetchData>(|actor, envelope| {
    let handle = actor.handle().clone();
    let url = envelope.message().url.clone();

    tokio::spawn(async move {
        let resp = reqwest::get(&url).await.unwrap();
        let body = resp.text().await.unwrap();
        handle.send(FetchResponse { body }).await;
    });

    Reply::ready()
});

// Then handle the result as a normal message
builder.mutate_on::<FetchResponse>(|actor, envelope| {
    actor.model.last_body = envelope.message().body.clone();
    Reply::ready()
});
```

### Read-Only Handler (Sync — No Future Allocation)

```rust
builder.act_on_sync::<LogCount>(|actor, _envelope| {
    tracing::info!("count = {}", actor.model.count);
});
```

---

## Fallible Handlers

Return a `Result` from a handler and handle the error separately.

```rust
builder
    .try_mutate_on::<Withdraw, Receipt, BankError>(|actor, ctx| {
        let amount = ctx.message().amount;
        let balance = actor.model.balance;
        Reply::try_pending(async move {
            if balance < amount {
                Err(BankError::InsufficientFunds { balance, amount })
            } else {
                Ok(Receipt { remaining: balance - amount })
            }
        })
    })
    .on_error::<Withdraw, BankError>(|actor, ctx, err| {
        tracing::error!("Withdrawal failed: {}", err);
        Reply::ready()
    });
```

Immediate results skip the async block entirely:

```rust
builder.try_act_on::<GetBalance, Balance, BankError>(|actor, _ctx| {
    Reply::try_ok(Balance(actor.model.balance))
});
```

---

## Child Actors

### Create and Supervise Child

Build the child from the runtime, give it a **parent reference**, then hand it to `supervise()`:

```rust
// Start the parent
let parent = runtime.new_actor::<ParentState>();
let parent_handle = parent.start().await;

// Build the child with the parent in its config — this is what enables
// ChildTerminated notifications back to the parent.
let config = ActorConfig::new(
    Ern::with_root("worker")?,
    Some(parent_handle.clone()),
    None,
)?;
let mut child = runtime.new_actor_with_config::<WorkerState>(config);
child.mutate_on::<Task>(handle_task);

// supervise() starts the child and registers it under the parent
let child_handle = parent_handle.supervise(child).await?;
```

{% callout type="note" title="create_child() is Idle-only" %}
`create_child()` exists only on a builder (`ManagedActor<Idle, _>`), not on a running actor — so you can't call it from inside a handler, and it returns an actor with the *same* state type as its parent. To spawn a differently-typed worker, use `runtime.new_actor_with_config()` as above.
{% /callout %}

### Access Children

`children()` lives on the **handle**, not on the actor:

```rust
for child in actor.handle().children().iter() {
    child.value().send(Ping).await;
}
```

---

## Lifecycle Hooks

### Before Start

```rust
builder.before_start(|actor| async move {
    println!("Actor starting!");
});
```

### After Stop

```rust
builder.after_stop(|actor| async move {
    println!("Actor stopped");
});
```

---

## Common State Patterns

### With Default

```rust
#[acton_actor]
struct Counter {
    count: i32,  // defaults to 0
}
```

### With Custom Default

Use `no_default` to stop the macro deriving `Default`, then write your own. (Adding `#[derive(Default)]` *and* a manual `impl Default` collides — you'd get "conflicting implementations".)

```rust
#[acton_actor(no_default)]
struct Config {
    timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self { timeout: Duration::from_secs(30) }
    }
}
```

`no_default` is also how you hold a field whose type has no `Default` (a `Stdout`, a client handle, and so on).

### With External Resources

```rust
#[acton_actor]
struct DbActor {
    pool: Option<PgPool>,
}

// Initialize via message or before_start
```

---

## Error Handling

### In Handlers

```rust
builder.mutate_on::<RiskyOp>(|actor, envelope| {
    match do_risky_thing() {
        Ok(result) => {
            actor.model.data = result;
            Reply::ready()
        }
        Err(e) => {
            tracing::error!("Failed: {}", e);
            Reply::ready()  // Actor continues
        }
    }
});
```

### Signal Errors to Other Actors

```rust
builder.mutate_on::<Query>(|actor, envelope| {
    let reply = envelope.reply_envelope();
    match do_query() {
        Ok(data) => Reply::pending(async move {
            reply.send(QuerySuccess(data)).await;
        }),
        Err(e) => Reply::pending(async move {
            reply.send(QueryFailed(e.to_string())).await;
        }),
    }
});
```

---

## Testing

### Basic Test

```rust
#[tokio::test]
async fn test_actor() {
    let mut runtime = ActonApp::launch_async().await;

    let mut counter = runtime.new_actor::<Counter>();
    counter
        .mutate_on::<Increment>(|actor, _env| {
            actor.model.count += 1;
            Reply::ready()
        });

    let handle = counter.start().await;
    handle.send(Increment).await;

    // Use probe actor or atomic counter to verify
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    runtime.shutdown_all().await.ok();
}
```

---

## IPC

### Expose an Actor (Server)

```rust
let mut runtime = ActonApp::launch_async().await;

// Register the message type so it can be deserialized from the wire
runtime.ipc_registry().register::<GetPrice>("GetPrice");

let mut service = runtime.new_actor_with_name::<PriceService>("prices".to_string());
service
    .act_on::<GetPrice>(|actor, ctx| { /* ... */ })
    .expose_for_ipc();          // reachable as "prices"
let handle = service.start().await;

let listener = runtime.start_ipc_listener().await?;
```

### Connect and Send (Client)

`IpcClient` is the channel-based client — connect once, then send, request, or subscribe:

```rust
use acton_reactive::ipc::{IpcClient, IpcConfig, IpcEnvelope};

let client = IpcClient::connect(IpcConfig::load().socket_path()).await?;

// Request-response — new_request() sets expects_reply and generates a correlation ID
let response = client.request(IpcEnvelope::new_request(
    "prices",
    "GetPrice",
    serde_json::json!({ "symbol": "ACTON" }),
)).await?;

if response.success {
    println!("{:?}", response.payload);
}

// Fire-and-forget — IpcEnvelope::new() instead
client.send(IpcEnvelope::new(
    "prices",
    "RefreshCache",
    serde_json::json!({}),
)).await?;

client.disconnect().await?;
```

### Subscribe to Broadcasts (Client)

```rust
client.subscribe(vec!["PriceUpdate".to_string()]).await?;
let mut pushes = client.take_push_receiver().expect("already taken");

while let Some(note) = pushes.recv().await {
    println!("push: {:?}", note.payload);
}
```

See [IPC Setup](/docs/ipc-setup) and [IPC Patterns](/docs/ipc-patterns) for the full picture.

---

## Quick Imports

```rust
// Everything you need
use acton_reactive::prelude::*;

// For IPC clients
use acton_reactive::ipc::{IpcClient, IpcConfig, IpcEnvelope};

// Only if you're speaking the wire protocol by hand
use acton_reactive::ipc::protocol::{write_envelope, read_response};
```

---

## Common Mistakes

| Wrong | Right |
|-------|-------|
| `ActonApp::launch()` **from an async context** (panics) | `ActonApp::launch_async().await`. `launch()` is fine from `fn main()`. |
| `ctx.message` | `ctx.message()` — it's an accessor, not a field |
| `actor.children()` | `actor.handle().children()` |
| `actor.create_child(..)` inside a handler | `create_child` is `Idle`-only; build from the runtime and `supervise()` |
| `#[acton_actor]` + `#[derive(Default)]` + manual `impl Default` | `#[acton_actor(no_default)]` + manual `impl Default` |
| `runtime.new_actor::<T>().mutate_on::<M>(h).start()` | Configure via `&mut` first, *then* `builder.start().await` |
| `handle.ask(msg)` | Use reply envelope pattern |
| `Reply::with(value)` | Use `Reply::pending` + reply envelope |
| Forgetting `.await` on `start()` | `builder.start().await` |
| Mutating in `act_on` | Use `mutate_on` for state changes |
| Expecting a panic to kill an actor | Panics are caught by default; use `try_mutate_on` + `on_error` |
