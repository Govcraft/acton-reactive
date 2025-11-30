---
title: Troubleshooting
nextjs:
  metadata:
    title: Troubleshooting - acton-reactive
    description: Common errors and their solutions when working with acton-reactive.
---

Common errors and their solutions when working with `acton-reactive`.

---

## Compilation Errors

### "unresolved import `acton_reactive`"

**Cause:** Cargo can't find the crate.

**Fix:** Make sure your `Cargo.toml` has the dependency:

```toml
[dependencies]
acton-reactive = "0.1"
```

Then run `cargo build` to fetch it.

---

### "the trait `Default` is not implemented"

**Cause:** Your actor state struct doesn't implement `Default`.

**Fix:** Add `#[acton_actor]` macro or derive `Default` manually:

```rust
// Option 1: Use the macro
#[acton_actor]
struct MyState {
    count: u32,
}

// Option 2: Derive manually
#[derive(Default, Clone, Debug)]
struct MyState {
    count: u32,
}
```

---

### "async runtime not available"

**Cause:** You're missing the `#[acton_main]` attribute on your main function.

**Fix:** Make sure your main function uses the `#[acton_main]` macro:

```rust
use acton_reactive::prelude::*;

#[acton_main]  // This sets up the async runtime for you!
async fn main() {
    let mut runtime = ActonApp::launch();
    // ...
}
```

The `#[acton_main]` macro automatically configures the async runtime with the correct settings for Acton. You don't need to add tokio to your `Cargo.toml` - it's included with acton-reactive.

---

### "cannot find macro `acton_actor`"

**Cause:** You're not importing the prelude.

**Fix:** Import the prelude which includes all macros:

```rust
use acton_reactive::prelude::*;  // Includes acton_actor, acton_message, etc.
```

---

## Runtime Issues

### Messages aren't being received

**Common causes:**

1. **Handler not registered:** Did you call `mutate_on` or `act_on` for that message type?

   ```rust
   actor.mutate_on::<MyMessage>(|actor, ctx| {
       // Handler must be registered!
       Reply::ready()
   });
   ```

2. **Actor not started:** Handlers only work after `start()`:

   ```rust
   let handle = actor.start().await;  // Start first!
   handle.send(MyMessage).await;      // Then send
   ```

3. **Wrong message type:** Rust's type system is strict. `MyMessage` and `my_message::MyMessage` are different types.

4. **Shutdown too fast:** If your program exits immediately after sending, messages might not process:

   ```rust
   handle.send(MyMessage).await;
   tokio::time::sleep(Duration::from_millis(100)).await;  // Give it time
   runtime.shutdown_all().await?;
   ```

---

### Broadcasts aren't received

**Cause:** Actor didn't subscribe, or subscribed after the broadcast.

**Fix:** Subscribe *before* starting the actor:

```rust
// Good order
actor.handle().subscribe::<MyEvent>().await;
let handle = actor.start().await;

// Bad order - might miss messages
let handle = actor.start().await;
handle.subscribe::<MyEvent>().await;  // Too late for early broadcasts
```

---

### Handler panics crash the actor

**Cause:** Panics in handlers are not caught by default.

**Fix:** Use `try_mutate_on` or `try_act_on` for fallible operations:

```rust
actor.try_mutate_on::<RiskyOperation>(|actor, ctx| {
    if something_bad() {
        Reply::try_err(MyError::new("something went wrong"))
    } else {
        Reply::try_ok(Success)
    }
});

actor.on_error::<RiskyOperation, MyError>(|actor, ctx, error| {
    println!("Error handled: {}", error);
    Reply::ready()
});
```

---

## Performance Issues

### Compilation is slow

**Cause:** Probably building all of tokio's features every time.

**Fix:** Use incremental compilation and optimize dependencies:

```toml
# Cargo.toml
[profile.dev]
incremental = true

[profile.dev.package."*"]
opt-level = 2  # Optimize deps, not your code
```

---

### Messages processing slowly

**Cause:** Using `mutate_on` for read-only operations serializes all handlers.

**Fix:** Use `act_on` for read-only operations - they run concurrently:

```rust
// Bad: serializes all queries
actor.mutate_on::<GetData>(|actor, ctx| { /* read-only but sequential */ });

// Good: queries run concurrently
actor.act_on::<GetData>(|actor, ctx| { /* read-only and parallel */ });
```

---

### Actor inbox filling up

**Cause:** Messages arriving faster than they're processed.

**Fix:** Increase inbox capacity or add backpressure:

```toml
# ~/.config/acton/config.toml
[limits]
actor_inbox_capacity = 1000  # Default is 255
```

Or use `try_send` to handle backpressure:

```rust
if handle.try_send(Message).is_err() {
    // Handle backpressure - maybe queue, drop, or slow down
}
```

---

## IPC Issues

### IPC socket already in use

**Cause:** A previous process left the socket file behind.

**Fix:** Delete the stale socket:

```shell
rm /tmp/acton.sock
```

Or handle it in your code:

```rust
use std::path::Path;

let socket_path = Path::new("/tmp/acton.sock");
if socket_path.exists() {
    std::fs::remove_file(socket_path)?;
}
```

---

### IPC connection refused

**Cause:** Server not running or wrong socket path.

**Fix:** Verify the server is running and check the socket path:

```rust
use acton_reactive::ipc::{socket_exists, socket_is_alive};

let path = "/tmp/acton.sock";

if !socket_exists(path) {
    println!("Socket doesn't exist");
} else if !socket_is_alive(path) {
    println!("Socket exists but server not responding");
} else {
    println!("Server is ready");
}
```

---

### IPC messages not deserializing

**Cause:** Message type not registered in the IPC type registry.

**Fix:** Register all message types before starting the listener:

```rust
let registry = runtime.ipc_registry();
registry.register::<MyRequest>("MyRequest");
registry.register::<MyResponse>("MyResponse");
```

---

## Lifecycle Issues

### Actor stops unexpectedly

**Cause:** Parent actor stopped, or cancellation token triggered.

**Fix:** Check supervision hierarchy and cancellation sources:

```rust
actor.before_stop(|actor| {
    println!("Actor {} stopping", actor.id());
    Reply::ready()
});

actor.after_stop(|actor| {
    println!("Actor {} stopped", actor.id());
    Reply::ready()
});
```

---

### Hooks not running

**Cause:** Hooks registered after `start()` was called.

**Fix:** Register hooks before starting:

```rust
// Good: hooks registered before start
actor.before_start(|_| { Reply::ready() });
actor.after_stop(|_| { Reply::ready() });
let handle = actor.start().await;

// Won't work: can't add hooks after start
let handle = actor.start().await;
// actor.before_start(...);  // This method doesn't exist on started actors
```

---

## Debugging Tips

### Enable tracing

```rust
use tracing_subscriber;

#[acton_main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    // Your code here - will show message flow
}
```

### Check actor state in tests

```rust
actor.after_stop(|actor| {
    assert_eq!(actor.model.count, 10);
    Reply::ready()
});
```

### Print actor IDs

```rust
let handle = actor.start().await;
println!("Actor ID: {}", handle.id());
```

---

## Still Stuck?

- Check the [FAQ](/docs/faq) for common questions
- Look at [Examples](/docs/examples) for working patterns
- Review [API Reference](/docs/api-reference) for detailed documentation
- Open an issue on [GitHub](https://github.com/govcraft/acton-reactive)
