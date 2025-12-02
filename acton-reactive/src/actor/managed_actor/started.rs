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

use std::any::Any;
use std::fmt::Debug;
use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use futures::future::join_all;
use futures::stream::{FuturesUnordered, StreamExt};
use futures::FutureExt;
use tracing::{error, instrument, trace};

use crate::actor::{ManagedActor, TerminationReason};
use crate::common::config::CONFIG;
use crate::common::{Envelope, FutureBoxReadOnly, OutboundEnvelope, ReactorItem, ReactorMap};
use crate::message::{BrokerRequestEnvelope, ChildTerminated, MessageAddress, SystemSignal};
use crate::traits::ActorHandleInterface;

/// Type-state marker for a [`ManagedActor`] that is actively running and processing messages.
///
/// When a `ManagedActor` is in the `Started` state, its main asynchronous task (`wake`)
/// is running, receiving messages from its inbox and dispatching them to the appropriate
/// handlers registered during the [`Idle`](super::Idle) state.
///
/// Actors in this state can create message envelopes using methods like [`ManagedActor::new_envelope`]
/// and [`ManagedActor::new_parent_envelope`]. Interaction typically occurs via the actor's
/// [`ActorHandle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)] // Add common derives
pub struct Started;

/// Extracts a human-readable message from a panic payload.
///
/// Handles the common cases where the panic was created with `panic!("message")` (produces `&str`)
/// or `panic!("{}", formatted)` (produces `String`). For other payload types, returns a debug
/// representation of the type.
fn extract_panic_message(payload: &Box<dyn Any + Send>) -> String {
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
fn log_handler_panic(
    actor_id: &acton_ern::Ern,
    message_type_id: std::any::TypeId,
    panic_payload: &Box<dyn Any + Send>,
    context: &str,
) {
    let panic_msg = extract_panic_message(panic_payload);
    error!(
        actor_id = %actor_id,
        message_type = ?message_type_id,
        panic_message = %panic_msg,
        "{context}"
    );
}

/// Logs a panic that occurred in an error handler.
fn log_error_handler_panic(
    actor_id: &acton_ern::Ern,
    message_type_id: std::any::TypeId,
    error_type_id: std::any::TypeId,
    panic_payload: &Box<dyn Any + Send>,
    context: &str,
) {
    let panic_msg = extract_panic_message(panic_payload);
    error!(
        actor_id = %actor_id,
        message_type = ?message_type_id,
        error_type = ?error_type_id,
        panic_message = %panic_msg,
        "{context}"
    );
}

/// Logs a panic that occurred in a lifecycle hook.
fn log_lifecycle_panic(actor_id: &acton_ern::Ern, panic_payload: &Box<dyn Any + Send>, context: &str) {
    let panic_msg = extract_panic_message(panic_payload);
    error!(actor_id = %actor_id, panic_message = %panic_msg, "{context}");
}

/// Runs a lifecycle hook with panic protection and logs any panics.
/// This macro avoids borrowing issues that would occur with a function.
macro_rules! run_lifecycle_hook {
    ($self:expr, $hook:ident, $hook_name:literal) => {{
        let hook_result = std::panic::catch_unwind(AssertUnwindSafe(|| ($self.$hook)($self)));
        match hook_result {
            Ok(future) => {
                if let Err(ref panic_payload) = AssertUnwindSafe(future).catch_unwind().await {
                    log_lifecycle_panic($self.id(), panic_payload,
                        concat!("Panic in ", $hook_name, " lifecycle hook (during await)"));
                }
            }
            Err(ref panic_payload) => {
                log_lifecycle_panic($self.id(), panic_payload,
                    concat!("Panic in ", $hook_name, " lifecycle hook (during closure invocation)"));
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
            ReactorItem::ReadOnly(_) | ReactorItem::ReadOnlyFallible(_) => {
                tracing::warn!("Found read-only handler in mutable_reactors map");
            }
        }
    }

    /// Dispatches an infallible mutable handler with panic protection.
    async fn dispatch_mutable_infallible(
        &mut self,
        fut: &crate::common::FutureHandler<Actor>,
        envelope: &mut Envelope,
        message_type_id: std::any::TypeId,
    ) {
        let future_result = std::panic::catch_unwind(AssertUnwindSafe(|| fut(self, envelope)));
        match future_result {
            Ok(future) => {
                if let Err(ref panic_payload) = AssertUnwindSafe(future).catch_unwind().await {
                    log_handler_panic(self.id(), message_type_id, panic_payload,
                        "Panic in mutable message handler (during await)");
                }
            }
            Err(ref panic_payload) => {
                log_handler_panic(self.id(), message_type_id, panic_payload,
                    "Panic in mutable message handler (during closure invocation)");
            }
        }
    }

    /// Dispatches a fallible mutable handler with panic and error handling.
    async fn dispatch_mutable_fallible(
        &mut self,
        fut: &crate::common::FutureHandlerResult<Actor>,
        envelope: &mut Envelope,
        message_type_id: std::any::TypeId,
    ) {
        let future_result = std::panic::catch_unwind(AssertUnwindSafe(|| fut(self, envelope)));
        match future_result {
            Ok(future) => {
                match AssertUnwindSafe(future).catch_unwind().await {
                    Ok(Ok(_)) => { /* Handler succeeded */ }
                    Ok(Err((err, error_type_id))) => {
                        self.handle_fallible_error(envelope, message_type_id, error_type_id, err).await;
                    }
                    Err(ref panic_payload) => {
                        log_handler_panic(self.id(), message_type_id, panic_payload,
                            "Panic in mutable fallible message handler (during await)");
                    }
                }
            }
            Err(ref panic_payload) => {
                log_handler_panic(self.id(), message_type_id, panic_payload,
                    "Panic in mutable fallible message handler (during closure invocation)");
            }
        }
    }


    /// Handles an error from a fallible handler, invoking the error handler if registered.
    async fn handle_fallible_error(
        &mut self,
        envelope: &mut Envelope,
        message_type_id: std::any::TypeId,
        error_type_id: std::any::TypeId,
        err: Box<dyn std::error::Error + Send + Sync>,
    ) {
        if let Some(handler) = self.error_handler_map.remove(&(message_type_id, error_type_id)) {
            let error_future_result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                handler(self, envelope, err.as_ref())
            }));

            match error_future_result {
                Ok(error_future) => {
                    if let Err(ref panic_payload) = AssertUnwindSafe(error_future).catch_unwind().await {
                        log_error_handler_panic(self.id(), message_type_id, error_type_id,
                            panic_payload, "Panic in error handler (during await)");
                    }
                }
                Err(ref panic_payload) => {
                    log_error_handler_panic(self.id(), message_type_id, error_type_id,
                        panic_payload, "Panic in error handler (during closure invocation)");
                }
            }
            self.error_handler_map.insert((message_type_id, error_type_id), handler);
        } else {
            error!(
                actor_id = %self.id(),
                message_type = ?message_type_id,
                error = ?err,
                "Unhandled error from message handler"
            );
        }
    }

    /// Enqueues a read-only handler as a future for concurrent execution.
    ///
    /// Instead of spawning a separate task for each handler, this pushes the future
    /// directly to `FuturesUnordered` for more efficient execution of lightweight handlers.
    /// This avoids the overhead of task creation and `JoinHandle` management.
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
        read_only_futures: &FuturesUnordered<FutureBoxReadOnly>,
    ) {
        let actor_id = self.id().clone();
        let message_type_id = envelope.message.as_any().type_id();

        match reactor {
            ReactorItem::ReadOnly(fut) => {
                // Protect the closure invocation from panics
                let future_result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    fut(self, envelope)
                }));

                match future_result {
                    Ok(future) => {
                        // Wrap handler with panic catching for the async execution
                        read_only_futures.push(Box::pin(async move {
                            let result = AssertUnwindSafe(future).catch_unwind().await;
                            if let Err(panic_payload) = result {
                                let panic_msg = extract_panic_message(&panic_payload);
                                error!(
                                    actor_id = %actor_id,
                                    message_type = ?message_type_id,
                                    panic_message = %panic_msg,
                                    "Panic in read-only message handler (during await)"
                                );
                            }
                        }));
                    }
                    Err(panic_payload) => {
                        let panic_msg = extract_panic_message(&panic_payload);
                        error!(
                            actor_id = %actor_id,
                            message_type = ?message_type_id,
                            panic_message = %panic_msg,
                            "Panic in read-only message handler (during closure invocation)"
                        );
                    }
                }
            }
            ReactorItem::ReadOnlyFallible(fut) => {
                // Protect the closure invocation from panics
                let future_result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    fut(self, envelope)
                }));

                match future_result {
                    Ok(future) => {
                        // Wrap fallible handler with panic catching and error logging
                        read_only_futures.push(Box::pin(async move {
                            let result = AssertUnwindSafe(future).catch_unwind().await;
                            match result {
                                Ok(Ok(_)) => { /* Handler succeeded */ }
                                Ok(Err((err, _error_type_id))) => {
                                    error!(
                                        actor_id = %actor_id,
                                        message_type = ?message_type_id,
                                        error = ?err,
                                        "Unhandled error from read-only message handler"
                                    );
                                }
                                Err(panic_payload) => {
                                    let panic_msg = extract_panic_message(&panic_payload);
                                    error!(
                                        actor_id = %actor_id,
                                        message_type = ?message_type_id,
                                        panic_message = %panic_msg,
                                        "Panic in read-only fallible message handler (during await)"
                                    );
                                }
                            }
                        }));
                    }
                    Err(panic_payload) => {
                        let panic_msg = extract_panic_message(&panic_payload);
                        error!(
                            actor_id = %actor_id,
                            message_type = ?message_type_id,
                            panic_message = %panic_msg,
                            "Panic in read-only fallible message handler (during closure invocation)"
                        );
                    }
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
        run_lifecycle_hook!(self, after_start, "after_start");
        assert!(
            self.cancellation_token.is_some(),
            "ManagedActor in Started state must always have a cancellation_token"
        );
        let cancel_token = self.cancellation_token.clone().unwrap();
        let mut cancel = Box::pin(cancel_token.cancelled());

        let mut read_only_futures: FuturesUnordered<FutureBoxReadOnly> = FuturesUnordered::new();
        let high_water_mark = CONFIG.limits.concurrent_handlers_high_water_mark;
        let max_wait_duration = Duration::from_millis(CONFIG.timeouts.read_only_handler_flush);
        let mut last_flush_time = Instant::now();

        // Track the termination reason - set when the loop exits
        let termination_reason;

        loop {
            tokio::select! {
                () = &mut cancel => {
                    trace!("Forceful cancellation triggered for actor: {}", self.id());
                    while read_only_futures.next().await.is_some() {}
                    // Parent-initiated shutdown via cancellation token
                    termination_reason = TerminationReason::ParentShutdown;
                    break;
                }

                () = tokio::time::sleep_until((last_flush_time + max_wait_duration).into()), if !read_only_futures.is_empty() => {
                    while read_only_futures.next().await.is_some() {}
                    last_flush_time = Instant::now();
                }

                incoming_opt = self.inbox.recv() => {
                    let Some(incoming_envelope) = incoming_opt else {
                        // Inbox closed without Terminate signal = unexpected closure
                        termination_reason = TerminationReason::InboxClosed;
                        break;
                    };
                    trace!("Received envelope from: {}", incoming_envelope.reply_to.sender.root);

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

                    // Dispatch to registered handler or handle system signals
                    if let Some(reactor) = mutable_reactors.get(&type_id) {
                        while read_only_futures.next().await.is_some() {}
                        last_flush_time = Instant::now();
                        self.dispatch_mutable_handler(reactor.value(), &mut envelope).await;
                    } else if let Some(reactor) = read_only_reactors.get(&type_id) {
                        self.enqueue_read_only_handler(reactor.value(), &mut envelope, &read_only_futures);
                        if read_only_futures.len() >= high_water_mark {
                            while read_only_futures.next().await.is_some() {}
                            last_flush_time = Instant::now();
                        }
                    } else if matches!(envelope.message.as_any().downcast_ref::<SystemSignal>(), Some(SystemSignal::Terminate)) {
                        while read_only_futures.next().await.is_some() {}
                        trace!("Terminate signal received for actor: {}. Closing inbox.", self.id());
                        run_lifecycle_hook!(self, before_stop, "before_stop");
                        self.inbox.close();
                        // Graceful shutdown via Terminate signal - break to avoid overwriting
                        termination_reason = TerminationReason::Normal;
                        break;
                    } else {
                        trace!("No handler found for message type {:?} for actor {}", type_id, self.id());
                    }
                }
            }
        }

        trace!("Message loop finished for actor: {}. Initiating final termination.", self.id());
        while read_only_futures.next().await.is_some() {}
        terminate_children(&self.handle, self.id()).await;
        run_lifecycle_hook!(self, after_stop, "after_stop");

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

/// Terminates all child actors of the given handle concurrently.
///
/// This is a standalone async function to avoid the `&mut self` / `&self` async borrow
/// checker constraints that would require `State: Sync`.
///
/// Timeout and error results are aggregated to avoid log flooding when many children
/// fail simultaneously.
#[instrument(skip(handle))]
async fn terminate_children(handle: &crate::common::ActorHandle, actor_id: &acton_ern::Ern) {
    use std::time::Duration;
    use tokio::time::timeout as tokio_timeout;

    trace!("Terminating children for actor: {}", actor_id);

    let timeout_ms = CONFIG.timeouts.actor_shutdown;

    let stop_futures: Vec<_> = handle
        .children()
        .iter()
        .map(|item| {
            let child_handle = item.value().clone();
            async move {
                trace!("Sending stop signal to child: {}", child_handle.id());
                let stop_res =
                    tokio_timeout(Duration::from_millis(timeout_ms), child_handle.stop()).await;
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
