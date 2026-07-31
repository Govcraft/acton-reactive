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
use std::fmt::Debug;
use std::future::Future;
use std::hash::{Hash, Hasher};

use acton_ern::Ern;
use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, watch, SetOnce};
use tokio_util::task::TaskTracker;
use tracing::{error, instrument, trace, warn}; // warn seems unused

use crate::actor::{
    status_channel, ActorConfig, ChildBlueprint, ChildSpawner, Idle, ManagedActor, RestartGeneration,
    SupervisedChild, SupervisionError, SupervisionState, SupervisionStatus, TypedSpawner,
};
use crate::common::{ActorRuntime, ActorSender, BrokerRef, OutboundEnvelope};
use crate::message::{
    BrokerRequest, CascadeTerminate, MessageAddress, RegisterSupervisedChild, RegistrationOutcome,
    SystemSignal, UnregisterSupervisedChild,
};
use crate::prelude::ActonMessage;
use crate::traits::{ActorHandleInterface, Broadcaster, Subscriber};

/// A clonable handle for interacting with an actor.
///
/// `ActorHandle` provides the primary mechanism for communicating with and managing
/// an actor from outside its own execution context. It encapsulates the necessary
/// information to send messages to the actor's inbox (`outbox`), identify the actor (`id`),
/// manage its lifecycle (`stop`), track its tasks (`tracker`), and navigate the
/// supervision hierarchy (`parent`, `children`, `supervise`).
///
/// Handles can be cloned freely, allowing multiple parts of the system to hold references
/// to the same actor. Sending messages through the handle is asynchronous.
///
/// Key functionalities are exposed through implemented traits:
/// *   [`ActorHandleInterface`]: Core methods for interaction (sending messages, stopping, etc.).
/// *   [`Broker`]: Methods for broadcasting messages via the system broker.
/// *   [`Subscriber`]: Method for accessing the system broker handle.
///
/// Equality and hashing are based solely on the actor's unique identifier (`id`).
#[derive(Debug, Clone)]
pub struct ActorHandle {
    /// The unique identifier (`Ern`) for the actor this handle refers to.
    pub(crate) id: Ern,
    /// The sender part of the MPSC channel connected to the actor's inbox.
    pub(crate) outbox: ActorSender,
    /// Tracks the actor's main task and potentially other associated tasks.
    tracker: TaskTracker,
    /// Optional reference to the parent (supervisor) actor's handle
    /// ([`ParentRef`](crate::common::ParentRef)).
    /// `None` if this is a top-level actor. Boxed to manage `ActorHandle` size.
    pub parent: Option<Box<Self>>,
    /// Optional reference to the system message broker's handle ([`BrokerRef`]).
    /// Boxed to manage `ActorHandle` size.
    pub broker: Box<Option<Self>>,
    /// A map holding handles to the direct children supervised by this actor.
    /// Keys are the string representation of the child actor's `Ern`.
    children: DashMap<String, Self>,
    /// The actor's cancellation token (clone).
    pub(crate) cancellation_token: tokio_util::sync::CancellationToken,
}

impl ActorHandle {
    /// Creates a new `ActorHandle` with the provided components.
    ///
    /// This constructor is more efficient than `Default::default()` as it accepts
    /// the channel sender directly instead of creating a throwaway channel.
    #[inline]
    pub(crate) fn new(id: Ern, outbox: ActorSender) -> Self {
        Self {
            id,
            outbox,
            tracker: TaskTracker::new(),
            parent: None,
            broker: Box::new(None),
            children: DashMap::new(),
            cancellation_token: tokio_util::sync::CancellationToken::new(),
        }
    }

    /// Creates a placeholder `ActorHandle` for use as a default broker reference.
    ///
    /// This is a minimal allocation for actors that don't have an assigned broker yet.
    /// The handle is not usable for messaging until a real broker is assigned.
    #[inline]
    pub(crate) fn placeholder() -> Self {
        use crate::common::config::CONFIG;

        let dummy_channel_size = CONFIG.limits.dummy_channel_size;
        let (outbox, _) = mpsc::channel(dummy_channel_size);
        Self {
            id: Ern::default(),
            outbox,
            tracker: TaskTracker::new(),
            parent: None,
            broker: Box::new(None),
            children: DashMap::new(),
            cancellation_token: tokio_util::sync::CancellationToken::new(),
        }
    }
}

impl Default for ActorHandle {
    /// Creates a default, placeholder `ActorHandle`.
    ///
    /// This handle is typically initialized with a default `Ern`, a closed channel,
    /// and no parent, broker, or children. It's primarily used as a starting point
    /// before being properly configured when a `ManagedActor` is created.
    fn default() -> Self {
        Self::placeholder()
    }
}

/// Implements the `Subscriber` trait, allowing access to the broker.
impl Subscriber for ActorHandle {
    /// Returns a clone of the optional broker handle associated with this actor.
    ///
    /// Returns `None` if the actor was not configured with a broker reference.
    fn get_broker(&self) -> Option<BrokerRef> {
        *self.broker.clone() // Clone the Option<BrokerRef> inside the Box
    }
}

/// Implements equality comparison based on the actor's unique ID (`Ern`).
impl PartialEq for ActorHandle {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

/// Derives `Eq` based on the `PartialEq` implementation.
impl Eq for ActorHandle {}

/// Implements hashing based on the actor's unique ID (`Ern`).
impl Hash for ActorHandle {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl ActorHandle {
    /// Starts a child actor and registers it under this actor's supervision.
    ///
    /// This method takes a `ManagedActor` configured in the [`Idle`] state,
    /// starts its execution by calling its `start` method, and then stores
    /// the resulting child `ActorHandle` in this parent handle's `children` map.
    ///
    /// # Type Parameters
    ///
    /// *   `State`: The user-defined state type of the child actor. Must implement
    ///     `Default`, `Send`, `Debug`, and be `'static`.
    ///
    /// # Arguments
    ///
    /// *   `child`: The [`ManagedActor<Idle, State>`] instance representing the child actor
    ///     to be started and supervised.
    ///
    /// # Returns
    ///
    /// A `Result` containing:
    /// *   `Ok(ActorHandle)`: The handle of the successfully started and registered child actor.
    /// *   `Err(anyhow::Error)`: If starting the child actor fails.
    #[instrument(skip(self, child))] // Skip child in instrument
    pub async fn supervise<State: Default + Send + Debug + 'static>(
        // Add 'static bound
        &self,
        child: ManagedActor<Idle, State>,
    ) -> anyhow::Result<Self> {
        let child_id = child.id().clone(); // Get ID before consuming child
        let restart_policy = child.restart_policy; // Read before `start` consumes the child
        trace!("Supervising child actor with id: {}", child_id);
        let handle = child.start().await; // Start the child actor
        trace!(
            "Child actor {} started, adding to parent {} children map",
            child_id,
            self.id
        );
        self.children.insert(handle.id.to_string(), handle.clone()); // Store child handle

        // Ask the supervising actor's own task to record the child, so that it
        // is stopped when the supervisor stops even if this handle is a clone
        // whose `children` map the actor's task cannot see.
        //
        // The receiving end is dropped: `supervise` returns a bare handle, so
        // there is no caller to hand it to. Publishing into a channel with no
        // watchers is harmless, and the alternative would change this method's
        // long-standing signature.
        let (status, _unwatched) = watch::channel(SupervisionStatus::new(
            child_id.clone(),
            Some(handle.clone()),
            RestartGeneration::FIRST,
            SupervisionState::Starting,
            0,
        ));

        self.send(RegisterSupervisedChild {
            child: child_id,
            handle: handle.clone(),
            // No blueprint: a child adopted this way cannot be recreated, so it
            // is reported when it terminates but never restarted.
            spawner: None,
            restart_policy,
            status,
            // Nothing to report back. Every child registered here carries a
            // freshly minted identifier, so registration cannot collide.
            outcome: None,
        })
        .await;

        Ok(handle)
    }

    /// Sends `signal` to the actor and waits for its task to finish.
    ///
    /// The shared body of [`stop`](ActorHandleInterface::stop) and
    /// [`stop_for_parent_shutdown`](Self::stop_for_parent_shutdown). Written
    /// once so the two stops cannot drift apart: they must differ only in the
    /// message sent, because that message is the sole thing that decides which
    /// [`TerminationReason`](crate::actor::TerminationReason) the actor records.
    async fn stop_with_signal<M: ActonMessage + 'static>(
        &self,
        signal: M,
    ) -> anyhow::Result<()> {
        let tracker = self.tracker();

        // Create an envelope to send the signal from self to self.
        let self_envelope = self.create_envelope(Some(self.reply_address()));

        trace!(actor = %self.id, signal = ?signal, "Sending stop signal");
        self_envelope.send(signal).await;

        // Wait for the actor's main task and any tracked tasks to finish.
        tracker.wait().await;

        trace!(actor = %self.id, "Actor terminated successfully.");
        Ok(())
    }

    /// Stops this actor because the supervisor above it is shutting down.
    ///
    /// Identical to [`stop`](ActorHandleInterface::stop) in every observable
    /// respect except the termination reason the actor records:
    /// [`ParentShutdown`] rather than [`Normal`]. Restarts are never warranted
    /// for the former, which is what keeps a supervisor from restarting the very
    /// children it is in the middle of stopping.
    ///
    /// Crate-internal: only a genuine framework-driven cascade may claim this
    /// reason. Callers reaching for a stop should use
    /// [`stop`](ActorHandleInterface::stop).
    ///
    /// [`ParentShutdown`]: crate::actor::TerminationReason::ParentShutdown
    /// [`Normal`]: crate::actor::TerminationReason::Normal
    pub(crate) async fn stop_for_parent_shutdown(&self) -> anyhow::Result<()> {
        self.stop_with_signal(CascadeTerminate).await
    }
}

impl ActorHandle {
    /// Starts a child under this actor's supervision and records how to rebuild
    /// it.
    ///
    /// Unlike [`supervise`](Self::supervise), which adopts an actor you built
    /// yourself, this stores a blueprint: the configuration plus your `configure`
    /// closure, re-applied to a fresh actor on every start. That is what lets the
    /// supervisor recreate the child later.
    ///
    /// The child is built and started here, then registered by asking the
    /// supervising actor to record it. This resolves once that actor has done so.
    ///
    /// # Deadlock
    ///
    /// Do **not** call this on your own handle from inside your own message
    /// handler. Registration is a message, and your actor cannot process it
    /// until your handler returns, so the wait would never end.
    ///
    /// To supervise from inside your own handler, use
    /// [`ManagedActor::supervise_deferred`] on the actor the handler is given.
    /// It records the child synchronously and leaves the start to the message
    /// loop, so there is no acknowledgement to wait for. Two other things work:
    ///
    /// - Start children before the supervisor begins handling messages — build
    ///   and supervise them during setup, ahead of `start()`.
    /// - Supervise through a handle held outside the handler, from a task or
    ///   `main` that is not the supervisor's own message loop.
    ///
    /// [`ManagedActor::supervise_deferred`]: crate::actor::ManagedActor::supervise_deferred
    ///
    /// # Why the `runtime` argument
    ///
    /// Creating an actor requires the runtime that owns the broker and the
    /// cancellation hierarchy, and a handle does not carry one. The
    /// `ManagedActor` form takes no `runtime` because it already has its own.
    ///
    /// # Errors
    ///
    /// - [`SupervisionError::DuplicateChild`] if this actor already supervises
    ///   that identifier. The freshly started child is stopped before returning.
    /// - [`SupervisionError::SupervisorStopped`] if the supervising actor stops
    ///   before recording the registration.
    /// - [`SupervisionError::RegistrationLost`] if the supervisor's task ends
    ///   with the registration still unprocessed.
    pub async fn supervise_with<S: Default + Send + Debug + 'static>(
        &self,
        runtime: &ActorRuntime,
        config: ActorConfig,
        configure: impl Fn(&mut ManagedActor<Idle, S>) + Send + Sync + 'static,
    ) -> Result<SupervisedChild, SupervisionError> {
        let blueprint: Arc<ChildBlueprint<S>> = Arc::new(configure);
        let spawner: Arc<dyn ChildSpawner> = Arc::new(TypedSpawner::new(config, blueprint));

        let child_id = spawner.child_id().clone();
        let restart_policy = spawner.restart_policy();
        let handle = spawner.spawn(runtime.clone(), self.clone()).await?;

        let (status, mut receiver) = status_channel(&child_id, Some(handle.clone()));
        let outcome: RegistrationOutcome = Arc::new(SetOnce::new());

        self.send(RegisterSupervisedChild {
            child: child_id.clone(),
            handle: handle.clone(),
            spawner: Some(spawner),
            restart_policy,
            status,
            outcome: Some(Arc::clone(&outcome)),
        })
        .await;

        // Three ways this can end, and exactly one of them always happens.
        // Without all three the caller could wait forever on a cell the
        // supervisor is no longer able to fill.
        let result = {
            let closed = async {
                // Only completes when every sender is gone, which means the
                // supervisor dropped the registration without recording it.
                while receiver.changed().await.is_ok() {}
            };

            tokio::select! {
                answer = outcome.wait() => answer.clone(),
                () = self.cancellation_token.cancelled() => Err(SupervisionError::SupervisorStopped {
                    supervisor: self.id.clone(),
                }),
                () = closed => outcome.get().map_or_else(
                    || Err(SupervisionError::RegistrationLost { child: child_id.clone() }),
                    Clone::clone,
                ),
            }
        };

        if let Err(error) = result {
            // Nothing is supervising this child, so it must not be left running.
            let _ = handle.stop().await;
            return Err(error);
        }

        Ok(SupervisedChild::new(child_id, self.id.clone(), receiver))
    }

    /// Stops a supervised child and removes it from supervision.
    ///
    /// Carries the same deadlock caveat as
    /// [`supervise_with`](Self::supervise_with): do not call it on your own
    /// handle from inside your own handler. Unlike registration, releasing has
    /// no synchronous counterpart yet — the restart engine lands one.
    ///
    /// # Errors
    ///
    /// - [`SupervisionError::UnknownChild`] if that child is not supervised.
    /// - [`SupervisionError::SupervisorStopped`] if the supervisor stops first.
    pub async fn unsupervise(&self, child: &Ern) -> Result<(), SupervisionError> {
        let outcome: RegistrationOutcome = Arc::new(SetOnce::new());
        // The caller keeps the receiver; the sender rides along inside the
        // message. An actor that stops without processing the message drops the
        // envelope, and that is the only signal the caller gets — a supervisor
        // terminating normally never cancels its own token.
        let (liveness, mut alive) = watch::channel(());

        self.send(UnregisterSupervisedChild {
            child: child.clone(),
            outcome: Arc::clone(&outcome),
            liveness,
        })
        .await;

        tokio::select! {
            answer = outcome.wait() => answer.clone(),
            () = self.cancellation_token.cancelled() => Err(SupervisionError::SupervisorStopped {
                supervisor: self.id.clone(),
            }),
            () = async { while alive.changed().await.is_ok() {} } => {
                // The message was handled and then dropped, which closes the
                // channel too. Check the cell before concluding it was lost.
                outcome.get().map_or_else(
                    || Err(SupervisionError::ReleaseLost { child: child.clone() }),
                    Clone::clone,
                )
            }
        }
    }
}

/// Implements the `Broadcaster` trait, allowing broadcasting via the associated broker.
impl Broadcaster for ActorHandle {
    /// Sends a message to the associated system broker for broadcasting.
    ///
    /// This method wraps the provided `message` in a [`BrokerRequest`] and sends it
    /// to the broker handle stored within this `ActorHandle`. If no broker handle
    /// is configured, an error is logged.
    ///
    /// # Arguments
    ///
    /// * `message`: The message payload (must implement `ActonMessage`) to be broadcast.
    fn broadcast(&self, message: impl ActonMessage) -> impl Future<Output = ()> + Send + Sync + '_ {
        trace!("Attempting broadcast via handle: {}", self.id);
        async move {
            if let Some(broker_handle) = self.broker.as_ref() {
                trace!("Broker found for handle {}, sending BrokerRequest", self.id);
                // Send the BrokerRequest to the actual broker actor.
                broker_handle.send(BrokerRequest::new(message)).await;
            } else {
                // Log an error if no broker is configured for this actor handle.
                error!(
                    "No broker configured for actor handle {}, cannot broadcast.",
                    self.id
                );
            }
        }
    }
}

/// Implements the core interface for interacting with an actor.
#[async_trait]
impl ActorHandleInterface for ActorHandle {
    /// Returns the [`MessageAddress`] for this actor, used for sending replies.
    #[inline]
    fn reply_address(&self) -> MessageAddress {
        MessageAddress::new(self.outbox.clone(), self.id.clone())
    }

    /// Creates an [`OutboundEnvelope`] for sending a message from this actor.
    ///
    /// # Arguments
    ///
    /// * `recipient_address`: An optional [`MessageAddress`] specifying the recipient.
    ///   If `None`, the envelope is created without a specific recipient (e.g., for broadcasting
    ///   or when the recipient is set later).
    ///
    /// # Returns
    ///
    /// An [`OutboundEnvelope`] with the `return_address` set to this actor's address.
    #[instrument(skip(self))]
    fn create_envelope(&self, recipient_address: Option<MessageAddress>) -> OutboundEnvelope {
        let return_address = self.reply_address();
        trace!(sender = %return_address.sender.root(), recipient = ?recipient_address.as_ref().map(|r| r.sender.root().as_str()), "Creating envelope");
        if let Some(recipient) = recipient_address {
            OutboundEnvelope::new_with_recipient(
                return_address,
                recipient,
                self.cancellation_token.clone(),
            )
        } else {
            OutboundEnvelope::new(return_address, self.cancellation_token.clone())
        }
    }

    /// Returns a reference to the map of children supervised **through this
    /// handle**.
    ///
    /// This is a local view, not the supervisor's roster. `ActorHandle` holds
    /// its children in a `DashMap` that is deep-copied on clone, so each clone
    /// accumulates only what was supervised through it. A child adopted through
    /// a different clone of the same actor's handle will not appear here, and
    /// neither will one adopted from inside the actor's own message handler.
    ///
    /// The handles stored here name one incarnation. If a child is restarted,
    /// the handle kept here goes stale. Use
    /// [`SupervisedChild`](crate::actor::SupervisedChild) for a reference that
    /// follows restarts.
    ///
    /// Use `.len()`, `.iter()`, `.get()`, or `.contains_key()` to query without
    /// cloning the map.
    #[inline]
    fn children(&self) -> &DashMap<String, ActorHandle> {
        &self.children
    }

    /// Searches for a child supervised **through this handle** by its identifier.
    ///
    /// Subject to the same local-view and staleness caveats as
    /// [`children`](Self::children).
    ///
    /// # Arguments
    ///
    /// * `ern`: The [`Ern`] of the child actor to find.
    ///
    /// # Returns
    ///
    /// * `Some(ActorHandle)`: If a child with the matching `Ern` was supervised
    ///   through this handle.
    /// * `None`: Otherwise.
    #[instrument(skip(self))]
    fn find_child(&self, ern: &Ern) -> Option<Self> {
        trace!("Searching for child with ERN: {}", ern);
        // Access the DashMap using the ERN's string representation as the key.
        self.children.get(&ern.to_string()).map(
            |entry| entry.value().clone(), // Clone the handle if found
        )
    }

    /// Returns a clone of the actor's task tracker.
    ///
    /// The tracker can be used to monitor the actor's main task.
    #[inline]
    fn tracker(&self) -> TaskTracker {
        self.tracker.clone()
    }

    /// Returns a clone of the actor's unique Entity Resource Name (`Ern`).
    #[inline]
    fn id(&self) -> Ern {
        self.id.clone()
    }

    /// Returns the actor's root name (the first part of its `Ern`) as a String.
    #[inline]
    fn name(&self) -> String {
        self.id.root().to_string()
    }

    /// Returns a clone of this `ActorHandle`.
    #[inline]
    fn clone_ref(&self) -> ActorHandle {
        self.clone()
    }

    /// Sends a [`SystemSignal::Terminate`] message to the actor and waits for its task to complete.
    ///
    /// This initiates a graceful shutdown of the actor. It sends the `Terminate` signal
    /// to the actor's inbox and then waits on the actor's `TaskTracker` until the main
    /// task (and potentially associated tasks) have finished execution.
    ///
    /// The actor's `wake` loop is responsible for handling the `Terminate` signal,
    /// potentially running `before_stop` and `after_stop` hooks, and stopping child actors.
    ///
    /// # Returns
    ///
    /// An `anyhow::Result<()>` indicating success or failure. Failure typically occurs
    /// if sending the `Terminate` signal to the actor's inbox fails (e.g., if the channel
    /// is already closed).
    #[allow(clippy::manual_async_fn)] // Keep async_trait style
    #[instrument(skip(self))]
    fn stop(&self) -> impl Future<Output = anyhow::Result<()>> + Send + Sync + '_ {
        self.stop_with_signal(SystemSignal::Terminate)
    }

    /// Sends a boxed message to the actor.
    ///
    /// This method accepts a boxed trait object, converts it to an Arc, and sends it
    /// to the actor's inbox. This is primarily used for IPC scenarios where messages
    /// are deserialized into trait objects at runtime.
    ///
    /// # Arguments
    ///
    /// * `message`: A boxed message implementing `ActonMessage + Send + Sync`.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the message was successfully queued for delivery, or an error
    /// if the send failed (e.g., channel closed).
    #[cfg(feature = "ipc")]
    #[instrument(skip(self, message))]
    fn send_boxed(
        &self,
        message: Box<dyn ActonMessage + Send + Sync>,
    ) -> impl Future<Output = anyhow::Result<()>> + Send + Sync + '_ {
        use std::sync::Arc;
        async move {
            let envelope = self.create_envelope(Some(self.reply_address()));
            trace!(recipient = %self.id, "Sending boxed message via IPC");
            // Convert Box to Arc for the internal send mechanism
            let arc_message: Arc<dyn ActonMessage + Send + Sync> = Arc::from(message);
            envelope.send_arc(arc_message).await;
            Ok(())
        }
    }

    /// Sends a boxed message to the actor with a custom reply-to address.
    ///
    /// This method is used for IPC request-response patterns where responses
    /// should be routed back to a temporary IPC proxy channel rather than
    /// another actor.
    ///
    /// When the target actor calls `reply_envelope.send(response)`, the response
    /// will be delivered to the specified `reply_to` address.
    ///
    /// # Arguments
    ///
    /// * `message`: A boxed message implementing `ActonMessage + Send + Sync`.
    /// * `reply_to`: The [`MessageAddress`] where responses should be sent.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the message was successfully queued for delivery, or an error
    /// if the send failed (e.g., channel closed).
    #[cfg(feature = "ipc")]
    #[instrument(skip(self, message, reply_to))]
    fn send_boxed_with_reply_to(
        &self,
        message: Box<dyn ActonMessage + Send + Sync>,
        reply_to: MessageAddress,
    ) -> impl Future<Output = anyhow::Result<()>> + Send + Sync + '_ {
        use std::sync::Arc;
        async move {
            // Create envelope with the custom reply-to address as sender
            // and self (target actor) as recipient
            let envelope = OutboundEnvelope::new_with_recipient(
                reply_to,             // The reply-to address (IPC proxy)
                self.reply_address(), // The recipient (target actor)
                self.cancellation_token.clone(),
            );
            trace!(
                recipient = %self.id,
                reply_to = ?envelope.return_address.sender.root().as_str(),
                "Sending boxed message with custom reply-to via IPC"
            );
            // Convert Box to Arc for the internal send mechanism
            let arc_message: Arc<dyn ActonMessage + Send + Sync> = Arc::from(message);
            envelope.send_arc(arc_message).await;
            Ok(())
        }
    }

    /// Tries to send a boxed message without blocking (backpressure-aware).
    ///
    /// This method attempts to send a message but returns immediately with an error
    /// if the target actor's inbox is full.
    ///
    /// # Arguments
    ///
    /// * `message`: A boxed message implementing `ActonMessage + Send + Sync`.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the message was successfully queued, or an `IpcError` if
    /// the actor's inbox is full (`TargetBusy`) or the channel is closed.
    #[cfg(feature = "ipc")]
    #[instrument(skip(self, message))]
    fn try_send_boxed(
        &self,
        message: Box<dyn ActonMessage + Send + Sync>,
    ) -> Result<(), crate::common::ipc::IpcError> {
        use std::sync::Arc;
        let envelope = self.create_envelope(Some(self.reply_address()));
        trace!(recipient = %self.id, "Trying to send boxed message via IPC (backpressure-aware)");
        // Convert Box to Arc for the internal send mechanism
        let arc_message: Arc<dyn ActonMessage + Send + Sync> = Arc::from(message);
        envelope.try_send_arc(arc_message)
    }

    /// Tries to send a boxed message with a custom reply-to address without blocking.
    ///
    /// This method is the backpressure-aware variant of `send_boxed_with_reply_to`.
    /// It returns immediately with an error if the target actor's inbox is full.
    ///
    /// # Arguments
    ///
    /// * `message`: A boxed message implementing `ActonMessage + Send + Sync`.
    /// * `reply_to`: The [`MessageAddress`] where responses should be sent.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the message was successfully queued, or an `IpcError` if
    /// the actor's inbox is full (`TargetBusy`) or the channel is closed.
    #[cfg(feature = "ipc")]
    #[instrument(skip(self, message, reply_to))]
    fn try_send_boxed_with_reply_to(
        &self,
        message: Box<dyn ActonMessage + Send + Sync>,
        reply_to: MessageAddress,
    ) -> Result<(), crate::common::ipc::IpcError> {
        use std::sync::Arc;
        // Create envelope with the custom reply-to address as sender
        // and self (target actor) as recipient
        let envelope = OutboundEnvelope::new_with_recipient(
            reply_to,             // The reply-to address (IPC proxy)
            self.reply_address(), // The recipient (target actor)
            self.cancellation_token.clone(),
        );
        trace!(
            recipient = %self.id,
            reply_to = ?envelope.return_address.sender.root().as_str(),
            "Trying to send boxed message with custom reply-to via IPC (backpressure-aware)"
        );
        // Convert Box to Arc for the internal send mechanism
        let arc_message: Arc<dyn ActonMessage + Send + Sync> = Arc::from(message);
        envelope.try_send_arc(arc_message)
    }
}
