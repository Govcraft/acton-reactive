---
title: Error Handling
nextjs:
  metadata:
    title: Error Handling - acton-reactive
    description: Handling errors in actor message handlers with try_mutate_on and try_act_on.
---

Some operations can fail. Acton provides fallible handler variants and error handler registration for clean error handling.

---

## Fallible Handlers

Use `try_mutate_on` and `try_act_on` for operations that can fail:

```rust
actor.try_mutate_on::<ProcessPayment>(|actor, ctx| {
    let amount = ctx.message().amount;

    if actor.model.balance < amount {
        Reply::try_err(InsufficientFunds {
            available: actor.model.balance,
            required: amount,
        })
    } else {
        actor.model.balance -= amount;
        Reply::try_ok(PaymentSuccess {
            remaining: actor.model.balance,
        })
    }
});
```

---

## Defining Error Types

Errors must implement `Error`, `Debug`, and `Display`:

```rust
use std::error::Error;
use std::fmt;

#[derive(Debug)]
struct InsufficientFunds {
    available: f64,
    required: f64,
}

impl fmt::Display for InsufficientFunds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Insufficient funds: need {}, have {}",
            self.required, self.available
        )
    }
}

impl Error for InsufficientFunds {}
```

### Using thiserror

The `thiserror` crate simplifies error definitions:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
enum PaymentError {
    #[error("Insufficient funds: need {required}, have {available}")]
    InsufficientFunds { available: f64, required: f64 },

    #[error("Payment gateway error: {0}")]
    GatewayError(String),

    #[error("Invalid amount: {0}")]
    InvalidAmount(f64),
}
```

---

## Registering Error Handlers

Use `on_error` to handle specific error types:

```rust
// The fallible handler
actor.try_mutate_on::<ProcessPayment>(|actor, ctx| {
    // ... validation and processing
});

// Error handler for a specific error type
actor.on_error::<ProcessPayment, InsufficientFunds>(|actor, ctx, error| {
    println!(
        "Payment failed for {}: {}",
        ctx.message().customer_id,
        error
    );

    // Optionally notify the sender
    let reply = ctx.reply_envelope();
    Reply::pending(async move {
        reply.send(PaymentFailed {
            reason: error.to_string(),
        }).await;
    })
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

### Multiple Error Types

Register handlers for each error type:

```rust
actor.try_mutate_on::<ProcessPayment>(|actor, ctx| {
    // Can return different errors
    if amount <= 0.0 {
        return Reply::try_err(InvalidAmount(amount));
    }
    if actor.model.balance < amount {
        return Reply::try_err(InsufficientFunds { ... });
    }
    // ... process
});

// Handle each error type
actor
    .on_error::<ProcessPayment, InvalidAmount>(|_, ctx, error| {
        println!("Invalid amount: {}", error.0);
        Reply::ready()
    })
    .on_error::<ProcessPayment, InsufficientFunds>(|actor, ctx, error| {
        // Maybe log, update state, or notify
        actor.model.failed_attempts += 1;
        Reply::ready()
    });
```

---

## Reply Types for Fallible Handlers

### Immediate Results

```rust
// Immediate success
Reply::try_ok(SuccessValue)

// Immediate error
Reply::try_err(ErrorValue)
```

### Async Results

```rust
Reply::try_pending(async move {
    match some_async_operation().await {
        Ok(result) => Ok(SuccessResponse { result }),
        Err(e) => Err(MyError::from(e)),
    }
})
```

### Complete Example

```rust
actor.try_mutate_on::<FetchData>(|actor, ctx| {
    let url = ctx.message().url.clone();
    let client = actor.model.http_client.clone();

    Reply::try_pending(async move {
        let response = client.get(&url).await
            .map_err(|e| FetchError::NetworkError(e.to_string()))?;

        if response.status() != 200 {
            return Err(FetchError::HttpError(response.status()));
        }

        let data = response.json().await
            .map_err(|e| FetchError::ParseError(e.to_string()))?;

        Ok(FetchSuccess { data })
    })
});
```

---

## Error Propagation

If no error handler is registered:

1. The error is logged at `error` level
2. The message is dropped
3. The actor continues processing other messages

```rust
// Without an error handler registered:
actor.try_mutate_on::<RiskyOperation>(|actor, ctx| {
    Reply::try_err(SomeError)
});

// Error is logged:
// ERROR acton_reactive: Unhandled error in RiskyOperation handler: SomeError
```

---

## Patterns

### Validation Before Mutation

```rust
actor.try_mutate_on::<UpdateProfile>(|actor, ctx| {
    let update = ctx.message();

    // Validate first
    if update.email.is_empty() {
        return Reply::try_err(ValidationError::EmptyEmail);
    }
    if !update.email.contains('@') {
        return Reply::try_err(ValidationError::InvalidEmail);
    }

    // Then mutate
    actor.model.email = update.email.clone();
    actor.model.name = update.name.clone();

    Reply::try_ok(ProfileUpdated)
});
```

### Retry Logic

```rust
actor.try_mutate_on::<SendEmail>(|actor, ctx| {
    let email = ctx.message().clone();
    let mailer = actor.model.mailer.clone();
    let max_retries = 3;

    Reply::try_pending(async move {
        let mut last_error = None;

        for attempt in 1..=max_retries {
            match mailer.send(&email).await {
                Ok(_) => return Ok(EmailSent),
                Err(e) => {
                    last_error = Some(e);
                    if attempt < max_retries {
                        tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                    }
                }
            }
        }

        Err(EmailError::MaxRetriesExceeded(last_error.unwrap()))
    })
});
```

### Circuit Breaker

```rust
#[acton_actor]
struct ServiceClient {
    failures: u32,
    circuit_open: bool,
    last_failure: Option<Instant>,
}

actor.try_mutate_on::<CallService>(|actor, ctx| {
    // Check circuit breaker
    if actor.model.circuit_open {
        if let Some(last) = actor.model.last_failure {
            if last.elapsed() < Duration::from_secs(30) {
                return Reply::try_err(CircuitBreakerOpen);
            }
            // Try to reset
            actor.model.circuit_open = false;
        }
    }

    let client = actor.model.client.clone();
    let request = ctx.message().clone();

    Reply::try_pending(async move {
        client.call(request).await
            .map(|r| CallSuccess(r))
            .map_err(|e| ServiceError(e))
    })
});

actor.on_error::<CallService, ServiceError>(|actor, ctx, error| {
    actor.model.failures += 1;
    actor.model.last_failure = Some(Instant::now());

    if actor.model.failures >= 5 {
        actor.model.circuit_open = true;
        println!("Circuit breaker opened after {} failures", actor.model.failures);
    }

    Reply::ready()
});
```

### Error Recovery with State Rollback

```rust
actor.try_mutate_on::<TransferFunds>(|actor, ctx| {
    let transfer = ctx.message();

    // Save state for rollback
    let original_balance = actor.model.balance;

    // Attempt transfer
    actor.model.balance -= transfer.amount;

    let external_service = actor.model.external_service.clone();
    let amount = transfer.amount;

    Reply::try_pending(async move {
        match external_service.complete_transfer(amount).await {
            Ok(_) => Ok(TransferComplete),
            Err(e) => {
                // Note: Can't rollback here directly
                // Need to send a rollback message to self
                Err(TransferFailed { amount, reason: e.to_string() })
            }
        }
    })
});

// Handle rollback via error handler
actor.on_error::<TransferFunds, TransferFailed>(|actor, ctx, error| {
    // Rollback
    actor.model.balance += error.amount;
    println!("Rolled back transfer of {}", error.amount);
    Reply::ready()
});
```

---

## Best Practices

### Be Specific with Error Types

```rust
// Good: specific errors
#[derive(Debug, Error)]
enum OrderError {
    #[error("Product {0} not found")]
    ProductNotFound(String),

    #[error("Insufficient stock: need {needed}, have {available}")]
    InsufficientStock { needed: u32, available: u32 },

    #[error("Order expired")]
    OrderExpired,
}

// Avoid: generic errors
#[derive(Debug, Error)]
#[error("{0}")]
struct GenericError(String);
```

### Always Register Error Handlers for Critical Operations

```rust
// Payment processing should never silently fail
actor.try_mutate_on::<ProcessPayment>(|actor, ctx| { ... });

actor
    .on_error::<ProcessPayment, PaymentDeclined>(|_, ctx, error| {
        // Notify customer, log for support
        Reply::ready()
    })
    .on_error::<ProcessPayment, GatewayError>(|_, ctx, error| {
        // Alert ops team, queue for retry
        Reply::ready()
    });
```

### Log Errors with Context

```rust
actor.on_error::<ProcessOrder, OrderError>(|actor, ctx, error| {
    tracing::error!(
        order_id = %ctx.message().order_id,
        customer_id = %ctx.message().customer_id,
        error = %error,
        "Order processing failed"
    );
    Reply::ready()
});
```

---

## Next Steps

- [Request-Response](/docs/request-response) - Coordinating between actors
- [Handler Types](/docs/handler-types) - Understanding handler execution models
- [Testing](/docs/testing) - Testing error scenarios
