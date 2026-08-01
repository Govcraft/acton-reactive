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

use std::any::{Any, TypeId};
use std::fmt::Debug;
use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use futures::future::join_all;
use futures::stream::{FuturesUnordered, StreamExt};
use futures::FutureExt;
use tokio_util::task::TaskTracker;
use tracing::{error, instrument, trace};

use crate::actor::{ManagedActor, TerminationReason};
use crate::common::config::CONFIG;
use crate::common::{
    Envelope, FutureBoxReadOnlyOutcome, OutboundEnvelope, ReactorItem, ReactorMap,
    ReadOnlyHandlerError,
};
use crate::message::{
    BrokerRequestEnvelope, CascadeTerminate, ChildTerminated, MessageAddress,
    RegisterSupervisedChild, RemoveAllSubscriptions, RestartDue, SupervisedChildStarted,
    SystemSignal, UnregisterSupervisedChild,
};
use crate::traits::ActorHandleInterface;

/// Type-state marker for a [`ManagedActor`] that is actively running and processing messages.
///
/// When a `ManagedActor` is in the `Started` state, its main asynchronous task (`wake`)
/// is running, receiving messages from its inbox and dispatching them to the appropriate
/// handlers registered during the [`Idle`](super::Idle) state.
///
/// Actors in this state can create message envelopes using methods like [`ManagedActor::new_envelope`]
/// and [`ManagedActor::new_parent_envelope`]. Interaction typically occurs via the actor's
/// [`ActorHandle`](crate::common::ActorHandle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)] // Add common derives
pub struct Started;

mod panic_helpers {
    use std::any::Any;

    /// Extracts a human-readable message from a panic payload.
    pub(super) fn extract_panic_message(payload: &Box<dyn Any + Send>) -> String {
        payload
            .downcast_ref::<&str>()
            .map_or_else(
                || {
                    payload.downcast_ref::<String>().map_or_else(
                        || format!("Panic with payload type: {:?}", (**payload).type_id()),
                        Clone::clone,
                    )
                },
                |s| (*s).to_string(),
            )
    }

    /// Logs a panic that occurred in a handler.
    #[cfg(feature = "catch-handler-panics")]
    pub(super) fn log_handler_panic(
        actor_id: &acton_ern::Ern,
        message_type_id: std::any::TypeId,
        panic_payload: &Box<dyn Any + Send>,
        context: &str,
    ) {
        let panic_msg = extract_panic_message(panic_payload);
        tracing::error!(
            actor_id = %actor_id,
            message_type = ?message_type_id,
            panic_message = %panic_msg,
            "{context}"
        );
    }

    /// Logs a panic that occurred in an error handler.
    #[cfg(feature = "catch-handler-panics")]
    pub(super) fn log_error_handler_panic(
        actor_id: &acton_ern::Ern,
        message_type_id: std::any::TypeId,
        error_type_id: std::any::TypeId,
        panic_payload: &Box<dyn Any + Send>,
        context: &str,
    ) {
        let panic_msg = extract_panic_message(panic_payload);
        tracing::error!(
            actor_id = %actor_id,
            message_type = ?message_type_id,
            error_type = ?error_type_id,
            panic_message = %panic_msg,
            "{context}"
        );
    }

    /// Logs a panic that occurred in a lifecycle hook.
    #[cfg(feature = "catch-handler-panics")]
    pub(super) fn log_lifecycle_panic(
        actor_id: &acton_ern::Ern,
        panic_payload: &Box<dyn Any + Send>,
        context: &str,
    ) {
        let panic_msg = extract_panic_message(panic_payload);
        tracing::error!(actor_id = %actor_id, panic_message = %panic_msg, "{context}");
    }
}

use panic_helpers::extract_panic_message;
#[cfg(feature = "catch-handler-panics")]
use panic_helpers::{log_error_handler_panic, log_handler_panic, log_lifecycle_panic};

/// Runs a lifecycle hook with optional panic protection.
/// Uses two-layer `catch_unwind` because hooks take `&self` (shared ref), which
/// requires `State: Sync` for a merged async block to be `Send`.
macro_rules! run_lifecycle_hook {
    ($self:expr, $hook:ident, $hook_name:literal) => {{
        if let Some(ref hook) = $self.$hook {
            #[cfg(feature = "catch-handler-panics")]
            {
                let hook_result =
                    std::panic::catch_unwind(AssertUnwindSafe(|| hook($self)));
                match hook_result {
                    Ok(future) => {
                        if let Err(ref panic_payload) =
                            AssertUnwindSafe(future).catch_unwind().await
                        {
                            log_lifecycle_panic(
                                $self.id(),
                                panic_payload,
                                concat!("Panic in ", $hook_name, " lifecycle hook"),
                            );
                        }
                    }
                    Err(ref panic_payload) => {
                        log_lifecycle_panic(
                            $self.id(),
                            panic_payload,
                            concat!("Panic in ", $hook_name, " lifecycle hook"),
                        );
                    }
                }
            }
            #[cfg(not(feature = "catch-handler-panics"))]
            {
                hook($self).await;
            }
        }
    }};
}

/// Implements methods specific to a `ManagedActor` in the `Started` state.
impl<Actor: Default + Send + Debug + 'static> ManagedActor<Started, Actor> {
    /// Creates a new [`OutboundEnvelope`] originating from this actor.
    ///
    /// This helper function constructs an envelope suitable for sending a message
    /// from this actor to another recipient. The envelope's `return_address`
    /// will be set to this actor's [`MessageAddress`]. The `recipient_address`
    /// field will be `None` initially and should typically be set using the
    /// envelope's methods before sending.
    ///
    /// # Returns
    ///
    /// An [`OutboundEnvelope`] configured with this actor as the sender.
    /// Returns `None` only if the actor's handle somehow lacks an outbox, which
    /// should not occur under normal circumstances.
    pub fn new_envelope(&self) -> Option<OutboundEnvelope> {
        self.cancellation_token.clone().map(|cancellation_token| {
            OutboundEnvelope::new(
                MessageAddress::new(self.handle.outbox.clone(), self.id.clone()),
                cancellation_token,
            )
        })
    }

    /// Creates a new [`OutboundEnvelope`] addressed to this actor's parent.
    ///
    /// This is a convenience method for creating an envelope specifically for
    /// replying or sending a message to the actor that supervises this one.
    /// It clones the parent's return address information.
    ///
    /// # Returns
    ///
    /// *   `Some(OutboundEnvelope)`: An envelope configured to be sent to the parent,
    ///     if this actor has a parent. The `return_address` will be the parent's address,
    ///     and the `recipient_address` will be this actor's address.
    /// *   `None`: If this actor does not have a parent (i.e., it's a top-level actor).
    pub fn new_parent_envelope(&self) -> Option<OutboundEnvelope> {
        // Only construct if both parent and cancellation_token exist
        let cancellation_token = self.cancellation_token.clone()?;
        self.parent.as_ref().map(|parent_handle| {
            OutboundEnvelope::new_with_recipient(
                MessageAddress::new(self.handle.outbox.clone(), self.id.clone()), // Self is sender
                parent_handle.reply_address(), // Parent is recipient
                cancellation_token,
            )
        })
    }

    /// Handles dispatching a mutable reactor with error and panic handling.
    ///
    /// Panics in message handlers are caught and logged, allowing the actor to continue
    /// processing subsequent messages. This provides fault isolation.
    async fn dispatch_mutable_handler(
        &mut self,
        reactor: &ReactorItem<Actor>,
        envelope: &mut Envelope,
    ) {
        let message_type_id = envelope.message.as_any().type_id();

        match reactor {
            ReactorItem::Mutable(fut) => {
                self.dispatch_mutable_infallible(fut, envelope, message_type_id).await;
            }
            ReactorItem::MutableFallible(fut) => {
                self.dispatch_mutable_fallible(fut, envelope, message_type_id).await;
            }
            ReactorItem::MutableSync(handler) => {
                #[cfg(feature = "catch-handler-panics")]
                {
                    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        handler(self, envelope);
                    }));
                    if let Err(ref panic_payload) = result {
                        log_handler_panic(
                            self.id(),
                            message_type_id,
                            panic_payload,
                            "Panic in sync mutable message handler",
                        );
                    }
                }
                #[cfg(not(feature = "catch-handler-panics"))]
                {
                    let _ = message_type_id;
                    handler(self, envelope);
                }
            }
            ReactorItem::ReadOnly(_) | ReactorItem::ReadOnlyFallible(_) | ReactorItem::ReadOnlySync(_) => {
                tracing::warn!("Found read-only handler in mutable_reactors map");
            }
        }
    }

    /// Dispatches an infallible mutable handler with optional panic protection.
    async fn dispatch_mutable_infallible(
        &mut self,
        fut: &crate::common::FutureHandler<Actor>,
        envelope: &mut Envelope,
        message_type_id: TypeId,
    ) {
        #[cfg(feature = "catch-handler-panics")]
        {
            let result = AssertUnwindSafe(async { fut(self, envelope).await })
                .catch_unwind()
                .await;
            if let Err(ref panic_payload) = result {
                log_handler_panic(
                    self.id(),
                    message_type_id,
                    panic_payload,
                    "Panic in mutable message handler",
                );
            }
        }
        #[cfg(not(feature = "catch-handler-panics"))]
        {
            let _ = message_type_id;
            fut(self, envelope).await;
        }
    }

    /// Dispatches a fallible mutable handler with optional panic protection and error handling.
    async fn dispatch_mutable_fallible(
        &mut self,
        fut: &crate::common::FutureHandlerResult<Actor>,
        envelope: &mut Envelope,
        message_type_id: TypeId,
    ) {
        #[cfg(feature = "catch-handler-panics")]
        {
            let result = AssertUnwindSafe(async { fut(self, envelope).await })
                .catch_unwind()
                .await;
            match result {
                Ok(Ok(_)) => { /* Handler succeeded */ }
                Ok(Err((err, error_type_id))) => {
                    self.handle_fallible_error(envelope, message_type_id, error_type_id, err)
                        .await;
                }
                Err(ref panic_payload) => {
                    log_handler_panic(
                        self.id(),
                        message_type_id,
                        panic_payload,
                        "Panic in mutable fallible message handler",
                    );
                }
            }
        }
        #[cfg(not(feature = "catch-handler-panics"))]
        {
            match fut(self, envelope).await {
                Ok(_) => { /* Handler succeeded */ }
                Err((err, error_type_id)) => {
                    self.handle_fallible_error(envelope, message_type_id, error_type_id, err)
                        .await;
                }
            }
        }
    }


    /// Handles an error from a fallible handler, invoking the error handler if registered.
    async fn handle_fallible_error(
        &mut self,
        envelope: &mut Envelope,
        message_type_id: TypeId,
        error_type_id: TypeId,
        err: Box<dyn std::error::Error + Send + Sync>,
    ) {
        if let Some(handler) = self.error_handler_map.remove(&(message_type_id, error_type_id)) {
            #[cfg(feature = "catch-handler-panics")]
            {
                let result =
                    AssertUnwindSafe(async { handler(self, envelope, err.as_ref()).await })
                        .catch_unwind()
                        .await;
                if let Err(ref panic_payload) = result {
                    log_error_handler_panic(
                        self.id(),
                        message_type_id,
                        error_type_id,
                        panic_payload,
                        "Panic in error handler",
                    );
                }
            }
            #[cfg(not(feature = "catch-handler-panics"))]
            {
                handler(self, envelope, err.as_ref()).await;
            }
            self.error_handler_map
                .insert((message_type_id, error_type_id), handler);
        } else {
            error!(
                actor_id = %self.id(),
                message_type = ?message_type_id,
                error = ?err,
                "Unhandled error from message handler"
            );
        }
    }

    /// Drains all in-flight read-only handler futures, then dispatches any errors
    /// their fallible handlers returned.
    ///
    /// Read-only handler futures run concurrently and cannot access the actor mutably,
    /// so a failing `try_act_on` handler cannot invoke its registered error handler
    /// directly. Instead, each completed fallible future carries its error context
    /// back here, where the actor loop has exclusive (`&mut`) access, and the errors
    /// are routed through [`handle_fallible_error`](Self::handle_fallible_error)
    /// exactly like the `try_mutate_on` path. All concurrent handlers are drained
    /// before any error handler runs, preserving read-only handler concurrency.
    async fn flush_read_only_handlers(
        &mut self,
        read_only_futures: &mut FuturesUnordered<FutureBoxReadOnlyOutcome>,
    ) {
        let mut deferred_errors: Vec<ReadOnlyHandlerError> = Vec::new();
        while let Some(outcome) = read_only_futures.next().await {
            if let Some(handler_error) = outcome {
                deferred_errors.push(handler_error);
            }
        }
        for mut handler_error in deferred_errors {
            self.handle_fallible_error(
                &mut handler_error.envelope,
                handler_error.message_type_id,
                handler_error.error_type_id,
                handler_error.error,
            )
            .await;
        }
    }

    /// Enqueues a read-only handler as a future for concurrent execution.
    ///
    /// Instead of spawning a separate task for each handler, this pushes the future
    /// directly to `FuturesUnordered` for more efficient execution of lightweight handlers.
    /// This avoids the overhead of task creation and `JoinHandle` management.
    ///
    /// Fallible handlers ([`ReactorItem::ReadOnlyFallible`]) that return an error yield
    /// a [`ReadOnlyHandlerError`] from their future so the error can be dispatched to a
    /// registered error handler at the next flush point
    /// (see [`flush_read_only_handlers`](Self::flush_read_only_handlers)).
    ///
    /// Panics in read-only handlers are caught and logged, preventing one panicking
    /// handler from affecting other concurrent handlers or crashing the actor.
    ///
    /// # Panic Safety
    ///
    /// Both the synchronous closure invocation (which creates the future) and the
    /// asynchronous execution of the future are protected from panics.
    fn enqueue_read_only_handler(
        &self,
        reactor: &ReactorItem<Actor>,
        envelope: &mut Envelope,
        read_only_futures: &FuturesUnordered<FutureBoxReadOnlyOutcome>,
    ) {
        let actor_id = self.id().clone();
        let message_type_id = envelope.message.as_any().type_id();

        match reactor {
            ReactorItem::ReadOnly(fut) => {
                #[cfg(feature = "catch-handler-panics")]
                {
                    let future_result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        fut(self, envelope)
                    }));
                    match future_result {
                        Ok(future) => {
                            read_only_futures.push(Box::pin(async move {
                                if let Err(panic_payload) =
                                    AssertUnwindSafe(future).catch_unwind().await
                                {
                                    let panic_msg = extract_panic_message(&panic_payload);
                                    error!(
                                        actor_id = %actor_id,
                                        message_type = ?message_type_id,
                                        panic_message = %panic_msg,
                                        "Panic in read-only message handler"
                                    );
                                }
                                None
                            }));
                        }
                        Err(panic_payload) => {
                            log_handler_panic(
                                &actor_id,
                                message_type_id,
                                &panic_payload,
                                "Panic in read-only message handler (during closure invocation)",
                            );
                        }
                    }
                }
                #[cfg(not(feature = "catch-handler-panics"))]
                {
                    let _ = (actor_id, message_type_id);
                    let future = fut(self, envelope);
                    read_only_futures.push(Box::pin(async move {
                        future.await;
                        None
                    }));
                }
            }
            ReactorItem::ReadOnlyFallible(fut) => {
                #[cfg(feature = "catch-handler-panics")]
                {
                    let future_result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        fut(self, envelope)
                    }));
                    match future_result {
                        Ok(future) => {
                            // Clone the envelope so the error handler retains the original
                            // message and reply context, mirroring the `try_mutate_on` path.
                            let envelope = envelope.clone();
                            read_only_futures.push(Box::pin(async move {
                                match AssertUnwindSafe(future).catch_unwind().await {
                                    Ok(Ok(_)) => None, /* Handler succeeded */
                                    Ok(Err((error, error_type_id))) => {
                                        // Carry the error back to the actor loop so it can be
                                        // dispatched to a registered error handler at the next
                                        // flush point, where `&mut` access is available.
                                        Some(ReadOnlyHandlerError {
                                            envelope,
                                            message_type_id,
                                            error_type_id,
                                            error,
                                        })
                                    }
                                    Err(panic_payload) => {
                                        let panic_msg = extract_panic_message(&panic_payload);
                                        error!(
                                            actor_id = %actor_id,
                                            message_type = ?message_type_id,
                                            panic_message = %panic_msg,
                                            "Panic in read-only fallible message handler"
                                        );
                                        None
                                    }
                                }
                            }));
                        }
                        Err(panic_payload) => {
                            log_handler_panic(
                                &actor_id,
                                message_type_id,
                                &panic_payload,
                                "Panic in read-only fallible message handler (during closure invocation)",
                            );
                        }
                    }
                }
                #[cfg(not(feature = "catch-handler-panics"))]
                {
                    let future = fut(self, envelope);
                    // Clone the envelope so the error handler retains the original
                    // message and reply context, mirroring the `try_mutate_on` path.
                    let envelope = envelope.clone();
                    read_only_futures.push(Box::pin(async move {
                        match future.await {
                            Ok(_) => None, /* Handler succeeded */
                            Err((error, error_type_id)) => {
                                // Carry the error back to the actor loop so it can be
                                // dispatched to a registered error handler at the next
                                // flush point, where `&mut` access is available.
                                Some(ReadOnlyHandlerError {
                                    envelope,
                                    message_type_id,
                                    error_type_id,
                                    error,
                                })
                            }
                        }
                    }));
                }
            }
            ReactorItem::ReadOnlySync(handler) => {
                #[cfg(feature = "catch-handler-panics")]
                {
                    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        handler(self, envelope);
                    }));
                    if let Err(ref panic_payload) = result {
                        log_handler_panic(
                            &actor_id,
                            message_type_id,
                            panic_payload,
                            "Panic in sync read-only message handler",
                        );
                    }
                }
                #[cfg(not(feature = "catch-handler-panics"))]
                {
                    let _ = (actor_id, message_type_id);
                    handler(self, envelope);
                }
            }
            _ => {
                tracing::warn!("Found mutable handler in read_only_reactors map");
            }
        }
    }

    // wake() and terminate() are internal implementation details (`pub(crate)` or private)
    // and do not require public documentation.
    #[instrument(skip(mutable_reactors, read_only_reactors, self))]
    pub(crate) async fn wake(
        &mut self,
        mutable_reactors: ReactorMap<Actor>,
        read_only_reactors: ReactorMap<Actor>,
    ) {
        // With `catch-handler-panics` enabled, panics are caught (and logged) at each
        // handler dispatch site, so the message loop itself cannot unwind and the
        // actor keeps running after a panicking handler.
        #[cfg(feature = "catch-handler-panics")]
        let termination_reason = self
            .run_message_loop(&mutable_reactors, &read_only_reactors)
            .await;

        // Without `catch-handler-panics`, a panicking handler unwinds the message
        // loop. Catch it here, at the actor-task boundary, so the actor terminates
        // cleanly and its parent is notified with `TerminationReason::Panic`.
        //
        // `AssertUnwindSafe` is sound here because the actor never processes another
        // message after the catch: only the shutdown path below runs (broker
        // cleanup, terminate children, `after_stop` hook, parent notification) and
        // then the task exits, dropping the actor state. The one piece of user code
        // that still observes the post-panic state is the `after_stop` hook — see
        // the cleanup guard below, which keeps a panic there from swallowing the
        // parent notification.
        #[cfg(not(feature = "catch-handler-panics"))]
        let termination_reason = match AssertUnwindSafe(
            self.run_message_loop(&mutable_reactors, &read_only_reactors),
        )
        .catch_unwind()
        .await
        {
            Ok(reason) => reason,
            Err(panic_payload) => {
                let panic_msg = extract_panic_message(&panic_payload);
                error!(
                    actor_id = %self.id(),
                    panic_message = %panic_msg,
                    "Actor terminated due to panic in message handler"
                );
                TerminationReason::Panic(panic_msg)
            }
        };

        trace!("Message loop finished for actor: {}. Initiating final termination.", self.id());

        // The cleanup runs user code (`after_stop`) against whatever state the
        // terminated actor was left in — after a caught handler panic, that
        // includes state the handler abandoned mid-mutation. Guard the whole
        // cleanup so a second panic there cannot swallow the parent notification:
        // the notification below is sent regardless, a handler panic's reason is
        // preserved over a subsequent cleanup panic, and a cleanup panic during an
        // otherwise clean termination is reported as `Panic` itself.
        #[cfg(not(feature = "catch-handler-panics"))]
        let termination_reason = match AssertUnwindSafe(self.shutdown_cleanup())
            .catch_unwind()
            .await
        {
            Ok(()) => termination_reason,
            Err(panic_payload) => {
                let panic_msg = extract_panic_message(&panic_payload);
                error!(
                    actor_id = %self.id(),
                    panic_message = %panic_msg,
                    "Panic during actor shutdown cleanup; parent notification is still sent"
                );
                if matches!(termination_reason, TerminationReason::Panic(_)) {
                    termination_reason
                } else {
                    TerminationReason::Panic(panic_msg)
                }
            }
        };

        // With `catch-handler-panics` enabled, `run_lifecycle_hook!` already
        // catches and logs hook panics, so the cleanup cannot unwind.
        #[cfg(feature = "catch-handler-panics")]
        self.shutdown_cleanup().await;

        // Notify parent of termination if we have a parent
        // We extract everything we need before the await to avoid holding &self across await
        if let Some(parent) = &self.parent {
            let notification = ChildTerminated::new(
                self.id.clone(),
                termination_reason,
                self.restart_policy,
            );

            trace!(
                "Notifying parent {} of child {} termination: {:?}",
                parent.id(),
                self.id(),
                notification
            );

            // Clone the parent handle to avoid borrowing self across await
            let parent_clone = parent.clone();
            parent_clone.send(notification).await;
        }

        trace!("Actor {} stopped.", self.id());
    }

    /// Runs the actor's message loop until it terminates, returning the reason.
    ///
    /// Encapsulating the loop lets [`wake`](Self::wake) wrap it in `catch_unwind`
    /// when `catch-handler-panics` is disabled, so a panicking handler still
    /// produces a parent notification.
    async fn run_message_loop(
        &mut self,
        mutable_reactors: &ReactorMap<Actor>,
        read_only_reactors: &ReactorMap<Actor>,
    ) -> TerminationReason {
        run_lifecycle_hook!(self, after_start, "after_start");
        assert!(
            self.cancellation_token.is_some(),
            "ManagedActor in Started state must always have a cancellation_token"
        );
        let cancel_token = self.cancellation_token.clone().unwrap();
        let mut cancel = Box::pin(cancel_token.cancelled());

        let mut read_only_futures: FuturesUnordered<FutureBoxReadOnlyOutcome> =
            FuturesUnordered::new();
        let high_water_mark = CONFIG.limits.concurrent_handlers_high_water_mark;
        let max_wait_duration = Duration::from_millis(CONFIG.timeouts.read_only_handler_flush);
        let mut last_flush_time = Instant::now();

        // Track the termination reason - set when the loop exits
        let termination_reason;

        loop {
            // Children recorded by a handler are handed to start tasks here, at
            // the top of the turn, before this actor waits for anything else.
            // Deliberately not inside the message arm: a handler's registration
            // must be acted on even if no further message ever arrives, and both
            // supervision arms below `continue` back to here rather than falling
            // through. No await: the start itself runs elsewhere and reports
            // back through the inbox, so a child slow to start cannot stop this
            // actor from taking messages.
            if self.supervision.has_pending_starts() {
                self.launch_pending_starts();
            }

            tokio::select! {
                () = &mut cancel => {
                    trace!("Forceful cancellation triggered for actor: {}", self.id());
                    self.flush_read_only_handlers(&mut read_only_futures).await;
                    // Parent-initiated shutdown via cancellation token
                    termination_reason = TerminationReason::ParentShutdown;
                    break;
                }

                () = tokio::time::sleep_until((last_flush_time + max_wait_duration).into()), if !read_only_futures.is_empty() => {
                    self.flush_read_only_handlers(&mut read_only_futures).await;
                    last_flush_time = Instant::now();
                }

                incoming_opt = self.inbox.recv() => {
                    let Some(incoming_envelope) = incoming_opt else {
                        // Inbox closed without Terminate signal = unexpected closure
                        termination_reason = TerminationReason::InboxClosed;
                        break;
                    };
                    // Extract envelope and type_id, handling BrokerRequestEnvelope indirection
                    let (mut envelope, type_id) = if let Some(broker_req) = incoming_envelope
                        .message.as_any().downcast_ref::<BrokerRequestEnvelope>()
                    {
                        (
                            Envelope::new(broker_req.message.clone(), incoming_envelope.reply_to.clone(), incoming_envelope.recipient.clone()),
                            broker_req.message.as_any().type_id()
                        )
                    } else {
                        let type_id = incoming_envelope.message.as_any().type_id();
                        (incoming_envelope, type_id)
                    };

                    // Supervision bookkeeping runs ahead of handler dispatch and is
                    // never user-dispatchable. Compared by TypeId against the value
                    // already computed above, so the hot path costs integer compares
                    // rather than a downcast per message.
                    if type_id == TypeId::of::<RegisterSupervisedChild>() {
                        if let Some(registration) = envelope.message.as_any().downcast_ref::<RegisterSupervisedChild>() {
                            self.register_supervised_child(registration);
                        }
                        continue;
                    } else if type_id == TypeId::of::<SupervisedChildStarted>() {
                        if let Some(started) = envelope.message.as_any().downcast_ref::<SupervisedChildStarted>() {
                            self.record_started_child(started);
                        }
                        continue;
                    } else if type_id == TypeId::of::<UnregisterSupervisedChild>() {
                        if let Some(release) = envelope.message.as_any().downcast_ref::<UnregisterSupervisedChild>() {
                            self.unregister_supervised_child(release);
                        }
                        continue;
                    } else if type_id == TypeId::of::<RestartDue>() {
                        if let Some(due) = envelope.message.as_any().downcast_ref::<RestartDue>() {
                            self.record_restart_due(due);
                        }
                        continue;
                    }

                    // `ChildTerminated` is intercepted too, and is the one that
                    // deliberately does **not** `continue`. The arms above are
                    // crate-internal messages a user cannot handle. This one is
                    // public, is in the prelude, and has user handlers pinned in
                    // this crate's own test suite — and until this release the
                    // deprecated supervision config setters actively instructed
                    // people to write one. Skipping dispatch here would silently
                    // break every one of them, so the restart engine does its
                    // bookkeeping and then lets the message carry on.
                    //
                    // Bookkeeping first, so that a user handler which inspects
                    // its supervisor sees the decision already recorded rather
                    // than a half-updated registry.
                    if type_id == TypeId::of::<ChildTerminated>() {
                        if let Some(notice) = envelope.message.as_any().downcast_ref::<ChildTerminated>() {
                            self.record_child_terminated(notice);
                        }
                    }

                    // Dispatch to registered handler or handle system signals
                    if let Some(reactor) = mutable_reactors.get(&type_id) {
                        self.flush_read_only_handlers(&mut read_only_futures).await;
                        last_flush_time = Instant::now();
                        self.dispatch_mutable_handler(reactor, &mut envelope).await;
                    } else if let Some(reactor) = read_only_reactors.get(&type_id) {
                        self.enqueue_read_only_handler(reactor, &mut envelope, &read_only_futures);
                        if read_only_futures.len() >= high_water_mark {
                            self.flush_read_only_handlers(&mut read_only_futures).await;
                            last_flush_time = Instant::now();
                        }
                    } else if let Some(stop_reason) = graceful_stop_reason(type_id, envelope.message.as_any()) {
                        // Both stop signals shut down identically; they differ
                        // only in the reason recorded. `CascadeTerminate` means
                        // the supervisor above is going away, which must never
                        // read as a restartable termination, so it reports
                        // `ParentShutdown` where a user-initiated `stop()`
                        // reports `Normal`.
                        self.flush_read_only_handlers(&mut read_only_futures).await;
                        trace!("Stop signal ({:?}) received for actor: {}. Closing inbox.", stop_reason, self.id());
                        run_lifecycle_hook!(self, before_stop, "before_stop");
                        self.inbox.close();
                        // Graceful shutdown - break to avoid overwriting
                        termination_reason = stop_reason;
                        break;
                    } else {
                        trace!("No handler found for message type {:?} for actor {}", type_id, self.id());
                    }
                }
            }
        }

        // Drain any in-flight read-only handlers (dispatching their collected
        // errors) before reporting the reason.
        self.flush_read_only_handlers(&mut read_only_futures).await;

        termination_reason
    }

    /// Final cleanup shared by every termination path: purge this actor's broker
    /// subscriptions, stop its children, and run the `after_stop` hook.
    ///
    /// With `catch-handler-panics` disabled this also runs after a panic
    /// termination, so a panicked actor's broker subscriptions do not leak.
    async fn shutdown_cleanup(&mut self) {
        // Suppress restart decisions before any child is touched. Stopping a
        // child produces a normal termination, and a `Permanent` child warrants
        // a restart on a normal termination, so without this a cascading
        // shutdown would read as a wave of failures worth restarting.
        self.supervision_mut().begin_shutdown();

        // Every start this actor asked for and never finished is answered here:
        // the ones still queued, which will now never be launched, and the ones
        // in flight, which this actor is no longer in a position to take on. The
        // callers waiting on their status channels are told, and the blueprints
        // they were holding are dropped.
        self.cancel_unfinished_children();

        // Stop accepting new messages on every termination path. The graceful
        // path has already closed the inbox; closing again is a no-op, but the
        // panic and cancellation paths reach here with it still open.
        //
        // This is also what tells an in-flight start task that there is nobody
        // to hand its child to, which is its cue to stop that child rather than
        // drop the only handle to it. Closed before the drain below, so a start
        // that has not delivered yet cannot land in a queue nothing will read.
        self.inbox.close();

        // Remove this actor from all broker subscriptions so subsequent broadcasts
        // are no longer sent to its (now closing) inbox. Skipped when no live broker
        // is reachable (e.g. the broker actor itself, or the broker already stopped).
        if !self.broker.outbox.is_closed() {
            trace!("Unsubscribing actor {} from all broker subscriptions.", self.id());
            let unsubscription = RemoveAllSubscriptions {
                subscriber_id: self.id.clone(),
            };
            self.broker.send(unsubscription).await;
        }

        // Nothing can be building a child by the time the inbox is drained.
        // Every start task has either delivered its child or, finding the inbox
        // closed just above, stopped it. Without this the drain would be racing
        // a writer, and a report that landed after it would take the only handle
        // to a running actor down with the queue.
        debug_assert!(
            self.inbox.is_closed(),
            "the inbox must be closed first, or a start delivering into it can never finish"
        );
        await_start_tasks(&self.start_tasks, self.id()).await;

        // A start that *did* deliver, just before the loop exited, is sitting in
        // the closed inbox right now holding a running child. Nobody is adding
        // to that queue any more, so what is in it is all there will be.
        let late_arrivals = self.take_late_started_children();

        // Ahead of the stops, so that no name outlives the mailbox it points at.
        // A cascading shutdown never reaches the terminal-state sweep — the
        // registry is already `shutting_down`, so every termination reads as
        // expected and no slot reaches `Down` — and without this a supervisor
        // that stops inside a still-running system leaves its children's names
        // pointing at dead mailboxes.
        #[cfg(feature = "ipc")]
        self.forget_children_ipc_names();

        terminate_children(self.shutdown_child_handles(late_arrivals), self.id()).await;
        run_lifecycle_hook!(self, after_stop, "after_stop");
    }
}

/// Waits out the start tasks a supervisor still has in flight.
///
/// This is what makes the inbox drain that follows it sound rather than merely
/// likely. A start task ends in one of two states — it delivered its child, or
/// it found nobody to deliver to and stopped the child itself — and only once
/// every task has reached one of them is it true that no further report can
/// arrive. Draining a queue somebody may still be writing to would leave
/// exactly the gap this closes.
///
/// **The inbox must already be closed.** That is what turns a delivery waiting
/// for capacity into a failed one; waiting first would mean waiting on tasks
/// that are waiting on this actor.
///
/// Waiting is possible at all only because start tasks are tracked separately
/// from the actor's own message loop. `wait()` completes when the tracker is
/// closed and empty, and this runs *inside* the loop's task — on the handle's
/// tracker it would be waiting for itself.
///
/// Bounded by the shutdown deadline, like every other wait on the way down. A
/// start that overruns it is not abandoned: that task still holds its child and
/// still stops it once it finds the inbox closed. What is lost is only this
/// actor's chance to stop it first.
///
/// A standalone async function for the same reason [`terminate_children`] is
/// one: an `async fn(&self)` on the actor yields a future holding `&Self`, which
/// is `Send` only if the user's model is `Sync`. `TaskTracker` and `Ern` are
/// both `Sync`, so borrowing them individually costs nothing.
async fn await_start_tasks(start_tasks: &TaskTracker, actor: &acton_ern::Ern) {
    start_tasks.close();
    if start_tasks.is_empty() {
        return;
    }

    let deadline = Duration::from_millis(CONFIG.timeouts.actor_shutdown);
    trace!(
        "Actor {actor} is waiting for {} in-flight child start(s)",
        start_tasks.len()
    );

    if tokio::time::timeout(deadline, start_tasks.wait())
        .await
        .is_err()
    {
        error!(
            "Actor {actor} stopped with {} child start(s) still in flight after {} ms; each remaining task stops its own child",
            start_tasks.len(),
            CONFIG.timeouts.actor_shutdown
        );
    }
}

/// The termination reason a graceful stop signal calls for, or `None` if this
/// message is not a stop signal at all.
///
/// The one place the two stops are told apart. A cascade is the framework
/// stopping a child because the supervisor above it is going away, which
/// [`RestartPolicy::should_restart`](crate::actor::RestartPolicy::should_restart)
/// never warrants a restart from; a `SystemSignal::Terminate` is someone calling
/// `stop()`, which a `Permanent` child legitimately is restarted from. Recording
/// `Normal` for both is what would make a supervisor restart the very children
/// it is shutting down.
///
/// Pure, and takes `&dyn Any` rather than an envelope, so the mapping can be
/// tested without a running actor.
fn graceful_stop_reason(type_id: TypeId, message: &dyn Any) -> Option<TerminationReason> {
    if type_id == TypeId::of::<CascadeTerminate>() {
        return Some(TerminationReason::ParentShutdown);
    }

    match message.downcast_ref::<SystemSignal>() {
        Some(SystemSignal::Terminate) => Some(TerminationReason::Normal),
        _ => None,
    }
}

/// Result of attempting to stop a single child actor.
enum ChildStopResult {
    /// Child stopped successfully
    Success,
    /// Child stop returned an error
    Error { child_id: String, error: String },
    /// Child stop timed out
    Timeout { child_id: String },
}

/// Terminates the given child actors concurrently.
///
/// Takes an owned list rather than reading it from a handle, because the set of
/// children to stop is the union of two views that can disagree — see
/// `ManagedActor::shutdown_child_handles`.
///
/// This is a standalone async function to avoid the `&mut self` / `&self` async borrow
/// checker constraints that would require `State: Sync`.
///
/// Timeout and error results are aggregated to avoid log flooding when many children
/// fail simultaneously.
#[instrument(skip(children))]
async fn terminate_children(children: Vec<crate::common::ActorHandle>, actor_id: &acton_ern::Ern) {
    use std::time::Duration;
    use tokio::time::timeout as tokio_timeout;

    trace!("Terminating children for actor: {}", actor_id);

    let timeout_ms = CONFIG.timeouts.actor_shutdown;

    let stop_futures: Vec<_> = children
        .into_iter()
        .map(|child_handle| {
            async move {
                trace!("Sending stop signal to child: {}", child_handle.id());
                // Not `stop()`: this is a cascade, so the child must record
                // `ParentShutdown` rather than `Normal`. Otherwise a `Permanent`
                // child reads as a normal termination worth restarting, and a
                // supervisor shutting down restarts the children it is stopping.
                let stop_res = tokio_timeout(
                    Duration::from_millis(timeout_ms),
                    child_handle.stop_for_parent_shutdown(),
                )
                .await;
                match stop_res {
                    Ok(Ok(())) => {
                        trace!(
                            "Stop signal sent to and child {} shut down successfully.",
                            child_handle.id()
                        );
                        ChildStopResult::Success
                    }
                    Ok(Err(e)) => {
                        trace!(
                            "Stop signal to child {} returned error: {:?}",
                            child_handle.id(),
                            e
                        );
                        ChildStopResult::Error {
                            child_id: child_handle.id().to_string(),
                            error: format!("{e:?}"),
                        }
                    }
                    Err(_) => {
                        trace!(
                            "Shutdown timeout for child {} after {} ms",
                            child_handle.id(),
                            timeout_ms
                        );
                        ChildStopResult::Timeout {
                            child_id: child_handle.id().to_string(),
                        }
                    }
                }
            }
        })
        .collect();

    let results = join_all(stop_futures).await;

    // Aggregate and log failures
    let mut timeout_children: Vec<&str> = Vec::new();
    let mut error_children: Vec<(&str, &str)> = Vec::new();

    for result in &results {
        match result {
            ChildStopResult::Success => {}
            ChildStopResult::Timeout { child_id } => {
                timeout_children.push(child_id);
            }
            ChildStopResult::Error { child_id, error } => {
                error_children.push((child_id, error));
            }
        }
    }

    if !timeout_children.is_empty() {
        tracing::error!(
            "Shutdown timeout ({} ms) for {} child(ren) of actor {}: [{}]",
            timeout_ms,
            timeout_children.len(),
            actor_id,
            timeout_children.join(", ")
        );
    }

    if !error_children.is_empty() {
        tracing::error!(
            "Shutdown errors for {} child(ren) of actor {}: [{}]",
            error_children.len(),
            actor_id,
            error_children
                .iter()
                .map(|(id, err)| format!("{id}: {err}"))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }

    trace!("All children stopped for actor: {}.", actor_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cascade is the one thing that must never read as restartable.
    #[test]
    fn a_cascade_terminate_maps_to_parent_shutdown() {
        let signal: &dyn Any = &CascadeTerminate;
        assert_eq!(
            graceful_stop_reason(TypeId::of::<CascadeTerminate>(), signal),
            Some(TerminationReason::ParentShutdown)
        );
    }

    /// The regression guard: an ordinary `stop()` stays restartable.
    #[test]
    fn a_terminate_signal_maps_to_normal() {
        let signal: &dyn Any = &SystemSignal::Terminate;
        assert_eq!(
            graceful_stop_reason(TypeId::of::<SystemSignal>(), signal),
            Some(TerminationReason::Normal)
        );
    }

    /// Anything else is not a stop signal and must not end the message loop.
    #[test]
    fn an_unrelated_message_is_not_a_stop_signal() {
        let message: &dyn Any = &42_u32;
        assert_eq!(graceful_stop_reason(TypeId::of::<u32>(), message), None);
    }
}
