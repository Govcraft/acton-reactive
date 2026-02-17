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
builder.act_on::<FetchData>(|_actor, envelope| {
    let url = envelope.message().url.clone();
    let reply = envelope.reply_envelope();
    Reply::pending(async move {
        let resp = reqwest::get(&url).await.unwrap();
        let body = resp.text().await.unwrap();
        reply.send(FetchResponse { body }).await;
    })
});
```

---

## Child Actors

### Create and Supervise Child

```rust
builder.mutate_on::<SpawnChild>(|actor, _envelope| {
    let mut child = actor.create_child("worker".to_string())
        .expect("Failed to create child");
    child.mutate_on::<Task>(handle_task);

    let parent_handle = actor.handle().clone();
    Reply::pending(async move {
        let child_handle = parent_handle.supervise(child).await
            .expect("Failed to supervise");
        println!("Child started: {}", child_handle.id());
    })
});
```

### Access Children

```rust
for child in actor.children().iter() {
    child.send(Ping).await;
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

```rust
#[acton_actor]
#[derive(Default)]
struct Config {
    timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self { timeout: Duration::from_secs(30) }
    }
}
```

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

## Quick Imports

```rust
// Everything you need
use acton_reactive::prelude::*;

// For IPC
use acton_reactive::ipc::{IpcEnvelope, IpcConfig};
use acton_reactive::ipc::protocol::{write_envelope, read_response};
```

---

## Common Mistakes

| Wrong | Right |
|-------|-------|
| `ActonApp::launch()` | `ActonApp::launch_async().await` |
| `handle.ask(msg)` | Use reply envelope pattern |
| `Reply::with(value)` | Use `Reply::pending` + reply envelope |
| Forgetting `.await` on `start()` | `builder.start().await` |
| Mutating in `act_on` | Use `mutate_on` for state changes |
| Using `_msg` parameter | Use `envelope.message()` |
