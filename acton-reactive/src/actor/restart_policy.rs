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

//! Restart policies for supervised actors.
//!
//! This module provides Erlang/Elixir-style restart policies that determine
//! how actors should be handled when they terminate. These policies are
//! evaluated by the supervision system to decide whether to restart a child
//! actor.
//!
//! # Policies
//!
//! - [`RestartPolicy::Permanent`]: Always restart (except during parent shutdown)
//! - [`RestartPolicy::Temporary`]: Never restart
//! - [`RestartPolicy::Transient`]: Restart only on abnormal termination

use serde::{Deserialize, Serialize};

/// Restart policy for supervised actors.
///
/// Determines whether and when an actor should be restarted after termination.
/// These policies follow Erlang/Elixir supervision patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum RestartPolicy {
    /// Always restart the actor when it terminates.
    ///
    /// The actor will be restarted regardless of whether it terminated normally
    /// or abnormally. This is appropriate for actors that must always be running.
    ///
    /// **Exception**: The actor will NOT be restarted during a parent-initiated
    /// cascading shutdown.
    #[default]
    Permanent,

    /// Never restart the actor when it terminates.
    ///
    /// The actor will not be restarted under any circumstances. This is appropriate
    /// for actors that represent one-time operations or where the caller handles
    /// failure explicitly.
    Temporary,

    /// Restart only on abnormal termination.
    ///
    /// The actor will be restarted if it terminated due to a panic or error,
    /// but not if it terminated normally (via shutdown signal) or due to
    /// parent shutdown.
    Transient,
}

/// Reason for actor termination.
///
/// This enum captures the different ways an actor can terminate, enabling
/// the restart policy to make informed decisions about whether to restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminationReason {
    /// Normal graceful shutdown via `SystemSignal::Terminate`.
    ///
    /// The actor received a shutdown signal and terminated cleanly.
    Normal,

    /// Actor panicked during message handling or lifecycle hook.
    ///
    /// The panic was caught and the actor terminated abnormally.
    /// The string contains the panic message if available.
    Panic(String),

    /// Actor inbox closed unexpectedly.
    ///
    /// All senders were dropped without a formal shutdown signal.
    InboxClosed,

    /// Parent-initiated cascading shutdown.
    ///
    /// The parent actor is shutting down and terminating all children.
    /// This should never trigger a restart regardless of policy.
    ParentShutdown,
}

impl RestartPolicy {
    /// Determine if an actor should be restarted based on this policy and termination reason.
    ///
    /// # Arguments
    ///
    /// * `reason` - The reason the actor terminated
    ///
    /// # Returns
    ///
    /// `true` if the actor should be restarted, `false` otherwise
    #[must_use]
    pub const fn should_restart(&self, reason: &TerminationReason) -> bool {
        // Never restart during parent shutdown, regardless of policy
        if matches!(reason, TerminationReason::ParentShutdown) {
            return false;
        }

        match self {
            // Permanent: always restart
            Self::Permanent => true,

            // Temporary: never restart
            Self::Temporary => false,

            // Transient: restart only on abnormal termination
            Self::Transient => !matches!(reason, TerminationReason::Normal),
        }
    }
}

impl std::fmt::Display for RestartPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Permanent => write!(f, "permanent"),
            Self::Temporary => write!(f, "temporary"),
            Self::Transient => write!(f, "transient"),
        }
    }
}

impl std::fmt::Display for TerminationReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Normal => write!(f, "normal shutdown"),
            Self::Panic(msg) => write!(f, "panic: {msg}"),
            Self::InboxClosed => write!(f, "inbox closed"),
            Self::ParentShutdown => write!(f, "parent shutdown"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permanent_restarts_on_panic() {
        let policy = RestartPolicy::Permanent;
        assert!(policy.should_restart(&TerminationReason::Panic("test".to_string())));
    }

    #[test]
    fn permanent_restarts_on_normal() {
        let policy = RestartPolicy::Permanent;
        assert!(policy.should_restart(&TerminationReason::Normal));
    }

    #[test]
    fn permanent_does_not_restart_on_parent_shutdown() {
        let policy = RestartPolicy::Permanent;
        assert!(!policy.should_restart(&TerminationReason::ParentShutdown));
    }

    #[test]
    fn temporary_never_restarts() {
        let policy = RestartPolicy::Temporary;
        assert!(!policy.should_restart(&TerminationReason::Normal));
        assert!(!policy.should_restart(&TerminationReason::Panic("test".to_string())));
        assert!(!policy.should_restart(&TerminationReason::InboxClosed));
        assert!(!policy.should_restart(&TerminationReason::ParentShutdown));
    }

    #[test]
    fn transient_restarts_on_panic() {
        let policy = RestartPolicy::Transient;
        assert!(policy.should_restart(&TerminationReason::Panic("test".to_string())));
    }

    #[test]
    fn transient_restarts_on_inbox_closed() {
        let policy = RestartPolicy::Transient;
        assert!(policy.should_restart(&TerminationReason::InboxClosed));
    }

    #[test]
    fn transient_does_not_restart_on_normal() {
        let policy = RestartPolicy::Transient;
        assert!(!policy.should_restart(&TerminationReason::Normal));
    }

    #[test]
    fn transient_does_not_restart_on_parent_shutdown() {
        let policy = RestartPolicy::Transient;
        assert!(!policy.should_restart(&TerminationReason::ParentShutdown));
    }
}
