---
title: Messages and Handlers
description: How actors communicate through typed messages, and the difference between mutate_on and act_on.
---

Messages are the only way actors interact. This constraint is what makes the actor model powerful: actors exchange data through explicit, typed messages. Rust's type system ensures you can only send messages an actor knows how to handle.

## Defining Messages

A message in Acton is any type marked with `#[acton_message]`:

```rust
use acton_reactive::prelude::*;

#[acton_message]
struct AddItem {
    name: String,
    quantity: u32,
}

#[acton_message]
struct GetTotal;
```

{% callout title="Message Requirements" %}
Messages must implement `Clone` and `Debug`. The `#[acton_message]` attribute handles this automatically.
{% /callout %}

---

## Two Types of Handlers: mutate_on vs act_on

This is the most important concept in Acton.

### mutate_on: Sequential State Changes

Use `mutate_on` when your handler needs to **modify the actor's state**:

```rust
builder.mutate_on::<AddItem>(|actor, msg| {
    actor.model.items.push(msg.name.clone());
    actor.model.total += msg.quantity;
    Reply::ready()
});
```

Handlers with `mutate_on`:
- Get `&mut` access to actor state
- Run **sequentially** - only one runs at a time
- Are guaranteed no concurrent state modifications

### act_on: Concurrent Read-Only Operations

Use `act_on` when your handler only needs to **read state**:

```rust
builder.act_on::<GetTotal>(|actor, _msg| {
    Reply::with(actor.model.total)
});
```

Handlers with `act_on`:
- Get `&` (immutable) access to actor state
- Can run **concurrently** with other `act_on` handlers
- Cannot modify actor state (the compiler enforces this)

---

## Why This Distinction Matters

```rust
// WRONG: Using act_on when you need to mutate
builder.act_on::<Increment>(|actor, _msg| {
    actor.model.counter += 1;  // Compile error!
    Reply::ready()
});

// CORRECT: Use mutate_on for mutations
builder.mutate_on::<Increment>(|actor, _msg| {
    actor.model.counter += 1;  // Works
    Reply::ready()
});
```

**The compiler prevents accidental mutation in concurrent handlers.** Bugs that would be runtime races become compile-time errors.

{% callout type="note" title="Choosing the Right Handler" %}
**Use `mutate_on` when:**
- The handler changes any field in the actor's state
- You need to guarantee the operation completes before other handlers run

**Use `act_on` when:**
- The handler only reads from state
- You want maximum throughput for read operations
{% /callout %}

---

## Replying to Messages

### No Response Needed

```rust
builder.mutate_on::<LogEvent>(|actor, msg| {
    actor.model.events.push(msg.clone());
    Reply::ready()  // Done, no response
});
```

### Return a Value

```rust
builder.act_on::<GetCount>(|actor, _msg| {
    Reply::with(actor.model.count)  // Return the value
});
```

---

## Type Safety

If you try to send a message an actor doesn't handle, you get a compile error:

```rust
// If Counter only handles Increment and GetCount...
counter.send(Reset).await;  // Compile error: no handler for Reset
```

This means:
- **No runtime "message not handled" errors**
- **Refactoring is safe** - change a message type and the compiler finds all uses

---

## Next

[The Actor System](/docs/core-concepts/the-actor-system) - Managing actors with ActonApp
