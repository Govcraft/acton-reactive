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

//! Recreating a supervised child.
//!
//! Restarting an actor means building a new one that behaves like the old one.
//! An [`ActorHandle`] cannot do that: it can send to a mailbox but knows nothing
//! about how the actor behind it was configured. A supervisor that can restart a
//! child therefore holds a [`ChildSpawner`] — the recipe rather than the result.

use std::fmt;
use std::future::Future;
use std::fmt::Debug;
use std::pin::Pin;
use std::sync::Arc;

use acton_ern::Ern;

use super::SupervisionError;
use crate::actor::{ActorConfig, Idle, ManagedActor, RestartPolicy};
use crate::common::{ActorHandle, ActorRuntime};

/// The future returned by [`ChildSpawner::spawn`].
///
/// Boxed because [`ChildSpawner`] is used as a trait object: a supervisor holds
/// children of many different model types in one list, so the concrete future
/// type cannot appear in the signature.
type SpawnFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ActorHandle, SupervisionError>> + Send + 'a>>;

/// The ability to create and start one supervised child, repeatedly.
///
/// Implementors capture a child's configuration and setup closure so that every
/// incarnation is built the same way. The supervisor calls [`spawn`] once at
/// registration and again for each restart.
///
/// [`spawn`]: ChildSpawner::spawn
pub trait ChildSpawner: Send + Sync + Debug {
    /// The identifier every incarnation of this child is created with.
    ///
    /// Stable across restarts: the mailbox is replaced, the identity is not.
    fn child_id(&self) -> &Ern;

    /// The restart policy every incarnation of this child is created with.
    fn restart_policy(&self) -> RestartPolicy;

    /// Creates and starts a fresh incarnation, returning a handle to it.
    ///
    /// `parent` is the supervising actor, so the new child reports its own
    /// termination back to the supervisor that created it.
    fn spawn(&self, runtime: ActorRuntime, parent: ActorHandle) -> SpawnFuture<'_>;
}

/// User-supplied setup applied to each fresh incarnation of a supervised child.
///
/// This is the part a supervisor cannot infer: which handlers and lifecycle
/// hooks the child needs. It is re-run against a brand-new actor on every start,
/// which is why it is a `Fn` rather than a `FnOnce`.
pub type ChildBlueprint<S> = dyn Fn(&mut ManagedActor<Idle, S>) + Send + Sync + 'static;

/// A [`ChildSpawner`] for one concrete model type.
///
/// Holds the child's already-resolved [`ActorConfig`] and re-applies the
/// blueprint to a fresh actor on every start. Reusing the resolved config
/// verbatim is what keeps the child's identity stable across restarts: the
/// `Ern` is not recomputed, it is carried.
pub struct TypedSpawner<S: Default + Send + Debug + 'static> {
    child_id: Ern,
    config: ActorConfig,
    blueprint: Arc<ChildBlueprint<S>>,
}

impl<S: Default + Send + Debug + 'static> TypedSpawner<S> {
    /// Creates a spawner from a resolved configuration and its blueprint.
    pub fn new(config: ActorConfig, blueprint: Arc<ChildBlueprint<S>>) -> Self {
        Self {
            child_id: config.id(),
            config,
            blueprint,
        }
    }
}

impl<S: Default + Send + Debug + 'static> Debug for TypedSpawner<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The blueprint is a closure and has no useful representation.
        f.debug_struct("TypedSpawner")
            .field("child_id", &self.child_id)
            .field("model", &std::any::type_name::<S>())
            .finish_non_exhaustive()
    }
}

impl<S: Default + Send + Debug + 'static> ChildSpawner for TypedSpawner<S> {
    fn child_id(&self) -> &Ern {
        &self.child_id
    }

    fn restart_policy(&self) -> RestartPolicy {
        self.config.restart_policy()
    }

    fn spawn(&self, runtime: ActorRuntime, _parent: ActorHandle) -> SpawnFuture<'_> {
        // The parent link already lives in the resolved config, so the handle
        // passed here is not needed to establish it.
        Box::pin(async move {
            let mut actor = ManagedActor::<Idle, S>::new(Some(&runtime), Some(&self.config));
            (self.blueprint)(&mut actor);
            Ok(actor.start().await)
        })
    }
}
