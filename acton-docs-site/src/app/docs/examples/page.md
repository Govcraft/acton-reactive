---
title: Examples
nextjs:
  metadata:
    title: Examples - acton-reactive
    description: Learn from working code. Each example demonstrates specific patterns and concepts.
---

Learn from working code. Each example demonstrates specific patterns and concepts.

{% quick-links %}

{% quick-link title="Basic Actor" icon="installation" href="#basic-actor" description="State, messages, handlers, replies" /%}

{% quick-link title="Lifecycle Hooks" icon="presets" href="#lifecycle-hooks" description="Startup and shutdown timing" /%}

{% quick-link title="Broadcasting" icon="plugins" href="#broadcast-messaging" description="Pub/sub with the broker" /%}

{% quick-link title="Fruit Market" icon="theming" href="#fruit-market" description="Multi-actor coordination" /%}

{% /quick-links %}

---

## Basic Actor

**Location:** `examples/basic/main.rs`

Demonstrates fundamental actor concepts: state management, message handling, and request-reply patterns.

```rust
#[acton_actor]
struct BasicActorState {
    some_state: usize,
}

#[acton_message]
struct PingMsg;

#[acton_message]
struct PongMsg;

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

```bash
cargo run --example basic
```

---

## Lifecycle Hooks

**Location:** `examples/lifecycles/main.rs`

Shows all four lifecycle hooks and their execution order.

```rust
actor
    .before_start(|_| {
        println!("Preparing...");
        Reply::ready()
    })
    .after_start(|_| {
        println!("Ready!");
        Reply::ready()
    })
    .before_stop(|_| {
        println!("Finishing up...");
        Reply::ready()
    })
    .after_stop(|actor| {
        println!("Final state: {:?}", actor.model);
        Reply::ready()
    });
```

**Execution order:**
```text
before_start → message loop → after_start → [processing] → before_stop → after_stop
```

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
let broker = runtime.broker();

// Subscribe before starting
data_collector.handle().subscribe::<NewData>().await;
aggregator.handle().subscribe::<NewData>().await;
printer.handle().subscribe::<StatusUpdate>().await;

// Broadcast - all subscribed actors receive
broker.broadcast(NewData(5)).await;
```

```bash
cargo run --example broadcast
```

---

## Configuration

**Location:** `examples/configuration/main.rs`

Shows how to load and use configuration files from XDG locations.

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

```bash
cargo run --example fruit_market
```

---

## IPC Examples

These examples require the `ipc` feature. See [IPC Setup](/docs/ipc-setup) for configuration details.

### IPC Basic

Foundation example for IPC setup and type registration.

```bash
cargo run --example ipc_basic --features ipc
```

### IPC Bidirectional

Request-response patterns over IPC with multiple services.

```bash
# Terminal 1
cargo run --example ipc_bidirectional_demo --bin server --features ipc

# Terminal 2
cargo run --example ipc_bidirectional_demo --bin client --features ipc
```

### IPC Streaming

Multi-frame streaming responses (countdowns, pagination).

```bash
# Terminal 1
cargo run --example ipc_streaming --bin server --features ipc

# Terminal 2
cargo run --example ipc_streaming --bin client --features ipc
```

### IPC Subscriptions

Push notifications via broker subscriptions.

```bash
# Terminal 1
cargo run --example ipc_subscriptions --bin server --features ipc

# Terminal 2
cargo run --example ipc_subscriptions --bin client --features ipc
```

### IPC Client Libraries

Polyglot clients in Python and Node.js.

```bash
# Terminal 1
cargo run --example ipc_client_libraries --features ipc

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
| lifecycles | Lifecycle hooks | Hook timing |
| broadcast | Pub/Sub | Broker, subscriptions |
| configuration | Config loading | XDG paths, TOML |
| fruit_market | Multi-actor | Coordination, real-world |
| ipc_* | IPC communication | Type registry, wire protocol |

---

## Running Examples

```bash
# List all examples
cargo run --example

# Run core examples
cargo run --example basic
cargo run --example lifecycles
cargo run --example broadcast
cargo run --example fruit_market

# IPC examples need the feature flag
cargo run --example ipc_basic --features ipc
```
