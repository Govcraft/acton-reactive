---
title: Testing Actors
description: Strategies for testing actor-based code.
---

Actors are inherently testable. Their message-based interface makes it clear what inputs you can send and what behaviors to verify.

## Basic Test Setup

Each test creates its own runtime:

```rust
#[tokio::test]
async fn test_counter_increments() {
    let mut runtime = ActonApp::launch_async().await;
    let mut counter = runtime.new_actor::<Counter>();

    counter
        .mutate_on::<Increment>(handle_increment)
        .act_on::<PrintCount>(handle_print);

    let handle = counter.start().await;

    handle.send(Increment).await;
    handle.send(Increment).await;
    handle.send(PrintCount).await;

    // Give time for async processing
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    runtime.shutdown_all().await.ok();
}
```

---

## Testing with Response Actors

Create a "probe" actor to receive and verify responses:

```rust
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

#[acton_actor]
struct Probe {
    received_count: Arc<AtomicI32>,
}

#[acton_message]
struct CountResponse(i32);

#[tokio::test]
async fn test_get_count() {
    let mut runtime = ActonApp::launch_async().await;

    // Create the actor under test
    let mut counter = runtime.new_actor::<Counter>();
    counter
        .mutate_on::<Increment>(|actor, _env| {
            actor.model.count += 1;
            Reply::ready()
        })
        .act_on::<GetCount>(|actor, env| {
            let count = actor.model.count;
            let reply = env.reply_envelope();
            Reply::pending(async move {
                reply.send(CountResponse(count)).await;
            })
        });

    let counter_handle = counter.start().await;

    // Create probe to receive response
    let received = Arc::new(AtomicI32::new(-1));
    let received_clone = received.clone();

    let mut probe = runtime.new_actor::<Probe>();
    probe.model.received_count = received_clone.clone();

    probe.mutate_on::<CountResponse>(|actor, env| {
        let count = env.message().0;
        actor.model.received_count.store(count, Ordering::SeqCst);
        Reply::ready()
    });

    let probe_handle = probe.start().await;

    // Increment counter
    counter_handle.send(Increment).await;
    counter_handle.send(Increment).await;

    // Query count with probe as recipient
    let query = probe_handle.create_envelope(
        Some(counter_handle.reply_address())
    );
    query.send(GetCount).await;

    // Wait for response
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    assert_eq!(received.load(Ordering::SeqCst), 2);

    runtime.shutdown_all().await.ok();
}
```

---

## Testing Multiple Actors

Create multiple actors and test their interactions:

```rust
#[tokio::test]
async fn test_producer_consumer() {
    let mut runtime = ActonApp::launch_async().await;

    let received = Arc::new(AtomicUsize::new(0));
    let received_clone = received.clone();

    // Consumer tracks received items
    let mut consumer = runtime.new_actor::<Consumer>();
    consumer.model.received = received_clone;
    consumer.mutate_on::<Item>(|actor, _env| {
        actor.model.received.fetch_add(1, Ordering::SeqCst);
        Reply::ready()
    });

    let consumer_handle = consumer.start().await;

    // Producer sends to consumer
    let mut producer = runtime.new_actor::<Producer>();
    producer.model.consumer = Some(consumer_handle.clone());
    producer.mutate_on::<Produce>(|actor, env| {
        let count = env.message().count;
        let consumer = actor.model.consumer.clone().unwrap();

        Reply::pending(async move {
            for _ in 0..count {
                consumer.send(Item).await;
            }
        })
    });

    let producer_handle = producer.start().await;

    producer_handle.send(Produce { count: 5 }).await;

    // Wait for async processing
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    assert_eq!(received.load(Ordering::SeqCst), 5);

    runtime.shutdown_all().await.ok();
}
```

---

## Test Helpers

Create helper functions for common setup:

```rust
async fn setup_counter(runtime: &mut ActorRuntime) -> ActorHandle {
    let mut counter = runtime.new_actor::<Counter>();
    counter
        .mutate_on::<Increment>(handle_increment)
        .act_on::<GetCount>(handle_get);
    counter.start().await
}

#[tokio::test]
async fn test_with_helper() {
    let mut runtime = ActonApp::launch_async().await;
    let counter = setup_counter(&mut runtime).await;

    counter.send(Increment).await;
    // Test logic here

    runtime.shutdown_all().await.ok();
}
```

---

## Testing Pub/Sub

Test broker-based messaging:

```rust
#[tokio::test]
async fn test_broadcast() {
    let mut runtime = ActonApp::launch_async().await;
    let broker = runtime.broker();

    let received = Arc::new(AtomicUsize::new(0));

    // Create subscribers
    for _ in 0..3 {
        let received_clone = received.clone();
        let mut subscriber = runtime.new_actor::<Subscriber>();
        subscriber.model.counter = received_clone;

        subscriber.mutate_on::<Event>(|actor, _env| {
            actor.model.counter.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        });

        // Subscribe before starting
        subscriber.handle().subscribe::<Event>().await;
        subscriber.start().await;
    }

    // Broadcast event
    broker.broadcast(Event).await;

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // All 3 subscribers should have received the event
    assert_eq!(received.load(Ordering::SeqCst), 3);

    runtime.shutdown_all().await.ok();
}
```

---

## Avoiding Flaky Tests

### Use atomic counters for verification

```rust
// GOOD: Use atomics for cross-actor state verification
let count = Arc::new(AtomicI32::new(0));
// ... share with probe actor ...
assert_eq!(count.load(Ordering::SeqCst), expected);
```

### Allow time for async processing

```rust
// Give messages time to be processed
tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
```

### Isolate each test

```rust
// Each test gets its own runtime
#[tokio::test]
async fn test_one() {
    let mut runtime = ActonApp::launch_async().await;  // Isolated
    // ...
    runtime.shutdown_all().await.ok();
}

#[tokio::test]
async fn test_two() {
    let mut runtime = ActonApp::launch_async().await;  // Isolated
    // ...
    runtime.shutdown_all().await.ok();
}
```

---

## Summary

- Use `#[tokio::test]` for async tests
- Each test creates its own `ActorRuntime`
- Use probe actors with atomic counters to verify responses
- Create helper functions for common setup
- Allow time for async message processing
- Clean up with `shutdown_all()`

---

## Continue Learning

You've covered the Building Apps section:
- Parent-child actors
- Request-response patterns
- Error handling
- Testing strategies

Continue to [Advanced](/docs/advanced/ipc) for topics like IPC and performance.
