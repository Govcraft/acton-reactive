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

Acton's IPC uses Unix domain sockets for fast, local communication. Messages are serialized as JSON.

{% callout type="note" title="Local Only" %}
IPC is designed for same-machine communication. For network distribution, build on top with your preferred transport.
{% /callout %}

---

## Server Side Setup

### Step 1: Mark Messages for IPC

Add the `ipc` option to enable serialization:

```rust
#[acton_message(ipc)]
struct GetStatus;

#[acton_message(ipc)]
struct SetValue { value: i32 }

#[acton_message(ipc)]
struct StatusResponse { value: i32 }
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

    // Create the service actor
    let mut service = runtime.new_actor::<MyService>();

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
        });

    let handle = service.start().await;

    // Expose the actor for IPC access
    runtime.ipc_expose("my-service", handle);

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

---

## Client Side

External clients connect via Unix domain sockets using the wire protocol:

```rust
use tokio::net::UnixStream;
use acton_reactive::ipc::protocol::{write_envelope, read_response};
use acton_reactive::ipc::IpcEnvelope;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Connect to the socket
    let stream = UnixStream::connect("/run/user/1000/acton/ipc.sock").await?;
    let (mut reader, mut writer) = stream.into_split();

    // Create an envelope targeting the exposed actor
    let envelope = IpcEnvelope::new(
        "my-service",  // Logical name from ipc_expose
        "GetValue",    // Registered type name
        serde_json::json!({}),
    );

    // Send the request
    write_envelope(&mut writer, &envelope).await?;

    // Read the response
    let response = read_response(&mut reader, 1024 * 1024).await?;
    println!("Response: {:?}", response);

    Ok(())
}
```

---

## Client Libraries

Acton includes example client libraries for other languages:

### Python

```python
from acton_ipc import ActonClient

client = ActonClient("/run/user/1000/acton/ipc.sock")
response = client.send("my-service", "GetValue", {})
print(f"Value: {response}")
```

### Node.js

```typescript
import { ActonClient } from 'acton-ipc';

const client = new ActonClient('/run/user/1000/acton/ipc.sock');
const response = await client.send('my-service', 'GetValue', {});
console.log('Value:', response);
```

See the `examples/ipc_client_libraries/` directory for complete implementations.

---

## Security Considerations

- Unix sockets respect file permissions
- Set appropriate permissions on the socket file
- Validate all incoming messages
- Consider authentication for sensitive operations

---

## Next

[Custom Supervision](/docs/advanced/custom-supervision) — Advanced failure recovery
