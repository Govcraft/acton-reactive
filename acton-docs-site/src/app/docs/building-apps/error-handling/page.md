---
title: Error Handling
description: Building resilient actor systems that handle failures gracefully.
---

Errors happen. Actor systems are designed to handle them gracefully through isolation, supervision, and explicit error handling in handlers.

## Errors in Handlers

Handle expected errors within your handlers:

```rust
builder.mutate_on::<ProcessOrder>(|actor, msg| {
    match validate_order(&msg) {
        Ok(order) => {
            actor.model.orders.push(order);
            Reply::ready()
        }
        Err(e) => {
            tracing::warn!("Invalid order: {}", e);
            Reply::ready()  // Continue processing other messages
        }
    }
});
```

---

## Returning Errors to Callers

When using `ask`, return errors explicitly:

```rust
#[acton_message]
struct ProcessPayment { amount: u64 }

builder.mutate_on::<ProcessPayment>(|actor, msg| {
    if msg.amount > actor.model.balance {
        Reply::with(Err("Insufficient funds"))
    } else {
        actor.model.balance -= msg.amount;
        Reply::with(Ok(msg.amount))
    }
});

// Caller handles the result
let result: Result<u64, &str> = account.ask(ProcessPayment { amount: 100 }).await;
match result {
    Ok(amount) => println!("Processed ${}", amount),
    Err(e) => println!("Failed: {}", e),
}
```

---

## Isolation

One of the actor model's strengths is failure isolation. When one actor fails, others continue:

```rust
// If worker_1 panics, worker_2 and worker_3 keep running
worker_1.send(DangerousTask).await;
worker_2.send(SafeTask).await;  // Still works
worker_3.send(SafeTask).await;  // Still works
```

---

## Design Patterns

### Fail Fast for Configuration Errors

Don't retry configuration or startup errors:

```rust
builder.after_start(|actor| async move {
    let config = load_config().expect("Config required");
    actor.model.config = config;
});
```

### Graceful Degradation

Handle missing actors:

```rust
match worker.send(Task).await {
    Ok(_) => {},
    Err(_) => {
        tracing::warn!("Worker unavailable, queuing task");
        queue.push(task);
    }
}
```

### Circuit Breaker

Track failures and stop sending to failing actors:

```rust
#[acton_actor]
struct Caller {
    failures: u32,
    circuit_open: bool,
}

builder.mutate_on::<CallService>(|actor, msg| {
    if actor.model.circuit_open {
        return Reply::with(Err("Circuit open"));
    }

    match call_external_service(&msg) {
        Ok(result) => {
            actor.model.failures = 0;
            Reply::with(Ok(result))
        }
        Err(e) => {
            actor.model.failures += 1;
            if actor.model.failures >= 5 {
                actor.model.circuit_open = true;
            }
            Reply::with(Err(e))
        }
    }
});
```

---

## Logging

Always log before failures for debugging:

```rust
builder.mutate_on::<RiskyOperation>(|actor, msg| {
    tracing::info!(operation = %msg.id, "Starting risky operation");

    match perform_operation(&msg) {
        Ok(result) => {
            tracing::info!(operation = %msg.id, "Operation succeeded");
            Reply::with(Ok(result))
        }
        Err(e) => {
            tracing::error!(operation = %msg.id, error = %e, "Operation failed");
            Reply::with(Err(e))
        }
    }
});
```

---

## Summary

- Handle expected errors in handlers
- Return `Result` or `Option` from `ask` handlers
- Rely on actor isolation for fault tolerance
- Log errors for debugging
- Consider patterns like circuit breakers for external services

---

## Next

[Testing Actors](/docs/building-apps/testing-actors) - Strategies for testing
