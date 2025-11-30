---
title: Request-Response Patterns
nextjs:
  metadata:
    title: Request-Response Patterns - acton-reactive
    description: Advanced patterns for coordinating between actors including request-reply, aggregation, and saga patterns.
---

Beyond simple fire-and-forget messaging, actors often need to coordinate complex workflows. This page covers advanced patterns for actor coordination.

---

## Basic Request-Reply

The simplest coordination pattern:

```rust
#[acton_message]
struct GetBalance;

#[acton_message]
struct BalanceResponse(f64);

// Responder
account_actor.act_on::<GetBalance>(|actor, ctx| {
    let balance = actor.model.balance;
    let reply = ctx.reply_envelope();

    Reply::pending(async move {
        reply.send(BalanceResponse(balance)).await;
    })
});

// Requester
requester.mutate_on::<CheckAccountBalance>(|actor, ctx| {
    let account = actor.model.account_handle.clone();

    Reply::pending(async move {
        account.send(GetBalance).await;
    })
});

// Handle the response separately
requester.mutate_on::<BalanceResponse>(|actor, ctx| {
    actor.model.last_known_balance = ctx.message().0;
    Reply::ready()
});
```

---

## Self-Messaging

Actors can send messages to themselves for deferred work or state machines:

```rust
#[acton_message]
struct ProcessStep { step: u32 }

actor.mutate_on::<ProcessStep>(|actor, ctx| {
    let step = ctx.message().step;
    let self_handle = actor.handle().clone();

    match step {
        1 => {
            println!("Step 1: Initializing");
            actor.model.initialized = true;
            Reply::pending(async move {
                self_handle.send(ProcessStep { step: 2 }).await;
            })
        }
        2 => {
            println!("Step 2: Processing");
            actor.model.processed = true;
            Reply::pending(async move {
                self_handle.send(ProcessStep { step: 3 }).await;
            })
        }
        3 => {
            println!("Step 3: Complete");
            actor.model.complete = true;
            Reply::ready()
        }
        _ => Reply::ready()
    }
});
```

---

## Deferred Processing

Schedule work for later:

```rust
#[acton_message]
struct ScheduleTask {
    delay: Duration,
    task: TaskData,
}

#[acton_message]
struct ExecuteTask(TaskData);

actor.mutate_on::<ScheduleTask>(|actor, ctx| {
    let delay = ctx.message().delay;
    let task = ctx.message().task.clone();
    let self_handle = actor.handle().clone();

    Reply::pending(async move {
        tokio::time::sleep(delay).await;
        self_handle.send(ExecuteTask(task)).await;
    })
});

actor.mutate_on::<ExecuteTask>(|actor, ctx| {
    // Actually do the work
    process_task(&ctx.message().0);
    Reply::ready()
});
```

### Periodic Tasks

```rust
actor.after_start(|actor| {
    let self_handle = actor.handle().clone();

    Reply::pending(async move {
        // Start a periodic timer
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                self_handle.send(Heartbeat).await;
            }
        });
    })
});

actor.mutate_on::<Heartbeat>(|actor, ctx| {
    actor.model.last_heartbeat = Instant::now();
    // Check health, send metrics, etc.
    Reply::ready()
});
```

---

## Aggregation Pattern

Collect responses from multiple actors:

```rust
#[acton_actor]
struct Aggregator {
    pending: usize,
    results: Vec<WorkResult>,
    requester: Option<OutboundEnvelope>,
}

#[acton_message]
struct AggregateRequest {
    worker_handles: Vec<ActorHandle>,
}

#[acton_message]
struct WorkRequest { data: String }

#[acton_message]
struct WorkResult { output: String }

#[acton_message]
struct AggregatedResults(Vec<WorkResult>);

// Handle incoming aggregate request
aggregator.mutate_on::<AggregateRequest>(|actor, ctx| {
    let workers = ctx.message().worker_handles.clone();
    let self_handle = actor.handle().clone();

    // Track state
    actor.model.pending = workers.len();
    actor.model.results.clear();
    actor.model.requester = Some(ctx.reply_envelope().clone());

    Reply::pending(async move {
        for worker in workers {
            // Route replies back to this aggregator
            let envelope = worker.create_envelope(&self_handle.reply_address());
            envelope.send(WorkRequest { data: "task".into() }).await;
        }
    })
});

// Collect results
aggregator.mutate_on::<WorkResult>(|actor, ctx| {
    actor.model.results.push(ctx.message().clone());
    actor.model.pending -= 1;

    if actor.model.pending == 0 {
        // All results collected
        if let Some(requester) = actor.model.requester.take() {
            let results = std::mem::take(&mut actor.model.results);
            return Reply::pending(async move {
                requester.send(AggregatedResults(results)).await;
            });
        }
    }

    Reply::ready()
});
```

### With Timeout

```rust
#[acton_message]
struct AggregationTimeout;

aggregator.mutate_on::<AggregateRequest>(|actor, ctx| {
    // ... setup ...

    let self_handle = actor.handle().clone();

    Reply::pending(async move {
        // Send to workers...

        // Start timeout
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            self_handle.send(AggregationTimeout).await;
        });
    })
});

aggregator.mutate_on::<AggregationTimeout>(|actor, ctx| {
    if actor.model.pending > 0 {
        // Some workers didn't respond
        println!("Timeout: {} workers didn't respond", actor.model.pending);

        if let Some(requester) = actor.model.requester.take() {
            let results = std::mem::take(&mut actor.model.results);
            return Reply::pending(async move {
                requester.send(AggregatedResults(results)).await;
            });
        }
    }
    Reply::ready()
});
```

---

## Pipeline Pattern

Chain actors for multi-stage processing:

```rust
// Stage 1: Validate
validator.mutate_on::<RawData>(|actor, ctx| {
    let data = ctx.message().clone();
    let next_stage = actor.model.transformer.clone();

    if is_valid(&data) {
        Reply::pending(async move {
            next_stage.send(ValidatedData(data)).await;
        })
    } else {
        Reply::ready() // Drop invalid data
    }
});

// Stage 2: Transform
transformer.mutate_on::<ValidatedData>(|actor, ctx| {
    let transformed = transform(&ctx.message().0);
    let next_stage = actor.model.persister.clone();

    Reply::pending(async move {
        next_stage.send(TransformedData(transformed)).await;
    })
});

// Stage 3: Persist
persister.mutate_on::<TransformedData>(|actor, ctx| {
    actor.model.storage.save(&ctx.message().0);
    Reply::ready()
});
```

---

## Saga Pattern

Coordinate multi-step transactions with compensation:

```rust
#[acton_actor]
struct OrderSaga {
    order_id: String,
    state: SagaState,
    inventory: ActorHandle,
    payment: ActorHandle,
    shipping: ActorHandle,
}

#[derive(Debug, Clone)]
enum SagaState {
    Started,
    InventoryReserved,
    PaymentProcessed,
    ShipmentScheduled,
    Complete,
    Failed(String),
}

#[acton_message]
struct StartOrder { order_id: String, items: Vec<Item>, amount: f64 }

#[acton_message]
struct InventoryReserved { order_id: String }
#[acton_message]
struct InventoryFailed { order_id: String, reason: String }

#[acton_message]
struct PaymentProcessed { order_id: String }
#[acton_message]
struct PaymentFailed { order_id: String, reason: String }

// Start the saga
saga.mutate_on::<StartOrder>(|actor, ctx| {
    let order = ctx.message();
    actor.model.order_id = order.order_id.clone();
    actor.model.state = SagaState::Started;

    let inventory = actor.model.inventory.clone();
    let self_handle = actor.handle().clone();
    let items = order.items.clone();

    Reply::pending(async move {
        let envelope = inventory.create_envelope(&self_handle.reply_address());
        envelope.send(ReserveInventory { items }).await;
    })
});

// Step 2: Inventory reserved, process payment
saga.mutate_on::<InventoryReserved>(|actor, ctx| {
    actor.model.state = SagaState::InventoryReserved;

    let payment = actor.model.payment.clone();
    let self_handle = actor.handle().clone();

    Reply::pending(async move {
        let envelope = payment.create_envelope(&self_handle.reply_address());
        envelope.send(ProcessPayment { /* ... */ }).await;
    })
});

// Compensation: inventory reservation failed
saga.mutate_on::<InventoryFailed>(|actor, ctx| {
    actor.model.state = SagaState::Failed(ctx.message().reason.clone());
    // No compensation needed - nothing was done yet
    Reply::ready()
});

// Step 3: Payment processed, schedule shipping
saga.mutate_on::<PaymentProcessed>(|actor, ctx| {
    actor.model.state = SagaState::PaymentProcessed;

    let shipping = actor.model.shipping.clone();
    let self_handle = actor.handle().clone();

    Reply::pending(async move {
        let envelope = shipping.create_envelope(&self_handle.reply_address());
        envelope.send(ScheduleShipment { /* ... */ }).await;
    })
});

// Compensation: payment failed, release inventory
saga.mutate_on::<PaymentFailed>(|actor, ctx| {
    actor.model.state = SagaState::Failed(ctx.message().reason.clone());

    let inventory = actor.model.inventory.clone();
    let order_id = actor.model.order_id.clone();

    Reply::pending(async move {
        // Compensating action
        inventory.send(ReleaseInventory { order_id }).await;
    })
});
```

---

## Router Pattern

Distribute work across multiple workers:

```rust
#[acton_actor]
struct Router {
    workers: Vec<ActorHandle>,
    next_worker: usize,
}

// Round-robin routing
router.mutate_on::<Task>(|actor, ctx| {
    let worker = actor.model.workers[actor.model.next_worker].clone();
    actor.model.next_worker = (actor.model.next_worker + 1) % actor.model.workers.len();

    let task = ctx.message().clone();
    let reply_to = ctx.reply_envelope().clone();

    Reply::pending(async move {
        let envelope = worker.create_envelope(&reply_to.return_address().clone());
        envelope.send(task).await;
    })
});
```

### Load-Based Routing

```rust
#[acton_actor]
struct SmartRouter {
    workers: Vec<(ActorHandle, usize)>, // (handle, pending_count)
}

router.mutate_on::<Task>(|actor, ctx| {
    // Find least loaded worker
    let (idx, (worker, _)) = actor.model.workers
        .iter()
        .enumerate()
        .min_by_key(|(_, (_, load))| *load)
        .unwrap();

    let worker = worker.clone();
    actor.model.workers[idx].1 += 1; // Increment pending

    let task = ctx.message().clone();
    let self_handle = actor.handle().clone();

    Reply::pending(async move {
        worker.send(task).await;
        // Worker should send TaskComplete back to decrement counter
    })
});
```

---

## Best Practices

### Design for Failure

```rust
// Always handle the case where a response never comes
actor.mutate_on::<RequestWithTimeout>(|actor, ctx| {
    let request_id = uuid::Uuid::new_v4();
    actor.model.pending.insert(request_id, Instant::now());

    let self_handle = actor.handle().clone();

    Reply::pending(async move {
        // Set up timeout
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            self_handle.send(RequestTimeout { request_id }).await;
        });

        // Send actual request...
    })
});
```

### Keep Coordination Logic Centralized

```rust
// Good: coordinator knows the workflow
coordinator.mutate_on::<StartWorkflow>(|actor, ctx| {
    // Coordinator orchestrates all steps
});

// Avoid: distributed spaghetti
actor_a.mutate_on::<Step1>(|actor, ctx| {
    // Send to B which sends to C which sends to D...
});
```

### Use Message Correlation

```rust
#[acton_message]
struct Request {
    correlation_id: Uuid,
    // ...
}

#[acton_message]
struct Response {
    correlation_id: Uuid,
    // ...
}

// Match responses to requests
actor.mutate_on::<Response>(|actor, ctx| {
    let id = ctx.message().correlation_id;
    if let Some(pending) = actor.model.pending_requests.remove(&id) {
        // Handle matched response
    }
    Reply::ready()
});
```

---

## Next Steps

- [Supervision](/docs/supervision) - Parent-child relationships
- [Pub/Sub](/docs/pub-sub) - Broadcasting messages
- [Error Handling](/docs/error-handling) - Handling failures in workflows
