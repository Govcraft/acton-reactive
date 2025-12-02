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

use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;

use acton_ern::prelude::*;
use tokio::sync::mpsc::Receiver;
use tokio_util::task::TaskTracker;

pub use idle::Idle;

use crate::actor::{RestartPolicy, SupervisionStrategy};
use crate::common::{
    ActorHandle, AsyncLifecycleHandler, BrokerRef, HaltSignal, ParentRef, ReactorMap,
};
use crate::message::Envelope;
use crate::prelude::ActorRuntime;

mod idle;
/// Contains the `Started` type-state marker and associated implementations for running actors.
pub mod started;

/// Represents an actor whose lifecycle and message processing are managed by the Acton framework.
///
/// `ManagedActor` acts as a runtime wrapper around user-defined actor logic and state (`Model`).
/// It utilizes a type-state pattern via the `ActorState` generic parameter (e.g., [`Idle`], [`started::Started`])
/// to enforce valid operations during different phases of the actor's lifecycle (configuration vs. active processing).
///
/// The framework handles the underlying task spawning, message reception from the `inbox`,
/// dispatching messages to registered `message_handlers`, executing lifecycle hooks
/// (like `before_start`, `after_stop`), and managing graceful shutdown via the `halt_signal`.
///
/// Users typically interact with `ManagedActor` indirectly through an [`ActorHandle`] after the actor
/// has been started, or directly during the configuration phase (when in the [`Idle`] state)
/// to register message handlers and lifecycle hooks.
///
/// # Type Parameters
///
/// *   `ActorState`: A marker type (e.g., [`Idle`], [`started::Started`]) indicating the current lifecycle state of the actor.
/// *   `Model`: The user-defined type containing the actor's state and associated logic. It must
///     implement `Default`, `Send`, `Debug`, and be `'static`. Message handlers and lifecycle
///     hooks operate on an instance of this `Model`.
pub struct ManagedActor<ActorState, Model: Default + Send + Debug + 'static> {
    /// Handle for external interaction with this actor.
    pub(crate) handle: ActorHandle,

    /// Optional handle to the parent (supervisor) actor.
    pub(crate) parent: Option<ParentRef>,

    /// Handle to the system message broker.
    pub(crate) broker: BrokerRef,

    /// Signal used for initiating graceful shutdown.
    pub(crate) halt_signal: HaltSignal,

    /// The actor's unique and potentially hierarchical identifier.
    pub(crate) id: Ern,
    /// Reference to the Acton system runtime.
    pub(crate) runtime: ActorRuntime,

    /// The user-defined state and logic associated with this actor.
    ///
    /// This field holds the instance of the type provided as the `Model` generic
    /// parameter. Message handlers (registered via `act_on` in the [`Idle`] state)
    /// and lifecycle hooks (e.g., `before_start`, `after_stop`) receive mutable
    /// access to this `model` to implement the actor's specific behavior and manage its data.
    pub model: Model,

    /// Tracks associated Tokio tasks, primarily the actor's main loop.
    pub(crate) tracker: TaskTracker,

    /// MPSC receiver for incoming messages.
    pub(crate) inbox: Receiver<Envelope>,
    /// Asynchronous hook executed before the actor starts its message loop.
    pub(crate) before_start: AsyncLifecycleHandler<Model>,
    /// Asynchronous hook executed after the actor starts its message loop.
    pub(crate) after_start: AsyncLifecycleHandler<Model>,
    /// Asynchronous hook executed just before the actor stops its message loop.
    pub(crate) before_stop: AsyncLifecycleHandler<Model>,
    /// Asynchronous hook executed after the actor stops its message loop.
    pub(crate) after_stop: AsyncLifecycleHandler<Model>,
    /// Map storing registered mutable message handlers (`TypeId` -> handler function).
    pub(crate) message_handlers: ReactorMap<Model>,
    /// Map storing registered read-only message handlers (`TypeId` -> handler function).
    pub(crate) read_only_handlers: ReactorMap<Model>,
    /// Map storing registered error handlers (`TypeId` -> error handler closure, wrapped in Arc for clone safety).
    pub(crate) error_handler_map: std::collections::HashMap<
        (std::any::TypeId, std::any::TypeId),
        Box<crate::common::ErrorHandler<Model>>,
    >,
    /// Tokio cancellation token for managing shutdown/cancellation.
    /// This will be set to Some(token) when the actor is created, by cloning the root token.
    pub(crate) cancellation_token: Option<tokio_util::sync::CancellationToken>,
    /// The restart policy for this actor when supervised.
    /// Determines whether and when the actor should be restarted after termination.
    pub(crate) restart_policy: RestartPolicy,
    /// The supervision strategy for managing child actors.
    /// Determines how to handle child terminations (`OneForOne`, `OneForAll`, `RestForOne`).
    pub(crate) supervision_strategy: SupervisionStrategy,
    /// Phantom data to associate the `ActorState` type parameter.
    _actor_state: std::marker::PhantomData<ActorState>,
}

// implement getter functions for ManagedActor
impl<ActorState, Model: Default + Send + Debug + 'static> ManagedActor<ActorState, Model> {
    /// Returns a reference to the actor's unique identifier (`Ern`).
    #[inline]
    pub const fn id(&self) -> &Ern {
        &self.id
    }

    /// Returns the root name segment of the actor's identifier (`Ern`).
    #[inline]
    pub fn name(&self) -> &str {
        self.id.root.as_str()
    }

    /// Returns a reference to the actor's [`ActorHandle`].
    ///
    /// The handle is the primary means for external interaction with the actor
    /// once it has started.
    #[inline]
    pub const fn handle(&self) -> &ActorHandle {
        &self.handle
    }

    /// Returns a reference to the optional parent actor's handle ([`ParentRef`]).
    ///
    /// Returns `None` if this is a top-level actor.
    #[inline]
    pub const fn parent(&self) -> &Option<ParentRef> {
        &self.parent
    }

    /// Returns a reference to the system message broker's handle ([`BrokerRef`]).
    #[inline]
    pub const fn broker(&self) -> &BrokerRef {
        &self.broker
    }

    /// Returns a reference to the [`ActorRuntime`] this actor belongs to.
    #[inline]
    pub const fn runtime(&self) -> &ActorRuntime {
        &self.runtime
    }
}

impl<ActorState, Model: Default + Send + Debug + 'static> Debug
    for ManagedActor<ActorState, Model>
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManagedActor")
            .field("id", &self.id) // Changed from "key" to "id" for clarity
            .field("model", &self.model) // Optionally include model debug info
            .field("parent", &self.parent)
            .field("broker", &self.broker)
            // Avoid showing channels/handlers in Debug output
            .finish_non_exhaustive() // Indicate not all fields are shown
    }
}
