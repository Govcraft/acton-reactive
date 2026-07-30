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

//! A supervisor's published view of one supervised child.
//!
//! A supervisor owns its children's bookkeeping privately and publishes
//! [`SupervisionStatus`] snapshots outward. Callers read the snapshot rather
//! than querying the supervisor, so observing a child never costs a message
//! round trip and never blocks the supervisor's own task.

use std::fmt;

use acton_ern::Ern;

use super::RestartGeneration;
use crate::common::ActorHandle;

/// Lifecycle state of a supervised child, as published by its supervisor.
///
/// The states form a cycle for a child that keeps failing and being restarted:
/// `Running` to `RestartPending` to `Restarting` to `Running`. The remaining
/// states are terminal for the current registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SupervisionState {
    /// The child has been created but has not yet been recorded as running.
    Starting,

    /// The child is running and processing messages.
    Running,

    /// The child terminated and a restart is scheduled, waiting out its backoff.
    RestartPending,

    /// A replacement for the child is being created.
    Restarting,

    /// The child terminated and will not be restarted.
    ///
    /// Either its [`RestartPolicy`](crate::actor::RestartPolicy) forbids a
    /// restart, or it was registered without a blueprint.
    Down,

    /// The child exhausted its restart allowance and its supervisor gave up.
    Escalated,

    /// The child was removed from supervision.
    Retired,
}

impl SupervisionState {
    /// Returns `true` when the supervisor may still bring this child back.
    ///
    /// [`Down`](SupervisionState::Down), [`Escalated`](SupervisionState::Escalated)
    /// and [`Retired`](SupervisionState::Retired) are terminal; every other
    /// state either is running or leads back to running.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Down | Self::Escalated | Self::Retired)
    }
}

impl fmt::Display for SupervisionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::RestartPending => "restart_pending",
            Self::Restarting => "restarting",
            Self::Down => "down",
            Self::Escalated => "escalated",
            Self::Retired => "retired",
        };
        f.write_str(text)
    }
}

/// A supervisor's published view of one supervised child.
///
/// A snapshot, not a live view: it describes the child at the moment the
/// supervisor published it.
///
/// The [`handle`](SupervisionStatus::handle) belongs to the incarnation named
/// by [`generation`](SupervisionStatus::generation). A restart keeps the
/// child's [`Ern`] but replaces its mailbox, so a handle from an earlier
/// generation is stale and will not reach the current incarnation.
#[derive(Debug, Clone)]
pub struct SupervisionStatus {
    child: Ern,
    handle: Option<ActorHandle>,
    generation: RestartGeneration,
    state: SupervisionState,
    restarts_in_window: usize,
}

impl SupervisionStatus {
    /// Creates a status snapshot.
    ///
    /// Supervisors build these as children change state. It is public so that
    /// callers can construct the fixtures their own tests need.
    #[must_use]
    pub const fn new(
        child: Ern,
        handle: Option<ActorHandle>,
        generation: RestartGeneration,
        state: SupervisionState,
        restarts_in_window: usize,
    ) -> Self {
        Self {
            child,
            handle,
            generation,
            state,
            restarts_in_window,
        }
    }

    /// The identifier of the supervised child.
    ///
    /// Stable across restarts.
    #[must_use]
    pub const fn child(&self) -> &Ern {
        &self.child
    }

    /// The handle for the incarnation named by
    /// [`generation`](SupervisionStatus::generation), or `None` while the child
    /// is not running.
    #[must_use]
    pub const fn handle(&self) -> Option<&ActorHandle> {
        self.handle.as_ref()
    }

    /// Which incarnation of the child this snapshot describes.
    #[must_use]
    pub const fn generation(&self) -> RestartGeneration {
        self.generation
    }

    /// What the child was doing when the supervisor published this snapshot.
    #[must_use]
    pub const fn state(&self) -> SupervisionState {
        self.state
    }

    /// How many restarts the supervisor has recorded inside its current window.
    #[must_use]
    pub const fn restarts_in_window(&self) -> usize {
        self.restarts_in_window
    }
}

impl fmt::Display for SupervisionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "child '{}' is {} at {} ({} restarts in window)",
            self.child, self.state, self.generation, self.restarts_in_window
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child() -> Ern {
        Ern::with_root("worker").expect("'worker' is a valid Ern root")
    }

    #[test]
    fn accessors_return_what_was_supplied() {
        // `Ern::with_root` appends a generated suffix, so the identifier is
        // built once and cloned rather than reconstructed.
        let child = child();
        let snapshot = SupervisionStatus::new(
            child.clone(),
            None,
            RestartGeneration::FIRST.next(),
            SupervisionState::Running,
            3,
        );

        assert_eq!(snapshot.child(), &child);
        assert!(snapshot.handle().is_none());
        assert_eq!(snapshot.generation(), RestartGeneration::FIRST.next());
        assert_eq!(snapshot.state(), SupervisionState::Running);
        assert_eq!(snapshot.restarts_in_window(), 3);
    }

    #[test]
    fn terminal_states_are_the_ones_a_supervisor_cannot_recover_from() {
        for state in [
            SupervisionState::Down,
            SupervisionState::Escalated,
            SupervisionState::Retired,
        ] {
            assert!(state.is_terminal(), "{state} should be terminal");
        }

        for state in [
            SupervisionState::Starting,
            SupervisionState::Running,
            SupervisionState::RestartPending,
            SupervisionState::Restarting,
        ] {
            assert!(!state.is_terminal(), "{state} should not be terminal");
        }
    }

    #[test]
    fn every_state_displays_in_snake_case() {
        assert_eq!(SupervisionState::Starting.to_string(), "starting");
        assert_eq!(SupervisionState::Running.to_string(), "running");
        assert_eq!(SupervisionState::RestartPending.to_string(), "restart_pending");
        assert_eq!(SupervisionState::Restarting.to_string(), "restarting");
        assert_eq!(SupervisionState::Down.to_string(), "down");
        assert_eq!(SupervisionState::Escalated.to_string(), "escalated");
        assert_eq!(SupervisionState::Retired.to_string(), "retired");
    }

    #[test]
    fn status_displays_child_state_and_generation() {
        let child = child();
        let text = SupervisionStatus::new(
            child.clone(),
            None,
            RestartGeneration::FIRST,
            SupervisionState::Running,
            0,
        )
        .to_string();

        assert!(text.contains(&child.to_string()), "{text}");
        assert!(text.contains("running"), "{text}");
        assert!(text.contains("generation 0"), "{text}");
    }

    #[test]
    fn states_compare_by_variant() {
        assert_eq!(SupervisionState::Running, SupervisionState::Running);
        assert_ne!(SupervisionState::Running, SupervisionState::Down);
    }
}
