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
//! These run on the supervisor's own task, reached from its message loop. They
//! are deliberately **not** `async`: recording a child touches only data the
//! actor already owns, plus a single non-blocking write to the waiting caller's
//! cell. Making registration synchronous is the point of giving the supervisor
//! sole ownership of its registry.
//!
//! They take `&mut self` rather than `&self` for a reason worth keeping: an
//! `async fn(&self)` produces a future holding `&Self`, which is `Send` only if
//! `Self: Sync` — that is, only if the user's model is `Sync`, a bound this
//! crate refuses to impose.

use std::fmt::Debug;

use tracing::{trace, warn};

use std::sync::Arc;

use tokio::sync::watch;

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
