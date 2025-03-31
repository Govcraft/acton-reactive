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
pub mod started;

/// Represents an agent whose lifecycle and message processing loop are managed by the Acton framework.
///
/// This struct encapsulates the agent's state (`Model`), its communication channels (`handle`, `inbox`),
/// lifecycle hooks (`before_start`, `after_stop`, etc.), and message handlers (`message_handlers`).
/// The "Managed" aspect signifies that the framework takes care of spawning the agent's task,
/// receiving messages, dispatching them to the appropriate handlers defined on the `Model`,
/// and managing shutdown signals.
pub struct ManagedAgent<AgentState, Model: Default + Send + Debug + 'static> {
    pub(crate) handle: AgentHandle,

    pub(crate) parent: Option<ParentRef>,

    pub(crate) broker: BrokerRef,

    pub(crate) halt_signal: HaltSignal,

    pub(crate) id: Ern,
    pub(crate) runtime: AgentRuntime,
    /// The user-defined state and logic associated with this agent.
    ///
    /// This field holds the instance of the type provided as the `Model` generic
    /// parameter. Message handlers and lifecycle hooks operate on this `model`
    /// to manage the agent's behavior and data.
    pub model: Model,

    pub(crate) tracker: TaskTracker,

    pub(crate) inbox: Receiver<Envelope>,
    /// Reactor called when the actor wakes up but before listening begins.
    pub(crate) before_start: AsyncLifecycleHandler<Model>,
    /// Reactor called when the actor wakes up but before listening begins.
    pub(crate) after_start: AsyncLifecycleHandler<Model>,
    /// Reactor called just before the actor stops listening for messages.
    pub(crate) before_stop: AsyncLifecycleHandler<Model>,
    /// Reactor called when the actor stops listening for messages.
    pub(crate) after_stop: AsyncLifecycleHandler<Model>,
    /// Map of message handlers for processing different message types.
    pub(crate) message_handlers: ReactorMap<Model>,
    _actor_state: std::marker::PhantomData<AgentState>,
}

// implement getter functions for ManagedAgent
impl<AgentState, Model: Default + Send + Debug + 'static> ManagedAgent<AgentState, Model> {
    /// Returns the unique identifier of the actor.
    pub fn id(&self) -> &Ern {
        &self.id
    }
    /// Returns the name of the actor.
    pub fn name(&self) -> &str {
        self.id.root.as_str()
    }

    /// Returns the handle of the actor.
    pub fn handle(&self) -> &AgentHandle {
        &self.handle
    }

    /// Returns the parent of the actor.
    pub fn parent(&self) -> &Option<ParentRef> {
        &self.parent
    }
    /// Returns the broker of the actor.
    pub fn broker(&self) -> &BrokerRef {
        &self.broker
    }
    /// Returns the app runtime.
    pub fn runtime(&self) -> &AgentRuntime {
        &self.runtime
    }
}

impl<AgentState, Model: Default + Send + Debug + 'static> Debug
for ManagedAgent<AgentState, Model>
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManagedAgent")
            .field("key", &self.id)
            .finish()
    }
}
