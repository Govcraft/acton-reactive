/*
 * Copyright (c) 2024. Govcraft
 *
 * Licensed under either of
 *   * Apache License, Version 2.0 (the "License");
 *     you may not use this file except in compliance with the License.
 *     You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 *   * MIT license: http://opensource.org/licenses/MIT
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the applicable License for the specific language governing permissions and
 * limitations under that License.
 */

//! A supervisor's record of its children.
//!
//! [`SupervisionRegistry`] is owned by the supervising actor and reached only
//! through `&mut self` inside that actor's own task. It is never cloned, never
//! shared and never locked, so the rule "do not hold the registry across an
//! `await`" is enforced by the borrow checker rather than by a lint.
//!
//! Alongside it are the small values the subsystem passes around: which
//! incarnation of a child is current ([`RestartGeneration`]), where a child sits
//! in its supervisor's start order ([`ChildIndex`]), and how long to wait before
//! trying again ([`BackoffDelay`]).

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use acton_ern::Ern;
use tokio::sync::watch;

use super::plan::{SlotSnapshot, SlotView};
use super::{ChildSpawner, SupervisionError, SupervisionState, SupervisionStatus};
use crate::actor::{RestartLimiter, RestartPolicy};
use crate::common::ActorHandle;

/// Monotonic incarnation counter for a supervised child slot.
///
/// Each restart bumps the generation. Timers and deferred restart signals carry
/// the generation they were scheduled under, so a signal that arrives after the
/// slot has moved on can be discarded instead of restarting a child that has
/// already been replaced or retired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct RestartGeneration(u64);

impl RestartGeneration {
    /// The generation of a child's first incarnation.
    pub const FIRST: Self = Self(0);

    /// Returns the next generation.
    ///
    /// Wraps on overflow rather than panicking. Wrapping is unreachable in
    /// practice — it would take `u64::MAX` restarts of a single child — and a
    /// supervisor must never panic on a counter.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }

    /// Returns the underlying counter value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RestartGeneration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "generation {}", self.0)
    }
}

/// Position of a child in its supervisor's start-ordered child list.
///
/// [`SupervisionStrategy::RestForOne`] restarts the failed child and every
/// child at a higher index, so this ordering is load-bearing rather than
/// incidental.
///
/// [`SupervisionStrategy::RestForOne`]: super::SupervisionStrategy::RestForOne
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChildIndex(usize);

impl ChildIndex {
    /// Creates a child index from a position in the supervisor's child list.
    #[must_use]
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    /// Returns the underlying position.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

impl From<ChildIndex> for usize {
    fn from(value: ChildIndex) -> Self {
        value.0
    }
}

impl fmt::Display for ChildIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "child index {}", self.0)
    }
}

/// A computed delay to wait before a restart attempt.
///
/// Produced by a [`RestartLimiter`](crate::actor::RestartLimiter), which grows
/// the delay exponentially across consecutive restarts so that a child failing
/// in a loop backs off instead of spinning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct BackoffDelay(Duration);

impl BackoffDelay {
    /// No delay; restart immediately.
    pub const NONE: Self = Self(Duration::ZERO);

    /// Returns the delay as a [`Duration`].
    #[must_use]
    pub const fn duration(self) -> Duration {
        self.0
    }

    /// Returns `true` when no waiting is required.
    #[must_use]
    pub const fn is_immediate(self) -> bool {
        self.0.is_zero()
    }
}

impl From<Duration> for BackoffDelay {
    fn from(value: Duration) -> Self {
        Self(value)
    }
}

impl From<BackoffDelay> for Duration {
    fn from(value: BackoffDelay) -> Self {
        value.0
    }
}

impl fmt::Display for BackoffDelay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ms", self.0.as_millis())
    }
}

/// What a supervisor believes one of its children is doing.
///
/// Distinct from [`SupervisionState`], which is the outward-facing summary.
/// This carries the extra distinctions the supervisor needs internally and
/// callers do not, in particular whether a stop was the supervisor's own doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    /// Recorded, queued, nothing launched.
    ///
    /// The supervisor accepted the child — its name is taken and its blueprint
    /// is held — but has not yet handed it to a start task, so no incarnation
    /// exists or is being built. Nothing is running that a shutdown would have
    /// to stop.
    Pending,

    /// A start task is in flight.
    ///
    /// The supervisor has handed this child to a task that is building it, and
    /// is waiting to be told the outcome. Distinct from [`Pending`] in the way
    /// that matters on the way down: a child in this state **may already
    /// exist**, so a shutdown cannot simply forget it. The start task is the
    /// one holding the handle, and is responsible for stopping the child if it
    /// cannot hand it over.
    ///
    /// [`Pending`]: SlotState::Pending
    Starting,

    /// Running and processing messages.
    Running,

    /// Terminated, with a backoff timer armed for the slot's current generation.
    ///
    /// Nothing is running and nothing is being built. The timer holds no handle,
    /// which is why it is not tracked alongside the start tasks: a shutdown has
    /// nothing to wait for here, only a record to settle.
    AwaitingBackoff,

    /// The backoff has elapsed and the restart is queued, not yet launched.
    ///
    /// The restart-path twin of [`Pending`], and it shares that state's whole
    /// meaning: recorded, queued, nothing created. It is a separate variant
    /// because the state it leads to is different — a first start publishes as
    /// `Starting`, a restart as `Restarting` — and because a caller watching a
    /// child can tell a first attempt from a replacement.
    ///
    /// [`Pending`]: SlotState::Pending
    AwaitingRestart,

    /// A replacement is being created.
    ///
    /// The restart-path twin of [`Starting`], with the same obligation on the
    /// way down: a start task holds the handle and is the one that stops the
    /// child if it cannot hand it over.
    ///
    /// [`Starting`]: SlotState::Starting
    Restarting,

    /// The supervisor asked this child to stop.
    ///
    /// Its termination notice is expected, so it must not be read as a fresh
    /// failure. This is the distinction that keeps a group restart from looping:
    /// a child stopped this way reports `Normal`, and a `Permanent` child would
    /// otherwise be restarted for it.
    ExpectedStop {
        /// Whether the supervisor intends to start it again once it is down.
        then_restart: bool,
    },

    /// Terminated and not being restarted.
    Down,

    /// Out of restart allowance; the supervisor has given up.
    Escalated,

    /// Removed from supervision.
    ///
    /// The slot stays in the list so that later children keep their positions.
    Retired,
}

impl SlotState {
    /// The outward-facing state published to callers.
    ///
    /// `ExpectedStop` maps to [`SupervisionState::Restarting`] because from
    /// outside there is no difference between "we are stopping it in order to
    /// restart it" and "we are restarting it" — both mean a new incarnation is
    /// coming.
    #[must_use]
    pub const fn published(self) -> SupervisionState {
        match self {
            // A queued child is indistinguishable from one being built to a
            // caller: both mean "on its way up, no handle yet". The difference
            // is only whether a start task has it, which matters to the
            // supervisor and to nobody else.
            Self::Pending | Self::Starting => SupervisionState::Starting,
            Self::Running => SupervisionState::Running,
            Self::AwaitingBackoff => SupervisionState::RestartPending,
            // `AwaitingRestart` joins these for the same reason `Pending` joins
            // `Starting`: queued and being built are indistinguishable to a
            // caller, and collapsing them means a slot moving from one to the
            // other does not wake a watcher to tell it nothing.
            Self::AwaitingRestart | Self::Restarting | Self::ExpectedStop { .. } => {
                SupervisionState::Restarting
            }
            Self::Down => SupervisionState::Down,
            Self::Escalated => SupervisionState::Escalated,
            Self::Retired => SupervisionState::Retired,
        }
    }

    /// Whether the child is believed to be up and reachable.
    ///
    /// `ExpectedStop` is excluded: the supervisor has already sent it a stop, so
    /// a group restart must not send it a second one. `Pending` is excluded for
    /// the stronger reason that nothing has been created yet — there is no
    /// incarnation to reach.
    #[must_use]
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }

    /// Whether a termination notice in this state is a fresh failure.
    ///
    /// `false` for a stop the supervisor asked for, and for the states that mean
    /// a notice has already been acted on — which is how a duplicate notice from
    /// a previous incarnation is discarded. Also `false` for `Pending`: a slot
    /// with no incarnation cannot have produced the notice.
    #[must_use]
    pub const fn accepts_termination(self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }

    /// Whether a start task is in flight for this child.
    ///
    /// Both kinds: a first incarnation and a replacement. The obligation that
    /// makes them one state is the one that matters on the way down — some
    /// other task is holding a child that may already exist.
    #[must_use]
    pub const fn is_being_started(self) -> bool {
        matches!(self, Self::Starting | Self::Restarting)
    }

    /// Whether this child is queued for a start that has not been launched.
    ///
    /// Again both kinds, and again nothing exists yet in either.
    #[must_use]
    pub const fn is_queued_to_start(self) -> bool {
        matches!(self, Self::Pending | Self::AwaitingRestart)
    }

    /// Whether the supervisor still owes this child an incarnation.
    ///
    /// The union of queued and in flight: every state in which a caller is
    /// waiting for a child to come up and none has. What a shutdown must
    /// answer, so that nobody waits on a start that can no longer land.
    #[must_use]
    pub const fn is_unfinished_start(self) -> bool {
        self.is_queued_to_start() || self.is_being_started()
    }

    /// Whether a backoff timer is armed for this child.
    ///
    /// Kept apart from [`is_unfinished_start`](Self::is_unfinished_start)
    /// because the two are settled for different reasons. An unfinished start
    /// has somebody working on it; a backoff has only a timer that holds
    /// nothing, and on the way down there is nothing to wait for at all.
    #[must_use]
    pub const fn is_awaiting_backoff(self) -> bool {
        matches!(self, Self::AwaitingBackoff)
    }
}

impl fmt::Display for SlotState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => f.write_str("pending"),
            Self::Starting => f.write_str("starting"),
            Self::Running => f.write_str("running"),
            Self::AwaitingBackoff => f.write_str("awaiting_backoff"),
            Self::Restarting => f.write_str("restarting"),
            Self::ExpectedStop { then_restart } => {
                write!(f, "expected_stop(then_restart={then_restart})")
            }
            Self::AwaitingRestart => f.write_str("awaiting_restart"),
            Self::Down => f.write_str("down"),
            Self::Escalated => f.write_str("escalated"),
            Self::Retired => f.write_str("retired"),
        }
    }
}

/// Everything needed to place one child under supervision.
///
/// A struct rather than a long argument list, so that adding a property later
/// does not churn every call site.
#[derive(Debug)]
pub struct NewSlot {
    /// The child's identifier.
    pub ern: Ern,

    /// A handle to the running child.
    pub handle: ActorHandle,

    /// How to recreate the child, or `None` when the supervisor cannot.
    ///
    /// `None` is the legacy `supervise()` path: the supervisor is told when the
    /// child terminates but has no recipe for building another one.
    pub spawner: Option<Arc<dyn ChildSpawner>>,

    /// Whether this child warrants a restart, and when.
    pub restart_policy: RestartPolicy,

    /// This child's own restart allowance and backoff schedule.
    pub limiter: RestartLimiter,

    /// The publishing end of this child's status channel.
    ///
    /// The caller keeps the receiving end. The supervising actor's task is the
    /// only writer.
    pub status: watch::Sender<SupervisionStatus>,
}

/// Everything needed to record a child the supervisor has not created yet.
///
/// Distinct from [`NewSlot`] in the two ways that matter: there is no handle,
/// because nothing has been started, and the spawner is required rather than
/// optional, because running the spawner is the only way this child can ever
/// come up.
#[derive(Debug)]
pub struct PendingSlot {
    /// The child's identifier, already resolved from its configuration.
    pub ern: Ern,

    /// How to create the child, which the supervisor will do on its next turn.
    pub spawner: Arc<dyn ChildSpawner>,

    /// Whether this child warrants a restart, and when.
    pub restart_policy: RestartPolicy,

    /// This child's own restart allowance and backoff schedule.
    pub limiter: RestartLimiter,

    /// The publishing end of this child's status channel.
    pub status: watch::Sender<SupervisionStatus>,
}

/// Everything a start task needs, and nothing more.
///
/// A start runs on its own task so that building a child — which includes the
/// child's own `before_start` hook — does not happen on the supervisor's task.
/// That task cannot be handed the registry, which never leaves the supervisor,
/// so it is handed this instead: what to build, and which slot to report back
/// against.
#[derive(Debug, Clone)]
pub struct StartTicket {
    /// Which slot this start belongs to. Stable: slots are append-only.
    pub index: ChildIndex,

    /// The child's identifier, carried so the report can be matched to the slot
    /// by identity rather than by position alone.
    pub ern: Ern,

    /// How to build the child.
    pub spawner: Arc<dyn ChildSpawner>,
}

/// What a supervisor did with a start task's report.
///
/// Three outcomes rather than a `bool`, because the caller has to act
/// differently on each: a refusal leaves it holding a live child it must stop,
/// and a restart is the moment an actor's IPC names have to be pointed at a new
/// mailbox. A `bool` would collapse the two cases that do work into one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartRecorded {
    /// A first incarnation was recorded.
    First,

    /// A replacement was recorded; the slot advanced a generation.
    Restart,

    /// Refused, because the slot had moved on. **The caller still owns the
    /// handle and must stop it.**
    Refused,
}

impl StartRecorded {
    /// Whether the supervisor took the child on.
    #[must_use]
    pub const fn is_recorded(self) -> bool {
        matches!(self, Self::First | Self::Restart)
    }
}

impl fmt::Display for StartRecorded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::First => f.write_str("first start"),
            Self::Restart => f.write_str("restart"),
            Self::Refused => f.write_str("refused"),
        }
    }
}

/// One supervised child, as recorded by its supervisor.
#[derive(Debug)]
pub struct ChildSlot {
    ern: Ern,
    index: ChildIndex,
    handle: Option<ActorHandle>,
    spawner: Option<Arc<dyn ChildSpawner>>,
    restart_policy: RestartPolicy,
    limiter: RestartLimiter,
    generation: RestartGeneration,
    state: SlotState,
    last_restart: Option<Instant>,
    status: watch::Sender<SupervisionStatus>,
    failure: Option<SupervisionError>,
}

impl ChildSlot {
    /// The child's identifier. Stable across restarts.
    pub const fn ern(&self) -> &Ern {
        &self.ern
    }

    /// The child's position in its supervisor's start order.
    pub const fn index(&self) -> ChildIndex {
        self.index
    }

    /// A handle to the current incarnation, or `None` while the child is down.
    pub const fn handle(&self) -> Option<&ActorHandle> {
        self.handle.as_ref()
    }

    /// The configured restart policy.
    pub const fn restart_policy(&self) -> RestartPolicy {
        self.restart_policy
    }

    /// Which incarnation is current.
    pub const fn generation(&self) -> RestartGeneration {
        self.generation
    }

    /// What the supervisor believes the child is doing.
    pub const fn state(&self) -> SlotState {
        self.state
    }

    /// When the child was last restarted, if it ever has been.
    pub const fn last_restart(&self) -> Option<Instant> {
        self.last_restart
    }

    /// Whether the supervisor holds a recipe for recreating this child.
    pub const fn is_restartable(&self) -> bool {
        self.spawner.is_some()
    }

    /// Whether this child is recorded and queued, with nothing launched yet.
    pub const fn is_pending(&self) -> bool {
        matches!(self.state, SlotState::Pending)
    }

    /// Whether a start task is in flight for this child, of either kind.
    pub const fn is_starting(&self) -> bool {
        self.state.is_being_started()
    }

    /// Whether the supervisor still owes this child an incarnation.
    ///
    /// Both halves of "not up yet": queued, and being built. What a shutdown
    /// has to answer for, and what a caller is still waiting on.
    pub const fn is_unfinished_start(&self) -> bool {
        self.state.is_unfinished_start()
    }

    /// Whether this child is down with a backoff timer armed.
    pub const fn is_awaiting_backoff(&self) -> bool {
        self.state.is_awaiting_backoff()
    }

    /// Why this child is not running, when the supervisor knows a reason.
    ///
    /// Set when a start or restart fails, so that a caller waiting on the
    /// status channel learns what went wrong instead of only that the child
    /// will not be coming.
    pub const fn failure(&self) -> Option<&SupervisionError> {
        self.failure.as_ref()
    }

    /// Records why this child is not running, without publishing it.
    pub fn set_failure(&mut self, failure: SupervisionError) {
        self.failure = Some(failure);
    }

    /// The recipe for recreating this child, if there is one.
    ///
    /// Returned as an owned [`Arc`] on purpose: a restart must clone the spawner
    /// out before awaiting, so that no borrow of the registry is held across the
    /// `await`.
    pub fn spawner(&self) -> Option<Arc<dyn ChildSpawner>> {
        self.spawner.clone()
    }

    /// This child's restart allowance, for the engine to consult and charge.
    pub const fn limiter_mut(&mut self) -> &mut RestartLimiter {
        &mut self.limiter
    }

    /// Records what the supervisor now believes, without publishing it.
    pub const fn set_state(&mut self, state: SlotState) {
        self.state = state;
    }

    /// Attaches a fresh incarnation's handle, or clears it when the child is down.
    pub fn set_handle(&mut self, handle: Option<ActorHandle>) {
        self.handle = handle;
    }

    /// Moves the slot to the next incarnation.
    pub const fn advance_generation(&mut self) {
        self.generation = self.generation.next();
    }

    /// Notes that the child came back at `at`.
    pub const fn mark_restarted_at(&mut self, at: Instant) {
        self.last_restart = Some(at);
    }

    /// Publishes the slot's current state to anyone watching this child.
    ///
    /// Returns whether watchers were woken.
    ///
    /// The comparison is written out field by field rather than derived. A
    /// restart replaces the handle but keeps the [`Ern`], and [`ActorHandle`]
    /// compares by `Ern` alone, so a whole-struct comparison would report "no
    /// change" across a handle swap. Only the fields that actually discriminate
    /// are compared, so a state the caller cannot distinguish does not wake it.
    /// The recorded failure is one of them: learning *why* a child is not
    /// running is news even when its state is unchanged.
    pub fn publish(&self) -> bool {
        let mut next = SupervisionStatus::new(
            self.ern.clone(),
            self.handle.clone(),
            self.generation,
            self.state.published(),
            self.limiter.restarts_in_window(),
        );
        if let Some(failure) = self.failure.clone() {
            next = next.with_failure(failure);
        }

        self.status.send_if_modified(|current| {
            let unchanged = current.generation() == next.generation()
                && current.state() == next.state()
                && current.restarts_in_window() == next.restarts_in_window()
                && current.failure() == next.failure();
            if unchanged {
                return false;
            }
            *current = next;
            true
        })
    }

    /// The read-only view the restart planner needs of this slot.
    const fn view(&self) -> SlotView {
        SlotView {
            index: self.index,
            restartable: self.is_restartable(),
            alive: self.handle.is_some() && self.state.is_running(),
        }
    }
}

/// One supervisor's record of its children.
///
/// Owned by the supervising actor's own task. Not [`Clone`], never shared, and
/// deliberately not behind a lock: every mutation happens on one task in message
/// order, so there is no interleaving to reason about.
#[derive(Debug, Default)]
pub struct SupervisionRegistry {
    /// Start order. [`SupervisionStrategy::RestForOne`] indexes into this.
    ///
    /// Append-only. Removing a child marks its slot [`SlotState::Retired`]
    /// rather than deleting it, because deleting would shift every later child's
    /// index and silently change which children a `RestForOne` restart covers.
    ///
    /// [`SupervisionStrategy::RestForOne`]: super::SupervisionStrategy::RestForOne
    slots: Vec<ChildSlot>,

    /// Lookup from identifier to position, for children under active
    /// supervision. Retired children are removed from here, which is what frees
    /// their identifier to be supervised again.
    by_ern: HashMap<Ern, ChildIndex>,

    /// Children recorded but not yet created, in the order they were recorded.
    ///
    /// Holds positions rather than spawners: the slot already owns the spawner,
    /// and [`ChildSlot::spawner`] hands out an owned [`Arc`], so a second copy
    /// here would only be a second thing to keep in step.
    ///
    /// Drained by the supervising actor's message loop. An entry whose slot has
    /// moved on — retired before its turn came — is discarded rather than
    /// started, which is why the queue is advisory and the slot is the truth.
    pending_starts: VecDeque<ChildIndex>,

    /// Set before children are terminated, to suppress every restart decision.
    shutting_down: bool,
}

impl SupervisionRegistry {
    /// Places a child under supervision, returning its position in start order.
    ///
    /// The child is recorded as [`SlotState::Running`] and its status published,
    /// so a caller watching the channel observes the transition out of
    /// `Starting`.
    ///
    /// # Errors
    ///
    /// [`SupervisionError::DuplicateChild`] if this supervisor already
    /// supervises that identifier.
    pub fn register(&mut self, new: NewSlot) -> Result<ChildIndex, SupervisionError> {
        if self.by_ern.contains_key(&new.ern) {
            return Err(SupervisionError::DuplicateChild { child: new.ern });
        }

        let index = ChildIndex::new(self.slots.len());
        let slot = ChildSlot {
            ern: new.ern.clone(),
            index,
            handle: Some(new.handle),
            spawner: new.spawner,
            restart_policy: new.restart_policy,
            limiter: new.limiter,
            generation: RestartGeneration::FIRST,
            state: SlotState::Running,
            last_restart: None,
            status: new.status,
            failure: None,
        };

        slot.publish();
        self.slots.push(slot);
        self.by_ern.insert(new.ern, index);

        Ok(index)
    }

    /// Records a child the supervisor has not created yet, and queues its start.
    ///
    /// Takes the child's name and its blueprint now so that the collision is
    /// found now: the caller learns about a duplicate at the call site, before
    /// anything has been built. The slot is recorded [`SlotState::Pending`] with
    /// no handle, and its position is queued for the supervisor's next turn.
    ///
    /// # Errors
    ///
    /// [`SupervisionError::DuplicateChild`] if this supervisor already
    /// supervises that identifier. Nothing is recorded and nothing is queued.
    pub fn register_pending(&mut self, new: PendingSlot) -> Result<ChildIndex, SupervisionError> {
        if self.by_ern.contains_key(&new.ern) {
            return Err(SupervisionError::DuplicateChild { child: new.ern });
        }

        let index = ChildIndex::new(self.slots.len());
        let slot = ChildSlot {
            ern: new.ern.clone(),
            index,
            handle: None,
            spawner: Some(new.spawner),
            restart_policy: new.restart_policy,
            limiter: new.limiter,
            generation: RestartGeneration::FIRST,
            state: SlotState::Pending,
            last_restart: None,
            status: new.status,
            failure: None,
        };

        slot.publish();
        self.slots.push(slot);
        self.by_ern.insert(new.ern, index);
        self.pending_starts.push_back(index);

        Ok(index)
    }

    /// Whether any recorded child is still waiting to be created.
    ///
    /// Read once per turn of the supervisor's message loop, so it is a plain
    /// emptiness check rather than anything the loop pays for.
    pub fn has_pending_starts(&self) -> bool {
        !self.pending_starts.is_empty()
    }

    /// Hands the next queued child to a start task, marking it in flight.
    ///
    /// Serves both kinds of start. A slot queued for its first incarnation
    /// (`Pending`) goes to `Starting`; one queued for a replacement
    /// (`AwaitingRestart`) goes to `Restarting`. The restart path therefore
    /// reuses this queue, this ticket, and the start task behind them rather
    /// than learning a second way to create an actor — which is the whole
    /// reason the deferred-start machinery was built first.
    ///
    /// Skips entries whose slot has moved on — retired before its turn came —
    /// rather than starting a child nobody supervises any more. Returns what the
    /// start task needs and nothing it does not: the registry itself never
    /// leaves this actor.
    ///
    /// The slot's outward state does not change either way. `Pending` and
    /// `Starting` both publish as [`SupervisionState::Starting`], and
    /// `AwaitingRestart` and `Restarting` both publish as
    /// [`SupervisionState::Restarting`], so a watcher is not woken to be told
    /// the same thing twice.
    pub fn begin_start(&mut self) -> Option<StartTicket> {
        while let Some(index) = self.pending_starts.pop_front() {
            let Some(slot) = self.slots.get_mut(index.get()) else {
                continue;
            };
            let launched = match slot.state() {
                SlotState::Pending => SlotState::Starting,
                SlotState::AwaitingRestart => SlotState::Restarting,
                _ => continue,
            };
            let Some(spawner) = slot.spawner() else {
                // Unreachable through `register_pending`, which requires a
                // spawner, and through `queue_restart`, which only ever reaches
                // a slot that had one. Skipping beats unwrapping in a
                // supervisor.
                continue;
            };

            slot.set_state(launched);
            slot.publish();

            return Some(StartTicket {
                index,
                ern: slot.ern.clone(),
                spawner,
            });
        }

        None
    }

    /// Records that a child being started is now running behind `handle`.
    ///
    /// Refuses a slot that is not being started, or whose identifier does not
    /// match: it was retired while its start was in flight, and resurrecting it
    /// would supervise a child nobody asked for. **The caller owns the handle
    /// when this returns [`StartRecorded::Refused`], and must stop it.**
    ///
    /// # Where the generation increments
    ///
    /// Here, on a replacement that is actually running — not when the restart
    /// was decided and not when it was launched. Two reasons, pointing the same
    /// way. [`RestartGeneration`] exists so a timer armed under one incarnation
    /// can be discarded once the slot has moved past it, and that comparison
    /// only works if the generation holds still for as long as the timer is
    /// outstanding; bumping at decision time would make every timer stale on
    /// arrival. And [`wait_generation`] resolves on `Running && generation >=
    /// n`, so bumping on success is what makes `wait_generation(FIRST.next())`
    /// mean "incarnation 1 is up".
    ///
    /// `last_restart` is stamped at the same moment and for the same reason:
    /// it feeds "has this child been up long enough to count as recovered",
    /// which is a question about when it came *back*, not when it fell over.
    ///
    /// [`wait_generation`]: super::SupervisedChild::wait_generation
    pub fn complete_start(
        &mut self,
        index: ChildIndex,
        ern: &Ern,
        handle: ActorHandle,
        now: Instant,
    ) -> StartRecorded {
        let Some(slot) = self.slots.get_mut(index.get()) else {
            return StartRecorded::Refused;
        };
        let restarted = match slot.state() {
            SlotState::Starting => false,
            SlotState::Restarting => true,
            _ => return StartRecorded::Refused,
        };
        if &slot.ern != ern {
            return StartRecorded::Refused;
        }

        slot.set_handle(Some(handle));
        if restarted {
            slot.advance_generation();
            slot.mark_restarted_at(now);
        }
        slot.set_state(SlotState::Running);
        slot.publish();

        if restarted {
            StartRecorded::Restart
        } else {
            StartRecorded::First
        }
    }

    /// Moves a slot whose backoff has elapsed onto the pending-start queue.
    ///
    /// Refuses anything that is not exactly the slot the timer was armed for:
    /// a different position, a different child, a different incarnation, or a
    /// slot that is no longer waiting on a backoff at all. A timer is the one
    /// input to this registry that can arrive arbitrarily late, so it is
    /// checked against all four rather than trusted.
    ///
    /// Returns whether the restart was queued.
    pub fn queue_restart(
        &mut self,
        index: ChildIndex,
        ern: &Ern,
        generation: RestartGeneration,
    ) -> bool {
        let Some(slot) = self.slots.get_mut(index.get()) else {
            return false;
        };
        if !slot.is_awaiting_backoff() || &slot.ern != ern || slot.generation() != generation {
            return false;
        }

        slot.set_state(SlotState::AwaitingRestart);
        slot.publish();
        self.pending_starts.push_back(index);
        true
    }

    /// Records that a terminated child is not coming back, and why.
    ///
    /// `state` is the caller's decision: [`SlotState::Down`] when nothing
    /// warranted a restart, [`SlotState::Escalated`] when the child used up its
    /// allowance. Both are terminal, so a caller waiting on this child's status
    /// stops waiting rather than watching a state that will never change again.
    ///
    /// The slot keeps its name and its position. Unlike a failed *first* start,
    /// this child existed and its supervisor still has a record of it; freeing
    /// the identifier here would let a fresh child take a name that is still
    /// spoken for.
    pub fn mark_terminal(
        &mut self,
        index: ChildIndex,
        state: SlotState,
        failure: Option<SupervisionError>,
    ) {
        debug_assert!(
            matches!(state, SlotState::Down | SlotState::Escalated),
            "mark_terminal records why a child stopped, not that it was released"
        );
        let Some(slot) = self.slots.get_mut(index.get()) else {
            return;
        };
        slot.set_handle(None);
        if let Some(failure) = failure {
            slot.set_failure(failure);
        }
        slot.set_state(state);
        slot.publish();
    }

    /// Retires a child the supervisor will not be bringing up, recording why.
    ///
    /// The slot keeps its position but leaves active supervision, which frees
    /// the identifier for another attempt. The reason rides out on the status
    /// channel so that a caller waiting for the child to come up learns what
    /// happened rather than waiting for a start that will never be retried.
    ///
    /// Only for a slot the supervisor still owes an incarnation: queued, being
    /// built, or waiting out a backoff. A child that is up, or one already
    /// settled, is not this method's business, and the filter below is what
    /// says so.
    pub fn fail_start(&mut self, index: ChildIndex, failure: &SupervisionError) {
        let Some(ern) = self
            .slots
            .get(index.get())
            .filter(|slot| slot.is_unfinished_start() || slot.is_awaiting_backoff())
            .map(|slot| slot.ern.clone())
        else {
            return;
        };
        self.by_ern.remove(&ern);

        let Some(slot) = self.slots.get_mut(index.get()) else {
            return;
        };
        slot.set_handle(None);
        slot.set_failure(failure.clone());
        slot.set_state(SlotState::Retired);
        slot.publish();
    }

    /// Abandons every start that has not finished, and says why.
    ///
    /// Called on the way down, and it covers every state in which somebody is
    /// waiting for a child that is not up.
    ///
    /// A queued child was never handed to anyone, so it simply will not happen.
    /// A child whose start is in flight is a different matter: it may already
    /// exist, and the task holding its handle is the one that will stop it when
    /// it finds nobody to hand it to. What is settled here is the *record* and
    /// the caller's answer, not the child.
    ///
    /// A child waiting out a backoff is settled too, and it has to be. Its
    /// timer will fire into a message loop that has ended, so the restart it
    /// was arranged for can never happen; without this the caller waits on a
    /// [`SupervisionState::RestartPending`] that nothing will ever move.
    ///
    /// Returns how many were abandoned, which is what a shutdown logs.
    pub fn cancel_unfinished_starts(&mut self, supervisor: &Ern) -> usize {
        self.pending_starts.clear();

        let unfinished: Vec<ChildIndex> = self
            .slots
            .iter()
            .filter(|slot| slot.is_unfinished_start() || slot.is_awaiting_backoff())
            .map(ChildSlot::index)
            .collect();

        for index in &unfinished {
            self.fail_start(
                *index,
                &SupervisionError::SupervisorStopped {
                    supervisor: supervisor.clone(),
                },
            );
        }

        unfinished.len()
    }

    /// Points an existing registration at a new handle for the same child.
    ///
    /// The legacy `supervise()` path can be called more than once for the same
    /// child; this refreshes the recorded handle instead of rejecting it.
    ///
    /// # Errors
    ///
    /// [`SupervisionError::UnknownChild`] if that child is not supervised.
    pub fn replace_legacy(
        &mut self,
        ern: &Ern,
        handle: ActorHandle,
    ) -> Result<(), SupervisionError> {
        let index = self
            .by_ern
            .get(ern)
            .copied()
            .ok_or_else(|| SupervisionError::UnknownChild { child: ern.clone() })?;

        let Some(slot) = self.slots.get_mut(index.get()) else {
            return Err(SupervisionError::UnknownChild { child: ern.clone() });
        };

        slot.set_handle(Some(handle));
        slot.set_state(SlotState::Running);
        slot.publish();

        Ok(())
    }

    /// Removes a child from supervision.
    ///
    /// Returns the handle the supervisor was holding, so the caller can stop the
    /// child; `None` when the child was already down. The slot is marked
    /// [`SlotState::Retired`] rather than removed, keeping every other child's
    /// index intact, and the identifier is released so it can be supervised
    /// again.
    ///
    /// # Errors
    ///
    /// [`SupervisionError::UnknownChild`] if that child is not supervised.
    pub fn retire(&mut self, ern: &Ern) -> Result<Option<ActorHandle>, SupervisionError> {
        let index = self
            .by_ern
            .remove(ern)
            .ok_or_else(|| SupervisionError::UnknownChild { child: ern.clone() })?;

        let Some(slot) = self.slots.get_mut(index.get()) else {
            return Err(SupervisionError::UnknownChild { child: ern.clone() });
        };

        let handle = slot.handle.take();
        slot.set_state(SlotState::Retired);
        slot.publish();

        Ok(handle)
    }

    /// The position of a supervised child, or `None` if it is not supervised.
    pub fn index_of(&self, ern: &Ern) -> Option<ChildIndex> {
        self.by_ern.get(ern).copied()
    }

    /// The slot at `index`, or `None` if there is none.
    ///
    /// Returns an [`Option`] rather than indexing, because indices reach this
    /// method from termination notices and expired timers, both of which can
    /// name a slot that no longer exists. A panic there would take down the
    /// supervisor and every child under it.
    pub fn slot(&self, index: ChildIndex) -> Option<&ChildSlot> {
        self.slots.get(index.get())
    }

    /// The slot at `index` for modification, or `None` if there is none.
    pub fn slot_mut(&mut self, index: ChildIndex) -> Option<&mut ChildSlot> {
        self.slots.get_mut(index.get())
    }

    /// The slot for a supervised child, or `None` if it is not supervised.
    pub fn slot_of(&self, ern: &Ern) -> Option<&ChildSlot> {
        self.index_of(ern).and_then(|index| self.slot(index))
    }

    /// The slot for a supervised child, for modification.
    pub fn slot_of_mut(&mut self, ern: &Ern) -> Option<&mut ChildSlot> {
        self.index_of(ern).and_then(|index| self.slot_mut(index))
    }

    /// The read-only views the restart planner works from.
    ///
    /// Retired slots are omitted: they exist only to hold their position, and a
    /// restart must never resurrect one. Positions are carried on each view
    /// rather than implied, so omitting slots does not disturb the ordering the
    /// planner relies on.
    pub fn views(&self) -> Vec<SlotView> {
        self.slots
            .iter()
            .filter(|slot| slot.state != SlotState::Retired)
            .map(ChildSlot::view)
            .collect()
    }

    /// Everything the planner needs about the child that just terminated.
    ///
    /// `None` if no such slot exists.
    pub fn snapshot(&self, index: ChildIndex) -> Option<SlotSnapshot> {
        let slot = self.slot(index)?;
        Some(SlotSnapshot {
            index: slot.index,
            restartable: slot.is_restartable(),
            // A shutdown makes every termination expected, which is what stops
            // a cascading stop from being read as a wave of failures.
            expected_stop: self.shutting_down || !slot.state.accepts_termination(),
            last_restart: slot.last_restart,
        })
    }

    /// Handles for every child still believed to be running.
    ///
    /// Feeds cascading shutdown. Retired children are excluded; the caller
    /// already took their handles when retiring them.
    pub fn live_handles(&self) -> Vec<ActorHandle> {
        self.slots
            .iter()
            .filter(|slot| slot.state != SlotState::Retired)
            .filter_map(|slot| slot.handle.clone())
            .collect()
    }

    /// The identifiers of every child this supervisor could recreate.
    ///
    /// "Engine-managed" in one predicate: holding a blueprint is exactly what
    /// distinguishes a child registered through `supervise_with` or
    /// `supervise_deferred` from one adopted through the legacy `supervise()`
    /// path, and it is the same test the restart decision itself turns on.
    ///
    /// Retired children are excluded: a released child may still be running,
    /// and is not the supervisor's to speak for any more.
    pub fn engine_managed_children(&self) -> Vec<Ern> {
        self.slots
            .iter()
            .filter(|slot| slot.state != SlotState::Retired && slot.is_restartable())
            .map(|slot| slot.ern.clone())
            .collect()
    }

    /// Whether this supervisor has no children under active supervision.
    ///
    /// Retired slots do not count, so a supervisor that has retired all of its
    /// children reads as empty even though their positions are still held.
    pub fn is_empty(&self) -> bool {
        self.by_ern.is_empty()
    }

    /// The number of children under active supervision.
    pub fn len(&self) -> usize {
        self.by_ern.len()
    }

    /// Suppresses every restart decision from here on.
    ///
    /// Called before children are terminated, so that the terminations a
    /// shutdown causes are not mistaken for failures worth restarting.
    pub const fn begin_shutdown(&mut self) {
        self.shutting_down = true;
    }

    /// Whether this supervisor has begun shutting down.
    pub const fn is_shutting_down(&self) -> bool {
        self.shutting_down
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::ActorHandleInterface;

    #[test]
    fn first_generation_is_zero() {
        assert_eq!(RestartGeneration::FIRST.get(), 0);
        assert_eq!(RestartGeneration::default(), RestartGeneration::FIRST);
    }

    #[test]
    fn generation_advances_monotonically() {
        let first = RestartGeneration::FIRST;
        let second = first.next();
        let third = second.next();

        assert!(first < second);
        assert!(second < third);
        assert_eq!(third.get(), 2);
    }

    #[test]
    fn generation_wraps_instead_of_panicking_at_the_maximum() {
        // Overflow is unreachable in practice, but a supervisor must not panic
        // on a counter, so the wrap is the specified behaviour.
        let highest = RestartGeneration::FIRST.next();
        assert_eq!(highest.get(), 1);

        let mut at_max = RestartGeneration::FIRST;
        for _ in 0..3 {
            at_max = at_max.next();
        }
        assert_eq!(at_max.get(), 3);
    }

    #[test]
    fn generation_displays_with_its_counter() {
        assert_eq!(RestartGeneration::FIRST.next().next().next().to_string(), "generation 3");
    }

    #[test]
    fn child_index_round_trips_through_usize() {
        let index = ChildIndex::new(7);
        assert_eq!(index.get(), 7);
        assert_eq!(usize::from(index), 7);
    }

    #[test]
    fn child_index_orders_by_start_position() {
        assert!(ChildIndex::new(1) < ChildIndex::new(2));
        assert_eq!(ChildIndex::new(2), ChildIndex::new(2));
    }

    #[test]
    fn child_index_displays_with_its_position() {
        assert_eq!(ChildIndex::new(2).to_string(), "child index 2");
    }

    #[test]
    fn no_backoff_is_immediate() {
        assert!(BackoffDelay::NONE.is_immediate());
        assert_eq!(BackoffDelay::NONE.duration(), Duration::ZERO);
        assert_eq!(BackoffDelay::default(), BackoffDelay::NONE);
    }

    #[test]
    fn nonzero_backoff_is_not_immediate() {
        let delay = BackoffDelay::from(Duration::from_millis(250));
        assert!(!delay.is_immediate());
        assert_eq!(delay.duration(), Duration::from_millis(250));
        assert_eq!(Duration::from(delay), Duration::from_millis(250));
    }

    #[test]
    fn backoff_orders_by_duration() {
        assert!(BackoffDelay::from(Duration::from_millis(100))
            < BackoffDelay::from(Duration::from_millis(200)));
    }

    #[test]
    fn backoff_displays_in_milliseconds() {
        assert_eq!(BackoffDelay::from(Duration::from_millis(250)).to_string(), "250ms");
        assert_eq!(BackoffDelay::NONE.to_string(), "0ms");
    }

    // ---- registry fixtures -----------------------------------------------
    //
    // Every fixture below is built without a Tokio runtime. `mpsc::channel` and
    // `watch::channel` both allocate without touching an executor, and nothing
    // here polls a future, so these tests run on a bare thread.

    /// A spawner that exists only to make a slot restartable.
    ///
    /// `spawn` is never polled by these tests — only `is_restartable()` reads it
    /// — so it returns an error rather than pretending to build an actor.
    #[derive(Debug)]
    struct StubSpawner {
        child: Ern,
    }

    impl ChildSpawner for StubSpawner {
        fn child_id(&self) -> &Ern {
            &self.child
        }

        fn restart_policy(&self) -> RestartPolicy {
            RestartPolicy::Permanent
        }

        fn spawn(
            &self,
            _runtime: crate::common::ActorRuntime,
            _parent: ActorHandle,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<ActorHandle, SupervisionError>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async move {
                Err(SupervisionError::ConfigRejected {
                    child: self.child.clone(),
                    reason: "stub spawner never builds an actor".to_string(),
                })
            })
        }
    }

    fn ern(name: &str) -> Ern {
        Ern::with_root(name).expect("valid Ern root")
    }

    /// A real `ActorHandle` with a real mailbox, built without a runtime.
    fn handle(id: &Ern) -> ActorHandle {
        let (outbox, _inbox) = tokio::sync::mpsc::channel(8);
        ActorHandle::new(id.clone(), outbox)
    }

    fn status_channel(
        id: &Ern,
    ) -> (
        watch::Sender<SupervisionStatus>,
        watch::Receiver<SupervisionStatus>,
    ) {
        watch::channel(SupervisionStatus::new(
            id.clone(),
            None,
            RestartGeneration::FIRST,
            SupervisionState::Starting,
            0,
        ))
    }

    fn new_slot(id: &Ern, restartable: bool) -> (NewSlot, watch::Receiver<SupervisionStatus>) {
        let (status, receiver) = status_channel(id);
        let spawner: Option<Arc<dyn ChildSpawner>> = if restartable {
            Some(Arc::new(StubSpawner { child: id.clone() }))
        } else {
            None
        };

        (
            NewSlot {
                ern: id.clone(),
                handle: handle(id),
                spawner,
                restart_policy: RestartPolicy::Permanent,
                limiter: RestartLimiter::default(),
                status,
            },
            receiver,
        )
    }

    /// Registers `count` restartable children named `child-0..n`.
    fn registry_with(count: usize) -> (SupervisionRegistry, Vec<Ern>) {
        let mut registry = SupervisionRegistry::default();
        let mut erns = Vec::new();

        for _ in 0..count {
            let id = ern("child");
            let (slot, _receiver) = new_slot(&id, true);
            registry.register(slot).expect("distinct Erns never collide");
            erns.push(id);
        }

        (registry, erns)
    }

    #[test]
    fn a_spawner_reports_the_identity_and_policy_it_will_build_with() {
        let id = ern("child");
        let spawner = StubSpawner { child: id.clone() };

        assert_eq!(spawner.child_id(), &id);
        assert_eq!(spawner.restart_policy(), RestartPolicy::Permanent);
    }

    // ---- SlotState -------------------------------------------------------

    #[test]
    fn every_slot_state_maps_to_a_published_state() {
        assert_eq!(SlotState::Starting.published(), SupervisionState::Starting);
        assert_eq!(SlotState::Running.published(), SupervisionState::Running);
        assert_eq!(
            SlotState::AwaitingBackoff.published(),
            SupervisionState::RestartPending
        );
        assert_eq!(
            SlotState::Restarting.published(),
            SupervisionState::Restarting
        );
        assert_eq!(SlotState::Down.published(), SupervisionState::Down);
        assert_eq!(SlotState::Escalated.published(), SupervisionState::Escalated);
        assert_eq!(SlotState::Retired.published(), SupervisionState::Retired);
    }

    #[test]
    fn an_expected_stop_publishes_as_restarting_either_way() {
        // Callers cannot act on the difference between "being stopped so it can
        // restart" and "being restarted", so both read as Restarting.
        for then_restart in [true, false] {
            assert_eq!(
                SlotState::ExpectedStop { then_restart }.published(),
                SupervisionState::Restarting
            );
        }
    }

    #[test]
    fn only_starting_and_running_count_as_up() {
        assert!(SlotState::Starting.is_running());
        assert!(SlotState::Running.is_running());

        for state in [
            SlotState::AwaitingBackoff,
            SlotState::Restarting,
            SlotState::ExpectedStop { then_restart: true },
            SlotState::Down,
            SlotState::Escalated,
            SlotState::Retired,
        ] {
            assert!(!state.is_running(), "{state} should not count as up");
        }
    }

    #[test]
    fn a_termination_is_only_fresh_news_while_the_child_was_up() {
        // The guard against both re-entrant restarts and duplicate notices from
        // a previous incarnation.
        assert!(SlotState::Starting.accepts_termination());
        assert!(SlotState::Running.accepts_termination());

        for state in [
            SlotState::AwaitingBackoff,
            SlotState::Restarting,
            SlotState::ExpectedStop { then_restart: true },
            SlotState::ExpectedStop {
                then_restart: false,
            },
            SlotState::Down,
            SlotState::Escalated,
            SlotState::Retired,
        ] {
            assert!(
                !state.accepts_termination(),
                "{state} should ignore a termination notice"
            );
        }
    }

    // ---- register --------------------------------------------------------

    #[test]
    fn registering_assigns_positions_in_call_order() {
        let (registry, erns) = registry_with(3);

        for (position, id) in erns.iter().enumerate() {
            assert_eq!(registry.index_of(id), Some(ChildIndex::new(position)));
        }
        assert_eq!(registry.len(), 3);
        assert!(!registry.is_empty());
    }

    #[test]
    fn registering_publishes_the_child_as_running() {
        let id = ern("child");
        let (slot, receiver) = new_slot(&id, true);
        let mut registry = SupervisionRegistry::default();

        assert_eq!(receiver.borrow().state(), SupervisionState::Starting);
        registry.register(slot).expect("first registration succeeds");

        let published = receiver.borrow().clone();
        assert_eq!(published.state(), SupervisionState::Running);
        assert_eq!(published.generation(), RestartGeneration::FIRST);
        assert!(published.handle().is_some());
    }

    #[test]
    fn registering_the_same_child_twice_is_rejected() {
        let id = ern("child");
        let mut registry = SupervisionRegistry::default();

        let (first, _first_rx) = new_slot(&id, true);
        registry.register(first).expect("first registration succeeds");

        let (second, _second_rx) = new_slot(&id, true);
        let error = registry
            .register(second)
            .expect_err("the same Ern cannot be registered twice");

        assert_eq!(error, SupervisionError::DuplicateChild { child: id });
        assert_eq!(registry.len(), 1, "the rejected slot was not recorded");
    }

    #[test]
    fn an_empty_registry_reports_empty() {
        let registry = SupervisionRegistry::default();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.views().is_empty());
        assert!(registry.live_handles().is_empty());
        assert!(!registry.is_shutting_down());
    }

    // ---- register_pending ------------------------------------------------

    fn pending_slot(id: &Ern) -> (PendingSlot, watch::Receiver<SupervisionStatus>) {
        let (status, receiver) = status_channel(id);
        (
            PendingSlot {
                ern: id.clone(),
                spawner: Arc::new(StubSpawner { child: id.clone() }),
                restart_policy: RestartPolicy::Permanent,
                limiter: RestartLimiter::default(),
                status,
            },
            receiver,
        )
    }

    #[test]
    fn a_pending_child_is_recorded_without_a_handle_and_queued() {
        let id = ern("child");
        let (slot, receiver) = pending_slot(&id);
        let mut registry = SupervisionRegistry::default();

        let index = registry
            .register_pending(slot)
            .expect("first registration succeeds");

        let child = registry.slot(index).expect("the slot exists");
        assert_eq!(child.state(), SlotState::Pending);
        assert!(child.is_pending());
        assert!(child.handle().is_none(), "nothing has been created yet");
        assert!(child.is_restartable(), "a pending child always has a spawner");
        assert_eq!(
            receiver.borrow().state(),
            SupervisionState::Starting,
            "pending is indistinguishable from starting to a caller"
        );
        assert!(registry.has_pending_starts());
        assert_eq!(registry.len(), 1, "the name is taken from this moment on");
    }

    #[test]
    fn a_pending_duplicate_is_rejected_before_anything_is_built() {
        // The point of registering before spawning: the collision costs a
        // rejected call rather than an actor started and then stopped again.
        let id = ern("child");
        let mut registry = SupervisionRegistry::default();
        let (first, _first_rx) = pending_slot(&id);
        registry
            .register_pending(first)
            .expect("first registration succeeds");

        let (second, _second_rx) = pending_slot(&id);
        let error = registry
            .register_pending(second)
            .expect_err("the same Ern cannot be registered twice");

        assert_eq!(error, SupervisionError::DuplicateChild { child: id });
        assert_eq!(registry.len(), 1, "the rejected slot was not recorded");
        let ticket = registry.begin_start().expect("the accepted child is queued");
        assert_eq!(ticket.index, ChildIndex::new(0));
        assert!(!registry.has_pending_starts(), "and nothing extra was queued");
    }

    #[test]
    fn a_pending_child_collides_with_a_running_one_of_the_same_name() {
        let id = ern("child");
        let mut registry = SupervisionRegistry::default();
        let (running, _running_rx) = new_slot(&id, true);
        registry.register(running).expect("registration succeeds");

        let (pending, _pending_rx) = pending_slot(&id);
        assert!(matches!(
            registry.register_pending(pending),
            Err(SupervisionError::DuplicateChild { .. })
        ));
    }

    #[test]
    fn starting_a_pending_child_attaches_its_handle_and_publishes_running() {
        let id = ern("child");
        let (slot, receiver) = pending_slot(&id);
        let mut registry = SupervisionRegistry::default();
        let index = registry.register_pending(slot).expect("registration succeeds");
        let ticket = registry.begin_start().expect("one start was queued");
        assert_eq!(ticket.index, index);
        assert_eq!(ticket.ern, id);
        assert_eq!(
            registry.slot(index).map(ChildSlot::state),
            Some(SlotState::Starting),
            "handing the child to a start task is recorded"
        );
        assert_eq!(
            receiver.borrow().state(),
            SupervisionState::Starting,
            "and looks no different from outside"
        );

        assert!(registry.complete_start(index, &id, handle(&id), Instant::now()).is_recorded());

        let child = registry.slot(index).expect("the slot exists");
        assert_eq!(child.state(), SlotState::Running);
        assert!(child.handle().is_some());
        assert_eq!(receiver.borrow().state(), SupervisionState::Running);
        assert!(receiver.borrow().failure().is_none());
    }

    #[test]
    fn starting_a_slot_that_moved_on_is_refused() {
        // A child retired while its start was in flight must not be adopted:
        // the caller that got `false` is the one holding the live handle.
        let id = ern("child");
        let (slot, _receiver) = pending_slot(&id);
        let mut registry = SupervisionRegistry::default();
        let index = registry.register_pending(slot).expect("registration succeeds");
        registry.begin_start().expect("the start is in flight");
        registry.retire(&id).expect("the child is supervised");

        assert!(!registry.complete_start(index, &id, handle(&id), Instant::now()).is_recorded());
        assert!(
            !registry
                .complete_start(ChildIndex::new(9), &id, handle(&id), Instant::now())
                .is_recorded(),
            "an index that never existed is refused too"
        );

        // And a report for the right position but the wrong child is refused
        // too, so a stale index cannot hand a slot somebody else's incarnation.
        let other = ern("other");
        let (slot, _receiver) = pending_slot(&other);
        let index = registry.register_pending(slot).expect("registration succeeds");
        registry.begin_start().expect("the start is in flight");
        assert!(!registry.complete_start(index, &id, handle(&id), Instant::now()).is_recorded());
        assert!(registry
            .complete_start(index, &other, handle(&other), Instant::now())
            .is_recorded());
    }

    #[test]
    fn a_failed_start_retires_the_slot_and_publishes_the_reason() {
        let id = ern("child");
        let (slot, receiver) = pending_slot(&id);
        let mut registry = SupervisionRegistry::default();
        let index = registry.register_pending(slot).expect("registration succeeds");

        let failure = SupervisionError::ConfigRejected {
            child: id.clone(),
            reason: "the spawner said no".to_string(),
        };
        registry.fail_start(index, &failure);

        let published = receiver.borrow().clone();
        assert!(
            published.state().is_terminal(),
            "a caller waiting to see it run must stop waiting"
        );
        assert_eq!(published.state(), SupervisionState::Retired);
        assert_eq!(published.failure(), Some(&failure));
        assert!(published.handle().is_none());

        assert_eq!(
            registry.slot(index).and_then(ChildSlot::failure),
            Some(&failure),
            "the slot keeps the reason it retired"
        );
        assert_eq!(registry.index_of(&id), None, "the name is free again");
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn cancelling_queued_starts_tells_every_waiting_caller() {
        let supervisor = ern("pool");
        let mut registry = SupervisionRegistry::default();
        let mut receivers = Vec::new();
        for _ in 0..3 {
            let id = ern("child");
            let (slot, receiver) = pending_slot(&id);
            registry.register_pending(slot).expect("registration succeeds");
            receivers.push(receiver);
        }

        let abandoned = registry.cancel_unfinished_starts(&supervisor);

        assert_eq!(abandoned, 3);
        assert!(!registry.has_pending_starts(), "nothing is left holding a blueprint");
        assert!(registry.is_empty());
        for receiver in &receivers {
            let published = receiver.borrow().clone();
            assert!(published.state().is_terminal());
            assert_eq!(
                published.failure(),
                Some(&SupervisionError::SupervisorStopped {
                    supervisor: supervisor.clone()
                })
            );
        }
    }

    #[test]
    fn cancelling_leaves_children_that_already_started_alone() {
        let supervisor = ern("pool");
        let id = ern("child");
        let (slot, receiver) = pending_slot(&id);
        let mut registry = SupervisionRegistry::default();
        let index = registry.register_pending(slot).expect("registration succeeds");
        let ticket = registry.begin_start().expect("one start was queued");
        assert!(registry
            .complete_start(ticket.index, &id, handle(&id), Instant::now())
            .is_recorded());

        assert_eq!(registry.cancel_unfinished_starts(&supervisor), 0);

        assert_eq!(
            registry.slot(index).map(ChildSlot::state),
            Some(SlotState::Running)
        );
        assert!(receiver.borrow().failure().is_none());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn cancelling_settles_a_child_that_was_waiting_out_a_backoff() {
        // A slot in `AwaitingBackoff` has no start in flight and nothing queued,
        // so it is invisible to a shutdown that only looks for unfinished
        // starts — and its restart can never happen, because the timer will
        // fire into a message loop that has ended.
        //
        // This is asserted here rather than through a running supervisor
        // because it is not observable from there: when a supervisor's task
        // ends it drops the status sender, and a caller blocked on
        // `wait_running` gets `SupervisorStopped` from the channel closing
        // whether or not the slot was ever settled. The registry is the only
        // level at which settling and not settling look different.
        let supervisor = ern("pool");
        let id = ern("child");
        let (slot, receiver) = pending_slot(&id);
        let mut registry = SupervisionRegistry::default();
        let index = registry.register_pending(slot).expect("registration succeeds");
        let ticket = registry.begin_start().expect("one start was queued");
        assert!(registry
            .complete_start(ticket.index, &id, handle(&id), Instant::now())
            .is_recorded());

        // The child ran, then died, and a backoff was armed for it.
        let child = registry.slot_mut(index).expect("the slot exists");
        child.set_handle(None);
        child.set_state(SlotState::AwaitingBackoff);
        child.publish();
        assert_eq!(
            receiver.borrow().state(),
            SupervisionState::RestartPending,
            "the caller is watching a restart that is about to become impossible"
        );

        assert_eq!(
            registry.cancel_unfinished_starts(&supervisor),
            1,
            "a child waiting out a backoff is abandoned like any other start"
        );

        let published = receiver.borrow().clone();
        assert!(
            published.state().is_terminal(),
            "the caller must stop waiting, not sit on RestartPending"
        );
        assert_eq!(
            published.failure(),
            Some(&SupervisionError::SupervisorStopped {
                supervisor: supervisor.clone()
            }),
            "and learn why the restart is not coming"
        );
        assert!(registry.is_empty());
    }

    #[test]
    fn only_children_with_a_blueprint_count_as_engine_managed() {
        // The one predicate that separates a child the engine looks after from
        // one adopted through the legacy path, used for restart decisions and
        // for IPC name removal alike. Two predicates that agree today would
        // drift; this is the same question asked once.
        let mut registry = SupervisionRegistry::default();
        let engine = ern("engine");
        let (slot, _rx) = new_slot(&engine, true);
        registry.register(slot).expect("registration succeeds");

        let legacy = ern("legacy");
        let (slot, _rx) = new_slot(&legacy, false);
        registry.register(slot).expect("registration succeeds");

        assert_eq!(registry.engine_managed_children(), vec![engine.clone()]);

        // A released child is excluded even though it has a blueprint: it may
        // still be running, and is not this supervisor's to speak for.
        registry.retire(&engine).expect("the child is supervised");
        assert!(registry.engine_managed_children().is_empty());
    }

    #[test]
    fn a_pending_child_contributes_no_handle_to_shutdown() {
        // It has no mailbox to send a stop to, and asking for one must not
        // disturb the children that do.
        let (mut registry, erns) = registry_with(2);
        let id = ern("not-yet");
        let (slot, _receiver) = pending_slot(&id);
        registry.register_pending(slot).expect("registration succeeds");

        let handles = registry.live_handles();

        assert_eq!(handles.len(), 2);
        assert!(handles.iter().all(|handle| handle.id() != id));
        assert!(erns.iter().all(|ern| handles.iter().any(|h| &h.id() == ern)));
    }

    #[test]
    fn a_pending_child_is_neither_up_nor_a_source_of_terminations() {
        assert!(!SlotState::Pending.is_running());
        assert!(!SlotState::Pending.accepts_termination());
        assert_eq!(SlotState::Pending.published(), SupervisionState::Starting);
        assert_eq!(SlotState::Pending.to_string(), "pending");
    }

    // ---- retire ----------------------------------------------------------

    #[test]
    fn retiring_returns_the_handle_so_the_caller_can_stop_the_child() {
        let (mut registry, erns) = registry_with(1);

        let handle = registry
            .retire(&erns[0])
            .expect("the child is supervised")
            .expect("the child was running");

        assert_eq!(handle.id(), erns[0]);
    }

    #[test]
    fn retiring_leaves_later_children_at_their_original_positions() {
        // The reason retire marks rather than removes: RestForOne restarts "this
        // child and everything after it", so a shifted index would silently
        // change which children a restart covers.
        let (mut registry, erns) = registry_with(4);

        registry.retire(&erns[1]).expect("the child is supervised");

        assert_eq!(registry.index_of(&erns[0]), Some(ChildIndex::new(0)));
        assert_eq!(registry.index_of(&erns[2]), Some(ChildIndex::new(2)));
        assert_eq!(registry.index_of(&erns[3]), Some(ChildIndex::new(3)));
    }

    #[test]
    fn a_retired_child_is_no_longer_supervised() {
        let (mut registry, erns) = registry_with(2);

        registry.retire(&erns[0]).expect("the child is supervised");

        assert_eq!(registry.index_of(&erns[0]), None);
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.slot(ChildIndex::new(0)).map(ChildSlot::state),
            Some(SlotState::Retired),
            "the slot is still there, holding its position"
        );
    }

    #[test]
    fn retiring_frees_the_identifier_for_reuse() {
        let id = ern("child");
        let mut registry = SupervisionRegistry::default();

        let (first, _first_rx) = new_slot(&id, true);
        registry.register(first).expect("first registration succeeds");
        registry.retire(&id).expect("the child is supervised");

        let (second, _second_rx) = new_slot(&id, true);
        let index = registry
            .register(second)
            .expect("the identifier was released by retiring");

        assert_eq!(
            index,
            ChildIndex::new(1),
            "re-registration takes a fresh position, behind the retired slot"
        );
    }

    #[test]
    fn retiring_a_child_that_is_already_down_yields_no_handle() {
        let (mut registry, erns) = registry_with(1);
        registry
            .slot_of_mut(&erns[0])
            .expect("the child is supervised")
            .set_handle(None);

        let handle = registry.retire(&erns[0]).expect("the child is supervised");

        assert!(handle.is_none());
    }

    #[test]
    fn retiring_an_unknown_child_is_an_error() {
        let mut registry = SupervisionRegistry::default();
        let missing = ern("nobody");

        let error = registry
            .retire(&missing)
            .expect_err("nothing was ever registered");

        assert_eq!(
            error,
            SupervisionError::UnknownChild {
                child: missing.clone()
            }
        );

        // And retiring twice reports the same thing.
        let (mut registry, erns) = registry_with(1);
        registry.retire(&erns[0]).expect("the child is supervised");
        assert!(matches!(
            registry.retire(&erns[0]),
            Err(SupervisionError::UnknownChild { .. })
        ));
    }

    // ---- replace_legacy --------------------------------------------------

    #[test]
    fn replacing_a_legacy_handle_points_the_slot_at_the_new_mailbox() {
        let (mut registry, erns) = registry_with(1);
        let replacement = handle(&erns[0]);

        registry
            .replace_legacy(&erns[0], replacement)
            .expect("the child is supervised");

        let slot = registry.slot_of(&erns[0]).expect("the child is supervised");
        assert_eq!(slot.state(), SlotState::Running);
        assert!(slot.handle().is_some());
    }

    #[test]
    fn replacing_the_handle_of_an_unknown_child_is_an_error() {
        let mut registry = SupervisionRegistry::default();
        let missing = ern("nobody");

        let error = registry
            .replace_legacy(&missing, handle(&missing))
            .expect_err("nothing was ever registered");

        assert_eq!(error, SupervisionError::UnknownChild { child: missing });
    }

    // ---- views and snapshots ---------------------------------------------

    #[test]
    fn views_describe_every_actively_supervised_child() {
        let (registry, _erns) = registry_with(3);

        let views = registry.views();

        assert_eq!(views.len(), 3);
        for (position, view) in views.iter().enumerate() {
            assert_eq!(view.index, ChildIndex::new(position));
            assert!(view.restartable, "every fixture child has a spawner");
            assert!(view.alive, "every fixture child is running");
        }
    }

    #[test]
    fn a_child_without_a_spawner_is_not_restartable() {
        let id = ern("legacy");
        let (slot, _receiver) = new_slot(&id, false);
        let mut registry = SupervisionRegistry::default();
        registry.register(slot).expect("registration succeeds");

        let views = registry.views();
        assert_eq!(views.len(), 1);
        assert!(!views[0].restartable);
        assert!(views[0].alive, "it is running, it just cannot be recreated");
    }

    #[test]
    fn views_omit_retired_slots_but_keep_everyone_elses_position() {
        let (mut registry, erns) = registry_with(4);
        registry.retire(&erns[1]).expect("the child is supervised");

        let views = registry.views();

        let positions: Vec<usize> = views.iter().map(|view| view.index.get()).collect();
        assert_eq!(
            positions,
            vec![0, 2, 3],
            "the retired slot is gone from planning, the rest keep their indices"
        );
    }

    #[test]
    fn a_child_that_is_down_is_not_alive() {
        let (mut registry, erns) = registry_with(2);
        let slot = registry
            .slot_of_mut(&erns[0])
            .expect("the child is supervised");
        slot.set_state(SlotState::Down);
        slot.set_handle(None);

        let views = registry.views();
        assert!(!views[0].alive);
        assert!(views[1].alive);
    }

    #[test]
    fn a_snapshot_of_a_running_child_reads_as_a_fresh_failure() {
        let (registry, erns) = registry_with(1);
        let index = registry.index_of(&erns[0]).expect("supervised");

        let snapshot = registry.snapshot(index).expect("the slot exists");

        assert_eq!(snapshot.index, index);
        assert!(snapshot.restartable);
        assert!(
            !snapshot.expected_stop,
            "a running child terminating is news"
        );
        assert!(snapshot.last_restart.is_none());
    }

    #[test]
    fn a_snapshot_of_a_child_we_stopped_reads_as_expected() {
        // Feeds evaluate's first check: expected_stop wins over everything.
        let (mut registry, erns) = registry_with(1);
        registry
            .slot_of_mut(&erns[0])
            .expect("supervised")
            .set_state(SlotState::ExpectedStop { then_restart: true });
        let index = registry.index_of(&erns[0]).expect("supervised");

        let snapshot = registry.snapshot(index).expect("the slot exists");

        assert!(snapshot.expected_stop);
    }

    #[test]
    fn shutting_down_makes_every_termination_expected() {
        // Without this, a cascading stop looks like a wave of failures and each
        // Permanent child gets restarted on the way down.
        let (mut registry, erns) = registry_with(2);
        registry.begin_shutdown();

        assert!(registry.is_shutting_down());
        for id in &erns {
            let index = registry.index_of(id).expect("supervised");
            let snapshot = registry.snapshot(index).expect("the slot exists");
            assert!(snapshot.expected_stop);
        }
    }

    #[test]
    fn a_snapshot_of_a_missing_slot_is_none() {
        let (registry, _erns) = registry_with(1);
        assert!(registry.snapshot(ChildIndex::new(9)).is_none());
    }

    #[test]
    fn an_out_of_range_index_yields_no_slot_rather_than_panicking() {
        // Stale timers and late termination notices both arrive carrying
        // indices that may no longer exist.
        let (mut registry, _erns) = registry_with(1);
        assert!(registry.slot(ChildIndex::new(9)).is_none());
        assert!(registry.slot_mut(ChildIndex::new(9)).is_none());
    }

    // ---- live_handles ----------------------------------------------------

    #[test]
    fn live_handles_covers_every_child_still_holding_a_mailbox() {
        let (registry, _erns) = registry_with(3);
        assert_eq!(registry.live_handles().len(), 3);
    }

    #[test]
    fn live_handles_skips_children_that_are_down_or_retired() {
        let (mut registry, erns) = registry_with(3);
        registry
            .slot_of_mut(&erns[0])
            .expect("supervised")
            .set_handle(None);
        registry.retire(&erns[1]).expect("supervised");

        let handles = registry.live_handles();

        assert_eq!(handles.len(), 1);
        assert_eq!(handles[0].id(), erns[2]);
    }

    // ---- publish ---------------------------------------------------------

    #[test]
    fn republishing_an_unchanged_state_does_not_wake_watchers() {
        let id = ern("child");
        let (slot, mut receiver) = new_slot(&id, true);
        let mut registry = SupervisionRegistry::default();
        registry.register(slot).expect("registration succeeds");
        // Consume the Starting -> Running change.
        assert!(receiver.has_changed().expect("sender is alive"));
        let _ = receiver.borrow_and_update();

        let woke = registry
            .slot_of(&id)
            .expect("supervised")
            .publish();

        assert!(!woke);
        assert!(
            !receiver.has_changed().expect("sender is alive"),
            "an unchanged republish must not wake a watcher"
        );
    }

    #[test]
    fn swapping_the_handle_alone_does_not_wake_watchers() {
        // Pinned deliberately. ActorHandle compares by Ern only, so a derived
        // whole-struct comparison would also report "unchanged" here — but for
        // the wrong reason. This asserts the documented behaviour so that the
        // field-by-field comparison cannot be replaced by a derive.
        let id = ern("child");
        let (slot, mut receiver) = new_slot(&id, true);
        let mut registry = SupervisionRegistry::default();
        registry.register(slot).expect("registration succeeds");
        let _ = receiver.borrow_and_update();

        let replacement = handle(&id);
        let child = registry.slot_of_mut(&id).expect("supervised");
        child.set_handle(Some(replacement));
        let woke = child.publish();

        assert!(!woke, "same generation and state, so nothing to report");
    }

    #[test]
    fn advancing_the_generation_wakes_watchers() {
        let id = ern("child");
        let (slot, mut receiver) = new_slot(&id, true);
        let mut registry = SupervisionRegistry::default();
        registry.register(slot).expect("registration succeeds");
        let _ = receiver.borrow_and_update();

        let child = registry.slot_of_mut(&id).expect("supervised");
        child.advance_generation();
        let woke = child.publish();

        assert!(woke);
        assert!(receiver.has_changed().expect("sender is alive"));
        assert_eq!(
            receiver.borrow().generation(),
            RestartGeneration::FIRST.next()
        );
    }

    #[test]
    fn changing_state_wakes_watchers() {
        let id = ern("child");
        let (slot, mut receiver) = new_slot(&id, true);
        let mut registry = SupervisionRegistry::default();
        registry.register(slot).expect("registration succeeds");
        let _ = receiver.borrow_and_update();

        let child = registry.slot_of_mut(&id).expect("supervised");
        child.set_state(SlotState::AwaitingBackoff);
        let woke = child.publish();

        assert!(woke);
        assert_eq!(receiver.borrow().state(), SupervisionState::RestartPending);
    }

    #[test]
    fn retiring_publishes_the_terminal_state() {
        let id = ern("child");
        let (slot, receiver) = new_slot(&id, true);
        let mut registry = SupervisionRegistry::default();
        registry.register(slot).expect("registration succeeds");

        registry.retire(&id).expect("supervised");

        let published = receiver.borrow().clone();
        assert_eq!(published.state(), SupervisionState::Retired);
        assert!(published.state().is_terminal());
        assert!(published.handle().is_none(), "the handle was handed back");
    }

    // ---- slot bookkeeping -------------------------------------------------

    #[test]
    fn a_slot_reports_what_it_was_registered_with() {
        let id = ern("child");
        let (slot, _receiver) = new_slot(&id, true);
        let mut registry = SupervisionRegistry::default();
        let index = registry.register(slot).expect("registration succeeds");

        let child = registry.slot(index).expect("the slot exists");

        assert_eq!(child.ern(), &id);
        assert_eq!(child.index(), index);
        assert_eq!(child.restart_policy(), RestartPolicy::Permanent);
        assert_eq!(child.generation(), RestartGeneration::FIRST);
        assert_eq!(child.state(), SlotState::Running);
        assert!(child.is_restartable());
        assert!(child.spawner().is_some());
        assert!(child.last_restart().is_none());
    }

    #[test]
    fn a_slot_records_when_its_child_last_came_back() {
        let (mut registry, erns) = registry_with(1);
        let now = Instant::now();

        let child = registry.slot_of_mut(&erns[0]).expect("supervised");
        child.mark_restarted_at(now);

        assert_eq!(child.last_restart(), Some(now));
        let index = registry.index_of(&erns[0]).expect("supervised");
        assert_eq!(
            registry.snapshot(index).expect("the slot exists").last_restart,
            Some(now)
        );
    }

    #[test]
    fn a_slots_limiter_is_its_own() {
        // Each child backs off on its own schedule; one noisy child must not
        // consume another's allowance.
        let (mut registry, erns) = registry_with(2);

        let first = registry.slot_of_mut(&erns[0]).expect("supervised");
        let _ = first.limiter_mut().record_restart();
        let _ = first.limiter_mut().record_restart();

        assert!(
            registry.slot_of(&erns[0]).expect("supervised").publish(),
            "the restart count changed, so watchers are told"
        );

        let second_count = {
            let second = registry.slot_of_mut(&erns[1]).expect("supervised");
            second.limiter_mut().restarts_in_window()
        };
        assert_eq!(second_count, 0);
    }
}
