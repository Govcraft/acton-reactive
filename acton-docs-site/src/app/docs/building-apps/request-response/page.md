---
title: Request-Response
description: Using reply envelopes for request-response communication patterns.
---

While `send` is great for fire-and-forget, many scenarios require getting data back from an actor. Acton uses the **reply envelope pattern** for request-response communication.

## The Reply Envelope Pattern

Every message arrives in an envelope that knows where it came from. Use `envelope.reply_envelope()` to send a response back:

```rust
// Actor that responds to queries
builder.act_on::<GetCount>(|actor, envelope| {
    let count = actor.model.count;
    let reply_envelope = envelope.reply_envelope();

    Reply::pending(async move {
        reply_envelope.send(CountResponse(count)).await;
    })
});
```

The sender must have a handler for the response message:

```rust
// Requesting actor handles the response
requester.mutate_on::<CountResponse>(|actor, envelope| {
    let count = envelope.message().0;
    println!("Received count: {}", count);
    Reply::ready()
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

#[acton_actor]
#[derive(Default)]
struct Client;

#[acton_message]
struct Set { key: String, value: String }

#[acton_message]
struct Get { key: String }

#[acton_message]
struct GetResponse(Option<String>);

#[acton_main]
async fn main() {
    let mut runtime = ActonApp::launch_async().await;

    // Create the store
    let mut store = runtime.new_actor::<KVStore>();

    store
        .mutate_on::<Set>(|actor, envelope| {
            let msg = envelope.message();
            actor.model.data.insert(msg.key.clone(), msg.value.clone());
            Reply::ready()
        })
        .act_on::<Get>(|actor, envelope| {
            let key = &envelope.message().key;
            let value = actor.model.data.get(key).cloned();
            let reply_envelope = envelope.reply_envelope();

            Reply::pending(async move {
                reply_envelope.send(GetResponse(value)).await;
            })
        });

    let store_handle = store.start().await;

    // Create a client that queries the store
    let mut client = runtime.new_actor::<Client>();

    client.mutate_on::<GetResponse>(|_actor, envelope| {
        let value = &envelope.message().0;
        println!("Got: {:?}", value);
        Reply::ready()
    });

    let client_handle = client.start().await;

    // Store some data
    store_handle.send(Set {
        key: "name".into(),
        value: "Acton".into()
    }).await;

    // Query using an envelope addressed to the store, with reply going to client
    let query_envelope = client_handle.create_envelope(
        Some(store_handle.reply_address())
    );
    query_envelope.send(Get { key: "name".into() }).await;

    // Give time for async processing
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    runtime.shutdown_all().await.ok();
}
```

Output:

```
Got: Some("Acton")
```

---

## When to Use Each Pattern

| Use `send` (fire-and-forget) | Use reply envelopes |
|------------------------------|---------------------|
| Commands (do something) | Queries (get something) |
| Notifications | Data requests |
| High throughput needed | Coordination required |
| Don't care about result | Need the result |

{% callout type="note" title="Fire-and-Forget Default" %}
`send` is Acton's primary pattern. Use reply envelopes when you specifically need response data. This keeps your system decoupled and performant.
{% /callout %}

---

## Handling Missing Data

Return meaningful responses when data might not exist:

```rust
builder.act_on::<FindUser>(|actor, envelope| {
    let user_id = &envelope.message().id;
    let user = actor.model.users.get(user_id).cloned();
    let reply_envelope = envelope.reply_envelope();

    Reply::pending(async move {
        match user {
            Some(u) => reply_envelope.send(UserFound(u)).await,
            None => reply_envelope.send(UserNotFound).await,
        }
    })
});
```

The requesting actor handles both cases:

```rust
requester
    .mutate_on::<UserFound>(|_actor, envelope| {
        let user = &envelope.message().0;
        println!("Found user: {}", user.name);
        Reply::ready()
    })
    .mutate_on::<UserNotFound>(|_actor, _envelope| {
        println!("User not found");
        Reply::ready()
    });
```

---

## Next

[Error Handling](/docs/building-apps/error-handling) — Building resilient systems
