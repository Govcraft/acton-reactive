---
title: Sending Messages
description: Learn how actors communicate — fire-and-forget with send, request-response with reply envelopes.
---

Actors communicate exclusively through messages. No shared memory, no direct function calls — just messages. This constraint is what makes concurrent programming simpler with actors.

## Fire-and-Forget with Send

You've already used `send` in the previous example:

```rust
handle.send(Increment).await;
```

`send` delivers the message to the actor's mailbox and returns immediately. You're saying: "Here's a message. Handle it when you can. I don't need to know what happens."

### When to Use Send

- Triggering actions that don't return data
- Maximum throughput scenarios
- Fire-and-forget operations

---

## Request-Response with Reply Envelopes

Sometimes you need data back from an actor. Acton Reactive uses the **reply envelope pattern** — the sender provides a return address, and the receiver sends a response back to it.

This pattern requires two actors: one that sends a request and one that responds.

### A Complete Example

```rust
use acton_reactive::prelude::*;

// The service actor that responds to queries
#[acton_actor]
struct Counter {
    count: i32,
}

// The client actor that requests data
#[acton_actor]
#[derive(Default)]
struct Client {
    counter: Option<ActorHandle>,
}

// Messages
#[acton_message]
struct Increment;

#[acton_message]
struct GetCount;

#[acton_message]
struct CountResponse(i32);

#[acton_message]
struct RequestCount;

#[acton_main]
async fn main() {
    let mut runtime = ActonApp::launch_async().await;

    // Create the counter service
    let mut counter = runtime.new_actor::<Counter>();

    counter
        .mutate_on::<Increment>(|actor, _envelope| {
            actor.model.count += 1;
            Reply::ready()
        })
        .act_on::<GetCount>(|actor, envelope| {
            let count = actor.model.count;
            let reply_envelope = envelope.reply_envelope();

            Reply::pending(async move {
                reply_envelope.send(CountResponse(count)).await;
            })
        });

    let counter_handle = counter.start().await;

    // Create the client that will request data
    let mut client = runtime.new_actor::<Client>();
    client.model.counter = Some(counter_handle.clone());

    client
        .mutate_on::<RequestCount>(|actor, envelope| {
            let counter = actor.model.counter.clone().unwrap();
            let request_envelope = envelope.new_envelope(&counter.reply_address());

            Reply::pending(async move {
                request_envelope.send(GetCount).await;
            })
        })
        .act_on::<CountResponse>(|_actor, envelope| {
            let count = envelope.message().0;
            println!("Received count: {}", count);
            Reply::ready()
        });

    let client_handle = client.start().await;

    // Increment the counter a few times
    counter_handle.send(Increment).await;
    counter_handle.send(Increment).await;
    counter_handle.send(Increment).await;

    // Ask for the count via the client
    client_handle.send(RequestCount).await;

    // Give time for async messages to process
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    runtime.shutdown_all().await.ok();
}
```

Output:

```
Received count: 3
```

### Understanding the Pattern

The key insight is that every message arrives in an **envelope** that knows where it came from:

```rust
.act_on::<GetCount>(|actor, envelope| {
    let count = actor.model.count;
    let reply_envelope = envelope.reply_envelope();

    Reply::pending(async move {
        reply_envelope.send(CountResponse(count)).await;
    })
})
```

1. **`envelope.reply_envelope()`** — Creates a new envelope addressed back to whoever sent this message
2. **`Reply::pending(async move { ... })`** — Returns a future that sends the response asynchronously
3. **`reply_envelope.send(CountResponse(count)).await`** — Sends the response back to the sender

### Accessing Message Data

When your message contains data, access it through the envelope:

```rust
#[acton_message]
struct IncrementBy {
    amount: i32,
}

// In handler:
.mutate_on::<IncrementBy>(|actor, envelope| {
    let amount = envelope.message().amount;
    actor.model.count += amount;
    Reply::ready()
})
```

Use `envelope.message()` to get a reference to the message.

---

## Reply Types

### Reply::ready()

Use when processing completes synchronously:

```rust
.mutate_on::<Increment>(|actor, _envelope| {
    actor.model.count += 1;
    Reply::ready()
})
```

### Reply::pending(future)

Use when you need to do async work:

```rust
.act_on::<GetCount>(|actor, envelope| {
    let count = actor.model.count;
    let reply_envelope = envelope.reply_envelope();

    Reply::pending(async move {
        // Async work here
        reply_envelope.send(CountResponse(count)).await;
    })
})
```

The future runs to completion before the next `mutate_on` message is processed.

---

## Choosing Your Pattern

**Use `send` (fire-and-forget) when:**
- You don't need a response
- You want maximum throughput
- The operation is one-way

**Use reply envelopes when:**
- You need data back from another actor
- You're building request-response services
- Actors need to coordinate their work

{% callout title="A Mental Model" %}
Think of `send` like dropping a letter in a mailbox — you walk away immediately.

Think of reply envelopes like including a self-addressed stamped envelope with your letter — you're asking for a response to be sent back to you.
{% /callout %}

---

## What You've Learned

- **`send`** queues a message and returns immediately
- **`envelope.message()`** accesses the message data in a handler
- **`envelope.reply_envelope()`** creates an envelope addressed back to the sender
- **`Reply::ready()`** signals synchronous completion
- **`Reply::pending(future)`** handles async operations

---

## Next Step

You now know the fundamentals: creating actors, defining messages, and communication patterns.

[Next Steps](/docs/quick-start/next-steps) — Where to go from here.
