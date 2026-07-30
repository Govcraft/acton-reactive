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

//! Value types describing a supervisor's view of its children.
//!
//! These are the small, copyable values the supervision subsystem passes
//! around: which incarnation of a child is current ([`RestartGeneration`]),
//! where a child sits in its supervisor's start order ([`ChildIndex`]), and how
//! long to wait before trying again ([`BackoffDelay`]).
//!
//! All three are plain values with no invariants beyond those of the primitive
//! they wrap, so they are cheap to copy and safe to compare and order.

use std::fmt;
use std::time::Duration;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
