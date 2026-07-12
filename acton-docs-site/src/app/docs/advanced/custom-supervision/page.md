---
title: Custom Supervision
description: Advanced failure recovery and supervision strategies.
---

Acton gives you supervision *primitives*, not a supervision engine. The framework isolates failures and tells the parent when a child terminates; **your parent actor writes the recovery logic**. This page covers the primitives and the patterns you build with them.

{% callout type="note" title="Prerequisites" %}
This page assumes familiarity with [Supervision Basics](/docs/core-concepts/supervision-basics). Make sure you understand supervision strategies and restart policies before proceeding.
{% /callout %}

---

## What Acton Does and What You Do

This is the single most important thing to internalize before building custom supervision:

| Acton does | You do |
|------------|--------|
| Isolates the failure to one actor | Decide whether to recover |
| Stops children when the parent stops | Recreate and re-supervise children |
| Sends `ChildTerminated` to the parent | Handle it with `mutate_on::<ChildTerminated>` |
| Catches handler panics (default) | Rate-limit your own restarts |
| Provides `SupervisionStrategy::decide()`, `RestartPolicy::should_restart()`, and `RestartLimiter` as helpers | **Call** those helpers and act on what they return |

{% callout type="warning" title="Configuration is input, not behavior" %}
`ActorConfig::with_supervision_strategy()` and `ActorConfig::with_restart_limiter()` record your intent on the actor's configuration. They do **not** cause the runtime to restart anything on their own — Acton has no automatic restart loop. Treat them as values you read back and feed into your own `ChildTerminated` handler.

If you configure a restart limiter and never call it, nothing is rate-limited.
{% /callout %}

---

## The Restart Limiter

`RestartLimiter` is a helper you call from your own supervision handler to avoid restart storms. It tracks restarts in a sliding window and hands you an exponential backoff delay.

### Configuration

```rust
use acton_reactive::prelude::*;

let limiter_config = RestartLimiterConfig {
    enabled: true,
    max_restarts: 5,        // Max restarts in time window
    window_secs: 60,        // 1 minute window
    initial_backoff_ms: 100,
    max_backoff_ms: 30_000, // 30 second max delay
    backoff_multiplier: 2.0,
};
```

### Defaults

`RestartLimiterConfig::default()` uses:

| Setting | Default |
|---------|---------|
| `enabled` | `true` |
| `max_restarts` | 5 |
| `window_secs` | 60 |
| `initial_backoff_ms` | 100 |
| `max_backoff_ms` | 30,000 |
| `backoff_multiplier` | 2.0 |

`RestartLimiterConfig::disabled()` gives you one that always allows a restart.

### Using It

Keep a `RestartLimiter` in your supervisor's state and consult it before each restart:

```rust
#[acton_actor(no_default)]
struct Supervisor {
    limiter: RestartLimiter,
}

impl Default for Supervisor {
    fn default() -> Self {
        Self { limiter: RestartLimiter::new(RestartLimiterConfig::default()) }
    }
}

supervisor.mutate_on::<ChildTerminated>(|actor, ctx| {
    let note = ctx.message().clone();

    // 1. Is a restart allowed at all?
    if let Err(exceeded) = actor.model.limiter.can_restart() {
        tracing::error!("Restart limit exceeded: {exceeded}; escalating");
        return Reply::ready();  // Alert, degrade, or stop — your call
    }

    // 2. Record it and get the backoff to wait
    let backoff = actor.model.limiter.record_restart();

    let parent_handle = actor.handle().clone();
    Reply::pending(async move {
        tokio::time::sleep(backoff).await;
        // Recreate and re-supervise the child here
        tracing::info!("Restarting {} after {:?}", note.child_id, backoff);
    })
});
```

`can_restart()` returns `Err(RestartLimitExceeded)` once the window is full — that error is your cue to escalate.

---

## Handling ChildTerminated

When a child terminates, the parent receives a `ChildTerminated` message. You can handle this message for custom supervision logic.

### The ChildTerminated Message

```rust
pub struct ChildTerminated {
    pub child_id: Ern,            // Which child terminated
    pub reason: TerminationReason, // Why it terminated
    pub restart_policy: RestartPolicy, // Child's configured policy
}
```

{% callout type="warning" title="The child must know its parent" %}
`ChildTerminated` is only sent if the child was created with a **parent reference** in its `ActorConfig`. Passing `None` for the parent and then calling `supervise()` registers the child for cascading shutdown but leaves it unable to notify anyone when it terminates — your handler will simply never fire.

Always pass `Some(supervisor_handle.clone())` as the second argument to `ActorConfig::new`.
{% /callout %}

### Custom Handler

```rust
use acton_reactive::prelude::*;

#[acton_actor]
struct Supervisor {
    failure_counts: HashMap<String, u32>,
    max_failures: u32,
}

// Handle child termination notifications
supervisor.mutate_on::<ChildTerminated>(|actor, ctx| {
    let notification = ctx.message();  // accessor, not a field
    let child_id = notification.child_id.to_string();
    let reason = notification.reason.clone();

    // Track failures
    let count = actor.model.failure_counts
        .entry(child_id.clone())
        .or_insert(0);
    *count += 1;

    tracing::warn!(
        "Child {} terminated ({}): {:?}",
        child_id,
        count,
        reason
    );

    // Custom logic based on failure count
    if *count >= actor.model.max_failures {
        tracing::error!("Child {} exceeded failure limit, escalating", child_id);
        // Could notify monitoring, alert on-call, etc.
    }

    Reply::ready()
});
```

The message payload is reached with `ctx.message()`, which returns `&ChildTerminated`. There is no public `ctx.message` field.

This pattern enables:
- Tracking failure counts per child
- Custom escalation logic
- Alerting and monitoring integration
- Different handling based on termination reason

---

## Termination Reasons

The `TerminationReason` enum tells you *why* a child terminated:

```rust
pub enum TerminationReason {
    /// Normal graceful shutdown via `SystemSignal::Terminate`
    Normal,

    /// Reserved — see the note below. Not currently produced by the runtime.
    Panic(String),

    /// Actor inbox closed unexpectedly (all handles dropped)
    InboxClosed,

    /// Parent-initiated cascading shutdown
    ParentShutdown,
}
```

{% callout type="note" title="Panic is reserved and not currently emitted" %}
The runtime only produces `Normal`, `InboxClosed`, and `ParentShutdown`. With the default `catch-handler-panics` feature, a panicking handler is caught, logged, and **the actor keeps running** — so it never terminates and never reports `Panic`.

Keep a `Panic(_)` arm for forward compatibility if you like, but don't build recovery logic that depends on it firing. To react to failures, return errors from `try_mutate_on` / `try_act_on` and handle them with `on_error` instead.
{% /callout %}

### Using Termination Reasons

```rust
supervisor.mutate_on::<ChildTerminated>(|actor, ctx| {
    match &ctx.message().reason {
        TerminationReason::Normal => {
            // Expected shutdown, may not need action
            tracing::info!("Child shut down normally");
        }
        TerminationReason::InboxClosed => {
            // Unexpected closure - investigate
            tracing::warn!("Child inbox closed unexpectedly");
        }
        TerminationReason::ParentShutdown => {
            // Cascading shutdown - this is expected, never restart
            tracing::debug!("Child stopped due to parent shutdown");
        }
        TerminationReason::Panic(msg) => {
            // Reserved; not currently produced by the runtime
            tracing::error!("Child panicked: {}", msg);
        }
    }
    Reply::ready()
});
```

---

## Custom Recovery Patterns

When built-in supervision isn't enough, implement custom recovery logic.

### Restart with State Recovery

{% callout type="note" title="Actors always start from Default" %}
There is no constructor that seeds an actor with a pre-built state value — every actor begins life as `State::default()`. To recover state across a restart, **replay it into the fresh actor as a message**.
{% /callout %}

Keep the recoverable state in the supervisor (or, better, in an external store), then send it to the new child immediately after starting it:

```rust
#[acton_actor]
struct Supervisor {
    worker_state: HashMap<String, WorkerSnapshot>,
}

#[acton_message]
struct RestoreState { snapshot: WorkerSnapshot }

supervisor.mutate_on::<ChildTerminated>(|actor, ctx| {
    let child_id = ctx.message().child_id.to_string();

    // Get the last known snapshot for this worker
    let snapshot = actor.model.worker_state
        .get(&child_id)
        .cloned()
        .unwrap_or_default();

    let parent_handle = actor.handle().clone();
    let mut runtime = actor.runtime().clone();

    Reply::pending(async move {
        // Recreate the worker — note the parent reference, or it will never
        // notify us if it terminates again.
        let config = ActorConfig::new(
            Ern::with_root("worker").unwrap(),
            Some(parent_handle.clone()),
            None,
        ).unwrap();

        let mut worker = runtime.new_actor_with_config::<WorkerState>(config);
        worker
            .mutate_on::<RestoreState>(|actor, ctx| {
                actor.model.apply(ctx.message().snapshot.clone());
                Reply::ready()
            })
            .mutate_on::<Task>(handle_task);

        let new_handle = parent_handle.supervise(worker).await
            .expect("Failed to supervise");

        // Replay the recovered state as the worker's first message
        new_handle.send(RestoreState { snapshot }).await;

        tracing::info!("Worker {} restarted with recovered state", child_id);
    })
});
```

Because the worker's inbox is FIFO, `RestoreState` is guaranteed to be processed before any work sent afterwards.

### Circuit Breaker Pattern

Stop attempting restarts after repeated failures:

```rust
#[acton_actor]
struct CircuitBreaker {
    state: CircuitState,
    failure_count: u32,
    threshold: u32,
    reset_timeout: Duration,
    last_failure: Option<Instant>,
}

enum CircuitState {
    Closed,    // Normal operation
    Open,      // Failing, don't attempt
    HalfOpen,  // Testing if recovered
}

fn check_circuit(model: &mut CircuitBreaker) -> bool {
    match model.state {
        CircuitState::Closed => true,
        CircuitState::Open => {
            if let Some(last) = model.last_failure {
                if last.elapsed() > model.reset_timeout {
                    model.state = CircuitState::HalfOpen;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        }
        CircuitState::HalfOpen => true,
    }
}

fn record_failure(model: &mut CircuitBreaker) {
    model.failure_count += 1;
    model.last_failure = Some(Instant::now());

    if model.failure_count >= model.threshold {
        model.state = CircuitState::Open;
    }
}

fn record_success(model: &mut CircuitBreaker) {
    model.failure_count = 0;
    model.state = CircuitState::Closed;
}
```

{% callout type="note" title="Circuit breaker vs RestartLimiter" %}
Use a circuit breaker when you need to stop *all* operations to a subsystem, not just restarts. For rate-limiting restarts specifically, reach for `RestartLimiter` — it already implements the sliding window and exponential backoff, so you only have to call it.
{% /callout %}

---

## Supervision Tree Patterns

### Worker Pool with Shared Supervisor

```rust
let mut runtime = ActonApp::launch_async().await;

// Create the supervisor. The strategy is recorded on the config so the
// supervisor's own handler can read it back — it does not restart anything
// by itself.
let supervisor_config = ActorConfig::new(
    Ern::with_root("pool-supervisor")?,
    None,
    None,
)?
.with_supervision_strategy(SupervisionStrategy::OneForOne);

let mut supervisor = runtime.new_actor_with_config::<PoolSupervisor>(supervisor_config);
supervisor.mutate_on::<ChildTerminated>(restart_one_worker);  // you write this
let supervisor_handle = supervisor.start().await;

// Create workers with Permanent policy
for i in 0..4 {
    let worker_config = ActorConfig::new(
        Ern::with_root(format!("worker-{}", i))?,
        Some(supervisor_handle.clone()),  // required: enables ChildTerminated
        None,
    )?
    .with_restart_policy(RestartPolicy::Permanent);

    let mut worker = runtime.new_actor_with_config::<Worker>(worker_config);
    worker.mutate_on::<Task>(handle_task);

    supervisor_handle.supervise(worker).await?;
}
```

### Pipeline with RestForOne

For pipelines where later stages depend on earlier ones. `RestForOne` means: when stage *n* fails, stages *n* and later should come back — but **your handler carries that out**, using the child's index in your own ordered list.

```rust
let pipeline_config = ActorConfig::new(
    Ern::with_root("pipeline")?,
    None,
    None,
)?
.with_supervision_strategy(SupervisionStrategy::RestForOne);

let mut pipeline = runtime.new_actor_with_config::<Pipeline>(pipeline_config);
let pipeline_handle = pipeline.start().await;

// Build each stage with the pipeline as its parent, in dependency order.
let ingester  = create_stage(&mut runtime, "ingester",  &pipeline_handle)?;
let processor = create_stage(&mut runtime, "processor", &pipeline_handle)?;
let outputter = create_stage(&mut runtime, "outputter", &pipeline_handle)?;

// Track start order — RestForOne decisions are index-based.
pipeline_handle.supervise(ingester).await?;  // index 0
pipeline_handle.supervise(processor).await?; // index 1
pipeline_handle.supervise(outputter).await?; // index 2
```

In the pipeline's `ChildTerminated` handler, `decide()` turns the notification into an index to restart *from*:

```rust
pipeline.mutate_on::<ChildTerminated>(|actor, ctx| {
    let note = ctx.message();

    // Look up where this child sat in the start order
    let index = actor.model.stage_order
        .iter()
        .position(|id| id == &note.child_id)
        .unwrap_or(0);

    match SupervisionStrategy::RestForOne.decide(note, index) {
        SupervisionDecision::RestartFrom(from) => {
            // Stop and recreate stages `from`..end, in order — your code
            tracing::warn!("Restarting pipeline from stage {from}");
        }
        SupervisionDecision::NoRestart => { /* policy says leave it down */ }
        _ => {}
    }
    Reply::ready()
});
```

---

## Best Practices

1. **Give every child a parent reference** — without it, `ChildTerminated` is never sent and none of this runs
2. **Log failures** before restarting for debugging
3. **Use `RestartLimiter`** rather than hand-rolling backoff — but remember you have to *call* it
4. **Match strategy to architecture** — don't use OneForAll when children are independent
5. **Monitor restart patterns** for systemic issues
6. **Fail gracefully** when limits are exceeded (alert, degrade, escalate)
7. **Keep state external** so restarts can recover — actors always start from `Default`
8. **Don't wait for panics** — they're caught by default and never terminate an actor. Use `try_mutate_on` + `on_error` for expected failures.

---

## Next

[Performance](/docs/advanced/performance) — Optimizing actor systems
