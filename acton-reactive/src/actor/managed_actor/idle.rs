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
use std::collections::HashMap;
use std::fmt::Debug;
use std::future::Future;
use std::marker::PhantomData;
use std::mem;
use std::sync::atomic::AtomicBool;

use acton_ern::Ern;
use tokio::sync::mpsc::channel;
use tokio_util::task::TaskTracker;
use tracing::{error, instrument, trace};

use crate::actor::supervision::SupervisionRegistry;
use crate::actor::{ActorConfig, Escalation, ManagedActor, RestartPolicy, Started, SupervisionStrategy};
use crate::common::{
    ActorHandle, ActorRuntime, Envelope, FutureBox, OutboundEnvelope, ReactorItem,
};
use crate::message::MessageContext;
use crate::prelude::ActonMessage;
use crate::traits::{ActonMessageReply, ActorHandleInterface};

/// The IPC name an actor is exposed under, derived from its [`Ern`].
///
/// A root actor yields its plain name; a child yields its ancestry joined with `/`:
///
/// - `prices_01k…`            -> `prices`
/// - `prices_01k…/alpha`      -> `prices/alpha`
/// - `prices_01k…/alpha/deep` -> `prices/alpha/deep`
///
/// # Why this shape
///
/// Two properties have to hold at once, and only this split delivers both.
///
/// *Stable across runs*: the discarded piece is exactly the generated `UUIDv7`
/// suffix, which is regenerated on every process start. A name containing it could
/// not be written into a client or a config file.
///
/// *Distinct between actors that could collide*: the retained parts are exactly what
/// distinguishes a child from its parent and from its siblings. A supervised child
/// shares its parent's root — its own segment lives in the parts — so dropping the
/// parts would make every child collide with its parent.
///
/// Pure: derived entirely from `ern`, consulting no registry.
#[cfg(feature = "ipc")]
fn ipc_name_for(ern: &Ern) -> String {
    let prefix = ern.root().name().prefix();

    if ern.parts().is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}/{}", ern.parts())
    }
}

/// Type-state marker for a [`ManagedActor`] that has been configured but not yet started.
///
/// When a `ManagedActor` is in the `Idle` state, it can be configured with message handlers
/// (via [`ManagedActor::mutate_on`]) and lifecycle hooks (e.g., [`ManagedActor::before_start`],
/// [`ManagedActor::after_stop`]). Once configuration is complete, the actor can be
/// transitioned to the [`Started`](super::started::Started) state by calling [`ManagedActor::start`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)] // Add common derives
pub struct Idle;

impl<State: Default + Send + Debug + 'static> ManagedActor<Idle, State> {
    /// Registers an asynchronous message handler for a specific message type `M`.
    ///
    /// This method is called during the actor's configuration phase (while in the `Idle` state).
    /// It associates a specific message type `M` with a closure (`message_processor`) that
    /// will be executed when the actor receives a message of that type after it has started.
    ///
    /// The framework handles the necessary type erasure and downcasting internally. The
    /// provided `message_processor` receives the actor (in the `Started` state) and a
    /// [`MessageContext`] containing the concrete message and metadata.
    ///
    /// # Type Parameters
    ///
    /// *   `M`: The concrete message type this handler will process. Must implement
    ///     [`ActonMessage`], `Clone`, `Send`, `Sync`, and be `'static`.
    ///
    /// # Arguments
    ///
    /// *   `message_processor`: An asynchronous closure that takes the actor (`&mut ManagedActor<Started, State>`)
    ///     and the message context (`&mut MessageContext<M>`) and returns a `Future`
    ///     (specifically, a [`FutureBox`]). This closure contains the logic for handling messages of type `M`.
    ///
    /// # Returns
    ///
    /// Returns a mutable reference to `self` to allow for method chaining during configuration.
    ///
    /// # Cancellation Safety
    ///
    /// Message handlers should be written with cancellation safety in mind. If the actor
    /// is shut down (via `CancellationToken`) while a handler is executing, the handler's
    /// future will be dropped at the next `.await` point. Any side effects that occurred
    /// *before* that await point will persist, while operations *after* it will not execute.
    ///
    /// **Guidelines for cancellation-safe handlers:**
    ///
    /// - Perform validation and checks before making any state changes
    /// - Use atomic operations or transactions when modifying external state
    /// - Consider using [`Drop`] guards for cleanup of partial state changes
    /// - If a handler must not be interrupted, complete critical sections before awaiting
    ///
    /// **Example of a cancellation-safe pattern:**
    ///
    /// ```ignore
    /// actor.mutate_on::<MyMessage>(|actor, ctx| {
    ///     Box::pin(async move {
    ///         // Validate first (safe to cancel here)
    ///         if !is_valid(&ctx.message) {
    ///             return;
    ///         }
    ///
    ///         // Perform atomic state change (completes before any await)
    ///         actor.model.counter += 1;
    ///
    ///         // External I/O after state change (cancellation here is safe)
    ///         notify_external_system().await;
    ///     })
    /// });
    /// ```
    #[instrument(skip(self, message_processor), level = "debug")]
    pub fn mutate_on<M>(
        &mut self,
        message_processor: impl for<'a> Fn(&'a mut ManagedActor<Started, State>, &'a mut MessageContext<M>) -> FutureBox
            + Send
            + Sync
            + 'static,
    ) -> &mut Self
    where
        M: ActonMessage + Clone + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<M>();
        trace!(type_name=std::any::type_name::<M>(),type_id=?type_id, " Adding mutable message handler");
        let handler_box = Box::new(
            move |actor: &mut ManagedActor<Started, State>, envelope: &mut Envelope| -> FutureBox {
                if let Some(concrete_msg) = downcast_message::<M>(&*envelope.message) {
                    trace!(
                        "Downcast successful for message type: {}",
                        std::any::type_name::<M>()
                    );
                    let mut msg_context = {
                        let origin_envelope = OutboundEnvelope::new_with_recipient(
                            envelope.reply_to.clone(),
                            envelope.recipient.clone(),
                            actor.handle.cancellation_token.clone(),
                        );
                        let reply_envelope = OutboundEnvelope::new_with_recipient(
                            envelope.recipient.clone(),
                            envelope.reply_to.clone(),
                            actor.handle.cancellation_token.clone(),
                        );
                        MessageContext {
                            message: concrete_msg.clone(),

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
            .insert(type_id, ReactorItem::Mutable(handler_box));
        self
    }

    /// Registers a synchronous message handler for a specific message type `M`.
    ///
    /// Like [`mutate_on`](Self::mutate_on), but the handler closure returns `()` directly
    /// instead of a `Future`. This avoids the heap allocation of `Box::pin(async move {})` for
    /// handlers that don't need to perform any async work, providing measurably lower overhead
    /// on the dispatch hot path.
    ///
    /// # Type Parameters
    ///
    /// *   `M`: The concrete message type this handler will process. Must implement
    ///     [`ActonMessage`], `Clone`, `Send`, `Sync`, and be `'static`.
    ///
    /// # Arguments
    ///
    /// *   `message_processor`: A closure that takes the actor (`&mut ManagedActor<Started, State>`)
    ///     and the message context (`&mut MessageContext<M>`) and returns `()`.
    ///
    /// # Returns
    ///
    /// Returns a mutable reference to `self` to allow for method chaining during configuration.
    #[instrument(skip(self, message_processor), level = "debug")]
    pub fn mutate_on_sync<M>(
        &mut self,
        message_processor: impl for<'a> Fn(&'a mut ManagedActor<Started, State>, &'a mut MessageContext<M>)
            + Send
            + Sync
            + 'static,
    ) -> &mut Self
    where
        M: ActonMessage + Clone + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<M>();
        trace!(type_name=std::any::type_name::<M>(),type_id=?type_id, " Adding sync mutable message handler");
        let handler_box = Box::new(
            move |actor: &mut ManagedActor<Started, State>, envelope: &mut Envelope| {
                if let Some(concrete_msg) = downcast_message::<M>(&*envelope.message) {
                    trace!(
                        "Downcast successful for message type: {}",
                        std::any::type_name::<M>()
                    );
                    let mut msg_context = {
                        let origin_envelope = OutboundEnvelope::new_with_recipient(
                            envelope.reply_to.clone(),
                            envelope.recipient.clone(),
                            actor.handle.cancellation_token.clone(),
                        );
                        let reply_envelope = OutboundEnvelope::new_with_recipient(
                            envelope.recipient.clone(),
                            envelope.reply_to.clone(),
                            actor.handle.cancellation_token.clone(),
                        );
                        MessageContext {
                            message: concrete_msg.clone(),
                            origin_envelope,
                            reply_envelope,
                        }
                    };
                    message_processor(actor, &mut msg_context);
                } else {
                    error!(
                        type_name = std::any::type_name::<M>(),
                        "Sync message handler called with incompatible message type (downcast failed)"
                    );
                }
            },
        );
        self.message_handlers
            .insert(type_id, ReactorItem::MutableSync(handler_box));
        self
    }

    /// Registers an asynchronous error handler for a specific error type `E`.
    ///
    /// This allows the actor to handle errors of type `E` by executing the given closure
    /// whenever a message handler returns an error of this type. Both fallible handler
    /// kinds are covered: errors from [`try_mutate_on`](Self::try_mutate_on) handlers are
    /// dispatched immediately after the failing handler returns, while errors from
    /// [`try_act_on`](Self::try_act_on) handlers are dispatched at the actor's next flush
    /// point, once the concurrently running read-only handlers have completed.
    ///
    /// # Type Parameters
    ///
    /// * `E`: The concrete error type to handle. Must implement `std::error::Error` and be `'static`.
    ///
    /// # Arguments
    /// * `error_handler`: The handler closure executed with actor, envelope, and error reference.
    ///
    /// # Returns
    /// A mutable reference to `self` for chaining.
    pub fn on_error<M, E>(
        &mut self,
        error_handler: impl for<'a, 'b> Fn(
                &'a mut ManagedActor<Started, State>,
                &'b mut MessageContext<M>,
                &'b E,
            ) -> FutureBox
            + Send
            + Sync
            + 'static,
    ) -> &mut Self
    where
        M: ActonMessage + Clone + Send + Sync + 'static,
        E: std::error::Error + 'static,
    {
        use std::any::TypeId;
        let message_type_id = TypeId::of::<M>();
        let error_type_id = TypeId::of::<E>();

        // Wrap handler for dynamic dispatch
        let handler_box: Box<crate::common::ErrorHandler<State>> =
            Box::new(move |actor, envelope, err| {
                if let Some(concrete_msg) = downcast_message::<M>(&*envelope.message) {
                    // Downcast the error to &E
                    if let Some(specific_err) = err.downcast_ref::<E>() {
                        let mut msg_context = {
                            let origin_envelope = OutboundEnvelope::new_with_recipient(
                                envelope.reply_to.clone(),
                                envelope.recipient.clone(),
                                actor.handle.cancellation_token.clone(),
                            );
                            let reply_envelope = OutboundEnvelope::new_with_recipient(
                                envelope.recipient.clone(),
                                envelope.reply_to.clone(),
                                actor.handle.cancellation_token.clone(),
                            );
                            MessageContext {
                                message: concrete_msg.clone(),
    
                                origin_envelope,
                                reply_envelope,
                            }
                        };
                        error_handler(actor, &mut msg_context, specific_err)
                    } else {
                        // If type doesn't match, do nothing
                        Box::pin(async {})
                    }
                } else {
                    // If type doesn't match, do nothing
                    Box::pin(async {})
                }
            });
        self.error_handler_map
            .insert((message_type_id, error_type_id), handler_box);
        self
    }

    /// Registers an asynchronous read-only message handler for a specific message type `M`.
    ///
    /// This method is called during the actor's configuration phase (while in the `Idle` state).
    /// It associates a specific message type `M` with a closure (`message_processor`) that
    /// will be executed when the actor receives a message of that type after it has started.
    ///
    /// Unlike `mutate_on`, handlers registered with `act_on` operate on an immutable reference
    /// to the actor (`&ManagedActor`) and can be executed concurrently with other read-only handlers.
    /// Message ordering is not guaranteed for read-only handlers.
    ///
    /// # Type Parameters
    ///
    /// *   `M`: The concrete message type this handler will process. Must implement
    ///     [`ActonMessage`], `Clone`, `Send`, `Sync`, and be `'static`.
    ///
    /// # Arguments
    ///
    /// *   `message_processor`: An asynchronous closure that takes the actor (`&ManagedActor<Started, State>`)
    ///     and the message context (`&mut MessageContext<M>`) and returns a `Future`
    ///     (specifically, a [`FutureBox`]). This closure contains the logic for handling messages of type `M`.
    ///
    /// # Returns
    ///
    /// Returns a mutable reference to `self` to allow for method chaining during configuration.
    ///
    /// # Cancellation Safety
    ///
    /// Read-only handlers are spawned as separate tasks and may be cancelled independently
    /// when the actor shuts down. Since these handlers cannot modify actor state directly,
    /// cancellation is generally safer than with `mutate_on` handlers.
    ///
    /// However, if your handler interacts with external systems (databases, APIs, etc.),
    /// the same cancellation safety guidelines apply:
    ///
    /// - Side effects before an `.await` point will persist even if the handler is cancelled
    /// - Use idempotent operations for external system interactions when possible
    /// - Consider using timeouts to prevent handlers from blocking shutdown indefinitely
    ///
    /// **Note:** Read-only handlers run concurrently with each other but are flushed
    /// (awaited to completion) before any mutable handler executes, ensuring consistency.
    #[instrument(skip(self, message_processor), level = "debug")]
    pub fn act_on<M>(
        &mut self,
        message_processor: impl for<'a> Fn(&'a ManagedActor<Started, State>, &'a mut MessageContext<M>) -> FutureBox
            + Send
            + Sync
            + 'static,
    ) -> &mut Self
    where
        M: ActonMessage + Clone + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<M>();
        trace!(type_name=std::any::type_name::<M>(),type_id=?type_id, " Adding read-only message handler");
        let handler_box = Box::new(
            move |actor: &ManagedActor<Started, State>, envelope: &mut Envelope| -> FutureBox {
                if let Some(concrete_msg) = downcast_message::<M>(&*envelope.message) {
                    trace!(
                        "Downcast successful for message type: {}",
                        std::any::type_name::<M>()
                    );
                    let mut msg_context = {
                        let origin_envelope = OutboundEnvelope::new_with_recipient(
                            envelope.reply_to.clone(),
                            envelope.recipient.clone(),
                            actor.handle.cancellation_token.clone(),
                        );
                        let reply_envelope = OutboundEnvelope::new_with_recipient(
                            envelope.recipient.clone(),
                            envelope.reply_to.clone(),
                            actor.handle.cancellation_token.clone(),
                        );
                        MessageContext {
                            message: concrete_msg.clone(),

                            origin_envelope,
                            reply_envelope,
                        }
                    };
                    message_processor(actor, &mut msg_context)
                } else {
                    error!(
                        type_name = std::any::type_name::<M>(),
                        "Read-only message handler called with incompatible message type (downcast failed)"
                    );
                    Box::pin(async {})
                }
            },
        );
        self.read_only_handlers
            .insert(type_id, ReactorItem::ReadOnly(handler_box));
        self
    }
    /// Registers a synchronous read-only message handler for a specific message type `M`.
    ///
    /// Like [`act_on`](Self::act_on), but the handler closure returns `()` directly instead of
    /// a `Future`. This avoids the heap allocation overhead for handlers that don't need async.
    /// Sync read-only handlers complete immediately inline rather than being pushed to
    /// `FuturesUnordered`.
    ///
    /// # Type Parameters
    ///
    /// *   `M`: The concrete message type this handler will process. Must implement
    ///     [`ActonMessage`], `Clone`, `Send`, `Sync`, and be `'static`.
    ///
    /// # Arguments
    ///
    /// *   `message_processor`: A closure that takes the actor (`&ManagedActor<Started, State>`)
    ///     and the message context (`&mut MessageContext<M>`) and returns `()`.
    ///
    /// # Returns
    ///
    /// Returns a mutable reference to `self` to allow for method chaining during configuration.
    #[instrument(skip(self, message_processor), level = "debug")]
    pub fn act_on_sync<M>(
        &mut self,
        message_processor: impl for<'a> Fn(&'a ManagedActor<Started, State>, &'a mut MessageContext<M>)
            + Send
            + Sync
            + 'static,
    ) -> &mut Self
    where
        M: ActonMessage + Clone + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<M>();
        trace!(type_name=std::any::type_name::<M>(),type_id=?type_id, " Adding sync read-only message handler");
        let handler_box = Box::new(
            move |actor: &ManagedActor<Started, State>, envelope: &mut Envelope| {
                if let Some(concrete_msg) = downcast_message::<M>(&*envelope.message) {
                    trace!(
                        "Downcast successful for message type: {}",
                        std::any::type_name::<M>()
                    );
                    let mut msg_context = {
                        let origin_envelope = OutboundEnvelope::new_with_recipient(
                            envelope.reply_to.clone(),
                            envelope.recipient.clone(),
                            actor.handle.cancellation_token.clone(),
                        );
                        let reply_envelope = OutboundEnvelope::new_with_recipient(
                            envelope.recipient.clone(),
                            envelope.reply_to.clone(),
                            actor.handle.cancellation_token.clone(),
                        );
                        MessageContext {
                            message: concrete_msg.clone(),
                            origin_envelope,
                            reply_envelope,
                        }
                    };
                    message_processor(actor, &mut msg_context);
                } else {
                    error!(
                        type_name = std::any::type_name::<M>(),
                        "Sync read-only message handler called with incompatible message type (downcast failed)"
                    );
                }
            },
        );
        self.read_only_handlers
            .insert(type_id, ReactorItem::ReadOnlySync(handler_box));
        self
    }

    /// Registers an asynchronous read-only message handler for a specific message type `M` that returns a Result.
    ///
    /// This method is called during the actor's configuration phase (while in the `Idle` state).
    /// It associates a specific message type `M` with a closure (`message_processor`) that
    /// will be executed when the actor receives a message of that type after it has started.
    ///
    /// Unlike `try_mutate_on`, handlers registered with `try_act_on` operate on an immutable reference
    /// to the actor (`&ManagedActor`) and can be executed concurrently with other read-only handlers.
    /// Message ordering is not guaranteed for read-only handlers.
    ///
    /// Errors returned by the handler are routed to an error handler registered via
    /// [`on_error`](Self::on_error) for the same message and error types. Because
    /// read-only handlers execute concurrently, the error is dispatched at the actor's
    /// next flush point (where exclusive access to the actor is available) rather than
    /// immediately after the handler returns. If no matching error handler is
    /// registered, the error is logged.
    ///
    /// # Type Parameters
    ///
    /// *   `M`: The concrete message type this handler will process. Must implement
    ///     [`ActonMessage`], `Clone`, `Send`, `Sync`, and be `'static`.
    /// *   `T`: The success type returned by the handler. Must implement [`ActonMessageReply`] and be `'static`.
    /// *   `E`: The error type returned by the handler. Must implement [`std::error::Error`] and be `'static`.
    ///
    /// # Arguments
    ///
    /// *   `message_processor`: An asynchronous closure that takes the actor (`&ManagedActor<Started, State>`)
    ///     and the message context (`&mut MessageContext<M>`) and returns a `Future`
    ///     (specifically, a [`FutureBoxResult`]). This closure contains the logic for handling messages of type `M`.
    ///
    /// # Returns
    ///
    /// Returns a mutable reference to `self` to allow for method chaining during configuration.
    #[instrument(skip(self, message_processor), level = "debug")]
    pub fn try_act_on<M, T, E>(
        &mut self,
        message_processor: impl for<'a> Fn(
                &'a ManagedActor<Started, State>,
                &'a mut MessageContext<M>,
            ) -> std::pin::Pin<
                Box<dyn Future<Output = Result<T, E>> + Send + Sync + 'static>,
            > + Send
            + Sync
            + 'static,
    ) -> &mut Self
    where
        M: ActonMessage + Clone + Send + Sync + 'static,
        T: ActonMessageReply + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<M>();
        trace!(type_name=std::any::type_name::<M>(),type_id=?type_id, " Adding read-only Result-returning message handler");
        let handler_box = Box::new(
            move |actor: &ManagedActor<Started, State>,
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
                            actor.handle.cancellation_token.clone(),
                        );
                        let reply_envelope = OutboundEnvelope::new_with_recipient(
                            envelope.recipient.clone(),
                            envelope.reply_to.clone(),
                            actor.handle.cancellation_token.clone(),
                        );
                        MessageContext {
                            message: concrete_msg.clone(),

                            origin_envelope,
                            reply_envelope,
                        }
                    };
                    let fut = message_processor(actor, &mut msg_context);
                    Box::pin(async move {
                        match fut.await {
                            Ok(val) => {
                                let boxed: Box<dyn ActonMessageReply + Send> = Box::new(val);
                                Ok(boxed)
                            }
                            Err(e) => {
                                let error_type_id = TypeId::of::<E>();
                                let boxed_err: Box<dyn std::error::Error + Send + Sync> =
                                    Box::new(e);
                                Err((boxed_err, error_type_id))
                            }
                        }
                    })
                } else {
                    error!(
                        type_name = std::any::type_name::<M>(),
                        "Read-only Result handler called with incompatible message type (downcast failed)"
                    );
                    Box::pin(async {
                        let boxed: Box<dyn ActonMessageReply + Send> = Box::new(());
                        Ok(boxed)
                    })
                }
            },
        );
        self.read_only_handlers
            .insert(type_id, ReactorItem::ReadOnlyFallible(handler_box));
        self
    }

    /// Registers an asynchronous message handler for a specific message type `M` that returns a Result (new style, preferred).
    pub fn try_mutate_on<M, T, E>(
        &mut self,
        message_processor: impl for<'a> Fn(
                &'a mut ManagedActor<Started, State>,
                &'a mut MessageContext<M>,
            ) -> std::pin::Pin<
                Box<dyn Future<Output = Result<T, E>> + Send + Sync + 'static>,
            > + Send
            + Sync
            + 'static,
    ) -> &mut Self
    where
        M: ActonMessage + Clone + Send + Sync + 'static,
        T: ActonMessageReply + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<M>();
        trace!(type_name=std::any::type_name::<M>(),type_id=?type_id, " Adding Result-returning message handler");
        let handler_box = Box::new(
            move |actor: &mut ManagedActor<Started, State>,
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
                            actor.handle.cancellation_token.clone(),
                        );
                        let reply_envelope = OutboundEnvelope::new_with_recipient(
                            envelope.recipient.clone(),
                            envelope.reply_to.clone(),
                            actor.handle.cancellation_token.clone(),
                        );
                        MessageContext {
                            message: concrete_msg.clone(),

                            origin_envelope,
                            reply_envelope,
                        }
                    };
                    let fut = message_processor(actor, &mut msg_context);
                    Box::pin(async move {
                        match fut.await {
                            Ok(val) => {
                                let boxed: Box<dyn ActonMessageReply + Send> = Box::new(val);
                                Ok(boxed)
                            }
                            Err(e) => {
                                let error_type_id = TypeId::of::<E>();
                                let boxed_err: Box<dyn std::error::Error + Send + Sync> =
                                    Box::new(e);
                                Err((boxed_err, error_type_id))
                            }
                        }
                    })
                } else {
                    error!(
                        type_name = std::any::type_name::<M>(),
                        "Result handler called with incompatible message type (downcast failed)"
                    );
                    Box::pin(async {
                        let boxed: Box<dyn ActonMessageReply + Send> = Box::new(());
                        Ok(boxed)
                    })
                }
            },
        );
        self.message_handlers
            .insert(type_id, ReactorItem::MutableFallible(handler_box));
        self
    }

    /// Registers an asynchronous hook to be executed *after* the actor successfully starts its message loop.
    ///
    /// This hook is called once, shortly after the actor transitions to the `Started` state
    /// and its main task begins processing messages. It receives an immutable reference
    /// to the actor in the `Started` state.
    ///
    /// # Arguments
    ///
    /// * `f`: An asynchronous closure that takes `&ManagedActor<Started, State>` and returns a `Future`.
    ///
    /// # Returns
    ///
    /// Returns a mutable reference to `self` for chaining.
    pub fn after_start<F, Fut>(&mut self, f: F) -> &mut Self
    where
        F: for<'b> Fn(&'b ManagedActor<Started, State>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + Sync + 'static,
    {
        self.after_start = Some(Box::new(move |actor| Box::pin(f(actor))));
        self
    }

    /// Registers an asynchronous hook to be executed *before* the actor starts its message loop.
    ///
    /// This hook is called once, just before the actor's main task (`wake`) is spawned
    /// during the `start` process. It receives an immutable reference to the actor,
    /// technically still in the `Started` state contextually, though the loop hasn't begun.
    ///
    /// # Arguments
    ///
    /// * `f`: An asynchronous closure that takes `&ManagedActor<Started, State>` and returns a `Future`.
    ///
    /// # Returns
    ///
    /// Returns a mutable reference to `self` for chaining.
    pub fn before_start<F, Fut>(&mut self, f: F) -> &mut Self
    where
        F: for<'b> Fn(&'b ManagedActor<Started, State>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + Sync + 'static,
    {
        self.before_start = Some(Box::new(move |actor| Box::pin(f(actor))));
        self
    }

    /// Registers an asynchronous hook to be executed *after* the actor stops processing messages.
    ///
    /// This hook is called once when the actor's main loop terminates gracefully (e.g., upon
    /// receiving a `Terminate` signal or when the inbox closes). It receives an immutable
    /// reference to the actor in the `Started` state context.
    ///
    /// # Arguments
    ///
    /// * `f`: An asynchronous closure that takes `&ManagedActor<Started, State>` and returns a `Future`.
    ///
    /// # Returns
    ///
    /// Returns a mutable reference to `self` for chaining.
    pub fn after_stop<F, Fut>(&mut self, f: F) -> &mut Self
    where
        F: for<'b> Fn(&'b ManagedActor<Started, State>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + Sync + 'static,
    {
        self.after_stop = Some(Box::new(move |actor| Box::pin(f(actor))));
        self
    }

    /// Registers an asynchronous hook to be executed *before* the actor stops processing messages.
    ///
    /// This hook is called once, just before the actor's main loop begins its shutdown sequence
    /// (e.g., after receiving `Terminate` but before fully stopping). It receives an immutable
    /// reference to the actor in the `Started` state.
    ///
    /// # Arguments
    ///
    /// * `f`: An asynchronous closure that takes `&ManagedActor<Started, State>` and returns a `Future`.
    ///
    /// # Returns
    ///
    /// Returns a mutable reference to `self` for chaining.
    pub fn before_stop<F, Fut>(&mut self, f: F) -> &mut Self
    where
        F: for<'b> Fn(&'b ManagedActor<Started, State>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + Sync + 'static,
    {
        self.before_stop = Some(Box::new(move |actor| Box::pin(f(actor))));
        self
    }

    /// Marks this actor to be automatically exposed for IPC access when started.
    ///
    /// The actor is registered during [`start()`](Self::start) under the name it was
    /// given, so external processes can reach it over Unix domain sockets.
    ///
    /// # The name
    ///
    /// A root actor is exposed under its plain name. A supervised child is exposed
    /// under its parent's name, then its own, separated by `/`:
    ///
    /// | Actor | Exposed as |
    /// |---|---|
    /// | `new_actor_with_name("prices")` | `prices` |
    /// | child `"alpha"` of `prices` | `prices/alpha` |
    /// | child `"inner"` of that child | `prices/alpha/inner` |
    ///
    /// The generated identifier suffix carried by an actor's [`Ern`] is deliberately
    /// **not** part of the name. That suffix is a fresh `UUIDv7` on every process
    /// start, so a name built from it could never be written into a client or a
    /// config file — which is what made this method unusable before.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut service = runtime.new_actor_with_name::<MyService>("prices".to_string());
    /// service
    ///     .act_on::<GetPrice>(|actor, ctx| { /* ... */ })
    ///     .expose_for_ipc()  // Reachable as "prices" via IPC
    ///     .start().await;
    /// ```
    ///
    /// # This method cannot fail, and does not report a name conflict in-band
    ///
    /// If another actor already holds the name, the existing registration is kept
    /// and **this actor simply is not reachable over IPC**. The conflict is reported
    /// by logging at `error!`, naming both actors and the contested name; `start()`
    /// still returns a working [`ActorHandle`], because a fault confined to IPC
    /// should not make starting an actor fallible for every program.
    ///
    /// **If you need to handle a name conflict in code, call
    /// [`ActorRuntime::ipc_expose`](crate::prelude::ActorRuntime::ipc_expose) and
    /// match on its result.** That is the form with an error channel; this one is
    /// the convenience.
    ///
    /// # Returns
    ///
    /// Returns a mutable reference to `self` for chaining.
    ///
    /// # See Also
    ///
    /// - [`ActorRuntime::ipc_expose`](crate::prelude::ActorRuntime::ipc_expose) for manual IPC exposure with custom names, and for handling conflicts
    /// - [`ActorRuntime::ipc_hide`](crate::prelude::ActorRuntime::ipc_hide) for removing IPC exposure
    #[cfg(feature = "ipc")]
    pub const fn expose_for_ipc(&mut self) -> &mut Self {
        self.expose_for_ipc = true;
        self
    }

    /// Creates the configuration for a new child actor under this actor's supervision.
    ///
    /// This method generates a `ManagedActor<Idle, State>` instance pre-configured
    /// to be a child of the current actor. The child's [`Ern`] is its parent's with
    /// `name` appended as a part, so it reads as `<parent-ern>/<name>`.
    /// The child inherits the parent's broker reference.
    ///
    /// The returned actor is in the `Idle` state and still needs to be configured
    /// (e.g., with `mutate_on`, lifecycle hooks) and then started using its `start` method.
    /// The parent actor typically calls `handle.supervise(child_handle)` after the child
    /// is started to register it formally.
    ///
    /// # The name is part of the identifier
    ///
    /// Two children of one parent given different names get different
    /// identifiers *because of their names*, and the same parent and name always
    /// yield the same identifier. Before 9.0.0 neither held: the name was
    /// discarded and each call drew a fresh random suffix, so siblings differed
    /// only by accident.
    ///
    /// # Arguments
    ///
    /// * `name`: The name segment appended to the parent's [`Ern`].
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing a new `ManagedActor` instance for the child
    /// in the `Idle` state, ready for further configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if `name` is not a valid identifier segment, or if this
    /// actor already sits at
    /// [`MAX_SUPERVISION_DEPTH`](crate::actor::MAX_SUPERVISION_DEPTH).
    #[instrument(skip(self))]
    pub fn create_child(&self, name: String) -> anyhow::Result<Self> {
        // Derived through the supervised-child constructor so the child's
        // identifier keeps its name and is reproducible for a given parent.
        let config = ActorConfig::for_supervised_child(
            name,
            self.handle.clone(),         // Parent handle
            Some(self.runtime.broker()), // Inherited broker handle
        )?;
        // Create the Idle actor using the internal constructor.
        Ok(Self::new(Some(self.runtime()), Some(&config)))
    }

    // Internal constructor - not part of public API documentation
    #[instrument]
    pub(crate) fn new(runtime: Option<&ActorRuntime>, config: Option<&ActorConfig>) -> Self {
        let mut managed_actor: Self = Self::default();

        if let Some(app) = runtime {
            managed_actor.broker = app.0.broker.clone();
            managed_actor.handle.broker = Box::new(Some(app.0.broker.clone()));
            managed_actor.cancellation_token = Some(app.0.cancellation_token.child_token());
        }

        if let Some(config) = &config {
            managed_actor.handle.id = config.id();
            managed_actor.parent = config.parent().cloned();
            managed_actor.handle.broker = Box::new(config.get_broker().cloned());
            if let Some(broker) = config.get_broker().cloned() {
                managed_actor.broker = broker;
            }
            // Apply custom inbox capacity if specified
            if let Some(capacity) = config.inbox_capacity() {
                let (outbox, inbox) = channel(capacity);
                managed_actor.handle.outbox = outbox;
                managed_actor.inbox = inbox;
            }
            // Apply restart policy
            managed_actor.restart_policy = config.restart_policy();
            // Apply supervision strategy
            managed_actor.supervision_strategy = config.supervision_strategy();
            // Apply escalation policy
            managed_actor.escalation = config.escalation();
            managed_actor.restart_limiter_config = config.restart_limiter_config().cloned();
        }

        debug_assert!(
            !managed_actor.inbox.is_closed(),
            "Actor mailbox is closed in new"
        );

        trace!("NEW ACTOR: {}", &managed_actor.handle.id());

        // Ensure runtime always exists; creating a new one here is an error.
        assert!(
            runtime.is_some(),
            "ActorRuntime must be provided to ManagedActor::new"
        );
        let runtime = runtime.unwrap().clone();
        managed_actor.runtime = runtime;
        // Note: root registration is handled by ActorRuntime methods (new_actor_with_name, etc.)
        // to avoid double insertions into the roots DashMap

        managed_actor.id = managed_actor.handle.id();

        managed_actor
    }

    /// Starts the actor's processing loop and transitions it to the `Started` state.
    ///
    /// This method consumes the `ManagedActor` in the `Idle` state. It performs the following actions:
    /// 1.  Transitions the actor's type state from `Idle` to [`Started`](super::started::Started).
    /// 2.  Executes the registered `before_start` lifecycle hook.
    /// 3.  Spawns the actor's main asynchronous task (`wake`) which handles message processing.
    /// 4.  Closes the actor's `TaskTracker` to signal that the main task has been spawned.
    /// 5.  If [`expose_for_ipc()`](Self::expose_for_ipc) was called, registers the actor for IPC access.
    /// 6.  Returns the actor's [`ActorHandle`] for external interaction.
    ///
    /// After this method returns, the actor is running and ready to process messages sent to its handle.
    ///
    /// # Returns
    ///
    /// An [`ActorHandle`] that can be used to interact with the now-running actor.
    #[instrument(skip(self))]
    pub async fn start(mut self) -> ActorHandle {
        trace!("Starting actor: {}", self.id());
        trace!("Model state before start: {:?}", self.model);

        // Take ownership of handlers before converting state.
        let message_handlers = mem::take(&mut self.message_handlers);
        let read_only_handlers = mem::take(&mut self.read_only_handlers);
        let actor_ref = self.handle.clone(); // Clone handle before consuming self.

        // Capture IPC exposure settings before consuming self
        #[cfg(feature = "ipc")]
        let should_expose_for_ipc = self.expose_for_ipc;
        #[cfg(feature = "ipc")]
        let ipc_name = ipc_name_for(&self.id);
        #[cfg(feature = "ipc")]
        let runtime_for_ipc = self.runtime.clone();

        // Convert the actor to the Started state.
        let mut active_actor: ManagedActor<Started, State> = self.into();

        // Execute before_start hook if registered.
        if let Some(ref hook) = active_actor.before_start {
            trace!("Executing before_start hook for actor: {}", active_actor.id());
            hook(&active_actor).await;
        }

        trace!("Spawning main task (wake) for actor: {}", active_actor.id());
        // Move ownership of the actor into the spawned task.
        // This ensures proper memory cleanup when the task completes,
        // avoiding the previous Box::leak pattern.
        actor_ref.tracker().spawn(async move {
            active_actor
                .wake(message_handlers, read_only_handlers)
                .await;
            // active_actor is dropped here when the task completes
        });
        // Close the tracker to indicate the main task is launched.
        actor_ref.tracker().close();

        // Register for IPC access if requested
        #[cfg(feature = "ipc")]
        if should_expose_for_ipc {
            trace!("Exposing actor '{}' for IPC access", ipc_name);
            if let Err(conflict) = runtime_for_ipc.ipc_expose(&ipc_name, actor_ref.clone()) {
                // The actor still starts; it is simply not reachable under this
                // name. Logged at error because nothing else tells the operator:
                // the caller has no error channel here, and a silent overwrite is
                // the defect this refusal exists to prevent.
                error!(
                    ipc_name = %ipc_name,
                    wanted_by = %actor_ref.id(),
                    held_by = %conflict.held_by(),
                    "IPC name already claimed; this actor will not be reachable under it. \
                     Give one of the two actors a different name, or use ActorRuntime::ipc_expose \
                     to choose the name explicitly."
                );
            }
        }

        trace!("Actor {} started successfully.", actor_ref.id());
        actor_ref // Return the handle.
    }
}

// --- Utility Function ---

/// Attempts to downcast an `ActonMessage` trait object to a concrete type `T`.
///
/// This utility function is used internally by the message dispatch mechanism
/// (specifically within the closure generated by `mutate_on`) to safely convert
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
// (Default, From remain internal and undocumented)

impl<State: Default + Send + Debug + 'static> From<ManagedActor<Idle, State>>
    for ManagedActor<Started, State>
{
    fn from(value: ManagedActor<Idle, State>) -> Self {
        // Ensure cancellation_token is always present when transitioning to Started state
        assert!(
            value.cancellation_token.is_some(),
            "Cannot transition to ManagedActor<Started, State> without a cancellation_token"
        );
        // Move all fields from Idle state to Started state.
        Self {
            handle: value.handle,
            parent: value.parent,
            halt_signal: value.halt_signal,
            id: value.id,
            runtime: value.runtime,
            model: value.model,
            start_tasks: value.start_tasks,
            inbox: value.inbox,
            before_start: value.before_start,
            after_start: value.after_start,
            before_stop: value.before_stop,
            after_stop: value.after_stop,
            broker: value.broker,
            message_handlers: value.message_handlers,
            read_only_handlers: value.read_only_handlers,
            error_handler_map: value.error_handler_map, // transfer error handlers
            cancellation_token: value.cancellation_token,
            restart_policy: value.restart_policy,
            supervision_strategy: value.supervision_strategy,
            escalation: value.escalation,
            restart_limiter_config: value.restart_limiter_config,
            expose_for_ipc: value.expose_for_ipc,
            // Carried rather than reset: an actor may register children before
            // it starts, and those registrations must survive the transition.
            supervision: value.supervision,
            _actor_state: PhantomData,
        }
    }
}

impl<State: Default + Send + Debug + 'static> Default for ManagedActor<Idle, State> {
    fn default() -> Self {
        use crate::common::config::CONFIG;
        let capacity = CONFIG.limits.actor_inbox_capacity;
        let (outbox, inbox) = channel(capacity);
        let id = Ern::default();
        // Use efficient constructor that avoids creating a throwaway channel
        let handle = ActorHandle::new(id.clone(), outbox);

        Self {
            handle,
            id,
            inbox,
            // Lifecycle hooks are None by default to avoid allocation.
            // Only allocate when user registers a hook via before_start(), etc.
            before_start: None,
            after_start: None,
            before_stop: None,
            after_stop: None,
            model: State::default(),
            // Use placeholder for broker - will be set later in new() if runtime is provided
            broker: ActorHandle::placeholder(),
            error_handler_map: HashMap::new(),
            parent: Option::default(),
            runtime: ActorRuntime::default(),
            halt_signal: AtomicBool::default(),
            start_tasks: TaskTracker::default(),
            cancellation_token: Option::default(),
            message_handlers: HashMap::new(),
            read_only_handlers: HashMap::new(),
            restart_policy: RestartPolicy::default(),
            supervision_strategy: SupervisionStrategy::default(),
            escalation: Escalation::default(),
            restart_limiter_config: None,
            expose_for_ipc: false,
            supervision: SupervisionRegistry::default(),
            _actor_state: PhantomData,
        }
    }
}

