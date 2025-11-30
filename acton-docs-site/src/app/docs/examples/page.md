---
title: Examples
nextjs:
  metadata:
    title: Examples - acton-reactive
    description: Learn from working code. Each example demonstrates specific patterns and concepts.
---

Learn from working code. Each example demonstrates specific patterns and concepts you can apply in your own applications.

{% quick-links %}

{% quick-link title="Basic Actor" icon="installation" href="#basic-actor" description="State, messages, handlers, and replies" /%}

{% quick-link title="Lifecycle Hooks" icon="presets" href="#lifecycle-hooks" description="before_start, after_stop, and timing" /%}

{% quick-link title="Broadcasting" icon="plugins" href="#broadcast-messaging" description="Pub/sub with the broker" /%}

{% quick-link title="Fruit Market" icon="theming" href="#fruit-market" description="Multi-actor coordination" /%}

{% /quick-links %}

---

## Basic Actor

**Location:** `examples/basic/main.rs`

Demonstrates fundamental actor concepts: state management, message handling, and request-reply patterns.

```rust
// Define actor state
#[acton_actor]
struct BasicActorState {
    some_state: usize,
}

// Define messages
#[acton_message]
struct PingMsg;

#[acton_message]
struct PongMsg;

// Configure handlers
actor_builder
    .mutate_on::<PingMsg>(|actor, ctx| {
        actor.model.some_state += 1;
        let reply = ctx.reply_envelope();
        Reply::pending(async move {
            reply.send(PongMsg).await;
        })
    })
    .mutate_on::<PongMsg>(|actor, _| {
        actor.model.some_state += 1;
        Reply::ready()
    })
    .after_stop(|actor| {
        assert_eq!(actor.model.some_state, 2);
        Reply::ready()
    });
```

**Run it:**
```bash
cargo run --example basic
```

**Concepts covered:**
- Actor state with `#[acton_actor]`
- Messages with `#[acton_message]`
- Handler registration with `mutate_on`
- Reply envelopes for request-reply
- State assertions in `after_stop`

---

## Lifecycle Hooks

**Location:** `examples/lifecycles/main.rs`

Shows all four lifecycle hooks and their execution order.

```rust
tracker_actor
    .before_start(|_| {
        println!("Preparing to track items...");
        Reply::ready()
    })
    .after_start(|_| {
        println!("Now tracking items!");
        Reply::ready()
    })
    .mutate_on::<AddItem>(|actor, ctx| {
        actor.model.items.push(ctx.message().0.clone());
        Reply::ready()
    })
    .before_stop(|_| {
        println!("Finishing up...");
        Reply::ready()
    })
    .after_stop(|actor| {
        println!("Final items: {:?}", actor.model.items);
        Reply::ready()
    });
```

**Execution order:**
```text
before_start → message loop → after_start → [processing] → before_stop → after_stop
```

**Run it:**
```bash
cargo run --example lifecycles
```

---

## Broadcast Messaging

**Location:** `examples/broadcast/main.rs`

Demonstrates pub/sub messaging using the broker.

```text
┌──────────────────┐    broadcast    ┌─────────────────┐
│ Main Application │ ──────────────→ │  ActorBroker    │
└──────────────────┘                 └────────┬────────┘
                                              │
                    ┌─────────────────────────┼─────────────────────────┐
                    ▼                         ▼                         ▼
           ┌───────────────┐         ┌───────────────┐         ┌───────────────┐
           │ DataCollector │         │  Aggregator   │         │    Printer    │
           └───────────────┘         └───────────────┘         └───────────────┘
```

```rust
// Get the broker
let broker = runtime.broker();

// Subscribe before starting
data_collector.handle().subscribe::<NewData>().await;
aggregator.handle().subscribe::<NewData>().await;
printer.handle().subscribe::<StatusUpdate>().await;

// Start actors
let _dc = data_collector.start().await;
let _agg = aggregator.start().await;
let _printer = printer.start().await;

// Broadcast - all subscribed actors receive
broker.broadcast(NewData(5)).await;
broker.broadcast(NewData(10)).await;
```

**Run it:**
```bash
cargo run --example broadcast
```

---

## Configuration

**Location:** `examples/configuration/main.rs`

Shows how to load and use configuration files.

```rust
// Acton automatically loads from XDG locations:
// Linux: ~/.config/acton/config.toml
// macOS: ~/Library/Application Support/acton/config.toml
// Windows: %APPDATA%\acton\config.toml

let runtime = ActonApp::launch();
// Config is automatically loaded and available
```

**Run it:**
```bash
cargo run --example configuration
```

See [Configuration](/docs/configuration) for the full configuration guide.

---

## Fruit Market

**Location:** `examples/fruit_market/`

A complete multi-actor application showing real-world patterns.

**Components:**
- **Cart Actor**: Manages shopping cart items
- **Register Actor**: Handles checkout
- **Price Service**: Provides price lookups
- **Printer Actor**: Displays receipts

**Patterns demonstrated:**
- Multi-actor coordination
- Actor-to-actor messaging
- Complex state management
- Request-reply across actors

**Run it:**
```bash
cargo run --example fruit_market
```

---

## IPC Examples

These examples require the `ipc` feature and demonstrate inter-process communication.

### IPC Basic

**Location:** `examples/ipc_basic/main.rs`

Foundation example for IPC setup.

```rust
// 1. Get IPC registry
let registry = runtime.ipc_registry();

// 2. Register message types
registry.register::<PriceUpdate>("PriceUpdate");
registry.register::<GetPrice>("GetPrice");

// 3. Expose actor via IPC
runtime.ipc_expose("prices", price_handle);

// 4. Start listener
let listener = runtime.start_ipc_listener().await?;
```

**Run it:**
```bash
cargo run --example ipc_basic
```

---

### IPC Bidirectional

**Location:** `examples/ipc_bidirectional_demo/`

Request-response patterns over IPC.

```text
┌──────────────┐     Request      ┌──────────────┐
│    Client    │ ───────────────→ │ IPC Listener │
│              │                  └──────┬───────┘
│              │                         │
│              │     Response            ▼
│              │ ←────────────── │  Calculator  │
└──────────────┘                 └──────────────┘
```

**Run it:**
```bash
# Terminal 1
cargo run --example ipc_bidirectional_demo --bin server

# Terminal 2
cargo run --example ipc_bidirectional_demo --bin client
```

---

### IPC Streaming

**Location:** `examples/ipc_streaming/`

Multi-frame streaming responses.

```rust
// Handler sends multiple responses
actor.mutate_on::<CountdownRequest>(|_actor, ctx| {
    let start = ctx.message().from;
    let reply = ctx.reply_envelope();

    Reply::pending(async move {
        for i in (1..=start).rev() {
            reply.send(CountdownTick { value: i }).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
});
```

**Run it:**
```bash
# Terminal 1
cargo run --example ipc_streaming --bin server

# Terminal 2
cargo run --example ipc_streaming --bin client
```

---

### IPC Subscriptions

**Location:** `examples/ipc_subscriptions/`

Push notifications via broker subscriptions.

```text
                              ┌──────────────┐
                              │  Price Feed  │
                              └──────┬───────┘
                                     │ broadcast
                                     ▼
┌──────────────┐              ┌──────────────┐
│  Client A    │←─────────────│  ActorBroker │
│ (subscribed) │  push        └──────┬───────┘
└──────────────┘                     │
                                     │
┌──────────────┐                     │
│  Client B    │←────────────────────┘
│ (subscribed) │  push
└──────────────┘
```

**Run it:**
```bash
# Terminal 1
cargo run --example ipc_subscriptions --bin server

# Terminal 2
cargo run --example ipc_subscriptions --bin client
```

---

### IPC Client Libraries

**Location:** `examples/ipc_client_libraries/`

Polyglot clients in Python and Node.js.

**Python client:**
```python
sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect('/tmp/acton.sock')

send_request(sock, 'calculator', 'AddRequest', {'a': 5, 'b': 3})
response = read_response(sock)
print(f"Result: {response['payload']['result']}")  # 8
```

**Node.js client:**
```javascript
const client = net.createConnection('/tmp/acton.sock');
sendRequest('calculator', 'AddRequest', { a: 5, b: 3 });
// Response: { result: 8 }
```

**Run it:**
```bash
# Terminal 1
cargo run --example ipc_client_libraries

# Terminal 2
cd examples/ipc_client_libraries/python && python client.py
# or
cd examples/ipc_client_libraries/node && node client.js
```

---

## Quick Reference

| Example | Pattern | Key Concepts |
|---------|---------|--------------|
| basic | Direct messaging | State, replies, assertions |
| lifecycles | Lifecycle management | Hooks, timing |
| broadcast | Pub/Sub | Broker, subscriptions |
| configuration | Config loading | XDG paths, TOML |
| fruit_market | Multi-actor system | Coordination, real-world patterns |
| ipc_basic | IPC setup | Type registry, exposure |
| ipc_bidirectional | Request-Response over IPC | Multiple services |
| ipc_streaming | Streaming responses | Multi-frame, pagination |
| ipc_subscriptions | Push notifications | Broker over IPC |
| ipc_client_libraries | Polyglot clients | Python, Node.js |

---

## Running All Examples

```bash
# List all examples
cargo run --example

# Run a specific example
cargo run --example basic
cargo run --example lifecycles
cargo run --example broadcast
cargo run --example fruit_market

# IPC examples need the feature flag
cargo run --example ipc_basic --features ipc
```
