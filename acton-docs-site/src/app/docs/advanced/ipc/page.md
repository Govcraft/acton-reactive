---
title: Inter-Process Communication
description: Sending messages to actors from external processes via Unix sockets.
---

Actors can communicate across process boundaries using Acton's IPC system. This enables multi-process architectures, external tooling, and polyglot systems.

## When You Need IPC

- **Multi-process architectures** — Separate concerns into different processes
- **External monitoring** — Query actor state from monitoring tools
- **Language interop** — Python, Node.js, or other languages talking to Rust actors
- **Process isolation** — Crash one process without affecting others

---

## How It Works

Acton's IPC uses Unix domain sockets for fast, local communication. Messages are length-prefixed frames whose payload is serialized as **JSON** (always available) or **MessagePack** (with the `ipc-messagepack` feature). Each frame declares its own format, so both kinds of client can share one listener.

The socket lives at `$XDG_RUNTIME_DIR/acton/<app_name>/ipc.sock`, where `app_name` defaults to the binary name (falling back to `/tmp/acton/<app_name>/ipc.sock` when `XDG_RUNTIME_DIR` is unset). Resolve it with `IpcConfig::load().socket_path()` rather than hardcoding it.

{% callout type="note" title="Local Only" %}
IPC is designed for same-machine communication. For network distribution, build on top with your preferred transport.
{% /callout %}

---

## Server Side Setup

### Step 1: Mark Messages for IPC

Add the `ipc` option to enable serialization:

```rust
#[acton_message(ipc)]
struct GetValue;

#[acton_message(ipc)]
struct SetValue { value: i32 }

#[acton_message(ipc)]
struct ValueResponse { value: i32 }
```

The `ipc` option adds `Serialize` and `Deserialize` derives. You must still register types with the runtime.

### Step 2: Register Types and Expose Actors

```rust
use acton_reactive::prelude::*;

#[acton_actor]
struct MyService {
    value: i32,
}

#[acton_main]
async fn main() {
    let mut runtime = ActonApp::launch_async().await;

    // Register IPC message types
    let registry = runtime.ipc_registry();
    registry.register::<GetValue>("GetValue");
    registry.register::<SetValue>("SetValue");
    registry.register::<ValueResponse>("ValueResponse");

    // Create and configure the service actor
    let mut service = runtime.new_actor_with_name::<MyService>("my-service".to_string());

    service
        .act_on::<GetValue>(|actor, envelope| {
            let value = actor.model.value;
            let reply_envelope = envelope.reply_envelope();

            Reply::pending(async move {
                reply_envelope.send(ValueResponse { value }).await;
            })
        })
        .mutate_on::<SetValue>(|actor, envelope| {
            actor.model.value = envelope.message().value;
            Reply::ready()
        })
        .expose_for_ipc();  // Expose using the actor's name ("my-service")

    service.start().await;

    // Start the IPC listener
    let listener = runtime.start_ipc_listener().await
        .expect("Failed to start IPC listener");

    // Keep running until Ctrl+C
    tokio::signal::ctrl_c().await.ok();

    // Graceful shutdown
    listener.shutdown_gracefully().await;
    runtime.shutdown_all().await.ok();
}
```

{% callout type="tip" title="Custom IPC Names" %}
The `expose_for_ipc()` method uses the actor's ERN name automatically. If you need a different IPC name, use `runtime.ipc_expose("custom-name", handle)` after starting the actor.
{% /callout %}

---

## Client Side

Rust clients use `IpcClient`, which owns the socket, runs dedicated reader and writer tasks, and correlates responses to requests for you. No hand-rolled framing:

```rust
use acton_reactive::prelude::*;
use acton_reactive::ipc::{socket_exists, socket_is_alive};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Resolve the same socket path the server uses.
    let socket_path = IpcConfig::load().socket_path();

    if !socket_exists(&socket_path) || !socket_is_alive(&socket_path).await {
        eprintln!("Server not running at {}", socket_path.display());
        return Ok(());
    }

    let client = IpcClient::connect(&socket_path).await?;

    // new_request expects a reply; new() would be fire-and-forget.
    let envelope = IpcEnvelope::new_request(
        "my-service",  // Actor name (from expose_for_ipc or ipc_expose)
        "GetValue",    // Registered type name
        serde_json::json!({}),
    );

    let response = client.request(envelope).await?;

    if response.success {
        println!("Value: {:?}", response.payload);
    } else {
        eprintln!("{:?}: {:?}", response.error_code, response.error);
    }

    client.disconnect().await?;
    Ok(())
}
```

{% callout type="warning" title="Use new_request when you want an answer" %}
`IpcEnvelope::new` builds a **fire-and-forget** message: the server routes it to the actor and immediately replies `{"status": "delivered"}`, discarding whatever the actor sends back. Only `IpcEnvelope::new_request` (or `new_request_with_timeout`) sets `expects_reply`, which is what makes the listener wait for the actor's reply and forward it.
{% /callout %}

`IpcClient` also covers fire-and-forget (`send`), request-stream (`request_stream`), subscriptions (`subscribe` + `take_push_receiver`), and discovery (`discover`). See [IPC Patterns](/docs/ipc-patterns) for each. For request-stream, `request_stream` returns a channel that yields every frame in order and closes after the frame with `is_final: true`:

```rust
let envelope = IpcEnvelope::new_stream_request(
    "my-service",
    "ListValues",
    serde_json::json!({}),
);

let mut stream_rx = client.request_stream(envelope).await?;
while let Some(frame) = stream_rx.recv().await {
    println!("Frame #{}: {:?}", frame.sequence, frame.payload);
}
```

---

## Typed Requests with ask

`IpcClient::request` speaks in `IpcEnvelope`s and `serde_json::Value`. When you know the types, `IpcClient::actor` names a remote actor and gives back a `RemoteActorRef` whose `ask` is deliberately the **same call** as the local one:

```rust
let count: Count = handle.ask(GetCount).await?;                  // local
let count: Count = client.actor("counter").ask(GetCount).await?; // remote
```

This adds no transport. It is a typed façade over the correlated request machinery that already existed, plus the judgement about what a response means.

### The RemoteRequest bound

A remote request has to be able to travel, so it implements `RemoteRequest` rather than `Request`:

```rust
#[acton_message(ipc)]
struct GetCount;

#[acton_message(ipc)]
struct Count(u64);

impl Request for GetCount {
    type Response = Count;
}

impl RemoteRequest for GetCount {
    // Must equal the string the peer registered this type under.
    const MESSAGE_TYPE: &'static str = "GetCount";
}
```

`RemoteRequest` is `Request` plus `Serialize`, a `DeserializeOwned` reply, and a `MESSAGE_TYPE` constant. **A message that cannot cross the boundary is a compile error** against a `RemoteActorRef`, rather than a call that appears to work. Local-only users pay nothing: `ask` on `ActorHandle` still takes a bare `Request`.

The wire name is written down rather than derived from `std::any::type_name`, which changes when a type moves between modules and cannot describe a peer that is a different binary, a different version, or not Rust at all.

### Errors

Remote failures reuse `AskError`, with two variants that only a boundary can produce:

| Variant | Meaning |
|---|---|
| `PeerRejected { code, detail }` | Refused **before dispatch**: no such actor, no such registered type, busy, rate-limited, shutting down. Nothing ran, so a **retry is safe**. Carries the peer's own code, which is what tells a mistyped actor name from an unregistered type |
| `TransportFailed { detail }` | The connection failed, so whether the request was processed is **unknown** |

The rest carry their local meanings. A handler that returns without replying is `NoReply`. A deadline at either end is `TimedOut`. A reply that does not deserialize is `UnexpectedReply`, which covers a wrong-typed handler reply, an unregistered reply type, and version skew between peers, because all three mean the same actionable thing: the answer is not the answer this request declares.

**It cannot hang.** The client registers its correlation id before writing, so a dropped connection wakes the caller at once instead of waiting out the clock, and the deadline is stamped on the request as well as applied locally, so the peer stops waiting on its actor at the same moment.

{% callout type="warning" title="Scope: one actor" %}
`ask` has no meaning over `broadcast`, remotely or locally, because a broadcast has no single replier. Use subscriptions for that.

The deadlock warning applies unchanged: **do not `ask` from inside a `mutate_on` handler.** Crossing a process boundary does not change it, it only makes the other party harder to see.
{% /callout %}

---

## Client Libraries

Acton includes example client libraries for Python, Node.js, and Deno. Each speaks the same wire protocol as `IpcClient`.

### Python

`ActonIpcClient` is async; `ActonIpcClientSync` is the blocking equivalent.

```python
from acton_ipc import ActonIpcClient

client = ActonIpcClient("/run/user/1000/acton/my_app/ipc.sock")
await client.connect()

response = await client.request("my-service", "GetValue", {})
print(f"Value: {response.payload}")

# Push notifications
await client.subscribe(["PriceUpdate"])
```

### Node.js

The package is `acton-ipc-client`, exporting `ActonIpcClient`.

```typescript
import { ActonIpcClient } from 'acton-ipc-client';

const client = new ActonIpcClient('/run/user/1000/acton/my_app/ipc.sock');
await client.connect();

const response = await client.request('my-service', 'GetValue', {});
console.log('Value:', response.payload);
```

### Deno

A Deno client ships alongside the Node.js one, in `examples/ipc_client_libraries/deno/`.

See the `examples/ipc_client_libraries/` directory for complete implementations, and [IPC Setup](/docs/ipc-setup#wire-protocol) for the frame format if you are writing a client in another language.

---

## Security Considerations

- Unix sockets respect file permissions
- Set appropriate permissions on the socket file
- Validate all incoming messages
- Consider authentication for sensitive operations

### Peer identity

`PeerCredentials` carries the kernel-reported identity of the process behind a connection, read through `SubscriptionManager::peer_credentials()` and `peer_pid()`.

**Prefer `uid()` and `gid()` for access-control decisions.** PIDs are recycled, so a check that reads a PID and then acts on it can be defeated by the original process exiting between the two steps. The user and group ids are fixed for the life of the connection. Treat `pid()` as a diagnostic: it is what lets a log line name the process that connected.

### Connection limits

A server at its connection limit now writes a typed error before closing, and the client reports `IpcError::ConnectionLimitReached { limit }`. Previously the socket was accepted and dropped without a word, so the refusal surfaced as `Broken pipe (os error 32)` on the first write, which points at nothing.

The effective limit is logged at listener startup beside the socket path, so the ceiling is discoverable before it is reached. `IpcListenerStats::max_connections()` and `connections_available()` let an embedder check headroom, and `IpcClient::rejection_reason()` reports why a connection was refused, or `None` for one accepted normally.

`IpcError` is now `#[non_exhaustive]`, so a `match` listing every variant needs a wildcard arm. Nothing changes on the wire: a refusal travels as an ordinary error response carrying an `error_code` string, so a client built against 8.x still parses the frame. `CONNECTION_LIMIT_REACHED_CODE` and `CONNECTION_REJECTED_CORRELATION_ID` are the wire constants a non-Rust client needs to recognise one.

---

## Next

[Custom Supervision](/docs/advanced/custom-supervision) — Advanced failure recovery
