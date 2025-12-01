---
title: Testing Patterns
nextjs:
  metadata:
    title: Testing Patterns - acton-reactive
    description: Patterns for testing message handlers, error handling, and pub/sub in acton-reactive.
---

This guide covers testing patterns for message handlers, error handling, reply behavior, and pub/sub messaging.

---

## Testing Reply Behavior

Test request-response patterns between actors:

```rust
#[acton_message]
struct Query(String);

#[acton_message]
struct QueryResponse(String);

#[tokio::test]
async fn test_request_response() {
    let mut runtime = ActonApp::launch_async().await;

    let (tx, mut rx) = mpsc::channel::<String>(1);

    // Service actor
    let mut service = runtime.new_actor::<ServiceState>();
    service.mutate_on::<Query>(|_actor, ctx| {
        let query = ctx.message().0.clone();
        let reply = ctx.reply_envelope();
        Reply::pending(async move {
            reply.send(QueryResponse(format!("Response to: {}", query))).await;
        })
    });
    let service_handle = service.start().await;

    // Client actor
    let mut client = runtime.new_actor::<ClientState>();
    client.mutate_on::<QueryResponse>(|_actor, ctx| {
        let response = ctx.message().0.clone();
        let tx = tx.clone();
        Reply::pending(async move {
            tx.send(response).await.ok();
        })
    });
    let client_handle = client.start().await;

    // Send query with reply-to set to client
    let envelope = service_handle.create_envelope(&client_handle.reply_address());
    envelope.send(Query("test".to_string())).await;

    // Verify response
    let response = rx.recv().await.expect("No response received");
    assert_eq!(response, "Response to: test");

    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

---

## Testing Error Handlers

Verify that fallible handlers and error handlers work correctly:

```rust
#[derive(Debug)]
struct TestError(String);
impl std::error::Error for TestError {}
impl std::fmt::Display for TestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[acton_message]
struct RiskyOperation { should_fail: bool }

#[tokio::test]
async fn test_error_handling() {
    let mut runtime = ActonApp::launch_async().await;

    let (tx, mut rx) = mpsc::channel::<String>(1);

    let mut actor = runtime.new_actor::<TestState>();

    actor
        .try_mutate_on::<RiskyOperation>(|_actor, ctx| {
            if ctx.message().should_fail {
                Reply::try_err(TestError("Intentional failure".to_string()))
            } else {
                Reply::try_ok(Success)
            }
        })
        .on_error::<RiskyOperation, TestError>(|_actor, _ctx, error| {
            let msg = error.0.clone();
            let tx = tx.clone();
            Reply::pending(async move {
                tx.send(msg).await.ok();
            })
        });

    let handle = actor.start().await;

    // Trigger error
    handle.send(RiskyOperation { should_fail: true }).await;

    // Verify error was handled
    let error_msg = rx.recv().await.expect("No error received");
    assert_eq!(error_msg, "Intentional failure");

    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

### Testing Success Path

```rust
#[tokio::test]
async fn test_fallible_success() {
    let mut runtime = ActonApp::launch_async().await;

    let (success_tx, mut success_rx) = mpsc::channel::<()>(1);
    let (error_tx, mut error_rx) = mpsc::channel::<()>(1);

    let mut actor = runtime.new_actor::<TestState>();

    actor
        .try_mutate_on::<RiskyOperation>(|_actor, ctx| {
            if ctx.message().should_fail {
                Reply::try_err(TestError("fail".into()))
            } else {
                let tx = success_tx.clone();
                Reply::try_pending(async move {
                    tx.send(()).await.ok();
                    Ok(())
                })
            }
        })
        .on_error::<RiskyOperation, TestError>(|_actor, _ctx, _error| {
            let tx = error_tx.clone();
            Reply::pending(async move {
                tx.send(()).await.ok();
            })
        });

    let handle = actor.start().await;

    // Trigger success
    handle.send(RiskyOperation { should_fail: false }).await;

    // Verify success path was taken
    assert!(success_rx.recv().await.is_some());

    // Verify error handler was NOT called
    let error_result = tokio::time::timeout(
        Duration::from_millis(50),
        error_rx.recv()
    ).await;
    assert!(error_result.is_err(), "Error handler should not have been called");

    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

---

## Testing Pub/Sub

### Testing Broadcast Delivery

```rust
#[acton_message]
struct DataUpdate { value: i32 }

#[tokio::test]
async fn test_broadcast_delivery() {
    let mut runtime = ActonApp::launch_async().await;
    let broker = runtime.broker();

    let (tx1, mut rx1) = mpsc::channel::<i32>(10);
    let (tx2, mut rx2) = mpsc::channel::<i32>(10);

    // Subscriber 1
    let mut sub1 = runtime.new_actor::<SubState>();
    sub1.mutate_on::<DataUpdate>(|_actor, ctx| {
        let value = ctx.message().value;
        let tx = tx1.clone();
        Reply::pending(async move {
            tx.send(value).await.ok();
        })
    });
    sub1.handle().subscribe::<DataUpdate>().await;
    let _h1 = sub1.start().await;

    // Subscriber 2
    let mut sub2 = runtime.new_actor::<SubState>();
    sub2.mutate_on::<DataUpdate>(|_actor, ctx| {
        let value = ctx.message().value;
        let tx = tx2.clone();
        Reply::pending(async move {
            tx.send(value).await.ok();
        })
    });
    sub2.handle().subscribe::<DataUpdate>().await;
    let _h2 = sub2.start().await;

    // Broadcast
    broker.broadcast(DataUpdate { value: 42 }).await;

    // Both should receive
    assert_eq!(rx1.recv().await, Some(42));
    assert_eq!(rx2.recv().await, Some(42));

    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

### Testing Unsubscribe

```rust
#[tokio::test]
async fn test_unsubscribe() {
    let mut runtime = ActonApp::launch_async().await;
    let broker = runtime.broker();

    let (tx, mut rx) = mpsc::channel::<i32>(10);

    let mut subscriber = runtime.new_actor::<SubState>();
    subscriber.mutate_on::<DataUpdate>(|_actor, ctx| {
        let value = ctx.message().value;
        let tx = tx.clone();
        Reply::pending(async move {
            tx.send(value).await.ok();
        })
    });

    let sub_handle = subscriber.handle().clone();
    sub_handle.subscribe::<DataUpdate>().await;
    let _h = subscriber.start().await;

    // Should receive this
    broker.broadcast(DataUpdate { value: 1 }).await;
    assert_eq!(rx.recv().await, Some(1));

    // Unsubscribe
    sub_handle.unsubscribe::<DataUpdate>();

    // Should NOT receive this
    broker.broadcast(DataUpdate { value: 2 }).await;

    // Use timeout to verify no message
    let result = tokio::time::timeout(
        Duration::from_millis(100),
        rx.recv()
    ).await;
    assert!(result.is_err(), "Should not have received message");

    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

---

## Testing Handler Selection

Verify correct handler is invoked for each message type:

```rust
#[acton_message]
struct MessageA;

#[acton_message]
struct MessageB;

#[tokio::test]
async fn test_handler_selection() {
    let mut runtime = ActonApp::launch_async().await;

    let (tx, mut rx) = mpsc::channel::<&'static str>(10);

    let mut actor = runtime.new_actor::<TestState>();

    let tx1 = tx.clone();
    actor.mutate_on::<MessageA>(|_actor, _ctx| {
        let tx = tx1.clone();
        Reply::pending(async move {
            tx.send("A").await.ok();
        })
    });

    let tx2 = tx.clone();
    actor.mutate_on::<MessageB>(|_actor, _ctx| {
        let tx = tx2.clone();
        Reply::pending(async move {
            tx.send("B").await.ok();
        })
    });

    let handle = actor.start().await;

    handle.send(MessageB).await;
    handle.send(MessageA).await;
    handle.send(MessageB).await;

    assert_eq!(rx.recv().await, Some("B"));
    assert_eq!(rx.recv().await, Some("A"));
    assert_eq!(rx.recv().await, Some("B"));

    runtime.shutdown_all().await.unwrap();
}
```

---

## Testing act_on vs mutate_on

Verify concurrent vs sequential execution:

```rust
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

#[acton_message]
struct ReadQuery;

#[tokio::test]
async fn test_concurrent_reads() {
    let mut runtime = ActonApp::launch_async().await;

    let concurrent_count = Arc::new(AtomicU32::new(0));
    let max_concurrent = Arc::new(AtomicU32::new(0));

    let mut actor = runtime.new_actor::<TestState>();

    let cc = concurrent_count.clone();
    let mc = max_concurrent.clone();

    // act_on handlers should run concurrently
    actor.act_on::<ReadQuery>(|_actor, _ctx| {
        let cc = cc.clone();
        let mc = mc.clone();

        Reply::pending(async move {
            // Increment concurrent counter
            let current = cc.fetch_add(1, Ordering::SeqCst) + 1;

            // Track max concurrent
            mc.fetch_max(current, Ordering::SeqCst);

            // Simulate work
            tokio::time::sleep(Duration::from_millis(50)).await;

            // Decrement
            cc.fetch_sub(1, Ordering::SeqCst);
        })
    });

    let handle = actor.start().await;

    // Send many concurrent reads
    for _ in 0..10 {
        handle.send(ReadQuery).await;
    }

    // Wait for completion
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Should have seen concurrent execution
    assert!(max_concurrent.load(Ordering::SeqCst) > 1,
        "act_on should allow concurrent execution");

    runtime.shutdown_all().await.unwrap();
}
```

---

## Testing Edge Cases

### Empty Messages

```rust
#[tokio::test]
async fn test_empty_payload() {
    let mut runtime = ActonApp::launch_async().await;
    let (tx, mut rx) = mpsc::channel::<bool>(1);

    let mut actor = runtime.new_actor::<TestState>();
    actor.mutate_on::<EmptyMessage>(|_actor, _ctx| {
        let tx = tx.clone();
        Reply::pending(async move {
            tx.send(true).await.ok();
        })
    });

    let handle = actor.start().await;
    handle.send(EmptyMessage).await;

    assert_eq!(rx.recv().await, Some(true));
    runtime.shutdown_all().await.unwrap();
}
```

### Rapid Message Sending

```rust
#[tokio::test]
async fn test_rapid_messages() {
    let mut runtime = ActonApp::launch_async().await;
    let (tx, mut rx) = mpsc::channel::<u32>(1000);

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

    // Send 100 messages rapidly
    for i in 1..=100 {
        handle.send(Increment(1)).await;
    }

    // Collect all results
    let mut last = 0;
    for _ in 1..=100 {
        last = rx.recv().await.unwrap();
    }

    assert_eq!(last, 100);
    runtime.shutdown_all().await.unwrap();
}
```

---

## Next Steps

- [Testing Basics](/docs/testing-basics) - Test setup and fundamentals
- [Testing Integration](/docs/testing-integration) - Multi-actor and IPC tests
