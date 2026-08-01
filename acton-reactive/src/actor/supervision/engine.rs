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

use super::plan::{evaluate, RestartPlan, SupervisionOutcome};
use super::registry::{PendingSlot, SlotState, StartRecorded, StartTicket};
use super::{
    BackoffDelay, ChildBlueprint, ChildIndex, ChildSpawner, NewSlot, SupervisedChild,
    SupervisionError, SupervisionRegistry, SupervisionState, SupervisionStatus, TypedSpawner,
};
use crate::actor::managed_actor::started::Started;
use crate::actor::{
    ActorConfig, Idle, ManagedActor, RestartGeneration, RestartLimitExceeded, RestartLimiter,
    RestartLimiterConfig,
};
use crate::common::config::CONFIG;
use crate::common::ActorHandle;
use crate::message::{
    ChildTerminated, RegisterSupervisedChild, RestartDue, SupervisedChildStarted,
    UnregisterSupervisedChild,
};
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

/// Stops one sibling as a step of a group restart, within the shutdown
/// deadline.
///
/// Bounded for a reason particular to this caller: the stops are sequential, so
/// one child that will not go down would otherwise stop every child before it
/// from being asked, and hold the whole group open. Missing the deadline costs
/// that one child its restart, which is the smaller failure.
async fn stop_group_member(handle: ActorHandle) {
    let deadline = std::time::Duration::from_millis(CONFIG.timeouts.actor_shutdown);
    match tokio::time::timeout(deadline, handle.stop()).await {
        Ok(Ok(())) => trace!("Stopped {} for a group restart", handle.id()),
        Ok(Err(error)) => warn!(
            "Child {} reported an error while stopping for a group restart: {error:?}",
            handle.id()
        ),
        Err(_) => warn!(
            "Child {} did not stop within {} ms, so its group restart may be refused as stale",
            handle.id(),
            CONFIG.timeouts.actor_shutdown
        ),
    }
}

impl<Model: Default + Send + Debug + 'static> ManagedActor<Started, Model> {
    /// Builds the restart allowance one child will be held to.
    ///
    /// **A child's own setting wins; a child that set none inherits its
    /// supervisor's.** A child may therefore raise its own `max_restarts` and
    /// effectively decline its supervisor's policy, which is a deliberate
    /// choice rather than an oversight: the two settings are set by the same
    /// author, and a child that knows it is expensive to rebuild is the thing
    /// that knows it.
    ///
    /// One helper, reached from every registration path. The three paths could
    /// each resolve this inline and would agree today; they would stop agreeing
    /// the first time somebody edited one of them, and the symptom would be a
    /// child getting a different restart budget depending on which API
    /// supervised it — a difference nobody would predict from the signatures.
    ///
    /// The limiter is built **per slot**, not shared. A supervisor holding one
    /// limiter for all its children would let one noisy child eat a sibling's
    /// allowance and escalate a child that had never failed.
    fn resolve_limiter(&self, child: Option<&RestartLimiterConfig>) -> RestartLimiter {
        child.or(self.restart_limiter_config.as_ref()).map_or_else(
            RestartLimiter::default,
            |config| RestartLimiter::new(config.clone()),
        )
    }

    /// Records a child this actor should look after.
    ///
    /// Synchronous: no I/O, no await, nothing to block on.
    pub(crate) fn register_supervised_child(&mut self, message: &RegisterSupervisedChild) {
        let slot = NewSlot {
            ern: message.child.clone(),
            handle: message.handle.clone(),
            spawner: message.spawner.clone(),
            restart_policy: message.restart_policy,
            limiter: self.resolve_limiter(message.limiter.as_ref()),
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
                    "Actor {} released child {} (handle retained: {}, caller is stopping it: {})",
                    self.id(),
                    message.child,
                    handle.is_some(),
                    message.stopping
                );
                // A child on its way out should stop answering to its IPC
                // names; a child being released to keep serving must keep
                // them. This is the only side that can tell the difference and
                // the only side that holds the runtime.
                //
                // The whole statement is conditional rather than the body of a
                // helper, so that without the `ipc` feature there is no empty
                // function taking a `self` it never reads.
                #[cfg(feature = "ipc")]
                if message.stopping {
                    let forgotten = self.runtime.ipc_forget(&message.child);
                    if forgotten > 0 {
                        trace!(
                            "Actor {} dropped {} IPC name(s) for released child {}",
                            self.id(),
                            forgotten,
                            message.child
                        );
                    }
                }
                Ok(handle)
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

        if message.outcome.set(outcome).is_err() {
            warn!("Release outcome cell was already set; ignoring the later result");
        }
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
        let limiter = self.resolve_limiter(config.restart_limiter_config());
        let handle = spawner.spawn(self.runtime.clone(), self.handle.clone()).await?;
        let (status, receiver) = status_channel(&child_id, Some(handle.clone()));

        let slot = NewSlot {
            ern: child_id.clone(),
            handle: handle.clone(),
            spawner: Some(spawner),
            restart_policy,
            limiter,
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
        // Resolved before the configuration is consumed, and through the shared
        // helper rather than inline, so this path cannot drift from the other
        // two.
        let limiter = self.resolve_limiter(config.restart_limiter_config());
        let spawner: Arc<dyn ChildSpawner> = Arc::new(TypedSpawner::new(config, blueprint));

        let child_id = spawner.child_id().clone();
        let restart_policy = spawner.restart_policy();
        // No handle: this child does not exist yet.
        let (status, receiver) = status_channel(&child_id, None);

        self.supervision.register_pending(PendingSlot {
            ern: child_id.clone(),
            spawner,
            restart_policy,
            limiter,
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

    /// Decides what to do about a child that has just terminated, and starts
    /// doing it.
    ///
    /// Synchronous, and reached from the message loop **without** suppressing
    /// handler dispatch — see the interception site for why that asymmetry with
    /// the other supervision messages is deliberate.
    ///
    /// The decision itself lives in [`evaluate`], which is pure. This does the
    /// three things a decision cannot: record the new slot state, publish it,
    /// and arm a timer.
    pub(crate) fn record_child_terminated(&mut self, notice: &ChildTerminated) {
        let Some(index) = self.supervision.index_of(&notice.child_id) else {
            // Not a child of this actor's, or one already retired. Either way
            // there is no record to update; a user handler may still care.
            return;
        };
        let Some(snapshot) = self.supervision.snapshot(index) else {
            return;
        };

        let views = self.supervision.views();
        // The supervisor consults *its own* strategy, not the child's.
        // `for_supervised_child` builds a child's configuration and a reader may
        // reasonably expect the strategy set there to govern; it does not. What
        // to do about a failure is the supervising actor's policy.
        let strategy = self.supervision_strategy;
        let now = std::time::Instant::now();

        let Some(slot) = self.supervision.slot_mut(index) else {
            return;
        };
        let outcome = evaluate(notice, &snapshot, strategy, slot.limiter_mut(), &views, now);

        match outcome {
            // The supervisor asked for this stop, or has already acted on a
            // notice for this incarnation. Nothing to record.
            SupervisionOutcome::Ignore => {
                trace!(
                    "Actor {} expected child {} to stop; no restart considered",
                    self.id(),
                    notice.child_id
                );
            }
            SupervisionOutcome::GroupStopLanded { then_restart } => {
                self.record_group_stop_landed(index, &notice.child_id, then_restart);
            }
            SupervisionOutcome::Forget => self.record_child_down(index, &notice.child_id),
            SupervisionOutcome::Escalate(exceeded) => {
                self.record_child_escalated(index, &notice.child_id, exceeded);
            }
            SupervisionOutcome::Restart { plan, backoff } => {
                self.schedule_restart(&notice.child_id, &plan, backoff);
            }
        }
    }

    /// Records a sibling that has finished stopping for a group restart.
    ///
    /// The half of a group restart the message loop owns. The task sequencing
    /// the group asked for this stop and will ask for the restart; what it
    /// cannot do is move the slot, because slots belong to the actor.
    ///
    /// `then_restart` is the whole decision, made by
    /// [`evaluate`](super::plan::evaluate) from state this actor recorded when
    /// the stop was issued. A child on its way back waits in
    /// [`SlotState::AwaitingBackoff`], which is the only state
    /// [`queue_restart`](SupervisionRegistry::queue_restart) will accept a
    /// restart from. A child the group could not recreate goes down for good,
    /// through the same path as any other child that is not coming back — so it
    /// gives up its IPC names exactly as that one does.
    fn record_group_stop_landed(
        &mut self,
        index: ChildIndex,
        child: &acton_ern::Ern,
        then_restart: bool,
    ) {
        if !then_restart {
            trace!(
                "Actor {} stopped child {} for a group restart and is leaving it down",
                self.id(),
                child
            );
            self.record_child_down(index, child);
            return;
        }

        let Some(slot) = self.supervision.slot_mut(index) else {
            return;
        };
        slot.set_handle(None);
        slot.set_state(SlotState::AwaitingBackoff);
        slot.publish();

        trace!(
            "Actor {} has child {} down and waiting on its group's restart",
            self.id(),
            child
        );
    }

    /// Records a child that terminated and is not coming back.
    fn record_child_down(&mut self, index: ChildIndex, child: &acton_ern::Ern) {
        trace!("Actor {} is leaving child {} down", self.id(), child);
        self.supervision.mark_terminal(index, SlotState::Down, None);
        // Conditional at the call site rather than a helper with an empty body,
        // so that without the `ipc` feature nothing is compiled at all.
        #[cfg(feature = "ipc")]
        self.forget_ipc_names(index, child);
    }

    /// Records a child that used up its restart allowance.
    ///
    /// Terminal, and that is the point of handling it at all: `evaluate` has
    /// been able to return this since the decision layer landed, and a slot left
    /// in `AwaitingBackoff` for a restart that will never be arranged leaves
    /// every caller waiting on a status that cannot change.
    ///
    /// What the supervisor does *about* an escalation beyond giving up is
    /// [`Escalation`], set with
    /// [`ActorConfig::with_escalation`](crate::actor::ActorConfig::with_escalation)
    /// and read here. The bookkeeping below is the same either way; the policy
    /// decides only what happens after it.
    ///
    /// [`Escalation`]: super::Escalation
    fn record_child_escalated(
        &mut self,
        index: ChildIndex,
        child: &acton_ern::Ern,
        exceeded: RestartLimitExceeded,
    ) {
        warn!(
            "Actor {} is giving up on child {} ({}): {}",
            self.id(),
            child,
            self.escalation,
            exceeded
        );

        // Built from the exceeded limit rather than read back off the slot,
        // because the slot is about to be marked terminal and this has to
        // describe the moment the supervisor gave up.
        let stats = crate::actor::RestartStats {
            restarts_in_window: exceeded.attempts,
            consecutive_restarts: exceeded.attempts,
            window_secs: exceeded.window_secs,
            max_restarts: exceeded.max_restarts,
        };
        self.supervision.mark_terminal(
            index,
            SlotState::Escalated,
            Some(SupervisionError::RestartLimit {
                child: child.clone(),
                limit: exceeded,
            }),
        );
        #[cfg(feature = "ipc")]
        self.forget_ipc_names(index, child);

        self.apply_escalation(child, stats);
    }

    /// Carries out this supervisor's escalation policy.
    ///
    /// # Why there is no wildcard arm
    ///
    /// [`Escalation`](super::Escalation) is `#[non_exhaustive]`, which normally
    /// argues for one. It does not here, and the compiler says so: the
    /// attribute has no effect within the crate that defines the enum, so a
    /// wildcard is unreachable code rather than future-proofing.
    ///
    /// That is the better outcome anyway. Adding a variant breaks this match at
    /// compile time, which forces whoever adds it to decide what a supervisor
    /// should *do* about it — at the one place that can answer. A wildcard here
    /// would silently answer for them, and a new escalation policy that quietly
    /// behaved as the old default would be a bug nothing reported.
    ///
    /// Downstream crates matching on `Escalation` still need their own
    /// wildcard; the attribute is doing its job for them.
    fn apply_escalation(&self, child: &acton_ern::Ern, stats: crate::actor::RestartStats) {
        match self.escalation {
            super::Escalation::NotifyParent => self.notify_parent_of_escalation(child, stats),
            super::Escalation::StopSupervisor => self.stop_self_after_escalation(child),
        }
    }

    /// Tells this actor's own parent that it could not keep a child running.
    ///
    /// Sent to the parent directly rather than broadcast: the parent is the
    /// actor with a stake in this one's failures, and it is the actor that would
    /// have been told had this supervisor stopped instead.
    ///
    /// A supervisor with no parent has nobody to tell, which is not a failure —
    /// it is the top of a tree. The event is still logged by the caller.
    ///
    /// Delivery goes onto its own task because sending is `async` and this runs
    /// on the message loop. Nothing waits on the answer.
    fn notify_parent_of_escalation(&self, child: &acton_ern::Ern, stats: crate::actor::RestartStats) {
        let Some(parent) = self.parent.clone() else {
            trace!(
                "Actor {} has no parent to tell that it gave up on child {}",
                self.id(),
                child
            );
            return;
        };

        let notification = super::SupervisionEscalated::new(
            self.id.clone(),
            child.clone(),
            stats,
            crate::actor::TerminationReason::Normal,
        );

        trace!(
            "Actor {} is telling parent {} that it gave up on child {}",
            self.id(),
            parent.id(),
            child
        );

        tokio::spawn(async move {
            parent.send(notification).await;
        });
    }

    /// Stops this supervisor, cascading to the children it has left.
    ///
    /// The Erlang/OTP behaviour: a supervisor that cannot keep its children
    /// running terminates and lets its own supervisor deal with it. Its
    /// remaining children go down with it through the ordinary cascading
    /// shutdown, and its parent is told through the ordinary `ChildTerminated`
    /// every stopping actor sends — so this does not also notify the parent
    /// itself, which would report the same failure twice.
    ///
    /// Cancellation rather than a stop message, because the two differ in a way
    /// that matters here: a `Terminate` in the inbox is read after everything
    /// queued ahead of it, including restart requests for children this
    /// supervisor has just decided it is no longer going to look after.
    fn stop_self_after_escalation(&self, child: &acton_ern::Ern) {
        warn!(
            "Actor {} is stopping itself because it could not keep child {} running",
            self.id(),
            child
        );

        if let Some(token) = &self.cancellation_token {
            token.cancel();
        }
    }

    /// Drops the IPC names of a child that has reached a terminal state.
    ///
    /// Callers otherwise keep sending into a mailbox nobody reads and are told
    /// nothing; with the names gone they are told there is no such actor.
    ///
    /// # Engine-managed children only, and the reason is not arbitrary
    ///
    /// Restricted to children the supervisor holds a blueprint for — the ones
    /// registered through `supervise_with` and `supervise_deferred`. A child
    /// adopted through the legacy `supervise()` path keeps its names exactly as
    /// it does today.
    ///
    /// That asymmetry is the double-restart firewall wearing different clothes.
    /// The blueprint is what makes a child engine-managed, and blueprints only
    /// reach the registry through two APIs that have never appeared in a
    /// released version, so no existing program can observe any of this. It
    /// looks like an arbitrary special case without that reasoning, which is why
    /// the reasoning is here rather than in a commit message somebody would have
    /// to go looking for.
    ///
    /// `Retired` is deliberately not a caller of this. Three slot states are
    /// terminal and only two of them mean the child is gone:
    /// [`ActorHandle::release`] retires a slot and hands back a child that is
    /// still running and still legitimately reachable.
    ///
    /// [`ActorHandle::release`]: crate::common::ActorHandle::release
    #[cfg(feature = "ipc")]
    fn forget_ipc_names(&self, index: ChildIndex, child: &acton_ern::Ern) {
        let engine_managed = self
            .supervision
            .slot(index)
            .is_some_and(super::registry::ChildSlot::is_restartable);
        if !engine_managed {
            return;
        }

        let forgotten = self.runtime.ipc_forget(child);
        if forgotten > 0 {
            trace!(
                "Actor {} dropped {} IPC name(s) for departed child {}",
                self.id(),
                forgotten,
                child
            );
        }
    }

    /// Carries out a restart plan: stops what it says to stop, and arranges for
    /// what it says to start.
    ///
    /// # One task owns the whole sequence
    ///
    /// A group restart is stop-everything-then-start-everything, and both
    /// halves are ordered. Handing that to a single task rather than spreading
    /// it across the message loop is what keeps the ordering rules in one
    /// readable place, and it is why no group bookkeeping lives in the
    /// registry: there is no rendezvous to arrange.
    ///
    /// [`SupervisionStrategy::OneForOne`] falls out of the same code with
    /// nothing special about it — an empty `stop` and a single-entry `restart`.
    ///
    /// [`SupervisionStrategy::OneForOne`]: super::SupervisionStrategy::OneForOne
    fn schedule_restart(
        &mut self,
        child: &acton_ern::Ern,
        plan: &RestartPlan,
        backoff: BackoffDelay,
    ) {
        let stops = self.begin_group_stops(plan);
        let dues = self.park_group_for_restart(plan);

        trace!(
            "Actor {} will stop {} child(ren) and restart {} after {}, prompted by child {}",
            self.id(),
            stops.len(),
            dues.len(),
            backoff,
            child
        );

        self.spawn_group_restart(stops, dues, backoff);
    }

    /// Marks every child the plan stops and collects handles to stop them with.
    ///
    /// The state goes on before any stop is sent, so that a termination racing
    /// back into the inbox finds a slot that already knows the stop was asked
    /// for. `then_restart` is read straight off the plan: a child in `stop` but
    /// not in `restart` is one the supervisor had to take down for consistency
    /// and cannot recreate, and it must not be resurrected.
    ///
    /// The handle is deliberately **not** cleared here. It is what a cascading
    /// shutdown reaches these children through, and the stop this issues is not
    /// a reason to make them unreachable before they are actually down.
    fn begin_group_stops(&mut self, plan: &RestartPlan) -> Vec<ActorHandle> {
        let mut stops = Vec::with_capacity(plan.stop.len());
        for index in &plan.stop {
            let then_restart = plan.restart.contains(index);
            let Some(slot) = self.supervision.slot_mut(*index) else {
                continue;
            };
            let handle = slot.handle().cloned();
            slot.set_state(SlotState::ExpectedStop { then_restart });
            slot.publish();
            if let Some(handle) = handle {
                stops.push(handle);
            }
        }
        stops
    }

    /// Parks the children that are already down, and names every restart to ask
    /// for when the group's backoff elapses.
    ///
    /// Two groups reach [`SlotState::AwaitingBackoff`] and they arrive by
    /// different roads. A child that is *not* being stopped is already down —
    /// the child that failed, plus any sibling that was down when it did — so
    /// it is parked now. A child that is being stopped gets there when its
    /// termination lands, through
    /// [`SupervisionOutcome::GroupStopLanded`](super::plan::SupervisionOutcome::GroupStopLanded).
    ///
    /// The generation is captured here for both, and that is safe for the ones
    /// still stopping: a generation only advances in
    /// [`complete_start`](SupervisionRegistry::complete_start), which cannot run
    /// for a child on its way down. Captured early and checked late is the whole
    /// point — anything that moves a slot on in the meantime makes the restart
    /// stale, and [`record_restart_due`](Self::record_restart_due) refuses it.
    fn park_group_for_restart(&mut self, plan: &RestartPlan) -> Vec<RestartDue> {
        let mut dues = Vec::with_capacity(plan.restart.len());
        for index in &plan.restart {
            let being_stopped = plan.stop.contains(index);
            let Some(slot) = self.supervision.slot_mut(*index) else {
                continue;
            };
            if !being_stopped {
                slot.set_handle(None);
                slot.set_state(SlotState::AwaitingBackoff);
                slot.publish();
            }
            dues.push(RestartDue {
                child: slot.ern().clone(),
                index: *index,
                generation: slot.generation(),
            });
        }
        dues
    }

    /// Runs a group restart to completion on its own task.
    ///
    /// # Why the restarts cannot outrun the stops
    ///
    /// A child's `ChildTerminated` is delivered to its supervisor's inbox
    /// *before* [`ActorHandle::stop`] returns, and this task sends every
    /// [`RestartDue`] strictly after the last `stop` returned. The supervisor's
    /// inbox is a single-consumer FIFO, so it reads every termination — moving
    /// each slot to [`SlotState::AwaitingBackoff`] — before it reads the first
    /// restart request. The backoff sleep helps nothing here and is not what
    /// makes this safe; the ordering is.
    ///
    /// A child that misses the stop deadline is the one exception. Its
    /// termination may arrive after the restart request, in which case
    /// [`queue_restart`](SupervisionRegistry::queue_restart) refuses the stale
    /// request and the child stays down until the supervisor stops. That is
    /// preferable to waiting on it forever: a child that will not stop must not
    /// be able to hold a group restart open indefinitely.
    ///
    /// # Not tracked with the start tasks
    ///
    /// A start task holds the only handle to a live child, which is why a
    /// shutdown waits for those. This task holds handles to children it is
    /// stopping and then holds nothing at all. Putting it on that tracker would
    /// make every shutdown wait out a backoff whose default ceiling is 30
    /// seconds against a shutdown deadline of 10, so every shutdown with a
    /// pending restart would block for the full 10 and then log an overrun it
    /// did not earn. The rule that falls out is that the tracker is for tasks
    /// holding a live child in order to hand it over, not for every task the
    /// actor spawned.
    ///
    /// It watches the cancellation token instead, so a supervisor going down
    /// does not leave a task sleeping out a backoff for a restart that has
    /// already been abandoned. Even without that the message would simply fail
    /// delivery into a closed inbox; the token just makes it prompt.
    fn spawn_group_restart(
        &self,
        stops: Vec<ActorHandle>,
        dues: Vec<RestartDue>,
        backoff: BackoffDelay,
    ) {
        let supervisor = self.handle.clone();
        let token = self.cancellation_token.clone();
        let delay = backoff.duration();

        tokio::spawn(async move {
            // Reverse start order, each child fully down before the one before
            // it is asked. Sequential rather than concurrent because that
            // ordering is the entire content of the strategy: a later child
            // that depends on an earlier one has to go first.
            for handle in stops {
                stop_group_member(handle).await;
            }

            if let Some(token) = token {
                tokio::select! {
                    () = token.cancelled() => return,
                    () = tokio::time::sleep(delay) => {}
                }
            } else {
                tokio::time::sleep(delay).await;
            }

            // Start order, so an earlier child is back before the ones that
            // may depend on it. Delivery order into a FIFO inbox is what
            // carries that ordering to the supervisor.
            for due in dues {
                let envelope = supervisor.create_envelope(Some(supervisor.reply_address()));
                if let Err(undeliverable) = envelope.try_send(due).await {
                    trace!(
                        "Supervisor {} was gone before a restart came due ({})",
                        supervisor.id(),
                        undeliverable
                    );
                    return;
                }
            }
        });
    }

    /// Queues the restart a timer has just reported due.
    ///
    /// Synchronous, and it starts nothing: the slot goes onto the same
    /// pending-start queue a deferred first start uses, and the top of the next
    /// loop turn hands it to the same start task.
    pub(crate) fn record_restart_due(&mut self, due: &RestartDue) {
        if self
            .supervision
            .queue_restart(due.index, &due.child, due.generation)
        {
            trace!(
                "Actor {} queued a restart of child {} ({})",
                self.id(),
                due.child,
                due.generation
            );
        } else {
            // Not a failure. The slot was retired, restarted by another path,
            // or the supervisor began shutting down and settled it. A timer is
            // the one input that can arrive from a world that no longer exists.
            trace!(
                "Actor {} discarded a stale restart timer for child {} ({})",
                self.id(),
                due.child,
                due.generation
            );
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
                let recorded = self.supervision.complete_start(
                    message.index,
                    &message.child,
                    handle.clone(),
                    std::time::Instant::now(),
                );
                if recorded.is_recorded() {
                    trace!(
                        "Actor {} now supervises child {} ({})",
                        self.id(),
                        message.child,
                        recorded
                    );
                    if recorded == StartRecorded::Restart {
                        #[cfg(feature = "ipc")]
                        self.rebind_ipc_names(&message.child, handle);
                    }
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

    /// Points a restarted child's IPC names at its new mailbox.
    ///
    /// A restart keeps the child's [`Ern`](acton_ern::Ern) and replaces its
    /// mailbox, and `ipc_expose` stored a handle *by value*. Nothing updated it,
    /// so before this an actor exposed under a chosen name became unreachable
    /// over IPC from its first restart onward and reported nothing: sends landed
    /// in a queue with no reader.
    #[cfg(feature = "ipc")]
    fn rebind_ipc_names(&self, child: &acton_ern::Ern, fresh: &ActorHandle) {
        let repointed = self.runtime.ipc_rebind(child, fresh);
        if repointed > 0 {
            trace!(
                "Actor {} repointed {} IPC name(s) at the new incarnation of {}",
                self.id(),
                repointed,
                child
            );
        }
    }

    /// Drops the IPC names of every engine-managed child, on the way down.
    ///
    /// The terminal-state sweep cannot reach these. A cascading shutdown sets
    /// the registry `shutting_down`, which makes every termination an expected
    /// stop, which makes `evaluate` return `Ignore`, so no slot ever reaches
    /// `Down` and nothing calls [`forget_ipc_names`](Self::forget_ipc_names).
    /// A supervisor that stops while its runtime keeps going would therefore
    /// leave its children stopped and their names still pointing at dead
    /// mailboxes — the exact condition the terminal sweep exists to remove,
    /// reached by a different road.
    ///
    /// Engine-managed children only, on the same reasoning as the terminal
    /// sweep.
    #[cfg(feature = "ipc")]
    pub(crate) fn forget_children_ipc_names(&self) {
        for child in self.supervision.engine_managed_children() {
            let forgotten = self.runtime.ipc_forget(&child);
            if forgotten > 0 {
                trace!(
                    "Actor {} dropped {} IPC name(s) for child {} while stopping",
                    self.id(),
                    forgotten,
                    child
                );
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

    // ---- whose limiter governs a child ----------------------------------
    //
    // Precedence is asserted here rather than through a running supervisor
    // because this is the level at which it is visible. From outside, two
    // different allowances only diverge once a child has actually failed the
    // smaller number of times, which turns a question about configuration into
    // a test about restart timing.

    /// A supervisor whose own configuration sets a restart allowance.
    fn supervisor_allowing(
        runtime: &mut ActorRuntime,
        max_restarts: u32,
    ) -> ManagedActor<Started, Supervisor> {
        let config = ActorConfig::new(
            Ern::with_root("pool").expect("'pool' is a valid Ern root"),
            None,
        )
        .with_restart_limiter(RestartLimiterConfig {
            max_restarts,
            ..RestartLimiterConfig::default()
        });

        runtime.new_actor_with_config::<Supervisor>(config).into()
    }

    /// Records a child under `actor`, optionally with an allowance of its own.
    fn queue_child(
        actor: &mut ManagedActor<Started, Supervisor>,
        name: &str,
        max_restarts: Option<u32>,
    ) -> Ern {
        let mut config = ActorConfig::for_supervised_child(name, actor.handle.clone(), None)
            .expect("a name plus a live parent is a valid child configuration");
        if let Some(max_restarts) = max_restarts {
            config = config.with_restart_limiter(RestartLimiterConfig {
                max_restarts,
                ..RestartLimiterConfig::default()
            });
        }
        let child_id = config.id();
        actor
            .supervise_deferred(config, |_child: &mut ManagedActor<Idle, Supervisor>| {})
            .expect("the first registration of a name succeeds");
        child_id
    }

    /// The allowance the supervisor actually recorded for a child.
    fn recorded_allowance(actor: &mut ManagedActor<Started, Supervisor>, child: &Ern) -> u32 {
        actor
            .supervision
            .slot_of_mut(child)
            .expect("the child is supervised")
            .limiter_mut()
            .stats()
            .max_restarts
    }

    #[tokio::test]
    async fn a_child_that_sets_no_allowance_inherits_its_supervisors() {
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = supervisor_allowing(&mut runtime, 9);

        let child = queue_child(&mut actor, "worker", None);

        assert_eq!(recorded_allowance(&mut actor, &child), 9);
    }

    #[tokio::test]
    async fn a_childs_own_allowance_overrides_its_supervisors() {
        // The consequence of the ruling, stated plainly: a child can decline
        // its supervisor's policy. Accepted deliberately — a child that knows
        // it is expensive to rebuild is the thing that knows it — so this is a
        // test of intended behaviour, not a loophole somebody left open.
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = supervisor_allowing(&mut runtime, 2);

        let child = queue_child(&mut actor, "worker", Some(11));

        assert_eq!(
            recorded_allowance(&mut actor, &child),
            11,
            "the child's own setting wins where both are set"
        );
    }

    #[tokio::test]
    async fn a_child_burning_through_its_allowance_leaves_its_siblings_untouched() {
        // What per-slot limiter *instances* buy, and the one most likely to
        // regress silently if a limiter is ever hoisted onto the supervisor:
        // one noisy child must not spend a sibling's budget, or a child that
        // has never failed gets escalated for somebody else's crashes.
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = supervisor_allowing(&mut runtime, 3);

        let noisy = queue_child(&mut actor, "noisy", None);
        let quiet = queue_child(&mut actor, "quiet", None);

        // Spend the noisy child's entire allowance.
        {
            let limiter = actor
                .supervision
                .slot_of_mut(&noisy)
                .expect("the child is supervised")
                .limiter_mut();
            for _ in 0..3 {
                limiter.can_restart().expect("within the allowance");
                let _ = limiter.record_restart();
            }
            assert!(
                limiter.can_restart().is_err(),
                "the noisy child is out of restarts"
            );
        }

        let quiet_limiter = actor
            .supervision
            .slot_of_mut(&quiet)
            .expect("the child is supervised")
            .limiter_mut();
        assert_eq!(
            quiet_limiter.restarts_in_window(),
            0,
            "a sibling's crashes are not charged to this child"
        );
        assert!(
            quiet_limiter.can_restart().is_ok(),
            "and it keeps its full allowance"
        );
    }

    // ---- the double-restart firewall -------------------------------------

    /// Registers a child the way the legacy `supervise()` path does: running,
    /// with **no blueprint**, so its supervisor has no recipe for rebuilding it.
    fn adopt_legacy_child(
        actor: &mut ManagedActor<Started, Supervisor>,
        name: &str,
    ) -> (Ern, watch::Receiver<SupervisionStatus>) {
        let child = actor
            .id()
            .add_part(name)
            .expect("a short name is a valid Ern part");
        let (status, receiver) = status_channel(&child, None);

        actor
            .supervision
            .register(NewSlot {
                ern: child.clone(),
                handle: handle_for(&child),
                spawner: None,
                restart_policy: RestartPolicy::Permanent,
                limiter: RestartLimiter::default(),
                status,
            })
            .expect("the first registration of a name succeeds");

        (child, receiver)
    }

    /// A real `ActorHandle` with a real mailbox, built without a runtime.
    fn handle_for(id: &Ern) -> ActorHandle {
        let (outbox, _inbox) = tokio::sync::mpsc::channel(8);
        ActorHandle::new(id.clone(), outbox)
    }

    #[tokio::test]
    async fn a_child_with_no_blueprint_is_left_down_without_spending_an_allowance() {
        // The firewall that makes the whole engine safe to ship: a child adopted
        // through the legacy `supervise()` path has no blueprint, so it is never
        // restarted, and no program written against a released version can have
        // a child restarted twice.
        //
        // **The load-bearing assertion here is the limiter, not the state.**
        // `evaluate` returns `Forget` for a blueprint-less slot at check 2,
        // *before* the limiter is consulted. Delete that check and the outcome
        // is still `Forget` — `plan_restart` filters on `restartable` too, so
        // the plan comes back empty — and the slot still reaches `Down`. What
        // changes is that the child is charged a restart it can never use, and
        // enough of those escalate a child the supervisor was never going to
        // rebuild. Measured: with check 2 removed, the state assertion below
        // still passes and the limiter assertion fails.
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = supervisor(&mut runtime);
        let (child, receiver) = adopt_legacy_child(&mut actor, "legacy");

        // `Permanent` + `Normal` is the combination that *would* warrant a
        // restart, so the blueprint is the only thing standing in the way.
        actor.record_child_terminated(&ChildTerminated::new(
            child.clone(),
            crate::actor::TerminationReason::Normal,
            RestartPolicy::Permanent,
        ));

        let slot = actor
            .supervision
            .slot_of_mut(&child)
            .expect("the child is still recorded");
        assert_eq!(
            slot.state(),
            SlotState::Down,
            "a child nobody can rebuild is left down"
        );
        assert_eq!(
            slot.limiter_mut().restarts_in_window(),
            0,
            "and is not charged for a restart that was never going to happen"
        );

        assert_eq!(receiver.borrow().state(), SupervisionState::Down);
        assert!(
            !actor.supervision.has_pending_starts(),
            "nothing was queued to rebuild it"
        );
    }

    #[tokio::test]
    async fn a_child_with_a_blueprint_is_restarted_where_a_legacy_one_is_not() {
        // The other side of the same predicate, so the test above is shown to
        // be about the blueprint rather than about something incidental to the
        // fixture. Identical notification, identical policy, one difference.
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = supervisor(&mut runtime);

        let engine_managed = queue_child(&mut actor, "worker", None);
        actor.launch_pending_starts();
        let handle = handle_for(&engine_managed);
        assert!(actor
            .supervision
            .complete_start(
                ChildIndex::new(0),
                &engine_managed,
                handle,
                std::time::Instant::now(),
            )
            .is_recorded());

        actor.record_child_terminated(&ChildTerminated::new(
            engine_managed.clone(),
            crate::actor::TerminationReason::Normal,
            RestartPolicy::Permanent,
        ));

        let slot = actor
            .supervision
            .slot_of_mut(&engine_managed)
            .expect("the child is still recorded");
        assert_eq!(
            slot.state(),
            SlotState::AwaitingBackoff,
            "a child with a blueprint is on its way back"
        );
        assert_eq!(
            slot.limiter_mut().restarts_in_window(),
            1,
            "and this one really is charged for it"
        );
    }

    // ---- group restarts ---------------------------------------------------

    /// A supervisor with a group strategy, one child it can rebuild at index 0
    /// and one adopted child it cannot at index 1, both running.
    ///
    /// The shape that makes `then_restart` observable: the adopted child is in
    /// the plan's `stop` list and not in its `restart` list, which is the only
    /// way that flag is ever `false`.
    fn group_of_one_managed_and_one_adopted(
        actor: &mut ManagedActor<Started, Supervisor>,
    ) -> (Ern, Ern) {
        actor.supervision_strategy = super::super::SupervisionStrategy::OneForAll;

        let managed = queue_child(actor, "worker", None);
        actor.launch_pending_starts();
        assert!(actor
            .supervision
            .complete_start(
                ChildIndex::new(0),
                &managed,
                handle_for(&managed),
                std::time::Instant::now(),
            )
            .is_recorded());

        let (adopted, _status) = adopt_legacy_child(actor, "adopted");

        (managed, adopted)
    }

    #[tokio::test]
    async fn a_group_restart_marks_a_sibling_it_cannot_rebuild_as_not_coming_back() {
        // `then_restart` is read off the plan, and this is the only case where
        // the two lists disagree. Setting it unconditionally to `true` leaves
        // this slot in `ExpectedStop { then_restart: true }`, which becomes
        // `AwaitingBackoff` when the stop lands and then waits forever for a
        // restart the plan never asked for — a caller watching it is told
        // `Restarting` for the rest of the supervisor's life.
        //
        // Measured as slot state rather than as "was it rebuilt", because it is
        // *not* rebuilt either way: no `RestartDue` is ever sent for a child
        // outside the plan's restart list. The resting state is the whole
        // observable difference.
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = supervisor(&mut runtime);
        let (managed, adopted) = group_of_one_managed_and_one_adopted(&mut actor);

        actor.record_child_terminated(&ChildTerminated::new(
            managed,
            crate::actor::TerminationReason::Normal,
            RestartPolicy::Permanent,
        ));

        assert_eq!(
            actor
                .supervision
                .slot_of(&adopted)
                .expect("the adopted child is still recorded")
                .state(),
            SlotState::ExpectedStop {
                then_restart: false
            },
            "a sibling the supervisor holds no blueprint for is stopped and not promised back"
        );
    }

    #[tokio::test]
    async fn a_group_restart_marks_a_sibling_it_can_rebuild_as_coming_back() {
        // The other side of the same read, so the test above is shown to be
        // about the plan rather than about something incidental to the fixture.
        // Identical group, identical failure, one difference: this sibling has
        // a blueprint, so it is in both lists.
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = supervisor(&mut runtime);
        actor.supervision_strategy = super::super::SupervisionStrategy::OneForAll;

        let failed = queue_child(&mut actor, "worker", None);
        let sibling = queue_child(&mut actor, "sibling", None);
        actor.launch_pending_starts();
        for (index, child) in [(0, &failed), (1, &sibling)] {
            assert!(actor
                .supervision
                .complete_start(
                    ChildIndex::new(index),
                    child,
                    handle_for(child),
                    std::time::Instant::now(),
                )
                .is_recorded());
        }

        actor.record_child_terminated(&ChildTerminated::new(
            failed,
            crate::actor::TerminationReason::Normal,
            RestartPolicy::Permanent,
        ));

        assert_eq!(
            actor
                .supervision
                .slot_of(&sibling)
                .expect("the sibling is still recorded")
                .state(),
            SlotState::ExpectedStop { then_restart: true },
        );
    }

    #[tokio::test]
    async fn a_landed_group_stop_moves_a_sibling_to_the_state_its_flag_names() {
        // The engine's half of the crux. `evaluate` reports which half of the
        // group a landed stop belongs to; this is what that report is *for*.
        // A child on its way back has to reach `AwaitingBackoff`, because that
        // is the only state `queue_restart` will accept a restart from — land
        // it anywhere else and the group's own restart is refused as stale.
        for (then_restart, expected) in [
            (true, SlotState::AwaitingBackoff),
            (false, SlotState::Down),
        ] {
            let mut runtime = ActonApp::launch_async().await;
            let mut actor = supervisor(&mut runtime);
            let (child, _receiver) = adopt_legacy_child(&mut actor, "sibling");
            actor
                .supervision
                .slot_of_mut(&child)
                .expect("the child is supervised")
                .set_state(SlotState::ExpectedStop { then_restart });

            actor.record_child_terminated(&ChildTerminated::new(
                child.clone(),
                crate::actor::TerminationReason::Normal,
                RestartPolicy::Permanent,
            ));

            assert_eq!(
                actor
                    .supervision
                    .slot_of(&child)
                    .expect("the child is still recorded")
                    .state(),
                expected,
                "a group stop with then_restart={then_restart} must come to rest in {expected}"
            );
        }
    }

    #[tokio::test]
    async fn a_supervisor_shutting_down_abandons_a_group_restart_instead_of_driving_it() {
        // A shutdown makes every termination expected, and that has to outrank
        // the group. Driving the group here would have the supervisor rebuild,
        // during its own shutdown, children it is in the middle of stopping.
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = supervisor(&mut runtime);
        let (child, _receiver) = adopt_legacy_child(&mut actor, "sibling");
        actor
            .supervision
            .slot_of_mut(&child)
            .expect("the child is supervised")
            .set_state(SlotState::ExpectedStop { then_restart: true });
        actor.supervision.begin_shutdown();

        actor.record_child_terminated(&ChildTerminated::new(
            child.clone(),
            crate::actor::TerminationReason::Normal,
            RestartPolicy::Permanent,
        ));

        assert_eq!(
            actor
                .supervision
                .slot_of(&child)
                .expect("the child is still recorded")
                .state(),
            SlotState::ExpectedStop { then_restart: true },
            "a shutdown leaves the slot alone rather than parking it for a restart \
             that will never be performed"
        );
        assert!(
            !actor.supervision.has_pending_starts(),
            "and nothing is queued to come back"
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
