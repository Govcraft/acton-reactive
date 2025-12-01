---
title: Sending Messages
description: Learn how actors communicate - fire-and-forget with send, request-response with ask.
---

Actors communicate exclusively through messages. No shared memory, no direct function calls - just messages. This is what makes concurrent programming so much simpler with actors.

You'll learn the two ways to send messages: **send** (fire and forget) and **ask** (request and response).

## Two Ways to Communicate

| Method | Use When | Returns | Waits? |
|--------|----------|---------|--------|
| `send` | You don't need a response | Nothing | No |
| `ask` | You need data back | A value | Yes |

---

## Send: Fire and Forget

You've already used `send` in the previous example:

```rust
counter.send(Increment).await;
```

`send` delivers the message to the actor's mailbox and returns immediately. You're saying: "Here's a message. Handle it when you can. I don't need to know what happens."

### When to Use Send

- You're triggering an action that doesn't return data
- You want maximum throughput
- You're doing "fire and forget" operations

---

## Ask: Request and Response

Sometimes you need data back from an actor. That's what `ask` is for.

Let's extend our counter to support queries:

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

#[acton_main]
async fn main() {
    let mut app = ActonApp::launch();
    let mut builder = app.new_actor::<Counter>();

    builder
        .mutate_on::<Increment>(|actor, _msg| {
            actor.model.count += 1;
            Reply::ready()
        })
        .act_on::<GetCount>(|actor, _msg| {
            Reply::with(actor.model.count)
        });

    let counter = builder.start().await;

    // Send some increments (fire and forget)
    counter.send(Increment).await;
    counter.send(Increment).await;
    counter.send(Increment).await;

    // Ask for the current count (wait for response)
    let result: i32 = counter.ask(GetCount).await;
    println!("The count is: {}", result);

    app.shutdown_all().await.ok();
}
```

Output:

```
The count is: 3
```

### How Ask Works

```rust
let result: i32 = counter.ask(GetCount).await;
```

`ask` does three things:

1. Sends the message to the actor
2. Waits for the actor to process it
3. Returns the response

### Returning Values with Reply::with

In the handler:

```rust
.act_on::<GetCount>(|actor, _msg| {
    Reply::with(actor.model.count)
})
```

`Reply::with(value)` wraps your return value. The type is inferred from how you use the result.

### When to Use Ask

- You need data back from the actor
- You need to wait for an operation to complete
- You're implementing request/response patterns

---

## Complete Example: Task Manager

Here's a more complete example showing both patterns:

```rust
use acton_reactive::prelude::*;

#[acton_actor]
#[derive(Default)]
struct TaskManager {
    tasks: Vec<String>,
}

#[acton_message]
struct AddTask {
    description: String,
}

#[acton_message]
struct GetTaskCount;

#[acton_main]
async fn main() {
    let mut app = ActonApp::launch();
    let mut builder = app.new_actor::<TaskManager>();

    builder
        .mutate_on::<AddTask>(|actor, msg| {
            actor.model.tasks.push(msg.description.clone());
            println!("Added: {}", msg.description);
            Reply::ready()
        })
        .act_on::<GetTaskCount>(|actor, _msg| {
            Reply::with(actor.model.tasks.len())
        });

    let manager = builder.start().await;

    // Add tasks (fire and forget)
    manager.send(AddTask { description: "Learn Acton".into() }).await;
    manager.send(AddTask { description: "Build something cool".into() }).await;

    // Query the count (we need this data)
    let count: usize = manager.ask(GetTaskCount).await;
    println!("You have {} tasks", count);

    app.shutdown_all().await.ok();
}
```

Output:

```
Added: Learn Acton
Added: Build something cool
You have 2 tasks
```

---

## Choosing Between Send and Ask

**Use `send` by default.** It's faster and keeps your code non-blocking.

**Use `ask` when you genuinely need the response** before you can proceed.

{% callout title="A Mental Model" %}
Think of `send` like dropping a letter in a mailbox - you walk away immediately.

Think of `ask` like a phone call - you wait on the line for an answer.

Both are useful. Choose based on whether you need that answer.
{% /callout %}

---

## What You've Learned

- **`send`** queues a message and returns immediately
- **`ask`** sends a message and waits for a response
- Use `Reply::ready()` when no response is needed
- Use `Reply::with(value)` to return data from a handler

---

## Next Step

You now know the fundamentals: creating actors, defining messages, and communication patterns.

[Next Steps](/docs/quick-start/next-steps) - Where to go from here.
