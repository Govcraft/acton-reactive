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

//! Asking a supervisor to take a child on, or let one go.
//!
//! A supervisor owns its children's records privately, so nothing outside its
//! task can write to them. Registration therefore travels as a message: the
//! caller starts the child, then asks the supervisor to record it, and the
//! supervisor does so on its own task in message order.
//!
//! Both messages are crate-internal and never reach the prelude. They are
//! intercepted by the actor's message loop before handler dispatch, so a user
//! cannot register a handler for them.

use std::fmt::Debug;
use std::sync::Arc;

use acton_ern::Ern;
use tokio::sync::{watch, SetOnce};

use crate::actor::{ChildSpawner, RestartPolicy, SupervisionError, SupervisionStatus};
use crate::common::ActorHandle;

/// The cell a caller waits on for the result of its registration.
///
/// An [`Arc`] is load-bearing rather than incidental. [`SetOnce`]'s own `Clone`
/// snapshots the current value into a fresh, independent cell, so a bare
/// `SetOnce` in a message would hand the supervisor a cell the caller can never
/// observe. Sharing the one cell is what makes the answer visible.
pub type RegistrationOutcome = Arc<SetOnce<Result<(), SupervisionError>>>;

/// Asks a supervisor to record a child it should look after.
///
/// The child has already been created and started by the caller; this only asks
/// the supervisor to take responsibility for it.
#[derive(Debug, Clone)]
pub struct RegisterSupervisedChild {
    /// The child's identifier.
    pub child: Ern,

    /// A handle to the running child.
    pub handle: ActorHandle,

    /// How to recreate the child, or `None` when the supervisor cannot.
    ///
    /// `None` on the legacy `supervise()` path: the supervisor is told when the
    /// child terminates but has no recipe for building another one.
    pub spawner: Option<Arc<dyn ChildSpawner>>,

    /// Whether this child warrants a restart, and when.
    pub restart_policy: RestartPolicy,

    /// The publishing end of the child's status channel.
    ///
    /// Bare rather than wrapped in an [`Arc`], because [`watch::Sender`]'s
    /// `Clone` shares the real channel. The count of live senders is meaningful:
    /// once every sender is dropped, watchers learn the supervisor is gone. A
    /// lingering clone of this message would keep that channel artificially
    /// open, so the supervisor must not retain registration envelopes.
    pub status: watch::Sender<SupervisionStatus>,

    /// Where to report whether the registration succeeded.
    ///
    /// `None` when the caller has nothing to learn: the legacy `supervise()`
    /// path cannot fail, because every child it registers carries a freshly
    /// minted identifier that cannot collide.
    pub outcome: Option<RegistrationOutcome>,
}

/// Asks a supervisor to stop looking after a child.
#[derive(Debug, Clone)]
pub struct UnregisterSupervisedChild {
    /// The child to release.
    pub child: Ern,

    /// Where to report whether the child was released.
    ///
    /// Not optional: unlike registration, this can genuinely fail — the child
    /// may not be supervised at all — and the caller has no other way to find
    /// out.
    pub outcome: RegistrationOutcome,
}
