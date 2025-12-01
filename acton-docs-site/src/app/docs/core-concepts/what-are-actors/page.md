---
title: What Are Actors?
description: Understand the actor model - a simpler way to think about concurrent code.
---

Actors are independent workers that communicate through messages. Instead of sharing memory and coordinating access with locks, each actor owns its data and interacts only by sending and receiving messages.

This simple model makes concurrent programming dramatically easier.

## The Workers with Inboxes Mental Model

Think of actors like workers in an office:

- Each has their own **desk** (private state nobody else can touch)
- Each has an **inbox** where messages arrive
- Each processes messages **one at a time** (no interruptions mid-task)
- Each can **send memos** to other workers' inboxes

No worker ever reaches across to rummage through another worker's desk. All coordination happens through messages.

---

## The Three Things Actors Do

Every actor does exactly three things:

### 1. Receive Messages

When a message arrives, the actor wakes up to handle it.

### 2. Update State

While handling a message, an actor can modify its private state. Because messages are processed one at a time, there's no risk of data races.

### 3. Send Messages

Actors communicate with other actors by sending messages to their handles.

---

## Why This Matters: No Shared State Bugs

Consider traditional concurrent programming:

{% callout type="warning" title="The Shared State Problem" %}
```rust
// Traditional threading - careful coordination needed
let counter = Arc::new(Mutex::new(0));
let counter1 = counter.clone();

thread::spawn(move || {
    let mut val = counter1.lock().unwrap();
    *val += 1;  // Need the lock!
});
```

With mutexes, you need to think about lock ordering, deadlocks, and whether you've protected all the shared state.
{% /callout %}

With actors, this complexity vanishes:

```rust
// Actor model - messages processed one at a time
builder.mutate_on::<Increment>(|actor, _msg| {
    actor.model.count += 1;  // Always safe
    Reply::ready()
});
```

When two `Increment` messages arrive, they're processed sequentially. No locks, no races.

---

## Actors Are Lightweight

Unlike operating system threads, actors are cheap. They're Rust structs with a message queue, running on Tokio's async runtime.

You can create thousands:

```rust
for user_id in 0..10_000 {
    app.spawn(UserSession::new(user_id)).await;
}
```

---

## Actors Are Isolated

Each actor runs independently. If one encounters an error, others continue normally. The supervision system handles failures gracefully.

This means:
- **Failures are contained** - one broken actor doesn't crash your system
- **State is protected** - no actor can corrupt another's data
- **Testing is simpler** - test actors in isolation

---

{% callout title="For Experienced Developers" %}
If you've used Actix or other actor frameworks, Acton's approach will feel familiar with key differences:

- **No async_trait boilerplate** - Handlers use `mutate_on` and `act_on` attributes
- **Compile-time message routing** - The type system ensures valid message sends
- **Tokio-native** - Built directly on Tokio, not a separate runtime

The `mutate_on` vs `act_on` distinction is key - it determines sequential vs concurrent handler execution.
{% /callout %}

---

## Next

[Messages and Handlers](/docs/core-concepts/messages-and-handlers) - The crucial difference between `mutate_on` and `act_on`
