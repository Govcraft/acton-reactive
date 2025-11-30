---
title: Handler Types
nextjs:
  metadata:
    title: Handler Types - acton-reactive
    description: Understanding the four handler types - mutate_on, act_on, try_mutate_on, and try_act_on.
---

Acton provides four handler types to cover different combinations of state access and error handling. Choosing the right one affects both correctness and performance.

---

## Quick Reference

| Handler | State Access | Can Fail | Concurrency |
|---------|-------------|----------|-------------|
| `mutate_on` | Mutable | No | Sequential |
| `act_on` | Read-only | No | Concurrent |
| `try_mutate_on` | Mutable | Yes | Sequential |
| `try_act_on` | Read-only | Yes | Concurrent |

**Rule of thumb:** Start with `mutate_on`. Use `act_on` when you're sure the handler only reads. Add `try_` prefix when you need error handling.

---

## mutate_on

Use when you need to modify actor state.

```rust
actor.mutate_on::<UpdateCounter>(|actor, ctx| {
    // Mutable access to state
    actor.model.counter += ctx.message().increment;
    actor.model.last_updated = Instant::now();

    Reply::ready()
});
```

**Characteristics:**
- Exclusive access to `actor.model` (mutable reference)
- Handlers execute one at a time (sequential)
- Cannot return errors (infallible)
- Next message waits until current handler completes

**Execution model:**

```mermaid
flowchart LR
    subgraph Queue["Message Queue"]
        direction LR
        M1["M1"] ~~~ M2["M2"] ~~~ M3["M3"] ~~~ M4["M4"]
    end

    subgraph Exec["Sequential Execution"]
        direction LR
        E1["M1"] --> E2["M2"] --> E3["M3"] --> E4["M4"]
    end

    Queue --> Exec
```

Each message completes before the next starts.

**When to use:**
- State mutations (counters, adding to collections, updating fields)
- Any operation where order matters
- When you're unsure - this is the safe default

---

## act_on

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
- Shared (read-only) access to `actor.model`
- Multiple handlers can run concurrently
- Cannot return errors (infallible)
- Great for queries and notifications

**Execution model:**

```mermaid
flowchart LR
    subgraph Queue["Message Queue"]
        Q1["Q1"] ~~~ Q2["Q2"] ~~~ Q3["Q3"] ~~~ Q4["Q4"] ~~~ Q5["Q5"]
    end

    subgraph B1["Batch 1 (concurrent)"]
        E1["Q1"]
        E2["Q2"]
        E3["Q3"]
    end

    subgraph B2["Batch 2 (concurrent)"]
        E4["Q4"]
        E5["Q5"]
    end

    Queue --> B1 --> B2
```

**When to use:**
- Queries that don't modify state
- Sending notifications/replies
- Heavy read operations (can parallelize)

### High-Water Mark

The maximum concurrent handlers is configurable:

```toml
# config.toml
[limits]
concurrent_handlers_high_water_mark = 100
```

When the limit is reached:
1. Actor waits for all concurrent handlers to complete
2. Then processes the next batch

---

## try_mutate_on

Use when state mutation can fail.

```rust
actor.try_mutate_on::<ProcessPayment>(|actor, ctx| {
    let amount = ctx.message().amount;
    let balance = actor.model.balance;

    if balance < amount {
        // Immediate error
        Reply::try_err(InsufficientFunds { balance, required: amount })
    } else {
        actor.model.balance -= amount;
        // Immediate success
        Reply::try_ok(PaymentSuccess { remaining: actor.model.balance })
    }
});
```

**With async operations:**

```rust
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
- Sequential execution (like `mutate_on`)
- Requires error handler registration with `on_error`
- Use for operations that can fail

**When to use:**
- Validation before mutation
- External service calls that might fail
- Operations with preconditions

---

## try_act_on

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
```

**With immediate result:**

```rust
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
- Concurrent execution (like `act_on`)
- Requires error handler registration
- Use for fallible queries

---

## Error Handler Registration

When using `try_*` handlers, register error handlers with `on_error`:

```rust
// Define error type
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
        actor.model.data = data;
        Reply::try_ok(ProcessSuccess)
    }
});

// Register error handler
actor.on_error::<ProcessData, ValidationError>(|actor, ctx, error| {
    println!("Validation failed: {}", error.0);
    // Optionally update state, send notifications, etc.
    Reply::ready()
});
```

If no error handler is registered, errors are logged and the message is dropped.

---

## Reply Types Summary

| Handler Type | Sync Return | Async Return |
|-------------|-------------|--------------|
| `mutate_on` / `act_on` | `Reply::ready()` | `Reply::pending(async { })` |
| `try_mutate_on` / `try_act_on` | `Reply::try_ok(val)` or `Reply::try_err(err)` | `Reply::try_pending(async { Ok/Err })` |

---

## Choosing the Right Handler

```mermaid
flowchart TD
    Start{"Do you need to modify state?"}
    Start -->|Yes| ModYes{"Can the operation fail?"}
    Start -->|No| ModNo{"Can the operation fail?"}
    ModYes -->|Yes| TryMut["try_mutate_on"]
    ModYes -->|No| Mut["mutate_on"]
    ModNo -->|Yes| TryAct["try_act_on"]
    ModNo -->|No| Act["act_on"]
```

### Examples by Use Case

| Use Case | Handler | Reason |
|----------|---------|--------|
| Increment counter | `mutate_on` | Modifies state, can't fail |
| Get current value | `act_on` | Read-only, can't fail |
| Deduct from balance | `try_mutate_on` | Modifies state, can fail (insufficient funds) |
| Validate API key | `try_act_on` | Read-only, can fail (invalid key) |
| Log a message | `mutate_on` | Modifies state (log buffer) |
| Send notification | `act_on` | Read-only (just reading data to send) |

---

## Performance Considerations

### Use act_on for Read-Heavy Workloads

```rust
// Bad: serializes all queries
actor.mutate_on::<GetData>(|actor, ctx| { ... });

// Good: queries run concurrently
actor.act_on::<GetData>(|actor, ctx| { ... });
```

### Keep Handlers Lightweight

```rust
// Bad: heavy computation blocks other messages
actor.mutate_on::<Process>(|actor, ctx| {
    let result = expensive_computation(); // Blocks!
    actor.model.result = result;
    Reply::ready()
});

// Good: offload to async
actor.mutate_on::<Process>(|actor, ctx| {
    let data = actor.model.input.clone();

    Reply::pending(async move {
        let result = tokio::task::spawn_blocking(move || {
            expensive_computation(&data)
        }).await.unwrap();
        // Note: can't update actor.model here
        // Send result to self if needed
    })
});
```

### Batch Related Operations

```rust
// Instead of many small messages
#[acton_message]
struct IncrementBy(u32);  // Single message with amount

// Rather than
#[acton_message]
struct Increment;  // Sending this 1000 times
```

---

## Next Steps

- [Replies & Context](/docs/replies-and-context) - Full reply and context API
- [Error Handling](/docs/error-handling) - Comprehensive error handling patterns
- [Request-Response](/docs/request-response) - Complex coordination patterns
