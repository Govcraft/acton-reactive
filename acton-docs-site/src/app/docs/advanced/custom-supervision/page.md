---
title: Custom Supervision
description: Advanced failure recovery and supervision strategies.
---

While Acton's built-in supervision (strategies, policies, restart limiting) handles most cases, complex systems may need custom recovery logic. This page covers advanced supervision features and custom patterns.

{% callout type="note" title="Prerequisites" %}
This page assumes familiarity with [Supervision Basics](/docs/core-concepts/supervision-basics). Make sure you understand supervision strategies and restart policies before proceeding.
{% /callout %}

---

## Built-in Restart Limiting

Acton includes built-in restart limiting with exponential backoff. This prevents restart storms and cascading failures in production systems.

### Configuration

```rust
use acton_reactive::prelude::*;

let config = ActorConfig::new(
    Ern::with_root("worker")?,
    Some(parent_handle.clone()),
    None,
)?
.with_restart_limiter(RestartLimiterConfig {
    enabled: true,
    max_restarts: 5,        // Max restarts in time window
    window_secs: 60,        // 1 minute window
    initial_backoff_ms: 100,
    max_backoff_ms: 30_000, // 30 second max delay
    backoff_multiplier: 2.0,
});
```

### How It Works

1. **Sliding window**: Tracks restarts within a configurable time window
2. **Exponential backoff**: Delays between restarts grow exponentially (100ms → 200ms → 400ms...)
3. **Escalation**: When the limit is exceeded, the supervisor escalates to its parent

### Default Values

If you don't configure restart limiting, these defaults apply:

| Setting | Default |
|---------|---------|
| `max_restarts` | 5 |
| `window_secs` | 60 |
| `initial_backoff_ms` | 100 |
| `max_backoff_ms` | 30,000 |
| `backoff_multiplier` | 2.0 |

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
    let notification = ctx.message;
    let child_id = notification.child_id.to_string();

    // Track failures
    let count = actor.model.failure_counts
        .entry(child_id.clone())
        .or_insert(0);
    *count += 1;

    tracing::warn!(
        "Child {} terminated ({}): {:?}",
        child_id,
        count,
        notification.reason
    );

    // Custom logic based on failure count
    if *count >= actor.model.max_failures {
        tracing::error!("Child {} exceeded failure limit, escalating", child_id);
        // Could notify monitoring, alert on-call, etc.
    }

    Reply::ready()
});
```

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

    /// Actor panicked during message handling or lifecycle hook
    Panic(String),  // Contains the panic message

    /// Actor inbox closed unexpectedly
    InboxClosed,

    /// Parent-initiated cascading shutdown
    ParentShutdown,
}
```

### Using Termination Reasons

```rust
supervisor.mutate_on::<ChildTerminated>(|actor, ctx| {
    match &ctx.message.reason {
        TerminationReason::Normal => {
            // Expected shutdown, may not need action
            tracing::info!("Child shut down normally");
        }
        TerminationReason::Panic(msg) => {
            // Crash - log details and potentially alert
            tracing::error!("Child panicked: {}", msg);
        }
        TerminationReason::InboxClosed => {
            // Unexpected closure - investigate
            tracing::warn!("Child inbox closed unexpectedly");
        }
        TerminationReason::ParentShutdown => {
            // Cascading shutdown - this is expected
            tracing::debug!("Child stopped due to parent shutdown");
        }
    }
    Reply::ready()
});
```

---

## Custom Recovery Patterns

When built-in supervision isn't enough, implement custom recovery logic.

### Restart with State Recovery

```rust
#[acton_actor]
struct Supervisor {
    worker_handles: HashMap<String, ActorHandle>,
    worker_state: HashMap<String, WorkerState>,
}

supervisor.mutate_on::<ChildTerminated>(|actor, ctx| {
    let child_id = ctx.message.child_id.to_string();

    // Get persisted state for this worker
    let saved_state = actor.model.worker_state
        .get(&child_id)
        .cloned()
        .unwrap_or_default();

    let parent_handle = actor.handle().clone();
    let runtime = actor.runtime().clone();

    Reply::pending(async move {
        // Create new worker with recovered state
        let config = ActorConfig::new(
            Ern::with_root(&child_id).unwrap(),
            None,
            None,
        ).unwrap();

        let mut worker = runtime.new_actor_with_config_and_state::<WorkerState>(
            config,
            saved_state,
        );

        worker.mutate_on::<Task>(handle_task);

        let new_handle = parent_handle.supervise(worker).await
            .expect("Failed to supervise");

        tracing::info!("Worker {} restarted with recovered state", child_id);
    })
});
```

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

{% callout type="note" title="Built-in vs Custom" %}
The circuit breaker pattern is useful when you need to stop *all* operations to a subsystem, not just restarts. For restart limiting, use the built-in `RestartLimiterConfig`.
{% /callout %}

---

## Supervision Tree Patterns

### Worker Pool with Shared Supervisor

```rust
let mut runtime = ActonApp::launch_async().await;

// Create supervisor with OneForOne strategy
let supervisor_config = ActorConfig::new(
    Ern::with_root("pool-supervisor")?,
    None,
    None,
)?
.with_supervision_strategy(SupervisionStrategy::OneForOne);

let supervisor = runtime.new_actor_with_config::<PoolSupervisor>(supervisor_config);
let supervisor_handle = supervisor.start().await;

// Create workers with Permanent policy
for i in 0..4 {
    let worker_config = ActorConfig::new(
        Ern::with_root(format!("worker-{}", i))?,
        None,
        None,
    )?
    .with_restart_policy(RestartPolicy::Permanent);

    let mut worker = runtime.new_actor_with_config::<Worker>(worker_config);
    worker.mutate_on::<Task>(handle_task);

    supervisor_handle.supervise(worker).await?;
}
```

### Pipeline with RestForOne

For pipelines where later stages depend on earlier ones:

```rust
let pipeline_config = ActorConfig::new(
    Ern::with_root("pipeline")?,
    None,
    None,
)?
.with_supervision_strategy(SupervisionStrategy::RestForOne);

let pipeline = runtime.new_actor_with_config::<Pipeline>(pipeline_config);
let pipeline_handle = pipeline.start().await;

// Stages in order of dependency
let ingester = create_ingester(&mut runtime);
let processor = create_processor(&mut runtime);
let outputter = create_outputter(&mut runtime);

// Order matters with RestForOne!
pipeline_handle.supervise(ingester).await?;  // Stage 1
pipeline_handle.supervise(processor).await?; // Stage 2
pipeline_handle.supervise(outputter).await?; // Stage 3
// If processor fails, outputter is also restarted
```

---

## Best Practices

1. **Log failures** before restarting for debugging
2. **Use built-in restart limiting** rather than implementing your own
3. **Match strategy to architecture** — don't use OneForAll when children are independent
4. **Monitor restart patterns** for systemic issues
5. **Fail gracefully** when limits are exceeded (alert, degrade, escalate)
6. **Keep state external** so restarts can recover

---

## Next

[Performance](/docs/advanced/performance) — Optimizing actor systems
