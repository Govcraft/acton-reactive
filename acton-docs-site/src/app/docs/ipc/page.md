---
title: IPC Communication
nextjs:
  metadata:
    title: IPC Communication - acton-reactive
    description: Inter-Process Communication in acton-reactive, enabling external processes to communicate with agents via Unix Domain Sockets.
---

This guide covers Inter-Process Communication (IPC) in `acton-reactive`, enabling external processes to communicate with agents via Unix Domain Sockets.

---

## Overview

The IPC module allows external processes (written in any language) to communicate with `acton-reactive` agents via Unix Domain Sockets.

### Capabilities

| Pattern | Description | Use Case |
|---------|-------------|----------|
| **Request-Response** | Single reply to single request | RPC-style calls |
| **Request-Stream** | Multiple frames per request | Pagination, real-time data |
| **Push Notifications** | Server-initiated messages | Subscriptions, events |

### Architecture

```text
┌────────────────────────────────────────────────────────────┐
│                     External Processes                      │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐   │
│  │  Python  │  │ Node.js  │  │   Rust   │  │  Other   │   │
│  │  Client  │  │  Client  │  │  Client  │  │  Client  │   │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘   │
└───────│─────────────│─────────────│─────────────│──────────┘
        │             │             │             │
        └─────────────┴──────┬──────┴─────────────┘
                             │
                    Unix Domain Socket
                      /tmp/acton.sock
                             │
┌────────────────────────────┴───────────────────────────────┐
│                      acton-reactive                         │
│  ┌─────────────────────────────────────────────────────┐   │
│  │                    IPC Listener                      │   │
│  │  ┌───────────┐  ┌───────────┐  ┌─────────────────┐  │   │
│  │  │ Type      │  │ Rate      │  │ Subscription    │  │   │
│  │  │ Registry  │  │ Limiter   │  │ Manager         │  │   │
│  │  └───────────┘  └───────────┘  └─────────────────┘  │   │
│  └──────────────────────────┬──────────────────────────┘   │
│                             │                               │
│  ┌──────────────────────────┴──────────────────────────┐   │
│  │                    Agent System                      │   │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐             │   │
│  │  │ Agent A │  │ Agent B │  │  Broker │             │   │
│  │  └─────────┘  └─────────┘  └─────────┘             │   │
│  └─────────────────────────────────────────────────────┘   │
└────────────────────────────────────────────────────────────┘
```

---

## Enabling IPC

### Cargo Feature

Add the `ipc` feature to your `Cargo.toml`:

```toml
[dependencies]
acton-reactive = { version = "0.1", features = ["ipc"] }
```

For MessagePack serialization (smaller messages):

```toml
[dependencies]
acton-reactive = { version = "0.1", features = ["ipc-messagepack"] }
```

### Feature Comparison

| Feature | Serialization | Message Size | Speed |
|---------|---------------|--------------|-------|
| `ipc` | JSON | Larger | Good |
| `ipc-messagepack` | MessagePack | ~30-50% smaller | Better |

---

## Setup and Configuration

### Basic Setup

```rust
use acton_reactive::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Launch runtime
    let mut runtime = ActonApp::launch();

    // 2. Get IPC type registry
    let registry = runtime.ipc_registry();

    // 3. Register message types (see next section)
    registry.register::<MyRequest>("MyRequest");
    registry.register::<MyResponse>("MyResponse");

    // 4. Create and start agents
    let agent = runtime.new_agent::<MyState>().start().await;

    // 5. Expose agents via IPC
    runtime.ipc_expose("my_service", agent);

    // 6. Start IPC listener
    let listener = runtime.start_ipc_listener().await?;

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;

    // 7. Clean shutdown
    listener.shutdown().await?;
    runtime.shutdown_all().await?;

    Ok(())
}
```

### Custom Configuration

```rust
use acton_reactive::common::ipc::{IpcConfig, RateLimitConfig};
use std::path::PathBuf;
use std::time::Duration;

let config = IpcConfig {
    socket_path: PathBuf::from("/var/run/myapp/acton.sock"),
    max_connections: 100,
    connection_timeout: Duration::from_secs(60),
    rate_limit: Some(RateLimitConfig {
        requests_per_second: 1000,
        burst_size: 50,
    }),
};

let listener = runtime.start_ipc_listener_with_config(config).await?;
```

### Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `socket_path` | `PathBuf` | `/tmp/acton.sock` | Unix socket path |
| `max_connections` | `usize` | `100` | Max concurrent connections |
| `connection_timeout` | `Duration` | `30s` | Idle connection timeout |
| `rate_limit` | `Option<RateLimitConfig>` | `None` | Request rate limiting |

---

## Message Type Registration

All message types sent over IPC must be registered with the type registry.

### Registration Pattern

```rust
use serde::{Deserialize, Serialize};

// Message must implement these traits for IPC
#[derive(Clone, Debug, Serialize, Deserialize)]
struct MyRequest {
    query: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MyResponse {
    result: String,
}

// Register with string identifier
let registry = runtime.ipc_registry();
registry.register::<MyRequest>("MyRequest");
registry.register::<MyResponse>("MyResponse");
```

### Type Name Guidelines

- Use the struct name as the type identifier
- Keep names consistent between client and server
- Consider namespacing: `"calculator.AddRequest"`

### Checking Registration

```rust
if registry.is_registered("MyRequest") {
    println!("Type is registered");
}

// List all registered types
let types = registry.registered_types();
println!("Registered types: {:?}", types);
```

---

## Exposing Agents

### Basic Exposure

```rust
// Expose a single agent
runtime.ipc_expose("calculator", calculator_handle);

// Expose multiple agents
runtime.ipc_expose("kv_store", kv_store_handle);
runtime.ipc_expose("price_feed", price_feed_handle);
```

### Hiding Agents

```rust
// Remove agent from IPC (but keep it running)
runtime.ipc_hide("calculator");
```

### Dynamic Exposure

```rust
// Expose agents dynamically based on configuration
if config.enable_calculator {
    runtime.ipc_expose("calculator", calc_handle);
}
```

---

## Communication Patterns

### Pattern 1: Request-Response

Client sends a request, agent sends a single response.

**Server Side:**

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
struct AddRequest { a: i32, b: i32 }

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AddResult { sum: i32 }

// Register types
registry.register::<AddRequest>("AddRequest");
registry.register::<AddResult>("AddResult");

// Handler
calculator.mutate_on::<AddRequest>(|_agent, ctx| {
    let a = ctx.message().a;
    let b = ctx.message().b;
    let reply = ctx.reply_envelope();

    Box::pin(async move {
        reply.send(AddResult { sum: a + b }).await;
    })
});
```

**Client Side (Pseudocode):**

```text
SEND: IpcEnvelope {
    target: "calculator",
    message_type: "AddRequest",
    payload: { a: 5, b: 3 },
    expects_reply: true
}

RECEIVE: IpcResponse {
    success: true,
    payload: { sum: 8 }
}
```

---

### Pattern 2: Request-Stream

Client sends a request, agent sends multiple response frames.

**Server Side:**

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ListRequest { page_size: usize }

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ListItem { id: u64, name: String }

// Handler sends multiple responses
agent.mutate_on::<ListRequest>(|agent, ctx| {
    let page_size = ctx.message().page_size;
    let items = agent.model.items.clone();
    let reply = ctx.reply_envelope();

    Box::pin(async move {
        for chunk in items.chunks(page_size) {
            for item in chunk {
                reply.send(ListItem {
                    id: item.id,
                    name: item.name.clone(),
                }).await;
            }
        }
    })
});
```

**Client Side (Pseudocode):**

```text
SEND: IpcEnvelope {
    target: "list_service",
    message_type: "ListRequest",
    payload: { page_size: 10 },
    expects_stream: true
}

RECEIVE: IpcStreamFrame { sequence: 0, payload: {...}, is_final: false }
RECEIVE: IpcStreamFrame { sequence: 1, payload: {...}, is_final: false }
RECEIVE: IpcStreamFrame { sequence: 2, payload: {...}, is_final: true }
```

---

### Pattern 3: Push Notifications (Subscriptions)

Client subscribes to message types and receives pushed notifications.

**Server Side:**

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PriceUpdate { symbol: String, price: f64 }

// Register subscribable type
registry.register::<PriceUpdate>("PriceUpdate");

// Background task broadcasts updates
let broker = runtime.broker();
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        broker.broadcast(PriceUpdate {
            symbol: "ACME".to_string(),
            price: get_current_price(),
        }).await;
    }
});
```

**Client Side (Pseudocode):**

```text
SEND: SubscribeRequest {
    message_types: ["PriceUpdate", "TradeExecuted"]
}

// Continuous stream of push notifications
RECEIVE: IpcPushNotification { message_type: "PriceUpdate", payload: {...} }
RECEIVE: IpcPushNotification { message_type: "PriceUpdate", payload: {...} }
...
```

---

## Wire Protocol

### Frame Format

All messages use length-prefixed framing:

```text
┌───────────────────┬────────────────────────────┐
│  Length (4 bytes) │  Payload (JSON/MessagePack)│
│    Big-endian     │                            │
└───────────────────┴────────────────────────────┘
```

### IpcEnvelope Structure

```json
{
    "target": "service_name",
    "message_type": "RequestType",
    "payload": { "field": "value" },
    "request_id": "optional-correlation-id",
    "expects_reply": true,
    "expects_stream": false,
    "reply_to": "optional-reply-address"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `target` | `string` | Yes | Target agent name |
| `message_type` | `string` | Yes | Registered type name |
| `payload` | `object` | Yes | Message data |
| `request_id` | `string` | No | Correlation ID |
| `expects_reply` | `bool` | No | Request-response mode |
| `expects_stream` | `bool` | No | Request-stream mode |
| `reply_to` | `string` | No | Custom reply address |

### IpcResponse Structure

```json
{
    "request_id": "correlation-id",
    "success": true,
    "payload": { "result": "value" },
    "error": null
}
```

### IpcStreamFrame Structure

```json
{
    "stream_id": "stream-correlation-id",
    "sequence": 0,
    "payload": { "data": "value" },
    "is_final": false
}
```

### IpcPushNotification Structure

```json
{
    "notification_type": "subscription",
    "message_type": "PriceUpdate",
    "payload": { "symbol": "ACME", "price": 123.45 },
    "timestamp": 1699999999999
}
```

---

## Client Implementation

### Python Example

```python
import socket
import json
import struct

class ActonClient:
    def __init__(self, socket_path="/tmp/acton.sock"):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(socket_path)

    def send(self, target, message_type, payload, expects_reply=True):
        envelope = {
            "target": target,
            "message_type": message_type,
            "payload": payload,
            "expects_reply": expects_reply
        }
        data = json.dumps(envelope).encode('utf-8')
        # Length prefix (big-endian 4 bytes)
        self.sock.send(struct.pack('>I', len(data)) + data)

    def receive(self):
        # Read length prefix
        length_bytes = self.sock.recv(4)
        if not length_bytes:
            return None
        length = struct.unpack('>I', length_bytes)[0]
        # Read payload
        data = b''
        while len(data) < length:
            chunk = self.sock.recv(length - len(data))
            if not chunk:
                raise ConnectionError("Connection closed")
            data += chunk
        return json.loads(data.decode('utf-8'))

    def close(self):
        self.sock.close()

# Usage
client = ActonClient()
client.send("calculator", "AddRequest", {"a": 5, "b": 3})
response = client.receive()
print(f"Result: {response['payload']['sum']}")  # 8
client.close()
```

### Node.js Example

```javascript
const net = require('net');

class ActonClient {
    constructor(socketPath = '/tmp/acton.sock') {
        this.client = net.createConnection(socketPath);
        this.buffer = Buffer.alloc(0);
        this.responseCallbacks = [];
    }

    send(target, messageType, payload, expectsReply = true) {
        return new Promise((resolve, reject) => {
            const envelope = {
                target,
                message_type: messageType,
                payload,
                expects_reply: expectsReply
            };
            const data = Buffer.from(JSON.stringify(envelope));
            const length = Buffer.alloc(4);
            length.writeUInt32BE(data.length);

            this.responseCallbacks.push({ resolve, reject });
            this.client.write(Buffer.concat([length, data]));
        });
    }

    listen() {
        this.client.on('data', (chunk) => {
            this.buffer = Buffer.concat([this.buffer, chunk]);
            this.processBuffer();
        });
    }

    processBuffer() {
        while (this.buffer.length >= 4) {
            const length = this.buffer.readUInt32BE(0);
            if (this.buffer.length < 4 + length) break;

            const payload = this.buffer.slice(4, 4 + length);
            this.buffer = this.buffer.slice(4 + length);

            const response = JSON.parse(payload.toString());
            const callback = this.responseCallbacks.shift();
            if (callback) {
                callback.resolve(response);
            }
        }
    }

    close() {
        this.client.end();
    }
}

// Usage
const client = new ActonClient();
client.listen();

client.send('calculator', 'AddRequest', { a: 5, b: 3 })
    .then(response => {
        console.log('Result:', response.payload.sum);  // 8
        client.close();
    });
```

### Rust Client Example

```rust
use tokio::net::UnixStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct IpcEnvelope {
    target: String,
    message_type: String,
    payload: serde_json::Value,
    expects_reply: bool,
}

#[derive(Deserialize)]
struct IpcResponse {
    success: bool,
    payload: Option<serde_json::Value>,
    error: Option<String>,
}

async fn send_request(
    stream: &mut UnixStream,
    target: &str,
    message_type: &str,
    payload: serde_json::Value,
) -> anyhow::Result<IpcResponse> {
    let envelope = IpcEnvelope {
        target: target.to_string(),
        message_type: message_type.to_string(),
        payload,
        expects_reply: true,
    };

    let data = serde_json::to_vec(&envelope)?;
    let length = (data.len() as u32).to_be_bytes();

    stream.write_all(&length).await?;
    stream.write_all(&data).await?;

    let mut length_buf = [0u8; 4];
    stream.read_exact(&mut length_buf).await?;
    let length = u32::from_be_bytes(length_buf) as usize;

    let mut response_buf = vec![0u8; length];
    stream.read_exact(&mut response_buf).await?;

    Ok(serde_json::from_slice(&response_buf)?)
}
```

---

## Error Handling

### IpcError Types

```rust
pub enum IpcError {
    /// Message type not registered
    UnknownMessageType(String),

    /// Target agent not found
    AgentNotFound(String),

    /// JSON/MessagePack serialization error
    SerializationError(String),

    /// Agent inbox full (backpressure)
    TargetBusy(String),

    /// Protocol violation
    ProtocolError(String),

    /// Rate limit exceeded
    RateLimited,

    /// Connection closed unexpectedly
    ConnectionClosed,

    /// Operation timed out
    Timeout,
}
```

### Error Response

```json
{
    "success": false,
    "payload": null,
    "error": "UnknownMessageType: InvalidRequest is not registered"
}
```

### Client-Side Handling

```python
response = client.receive()
if not response['success']:
    error = response['error']
    if 'UnknownMessageType' in error:
        print("Server doesn't recognize message type")
    elif 'AgentNotFound' in error:
        print("Target service not available")
    elif 'RateLimited' in error:
        print("Too many requests, backing off...")
```

---

## Best Practices

### 1. Type Registration

Register all types at startup before starting the listener:

```rust
// Good: Register all at startup
registry.register::<Request1>("Request1");
registry.register::<Response1>("Response1");
registry.register::<Request2>("Request2");
registry.register::<Response2>("Response2");

// Then start listener
let listener = runtime.start_ipc_listener().await?;
```

### 2. Socket Cleanup

Clean up sockets on shutdown:

```rust
// Before starting, check for stale sockets
let socket_path = Path::new("/tmp/acton.sock");
if socket_path.exists() {
    std::fs::remove_file(socket_path)?;
}

// Start listener
let listener = runtime.start_ipc_listener().await?;

// On shutdown
listener.shutdown().await?;
```

### 3. Connection Timeouts

Set appropriate timeouts for your use case:

```rust
let config = IpcConfig {
    // Short timeout for request-response
    connection_timeout: Duration::from_secs(30),
    ..Default::default()
};

// Or longer for subscriptions
let config = IpcConfig {
    connection_timeout: Duration::from_secs(3600), // 1 hour
    ..Default::default()
};
```

### 4. Rate Limiting

Enable rate limiting for public-facing services:

```rust
let config = IpcConfig {
    rate_limit: Some(RateLimitConfig {
        requests_per_second: 100,
        burst_size: 20,
    }),
    ..Default::default()
};
```

### 5. Monitoring

Track listener statistics:

```rust
let listener = runtime.start_ipc_listener().await?;

// Periodic monitoring
tokio::spawn(async move {
    loop {
        let stats = listener.stats();
        println!(
            "Connections: {}, Messages: {}, Errors: {}",
            stats.active_connections,
            stats.messages_processed,
            stats.errors
        );
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
});
```

### 6. Health Checks

Implement health check endpoints:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
struct HealthCheck;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct HealthStatus { status: String }

registry.register::<HealthCheck>("HealthCheck");
registry.register::<HealthStatus>("HealthStatus");

agent.act_on::<HealthCheck>(|_, ctx| {
    let reply = ctx.reply_envelope();
    Box::pin(async move {
        reply.send(HealthStatus { status: "ok".to_string() }).await;
    })
});
```
