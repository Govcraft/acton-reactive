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

//! Supervision strategies for managing actor restarts.
//!
//! This module provides Erlang/OTP-style supervision strategies that determine
//! how a supervisor should respond when one of its supervised child actors
//! terminates. These strategies work in conjunction with [`RestartPolicy`] to
//! provide comprehensive fault-tolerance capabilities.
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
//! // Configure a supervisor with OneForOne strategy
//! let supervisor_config = ActorConfig::new(
//!     Ern::with_root("supervisor")?,
//!     None,
//!     None,
//! )?
//! .with_supervision_strategy(SupervisionStrategy::OneForOne);
//! ```

use serde::{Deserialize, Serialize};

use crate::message::ChildTerminated;

/// Supervision strategy for managing child actor restarts.
///
/// When a supervised child actor terminates, the supervision strategy determines
/// which children should be restarted (if any). This decision is made in
/// conjunction with the child's [`RestartPolicy`].
///
/// These strategies follow Erlang/OTP supervision patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SupervisionStrategy {
    /// Restart only the terminated child actor.
    ///
    /// When a child terminates, only that specific child is restarted
    /// (if its restart policy allows). Other children are unaffected.
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
    /// all children are stopped and restarted. This ensures all children
    /// start from a consistent state.
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
    /// that child and all children that were started after it are stopped
    /// and restarted, in start order.
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
