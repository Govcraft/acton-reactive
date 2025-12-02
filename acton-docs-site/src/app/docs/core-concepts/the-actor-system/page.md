---
title: The Actor System
description: How the runtime manages your actors — spawning, handles, and the broker.
---

The actor runtime brings your actors to life. It spawns actors, manages their lifecycles, provides handles for communication, and coordinates shutdown.

## Creating the Runtime

Every Acton application starts by launching the runtime:

```rust
use acton_reactive::prelude::*;

#[acton_main]
async fn main() {
    let mut runtime = ActonApp::launch_async().await;

    // Create actors, send messages...

    runtime.shutdown_all().await.ok();
}
```

---

## Creating Actors

Once you have a runtime, create actors:

```rust
let mut counter = runtime.new_actor::<Counter>();

counter
    .mutate_on::<Increment>(handle_increment)
    .act_on::<GetCount>(handle_get);

let handle = counter.start().await;
```

The `start()` method:
1. Creates the actor instance
2. Sets up its message queue
3. Starts it running on the async runtime
4. Returns a handle for sending messages

### Named Actors

Give actors meaningful names for easier debugging:

```rust
let mut counter = runtime.new_actor_with_name::<Counter>("user-counter".to_string());
```

---

## Working with Handles

A handle is your interface to an actor:

```rust
let handle = counter.start().await;

// Fire and forget
handle.send(Increment).await;
```

Handles are cheap to clone:

```rust
let handle_for_web = handle.clone();
let handle_for_background = handle.clone();
```

---

## The Broker: Pub/Sub

Not all communication is point-to-point. The Broker provides publish/subscribe messaging.

### Getting the Broker

```rust
let broker = runtime.broker();
```

### Publishing Events

```rust
#[acton_message]
struct UserLoggedIn { user_id: u64 }

broker.broadcast(UserLoggedIn { user_id: 123 }).await;
```

### Subscribing to Events

Subscribe actors **before** starting them:

```rust
let mut listener = runtime.new_actor::<EventListener>();

listener.mutate_on::<UserLoggedIn>(|actor, envelope| {
    let msg = envelope.message();
    println!("User {} logged in", msg.user_id);
    Reply::ready()
});

// Subscribe before starting
listener.handle().subscribe::<UserLoggedIn>().await;

let handle = listener.start().await;
```

Now the actor receives all `UserLoggedIn` events broadcast through the broker.

{% callout type="note" title="Broker vs Direct Messaging" %}
**Use handles when:**
- You know exactly which actor should receive the message
- You're doing point-to-point communication

**Use the broker when:**
- Multiple actors should react to an event
- The sender doesn't know (or care) who receives it
{% /callout %}

---

## Shutdown

Proper shutdown ensures messages aren't lost:

```rust
runtime.shutdown_all().await.ok();
```

For signal handling:

```rust
use tokio::signal;

// Wait for Ctrl+C
signal::ctrl_c().await.expect("Failed to listen");
runtime.shutdown_all().await.ok();
```

---

## Next

[Supervision Basics](/docs/core-concepts/supervision-basics) — Handling failures gracefully
