# Supervision and restarts (9.x)

**This changed completely in 9.0.** In 8.x Acton did not restart actors; the
strategy and limiter settings recorded intent and nothing read them. In 9.0
there is a real restart engine. If you carry over an 8.x habit of hand-rolling
restarts, the child comes back twice.

## The core idea: a blueprint, not a handle

To restart an actor, the framework needs to be able to build it again. So you
do not hand it a started actor; you hand it a **closure it replays** on every
restart.

```rust
let child = parent_handle
    .supervise_with::<WorkerState>(&runtime, config, |worker| {
        worker.mutate_on::<Job>(|actor, ctx| { /* ... */ Reply::ready() });
    })
    .await?;
```

Everything the child needs must be established inside that closure, because
that closure is the entire definition of the child. State set up outside it
exists only for the first incarnation.

```rust
pub async fn supervise_with<S: Default + Send + Debug + 'static>(
    &self,
    runtime: &ActorRuntime,
    config: ActorConfig,
    configure: impl Fn(&mut ManagedActor<Idle, S>) + Send + Sync + 'static,
) -> Result<SupervisedChild, SupervisionError>
```

**From inside a handler, use `supervise_deferred` instead.** `supervise_with`
awaits registration on the supervisor's own message loop, so calling it from a
`mutate_on` handler deadlocks for the same reason `ask` does.

```rust
pub fn supervise_deferred<C>(
    &mut self,
    config: ActorConfig,
    configure: impl Fn(&mut ManagedActor<Idle, C>) + Send + Sync + 'static,
) -> Result<SupervisedChild, SupervisionError>
```

It registers the child and returns immediately; the child starts afterwards.
`wait_running()` returns once it is actually up.

## Hold a `SupervisedChild`, not an `ActorHandle`

An `ActorHandle` names **one incarnation**. After a restart it points at a dead
actor and says nothing about it — sends just vanish. This is the single easiest
way to write a bug that only appears under failure.

```rust
pub fn current(&self) -> Option<ActorHandle>          // always the live one
pub fn status(&self) -> SupervisionStatus
pub async fn wait_running(&mut self) -> Result<ActorHandle, SupervisionError>
pub async fn wait_generation(&mut self, generation: RestartGeneration) -> ...
pub async fn wait_for(&mut self, ...) -> ...
```

`SupervisedChild` works as a field in an `#[acton_actor]` struct, including as
`Vec<SupervisedChild>` for a pool.

## Restart policy: should this child come back?

Set per child via `config.with_restart_policy(..)`.

| `RestartPolicy` | Comes back after |
|---|---|
| `Permanent` (default) | any termination, including a clean stop |
| `Temporary` | never |
| `Transient` | abnormal termination only (panic, error) |

## Supervision strategy: which children are affected?

Set on the supervisor via `config.with_supervision_strategy(..)`.

| `SupervisionStrategy` | On a child's termination |
|---|---|
| `OneForOne` (default) | restart just that child |
| `OneForAll` | stop and restart every child |
| `RestForOne` | restart that child and everything started after it |
| `NoRestart` | leave it down |
| `Escalate` | hand the failure to the supervisor's own supervisor |

`RestForOne` is the right choice for a pipeline where later stages depend on
earlier ones.

Group restarts (`OneForAll`, `RestForOne`) stop children in reverse start
order, apply **one** backoff for the whole group rather than per child, and
stop-and-leave-down any sibling that has no blueprint — a sibling adopted
through the legacy `supervise()` cannot be rebuilt.

## Restart limiter: stopping a crash loop

Without a bound, a child that fails on startup restarts forever and burns a
core.

```rust
let config = ActorConfig::new_with_name("worker")?
    .with_restart_limiter(RestartLimiterConfig {
        max_restarts: 5,
        ..Default::default()
    });
```

Build it with `..Default::default()`; the struct has an `enabled: bool`
alongside the counts and a literal that omits it will not compile. When the
limit is exceeded the child stays down and `RestartLimitExceeded` is emitted.

## Escalation

`Escalation::NotifyParent` (tell the grandparent and keep going) or
`Escalation::StopSupervisor` (this supervisor cannot do its job without the
child, so take it down too). Supervision nests to `MAX_SUPERVISION_DEPTH`.

## Observing, not driving

The supervisor emits these; subscribe to them for logging, metrics, or health
checks. Do **not** use them to recreate the child yourself — that is the
engine's job, and doing both restarts it twice.

| Event | Fields |
|---|---|
| `ChildSupervised` | registration of a new child |
| `ChildRestarted` | `supervisor`, `child`, `generation`, `reason` |
| `ChildTerminated` | includes `TerminationReason` |
| `SupervisionEscalated` | a failure moved up a level |
| `RestartLimitExceeded` | the limiter gave up |

`TerminationReason::ParentShutdown` means the whole tree is going down. Guard
on it, or your "child died, react!" logic fires during every clean shutdown.

Recovery logic belongs **inside the blueprint**: the closure runs on every
restart, so that is where you reload a checkpoint or re-establish a connection.

## `unsupervise` vs `release`

A silent 9.0 behaviour change.

| Call | Effect |
|---|---|
| `unsupervise(&child_id)` | detaches **and stops** the child |
| `release(&child_id)` | detaches and leaves it running; returns `Result<Option<ActorHandle>>` |

In 8.x, `unsupervise` only detached. Code that used it to hand off a still-live
actor now kills it, and nothing warns you.

## The legacy path

`supervise(child)` still exists and adopts an already-started actor. It has no
blueprint, so **the engine will never restart it**. That is a useful firewall
while migrating: existing code keeps its old behaviour. But the moment you move
a child to `supervise_with`, delete any hand-rolled restart logic around it, or
you will get two.
