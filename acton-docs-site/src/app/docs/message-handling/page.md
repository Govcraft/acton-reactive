---
title: Message Handling
nextjs:
  metadata:
    title: Message Handling - acton-reactive
    description: In-depth look at message handling in acton-reactive, including handler types, concurrency models, error handling, and advanced patterns.
---

This guide provides an in-depth look at message handling in `acton-reactive`, including handler types, concurrency models, error handling, and advanced patterns.

---

## Handler Types

`acton-reactive` provides four handler types to cover different use cases:

| Handler | State Access | Can Fail | Concurrency |
|---------|-------------|----------|-------------|
| `mutate_on` | Mutable | No | Sequential |
| `act_on` | Read-only | No | Concurrent |
| `try_mutate_on` | Mutable | Yes | Sequential |
| `try_act_on` | Read-only | Yes | Concurrent |

### mutate_on

Use when you need to modify actor state.

```rust
actor.mutate_on::<UpdateCounter>(|actor, ctx| {
    // Mutable access to state
    actor.model.counter += ctx.message().increment;

    // Return type is a future
    Reply::ready()
});
```

**Characteristics:**
- Exclusive access to actor state
- Handlers execute one at a time
- Cannot return errors (infallible)
- Use for state mutations

### act_on

Use for read-only operations that can run concurrently.

```rust
actor.act_on::<GetStatus>(|actor, ctx| {
    // Read-only access to state
    let status = actor.model.status.clone();
    let reply = ctx.reply_envelope();

    Reply::pending(async move {
        reply.send(StatusResponse(status)).await;
    })
});
```

**Characteristics:**
- Shared (read-only) access to state
- Multiple handlers can run concurrently
- Cannot return errors (infallible)
- Use for queries and notifications

### try_mutate_on

Use when state mutation can fail.

```rust
actor.try_mutate_on::<ProcessPayment>(|actor, ctx| {
    let amount = ctx.message().amount;
    let balance = actor.model.balance;

    if balance < amount {
        // Immediate error - no async needed
        Reply::try_err(InsufficientFunds { balance, required: amount })
    } else {
        actor.model.balance -= amount;
        // Immediate success - no async needed
        Reply::try_ok(PaymentSuccess { remaining: balance - amount })
    }
});

// Or with async operations:
actor.try_mutate_on::<ProcessPayment>(|actor, ctx| {
    let amount = ctx.message().amount;
    let balance = actor.model.balance;
    let payment_service = actor.model.payment_service.clone();

    Reply::try_pending(async move {
        if balance < amount {
            Err(InsufficientFunds { balance, required: amount })
        } else {
            payment_service.charge(amount).await?;
            Ok(PaymentSuccess { remaining: balance - amount })
        }
    })
});
```

**Characteristics:**
- Mutable access with error handling
- Sequential execution
- Requires error handler registration
- Use for operations that can fail

### try_act_on

Use for concurrent read-only operations that can fail.

```rust
actor.try_act_on::<ValidateToken>(|actor, ctx| {
    let token = ctx.message().token.clone();
    let validator = actor.model.validator.clone();

    Reply::try_pending(async move {
        let is_valid = validator.validate(&token).await?;
        Ok(ValidationResult { valid: is_valid })
    })
});

// Or with immediate result:
actor.try_act_on::<CheckCache>(|actor, ctx| {
    let key = ctx.message().key.clone();

    match actor.model.cache.get(&key) {
        Some(value) => Reply::try_ok(CacheHit { value: value.clone() }),
        None => Reply::try_err(CacheMiss { key }),
    }
});
```

**Characteristics:**
- Read-only access with error handling
- Concurrent execution
- Requires error handler registration
- Use for fallible queries

---

## Message Context

The `MessageContext<M>` provides access to the incoming message and reply capabilities.

### Context Methods

```rust
actor.mutate_on::<MyMessage>(|actor, ctx| {
    // Access the message
    let message: &MyMessage = ctx.message();

    // Get mutable access (if needed)
    let message_mut: &mut MyMessage = ctx.message_mut();

    // Get message timestamp
    let when: Instant = ctx.timestamp();

    // Get envelope for replying
    let reply_envelope: &OutboundEnvelope = ctx.reply_envelope();

    // Convenience method to reply
    ctx.reply(ResponseMessage { data: 42 });

    Reply::ready()
});
```

### Reply Envelope

The reply envelope is pre-addressed to the sender:

```rust
actor.mutate_on::<Request>(|actor, ctx| {
    let reply = ctx.reply_envelope();

    Reply::pending(async move {
        // Async send
        reply.send(Response { success: true }).await;

        // Or sync send (for use in sync contexts)
        reply.reply(Response { success: true });
    })
});
```

---

## Handler Concurrency

### Sequential Handlers (mutate_on)

Mutable handlers execute one at a time:

```text
Message Queue: [M1, M2, M3, M4]

Execution:
┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐
│   M1    │→ │   M2    │→ │   M3    │→ │   M4    │
└─────────┘  └─────────┘  └─────────┘  └─────────┘
   time →
```

### Concurrent Handlers (act_on)

Read-only handlers execute concurrently up to a high-water mark:

```text
Message Queue: [Q1, Q2, Q3, Q4, Q5]

Execution (HWM = 3):
┌─────┬─────┬─────┐     ┌─────┬─────┐
│ Q1  │ Q2  │ Q3  │ →   │ Q4  │ Q5  │
└─────┴─────┴─────┘     └─────┴─────┘
        │                     │
        ▼                     ▼
    wait for all          wait for all
```

### High-Water Mark Control

Configure via `config.toml`:

```toml
[limits]
concurrent_handlers_high_water_mark = 100
```

When the limit is reached:
1. Actor waits for all concurrent handlers to complete
2. Then processes the next batch

---

## Error Handling

### Registering Error Handlers

Use `on_error` to handle specific error types:

```rust
// Define error types
#[derive(Debug)]
struct ValidationError(String);
impl std::error::Error for ValidationError {}
impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Validation error: {}", self.0)
    }
}

// Register fallible handler
actor.try_mutate_on::<ProcessData>(|actor, ctx| {
    let data = ctx.message().data.clone();

    if data.is_empty() {
        Reply::try_err(ValidationError("Empty data".into()))
    } else {
        Reply::try_ok(Success)
    }
});

// Register error handler for specific error type
actor.on_error::<ProcessData, ValidationError>(|actor, ctx, error| {
    println!("Validation failed: {}", error.0);
    // Optionally update state, send notifications, etc.
    Reply::ready()
});
```

### Error Handler Signature

```rust
fn error_handler(
    actor: &mut ManagedActor<Started, Model>,
    ctx: &mut MessageContext<OriginalMessage>,
    error: &SpecificErrorType,
) -> impl Future<Output = ()>
```

### Error Propagation

If no error handler is registered, the error is logged and the message is dropped.

---

## Reply Patterns

### Single Reply

Most common pattern - one response per request:

```rust
actor.mutate_on::<GetBalance>(|actor, ctx| {
    let balance = actor.model.balance;
    let reply = ctx.reply_envelope();

    Reply::pending(async move {
        reply.send(BalanceResponse(balance)).await;
    })
});
```

### No Reply

For fire-and-forget messages:

```rust
actor.mutate_on::<LogEvent>(|actor, ctx| {
    actor.model.events.push(ctx.message().clone());
    // No reply needed
    Reply::ready()
});
```

### Multiple Replies (Streaming)

For streaming responses:

```rust
actor.mutate_on::<StreamRequest>(|actor, ctx| {
    let items = actor.model.items.clone();
    let reply = ctx.reply_envelope();

    Reply::pending(async move {
        for item in items {
            reply.send(StreamItem(item)).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        reply.send(StreamComplete).await;
    })
});
```

---

## Advanced Patterns

### Self-Messaging

Actors can send messages to themselves:

```rust
actor.mutate_on::<StartProcess>(|actor, ctx| {
    let self_handle = actor.handle().clone();

    Reply::pending(async move {
        // Process step 1
        // ...

        // Schedule next step
        self_handle.send(ProcessStep2).await;
    })
});
```

### Deferred Processing

Schedule work for later:

```rust
actor.mutate_on::<ScheduleTask>(|actor, ctx| {
    let delay = ctx.message().delay;
    let self_handle = actor.handle().clone();
    let task = ctx.message().task.clone();

    Reply::pending(async move {
        tokio::time::sleep(delay).await;
        self_handle.send(ExecuteTask(task)).await;
    })
});
```

### Request-Response Pattern

Coordinate between actors:

```rust
// Requester actor
actor.mutate_on::<InitiateProcess>(|actor, ctx| {
    let service = actor.model.service_handle.clone();
    let reply = ctx.reply_envelope();

    Reply::pending(async move {
        // Create envelope that routes response back to us
        let envelope = service.create_envelope(reply.return_address());
        envelope.send(ServiceRequest { /* ... */ }).await;
    })
});

// The response will come as a separate message
actor.mutate_on::<ServiceResponse>(|actor, ctx| {
    // Handle the response
    actor.model.last_result = Some(ctx.message().result.clone());
    Reply::ready()
});
```

### Aggregation Pattern

Collect responses from multiple actors:

```rust
#[acton_actor]
struct Aggregator {
    pending: usize,
    results: Vec<Result>,
    requester: Option<MessageAddress>,
}

actor.mutate_on::<AggregateRequest>(|actor, ctx| {
    let workers = actor.model.workers.clone();
    let self_handle = actor.handle().clone();

    actor.model.pending = workers.len();
    actor.model.results.clear();
    actor.model.requester = Some(ctx.reply_envelope().return_address().clone());

    Reply::pending(async move {
        for worker in workers {
            let envelope = worker.create_envelope(&self_handle.reply_address());
            envelope.send(WorkRequest { /* ... */ }).await;
        }
    })
});

actor.mutate_on::<WorkResult>(|actor, ctx| {
    actor.model.results.push(ctx.message().result.clone());
    actor.model.pending -= 1;

    if actor.model.pending == 0 {
        // All results collected
        if let Some(requester) = &actor.model.requester {
            let results = actor.model.results.clone();
            let envelope = actor.handle().create_envelope(requester);

            return Reply::pending(async move {
                envelope.send(AggregatedResults(results)).await;
            });
        }
    }

    Reply::ready()
});
```

---

## Handler Return Types

The `Reply` struct provides convenient helpers for creating handler return types.

### Infallible Handlers (`mutate_on`, `act_on`)

```rust
// For synchronous handlers (no async work)
Reply::ready()

// For async handlers
Reply::pending(async move {
    // async work
})
```

### Fallible Handlers (`try_mutate_on`, `try_act_on`)

```rust
// Async handler returning Result
Reply::try_pending(async move {
    if condition {
        Ok(SuccessResponse { data })
    } else {
        Err(MyError::new("reason"))
    }
})

// Immediate success (no async work)
Reply::try_ok(SuccessResponse { data })

// Immediate error (no async work)
Reply::try_err(MyError::new("reason"))
```

### Summary Table

| Handler Type | Sync Return | Async Return |
|-------------|-------------|--------------|
| `mutate_on` / `act_on` | `Reply::ready()` | `Reply::pending(async { })` |
| `try_mutate_on` / `try_act_on` | `Reply::try_ok(val)` or `Reply::try_err(err)` | `Reply::try_pending(async { Ok/Err })` |

---

## Performance Considerations

### Handler Weight

- Keep handlers lightweight
- Offload heavy computation to spawn tasks
- Use `act_on` for read-heavy workloads

### Backpressure

- Monitor inbox capacity
- Use `try_send` when backpressure handling is needed
- Consider rate limiting at the application level

### Memory

- Clone data before moving into async blocks
- Use `Arc` for large shared data
- Consider streaming for large payloads
