---
title: Project Structure
nextjs:
  metadata:
    title: Project Structure - acton-reactive
    description: Recommended project layouts for acton-reactive applications, from simple scripts to production systems.
---

Recommended project layouts for `acton-reactive` applications, from simple scripts to production systems.

---

## Simple Single-Binary

```text
my-app/
├── Cargo.toml
└── src/
    └── main.rs         # Runtime, actors, handlers all in one file
```

**Good for:** Learning, small tools, prototypes, scripts.

**Example:**

```rust
// src/main.rs
use acton_reactive::prelude::*;

#[acton_actor]
struct Counter { count: u32 }

#[acton_message]
struct Increment;

#[acton_main]
async fn main() {
    let mut app = ActonApp::launch();
    let mut counter = app.new_actor::<Counter>();

    counter.mutate_on::<Increment>(|actor, _| {
        actor.model.count += 1;
        Reply::ready()
    });

    let handle = counter.start().await;
    handle.send(Increment).await;

    app.shutdown_all().await.unwrap();
}
```

---

## Modular Application

```text
my-app/
├── Cargo.toml
└── src/
    ├── main.rs              # Runtime setup and coordination
    ├── actors/
    │   ├── mod.rs
    │   ├── counter.rs       # Counter actor
    │   ├── logger.rs        # Logger actor
    │   └── processor.rs     # Processor actor
    └── messages/
        ├── mod.rs
        └── events.rs        # Shared message types
```

**Good for:** Larger applications with multiple actors that need clear organization.

**Example structure:**

```rust
// src/actors/mod.rs
pub mod counter;
pub mod logger;
pub mod processor;

// src/actors/counter.rs
use acton_reactive::prelude::*;
use crate::messages::events::*;

#[acton_actor]
pub struct CounterState {
    pub count: u32,
}

pub fn configure_counter(actor: &mut ManagedActor<Idle, CounterState>) {
    actor.mutate_on::<Increment>(|actor, _| {
        actor.model.count += 1;
        Reply::ready()
    });
}

// src/messages/events.rs
use acton_reactive::prelude::*;

#[acton_message]
pub struct Increment;

#[acton_message]
pub struct GetCount;

#[acton_message]
pub struct CountResponse(pub u32);

// src/main.rs
mod actors;
mod messages;

use acton_reactive::prelude::*;
use actors::counter::{CounterState, configure_counter};

#[acton_main]
async fn main() {
    let mut app = ActonApp::launch();

    let mut counter = app.new_actor::<CounterState>();
    configure_counter(&mut counter);
    let counter_handle = counter.start().await;

    // Use the actors...

    app.shutdown_all().await.unwrap();
}
```

---

## Library + Binary

```text
my-app/
├── Cargo.toml
├── src/
│   └── lib.rs              # Actor definitions, reusable logic
└── examples/
    ├── basic.rs            # Simple usage example
    └── advanced.rs         # Complex example
```

**Good for:** Reusable actor libraries, frameworks, and shared components.

**Cargo.toml:**

```toml
[package]
name = "my-actor-lib"
version = "0.1.0"

[lib]
path = "src/lib.rs"

[[example]]
name = "basic"
path = "examples/basic.rs"

[dependencies]
acton-reactive = "0.1"
```

**Library code:**

```rust
// src/lib.rs
use acton_reactive::prelude::*;

#[acton_actor]
pub struct RateLimiter {
    requests: u32,
    limit: u32,
}

#[acton_message]
pub struct Request { pub id: String }

#[acton_message]
pub struct Allowed(pub bool);

pub fn configure_rate_limiter(actor: &mut ManagedActor<Idle, RateLimiter>) {
    actor.mutate_on::<Request>(|actor, ctx| {
        let allowed = actor.model.requests < actor.model.limit;
        if allowed {
            actor.model.requests += 1;
        }
        let reply = ctx.reply_envelope();
        Reply::pending(async move {
            reply.send(Allowed(allowed)).await;
        })
    });
}
```

---

## Workspace Layout

```text
my-project/
├── Cargo.toml              # Workspace root
├── crates/
│   ├── core/               # Core business logic
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   ├── actors/             # Actor definitions
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   └── messages/           # Shared message types
│       ├── Cargo.toml
│       └── src/lib.rs
└── apps/
    └── server/             # Main application
        ├── Cargo.toml
        └── src/main.rs
```

**Good for:** Large systems, multiple binaries, shared components across services.

**Root Cargo.toml:**

```toml
[workspace]
members = [
    "crates/core",
    "crates/actors",
    "crates/messages",
    "apps/server",
]

[workspace.dependencies]
acton-reactive = "0.1"
```

---

## Actor File Organization

Within an actor file, follow this order:

```rust
// 1. Imports
use acton_reactive::prelude::*;
use std::collections::HashMap;

// 2. Actor state definition
#[acton_actor]
pub struct MyActor {
    data: HashMap<String, u32>,
    active: bool,
}

// 3. Message definitions (if actor-specific)
#[acton_message]
pub struct MyCommand { pub key: String }

#[acton_message]
pub struct MyResponse { pub value: u32 }

// 4. Configuration function
pub fn configure_my_actor(actor: &mut ManagedActor<Idle, MyActor>) {
    actor
        .mutate_on::<MyCommand>(handle_command)
        .before_start(|_| { Reply::ready() })
        .after_stop(|_| { Reply::ready() });
}

// 5. Handler implementations (as separate functions for complex logic)
fn handle_command(
    actor: &mut ManagedActor<Started, MyActor>,
    ctx: &mut MessageContext<MyCommand>
) -> impl Future<Output = ()> {
    let key = ctx.message().key.clone();
    let value = actor.model.data.get(&key).copied().unwrap_or(0);
    let reply = ctx.reply_envelope();

    Reply::pending(async move {
        reply.send(MyResponse { value }).await;
    })
}
```

---

## IDE Setup

### VS Code

Install [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer). It works with `acton-reactive` out of the box.

**Recommended settings:**

```json
{
    "rust-analyzer.cargo.features": "all",
    "rust-analyzer.procMacro.enable": true
}
```

### IntelliJ / CLion

The [Rust plugin](https://www.jetbrains.com/rust/) provides full support, including macro expansion previews.

### Neovim

Use `nvim-lspconfig` with `rust-analyzer`. No special configuration needed.

---

## Next Steps

- [Your First Actor](/docs/your-first-actor) - Build something with your new project
- [Configuration](/docs/configuration) - Configure runtime behavior
- [Troubleshooting](/docs/troubleshooting) - Common issues and solutions
