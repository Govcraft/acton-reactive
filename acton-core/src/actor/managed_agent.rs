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

use crate::common::{AgentHandle, AsyncLifecycleHandler, BrokerRef, HaltSignal, ParentRef, ReactorMap};
use crate::message::Envelope;
use crate::prelude::AgentRuntime;

mod idle;
/// Contains the `Started` type-state marker and associated implementations for running agents.
pub mod started;

/// Represents an agent whose lifecycle and message processing are managed by the Acton framework.
///
/// `ManagedAgent` acts as a runtime wrapper around user-defined agent logic and state (`Model`).
/// It utilizes a type-state pattern via the `AgentState` generic parameter (e.g., [`Idle`], [`started::Started`])
/// to enforce valid operations during different phases of the agent's lifecycle (configuration vs. active processing).
///
/// The framework handles the underlying task spawning, message reception from the `inbox`,
/// dispatching messages to registered `message_handlers`, executing lifecycle hooks
/// (like `before_start`, `after_stop`), and managing graceful shutdown via the `halt_signal`.
///
/// Users typically interact with `ManagedAgent` indirectly through an [`AgentHandle`] after the agent
/// has been started, or directly during the configuration phase (when in the [`Idle`] state)
/// to register message handlers and lifecycle hooks.
///
/// # Type Parameters
///
/// *   `AgentState`: A marker type (e.g., [`Idle`], [`started::Started`]) indicating the current lifecycle state of the agent.
/// *   `Model`: The user-defined type containing the agent's state and associated logic. It must
///     implement `Default`, `Send`, `Debug`, and be `'static`. Message handlers and lifecycle
///     hooks operate on an instance of this `Model`.
pub struct ManagedAgent<AgentState, Model: Default + Send + Debug + 'static> {
    /// Handle for external interaction with this agent.
    pub(crate) handle: AgentHandle,

    /// Optional handle to the parent (supervisor) agent.
    pub(crate) parent: Option<ParentRef>,

    /// Handle to the system message broker.
    pub(crate) broker: BrokerRef,

    /// Signal used for initiating graceful shutdown.
    pub(crate) halt_signal: HaltSignal,

    /// The agent's unique and potentially hierarchical identifier.
    pub(crate) id: Ern,
    /// Reference to the Acton system runtime.
    pub(crate) runtime: AgentRuntime,

    /// The user-defined state and logic associated with this agent.
    ///
    /// This field holds the instance of the type provided as the `Model` generic
    /// parameter. Message handlers (registered via `act_on` in the [`Idle`] state)
    /// and lifecycle hooks (e.g., `before_start`, `after_stop`) receive mutable
    /// access to this `model` to implement the agent's specific behavior and manage its data.
    pub model: Model,

    /// Tracks associated Tokio tasks, primarily the agent's main loop.
    pub(crate) tracker: TaskTracker,

    /// MPSC receiver for incoming messages.
    pub(crate) inbox: Receiver<Envelope>,
    /// Asynchronous hook executed before the agent starts its message loop.
    pub(crate) before_start: AsyncLifecycleHandler<Model>,
    /// Asynchronous hook executed after the agent starts its message loop.
    pub(crate) after_start: AsyncLifecycleHandler<Model>,
    /// Asynchronous hook executed just before the agent stops its message loop.
    pub(crate) before_stop: AsyncLifecycleHandler<Model>,
    /// Asynchronous hook executed after the agent stops its message loop.
    pub(crate) after_stop: AsyncLifecycleHandler<Model>,
    /// Map storing registered message handlers (`TypeId` -> handler function).
    pub(crate) message_handlers: ReactorMap<Model>,
    /// Phantom data to associate the `AgentState` type parameter.
    _actor_state: std::marker::PhantomData<AgentState>,
}

// implement getter functions for ManagedAgent
impl<AgentState, Model: Default + Send + Debug + 'static> ManagedAgent<AgentState, Model> {
    /// Returns a reference to the agent's unique identifier (`Ern`).
    #[inline]
    pub fn id(&self) -> &Ern {
        &self.id
    }

    /// Returns the root name segment of the agent's identifier (`Ern`).
    #[inline]
    pub fn name(&self) -> &str {
        self.id.root.as_str()
    }

    /// Returns a reference to the agent's [`AgentHandle`].
    ///
    /// The handle is the primary means for external interaction with the agent
    /// once it has started.
    #[inline]
    pub fn handle(&self) -> &AgentHandle {
        &self.handle
    }

    /// Returns a reference to the optional parent agent's handle ([`ParentRef`]).
    ///
    /// Returns `None` if this is a top-level agent.
    #[inline]
    pub fn parent(&self) -> &Option<ParentRef> {
        &self.parent
    }

    /// Returns a reference to the system message broker's handle ([`BrokerRef`]).
    #[inline]
    pub fn broker(&self) -> &BrokerRef {
        &self.broker
    }

    /// Returns a reference to the [`AgentRuntime`] this agent belongs to.
    #[inline]
    pub fn runtime(&self) -> &AgentRuntime {
        &self.runtime
    }
}

impl<AgentState, Model: Default + Send + Debug + 'static> Debug
for ManagedAgent<AgentState, Model>
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManagedAgent")
            .field("id", &self.id) // Changed from "key" to "id" for clarity
            .field("model", &self.model) // Optionally include model debug info
            .field("parent", &self.parent)
            .field("broker", &self.broker)
            // Avoid showing channels/handlers in Debug output
            .finish_non_exhaustive() // Indicate not all fields are shown
    }
}
