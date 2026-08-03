---
title: Custom Supervision
description: Working with the restart engine, termination reasons, and recovery patterns.
---

Acton restarts supervised children for you. This page is about the parts you still control: what a restarted child is rebuilt from, how to react to failures the engine does not act on, and when to step outside the engine entirely.

{% callout type="note" title="Prerequisites" %}
This page assumes [Supervision Basics](/docs/core-concepts/supervision-basics). Make sure you understand blueprints, `SupervisedChild`, restart policies and supervision strategies before proceeding.
{% /callout %}

---

## What the framework does and what you do

| The framework does | You do |
|---|---|
| Isolates the failure to one actor | Write the blueprint that rebuilds it |
| Consults the child's `RestartPolicy` and the supervisor's `SupervisionStrategy` | Choose those settings |
| Rebuilds the child from its blueprint, after a backoff, keeping its identifier | Seed any state the replacement needs |
| Repoints the child's IPC names at the new incarnation | Hold a `SupervisedChild`, not a stale handle |
| Bounds retries with a restart limiter, then escalates | Decide what escalation means for your system |
| Publishes `ChildSupervised`, `ChildRestarted`, `SupervisionEscalated` | Observe them, if you want metrics or alerts |
| Sends `ChildTerminated` to the supervisor | Handle it for anything beyond restarting |

{% callout type="warning" title="Do not restart from your own ChildTerminated handler" %}
For a child registered through `supervise_with` or `supervise_deferred`, the framework is already restarting it. A hand-rolled restart in your `ChildTerminated` handler will bring it back a **second** time.

Your handler still runs, and it should: logging, metrics, alerting, and application-level bookkeeping all belong there. What it must not do any more is recreate the child.

Children adopted through the older `supervise()` call have no blueprint and are never restarted by the framework. If you have hand-rolled restarts for those, they keep working unchanged, and there is no double-restart, until you migrate that child.
{% /callout %}

---

## The blueprint is where recovery happens

A blueprint is a closure the framework replays to build every incarnation. It is the answer to "what does the replacement know?"

```rust
let blueprint = move |actor: &mut ManagedActor<Idle, Worker>| {
    actor.mutate_on::<Task>(handle_task);
    actor.act_on::<GetStatus>(report_status);
};
```

Because it is a closure, anything it captures is available to every incarnation. This is what replaces the "recreate the child by hand and replay its state" pattern earlier versions needed.

### Recovering state across a restart

{% callout type="note" title="Actors start from Default" %}
An actor's model always begins as `State::default()`. A restart is not a resume: the replacement does not inherit the old incarnation's memory.
{% /callout %}

Seed it from something outside the actor. A shared store the blueprint captures is the simplest form:

```rust
#[acton_actor]
struct Worker {
    checkpoint: u64,
}

// Durable enough to outlive an incarnation. A database or key-value store
// belongs here in a real system.
let checkpoints: Arc<Mutex<HashMap<String, u64>>> = Arc::default();

let blueprint = {
    let checkpoints = Arc::clone(&checkpoints);
    move |actor: &mut ManagedActor<Idle, Worker>| {
        // Runs on every start, including every restart.
        actor.model.checkpoint = checkpoints
            .lock()
            .unwrap()
            .get("worker")
            .copied()
            .unwrap_or(0);

        let checkpoints = Arc::clone(&checkpoints);
        actor.mutate_on::<Progress>(move |actor, ctx| {
            actor.model.checkpoint = ctx.message().position;
            checkpoints
                .lock()
                .unwrap()
                .insert("worker".into(), actor.model.checkpoint);
            Reply::ready()
        });
    }
};

let worker = supervisor_handle
    .supervise_with::<Worker>(&runtime, config, blueprint)
    .await?;
```

The restarted worker picks up from the last checkpoint, because the blueprint reads it at build time.

**Keep the blueprint cheap and infallible.** It runs on the supervisor's restart path; a blueprint that blocks or panics turns one failed child into a failing supervisor. Do the slow part in `after_start`, or in a message the actor sends itself.

---

## Handling ChildTerminated

The supervisor still receives `ChildTerminated` for every child that stops, whether or not the framework restarts it:

```rust
pub struct ChildTerminated {
    pub child_id: Ern,                 // which child terminated
    pub reason: TerminationReason,     // why it terminated
    pub restart_policy: RestartPolicy, // the child's configured policy
}
```

The framework's own bookkeeping runs **first**, so your handler sees a settled registry rather than a half-updated one.

```rust
supervisor.mutate_on::<ChildTerminated>(|actor, ctx| {
    let note = ctx.message();

    *actor.model.failure_counts
        .entry(note.child_id.to_string())
        .or_insert(0) += 1;

    tracing::warn!(
        child = %note.child_id,
        reason = ?note.reason,
        "child terminated"
    );

    Reply::ready()
});
```

The payload is reached with `ctx.message()`, which returns `&ChildTerminated`. There is no public `ctx.message` field.

{% callout type="warning" title="The child must know its parent" %}
`ChildTerminated` is only sent if the child was built with a parent reference. `ActorConfig::for_supervised_child(name, parent_handle, broker)` establishes that; a child built with `ActorConfig::new` and then handed to `supervise()` will stop with its parent but never notify anyone.
{% /callout %}

---

## Termination reasons

```rust
pub enum TerminationReason {
    /// Normal graceful shutdown via `SystemSignal::Terminate`
    Normal,

    /// Actor panicked (only with `catch-handler-panics` disabled)
    Panic(String),

    /// Actor inbox closed unexpectedly (all handles dropped)
    InboxClosed,

    /// Stopped because its supervisor is stopping
    ParentShutdown,
}
```

`ParentShutdown` is the one to check for in your own code. A `Permanent` child warrants a restart from a `Normal` termination, so a handler that restarts on "anything but a panic" will fight the shutdown it is part of. The framework suppresses its own restart decisions during shutdown; **a hand-rolled handler does not**.

```rust
supervisor.mutate_on::<ChildTerminated>(|actor, ctx| {
    match &ctx.message().reason {
        TerminationReason::ParentShutdown => {
            // We are on the way down. Do nothing.
            tracing::debug!("child stopped with its supervisor");
        }
        TerminationReason::Normal => {
            tracing::info!("child shut down normally");
        }
        TerminationReason::InboxClosed => {
            tracing::warn!("child inbox closed unexpectedly");
        }
        TerminationReason::Panic(msg) => {
            tracing::error!("child panicked: {msg}");
        }
    }
    Reply::ready()
});
```

{% callout type="note" title="When is Panic produced?" %}
Whether a handler panic produces `TerminationReason::Panic` depends on the `catch-handler-panics` feature:

- **Enabled (default)**: the panic is caught at the handler dispatch site and logged, and the actor keeps running. No termination occurs, so nothing is restarted. To react to failures in this mode, return errors from `try_mutate_on` / `try_act_on` and handle them with `on_error`.
- **Disabled** (`default-features = false`): a handler panic terminates the actor cleanly, its broker subscriptions are removed, its children are stopped, and its supervisor receives `ChildTerminated` with `TerminationReason::Panic` carrying the message. The notification is guaranteed even if `after_stop` panics during cleanup. Panics in `after_start` and `before_stop` report the same way; a panic in `before_start` propagates to the `start()` caller instead.
{% /callout %}

---

## Observing supervision without being the supervisor

The broker publishes three supervision events, so metrics and alerting do not have to live in the supervisor:

```rust
monitor.handle().subscribe::<ChildRestarted>().await;
monitor.handle().subscribe::<SupervisionEscalated>().await;

monitor
    .mutate_on::<ChildRestarted>(|actor, ctx| {
        let event = ctx.message();
        actor.model.restarts += 1;
        tracing::warn!(
            child = %event.child,
            generation = ?event.generation,
            reason = ?event.reason,
            "restarted"
        );
        Reply::ready()
    })
    .mutate_on::<SupervisionEscalated>(|actor, ctx| {
        let event = ctx.message();
        tracing::error!(
            child = %event.child,
            last_reason = ?event.last_reason,
            "supervisor gave up; paging on-call"
        );
        Reply::ready()
    });
```

`ChildSupervised` carries a `can_restart` flag, which is `false` for children registered without a blueprint. That is a useful thing to alert on: it means the child will not come back.

---

## Watching a child's status directly

`SupervisedChild` publishes a state machine you can poll or wait on:

| State | Meaning |
|---|---|
| `Starting` | Created, not yet recorded as running |
| `Running` | Processing messages |
| `RestartPending` | Terminated, restart scheduled, waiting out its backoff |
| `Restarting` | Replacement being built |
| `Down` | Terminated and will not come back: its policy forbids it, or it has no blueprint |
| `Escalated` | Exhausted its restart allowance; the supervisor gave up |

```rust
match worker.status().state() {
    SupervisionState::Running => worker.current()?.send(Task).await,
    SupervisionState::RestartPending | SupervisionState::Restarting => {
        // Wait for the replacement rather than sending into a dead mailbox.
        worker.wait_running().await?.send(Task).await;
    }
    SupervisionState::Escalated | SupervisionState::Down => {
        tracing::error!("worker is not coming back");
    }
    _ => {}
}
```

`wait_running()` returns an error rather than hanging once a child reaches `Escalated`.

---

## When to step outside the engine

The restart engine handles "the actor died, build another one". Two problems it deliberately does not solve:

### The dependency is down, not the actor

Restarting an actor whose database is unreachable produces a fresh actor that also cannot reach the database, on a backoff schedule. A **circuit breaker** belongs in the actor, around the dependency:

```rust
#[acton_actor]
struct Gateway {
    state: CircuitState,
    failure_count: u32,
    threshold: u32,
    reset_timeout: Duration,
    last_failure: Option<Instant>,
}

enum CircuitState {
    Closed,    // normal operation
    Open,      // failing; don't attempt
    HalfOpen,  // testing if recovered
}

fn may_attempt(model: &mut Gateway) -> bool {
    match model.state {
        CircuitState::Closed | CircuitState::HalfOpen => true,
        CircuitState::Open => {
            let expired = model
                .last_failure
                .is_some_and(|last| last.elapsed() > model.reset_timeout);
            if expired {
                model.state = CircuitState::HalfOpen;
            }
            expired
        }
    }
}

fn record_failure(model: &mut Gateway) {
    model.failure_count += 1;
    model.last_failure = Some(Instant::now());
    if model.failure_count >= model.threshold {
        model.state = CircuitState::Open;
    }
}
```

The actor stays up and refuses work cheaply, instead of dying and being rebuilt.

### The failure is expected

A malformed request is not a reason to restart anything. Use `try_mutate_on` / `try_act_on` with `on_error`, and keep the actor running.

---

## Supervision tree patterns

### Worker pool

```rust
let mut runtime = ActonApp::launch_async().await;

let supervisor_config = ActorConfig::new(Ern::with_root("pool-supervisor")?, None)
    .with_supervision_strategy(SupervisionStrategy::OneForOne);

let supervisor = runtime
    .new_actor_with_config::<PoolSupervisor>(supervisor_config)
    .start()
    .await;

let mut workers = Vec::new();
for i in 0..4 {
    let config = ActorConfig::for_supervised_child(
        format!("worker-{i}"),
        supervisor.clone(),
        None,
    )?
    .with_restart_policy(RestartPolicy::Permanent);

    workers.push(
        supervisor
            .supervise_with::<Worker>(&runtime, config, |actor| {
                actor.mutate_on::<Task>(handle_task);
            })
            .await?,
    );
}

// Dispatch through `current()`, which always names the live incarnation.
workers[0].current()?.send(Task).await;
```

`OneForOne` is the default, and the right choice here: one worker dying says nothing about the others.

### Pipeline with RestForOne

When later stages depend on earlier ones, a failure partway through should take the downstream stages with it:

```rust
let pipeline_config = ActorConfig::new(Ern::with_root("pipeline")?, None)
    .with_supervision_strategy(SupervisionStrategy::RestForOne);

let pipeline = runtime
    .new_actor_with_config::<Pipeline>(pipeline_config)
    .start()
    .await;

// Registration order is the start order the strategy uses.
let ingester  = supervise_stage(&pipeline, &runtime, "ingester").await?;   // index 0
let processor = supervise_stage(&pipeline, &runtime, "processor").await?;  // index 1
let outputter = supervise_stage(&pipeline, &runtime, "outputter").await?;  // index 2
```

If `processor` fails, the framework stops `outputter` then `processor`, in reverse start order, and brings both back. `ingester` is untouched.

**Rebuilds are requested in start order but not awaited in it**, because each start runs on its own task so a stage's `before_start` cannot stall the supervisor. A stage that cannot come up until its upstream is ready must wait for it:

```rust
// In outputter's blueprint: don't assume processor is up yet.
actor.after_start(|actor| {
    let processor = actor.model.processor.clone();
    Reply::pending(async move {
        let _ = processor.wait_running().await;
    })
});
```

---

## Best practices

1. **Register children with `supervise_with` or `supervise_deferred`** so they have a blueprint. Without one, a child that dies stays down.
2. **Delete hand-rolled restarts** for any child you migrate, or it will come back twice.
3. **Hold `SupervisedChild`, call `current()` at the point of use.** A stored `ActorHandle` goes stale on the first restart, silently.
4. **Keep blueprints cheap and infallible.** They run on the supervisor's restart path.
5. **Seed recoverable state from outside the actor**, captured by the blueprint. Actors always start from `Default`.
6. **Check `TerminationReason::ParentShutdown`** in any handler of your own that acts on termination.
7. **Match strategy to architecture.** `OneForAll` on independent children turns one failure into an outage.
8. **Alert on `SupervisionEscalated`.** It is the framework telling you it has run out of options.
9. **Don't wait for panics.** With the default `catch-handler-panics` feature they are caught and never terminate an actor. Use `try_mutate_on` + `on_error` for expected failures.

---

## Next

[Performance](/docs/advanced/performance) — Optimizing actor systems
