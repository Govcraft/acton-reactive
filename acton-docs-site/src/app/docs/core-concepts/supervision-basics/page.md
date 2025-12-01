---
title: Supervision Basics
description: How Acton keeps your application running when actors fail.
---

When actors fail, supervision ensures your system keeps running. Instead of letting one error crash everything, Acton contains failures and provides strategies for recovery.

## The Problem Supervision Solves

In traditional programs, an unhandled error often crashes the whole process. With actors, failures are isolated - if one actor fails, others continue normally.

Supervision adds organized recovery on top of this isolation.

---

## Parent-Child Relationships

When one actor spawns another, they form a parent-child relationship:

```rust
// Inside a parent actor's handler
let child = actor.spawn_child::<Worker>()
    .mutate_on::<Task>(handle_task)
    .start()
    .await;
```

When a child fails, the parent can decide what to do.

---

## What Happens When an Actor Fails

1. **The actor stops** processing messages
2. **The parent is notified**
3. **A supervision strategy is applied**

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

Supervision restarts actors fresh - any in-memory state is lost.
{% /callout %}

---

## Best Practices

### Return Errors, Don't Panic

Handlers should return `Result` for expected failures:

```rust
builder.mutate_on::<ProcessOrder>(|actor, msg| {
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

Assume your actor might restart at any time. Keep minimal state - restore from external sources when needed.

---

## Summary

- Parent actors supervise children
- Failures are isolated to individual actors
- Supervision strategies determine recovery behavior
- Critical state should be persisted externally

---

## Continue Learning

You now understand the core concepts of Acton:
- **Actors** as independent workers
- **Messages and Handlers** with type-safe routing
- **The Actor System** for management
- **Supervision** for fault tolerance

Continue to [Building Apps](/docs/building-apps/parent-child-actors) for practical patterns.
