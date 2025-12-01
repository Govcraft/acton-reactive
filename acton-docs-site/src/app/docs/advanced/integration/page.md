---
title: Integration
description: Working with databases, HTTP, and the async Rust ecosystem.
---

Acton Reactive is built on Tokio and integrates naturally with the async Rust ecosystem.

## Async Operations in Handlers

Use `Reply::pending` for handlers that need to await:

```rust
builder.act_on::<FetchData>(|actor, msg| {
    let url = msg.url.clone();

    Reply::pending(async move {
        let response = reqwest::get(&url).await.unwrap();
        let body = response.text().await.unwrap();
        body
    })
});
```

---

## Database Connections

Store connection pools in actor state:

```rust
use sqlx::PgPool;

#[acton_actor]
struct DatabaseActor {
    pool: Option<PgPool>,
}

#[acton_message]
struct Initialize { database_url: String }

#[acton_message]
struct Query { sql: String }

builder.mutate_on::<Initialize>(|actor, msg| {
    let url = msg.database_url.clone();

    Reply::pending(async move {
        let pool = PgPool::connect(&url).await.unwrap();
        // Store pool in actor state somehow
        pool
    })
});

builder.act_on::<Query>(|actor, msg| {
    let pool = actor.model.pool.clone().unwrap();
    let sql = msg.sql.clone();

    Reply::pending(async move {
        let rows = sqlx::query(&sql)
            .fetch_all(&pool)
            .await
            .unwrap();
        rows
    })
});
```

---

## HTTP Servers

Actors can handle HTTP requests:

```rust
use axum::{Router, routing::get, extract::State};

#[acton_actor]
struct ApiHandler {
    counter: AgentHandle,
}

async fn get_count(
    State(handler): State<AgentHandle>,
) -> String {
    let count: i32 = handler.ask(GetCount).await;
    format!("{}", count)
}

#[acton_main]
async fn main() {
    let mut app = ActonApp::launch();

    // Set up actor
    let counter = setup_counter(&mut app).await;

    // Set up HTTP server with actor handle
    let router = Router::new()
        .route("/count", get(get_count))
        .with_state(counter);

    axum::Server::bind(&"0.0.0.0:3000".parse().unwrap())
        .serve(router.into_make_service())
        .await
        .unwrap();
}
```

---

## Message Queues

Actors can consume from external queues:

```rust
#[acton_actor]
struct QueueConsumer {
    // Queue state
}

#[acton_message]
struct StartConsuming;

builder.mutate_on::<StartConsuming>(|actor, _| {
    let handle = actor.handle().clone();

    tokio::spawn(async move {
        loop {
            // Fetch from external queue
            if let Some(msg) = fetch_from_queue().await {
                handle.send(ProcessMessage { data: msg }).await.ok();
            }
        }
    });

    Reply::ready()
});
```

---

## Background Tasks

Spawn background work from actors:

```rust
builder.mutate_on::<StartBackgroundTask>(|actor, msg| {
    let handle = actor.handle().clone();
    let task_id = msg.id;

    tokio::spawn(async move {
        // Long-running work
        let result = perform_work().await;

        // Report completion back to actor
        handle.send(TaskComplete { id: task_id, result }).await.ok();
    });

    Reply::ready()
});
```

---

## Graceful Shutdown

Coordinate shutdown with external resources:

```rust
#[acton_main]
async fn main() {
    let mut app = ActonApp::launch();

    let actors = setup_actors(&mut app).await;

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await.ok();

    // Shutdown actors first
    app.shutdown_all().await.ok();

    // Then cleanup external resources
    database_pool.close().await;
}
```

---

## Best Practices

1. **Keep handlers fast** - Move slow work to spawned tasks
2. **Share pools** - Use connection pools, not per-actor connections
3. **Handle timeouts** - External calls can fail or hang
4. **Propagate shutdowns** - Clean up when actors stop
5. **Log errors** - External integrations often fail

---

## Reference

Continue to [Reference](/docs/reference/api-overview) for API documentation and cheatsheets.
