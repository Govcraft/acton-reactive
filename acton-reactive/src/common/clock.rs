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

//! The passage of time, as a seam.
//!
//! Scheduled sends are the crate's first feature whose whole observable
//! behaviour is *when* something happens. Testing that against the real clock
//! means sleeping, and sleeping in a test is the habit this codebase has spent
//! considerable effort removing: it is slow when it works and flaky when it
//! does not.
//!
//! # Why not `tokio::time::pause`
//!
//! Tokio's own virtual time would be the obvious answer, and it does not fit:
//!
//! * it requires the `test-util` feature, which the workspace does not enable;
//! * it requires a current-thread runtime, and `#[acton_test]` builds a
//!   `new_multi_thread` one;
//! * it is process-global, so it cannot be applied to one actor under test
//!   while the rest of the system runs on real time.
//!
//! An injected clock has none of those constraints, and it hands the same
//! capability to users: anyone writing an actor with a scheduled send has
//! exactly the problem this module solves, so [`ManualClock`] is public rather
//! than a test fixture hidden behind `#[cfg(test)]`.
//!
//! # Why `timer` is synchronous and returns a future
//!
//! The interesting subtlety. An `async fn sleep_until` would register the
//! timer on first poll, which happens on the spawned task at some unspecified
//! later moment. A test that armed a timer and then advanced the clock would
//! race that first poll, and whether the timer fired would depend on scheduler
//! luck.
//!
//! [`Clock::timer`] is an ordinary function that *arms* the timer and hands
//! back the future for waiting on it. [`ManualClock`] subscribes to its watch
//! channel inside that call, on the caller's thread, so by the time
//! [`ActorHandle::send_after`](crate::common::ActorHandle::send_after) returns
//! the timer is registered as a fact. That is what makes the scheduled-send
//! tests deterministic rather than merely usually-correct.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;

/// A future that resolves when an armed timer comes due.
///
/// Boxed because [`Clock`] is used as a trait object: a scheduled send holds
/// `Arc<dyn Clock>` so that the clock can be swapped without any of the types
/// around it becoming generic.
pub type Timer = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// The passage of time, as scheduled sends see it.
///
/// Implement this to drive scheduling from something other than the wall
/// clock. The crate ships two implementations: [`SystemClock`], which every
/// handle uses unless told otherwise, and [`ManualClock`], which advances only
/// when you say so.
///
/// # Contract
///
/// * [`now`](Self::now) must be monotonic — never going backwards — because
///   the fixed-rate scheduler computes deadlines by division against it and a
///   clock that ran backwards would emit a burst of catch-up ticks.
/// * [`timer`](Self::timer) must register the deadline *before it returns*,
///   not on first poll of the returned future. Callers rely on a timer being
///   armed the moment the call completes.
/// * A deadline at or before `now` must resolve immediately rather than
///   waiting for the next scheduling quantum.
pub trait Clock: fmt::Debug + Send + Sync + 'static {
    /// The current instant.
    fn now(&self) -> Instant;

    /// Arms a timer for `deadline` and returns the future that resolves when
    /// it comes due.
    ///
    /// Arming happens here, synchronously. See the module documentation for
    /// why that is not an implementation detail.
    fn timer(&self, deadline: Instant) -> Timer;
}

/// The wall clock, backed by [`Instant`] and `tokio::time`.
///
/// What every [`ActorHandle`](crate::common::ActorHandle) schedules against
/// unless [`with_clock`](crate::common::ActorHandle::with_clock) says
/// otherwise.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemClock;

impl Clock for SystemClock {
    #[inline]
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn timer(&self, deadline: Instant) -> Timer {
        // `sleep_until` is created here rather than inside the async block, so
        // that the timer is registered with tokio's driver before this returns
        // — the same guarantee `ManualClock` gives, for the same reason.
        let sleep = tokio::time::sleep_until(deadline.into());
        Box::pin(sleep)
    }
}

/// A clock that only moves when you move it.
///
/// Lets a test assert *when* a scheduled send fires without waiting for it to.
/// Hand one to a handle with
/// [`with_clock`](crate::common::ActorHandle::with_clock), arm a schedule, and
/// drive it with [`advance`](Self::advance):
///
/// ```
/// # use std::sync::Arc;
/// # use std::time::Duration;
/// # use acton_reactive::prelude::*;
/// # async fn example(handle: ActorHandle) {
/// # #[acton_message] struct Tick;
/// let clock = Arc::new(ManualClock::new());
/// let scheduled = handle
///     .with_clock(clock.clone())
///     .send_after(Tick, Duration::from_secs(60));
///
/// assert_eq!(clock.armed(), 1, "the timer is armed before `send_after` returns");
///
/// clock.advance(Duration::from_secs(60));
/// assert_eq!(scheduled.outcome().await, ScheduledSendOutcome::Delivered);
/// # }
/// ```
///
/// Cheap to clone, and every clone shares one timeline — clone it rather than
/// wrapping it in a second `Arc` if you need it in two places.
///
/// # How it wakes sleepers
///
/// A `watch` channel carrying the current instant. Arming subscribes;
/// [`advance`](Self::advance) stores a new instant, which wakes every waiter
/// at once and lets each decide for itself whether it is due. There is no
/// waker registry to keep consistent, and no ordering to get wrong: a waiter
/// that is late to look still sees the advanced value and fires immediately.
#[derive(Debug, Clone)]
pub struct ManualClock {
    now: Arc<watch::Sender<Instant>>,
}

impl ManualClock {
    /// A clock starting at the current wall-clock instant.
    ///
    /// The starting point is arbitrary — nothing may compare a `ManualClock`
    /// instant against a real one — but starting from `Instant::now()` keeps
    /// durations in logs recognisable.
    #[must_use]
    pub fn new() -> Self {
        Self::starting_at(Instant::now())
    }

    /// A clock starting at `start`.
    #[must_use]
    pub fn starting_at(start: Instant) -> Self {
        Self {
            now: Arc::new(watch::Sender::new(start)),
        }
    }

    /// Moves time forward by `by`, waking every timer that has come due.
    ///
    /// Returns once the new instant is visible to every waiter. It does not
    /// wait for what those waiters go on to *do* — for that, use a barrier on
    /// the thing being scheduled, such as
    /// [`ScheduledSend::wait_for_deliveries`](crate::common::ScheduledSend::wait_for_deliveries).
    ///
    /// Saturates rather than panicking if `by` would run the clock past the
    /// end of [`Instant`]'s range.
    pub fn advance(&self, by: Duration) {
        self.now.send_modify(|now| {
            *now = now.checked_add(by).unwrap_or(*now);
        });
    }

    /// Moves time forward to `deadline`, or does nothing if it has passed.
    ///
    /// Time never goes backwards, because the fixed-rate scheduler would
    /// respond to that by emitting catch-up ticks.
    pub fn advance_to(&self, deadline: Instant) {
        self.now.send_modify(|now| {
            if deadline > *now {
                *now = deadline;
            }
        });
    }

    /// How many timers are currently armed against this clock.
    ///
    /// Exact the moment an arming call returns, which is what makes it usable
    /// as an assertion rather than a hint: see the module documentation on why
    /// [`Clock::timer`] arms synchronously.
    #[must_use]
    pub fn armed(&self) -> usize {
        self.now.receiver_count()
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for ManualClock {
    #[inline]
    fn now(&self) -> Instant {
        *self.now.borrow()
    }

    fn timer(&self, deadline: Instant) -> Timer {
        // Load-bearing: subscribing *here* rather than inside the async block
        // is what makes the timer armed by the time this function returns.
        let mut observer = self.now.subscribe();

        Box::pin(async move {
            loop {
                // Scoped so the watch guard is released before the await
                // below. Holding it across one would deadlock every writer.
                if *observer.borrow_and_update() >= deadline {
                    return;
                }

                if observer.changed().await.is_err() {
                    // The clock itself was dropped. Nothing will ever advance
                    // it, so this deadline can never arrive; parking forever
                    // would leak the task and hang whatever awaits it.
                    return;
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deadline that has already passed must not wait for the next tick of
    /// anything. Scheduling code relies on this to make "send at a time in the
    /// past" mean "send now".
    #[tokio::test]
    async fn a_system_timer_for_a_past_deadline_resolves_immediately() {
        let clock = SystemClock;
        let deadline = clock
            .now()
            .checked_sub(Duration::from_mins(1))
            .expect("a minute ago");

        tokio::time::timeout(Duration::from_secs(5), clock.timer(deadline))
            .await
            .expect("a past deadline must resolve at once, not wait");
    }

    /// The synchronous-arming contract, asserted directly. If `timer` ever
    /// moved registration into the future's first poll, this is the test that
    /// would notice — and every scheduled-send test would become racy.
    #[test]
    fn a_manual_timer_is_armed_before_timer_returns() {
        let clock = ManualClock::new();
        assert_eq!(clock.armed(), 0, "nothing is armed yet");

        let _timer = clock.timer(clock.now() + Duration::from_secs(1));

        assert_eq!(
            clock.armed(),
            1,
            "arming must have happened inside `timer`, with no poll in between"
        );
    }

    /// Dropping the future disarms, so a cancelled schedule does not leave the
    /// clock believing it still has work.
    #[test]
    fn dropping_a_manual_timer_disarms_it() {
        let clock = ManualClock::new();
        let timer = clock.timer(clock.now() + Duration::from_secs(1));
        assert_eq!(clock.armed(), 1);

        drop(timer);

        assert_eq!(clock.armed(), 0);
    }

    /// The core of the abstraction: time passes only when told, and then the
    /// timer fires without any real waiting.
    #[tokio::test]
    async fn a_manual_timer_fires_only_once_the_clock_reaches_its_deadline() {
        let clock = ManualClock::new();
        let deadline = clock.now() + Duration::from_secs(10);
        let mut timer = clock.timer(deadline);

        clock.advance(Duration::from_secs(9));
        assert!(
            futures::poll!(&mut timer).is_pending(),
            "nine seconds is not ten"
        );

        clock.advance(Duration::from_secs(1));
        tokio::time::timeout(Duration::from_secs(5), timer)
            .await
            .expect("the deadline has been reached, so the timer must resolve");
    }

    /// Landing exactly on the deadline counts as due. The scheduler's
    /// grid arithmetic produces exact hits constantly, so an off-by-one here
    /// would show up as every tick firing one interval late.
    #[tokio::test]
    async fn a_deadline_reached_exactly_is_due() {
        let clock = ManualClock::new();
        let timer = clock.timer(clock.now() + Duration::from_secs(5));

        clock.advance(Duration::from_secs(5));

        tokio::time::timeout(Duration::from_secs(5), timer)
            .await
            .expect("an exactly-reached deadline is due, not pending");
    }

    /// One advance past several deadlines wakes all of them, not just the
    /// earliest.
    #[tokio::test]
    async fn one_advance_wakes_every_timer_it_passes() {
        let clock = ManualClock::new();
        let start = clock.now();
        let timers = vec![
            clock.timer(start + Duration::from_secs(1)),
            clock.timer(start + Duration::from_secs(2)),
            clock.timer(start + Duration::from_secs(3)),
        ];
        assert_eq!(clock.armed(), 3);

        clock.advance(Duration::from_secs(3));

        tokio::time::timeout(Duration::from_secs(5), futures::future::join_all(timers))
            .await
            .expect("every timer the advance passed must resolve");
    }

    /// A timer whose clock has gone away can never come due. Resolving is the
    /// only alternative to leaking the task forever.
    #[tokio::test]
    async fn a_timer_resolves_when_its_clock_is_dropped() {
        let clock = ManualClock::new();
        let timer = clock.timer(clock.now() + Duration::from_hours(1));

        drop(clock);

        tokio::time::timeout(Duration::from_secs(5), timer)
            .await
            .expect("a dropped clock must release its waiters rather than hang them");
    }

    /// Clones share one timeline; advancing through any of them is advancing
    /// the clock.
    #[test]
    fn clones_share_one_timeline() {
        let clock = ManualClock::new();
        let other = clock.clone();
        let start = clock.now();

        other.advance(Duration::from_secs(7));

        assert_eq!(clock.now(), start + Duration::from_secs(7));
    }

    /// Time is monotonic. The fixed-rate scheduler answers a backwards jump
    /// with a burst of catch-up ticks, so the clock must refuse to make one.
    #[test]
    fn advance_to_never_moves_time_backwards() {
        let clock = ManualClock::new();
        let start = clock.now();
        clock.advance(Duration::from_secs(10));

        clock.advance_to(start);

        assert_eq!(
            clock.now(),
            start + Duration::from_secs(10),
            "a deadline in the past must leave the clock where it was"
        );
    }

    /// An absurd advance must saturate, not abort the process.
    #[test]
    fn advancing_past_the_end_of_time_saturates() {
        let clock = ManualClock::new();
        let before = clock.now();

        clock.advance(Duration::MAX);

        assert!(clock.now() >= before, "the clock must survive an overflow");
    }
}
