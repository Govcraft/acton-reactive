---
title: Your First Actor
description: Build a working actor in about 5 minutes. See the complete code first, then understand each piece.
---

Let's build your first actor. By the end of this page, you'll have a working counter that responds to messages - and you'll understand how it works.

**Time needed:** About 5 minutes.

## The Complete Example

Here's the entire program. Take a look at it first, then we'll break it down piece by piece:

```rust
use acton_reactive::prelude::*;

// 1. Define your actor's data
#[acton_actor]
struct Counter {
    count: i32,
}

// 2. Define messages it can receive
#[acton_message]
struct Increment;

#[acton_message]
struct PrintCount;

// 3. Wire it all together
#[acton_main]
async fn main() {
    // Start the actor system
    let mut app = ActonApp::launch();

    // Create and configure the actor
    let mut builder = app.new_actor::<Counter>();

    builder
        .mutate_on::<Increment>(|actor, _msg| {
            actor.model.count += 1;
            Reply::ready()
        })
        .act_on::<PrintCount>(|actor, _msg| {
            println!("Current count: {}", actor.model.count);
            Reply::ready()
        });

    let counter = builder.start().await;

    // Send some messages
    counter.send(Increment).await;
    counter.send(Increment).await;
    counter.send(Increment).await;
    counter.send(PrintCount).await;

    // Clean shutdown
    app.shutdown_all().await.ok();
}
```

Run it:

```shell
cargo run
```

Output:

```
Current count: 3
```

**You just built a working actor.** It received three increment messages and printed the result. That wasn't so hard, was it?

---

## Understanding Each Part

Now let's understand what each piece does.

### Part 1: The Actor's Data

```rust
#[acton_actor]
struct Counter {
    count: i32,
}
```

An actor is just a struct with `#[acton_actor]` on top. This struct holds the actor's **state** - the data it owns and manages.

Our `Counter` has one field: a number. Actors can hold anything: configuration, cached data, connections, collections - whatever your actor needs to do its job.

{% callout title="Why Actors?" %}
Each actor owns its data exclusively. No other code can directly access `count`. The only way to interact with an actor is through messages. This eliminates entire categories of bugs that plague concurrent programs.
{% /callout %}

### Part 2: Messages

```rust
#[acton_message]
struct Increment;

#[acton_message]
struct PrintCount;
```

Messages are how the outside world talks to actors. The `#[acton_message]` attribute makes a struct usable as a message.

These are simple messages with no data. Messages can also carry data:

```rust
#[acton_message]
struct IncrementBy {
    amount: i32,
}
```

Think of messages as requests: "Please increment yourself" or "Please print your count."

### Part 3: The Actor System

```rust
let mut app = ActonApp::launch();
```

`ActonApp` is your actor system - it manages the lifecycle of all actors in your application. You start it once at the beginning of your program.

### Part 4: Creating and Configuring the Actor

```rust
let mut builder = app.new_actor::<Counter>();

builder
    .mutate_on::<Increment>(|actor, _msg| {
        actor.model.count += 1;
        Reply::ready()
    })
    .act_on::<PrintCount>(|actor, _msg| {
        println!("Current count: {}", actor.model.count);
        Reply::ready()
    });

let counter = builder.start().await;
```

This is where the magic happens:

1. **`new_actor::<Counter>()`** - Create a builder for a Counter actor
2. **`.mutate_on::<Increment>(...)`** - Register a handler that *changes* the actor's state
3. **`.act_on::<PrintCount>(...)`** - Register a handler that *reads* the actor's state
4. **`.start().await`** - Start the actor and get a handle to it

#### What's `Reply::ready()`?

Every handler returns a `Reply`. For now, just use `Reply::ready()` - it means "I'm done processing this message." You'll learn more in the next section.

#### mutate_on vs act_on

- **`mutate_on`** - For messages that *change* the actor's state
- **`act_on`** - For messages that only *read* the state

This distinction helps Acton Reactive keep your code safe and fast.

### Part 5: Sending Messages

```rust
counter.send(Increment).await;
counter.send(PrintCount).await;
```

The `counter` variable is a **handle** to your actor. Use it to send messages with `.send()`.

Messages are processed one at a time, in order. So after three `Increment` messages, the `PrintCount` sees a count of 3.

{% callout type="note" title="Fire and Forget" %}
`.send()` is "fire and forget" - it queues the message and returns immediately. The actor processes it asynchronously.
{% /callout %}

### Part 6: Shutdown

```rust
app.shutdown_all().await.ok();
```

Good practice: shut down your actors cleanly when you're done.

---

## Try It Yourself

Modify the example:

1. **Add more messages** - Send 10 increments instead of 3
2. **Add a new message type** - Try adding a `Decrement` message with its own `mutate_on` handler

---

## What You've Learned

- Actors are structs marked with `#[acton_actor]`
- Messages are structs marked with `#[acton_message]`
- Handlers connect messages to behavior: `mutate_on` for changes, `act_on` for reads
- Every handler returns `Reply::ready()` when done
- You send messages to actors through handles

---

## Next Step

What if you need a response back from an actor? That's where `send` vs `ask` comes in.

[Sending Messages](/docs/quick-start/sending-messages) - Learn about fire-and-forget vs request-response patterns.
