# Changelog

All notable changes to `acton-reactive` are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

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

  There is a second-order consequence worth checking before upgrading. Children
  stopped by a cascading shutdown terminate with `TerminationReason::Normal`,
  and `RestartPolicy::Permanent` warrants a restart on a normal termination. The
  built-in supervision bookkeeping suppresses restart decisions during shutdown,
  but a **hand-rolled `ChildTerminated` handler does not**. If you restart
  children from your own handler, check the termination reason, or you may
  restart children on the way down. A dedicated signal that lets children report
  `TerminationReason::ParentShutdown` ships in this same release and removes the
  ambiguity.

### Added

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
