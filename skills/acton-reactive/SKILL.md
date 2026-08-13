---
name: acton-reactive
description: Design and write idiomatic acton-reactive (9.x) actor systems in Rust. Use this whenever you are writing, reviewing, or planning code that uses acton-reactive, and also whenever a Rust async design starts reaching for Arc<Mutex<T>>, RwLock, shared mutable state across tasks, hand-rolled worker pools, retry/restart loops, or an orchestrator function that drives other tasks. Use it before writing the first actor, not after, because the expensive mistakes are architectural. Also use it when the user mentions actors, message passing, event-driven or reactive architecture, supervision, pub/sub brokers, or asks why their async Rust deadlocks.
---

# Writing acton-reactive that is actually reactive

Acton is not "async Rust with mailboxes bolted on". It is a decomposition:
the program is divided into **state owners that react to messages**. Almost
every bad Acton codebase comes from keeping an imperative design and using
actors as a concurrency primitive inside it.

If you have limited attention, spend it on **Part 1**. The API is easy to look
up and hard to get wrong once the shape is right; the shape is easy to get
wrong and expensive to fix.

Version: this describes **acton-reactive 9.x**. If the project pins 8.x, read
`references/migration.md` first, because several things changed silently.

---

## Part 1: The four reflexes to unlearn

These are the failure modes that show up over and over. Each one is a habit
carried over from lock-based or imperative async code, and each one produces
something that compiles, passes a smoke test, and is wrong.

### Reflex 1: reaching for `Arc<Mutex<T>>`

**The tell:** any `Arc<Mutex<_>>`, `Arc<RwLock<_>>`, or `Arc<DashMap<_>>`
holding state that more than one place writes to.

**Why it is wrong here:** the actor's message loop *is* the mutual exclusion.
Exactly one `mutate_on` handler touches `model` at a time, and the runtime
guarantees it. So a lock *inside* an actor is redundant. A lock *outside* one,
shared between actors or between actors and plain code, is worse than
redundant: it means the state has two owners again, which is the exact problem
the actor was supposed to solve. You also gain a new failure mode, because
holding a lock across an `.await` inside a handler stalls that actor's entire
message loop, and every sender behind it.

**The fix:** ask "who owns this state?" Give it to exactly one actor. Everyone
else sends messages.

```rust
// Wrong: state has no owner, so it needs a lock, and now nobody
// can reason about who mutates it when.
let inventory = Arc::new(Mutex::new(HashMap::new()));
let a = spawn_worker(inventory.clone());
let b = spawn_worker(inventory.clone());

// Right: the state has exactly one owner. Concurrency is still there,
// it just arrives as messages instead of as contention.
#[acton_actor]
struct Inventory {
    stock: HashMap<Sku, u32>,
}

inventory.mutate_on::<Restock>(|actor, ctx| {
    *actor.model.stock.entry(ctx.message().sku.clone()).or_default() += ctx.message().qty;
    Reply::ready()
});
```

**Genuinely fine, do not "fix" these:** an immutable `Arc<Config>` shared for
reading; an `Arc<AtomicU64>` for a metric nobody branches on; a lock wholly
inside one actor's `model` that is never cloned out (though at that point,
prefer a plain field). The rule is about shared *mutable decision* state.

### Reflex 2: writing the orchestrator in `main`

This is the most common and the most costly.

**The tell:** `main` (or any plain `async fn`) contains a sequence of sends
with logic between them, collects results into a `Vec`, matches on what to do
next, or tracks how many replies are still outstanding.

```rust
// Wrong: this is a script wearing an actor costume.
let order = orders.ask(GetOrder { id }).await?;
if order.needs_review {
    let verdict = reviewer.ask(Review { order: order.clone() }).await?;
    if verdict.approved {
        shipping.send(Ship { order }).await;
    }
}
```

**Why it is wrong:** "which step are we on", "what came back", "how many are
still outstanding" is *state*. Sitting in `main`, it cannot be supervised, cannot
be restarted, cannot be inspected, and cannot be tested without driving the
whole program. It also serialises a workflow that had no reason to be serial.

**The fix:** the workflow is an actor. Its `model` holds the progress; its
handlers react to results as they arrive.

```rust
#[acton_actor]
struct Fulfillment {
    awaiting_review: HashMap<OrderId, Order>,
    reviewer: Option<ActorHandle>,     // wired once at startup
    shipping: Option<ActorHandle>,
}

fulfillment.mutate_on::<OrderPlaced>(|actor, ctx| {
    let order = ctx.message().order.clone();
    if order.needs_review {
        actor.model.awaiting_review.insert(order.id, order.clone());
        // Cloned out of `model` before the async block: the closure is `Fn`
        // and cannot move captured values out.
        let Some(reviewer) = actor.model.reviewer.clone() else {
            return Reply::ready();
        };
        Reply::pending(async move { reviewer.send(Review { order }).await })
    } else {
        let Some(shipping) = actor.model.shipping.clone() else {
            return Reply::ready();
        };
        Reply::pending(async move { shipping.send(Ship { order }).await })
    }
});

fulfillment.mutate_on::<ReviewCompleted>(|actor, ctx| { /* react */ Reply::ready() });
```

**What `main` is allowed to do:** launch the runtime, construct and wire the
actors, send the message that starts things, await shutdown. If `main` is
growing branches, you have found an actor you have not written yet.

**Getting results back out** is the obvious next question, and the first answer
is that you usually should not need to. Broker subscribers must be actors, so
the thing that consumes the output should be an actor too. Push the edge
outward until the only thing left outside is a genuine process boundary: a
terminal, an HTTP response, a socket.

At that real edge, pick by who initiates and whether you can miss intermediate
values:

| The application... | Use |
|---|---|
| asks when it wants to know | `handle.ask(req).await?` |
| tracks the current state | actor holds a `watch::Sender` in `model`; app holds the `Receiver` |
| must see every event | `mpsc::Sender`, or `broadcast::Sender` for several consumers |
| is a different process | IPC subscriptions |

A `Sender` in an actor's `model` is **not** the `Arc<Mutex>` anti-pattern, even
though it also looks like a shared handle. The difference is authority: a
channel carries copies outward and has exactly one writer, while the actor
keeps exclusive ownership of the state it makes decisions on. Do not
over-correct into polling with `ask` in a loop; that is a worse design than the
channel it was avoiding.

Be honest about why this works, because the reason is not "locks are bad".
A `watch` channel is an `Arc<RwLock<T>>` internally. What you gain is that the
lock is held by one writer for one uncontended store, and readers get a
consistent snapshot instead of reading two fields that were written at
different times and calling the pair a state. The discipline is what helps;
the absence of a lock is a consequence, not the point.

(It is `Option<watch::Sender<_>>` in practice, because `#[acton_actor]` derives
`Default` and senders have none. Wire it in after construction.)

### Reflex 3: `tokio::spawn` inside a handler

**Why it is wrong:** the spawned task escapes the runtime. It no longer takes
part in graceful shutdown, supervision, or backpressure, so shutdown races it
and a panic inside it is invisible to the supervisor. It is also usually
pointless: `act_on` handlers *already* run concurrently, up to the actor's
high-water mark.

**The fix:** return the work as `Reply::pending(async move { ... })` and await
directly inside it. For fire-and-forget work triggered from outside the actor
system (an HTTP route, say), send a message to an actor that does the work; its
concurrency cap becomes natural backpressure.

**The special case worth naming: `spawn` plus `sleep` to send something later.**
That is the most common form of this reflex and it has had its own answer since
9.1 — `handle.send_after(msg, delay)`, `send_at`, and `send_every`. They return
a `ScheduledSend` you can cancel and await, they end when the target actor does,
and they take an injectable `Clock` so the test drives time instead of waiting
for it. Reach for them before writing a timer by hand.

### Reflex 4: treating `ask` as a function call

`ask` is new in 9.0 and it is genuinely useful, which is exactly why it gets
overused. It makes calling an actor look like calling a function, and if you
lean on it the message system quietly turns back into synchronous RPC.

**Use `ask` at the boundary** — from `main`, from tests, from an HTTP handler,
from anything outside the actor system reaching in. That is what it is for, and
it is what makes tests deterministic without sleeping.

**Prefer send-and-react inside the system.** An `ask` couples the caller's
progress to the callee's: while it waits, it is not processing anything else.
Two actors that ask each other deadlock. And `ask` from inside a `mutate_on`
handler is *always* a bug, including asking yourself, because a mutable handler
is awaited inline on the message loop, so the actor cannot process the very
message that would answer it. (You get `AskError::TimedOut` after 30s rather
than a hang, but it is still a bug.)

If a handler needs an answer, it already has a reply envelope. Send, and handle
the reply as an ordinary message.

---

## Part 2: Decomposing a problem into actors

Work through this before writing code. It takes a few minutes and it is the
difference between a system that grows well and one that needs rewriting.

1. **List the state that changes.** Not the operations, the state. Each
   distinct piece of mutable state that has its own lifetime is a candidate
   actor.
2. **Give each piece exactly one owner.** That owner is an actor; the state
   becomes its `#[acton_actor]` struct fields, reached as `actor.model`.
3. **List what crosses between owners.** Each of those is a message, a
   `#[acton_message]` struct. Name them as facts or requests
   (`OrderPlaced`, `Restock`), not as method calls (`DoTheThing`).
4. **Decide how each answer comes back:**
   - Caller is outside the system → `Request` + `ask`.
   - Caller is another actor → send, and let the reply arrive as a message.
   - Many listeners care → broadcast through the broker.
5. **Decide what happens when a piece fails.** If it should come back, it is a
   supervised child with a blueprint (see `references/supervision.md`). Do not
   write a restart loop by hand; 9.0 has a restart engine.
6. **Decide what an operator will want to change without recompiling.**
   Timeouts, mailbox capacities, paths, and log verbosity belong in
   `~/.config/acton/config.toml`; restart policies and supervision strategy do
   not, because a wrong value there is a bug rather than a tuning choice. When
   you write code that sets any of these, say in your answer which file the
   deployed system reads and which keys matter. See
   `references/configuration.md`.

**A useful smell test:** if a struct has state *and* you were about to wrap it
in a lock, or spawn a task that owns it, it wanted to be an actor. If a
function coordinates several actors and remembers anything between calls, it
wanted to be an actor.

**When not to use an actor:** pure computation with no state (a function);
values that are shared but never mutated (an `Arc`); a single counter nobody
branches on (an atomic). Actors cost a task and a mailbox. Use them for state
with an owner, not as a unit of code organisation.

---

## Part 3: Choosing a handler

Pick by **how the handler touches state**, never by how much concurrency you
want. The runtime already gives you the concurrency.

| | `mutate_on<M>` | `act_on<M>` |
|---|---|---|
| Access to `model` | `&mut` | `&` (read-only) |
| Ordering | one at a time, strictly sequential | concurrent, up to the high-water mark |
| `Reply::pending` future | **awaited inline** before the actor takes its next message | drained **concurrently** with later messages |
| Safe to `ask` from inside | **No** — deadlocks | Yes |
| Use for | changing state | reads, I/O, forwarding |

That third row is the one people get wrong, and it matters twice:

- It is why a slow `mutate_on` handler is a bottleneck for the whole actor:
  everything behind it waits for the future to finish.
- It is why a reply from a `mutate_on` handler proves the async work *finished*,
  while a reply from an `act_on` handler only proves the handler *started* it.
  Tests depend on this distinction.

Fallible variants `try_mutate_on<M, T, E>` and `try_act_on<M, T, E>` pair with
`on_error<M, E>` for centralised error handling. Sync variants
`mutate_on_sync` / `act_on_sync` exist for handlers with no `.await` at all.

Return `Reply::ready()` when the handler is done, `Reply::pending(fut)` when it
has async work. Clone any handle or broker you need *before* the async block.

---

## Part 4: The 9.0 API in one page

Enough to write correct code. Exact signatures are in `references/api.md`.

```rust
use acton_reactive::prelude::*;

#[acton_actor]                       // state, must be Default + Debug
struct Counter { count: u64 }

#[acton_message]                     // message, also works on enums
struct Increment;

#[acton_message]
struct GetCount;

#[acton_message]
struct Count { value: u64 }

impl Request for GetCount {          // makes GetCount usable with `ask`
    type Response = Count;
}

#[acton_main]
async fn main() -> anyhow::Result<()> {
    let mut app = ActonApp::launch_async().await;

    let mut counter = app.new_actor::<Counter>();
    counter.mutate_on::<Increment>(|actor, _| {
        actor.model.count += 1;
        Reply::ready()
    });
    counter.act_on::<GetCount>(|actor, ctx| {
        let reply = ctx.reply_envelope();
        let value = actor.model.count;
        Reply::pending(async move { reply.send(Count { value }).await })
    });
    let handle = counter.start().await;

    handle.send(Increment).await;
    let count = handle.ask(GetCount).await?;   // resolves on the first reply
    assert_eq!(count.value, 1);

    app.shutdown_all().await
}
```

**`ask` is also a barrier.** Mailboxes are FIFO, so a completed `ask` proves
every message sent to that actor *before* it has been processed. This is how
you write tests and startup sequences without sleeping. Never use
`tokio::time::sleep` to synchronise — it is slow when it works and flaky when
it does not.

**Cheat sheet of the pieces:**

| Need | Reach for |
|---|---|
| Fire and forget | `handle.send(msg).await` |
| Send it later | `handle.send_after(msg, delay)` / `send_at(msg, deadline)` |
| Send it repeatedly | `handle.send_every(msg, interval, Cadence::FixedRate)` |
| Drive scheduled time in a test | `handle.with_clock(Arc::new(ManualClock::new()))` |
| Ask and wait for one reply | `handle.ask(req).await?` (needs `impl Request`) |
| Ask with a deadline | `handle.ask_with_timeout(req, dur).await?` |
| Reply from a handler | `ctx.reply_envelope()` then `.send(reply).await` |
| Tell many listeners | `broker.broadcast(msg).await` |
| Listen for broadcasts | `builder.handle().subscribe::<M>().await` **before** `start()` |
| Know a broadcast landed | `broker.ask(FlushBroadcasts).await` |
| A child that restarts on failure | `handle.supervise_with(&runtime, config, |a| { .. }).await?` |
| Stop everything | `app.shutdown_all().await?` |

**Traps that bite specifically in 9.x:**

- `subscribe` after `start()` is silently ignored. Subscribe on the builder.
- `unsupervise` now **stops** the child. Use `release` to detach and keep it
  running. This changed in 9.0 and the compiler will not tell you.
- An `ActorHandle` names one incarnation and goes stale across a restart,
  silently. Hold a `SupervisedChild` and call `.current()` instead.
- `after_start` returning `Reply::pending` does **not** hold the actor back;
  its future runs alongside the message loop. "Started" does not mean
  "initialised". Hold the reply envelope and answer when you really are ready.

---

## Part 5: Before you say it is done

Run these. They catch the reflexes above mechanically.

```sh
rg 'Arc<(Mutex|RwLock)' --type rust     # every hit needs a reason
rg 'tokio::spawn' --type rust           # none inside handlers
rg 'sleep\(' --type rust                # none as synchronisation, none as a timer
cargo clippy --all-targets -- -D warnings
cargo nextest run
```

Then read it once against these questions:

- Does every piece of mutable state have exactly one owning actor?
- Is `main` only wiring and shutdown, with no workflow logic?
- Is every handler chosen by state access rather than by desired concurrency?
- Does anything `ask` from inside a `mutate_on` handler?
- Do the tests synchronise with `ask`, never with sleeps? Would each test
  actually fail if the behaviour it names were broken?
- If you hard-coded a timeout, a capacity, or a path, did you tell the reader
  it can be set in `~/.config/acton/config.toml` instead, and which key? Code
  that silently bakes in an operational value leaves whoever deploys it with no
  way to change it and no idea one exists.

---

## Reference files

Read these when the task calls for them; do not preload them all.

| Need | File |
|---|---|
| Exact signatures, prelude contents, error types | `references/api.md` |
| Supervision, restart policies, blueprints, escalation | `references/supervision.md` |
| Deterministic tests, barriers, no sleeps | `references/testing.md` |
| What belongs in `config.toml`, where it lives, every key | `references/configuration.md` |
| Broker, broadcast ordering, flush | `references/pubsub.md` |
| Cross-process actors over Unix sockets (`ipc` feature) | `references/ipc.md` |
| Coming from 8.x, including the silent changes | `references/migration.md` |

**When the answer is not here.** This skill covers the shape of the framework,
not all of it. If you need behaviour none of the references pin down, do not
guess from the API's shape — Acton has several places where the obvious reading
is wrong. Dispatch a subagent to read the actual source under
`acton-reactive/src/` (the message loop is `actor/managed_actor/started.rs`,
supervision is `actor/supervision/`) and report the specific answer. Verifying
costs a minute; a plausible-but-wrong claim about ordering or lifecycle costs
an afternoon of debugging.
