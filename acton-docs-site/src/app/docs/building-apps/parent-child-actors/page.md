---
title: Parent-Child Actors
description: Structuring actors hierarchically for modularity and fault tolerance.
---

Actors rarely work alone. When one actor spawns another, they form a parent-child relationship. This hierarchy provides modularity and enables supervision.

## Creating Child Actors

From within a parent actor's handler, spawn children:

```rust
use acton_reactive::prelude::*;

#[acton_actor]
struct Supervisor {
    workers: Vec<AgentHandle>,
}

#[acton_message]
struct SpawnWorker;

#[acton_main]
async fn main() {
    let mut app = ActonApp::launch();
    let mut builder = app.new_actor::<Supervisor>();

    builder.mutate_on::<SpawnWorker>(|actor, _msg| {
        // Spawn a child worker
        let child = actor.spawn_child::<Worker>()
            .mutate_on::<Task>(handle_task)
            .start();

        actor.model.workers.push(child);
        Reply::ready()
    });

    let supervisor = builder.start().await;
    supervisor.send(SpawnWorker).await;

    app.shutdown_all().await.ok();
}
```

---

## Lifecycle Implications

When a parent stops, its children stop too:

```rust
supervisor.stop().await;  // All worker children also stop
```

This cascade ensures clean shutdown of actor hierarchies.

---

## Communication Patterns

### Parent to Child

The parent keeps handles to children:

```rust
#[acton_actor]
struct Parent {
    children: Vec<AgentHandle>,
}

// Send work to children
for child in &actor.model.children {
    child.send(Task { id: task_id }).await;
}
```

### Child to Parent

Pass the parent's handle when spawning:

```rust
#[acton_actor]
struct Child {
    parent: AgentHandle,
}

// Child reports back
actor.model.parent.send(TaskComplete { id }).await;
```

---

## Example: Worker Pool

```rust
use acton_reactive::prelude::*;

#[acton_actor]
struct Pool {
    workers: Vec<AgentHandle>,
    next_worker: usize,
}

#[acton_message]
struct Initialize { worker_count: usize }

#[acton_message]
struct SubmitWork { data: String }

#[acton_main]
async fn main() {
    let mut app = ActonApp::launch();
    let mut builder = app.new_actor::<Pool>();

    builder
        .mutate_on::<Initialize>(|actor, msg| {
            for _ in 0..msg.worker_count {
                let worker = actor.spawn_child::<Worker>()
                    .mutate_on::<Work>(process_work)
                    .start();
                actor.model.workers.push(worker);
            }
            Reply::ready()
        })
        .mutate_on::<SubmitWork>(|actor, msg| {
            // Round-robin distribution
            let worker = &actor.model.workers[actor.model.next_worker];
            worker.send(Work { data: msg.data.clone() });

            actor.model.next_worker =
                (actor.model.next_worker + 1) % actor.model.workers.len();
            Reply::ready()
        });

    let pool = builder.start().await;
    pool.send(Initialize { worker_count: 4 }).await;
    pool.send(SubmitWork { data: "task1".into() }).await;

    app.shutdown_all().await.ok();
}
```

---

## When to Use Hierarchies

- **Organizing complex systems** - Group related actors under supervisors
- **Supervision** - Parents can restart failed children
- **Resource management** - Clean up children when parent stops
- **Load distribution** - Pools of workers under a coordinator

---

## Next

[Request-Response](/docs/building-apps/request-response) - Complex communication patterns
