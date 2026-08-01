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

//! Supervision strategies for deciding how to respond to child terminations.
//!
//! This module provides the Erlang/OTP supervision strategies. A strategy says
//! *which* children a supervisor restarts when one of them terminates; the
//! child's [`RestartPolicy`] says *whether* a given termination warrants a
//! restart at all. The two are consulted together.
//!
//! **The framework carries these out.** A supervisor set to one of these
//! strategies stops and rebuilds the children the strategy names, in order,
//! after a backoff, without a line of handler code. Set it with
//! [`ActorConfig::with_supervision_strategy`], and register children through
//! [`supervise_with`] or [`supervise_deferred`] so the supervisor holds a
//! blueprint it can rebuild them from.
//!
//! # Strategies
//!
//! - [`SupervisionStrategy::OneForOne`]: Restart only the failed child
//! - [`SupervisionStrategy::OneForAll`]: Restart all children when one fails
//! - [`SupervisionStrategy::RestForOne`]: Restart the failed child and all children started after it
//!
//! # Example
//!
//! ```rust,ignore
//! use acton_reactive::prelude::*;
//!
//! // Interdependent children: one failing makes the others' state suspect,
//! // so the supervisor stops and rebuilds the whole set.
//! let config = ActorConfig::new_with_name("pipeline")?
//!     .with_supervision_strategy(SupervisionStrategy::OneForAll);
//! ```
//!
//! # Applying a strategy by hand
//!
//! [`SupervisionStrategy::decide`] is public and is what the engine itself
//! calls, so a supervisor with its own reasons — children it built outside the
//! blueprint paths, or a decision that depends on state only its handler can
//! see — can still consult a strategy from a `ChildTerminated` handler and act
//! on the answer.
//!
//! That is now the exception rather than the way strategies are used, and it
//! does not compose with the engine: a child registered through
//! [`supervise_with`] or [`supervise_deferred`] is already being restarted, so
//! a handler that also restarts it brings it back twice.
//!
//! [`ActorConfig::with_supervision_strategy`]: crate::actor::ActorConfig::with_supervision_strategy
//! [`supervise_with`]: crate::common::ActorHandle::supervise_with
//! [`supervise_deferred`]: crate::actor::ManagedActor::supervise_deferred

use serde::{Deserialize, Serialize};

use crate::message::ChildTerminated;

/// Supervision strategy for deciding which children to restart.
///
/// When a supervised child actor terminates, the supervision strategy determines
/// which children should be restarted (if any). This decision is made in
/// conjunction with the child's [`RestartPolicy`](crate::actor::RestartPolicy).
///
/// The strategy consulted is the **supervisor's**, not the child's. That is
/// worth stating outright, because
/// [`ActorConfig::for_supervised_child`](crate::actor::ActorConfig::for_supervised_child)
/// builds a *child's* configuration and a reader may reasonably expect a
/// strategy set there to govern what happens when that child dies. It does not:
/// what to do about a failure is the supervising actor's policy.
///
/// The supervisor carries the strategy out itself — stopping and rebuilding the
/// children it names, in order, after a backoff. It governs children the
/// supervisor holds a blueprint for; a child adopted through the legacy
/// `supervise()` path is one it has no recipe for rebuilding, so no strategy
/// applies to it and it is left down.
///
/// These strategies follow Erlang/OTP supervision patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SupervisionStrategy {
    /// Restart only the terminated child actor.
    ///
    /// When a child terminates, the decision is to restart only that specific
    /// child (if its restart policy allows). Other children are unaffected.
    ///
    /// This is the most common strategy and is appropriate when children
    /// are independent and their failures don't affect each other.
    ///
    /// # Example
    ///
    /// If you have workers A, B, C and B crashes:
    /// - Only B is restarted
    /// - A and C continue running unaffected
    #[default]
    OneForOne,

    /// Restart all children when any child terminates.
    ///
    /// When any child terminates (and its restart policy allows a restart),
    /// the decision is to stop and restart all children. This ensures all
    /// children start from a consistent state.
    ///
    /// This is appropriate when children are interdependent and one child's
    /// failure could leave others in an inconsistent state.
    ///
    /// # Example
    ///
    /// If you have workers A, B, C and B crashes:
    /// - A and C are stopped
    /// - All three (A, B, C) are restarted
    OneForAll,

    /// Restart the terminated child and all children started after it.
    ///
    /// When a child terminates (and its restart policy allows a restart),
    /// the decision is to stop and restart that child and all children that
    /// were started after it, in start order.
    ///
    /// This is appropriate when children have sequential dependencies,
    /// where later children depend on earlier ones but not vice versa.
    ///
    /// # Example
    ///
    /// If you have workers A, B, C (started in that order) and B crashes:
    /// - C is stopped (started after B)
    /// - B and C are restarted (in order: B, then C)
    /// - A continues running unaffected
    RestForOne,
}

/// The decision made by a supervisor after evaluating a child termination.
///
/// This enum represents the possible actions a supervisor can take when
/// one of its children terminates. The actual decision depends on both
/// the supervision strategy and the child's restart policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisionDecision {
    /// Restart only the specified child.
    RestartChild,

    /// Restart all supervised children.
    RestartAll,

    /// Restart from the specified child index onwards.
    ///
    /// The value is the index in the children list from which to restart.
    RestartFrom(usize),

    /// Do not restart; the child should remain terminated.
    ///
    /// This happens when the restart policy indicates no restart
    /// (e.g., Temporary policy or Normal termination with Transient policy).
    NoRestart,

    /// Escalate the failure to the supervisor's parent.
    ///
    /// This happens when restart limits have been exceeded or when
    /// the supervisor cannot handle the failure.
    Escalate,
}

impl SupervisionStrategy {
    /// Determine what action to take when a child terminates.
    ///
    /// This method evaluates the supervision strategy in combination with
    /// the child's restart policy and termination reason to decide what
    /// action the supervisor should take.
    ///
    /// # Arguments
    ///
    /// * `notification` - The `ChildTerminated` message containing details
    ///   about the terminated child, including its ID, termination reason,
    ///   and restart policy.
    /// * `child_index` - The index of the terminated child in the supervisor's
    ///   ordered list of children (used for `RestForOne` strategy).
    ///
    /// # Returns
    ///
    /// A `SupervisionDecision` indicating what action the supervisor should take.
    #[must_use]
    pub const fn decide(
        &self,
        notification: &ChildTerminated,
        child_index: usize,
    ) -> SupervisionDecision {
        // First, check if restart is allowed by the policy
        if !notification.restart_policy.should_restart(&notification.reason) {
            return SupervisionDecision::NoRestart;
        }

        // Apply the supervision strategy
        match self {
            Self::OneForOne => SupervisionDecision::RestartChild,
            Self::OneForAll => SupervisionDecision::RestartAll,
            Self::RestForOne => SupervisionDecision::RestartFrom(child_index),
        }
    }

    /// Check if this strategy requires stopping other children before restart.
    ///
    /// Returns `true` for `OneForAll` and `RestForOne` strategies, which
    /// require stopping additional children before performing restarts.
    #[must_use]
    pub const fn requires_group_restart(&self) -> bool {
        matches!(self, Self::OneForAll | Self::RestForOne)
    }
}

impl std::fmt::Display for SupervisionStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OneForOne => write!(f, "one_for_one"),
            Self::OneForAll => write!(f, "one_for_all"),
            Self::RestForOne => write!(f, "rest_for_one"),
        }
    }
}

impl std::fmt::Display for SupervisionDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RestartChild => write!(f, "restart child"),
            Self::RestartAll => write!(f, "restart all children"),
            Self::RestartFrom(idx) => write!(f, "restart from child index {idx}"),
            Self::NoRestart => write!(f, "no restart"),
            Self::Escalate => write!(f, "escalate to parent"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::{RestartPolicy, TerminationReason};
    use acton_ern::Ern;

    fn make_notification(policy: RestartPolicy, reason: TerminationReason) -> ChildTerminated {
        ChildTerminated::new(Ern::with_root("test-child").unwrap(), reason, policy)
    }

    #[test]
    fn one_for_one_restarts_single_child_on_panic() {
        let strategy = SupervisionStrategy::OneForOne;
        let notification = make_notification(
            RestartPolicy::Permanent,
            TerminationReason::Panic("test".into()),
        );

        let decision = strategy.decide(&notification, 0);
        assert_eq!(decision, SupervisionDecision::RestartChild);
    }

    #[test]
    fn one_for_all_restarts_all_children_on_panic() {
        let strategy = SupervisionStrategy::OneForAll;
        let notification = make_notification(
            RestartPolicy::Permanent,
            TerminationReason::Panic("test".into()),
        );

        let decision = strategy.decide(&notification, 0);
        assert_eq!(decision, SupervisionDecision::RestartAll);
    }

    #[test]
    fn rest_for_one_restarts_from_index() {
        let strategy = SupervisionStrategy::RestForOne;
        let notification = make_notification(
            RestartPolicy::Permanent,
            TerminationReason::Panic("test".into()),
        );

        let decision = strategy.decide(&notification, 2);
        assert_eq!(decision, SupervisionDecision::RestartFrom(2));
    }

    #[test]
    fn temporary_policy_prevents_restart_for_all_strategies() {
        let notification = make_notification(
            RestartPolicy::Temporary,
            TerminationReason::Panic("test".into()),
        );

        for strategy in [
            SupervisionStrategy::OneForOne,
            SupervisionStrategy::OneForAll,
            SupervisionStrategy::RestForOne,
        ] {
            let decision = strategy.decide(&notification, 0);
            assert_eq!(decision, SupervisionDecision::NoRestart);
        }
    }

    #[test]
    fn transient_policy_no_restart_on_normal_termination() {
        let notification = make_notification(RestartPolicy::Transient, TerminationReason::Normal);

        let decision = SupervisionStrategy::OneForOne.decide(&notification, 0);
        assert_eq!(decision, SupervisionDecision::NoRestart);
    }

    #[test]
    fn transient_policy_restarts_on_panic() {
        let notification = make_notification(
            RestartPolicy::Transient,
            TerminationReason::Panic("test".into()),
        );

        let decision = SupervisionStrategy::OneForOne.decide(&notification, 0);
        assert_eq!(decision, SupervisionDecision::RestartChild);
    }

    #[test]
    fn parent_shutdown_never_restarts() {
        let notification =
            make_notification(RestartPolicy::Permanent, TerminationReason::ParentShutdown);

        let decision = SupervisionStrategy::OneForOne.decide(&notification, 0);
        assert_eq!(decision, SupervisionDecision::NoRestart);
    }

    #[test]
    fn requires_group_restart_for_one_for_all() {
        assert!(!SupervisionStrategy::OneForOne.requires_group_restart());
        assert!(SupervisionStrategy::OneForAll.requires_group_restart());
        assert!(SupervisionStrategy::RestForOne.requires_group_restart());
    }

    #[test]
    fn default_strategy_is_one_for_one() {
        assert_eq!(
            SupervisionStrategy::default(),
            SupervisionStrategy::OneForOne
        );
    }
}
