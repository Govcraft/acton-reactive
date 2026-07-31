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

//! The supervising actor's side of registration.
//!
//! These run on the supervisor's own task, reached from its message loop.
//! *Recording* a child is deliberately **not** `async`: it touches only data the
//! actor already owns, plus a single non-blocking write to the waiting caller's
//! cell. Making registration synchronous is the point of giving the supervisor
//! sole ownership of its registry, and it is what lets a handler register a
//! child of its own — a handler has `&mut ManagedActor<Started, _>` but cannot
//! hold it across an `await`.
//!
//! *Creating* a child does await, so it lives in the one place that can: the
//! message loop, which drives [`ManagedActor::start_pending_children`] between
//! messages.
//!
//! They take `&mut self` rather than `&self` for a reason worth keeping: an
//! `async fn(&self)` produces a future holding `&Self`, which is `Send` only if
//! `Self: Sync` — that is, only if the user's model is `Sync`, a bound this
//! crate refuses to impose.

use std::fmt::Debug;

use tracing::{trace, warn};

use std::sync::Arc;

use tokio::sync::watch;

use super::registry::{ChildSlot, PendingSlot};
use super::{
    ChildBlueprint, ChildSpawner, NewSlot, SupervisedChild, SupervisionError, SupervisionRegistry,
    SupervisionState, SupervisionStatus, TypedSpawner,
};
use crate::actor::managed_actor::started::Started;
use crate::actor::{ActorConfig, Idle, ManagedActor, RestartGeneration, RestartLimiter};
use crate::common::ActorHandle;
use crate::message::{RegisterSupervisedChild, UnregisterSupervisedChild};
use crate::traits::ActorHandleInterface;

/// Builds the status channel a supervised child publishes through.
pub fn status_channel(
    child: &acton_ern::Ern,
    handle: Option<ActorHandle>,
) -> (
    watch::Sender<SupervisionStatus>,
    watch::Receiver<SupervisionStatus>,
) {
    watch::channel(SupervisionStatus::new(
        child.clone(),
        handle,
        RestartGeneration::FIRST,
        SupervisionState::Starting,
        0,
    ))
}

impl<Model: Default + Send + Debug + 'static> ManagedActor<Started, Model> {
    /// Records a child this actor should look after.
    ///
    /// Synchronous: no I/O, no await, nothing to block on.
    pub(crate) fn register_supervised_child(&mut self, message: &RegisterSupervisedChild) {
        let slot = NewSlot {
            ern: message.child.clone(),
            handle: message.handle.clone(),
            spawner: message.spawner.clone(),
            restart_policy: message.restart_policy,
            // The supervisor's configured limiter settings are not yet read
            // from `ActorConfig`; that plumbing arrives with the restart engine.
            limiter: RestartLimiter::default(),
            status: message.status.clone(),
        };

        let outcome = match self.supervision.register(slot) {
            Ok(index) => {
                trace!(
                    "Actor {} now supervises child {} at {}",
                    self.id(),
                    message.child,
                    index
                );
                Ok(())
            }
            Err(error) => {
                warn!(
                    "Actor {} rejected supervision of child {}: {}",
                    self.id(),
                    message.child,
                    error
                );
                Err(error)
            }
        };

        Self::report(message.outcome.as_ref(), outcome);
    }

    /// Stops looking after a child, returning its handle to the caller's care.
    ///
    /// The child is not stopped here. Releasing a child and stopping it are
    /// separate decisions, and only the caller knows which it wants.
    pub(crate) fn unregister_supervised_child(&mut self, message: &UnregisterSupervisedChild) {
        if message.liveness.receiver_count() == 0 {
            trace!(
                "Releasing child {} with no caller waiting on the result",
                message.child
            );
        }

        let outcome = match self.supervision.retire(&message.child) {
            Ok(handle) => {
                trace!(
                    "Actor {} released child {} (handle retained: {})",
                    self.id(),
                    message.child,
                    handle.is_some()
                );
                Ok(())
            }
            Err(error) => {
                warn!(
                    "Actor {} cannot release child {}: {}",
                    self.id(),
                    message.child,
                    error
                );
                Err(error)
            }
        };

        Self::report(Some(&message.outcome), outcome);
    }

    /// Hands the result back to a waiting caller, if one is waiting.
    ///
    /// A failed `set` means the cell was already written, which can only happen
    /// if a caller reused it. Nothing is retried: the first answer stands.
    fn report(
        cell: Option<&crate::message::RegistrationOutcome>,
        outcome: Result<(), SupervisionError>,
    ) {
        if let Some(cell) = cell {
            if cell.set(outcome).is_err() {
                warn!("Supervision outcome cell was already set; ignoring the later result");
            }
        }
    }

    /// Every child this actor should stop when it shuts down.
    ///
    /// The union of two views that can legitimately disagree, deduplicated by
    /// identifier:
    ///
    /// - the registry, which is authoritative but only knows what this actor's
    ///   task has already processed;
    /// - `handle.children()`, which is written synchronously by `supervise()`
    ///   but only on the handle clone that call was made through.
    ///
    /// Neither alone is sufficient. A child supervised through a handle clone
    /// obtained after this actor started is absent from the task-local
    /// `children` map, because cloning a handle deep-copies that map. A child
    /// supervised from inside this actor's own handler is present there
    /// immediately, but its registration message may still be queued behind the
    /// very `Terminate` that triggered this shutdown. Reading only one view
    /// drops one of those children on the floor.
    pub(crate) fn shutdown_child_handles(&self) -> Vec<ActorHandle> {
        let mut seen = std::collections::HashSet::new();
        let mut handles = Vec::new();

        for handle in self.supervision.live_handles() {
            if seen.insert(handle.id()) {
                handles.push(handle);
            }
        }

        for entry in self.handle.children() {
            let handle = entry.value().clone();
            if seen.insert(handle.id()) {
                handles.push(handle);
            }
        }

        handles
    }

    /// This actor's record of its children.
    pub(crate) const fn supervision_mut(&mut self) -> &mut SupervisionRegistry {
        &mut self.supervision
    }

    /// Starts a child under this actor's supervision, recording it directly.
    ///
    /// Records **synchronously** into this actor's own registry: no message, no
    /// round trip, and nothing to wait on.
    ///
    /// # Not reachable from user code
    ///
    /// Crate-internal on purpose. Calling this needs `&mut self` held across an
    /// `await`, and no user-facing context provides that. A `mutate_on` handler
    /// returns a `'static` future that cannot borrow the actor, and all four
    /// lifecycle hooks take `&ManagedActor<Started, _>` rather than `&mut`. Only
    /// the framework's own message loop qualifies.
    ///
    /// It is kept because the restart engine will drive it from inside that
    /// loop. Making it public would ship a method nothing could call.
    /// [`supervise_deferred`](Self::supervise_deferred) is the public path: same
    /// outcome, with the `await` moved into the message loop where one is
    /// available.
    ///
    /// # Errors
    ///
    /// [`SupervisionError::DuplicateChild`] if this actor already supervises a
    /// child with that identifier. The freshly started child is stopped before
    /// returning, so a rejected registration leaves nothing running.
    pub(crate) async fn supervise_with<C>(
        &mut self,
        config: ActorConfig,
        configure: impl Fn(&mut ManagedActor<Idle, C>) + Send + Sync + 'static,
    ) -> Result<SupervisedChild, SupervisionError>
    where
        C: Default + Send + Debug + 'static,
    {
        let blueprint: Arc<ChildBlueprint<C>> = Arc::new(configure);
        let spawner: Arc<dyn ChildSpawner> =
            Arc::new(TypedSpawner::new(config.clone(), blueprint));

        let child_id = config.id();
        let restart_policy = spawner.restart_policy();
        let handle = spawner.spawn(self.runtime.clone(), self.handle.clone()).await?;
        let (status, receiver) = status_channel(&child_id, Some(handle.clone()));

        let slot = NewSlot {
            ern: child_id.clone(),
            handle: handle.clone(),
            spawner: Some(spawner),
            restart_policy,
            limiter: RestartLimiter::default(),
            status,
        };

        // The borrow of the registry ends with this statement, so the stop below
        // does not hold it across an await.
        let registered = self.supervision.register(slot);
        if let Err(error) = registered {
            // Nothing is supervising this child, so it must not be left running.
            let _ = handle.stop().await;
            return Err(error);
        }

        Ok(SupervisedChild::new(child_id, self.id.clone(), receiver))
    }

    /// Places a child under this actor's supervision from inside its own
    /// handler, starting it on the next turn of the message loop.
    ///
    /// This is the one registration path a supervisor can use on itself.
    /// [`ActorHandle::supervise_with`] cannot be: it waits for an
    /// acknowledgement this actor cannot produce while the handler asking for it
    /// is still running. The crate-internal `ManagedActor::supervise_with`
    /// cannot be either: it holds `&mut self` across an `await`, and no
    /// user-facing context provides that.
    ///
    /// So this one does not await at all. It records the child, queues the
    /// start, and returns. The message loop runs the spawner on its next turn,
    /// before it takes another message.
    ///
    /// Callable from a [`mutate_on`] closure body and from [`mutate_on_sync`],
    /// which is the whole point: both hand you `&mut ManagedActor<Started, _>`
    /// synchronously.
    ///
    /// # Waiting for the child
    ///
    /// The returned [`SupervisedChild`] starts out in
    /// [`SupervisionState::Starting`], because nothing has been created yet.
    /// Await [`wait_running`] to get the handle once it is up. Do not await it
    /// inside the handler that called this: the start happens after the handler
    /// returns, so waiting there would wait forever.
    ///
    /// # What runs on your task
    ///
    /// The child is built by *this* actor's message loop, so the child's
    /// `before_start` hook runs on this actor's task and this actor processes
    /// nothing else until it returns. A child whose `before_start` waits on
    /// this actor — on a reply, or on a `send` to an inbox this actor is not
    /// free to drain — will not be waited out, because this actor cannot drain
    /// it while it is building the child.
    ///
    /// Not new, and not particular to this method: `handle.supervise(child)`
    /// awaits `start()` on the caller's task too, so a handler that adopts a
    /// child has always run that child's `before_start` on its own actor's
    /// task. The rule either way is that a child's startup must not depend on
    /// its supervisor making progress.
    ///
    /// # Errors
    ///
    /// [`SupervisionError::DuplicateChild`] if this actor already supervises
    /// that identifier. Reported here, synchronously, before anything is built:
    /// the registry already knows the name at this point, so a collision costs
    /// a rejected call rather than an actor started and then stopped again.
    ///
    /// A start that fails later cannot be reported here. It arrives on the
    /// status channel instead, as a terminal state carrying the reason, which
    /// [`wait_running`] returns rather than waiting through.
    ///
    /// [`ActorHandle::supervise_with`]: crate::common::ActorHandle::supervise_with
    /// [`mutate_on`]: crate::actor::ManagedActor::mutate_on
    /// [`mutate_on_sync`]: crate::actor::ManagedActor::mutate_on_sync
    /// [`wait_running`]: SupervisedChild::wait_running
    pub fn supervise_deferred<C>(
        &mut self,
        config: ActorConfig,
        configure: impl Fn(&mut ManagedActor<Idle, C>) + Send + Sync + 'static,
    ) -> Result<SupervisedChild, SupervisionError>
    where
        C: Default + Send + Debug + 'static,
    {
        let blueprint: Arc<ChildBlueprint<C>> = Arc::new(configure);
        let spawner: Arc<dyn ChildSpawner> = Arc::new(TypedSpawner::new(config, blueprint));

        let child_id = spawner.child_id().clone();
        let restart_policy = spawner.restart_policy();
        // No handle: this child does not exist yet.
        let (status, receiver) = status_channel(&child_id, None);

        self.supervision.register_pending(PendingSlot {
            ern: child_id.clone(),
            spawner,
            restart_policy,
            // The supervisor's configured limiter settings are not yet read from
            // `ActorConfig`; that plumbing arrives with the restart engine.
            limiter: RestartLimiter::default(),
            status,
        })?;

        trace!(
            "Actor {} recorded child {} for a deferred start",
            self.id(),
            child_id
        );

        Ok(SupervisedChild::new(child_id, self.id.clone(), receiver))
    }

    /// Creates every child recorded by
    /// [`supervise_deferred`](Self::supervise_deferred) since the last turn.
    ///
    /// Driven from the message loop, ahead of the wait for the next message, so
    /// that a child registered by a handler is up before the handler's actor
    /// takes anything else on.
    ///
    /// Each spawner is cloned out of its slot before being awaited, so no borrow
    /// of the registry is held across the `await` and a slot can change
    /// underneath the start. It is re-checked afterwards: a child retired while
    /// its start was in flight is stopped rather than adopted.
    ///
    /// Stops between children once this actor is cancelled, rather than working
    /// through a long queue on the way down. Between children rather than
    /// during one: dropping a half-finished start would lose the handle to a
    /// child that had already been created. What is left stays queued, for
    /// shutdown to answer with
    /// [`cancel_pending_children`](Self::cancel_pending_children).
    pub(crate) async fn start_pending_children(&mut self) {
        while !self.is_cancelled() {
            let Some(index) = self.supervision.take_pending_start() else {
                break;
            };
            let Some(spawner) = self
                .supervision
                .slot(index)
                .filter(|slot| slot.is_pending())
                .and_then(ChildSlot::spawner)
            else {
                // Retired before its turn came. Nothing was created, so there is
                // nothing to undo.
                continue;
            };

            let started = spawner
                .spawn(self.runtime.clone(), self.handle.clone())
                .await;

            match started {
                Ok(handle) => {
                    if !self.supervision.start_pending(index, handle.clone()) {
                        // The slot moved on while the child was being built, so
                        // nothing is supervising it. It must not be left running.
                        let _ = handle.stop().await;
                        continue;
                    }
                    trace!(
                        "Actor {} started supervised child {}",
                        self.id(),
                        spawner.child_id()
                    );
                }
                Err(error) => {
                    warn!(
                        "Actor {} could not start supervised child {}: {}",
                        self.id(),
                        spawner.child_id(),
                        error
                    );
                    self.supervision.fail_pending(index, &error);
                }
            }
        }
    }

    /// Whether this actor has been told to shut down.
    ///
    /// `false` when there is no token, which cannot happen for a started actor
    /// — the message loop asserts it — but is the safe reading either way: an
    /// actor that cannot be cancelled has not been.
    fn is_cancelled(&self) -> bool {
        self.cancellation_token
            .as_ref()
            .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
    }

    /// Abandons every start still queued, on the way down.
    ///
    /// Nothing queued was ever created, so there is nothing to stop. What there
    /// is, is a caller per queued child waiting on a status channel for a start
    /// that can no longer happen; each is told the supervisor stopped.
    pub(crate) fn cancel_pending_children(&mut self) {
        let supervisor = self.id.clone();
        self.supervision.cancel_pending_starts(&supervisor);
    }

    /// Stops looking after a child, recording it directly. The child is stopped.
    ///
    /// Crate-internal for the same reason as
    /// [`supervise_with`](Self::supervise_with).
    ///
    /// # Errors
    ///
    /// [`SupervisionError::UnknownChild`] if this actor does not supervise it.
    pub(crate) async fn unsupervise(&mut self, child: &acton_ern::Ern) -> Result<(), SupervisionError> {
        let retired = self.supervision.retire(child);
        match retired {
            Ok(Some(handle)) => {
                let _ = handle.stop().await;
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use acton_ern::Ern;

    use super::*;
    use crate::actor::RestartPolicy;
    use crate::common::{ActonApp, ActorRuntime};

    /// Long enough to be decisive, short enough that a hang is not a break.
    const PATIENCE: Duration = Duration::from_secs(5);

    #[derive(Debug, Default)]
    struct Supervisor;

    /// A spawner that refuses to build anything, and counts the attempts.
    ///
    /// The only way to reach the drain's failure path: `TypedSpawner::spawn` is
    /// infallible today, because `ManagedActor::start` cannot fail.
    #[derive(Debug)]
    struct RefusingSpawner {
        child: Ern,
        attempts: Arc<AtomicUsize>,
    }

    impl ChildSpawner for RefusingSpawner {
        fn child_id(&self) -> &Ern {
            &self.child
        }

        fn restart_policy(&self) -> RestartPolicy {
            RestartPolicy::Permanent
        }

        fn spawn(
            &self,
            _runtime: ActorRuntime,
            _parent: ActorHandle,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<ActorHandle, SupervisionError>>
                    + Send
                    + '_,
            >,
        > {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Err(SupervisionError::ConfigRejected {
                    child: self.child.clone(),
                    reason: "this spawner never builds an actor".to_string(),
                })
            })
        }
    }

    /// A supervisor in the `Started` state whose message loop is *not* running.
    ///
    /// The drain is normally driven by that loop, which owns the actor. Taking
    /// the transition without spawning the loop is what lets a test drive the
    /// drain a turn at a time and look at the registry afterwards.
    fn supervisor(runtime: &mut ActorRuntime) -> ManagedActor<Started, Supervisor> {
        runtime.new_actor::<Supervisor>().into()
    }

    fn queue_refusal(
        actor: &mut ManagedActor<Started, Supervisor>,
        attempts: &Arc<AtomicUsize>,
    ) -> SupervisedChild {
        let child = actor
            .id()
            .add_part("worker")
            .expect("'worker' is a valid Ern part");
        let (status, receiver) = status_channel(&child, None);

        actor
            .supervision
            .register_pending(PendingSlot {
                ern: child.clone(),
                spawner: Arc::new(RefusingSpawner {
                    child: child.clone(),
                    attempts: Arc::clone(attempts),
                }),
                restart_policy: RestartPolicy::Permanent,
                limiter: RestartLimiter::default(),
                status,
            })
            .expect("the first registration of a name succeeds");

        SupervisedChild::new(child, actor.id().clone(), receiver)
    }

    #[tokio::test]
    async fn a_start_that_fails_reaches_the_caller_instead_of_stranding_it() {
        // **Fails by hanging** if a failed start only logs: the caller is
        // waiting on a status channel whose sender is alive and whose child
        // will never run. The timeout is the assertion.
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = supervisor(&mut runtime);
        let attempts = Arc::new(AtomicUsize::new(0));
        let mut child = queue_refusal(&mut actor, &attempts);

        actor.start_pending_children().await;

        let error = tokio::time::timeout(PATIENCE, child.wait_running())
            .await
            .expect("a failed start must end the wait")
            .expect_err("the child was never created");

        assert!(
            matches!(error, SupervisionError::ConfigRejected { .. }),
            "the spawner's own reason must survive: {error}"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(!actor.supervision.has_pending_starts());
        assert_eq!(
            actor.supervision.index_of(child.ern()),
            None,
            "a failed start frees the name for another attempt"
        );
        assert!(
            actor.supervision.live_handles().is_empty(),
            "nothing was created, so there is nothing to stop"
        );
    }

    #[tokio::test]
    async fn a_child_retired_before_its_turn_is_never_created() {
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = supervisor(&mut runtime);
        let attempts = Arc::new(AtomicUsize::new(0));
        let child = queue_refusal(&mut actor, &attempts);

        actor
            .supervision
            .retire(child.ern())
            .expect("the child is supervised, pending or not");
        actor.start_pending_children().await;

        assert_eq!(
            attempts.load(Ordering::SeqCst),
            0,
            "the spawner must not run for a child nobody supervises any more"
        );
        assert!(!actor.supervision.has_pending_starts());
    }

    #[tokio::test]
    async fn a_cancelled_supervisor_stops_creating_children() {
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = supervisor(&mut runtime);
        let attempts = Arc::new(AtomicUsize::new(0));
        let mut child = queue_refusal(&mut actor, &attempts);
        actor
            .cancellation_token
            .as_ref()
            .expect("a started actor always has a token")
            .cancel();

        actor.start_pending_children().await;

        assert_eq!(
            attempts.load(Ordering::SeqCst),
            0,
            "a supervisor on its way down starts nothing new"
        );
        assert!(
            actor.supervision.has_pending_starts(),
            "what it did not start stays queued for shutdown to answer"
        );

        // And shutdown answers it, rather than leaving the caller waiting.
        actor.cancel_pending_children();
        let error = tokio::time::timeout(PATIENCE, child.wait_running())
            .await
            .expect("shutdown must end the wait")
            .expect_err("the child was never created");
        assert!(
            matches!(error, SupervisionError::SupervisorStopped { .. }),
            "unexpected error: {error}"
        );
        assert!(!actor.supervision.has_pending_starts());
    }
}
