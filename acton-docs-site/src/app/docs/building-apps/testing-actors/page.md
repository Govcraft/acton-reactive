---
title: Testing Actors
description: Strategies for testing actor-based code.
---

Actors are inherently testable. Their message-based interface makes it clear what inputs you can send and what outputs to expect.

## Basic Test Setup

Each test creates its own `ActonApp`:

```rust
#[tokio::test]
async fn test_counter_increments() {
    let mut app = ActonApp::launch();
    let mut builder = app.new_actor::<Counter>();

    builder
        .mutate_on::<Increment>(handle_increment)
        .act_on::<GetCount>(handle_get);

    let counter = builder.start().await;

    counter.send(Increment).await;
    counter.send(Increment).await;

    let count: i32 = counter.ask(GetCount).await;
    assert_eq!(count, 2);

    app.shutdown_all().await.ok();
}
```

---

## Testing State Changes

Send messages, then query state:

```rust
#[tokio::test]
async fn test_add_item() {
    let mut app = ActonApp::launch();
    let store = setup_store(&mut app).await;

    store.send(AddItem { name: "widget".into() }).await;

    let items: Vec<String> = store.ask(GetItems).await;
    assert_eq!(items, vec!["widget"]);

    app.shutdown_all().await.ok();
}
```

---

## Testing Multiple Actors

Create multiple actors and test their interactions:

```rust
#[tokio::test]
async fn test_producer_consumer() {
    let mut app = ActonApp::launch();

    let consumer = setup_consumer(&mut app).await;
    let producer = setup_producer(&mut app, consumer.clone()).await;

    producer.send(Produce { count: 5 }).await;

    // Give time for async processing
    tokio::time::sleep(Duration::from_millis(100)).await;

    let received: usize = consumer.ask(GetReceived).await;
    assert_eq!(received, 5);

    app.shutdown_all().await.ok();
}
```

---

## Testing Error Conditions

```rust
#[tokio::test]
async fn test_insufficient_funds() {
    let mut app = ActonApp::launch();
    let account = setup_account(&mut app, 50).await;  // Starting balance: 50

    let result: Result<u64, &str> = account.ask(Withdraw { amount: 100 }).await;

    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "Insufficient funds");

    app.shutdown_all().await.ok();
}
```

---

## Test Helpers

Create helper functions for common setup:

```rust
async fn setup_counter(app: &mut ActonApp) -> AgentHandle {
    let mut builder = app.new_actor::<Counter>();
    builder
        .mutate_on::<Increment>(handle_increment)
        .act_on::<GetCount>(handle_get);
    builder.start().await
}

#[tokio::test]
async fn test_with_helper() {
    let mut app = ActonApp::launch();
    let counter = setup_counter(&mut app).await;

    // Test logic here

    app.shutdown_all().await.ok();
}
```

---

## Avoiding Flaky Tests

### Don't rely on timing

```rust
// BAD: Flaky
handle.send(Start).await;
tokio::time::sleep(Duration::from_millis(100)).await;
let result = check_result();

// GOOD: Use ask for synchronization
handle.send(Start).await;
let result: Result = handle.ask(GetResult).await;
```

### Isolate each test

```rust
// Each test gets its own ActonApp
#[tokio::test]
async fn test_one() {
    let mut app = ActonApp::launch();  // Isolated
    // ...
    app.shutdown_all().await.ok();
}

#[tokio::test]
async fn test_two() {
    let mut app = ActonApp::launch();  // Isolated
    // ...
    app.shutdown_all().await.ok();
}
```

---

## Summary

- Use `#[tokio::test]` for async tests
- Each test creates its own `ActonApp`
- Use `ask` for synchronization instead of sleep
- Create helper functions for common setup
- Clean up with `shutdown_all()`

---

## Continue Learning

You've covered the Building Apps section:
- Parent-child actors
- Request-response patterns
- Error handling
- Testing strategies

Continue to [Advanced](/docs/advanced/ipc) for topics like IPC and performance.
