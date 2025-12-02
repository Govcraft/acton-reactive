---
title: Error Handling
description: Building resilient actor systems that handle failures gracefully.
---

Errors happen. Actor systems are designed to handle them gracefully through isolation, supervision, and explicit error handling in handlers.

## Errors in Handlers

Handle expected errors within your handlers:

```rust
builder.mutate_on::<ProcessOrder>(|actor, envelope| {
    let msg = envelope.message();
    match validate_order(msg) {
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

## Signaling Errors to Other Actors

When another actor needs to know about failures, send error response messages:

```rust
#[acton_message]
struct ProcessPayment { amount: u64 }

#[acton_message]
struct PaymentSuccess { amount: u64 }

#[acton_message]
struct PaymentFailed { reason: String }

builder.mutate_on::<ProcessPayment>(|actor, envelope| {
    let msg = envelope.message();
    let reply_envelope = envelope.reply_envelope();

    if msg.amount > actor.model.balance {
        Reply::pending(async move {
            reply_envelope.send(PaymentFailed {
                reason: "Insufficient funds".into()
            }).await;
        })
    } else {
        actor.model.balance -= msg.amount;
        Reply::pending(async move {
            reply_envelope.send(PaymentSuccess {
                amount: msg.amount
            }).await;
        })
    }
});
```

The requesting actor handles both outcomes:

```rust
requester
    .mutate_on::<PaymentSuccess>(|_actor, envelope| {
        let amount = envelope.message().amount;
        println!("Processed ${}", amount);
        Reply::ready()
    })
    .mutate_on::<PaymentFailed>(|_actor, envelope| {
        let reason = &envelope.message().reason;
        println!("Payment failed: {}", reason);
        Reply::ready()
    });
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
builder.before_start(|actor| async move {
    let config = load_config().expect("Config required");
    // Store in actor state via initialization pattern
});
```

### Graceful Degradation

Log and continue when non-critical operations fail:

```rust
builder.mutate_on::<OptionalTask>(|actor, envelope| {
    let msg = envelope.message();
    match perform_optional_work(msg) {
        Ok(_) => tracing::info!("Optional work completed"),
        Err(e) => tracing::warn!("Optional work failed, continuing: {}", e),
    }
    Reply::ready()
});
```

### Circuit Breaker

Track failures and stop sending to failing services:

```rust
#[acton_actor]
struct Caller {
    failures: u32,
    circuit_open: bool,
}

#[acton_message]
struct CallService { data: String }

#[acton_message]
struct ServiceSuccess;

#[acton_message]
struct ServiceFailed { reason: String }

builder.mutate_on::<CallService>(|actor, envelope| {
    let reply_envelope = envelope.reply_envelope();

    if actor.model.circuit_open {
        return Reply::pending(async move {
            reply_envelope.send(ServiceFailed {
                reason: "Circuit open".into()
            }).await;
        });
    }

    let msg = envelope.message().clone();
    match call_external_service(&msg) {
        Ok(_) => {
            actor.model.failures = 0;
            Reply::pending(async move {
                reply_envelope.send(ServiceSuccess).await;
            })
        }
        Err(e) => {
            actor.model.failures += 1;
            if actor.model.failures >= 5 {
                actor.model.circuit_open = true;
            }
            Reply::pending(async move {
                reply_envelope.send(ServiceFailed {
                    reason: e.to_string()
                }).await;
            })
        }
    }
});
```

---

## Logging

Always log before failures for debugging:

```rust
builder.mutate_on::<RiskyOperation>(|actor, envelope| {
    let msg = envelope.message();
    tracing::info!(operation = %msg.id, "Starting risky operation");

    match perform_operation(msg) {
        Ok(_) => {
            tracing::info!(operation = %msg.id, "Operation succeeded");
            Reply::ready()
        }
        Err(e) => {
            tracing::error!(operation = %msg.id, error = %e, "Operation failed");
            Reply::ready()
        }
    }
});
```

---

## Summary

- Handle expected errors within handlers
- Send explicit success/failure response messages
- Rely on actor isolation for fault tolerance
- Log errors for debugging
- Consider patterns like circuit breakers for external services

---

## Next

[Testing Actors](/docs/building-apps/testing-actors) — Strategies for testing
