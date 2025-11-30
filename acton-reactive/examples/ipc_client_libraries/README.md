# IPC Client Libraries

This directory contains client library implementations for communicating with `acton-reactive` IPC servers from external languages. These libraries implement the full IPC protocol, enabling seamless integration between Rust actor systems and applications written in Python, Node.js, and other languages.

## Protocol Overview

The `acton-reactive` IPC protocol uses Unix domain sockets with length-prefixed binary framing:

```
┌─────────────────────────────────────────┐
│ Frame Length (4 bytes, big-endian u32)  │  Payload size only
├─────────────────────────────────────────┤
│ Protocol Version (1 byte)                │  0x02 for v2
├─────────────────────────────────────────┤
│ Message Type (1 byte)                   │  See below
├─────────────────────────────────────────┤
│ Format Byte (1 byte)                    │  0x01 = JSON, 0x02 = MessagePack
├─────────────────────────────────────────┤
│ Payload (remaining bytes)                │  Serialized data
└─────────────────────────────────────────┘
```

### Message Types

| Type | Byte | Direction | Purpose |
|------|------|-----------|---------|
| REQUEST | 0x01 | Client → Server | Send message to agent |
| RESPONSE | 0x02 | Server → Client | Successful response |
| ERROR | 0x03 | Server → Client | Error response |
| HEARTBEAT | 0x04 | Both | Keep-alive/ping |
| PUSH | 0x05 | Server → Client | Subscription notification |
| SUBSCRIBE | 0x06 | Client → Server | Subscribe to message types |
| UNSUBSCRIBE | 0x07 | Client → Server | Unsubscribe |
| DISCOVER | 0x08 | Client → Server | Service discovery |
| STREAM | 0x09 | Server → Client | Streaming response frame |

### Communication Patterns

1. **Request-Response**: Send a request, receive a single response
2. **Fire-and-Forget**: Send a message without expecting a response
3. **Streaming**: Request triggers multiple response frames
4. **Push Notifications**: Subscribe to message types, receive notifications

## Quick Start

### 1. Start the Example Server

```bash
cd /path/to/acton-reactive
cargo run --example ipc_client_libraries_server --features ipc
```

### 2. Run Client Examples

**Python:**
```bash
cd acton-reactive/examples/ipc_client_libraries/python
pip install msgpack  # Optional, for MessagePack support
python example_client.py
```

**Node.js:**
```bash
cd acton-reactive/examples/ipc_client_libraries/nodejs
npm install
npx ts-node src/example-client.ts
```

## Python Client

### Installation

The Python client has no required dependencies (uses standard library only). For MessagePack support:

```bash
pip install msgpack
```

### Usage

```python
import asyncio
from acton_ipc import ActonIpcClient, get_default_socket_path

async def main():
    socket_path = get_default_socket_path('ipc_client_example')

    async with ActonIpcClient(socket_path) as client:
        # Request-Response
        response = await client.request('calculator', 'Add', {'a': 5, 'b': 3})
        print(f"Result: {response.payload}")

        # Fire-and-Forget
        await client.fire_and_forget('logger', 'LogEvent', {
            'level': 'info',
            'message': 'Hello from Python!'
        })

        # Streaming
        async for frame in client.stream('search', 'SearchQuery', {'query': 'test', 'limit': 5}):
            print(f"Result: {frame.payload}")
            if frame.is_final:
                break

        # Subscriptions
        def on_price(notification):
            print(f"Price: {notification.payload}")

        client.on_push(on_price)
        await client.subscribe(['PriceUpdate'])

asyncio.run(main())
```

### Synchronous API

For simpler use cases, a synchronous client is also available:

```python
from acton_ipc import ActonIpcClientSync

with ActonIpcClientSync(socket_path) as client:
    response = client.request('calculator', 'Add', {'a': 5, 'b': 3})
    print(f"Result: {response.payload}")
```

## Node.js/TypeScript Client

### Installation

```bash
npm install
# Optional: npm install @msgpack/msgpack  # For MessagePack support
```

### Usage

```typescript
import { ActonIpcClient, getDefaultSocketPath } from './src/index.js';

const socketPath = getDefaultSocketPath('ipc_client_example');
const client = new ActonIpcClient(socketPath);

await client.connect();

// Request-Response
const response = await client.request('calculator', 'Add', { a: 5, b: 3 });
console.log(`Result: ${JSON.stringify(response.payload)}`);

// Fire-and-Forget
await client.fireAndForget('logger', 'LogEvent', {
  level: 'info',
  message: 'Hello from Node.js!'
});

// Streaming
for await (const frame of client.stream('search', 'SearchQuery', { query: 'test', limit: 5 })) {
  console.log(`Result: ${JSON.stringify(frame.payload)}`);
  if (frame.is_final) break;
}

// Subscriptions
client.onPush((notification) => {
  console.log(`Price: ${JSON.stringify(notification.payload)}`);
});
await client.subscribe(['PriceUpdate']);

await client.close();
```

## Available Services (Example Server)

The example server exposes these services:

| Service | Message Type | Pattern | Description |
|---------|--------------|---------|-------------|
| calculator | Add | Request-Response | Add two numbers `{a, b}` |
| calculator | Multiply | Request-Response | Multiply two numbers `{a, b}` |
| search | SearchQuery | Streaming | Search with `{query, limit}`, streams results |
| logger | LogEvent | Fire-and-Forget | Log `{level, message}` |
| (broker) | PriceUpdate | Push | Price updates every 5 seconds |
| (broker) | StatusChange | Push | System status changes |

## Service Discovery

Both clients support service discovery to query available agents and message types:

**Python:**
```python
discovery = await client.discover()
print(f"Agents: {discovery.agents}")
print(f"Message Types: {discovery.message_types}")
```

**Node.js:**
```typescript
const discovery = await client.discover();
console.log(`Agents: ${discovery.agents.map(a => a.name)}`);
console.log(`Message Types: ${discovery.message_types}`);
```

## Socket Path Convention

The socket path follows XDG conventions:

```
$XDG_RUNTIME_DIR/acton/<app_name>/ipc.sock
```

Default fallback (if `XDG_RUNTIME_DIR` is not set):
```
/tmp/acton/<app_name>/ipc.sock
```

## Error Handling

Both clients handle these error types:

| Error | Description |
|-------|-------------|
| ConnectionError | Socket connection failed |
| TimeoutError | Request exceeded timeout |
| ServerError | Server returned an error (includes error code) |
| ProtocolError | Invalid protocol format |

Error codes from server:
- `UNKNOWN_MESSAGE_TYPE` - Message type not registered
- `AGENT_NOT_FOUND` - Target agent not exposed
- `SERIALIZATION_ERROR` - JSON/MessagePack parsing failed
- `TARGET_BUSY` - Agent inbox full (backpressure)
- `TIMEOUT` - Server-side timeout
- `RATE_LIMITED` - Rate limit exceeded

## MessagePack Support

For ~30-50% smaller messages and faster parsing, enable MessagePack:

**Python:**
```python
client = ActonIpcClient(socket_path, use_messagepack=True)
```

**Node.js:**
```typescript
const client = new ActonIpcClient(socketPath, { useMessagePack: true });
```

Requires the `msgpack` package (Python) or `@msgpack/msgpack` (Node.js).

## Building Your Own Client

To implement a client in another language, you need to:

1. Connect to Unix domain socket at the calculated path
2. Implement frame encoding/decoding (7-byte header for v2)
3. Support JSON serialization (MessagePack optional)
4. Generate unique correlation IDs for request tracking
5. Handle async responses (map correlation IDs to pending requests)
6. Implement timeout handling per request

See the Python and Node.js implementations for reference.

## License

These client libraries are provided under the same license as `acton-reactive` (Apache 2.0 / MIT dual license).
