---
title: Testing Integration
nextjs:
  metadata:
    title: Testing Integration - acton-reactive
    description: Integration testing for multi-actor systems and IPC in acton-reactive.
---

This guide covers integration testing patterns for multi-actor systems, supervision hierarchies, and IPC functionality.

---

## Multi-Actor Tests

Test communication between multiple actors:

```rust
#[acton_message]
struct Greeting(String);

#[acton_message]
struct SendGreeting(String);

#[tokio::test]
async fn test_actor_communication() {
    let mut runtime = ActonApp::launch_async().await;

    let (tx, mut rx) = mpsc::channel::<String>(10);

    // Actor A sends to Actor B
    let mut actor_a = runtime.new_actor::<StateA>();
    let mut actor_b = runtime.new_actor::<StateB>();

    actor_b.mutate_on::<Greeting>(|_actor, ctx| {
        let greeting = ctx.message().0.clone();
        let tx = tx.clone();
        Reply::pending(async move {
            tx.send(greeting).await.ok();
        })
    });

    let handle_b = actor_b.start().await;
    let handle_b_clone = handle_b.clone();

    actor_a.mutate_on::<SendGreeting>(|_actor, ctx| {
        let target = handle_b_clone.clone();
        let message = ctx.message().0.clone();
        Reply::pending(async move {
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

---

## Testing Supervision

### Parent-Child Relationships

```rust
#[tokio::test]
async fn test_supervision_hierarchy() {
    let mut runtime = ActonApp::launch_async().await;

    let mut parent = runtime.new_actor::<ParentState>();
    let parent_handle = parent.start().await;

    // Create and supervise child
    let child = runtime.new_actor::<ChildState>();
    let child_handle = parent_handle.supervise(child)
        .await
        .expect("Supervise failed");

    // Verify child is registered
    let found = parent_handle.find_child(&child_handle.id());
    assert!(found.is_some());

    // Stop child explicitly
    child_handle.stop().await.expect("Stop failed");

    // Small delay for cleanup
    tokio::time::sleep(Duration::from_millis(50)).await;

    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

### Cascading Shutdown

```rust
#[tokio::test]
async fn test_cascading_shutdown() {
    let mut runtime = ActonApp::launch_async().await;

    let (tx, mut rx) = mpsc::channel::<String>(10);

    // Parent
    let mut parent = runtime.new_actor::<ParentState>();
    let tx1 = tx.clone();
    parent.after_stop(|_| {
        let tx = tx1.clone();
        Reply::pending(async move {
            tx.send("parent stopped".into()).await.ok();
        })
    });
    let parent_handle = parent.start().await;

    // Child
    let mut child = runtime.new_actor::<ChildState>();
    let tx2 = tx.clone();
    child.after_stop(|_| {
        let tx = tx2.clone();
        Reply::pending(async move {
            tx.send("child stopped".into()).await.ok();
        })
    });
    let _child_handle = parent_handle.supervise(child)
        .await
        .expect("Supervise failed");

    // Stop parent - should cascade to child
    parent_handle.stop().await.expect("Stop failed");

    // Child should stop first, then parent
    let first = rx.recv().await.unwrap();
    let second = rx.recv().await.unwrap();

    assert_eq!(first, "child stopped");
    assert_eq!(second, "parent stopped");

    runtime.shutdown_all().await.unwrap();
}
```

---

## Testing IPC

### Type Registration

```rust
#[cfg(feature = "ipc")]
#[tokio::test]
async fn test_type_registration() {
    let mut runtime = ActonApp::launch_async().await;
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

### Actor Exposure

```rust
#[cfg(feature = "ipc")]
#[tokio::test]
async fn test_ipc_exposure() {
    let mut runtime = ActonApp::launch_async().await;

    let actor = runtime.new_actor::<TestState>().start().await;

    // Expose actor
    runtime.ipc_expose("test_service", actor.clone());

    // Verify exposure (implementation-specific)

    // Hide actor
    runtime.ipc_hide("test_service");

    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

### Full IPC Round Trip

```rust
#[cfg(feature = "ipc")]
#[tokio::test]
async fn test_ipc_round_trip() {
    use tokio::net::UnixStream;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let socket_path = "/tmp/acton_test.sock";

    // Clean up any existing socket
    let _ = std::fs::remove_file(socket_path);

    let mut runtime = ActonApp::launch_async().await;
    let registry = runtime.ipc_registry();

    // Setup
    registry.register::<EchoRequest>("EchoRequest");
    registry.register::<EchoResponse>("EchoResponse");

    let mut actor = runtime.new_actor::<EchoState>();
    actor.mutate_on::<EchoRequest>(|_actor, ctx| {
        let msg = ctx.message().message.clone();
        let reply = ctx.reply_envelope();
        Reply::pending(async move {
            reply.send(EchoResponse { message: msg }).await;
        })
    });

    let handle = actor.start().await;
    runtime.ipc_expose("echo", handle);

    let config = IpcConfig {
        socket_path: socket_path.into(),
        ..Default::default()
    };
    let listener = runtime.start_ipc_listener_with_config(config)
        .await
        .expect("Failed to start IPC");

    // Give listener time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Connect as client
    let mut stream = UnixStream::connect(socket_path)
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
    let _ = std::fs::remove_file(socket_path);
}
```

---

## Best Practices

### 1. Clean Up IPC Sockets

```rust
#[cfg(feature = "ipc")]
fn cleanup_socket(path: &str) {
    let socket_path = std::path::Path::new(path);
    if socket_path.exists() {
        std::fs::remove_file(socket_path).ok();
    }
}

#[cfg(feature = "ipc")]
#[tokio::test]
async fn test_ipc_feature() {
    let socket = "/tmp/test_specific.sock";
    cleanup_socket(socket);

    // ... test

    cleanup_socket(socket);
}
```

### 2. Use Unique Socket Paths

```rust
#[cfg(feature = "ipc")]
fn unique_socket_path() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("/tmp/acton_test_{}.sock", timestamp)
}
```

### 3. Test Timeouts

```rust
#[tokio::test]
async fn test_with_timeout() {
    let mut runtime = ActonApp::launch_async().await;

    // ... setup

    // Test with timeout to prevent hanging
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        async {
            // ... test operations
            Ok::<_, ()>(())
        }
    ).await;

    assert!(result.is_ok(), "Test timed out");

    runtime.shutdown_all().await.unwrap();
}
```

### 4. Test Concurrent Access

```rust
#[tokio::test]
async fn test_concurrent_access() {
    let mut runtime = ActonApp::launch_async().await;
    let (tx, mut rx) = mpsc::channel::<u32>(100);

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

    // Spawn multiple concurrent senders
    let mut handles = vec![];
    for _ in 0..10 {
        let h = handle.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..10 {
                h.send(Increment(1)).await;
            }
        }));
    }

    // Wait for all senders
    for h in handles {
        h.await.unwrap();
    }

    // Collect all results
    let mut values = vec![];
    for _ in 0..100 {
        values.push(rx.recv().await.unwrap());
    }

    // Final value should be 100
    assert_eq!(*values.last().unwrap(), 100);

    runtime.shutdown_all().await.unwrap();
}
```

### 5. Test Large Payloads

```rust
#[tokio::test]
async fn test_large_payload() {
    let mut runtime = ActonApp::launch_async().await;
    let (tx, mut rx) = mpsc::channel::<usize>(1);

    let mut actor = runtime.new_actor::<TestState>();
    actor.mutate_on::<LargeMessage>(|_actor, ctx| {
        let len = ctx.message().data.len();
        let tx = tx.clone();
        Reply::pending(async move {
            tx.send(len).await.ok();
        })
    });

    let handle = actor.start().await;

    // Send large payload
    let large_data = vec![0u8; 1024 * 1024]; // 1MB
    handle.send(LargeMessage { data: large_data }).await;

    let received_len = rx.recv().await.unwrap();
    assert_eq!(received_len, 1024 * 1024);

    runtime.shutdown_all().await.unwrap();
}
```

---

## Next Steps

- [Testing Basics](/docs/testing-basics) - Test setup and fundamentals
- [Testing Patterns](/docs/testing-patterns) - Testing handlers and pub/sub
- [Examples](/docs/examples) - Complete application examples
