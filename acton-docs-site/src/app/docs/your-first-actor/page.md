---
title: Your First Actor
nextjs:
  metadata:
    title: Your First Actor - acton-reactive
    description: Build your first actor in 60 seconds. A hands-on introduction to actors, messages, and handlers.
---

Build a working actor in 60 seconds. This tutorial walks you through the essential concepts with real, runnable code.

---

## The Complete Example

Here's a simple counter actor. Don't worry about understanding everything yet - we'll break it down piece by piece.

```rust
use acton_reactive::prelude::*;

// This is our actor's "desk" - its private data
#[acton_actor]
struct CounterState {
    count: u32,
}

// This is a message - a memo telling the counter what to do
#[acton_message]
struct Increment(u32);

#[acton_main]
async fn main() {
    // Start the "office" (runtime)
    let mut runtime = ActonApp::launch_async().await;

    // Hire a counter actor
    let mut counter = runtime.new_actor::<CounterState>();

    // Tell the actor: "When you get an Increment memo, add to your count"
    counter.mutate_on::<Increment>(|actor, ctx| {
        actor.model.count += ctx.message().0;
        println!("Count is now: {}", actor.model.count);
        Reply::ready()
    });

    // The actor starts working
    let handle = counter.start().await;

    // Send some memos!
    handle.send(Increment(1)).await;
    handle.send(Increment(2)).await;
    handle.send(Increment(3)).await;

    // Close up shop
    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

**Output:**
```text
Count is now: 1
Count is now: 3
Count is now: 6
```

That's it! You've built a concurrent, message-driven application. No locks, no mutexes, no data races.

---

## Breaking It Down

### 1. Define the Actor's State

```rust
#[acton_actor]
struct CounterState {
    count: u32,
}
```

An actor's state is just a Rust struct. The `#[acton_actor]` macro derives the required traits (`Default` and `Debug`). This struct holds everything the actor "knows" - its private data that no other actor can touch.

### 2. Define Messages

```rust
#[acton_message]
struct Increment(u32);
```

Messages are how actors communicate. The `#[acton_message]` macro derives `Debug` and `Clone`. You can have as many message types as you need.

### 3. Create the Runtime

```rust
let mut runtime = ActonApp::launch_async().await;
```

The runtime manages all your actors. Think of it as the "office building" where all your actors work.

### 4. Create an Actor

```rust
let mut counter = runtime.new_actor::<CounterState>();
```

This creates an actor *builder*. The actor isn't running yet - we need to configure it first.

### 5. Register a Handler

```rust
counter.mutate_on::<Increment>(|actor, ctx| {
    actor.model.count += ctx.message().0;
    println!("Count is now: {}", actor.model.count);
    Reply::ready()
});
```

Handlers define what happens when a message arrives. Here we're saying: "When you receive an `Increment` message, add its value to the count."

- `actor.model` is the actor's state (`CounterState`)
- `ctx.message()` is the incoming message
- `Reply::ready()` says "I'm done processing"

### 6. Start the Actor

```rust
let handle = counter.start().await;
```

Now the actor is running and ready to receive messages. The `handle` is how we send messages to it.

### 7. Send Messages

```rust
handle.send(Increment(1)).await;
handle.send(Increment(2)).await;
handle.send(Increment(3)).await;
```

Each `send` puts a message in the actor's inbox. The actor processes them one at a time, in order.

### 8. Shut Down

```rust
runtime.shutdown_all().await.expect("Shutdown failed");
```

This stops all actors gracefully, waiting for them to finish processing.

---

## Adding a Query Handler

Let's add the ability to ask for the current count:

```rust
use acton_reactive::prelude::*;

#[acton_actor]
struct CounterState {
    count: u32,
}

#[acton_message]
struct Increment(u32);

#[acton_message]
struct GetCount;

#[acton_message]
struct CountResponse(u32);

#[acton_main]
async fn main() {
    let mut runtime = ActonApp::launch_async().await;
    let mut counter = runtime.new_actor::<CounterState>();

    // Handler for mutations
    counter.mutate_on::<Increment>(|actor, ctx| {
        actor.model.count += ctx.message().0;
        Reply::ready()
    });

    // Handler for queries (read-only)
    counter.act_on::<GetCount>(|actor, ctx| {
        let count = actor.model.count;
        let reply = ctx.reply_envelope();

        Reply::pending(async move {
            reply.send(CountResponse(count)).await;
        })
    });

    let handle = counter.start().await;

    handle.send(Increment(5)).await;
    handle.send(Increment(3)).await;
    handle.send(GetCount).await;

    // Give time for the reply to process
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

Notice we used `act_on` instead of `mutate_on`. That's because `GetCount` only *reads* the state - it doesn't change it. This distinction matters for [concurrency](/docs/handler-types).

---

## What's Next?

Now that you've built your first actor:

- [Actors & State](/docs/actors-and-state) - Understand the actor model in depth
- [Messages & Handlers](/docs/messages-and-handlers) - Learn the full messaging API
- [Handler Types](/docs/handler-types) - When to use `mutate_on` vs `act_on`
- [Examples](/docs/examples) - See more complete applications
