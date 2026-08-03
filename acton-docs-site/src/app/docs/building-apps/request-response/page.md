---
title: Request-Response
description: Waiting for an answer with ask, and replying from a handler with the reply envelope.
---

`send` is fire-and-forget: it returns as soon as the message is in the actor's inbox, and tells you nothing about what happened next. When you need the answer, use **`ask`**.

```rust
let count = counter_handle.ask(GetCount).await?;
```

That is the whole call. It sends the request, waits for the actor's reply, and gives it back to you typed.

{% callout type="note" title="New in 9.0.0" %}
`ask` replaces the hand-rolled request-response plumbing earlier versions required: a response handler on the calling actor, an envelope addressed by hand, and a sleep to wait for the answer. If you have that pattern in your code, see the [migration guide](/docs/reference/migration-guide).
{% /callout %}

---

## Making a message askable

A message becomes askable by implementing `Request`, which names the reply through an associated type:

```rust
use acton_reactive::prelude::*;

#[acton_message]
struct GetCount;

#[acton_message]
struct Count(usize);

impl Request for GetCount {
    type Response = Count;
}
```

Because the reply type is pinned to the request type, `handle.ask(GetCount).await?` needs no turbofish, and asking for the wrong reply type is a compile error rather than a runtime surprise.

One request type has exactly one reply type. If the same payload needs two different answers in two different contexts, define two request types: that ambiguity is worth making visible.

---

## Answering from a handler

Handlers are unchanged. They reply through the envelope exactly as they always have, and an actor cannot tell an `ask` from a `send`:

```rust
counter.act_on::<GetCount>(|actor, envelope| {
    let count = actor.model.count;
    let reply = envelope.reply_envelope();

    Reply::pending(async move {
        reply.send(Count(count)).await;
    })
});
```

A handler that returns without replying is legal. The caller gets `AskError::NoReply` rather than waiting forever.

---

## A complete example

```rust
use acton_reactive::prelude::*;
use std::collections::HashMap;

#[acton_actor]
struct KVStore {
    data: HashMap<String, String>,
}

#[acton_message]
struct Set { key: String, value: String }

#[acton_message]
struct Get { key: String }

#[acton_message]
struct GetResponse(Option<String>);

impl Request for Get {
    type Response = GetResponse;
}

#[acton_main]
async fn main() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;

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
            let reply = envelope.reply_envelope();

            Reply::pending(async move {
                reply.send(GetResponse(value)).await;
            })
        });

    let store_handle = store.start().await;

    store_handle.send(Set {
        key: "name".into(),
        value: "Acton".into(),
    }).await;

    let found = store_handle.ask(Get { key: "name".into() }).await?;
    println!("Got: {:?}", found.0);

    runtime.shutdown_all().await?;
    Ok(())
}
```

Output:

```
Got: Some("Acton")
```

Note what is *not* in that example: no client actor, no response handler, and no sleep. The `Set` is not waited on either, and does not need to be. Inboxes are FIFO, so a completed `ask` also proves every message sent to that actor beforehand has been processed.

{% callout type="note" title="One ask is a barrier for everything before it" %}
This is the property that replaces most sleeps in test and startup code. If you need to know that a batch of `send`s has been worked through, `ask` the actor anything afterwards and await the reply.
{% /callout %}

---

## When no answer comes

`ask` always finishes. It holds no reply address of its own while waiting, so the moment the actor lets go of the request the call returns, in microseconds. A 30-second deadline (`DEFAULT_ASK_TIMEOUT`) backstops the cases closure cannot see. Use `ask_with_timeout` for a different bound:

```rust
use std::time::Duration;

match handle.ask_with_timeout(Get { key }, Duration::from_millis(250)).await {
    Ok(response) => println!("Got: {:?}", response.0),
    Err(AskError::TimedOut { after }) => println!("No answer after {after:?}"),
    Err(e) => println!("Request failed: {e}"),
}
```

`AskError` distinguishes outcomes you can act on differently:

| Variant | Meaning |
|---|---|
| `NoReply` | Delivered, but no answer is coming: the handler returned without replying, or the actor stopped, was restarted, or panicked holding the request |
| `TimedOut { after }` | The actor still holds a live reply address and has not answered |
| `Undeliverable` | The inbox was already closed, so no handler ran |
| `Cancelled` | Delivery was abandoned during shutdown |
| `UnexpectedReply` | The handler answered with a type the request does not declare |

`AskError` is `#[non_exhaustive]`, so `match` on it with a wildcard arm.

---

## Do not ask from inside a mutable handler

**`mutate_on` handlers are awaited inline on the actor's message loop.** A handler that waits for a reply stops its own actor from processing anything, including the message that would produce the reply:

```rust
// Deadlock. The actor cannot process GetCount while this handler is waiting.
actor.mutate_on::<Refresh>(|actor, _| {
    let handle = actor.handle().clone();
    Reply::pending(async move {
        let _ = handle.ask(GetCount).await;   // never answers
    })
});
```

Asking your own handle this way can never succeed, and asking another actor deadlocks as soon as the two wait on each other. The ways out:

- **Send instead of asking**, and let the reply arrive as an ordinary message. The handler already has a reply envelope for exactly this.
- **Move the exchange off the message loop**, into a spawned task, so the loop stays free.
- **Ask from outside the actor**, from a task, a test, or `main`.

The deadline turns such a mistake into a prompt `TimedOut` rather than a permanent hang, but it does not fix it.

---

## When the reply envelope is still the right tool

`ask` is for a caller that wants an answer *now*. The reply envelope is still what you want when:

- **The answer is not ready yet.** A handler can store `envelope.reply_envelope()` in its model and answer later, when the work completes. The caller's `ask` simply waits. This is what makes a result independent of arrival order rather than merely likely.
- **There are several replies.** `ask` resolves on the first one. A stream of progress messages goes to a peer actor that handles each as an ordinary message.
- **The exchange is actor-to-actor inside a handler**, where asking would deadlock.

Deferred reply, in full:

```rust
#[acton_actor]
struct Connector {
    connected: bool,
    waiting: Option<OutboundEnvelope>,
}

connector
    .mutate_on::<AwaitReady>(|actor, envelope| {
        let reply = envelope.reply_envelope();
        if actor.model.connected {
            Reply::pending(async move { reply.send(Ready).await })
        } else {
            // Hold the envelope; answer when the connection lands.
            actor.model.waiting = Some(reply);
            Reply::ready()
        }
    })
    .mutate_on::<Connected>(|actor, _| {
        actor.model.connected = true;
        match actor.model.waiting.take() {
            Some(reply) => Reply::pending(async move { reply.send(Ready).await }),
            None => Reply::ready(),
        }
    });
```

---

## Choosing between them

| Use `send` | Use `ask` | Use a reply envelope by hand |
|---|---|---|
| Commands, notifications | Queries you need the answer to | Multiple replies, or a reply from inside a handler |
| Highest throughput | Ordinary request/response | Streaming progress |
| Don't care about the result | Need the result, or need to know it finished | The responder answers a peer, not a caller |

---

## Handling missing data

Model absence in the reply type rather than as an error:

```rust
#[acton_message]
enum UserLookup {
    Found(User),
    NotFound,
}

impl Request for FindUser {
    type Response = UserLookup;
}

builder.act_on::<FindUser>(|actor, envelope| {
    let user = actor.model.users.get(&envelope.message().id).cloned();
    let reply = envelope.reply_envelope();

    Reply::pending(async move {
        match user {
            Some(u) => reply.send(UserLookup::Found(u)).await,
            None => reply.send(UserLookup::NotFound).await,
        }
    })
});
```

The caller matches on the answer:

```rust
match store_handle.ask(FindUser { id }).await? {
    UserLookup::Found(user) => println!("Found user: {}", user.name),
    UserLookup::NotFound => println!("User not found"),
}
```

Reserve `AskError` for the request *failing*, and the reply type for the request succeeding with nothing to report.

---

## Asking an actor in another process

With the `ipc` feature, `IpcClient::actor` names a remote actor and gives back a reference whose `ask` is deliberately the same call:

```rust
let count: Count = handle.ask(GetCount).await?;                  // local
let count: Count = client.actor("counter").ask(GetCount).await?; // remote
```

A remote request needs to be able to travel, so it implements `RemoteRequest` rather than `Request`. See [IPC patterns](/docs/ipc-patterns).

---

## Next

[Error Handling](/docs/building-apps/error-handling) — Building resilient systems
