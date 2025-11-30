---
title: Actors & State
nextjs:
  metadata:
    title: Actors & State - acton-reactive
    description: Understanding actors as independent workers with private state. The foundation of the actor model.
---

Actors are the foundation of `acton-reactive`. Each actor is an independent worker with its own private state - no locks, no shared memory, just message passing.

---

## What is an Actor?

Think of an actor like an employee at their desk:

| Office Concept | Acton Concept | What It Does |
|----------------|---------------|--------------|
| **Employee** | Actor | A worker with their own desk and files |
| **Desk & Files** | State (model) | Private data only the actor can access |
| **Memo/Email** | Message | How workers communicate |
| **Job Description** | Handler | "When you get X, do Y" |
| **Employee Badge** | ActorHandle | How to reach a specific worker |

An actor:
- **Owns its state** - No other actor can directly access or modify it
- **Processes messages** - One at a time, in order (for mutable operations)
- **Runs independently** - Actors don't block each other
- **Communicates via messages** - The only way to interact with an actor

---

## Defining Actor State

Actor state is a regular Rust struct with the `#[acton_actor]` attribute:

```rust
use acton_reactive::prelude::*;

#[acton_actor]
struct ShoppingCart {
    items: Vec<String>,
    total: f64,
    customer_id: Option<String>,
}
```

The macro derives `Default` and `Debug` for you. If you need custom initialization, use `no_default` and implement `Default` yourself:

```rust
use std::io::{stdout, Stdout};

#[acton_actor(no_default)]
struct Printer {
    out: Stdout,
}

impl Default for Printer {
    fn default() -> Self {
        Self { out: stdout() }
    }
}
```

### State Requirements

Your actor state must be:
- `Default` - Actors start with default state (use `#[acton_actor(no_default)]` and implement manually if needed)
- `Debug` - For logging and debugging
- `Send + 'static` - Required for async runtime

---

## Creating Actors

Actors are created through the runtime:

```rust
let mut runtime = ActonApp::launch();

// Create an actor builder
let mut cart = runtime.new_actor::<ShoppingCart>();

// Configure handlers (while in Idle state)
cart.mutate_on::<AddItem>(|actor, ctx| {
    actor.model.items.push(ctx.message().name.clone());
    Reply::ready()
});

// Start the actor (transitions to Started state)
let cart_handle = cart.start().await;
```

### The Type-State Pattern

Actors exist in two states, enforced at compile time:

```mermaid
stateDiagram-v2
    direction TB
    state "ManagedActor&lt;Idle&gt;" as Idle {
        note left of Idle
            Available: mutate_on, act_on,
            try_mutate_on, try_act_on,
            lifecycle hooks, handle()
        end note
    }
    state "ManagedActor&lt;Started&gt;" as Started {
        note left of Started
            Available: send(), stop(),
            supervise()
        end note
    }
    Idle --> Started : start().await
```

**Why this matters:** If you try to register a handler after starting, or send a message before starting, the compiler stops you. No runtime surprises.

---

## Actor Handles

An `ActorHandle` is a lightweight reference for sending messages to an actor:

```rust
// Get a handle from the builder (before starting)
let handle = actor_builder.handle().clone();

// Or get it when starting
let handle = actor_builder.start().await;

// Handles are cheap to clone
let handle_copy = handle.clone();

// Send messages via the handle
handle.send(MyMessage { data: 42 }).await;
```

### Handle Properties

- **Cheap to clone** - Just an `Arc` wrapper around channel sender
- **Send + Sync** - Safe to share across threads
- **Identifies the actor** - Each handle has a unique ID

```rust
// Every actor has a unique ID
println!("Actor ID: {}", handle.id());
```

---

## Actor Identity

Actors have unique identifiers called ERNs (Entity Resource Names):

```rust
// Unnamed actor gets auto-generated ID
let actor = runtime.new_actor::<MyState>();
// ID: "my_state_a1b2c3d4"

// Named actor for easier debugging
let actor = runtime.new_actor_with_name::<MyState>("order-processor");
// ID: "order-processor"
```

ERNs reflect the supervision hierarchy:

```text
payment-service/                     # Root actor
├── payment-service/validator        # Child
├── payment-service/processor        # Child
│   └── payment-service/processor/retry-queue   # Grandchild
└── payment-service/logger           # Child
```

---

## Accessing State in Handlers

Inside a handler, access state through `actor.model`:

```rust
actor.mutate_on::<UpdateBalance>(|actor, ctx| {
    // Read state
    let current = actor.model.balance;

    // Modify state
    actor.model.balance += ctx.message().amount;
    actor.model.last_updated = Instant::now();

    Reply::ready()
});
```

### Mutable vs Read-Only Access

- **`mutate_on`** - Gives `&mut actor.model` (exclusive access)
- **`act_on`** - Gives `&actor.model` (shared access)

```rust
// Mutable access - one at a time
actor.mutate_on::<Deposit>(|actor, ctx| {
    actor.model.balance += ctx.message().amount;  // Can modify
    Reply::ready()
});

// Read-only access - can run concurrently
actor.act_on::<GetBalance>(|actor, ctx| {
    let balance = actor.model.balance;  // Can only read
    let reply = ctx.reply_envelope();
    Reply::pending(async move {
        reply.send(BalanceResponse(balance)).await;
    })
});
```

See [Handler Types](/docs/handler-types) for the full picture.

---

## Multiple Actors

Create as many actors as you need:

```rust
let mut runtime = ActonApp::launch();

// Create multiple actors of the same type
let mut worker1 = runtime.new_actor::<Worker>();
let mut worker2 = runtime.new_actor::<Worker>();
let mut worker3 = runtime.new_actor::<Worker>();

// Configure each...
// ...

let handle1 = worker1.start().await;
let handle2 = worker2.start().await;
let handle3 = worker3.start().await;

// Send to specific workers
handle1.send(Task { id: 1 }).await;
handle2.send(Task { id: 2 }).await;
handle3.send(Task { id: 3 }).await;
```

Each actor runs independently. They don't share state and process their own messages concurrently with other actors.

---

## Best Practices

### Keep State Focused

Each actor should have a single responsibility:

```rust
// Good: focused responsibility
#[acton_actor]
struct OrderProcessor {
    pending_orders: Vec<Order>,
    processing_order: Option<OrderId>,
}

// Avoid: too many responsibilities
#[acton_actor]
struct EverythingActor {
    orders: Vec<Order>,
    users: HashMap<UserId, User>,
    inventory: HashMap<ProductId, u32>,
    logs: Vec<LogEntry>,
    // ... this should be multiple actors
}
```

### Use Meaningful Names

```rust
// Good: clear purpose
let order_processor = runtime.new_actor_with_name::<OrderState>("order-processor");
let payment_gateway = runtime.new_actor_with_name::<PaymentState>("payment-gateway");

// Avoid: generic names
let actor1 = runtime.new_actor::<SomeState>();
let actor2 = runtime.new_actor::<OtherState>();
```

### Initialize Complex State in Default

When a field type doesn't implement `Default`, use `no_default` and provide your own:

```rust
use std::io::{stdout, Stdout};

#[acton_actor(no_default)]
struct Printer {
    out: Stdout,
}

impl Default for Printer {
    fn default() -> Self {
        Self { out: stdout() }
    }
}
```

For async initialization (like database connections), keep the field as `Option<T>` and initialize in `after_start` when the message loop is active:

```rust
#[acton_actor]
struct DatabaseActor {
    connection: Option<Connection>,
}

actor.after_start(|actor| {
    let self_handle = actor.handle().clone();
    Reply::pending(async move {
        let conn = Database::connect("...").await.unwrap();
        self_handle.send(SetConnection(conn)).await;
    })
});
```

---

## Next Steps

- [Messages & Handlers](/docs/messages-and-handlers) - How actors communicate
- [Actor Lifecycle](/docs/actor-lifecycle) - Startup, shutdown, and hooks
- [Supervision](/docs/supervision) - Parent-child actor relationships
