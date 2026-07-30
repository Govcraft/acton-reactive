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
use std::pin::Pin;

use acton_ern::Ern;

use super::SupervisionError;
use crate::actor::RestartPolicy;
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
pub trait ChildSpawner: Send + Sync + fmt::Debug {
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
