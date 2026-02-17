---
title: Getting started
nextjs:
  metadata:
    title: acton-reactive - Build concurrent Rust apps without the headaches
    description: Build fast, concurrent Rust applications without threading headaches. No locks. No race conditions. Just simple message-passing between independent workers.
---

Build concurrent Rust apps without locks, race conditions, or shared state nightmares. {% .lead %}

Acton gives you independent workers that own their state privately and communicate through messages. If you can write async Rust, you can write actors—and your concurrent code will be correct by construction.

{% quick-links %}

{% quick-link title="Quick Start" icon="installation" href="/docs/quick-start/installation" description="Get running in 5 minutes." /%}

{% quick-link title="Core Concepts" icon="presets" href="/docs/core-concepts/what-are-actors" description="Understand actors, messages, and handlers." /%}

{% quick-link title="Building Apps" icon="plugins" href="/docs/building-apps/parent-child-actors" description="Practical patterns for real applications." /%}

{% quick-link title="API Reference" icon="theming" href="/docs/reference/api-overview" description="Quick reference for types and traits." /%}

{% /quick-links %}

---

## Choose your path

{% callout type="note" title="New to concurrent programming?" %}
Start with [What are Actors?](/docs/core-concepts/what-are-actors) to understand the concepts, then work through the [Quick Start](/docs/quick-start/installation).
{% /callout %}

{% callout type="note" title="Experienced with actors?" %}
Jump straight to [Installation](/docs/quick-start/installation) and [Your First Actor](/docs/quick-start/your-first-actor). Check the [Cheatsheet](/docs/reference/cheatsheet) for quick patterns.
{% /callout %}

---

## Actors eliminate data races by making isolation the default

Traditional concurrent Rust requires careful lock management:

```rust
// Shared state protected by locks...
let counter = Arc::new(Mutex::new(0));
let counter_clone = counter.clone();

// Spawn threads...
let handle = thread::spawn(move || {
    let mut num = counter_clone.lock().unwrap(); // Hope this doesn't deadlock!
    *num += 1;
});

// Hope you got all the Arc<Mutex<...>> right...
```

**The problems:** Lock contention, deadlocks, shared state bugs, and subtle race conditions that only appear in production.

**Acton's solution:** Each actor owns its data privately. No locks needed.

```rust
#[acton_actor]
struct Counter { count: i32 }

#[acton_message]
struct Increment;

builder.mutate_on::<Increment>(|actor, _msg| {
    actor.model.count += 1;
    Reply::ready()
});

handle.send(Increment).await;
```

**What you get:** No locks, isolated failures, guaranteed message ordering, and compile-time safety. Data races become impossible because actors never share mutable state.

---

## A complete example in 20 lines

```rust
use acton_reactive::prelude::*;

#[acton_actor]
struct Counter { count: i32 }

#[acton_message]
struct Increment;

#[acton_main]
async fn main() {
    let mut app = ActonApp::launch();

    let handle = app
        .new_actor::<Counter>()
        .mutate_on::<Increment>(|actor, _| {
            actor.model.count += 1;
            println!("Count: {}", actor.model.count);
            Reply::ready()
        })
        .start()
        .await;

    handle.send(Increment).await.ok();
    handle.send(Increment).await.ok();
    handle.send(Increment).await.ok();

    app.shutdown_all().await.ok();
}
```

**Output:**
```text
Count: 1
Count: 2
Count: 3
```

Each message is processed in order. The actor's state is never accessed concurrently. Ready to build? Start with [Installation](/docs/quick-start/installation).

---

## Two handler types cover every use case

| If you need to... | Use this | Why |
|-------------------|----------|-----|
| **Modify** the actor's data | `mutate_on` / `mutate_on_sync` | Exclusive access, one message at a time |
| **Read** the actor's data | `act_on` / `act_on_sync` | Multiple reads can run concurrently |

Use the `_sync` variants when your handler doesn't need async — they avoid a heap allocation per invocation.

```rust
// For mutations - one at a time, exclusive access
builder.mutate_on::<Increment>(|actor, _| {
    actor.model.count += 1;
    Reply::ready()
});

// For queries - can run concurrently with other reads
builder.act_on::<GetCount>(|actor, _| {
    Reply::with(actor.model.count)
});
```

{% callout type="note" title="When in doubt, use mutate_on" %}
It's safer because it processes one message at a time. Optimize to `act_on` when you're sure the handler only reads.
{% /callout %}

See [Messages & Handlers](/docs/core-concepts/messages-and-handlers) for the complete picture.

---

## Current status

{% callout type="warning" title="Pre-1.0 Software" %}
`acton-reactive` is under active development. The API is stabilizing but may change before 1.0. Breaking changes bump the minor version (e.g., 0.7 → 0.8).
{% /callout %}

---

## Start building now

- **[Installation](/docs/quick-start/installation)** — Add acton-reactive to your project
- **[Your First Actor](/docs/quick-start/your-first-actor)** — Hands-on tutorial with working code
- **[What are Actors?](/docs/core-concepts/what-are-actors)** — Understand the actor model
- **[Cheatsheet](/docs/reference/cheatsheet)** — Copy-paste patterns for common tasks
