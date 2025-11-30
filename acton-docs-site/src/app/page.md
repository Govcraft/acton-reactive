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

{% quick-link title="Installation" icon="installation" href="/docs/installation" description="Get up and running in under 5 minutes." /%}

{% quick-link title="Architecture" icon="presets" href="/docs/architecture" description="Understand how the pieces fit together." /%}

{% quick-link title="Examples" icon="plugins" href="/docs/examples" description="Learn from real, working code." /%}

{% quick-link title="API Reference" icon="theming" href="/docs/api-reference" description="Complete documentation for all types." /%}

{% /quick-links %}

---

## What Problem Does This Solve?

If you've ever written concurrent Rust code, you've probably hit these walls:

### The Traditional Approach

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
// Hope nothing holds a lock too long...
// Hope you don't have circular lock dependencies...
```

**Problems:**
- Lock contention slows everything down
- Deadlocks are hard to debug and easy to introduce
- Shared state means any thread can break any other thread
- Error handling becomes a mess

### The Acton Approach

```rust
// Each actor owns its data. No locks needed.
#[acton_actor]
struct Counter {
    count: u32,
}

// Messages are the only way in.
#[acton_message]
struct Increment(u32);

// Handlers are simple functions.
counter.mutate_on::<Increment>(|actor, ctx| {
    actor.model.count += ctx.message().0;
    Reply::ready()
});

// Send messages. That's it.
handle.send(Increment(1)).await;
```

**Benefits:**
- No locks, no deadlocks, no shared state
- Each actor is an independent unit - failures are isolated
- Message ordering is guaranteed within an actor
- The compiler catches mistakes at build time, not runtime

---

## Think of It Like an Office

If the code looks intimidating, here's a simpler way to think about it:

| Office Concept | Acton Concept | What It Does |
|----------------|---------------|--------------|
| **Employee** | Actor | A worker with their own desk and files |
| **Memo/Email** | Message | How workers communicate |
| **Job Description** | Handler | "When you get X, do Y" |
| **Employee Badge** | ActorHandle | How to reach a specific worker |
| **Bulletin Board** | Broker | Announcements everyone can see |
| **HR** | Runtime | Hires, manages, and retires workers |

An **actor** is just a worker at their desk. They have their own stuff (state) and nobody else can touch it. When they need something from another worker, they send a **message**. When they receive a message, they follow their **handler** instructions for what to do with it.

No two workers share a desk. No fighting over resources. Just memos flying around.

---

## Your First Actor in 60 Seconds

Let's build a simple counter. Don't worry about understanding everything yet - just follow along.

### Step 1: Add Dependencies

```toml
[dependencies]
acton-reactive = "0.1"
```

### Step 2: Write the Code

```rust
use acton_reactive::prelude::*;

// This is our actor's "desk" - its private data
#[acton_actor]
struct CounterState {
    count: u32,
}

// This is a message - a memo telling the counter what to do
#[acton_message]
struct Increment(u32);

#[acton_main]
async fn main() {
    // Start the "office" (runtime)
    let mut runtime = ActonApp::launch();

    // Hire a counter actor
    let mut counter = runtime.new_actor::<CounterState>();

    // Tell the actor: "When you get an Increment memo, add to your count"
    counter.mutate_on::<Increment>(|actor, ctx| {
        actor.model.count += ctx.message().0;
        println!("Count is now: {}", actor.model.count);
        Reply::ready()
    });

    // The actor starts working
    let handle = counter.start().await;

    // Send some memos!
    handle.send(Increment(1)).await;
    handle.send(Increment(2)).await;
    handle.send(Increment(3)).await;

    // Close up shop
    runtime.shutdown_all().await.expect("Shutdown failed");
}
```

### Step 3: Run It

```shell
cargo run
```

**Output:**
```text
Count is now: 1
Count is now: 3
Count is now: 6
```

That's it! You've built a concurrent, message-driven application. No locks, no mutexes, no data races.

---

## Handler Types

When defining what an actor does with messages, you have two main choices:

| If you need to... | Use this | Why |
|-------------------|----------|-----|
| **Change** the actor's data | `mutate_on` | Exclusive access, one at a time |
| **Read** the actor's data | `act_on` | Can run many at once (concurrent) |

### mutate_on: For Changes

```rust
counter.mutate_on::<Increment>(|actor, ctx| {
    actor.model.count += ctx.message().0; // Changing data
    Reply::ready()
});
```

### act_on: For Reading

```rust
counter.act_on::<GetCount>(|actor, ctx| {
    let count = actor.model.count; // Just reading
    let reply = ctx.reply_envelope();

    Reply::pending(async move {
        reply.send(CountResponse(count)).await;
    })
});
```

{% callout type="note" title="When in doubt, use mutate_on" %}
If you're not sure which to use, start with `mutate_on`. It's safer because it processes one message at a time. You can optimize to `act_on` later when you know a handler only reads data.
{% /callout %}

---

## Lifecycle Hooks

Want to run code when an actor starts or stops? Use lifecycle hooks:

```rust
counter
    .before_start(|_| {
        println!("Actor is booting up...");
        Reply::ready()
    })
    .after_start(|_| {
        println!("Actor is ready for messages!");
        Reply::ready()
    })
    .before_stop(|_| {
        println!("Actor is shutting down...");
        Reply::ready()
    })
    .after_stop(|actor| {
        println!("Final count was: {}", actor.model.count);
        Reply::ready()
    });
```

**Hook Order:**
```text
before_start → [message loop runs] → before_stop → after_stop
     ↓                                    ↑
after_start ─────────────────────────────┘
```

---

## Broadcasting Messages

Sometimes you want to announce something to everyone who cares, not just one actor. That's what the **broker** is for - think of it as the office bulletin board.

```rust
// Get the bulletin board
let broker = runtime.broker();

// Actors can subscribe to see certain announcements
actor.handle().subscribe::<PriceUpdate>().await;

// Anyone can post an announcement
broker.broadcast(PriceUpdate { symbol: "ACME", price: 123.45 }).await;
// All subscribers receive it!
```

---

## Status

{% callout type="warning" title="Pre-1.0 Software" %}
`acton-reactive` is under active development. The API is stabilizing but may change before 1.0. We follow semver, so breaking changes bump the minor version until 1.0.
{% /callout %}

---

## Next Steps

Now that you understand the basics:

- [Installation](/docs/installation) - Detailed setup and troubleshooting
- [Architecture](/docs/architecture) - How everything connects under the hood
- [Message Handling](/docs/message-handling) - All the handler patterns in depth
- [Examples](/docs/examples) - Real applications showing common patterns
- [Testing](/docs/testing) - How to test your actors
- [IPC Communication](/docs/ipc) - Talk to actors from other processes (Python, Node.js, etc.)
- [FAQ & Troubleshooting](/docs/faq) - Common questions and gotchas
- [API Reference](/docs/api-reference) - Every type and trait documented
