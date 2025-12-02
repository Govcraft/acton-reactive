---
title: Performance
description: Optimizing actor systems for throughput and latency.
---

Acton Reactive is built for performance. Understanding how actors work helps you optimize for your specific needs.

## mutate_on vs act_on

The choice between `mutate_on` and `act_on` has performance implications:

| Handler | Execution | Best For |
|---------|-----------|----------|
| `mutate_on` | Sequential | State changes |
| `act_on` | Concurrent | Read operations |

For read-heavy workloads, use `act_on` to enable parallel processing:

```rust
// These can run concurrently
builder.act_on::<GetCount>(|actor, envelope| {
    let count = actor.model.count;
    let reply = envelope.reply_envelope();
    Reply::pending(async move {
        reply.send(CountResponse(count)).await;
    })
});

builder.act_on::<GetName>(|actor, envelope| {
    let name = actor.model.name.clone();
    let reply = envelope.reply_envelope();
    Reply::pending(async move {
        reply.send(NameResponse(name)).await;
    })
});
```

---

## Batching Messages

When processing many items, batch them:

```rust
// Instead of many individual messages
for item in items {
    actor.send(ProcessItem { item }).await;
}

// Use a batch message
#[acton_message]
struct ProcessBatch { items: Vec<Item> }

builder.mutate_on::<ProcessBatch>(|actor, envelope| {
    let items = &envelope.message().items;
    for item in items {
        process(item);
    }
    Reply::ready()
});
```

---

## Avoiding Bottlenecks

### Single Actor Bottleneck

If one actor handles all requests, it becomes a bottleneck:

```rust
// All requests go through one actor - bottleneck!
single_actor.send(Request).await;
```

**Solution: Worker Pool**

```rust
// Distribute across multiple actors
let worker = &workers[request_id % workers.len()];
worker.send(Request).await;
```

### Request Chains Add Latency

Long chains of requests add latency:

```rust
// Each request waits for the previous
// actor1 responds, then actor2 processes, then actor3...
```

**Solution: Parallelize Independent Requests**

When requests are independent, send them concurrently:

```rust
// Send independent requests in parallel
let request1 = actor1_handle.create_envelope(Some(receiver.reply_address()));
let request2 = actor2_handle.create_envelope(Some(receiver.reply_address()));

request1.send(Query1).await;
request2.send(Query2).await;
// Both process concurrently
```

---

## Memory Considerations

### Clone Wisely

Messages are cloned when sent. Avoid cloning large data:

```rust
// Expensive: clones large Vec on every send
#[acton_message]
struct ProcessData { data: Vec<u8> }  // Large

// Better: use Arc for large data
#[acton_message]
struct ProcessData { data: Arc<Vec<u8>> }  // Cheap clone
```

### Clean Up Actor State

Long-running actors can accumulate state. Clean up periodically:

```rust
#[acton_message]
struct Cleanup;

builder.mutate_on::<Cleanup>(|actor, _envelope| {
    actor.model.cache.retain(|_k, v| !v.is_expired());
    Reply::ready()
});

// Schedule periodic cleanup
let cleanup_handle = handle.clone();
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        cleanup_handle.send(Cleanup).await;
    }
});
```

---

## Profiling

Use tracing to identify bottlenecks:

```rust
builder.mutate_on::<ExpensiveOperation>(|actor, envelope| {
    let id = envelope.message().id;
    let span = tracing::info_span!("expensive_op", id = %id);
    let _guard = span.enter();

    // Operation timing will be captured
    perform_operation();

    Reply::ready()
});
```

View traces with tools like Jaeger or the console:

```rust
// In main
tracing_subscriber::fmt::init();
```

---

## Summary

- Use `act_on` for read operations (concurrent)
- Use `mutate_on` only when modifying state (sequential)
- Batch operations when possible
- Avoid single-actor bottlenecks with worker pools
- Send independent requests concurrently
- Use `Arc` for large data in messages
- Profile with tracing to find bottlenecks

---

## Next

[Integration](/docs/advanced/integration) — Working with the Rust ecosystem
