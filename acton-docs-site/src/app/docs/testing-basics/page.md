---
title: Testing Basics
nextjs:
  metadata:
    title: Testing Basics - acton-reactive
    description: Getting started with testing acton-reactive applications.
---

This guide covers the fundamentals of testing `acton-reactive` applications, including test setup, basic patterns, and state verification.

---

## Test Setup

### Dependencies

Add test dependencies to your `Cargo.toml`:

```toml
[dev-dependencies]
acton-test = "0.1"  # Optional: Test utilities
```

{% callout type="note" title="Tokio Available" %}
Since `acton-reactive` re-exports `tokio` in its prelude, you can use `#[tokio::test]` directly without adding tokio as a dev-dependency. If you need additional tokio features like `test-util`, add it explicitly.
{% /callout %}

### Basic Test Structure

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use acton_reactive::prelude::*;

    #[tokio::test]
    async fn test_actor_behavior() {
        // 1. Setup
        let mut runtime = ActonApp::launch_async().await;

        // 2. Create and configure actor
        let mut actor = runtime.new_actor::<TestState>();
        // ... configure handlers

        let handle = actor.start().await;

        // 3. Test - send messages
        handle.send(TestMessage).await;

        // 4. Verify (via hooks or channels)

        // 5. Cleanup
        runtime.shutdown_all().await.expect("Shutdown failed");
    }
}
```

---

## Unit Testing Actors

### Testing Initial State

Verify default state using `after_stop`:

```rust
use acton_reactive::prelude::*;

#[acton_actor]
struct CounterState {
    count: u32,
}

#[tokio::test]
async fn test_initial_state() {
    let mut runtime = ActonApp::launch_async().await;

    let mut counter = runtime.new_actor::<CounterState>();

    // Verify initial state via after_stop hook
    counter.after_stop(|actor| {
        assert_eq!(actor.model.count, 0);
        Reply::ready()
    });

    let _handle = counter.start().await;
    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

### Testing State Mutations

```rust
#[acton_message]
struct Increment(u32);

#[tokio::test]
async fn test_increment_handler() {
    let mut runtime = ActonApp::launch_async().await;

    let mut counter = runtime.new_actor::<CounterState>();

    counter
        .mutate_on::<Increment>(|actor, ctx| {
            actor.model.count += ctx.message().0;
            Reply::ready()
        })
        .after_stop(|actor| {
            // Verify final state
            assert_eq!(actor.model.count, 15);
            Reply::ready()
        });

    let handle = counter.start().await;

    // Send test messages
    handle.send(Increment(5)).await;
    handle.send(Increment(10)).await;

    // Allow processing
    tokio::time::sleep(Duration::from_millis(100)).await;

    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

---

## Verification with Channels

Use channels to capture results for verification outside the actor:

```rust
use tokio::sync::mpsc;

#[tokio::test]
async fn test_with_channel_verification() {
    let mut runtime = ActonApp::launch_async().await;

    // Create channel for test verification
    let (tx, mut rx) = mpsc::channel::<u32>(10);

    let mut actor = runtime.new_actor::<CounterState>();

    actor.mutate_on::<Increment>(|actor, ctx| {
        actor.model.count += ctx.message().0;
        let count = actor.model.count;
        let tx = tx.clone();

        Reply::pending(async move {
            tx.send(count).await.ok();
        })
    });

    let handle = actor.start().await;

    handle.send(Increment(5)).await;
    handle.send(Increment(3)).await;

    // Verify via channel
    assert_eq!(rx.recv().await, Some(5));
    assert_eq!(rx.recv().await, Some(8));

    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

---

## Testing Lifecycle Hooks

Verify hooks run in the correct order:

```rust
use tokio::sync::mpsc;

#[tokio::test]
async fn test_lifecycle_order() {
    let mut runtime = ActonApp::launch_async().await;
    let (tx, mut rx) = mpsc::channel::<&'static str>(10);

    let mut actor = runtime.new_actor::<TestState>();

    let tx1 = tx.clone();
    actor.before_start(|_| {
        let tx = tx1.clone();
        Reply::pending(async move {
            tx.send("before_start").await.ok();
        })
    });

    let tx2 = tx.clone();
    actor.after_start(|_| {
        let tx = tx2.clone();
        Reply::pending(async move {
            tx.send("after_start").await.ok();
        })
    });

    let tx3 = tx.clone();
    actor.before_stop(|_| {
        let tx = tx3.clone();
        Reply::pending(async move {
            tx.send("before_stop").await.ok();
        })
    });

    let tx4 = tx.clone();
    actor.after_stop(|_| {
        let tx = tx4.clone();
        Reply::pending(async move {
            tx.send("after_stop").await.ok();
        })
    });

    let _handle = actor.start().await;
    runtime.shutdown_all().await.unwrap();

    // Verify order
    assert_eq!(rx.recv().await, Some("before_start"));
    assert_eq!(rx.recv().await, Some("after_start"));
    assert_eq!(rx.recv().await, Some("before_stop"));
    assert_eq!(rx.recv().await, Some("after_stop"));
}
```

---

## Test Isolation

Each test should have its own runtime:

```rust
#[tokio::test]
async fn test_a() {
    let mut runtime = ActonApp::launch_async().await;
    // ... test
    runtime.shutdown_all().await.unwrap();
}

#[tokio::test]
async fn test_b() {
    let mut runtime = ActonApp::launch_async().await; // Fresh runtime
    // ... test
    runtime.shutdown_all().await.unwrap();
}
```

---

## Test Helpers

Create reusable test fixtures:

```rust
mod test_helpers {
    use super::*;

    pub async fn setup_test_runtime() -> ActorRuntime {
        ActonApp::launch_async().await
    }

    pub async fn create_echo_actor(runtime: &mut ActorRuntime) -> ActorHandle {
        let mut actor = runtime.new_actor::<EchoState>();
        actor.mutate_on::<Echo>(|_actor, ctx| {
            let reply = ctx.reply_envelope();
            let msg = ctx.message().clone();
            Reply::pending(async move {
                reply.send(msg).await;
            })
        });
        actor.start().await
    }
}
```

Usage:

```rust
#[tokio::test]
async fn test_using_helper() {
    let mut runtime = test_helpers::setup_test_runtime().await;
    let echo = test_helpers::create_echo_actor(&mut runtime).await;

    // ... test

    runtime.shutdown_all().await.unwrap();
}
```

---

## Deterministic Timing

Avoid flaky tests with explicit synchronization:

```rust
use tokio::sync::oneshot;

// Bad: Arbitrary sleep
tokio::time::sleep(Duration::from_millis(100)).await;

// Good: Use channels for synchronization
let (tx, rx) = oneshot::channel();
actor.after_stop(|_| {
    tx.send(()).ok();
    Reply::ready()
});
// ... start and use actor
rx.await.expect("Actor should have stopped");
```

---

## Next Steps

- [Testing Patterns](/docs/testing-patterns) - Testing handlers, errors, and pub/sub
- [Testing Integration](/docs/testing-integration) - Multi-actor and IPC tests
