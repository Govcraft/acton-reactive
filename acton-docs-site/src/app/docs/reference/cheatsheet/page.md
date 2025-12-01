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
    let mut app = ActonApp::launch();

    let handle = app
        .new_actor::<MyActor>()
        .mutate_on::<MyMessage>(|actor, msg| {
            // handle message
            Reply::ready()
        })
        .start()
        .await;

    handle.send(MyMessage).await.ok();
    app.shutdown_all().await.ok();
}
```

---

## Message Patterns

### Fire-and-Forget

```rust
handle.send(DoSomething).await.ok();
```

### Request-Response

```rust
let result: i32 = handle.ask(GetValue).await;
```

### Broadcast

```rust
// Publisher
let broker = app.get_broker();
broker.publish(Event { data: "hello" }).await;

// Subscriber
handle.subscribe::<Event>().await;
builder.act_on::<Event>(|actor, msg| {
    println!("Got: {}", msg.data);
    Reply::ready()
});
```

---

## Handler Patterns

### Mutate State

```rust
builder.mutate_on::<Increment>(|actor, _| {
    actor.model.count += 1;
    Reply::ready()
});
```

### Read State

```rust
builder.act_on::<GetCount>(|actor, _| {
    Reply::with(actor.model.count)
});
```

### Async Handler

```rust
builder.act_on::<FetchData>(|actor, msg| {
    let url = msg.url.clone();
    Reply::pending(async move {
        let resp = reqwest::get(&url).await.unwrap();
        resp.text().await.unwrap()
    })
});
```

---

## Child Actors

### Spawn Child

```rust
builder.mutate_on::<SpawnChild>(|actor, _| {
    let child = actor
        .new_child::<ChildActor>()
        .mutate_on::<ChildMessage>(handler)
        .start();

    Reply::pending(async move {
        let handle = child.await;
        // store handle if needed
    })
});
```

### Access Children

```rust
for (id, child) in actor.children() {
    child.send(Ping).await.ok();
}
```

---

## Lifecycle Hooks

### After Start

```rust
builder.after_start(|actor| {
    println!("Actor started!");
    Reply::ready()
});
```

### Before Stop

```rust
builder.before_stop(|actor| {
    println!("Actor stopping...");
    Reply::ready()
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

// Initialize in after_start or via message
```

---

## Error Handling

### In Handlers

```rust
builder.mutate_on::<RiskyOp>(|actor, msg| {
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

### Propagate to Caller

```rust
builder.act_on::<Query>(|actor, msg| {
    Reply::with(
        do_query().map_err(|e| format!("{}", e))
    )
});

// Caller
let result: Result<Data, String> = handle.ask(Query).await;
```

---

## Testing

### Basic Test

```rust
#[tokio::test]
async fn test_actor() {
    let mut app = ActonApp::launch();

    let handle = app
        .new_actor::<Counter>()
        .mutate_on::<Increment>(increment_handler)
        .act_on::<GetCount>(get_count_handler)
        .start()
        .await;

    handle.send(Increment).await.ok();
    let count: i32 = handle.ask(GetCount).await;

    assert_eq!(count, 1);
    app.shutdown_all().await.ok();
}
```

---

## Quick Imports

```rust
// Everything you need
use acton_reactive::prelude::*;

// For IPC
use acton_reactive::ipc::IpcClient;
```

---

## Common Mistakes

| Wrong | Right |
|-------|-------|
| `ActonApp::launch().await` | `ActonApp::launch()` (sync) |
| `Reply::with(())` for no return | `Reply::ready()` |
| Forgetting `.await` on `start()` | `builder.start().await` |
| Mutating in `act_on` | Use `mutate_on` for state changes |

