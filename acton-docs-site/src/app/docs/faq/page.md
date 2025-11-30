---
title: FAQ
nextjs:
  metadata:
    title: FAQ - acton-reactive
    description: Frequently asked questions about acton-reactive.
---

Frequently asked questions about `acton-reactive`.

---

## Frequently Asked Questions

### What's the difference between acton-reactive and Actix?

Both are actor frameworks for Rust, but they have different goals:

| | acton-reactive | Actix |
|---|----------------|-------|
| **Focus** | Simplicity, accessibility | Performance, ecosystem |
| **Message Types** | Any `Clone + Debug` type | Requires `actix::Message` trait |
| **Learning Curve** | Gentler | Steeper |
| **Ecosystem** | Standalone | actix-web, actix-rt, etc. |
| **Maturity** | Pre-1.0, evolving | Mature, stable |

**Choose acton-reactive if:** You want to learn actor patterns without fighting boilerplate, or you need a lightweight actor system for a specific use case.

**Choose Actix if:** You need maximum performance, want to build on the actix-web ecosystem, or need a battle-tested solution for production.

---

### How do I send a reply back to the sender?

Use the `reply_envelope` from the message context:

```rust
actor.mutate_on::<RequestMessage>(|actor, ctx| {
    let reply = ctx.reply_envelope();
    let data = actor.model.some_data.clone();

    Reply::pending(async move {
        reply.send(ResponseMessage { data }).await;
    })
});
```

Or for simple, synchronous replies:

```rust
actor.mutate_on::<RequestMessage>(|actor, ctx| {
    ctx.reply(ResponseMessage { data: actor.model.data.clone() });
    Reply::ready()
});
```

---

### How do I create child actors?

Use the `supervise` method on a started actor:

```rust
let parent_handle = parent.start().await;

// Create child
let child = runtime.new_actor::<ChildState>();
let child_handle = parent_handle.supervise(child).await?;

// When parent stops, child stops automatically
```

---

### Can actors communicate across processes?

Yes! Enable the `ipc` feature and use Unix Domain Sockets:

```toml
[dependencies]
acton-reactive = { version = "0.1", features = ["ipc"] }
```

See the [IPC Communication](/docs/ipc) guide for details.

---

### Is acton-reactive production ready?

`acton-reactive` is pre-1.0 software. The core functionality is stable and well-tested, but the API may change in minor versions. We recommend:

- Pinning to a specific version in production
- Reviewing changelogs before upgrading
- Using it for appropriate use cases (not mission-critical systems without thorough testing)

---

### How do I debug message flow?

Enable tracing to see what's happening:

```rust
use tracing_subscriber;

#[acton_main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    // Your code here
}
```

You'll see messages like:
```text
DEBUG acton_reactive: Actor actor_1 received message: Increment
DEBUG acton_reactive: Handler for Increment completed
```

---

### How fast is acton-reactive?

It depends on your use case, but generally:

- **Message throughput:** Hundreds of thousands of messages per second on modern hardware
- **Latency:** Sub-millisecond for simple handlers
- **Memory:** Each actor adds minimal overhead (mostly channel buffers)

For performance-critical applications, profile your specific workload. The `act_on` handlers run concurrently, which can significantly improve throughput for read-heavy workloads.

---

### Can I use acton-reactive with async database clients?

Absolutely! Handlers return futures, so you can use any async code:

```rust
actor.mutate_on::<SaveUser>(|actor, ctx| {
    let user = ctx.message().user.clone();
    let db = actor.model.db_pool.clone();

    Reply::pending(async move {
        sqlx::query("INSERT INTO users ...")
            .bind(&user.name)
            .execute(&db)
            .await
            .expect("DB error");
    })
});
```

---

### How do I handle errors in handlers?

Use fallible handlers with `Reply::try_*` helpers:

```rust
// Immediate result (sync)
actor.try_mutate_on::<RiskyOperation>(|actor, ctx| {
    if something_bad() {
        Reply::try_err(MyError::new("something went wrong"))
    } else {
        Reply::try_ok(SuccessResult)
    }
});

// Or with async operations
actor.try_mutate_on::<RiskyOperation>(|actor, ctx| {
    Reply::try_pending(async move {
        let result = do_risky_thing().await?;
        Ok(SuccessResult { data: result })
    })
});

// Register error handler
actor.on_error::<RiskyOperation, MyError>(|actor, ctx, error| {
    println!("Error: {}", error);
    Reply::ready()
});
```

---

## Common Gotchas

### Gotcha: Borrowing `actor` in async blocks

**Problem:**

```rust
actor.mutate_on::<MyMessage>(|actor, ctx| {
    Reply::pending(async move {
        // ERROR: actor.model is borrowed in async block
        println!("{}", actor.model.value);
    })
});
```

**Solution:** Clone what you need before the async block:

```rust
actor.mutate_on::<MyMessage>(|actor, ctx| {
    let value = actor.model.value; // Clone before async

    Reply::pending(async move {
        println!("{}", value);  // Use the clone
    })
});
```

---

### Gotcha: Forgetting to await shutdown

**Problem:**

```rust
fn main() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let mut app = ActonApp::launch();
        // ... do stuff
    }); // Runtime drops, actors may not finish!
}
```

**Solution:** Always await `shutdown_all`:

```rust
#[acton_main]
async fn main() {
    let mut app = ActonApp::launch();
    // ... do stuff
    app.shutdown_all().await.expect("Shutdown failed");
}
```

---

### Gotcha: Using the wrong handler type

**Problem:** Using `act_on` (read-only) when you need to mutate state:

```rust
// This won't compile!
actor.act_on::<Increment>(|actor, ctx| {
    actor.model.count += 1;  // ERROR: actor.model is immutable
    Reply::ready()
});
```

**Solution:** Use `mutate_on` for mutations:

```rust
actor.mutate_on::<Increment>(|actor, ctx| {
    actor.model.count += 1;  // Works!
    Reply::ready()
});
```

---

### Gotcha: Subscription order matters

**Problem:** Subscribing after messages are sent means you miss them:

```rust
broker.broadcast(ImportantEvent).await;  // Sent before subscription!
actor.handle().subscribe::<ImportantEvent>().await;  // Too late
```

**Solution:** Subscribe before starting, or before any broadcasts:

```rust
actor.handle().subscribe::<ImportantEvent>().await;  // Subscribe first
let handle = actor.start().await;
// Now safe to broadcast
```

---

### Gotcha: Deadlock from synchronous send

**Problem:** Calling `send_sync` from within a handler can deadlock if the channel is full:

```rust
actor.mutate_on::<Trigger>(|actor, ctx| {
    // DANGER: If the channel is full, this blocks forever
    some_handle.send_sync(BlockingMessage, &address);
    Reply::ready()
});
```

**Solution:** Use async send inside handlers:

```rust
actor.mutate_on::<Trigger>(|actor, ctx| {
    let handle = some_handle.clone();

    Reply::pending(async move {
        handle.send(AsyncMessage).await;  // Non-blocking
    })
});
```

---

### Gotcha: Message type confusion

**Problem:** Two message types with the same name in different modules:

```rust
mod a {
    use acton_reactive::prelude::*;
    #[acton_message]
    pub struct Event { pub value: i32 }
}

mod b {
    use acton_reactive::prelude::*;
    #[acton_message]
    pub struct Event { pub data: String }  // Different type!
}

// Handler registered for a::Event
actor.mutate_on::<a::Event>(|actor, ctx| { ... });

// But sending b::Event - handler won't fire!
handle.send(b::Event { data: "test".into() }).await;
```

**Solution:** Be explicit about types, and consider unique naming:

```rust
use acton_reactive::prelude::*;

#[acton_message]
pub struct SensorEvent { pub value: i32 }

#[acton_message]
pub struct UserEvent { pub data: String }
```

---

## Performance Tips

### Tip: Use `act_on` for read-heavy workloads

If a handler only reads state and doesn't modify it, use `act_on` instead of `mutate_on`. Multiple `act_on` handlers can run concurrently:

```rust
// These can run in parallel
actor.act_on::<Query1>(|actor, ctx| { /* read-only */ });
actor.act_on::<Query2>(|actor, ctx| { /* read-only */ });
actor.act_on::<Query3>(|actor, ctx| { /* read-only */ });
```

---

### Tip: Clone data, not the whole actor

When you need data in an async block, clone just what you need:

```rust
// Good - clone only what's needed
let value = actor.model.expensive_data.clone();

Reply::pending(async move {
    use_value(value).await;
})

// Bad - cloning entire model when you only need one field
let model = actor.model.clone();

Reply::pending(async move {
    use_value(model.expensive_data).await;
})
```

---

### Tip: Adjust inbox capacity for your workload

High-throughput actors might benefit from larger inbox buffers:

```toml
# ~/.config/acton/config.toml
[limits]
actor_inbox_capacity = 1000  # Default is 255
```

---

### Tip: Use MessagePack for IPC

If you're using IPC heavily, MessagePack is faster and smaller than JSON:

```toml
[dependencies]
acton-reactive = { version = "0.1", features = ["ipc-messagepack"] }
```

---

## Still Stuck?

If you can't find the answer here:

1. **Check the [Examples](/docs/examples)** - They cover most common patterns
2. **Read the [API Reference](/docs/api-reference)** - Every type is documented
3. **Look at the tests** in the `acton-reactive` repository - They're essentially documentation
4. **Open an issue** on [GitHub](https://github.com/govcraft/acton-reactive) - We're happy to help

---

## Migration Guide: v4.x to v5.0

Version 5.0 renamed the handler methods for clarity:

### Handler Renames

| v4.x | v5.0 | Purpose |
|------|------|---------|
| `act_on` (mutable) | `mutate_on` | Mutable state access, sequential |
| - | `act_on` | Read-only state access, concurrent |

### Before (v4.x)

```rust
builder.act_on::<MyMessage>(|actor, _| {
    actor.model.value += 1;  // Was mutable in v4
    Reply::ready()
});
```

### After (v5.0)

```rust
// For mutations, use mutate_on
builder.mutate_on::<MyMessage>(|actor, _| {
    actor.model.value += 1;
    Reply::ready()
});

// act_on is now read-only and concurrent
builder.act_on::<QueryMessage>(|actor, _| {
    let value = actor.model.value;  // Read-only
    Reply::ready()
});
```

### Quick Migration

1. Replace all `act_on` calls that mutate state with `mutate_on`
2. Keep `act_on` for read-only operations (they'll now run concurrently!)
3. Run `cargo build` - the compiler will catch any mistakes
