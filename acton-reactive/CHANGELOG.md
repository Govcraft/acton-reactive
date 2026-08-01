# Changelog

All notable changes to `acton-reactive` are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **The framework now restarts supervised children.** A child registered with
  `ActorHandle::supervise_with` or `ManagedActor::supervise_deferred` that
  terminates in a way its `RestartPolicy` warrants a restart from is rebuilt
  from its blueprint, after an exponential backoff, keeping its identifier.
  All three supervision strategies are carried out; see the group-restart entry
  below for `OneForAll` and `RestForOne`.

  **This cannot restart a child in any existing program.** A supervisor can only
  rebuild a child it holds a blueprint for, and blueprints reach the registry
  only through `supervise_with` and `supervise_deferred`, neither of which has
  appeared in a released version. Children adopted through `supervise()` have no
  blueprint, so the decision layer leaves them down — before their restart
  allowance is even consulted — exactly as today. **No program written against a
  released version can have a child restarted twice**, including one that
  hand-rolls restarts from its own `ChildTerminated` handler.

  That guarantee covers **restarts, and the IPC name sweeps that follow a child
  reaching a terminal state or being cascaded down with its supervisor**. It
  does *not* extend to `ActorHandle::unsupervise`, which drops the IPC names of
  any child it stops — see its own entry below.

  That firewall stops applying the moment you migrate a child to
  `supervise_with` or `supervise_deferred`. **When you do, delete your
  hand-rolled restart for that child**, or it will come back twice: once from
  your handler and once from the framework.

  A `ChildTerminated` handler you already have keeps running either way. The
  framework's bookkeeping is additive and does not suppress your handler; it
  only runs first, so a handler that inspects its supervisor sees a settled
  registry rather than a half-updated one.

- **`SupervisionStrategy::OneForAll` and `RestForOne` are now carried out.**
  Previously they were planned correctly and then ignored: the supervisor
  restarted only the child that failed and logged that the rest of the plan was
  not performed.

  A group restart stops the siblings the strategy names in **reverse start
  order**, each one fully down before the one before it is asked, and the whole
  group is down before any of it comes back. The rebuilds are then *requested*
  in start order — but not awaited in that order, because each start runs on its
  own task so that a child's `before_start` cannot stop the supervisor taking
  messages. A child that cannot come up until a sibling is ready should wait for
  it rather than assume start order has done that for it.

  **One backoff is charged for the whole group**, against the child that failed.
  A sibling stopped and rebuilt by a group restart it did not cause spends none
  of its own restart allowance.

  A sibling the supervisor holds no blueprint for is still **stopped** and
  simply never comes back. That is deliberate: the point of `OneForAll` is that
  the children are interdependent, so leaving one running against a freshly
  restarted set would expose exactly the inconsistent state the strategy exists
  to prevent. In practice this only reaches children adopted through the legacy
  `supervise()` path.

  A supervisor that begins shutting down mid-group-restart abandons the group
  rather than driving it, and settles every child left part-way through so that
  nothing waits on an incarnation that will never be built.

- **Fixed: a group plan listed the child that failed among the children to
  stop.** The registry is consulted before the supervisor records the
  termination, so the child that just died still read as running. Harmless while
  group plans were never performed; now it would have sent a stop to a dead
  mailbox and waited for a termination notice that had already been delivered,
  leaving the group incomplete and the failed child down for good.

- **A child that exhausts its restart allowance now reaches a terminal state.**
  Its supervisor gives up, publishes `SupervisionState::Escalated`, and records
  the reason, so `wait_running()` returns instead of waiting forever.

### Added

- **`ActorConfig::with_escalation`, which makes `Escalation` reachable.**
  `Escalation` shipped in 8.2.0 public, documented, and exported from the
  prelude, with nothing in the crate reading it and no way to set it. It now
  decides what a supervisor does once restarting a child has stopped working:

  - `Escalation::NotifyParent` (the default) logs the failure, sends a
    `SupervisionEscalated` to the supervisor's own parent if it has one, leaves
    the child stopped, and keeps the supervisor running. A supervisor at the top
    of a tree has nobody to tell, which is not a failure.
  - `Escalation::StopSupervisor` stops the supervisor itself, cascading to its
    remaining children — the Erlang/OTP behaviour. Its parent learns through the
    ordinary `ChildTerminated` every stopping actor sends, so the failure is not
    reported twice.

  Like `with_supervision_strategy`, this is the **supervisor's** setting rather
  than the child's, and it only applies to children the supervisor holds a
  blueprint for: a child adopted through the legacy `supervise()` path is never
  restarted, so it never exhausts an allowance and never escalates.

  Behaviour is unchanged for anyone who does not call it. The default is what
  the engine already did, plus the notification that was missing.

- **A restarted actor stays reachable over IPC.** `ipc_expose` stores a handle
  by value, so before this an actor exposed under a chosen name became
  unreachable from its first restart onward, and silently: sends landed in a
  mailbox with no reader. Its names now follow it across restarts.

  Names are also dropped when a child reaches a terminal state, and when a
  supervisor takes its children down with it. **Those two sweeps are limited to
  children the supervisor holds a blueprint for**, so a child adopted through
  `supervise()` is unaffected by either.

- **BREAKING: `ActorHandle::unsupervise` now stops the child it releases.**

  It previously retired the supervisor's record and left the actor running,
  which contradicted its own documentation — *"Stops a supervised child and
  removes it from supervision"* — and contradicted its `pub(crate)` sibling
  `ManagedActor::unsupervise`, which did stop the child. The test covering it
  asserted only that the name was freed, so the doc, the sibling and the test
  name all promised a stop that never happened and nothing could tell.

  **Signatures are unchanged, so this is a silent behaviour change rather than
  a compile error** — which is exactly why it ships in a major rather than
  being smoothed over in a minor. **If you relied on the child surviving,
  switch to [`ActorHandle::release`]** (below), which is that behaviour under a
  name that says so.

  Two consequences follow from the child now being stopped:

  - `unsupervise` does not return until the child really has stopped. The
    caller does the stopping, not the supervisor: awaiting a child's shutdown
    on the supervisor's task would stall its message loop for as long as that
    child took.
  - **It drops the child's IPC names** — whichever way that child was
    registered, including through `supervise()`. A name that still resolves to
    a mailbox nobody is reading is the precise failure this area exists to
    prevent: sends succeed and vanish. So the names go with the actor. If you
    exposed a child for IPC and later `unsupervise` it, external callers are
    now told there is no such actor instead of sending into nothing.

  This second point is **the one place the "no shipped program can observe a
  difference" reasoning does not hold**, because `unsupervise` and
  `supervise()` are both shipped APIs. It is stated here rather than folded
  into the restart firewall above, which genuinely does hold.

  [`ActorHandle::release`]: #added

- **Cascading shutdown now reaches every supervised child.** A supervisor keeps
  its own record of the children it supervises, and stops all of them when it
  stops.

  Previously, a child supervised through a **handle clone obtained after the
  parent started** was never stopped when the parent stopped. `ActorHandle`
  stores its children in a map that is deep-copied on clone, so such a child was
  invisible to the parent's own task and simply outlived it.

  **If your program relies on a child supervised that way outliving its parent,
  that child will now be stopped.** Start it as a root actor instead of
  supervising it, if it genuinely should outlive its supervisor.

  There is a second-order consequence worth checking before upgrading, and it
  is narrower than it may look. Children stopped by a cascading shutdown
  terminate with `TerminationReason::Normal`, and `RestartPolicy::Permanent`
  warrants a restart on a normal termination. The framework's own bookkeeping
  suppresses restart decisions during shutdown, but a **hand-rolled
  `ChildTerminated` handler does not**. If you restart children from your own
  handler, check the termination reason, or you may restart children on the way
  down. Children stopped this way now report
  `TerminationReason::ParentShutdown`, which removes the ambiguity for handlers
  that check it.

  This is a hazard in code you already have, not one this release introduces:
  the framework restarts only children registered through APIs that have never
  shipped, so it cannot be competing with your handler over the same child.

- **The IPC listener now tells a refused client why it was refused.** A server
  at its connection limit accepted the socket and then dropped it without a
  word. The client's `connect()` had already succeeded, so the refusal surfaced
  only as `Broken pipe (os error 32)` on the first write — and nothing in that
  points at a connection limit. The listener now writes a typed error before
  closing, and the client reports
  `IpcError::ConnectionLimitReached { limit }`.

  The effective limit is also logged at listener startup, beside the socket
  path, so the ceiling is discoverable before it is reached rather than after.

  **`IpcError` has gained a variant and is now `#[non_exhaustive]`, so a
  downstream `match` that lists every variant will stop compiling.** Add a
  wildcard arm. This is a one-time cost: because the type is now
  `#[non_exhaustive]`, later variants are additive and will not break that
  match again.

  Nothing changes on the wire. `IpcError` is not serialized — a refusal travels
  as an ordinary error response carrying an `error_code` string — so the
  protocol version is unchanged and a client built against 8.x still parses the
  frame.

- **`max_connections` now defaults to 1024, where it previously defaulted to
  100.** One connection per participant, held for that participant's process
  lifetime, is an ordinary topology, and 100 was low enough to be reached in
  normal use. The new figure is sized from the measured per-connection buffer
  reservation of roughly 20 KiB, so a full listener costs about 20 MiB rather
  than the ~2 MiB the old default implied.

  **If you were relying on the old default as a resource ceiling, set
  `limits.max_connections` explicitly.** Nothing else restores 100.

- **`IpcConfig::load()` now prefers a per-application configuration file.** It
  looks for `$XDG_CONFIG_HOME/acton/<app_name>/ipc.toml` first, falls back to
  `$XDG_CONFIG_HOME/acton/ipc.toml`, and logs which of the two it used.
  Previously only the shared path was ever read, while the documentation
  promised the per-application one — so a file placed where the docs said it
  should go produced default settings, with no warning that it had been
  ignored.

  **The shared location still loads, so no action is required and no existing
  configuration stops working.** Move a file to the per-application path only
  if you want that application's settings to stop being shared with every other
  acton IPC server on the machine.

- **`SubscriptionManager::register_connection` takes a third argument**,
  `peer: Option<PeerCredentials>`. **Pass `None` if you do not need the identity
  of the connecting process.**

- **`expose_for_ipc()` now registers the name you chose.** **This is a breaking
  change that costs you nothing to migrate, and it is worth saying why before
  anything else:** the old name contained a `UUIDv7` regenerated on every process
  start, so it was different on every run and no client, config file or script
  could ever have named it. No working program can have depended on the old
  value.

  An actor is now exposed under its own name, and a supervised child under its
  parent's name then its own:

  | Actor | Was | Now |
  |---|---|---|
  | `new_actor_with_name("prices")` | `prices_01kyww2gfb…` | `prices` |
  | child `"alpha"` of `prices` | `prices_01kyww2gfb…` | `prices/alpha` |
  | child `"beta"` of `prices` | `prices_01kyww2gfb…` | `prices/beta` |

  The middle column is not a typo. A supervised child shares its parent's `Ern`
  root and is distinguished only by the part the old derivation discarded, so
  **every child of one parent registered under the same name, and each silently
  replaced the last** — along with the parent itself. Messages addressed to the
  first were delivered to whichever actor registered most recently. That is
  fixed: the retained parts are exactly what tells those actors apart.

  This also makes the documented example true. `expose_for_ipc()` on an actor
  named `prices` really is reachable as `"prices"` now, which is what the docs
  claimed and what every in-tree example had to sidestep by calling
  `ipc_expose` manually.

- **`ActorRuntime::ipc_expose` returns `Result<(), IpcNameInUse>` and no longer
  replaces an existing registration.** **Handle or `expect()` the result at your
  call sites**; that is the whole migration.

  Overwriting silently redirected traffic away from an actor that was already
  serving, and that actor had no way to learn it had been displaced. Refusing
  the second claim confines the problem to the actor that has not started
  serving yet — which is also the one whose caller is positioned to do something
  about it. Release a name with `ipc_hide` if you intend to reuse it.

  `ipc_rebind` still overwrites, deliberately. The two are not inconsistent:
  `ipc_expose` is a caller *claiming* a name, where a second claim is a
  conflict, while `ipc_rebind` is the supervision engine *repointing a name it
  already owns* at a restarted incarnation, where overwriting is the point.

  `expose_for_ipc()` remains infallible and still returns `&mut Self`. A name
  conflict there is reported by logging at `error!` with both actors named; the
  actor still starts, but is not reachable under that name. **If you need to
  handle a conflict in code, call `ipc_expose` and match on the result** —
  making `expose_for_ipc()` fallible would have forced `start()` to return a
  `Result` and broken every actor in every program for a fault confined to IPC.

- **A child built with `create_child` now keeps the name you gave it.** Its
  `Ern` is its parent's with the name appended, `<parent-ern>/<name>`, and the
  same parent and name always produce the same identifier.

  Before this, `create_child` parsed the parent's *display string* back into an
  `Ern` and added the child's `Ern` to the result. Two defects composed there.
  Parsing calls `EntityRoot::new`, which stamps a **fresh `UUIDv7`** on every
  call, so the derivation was neither deterministic nor actually descended from
  the parent. And `Add for Ern` keeps the left root and concatenates parts,
  while `Ern::with_root(name)` puts the name in the *root* with no parts — so
  **the child's name contributed nothing at all**.

  Read together: holding the parsed parent fixed, children named `alpha` and
  `beta` came out **identical**. Siblings differed in practice only because each
  call happened to draw a new random suffix. Sibling collision was avoided by
  accident, not by design — and `ActorHandle: PartialEq` compares `Ern` alone,
  while the supervision registry, the IPC registry, and `unsupervise`/`retire`
  all key on it.

  | Child of `prices` | Was | Now |
  |---|---|---|
  | `create_child("alpha")` | `prices_kywwgfbfehasqebwb` (fresh each call) | `prices_01kyww2gfb…/alpha` |
  | `create_child("beta")` | `prices_kyxtneevfbykdcws` (fresh each call) | `prices_01kyww2gfb…/beta` |

  **Consequence for IPC names:** a `create_child` actor that calls
  `expose_for_ipc()` is now reachable as `prices/alpha` rather than
  `prices_kywwgfbfehasqebwb`. That is the IPC naming change above working
  correctly on a fixed input, not a separate regression — the old name was
  unusable anyway, since it was regenerated on every process start.

- **`ActorConfig::new` no longer takes a parent, and no longer returns a
  `Result`.** It builds root actors only:

  ```rust
  // Was
  ActorConfig::new(id, None, broker)?                          // root
  ActorConfig::new(Ern::with_root(name)?, Some(parent), broker)?  // child

  // Now
  ActorConfig::new(id, broker)                                 // root, infallible
  ActorConfig::for_supervised_child(name, parent, broker)?     // child
  ```

  Downstream code calling the three-argument form will not compile. Migration is
  mechanical: drop the `None` and the `?`/`expect`/`unwrap` for a root; for a
  child, pass the plain name where the `Ern` used to be built and keep the `?`.

  The parent branch was where the defect above lived, so it is deleted rather
  than patched. Taking an `Ern` for what is really a *name* is what let the bug
  hide in plain sight: `Ern::with_root("alpha")` looks like it carries "alpha",
  and it does — in a field `Add` never reads. There is now one way to build a
  child, and it takes a string. The `Result` went with the parent parameter,
  which held its only failure mode.

- **A supervision chain is limited to `MAX_SUPERVISION_DEPTH` (10) levels**, a
  new public constant. `for_supervised_child` and `create_child` check depth
  before building the identifier, so exceeding it reports supervision depth and
  names the child refused, rather than surfacing `acton-ern`'s generic "cannot
  exceed maximum of 10 parts".

  The value is not free to change. `acton-ern` 2 hardcodes the same cap inside
  `Ern::add_part` and exposes no constant, accessor, or `add_part_with_limit` to
  read it from, so **raising this number requires `acton-ern` 3**; raising it
  alone would only move which error you get.
  `a_child_at_the_depth_limit_is_refused_by_name` fails if the two drift apart.

### Undeprecated

- `ActorConfig::with_supervision_strategy` and `ActorConfig::with_restart_limiter`
  no longer carry deprecation notices. They were deprecated because the
  framework never read them and their notices told you to hand-roll a
  `ChildTerminated` handler instead. Both are now read. Leaving the notices in
  place would have had the compiler actively advising users into the
  double-restart described above.

  `with_restart_limiter` is meaningful on a child as well as on a supervisor:
  **a child's own setting wins, and a child that sets none inherits its
  supervisor's.** Each child is held to a limiter of its own, so one child
  failing repeatedly cannot consume a sibling's allowance.

### Added

- `ActorHandle::release`, the counterpart to the corrected
  `unsupervise`: it retires the supervisor's record and hands the child back
  **still running**. This is "stop supervising this, but keep it serving" —
  nothing will restart it and nothing will stop it when its former supervisor
  stops, and its IPC names are left alone because it is still there to answer
  them.

  It returns the released child's handle rather than a bare acknowledgement,
  which is what makes it useful: you need a handle to keep talking to a child
  you have just released. **This is the migration path if you relied on the old
  `unsupervise` leaving the child running.**

- `SupervisedChild`, a reference to a supervised child that survives its
  restarts. An `ActorHandle` names one incarnation and goes stale when the child
  is restarted; a `SupervisedChild` reads a status channel its supervisor
  publishes to, so `current()`, `status()`, `wait_running()` and
  `wait_generation()` always describe the incarnation that is actually running.
- `SupervisionStatus` and `SupervisionState`, the published view of one child.
- `SupervisionError`, the error type for supervision operations.
- `Escalation`, controlling what a supervisor does once a child exhausts its
  restart allowance.
- `ChildSupervised`, `ChildRestarted` and `SupervisionEscalated` broker events,
  so an unrelated actor can observe supervision without the supervisor knowing.
- `RestartGeneration`, `ChildIndex` and `BackoffDelay` value types.
- `IpcError::ConnectionLimitReached { limit }`, reporting the server's
  configured ceiling to a client it refused.
- `IpcClient::rejection_reason()`, the reason the server refused this
  connection, or `None` for a connection it accepted normally.
- `IpcListenerStats::max_connections()` and `connections_available()`, so an
  embedder can check headroom against the limit rather than discovering it by
  being refused.
- `PeerCredentials`, the kernel-reported identity of the process behind an IPC
  connection, with `SubscriptionManager::peer_credentials()` and `peer_pid()`
  to read it.

  **Prefer `uid()` and `gid()` for access-control decisions.** PIDs are
  recycled, so a check that reads a PID and then acts on it can be defeated by
  the original process exiting between the two steps; the user and group ids
  are fixed for the life of the connection. Treat `pid()` as a diagnostic — it
  is what lets a log line name the process that connected.

- `ConfigSource`, reporting which of the two searched locations supplied the
  loaded IPC configuration.
- `CONNECTION_LIMIT_REACHED_CODE` and `CONNECTION_REJECTED_CORRELATION_ID`, the
  wire constants a non-Rust client needs in order to recognise a
  connection-level refusal.
- `IpcNameInUse`, returned by `ActorRuntime::ipc_expose` when a name is already
  claimed. Carries the contested name and the `Ern` of the actor holding it.

### Clarified

- `ActorHandle::children()` and `find_child()` are documented as what they have
  always been: the local view of what was supervised **through that particular
  handle clone**, holding handles that go stale across a restart. Their
  signatures and behavior are unchanged. Use `SupervisedChild` when you need a
  reference that follows restarts.
