---
title: Troubleshooting
description: Common problems and their solutions.
---

Solutions to common issues when working with Acton Reactive.

## Compilation Errors

### "cannot find macro `acton_actor`"

**Problem**: The macro isn't in scope.

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

### "expected `Reply`, found `()`"

**Problem**: Handler must return a `Reply`.

**Solution**: Return `Reply::ready()` for no response:

```rust
builder.mutate_on::<Message>(|actor, envelope| {
    actor.model.value = envelope.message().value;
    Reply::ready()  // Don't forget this!
});
```

---

### "`Send` is not implemented for..."

**Problem**: Something in your async block isn't thread-safe.

**Solution**: Ensure data moved into async blocks is `Send`:

```rust
// Bad: Rc is not Send
let data = Rc::new(value);

// Good: Arc is Send
let data = Arc::new(value);
```

---

## Runtime Errors

### Actor stops unexpectedly

**Problem**: A panic in a handler stops the actor.

**Solution**: Catch panics or use Result types:

```rust
builder.mutate_on::<RiskyMessage>(|actor, envelope| {
    // Catch potential panics
    let result = std::panic::catch_unwind(|| {
        risky_operation()
    });

    match result {
        Ok(_) => Reply::ready(),
        Err(e) => {
            tracing::error!("Panic caught: {:?}", e);
            Reply::ready()
        }
    }
});
```

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
if !std::path::Path::new(&socket_path).exists() {
    eprintln!("Socket not found at: {}", socket_path);
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
