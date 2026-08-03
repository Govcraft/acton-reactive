# Migrating from 8.x to 9.0

## Start here: the changes no compiler will catch

These compile fine and behave differently. They are the reason a "just bump the
version" upgrade goes wrong.

**1. `unsupervise` now stops the child.** In 8.x it only detached. Code that
used it to hand a still-running actor to someone else now kills it. Use
`release` to detach and keep the child alive; it returns
`Result<Option<ActorHandle>>`.

**2. Supervision actually restarts things now.** In 8.x
`with_supervision_strategy` and `with_restart_limiter` recorded intent that
nothing read, so many codebases hand-rolled a restart loop on top. In 9.0 the
engine reads them. A child migrated to `supervise_with` plus a surviving
hand-rolled restart comes back **twice**.

The firewall: children adopted through the legacy `supervise()` have no
blueprint and are never restarted by the engine, so nothing already shipped
changes behaviour on its own. That protection ends the moment you migrate a
child — at which point the old restart logic has to go.

**3. Cascading shutdown reaches further.** It now includes children supervised
via a handle clone taken after start, which 8.x missed.

**4. A graceful stop drains its backlog again.** Messages already queued are
processed before the actor stops, rather than being dropped.

## Changes the compiler will catch

**`ActorConfig::new` takes two arguments**, not three:

```rust
// 8.x
ActorConfig::new(id, Some(parent), broker)
// 9.x
ActorConfig::new(id, broker)                                  // Ern, Option<BrokerRef>
ActorConfig::for_supervised_child(name, parent, broker)?      // for children
```

`for_supervised_child` also enforces `MAX_SUPERVISION_DEPTH`, returning an
error rather than building an over-deep identifier.

**`ipc_expose` returns a `Result`.** Handle it rather than discarding it.

## New in 9.0, and worth adopting

**`ask` and `Request`.** Request/response without a client actor and without a
response handler:

```rust
impl Request for GetCount { type Response = Count; }
let count = handle.ask(GetCount).await?;
```

This is the change with the widest blast radius on code *quality*, because it
removes the main reason tests reached for `sleep`. See `testing.md`.

Do not let it become RPC, though: `ask` belongs at the system's boundary. Never
call it from inside a `mutate_on` handler — mutable handlers are awaited inline
on the message loop, so the actor cannot process the message that would answer.

**`FlushBroadcasts`.** A broadcast has no reply path, so previously there was no
way to know a fan-out had landed. `broker.ask(FlushBroadcasts).await?` answers
with `BroadcastsFlushed` once every subscriber's inbox has the message.

**`SupervisedChild`.** Replaces holding an `ActorHandle` for a child. A handle
names one incarnation and goes stale across a restart silently; `current()`
always resolves to the live one.

**The restart engine.** `supervise_with` / `supervise_deferred`, restart
policies, supervision strategies, the restart limiter, escalation. See
`supervision.md`.

## Suggested order

1. Bump the version and fix what fails to compile (`ActorConfig::new`,
   `ipc_expose`).
2. Grep for `unsupervise` and decide, per call site, whether you meant
   `release`.
3. Grep for hand-rolled restart logic and leave it alone for now — it still
   works, because legacy `supervise()` children are not restarted by the
   engine.
4. Migrate children to `supervise_with` **one at a time**, deleting that
   child's hand-rolled restart logic in the same change.
5. Replace test sleeps with `ask` and `FlushBroadcasts` barriers. Verify each
   converted test by breaking the behaviour it names and confirming it fails.
