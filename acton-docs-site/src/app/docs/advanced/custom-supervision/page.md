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

Implement recovery logic in the parent:

```rust
#[acton_actor]
struct Supervisor {
    workers: HashMap<String, AgentHandle>,
    restart_counts: HashMap<String, u32>,
}

#[acton_message]
struct WorkerFailed { worker_id: String }

builder.mutate_on::<WorkerFailed>(|actor, msg| {
    let restarts = actor.model.restart_counts
        .entry(msg.worker_id.clone())
        .or_insert(0);

    if *restarts < 3 {
        *restarts += 1;
        tracing::info!("Restarting worker {}", msg.worker_id);
        let new_worker = spawn_worker(&msg.worker_id);
        actor.model.workers.insert(msg.worker_id.clone(), new_worker);
    } else {
        tracing::error!("Worker {} exceeded restart limit", msg.worker_id);
    }

    Reply::ready()
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

fn should_restart(actor: &mut ManagedAgent<RateLimitedSupervisor>) -> bool {
    let now = Instant::now();
    let window_start = now - actor.model.window;

    // Remove old failures
    actor.model.failures.retain(|&t| t > window_start);

    // Check if we're under the limit
    if actor.model.failures.len() < actor.model.max_failures {
        actor.model.failures.push(now);
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

fn get_backoff_delay(actor: &BackoffSupervisor) -> Duration {
    let delay = actor.base_delay * 2u32.pow(actor.current_attempt);
    std::cmp::min(delay, actor.max_delay)
}

builder.mutate_on::<WorkerFailed>(|actor, msg| {
    let delay = get_backoff_delay(&actor.model);
    actor.model.current_attempt += 1;

    let handle = actor.handle().clone();
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        handle.send(RestartWorker { id: msg.worker_id }).await.ok();
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

fn check_circuit(actor: &mut CircuitBreaker) -> bool {
    match actor.state {
        CircuitState::Closed => true,
        CircuitState::Open => {
            if let Some(last) = actor.last_failure {
                if last.elapsed() > actor.reset_timeout {
                    actor.state = CircuitState::HalfOpen;
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

[Performance](/docs/advanced/performance) - Optimizing actor systems
