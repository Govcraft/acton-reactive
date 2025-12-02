---
title: Supervision Basics
description: How Acton keeps your application running when actors fail.
---

When actors fail, supervision ensures your system keeps running. Instead of letting one error crash everything, Acton contains failures and provides strategies for recovery.

## The Problem Supervision Solves

In traditional programs, an unhandled error often crashes the whole process. With actors, failures are isolated — if one actor fails, others continue normally.

Supervision adds organized recovery on top of this isolation.

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

// Create and configure a child
let mut child = runtime.new_actor::<ChildState>();
child.mutate_on::<Task>(|actor, _ctx| {
    // Handle task
    Reply::ready()
});

// Parent supervises the child (starts it and registers the relationship)
let child_handle = parent_handle.supervise(child).await?;
```

The `supervise()` method:
1. **Starts** the child actor
2. **Registers** it under the parent's supervision
3. **Returns** the child's handle for sending messages

Children inherit their parent's broker and have hierarchical identifiers (e.g., `parent/worker`).

---

## What Happens When an Actor Fails

When a child actor terminates:

1. **The child stops** processing messages
2. **The parent is notified** via a `ChildTerminated` message containing:
   - Which child terminated
   - Why it terminated (panic, normal shutdown, etc.)
   - The child's restart policy
3. **The parent makes a decision** based on its supervision strategy and the child's restart policy
4. **Children stop** when their parent stops (cascading shutdown)

This gives you fine-grained control over failure recovery.

---

## Supervision Strategies

Acton provides three Erlang/OTP-style strategies that determine how a parent responds when a child fails:

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

Each child actor has a restart policy that determines whether it should be restarted when it terminates:

### Permanent (Default)

Always restart the actor when it terminates (except during parent shutdown).

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

Never restart the actor when it terminates.

```rust
.with_restart_policy(RestartPolicy::Temporary)
```

**Use for**: One-time operations or when the caller handles failures explicitly.

### Transient

Restart only on abnormal termination (panic, inbox closed). Don't restart on normal shutdown.

```rust
.with_restart_policy(RestartPolicy::Transient)
```

**Use for**: Workers that may complete normally but should restart on unexpected failures.

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

Supervision restarts actors fresh — any in-memory state is lost.
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

- Parent actors supervise children registered with `supervise()`
- Failures are isolated to individual actors
- **Supervision strategies** control which children restart (OneForOne, OneForAll, RestForOne)
- **Restart policies** control whether a child restarts (Permanent, Temporary, Transient)
- Children stop when their parent stops
- Critical state should be persisted externally

---

## Continue Learning

You now understand the core concepts of Acton:
- **Actors** as independent workers
- **Messages and Handlers** with type-safe routing
- **The Actor System** for management
- **Supervision** for fault tolerance

For advanced supervision features like restart limiting and custom recovery logic, see [Custom Supervision](/docs/advanced/custom-supervision).

Continue to [Building Apps](/docs/building-apps/parent-child-actors) for practical patterns.
