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

When one actor creates another, they form a parent-child relationship. The parent supervises its children.

### Creating Child Actors

Use `create_child()` to create a child actor, then `supervise()` to start and register it:

```rust
// Inside a parent actor's handler
let mut child = actor.create_child("worker".to_string())?;

child.mutate_on::<Task>(|child_actor, envelope| {
    // Handle task
    Reply::ready()
});

// Start and register the child
let child_handle = actor.handle().supervise(child).await?;
```

The two-step process:
1. **`create_child(name)`** — Creates a child actor builder with hierarchical naming
2. **`supervise(child)`** — Starts the child and registers it under the parent

Children inherit their parent's broker and have hierarchical identifiers (e.g., `parent/worker`).

---

## What Happens When an Actor Fails

1. **The actor stops** processing messages
2. **The parent is notified**
3. **Children are stopped** when their parent stops

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

---

## Summary

- Parent actors supervise children created with `create_child()` and `supervise()`
- Failures are isolated to individual actors
- Children stop when their parent stops
- Critical state should be persisted externally

---

## Continue Learning

You now understand the core concepts of Acton:
- **Actors** as independent workers
- **Messages and Handlers** with type-safe routing
- **The Actor System** for management
- **Supervision** for fault tolerance

Continue to [Building Apps](/docs/building-apps/parent-child-actors) for practical patterns.
