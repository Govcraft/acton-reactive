---
title: Parent-Child Actors
description: Structuring actors hierarchically for modularity and fault tolerance.
---

Actors rarely work alone. When one actor creates another, they form a parent-child relationship. This hierarchy provides modularity and enables supervision.

## Creating Child Actors

From within a parent actor's handler, use `create_child()` and `supervise()`:

```rust
use acton_reactive::prelude::*;

#[acton_actor]
struct Supervisor {
    workers: Vec<ActorHandle>,
}

#[acton_actor]
struct Worker;

#[acton_message]
struct SpawnWorker;

#[acton_message]
struct Task { id: u32 }

#[acton_main]
async fn main() {
    let mut runtime = ActonApp::launch_async().await;
    let mut builder = runtime.new_actor::<Supervisor>();

    builder.mutate_on::<SpawnWorker>(|actor, _envelope| {
        // Create child actor
        let mut child = actor.create_child("worker".to_string())
            .expect("Failed to create child");

        child.mutate_on::<Task>(|_child, envelope| {
            let task = envelope.message();
            println!("Processing task {}", task.id);
            Reply::ready()
        });

        // Use Reply::pending for async supervision
        let parent_handle = actor.handle().clone();
        Reply::pending(async move {
            let child_handle = parent_handle.supervise(child).await
                .expect("Failed to supervise child");
            // Note: storing handle requires different pattern
            println!("Child started: {}", child_handle.id());
        })
    });

    let supervisor = builder.start().await;
    supervisor.send(SpawnWorker).await;

    // Give time for async work
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    runtime.shutdown_all().await.ok();
}
```

---

## Lifecycle Implications

When a parent stops, its children stop too:

```rust
supervisor.stop().await.ok();  // All worker children also stop
```

This cascade ensures clean shutdown of actor hierarchies.

---

## Communication Patterns

### Parent to Child

The parent keeps handles to children:

```rust
#[acton_actor]
struct Parent {
    children: Vec<ActorHandle>,
}

// Send work to children
for child in &actor.model.children {
    child.send(Task { id: task_id }).await;
}
```

### Child to Parent

Pass the parent's handle when creating the child, or use reply envelopes:

```rust
#[acton_actor]
struct Child {
    parent: ActorHandle,
}

// Child reports back using the stored handle
actor.model.parent.send(TaskComplete { id }).await;
```

Or use the reply envelope pattern for request-response:

```rust
child.act_on::<DoWork>(|actor, envelope| {
    let reply_envelope = envelope.reply_envelope();
    Reply::pending(async move {
        reply_envelope.send(WorkComplete).await;
    })
});
```

---

## When to Use Hierarchies

- **Organizing complex systems** — Group related actors under supervisors
- **Supervision** — Parents are notified when children fail
- **Resource management** — Clean up children when parent stops
- **Load distribution** — Pools of workers under a coordinator

---

## Next

[Request-Response](/docs/building-apps/request-response) — Communication patterns between actors
