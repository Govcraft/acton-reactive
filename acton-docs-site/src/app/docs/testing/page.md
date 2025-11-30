---
title: Testing
nextjs:
  metadata:
    title: Testing - acton-reactive
    description: Strategies and patterns for testing acton-reactive applications.
---

This guide covers strategies and patterns for testing `acton-reactive` applications.

---

## Test Setup

### Dependencies

Add test dependencies to your `Cargo.toml`:

```toml
[dev-dependencies]
tokio = { version = "1", features = ["full", "test-util"] }
acton-test = "0.1"  # Optional: Test utilities
```

### Basic Test Structure

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use acton_reactive::prelude::*;

    #[tokio::test]
    async fn test_actor_behavior() {
        // Setup
        let mut runtime = ActonApp::launch();

        // Create actor
        let mut actor = runtime.new_actor::<TestState>();
        // ... configure

        let handle = actor.start().await;

        // Test
        handle.send(TestMessage).await;

        // Verify (via lifecycle hooks or state inspection)

        // Cleanup
        runtime.shutdown_all().await.expect("Shutdown failed");
    }
}
```

---

## Unit Testing Actors

### Testing State Initialization

```rust
#[derive(Default, Clone, Debug)]
struct CounterState {
    count: u32,
}

#[tokio::test]
async fn test_initial_state() {
    let mut runtime = ActonApp::launch();

    let mut counter = runtime.new_actor::<CounterState>();

    // Verify initial state via after_stop hook
    counter.after_stop(|actor| {
        assert_eq!(actor.model.count, 0);
        ActorReply::immediate()
    });

    let _handle = counter.start().await;
    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

### Testing State Mutations

```rust
#[derive(Clone, Debug)]
struct Increment(u32);

#[tokio::test]
async fn test_increment_handler() {
    let mut runtime = ActonApp::launch();

    let mut counter = runtime.new_actor::<CounterState>();

    counter
        .mutate_on::<Increment>(|actor, ctx| {
            actor.model.count += ctx.message().0;
            ActorReply::immediate()
        })
        .after_stop(|actor| {
            // Verify final state
            assert_eq!(actor.model.count, 15);
            ActorReply::immediate()
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

### Testing with Channels for Verification

Use channels to capture results for verification:

```rust
use tokio::sync::mpsc;

#[tokio::test]
async fn test_with_channel_verification() {
    let mut runtime = ActonApp::launch();

    // Create channel for test verification
    let (tx, mut rx) = mpsc::channel::<u32>(10);

    let mut actor = runtime.new_actor::<CounterState>();

    actor.mutate_on::<Increment>(|actor, ctx| {
        actor.model.count += ctx.message().0;
        let count = actor.model.count;
        let tx = tx.clone();

        Box::pin(async move {
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

## Integration Testing

### Multi-Actor Tests

```rust
#[tokio::test]
async fn test_actor_communication() {
    let mut runtime = ActonApp::launch();

    let (tx, mut rx) = mpsc::channel::<String>(10);

    // Actor A sends to Actor B
    let mut actor_a = runtime.new_actor::<StateA>();
    let mut actor_b = runtime.new_actor::<StateB>();

    actor_b.mutate_on::<Greeting>(|_actor, ctx| {
        let greeting = ctx.message().0.clone();
        let tx = tx.clone();
        Box::pin(async move {
            tx.send(greeting).await.ok();
        })
    });

    let handle_b = actor_b.start().await;
    let handle_b_clone = handle_b.clone();

    actor_a.mutate_on::<SendGreeting>(|_actor, ctx| {
        let target = handle_b_clone.clone();
        let message = ctx.message().0.clone();
        Box::pin(async move {
            target.send(Greeting(message)).await;
        })
    });

    let handle_a = actor_a.start().await;

    // Test communication
    handle_a.send(SendGreeting("Hello".to_string())).await;

    assert_eq!(rx.recv().await, Some("Hello".to_string()));

    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

### Testing Supervision

```rust
#[tokio::test]
async fn test_supervision_hierarchy() {
    let mut runtime = ActonApp::launch();

    let mut parent = runtime.new_actor::<ParentState>();
    let parent_handle = parent.start().await;

    // Create and supervise child
    let mut child = runtime.new_actor::<ChildState>();
    let child_handle = parent_handle.supervise(child).await.expect("Supervise failed");

    // Verify child is registered
    let found = parent_handle.find_child(&child_handle.id());
    assert!(found.is_some());

    // Stop child
    child_handle.stop().await.expect("Stop failed");

    // Small delay for cleanup
    tokio::time::sleep(Duration::from_millis(50)).await;

    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

---

## Testing Message Handlers

### Testing Reply Behavior

```rust
#[derive(Clone, Debug)]
struct Query(String);

#[derive(Clone, Debug)]
struct QueryResponse(String);

#[tokio::test]
async fn test_request_response() {
    let mut runtime = ActonApp::launch();

    let (tx, mut rx) = mpsc::channel::<String>(1);

    // Service actor
    let mut service = runtime.new_actor::<ServiceState>();
    service.mutate_on::<Query>(|actor, ctx| {
        let query = ctx.message().0.clone();
        let reply = ctx.reply_envelope();
        Box::pin(async move {
            reply.send(QueryResponse(format!("Response to: {}", query))).await;
        })
    });
    let service_handle = service.start().await;

    // Client actor
    let mut client = runtime.new_actor::<ClientState>();
    client.mutate_on::<QueryResponse>(|_actor, ctx| {
        let response = ctx.message().0.clone();
        let tx = tx.clone();
        Box::pin(async move {
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

### Testing Error Handlers

```rust
#[derive(Debug)]
struct TestError(String);
impl std::error::Error for TestError {}
impl std::fmt::Display for TestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[tokio::test]
async fn test_error_handling() {
    let mut runtime = ActonApp::launch();

    let (tx, mut rx) = mpsc::channel::<String>(1);

    let mut actor = runtime.new_actor::<TestState>();

    actor
        .mutate_on_fallible::<RiskyOperation>(|_actor, ctx| {
            Box::pin(async move {
                if ctx.message().should_fail {
                    Err(Box::new(TestError("Intentional failure".to_string()))
                        as Box<dyn std::error::Error>)
                } else {
                    Ok(Box::new(Success) as Box<dyn ActonMessageReply>)
                }
            })
        })
        .on_error::<RiskyOperation, TestError>(|_actor, _ctx, error| {
            let msg = error.0.clone();
            let tx = tx.clone();
            Box::pin(async move {
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

---

## Testing Pub/Sub

### Testing Broadcasts

```rust
#[tokio::test]
async fn test_broadcast_delivery() {
    let mut runtime = ActonApp::launch();
    let broker = runtime.broker();

    let (tx1, mut rx1) = mpsc::channel::<i32>(10);
    let (tx2, mut rx2) = mpsc::channel::<i32>(10);

    // Subscriber 1
    let mut sub1 = runtime.new_actor::<SubState>();
    sub1.mutate_on::<DataUpdate>(|_actor, ctx| {
        let value = ctx.message().value;
        let tx = tx1.clone();
        Box::pin(async move {
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
        Box::pin(async move {
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
    let mut runtime = ActonApp::launch();
    let broker = runtime.broker();

    let (tx, mut rx) = mpsc::channel::<i32>(10);

    let mut subscriber = runtime.new_actor::<SubState>();
    subscriber.mutate_on::<DataUpdate>(|_actor, ctx| {
        let value = ctx.message().value;
        let tx = tx.clone();
        Box::pin(async move {
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

## Testing IPC

### Testing Type Registration

```rust
#[cfg(feature = "ipc")]
#[tokio::test]
async fn test_type_registration() {
    let mut runtime = ActonApp::launch();
    let registry = runtime.ipc_registry();

    // Register types
    registry.register::<TestRequest>("TestRequest");
    registry.register::<TestResponse>("TestResponse");

    // Verify registration
    assert!(registry.is_registered("TestRequest"));
    assert!(registry.is_registered("TestResponse"));
    assert!(!registry.is_registered("UnknownType"));

    // List registered types
    let types = registry.registered_types();
    assert!(types.contains(&"TestRequest".to_string()));

    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

### Testing Actor Exposure

```rust
#[cfg(feature = "ipc")]
#[tokio::test]
async fn test_ipc_exposure() {
    let mut runtime = ActonApp::launch();

    let actor = runtime.new_actor::<TestState>().start().await;

    // Expose actor
    runtime.ipc_expose("test_service", actor.clone());

    // Verify exposure (implementation-specific)
    // This depends on how your IPC module exposes lookup

    // Hide actor
    runtime.ipc_hide("test_service");

    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

### Integration Test with IPC

```rust
#[cfg(feature = "ipc")]
#[tokio::test]
async fn test_ipc_round_trip() {
    use tokio::net::UnixStream;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut runtime = ActonApp::launch();
    let registry = runtime.ipc_registry();

    // Setup
    registry.register::<EchoRequest>("EchoRequest");
    registry.register::<EchoResponse>("EchoResponse");

    let mut actor = runtime.new_actor::<EchoState>();
    actor.mutate_on::<EchoRequest>(|_actor, ctx| {
        let msg = ctx.message().message.clone();
        let reply = ctx.reply_envelope();
        Box::pin(async move {
            reply.send(EchoResponse { message: msg }).await;
        })
    });

    let handle = actor.start().await;
    runtime.ipc_expose("echo", handle);

    let listener = runtime.start_ipc_listener().await.expect("Failed to start IPC");

    // Give listener time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Connect as client
    let mut stream = UnixStream::connect("/tmp/acton.sock")
        .await
        .expect("Failed to connect");

    // Send request
    let envelope = serde_json::json!({
        "target": "echo",
        "message_type": "EchoRequest",
        "payload": { "message": "Hello" },
        "expects_reply": true
    });
    let data = serde_json::to_vec(&envelope).unwrap();
    let length = (data.len() as u32).to_be_bytes();
    stream.write_all(&length).await.unwrap();
    stream.write_all(&data).await.unwrap();

    // Read response
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut response_buf = vec![0u8; len];
    stream.read_exact(&mut response_buf).await.unwrap();

    let response: serde_json::Value = serde_json::from_slice(&response_buf).unwrap();
    assert!(response["success"].as_bool().unwrap());
    assert_eq!(response["payload"]["message"], "Hello");

    // Cleanup
    listener.shutdown().await.expect("Shutdown failed");
    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

---

## Best Practices

### 1. Use Deterministic Timing

Avoid flaky tests with explicit synchronization:

```rust
// Bad: Arbitrary sleep
tokio::time::sleep(Duration::from_millis(100)).await;

// Good: Use channels for synchronization
let (tx, rx) = oneshot::channel();
actor.after_stop(|_| {
    tx.send(()).ok();
    ActorReply::immediate()
});
// ... start and use actor
rx.await.expect("Actor should have stopped");
```

### 2. Isolate Tests

Each test should have its own runtime:

```rust
#[tokio::test]
async fn test_a() {
    let mut runtime = ActonApp::launch();
    // ... test
    runtime.shutdown_all().await.unwrap();
}

#[tokio::test]
async fn test_b() {
    let mut runtime = ActonApp::launch(); // Fresh runtime
    // ... test
    runtime.shutdown_all().await.unwrap();
}
```

### 3. Test Edge Cases

```rust
#[tokio::test]
async fn test_empty_message() {
    // Test with empty/default values
}

#[tokio::test]
async fn test_large_payload() {
    // Test with large data
}

#[tokio::test]
async fn test_rapid_messages() {
    // Test high-frequency message sending
}

#[tokio::test]
async fn test_concurrent_access() {
    // Test concurrent message handling
}
```

### 4. Clean Up IPC Sockets

```rust
#[cfg(feature = "ipc")]
fn cleanup_socket() {
    let socket_path = std::path::Path::new("/tmp/acton.sock");
    if socket_path.exists() {
        std::fs::remove_file(socket_path).ok();
    }
}

#[cfg(feature = "ipc")]
#[tokio::test]
async fn test_ipc_feature() {
    cleanup_socket();
    // ... test
    cleanup_socket();
}
```

### 5. Use Test Fixtures

Create reusable test helpers:

```rust
mod test_helpers {
    use super::*;

    pub async fn setup_test_runtime() -> ActorRuntime {
        ActonApp::launch()
    }

    pub async fn create_echo_actor(runtime: &mut ActorRuntime) -> ActorHandle {
        let mut actor = runtime.new_actor::<EchoState>();
        actor.mutate_on::<Echo>(|_actor, ctx| {
            let reply = ctx.reply_envelope();
            let msg = ctx.message().clone();
            Box::pin(async move {
                reply.send(msg).await;
            })
        });
        actor.start().await
    }
}
```

### 6. Test Lifecycle Hooks

```rust
#[tokio::test]
async fn test_lifecycle_order() {
    let mut runtime = ActonApp::launch();
    let (tx, mut rx) = mpsc::channel::<&'static str>(10);

    let mut actor = runtime.new_actor::<TestState>();

    let tx1 = tx.clone();
    actor.before_start(|_| {
        let tx = tx1.clone();
        Box::pin(async move {
            tx.send("before_start").await.ok();
        })
    });

    let tx2 = tx.clone();
    actor.after_start(|_| {
        let tx = tx2.clone();
        Box::pin(async move {
            tx.send("after_start").await.ok();
        })
    });

    let tx3 = tx.clone();
    actor.before_stop(|_| {
        let tx = tx3.clone();
        Box::pin(async move {
            tx.send("before_stop").await.ok();
        })
    });

    let tx4 = tx.clone();
    actor.after_stop(|_| {
        let tx = tx4.clone();
        Box::pin(async move {
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
