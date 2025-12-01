---
title: Installation
nextjs:
  metadata:
    title: Installation - acton-reactive
    description: Get acton-reactive set up in your Rust project in under 5 minutes.
---

Get `acton-reactive` set up in your Rust project in under 5 minutes.

---

## Quick Start

Add this to your `Cargo.toml`:

```toml
[dependencies]
acton-reactive = "0.1"
```

That's it! You're ready to build actors.

{% callout type="note" title="Batteries Included" %}
`acton-reactive` includes everything you need, including the async runtime (Tokio). The `#[acton_main]` macro handles runtime setup automatically, so you don't need to add any additional dependencies.
{% /callout %}

---

## Verify It Works

Create a quick test to make sure everything is wired up correctly:

```rust
use acton_reactive::prelude::*;

#[acton_actor]
struct TestActor {
    ready: bool,
}

#[acton_message]
struct Ping;

#[acton_main]
async fn main() {
    let mut runtime = ActonApp::launch_async().await;

    let mut actor = runtime.new_actor::<TestActor>();

    actor.mutate_on::<Ping>(|actor, _ctx| {
        actor.model.ready = true;
        println!("Pong! acton-reactive is working.");
        Reply::ready()
    });

    let handle = actor.start().await;
    handle.send(Ping).await;

    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

Run it:

```shell
cargo run
```

You should see:

```text
Pong! acton-reactive is working.
```

---

## Feature Flags

### IPC Support

Need external processes (Python, Node.js, etc.) to talk to your actors? Enable IPC:

```toml
[dependencies]
acton-reactive = { version = "0.1", features = ["ipc"] }
```

This adds:
- Unix Domain Socket communication
- JSON serialization for messages
- Rate limiting and connection management
- External process integration

### MessagePack Serialization

For smaller, faster IPC messages:

```toml
[dependencies]
acton-reactive = { version = "0.1", features = ["ipc-messagepack"] }
```

### Feature Comparison

| Feature | What It Adds | Use When |
|---------|--------------|----------|
| *(default)* | Core actors, messaging, broker | Basic actor-based applications |
| `ipc` | Unix socket IPC, JSON serialization | External process integration |
| `ipc-messagepack` | `ipc` + MessagePack serialization | High-throughput IPC, smaller messages |

---

## Crate Overview

| Crate | What It Does | Required? |
|-------|-------------|-----------|
| `acton-reactive` | Core framework: actors, messaging, runtime, and macros | Yes |
| `acton-ern` | Entity Resource Names for actor identification | Included with core |

{% callout type="note" title="Macros Included in Prelude" %}
The `acton_reactive::prelude` re-exports all macros from `acton-macro`, so you only need one import:
```rust
use acton_reactive::prelude::*;  // Includes #[acton_actor], #[acton_message], etc.
```
No need to add `acton-macro` as a separate dependency.
{% /callout %}

### Without the Macros

You can use `acton-reactive` without the macros, but you'll need to derive traits manually:

```rust
// With macros (recommended)
#[acton_actor]
struct MyState {
    value: u32,
}

// Without macros (equivalent)
#[derive(Default, Debug)]
struct MyState {
    value: u32,
}
```

The macro also adds a compile-time check that your type is `Send + 'static`. Without the macro, you'll get an error later if your type doesn't satisfy those bounds.

---

## Minimum Rust Version

`acton-reactive` requires **Rust 1.75** or later.

This is needed for:
- Native async trait support (no more `#[async_trait]` everywhere)
- Other modern Rust features

Check your version:

```shell
rustc --version
```

Update if needed:

```shell
rustup update stable
```

---

## Next Steps

Now that you're set up:

- [Your First Actor](/docs/your-first-actor) - Build your first real actor
- [Project Structure](/docs/project-structure) - Organize your codebase
- [Troubleshooting](/docs/troubleshooting) - Common errors and solutions
- [Examples](/docs/examples) - Learn from working code
