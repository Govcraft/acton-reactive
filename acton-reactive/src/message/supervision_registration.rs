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
//! The same route carries the answer back when a supervisor starts a child
//! itself: the start runs on its own task and reports through
//! [`SupervisedChildStarted`].
//!
//! Every message here is crate-internal and never reaches the prelude. They are
//! intercepted by the actor's message loop before handler dispatch, so a user
//! cannot register a handler for them.

use std::fmt::Debug;
use std::sync::Arc;

use acton_ern::Ern;
use tokio::sync::{watch, SetOnce};

use crate::actor::{
    ChildIndex, ChildSpawner, RestartPolicy, SupervisionError, SupervisionStatus,
};
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

/// Reports the outcome of a start the supervisor itself asked for.
///
/// A supervisor does not build its children on its own task: it launches a
/// start task and carries on taking messages. This is how the answer gets back,
/// and it travels the same way every other answer does.
///
/// # Undeliverable means "stop the child"
///
/// The `Ok` case carries the only handle to a live actor. If this message
/// cannot be delivered — the supervisor stopped, its inbox closed — the start
/// task must stop that child rather than drop the handle, because dropping it
/// leaves an actor running that nothing can reach. That obligation belongs to
/// whoever holds the message before it is delivered, which is why the handle
/// travels inside it rather than being registered anywhere first.
#[derive(Debug, Clone)]
pub struct SupervisedChildStarted {
    /// The child's identifier, so the report can be matched to its slot by
    /// identity and not by position alone.
    pub child: Ern,

    /// The slot the supervisor recorded before the start was launched.
    pub index: ChildIndex,

    /// The started child, or why it could not be started.
    pub outcome: Result<ActorHandle, SupervisionError>,
}

/// The cell a caller waits on when releasing a child.
///
/// Carries the released child's handle back, because the caller is the one who
/// decides what happens to it next: [`ActorHandle::unsupervise`] stops it,
/// [`ActorHandle::release`] hands it to you still running. `None` where the
/// supervisor held no handle, which means the child was already down.
///
/// [`ActorHandle::unsupervise`]: crate::common::ActorHandle::unsupervise
/// [`ActorHandle::release`]: crate::common::ActorHandle::release
pub type ReleaseOutcome = Arc<SetOnce<Result<Option<ActorHandle>, SupervisionError>>>;

/// Asks a supervisor to stop looking after a child.
///
/// Releasing a child and stopping it are separate decisions, and only the
/// caller knows which it wants. The supervisor always does the same thing here
/// — retire the slot, hand the handle back — and the caller stops the child or
/// does not. What the supervisor *does* need to know is which was intended,
/// because it is the only side that can reach the IPC registry.
#[derive(Debug, Clone)]
pub struct UnregisterSupervisedChild {
    /// The child to release.
    pub child: Ern,

    /// Whether the caller is going to stop the child once it has it back.
    ///
    /// Not the supervisor's business except in one respect: a child that is
    /// being stopped should stop answering to its IPC names, and the supervisor
    /// is the side holding the runtime that owns that registry.
    pub stopping: bool,

    /// Where to report whether the child was released, and its handle.
    ///
    /// Not optional: unlike registration, this can genuinely fail — the child
    /// may not be supervised at all — and the caller has no other way to find
    /// out.
    pub outcome: ReleaseOutcome,

    /// Proof that this message still exists.
    ///
    /// Carried solely so the caller can tell that it *stopped* existing. A
    /// supervisor that ends its task with this message still queued drops the
    /// envelope, dropping this sender with it, and the caller's receiver
    /// observes the closure.
    ///
    /// Without it the caller would wait forever: it holds the outcome cell
    /// alive through its own `Arc`, and a `SetOnce` has no notion of a sender
    /// going away. Unlike registration, this message creates no other channel
    /// whose lifetime tracks the supervisor's.
    pub liveness: watch::Sender<()>,
}
