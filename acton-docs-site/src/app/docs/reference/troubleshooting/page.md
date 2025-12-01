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
builder.mutate_on::<Message>(|actor, msg| {
    actor.model.value = msg.value;
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
builder.mutate_on::<RiskyMessage>(|actor, msg| {
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
builder.mutate_on::<Message>(|actor, msg| {
    tracing::debug!("Received message: {:?}", msg);
    // ...
    Reply::ready()
});
```

---

### `ask` hangs forever

**Problem**: The handler never returns a Reply.

**Solution**: Ensure all code paths return a Reply:

```rust
// Bad: missing Reply in else branch
builder.act_on::<Query>(|actor, msg| {
    if actor.model.ready {
        Reply::with(actor.model.value)
    }
    // Missing else!
});

// Good: all paths return Reply
builder.act_on::<Query>(|actor, msg| {
    if actor.model.ready {
        Reply::with(Some(actor.model.value))
    } else {
        Reply::with(None)
    }
});
```

---

### Deadlock between actors

**Problem**: Actor A waits for B, B waits for A.

**Solution**: Avoid circular `ask` chains. Use `send` or restructure:

```rust
// Bad: potential deadlock
// Actor A: let result = actor_b.ask(Query).await;
// Actor B: let result = actor_a.ask(Query).await;

// Good: use fire-and-forget with callbacks
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
builder.act_on::<HeavyWork>(|actor, msg| {
    let data = msg.data.clone();
    Reply::pending(async move {
        // Move to blocking thread pool
        tokio::task::spawn_blocking(move || {
            heavy_computation(&data)
        }).await.unwrap()
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

builder.mutate_on::<Cleanup>(|actor, _| {
    actor.model.cache.retain(|_, v| !v.is_expired());
    Reply::ready()
});

// Schedule cleanup
let handle = handle.clone();
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        handle.send(Cleanup).await.ok();
    }
});
```

---

## IPC Issues

### "Connection refused"

**Problem**: Socket doesn't exist or wrong path.

**Solution**: Verify the server is running and socket path matches:

```rust
// Server
handle.enable_ipc("/tmp/my-app.sock").await;

// Client - must match exactly
let client = IpcClient::connect("/tmp/my-app.sock").await?;
```

---

### "Permission denied" on socket

**Problem**: Unix socket file permissions.

**Solution**: Set appropriate permissions or run with correct user:

```bash
# Check permissions
ls -la /tmp/my-app.sock

# Or use a path in a writable directory
```

---

## Testing Issues

### Tests hang

**Problem**: Actors not shut down, runtime waiting.

**Solution**: Always shutdown in tests:

```rust
#[tokio::test]
async fn test() {
    let mut app = ActonApp::launch();
    // ... test code ...
    app.shutdown_all().await.ok();  // Don't forget!
}
```

---

### Flaky tests

**Problem**: Race conditions in async tests.

**Solution**: Use explicit synchronization:

```rust
#[tokio::test]
async fn test() {
    let mut app = ActonApp::launch();
    let handle = setup_actor(&mut app).await;

    handle.send(DoWork).await.ok();

    // Wait for work to complete
    let result: bool = handle.ask(IsComplete).await;
    assert!(result);

    app.shutdown_all().await.ok();
}
```

---

## Getting Help

If you can't find your answer here:

1. Check the [API docs](https://docs.rs/acton-reactive)
2. Search [GitHub issues](https://github.com/acton-lang/acton-reactive/issues)
3. Open a new issue with:
   - Minimal reproduction code
   - Error messages
   - Rust and acton-reactive versions

