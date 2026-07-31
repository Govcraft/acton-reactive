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
//! *Creating* a child does await, and it awaits on user code — the child's own
//! `before_start` hook — so it does not run here at all. The message loop hands
//! each start to its own task and carries on; the answer comes back as a
//! message, like every other answer in this system.
//!
//! They take `&mut self` rather than `&self` for a reason worth keeping: an
//! `async fn(&self)` produces a future holding `&Self`, which is `Send` only if
//! `Self: Sync` — that is, only if the user's model is `Sync`, a bound this
//! crate refuses to impose.

use std::fmt::Debug;

use tracing::{trace, warn};

use std::sync::Arc;

use tokio::sync::watch;

use super::registry::{PendingSlot, StartTicket};
use super::{
    ChildBlueprint, ChildSpawner, NewSlot, SupervisedChild, SupervisionError, SupervisionRegistry,
    SupervisionState, SupervisionStatus, TypedSpawner,
};
use crate::actor::managed_actor::started::Started;
use crate::actor::{ActorConfig, Idle, ManagedActor, RestartGeneration, RestartLimiter};
use crate::common::config::CONFIG;
use crate::common::ActorHandle;
use crate::message::{RegisterSupervisedChild, SupervisedChildStarted, UnregisterSupervisedChild};
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

/// Builds one supervised child, then hands it to its supervisor.
///
/// A free function rather than a method because it runs on its own task and
/// must not borrow the actor. It is the whole reason a supervisor stays
/// responsive while a child starts: everything slow about creating an actor,
/// including the child's `before_start` hook, happens here.
///
/// # The handle is never dropped on the floor
///
/// Between `spawn` returning and the supervisor recording it, this task holds
/// the only handle to a live actor. Two things can go wrong, and both end the
/// same way:
///
/// - delivery fails, because the supervisor stopped or its inbox closed;
/// - delivery is refused by this actor's own cancellation token.
///
/// In either case the child is stopped here. Dropping the handle instead would
/// leave an actor running that nothing in the system can reach.
///
/// # A full inbox cannot wedge this
///
/// Delivery goes into the supervisor's *bounded* inbox, so it is worth being
/// explicit about why waiting for capacity cannot become a standoff. The
/// supervisor never waits on this task — it launched it and moved on — so the
/// cycle that would be needed for a deadlock does not exist. What is left is:
///
/// - **The supervisor is running.** It keeps taking messages, capacity frees
///   up, delivery lands.
/// - **The supervisor is stopping.** Its shutdown closes the inbox, which fails
///   a waiting send rather than leaving it parked; a cancelled token fails it
///   sooner still. Either way this task learns, and stops the child.
/// - **The supervisor is wedged in a handler of its own that never returns.**
///   Delivery waits, and the child stays reachable through this task the whole
///   time. That is a stalled actor, which stops every other message equally;
///   nothing here makes it worse, and nothing is lost when it resolves.
///
/// `ActorHandle::stop` does wait for this task, but indirectly: it waits on the
/// handle's tracker, which holds the supervisor's message loop, and that loop's
/// shutdown waits on the tracker holding this one. An outside caller waiting,
/// then — not the supervisor, whose loop is still free to run, close its inbox,
/// and let this task finish.
async fn start_supervised_child(
    ticket: StartTicket,
    runtime: crate::common::ActorRuntime,
    supervisor: ActorHandle,
) {
    let outcome = ticket.spawner.spawn(runtime, supervisor.clone()).await;
    let started = SupervisedChildStarted {
        child: ticket.ern,
        index: ticket.index,
        outcome: outcome.clone(),
    };

    // Targets the supervisor's own inbox, exactly as its other supervision
    // messages do. `try_send` rather than `send` because the difference between
    // "delivered" and "nobody to deliver to" decides whether a live child needs
    // stopping, and `send` reports neither.
    let envelope = supervisor.create_envelope(Some(supervisor.reply_address()));
    let delivery = envelope.try_send(started).await;

    let Err(undeliverable) = delivery else {
        return;
    };

    match outcome {
        Ok(handle) => {
            warn!(
                "Supervisor {} could not be told about child {} ({}); stopping the child rather than losing it",
                supervisor.id(),
                handle.id(),
                undeliverable
            );
            stop_stray_child(handle).await;
        }
        Err(error) => {
            // Nothing was created, so nothing is stranded. The caller learns
            // from the status channel closing with the supervisor.
            trace!(
                "Supervisor {} could not be told that a child failed to start ({}): {}",
                supervisor.id(),
                undeliverable,
                error
            );
        }
    }
}

/// Stops a child nobody supervises, within the shutdown deadline.
///
/// Bounded for the same reason `terminate_children` bounds its stops: a child
/// that will not stop must not hold a task open forever. A child that outlives
/// the deadline is logged rather than waited on.
async fn stop_stray_child(handle: ActorHandle) {
    let deadline = std::time::Duration::from_millis(CONFIG.timeouts.actor_shutdown);
    match tokio::time::timeout(deadline, handle.stop()).await {
        Ok(Ok(())) => trace!("Stopped unsupervised child {}", handle.id()),
        Ok(Err(error)) => warn!(
            "Unsupervised child {} reported an error while stopping: {error:?}",
            handle.id()
        ),
        Err(_) => warn!(
            "Unsupervised child {} did not stop within {} ms",
            handle.id(),
            CONFIG.timeouts.actor_shutdown
        ),
    }
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
    /// The union of three views that can legitimately disagree, deduplicated by
    /// identifier:
    ///
    /// - the registry, which is authoritative but only knows what this actor's
    ///   task has already processed;
    /// - `handle.children()`, which is written synchronously by `supervise()`
    ///   but only on the handle clone that call was made through;
    /// - `late_arrivals`, children whose start landed in the inbox after this
    ///   actor stopped reading it.
    ///
    /// No one of them is sufficient. A child supervised through a handle clone
    /// obtained after this actor started is absent from the task-local
    /// `children` map, because cloning a handle deep-copies that map. A child
    /// supervised from inside this actor's own handler is present there
    /// immediately, but its registration message may still be queued behind the
    /// very `Terminate` that triggered this shutdown. And a child this actor
    /// started itself is in neither until its start task's report is processed,
    /// which may never happen. Reading fewer views drops one of those children
    /// on the floor.
    pub(crate) fn shutdown_child_handles(&self, late_arrivals: Vec<ActorHandle>) -> Vec<ActorHandle> {
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

        for handle in late_arrivals {
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
    /// start, and returns. On its next turn the message loop hands the queued
    /// start to its own task, before it takes another message.
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
    /// # What does not run on your task
    ///
    /// The child is built on a task of its own, so a child that is slow to
    /// start — a `before_start` hook doing real work — does not stop its
    /// supervisor from taking messages meanwhile. A child's startup may
    /// therefore depend on its supervisor making progress: it can send to it,
    /// and be answered.
    ///
    /// That is not true of [`supervise`], which awaits `start()` on the
    /// caller's task, so a handler that adopts a child does run that child's
    /// `before_start` on its own actor's task.
    ///
    /// The order this buys you is worth being precise about: this call returns
    /// before the child exists, and the child may still be starting when the
    /// next message is handled. What is settled by the time this returns is the
    /// child's name and its place in start order, not its existence.
    ///
    /// [`supervise`]: crate::common::ActorHandle::supervise
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

    /// Launches a start task for every child recorded by
    /// [`supervise_deferred`](Self::supervise_deferred) since the last turn.
    ///
    /// Driven from the message loop, ahead of the wait for the next message, so
    /// that a child a handler asked for is on its way before this actor takes
    /// anything else on.
    ///
    /// Synchronous, and that is the point. Building a child means running the
    /// child's `before_start` hook, which is user code of unbounded duration;
    /// awaiting it here would stop this actor from taking messages — including
    /// its own `Terminate` — until somebody else's hook finished. So each start
    /// gets its own task and reports back through the inbox, and this loop turn
    /// costs a few task spawns.
    ///
    /// Launches nothing once this actor is cancelled. What is left stays queued
    /// for [`cancel_unfinished_children`](Self::cancel_unfinished_children) to
    /// answer.
    pub(crate) fn launch_pending_starts(&mut self) {
        while !self.is_cancelled() {
            let Some(ticket) = self.supervision.begin_start() else {
                break;
            };

            trace!(
                "Actor {} is starting supervised child {}",
                self.id(),
                ticket.ern
            );

            let runtime = self.runtime.clone();
            let supervisor = self.handle.clone();
            self.start_tasks
                .spawn(start_supervised_child(ticket, runtime, supervisor));
        }
    }

    /// Records the outcome a start task reported.
    ///
    /// Synchronous, like every other piece of registration bookkeeping. The one
    /// case that needs an `await` — stopping a child this actor turns out not to
    /// supervise — is handed to its own task rather than done here, so that a
    /// child slow to stop cannot hold up the message loop.
    pub(crate) fn record_started_child(&mut self, message: &SupervisedChildStarted) {
        match &message.outcome {
            Ok(handle) => {
                if self
                    .supervision
                    .complete_start(message.index, &message.child, handle.clone())
                {
                    trace!(
                        "Actor {} now supervises child {}",
                        self.id(),
                        message.child
                    );
                } else {
                    // The slot moved on while the start was in flight, so
                    // nothing is supervising this child. It must not be left
                    // running with nobody holding it.
                    warn!(
                        "Actor {} no longer supervises child {}; stopping the incarnation it started",
                        self.id(),
                        message.child
                    );
                    self.stop_disowned_child(handle.clone());
                }
            }
            Err(error) => {
                warn!(
                    "Actor {} could not start supervised child {}: {}",
                    self.id(),
                    message.child,
                    error
                );
                self.supervision.fail_start(message.index, error);
            }
        }
    }

    /// Stops a child this actor does not supervise, off its own task.
    ///
    /// Tracked rather than detached: a shutdown that did not wait for this would
    /// return while the child was still stopping.
    fn stop_disowned_child(&self, handle: ActorHandle) {
        self.start_tasks.spawn(async move {
            stop_stray_child(handle).await;
        });
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

    /// Abandons every start that has not finished, on the way down.
    ///
    /// Two different situations, one answer. A queued child was never handed to
    /// anyone and now never will be. A child whose start is in flight may
    /// already exist, but this actor is no longer in a position to take it on;
    /// the task holding its handle finds the inbox closed and stops it. Either
    /// way the caller waiting on the status channel is told the supervisor
    /// stopped, rather than waiting for a start that cannot land.
    pub(crate) fn cancel_unfinished_children(&mut self) {
        let supervisor = self.id.clone();
        let abandoned = self.supervision.cancel_unfinished_starts(&supervisor);
        if abandoned > 0 {
            trace!(
                "Actor {} abandoned {} unfinished child start(s) while stopping",
                self.id(),
                abandoned
            );
        }
    }

    /// Takes the children whose start landed in an inbox nobody will read.
    ///
    /// The last gap in the hand-over. A start task that delivers successfully
    /// has done its job and stops holding the child, but if this actor's loop
    /// has already exited, that message is sitting in a queue that is about to
    /// be dropped — and with it the only handle to a running actor. Draining it
    /// here turns those into children the shutdown stops.
    ///
    /// Sound only because the in-flight starts were awaited first: with every
    /// start task finished, this queue has no writers left, and what is in it
    /// now is all there will ever be.
    ///
    /// Everything else in the inbox is discarded, which is what would have
    /// happened anyway: the receiver is dropped moments later, and every
    /// message it still holds goes with it.
    pub(crate) fn take_late_started_children(&mut self) -> Vec<ActorHandle> {
        let mut handles = Vec::new();

        while let Ok(envelope) = self.inbox.try_recv() {
            if let Some(started) = envelope
                .message
                .as_any()
                .downcast_ref::<SupervisedChildStarted>()
            {
                if let Ok(handle) = &started.outcome {
                    warn!(
                        "Actor {} stopped before adopting child {}; stopping it with the rest",
                        self.id(),
                        started.child
                    );
                    handles.push(handle.clone());
                }
            }
        }

        handles
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use acton_ern::Ern;

    use super::super::registry::{ChildSlot, SlotState};
    use super::*;
    use crate::actor::{ChildIndex, RestartPolicy};
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

    /// A spawner that never finishes building its child.
    ///
    /// Stands in for the thing this whole step is about: a child whose
    /// `before_start` hook takes as long as it likes. Under the old shape the
    /// supervisor awaited this and stopped taking messages.
    #[derive(Debug)]
    struct NeverFinishingSpawner {
        child: Ern,
    }

    impl ChildSpawner for NeverFinishingSpawner {
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
            Box::pin(std::future::pending())
        }
    }

    /// Waits for `flag` to be set, up to a bounded time.
    ///
    /// Stopping a child nobody supervises happens on its own task, so the proof
    /// that it happened arrives a moment later.
    async fn wait_for_flag(flag: &Arc<AtomicBool>) -> bool {
        for _ in 0..300 {
            if flag.load(Ordering::SeqCst) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        flag.load(Ordering::SeqCst)
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

    /// Feeds one start task's report back to the supervisor by hand.
    ///
    /// These tests hold a `Started` actor whose message loop is not running, so
    /// nothing is draining the inbox the start task delivers to. This is the
    /// loop's supervision arm, minus the loop.
    async fn deliver_one_report(actor: &mut ManagedActor<Started, Supervisor>) {
        let envelope = tokio::time::timeout(PATIENCE, actor.inbox.recv())
            .await
            .expect("a start task must report back")
            .expect("the inbox is open");
        let started = envelope
            .message
            .as_any()
            .downcast_ref::<SupervisedChildStarted>()
            .expect("a start task sends nothing else")
            .clone();

        actor.record_started_child(&started);
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

        actor.launch_pending_starts();
        deliver_one_report(&mut actor).await;

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
    async fn launching_a_start_does_not_wait_for_it() {
        // The whole point of the start task. A spawner that never finishes must
        // not stop the supervisor from getting on with its turn, so this call
        // returns while the start is still in flight.
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = supervisor(&mut runtime);
        let child = actor
            .id()
            .add_part("slow")
            .expect("'slow' is a valid Ern part");
        let (status, _receiver) = status_channel(&child, None);
        actor
            .supervision
            .register_pending(PendingSlot {
                ern: child.clone(),
                spawner: Arc::new(NeverFinishingSpawner {
                    child: child.clone(),
                }),
                restart_policy: RestartPolicy::Permanent,
                limiter: RestartLimiter::default(),
                status,
            })
            .expect("the first registration of a name succeeds");

        tokio::time::timeout(PATIENCE, async { actor.launch_pending_starts() })
            .await
            .expect("launching must not wait on the spawner");

        assert!(!actor.supervision.has_pending_starts());
        assert_eq!(
            actor.supervision.slot_of(&child).map(ChildSlot::state),
            Some(SlotState::Starting),
            "the slot records that somebody else is building it"
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
            .expect("the child is supervised, queued or not");
        actor.launch_pending_starts();

        assert_eq!(
            attempts.load(Ordering::SeqCst),
            0,
            "the spawner must not run for a child nobody supervises any more"
        );
        assert!(!actor.supervision.has_pending_starts());
    }

    #[tokio::test]
    async fn a_cancelled_supervisor_launches_nothing_new() {
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = supervisor(&mut runtime);
        let attempts = Arc::new(AtomicUsize::new(0));
        let mut child = queue_refusal(&mut actor, &attempts);
        actor
            .cancellation_token
            .as_ref()
            .expect("a started actor always has a token")
            .cancel();

        actor.launch_pending_starts();

        assert_eq!(
            attempts.load(Ordering::SeqCst),
            0,
            "a supervisor on its way down starts nothing new"
        );
        assert!(
            actor.supervision.has_pending_starts(),
            "what it did not launch stays queued for shutdown to answer"
        );

        // And shutdown answers it, rather than leaving the caller waiting.
        actor.cancel_unfinished_children();
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

    #[tokio::test]
    async fn shutdown_answers_a_start_that_is_still_in_flight() {
        // The case that only exists because starts run elsewhere: the slot is
        // neither queued nor running, and the caller is waiting on a child that
        // is genuinely being built right now.
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = supervisor(&mut runtime);
        let child = actor
            .id()
            .add_part("slow")
            .expect("'slow' is a valid Ern part");
        let (status, receiver) = status_channel(&child, None);
        actor
            .supervision
            .register_pending(PendingSlot {
                ern: child.clone(),
                spawner: Arc::new(NeverFinishingSpawner {
                    child: child.clone(),
                }),
                restart_policy: RestartPolicy::Permanent,
                limiter: RestartLimiter::default(),
                status,
            })
            .expect("the first registration of a name succeeds");
        let mut waiting = SupervisedChild::new(child.clone(), actor.id().clone(), receiver);

        actor.launch_pending_starts();
        assert!(
            actor
                .supervision
                .slot_of(&child)
                .is_some_and(ChildSlot::is_starting),
            "the start really is in flight"
        );

        actor.cancel_unfinished_children();

        let error = tokio::time::timeout(PATIENCE, waiting.wait_running())
            .await
            .expect("a supervisor stopping mid-start must end the wait")
            .expect_err("the child never came up");
        assert!(
            matches!(error, SupervisionError::SupervisorStopped { .. }),
            "unexpected error: {error}"
        );
        assert_eq!(
            actor.supervision.index_of(&child),
            None,
            "and the record is settled rather than left half-started"
        );
    }

    #[tokio::test]
    async fn a_child_this_actor_stopped_supervising_is_stopped_too() {
        // The third way a started child can end up with nobody holding it: the
        // report arrives, but the slot was retired while the start was in
        // flight. Recording it would supervise a child nobody asked for;
        // dropping the handle would leave it running.
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = supervisor(&mut runtime);
        let stopped = Arc::new(AtomicBool::new(false));

        let config = ActorConfig::for_supervised_child("worker", actor.handle.clone(), None)
            .expect("a name plus a live parent is a valid child configuration");
        let child_id = config.id();
        let (status, _receiver) = status_channel(&child_id, None);
        let blueprint: Arc<ChildBlueprint<Supervisor>> = {
            let stopped = Arc::clone(&stopped);
            Arc::new(move |child: &mut ManagedActor<Idle, Supervisor>| {
                let stopped = Arc::clone(&stopped);
                child.after_stop(move |_actor| {
                    let stopped = Arc::clone(&stopped);
                    async move {
                        stopped.store(true, Ordering::SeqCst);
                    }
                });
            })
        };

        actor
            .supervision
            .register_pending(PendingSlot {
                ern: child_id.clone(),
                spawner: Arc::new(TypedSpawner::new(config, blueprint)),
                restart_policy: RestartPolicy::Permanent,
                limiter: RestartLimiter::default(),
                status,
            })
            .expect("the first registration of a name succeeds");

        actor.launch_pending_starts();
        actor
            .supervision
            .retire(&child_id)
            .expect("the child is supervised while its start is in flight");

        deliver_one_report(&mut actor).await;

        assert!(
            wait_for_flag(&stopped).await,
            "the child was built, disowned, and then left running"
        );
        assert!(
            actor.supervision.live_handles().is_empty(),
            "and it was not recorded on the way past"
        );
    }

    #[tokio::test]
    async fn a_report_that_lands_after_the_loop_stops_is_not_lost() {
        // The narrow window the drain exists for: delivery succeeded, so the
        // start task has let go of the child, but the loop is already over and
        // nothing will ever dispatch that message.
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = supervisor(&mut runtime);
        let child = runtime.new_actor::<Supervisor>().start().await;
        let envelope = actor
            .handle
            .create_envelope(Some(actor.handle.reply_address()));
        envelope
            .try_send(SupervisedChildStarted {
                child: child.id(),
                index: ChildIndex::new(0),
                outcome: Ok(child.clone()),
            })
            .await
            .expect("the inbox is open");

        let late = actor.take_late_started_children();

        assert_eq!(late.len(), 1, "the child in the undelivered report");
        assert_eq!(late[0].id(), child.id());
        assert!(
            actor
                .shutdown_child_handles(late)
                .iter()
                .any(|handle| handle.id() == child.id()),
            "and it joins the children the shutdown stops"
        );
    }
}
