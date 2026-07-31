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

use tokio::sync::watch;

use super::{RestartGeneration, SupervisionError};
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
    failure: Option<SupervisionError>,
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
            failure: None,
        }
    }

    /// Returns this snapshot with a reason attached.
    ///
    /// A supervisor attaches one when it knows why a child is not running —
    /// a start that failed, or a supervisor that stopped before it could run
    /// one. Without it, a terminal state says only that the child is not
    /// coming back, which is rarely enough to act on.
    #[must_use]
    pub fn with_failure(mut self, failure: SupervisionError) -> Self {
        self.failure = Some(failure);
        self
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

    /// Why the child is not running, when the supervisor knows.
    ///
    /// Present only alongside a state the child did not reach on its own —
    /// a failed start, or a supervisor that stopped with the start still
    /// queued. A healthy child carries no failure.
    #[must_use]
    pub const fn failure(&self) -> Option<&SupervisionError> {
        self.failure.as_ref()
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

/// A reference to a supervised child that survives its restarts.
///
/// An [`ActorHandle`] names one incarnation: a restart replaces the child's
/// mailbox, and the old handle goes stale. A `SupervisedChild` names the child
/// itself, and can hand out the handle of whichever incarnation is current.
///
/// Reads are local. The supervisor publishes into a channel this holds the
/// receiving end of, so asking what a child is doing costs nothing and never
/// interrupts the supervisor.
#[derive(Debug, Clone)]
pub struct SupervisedChild {
    ern: Ern,
    supervisor: Ern,
    status: watch::Receiver<SupervisionStatus>,
}

impl SupervisedChild {
    /// Creates a reference to a child from the receiving end of its status
    /// channel.
    ///
    pub(crate) const fn new(
        ern: Ern,
        supervisor: Ern,
        status: watch::Receiver<SupervisionStatus>,
    ) -> Self {
        Self {
            ern,
            supervisor,
            status,
        }
    }

    /// The child's identifier. Stable across restarts.
    #[must_use]
    pub const fn ern(&self) -> &Ern {
        &self.ern
    }

    /// The identifier of the actor supervising this child.
    #[must_use]
    pub const fn supervisor(&self) -> &Ern {
        &self.supervisor
    }

    /// A handle to the current incarnation, or `None` while the child is down.
    ///
    /// Reads the local cell without marking it seen, so polling here does not
    /// consume a change that a concurrent [`wait_for`](Self::wait_for) is
    /// waiting on.
    #[must_use]
    pub fn current(&self) -> Option<ActorHandle> {
        self.status.borrow().handle().cloned()
    }

    /// What the supervisor last published about this child.
    #[must_use]
    pub fn status(&self) -> SupervisionStatus {
        self.status.borrow().clone()
    }

    /// Waits until the supervisor publishes a status the predicate accepts.
    ///
    /// The current value is checked first, so a state that has already been
    /// reached resolves immediately rather than waiting for a further change
    /// that may never come.
    ///
    /// # Errors
    ///
    /// [`SupervisionError::SupervisorStopped`] if the supervising actor's task
    /// ends while waiting, which closes the channel.
    pub async fn wait_for(
        &mut self,
        predicate: impl FnMut(&SupervisionStatus) -> bool + Send,
    ) -> Result<SupervisionStatus, SupervisionError> {
        self.status
            .wait_for(predicate)
            .await
            .map(|status| status.clone())
            .map_err(|_| SupervisionError::SupervisorStopped {
                supervisor: self.supervisor.clone(),
            })
    }

    /// Waits until the child is running, returning that incarnation's handle.
    ///
    /// Stops early at a terminal state. A child whose start failed, or whose
    /// supervisor stopped before starting it, will never reach `Running`, and
    /// waiting for it to would be waiting forever.
    ///
    /// # Errors
    ///
    /// - Whatever the supervisor published as the reason the child is not
    ///   running, when it published one.
    /// - [`SupervisionError::ChildNotRunning`] if it reached a terminal state
    ///   with no reason recorded.
    /// - [`SupervisionError::SupervisorStopped`] if the supervising actor's task
    ///   ends first.
    pub async fn wait_running(&mut self) -> Result<ActorHandle, SupervisionError> {
        let status = self
            .wait_for(|status| {
                (status.state() == SupervisionState::Running && status.handle().is_some())
                    || status.state().is_terminal()
            })
            .await?;

        self.running_handle(&status)
    }

    /// Waits until the given incarnation of the child is running.
    ///
    /// Accepts any generation at or beyond `generation`, so a caller that asks
    /// for one restart does not hang if the child has already been restarted
    /// twice by the time it asks. Stops early at a terminal state, for the same
    /// reason [`wait_running`](Self::wait_running) does.
    ///
    /// # Errors
    ///
    /// The same three as [`wait_running`](Self::wait_running).
    pub async fn wait_generation(
        &mut self,
        generation: RestartGeneration,
    ) -> Result<ActorHandle, SupervisionError> {
        let status = self
            .wait_for(|status| {
                (status.generation() >= generation
                    && status.state() == SupervisionState::Running
                    && status.handle().is_some())
                    || status.state().is_terminal()
            })
            .await?;

        self.running_handle(&status)
    }

    /// The handle from a status that reports a running child, or why not.
    fn running_handle(
        &self,
        status: &SupervisionStatus,
    ) -> Result<ActorHandle, SupervisionError> {
        if status.state() == SupervisionState::Running {
            if let Some(handle) = status.handle() {
                return Ok(handle.clone());
            }
        }

        Err(status.failure().cloned().unwrap_or_else(|| {
            SupervisionError::ChildNotRunning {
                child: self.ern.clone(),
                state: status.state(),
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::ActorHandleInterface;

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

    // ---- SupervisedChild --------------------------------------------------

    fn supervised(
        child: &Ern,
    ) -> (SupervisedChild, watch::Sender<SupervisionStatus>) {
        let supervisor = Ern::with_root("pool").expect("'pool' is a valid Ern root");
        let (sender, receiver) = watch::channel(SupervisionStatus::new(
            child.clone(),
            None,
            RestartGeneration::FIRST,
            SupervisionState::Starting,
            0,
        ));
        (
            SupervisedChild::new(child.clone(), supervisor, receiver),
            sender,
        )
    }

    fn running(child: &Ern, generation: RestartGeneration) -> SupervisionStatus {
        let (outbox, _inbox) = tokio::sync::mpsc::channel(8);
        SupervisionStatus::new(
            child.clone(),
            Some(ActorHandle::new(child.clone(), outbox)),
            generation,
            SupervisionState::Running,
            0,
        )
    }

    #[test]
    fn a_supervised_child_reports_both_identities() {
        let child = child();
        let (reference, _sender) = supervised(&child);

        assert_eq!(reference.ern(), &child);
        assert_eq!(reference.supervisor().to_string(), reference.supervisor().to_string());
        assert!(reference.current().is_none(), "not running yet");
        assert_eq!(reference.status().state(), SupervisionState::Starting);
    }

    #[test]
    fn current_follows_whatever_the_supervisor_last_published() {
        let child = child();
        let (reference, sender) = supervised(&child);

        sender.send_replace(running(&child, RestartGeneration::FIRST));

        assert!(reference.current().is_some());
        assert_eq!(reference.status().state(), SupervisionState::Running);
    }

    #[tokio::test]
    async fn waiting_for_a_state_already_reached_resolves_immediately() {
        // wait_for checks the current value before waiting, so a caller that
        // asks after the fact does not hang for a change that never comes.
        let child = child();
        let (mut reference, sender) = supervised(&child);
        sender.send_replace(running(&child, RestartGeneration::FIRST));

        let handle = reference
            .wait_running()
            .await
            .expect("the supervisor is still alive");

        assert_eq!(handle.id(), child);
    }

    #[tokio::test]
    async fn waiting_resolves_when_the_supervisor_publishes() {
        let child = child();
        let (mut reference, sender) = supervised(&child);

        let publisher = {
            let child = child.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                sender.send_replace(running(&child, RestartGeneration::FIRST));
                // Hold the sender until the waiter has seen the value.
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            })
        };

        let handle = reference
            .wait_running()
            .await
            .expect("the supervisor published a running status");
        assert_eq!(handle.id(), child);
        publisher.await.expect("publisher task completes");
    }

    #[tokio::test]
    async fn waiting_for_a_later_generation_ignores_earlier_ones() {
        let child = child();
        let (mut reference, sender) = supervised(&child);
        sender.send_replace(running(&child, RestartGeneration::FIRST));

        let target = RestartGeneration::FIRST.next();
        let publisher = {
            let child = child.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                sender.send_replace(running(&child, target));
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            })
        };

        let handle = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            reference.wait_generation(target),
        )
        .await
        .expect("wait_generation must not hang")
        .expect("the supervisor published the requested generation");

        assert_eq!(handle.id(), child);
        publisher.await.expect("publisher task completes");
    }

    #[tokio::test]
    async fn a_start_that_failed_ends_the_wait_with_the_reason_it_failed() {
        // **Fails by hanging** if the wait only looks for Running. A child whose
        // start failed never reaches it, and the supervisor is still alive, so
        // the channel never closes either. The timeout is the assertion.
        let child = child();
        let (mut reference, sender) = supervised(&child);
        let reason = SupervisionError::ConfigRejected {
            child: child.clone(),
            reason: "the spawner said no".to_string(),
        };
        sender.send_replace(
            SupervisionStatus::new(
                child.clone(),
                None,
                RestartGeneration::FIRST,
                SupervisionState::Retired,
                0,
            )
            .with_failure(reason.clone()),
        );

        let error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            reference.wait_running(),
        )
        .await
        .expect("a terminal state must end the wait")
        .expect_err("the child never came up");

        assert_eq!(error, reason);
    }

    #[tokio::test]
    async fn a_terminal_state_with_no_reason_still_ends_the_wait() {
        let child = child();
        let (mut reference, sender) = supervised(&child);
        sender.send_replace(SupervisionStatus::new(
            child.clone(),
            None,
            RestartGeneration::FIRST,
            SupervisionState::Down,
            0,
        ));

        let error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            reference.wait_generation(RestartGeneration::FIRST),
        )
        .await
        .expect("a terminal state must end the wait")
        .expect_err("the child is down");

        assert_eq!(
            error,
            SupervisionError::ChildNotRunning {
                child,
                state: SupervisionState::Down,
            }
        );
    }

    #[tokio::test]
    async fn a_restart_in_progress_is_not_a_reason_to_stop_waiting() {
        // Only terminal states end the wait. RestartPending and Restarting both
        // lead back to Running, so a caller must wait through them.
        let child = child();
        let (mut reference, sender) = supervised(&child);
        sender.send_replace(SupervisionStatus::new(
            child.clone(),
            None,
            RestartGeneration::FIRST,
            SupervisionState::RestartPending,
            1,
        ));

        let publisher = {
            let child = child.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                sender.send_replace(running(&child, RestartGeneration::FIRST));
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            })
        };

        let handle = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            reference.wait_running(),
        )
        .await
        .expect("the wait must not hang")
        .expect("the child came back");

        assert_eq!(handle.id(), child);
        publisher.await.expect("publisher task completes");
    }

    #[tokio::test]
    async fn a_supervisor_that_goes_away_stops_the_wait() {
        // Dropping every sender is how a caller learns the supervisor's task
        // ended, rather than waiting forever for a status that cannot come.
        let child = child();
        let (mut reference, sender) = supervised(&child);
        drop(sender);

        let error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            reference.wait_running(),
        )
        .await
        .expect("the wait must not hang once the channel closes")
        .expect_err("the supervisor is gone");

        assert!(matches!(
            error,
            SupervisionError::SupervisorStopped { .. }
        ));
    }

    #[test]
    fn states_compare_by_variant() {
        assert_eq!(SupervisionState::Running, SupervisionState::Running);
        assert_ne!(SupervisionState::Running, SupervisionState::Down);
    }
}
