---
title: Examples
nextjs:
  metadata:
    title: Examples - acton-reactive
    description: Detailed walkthroughs of all example applications in the acton-reactive crate.
---

This guide provides detailed walkthroughs of all example applications in the `acton-reactive` crate.

---

## Basic Examples

### Basic Actor

**Location:** `examples/basic/main.rs`

**Purpose:** Demonstrates fundamental actor concepts including state management, message handling, and request-reply patterns.

**Key Concepts:**
- Creating an actor with custom state
- Defining message types
- Registering message handlers with `mutate_on`
- Sending replies to message senders
- Using `ActorReply` helpers

**Code Walkthrough:**

```rust
// 1. Define actor state using the acton_actor macro
#[acton_actor]
struct BasicActorState {
    some_state: usize,
}

// 2. Define messages
#[acton_message]
struct PingMsg;

#[acton_message]
struct PongMsg;

#[acton_message]
struct BuhByeMsg;

// 3. Configure handlers
actor_builder
    // Handle Ping: increment state, reply with Pong
    .mutate_on::<PingMsg>(|actor, envelope| {
        actor.model.some_state += 1;
        let reply_envelope = envelope.reply_envelope();
        Box::pin(async move {
            reply_envelope.send(PongMsg).await;
        })
    })
    // Handle Pong: increment state, send BuhBye to self
    .mutate_on::<PongMsg>(|actor, _envelope| {
        actor.model.some_state += 1;
        let self_handle = actor.handle().clone();
        ActorReply::from_async(async move {
            self_handle.send(BuhByeMsg).await;
        })
    })
    // Handle BuhBye: just log
    .mutate_on::<BuhByeMsg>(|_actor, _envelope| {
        println!("Thanks for all the fish! Buh Bye!");
        ActorReply::immediate()
    })
    // Verify final state
    .after_stop(|actor| {
        assert_eq!(actor.model.some_state, 2);
        ActorReply::immediate()
    });
```

**Running:**
```bash
cargo run --example basic
```

---

### Lifecycle Hooks

**Location:** `examples/lifecycles/main.rs`

**Purpose:** Demonstrates all four lifecycle hooks and their execution order.

**Key Concepts:**
- `before_start` - Runs before message loop
- `after_start` - Runs after message loop starts
- `before_stop` - Runs when stop is requested
- `after_stop` - Runs after actor fully stops
- Async operations in handlers

**Hook Execution Order:**

```text
┌─────────────────────────────────────────┐
│ start().await                           │
├─────────────────────────────────────────┤
│ 1. before_start()                       │
│ 2. Message loop begins                  │
│ 3. after_start()                        │
│    [Processing messages...]             │
├─────────────────────────────────────────┤
│ stop() or shutdown_all()                │
├─────────────────────────────────────────┤
│ 4. before_stop()                        │
│ 5. Message loop ends                    │
│ 6. after_stop()                         │
└─────────────────────────────────────────┘
```

**Code Walkthrough:**

```rust
tracker_actor_builder
    .before_start(|_| {
        println!("Actor is preparing to track items... Here we go!");
        ActorReply::immediate()
    })
    .after_start(|_| {
        println!("Actor is now tracking items!");
        ActorReply::immediate()
    })
    .mutate_on::<AddItem>(|actor, envelope| {
        actor.model.items.push(envelope.message().0.clone());
        ActorReply::immediate()
    })
    .mutate_on::<GetItems>(|actor, _| {
        let items = actor.model.items.clone();
        // Demonstrate async work in handler
        ActorReply::from_async(async move {
            sleep(Duration::from_secs(2)).await;
            println!("Current items: {items:?}");
        })
    })
    .before_stop(|_| {
        println!("Actor is stopping... finishing up!");
        ActorReply::immediate()
    })
    .after_stop(|actor| {
        println!("Actor stopped! Final items: {:?}", actor.model.items);
        ActorReply::immediate()
    });
```

**Running:**
```bash
cargo run --example lifecycles
```

---

### Broadcast Messaging

**Location:** `examples/broadcast/main.rs`

**Purpose:** Demonstrates pub/sub messaging using the broker.

**Key Concepts:**
- Using `ActorBroker` for message broadcasting
- Subscribing actors to message types
- Multiple actors receiving the same broadcast
- Coordination via shared messages

**Architecture:**

```text
┌──────────────────┐    broadcast    ┌─────────────────┐
│ Main Application │ ──────────────→ │  ActorBroker    │
└──────────────────┘                 └────────┬────────┘
                                              │
                    ┌─────────────────────────┼─────────────────────────┐
                    │                         │                         │
                    ▼                         ▼                         ▼
           ┌───────────────┐         ┌───────────────┐         ┌───────────────┐
           │ DataCollector │         │  Aggregator   │         │    Printer    │
           │ subscribes to │         │ subscribes to │         │ subscribes to │
           │   NewData     │         │   NewData     │         │ StatusUpdate  │
           └───────────────┘         └───────────────┘         └───────────────┘
```

**Code Walkthrough:**

```rust
// Get the broker
let broker_handle = runtime.broker();

// Configure actors to broadcast status updates
data_collector_builder
    .mutate_on::<NewData>(|actor, envelope| {
        actor.model.data_points.push(envelope.message().0);
        let broker = actor.broker().clone();
        let value = envelope.message().0;
        Box::pin(async move {
            broker.broadcast(StatusUpdate::Updated("DataCollector".into(), value)).await
        })
    });

// Subscribe actors BEFORE starting
data_collector_builder.handle().subscribe::<NewData>().await;
aggregator_builder.handle().subscribe::<NewData>().await;
printer_builder.handle().subscribe::<StatusUpdate>().await;

// Start actors
let _data_collector = data_collector_builder.start().await;
let _aggregator = aggregator_builder.start().await;
let _printer = printer_builder.start().await;

// Broadcast messages - all subscribed actors receive them
broker_handle.broadcast(NewData(5)).await;
broker_handle.broadcast(NewData(10)).await;
```

**Running:**
```bash
cargo run --example broadcast
```

---

### Configuration Loading

**Location:** `examples/configuration/main.rs`

**Purpose:** Demonstrates loading and using configuration files.

**Key Concepts:**
- XDG-compliant configuration locations
- Accessing the global `CONFIG` static
- Custom configuration for actors

**Running:**
```bash
cargo run --example configuration
```

---

### Fruit Market

**Location:** `examples/fruit_market/`

**Purpose:** A complex multi-actor application demonstrating real-world patterns.

**Components:**
- **Cart Actor**: Manages shopping cart items
- **Register Actor**: Handles checkout process
- **Price Service**: Provides price lookups
- **Printer Actor**: Displays receipts

**Key Concepts:**
- Multi-actor coordination
- Actor-to-actor messaging
- Complex state management
- Request-reply patterns

**Running:**
```bash
cargo run --example fruit_market
```

---

## IPC Examples

### IPC Basic

**Location:** `examples/ipc_basic/main.rs`

**Purpose:** Foundation example demonstrating IPC setup and basic communication.

**Key Concepts:**
- IPC type registry setup
- Actor exposure via IPC
- Socket utilities
- In-process IPC simulation

**Setup Pattern:**

```rust
// 1. Launch runtime and get IPC registry
let mut runtime = ActonApp::launch();
let registry = runtime.ipc_registry();

// 2. Register message types for IPC
registry.register::<PriceUpdate>("PriceUpdate");
registry.register::<GetPrice>("GetPrice");
registry.register::<PriceResponse>("PriceResponse");

// 3. Create and configure actor
let mut price_actor = runtime.new_actor::<PriceState>();
price_actor.mutate_on::<GetPrice>(|actor, ctx| {
    // Handle price lookups
});
let price_handle = price_actor.start().await;

// 4. Expose actor via IPC
runtime.ipc_expose("prices", price_handle.clone());

// 5. Start IPC listener
let config = IpcConfig::load();
let listener = runtime.start_ipc_listener_with_config(config).await?;
```

**Socket Utilities:**

```rust
// Check if socket exists
if socket_exists(&socket_path) {
    println!("Socket found at: {}", socket_path);
}

// Check if socket is responsive
if socket_is_alive(&socket_path) {
    println!("Socket is accepting connections");
}
```

**Running:**
```bash
cargo run --example ipc_basic
```

---

### IPC Bidirectional

**Location:** `examples/ipc_bidirectional_demo/`

**Purpose:** Demonstrates request-response patterns over IPC.

**Architecture:**

```text
┌──────────────┐     IpcEnvelope      ┌──────────────┐
│    Client    │ ──────────────────→  │ IPC Listener │
│              │                      └──────┬───────┘
│              │                             │
│              │                             ▼
│              │                      ┌──────────────┐
│              │                      │ Calculator   │
│              │                      │ or KV Store  │
│              │                      └──────┬───────┘
│              │                             │
│              │     IpcResponse             │
│              │ ←──────────────────────────┘
└──────────────┘
```

**Message Types:**

```rust
use acton_reactive::prelude::*;

// Calculator messages - use #[acton_message(ipc)] for IPC-compatible types
#[acton_message(ipc)]
struct AddRequest { a: i32, b: i32 }

#[acton_message(ipc)]
struct CalculationResult { result: i32 }

// KV Store messages
#[acton_message(ipc)]
struct SetValue { key: String, value: String }

#[acton_message(ipc)]
struct GetValue { key: String }

#[acton_message(ipc)]
struct ValueResponse { value: Option<String> }
```

**Server Setup:**

```rust
// Register request and response types
registry.register::<AddRequest>("AddRequest");
registry.register::<CalculationResult>("CalculationResult");
registry.register::<SetValue>("SetValue");
registry.register::<GetValue>("GetValue");
registry.register::<ValueResponse>("ValueResponse");

// Expose multiple services
runtime.ipc_expose("calculator", calculator_handle);
runtime.ipc_expose("kv_store", kv_store_handle);
```

**Running:**
```bash
# Terminal 1: Start server
cargo run --example ipc_bidirectional_demo --bin server

# Terminal 2: Run client
cargo run --example ipc_bidirectional_demo --bin client
```

---

### IPC Streaming

**Location:** `examples/ipc_streaming/`

**Purpose:** Demonstrates multi-frame streaming responses.

**Architecture:**

```text
┌──────────────┐   CountdownRequest   ┌──────────────┐
│    Client    │ ──────────────────→  │ IPC Listener │
│              │                      └──────┬───────┘
│              │                             │
│              │                             ▼
│              │                      ┌──────────────┐
│              │                      │  Countdown   │
│              │                      │    Actor     │
│              │                      └──────┬───────┘
│              │                             │
│              │   IpcStreamFrame (5)        │
│              │ ←─────────────────────────  │
│              │   IpcStreamFrame (4)        │
│              │ ←─────────────────────────  │
│              │   IpcStreamFrame (3)        │
│              │ ←─────────────────────────  │
│              │   IpcStreamFrame (2)        │
│              │ ←─────────────────────────  │
│              │   IpcStreamFrame (1, final) │
│              │ ←─────────────────────────  │
└──────────────┘
```

**Stream Frame Structure:**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcStreamFrame {
    pub stream_id: String,     // Correlation ID
    pub sequence: u64,         // Frame order
    pub payload: Value,        // Frame data
    pub is_final: bool,        // Last frame marker
}
```

**Handler Pattern:**

```rust
actor.mutate_on::<CountdownRequest>(|_actor, ctx| {
    let start = ctx.message().from;
    let delay = ctx.message().delay_ms;
    let reply = ctx.reply_envelope();

    Box::pin(async move {
        for i in (1..=start).rev() {
            // Send multiple responses
            reply.send(CountdownTick { value: i }).await;
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
    })
});
```

**Running:**
```bash
# Terminal 1: Start server
cargo run --example ipc_streaming --bin server

# Terminal 2: Run client
cargo run --example ipc_streaming --bin client
```

---

### IPC Subscriptions

**Location:** `examples/ipc_subscriptions/`

**Purpose:** Demonstrates push notifications via broker subscriptions.

**Architecture:**

```text
                                      ┌──────────────┐
                                      │  Price Feed  │
                                      │    Actor     │
                                      └──────┬───────┘
                                             │ broadcast
                                             ▼
┌──────────────┐                      ┌──────────────┐
│  Client A    │←────────────────────│  ActorBroker │
│ subscribed   │   IpcPushNotification└──────┬───────┘
│ to: Price    │                             │
└──────────────┘                             │
                                             │
┌──────────────┐                             │
│  Client B    │←────────────────────────────┘
│ subscribed   │   IpcPushNotification
│ to: Trade    │
└──────────────┘
```

**Message Types:**

```rust
use acton_reactive::prelude::*;

#[acton_message(ipc)]
struct PriceUpdate {
    symbol: String,
    price: f64,
    timestamp: u64,
}

#[acton_message(ipc)]
struct TradeExecuted {
    symbol: String,
    quantity: u32,
    price: f64,
}

#[acton_message(ipc)]
struct SystemStatus {
    status: String,
    message: String,
}
```

**Server Setup:**

```rust
// Register subscribable message types
registry.register::<PriceUpdate>("PriceUpdate");
registry.register::<TradeExecuted>("TradeExecuted");
registry.register::<SystemStatus>("SystemStatus");

// Background task broadcasts periodic updates
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        broker.broadcast(PriceUpdate {
            symbol: "ACME".to_string(),
            price: generate_price(),
            timestamp: now(),
        }).await;
    }
});
```

**Client Subscription:**

```rust
// Client sends subscription request
let subscribe_frame = IpcFrame {
    frame_type: MSG_TYPE_SUBSCRIBE,
    message_types: vec!["PriceUpdate", "TradeExecuted"],
};
write_frame(&mut stream, &subscribe_frame).await?;

// Client receives push notifications
loop {
    let notification: IpcPushNotification = read_frame(&mut stream).await?;
    println!("Received: {:?}", notification);
}
```

**Running:**
```bash
# Terminal 1: Start server
cargo run --example ipc_subscriptions --bin server

# Terminal 2: Run subscriber client
cargo run --example ipc_subscriptions --bin client
```

---

### IPC Client Libraries

**Location:** `examples/ipc_client_libraries/`

**Purpose:** Demonstrates IPC clients in Python and Node.js.

**Components:**
- `server.rs` - Rust server exposing calculator service
- `python/client.py` - Python IPC client
- `node/client.js` - Node.js IPC client

**Python Client Example:**

```python
import socket
import json
import struct

def send_request(sock, target, message_type, payload):
    envelope = {
        "target": target,
        "message_type": message_type,
        "payload": payload,
        "expects_reply": True
    }
    data = json.dumps(envelope).encode('utf-8')
    sock.send(struct.pack('>I', len(data)) + data)

def read_response(sock):
    length_bytes = sock.recv(4)
    length = struct.unpack('>I', length_bytes)[0]
    data = sock.recv(length)
    return json.loads(data)

# Connect and send request
sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect('/tmp/acton.sock')

send_request(sock, 'calculator', 'AddRequest', {'a': 5, 'b': 3})
response = read_response(sock)
print(f"Result: {response['payload']['result']}")  # Result: 8
```

**Node.js Client Example:**

```javascript
const net = require('net');

const client = net.createConnection('/tmp/acton.sock');

function sendRequest(target, messageType, payload) {
    const envelope = {
        target,
        message_type: messageType,
        payload,
        expects_reply: true
    };
    const data = Buffer.from(JSON.stringify(envelope));
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    client.write(Buffer.concat([length, data]));
}

client.on('connect', () => {
    sendRequest('calculator', 'AddRequest', { a: 5, b: 3 });
});

client.on('data', (data) => {
    const response = JSON.parse(data.slice(4).toString());
    console.log('Result:', response.payload.result);  // Result: 8
});
```

**Running:**
```bash
# Terminal 1: Start Rust server
cargo run --example ipc_client_libraries

# Terminal 2: Run Python client
cd examples/ipc_client_libraries/python
python client.py

# Or run Node.js client
cd examples/ipc_client_libraries/node
node client.js
```

---

## Example Patterns Summary

| Example | Pattern | Key Features |
|---------|---------|--------------|
| Basic | Direct messaging | State mutation, replies, self-messaging |
| Lifecycles | Lifecycle management | Hooks, async handlers |
| Broadcast | Pub/Sub | Broker, subscriptions, multi-actor |
| Fruit Market | Complex system | Multi-actor coordination |
| IPC Basic | IPC setup | Type registry, actor exposure |
| IPC Bidirectional | Request-Response | Multiple services, stateful actors |
| IPC Streaming | Multi-frame response | Pagination, countdowns |
| IPC Subscriptions | Push notifications | Broker-based subscriptions |
| IPC Client Libraries | Polyglot clients | Python, Node.js integration |
