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
  `SupervisionStrategy::OneForOne` — the default — is what is carried out.

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

  `SupervisionStrategy::OneForAll` and `RestForOne` are recorded and planned but
  not yet sequenced. Setting one today restarts the child that failed and logs
  that the rest of the plan was not carried out.

- **A child that exhausts its restart allowance now reaches a terminal state.**
  Its supervisor gives up, publishes `SupervisionState::Escalated`, and records
  the reason, so `wait_running()` returns instead of waiting forever. Escalation
  *policy* — `Escalation::NotifyParent` versus `StopSupervisor` — is not yet
  honoured; there is no configuration setter for it to read.

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

### Clarified

- `ActorHandle::children()` and `find_child()` are documented as what they have
  always been: the local view of what was supervised **through that particular
  handle clone**, holding handles that go stale across a restart. Their
  signatures and behavior are unchanged. Use `SupervisedChild` when you need a
  reference that follows restarts.
