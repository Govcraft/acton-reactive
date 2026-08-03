---
title: Sending Messages
description: Learn how actors communicate, from fire-and-forget send to request-response with ask.
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

## Request-Response with ask

Sometimes you need data back from an actor. `ask` sends a request and waits for the reply:

```rust
let count = counter_handle.ask(GetCount).await?;
println!("Received count: {}", count.0);
```

That is the whole call. There is no second actor, no return address to wire up, and nothing to wait on afterwards.

### Making a message askable

A message becomes askable by implementing `Request`, which names the reply through an associated type:

```rust
#[acton_message]
struct GetCount;

#[acton_message]
struct CountResponse(i32);

impl Request for GetCount {
    type Response = CountResponse;
}
```

Because the reply type is pinned to the request type, the call needs no turbofish, and asking for the wrong reply type is a compile error.

### A Complete Example

```rust
use acton_reactive::prelude::*;

#[acton_actor]
struct Counter {
    count: i32,
}

#[acton_message]
struct Increment;

#[acton_message]
struct GetCount;

#[acton_message]
struct CountResponse(i32);

impl Request for GetCount {
    type Response = CountResponse;
}

#[acton_main]
async fn main() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;

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

    counter_handle.send(Increment).await;
    counter_handle.send(Increment).await;
    counter_handle.send(Increment).await;

    let count = counter_handle.ask(GetCount).await?;
    println!("Received count: {}", count.0);

    runtime.shutdown_all().await?;
    Ok(())
}
```

Output:

```
Received count: 3
```

Note that the three `Increment` messages are not waited on, and do not need to be. **Inboxes are FIFO, so a completed `ask` also proves every message sent to that actor beforehand has been processed.**

{% callout type="note" title="This is how you avoid sleeping" %}
If you find yourself reaching for `tokio::time::sleep` to let messages "finish processing", `ask` the actor instead. A sleep is a guess about scheduling; a reply is a fact.
{% /callout %}

### Understanding the Reply Envelope

`ask` changes nothing on the handler side. Every message arrives in an **envelope** that knows where it came from, and the handler answers through it:

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

The actor cannot tell an `ask` from a `send`. A handler that returns without replying is legal; the caller gets `AskError::NoReply` rather than waiting forever.

### When the reply envelope is still the right tool

`ask` resolves on the **first** reply, and asking from inside a `mutate_on` handler deadlocks, because a mutable handler is awaited on the actor's own message loop. For those cases, an actor sends the request with `send` and handles the answer as an ordinary message. See [Request-Response](/docs/building-apps/request-response).

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

**How the future is driven depends on the handler.** A `mutate_on` handler's future is awaited **inline**, so the actor takes no further messages until it completes. An `act_on` handler's future is drained **concurrently**, so the actor can move on to the next message while it is still running.

That difference matters when you are reasoning about ordering: a reply from an `act_on` handler does not prove the handler's async work has finished, only that the reply was sent.

---

## Choosing Your Pattern

**Use `send` (fire-and-forget) when:**
- You don't need a response
- You want maximum throughput
- The operation is one-way

**Use `ask` when:**
- You need data back from an actor
- You need to know a message has been processed
- You're calling from `main`, a task, or a test

**Use reply envelopes directly when:**
- One request produces several replies
- The responder is answering a peer actor rather than a caller
- You are inside a `mutate_on` handler, where asking would deadlock

{% callout title="A Mental Model" %}
Think of `send` like dropping a letter in a mailbox: you walk away immediately.

Think of `ask` like a phone call: you wait on the line for the answer.

Think of the reply envelope as the self-addressed stamped envelope inside the letter. It is what the receiver writes back to, and `ask` simply supplies one whose return address is a private channel it is waiting on.
{% /callout %}

---

## What You've Learned

- **`send`** queues a message and returns immediately
- **`ask`** sends a request and waits for the reply, which also proves everything sent beforehand was processed
- **`Request`** names a message's reply type, which is what makes it askable
- **`envelope.message()`** accesses the message data in a handler
- **`envelope.reply_envelope()`** creates an envelope addressed back to the sender
- **`Reply::ready()`** signals synchronous completion
- **`Reply::pending(future)`** handles async operations

---

## Next Step

You now know the fundamentals: creating actors, defining messages, and communication patterns.

[Next Steps](/docs/quick-start/next-steps) — Where to go from here.
