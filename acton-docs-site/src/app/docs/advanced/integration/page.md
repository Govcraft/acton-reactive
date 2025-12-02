---
title: Integration
description: Working with databases, HTTP, and the async Rust ecosystem.
---

Acton Reactive is built on Tokio and integrates naturally with the async Rust ecosystem.

## Async Operations in Handlers

Use `Reply::pending` for handlers that need to await:

```rust
builder.act_on::<FetchData>(|_actor, envelope| {
    let url = envelope.message().url.clone();
    let reply = envelope.reply_envelope();

    Reply::pending(async move {
        let response = reqwest::get(&url).await.unwrap();
        let body = response.text().await.unwrap();
        reply.send(FetchResponse { body }).await;
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

#[acton_message]
struct QueryResult { rows: Vec<Row> }

// Initialize with before_start or a message
builder.mutate_on::<Initialize>(|actor, envelope| {
    let url = envelope.message().database_url.clone();

    Reply::pending(async move {
        let pool = PgPool::connect(&url).await.unwrap();
        // Note: Storing requires access pattern adjustment
        tracing::info!("Database connected");
    })
});

builder.act_on::<Query>(|actor, envelope| {
    let pool = actor.model.pool.clone().unwrap();
    let sql = envelope.message().sql.clone();
    let reply = envelope.reply_envelope();

    Reply::pending(async move {
        let rows = sqlx::query(&sql)
            .fetch_all(&pool)
            .await
            .unwrap();
        reply.send(QueryResult { rows }).await;
    })
});
```

---

## HTTP Servers

Actors can handle HTTP requests. Pass actor handles to your HTTP framework:

```rust
use axum::{Router, routing::get, extract::State};

async fn get_count(
    State(counter): State<ActorHandle>,
) -> String {
    // Send increment and let client poll for result
    counter.send(Increment).await;
    "Incremented".to_string()
}

#[acton_main]
async fn main() {
    let mut runtime = ActonApp::launch_async().await;

    // Set up actor
    let counter = setup_counter(&mut runtime).await;

    // Set up HTTP server with actor handle
    let router = Router::new()
        .route("/increment", get(get_count))
        .with_state(counter);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, router).await.unwrap();
}
```

For request-response, use a probe pattern or channels to collect responses.

---

## Message Queues

Actors can consume from external queues:

```rust
#[acton_actor]
struct QueueConsumer;

#[acton_message]
struct StartConsuming;

#[acton_message]
struct ProcessMessage { data: String }

builder.mutate_on::<StartConsuming>(|actor, _envelope| {
    let handle = actor.handle().clone();

    tokio::spawn(async move {
        loop {
            // Fetch from external queue
            if let Some(msg) = fetch_from_queue().await {
                handle.send(ProcessMessage { data: msg }).await;
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
#[acton_message]
struct StartBackgroundTask { id: u32 }

#[acton_message]
struct TaskComplete { id: u32, result: String }

builder.mutate_on::<StartBackgroundTask>(|actor, envelope| {
    let handle = actor.handle().clone();
    let task_id = envelope.message().id;

    tokio::spawn(async move {
        // Long-running work
        let result = perform_work().await;

        // Report completion back to actor
        handle.send(TaskComplete { id: task_id, result }).await;
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
    let mut runtime = ActonApp::launch_async().await;
    let database_pool = setup_database().await;

    let actors = setup_actors(&mut runtime, &database_pool).await;

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await.ok();

    // Shutdown actors first
    runtime.shutdown_all().await.ok();

    // Then cleanup external resources
    database_pool.close().await;
}
```

---

## Best Practices

1. **Keep handlers fast** — Move slow work to spawned tasks
2. **Share pools** — Use connection pools, not per-actor connections
3. **Handle timeouts** — External calls can fail or hang
4. **Propagate shutdowns** — Clean up when actors stop
5. **Log errors** — External integrations often fail

---

## Reference

Continue to [Reference](/docs/reference/api-overview) for API documentation and cheatsheets.
