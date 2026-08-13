# API reference (acton-reactive 9.x)

Exact surface, verified against source. Everything here comes from
`acton_reactive::prelude::*` unless noted.

## Contents

- [Setting up](#setting-up)
- [Building an actor](#building-an-actor)
- [Handlers](#handlers)
- [Lifecycle hooks](#lifecycle-hooks)
- [Sending and asking](#sending-and-asking)
- [Sending later](#sending-later)
- [ActorConfig](#actorconfig)
- [What the prelude exports](#what-the-prelude-exports)

## Setting up

```toml
[dependencies]
acton-reactive = "9"
```

Features: `catch-handler-panics` (default, wraps handler dispatch in
`catch_unwind` so a panicking handler cannot kill the actor task), `ipc`,
`ipc-messagepack`. Tokio is re-exported through the prelude, so you do not need
to depend on it directly.

```rust
let mut app: ActorRuntime = ActonApp::launch_async().await;
app.shutdown_all().await?;      // anyhow::Result<()>
```

`#[acton_main]` wraps `main` the way `#[tokio::main]` does.

## Building an actor

```rust
#[acton_actor]              // requires Default + Debug; both are derived
struct MyState { field: u32 }

#[acton_message]            // works on structs and enums
struct MyMessage { data: String }
```

Actor state must implement `Default`, because the actor is constructed with its
default state before handlers are registered. When a field's type has no
`Default` of its own, use `#[acton_actor(no_default)]` and write the impl
yourself:

```rust
#[acton_actor(no_default)]
struct Printer { out: Stdout }

impl Default for Printer {
    fn default() -> Self { Self { out: stdout() } }
}
```

For a field you can only supply *after* construction — a `watch::Sender` used
for egress, a handle to a peer actor — prefer `Option<T>` and wire it in on the
builder (`builder.model.field = Some(..)`) rather than inventing a placeholder.

Three ways to construct, all returning `ManagedActor<Idle, State>`:

```rust
app.new_actor::<MyState>()
app.new_actor_with_name::<MyState>("worker".to_string())
app.new_actor_with_config::<MyState>(config)
```

Configure handlers on the `Idle` builder, then consume it:

```rust
let handle: ActorHandle = builder.start().await;
```

Subscriptions must be registered **before** `start()`:

```rust
builder.handle().subscribe::<SomeBroadcast>().await;
```

## Handlers

```rust
pub fn mutate_on<M>(
    &mut self,
    f: impl for<'a> Fn(&'a mut ManagedActor<Started, State>, &'a mut MessageContext<M>) -> FutureBox
        + Send + Sync + 'static,
) -> &mut Self
where M: ActonMessage + Clone + Send + Sync + 'static
```

In practice you write the closure and let inference do the rest:
`|actor, ctx| { ...; Reply::ready() }`. The companions are the same shape:

| Method | Access | Returns |
|---|---|---|
| `mutate_on<M>` | `&mut ManagedActor` | `FutureBox` |
| `act_on<M>` | `&ManagedActor` | `FutureBox` |
| `mutate_on_sync<M>` | `&mut ManagedActor` | nothing; no `.await` allowed |
| `act_on_sync<M>` | `&ManagedActor` | nothing |
| `try_mutate_on<M, T, E>` | `&mut ManagedActor` | `Result<T, E>` |
| `try_act_on<M, T, E>` | `&ManagedActor` | `Result<T, E>` |
| `on_error<M, E>` | `&mut ManagedActor` | centralised handler for the `try_*` pair |

Inside a handler: `actor.model` is your state, `ctx.message()` is the message,
`ctx.reply_envelope()` is how you answer the sender.

Return `Reply::ready()` for synchronous completion, or
`Reply::pending(async move { ... })` for async work. Clone handles and brokers
*before* the async block; the closure is `Fn`, so it cannot move captured
values out.

**The ordering guarantee, precisely** (`actor/managed_actor/started.rs`):

- `mutate_on` dispatch is awaited inline in the message loop. The pending
  future completes before the actor takes its next message.
- `act_on` futures are pushed into a `FuturesUnordered` and drained
  concurrently with subsequent messages, up to a high-water mark (default 100).

So a reply from a `mutate_on` handler proves its async work finished; a reply
from an `act_on` handler proves only that the work started.

## Lifecycle hooks

Registered on the builder, each taking an async closure:

```rust
builder.before_start(|actor| async move { ... });
builder.after_start(|actor| async move { ... });
builder.before_stop(|actor| async move { ... });
builder.after_stop(|actor| async move { ... });
```

**`after_start` does not gate the mailbox.** A future returned from it runs
alongside the message loop, so the actor can answer messages while
initialisation is still running. If callers must not see an uninitialised
actor, hold their reply envelope in `model` and answer once you are ready,
rather than assuming "started" means "ready".

## Sending and asking

From `ActorHandleInterface`:

```rust
fn send(&self, message: impl ActonMessage) -> impl Future<Output = ()>
fn ask<R: Request>(&self, request: R) -> impl Future<Output = Result<R::Response, AskError>>
fn ask_with_timeout<R: Request>(&self, request: R, timeout: Duration) -> ...
fn stop(&self) -> impl Future<Output = anyhow::Result<()>>
fn id(&self) -> Ern
fn name(&self) -> String
fn children(&self) -> &DashMap<String, ActorHandle>
fn find_child(&self, id: &Ern) -> Option<ActorHandle>
fn reply_address(&self) -> MessageAddress
fn create_envelope(&self, recipient: Option<MessageAddress>) -> OutboundEnvelope
```

`ask` requires the message to declare its answer:

```rust
impl Request for GetCount {
    type Response = Count;      // must be ActonMessage + Clone
}
```

One request type has exactly one response type, on purpose: a mismatched pair
becomes a compile error rather than a runtime surprise. If the same payload
needs two different answers in two contexts, define two request types.

`ask` resolves on the **first** reply, with `DEFAULT_ASK_TIMEOUT` (30s) unless
you use `ask_with_timeout`. The deadline covers the whole exchange including
delivery, because a full inbox makes delivery itself wait.

`AskError`:

| Variant | Means |
|---|---|
| `Undeliverable` | the actor is gone or stopping |
| `NoReply` | the handler returned without replying |
| `TimedOut` | deadline elapsed |
| `Cancelled` | the actor shut down mid-exchange |
| `UnexpectedReply` | handler sent a type other than `R::Response` |

`NoReply` exists so a handler that forgets to reply produces an error rather
than a caller that waits forever.

**Never `ask` from inside a `mutate_on` handler.** Mutable handlers are awaited
inline on the message loop, so the actor cannot process the message that would
answer. Asking your own handle can never succeed; asking another actor
deadlocks the moment it asks back. Send instead, or move the exchange into a
`Reply::pending` future, or ask from outside the actor.

`send_sync` exists but spawns a blocking task and builds a new runtime. Avoid
it.

## Sending later

Inherent on `ActorHandle`, new in 9.1. All three return immediately, having
armed a timer.

```rust
fn send_after(&self, message: impl ActonMessage, delay: Duration) -> ScheduledSend
fn send_at(&self, message: impl ActonMessage, deadline: Instant) -> ScheduledSend
fn send_every(&self, message: impl ActonMessage, interval: Interval, cadence: Cadence)
    -> ScheduledSend
fn with_clock(&self, clock: Arc<dyn Clock>) -> ActorHandle
fn clock(&self) -> Arc<dyn Clock>
```

Reach for these instead of `tokio::spawn` plus `tokio::time::sleep`. That task
belongs to nobody: it survives shutdown, cannot be cancelled, and can only be
tested by sleeping. A `ScheduledSend` ends when the actor does, reports what
became of it, and runs on an injectable clock.

`ScheduledSend`:

```rust
fn cancel(&self)                                    // idempotent
fn due_at(&self) -> Option<FireAt>                  // next deadline, None once settled
fn deliveries(&self) -> u64
fn settled(&self) -> Option<ScheduledSendOutcome>   // non-blocking peek
fn is_settled(&self) -> bool
async fn outcome(&self) -> ScheduledSendOutcome     // barrier
async fn wait_for_deliveries(&self, at_least: u64) -> u64
```

Dropping it does **not** cancel the schedule. `outcome` and
`wait_for_deliveries` always resolve — they report `Abandoned` rather than
hanging if the timer is torn down — which is what makes them usable as test
barriers in place of a sleep.

`ScheduledSendOutcome`:

| Variant | Means |
|---|---|
| `Delivered` | reached the inbox; terminal for one-shots only |
| `Cancelled` | `cancel()` beat the deadline |
| `Abandoned` | the actor's inbox closed first |
| `Undeliverable { detail }` | came due, but the send failed |

### Repeating sends

The first tick lands **one whole interval after the call**, never immediately
(Erlang's `timer:send_interval` convention). `send` first if you also want one
now.

`Interval` cannot be zero — a zero interval is a spin loop, not a schedule.
Build it with `Interval::new(Duration)`, `from_secs`, `from_millis` (all
returning `Option`) or `Duration::try_into()`, which yields `ZeroInterval`.

`Cadence` picks how the gap is measured:

| Variant | Next deadline | Use for |
|---|---|---|
| `FixedRate` | `armed_at + n * interval` — a grid that slow sends do not shift | ticks that mark *times* |
| `FixedDelay` | `now_after_send + interval` | ticks that mark *gaps* |

**Missed ticks are skipped and coalesced.** If a schedule falls behind, the
deadlines that went by are stepped over rather than fired as a burst; at most
one tick is ever pending. So advancing a clock by three intervals delivers
**one** message, not three, under either cadence.

A repeating schedule never settles on `Delivered`. It runs until cancelled, the
actor stops, or a delivery fails — including when the returned handle is
dropped, which is worth being deliberate about.

### Testing scheduled sends

`tokio::time::pause` does not work here: it needs the `test-util` feature and a
current-thread runtime, while `#[acton_test]` builds a multi-thread one. Inject
a `ManualClock` instead.

```rust
let clock = Arc::new(ManualClock::new());
let scheduled = handle.with_clock(clock.clone()).send_after(Tick, Duration::from_secs(60));

assert_eq!(clock.armed(), 1);          // armed before `send_after` returned
clock.advance(Duration::from_secs(60));
assert_eq!(scheduled.outcome().await, ScheduledSendOutcome::Delivered);
```

`Clock` is a two-method trait: `now()` and `timer(deadline) -> Timer`. `timer`
is deliberately *not* `async` — it registers the deadline before returning, so
a test that arms a schedule and immediately advances the clock is not racing
the timer task's first poll. `ManualClock::armed()` is exact for the same
reason. `ManualClock` also has `starting_at`, `advance`, and `advance_to`,
which never moves time backwards.

## ActorConfig

```rust
ActorConfig::new(id: Ern, broker: Option<BrokerRef>) -> Self
ActorConfig::new_with_name(name: impl Into<String>) -> anyhow::Result<Self>
ActorConfig::for_supervised_child(name, parent: ParentRef, broker: Option<BrokerRef>) -> anyhow::Result<Self>
```

Note `new` takes **two** arguments in 9.x. The 8.x three-argument form
`ActorConfig::new(id, Some(parent), broker)` no longer exists.

`for_supervised_child` enforces `MAX_SUPERVISION_DEPTH` and returns an error
rather than building an over-deep identifier.

Const builders, all chainable:

```rust
.with_inbox_capacity(usize)
.with_restart_policy(RestartPolicy)
.with_supervision_strategy(SupervisionStrategy)
.with_escalation(Escalation)
.with_restart_limiter(RestartLimiterConfig)
```

`RestartLimiterConfig` has an `enabled: bool` field alongside the counts, so
build it with `..Default::default()` rather than listing fields.

## What the prelude exports

Macros from `acton_macro`, all of `acton_ern`, `async_trait`, and `tokio`.

**Core:** `ActonApp`, `ActorRuntime`, `ActorHandle`, `Broker`, `BrokerRef`,
`ParentRef`, `Reply`, `AskError`, `DEFAULT_ASK_TIMEOUT`, `ScheduledSend`,
`ScheduledSendOutcome`, `Cadence`, `Interval`, `ZeroInterval`, `FireAt`, `Clock`,
`SystemClock`, `ManualClock`, `Timer`.

**Actors and supervision:** `ActorConfig`, `ManagedActor`, `Idle`, `Started`,
`SupervisedChild`, `SupervisionState`, `SupervisionStatus`,
`SupervisionStrategy`, `SupervisionDecision`, `SupervisionError`,
`SupervisionEscalated`, `RestartPolicy`, `RestartLimiter`,
`RestartLimiterConfig`, `RestartStats`, `RestartGeneration`,
`RestartLimitExceeded`, `BackoffDelay`, `ChildIndex`, `ChildRestarted`,
`ChildSupervised`, `Escalation`, `TerminationReason`, `MAX_SUPERVISION_DEPTH`.

**Messages:** `ActonMessage`, `MessageAddress`, `OutboundEnvelope`,
`SystemSignal`, `BrokerRequest`, `BrokerRequestEnvelope`, `ChildTerminated`,
`FlushBroadcasts`, `BroadcastsFlushed`.

**Traits:** `ActorHandleInterface`, `Broadcaster`, `Request`, `Subscribable`,
`Subscriber`.

**Under `ipc`:** `IpcClient`, `IpcClientConfig`, `IpcConfig`, `IpcEnvelope`,
`IpcError`, `IpcListenerHandle`, `IpcListenerStats`, `IpcResponse`,
`IpcTypeRegistry`, `RemoteActorRef`, `ShutdownResult`, `RemoteRequest`.
