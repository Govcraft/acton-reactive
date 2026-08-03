---
title: Supervision Basics
description: How Acton keeps your application running when actors fail.
---

When actors fail, supervision ensures your system keeps running. Instead of letting one error crash everything, Acton contains the failure and brings the actor back.

## The Problem Supervision Solves

In traditional programs, an unhandled error often crashes the whole process. With actors, failures are isolated: if one actor fails, others continue normally.

Supervision adds organized recovery on top of that isolation. **A supervised child that dies is rebuilt from its blueprint, after a backoff, keeping its identifier.** You describe how the child is built and what should happen when it fails; the framework carries it out.

{% callout type="warning" title="Changed in 9.0.0" %}
Earlier versions delivered a `ChildTerminated` notification and left the restart to you. The framework now restarts children registered through `supervise_with` and `supervise_deferred`.

**If you hand-rolled restarts in a `ChildTerminated` handler, do not migrate a child to those APIs without deleting your own restart for it**, or it will come back twice. Children adopted through the older `supervise()` call have no blueprint and are never restarted by the framework, so existing code is unaffected until you change it. See the [migration guide](/docs/reference/migration-guide).
{% /callout %}

---

## Registering a supervised child

A supervised child needs three things: a config naming it and its parent, a **blueprint** describing how to build it, and a supervisor to register it with.

```rust
let mut runtime = ActonApp::launch_async().await;

let parent = runtime.new_actor::<ParentState>();
let parent_handle = parent.start().await;

let config = ActorConfig::for_supervised_child("worker", parent_handle.clone(), None)?
    .with_restart_policy(RestartPolicy::Permanent);

let worker = parent_handle
    .supervise_with::<ChildState>(&runtime, config, |actor| {
        // The blueprint. This runs on every start, including every restart.
        actor.mutate_on::<Task>(|actor, ctx| {
            actor.model.done += 1;
            Reply::ready()
        });
    })
    .await?;
```

The blueprint is a closure, not a one-off setup step: it is what the framework replays to build the replacement. Anything the child needs in order to work has to be established there.

`supervise_with` returns a `SupervisedChild`, not an `ActorHandle`. That distinction matters, and the next section explains why.

{% callout type="note" title="From inside a handler, use supervise_deferred" %}
`supervise_with` awaits the child's start, so calling it from a `mutate_on` handler would stall the supervisor's own message loop. Inside a handler use `supervise_deferred`, which records the child and queues its start for the loop's next turn:

```rust
parent.mutate_on::<HireWorker>(move |actor, _ctx| {
    let config = ActorConfig::for_supervised_child("worker", actor.handle().clone(), None)
        .expect("a name plus a live parent is a valid child configuration");
    let child = actor.supervise_deferred(config, blueprint.clone());
    Reply::ready()
});
```

It returns the `SupervisedChild` synchronously, so the handler can keep it without waiting.
{% /callout %}

---

## SupervisedChild: a reference that survives restarts

An `ActorHandle` names **one incarnation**. When a child is restarted, handles to the old one go stale: sends land in a mailbox nobody is reading.

A `SupervisedChild` reads a status channel its supervisor publishes to, so it always describes the incarnation that is actually running:

```rust
// The handle for the incarnation running right now.
let handle = worker.current()?;
handle.send(Task).await;

// Block until a child is up, e.g. after registering it.
let handle = worker.wait_running().await?;

// The published status: state, restart generation, last termination reason.
let status = worker.status();
assert_eq!(status.state(), SupervisionState::Running);

// Wait for a specific incarnation, which is how you observe a restart.
let restarted = worker.wait_generation(RestartGeneration::FIRST.next()).await?;
```

**Store the `SupervisedChild`, and call `current()` at the point of use.** Storing an `ActorHandle` for a supervised child is the mistake this type exists to prevent.

---

## What happens when a child fails

1. **The child stops** processing messages.
2. **The supervisor records the termination** and consults the child's `RestartPolicy` and its own `SupervisionStrategy`.
3. **If a restart is warranted**, the child is rebuilt from its blueprint after an exponential backoff, keeping its identifier. Its IPC names follow it.
4. **If the child exhausts its restart allowance**, the supervisor gives up and applies its `Escalation` setting.
5. **Your `ChildTerminated` handler still runs**, if you have one. The framework's bookkeeping is additive and runs first, so your handler sees a settled registry rather than a half-updated one.

The backoff is a timer, not a sleep: a supervisor keeps taking messages, including its own `Terminate`, while a child is backing off.

{% callout type="note" title="Panics and the catch-handler-panics feature" %}
How a handler panic is reported depends on the `catch-handler-panics` feature:

- **Enabled (default)**: the panic is caught and logged and the actor keeps running. It does not terminate, so nothing is restarted.
- **Disabled** (`default-features = false`): a handler panic terminates the actor and reports `TerminationReason::Panic` carrying the panic message, so a `Transient` child restarts from it.
{% /callout %}

---

## Restart policies

A restart policy is the **child's** setting. It decides whether that child should come back.

### Permanent (default)

Always restarted, including from a normal termination.

```rust
ActorConfig::for_supervised_child("worker", parent_handle.clone(), None)?
    .with_restart_policy(RestartPolicy::Permanent)
```

**Use for**: critical services that must always be running.

### Temporary

Never restarted.

```rust
.with_restart_policy(RestartPolicy::Temporary)
```

**Use for**: one-time operations, or when the caller handles failures explicitly.

### Transient

Restarted only on abnormal termination (a panic with `catch-handler-panics` disabled, or an unexpectedly closed inbox), not on a normal stop.

```rust
.with_restart_policy(RestartPolicy::Transient)
```

**Use for**: workers that may complete normally but should come back from an unexpected failure.

---

## Supervision strategies

A strategy is the **supervisor's** setting. It decides *which* children are restarted when one of them fails.

```rust
let config = ActorConfig::new(Ern::with_root("pipeline")?, None)
    .with_supervision_strategy(SupervisionStrategy::RestForOne);
```

### OneForOne (default)

Restart only the failed child. Other children keep running.

**Use when**: children are independent.

### OneForAll

Stop every child, then bring them all back, so the group starts from a consistent state.

**Use when**: children are interdependent, and one failing could leave the others holding stale assumptions.

### RestForOne

Stop and restart the failed child and every child started after it, preserving start order.

**Use when**: children have sequential dependencies.

### How a group restart is carried out

For `OneForAll` and `RestForOne`:

- Siblings are stopped in **reverse start order**, each fully down before the one before it is asked, and the whole group is down before any of it comes back.
- Rebuilds are **requested** in start order, but not awaited in that order: each start runs on its own task so that a child's `before_start` cannot stall the supervisor. **A child that cannot come up until a sibling is ready must wait for that sibling** rather than assume start order has done it.
- **One backoff is charged for the whole group**, against the child that failed. A sibling stopped and rebuilt by a group restart it did not cause spends none of its own restart allowance.
- A sibling the supervisor holds no blueprint for is still **stopped**, and never comes back. That is deliberate: leaving one child running against a freshly restarted set is exactly the inconsistent state the strategy exists to prevent. In practice this only reaches children adopted through the older `supervise()` path.

---

## Restart limits and escalation

A child that fails immediately on every restart would otherwise restart forever. The **restart limiter** bounds that:

```rust
config.with_restart_limiter(RestartLimiterConfig {
    max_restarts: 3,
    window_secs: 30,
    ..Default::default()
})
```

The defaults are five restarts in a 60-second window, backing off from 100 ms to a 30-second ceiling, doubling each time. Set `enabled: false` to allow restarts without a limit.

A child's own limiter wins; a child that sets none inherits its supervisor's. Each child is held to a limiter of its own, so one child failing repeatedly cannot consume a sibling's allowance.

When the allowance runs out, the supervisor stops trying and applies its `Escalation`:

```rust
let config = ActorConfig::new(Ern::with_root("supervisor")?, None)
    .with_escalation(Escalation::StopSupervisor);
```

- **`Escalation::NotifyParent`** (the default) logs the failure, sends `SupervisionEscalated` to the supervisor's own parent if it has one, leaves the child stopped, and keeps the supervisor running. A supervisor at the top of a tree has nobody to tell, which is not a failure.
- **`Escalation::StopSupervisor`** stops the supervisor itself, cascading to its remaining children. This is the Erlang/OTP behaviour. Its parent learns through the ordinary `ChildTerminated`, so the failure is not reported twice.

Either way the child reaches a terminal state and publishes `SupervisionState::Escalated`, so `wait_running()` returns an error instead of waiting forever.

Like `with_supervision_strategy`, escalation is the **supervisor's** setting, and it only applies to children the supervisor holds a blueprint for.

---

## Observing supervision from elsewhere

The broker publishes `ChildSupervised`, `ChildRestarted` and `SupervisionEscalated`, so an unrelated actor can watch a supervision tree without the supervisor knowing about it:

```rust
monitor.handle().subscribe::<ChildRestarted>().await;

monitor.mutate_on::<ChildRestarted>(|actor, ctx| {
    let event = ctx.message();
    tracing::warn!(
        child = %event.child,
        generation = ?event.generation,
        reason = ?event.reason,
        "child restarted"
    );
    Reply::ready()
});
```

This is the right place for metrics and alerting. Putting them in the supervisor couples recovery to reporting.

---

## Depth limit

A supervision chain is limited to `MAX_SUPERVISION_DEPTH` (10) levels. `for_supervised_child` and `create_child` check depth before building the identifier, so exceeding it names the child that was refused rather than surfacing a generic identifier error.

---

## Giving a child back

Two methods retire a supervisor's record of a child, and they differ in what happens to the actor:

| | Stops the child | Keeps IPC names | Returns |
|---|---|---|---|
| `unsupervise(&child_ern)` | **Yes** | No, they are dropped | `Result<(), SupervisionError>` |
| `release(&child_ern)` | No, it keeps running | Yes | `Result<Option<ActorHandle>, SupervisionError>` |

**`unsupervise` stops the child.** If you want "stop supervising this, but keep it serving", use `release`, which hands the still-running child back so you can keep talking to it.

{% callout type="warning" title="Behaviour change in 9.0.0" %}
`unsupervise` previously left the child running, contradicting its own documentation. It now stops it, with no change to its signature, so this is a silent behaviour change. If you relied on the old behaviour, switch to `release`.
{% /callout %}

---

## When NOT to rely on restarts

{% callout type="warning" title="Persist critical state" %}
Never rely on actor memory for data that must survive a failure. Use:
- Database writes for durability
- Event sourcing for recovery
- External state stores

A restarted actor runs its blueprint again and starts from a fresh model. In-memory state is lost.
{% /callout %}

---

## Best practices

### Return errors, don't panic

```rust
builder.mutate_on::<ProcessOrder>(|actor, envelope| {
    match process(envelope.message()) {
        Ok(_) => Reply::ready(),
        Err(e) => {
            tracing::error!("Order failed: {}", e);
            Reply::ready()
        }
    }
});
```

A restart is for failures the actor cannot recover from in place. Expected failures belong in the handler.

### Design for restart

Assume your actor might restart at any time. Keep minimal state, and restore from external sources in the blueprint.

### Match strategy to dependencies

| Pattern | Strategy | Policy |
|---------|----------|--------|
| Independent workers | OneForOne | Permanent |
| Interdependent services | OneForAll | Permanent |
| Pipeline stages | RestForOne | Permanent |
| One-time tasks | OneForOne | Temporary |
| Optional services | OneForOne | Transient |

---

## Summary

- Register children with `supervise_with`, or `supervise_deferred` from inside a handler. Both take a **blueprint** that is replayed on every restart.
- **The framework restarts them**: rebuilt from the blueprint after a backoff, keeping their identifier and IPC names.
- Hold a **`SupervisedChild`**, not an `ActorHandle`. Handles go stale across a restart.
- **Restart policies** are the child's setting: whether it comes back (Permanent, Temporary, Transient).
- **Supervision strategies** are the supervisor's: which children come back (OneForOne, OneForAll, RestForOne).
- A **restart limiter** bounds retries; **escalation** decides what happens when the allowance runs out.
- `unsupervise` stops the child; `release` hands it back still running.
- Children stop when their supervisor stops, reporting `TerminationReason::ParentShutdown`.
- Critical state belongs outside the actor.

---

## Continue Learning

You now understand the core concepts of Acton:
- **Actors** as independent workers
- **Messages and Handlers** with type-safe routing
- **The Actor System** for management
- **Supervision** for fault tolerance

For termination reasons, custom recovery on top of the restart engine, and supervision tree patterns, see [Custom Supervision](/docs/advanced/custom-supervision).

Continue to [Building Apps](/docs/building-apps/parent-child-actors) for practical patterns.
