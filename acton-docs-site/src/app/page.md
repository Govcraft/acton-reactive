---
title: Getting started
nextjs:
  metadata:
    title: acton-reactive - Build concurrent Rust apps without the headaches
    description: Build fast, concurrent Rust applications without threading headaches. No locks. No race conditions. Just simple message-passing between independent workers.
---

Build fast, concurrent Rust apps without threading headaches. {% .lead %}

No locks. No race conditions. No shared mutable state nightmares. Just independent workers passing messages to each other.

{% quick-links %}

{% quick-link title="Your First Actor" icon="installation" href="/docs/your-first-actor" description="Build a working actor in 60 seconds." /%}

{% quick-link title="Core Concepts" icon="presets" href="/docs/actors-and-state" description="Understand actors, messages, and handlers." /%}

{% quick-link title="Examples" icon="plugins" href="/docs/examples" description="Learn from real, working code." /%}

{% quick-link title="API Reference" icon="theming" href="/docs/api-reference" description="Complete type and trait documentation." /%}

{% /quick-links %}

---

## What Problem Does This Solve?

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

**Problems:** Lock contention, deadlocks, shared state bugs, complex error handling.

**The Acton approach:**

```rust
// Each actor owns its data. No locks needed.
#[acton_actor]
struct Counter { count: u32 }

#[acton_message]
struct Increment(u32);

counter.mutate_on::<Increment>(|actor, ctx| {
    actor.model.count += ctx.message().0;
    Reply::ready()
});

handle.send(Increment(1)).await;
```

**Benefits:** No locks, isolated failures, guaranteed ordering, compile-time safety.

---

## Think of It Like an Office

| Office Concept | Acton Concept | What It Does |
|----------------|---------------|--------------|
| **Employee** | Actor | A worker with their own desk and files |
| **Memo/Email** | Message | How workers communicate |
| **Job Description** | Handler | "When you get X, do Y" |
| **Employee Badge** | ActorHandle | How to reach a specific worker |
| **Bulletin Board** | Broker | Announcements everyone can see |
| **HR** | Runtime | Hires, manages, and retires workers |

An **actor** is just a worker at their desk. They have their own stuff (state) and nobody else can touch it. When they need something from another worker, they send a **message**. No two workers share a desk. No fighting over resources.

---

## Quick Example

```rust
use acton_reactive::prelude::*;

#[acton_actor]
struct CounterState { count: u32 }

#[acton_message]
struct Increment(u32);

#[acton_main]
async fn main() {
    let mut runtime = ActonApp::launch_async().await;
    let mut counter = runtime.new_actor::<CounterState>();

    counter.mutate_on::<Increment>(|actor, ctx| {
        actor.model.count += ctx.message().0;
        println!("Count: {}", actor.model.count);
        Reply::ready()
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

Ready to build? Start with [Your First Actor](/docs/your-first-actor).

---

## Handler Types at a Glance

| If you need to... | Use this | Why |
|-------------------|----------|-----|
| **Change** the actor's data | `mutate_on` | Exclusive access, one at a time |
| **Read** the actor's data | `act_on` | Can run many at once (concurrent) |

```rust
// For changes
counter.mutate_on::<Increment>(|actor, ctx| {
    actor.model.count += ctx.message().0;
    Reply::ready()
});

// For queries
counter.act_on::<GetCount>(|actor, ctx| {
    let count = actor.model.count;
    let reply = ctx.reply_envelope();
    Reply::pending(async move {
        reply.send(CountResponse(count)).await;
    })
});
```

{% callout type="note" title="When in doubt, use mutate_on" %}
It's safer because it processes one message at a time. Optimize to `act_on` later.
{% /callout %}

See [Handler Types](/docs/handler-types) for the full picture.

---

## Status

{% callout type="warning" title="Pre-1.0 Software" %}
`acton-reactive` is under active development. The API is stabilizing but may change before 1.0. Breaking changes bump the minor version.
{% /callout %}

---

## Next Steps

- [Installation](/docs/installation) - Add acton-reactive to your project
- [Your First Actor](/docs/your-first-actor) - Hands-on tutorial
- [Actors & State](/docs/actors-and-state) - Understand the actor model
- [Examples](/docs/examples) - See complete applications
