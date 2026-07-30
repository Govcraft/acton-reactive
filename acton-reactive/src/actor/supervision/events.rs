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

//! Supervision events broadcast over the system broker.
//!
//! A supervisor publishes these so that actors with no relationship to the
//! child — metrics collectors, health reporters, alerting actors — can observe
//! supervision without the supervisor knowing they exist.
//!
//! Subscribing requires an [`ActorHandle`](crate::common::ActorHandle), so only
//! an actor can receive them. Code that is not an actor, such as a test body or
//! a `main`, observes a single child through its published
//! [`SupervisionStatus`](super::SupervisionStatus) instead.

use acton_ern::Ern;

use super::{BackoffDelay, RestartGeneration};
use crate::actor::{RestartStats, TerminationReason};

/// Broadcast when a supervisor takes responsibility for a child.
#[derive(Debug, Clone)]
pub struct ChildSupervised {
    /// The supervising actor.
    pub supervisor: Ern,
    /// The child now under supervision.
    pub child: Ern,
    /// Whether the supervisor can recreate this child if it terminates.
    ///
    /// `false` for children registered without a blueprint: the supervisor will
    /// be told when they terminate but cannot bring them back.
    pub restartable: bool,
}

impl ChildSupervised {
    /// Creates a supervision notification.
    #[must_use]
    pub const fn new(supervisor: Ern, child: Ern, restartable: bool) -> Self {
        Self {
            supervisor,
            child,
            restartable,
        }
    }
}

/// Broadcast when a supervisor schedules a child to be recreated.
#[derive(Debug, Clone)]
pub struct ChildRestarted {
    /// The supervising actor.
    pub supervisor: Ern,
    /// The child being restarted. Unchanged by the restart.
    pub child: Ern,
    /// The incarnation the child is being restarted *into*.
    pub generation: RestartGeneration,
    /// Why the previous incarnation terminated.
    pub reason: TerminationReason,
    /// How long the supervisor waited before recreating the child.
    pub backoff: BackoffDelay,
}

impl ChildRestarted {
    /// Creates a restart notification.
    #[must_use]
    pub const fn new(
        supervisor: Ern,
        child: Ern,
        generation: RestartGeneration,
        reason: TerminationReason,
        backoff: BackoffDelay,
    ) -> Self {
        Self {
            supervisor,
            child,
            generation,
            reason,
            backoff,
        }
    }
}

/// Broadcast when a supervisor gives up on a child that exhausted its restart
/// allowance.
#[derive(Debug, Clone)]
pub struct SupervisionEscalated {
    /// The supervisor that gave up on the child.
    pub supervisor: Ern,
    /// The child that could not be kept running.
    pub child: Ern,
    /// Restart bookkeeping at the moment the supervisor gave up.
    pub stats: RestartStats,
    /// Why the child terminated the last time.
    pub last_reason: TerminationReason,
}

impl SupervisionEscalated {
    /// Creates an escalation notification.
    #[must_use]
    pub const fn new(
        supervisor: Ern,
        child: Ern,
        stats: RestartStats,
        last_reason: TerminationReason,
    ) -> Self {
        Self {
            supervisor,
            child,
            stats,
            last_reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::traits::ActonMessage;

    fn supervisor() -> Ern {
        Ern::with_root("pool").expect("'pool' is a valid Ern root")
    }

    fn child() -> Ern {
        Ern::with_root("worker-1").expect("'worker-1' is a valid Ern root")
    }

    fn stats() -> RestartStats {
        RestartStats {
            restarts_in_window: 5,
            consecutive_restarts: 5,
            window_secs: 60,
            max_restarts: 5,
        }
    }

    #[test]
    fn child_supervised_carries_whether_the_child_can_be_recreated() {
        // `Ern::with_root` appends a generated suffix, so each identifier is
        // built once and cloned rather than reconstructed.
        let supervisor = supervisor();
        let child = child();

        let event = ChildSupervised::new(supervisor.clone(), child.clone(), true);
        assert_eq!(event.supervisor, supervisor);
        assert_eq!(event.child, child);
        assert!(event.restartable);

        let legacy = ChildSupervised::new(supervisor, child, false);
        assert!(!legacy.restartable);
    }

    #[test]
    fn child_restarted_names_the_incarnation_it_restarts_into() {
        let event = ChildRestarted::new(
            supervisor(),
            child(),
            RestartGeneration::FIRST.next(),
            TerminationReason::Panic("boom".to_string()),
            BackoffDelay::from(Duration::from_millis(200)),
        );

        assert_eq!(event.generation, RestartGeneration::FIRST.next());
        assert_eq!(event.reason, TerminationReason::Panic("boom".to_string()));
        assert_eq!(event.backoff.duration(), Duration::from_millis(200));
    }

    #[test]
    fn supervision_escalated_carries_the_final_stats_and_reason() {
        let event =
            SupervisionEscalated::new(supervisor(), child(), stats(), TerminationReason::InboxClosed);

        assert_eq!(event.stats.restarts_in_window, 5);
        assert_eq!(event.stats.max_restarts, 5);
        assert_eq!(event.last_reason, TerminationReason::InboxClosed);
    }

    #[test]
    fn every_event_is_a_broadcastable_message() {
        // The blanket impl of ActonMessage is what lets these go over the
        // broker. If a field ever stops being Send + Sync + Clone + Debug,
        // this stops compiling.
        fn assert_message<M: ActonMessage>(_: &M) {}

        assert_message(&ChildSupervised::new(supervisor(), child(), true));
        assert_message(&ChildRestarted::new(
            supervisor(),
            child(),
            RestartGeneration::FIRST,
            TerminationReason::Normal,
            BackoffDelay::NONE,
        ));
        assert_message(&SupervisionEscalated::new(
            supervisor(),
            child(),
            stats(),
            TerminationReason::Normal,
        ));
    }
}
