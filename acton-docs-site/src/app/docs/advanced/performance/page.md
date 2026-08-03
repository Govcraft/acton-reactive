---
title: Performance
description: Optimizing actor systems for throughput and latency.
---

Acton Reactive is built for performance. Understanding how actors work helps you optimize for your specific needs.

## Choosing the Right Handler

The choice of handler type has performance implications:

| Handler | Execution | Allocates Future | Best For |
|---------|-----------|-----------------|----------|
| `mutate_on` | Sequential | Yes | State changes with async work |
| `mutate_on_sync` | Sequential | No | State changes without async |
| `act_on` | Concurrent | Yes | Read operations with async work |
| `act_on_sync` | Concurrent | No | Read operations without async |

### Use _sync variants to eliminate allocation overhead

If your handler doesn't need `.await`, prefer the `_sync` variant. It avoids the `Box::pin(async move {})` heap allocation per invocation:

```rust
// Allocates a future unnecessarily
builder.mutate_on::<Increment>(|actor, _ctx| {
    actor.model.count += 1;
    Reply::ready()
});

// Zero allocation — handler returns () directly
builder.mutate_on_sync::<Increment>(|actor, _ctx| {
    actor.model.count += 1;
});
```

### Use act_on for read-heavy workloads

For read-heavy workloads, use `act_on` (or `act_on_sync`) to enable parallel processing:

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

## Panic Protection Has a Cost

The `catch-handler-panics` feature is **enabled by default**. It wraps every handler dispatch in `catch_unwind`, so a panicking handler is caught and logged instead of taking the actor down. That safety costs a small amount of overhead on the dispatch hot path.

For a production workload with well-tested handlers, you can turn it off:

```toml
[dependencies]
acton-reactive = { version = "9", default-features = false }
```

{% callout type="warning" title="Know what you're trading" %}
With the feature disabled, a panic in a handler propagates and kills the actor's task. Only disable it if your handlers genuinely don't panic — and prefer `try_mutate_on` + `on_error` for failures you can anticipate.

Measure before you reach for this. It is a hot-path constant, not an algorithmic win, and it will be dwarfed by anything your handler actually does.
{% /callout %}

---

## Tuning Inbox Capacity

Each actor's inbox is a bounded MPSC channel. When it fills, senders wait — that's backpressure, and usually you want it. But a high-throughput actor being fed in bursts can spend time blocked on a too-small inbox.

Override it per actor:

```rust
let config = ActorConfig::new(Ern::with_root("ingest")?, None)
    .with_inbox_capacity(1024);

let mut actor = runtime.new_actor_with_config::<Ingest>(config);
```

Larger inboxes trade memory for burst tolerance. They don't make a slow handler faster — if an actor is *persistently* behind, the inbox will fill regardless and you need a worker pool, not a bigger buffer.

### System-Wide Knobs

Runtime defaults come from Acton's configuration file, and several are performance-relevant:

| Setting | Effect |
|---------|--------|
| `limits.actor_inbox_capacity` | Default inbox size for every actor |
| `limits.concurrent_handlers_high_water_mark` | How many read-only handlers may be in flight before the actor forces a flush |
| `timeouts.read_only_handler_flush` | How long pending read-only handlers may sit before being flushed |

See [Configuration](/docs/configuration) for where the file lives and the full list.

---

## Measuring

Don't tune on intuition. The repository ships a [Divan](https://docs.rs/divan) benchmark suite covering actor creation, single- and multi-actor message throughput, and ping-pong latency:

```bash
cargo bench --package acton-reactive
```

Run it before and after a change on your own hardware — absolute numbers vary far too much across machines to be worth quoting. For finding hot spots in *your* application rather than the framework, reach for `tracing` spans (below) or a sampling profiler such as `perf` or `samply`.

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

**Solution: Fan Out Independent Requests**

When requests don't depend on each other, dispatch them all before waiting on any of them.

`create_envelope` reads as *"from me, to them"*: the actor you call it on becomes the envelope's **return address**, and the argument is the **recipient**. So the envelope is built by the actor that wants the replies, addressed to the actor being queried:

```rust
// `receiver` is the actor collecting the responses.
// Each envelope: from `receiver`, to the actor being queried.
let request1 = receiver.create_envelope(Some(actor1_handle.reply_address()));
let request2 = receiver.create_envelope(Some(actor2_handle.reply_address()));

request1.send(Query1).await;
request2.send(Query2).await;
```

Both queries are now sitting in their target inboxes and are processed independently — `receiver` gets each answer as a normal message whenever it lands, rather than blocking on the first before issuing the second.

{% callout type="note" title="These awaits are enqueues, not round-trips" %}
`send().await` completes as soon as the message is in the recipient's inbox; it does not wait for the handler to run. Awaiting the two sends in sequence still gets both actors working concurrently. If a target's inbox is full, `send` applies backpressure and waits — use `try_send` if you'd rather fail fast.
{% /callout %}

**From outside an actor**, `ask` does wait for the answer, so serialising the calls really does serialise the work. Drive them concurrently instead:

```rust
// Sequential: the second request is not even sent until the first answers.
let a = actor1_handle.ask(Query1).await?;
let b = actor2_handle.ask(Query2).await?;

// Concurrent: both actors work at once.
let (a, b) = tokio::try_join!(
    actor1_handle.ask(Query1),
    actor2_handle.ask(Query2),
)?;
```

This is the main performance trap `ask` introduces, and the reason `send` remains the default for work you do not need an answer to.

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

- Use `_sync` variants (`mutate_on_sync`, `act_on_sync`) for handlers that don't need async
- Use `act_on` for read operations (concurrent)
- Use `mutate_on` only when modifying state (sequential)
- Batch operations when possible
- Avoid single-actor bottlenecks with worker pools
- Fan out independent requests before awaiting any of them
- Use `Arc` for large data in messages
- Tune `with_inbox_capacity` for bursty actors
- Consider `default-features = false` to drop panic protection in well-tested production handlers
- Measure with `cargo bench` and `tracing` — don't guess

---

## Next

[Integration](/docs/advanced/integration) — Working with the Rust ecosystem
