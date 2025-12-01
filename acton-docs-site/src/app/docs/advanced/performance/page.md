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
builder.act_on::<GetCount>(|actor, _| Reply::with(actor.model.count));
builder.act_on::<GetName>(|actor, _| Reply::with(actor.model.name.clone()));
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

builder.mutate_on::<ProcessBatch>(|actor, msg| {
    for item in &msg.items {
        process(&item);
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

### Ask Chains

Long chains of `ask` calls add latency:

```rust
// Each ask waits for the previous
let a = actor1.ask(Query1).await;
let b = actor2.ask(Query2 { data: a }).await;
let c = actor3.ask(Query3 { data: b }).await;
```

**Solution: Parallelize Independent Requests**

```rust
// Independent requests can run in parallel
let (a, b) = tokio::join!(
    actor1.ask(Query1),
    actor2.ask(Query2)
);
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

builder.mutate_on::<Cleanup>(|actor, _| {
    actor.model.cache.retain(|k, v| !v.is_expired());
    Reply::ready()
});

// Schedule periodic cleanup
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        handle.send(Cleanup).await.ok();
    }
});
```

---

## Profiling

Use tracing to identify bottlenecks:

```rust
builder.mutate_on::<ExpensiveOperation>(|actor, msg| {
    let span = tracing::info_span!("expensive_op", id = %msg.id);
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
- Parallelize independent `ask` calls
- Use `Arc` for large data in messages
- Profile with tracing to find bottlenecks

---

## Next

[Integration](/docs/advanced/integration) - Working with the Rust ecosystem
