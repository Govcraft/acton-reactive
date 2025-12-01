---
title: Request-Response
description: Using ask for complex communication patterns.
---

While `send` is great for fire-and-forget, many scenarios require waiting for a response. The `ask` pattern enables request-response communication.

## Basic Ask Pattern

```rust
// Send and wait for response
let count: i32 = counter.ask(GetCount).await;
```

The caller blocks until the actor processes the message and returns a value.

---

## Returning Values

Use `Reply::with()` to return data:

```rust
builder.act_on::<GetCount>(|actor, _msg| {
    Reply::with(actor.model.count)
});

builder.act_on::<GetUser>(|actor, msg| {
    let user = actor.model.users.get(&msg.id).cloned();
    Reply::with(user)  // Returns Option<User>
});
```

---

## Example: Key-Value Store

```rust
use acton_reactive::prelude::*;
use std::collections::HashMap;

#[acton_actor]
#[derive(Default)]
struct KVStore {
    data: HashMap<String, String>,
}

#[acton_message]
struct Set { key: String, value: String }

#[acton_message]
struct Get { key: String }

#[acton_main]
async fn main() {
    let mut app = ActonApp::launch();
    let mut builder = app.new_actor::<KVStore>();

    builder
        .mutate_on::<Set>(|actor, msg| {
            actor.model.data.insert(msg.key.clone(), msg.value.clone());
            Reply::ready()
        })
        .act_on::<Get>(|actor, msg| {
            let value = actor.model.data.get(&msg.key).cloned();
            Reply::with(value)
        });

    let store = builder.start().await;

    store.send(Set {
        key: "name".into(),
        value: "Acton".into()
    }).await;

    let value: Option<String> = store.ask(Get { key: "name".into() }).await;
    println!("Got: {:?}", value);  // Got: Some("Acton")

    app.shutdown_all().await.ok();
}
```

---

## When to Use Ask vs Send

| Use `send` | Use `ask` |
|-----------|-----------|
| Commands (do something) | Queries (get something) |
| Notifications | Confirmations |
| High throughput needed | Response required to continue |
| Don't care about result | Need the result |

{% callout type="note" title="Performance Consideration" %}
`ask` blocks the caller until the response arrives. If you don't need the response, use `send` for better throughput.
{% /callout %}

---

## Chaining Requests

Sometimes you need data from multiple actors:

```rust
// Sequential
let user: User = user_store.ask(GetUser { id }).await;
let orders: Vec<Order> = order_store.ask(GetOrders { user_id: id }).await;

// The second request uses data from the first
```

---

## Error Handling

When an actor might fail to produce a value, return `Option` or `Result`:

```rust
builder.act_on::<FindUser>(|actor, msg| {
    match actor.model.users.get(&msg.id) {
        Some(user) => Reply::with(Ok(user.clone())),
        None => Reply::with(Err("User not found")),
    }
});
```

---

## Next

[Error Handling](/docs/building-apps/error-handling) - Building resilient systems
