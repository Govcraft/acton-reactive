---
title: Custom Supervision
description: Advanced failure recovery and supervision strategies.
---

While Acton's default supervision handles most cases, complex systems may need custom recovery logic.

## Default Behavior

When an actor fails:
1. The actor stops
2. Parent is notified (if applicable)
3. Children stop (cascade)

---

## Custom Recovery

Implement recovery logic in the parent by tracking child failures:

```rust
#[acton_actor]
struct Supervisor {
    workers: HashMap<String, ActorHandle>,
    restart_counts: HashMap<String, u32>,
}

#[acton_message]
struct WorkerFailed { worker_id: String }

builder.mutate_on::<WorkerFailed>(|actor, envelope| {
    let worker_id = &envelope.message().worker_id;
    let restarts = actor.model.restart_counts
        .entry(worker_id.clone())
        .or_insert(0);

    if *restarts < 3 {
        *restarts += 1;
        tracing::info!("Restarting worker {}", worker_id);

        // Create new worker using create_child + supervise
        let mut worker = actor.create_child(worker_id.clone())
            .expect("Failed to create child");
        worker.mutate_on::<Task>(handle_task);

        let parent_handle = actor.handle().clone();
        let worker_id_clone = worker_id.clone();

        Reply::pending(async move {
            let worker_handle = parent_handle.supervise(worker).await
                .expect("Failed to supervise");
            // Store handle if needed
            tracing::info!("Worker {} restarted", worker_id_clone);
        })
    } else {
        tracing::error!("Worker {} exceeded restart limit", worker_id);
        Reply::ready()
    }
});
```

---

## Rate-Limited Restarts

Prevent restart storms by tracking failure frequency:

```rust
use std::time::{Duration, Instant};

#[acton_actor]
struct RateLimitedSupervisor {
    failures: Vec<Instant>,
    max_failures: usize,
    window: Duration,
}

fn should_restart(model: &mut RateLimitedSupervisor) -> bool {
    let now = Instant::now();
    let window_start = now - model.window;

    // Remove old failures
    model.failures.retain(|&t| t > window_start);

    // Check if we're under the limit
    if model.failures.len() < model.max_failures {
        model.failures.push(now);
        true
    } else {
        false
    }
}
```

---

## Exponential Backoff

Delay restarts with increasing intervals:

```rust
#[acton_actor]
struct BackoffSupervisor {
    base_delay: Duration,
    max_delay: Duration,
    current_attempt: u32,
}

#[acton_message]
struct RestartWorker { id: String }

fn get_backoff_delay(model: &BackoffSupervisor) -> Duration {
    let delay = model.base_delay * 2u32.pow(model.current_attempt);
    std::cmp::min(delay, model.max_delay)
}

builder.mutate_on::<WorkerFailed>(|actor, envelope| {
    let delay = get_backoff_delay(&actor.model);
    actor.model.current_attempt += 1;

    let handle = actor.handle().clone();
    let worker_id = envelope.message().id.clone();

    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        handle.send(RestartWorker { id: worker_id }).await;
    });

    Reply::ready()
});
```

---

## Circuit Breaker Pattern

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

---

## Best Practices

1. **Log failures** before restarting for debugging
2. **Set limits** on restart attempts
3. **Use backoff** to avoid restart storms
4. **Monitor** restart patterns for systemic issues
5. **Fail gracefully** when limits are exceeded

---

## Next

[Performance](/docs/advanced/performance) — Optimizing actor systems
