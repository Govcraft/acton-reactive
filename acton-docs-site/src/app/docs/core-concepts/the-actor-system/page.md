---
title: The Actor System
description: How ActonApp manages your actors - spawning, handles, and the broker.
---

The `ActonApp` is the runtime that brings your actors to life. It spawns actors, manages their lifecycles, provides handles for communication, and coordinates shutdown.

## Creating an ActonApp

Every Acton application starts by creating an app:

```rust
use acton_reactive::prelude::*;

#[acton_main]
async fn main() {
    let mut app = ActonApp::launch();

    // Spawn actors, send messages...

    app.shutdown_all().await.ok();
}
```

---

## Spawning Actors

Once you have an app, spawn actors:

```rust
let mut builder = app.new_actor::<Counter>();

builder
    .mutate_on::<Increment>(handle_increment)
    .act_on::<GetCount>(handle_get);

let handle = builder.start().await;
```

The `start()` method:
1. Creates the actor instance
2. Sets up its message queue
3. Starts it running on the async runtime
4. Returns a handle for sending messages

---

## Working with Handles

A handle is your interface to an actor:

```rust
let handle = builder.start().await;

// Fire and forget
handle.send(Increment).await;

// Request-response
let count: i32 = handle.ask(GetCount).await;
```

Handles are cheap to clone:

```rust
let handle_for_web = handle.clone();
let handle_for_background = handle.clone();
```

---

## The Broker: Pub/Sub

Not all communication is point-to-point. The Broker provides publish/subscribe messaging.

### Publishing Events

```rust
let broker = app.get_broker();

#[acton_message]
struct UserLoggedIn { user_id: u64 }

broker.broadcast(UserLoggedIn { user_id: 123 }).await;
```

### Subscribing to Events

```rust
handle.subscribe::<UserLoggedIn>().await;
```

Now the actor receives all `UserLoggedIn` events.

{% callout type="note" title="Broker vs Direct Messaging" %}
**Use handles when:**
- You know exactly which actor should receive the message
- You need a response

**Use the broker when:**
- Multiple actors should react to an event
- The sender doesn't know who should receive it
{% /callout %}

---

## Shutdown

Proper shutdown ensures messages aren't lost:

```rust
app.shutdown_all().await.ok();
```

For signal handling:

```rust
use tokio::signal;

// Wait for Ctrl+C
signal::ctrl_c().await.expect("Failed to listen");
app.shutdown_all().await.ok();
```

---

## Next

[Supervision Basics](/docs/core-concepts/supervision-basics) - Handling failures gracefully
