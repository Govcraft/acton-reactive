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

use std::any::TypeId;
use std::fmt::Debug;
use std::future::Future;
use std::mem;

use acton_ern::Ern;
use tokio::sync::mpsc::channel;
use tracing::*;

use crate::actor::{AgentConfig, ManagedAgent, Started};
use crate::common::{
    ActonInner, AgentHandle, AgentRuntime, Envelope, FutureBox, OutboundEnvelope, ReactorItem,
};
use crate::message::MessageContext;
use crate::prelude::ActonMessage;
use crate::traits::AgentHandleInterface;

/// Type-state marker for a [`ManagedAgent`] that has been configured but not yet started.
///
/// When a `ManagedAgent` is in the `Idle` state, it can be configured with message handlers
/// (via [`ManagedAgent::act_on`]) and lifecycle hooks (e.g., [`ManagedAgent::before_start`],
/// [`ManagedAgent::after_stop`]). Once configuration is complete, the agent can be
/// transitioned to the [`Started`](super::started::Started) state by calling [`ManagedAgent::start`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)] // Add common derives
pub struct Idle;

use crate::common::ErrorHandler;
use std::collections::HashMap;

impl<State: Default + Send + Debug + 'static> ManagedAgent<Idle, State> {
    /// Registers an asynchronous message handler for a specific message type `M`.
    ///
    /// This method is called during the agent's configuration phase (while in the `Idle` state).
    /// It associates a specific message type `M` with a closure (`message_processor`) that
    /// will be executed when the agent receives a message of that type after it has started.
    ///
    /// The framework handles the necessary type erasure and downcasting internally. The
    /// provided `message_processor` receives the agent (in the `Started` state) and a
    /// [`MessageContext`] containing the concrete message and metadata.
    ///
    /// # Type Parameters
    ///
    /// *   `M`: The concrete message type this handler will process. Must implement
    ///     [`ActonMessage`], `Clone`, `Send`, `Sync`, and be `'static`.
    ///
    /// # Arguments
    ///
    /// *   `message_processor`: An asynchronous closure that takes the agent (`&mut ManagedAgent<Started, State>`)
    ///     and the message context (`&mut MessageContext<M>`) and returns a `Future`
    ///     (specifically, a [`FutureBox`]). This closure contains the logic for handling messages of type `M`.
    ///
    /// # Returns
    ///
    /// Returns a mutable reference to `self` to allow for method chaining during configuration.
    #[instrument(skip(self, message_processor), level = "debug")]
    #[deprecated(
        note = "act_on for handlers returning () will be deprecated in the next version. Use act_on_result for Result-returning handlers."
    )]
    pub fn act_on<M>(
        &mut self,
        message_processor: impl for<'a> Fn(&'a mut ManagedAgent<Started, State>, &'a mut MessageContext<M>) -> FutureBox
            + Send
            + Sync
            + 'static,
    ) -> &mut Self
    where
        M: ActonMessage + Clone + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<M>();
        trace!(type_name=std::any::type_name::<M>(),type_id=?type_id, " Adding legacy message handler (will be deprecated)");
        let handler_box = Box::new(
            move |actor: &mut ManagedAgent<Started, State>, envelope: &mut Envelope| -> FutureBox {
                if let Some(concrete_msg) = downcast_message::<M>(&*envelope.message) {
                    trace!(
                        "Downcast successful for message type: {}",
                        std::any::type_name::<M>()
                    );
                    let mut msg_context = {
                        let origin_envelope = OutboundEnvelope::new_with_recipient(
                            envelope.reply_to.clone(),
                            envelope.recipient.clone(),
                        );
                        let reply_envelope = OutboundEnvelope::new_with_recipient(
                            envelope.recipient.clone(),
                            envelope.reply_to.clone(),
                        );
                        MessageContext {
                            message: concrete_msg.clone(),
                            timestamp: envelope.timestamp,
                            origin_envelope,
                            reply_envelope,
                        }
                    };
                    message_processor(actor, &mut msg_context)
                } else {
                    error!(
                        type_name = std::any::type_name::<M>(),
                        "Message handler called with incompatible message type (downcast failed)"
                    );
                    Box::pin(async {})
                }
            },
        );
        self.message_handlers
            .insert(type_id, ReactorItem::FutureReactor(handler_box));
        self
    }

    /// Registers an asynchronous message handler for a specific message type `M` that returns a Result (new style, preferred).
    pub fn act_on_result<M, E>(
        &mut self,
        message_processor: impl for<'a> Fn(
                &'a mut ManagedAgent<Started, State>,
                &'a mut MessageContext<M>,
            ) -> crate::common::FutureBoxResult
            + Send
            + Sync
            + 'static,
    ) -> &mut Self
    where
        M: ActonMessage + Clone + Send + Sync + 'static,
        E: std::error::Error + 'static,
    {
        let type_id = TypeId::of::<M>();
        trace!(type_name=std::any::type_name::<M>(),type_id=?type_id, " Adding Result-returning message handler");
        let handler_box = Box::new(
            move |actor: &mut ManagedAgent<Started, State>,
                  envelope: &mut Envelope|
                  -> crate::common::FutureBoxResult {
                if let Some(concrete_msg) = downcast_message::<M>(&*envelope.message) {
                    trace!(
                        "Downcast successful for message type: {}",
                        std::any::type_name::<M>()
                    );
                    let mut msg_context = {
                        let origin_envelope = OutboundEnvelope::new_with_recipient(
                            envelope.reply_to.clone(),
                            envelope.recipient.clone(),
                        );
                        let reply_envelope = OutboundEnvelope::new_with_recipient(
                            envelope.recipient.clone(),
                            envelope.reply_to.clone(),
                        );
                        MessageContext {
                            message: concrete_msg.clone(),
                            timestamp: envelope.timestamp,
                            origin_envelope,
                            reply_envelope,
                        }
                    };
                    message_processor(actor, &mut msg_context)
                } else {
                    error!(
                        type_name = std::any::type_name::<M>(),
                        "Result handler called with incompatible message type (downcast failed)"
                    );
                    Box::pin(async { Ok(()) })
                }
            },
        );
        self.message_handlers
            .insert(type_id, ReactorItem::FutureReactorResult(handler_box));
        self
    }

    /// Registers an asynchronous hook to be executed *after* the agent successfully starts its message loop.
    ///
    /// This hook is called once, shortly after the agent transitions to the `Started` state
    /// and its main task begins processing messages. It receives an immutable reference
    /// to the agent in the `Started` state.
    ///
    /// # Arguments
    ///
    /// * `f`: An asynchronous closure that takes `&ManagedAgent<Started, State>` and returns a `Future`.
    ///
    /// # Returns
    ///
    /// Returns a mutable reference to `self` for chaining.
    pub fn after_start<F, Fut>(&mut self, f: F) -> &mut Self
    where
        F: for<'b> Fn(&'b ManagedAgent<Started, State>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + Sync + 'static,
    {
        self.after_start = Box::new(move |agent| Box::pin(f(agent)) as FutureBox);
        self
    }

    /// Registers an asynchronous hook to be executed *before* the agent starts its message loop.
    ///
    /// This hook is called once, just before the agent's main task (`wake`) is spawned
    /// during the `start` process. It receives an immutable reference to the agent,
    /// technically still in the `Started` state contextually, though the loop hasn't begun.
    ///
    /// # Arguments
    ///
    /// * `f`: An asynchronous closure that takes `&ManagedAgent<Started, State>` and returns a `Future`.
    ///
    /// # Returns
    ///
    /// Returns a mutable reference to `self` for chaining.
    pub fn before_start<F, Fut>(&mut self, f: F) -> &mut Self
    where
        F: for<'b> Fn(&'b ManagedAgent<Started, State>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + Sync + 'static,
    {
        self.before_start = Box::new(move |agent| Box::pin(f(agent)) as FutureBox);
        self
    }

    /// Registers an asynchronous hook to be executed *after* the agent stops processing messages.
    ///
    /// This hook is called once when the agent's main loop terminates gracefully (e.g., upon
    /// receiving a `Terminate` signal or when the inbox closes). It receives an immutable
    /// reference to the agent in the `Started` state context.
    ///
    /// # Arguments
    ///
    /// * `f`: An asynchronous closure that takes `&ManagedAgent<Started, State>` and returns a `Future`.
    ///
    /// # Returns
    ///
    /// Returns a mutable reference to `self` for chaining.
    pub fn after_stop<F, Fut>(&mut self, f: F) -> &mut Self
    where
        F: for<'b> Fn(&'b ManagedAgent<Started, State>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + Sync + 'static,
    {
        self.after_stop = Box::new(move |agent| Box::pin(f(agent)) as FutureBox);
        self
    }

    /// Registers an asynchronous hook to be executed *before* the agent stops processing messages.
    ///
    /// This hook is called once, just before the agent's main loop begins its shutdown sequence
    /// (e.g., after receiving `Terminate` but before fully stopping). It receives an immutable
    /// reference to the agent in the `Started` state.
    ///
    /// # Arguments
    ///
    /// * `f`: An asynchronous closure that takes `&ManagedAgent<Started, State>` and returns a `Future`.
    ///
    /// # Returns
    ///
    /// Returns a mutable reference to `self` for chaining.
    pub fn before_stop<F, Fut>(&mut self, f: F) -> &mut Self
    where
        F: for<'b> Fn(&'b ManagedAgent<Started, State>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + Sync + 'static,
    {
        self.before_stop = Box::new(move |agent| Box::pin(f(agent)) as FutureBox);
        self
    }

    /// Creates the configuration for a new child agent under this agent's supervision.
    ///
    /// This method generates a `ManagedAgent<Idle, State>` instance pre-configured
    /// to be a child of the current agent. It automatically derives a hierarchical
    /// [`Ern`] for the child based on the parent's ID and the provided `name`.
    /// The child inherits the parent's broker reference.
    ///
    /// The returned agent is in the `Idle` state and still needs to be configured
    /// (e.g., with `act_on`, lifecycle hooks) and then started using its `start` method.
    /// The parent agent typically calls `handle.supervise(child_handle)` after the child
    /// is started to register it formally.
    ///
    /// # Arguments
    ///
    /// * `name`: The name segment for the child agent's [`Ern`].
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing a new `ManagedAgent` instance for the child
    /// in the `Idle` state, ready for further configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the child's `Ern` fails or if creating the
    /// `AgentConfig` fails (e.g., parsing the parent ID).
    #[instrument(skip(self))]
    pub async fn create_child(&self, name: String) -> anyhow::Result<ManagedAgent<Idle, State>> {
        // Configure the child with parent and broker references.
        let config = AgentConfig::new(
            Ern::with_root(name)?,               // Child's name segment
            Some(self.handle.clone()),           // Parent handle
            Some(self.runtime.broker().clone()), // Inherited broker handle
        )?;
        // Create the Idle agent using the internal constructor.
        Ok(ManagedAgent::new(&Some(self.runtime().clone()), Some(config)).await)
    }

    // Internal constructor - not part of public API documentation
    #[instrument]
    pub(crate) async fn new(runtime: &Option<AgentRuntime>, config: Option<AgentConfig>) -> Self {
        let mut managed_actor: ManagedAgent<Idle, State> = ManagedAgent::default();

        if let Some(app) = runtime {
            managed_actor.broker = app.0.broker.clone();
            managed_actor.handle.broker = Box::new(Some(app.0.broker.clone()));
        }

        if let Some(config) = &config {
            managed_actor.handle.id = config.id();
            managed_actor.parent = config.parent().clone();
            managed_actor.handle.broker = Box::new(config.get_broker().clone());
            if let Some(broker) = config.get_broker().clone() {
                managed_actor.broker = broker;
            }
        }

        debug_assert!(
            !managed_actor.inbox.is_closed(),
            "Agent mailbox is closed in new"
        );

        trace!("NEW ACTOR: {}", &managed_actor.handle.id());

        managed_actor.runtime = runtime.clone().unwrap_or_else(|| {
            AgentRuntime(ActonInner {
                broker: managed_actor.handle.broker.clone().unwrap_or_default(),
                ..Default::default()
            })
        });

        managed_actor.id = managed_actor.handle.id();

        managed_actor
    }

    /// Starts the agent's processing loop and transitions it to the `Started` state.
    ///
    /// This method consumes the `ManagedAgent` in the `Idle` state. It performs the following actions:
    /// 1.  Transitions the agent's type state from `Idle` to [`Started`](super::started::Started).
    /// 2.  Executes the registered `before_start` lifecycle hook.
    /// 3.  Spawns the agent's main asynchronous task (`wake`) which handles message processing.
    /// 4.  Closes the agent's `TaskTracker` to signal that the main task has been spawned.
    /// 5.  Returns the agent's [`AgentHandle`] for external interaction.
    ///
    /// After this method returns, the agent is running and ready to process messages sent to its handle.
    ///
    /// # Returns
    ///
    /// An [`AgentHandle`] that can be used to interact with the now-running agent.
    #[instrument(skip(self))]
    pub async fn start(mut self) -> AgentHandle {
        trace!("Starting agent: {}", self.id());
        trace!("Model state before start: {:?}", self.model);

        // Take ownership of handlers before converting state.
        let message_handlers = mem::take(&mut self.message_handlers);
        let actor_ref = self.handle.clone(); // Clone handle before consuming self.

        // Convert the agent to the Started state.
        let active_actor: ManagedAgent<Started, State> = self.into();
        // Leak the agent into a static reference for the spawned task.
        // The task itself is responsible for managing the agent's lifetime.
        let actor = Box::leak(Box::new(active_actor));

        trace!("Executing before_start hook for agent: {}", actor.id());
        (actor.before_start)(actor).await; // Execute before_start hook.

        trace!("Spawning main task (wake) for agent: {}", actor.id());
        // Spawn the main message processing loop.
        actor_ref.tracker().spawn(actor.wake(message_handlers));
        // Close the tracker to indicate the main task is launched.
        actor_ref.tracker().close();

        trace!("Agent {} started successfully.", actor_ref.id());
        actor_ref // Return the handle.
    }

    /// Registers an asynchronous error handler for a specific error type `E`.
    ///
    /// This allows the agent to handle errors of type `E` by executing the given closure
    /// whenever a message handler returns an error of this type.
    ///
    /// # Type Parameters
    ///
    /// * `E`: The concrete error type to handle. Must implement `std::error::Error` and be `'static`.
    ///
    /// # Arguments
    /// * `error_handler`: The handler closure executed with agent, envelope, and error reference.
    ///
    /// # Returns
    /// A mutable reference to `self` for chaining.
    pub fn on_error<E>(
        &mut self,
        error_handler: impl for<'a, 'b> Fn(
                &'a mut ManagedAgent<Started, State>,
                &'b mut crate::message::Envelope,
                &'b E,
            ) -> crate::common::FutureBox
            + Send
            + Sync
            + 'static,
    ) -> &mut Self
    where
        E: std::error::Error + 'static,
    {
        use std::any::TypeId;
        // Wrap handler for dynamic dispatch
        use std::sync::Arc;
        let handler_box: Arc<Box<crate::common::ErrorHandler<State>>> =
            Arc::new(Box::new(move |agent, envelope, err| {
                // Downcast the error to &E
                if let Some(specific) = err.downcast_ref::<E>() {
                    error_handler(agent, envelope, specific)
                } else {
                    // If type doesn't match, do nothing
                    Box::pin(async {})
                }
            }));
        self.error_handler_map
            .insert(TypeId::of::<E>(), handler_box);
        self
    }
}

// --- Utility Function ---

/// Attempts to downcast an `ActonMessage` trait object to a concrete type `T`.
///
/// This utility function is used internally by the message dispatch mechanism
/// (specifically within the closure generated by `act_on`) to safely convert
/// a type-erased message (`&dyn ActonMessage`) back into its original concrete type (`&T`).
///
/// # Type Parameters
///
/// * `T`: The concrete message type to attempt downcasting to. Must be `'static`
///   and implement [`ActonMessage`].
///
/// # Arguments
///
/// * `msg`: A reference to the `ActonMessage` trait object.
///
/// # Returns
///
/// * `Some(&T)`: If the trait object `msg` actually holds a value of type `T`.
/// * `None`: If the trait object does not hold a value of type `T`.
pub fn downcast_message<T: ActonMessage + 'static>(msg: &dyn ActonMessage) -> Option<&T> {
    // Use the Any trait's downcast_ref method provided via ActonMessage's supertraits.
    msg.as_any().downcast_ref::<T>()
}

// --- Internal Implementations ---
// (Default, From, default_handler remain internal and undocumented)

impl<State: Default + Send + Debug + 'static> From<ManagedAgent<Idle, State>>
    for ManagedAgent<Started, State>
{
    fn from(value: ManagedAgent<Idle, State>) -> Self {
        // Move all fields from Idle state to Started state.
        ManagedAgent::<Started, State> {
            handle: value.handle,
            parent: value.parent,
            halt_signal: value.halt_signal,
            id: value.id,
            runtime: value.runtime,
            model: value.model,
            tracker: value.tracker,
            inbox: value.inbox,
            before_start: value.before_start,
            after_start: value.after_start,
            before_stop: value.before_stop,
            after_stop: value.after_stop,
            broker: value.broker,
            message_handlers: value.message_handlers,
            error_handler_map: value.error_handler_map, // transfer error handlers
            _actor_state: Default::default(),
        }
    }
}

impl<State: Default + Send + Debug + 'static> Default for ManagedAgent<Idle, State> {
    fn default() -> Self {
        let (outbox, inbox) = channel(255); // Default channel size
        let id: Ern = Default::default();
        let mut handle: crate::common::AgentHandle = Default::default();
        handle.id = id.clone();
        handle.outbox = outbox.clone();

        ManagedAgent::<Idle, State> {
            handle,
            id,
            inbox,
            // Initialize lifecycle hooks with default no-op handlers.
            before_start: Box::new(|_| default_handler()),
            after_start: Box::new(|_| default_handler()),
            before_stop: Box::new(|_| default_handler()),
            after_stop: Box::new(|_| default_handler()),
            model: State::default(),
            broker: Default::default(),
            error_handler_map: std::collections::HashMap::new(),
            parent: Default::default(),
            runtime: Default::default(),
            halt_signal: Default::default(),
            tracker: Default::default(),
            message_handlers: Default::default(),
            _actor_state: Default::default(),
        }
    }
}

// Default no-op async handler for lifecycle events.
fn default_handler() -> FutureBox {
    Box::pin(async {})
}
