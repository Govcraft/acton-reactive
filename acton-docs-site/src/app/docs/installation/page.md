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
    let mut runtime = ActonApp::launch();

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
#[derive(Default, Clone, Debug)]
struct MyState {
    value: u32,
}
```

The macro just saves boilerplate - there's no magic.

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

## Common Project Structures

### Simple Single-Binary

```text
my-app/
├── Cargo.toml
└── src/
    └── main.rs         # Runtime, actors, handlers all in one file
```

Good for: Learning, small tools, prototypes.

### Modular Application

```text
my-app/
├── Cargo.toml
└── src/
    ├── main.rs         # Runtime setup and coordination
    ├── actors/
    │   ├── mod.rs
    │   ├── counter.rs  # Counter actor
    │   └── logger.rs   # Logger actor
    └── messages/
        ├── mod.rs
        └── events.rs   # Shared message types
```

Good for: Larger applications with multiple actors.

### Library + Binary

```text
my-app/
├── Cargo.toml
├── src/
│   └── lib.rs          # Actor definitions, reusable logic
└── examples/
    └── demo.rs         # Example usage
```

Good for: Reusable actor libraries.

---

## IDE Setup

### VS Code

Install the [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer) extension. It works well with `acton-reactive` out of the box.

### IntelliJ / CLion

The [Rust plugin](https://www.jetbrains.com/rust/) provides full support, including macro expansion previews.

### Neovim

Use `nvim-lspconfig` with `rust-analyzer`. No special configuration needed.

---

## Troubleshooting

### "unresolved import `acton_reactive`"

**Cause:** Cargo can't find the crate.

**Fix:** Make sure your `Cargo.toml` has the dependency:

```toml
[dependencies]
acton-reactive = "0.1"
```

Then run `cargo build` to fetch it.

---

### "the trait `Default` is not implemented"

**Cause:** Your actor state struct doesn't implement `Default`.

**Fix:** Add `#[acton_actor]` macro or derive `Default` manually:

```rust
// Option 1: Use the macro
#[acton_actor]
struct MyState {
    count: u32,
}

// Option 2: Derive manually
#[derive(Default, Clone, Debug)]
struct MyState {
    count: u32,
}
```

---

### "async runtime not available"

**Cause:** You're missing the `#[acton_main]` attribute on your main function.

**Fix:** Make sure your main function uses the `#[acton_main]` macro:

```rust
use acton_reactive::prelude::*;

#[acton_main]  // This sets up the async runtime for you!
async fn main() {
    let mut runtime = ActonApp::launch();
    // ...
}
```

The `#[acton_main]` macro automatically configures the async runtime with the correct settings for Acton. You don't need to add tokio to your `Cargo.toml` - it's included with acton-reactive.

---

### "cannot find macro `acton_actor`"

**Cause:** You're not importing the prelude.

**Fix:** Import the prelude which includes all macros:

```rust
use acton_reactive::prelude::*;  // Includes acton_actor, acton_message, etc.
```

---

### Messages aren't being received

**Common causes:**

1. **Handler not registered:** Did you call `mutate_on` or `act_on` for that message type?

   ```rust
   actor.mutate_on::<MyMessage>(|actor, ctx| {
       // Handler must be registered!
       Reply::ready()
   });
   ```

2. **Actor not started:** Handlers only work after `start()`:

   ```rust
   let handle = actor.start().await;  // Start first!
   handle.send(MyMessage).await;      // Then send
   ```

3. **Wrong message type:** Rust's type system is strict. `MyMessage` and `my_message::MyMessage` are different types.

4. **Shutdown too fast:** If your program exits immediately after sending, messages might not process:

   ```rust
   handle.send(MyMessage).await;
   tokio::time::sleep(Duration::from_millis(100)).await;  // Give it time
   runtime.shutdown_all().await?;
   ```

---

### Broadcasts aren't received

**Cause:** Actor didn't subscribe, or subscribed after the broadcast.

**Fix:** Subscribe *before* starting the actor:

```rust
// Good order
actor.handle().subscribe::<MyEvent>().await;
let handle = actor.start().await;

// Bad order - might miss messages
let handle = actor.start().await;
handle.subscribe::<MyEvent>().await;  // Too late for early broadcasts
```

---

### Compilation is slow

**Cause:** Probably building all of tokio's features every time.

**Fix:** Use incremental compilation and consider a workspace:

```toml
# Cargo.toml
[profile.dev]
incremental = true

[profile.dev.package."*"]
opt-level = 2  # Optimize deps, not your code
```

---

### IPC socket already in use

**Cause:** A previous process left the socket file behind.

**Fix:** Delete the stale socket:

```shell
rm /tmp/acton.sock
```

Or handle it in your code:

```rust
use std::path::Path;

let socket_path = Path::new("/tmp/acton.sock");
if socket_path.exists() {
    std::fs::remove_file(socket_path)?;
}
```

---

## Next Steps

Now that you're set up:

- [Getting Started](/) - Build your first real actor
- [Architecture](/docs/architecture) - Understand how it all works
- [Examples](/docs/examples) - Learn from working code
- [FAQ & Troubleshooting](/docs/faq) - More common issues and answers
