---
title: Messages & Handlers
nextjs:
  metadata:
    title: Messages & Handlers - acton-reactive
    description: How actors communicate through messages and process them with handlers.
---

Messages are the only way actors communicate. This page covers how to define messages, register handlers, and understand the message flow.

---

## Defining Messages

Messages are Rust structs with the `#[acton_message]` attribute:

```rust
use acton_reactive::prelude::*;

// Simple message with no data
#[acton_message]
struct Ping;

// Message with data
#[acton_message]
struct AddItem {
    name: String,
    quantity: u32,
}

// Tuple struct message
#[acton_message]
struct Increment(u32);

// Message with complex data
#[acton_message]
struct ProcessOrder {
    order_id: String,
    items: Vec<OrderItem>,
    customer: Customer,
}
```

The `#[acton_message]` macro derives `Debug` and `Clone`. Your message fields must also be `Clone`.

### Message Requirements

Messages must be:
- `Debug` - For logging
- `Clone` - Messages may be broadcast to multiple actors
- `Send + 'static` - Required for async runtime

---

## Registering Handlers

Handlers define what happens when a message arrives. Register them before starting the actor:

```rust
let mut actor = runtime.new_actor::<MyState>();

// Register handlers
actor
    .mutate_on::<MessageA>(|actor, ctx| {
        // Handle MessageA
        Reply::ready()
    })
    .mutate_on::<MessageB>(|actor, ctx| {
        // Handle MessageB
        Reply::ready()
    })
    .act_on::<MessageC>(|actor, ctx| {
        // Handle MessageC (read-only)
        Reply::ready()
    });

// Now start
let handle = actor.start().await;
```

### Handler Methods

| Method | State Access | Execution | Use Case |
|--------|-------------|-----------|----------|
| `mutate_on` | Mutable | Sequential | State changes |
| `act_on` | Read-only | Concurrent | Queries, notifications |
| `try_mutate_on` | Mutable | Sequential | Fallible state changes |
| `try_act_on` | Read-only | Concurrent | Fallible queries |

See [Handler Types](/docs/handler-types) for detailed coverage of each type.

---

## Handler Anatomy

Every handler receives two arguments:

```rust
actor.mutate_on::<MyMessage>(|actor, ctx| {
    // `actor` - the ManagedActor with state access
    // `ctx` - MessageContext with the message and reply tools

    // Access the actor's state
    let value = actor.model.some_field;

    // Access the incoming message
    let message = ctx.message();

    // Return a Reply
    Reply::ready()
});
```

### The `actor` Parameter

```rust
|actor, ctx| {
    // Access state
    actor.model.count += 1;

    // Get actor's handle (for self-messaging)
    let self_handle = actor.handle().clone();

    // Get actor's ID
    let id = actor.id();

    // Get the broker (for pub/sub)
    let broker = actor.broker();
}
```

### The `ctx` (MessageContext) Parameter

```rust
|actor, ctx| {
    // Get the message
    let msg: &MyMessage = ctx.message();

    // Get mutable message access (if needed)
    let msg_mut: &mut MyMessage = ctx.message_mut();

    // Get message timestamp
    let when: Instant = ctx.timestamp();

    // Get envelope for replying
    let reply = ctx.reply_envelope();
}
```

---

## Sending Messages

Send messages using an actor's handle:

```rust
// Get the handle
let handle = actor.start().await;

// Send a message
handle.send(MyMessage { data: 42 }).await;

// Send from within a handler
actor.mutate_on::<TriggerMessage>(|actor, ctx| {
    let other_handle = actor.model.other_actor.clone();

    Reply::pending(async move {
        other_handle.send(SomeMessage).await;
    })
});
```

### Self-Messaging

Actors can send messages to themselves:

```rust
actor.mutate_on::<StartProcess>(|actor, ctx| {
    let self_handle = actor.handle().clone();

    Reply::pending(async move {
        // Do some work...

        // Then send a message to self for the next step
        self_handle.send(ProcessStep2).await;
    })
});
```

This is useful for breaking up long operations or scheduling work.

---

## Reply Types

Handlers return a `Reply` that indicates completion:

### Immediate Completion

```rust
// No async work needed
actor.mutate_on::<SimpleMessage>(|actor, ctx| {
    actor.model.count += 1;
    Reply::ready()  // Done immediately
});
```

### Async Completion

```rust
// Has async work to do
actor.mutate_on::<AsyncMessage>(|actor, ctx| {
    let data = ctx.message().data.clone();

    Reply::pending(async move {
        // Async work happens here
        tokio::time::sleep(Duration::from_secs(1)).await;
        println!("Processed: {}", data);
    })
});
```

### For Fallible Handlers

```rust
// Immediate success or error
actor.try_mutate_on::<ValidateMessage>(|actor, ctx| {
    if ctx.message().is_valid() {
        Reply::try_ok(ValidationSuccess)
    } else {
        Reply::try_err(ValidationError("Invalid input".into()))
    }
});

// Async with Result
actor.try_mutate_on::<FetchMessage>(|actor, ctx| {
    let url = ctx.message().url.clone();

    Reply::try_pending(async move {
        let response = fetch(&url).await?;
        Ok(FetchSuccess(response))
    })
});
```

---

## Message Flow

Here's what happens when you send a message:

```text
1. handle.send(MyMessage)
   │
   ▼
2. Message wrapped in envelope
   │
   ▼
3. Envelope sent through channel
   │
   ▼
4. Actor receives envelope
   │
   ▼
5. Actor looks up handler by message TypeId
   │
   ├─ Handler found → Execute handler
   │
   └─ No handler → Message dropped (silent)
```

### No Handler = Silent Drop

If you send a message type that an actor doesn't have a handler for, it's silently dropped:

```rust
actor.mutate_on::<Ping>(|_, _| Reply::ready());
// Only Ping is handled

let handle = actor.start().await;
handle.send(Ping).await;    // Handled
handle.send(Pong).await;    // Silently dropped - no handler!
```

This is intentional - it allows flexible message routing without errors.

---

## Message Ordering

Within a single actor:

- **Same message type**: Processed in order sent
- **Different message types**: Processed in order received
- **Mutable handlers**: One at a time, sequential
- **Read-only handlers**: Can run concurrently

```text
Actor inbox: [M1, M2, M3, Q1, Q2, M4]
             └──mutate──┘  └act_on┘  └mutate┘

Execution (mutate_on handlers):
M1 → M2 → M3 → M4  (sequential)

Execution (act_on handlers):
Q1 ┬→ (concurrent)
Q2 ┘
```

---

## Common Patterns

### Request-Reply

```rust
#[acton_message]
struct GetBalance;

#[acton_message]
struct BalanceResponse(f64);

actor.act_on::<GetBalance>(|actor, ctx| {
    let balance = actor.model.balance;
    let reply = ctx.reply_envelope();

    Reply::pending(async move {
        reply.send(BalanceResponse(balance)).await;
    })
});
```

### Command Pattern

```rust
#[acton_message]
enum Command {
    Start,
    Stop,
    Reset,
    SetValue(u32),
}

actor.mutate_on::<Command>(|actor, ctx| {
    match ctx.message() {
        Command::Start => actor.model.running = true,
        Command::Stop => actor.model.running = false,
        Command::Reset => actor.model.value = 0,
        Command::SetValue(v) => actor.model.value = *v,
    }
    Reply::ready()
});
```

### Forwarding Messages

```rust
actor.mutate_on::<TaskRequest>(|actor, ctx| {
    // Pick a worker and forward
    let worker = actor.model.workers.next();
    let task = ctx.message().clone();

    Reply::pending(async move {
        worker.send(task).await;
    })
});
```

---

## Best Practices

### Keep Messages Simple

```rust
// Good: focused message
#[acton_message]
struct UpdatePrice {
    product_id: String,
    new_price: f64,
}

// Avoid: message that does too much
#[acton_message]
struct DoEverything {
    command: String,
    data: HashMap<String, Value>,
    flags: Vec<bool>,
}
```

### Use Descriptive Names

```rust
// Good: clear intent
#[acton_message]
struct OrderPlaced { order_id: String }

#[acton_message]
struct PaymentProcessed { transaction_id: String }

// Avoid: vague names
#[acton_message]
struct Update { id: String }

#[acton_message]
struct Event { data: String }
```

### Prefer Immutable Message Data

```rust
// Good: message data is naturally immutable
#[acton_message]
struct ProcessData {
    items: Vec<Item>,    // Actor will clone if needed
    timestamp: Instant,
}

// The handler receives &Message, can't accidentally modify
```

---

## Next Steps

- [Handler Types](/docs/handler-types) - Deep dive into `mutate_on` vs `act_on`
- [Replies & Context](/docs/replies-and-context) - Full reply and context API
- [Error Handling](/docs/error-handling) - Using fallible handlers
