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

//! Unix Domain Socket listener for IPC communication.
//!
//! This module provides the core IPC listener that accepts connections from
//! external processes and routes messages to actors.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_ern::prelude::*;
use dashmap::DashMap;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use super::config::IpcConfig;
use super::protocol::{
    is_discover, is_heartbeat, is_subscribe, is_unsubscribe, read_frame,
    write_discovery_response_with_format, write_heartbeat, write_push, write_response_with_format,
    write_stream_frame_with_format, write_subscription_response_with_format, Format,
    MSG_TYPE_REQUEST,
};
use super::rate_limiter::RateLimiter;
use super::registry::IpcTypeRegistry;
use super::subscription_manager::{create_push_channel, PushReceiver, SubscriptionManager};
use super::types::{
    ActorInfo, IpcDiscoverRequest, IpcDiscoverResponse, IpcEnvelope, IpcError, IpcResponse,
    IpcStreamFrame, IpcSubscribeRequest, IpcSubscriptionResponse, IpcUnsubscribeRequest,
};
use crate::common::{ActorHandle, Envelope};
use crate::message::MessageAddress;
use crate::traits::{ActonMessage, ActorHandleInterface};

/// Statistics for the IPC listener.
#[derive(Debug, Default)]
pub struct IpcListenerStats {
    /// Total connections accepted.
    pub connections_accepted: AtomicUsize,
    /// Currently active connections.
    pub connections_active: AtomicUsize,
    /// Total messages received.
    pub messages_received: AtomicUsize,
    /// Total messages successfully routed.
    pub messages_routed: AtomicUsize,
    /// Total errors encountered.
    pub errors: AtomicUsize,
    /// Total requests rate limited.
    pub rate_limited: AtomicUsize,
    /// Total requests rejected due to actor backpressure.
    pub backpressure_rejections: AtomicUsize,
    /// Total requests rejected due to shutdown.
    pub shutdown_rejections: AtomicUsize,
    /// Current number of in-flight requests being processed.
    pub in_flight_requests: AtomicUsize,
    /// Total subscription requests processed.
    pub subscriptions_processed: AtomicUsize,
    /// Total push notifications sent to IPC clients.
    pub push_notifications_sent: AtomicUsize,
}

/// Shutdown state for graceful shutdown coordination.
///
/// This tracks whether the listener is in the draining phase and how many
/// requests are currently being processed. During draining, new connections
/// are rejected but existing in-flight requests are allowed to complete.
#[derive(Debug, Default)]
pub struct ShutdownState {
    /// Whether we're in the draining phase (no new requests accepted).
    draining: AtomicBool,
}

impl ShutdownState {
    /// Create a new shutdown state.
    const fn new() -> Self {
        Self {
            draining: AtomicBool::new(false),
        }
    }

    /// Signal that we're entering the drain phase.
    pub fn start_draining(&self) {
        self.draining.store(true, Ordering::SeqCst);
    }

    /// Check if we're currently draining.
    #[must_use]
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }
}

/// Result of a graceful shutdown operation.
#[derive(Debug, Clone)]
pub struct ShutdownResult {
    /// Number of requests in-flight when shutdown started.
    pub in_flight_at_start: usize,
    /// Whether all requests completed before the timeout.
    pub drained_gracefully: bool,
    /// Number of requests that were force-closed.
    pub forced_closed: usize,
}

impl IpcListenerStats {
    /// Create new statistics.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the number of connections accepted.
    #[must_use]
    pub fn connections_accepted(&self) -> usize {
        self.connections_accepted.load(Ordering::Relaxed)
    }

    /// Get the number of active connections.
    #[must_use]
    pub fn connections_active(&self) -> usize {
        self.connections_active.load(Ordering::Relaxed)
    }

    /// Get the number of messages received.
    #[must_use]
    pub fn messages_received(&self) -> usize {
        self.messages_received.load(Ordering::Relaxed)
    }

    /// Get the number of messages successfully routed.
    #[must_use]
    pub fn messages_routed(&self) -> usize {
        self.messages_routed.load(Ordering::Relaxed)
    }

    /// Get the number of errors.
    #[must_use]
    pub fn errors(&self) -> usize {
        self.errors.load(Ordering::Relaxed)
    }

    /// Get the number of rate limited requests.
    #[must_use]
    pub fn rate_limited(&self) -> usize {
        self.rate_limited.load(Ordering::Relaxed)
    }

    /// Get the number of requests rejected due to actor backpressure.
    #[must_use]
    pub fn backpressure_rejections(&self) -> usize {
        self.backpressure_rejections.load(Ordering::Relaxed)
    }

    /// Get the number of requests rejected due to shutdown.
    #[must_use]
    pub fn shutdown_rejections(&self) -> usize {
        self.shutdown_rejections.load(Ordering::Relaxed)
    }

    /// Get the number of in-flight requests currently being processed.
    #[must_use]
    pub fn in_flight_requests(&self) -> usize {
        self.in_flight_requests.load(Ordering::SeqCst)
    }

    /// Get the number of subscription requests processed.
    #[must_use]
    pub fn subscriptions_processed(&self) -> usize {
        self.subscriptions_processed.load(Ordering::Relaxed)
    }

    /// Get the number of push notifications sent.
    #[must_use]
    pub fn push_notifications_sent(&self) -> usize {
        self.push_notifications_sent.load(Ordering::Relaxed)
    }

    /// Increment the in-flight request counter.
    fn increment_in_flight(&self) {
        self.in_flight_requests.fetch_add(1, Ordering::SeqCst);
    }

    /// Decrement the in-flight request counter.
    fn decrement_in_flight(&self) {
        self.in_flight_requests.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Shared context for connection handling.
///
/// This groups related parameters passed to connection handlers to reduce
/// function argument count and improve code clarity.
#[derive(Clone)]
struct ConnectionContext {
    /// IPC configuration.
    config: IpcConfig,
    /// Registry for deserializing message types.
    type_registry: Arc<IpcTypeRegistry>,
    /// Registry mapping logical names to actor handles.
    actor_registry: Arc<DashMap<String, ActorHandle>>,
    /// Token for shutdown signaling.
    cancel_token: CancellationToken,
    /// Semaphore for limiting concurrent connections.
    connection_semaphore: Arc<Semaphore>,
    /// Listener statistics.
    stats: Arc<IpcListenerStats>,
    /// Shutdown coordination state.
    shutdown_state: Arc<ShutdownState>,
    /// Subscription manager for broker forwarding.
    subscription_manager: Arc<SubscriptionManager>,
}

/// IPC listener handle for managing the listener lifecycle.
pub struct IpcListenerHandle {
    /// Statistics for the listener.
    pub stats: Arc<IpcListenerStats>,
    /// Cancellation token for graceful shutdown.
    cancel_token: CancellationToken,
    /// Shutdown state for coordinating graceful drain.
    shutdown_state: Arc<ShutdownState>,
    /// Configured drain timeout.
    drain_timeout: Duration,
    /// Subscription manager for broker forwarding.
    subscription_manager: Arc<SubscriptionManager>,
}

impl IpcListenerHandle {
    /// Request the listener to stop immediately.
    ///
    /// This cancels the listener without waiting for in-flight requests to complete.
    /// For a graceful shutdown that allows requests to finish, use [`shutdown_gracefully`].
    ///
    /// [`shutdown_gracefully`]: Self::shutdown_gracefully
    pub fn stop(&self) {
        self.cancel_token.cancel();
    }

    /// Check if the listener has been cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    /// Check if the listener is currently draining (rejecting new requests).
    #[must_use]
    pub fn is_draining(&self) -> bool {
        self.shutdown_state.is_draining()
    }

    /// Returns a reference to the subscription manager.
    ///
    /// Use this to forward broker broadcasts to subscribed IPC connections.
    #[must_use]
    pub const fn subscription_manager(&self) -> &Arc<SubscriptionManager> {
        &self.subscription_manager
    }

    /// Gracefully shut down the listener, allowing in-flight requests to complete.
    ///
    /// This initiates a two-phase shutdown:
    ///
    /// 1. **Drain phase**: Stop accepting new requests. Existing connections can
    ///    finish their current request but new requests receive a `ShuttingDown` error.
    ///
    /// 2. **Force close**: After the drain timeout expires, any remaining connections
    ///    are forcibly closed.
    ///
    /// # Arguments
    ///
    /// Uses the configured `drain_timeout` from [`IpcConfig`](super::IpcConfig).
    ///
    /// # Returns
    ///
    /// A [`ShutdownResult`] containing statistics about the shutdown process.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let result = listener_handle.shutdown_gracefully().await;
    /// if result.drained_gracefully {
    ///     println!("All {} requests completed cleanly", result.in_flight_at_start);
    /// } else {
    ///     println!("Force-closed {} requests", result.forced_closed);
    /// }
    /// ```
    pub async fn shutdown_gracefully(&self) -> ShutdownResult {
        self.shutdown_gracefully_with_timeout(self.drain_timeout)
            .await
    }

    /// Gracefully shut down with a custom timeout.
    ///
    /// See [`shutdown_gracefully`](Self::shutdown_gracefully) for details.
    pub async fn shutdown_gracefully_with_timeout(
        &self,
        drain_timeout: Duration,
    ) -> ShutdownResult {
        // Signal to stop accepting new requests
        self.shutdown_state.start_draining();
        info!("IPC listener entering drain phase");

        // Capture initial in-flight count
        let in_flight_at_start = self.stats.in_flight_requests();

        if in_flight_at_start > 0 {
            debug!(
                in_flight = in_flight_at_start,
                timeout_ms = drain_timeout.as_millis(),
                "Waiting for in-flight requests to complete"
            );
        }

        // Wait for in-flight requests to complete (with timeout)
        let drain_result = tokio::time::timeout(drain_timeout, async {
            // Poll until all in-flight requests complete
            while self.stats.in_flight_requests() > 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        let drained_gracefully = drain_result.is_ok();
        let remaining = self.stats.in_flight_requests();

        if drained_gracefully {
            info!("IPC listener drained successfully");
        } else {
            warn!(
                remaining,
                "IPC listener drain timeout expired, force-closing connections"
            );
        }

        // Now force cancel everything
        self.cancel_token.cancel();

        ShutdownResult {
            in_flight_at_start,
            drained_gracefully,
            forced_closed: remaining,
        }
    }
}

/// Run the IPC listener.
///
/// This function creates and binds the Unix socket, then enters a loop
/// accepting connections and spawning handlers for each.
///
/// # Arguments
///
/// * `config` - IPC configuration.
/// * `type_registry` - Registry for message type deserialization.
/// * `actor_registry` - Registry mapping logical names to actor handles.
/// * `cancel_token` - Token for graceful shutdown.
///
/// # Returns
///
/// An `IpcListenerHandle` for managing the listener, or an error if the
/// listener could not be started.
pub async fn run(
    config: IpcConfig,
    type_registry: Arc<IpcTypeRegistry>,
    actor_registry: Arc<DashMap<String, ActorHandle>>,
    cancel_token: CancellationToken,
) -> Result<IpcListenerHandle, IpcError> {
    let socket_path = config.socket_path();
    let stats = Arc::new(IpcListenerStats::new());

    // Create the socket directory if it doesn't exist
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            IpcError::IoError(format!(
                "Failed to create socket directory {}: {}",
                parent.display(),
                e
            ))
        })?;
    }

    // Remove stale socket if it exists
    if socket_path.exists() {
        if UnixStream::connect(&socket_path).await.is_ok() {
            return Err(IpcError::ProtocolError(
                "Another IPC listener is already running at this socket".to_string(),
            ));
        }
        // Stale socket, remove it
        warn!("Removing stale socket: {}", socket_path.display());
        tokio::fs::remove_file(&socket_path).await.map_err(|e| {
            IpcError::IoError(format!(
                "Failed to remove stale socket {}: {}",
                socket_path.display(),
                e
            ))
        })?;
    }

    // Bind the socket
    let listener = UnixListener::bind(&socket_path).map_err(|e| {
        IpcError::IoError(format!(
            "Failed to bind socket at {}: {}",
            socket_path.display(),
            e
        ))
    })?;

    // Set socket permissions (Unix only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(config.socket.mode);
        std::fs::set_permissions(&socket_path, perms).map_err(|e| {
            IpcError::IoError(format!(
                "Failed to set socket permissions on {}: {}",
                socket_path.display(),
                e
            ))
        })?;
    }

    info!("IPC listener started on: {}", socket_path.display());

    // Create a semaphore to limit concurrent connections
    let connection_semaphore = Arc::new(Semaphore::new(config.limits.max_connections));

    // Create shutdown state for graceful drain coordination
    let shutdown_state = Arc::new(ShutdownState::new());
    let drain_timeout = config.drain_timeout();

    // Create subscription manager for broker forwarding
    let subscription_manager = Arc::new(SubscriptionManager::new());

    // Create connection context for the accept loop
    let context = ConnectionContext {
        config,
        type_registry,
        actor_registry,
        cancel_token: cancel_token.clone(),
        connection_semaphore,
        stats: stats.clone(),
        shutdown_state: shutdown_state.clone(),
        subscription_manager: subscription_manager.clone(),
    };
    let socket_path_clone = socket_path.clone();

    // Spawn the accept loop
    tokio::spawn(async move {
        accept_loop(listener, context).await;

        // Cleanup socket on shutdown
        if let Err(e) = tokio::fs::remove_file(&socket_path_clone).await {
            warn!("Failed to remove socket file on shutdown: {}", e);
        } else {
            debug!("Socket file removed: {}", socket_path_clone.display());
        }

        info!("IPC listener shut down");
    });

    Ok(IpcListenerHandle {
        stats,
        cancel_token,
        shutdown_state,
        drain_timeout,
        subscription_manager,
    })
}

/// Main accept loop for the listener.
async fn accept_loop(listener: UnixListener, ctx: ConnectionContext) {
    loop {
        tokio::select! {
            biased;

            () = ctx.cancel_token.cancelled() => {
                info!("IPC listener received shutdown signal");
                break;
            }

            accept_result = listener.accept() => {
                // During draining, still accept connections but they'll get ShuttingDown errors
                // This allows clients to receive proper error responses instead of connection refused

                match accept_result {
                    Ok((stream, _addr)) => {
                        // Try to acquire a connection permit
                        let Ok(permit) = ctx.connection_semaphore.clone().try_acquire_owned() else {
                            warn!("Maximum concurrent connections reached, rejecting connection");
                            ctx.stats.errors.fetch_add(1, Ordering::Relaxed);
                            continue;
                        };

                        ctx.stats.connections_accepted.fetch_add(1, Ordering::Relaxed);
                        ctx.stats.connections_active.fetch_add(1, Ordering::Relaxed);

                        let conn_id = ctx.stats.connections_accepted.load(Ordering::Relaxed);
                        trace!("Accepted connection #{}", conn_id);

                        // Spawn connection handler with cloned context
                        let ctx_clone = ctx.clone();

                        tokio::spawn(async move {
                            handle_connection(stream, conn_id, ctx_clone).await;

                            // Release the permit when done
                            drop(permit);
                        });
                    }
                    Err(e) => {
                        error!("Failed to accept connection: {}", e);
                        ctx.stats.errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
    }
}

/// Result of handling a single request frame.
enum RequestResult {
    /// Continue processing more frames.
    Continue,
    /// Break out of the connection loop.
    Break,
}

/// Handle a single request frame within a connection.
async fn handle_request_frame(
    conn_id: usize,
    payload: &[u8],
    format: Format,
    rate_limiter: &mut RateLimiter,
    ctx: &ConnectionContext,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> RequestResult {
    ctx.stats.messages_received.fetch_add(1, Ordering::Relaxed);

    // Check if we're draining - reject new requests during shutdown
    if ctx.shutdown_state.is_draining() {
        ctx.stats
            .shutdown_rejections
            .fetch_add(1, Ordering::Relaxed);

        let correlation_id = format
            .deserialize::<IpcEnvelope>(payload)
            .map_or_else(|_| "unknown".to_string(), |e| e.correlation_id);

        let response = IpcResponse::error(&correlation_id, &IpcError::ShuttingDown);

        trace!(conn_id, "Rejecting request during shutdown drain");

        if let Err(e) = write_response_with_format(writer, &response, format).await {
            error!(conn_id, error = %e, "Failed to send shutdown response");
            return RequestResult::Break;
        }
        return RequestResult::Continue;
    }

    // Check rate limit before processing
    if !rate_limiter.try_acquire() {
        let retry_after_ms =
            u64::try_from(rate_limiter.time_until_available().as_millis()).unwrap_or(u64::MAX);
        ctx.stats.rate_limited.fetch_add(1, Ordering::Relaxed);

        let correlation_id = format
            .deserialize::<IpcEnvelope>(payload)
            .map_or_else(|_| "unknown".to_string(), |e| e.correlation_id);

        let err = IpcError::RateLimited { retry_after_ms };
        let response = IpcResponse::error(&correlation_id, &err);

        trace!(conn_id, retry_after_ms, "Rate limited");

        if let Err(e) = write_response_with_format(writer, &response, format).await {
            error!(conn_id, error = %e, "Failed to send rate limit response");
            return RequestResult::Break;
        }
        return RequestResult::Continue;
    }

    // Track in-flight request for graceful shutdown
    ctx.stats.increment_in_flight();

    // Parse envelope
    let envelope: IpcEnvelope = match format.deserialize(payload) {
        Ok(env) => env,
        Err(e) => {
            ctx.stats.decrement_in_flight();
            let response = IpcResponse::error_with_message(
                "unknown",
                "SERIALIZATION_ERROR",
                format!("Failed to parse envelope: {e}"),
            );
            if let Err(e) = write_response_with_format(writer, &response, format).await {
                error!(conn_id, error = %e, "Failed to send error response");
                return RequestResult::Break;
            }
            ctx.stats.errors.fetch_add(1, Ordering::Relaxed);
            return RequestResult::Continue;
        }
    };

    trace!(
        conn_id,
        correlation_id = %envelope.correlation_id,
        target = %envelope.target,
        message_type = %envelope.message_type,
        expects_stream = envelope.expects_stream,
        "Received request"
    );

    // Handle streaming requests differently
    if envelope.expects_stream {
        let result = process_stream_request(&envelope, format, ctx, writer).await;
        ctx.stats.decrement_in_flight();
        return result;
    }

    // Process the envelope and send response
    let response = process_envelope(
        &envelope,
        &ctx.type_registry,
        &ctx.actor_registry,
        &ctx.stats,
    )
    .await;

    // Done processing - decrement in-flight counter
    ctx.stats.decrement_in_flight();

    if let Err(e) = write_response_with_format(writer, &response, format).await {
        error!(conn_id, error = %e, "Failed to send response");
        return RequestResult::Break;
    }

    RequestResult::Continue
}

/// Handle a single client connection.
async fn handle_connection(stream: UnixStream, conn_id: usize, ctx: ConnectionContext) {
    let (mut reader, writer) = stream.into_split();
    let max_message_size = ctx.config.limits.max_message_size;
    let push_buffer_size = ctx.config.limits.push_buffer_size;
    let read_timeout = ctx.config.read_timeout();
    let mut rate_limiter = RateLimiter::new(&ctx.config.rate_limit);

    // Create push notification channel and register connection for subscriptions
    let (push_sender, push_receiver) = create_push_channel(conn_id, push_buffer_size);
    ctx.subscription_manager
        .register_connection(conn_id, push_sender);

    debug!(
        conn_id,
        rate_limiting_enabled = rate_limiter.is_enabled(),
        available_tokens = rate_limiter.available_tokens(),
        push_buffer_size,
        "Connection handler started with subscription support"
    );

    // Spawn push notification forwarder task
    // The writer half is shared between the main loop and push forwarder via Arc<Mutex>
    let writer = Arc::new(tokio::sync::Mutex::new(writer));
    let push_writer = writer.clone();
    let push_cancel = ctx.cancel_token.clone();
    let push_stats = ctx.stats.clone();

    let push_task = tokio::spawn(async move {
        run_push_forwarder(conn_id, push_receiver, push_writer, push_cancel, push_stats).await;
    });

    loop {
        tokio::select! {
            biased;

            () = ctx.cancel_token.cancelled() => {
                trace!(conn_id, "Received shutdown signal");
                break;
            }

            frame_result = tokio::time::timeout(read_timeout, read_frame(&mut reader, max_message_size)) => {
                // Handle read timeout
                let frame_result = match frame_result {
                    Ok(result) => result,
                    Err(_elapsed) => {
                        debug!(conn_id, timeout_ms = read_timeout.as_millis(), "Connection read timeout");
                        ctx.stats.errors.fetch_add(1, Ordering::Relaxed);
                        break;
                    }
                };

                match frame_result {
                    Ok((msg_type, format, payload)) => {
                        if is_heartbeat(msg_type) {
                            trace!(conn_id, "Received heartbeat");
                            if let Err(e) = write_heartbeat(&mut *writer.lock().await).await {
                                error!(conn_id, error = %e, "Failed to send heartbeat response");
                                break;
                            }
                            continue;
                        }

                        // Handle subscription requests
                        if is_subscribe(msg_type) {
                            handle_subscribe_frame(conn_id, &payload, format, &ctx, &mut *writer.lock().await).await;
                            continue;
                        }

                        if is_unsubscribe(msg_type) {
                            handle_unsubscribe_frame(conn_id, &payload, format, &ctx, &mut *writer.lock().await).await;
                            continue;
                        }

                        // Handle discovery requests
                        if is_discover(msg_type) {
                            handle_discover_frame(conn_id, &payload, format, &ctx, &mut *writer.lock().await).await;
                            continue;
                        }

                        if msg_type != MSG_TYPE_REQUEST {
                            warn!(conn_id, msg_type, "Unexpected message type");
                            continue;
                        }

                        if matches!(
                            handle_request_frame(
                                conn_id, &payload, format, &mut rate_limiter, &ctx, &mut *writer.lock().await,
                            ).await,
                            RequestResult::Break
                        ) {
                            break;
                        }
                    }
                    Err(IpcError::ConnectionClosed) => {
                        debug!(conn_id, "Connection closed by client");
                        break;
                    }
                    Err(e) => {
                        error!(conn_id, error = %e, "Connection error");
                        ctx.stats.errors.fetch_add(1, Ordering::Relaxed);
                        break;
                    }
                }
            }
        }
    }

    // Unregister connection from subscription manager (cleans up all subscriptions)
    ctx.subscription_manager.unregister_connection(conn_id);

    // Wait for push forwarder task to finish
    push_task.abort();
    let _ = push_task.await;

    ctx.stats.connections_active.fetch_sub(1, Ordering::Relaxed);
    debug!(conn_id, "Connection handler finished");
}

/// Run the push notification forwarder task.
///
/// This task receives push notifications from the subscription manager and
/// writes them to the client connection. It runs concurrently with the main
/// connection loop.
async fn run_push_forwarder(
    conn_id: usize,
    mut receiver: PushReceiver,
    writer: Arc<tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>>,
    cancel_token: CancellationToken,
    stats: Arc<IpcListenerStats>,
) {
    trace!(conn_id, "Push forwarder started");

    loop {
        tokio::select! {
            biased;

            () = cancel_token.cancelled() => {
                trace!(conn_id, "Push forwarder received shutdown signal");
                break;
            }

            notification = receiver.receiver.recv() => {
                if let Some(push) = notification {
                    match write_push(&mut *writer.lock().await, &push).await {
                        Ok(()) => {
                            stats.push_notifications_sent.fetch_add(1, Ordering::Relaxed);
                            trace!(
                                conn_id,
                                message_type = %push.message_type,
                                notification_id = %push.notification_id,
                                "Sent push notification"
                            );
                        }
                        Err(e) => {
                            error!(conn_id, error = %e, "Failed to send push notification");
                            // Connection is broken, exit the forwarder
                            break;
                        }
                    }
                } else {
                    // Channel closed, connection is shutting down
                    trace!(conn_id, "Push channel closed");
                    break;
                }
            }
        }
    }

    trace!(conn_id, "Push forwarder finished");
}

/// Handle a subscribe request frame.
async fn handle_subscribe_frame(
    conn_id: usize,
    payload: &[u8],
    format: Format,
    ctx: &ConnectionContext,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) {
    let request: IpcSubscribeRequest = match format.deserialize(payload) {
        Ok(req) => req,
        Err(e) => {
            error!(conn_id, error = %e, "Failed to parse subscribe request");
            let response = IpcSubscriptionResponse::error("unknown", format!("Parse error: {e}"));
            let _ = write_subscription_response_with_format(writer, &response, format).await;
            ctx.stats.errors.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };

    trace!(
        conn_id,
        correlation_id = %request.correlation_id,
        message_types = ?request.message_types,
        "Received subscribe request"
    );

    // Subscribe to the requested message types
    let subscribed_types = ctx
        .subscription_manager
        .subscribe(conn_id, &request.message_types);

    ctx.stats
        .subscriptions_processed
        .fetch_add(1, Ordering::Relaxed);

    let response = IpcSubscriptionResponse::success(&request.correlation_id, subscribed_types);

    if let Err(e) = write_subscription_response_with_format(writer, &response, format).await {
        error!(conn_id, error = %e, "Failed to send subscribe response");
    }
}

/// Handle an unsubscribe request frame.
async fn handle_unsubscribe_frame(
    conn_id: usize,
    payload: &[u8],
    format: Format,
    ctx: &ConnectionContext,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) {
    let request: IpcUnsubscribeRequest = match format.deserialize(payload) {
        Ok(req) => req,
        Err(e) => {
            error!(conn_id, error = %e, "Failed to parse unsubscribe request");
            let response = IpcSubscriptionResponse::error("unknown", format!("Parse error: {e}"));
            let _ = write_subscription_response_with_format(writer, &response, format).await;
            ctx.stats.errors.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };

    trace!(
        conn_id,
        correlation_id = %request.correlation_id,
        message_types = ?request.message_types,
        "Received unsubscribe request"
    );

    // Unsubscribe from the requested message types
    let remaining_types = ctx
        .subscription_manager
        .unsubscribe(conn_id, &request.message_types);

    ctx.stats
        .subscriptions_processed
        .fetch_add(1, Ordering::Relaxed);

    let response = IpcSubscriptionResponse::success(&request.correlation_id, remaining_types);

    if let Err(e) = write_subscription_response_with_format(writer, &response, format).await {
        error!(conn_id, error = %e, "Failed to send unsubscribe response");
    }
}

/// Handle a discovery request frame.
async fn handle_discover_frame(
    conn_id: usize,
    payload: &[u8],
    format: Format,
    ctx: &ConnectionContext,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) {
    let request: IpcDiscoverRequest = match format.deserialize(payload) {
        Ok(req) => req,
        Err(e) => {
            error!(conn_id, error = %e, "Failed to parse discover request");
            let response = IpcDiscoverResponse::error("unknown", format!("Parse error: {e}"));
            let _ = write_discovery_response_with_format(writer, &response, format).await;
            ctx.stats.errors.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };

    trace!(
        conn_id,
        correlation_id = %request.correlation_id,
        include_actors = request.include_actors,
        include_message_types = request.include_message_types,
        "Received discover request"
    );

    // Gather actor information if requested
    let actors = if request.include_actors {
        let actor_list: Vec<ActorInfo> = ctx
            .actor_registry
            .iter()
            .map(|entry| ActorInfo {
                name: entry.key().clone(),
                ern: entry.value().id().to_string(),
            })
            .collect();
        Some(actor_list)
    } else {
        None
    };

    // Gather message type information if requested
    let message_types = if request.include_message_types {
        Some(ctx.type_registry.type_names().collect())
    } else {
        None
    };

    let response = IpcDiscoverResponse::success(&request.correlation_id, actors, message_types);

    if let Err(e) = write_discovery_response_with_format(writer, &response, format).await {
        error!(conn_id, error = %e, "Failed to send discover response");
    }

    debug!(
        conn_id,
        correlation_id = %request.correlation_id,
        "Sent discover response"
    );
}

/// Channel capacity for the IPC response proxy channel.
const IPC_RESPONSE_CHANNEL_CAPACITY: usize = 1;

/// Creates a temporary `MessageAddress` for receiving IPC responses.
///
/// This creates a short-lived MPSC channel that acts as a "reply-to" address
/// for IPC request-response patterns. When an actor calls `reply_envelope.send()`,
/// the response is sent to this channel.
fn create_ipc_response_proxy(correlation_id: &str) -> (mpsc::Receiver<Envelope>, MessageAddress) {
    let (sender, receiver) = mpsc::channel::<Envelope>(IPC_RESPONSE_CHANNEL_CAPACITY);

    // Create a unique ERN for this IPC response proxy
    let ern = Ern::with_root(format!("ipc_proxy_{correlation_id}"))
        .expect("Failed to create ERN for IPC response proxy");

    let address = MessageAddress::new(sender, ern);

    (receiver, address)
}

/// Serializes a response message to JSON using the type registry.
///
/// If the message type is registered in the IPC type registry, it will be
/// properly serialized to JSON. Otherwise, falls back to a debug representation.
///
/// # Arguments
///
/// * `message` - The message to serialize
/// * `type_registry` - The IPC type registry containing registered serializers
fn serialize_response(
    message: &dyn ActonMessage,
    type_registry: &IpcTypeRegistry,
) -> serde_json::Value {
    let type_id = message.type_id();

    // Try to serialize using the registered serializer
    match type_registry.serialize_by_type_id(&type_id, message) {
        Ok(value) => value,
        Err(err) => {
            // Fall back to debug representation for unregistered types
            trace!(
                type_name = std::any::type_name_of_val(message),
                error = %err,
                "Response type not registered for IPC serialization, using debug representation"
            );
            serde_json::json!({
                "_ipc_fallback": true,
                "type": std::any::type_name_of_val(message),
                "debug": format!("{:?}", message),
            })
        }
    }
}

/// Process an IPC envelope and route to the target actor.
///
/// If `expects_reply` is `true`, this function creates a temporary channel
/// to receive the actor's response and waits for it (with timeout).
async fn process_envelope(
    envelope: &IpcEnvelope,
    type_registry: &Arc<IpcTypeRegistry>,
    actor_registry: &Arc<DashMap<String, ActorHandle>>,
    stats: &Arc<IpcListenerStats>,
) -> IpcResponse {
    let correlation_id = &envelope.correlation_id;

    // Look up the target actor
    let Some(entry) = actor_registry.get(&envelope.target) else {
        let err = IpcError::ActorNotFound(envelope.target.clone());
        stats.errors.fetch_add(1, Ordering::Relaxed);
        return IpcResponse::error(correlation_id, &err);
    };
    let actor_handle = entry.value().clone();

    // Deserialize the message
    let message = match type_registry.deserialize_value(&envelope.message_type, &envelope.payload) {
        Ok(msg) => msg,
        Err(e) => {
            stats.errors.fetch_add(1, Ordering::Relaxed);
            return IpcResponse::error(correlation_id, &e);
        }
    };

    // Handle request-response vs fire-and-forget
    if envelope.expects_reply {
        process_request_response(
            correlation_id,
            &actor_handle,
            message,
            envelope.response_timeout(),
            stats,
            type_registry,
        )
        .await
    } else {
        process_fire_and_forget(correlation_id, &actor_handle, message, stats)
    }
}

/// Process a fire-and-forget message (no response expected).
fn process_fire_and_forget(
    correlation_id: &str,
    actor_handle: &ActorHandle,
    message: Box<dyn ActonMessage + Send + Sync>,
    stats: &Arc<IpcListenerStats>,
) -> IpcResponse {
    // Send the message to the actor using backpressure-aware method
    match actor_handle.try_send_boxed(message) {
        Ok(()) => {
            stats.messages_routed.fetch_add(1, Ordering::Relaxed);
            // Return a simple acknowledgment for fire-and-forget messages
            IpcResponse::success(
                correlation_id,
                Some(serde_json::json!({ "status": "delivered" })),
            )
        }
        Err(IpcError::TargetBusy) => {
            stats
                .backpressure_rejections
                .fetch_add(1, Ordering::Relaxed);
            IpcResponse::error(correlation_id, &IpcError::TargetBusy)
        }
        Err(e) => {
            stats.errors.fetch_add(1, Ordering::Relaxed);
            IpcResponse::error(correlation_id, &e)
        }
    }
}

/// Send a stream error frame and return the appropriate result.
async fn send_stream_error(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    frame: &IpcStreamFrame,
    format: Format,
) -> RequestResult {
    if let Err(e) = write_stream_frame_with_format(writer, frame, format).await {
        error!(error = %e, "Failed to send stream error frame");
        return RequestResult::Break;
    }
    RequestResult::Continue
}

/// Run the stream response loop until completion or timeout.
async fn run_stream_loop(
    correlation_id: &str,
    timeout: Duration,
    receiver: &mut mpsc::Receiver<Envelope>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    format: Format,
    type_registry: &IpcTypeRegistry,
    stats: &IpcListenerStats,
) -> RequestResult {
    let mut sequence: u32 = 0;
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            let frame = IpcStreamFrame::error_with_code(
                correlation_id,
                sequence,
                "TIMEOUT",
                format!("Stream timed out after {} ms", timeout.as_millis()),
            );
            stats.errors.fetch_add(1, Ordering::Relaxed);
            return send_stream_error(writer, &frame, format).await;
        }

        match tokio::time::timeout(remaining, receiver.recv()).await {
            Ok(Some(response_envelope)) => {
                let payload = serialize_response(response_envelope.message.as_ref(), type_registry);
                let frame = IpcStreamFrame::data(correlation_id, sequence, payload);
                trace!(correlation_id, sequence, "Sending stream frame");
                if let Err(e) = write_stream_frame_with_format(writer, &frame, format).await {
                    error!(error = %e, "Failed to send stream frame");
                    return RequestResult::Break;
                }
                sequence += 1;
            }
            Ok(None) => {
                let frame = IpcStreamFrame::final_frame(correlation_id, sequence, None);
                debug!(correlation_id, total_frames = sequence, "Stream completed");
                if let Err(e) = write_stream_frame_with_format(writer, &frame, format).await {
                    error!(error = %e, "Failed to send final stream frame");
                    return RequestResult::Break;
                }
                break;
            }
            Err(_) => {
                let frame = IpcStreamFrame::error_with_code(
                    correlation_id,
                    sequence,
                    "TIMEOUT",
                    format!("Stream timed out after {} ms", timeout.as_millis()),
                );
                stats.errors.fetch_add(1, Ordering::Relaxed);
                return send_stream_error(writer, &frame, format).await;
            }
        }
    }
    RequestResult::Continue
}

/// Process a streaming request (multiple responses from actor).
///
/// The actor can send multiple responses through the reply-to channel.
/// Each response is serialized and sent as a stream frame. The stream
/// ends when the channel is closed or the timeout expires.
async fn process_stream_request(
    envelope: &IpcEnvelope,
    format: Format,
    ctx: &ConnectionContext,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> RequestResult {
    let correlation_id = &envelope.correlation_id;
    let timeout = envelope.response_timeout();

    // Look up the target actor
    let Some(entry) = ctx.actor_registry.get(&envelope.target) else {
        let frame = IpcStreamFrame::error(
            correlation_id,
            0,
            format!("Actor not found: {}", envelope.target),
        );
        ctx.stats.errors.fetch_add(1, Ordering::Relaxed);
        return send_stream_error(writer, &frame, format).await;
    };
    let actor_handle = entry.value().clone();
    drop(entry);

    // Deserialize the message
    let message = match ctx
        .type_registry
        .deserialize_value(&envelope.message_type, &envelope.payload)
    {
        Ok(msg) => msg,
        Err(e) => {
            let frame = IpcStreamFrame::error_with_code(
                correlation_id,
                0,
                "SERIALIZATION_ERROR",
                e.to_string(),
            );
            ctx.stats.errors.fetch_add(1, Ordering::Relaxed);
            return send_stream_error(writer, &frame, format).await;
        }
    };

    // Create response channel and send message
    let (mut response_receiver, reply_to_address) = create_ipc_response_proxy(correlation_id);

    if let Err(e) = actor_handle.try_send_boxed_with_reply_to(message, reply_to_address) {
        let frame = if matches!(e, IpcError::TargetBusy) {
            ctx.stats
                .backpressure_rejections
                .fetch_add(1, Ordering::Relaxed);
            IpcStreamFrame::error_with_code(correlation_id, 0, "TARGET_BUSY", "Actor inbox is full")
        } else {
            ctx.stats.errors.fetch_add(1, Ordering::Relaxed);
            IpcStreamFrame::error(correlation_id, 0, e.to_string())
        };
        return send_stream_error(writer, &frame, format).await;
    }

    ctx.stats.messages_routed.fetch_add(1, Ordering::Relaxed);

    run_stream_loop(
        correlation_id,
        timeout,
        &mut response_receiver,
        writer,
        format,
        &ctx.type_registry,
        &ctx.stats,
    )
    .await
}

/// Process a request-response message (wait for actor's reply).
async fn process_request_response(
    correlation_id: &str,
    actor_handle: &ActorHandle,
    message: Box<dyn ActonMessage + Send + Sync>,
    timeout: Duration,
    stats: &Arc<IpcListenerStats>,
    type_registry: &Arc<IpcTypeRegistry>,
) -> IpcResponse {
    // Create a temporary channel to receive the response
    let (mut response_receiver, reply_to_address) = create_ipc_response_proxy(correlation_id);

    // Send the message with our proxy as the reply-to address using backpressure-aware method
    match actor_handle.try_send_boxed_with_reply_to(message, reply_to_address) {
        Ok(()) => {
            stats.messages_routed.fetch_add(1, Ordering::Relaxed);
        }
        Err(IpcError::TargetBusy) => {
            stats
                .backpressure_rejections
                .fetch_add(1, Ordering::Relaxed);
            return IpcResponse::error(correlation_id, &IpcError::TargetBusy);
        }
        Err(e) => {
            stats.errors.fetch_add(1, Ordering::Relaxed);
            return IpcResponse::error(correlation_id, &e);
        }
    }

    // Wait for the response with timeout
    match tokio::time::timeout(timeout, response_receiver.recv()).await {
        Ok(Some(response_envelope)) => {
            // Serialize the response message using the type registry
            let payload = serialize_response(response_envelope.message.as_ref(), type_registry);

            trace!(
                "Received response for correlation_id={}: {:?}",
                correlation_id,
                payload
            );

            IpcResponse::success(correlation_id, Some(payload))
        }
        Ok(None) => {
            // Channel closed without receiving a response
            let err = IpcError::IoError(
                "Response channel closed without receiving a response".to_string(),
            );
            stats.errors.fetch_add(1, Ordering::Relaxed);
            IpcResponse::error(correlation_id, &err)
        }
        Err(_) => {
            // Timeout waiting for response
            stats.errors.fetch_add(1, Ordering::Relaxed);
            IpcResponse::error_with_message(
                correlation_id,
                "TIMEOUT",
                format!(
                    "Request timed out after {} ms waiting for response",
                    timeout.as_millis()
                ),
            )
        }
    }
}

/// Check if a socket path exists and is accessible.
#[must_use]
pub fn socket_exists(path: &Path) -> bool {
    path.exists()
}

/// Attempt to connect to an existing socket to check if it's alive.
pub async fn socket_is_alive(path: &Path) -> bool {
    UnixStream::connect(path).await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stats_default() {
        let stats = IpcListenerStats::new();
        assert_eq!(stats.connections_accepted(), 0);
        assert_eq!(stats.connections_active(), 0);
        assert_eq!(stats.messages_received(), 0);
        assert_eq!(stats.messages_routed(), 0);
        assert_eq!(stats.errors(), 0);
        assert_eq!(stats.rate_limited(), 0);
        assert_eq!(stats.backpressure_rejections(), 0);
        assert_eq!(stats.shutdown_rejections(), 0);
        assert_eq!(stats.in_flight_requests(), 0);
    }

    #[test]
    fn test_stats_increment() {
        let stats = IpcListenerStats::new();
        stats.connections_accepted.fetch_add(1, Ordering::Relaxed);
        stats.messages_received.fetch_add(5, Ordering::Relaxed);

        assert_eq!(stats.connections_accepted(), 1);
        assert_eq!(stats.messages_received(), 5);
    }

    #[test]
    fn test_stats_in_flight_tracking() {
        let stats = IpcListenerStats::new();

        assert_eq!(stats.in_flight_requests(), 0);

        stats.increment_in_flight();
        assert_eq!(stats.in_flight_requests(), 1);

        stats.increment_in_flight();
        assert_eq!(stats.in_flight_requests(), 2);

        stats.decrement_in_flight();
        assert_eq!(stats.in_flight_requests(), 1);

        stats.decrement_in_flight();
        assert_eq!(stats.in_flight_requests(), 0);
    }

    #[test]
    fn test_shutdown_state() {
        let state = ShutdownState::new();

        assert!(!state.is_draining());

        state.start_draining();
        assert!(state.is_draining());

        // Should remain draining (no way to reset)
        assert!(state.is_draining());
    }

    #[test]
    fn test_listener_handle_cancel() {
        let stats = Arc::new(IpcListenerStats::new());
        let cancel_token = CancellationToken::new();
        let shutdown_state = Arc::new(ShutdownState::new());
        let subscription_manager = Arc::new(SubscriptionManager::new());

        let handle = IpcListenerHandle {
            stats,
            cancel_token,
            shutdown_state,
            drain_timeout: Duration::from_secs(5),
            subscription_manager,
        };

        assert!(!handle.is_cancelled());
        assert!(!handle.is_draining());
        handle.stop();
        assert!(handle.is_cancelled());
    }

    #[test]
    fn test_listener_handle_draining() {
        let stats = Arc::new(IpcListenerStats::new());
        let cancel_token = CancellationToken::new();
        let shutdown_state = Arc::new(ShutdownState::new());
        let subscription_manager = Arc::new(SubscriptionManager::new());

        let handle = IpcListenerHandle {
            stats,
            cancel_token,
            shutdown_state: shutdown_state.clone(),
            drain_timeout: Duration::from_secs(5),
            subscription_manager,
        };

        assert!(!handle.is_draining());

        // Manually start draining
        shutdown_state.start_draining();
        assert!(handle.is_draining());
    }

    #[test]
    fn test_shutdown_result() {
        let result = ShutdownResult {
            in_flight_at_start: 10,
            drained_gracefully: true,
            forced_closed: 0,
        };

        assert_eq!(result.in_flight_at_start, 10);
        assert!(result.drained_gracefully);
        assert_eq!(result.forced_closed, 0);
    }

    #[tokio::test]
    async fn test_graceful_shutdown_no_in_flight() {
        let stats = Arc::new(IpcListenerStats::new());
        let cancel_token = CancellationToken::new();
        let shutdown_state = Arc::new(ShutdownState::new());
        let subscription_manager = Arc::new(SubscriptionManager::new());

        let handle = IpcListenerHandle {
            stats,
            cancel_token: cancel_token.clone(),
            shutdown_state,
            drain_timeout: Duration::from_millis(100),
            subscription_manager,
        };

        // No in-flight requests, should drain immediately
        let result = handle.shutdown_gracefully().await;

        assert_eq!(result.in_flight_at_start, 0);
        assert!(result.drained_gracefully);
        assert_eq!(result.forced_closed, 0);
        assert!(cancel_token.is_cancelled());
    }

    #[test]
    fn test_socket_exists() {
        // Non-existent path should return false
        assert!(!socket_exists(Path::new("/nonexistent/path/socket.sock")));
    }
}
