---
title: Installation
nextjs:
  metadata:
    title: Installation - acton-reactive
    description: Add acton-reactive to your Rust project and configure dependencies.
---

Add `acton-reactive` to your Rust project and start building agent-based systems.

---

## Basic Installation

Add `acton-reactive` to your `Cargo.toml`:

```toml
[dependencies]
acton-reactive = "0.1"  # Check crates.io for latest version
acton-macro = "0.1"     # Optional: Convenience macros
tokio = { version = "1", features = ["full"] }
```

{% callout type="note" title="Tokio Runtime" %}
`acton-reactive` requires the Tokio async runtime. Make sure to include it with the `full` feature set for complete functionality.
{% /callout %}

---

## Optional Features

### IPC Support

For inter-process communication support, enable the `ipc` feature:

```toml
[dependencies]
acton-reactive = { version = "0.1", features = ["ipc"] }
```

This enables:
- Unix socket communication
- JSON and MessagePack serialization
- External process integration
- Rate limiting and security features

---

## Crate Overview

| Crate | Purpose |
|-------|---------|
| `acton-reactive` | Core framework for agents, messaging, and runtime |
| `acton-macro` | Procedural macros for deriving agent traits |
| `acton-ern` | Entity Resource Naming for agent identification |

---

## Minimum Rust Version

`acton-reactive` requires Rust 1.75 or later for async trait support and other modern features.

Check your Rust version:

```shell
rustc --version
```

Update if needed:

```shell
rustup update stable
```

---

## Verifying Installation

Create a simple test file to verify everything works:

```rust
use acton_reactive::prelude::*;
use acton_macro::acton_actor;

#[acton_actor]
struct TestAgent {
    initialized: bool,
}

#[derive(Clone, Debug)]
struct Ping;

#[tokio::main]
async fn main() {
    let mut runtime = ActonApp::launch();

    let mut agent = runtime.new_agent::<TestAgent>();

    agent.mutate_on::<Ping>(|agent, _ctx| {
        agent.model.initialized = true;
        println!("Pong! Agent is working.");
        AgentReply::immediate()
    });

    let handle = agent.start().await;
    handle.send(Ping).await;

    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

Run with:

```shell
cargo run
```

You should see:

```text
Pong! Agent is working.
```

---

## Next Steps

- [Getting Started](/) - Build your first agent
- [Architecture](/docs/architecture) - Understand the system design
- [Configuration](/docs/configuration) - Configure the runtime
