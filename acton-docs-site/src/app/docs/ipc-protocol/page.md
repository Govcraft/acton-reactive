---
title: IPC Protocol
nextjs:
  metadata:
    title: IPC Protocol - acton-reactive
    description: Wire format, message structures, and error handling for IPC communication.
---

This page documents the wire protocol for IPC communication, including frame formats, message structures, and error handling.

---

## Wire Format

### Frame Structure

All messages use length-prefixed framing:

```text
┌───────────────────┬────────────────────────────┐
│  Length (4 bytes) │  Payload (JSON/MessagePack)│
│    Big-endian     │                            │
└───────────────────┴────────────────────────────┘
```

The length field contains the size of the payload in bytes, encoded as a 4-byte big-endian unsigned integer.

---

## Message Structures

### IpcEnvelope (Request)

Clients send requests using this structure:

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
| `target` | `string` | Yes | Target actor name |
| `message_type` | `string` | Yes | Registered type name |
| `payload` | `object` | Yes | Message data |
| `request_id` | `string` | No | Correlation ID for matching responses |
| `expects_reply` | `bool` | No | Request-response mode |
| `expects_stream` | `bool` | No | Request-stream mode |
| `reply_to` | `string` | No | Custom reply address |

### IpcResponse (Single Response)

For request-response patterns:

```json
{
    "request_id": "correlation-id",
    "success": true,
    "payload": { "result": "value" },
    "error": null
}
```

| Field | Type | Description |
|-------|------|-------------|
| `request_id` | `string` | Matches the request's correlation ID |
| `success` | `bool` | Whether the operation succeeded |
| `payload` | `object\|null` | Response data (if successful) |
| `error` | `string\|null` | Error message (if failed) |

### IpcStreamFrame (Streaming Response)

For request-stream patterns:

```json
{
    "stream_id": "stream-correlation-id",
    "sequence": 0,
    "payload": { "data": "value" },
    "is_final": false
}
```

| Field | Type | Description |
|-------|------|-------------|
| `stream_id` | `string` | Correlation ID for the stream |
| `sequence` | `u64` | Frame order (0-indexed) |
| `payload` | `object` | Frame data |
| `is_final` | `bool` | True for the last frame |

### IpcPushNotification (Subscription)

For push notifications via subscriptions:

```json
{
    "notification_type": "subscription",
    "message_type": "PriceUpdate",
    "payload": { "symbol": "ACME", "price": 123.45 },
    "timestamp": 1699999999999
}
```

| Field | Type | Description |
|-------|------|-------------|
| `notification_type` | `string` | Always "subscription" |
| `message_type` | `string` | The subscribed message type |
| `payload` | `object` | Notification data |
| `timestamp` | `u64` | Unix timestamp in milliseconds |

---

## Error Handling

### Error Types

```rust
pub enum IpcError {
    /// Message type not registered
    UnknownMessageType(String),

    /// Target actor not found
    ActorNotFound(String),

    /// JSON/MessagePack serialization error
    SerializationError(String),

    /// Actor inbox full (backpressure)
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

### Error Response Format

```json
{
    "success": false,
    "payload": null,
    "error": "UnknownMessageType: InvalidRequest is not registered"
}
```

### Common Errors

| Error | Cause | Solution |
|-------|-------|----------|
| `UnknownMessageType` | Type not registered | Register with `registry.register::<Type>()` |
| `ActorNotFound` | Actor not exposed | Use `runtime.ipc_expose()` |
| `SerializationError` | Invalid JSON/MessagePack | Check payload format |
| `TargetBusy` | Actor inbox full | Retry with backoff |
| `RateLimited` | Too many requests | Back off and retry |

---

## Protocol Examples

### Reading a Frame (Pseudocode)

```text
1. Read 4 bytes for length (big-endian)
2. Read `length` bytes for payload
3. Parse payload as JSON or MessagePack
4. Process the message
```

### Writing a Frame (Pseudocode)

```text
1. Serialize message to JSON or MessagePack
2. Calculate payload length
3. Write length as 4 big-endian bytes
4. Write payload bytes
```

---

## Client Implementation Examples

### Python

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
```

### Node.js

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
```

### Rust

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

## Next Steps

- [IPC Setup](/docs/ipc-setup) - Enable and configure IPC
- [IPC Patterns](/docs/ipc-patterns) - Request-response, streaming, subscriptions
- [Examples](/docs/examples) - Working IPC examples
