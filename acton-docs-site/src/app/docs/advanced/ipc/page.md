---
title: Inter-Process Communication
description: Sending messages to actors from external processes via Unix sockets.
---

Actors can communicate across process boundaries using Acton's IPC system. This enables multi-process architectures, external tooling, and polyglot systems.

## When You Need IPC

- **Multi-process architectures** - Separate concerns into different processes
- **External monitoring** - Query actor state from monitoring tools
- **Language interop** - Python, Node.js, or other languages talking to Rust actors
- **Process isolation** - Crash one process without affecting others

---

## How It Works

Acton's IPC uses Unix domain sockets for fast, local communication. Messages are serialized as JSON or MessagePack.

{% callout type="note" title="Local Only" %}
IPC is designed for same-machine communication. For network distribution, consider building on top with your preferred transport.
{% /callout %}

---

## Enabling IPC on an Actor

Mark messages for IPC with the `ipc` attribute:

```rust
#[acton_message(ipc)]
struct GetStatus;

#[acton_message(ipc)]
struct SetValue { value: i32 }
```

The `ipc` attribute adds serialization support.

---

## Server Side

Create an actor that listens for IPC connections:

```rust
use acton_reactive::prelude::*;

#[acton_actor]
struct MyService {
    value: i32,
}

#[acton_message(ipc)]
struct GetValue;

#[acton_message(ipc)]
struct SetValue { value: i32 }

#[acton_main]
async fn main() {
    let mut app = ActonApp::launch();
    let mut builder = app.new_actor::<MyService>();

    builder
        .act_on::<GetValue>(|actor, _| Reply::with(actor.model.value))
        .mutate_on::<SetValue>(|actor, msg| {
            actor.model.value = msg.value;
            Reply::ready()
        });

    let handle = builder.start().await;

    // Enable IPC on a socket path
    handle.enable_ipc("/tmp/my-service.sock").await;

    // Keep running
    tokio::signal::ctrl_c().await.ok();
    app.shutdown_all().await.ok();
}
```

---

## Client Side

From another process or language, connect to the socket and send messages:

```rust
use acton_reactive::ipc::IpcClient;

#[tokio::main]
async fn main() {
    let client = IpcClient::connect("/tmp/my-service.sock").await.unwrap();

    // Send a query
    let value: i32 = client.ask(GetValue).await.unwrap();
    println!("Current value: {}", value);

    // Send a command
    client.send(SetValue { value: 42 }).await.unwrap();
}
```

---

## Serialization

By default, IPC uses JSON for human-readable messages. For performance, use MessagePack:

```rust
#[acton_message(ipc, format = "msgpack")]
struct HighThroughputMessage {
    data: Vec<u8>,
}
```

---

## Error Handling

IPC operations can fail. Handle connection and serialization errors:

```rust
match client.ask::<GetValue, i32>(GetValue).await {
    Ok(value) => println!("Got: {}", value),
    Err(e) => eprintln!("IPC error: {}", e),
}
```

---

## Security Considerations

- Unix sockets respect file permissions
- Set appropriate permissions on the socket file
- Validate all incoming messages
- Consider authentication for sensitive operations

---

## Next

[Custom Supervision](/docs/advanced/custom-supervision) - Advanced failure recovery
