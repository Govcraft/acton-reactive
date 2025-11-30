---
title: Replies & Context
nextjs:
  metadata:
    title: Replies & Context - acton-reactive
    description: Using MessageContext and reply envelopes for actor communication.
---

Every handler receives a `MessageContext` that provides access to the incoming message and tools for replying. This page covers the full API.

---

## MessageContext

The context is the second parameter to every handler:

```rust
actor.mutate_on::<MyMessage>(|actor, ctx| {
    // ctx is MessageContext<MyMessage>
    Reply::ready()
});
```

### Accessing the Message

```rust
actor.mutate_on::<OrderRequest>(|actor, ctx| {
    // Get immutable reference
    let order: &OrderRequest = ctx.message();
    println!("Order ID: {}", order.id);

    // Get mutable reference (if needed)
    let order_mut: &mut OrderRequest = ctx.message_mut();

    Reply::ready()
});
```

### Message Timestamp

Every message has a timestamp from when it was sent:

```rust
actor.mutate_on::<TimeSensitive>(|actor, ctx| {
    let sent_at: Instant = ctx.timestamp();
    let latency = sent_at.elapsed();

    if latency > Duration::from_secs(5) {
        println!("Warning: message is {} seconds old", latency.as_secs());
    }

    Reply::ready()
});
```

---

## Reply Envelope

The reply envelope is pre-addressed to send messages back to the sender:

```rust
actor.mutate_on::<GetBalance>(|actor, ctx| {
    let balance = actor.model.balance;
    let reply = ctx.reply_envelope();

    Reply::pending(async move {
        reply.send(BalanceResponse(balance)).await;
    })
});
```

### reply_envelope() Methods

```rust
let reply = ctx.reply_envelope();

// Async send (preferred in async blocks)
reply.send(MyResponse { ... }).await;

// Sync send (for use in sync contexts)
reply.reply(MyResponse { ... });

// Get the return address (for forwarding)
let address = reply.return_address();
```

### Convenience Reply

For simple cases, use the convenience method:

```rust
actor.mutate_on::<Query>(|actor, ctx| {
    // Direct reply without getting envelope
    ctx.reply(QueryResponse { value: actor.model.value });
    Reply::ready()
});
```

---

## Reply Patterns

### Single Reply (Request-Response)

The most common pattern - one response per request:

```rust
actor.act_on::<GetStatus>(|actor, ctx| {
    let status = actor.model.status.clone();
    let reply = ctx.reply_envelope();

    Reply::pending(async move {
        reply.send(StatusResponse(status)).await;
    })
});
```

### No Reply (Fire-and-Forget)

Some messages don't need a response:

```rust
actor.mutate_on::<LogEvent>(|actor, ctx| {
    actor.model.events.push(ctx.message().clone());
    // No reply needed
    Reply::ready()
});
```

### Multiple Replies (Streaming)

Send multiple responses for a single request:

```rust
actor.mutate_on::<StreamRequest>(|actor, ctx| {
    let items = actor.model.items.clone();
    let reply = ctx.reply_envelope();

    Reply::pending(async move {
        for (i, item) in items.iter().enumerate() {
            reply.send(StreamItem {
                data: item.clone(),
                index: i,
            }).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        reply.send(StreamComplete).await;
    })
});
```

### Deferred Reply

Start work now, reply later:

```rust
actor.mutate_on::<LongRunningTask>(|actor, ctx| {
    let task_id = uuid::Uuid::new_v4();
    let reply = ctx.reply_envelope();

    // Store the reply channel for later
    actor.model.pending_replies.insert(task_id, reply.clone());

    // Acknowledge immediately
    Reply::pending(async move {
        reply.send(TaskAccepted { task_id }).await;
    })
});

// Later, when work completes:
actor.mutate_on::<TaskComplete>(|actor, ctx| {
    let task_id = ctx.message().task_id;

    if let Some(reply) = actor.model.pending_replies.remove(&task_id) {
        let result = ctx.message().result.clone();
        return Reply::pending(async move {
            reply.send(TaskResult { result }).await;
        });
    }

    Reply::ready()
});
```

---

## Creating New Envelopes

Sometimes you need to send to actors other than the original sender:

### Send to a Known Actor

```rust
actor.mutate_on::<ForwardRequest>(|actor, ctx| {
    let target_handle = actor.model.target.clone();
    let message = ctx.message().clone();

    Reply::pending(async move {
        target_handle.send(message).await;
    })
});
```

### Send with Reply Routing

Route replies to a specific address:

```rust
actor.mutate_on::<ProxyRequest>(|actor, ctx| {
    let service = actor.model.service.clone();
    let original_sender = ctx.reply_envelope().return_address().clone();

    Reply::pending(async move {
        // Create envelope that routes replies back to original sender
        let envelope = service.create_envelope(&original_sender);
        envelope.send(ServiceRequest { ... }).await;
    })
});
```

---

## The OutboundEnvelope

The reply envelope is an `OutboundEnvelope`:

```rust
pub struct OutboundEnvelope {
    // Internal fields
}

impl OutboundEnvelope {
    /// Send a message asynchronously
    pub async fn send<M: ActonMessage>(&self, message: M);

    /// Send a message synchronously
    pub fn reply<M: ActonMessage>(&self, message: M);

    /// Get the return address
    pub fn return_address(&self) -> &MessageAddress;
}
```

### When to Use send() vs reply()

| Method | Use When |
|--------|----------|
| `send(msg).await` | Inside `Reply::pending(async { })` blocks |
| `reply(msg)` | In synchronous contexts (rare) |

```rust
// Async context - use send
Reply::pending(async move {
    reply.send(Response).await;  // ✓
});

// Sync context - use reply
actor.mutate_on::<Msg>(|actor, ctx| {
    ctx.reply_envelope().reply(Response);  // Works but unusual
    Reply::ready()
});
```

---

## Message Addresses

Every actor has a unique address for receiving messages:

```rust
// Get an actor's address
let address: &MessageAddress = handle.reply_address();

// Addresses can be cloned and stored
actor.model.known_actors.insert("logger", address.clone());

// Create envelope to send to an address
let envelope = some_handle.create_envelope(&target_address);
envelope.send(MyMessage).await;
```

---

## Common Patterns

### Request-Reply with Timeout

```rust
actor.act_on::<Query>(|actor, ctx| {
    let data = actor.model.data.clone();
    let reply = ctx.reply_envelope();

    Reply::pending(async move {
        // Simulate slow operation
        match tokio::time::timeout(
            Duration::from_secs(5),
            async { process(data).await }
        ).await {
            Ok(result) => reply.send(QuerySuccess(result)).await,
            Err(_) => reply.send(QueryTimeout).await,
        }
    })
});
```

### Acknowledgment Pattern

```rust
#[acton_message]
struct Ack;

actor.mutate_on::<ImportantMessage>(|actor, ctx| {
    let reply = ctx.reply_envelope();

    // Process the message
    actor.model.process(ctx.message());

    // Acknowledge
    Reply::pending(async move {
        reply.send(Ack).await;
    })
});
```

### Scatter-Gather

Send to multiple actors, collect responses:

```rust
actor.mutate_on::<ScatterRequest>(|actor, ctx| {
    let workers = actor.model.workers.clone();
    let self_handle = actor.handle().clone();
    let request = ctx.message().clone();

    // Track how many responses we need
    actor.model.pending_responses = workers.len();
    actor.model.responses.clear();

    Reply::pending(async move {
        for worker in workers {
            let envelope = worker.create_envelope(&self_handle.reply_address());
            envelope.send(request.clone()).await;
        }
    })
});

actor.mutate_on::<WorkerResponse>(|actor, ctx| {
    actor.model.responses.push(ctx.message().clone());
    actor.model.pending_responses -= 1;

    if actor.model.pending_responses == 0 {
        // All responses collected
        let aggregated = aggregate(&actor.model.responses);
        // Do something with aggregated result
    }

    Reply::ready()
});
```

---

## Best Practices

### Don't Hold Reply Envelopes Forever

```rust
// Bad: memory leak risk
actor.mutate_on::<Subscribe>(|actor, ctx| {
    actor.model.subscribers.push(ctx.reply_envelope().clone());
    // Never cleaned up!
    Reply::ready()
});

// Good: track with cleanup mechanism
actor.mutate_on::<Subscribe>(|actor, ctx| {
    let subscriber_id = ctx.message().id.clone();
    actor.model.subscribers.insert(
        subscriber_id,
        ctx.reply_envelope().clone()
    );
    Reply::ready()
});

actor.mutate_on::<Unsubscribe>(|actor, ctx| {
    actor.model.subscribers.remove(&ctx.message().id);
    Reply::ready()
});
```

### Clone What You Need Before Async

```rust
actor.mutate_on::<Request>(|actor, ctx| {
    // Clone before entering async block
    let reply = ctx.reply_envelope().clone();
    let data = actor.model.data.clone();

    Reply::pending(async move {
        // Now we own these values
        reply.send(Response { data }).await;
    })
});
```

---

## Next Steps

- [Error Handling](/docs/error-handling) - Handling failures in handlers
- [Request-Response](/docs/request-response) - Complex coordination patterns
- [Pub/Sub](/docs/pub-sub) - Broadcasting to multiple actors
