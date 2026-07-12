---
title: Integration
description: Working with databases, HTTP, and the async Rust ecosystem.
---

Acton Reactive is built on Tokio and integrates naturally with the async Rust ecosystem — with one wrinkle worth understanding before you write your first handler.

## The `Sync` Bound on Handler Futures

`Reply::pending` produces a `Pin<Box<dyn Future<Output = ()> + Send + Sync + 'static>>`. That **`Sync`** is stricter than the `Send` bound you're used to from `tokio::spawn`, and it's the thing that trips people up when wiring in an HTTP or database client:

```
error: future cannot be shared between threads safely
       future created by async block is not `Sync`
```

Plenty of third-party futures are `Send` but not `Sync`. When you hit this, don't fight it — move the work off the handler.

### The Pattern: Spawn and Report Back

`tokio::spawn` only requires `Send`. Spawn the I/O, then send the result to the actor as an ordinary message:

```rust
#[acton_message]
struct FetchData { url: String }

#[acton_message]
struct FetchDone { url: String, body: String }

// 1. Handler kicks off the work and returns immediately
builder.mutate_on::<FetchData>(|actor, ctx| {
    let handle = actor.handle().clone();
    let url = ctx.message().url.clone();

    tokio::spawn(async move {
        match reqwest::get(&url).await {
            Ok(resp) => match resp.text().await {
                Ok(body) => handle.send(FetchDone { url, body }).await,
                Err(e) => tracing::error!("read failed for {url}: {e}"),
            },
            Err(e) => tracing::error!("fetch failed for {url}: {e}"),
        }
    });

    Reply::ready()
});

// 2. The result arrives as a normal message — state changes happen here
builder.mutate_on::<FetchDone>(|actor, ctx| {
    let msg = ctx.message();
    actor.model.cache.insert(msg.url.clone(), msg.body.clone());
    Reply::ready()
});
```

This is a better shape anyway: the actor's mailbox keeps moving while the request is in flight, instead of the handler occupying the actor for the duration of a network round-trip.

### When `Reply::pending` Is Fine

If everything your async block holds across an `.await` is `Sync`, use it directly — it's simpler:

```rust
builder.act_on::<GetCount>(|actor, ctx| {
    let count = actor.model.count;     // copy out before the async block
    let reply = ctx.reply_envelope();

    Reply::pending(async move {
        reply.send(CountResponse(count)).await;
    })
});
```

Sending messages, awaiting timers, and awaiting `tokio::task::JoinHandle`s all satisfy the bound.

---

## Database Connections

Store the connection pool in actor state. The pool is cheap to clone (it's an `Arc` internally), so handlers clone it out and use it from a spawned task.

Connecting is async, and you can't assign to `actor.model` from inside a `'static` async block — so use the same spawn-and-report-back pattern: connect off-actor, then send the pool back to be stored.

```rust
use sqlx::PgPool;

#[acton_actor]
struct DatabaseActor {
    pool: Option<PgPool>,
}

#[acton_message]
struct Connect { database_url: String }

#[acton_message]
struct Connected { pool: PgPool }

#[acton_message]
struct Query { sql: String }

#[acton_message]
struct QueryResult { rows: Vec<Row> }

// 1. Connect off-actor, then hand the pool back
builder.mutate_on::<Connect>(|actor, ctx| {
    let handle = actor.handle().clone();
    let url = ctx.message().database_url.clone();

    tokio::spawn(async move {
        match PgPool::connect(&url).await {
            Ok(pool) => handle.send(Connected { pool }).await,
            Err(e) => tracing::error!("DB connect failed: {e}"),
        }
    });

    Reply::ready()
});

// 2. Store it — a plain sync handler, no future needed
builder.mutate_on_sync::<Connected>(|actor, ctx| {
    actor.model.pool = Some(ctx.message().pool.clone());
    tracing::info!("Database connected");
});

// 3. Query using the stored pool
builder.mutate_on::<Query>(|actor, ctx| {
    let Some(pool) = actor.model.pool.clone() else {
        tracing::warn!("Query before pool was ready");
        return Reply::ready();
    };
    let sql = ctx.message().sql.clone();
    let reply = ctx.reply_envelope();

    tokio::spawn(async move {
        match sqlx::query(&sql).fetch_all(&pool).await {
            Ok(rows) => reply.send(QueryResult { rows }).await,
            Err(e) => tracing::error!("query failed: {e}"),
        }
    });

    Reply::ready()
});
```

{% callout type="note" title="Why spawn instead of Reply::pending?" %}
Two reasons. Handler futures must be `Sync`, and `sqlx`'s query futures generally aren't. And a query awaited inside the handler would block this actor's mailbox for the duration of the round-trip — spawning keeps it responsive.
{% /callout %}

{% callout type="warning" title="`#[acton_message]` requires Clone + Debug" %}
`Connected { pool: PgPool }` works because `PgPool` is both. A type that isn't (a raw connection, say) can't ride in a message — put it behind an `Arc`, or hand the actor a factory instead.
{% /callout %}

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

## Talking to Actors From Other Processes

Everything above keeps the integration inside one process. When the other side is a *separate* process — a Python script, a Node service, a CLI, another Rust binary — use Acton's IPC layer instead of building your own socket protocol.

Enable the feature:

```toml
[dependencies]
{% $dep.ipc %}
```

**Server side** — register the message types that can arrive over the wire, mark actors as reachable, and start the listener:

```rust
let mut runtime = ActonApp::launch_async().await;

runtime.ipc_registry().register::<GetPrice>("GetPrice");

let mut service = runtime.new_actor_with_name::<PriceService>("prices".to_string());
service
    .act_on::<GetPrice>(|actor, ctx| { /* ... */ })
    .expose_for_ipc();              // reachable as "prices"
service.start().await;

let listener = runtime.start_ipc_listener().await?;
```

**Client side** — `IpcClient` connects over a Unix domain socket and gives you fire-and-forget, request-response, and subscriptions:

```rust
use acton_reactive::ipc::{IpcClient, IpcConfig, IpcEnvelope};

let client = IpcClient::connect(IpcConfig::load().socket_path()).await?;

let response = client.request(IpcEnvelope::new_request(
    "prices",
    "GetPrice",
    serde_json::json!({ "symbol": "ACTON" }),
)).await?;

client.disconnect().await?;
```

Messages that cross the boundary need serde derives — `#[acton_message(ipc)]` adds them for you.

See [IPC Setup](/docs/ipc-setup) to get a socket running, [IPC Patterns](/docs/ipc-patterns) for request-response, streaming, and push subscriptions, and [Advanced IPC](/docs/advanced/ipc) for the wire protocol.

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

1. **Keep handlers fast** — Move slow work to spawned tasks and report back with a message
2. **Expect the `Sync` bound** — If a third-party future won't fit in `Reply::pending`, that's your cue to spawn it
3. **Share pools** — Use connection pools, not per-actor connections
4. **Handle timeouts** — External calls can fail or hang
5. **Propagate shutdowns** — Shut actors down before the resources they depend on
6. **Log errors** — External integrations often fail, and a spawned task has nowhere else to report

---

## Reference

Continue to [Reference](/docs/reference/api-overview) for API documentation and cheatsheets.
