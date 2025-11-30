---
title: Getting started
nextjs:
  metadata:
    title: acton-reactive - Agent-based reactive systems in Rust
    description: A type-safe, async-first framework for building reactive, event-driven systems using an agent-based architecture in Rust.
---

Build reactive, event-driven systems with type-safe agents in Rust. {% .lead %}

{% quick-links %}

{% quick-link title="Installation" icon="installation" href="/docs/installation" description="Add acton-reactive to your Rust project and get started in minutes." /%}

{% quick-link title="Architecture" icon="presets" href="/docs/architecture" description="Understand the system design and component relationships." /%}

{% quick-link title="Examples" icon="plugins" href="/docs/examples" description="Learn from practical examples covering common patterns." /%}

{% quick-link title="API Reference" icon="theming" href="/docs/api-reference" description="Complete documentation of all public types and traits." /%}

{% /quick-links %}

---

## Core Concepts

### Agents

An **agent** is an independent unit of computation with:
- **State (Model)**: Private data the agent owns
- **Handlers**: Functions that process incoming messages
- **Lifecycle Hooks**: Callbacks for startup and shutdown events

### Messages

**Messages** are the primary way agents communicate. Any type that implements `Clone + Debug + Send + Sync + 'static` can be a message.

### The Runtime

The **runtime** (`AgentRuntime`) manages all agents and provides:
- Agent creation
- A central message broker for pub/sub
- Graceful shutdown coordination

---

## Your First Agent

Let's create a simple counter agent that responds to increment messages.

### Step 1: Define the Agent State

```rust
use acton_reactive::prelude::*;
use acton_macro::acton_actor;

// The #[acton_actor] macro derives Default, Clone, and Debug
#[acton_actor]
struct CounterState {
    count: u32,
}
```

### Step 2: Define Messages

```rust
// Messages must be Clone + Debug (ActonMessage is auto-implemented)
#[derive(Clone, Debug)]
struct Increment(u32);

#[derive(Clone, Debug)]
struct GetCount;

#[derive(Clone, Debug)]
struct CountResponse(u32);
```

### Step 3: Create and Configure the Agent

```rust
#[tokio::main]
async fn main() {
    // 1. Launch the runtime
    let mut runtime = ActonApp::launch();

    // 2. Create an agent builder
    let mut counter = runtime.new_agent::<CounterState>();

    // 3. Register message handlers
    counter
        .mutate_on::<Increment>(|agent, ctx| {
            // Access and modify the agent's state
            agent.model.count += ctx.message().0;
            println!("Count is now: {}", agent.model.count);
            AgentReply::immediate()
        })
        .mutate_on::<GetCount>(|agent, ctx| {
            let count = agent.model.count;
            let reply_envelope = ctx.reply_envelope();

            // Reply to the sender
            Box::pin(async move {
                reply_envelope.send(CountResponse(count)).await;
            })
        });

    // 4. Start the agent
    let handle = counter.start().await;

    // 5. Send messages
    handle.send(Increment(5)).await;
    handle.send(Increment(3)).await;

    // 6. Shutdown
    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

### Complete Example

Here's the full working example:

```rust
use acton_reactive::prelude::*;
use acton_macro::acton_actor;

#[acton_actor]
struct CounterState {
    count: u32,
}

#[derive(Clone, Debug)]
struct Increment(u32);

#[tokio::main]
async fn main() {
    let mut runtime = ActonApp::launch();

    let mut counter = runtime.new_agent::<CounterState>();

    counter.mutate_on::<Increment>(|agent, ctx| {
        agent.model.count += ctx.message().0;
        println!("Count: {}", agent.model.count);
        AgentReply::immediate()
    });

    let handle = counter.start().await;

    handle.send(Increment(1)).await;
    handle.send(Increment(2)).await;
    handle.send(Increment(3)).await;

    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

**Output:**
```text
Count: 1
Count: 3
Count: 6
```

---

## Message Handling

### Handler Types

`acton-reactive` provides four types of message handlers:

| Handler | Access | Errors | Use Case |
|---------|--------|--------|----------|
| `mutate_on` | Mutable | No | State modifications |
| `act_on` | Read-only | No | Concurrent read operations |
| `mutate_on_fallible` | Mutable | Yes | Operations that can fail |
| `act_on_fallible` | Read-only | Yes | Concurrent ops that can fail |

### Mutable Handlers

Use `mutate_on` when you need to modify the agent's state:

```rust
agent.mutate_on::<MyMessage>(|agent, ctx| {
    // agent.model is mutable here
    agent.model.some_field = ctx.message().value;
    AgentReply::immediate()
});
```

### Read-Only Handlers

Use `act_on` for operations that don't modify state. These can run concurrently:

```rust
agent.act_on::<QueryMessage>(|agent, ctx| {
    // agent.model is read-only here
    let data = agent.model.some_field.clone();
    let reply = ctx.reply_envelope();

    Box::pin(async move {
        reply.send(QueryResponse(data)).await;
    })
});
```

---

## Lifecycle Hooks

Lifecycle hooks let you run code at specific points in an agent's lifecycle.

| Hook | When It Runs |
|------|--------------|
| `before_start` | Before the message loop begins |
| `after_start` | After the message loop starts |
| `before_stop` | When stop is requested, before cleanup |
| `after_stop` | After the agent fully stops |

```rust
tracker
    .before_start(|_| {
        println!("Preparing to track items...");
        AgentReply::immediate()
    })
    .after_start(|_| {
        println!("Tracker is now running!");
        AgentReply::immediate()
    })
    .before_stop(|_| {
        println!("Tracker is shutting down...");
        AgentReply::immediate()
    })
    .after_stop(|agent| {
        println!("Final items: {:?}", agent.model.items);
        AgentReply::immediate()
    });
```

---

## Pub/Sub Messaging

The broker enables publish/subscribe messaging between agents.

### Broadcasting Messages

```rust
// Get the broker handle
let broker = runtime.broker();

// Broadcast a message (all subscribers receive it)
broker.broadcast(MyEvent { data: 42 }).await;
```

### Subscribing to Messages

```rust
// Subscribe before starting the agent
agent.handle().subscribe::<MyEvent>().await;
```

---

## Next Steps

Now that you understand the basics, explore these topics:

- [Installation](/docs/installation) - Detailed setup instructions
- [Architecture](/docs/architecture) - System design and diagrams
- [Configuration](/docs/configuration) - All configuration options
- [Message Handling](/docs/message-handling) - Advanced handler patterns
- [Examples](/docs/examples) - Practical example walkthroughs
- [Testing](/docs/testing) - Testing strategies and utilities
- [IPC Communication](/docs/ipc) - Inter-process communication
- [API Reference](/docs/api-reference) - Complete type documentation
