---
title: Supervision Basics
description: How Acton keeps your application running when actors fail.
---

When actors fail, supervision ensures your system keeps running. Instead of letting one error crash everything, Acton contains failures and gives you the building blocks for organized recovery.

## The Problem Supervision Solves

In traditional programs, an unhandled error often crashes the whole process. With actors, failures are isolated — if one actor fails, others continue normally.

Supervision adds organized recovery on top of this isolation. Acton's approach is explicit: the framework delivers termination notifications and provides decision helpers (`SupervisionStrategy`, `RestartPolicy`), and **your parent actor decides what to do** — Acton does not restart actors automatically.

---

## Parent-Child Relationships

When one actor supervises another, they form a parent-child relationship. The parent monitors its children and decides what happens when they fail.

### Creating Supervised Children

Use `supervise()` to register a child under a parent's supervision:

```rust
let mut runtime = ActonApp::launch_async().await;

// Create and start the parent
let parent = runtime.new_actor::<ParentState>();
let parent_handle = parent.start().await;

// Create a child that knows its parent
let config = ActorConfig::new(
    Ern::with_root("worker")?,
    Some(parent_handle.clone()),  // parent reference
    None,
)?;
let mut child = runtime.new_actor_with_config::<ChildState>(config);
child.mutate_on::<Task>(|actor, _ctx| {
    // Handle task
    Reply::ready()
});

// Parent supervises the child (starts it and registers the relationship)
let child_handle = parent_handle.supervise(child).await?;
```

The `supervise()` method:
1. **Starts** the child actor
2. **Registers** it in the parent's children map (so it stops when the parent stops)
3. **Returns** the child's handle for sending messages

The parent reference in `ActorConfig` is what makes the child a real child: it gives the child a hierarchical identifier (e.g., `parent/worker`) and is required for the child to notify the parent when it terminates. A child created with plain `new_actor()` and then passed to `supervise()` will still stop with the parent, but it will never send a `ChildTerminated` notification. (An actor that is still in the `Idle` state — before `start()` — can also call `create_child()`, which wires up the parent and broker for a child of the same state type.)

---

## What Happens When an Actor Fails

When a child actor terminates:

1. **The child stops** processing messages
2. **The parent is notified** via a `ChildTerminated` message (if the child was created with a parent reference) containing:
   - Which child terminated (`child_id`)
   - Why it terminated (`TerminationReason`: `Normal`, `InboxClosed`, `ParentShutdown`, ...)
   - The child's restart policy
3. **Your parent actor decides what to do** — register a `mutate_on::<ChildTerminated>` handler and recreate the child, escalate, or move on. `SupervisionStrategy::decide()` turns the notification into a `SupervisionDecision` for you, but acting on it is your code's job.
4. **Children stop** when their parent stops (cascading shutdown)

```rust
parent.mutate_on::<ChildTerminated>(|actor, ctx| {
    let note = ctx.message();
    match SupervisionStrategy::OneForOne.decide(note, 0) {
        SupervisionDecision::RestartChild => {
            // Recreate and re-supervise the child here
        }
        _ => { /* log, escalate, or ignore */ }
    }
    Reply::ready()
});
```

This gives you fine-grained control over failure recovery.

---

## Supervision Strategies

Acton provides three Erlang/OTP-style strategies. A strategy is a **decision helper**: calling `strategy.decide(&notification, child_index)` in your `ChildTerminated` handler tells you which children should be restarted, and your handler carries it out.

### OneForOne (Default)

Restart only the failed child. Other children continue running.

```rust
use acton_reactive::prelude::*;

let config = ActorConfig::new(
    Ern::with_root("supervisor")?,
    None,
    None,
)?
.with_supervision_strategy(SupervisionStrategy::OneForOne);
```

**Use when**: Children are independent and their failures don't affect each other.

### OneForAll

Restart all children when any child fails. This ensures all children start from a consistent state.

```rust
.with_supervision_strategy(SupervisionStrategy::OneForAll)
```

**Use when**: Children are interdependent and one child's failure could leave others in an inconsistent state.

### RestForOne

Restart the failed child and all children started after it, preserving start order.

```rust
.with_supervision_strategy(SupervisionStrategy::RestForOne)
```

**Use when**: Children have sequential dependencies (later children depend on earlier ones).

---

## Restart Policies

Each child actor has a restart policy that travels with its `ChildTerminated` notification. `SupervisionStrategy::decide()` consults it to determine whether the child should be restarted:

### Permanent (Default)

Should always be restarted when it terminates (except during parent shutdown).

```rust
let config = ActorConfig::new(
    Ern::with_root("worker")?,
    Some(parent_handle.clone()),
    None,
)?
.with_restart_policy(RestartPolicy::Permanent);
```

**Use for**: Critical services that must always be running.

### Temporary

Should never be restarted when it terminates.

```rust
.with_restart_policy(RestartPolicy::Temporary)
```

**Use for**: One-time operations or when the caller handles failures explicitly.

### Transient

Should be restarted only on abnormal termination (e.g., an unexpectedly closed inbox), not on normal shutdown.

```rust
.with_restart_policy(RestartPolicy::Transient)
```

**Use for**: Workers that may complete normally but should restart on unexpected failures.

{% callout type="note" title="Panics don't terminate actors by default" %}
With the default `catch-handler-panics` feature enabled, a panicking handler is caught and logged and the actor keeps running — a panic doesn't trigger the supervision flow at all.
{% /callout %}

---

## When to Use Supervision

### Good Use Cases

**Transient failures:**
- Network blips
- Temporary resource exhaustion
- Race conditions

**Stateless workers:**
- Request handlers
- Image processors
- Log forwarders

### When NOT to Use Supervision

{% callout type="warning" title="Persist Critical State" %}
Never rely on actor memory for data that must survive failures. Use:
- Database writes for durability
- Event sourcing for recovery
- External state stores

A recreated actor starts fresh — any in-memory state is lost.
{% /callout %}

---

## Best Practices

### Return Errors, Don't Panic

Handlers should handle expected failures gracefully:

```rust
builder.mutate_on::<ProcessOrder>(|actor, envelope| {
    let msg = envelope.message();
    match process(&msg) {
        Ok(_) => Reply::ready(),
        Err(e) => {
            tracing::error!("Order failed: {}", e);
            Reply::ready()  // Handle gracefully
        }
    }
});
```

### Design for Restart

Assume your actor might restart at any time. Keep minimal state — restore from external sources when needed.

### Match Strategy to Dependencies

| Pattern | Strategy | Policy |
|---------|----------|--------|
| Independent workers | OneForOne | Permanent |
| Interdependent services | OneForAll | Permanent |
| Pipeline stages | RestForOne | Permanent |
| One-time tasks | OneForOne | Temporary |
| Optional services | OneForOne | Transient |

---

## Summary

- Parent actors supervise children registered with `supervise()`; create children with a parent reference (or `create_child()`) so termination notifications flow
- Failures are isolated to individual actors
- Restarts are **your code's responsibility**: handle `ChildTerminated` in the parent and recreate children as needed
- **Supervision strategies** decide which children should restart (OneForOne, OneForAll, RestForOne)
- **Restart policies** decide whether a child should restart (Permanent, Temporary, Transient)
- Children stop when their parent stops
- Critical state should be persisted externally

---

## Continue Learning

You now understand the core concepts of Acton:
- **Actors** as independent workers
- **Messages and Handlers** with type-safe routing
- **The Actor System** for management
- **Supervision** for fault tolerance

For custom recovery logic and the `RestartLimiter` helper (a rate-limiting building block you can call from your own supervision handler), see [Custom Supervision](/docs/advanced/custom-supervision).

Continue to [Building Apps](/docs/building-apps/parent-child-actors) for practical patterns.
