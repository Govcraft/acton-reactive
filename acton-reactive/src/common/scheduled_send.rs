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

//! Scheduled sends: the machinery behind
//! [`send_after`](crate::common::ActorHandle::send_after),
//! [`send_at`](crate::common::ActorHandle::send_at), and
//! [`send_every`](crate::common::ActorHandle::send_every).
//!
//! # Why this is not `tokio::spawn` plus `sleep`
//!
//! Because that is a task nobody owns. It does not take part in shutdown, it
//! cannot be cancelled, a panic inside it is invisible, and the only way to
//! test it is to sleep in the test and hope. A [`ScheduledSend`] is the same
//! deferral with the ownership put back: it can be cancelled, it reports what
//! became of it, it ends when the target actor does, and it can be driven by
//! an injected [`Clock`] so a test never waits at all.
//!
//! # What ends a scheduled send
//!
//! Three things, and exactly one of them always happens, which is what lets
//! [`ScheduledSend::outcome`] promise to resolve:
//!
//! * the caller [`cancel`](ScheduledSend::cancel)s it;
//! * the target actor's inbox closes;
//! * the deadline arrives and the message is delivered — or fails to be.
//!
//! The middle one deserves its reasoning written down, because two more
//! obvious mechanisms are both wrong here.
//!
//! **Not the actor's `TaskTracker`.** [`ActorHandle::stop`] waits on
//! `tracker.wait()`, which needs the tracker closed *and empty*. The tracker is
//! closed once at start-up and closing does not prevent later spawns, so a
//! timer parked on it would keep it non-empty and `stop()` would block for the
//! whole delay — forever, for a repeating schedule. The supervision engine
//! learned this with restart backoffs and its timer uses a bare
//! `tokio::spawn` for the same reason.
//!
//! **Not `ActorHandle`'s cancellation token.** That field is an independent
//! token created per handle, parented to nothing, and cancelled nowhere in the
//! crate. The real hierarchy hangs off `ManagedActor`, and even the runtime
//! root fires only when `shutdown_all` *times out*. A `select!` on the handle's
//! token would compile, read as though it worked, and never fire.
//!
//! **The outbox closing is the signal.** `ActorSender` is an
//! `mpsc::Sender<Envelope>`, so awaiting its `closed()` resolves when the
//! actor's receiver is dropped or closed. That happens on every termination
//! path — graceful stop, panic, restart — with no new plumbing, and it is
//! *ordered* with respect to `stop()`: the actor is dropped when its tracked
//! task ends, and `stop()` returns only after that. So a test may stop an actor
//! and then assert the outcome, with no sleep and no race.
//!
//! [`ActorHandle::stop`]: crate::traits::ActorHandleInterface::stop

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::trace;

use crate::common::clock::{Clock, Timer};
use crate::common::types::ActorSender;
use crate::message::OutboundEnvelope;
use crate::traits::ActonMessage;

/// The moment a scheduled send comes due.
///
/// A newtype over [`Instant`] rather than a bare one, because the arithmetic
/// that produces it is the arithmetic that can go wrong: adding a delay can
/// overflow, and subtracting a deadline that has already passed can underflow.
/// Both are handled once, here, as pure functions with tests, instead of at
/// each call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FireAt(Instant);

impl FireAt {
    /// `delay` after `now`.
    ///
    /// Saturates towards the far future rather than panicking if the sum
    /// cannot be represented: a delay of [`Duration::MAX`] describes a send
    /// that should effectively never happen, and aborting the process is not a
    /// reasonable reading of that. Saturating *downwards* would be worse than
    /// either, since a repeating schedule whose next deadline landed in the
    /// past would spin.
    #[must_use]
    pub fn after(now: Instant, delay: Duration) -> Self {
        if let Some(deadline) = now.checked_add(delay) {
            return Self(deadline);
        }

        // Halve until the sum fits. Converges in at most as many steps as
        // `Duration` has bits, and lands within a factor of two of the
        // furthest instant this platform can represent — far enough that
        // "effectively never" is an honest description.
        let mut delay = delay;
        loop {
            delay /= 2;
            if delay.is_zero() {
                return Self(now);
            }
            if let Some(deadline) = now.checked_add(delay) {
                return Self(deadline);
            }
        }
    }

    /// Exactly `deadline`.
    #[must_use]
    pub const fn at(deadline: Instant) -> Self {
        Self(deadline)
    }

    /// The underlying instant.
    #[must_use]
    pub const fn instant(self) -> Instant {
        self.0
    }

    /// How long remains, as of `now`.
    ///
    /// [`Duration::ZERO`] once due, never a negative or wrapped value.
    #[must_use]
    pub fn remaining_from(self, now: Instant) -> Duration {
        self.0.saturating_duration_since(now)
    }

    /// Whether the deadline has arrived, as of `now`.
    ///
    /// Landing exactly on the deadline counts as due. The fixed-rate scheduler
    /// produces exact hits routinely, and the alternative reading would make
    /// every such tick a whole interval late.
    #[must_use]
    pub fn is_due_at(self, now: Instant) -> bool {
        now >= self.0
    }
}

/// A repeating interval. Never zero.
///
/// A zero interval is not a schedule, it is a spin loop, and the fixed-rate
/// grid arithmetic divides by it. Making zero unrepresentable settles both
/// concerns at the one place a value is introduced, rather than with a check
/// at every call site or a `Result` on a method whose failure nobody wants to
/// handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Interval(Duration);

impl Interval {
    /// An interval of `period`, or `None` if it is zero.
    #[must_use]
    pub const fn new(period: Duration) -> Option<Self> {
        if period.is_zero() {
            None
        } else {
            Some(Self(period))
        }
    }

    /// An interval of `secs` seconds, or `None` if that is zero.
    #[must_use]
    pub const fn from_secs(secs: u64) -> Option<Self> {
        Self::new(Duration::from_secs(secs))
    }

    /// An interval of `millis` milliseconds, or `None` if that is zero.
    #[must_use]
    pub const fn from_millis(millis: u64) -> Option<Self> {
        Self::new(Duration::from_millis(millis))
    }

    /// The interval as a [`Duration`].
    #[must_use]
    pub const fn get(self) -> Duration {
        self.0
    }
}

impl TryFrom<Duration> for Interval {
    type Error = ZeroInterval;

    fn try_from(period: Duration) -> Result<Self, Self::Error> {
        Self::new(period).ok_or(ZeroInterval)
    }
}

impl From<Interval> for Duration {
    fn from(interval: Interval) -> Self {
        interval.0
    }
}

/// An [`Interval`] was asked for with a period of zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZeroInterval;

impl fmt::Display for ZeroInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a repeating interval must be greater than zero; a zero interval is a spin loop, \
             not a schedule"
        )
    }
}

impl std::error::Error for ZeroInterval {}

/// How a repeating schedule spaces its ticks.
///
/// The distinction only becomes visible when a tick is late — because the
/// send itself was slow, or because nothing polled the timer for a while. Both
/// modes skip and coalesce missed ticks; they differ in where the next
/// deadline is measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cadence {
    /// Deadlines sit on a fixed grid anchored at the moment the schedule was
    /// armed: `armed_at + n * interval`.
    ///
    /// A slow send does not push the grid, so the schedule keeps long-run
    /// average pace. Use this when ticks mark *times* — a sample every second,
    /// a report on the minute.
    FixedRate,

    /// Each interval is measured from the completion of the previous send:
    /// `next = now_after_send + interval`.
    ///
    /// A slow send pushes everything after it out, so consecutive ticks are
    /// always at least one interval apart. Use this when ticks mark *gaps* —
    /// a poll that should rest between attempts.
    FixedDelay,
}

/// The first fixed-rate deadline strictly after `now`.
///
/// Deadlines lie on the grid `armed_at + n * interval`. This returns the
/// earliest grid point past `now`, which is what both arms the schedule and
/// implements skip-and-coalesce:
///
/// * at arming, `now == armed_at`, and the answer is `armed_at + interval` —
///   the first tick comes after one full interval, never immediately;
/// * after a tick, whole intervals that went by unserviced are stepped over in
///   one division rather than fired as a catch-up burst;
/// * `now` sitting exactly on a grid point yields the *next* point, because
///   the tick at `now` is the one that has just been delivered.
///
/// A division, not a loop: a schedule that fell an hour behind costs the same
/// as one that did not.
#[must_use]
pub fn next_fixed_rate(armed_at: Instant, interval: Interval, now: Instant) -> FireAt {
    let elapsed = now.saturating_duration_since(armed_at).as_nanos();
    // Non-zero by construction, which is the whole reason `Interval` exists.
    let step = interval.get().as_nanos();

    let ticks = elapsed / step + 1;
    let offset_nanos = ticks.saturating_mul(step);

    let seconds = offset_nanos / 1_000_000_000;
    let remainder = offset_nanos % 1_000_000_000;
    let offset = u64::try_from(seconds).map_or(Duration::MAX, |seconds| {
        // Always in range: a remainder modulo one billion cannot exceed it.
        Duration::new(seconds, u32::try_from(remainder).unwrap_or(0))
    });

    FireAt::after(armed_at, offset)
}

/// What became of a scheduled send.
///
/// Not an error type: three of the four outcomes are ordinary, and only
/// [`Undeliverable`](Self::Undeliverable) describes something going wrong.
/// `#[non_exhaustive]`, so further outcomes can be added without a breaking
/// change; match with a `_` arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScheduledSendOutcome {
    /// The deadline arrived and the message reached the actor's inbox.
    ///
    /// Terminal for a one-shot send. A repeating schedule records deliveries
    /// in [`ScheduledSend::deliveries`] and keeps running, so it never settles
    /// on this.
    Delivered,

    /// [`ScheduledSend::cancel`] was called before the deadline.
    ///
    /// Nothing was sent and nothing will be.
    Cancelled,

    /// The target actor went away before the deadline.
    ///
    /// Its inbox closed — a stop, a panic, or a restart — or the runtime tore
    /// the timer down. Distinct from [`Cancelled`](Self::Cancelled) because
    /// the difference is worth acting on: cancelling is something you did,
    /// while this is something that happened to a message you meant to send.
    Abandoned,

    /// The deadline arrived, but delivery failed.
    ///
    /// The inbox closed in the window between the timer firing and the send,
    /// or the send was cancelled mid-flight. A repeating schedule stops here
    /// rather than retrying into an inbox that is not coming back.
    Undeliverable {
        /// What the delivery attempt reported.
        detail: String,
    },
}

/// A consistent snapshot of a schedule's state.
///
/// One `watch` channel carries all of it, so `deliveries`, `due`, and `settled`
/// can never be read from different moments and mistaken for a state that
/// never existed.
#[derive(Debug, Clone)]
struct Progress {
    /// How many messages have reached the inbox so far.
    deliveries: u64,
    /// The deadline currently armed, or `None` once the schedule has settled.
    due: Option<FireAt>,
    /// The terminal outcome, once there is one.
    settled: Option<ScheduledSendOutcome>,
}

/// A handle on a message that has been scheduled but not yet sent.
///
/// Returned by [`send_after`](crate::common::ActorHandle::send_after),
/// [`send_at`](crate::common::ActorHandle::send_at), and
/// [`send_every`](crate::common::ActorHandle::send_every). Dropping it does
/// **not** cancel the schedule — a fire-and-forget deferral is a legitimate
/// thing to want, and a timer that vanished when its receipt went out of scope
/// would be a trap. Call [`cancel`](Self::cancel) to stop it.
///
/// Cloneable; every clone observes the same schedule and can cancel it.
///
/// # As a barrier
///
/// [`outcome`](Self::outcome) and
/// [`wait_for_deliveries`](Self::wait_for_deliveries) exist so that tests and
/// startup sequences never have to sleep. Both are guaranteed to resolve:
/// if the timer task disappears without settling, they report
/// [`Abandoned`](ScheduledSendOutcome::Abandoned) rather than hanging.
#[derive(Debug, Clone)]
pub struct ScheduledSend {
    cancel: CancellationToken,
    progress: watch::Receiver<Progress>,
}

impl ScheduledSend {
    /// The deadline currently armed, or `None` once the schedule has settled.
    ///
    /// For a repeating schedule this moves forward with each tick, which is
    /// what makes the two [`Cadence`] modes distinguishable without waiting
    /// for real time to pass.
    #[must_use]
    pub fn due_at(&self) -> Option<FireAt> {
        self.progress.borrow().due
    }

    /// How many messages have reached the actor's inbox so far.
    ///
    /// Always `0` or `1` for a one-shot send.
    #[must_use]
    pub fn deliveries(&self) -> u64 {
        self.progress.borrow().deliveries
    }

    /// The terminal outcome if the schedule has ended, `None` while it runs.
    ///
    /// Non-blocking. Use [`outcome`](Self::outcome) to wait for one.
    #[must_use]
    pub fn settled(&self) -> Option<ScheduledSendOutcome> {
        self.progress.borrow().settled.clone()
    }

    /// Whether the schedule has ended.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.progress.borrow().settled.is_some()
    }

    /// Stops the schedule.
    ///
    /// Idempotent, and safe to call after the schedule has already ended: a
    /// send that has been delivered stays [`Delivered`], because cancelling
    /// something that already happened cannot unhappen it.
    ///
    /// Returns immediately. Await [`outcome`](Self::outcome) to know the
    /// timer has actually wound up.
    ///
    /// # What it does not interrupt
    ///
    /// A tick that has already come due and begun delivering finishes. A full
    /// inbox makes delivery itself wait, so cancelling during one takes effect
    /// once that send completes, and the schedule then settles on
    /// [`Cancelled`](ScheduledSendOutcome::Cancelled) without arming another
    /// tick.
    ///
    /// This is deliberate rather than a limitation: the deadline passed before
    /// the cancellation arrived, so the tick was owed. Cancelling governs the
    /// ticks still to come.
    ///
    /// [`Delivered`]: ScheduledSendOutcome::Delivered
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Waits for the schedule to end, and reports how.
    ///
    /// The barrier that replaces sleeping. A one-shot send settles on its
    /// first delivery; a repeating one settles only when cancelled, abandoned,
    /// or unable to deliver.
    ///
    /// Resolves even if the timer task is torn down without settling — the
    /// channel closing is itself the answer, reported as
    /// [`Abandoned`](ScheduledSendOutcome::Abandoned). A handle that could
    /// hang forever would be worse than one that occasionally answers coarsely.
    pub async fn outcome(&self) -> ScheduledSendOutcome {
        let mut observer = self.progress.clone();

        loop {
            {
                let progress = observer.borrow_and_update();
                if let Some(outcome) = progress.settled.clone() {
                    return outcome;
                }
            }

            if observer.changed().await.is_err() {
                return ScheduledSendOutcome::Abandoned;
            }
        }
    }

    /// Waits until at least `at_least` messages have been delivered, and
    /// reports how many have.
    ///
    /// The barrier for repeating schedules. Returns early if the schedule
    /// settles first, so a cancelled or abandoned schedule cannot leave a
    /// caller waiting for a count that will never be reached — which means the
    /// answer may be smaller than asked for, and is always the true count.
    ///
    /// Because a tick can only be armed for a deadline strictly in the future,
    /// a count read after this returns is final for as long as the clock does
    /// not advance again. That is what lets a test assert an exact number of
    /// deliveries rather than a lower bound.
    pub async fn wait_for_deliveries(&self, at_least: u64) -> u64 {
        let mut observer = self.progress.clone();

        loop {
            {
                let progress = observer.borrow_and_update();
                if progress.deliveries >= at_least || progress.settled.is_some() {
                    return progress.deliveries;
                }
            }

            if observer.changed().await.is_err() {
                return observer.borrow().deliveries;
            }
        }
    }
}

/// What a schedule does after each tick.
#[derive(Debug, Clone, Copy)]
pub enum Plan {
    /// Deliver once, at this deadline, then settle.
    Once(FireAt),
    /// Deliver repeatedly, starting one interval after `armed_at`.
    Repeat {
        /// The instant the schedule was created; the anchor of the fixed-rate
        /// grid.
        armed_at: Instant,
        /// The gap between ticks.
        interval: Interval,
        /// How the gap is measured.
        cadence: Cadence,
    },
}

impl Plan {
    /// The first deadline this plan arms.
    fn first_deadline(self) -> FireAt {
        match self {
            Self::Once(due) => due,
            Self::Repeat {
                armed_at, interval, ..
            } => next_fixed_rate(armed_at, interval, armed_at),
        }
    }

    /// The deadline after a tick delivered at `now`, or `None` if the plan is
    /// finished.
    fn deadline_after_tick(self, now: Instant) -> Option<FireAt> {
        match self {
            Self::Once(_) => None,
            Self::Repeat {
                armed_at,
                interval,
                cadence,
            } => Some(match cadence {
                Cadence::FixedRate => next_fixed_rate(armed_at, interval, now),
                Cadence::FixedDelay => FireAt::after(now, interval.get()),
            }),
        }
    }
}

/// Everything the timer task needs, gathered so the spawn site stays readable.
pub struct Schedule {
    /// The clock deadlines are measured against.
    pub clock: Arc<dyn Clock>,
    /// Addressed at the target actor, and how the message is delivered.
    pub envelope: OutboundEnvelope,
    /// The target's inbox sender, held only to await its closure.
    pub outbox: ActorSender,
    /// The message, cloned once per tick.
    pub message: Box<dyn ActonMessage + Send + Sync>,
    /// One-shot or repeating.
    pub plan: Plan,
}

/// Arms `schedule` and returns the handle that observes and cancels it.
///
/// The timer runs on a bare `tokio::spawn`, deliberately; see the module
/// documentation for why neither the actor's task tracker nor its cancellation
/// token is the right thing to hang it on.
pub fn arm(schedule: Schedule) -> ScheduledSend {
    let cancel = CancellationToken::new();
    let first = schedule.plan.first_deadline();

    // Load-bearing: the first timer is armed *here*, on the caller's thread,
    // before the task exists. Arming inside the task would push registration
    // to whenever the runtime first polled it, so a caller that armed a
    // schedule and immediately advanced a `ManualClock` would race that first
    // poll. `Clock::timer` promising to register synchronously buys nothing if
    // the call to it is itself deferred.
    let armed = schedule.clock.timer(first.instant());

    let (progress, observer) = watch::channel(Progress {
        deliveries: 0,
        due: Some(first),
        settled: None,
    });

    let handle = ScheduledSend {
        cancel: cancel.clone(),
        progress: observer,
    };

    tokio::spawn(run(schedule, armed, cancel, progress));

    handle
}

/// The timer loop.
///
/// Returns once the schedule settles, at which point dropping `progress`
/// closes the channel. The final value stays readable to observers, so the
/// settled outcome is never lost by the sender going away.
async fn run(
    schedule: Schedule,
    armed: Timer,
    cancel: CancellationToken,
    progress: watch::Sender<Progress>,
) {
    let Schedule {
        clock,
        envelope,
        outbox,
        message,
        plan,
    } = schedule;

    // Handed in already armed, so the first deadline was registered before this
    // task existed. Later ticks are necessarily armed here, since nothing can
    // know when a send finishes until it does.
    let mut timer = armed;

    loop {
        let outcome = tokio::select! {
            () = cancel.cancelled() => Some(ScheduledSendOutcome::Cancelled),
            () = outbox.closed() => Some(ScheduledSendOutcome::Abandoned),
            () = &mut timer => {
                match deliver(&envelope, message.as_ref()).await {
                    Ok(()) => None,
                    Err(detail) => Some(ScheduledSendOutcome::Undeliverable { detail }),
                }
            }
        };

        if let Some(outcome) = outcome {
            trace!(?outcome, "scheduled send settled");
            progress.send_modify(|state| {
                state.due = None;
                state.settled = Some(outcome);
            });
            return;
        }

        // Delivered. Work out the next deadline *before* publishing, so that
        // an observer which sees the delivery count rise also sees the
        // deadline that follows it — that consistency is what lets a test
        // distinguish the two cadences.
        let next = plan.deadline_after_tick(clock.now());

        progress.send_modify(|state| {
            state.deliveries += 1;
            state.due = next;
            if next.is_none() {
                state.settled = Some(ScheduledSendOutcome::Delivered);
            }
        });

        let Some(next) = next else {
            return;
        };
        timer = clock.timer(next.instant());
    }
}

/// Delivers one copy of `message`, describing any failure.
///
/// The clone is `Arc`ed *from the box*, never `Arc::new(the_box)`: a
/// `Box<dyn ActonMessage>` itself satisfies the `ActonMessage` blanket impl,
/// so wrapping one would double-wrap the payload and every handler's downcast
/// would fail while the message still looked correct in logs. `ask` documents
/// the same trap from the receiving side.
async fn deliver(
    envelope: &OutboundEnvelope,
    message: &(dyn ActonMessage + Send + Sync),
) -> Result<(), String> {
    let copy: Box<dyn ActonMessage + Send + Sync> = dyn_clone::clone_box(message);
    envelope
        .try_send_dyn(Arc::from(copy))
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    fn interval(secs: u64) -> Interval {
        Interval::from_secs(secs).expect("a non-zero interval")
    }

    // --- FireAt -----------------------------------------------------------

    /// A zero delay is due the instant it is created. Scheduling code turns
    /// "already due" into "send now", so this is the boundary that decides
    /// whether `send_after(msg, ZERO)` sends at all.
    #[test]
    fn a_zero_delay_is_due_immediately() {
        let now = Instant::now();
        let due = FireAt::after(now, Duration::ZERO);

        assert!(due.is_due_at(now));
        assert_eq!(due.remaining_from(now), Duration::ZERO);
    }

    /// Landing exactly on the deadline counts as due. The fixed-rate grid
    /// produces exact hits constantly; the other reading would make every tick
    /// one interval late.
    #[test]
    fn a_deadline_reached_exactly_is_due() {
        let now = Instant::now();
        let due = FireAt::after(now, SECOND);

        assert!(!due.is_due_at(now), "not due a second early");
        assert!(due.is_due_at(now + SECOND), "due exactly on time");
    }

    /// A past deadline has zero remaining, not a wrapped or negative one.
    #[test]
    fn a_past_deadline_has_no_time_remaining() {
        let now = Instant::now();
        let due = FireAt::at(
            now.checked_sub(Duration::from_mins(1))
                .expect("a minute ago"),
        );

        assert_eq!(due.remaining_from(now), Duration::ZERO);
    }

    #[test]
    fn remaining_is_exact_for_a_future_deadline() {
        let now = Instant::now();
        let due = FireAt::after(now, Duration::from_secs(30));

        assert_eq!(due.remaining_from(now), Duration::from_secs(30));
    }

    /// An absurd delay must not abort the process, and must not saturate
    /// *downwards* either: a repeating schedule whose next deadline landed in
    /// the past would spin at full speed.
    #[test]
    fn an_unrepresentable_delay_saturates_into_the_future() {
        let now = Instant::now();
        let due = FireAt::after(now, Duration::MAX);

        assert!(
            !due.is_due_at(now + Duration::from_hours(24 * 365)),
            "an overflowing delay must land far in the future, not in the past"
        );
    }

    #[test]
    fn at_round_trips_through_instant() {
        let now = Instant::now();
        assert_eq!(FireAt::at(now).instant(), now);
    }

    /// Ordering follows the instants, so a scheduler can compare deadlines
    /// directly.
    #[test]
    fn deadlines_order_by_time() {
        let now = Instant::now();
        let sooner = FireAt::after(now, SECOND);
        let later = FireAt::after(now, Duration::from_secs(2));

        assert!(sooner < later);
    }

    // --- Interval ---------------------------------------------------------

    /// The reason the newtype exists: a zero interval divides by zero in the
    /// grid arithmetic and spins in the loop.
    #[test]
    fn a_zero_interval_cannot_be_built() {
        assert_eq!(Interval::new(Duration::ZERO), None);
        assert_eq!(Interval::from_secs(0), None);
        assert_eq!(Interval::from_millis(0), None);
        assert_eq!(Interval::try_from(Duration::ZERO), Err(ZeroInterval));
    }

    #[test]
    fn a_non_zero_interval_keeps_its_period() {
        let interval = Interval::try_from(SECOND).expect("one second is not zero");

        assert_eq!(interval.get(), SECOND);
        assert_eq!(Duration::from(interval), SECOND);
    }

    #[test]
    fn the_zero_interval_error_describes_itself() {
        assert!(!ZeroInterval.to_string().is_empty());
    }

    // --- next_fixed_rate --------------------------------------------------

    /// The first tick comes after one full interval, never at arming time.
    /// Erlang's `timer:send_interval` convention, and the only one under which
    /// `send_every` and `send_after` compose predictably.
    #[test]
    fn the_first_tick_is_one_whole_interval_out() {
        let armed_at = Instant::now();

        let first = next_fixed_rate(armed_at, interval(5), armed_at);

        assert_eq!(first.instant(), armed_at + Duration::from_secs(5));
    }

    /// Sitting exactly on a grid point yields the *next* one. That point is
    /// the tick that has just been delivered, so returning it would deliver it
    /// twice and then spin.
    #[test]
    fn a_deadline_landed_on_exactly_advances_to_the_next() {
        let armed_at = Instant::now();
        let now = armed_at + Duration::from_secs(5);

        let next = next_fixed_rate(armed_at, interval(5), now);

        assert_eq!(next.instant(), armed_at + Duration::from_secs(10));
    }

    /// Skip and coalesce: intervals that went by unserviced are stepped over
    /// in one move, not fired as a catch-up burst. Two and a half intervals
    /// late, the answer is the third grid point — not the first missed one.
    #[test]
    fn missed_intervals_are_skipped_rather_than_queued() {
        let armed_at = Instant::now();
        let now = armed_at + Duration::from_secs(25);

        let next = next_fixed_rate(armed_at, interval(10), now);

        assert_eq!(
            next.instant(),
            armed_at + Duration::from_secs(30),
            "roll forward to the first future grid point"
        );
    }

    /// The invariant the loop depends on: a returned deadline is always
    /// strictly in the future, so re-arming can never spin.
    #[test]
    fn the_next_deadline_is_always_strictly_in_the_future() {
        let armed_at = Instant::now();
        let step = interval(3);

        for elapsed_secs in 0..40_u64 {
            let now = armed_at + Duration::from_secs(elapsed_secs);
            let next = next_fixed_rate(armed_at, step, now);

            assert!(
                next.instant() > now,
                "a deadline at or before `now` would spin (elapsed {elapsed_secs}s)"
            );
        }
    }

    /// Deadlines stay on the grid however late the caller is, which is what
    /// separates fixed-rate from fixed-delay.
    #[test]
    fn deadlines_stay_on_the_grid_however_late() {
        let armed_at = Instant::now();
        let step = interval(4);

        for elapsed_secs in 0..40_u64 {
            let now = armed_at + Duration::from_secs(elapsed_secs);
            let offset = next_fixed_rate(armed_at, step, now)
                .instant()
                .duration_since(armed_at);

            assert_eq!(
                offset.as_secs() % 4,
                0,
                "every fixed-rate deadline is a whole number of intervals from arming"
            );
        }
    }

    /// An enormous elapsed time must saturate rather than overflow, and still
    /// land in the future.
    #[test]
    fn an_enormous_elapsed_time_saturates_into_the_future() {
        let armed_at = Instant::now();
        let now = armed_at + Duration::from_hours(24 * 365 * 100);

        let next = next_fixed_rate(
            armed_at,
            Interval::new(Duration::from_nanos(1)).unwrap(),
            now,
        );

        assert!(
            next.instant() > now,
            "saturation must not produce a deadline in the past"
        );
    }

    // --- Plan -------------------------------------------------------------

    /// A one-shot plan settles after its single delivery.
    #[test]
    fn a_one_shot_plan_has_no_deadline_after_its_tick() {
        let now = Instant::now();
        let plan = Plan::Once(FireAt::after(now, SECOND));

        assert_eq!(plan.first_deadline().instant(), now + SECOND);
        assert!(plan.deadline_after_tick(now + SECOND).is_none());
    }

    /// Both cadences start one interval after arming; they diverge only once a
    /// tick has been delivered late.
    #[test]
    fn both_cadences_start_one_interval_after_arming() {
        let armed_at = Instant::now();

        for cadence in [Cadence::FixedRate, Cadence::FixedDelay] {
            let plan = Plan::Repeat {
                armed_at,
                interval: interval(10),
                cadence,
            };

            assert_eq!(
                plan.first_deadline().instant(),
                armed_at + Duration::from_secs(10),
                "{cadence:?} must wait one whole interval for its first tick"
            );
        }
    }

    /// The difference between the modes, stated as an assertion. Delivering a
    /// tick two and a half intervals late puts fixed-rate back on the grid and
    /// pushes fixed-delay off it.
    #[test]
    fn a_late_tick_separates_the_two_cadences() {
        let armed_at = Instant::now();
        let late = armed_at + Duration::from_secs(25);

        let fixed_rate = Plan::Repeat {
            armed_at,
            interval: interval(10),
            cadence: Cadence::FixedRate,
        }
        .deadline_after_tick(late)
        .expect("a repeating plan always has a next deadline");

        let fixed_delay = Plan::Repeat {
            armed_at,
            interval: interval(10),
            cadence: Cadence::FixedDelay,
        }
        .deadline_after_tick(late)
        .expect("a repeating plan always has a next deadline");

        assert_eq!(
            fixed_rate.instant(),
            armed_at + Duration::from_secs(30),
            "fixed-rate returns to the grid"
        );
        assert_eq!(
            fixed_delay.instant(),
            armed_at + Duration::from_secs(35),
            "fixed-delay measures the interval from when the send finished"
        );
    }

    // --- ScheduledSendOutcome ---------------------------------------------

    /// Cancelling and being abandoned must stay distinct: one is something the
    /// caller did, the other is something that happened to it.
    #[test]
    fn cancellation_and_abandonment_are_different_outcomes() {
        assert_ne!(
            ScheduledSendOutcome::Cancelled,
            ScheduledSendOutcome::Abandoned
        );
    }
}
