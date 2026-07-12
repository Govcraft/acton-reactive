---
title: Troubleshooting
description: Common problems and their solutions.
---

Solutions to common issues when working with Acton Reactive.

## Compilation Errors

### "cannot find attribute `acton_actor` in this scope"

**Problem**: The attribute macro isn't in scope.

**Solution**: Import the prelude:

```rust
use acton_reactive::prelude::*;
```

---

### "the trait `Default` is not implemented"

**Problem**: Actor state must implement `Default`.

**Solution**: Add `#[derive(Default)]` or implement it manually:

```rust
#[acton_actor]
#[derive(Default)]
struct MyActor {
    count: i32,
}
```

Or for custom initialization:

```rust
impl Default for MyActor {
    fn default() -> Self {
        Self { count: 100 }
    }
}
```

---

### "expected `Pin<Box<dyn Future<Output = ()> + Send + Sync>>`, found `()`"

**Problem**: `mutate_on` and `act_on` handlers must return a boxed future, not `()`. `Reply` is the helper that builds one.

**Solution**: Return `Reply::ready()` for no async work, or use a `_sync` variant that returns `()` directly:

```rust
// Async handler — must return a Reply::* value
builder.mutate_on::<Message>(|actor, ctx| {
    actor.model.value = ctx.message().value;
    Reply::ready()  // Don't forget this!
});

// Sync handler — returns () directly, no Reply needed
builder.mutate_on_sync::<Message>(|actor, ctx| {
    actor.model.value = ctx.message().value;
});
```

---

### "future cannot be shared between threads safely" / "future created by async block is not `Sync`"

**Problem**: This is the most common — and most surprising — handler error.

Handlers return `Pin<Box<dyn Future<Output = ()> + Send + Sync + 'static>>`. Note the **`Sync`** bound: everything your `Reply::pending` async block holds *across an `.await`* must be `Sync`, not just `Send`. Plenty of perfectly good types (and plenty of third-party futures) are `Send` but not `Sync`.

```rust
// Bad: RefCell is Send but NOT Sync — held across an await
builder.act_on::<Fetch>(|_actor, _ctx| {
    let cell = std::cell::RefCell::new(0);
    Reply::pending(async move {
        do_work().await;
        let _ = cell.borrow();  // error: future is not `Sync`
    })
});
```

**Solution 1 — don't hold the non-`Sync` value across the await.** Finish with it before the first `.await`, or swap it for a `Sync` equivalent (`Mutex` instead of `RefCell`, `Arc` instead of `Rc`).

**Solution 2 — move the work off the handler.** This is the right answer when the *future itself* is not `Sync` (common with HTTP and database clients). Spawn the work with `tokio::spawn`, which only requires `Send`, and message the result back to the actor:

```rust
builder.mutate_on::<Fetch>(|actor, ctx| {
    let handle = actor.handle().clone();
    let url = ctx.message().url.clone();

    tokio::spawn(async move {
        // Any Send future works here — no Sync bound
        let body = fetch(&url).await;
        handle.send(FetchDone { body }).await;
    });

    Reply::ready()
});
```

See [Integration](/docs/advanced/integration) for the full pattern.

---

### "`Send` is not implemented for..."

**Problem**: Something in your async block isn't thread-safe at all.

**Solution**: Ensure data moved into async blocks is `Send`:

```rust
// Bad: Rc is not Send
let data = Rc::new(value);

// Good: Arc is Send
let data = Arc::new(value);
```

---

## Runtime Errors

### A handler panicked — did my actor die?

**No, not by default.** The `catch-handler-panics` feature is enabled by default. It wraps every handler dispatch in `catch_unwind`, so a panicking handler is caught and logged with `error!` and the actor **keeps processing messages**. A panic does not terminate the actor and does not trigger the supervision flow.

If you're seeing a panic logged but the actor is still alive, that's working as designed. Look for the `Panic in ... message handler` log line to find it:

```toml
[dependencies]
{% $dep.base %}
```

If you want a panicking handler to bring the actor down instead — or you've measured the `catch_unwind` overhead and want it gone in a well-tested production workload — turn the feature off:

```toml
[dependencies]
acton-reactive = { version = "8", default-features = false }
```

For *expected* failures, don't rely on panics at all. Use `try_mutate_on` / `try_act_on` with a real error type and register an `on_error` handler:

```rust
builder
    .try_mutate_on::<RiskyMessage, Success, MyError>(|actor, ctx| {
        Reply::try_pending(async move { do_risky_thing().await })
    })
    .on_error::<RiskyMessage, MyError>(|actor, ctx, err| {
        tracing::error!("Risky op failed: {}", err);
        Reply::ready()
    });
```

---

### Actor stops unexpectedly

**Problem**: The actor's inbox closed, or its parent shut down.

**Solution**: Check for these, in order:

1. **Parent shutdown** — children stop when their parent stops (cascading shutdown). Terminating the parent is the most common cause.
2. **Inbox closed** — every `ActorHandle` for the actor was dropped. Keep a handle alive for as long as you need the actor.
3. **`shutdown_all()`** was called on the runtime.

If the actor has a parent, the parent receives a `ChildTerminated` message carrying the `TerminationReason` (`Normal`, `InboxClosed`, or `ParentShutdown`). Handle it to find out why:

```rust
parent.mutate_on::<ChildTerminated>(|actor, ctx| {
    let note = ctx.message();
    tracing::warn!("Child {} terminated: {:?}", note.child_id, note.reason);
    Reply::ready()
});
```

See [Supervision Basics](/docs/core-concepts/supervision-basics).

---

### Messages not being received

**Possible causes**:

1. **Actor stopped**: Check if the actor is still running
2. **Wrong handler type**: Using `act_on` when you need `mutate_on`
3. **Not awaiting send**: `handle.send(msg).await`

**Debug with logging**:

```rust
builder.mutate_on::<Message>(|actor, envelope| {
    tracing::debug!("Received message: {:?}", envelope.message());
    // ...
    Reply::ready()
});
```

---

### Reply envelope not working

**Problem**: Response never arrives at sender.

**Solution**: Ensure the sender has a handler for the response type:

```rust
// Sender must handle the response
sender.mutate_on::<CountResponse>(|actor, envelope| {
    let count = envelope.message().0;
    println!("Got count: {}", count);
    Reply::ready()
});
```

---

### Deadlock between actors

**Problem**: Actor A waits for B, B waits for A via reply envelopes.

**Solution**: Avoid circular request chains. Use fire-and-forget with callbacks:

```rust
// Bad: potential circular wait
// Actor A sends to B, B sends back to A, A sends back to B...

// Good: use fire-and-forget
actor_b.send(QueryRequest { reply_to: self_handle }).await;
```

---

## Performance Issues

### Slow message processing

**Possible causes**:

1. **Blocking in handlers**: Use async or spawn blocking work
2. **Single actor bottleneck**: Use worker pools
3. **Too many clones**: Use `Arc` for large data

**Solution for blocking work**:

```rust
builder.act_on::<HeavyWork>(|actor, envelope| {
    let data = envelope.message().data.clone();
    let reply = envelope.reply_envelope();

    Reply::pending(async move {
        // Move to blocking thread pool
        let result = tokio::task::spawn_blocking(move || {
            heavy_computation(&data)
        }).await.unwrap();

        reply.send(WorkResult(result)).await;
    })
});
```

---

### Memory growing over time

**Problem**: Actor state accumulating without cleanup.

**Solution**: Implement periodic cleanup:

```rust
#[acton_message]
struct Cleanup;

builder.mutate_on::<Cleanup>(|actor, _envelope| {
    actor.model.cache.retain(|_, v| !v.is_expired());
    Reply::ready()
});

// Schedule cleanup
let cleanup_handle = handle.clone();
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        cleanup_handle.send(Cleanup).await;
    }
});
```

---

## IPC Issues

### "Connection refused"

**Problem**: Socket doesn't exist or server isn't running.

**Solution**: Verify the server started successfully:

```rust
// Server
let listener = runtime.start_ipc_listener().await
    .expect("Failed to start IPC listener");
println!("IPC listener started");

// Client - check socket exists
let socket_path = IpcConfig::load().socket_path();
if !socket_path.exists() {
    // socket_path is a PathBuf — use .display() to print it
    eprintln!("Socket not found at: {}", socket_path.display());
}
```

---

### "Permission denied" on socket

**Problem**: Unix socket file permissions.

**Solution**: Set appropriate permissions or run with correct user:

```bash
# Check permissions
ls -la /run/user/$(id -u)/acton/

# Socket should be writable by your user
```

---

### Subscriber clients keep getting disconnected

**Problem**: A client that subscribes and then only *listens* (receiving push notifications without sending anything) gets dropped by the server after a period of silence.

**Cause**: The listener applies a read timeout to idle connections. Two separate timeouts are in play, and which one you get depends on whether the connection has an active subscription:

| Setting | Applies to | Default |
|---------|-----------|---------|
| `read_timeout_ms` | Connections with **no** active subscription | 60000 (60s) |
| `subscription_read_timeout_ms` | Connections **with** an active subscription | 0 |

**In both settings, `0` is a sentinel meaning "no timeout"** — the connection may stay idle indefinitely.

Subscription connections already default to `0`, so a pure subscriber should not time out. If yours is being dropped, check that the subscription actually registered (the server only applies `subscription_read_timeout_ms` once the connection has a live subscription), or raise the plain read timeout:

```toml
# $XDG_CONFIG_HOME/acton/ipc.toml
[timeouts]
read_timeout_ms = 0               # 0 = never time out an idle connection
subscription_read_timeout_ms = 0  # already the default
```

{% callout type="warning" title="0 disables the timeout entirely" %}
Setting `read_timeout_ms = 0` means idle connections are never reaped. That's the right call for long-lived subscribers, but on a public-facing socket it lets dead connections accumulate. Prefer leaving `read_timeout_ms` alone and relying on the subscription-specific default.
{% /callout %}

---

## Testing Issues

### Tests hang

**Problem**: Actors not shut down, runtime waiting.

**Solution**: Always shutdown in tests:

```rust
#[tokio::test]
async fn test() {
    let mut runtime = ActonApp::launch_async().await;
    // ... test code ...
    runtime.shutdown_all().await.ok();  // Don't forget!
}
```

---

### Flaky tests

**Problem**: Race conditions in async tests.

**Solution**: Use atomic counters and allow processing time:

```rust
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

#[tokio::test]
async fn test() {
    let mut runtime = ActonApp::launch_async().await;
    let count = Arc::new(AtomicI32::new(0));
    let count_clone = count.clone();

    // Set up actor that updates atomic counter
    let mut actor = runtime.new_actor::<MyActor>();
    actor.mutate_on::<Increment>(move |_actor, _env| {
        count_clone.fetch_add(1, Ordering::SeqCst);
        Reply::ready()
    });

    let handle = actor.start().await;
    handle.send(Increment).await;

    // Wait for async processing
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    assert_eq!(count.load(Ordering::SeqCst), 1);
    runtime.shutdown_all().await.ok();
}
```

---

## Getting Help

If you can't find your answer here:

1. Check the [API docs](https://docs.rs/acton-reactive)
2. Search [GitHub issues](https://github.com/Govcraft/acton-reactive/issues)
3. Open a new issue with:
   - Minimal reproduction code
   - Error messages
   - Rust and acton-reactive versions
