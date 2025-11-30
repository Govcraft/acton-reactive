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
//! external processes and routes messages to agents.

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
    is_heartbeat, read_frame, write_heartbeat, write_response, MSG_TYPE_REQUEST,
};
use super::rate_limiter::RateLimiter;
use super::registry::IpcTypeRegistry;
use super::types::{IpcEnvelope, IpcError, IpcResponse};
use crate::common::{AgentHandle, Envelope};
use crate::message::MessageAddress;
use crate::traits::{ActonMessage, AgentHandleInterface};

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
    /// Total requests rejected due to agent backpressure.
    pub backpressure_rejections: AtomicUsize,
    /// Total requests rejected due to shutdown.
    pub shutdown_rejections: AtomicUsize,
    /// Current number of in-flight requests being processed.
    pub in_flight_requests: AtomicUsize,
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

    /// Get the number of requests rejected due to agent backpressure.
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
    /// Registry mapping logical names to agent handles.
    agent_registry: Arc<DashMap<String, AgentHandle>>,
    /// Token for shutdown signaling.
    cancel_token: CancellationToken,
    /// Semaphore for limiting concurrent connections.
    connection_semaphore: Arc<Semaphore>,
    /// Listener statistics.
    stats: Arc<IpcListenerStats>,
    /// Shutdown coordination state.
    shutdown_state: Arc<ShutdownState>,
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
        self.shutdown_gracefully_with_timeout(self.drain_timeout).await
    }

    /// Gracefully shut down with a custom timeout.
    ///
    /// See [`shutdown_gracefully`](Self::shutdown_gracefully) for details.
    pub async fn shutdown_gracefully_with_timeout(&self, drain_timeout: Duration) -> ShutdownResult {
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
/// * `agent_registry` - Registry mapping logical names to agent handles.
/// * `cancel_token` - Token for graceful shutdown.
///
/// # Returns
///
/// An `IpcListenerHandle` for managing the listener, or an error if the
/// listener could not be started.
pub async fn run(
    config: IpcConfig,
    type_registry: Arc<IpcTypeRegistry>,
    agent_registry: Arc<DashMap<String, AgentHandle>>,
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

    // Create connection context for the accept loop
    let context = ConnectionContext {
        config,
        type_registry,
        agent_registry,
        cancel_token: cancel_token.clone(),
        connection_semaphore,
        stats: stats.clone(),
        shutdown_state: shutdown_state.clone(),
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
    rate_limiter: &mut RateLimiter,
    ctx: &ConnectionContext,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> RequestResult {
    ctx.stats.messages_received.fetch_add(1, Ordering::Relaxed);

    // Check if we're draining - reject new requests during shutdown
    if ctx.shutdown_state.is_draining() {
        ctx.stats.shutdown_rejections.fetch_add(1, Ordering::Relaxed);

        let correlation_id = serde_json::from_slice::<IpcEnvelope>(payload)
            .map_or_else(|_| "unknown".to_string(), |e| e.correlation_id);

        let response = IpcResponse::error(&correlation_id, &IpcError::ShuttingDown);

        trace!(conn_id, "Rejecting request during shutdown drain");

        if let Err(e) = write_response(writer, &response).await {
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

        let correlation_id = serde_json::from_slice::<IpcEnvelope>(payload)
            .map_or_else(|_| "unknown".to_string(), |e| e.correlation_id);

        let err = IpcError::RateLimited { retry_after_ms };
        let response = IpcResponse::error(&correlation_id, &err);

        trace!(conn_id, retry_after_ms, "Rate limited");

        if let Err(e) = write_response(writer, &response).await {
            error!(conn_id, error = %e, "Failed to send rate limit response");
            return RequestResult::Break;
        }
        return RequestResult::Continue;
    }

    // Track in-flight request for graceful shutdown
    ctx.stats.increment_in_flight();

    // Parse envelope
    let envelope: IpcEnvelope = match serde_json::from_slice(payload) {
        Ok(env) => env,
        Err(e) => {
            ctx.stats.decrement_in_flight();
            let response = IpcResponse::error_with_message(
                "unknown",
                "SERIALIZATION_ERROR",
                format!("Failed to parse envelope: {e}"),
            );
            if let Err(e) = write_response(writer, &response).await {
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
        "Received request"
    );

    // Process the envelope and send response
    let response =
        process_envelope(&envelope, &ctx.type_registry, &ctx.agent_registry, &ctx.stats).await;

    // Done processing - decrement in-flight counter
    ctx.stats.decrement_in_flight();

    if let Err(e) = write_response(writer, &response).await {
        error!(conn_id, error = %e, "Failed to send response");
        return RequestResult::Break;
    }

    RequestResult::Continue
}

/// Handle a single client connection.
async fn handle_connection(stream: UnixStream, conn_id: usize, ctx: ConnectionContext) {
    let (mut reader, mut writer) = stream.into_split();
    let max_message_size = ctx.config.limits.max_message_size;
    let mut rate_limiter = RateLimiter::new(&ctx.config.rate_limit);

    debug!(
        conn_id,
        rate_limiting_enabled = rate_limiter.is_enabled(),
        available_tokens = rate_limiter.available_tokens(),
        "Connection handler started"
    );

    loop {
        tokio::select! {
            biased;

            () = ctx.cancel_token.cancelled() => {
                trace!(conn_id, "Received shutdown signal");
                break;
            }

            frame_result = read_frame(&mut reader, max_message_size) => {
                match frame_result {
                    Ok((msg_type, payload)) => {
                        if is_heartbeat(msg_type) {
                            trace!(conn_id, "Received heartbeat");
                            if let Err(e) = write_heartbeat(&mut writer).await {
                                error!(conn_id, error = %e, "Failed to send heartbeat response");
                                break;
                            }
                            continue;
                        }

                        if msg_type != MSG_TYPE_REQUEST {
                            warn!(conn_id, msg_type, "Unexpected message type");
                            continue;
                        }

                        if matches!(
                            handle_request_frame(
                                conn_id, &payload, &mut rate_limiter, &ctx, &mut writer,
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

    ctx.stats.connections_active.fetch_sub(1, Ordering::Relaxed);
    debug!(conn_id, "Connection handler finished");
}

/// Channel capacity for the IPC response proxy channel.
const IPC_RESPONSE_CHANNEL_CAPACITY: usize = 1;

/// Creates a temporary `MessageAddress` for receiving IPC responses.
///
/// This creates a short-lived MPSC channel that acts as a "reply-to" address
/// for IPC request-response patterns. When an agent calls `reply_envelope.send()`,
/// the response is sent to this channel.
fn create_ipc_response_proxy(correlation_id: &str) -> (mpsc::Receiver<Envelope>, MessageAddress) {
    let (sender, receiver) = mpsc::channel::<Envelope>(IPC_RESPONSE_CHANNEL_CAPACITY);

    // Create a unique ERN for this IPC response proxy
    let ern = Ern::with_root(format!("ipc_proxy_{correlation_id}"))
        .expect("Failed to create ERN for IPC response proxy");

    let address = MessageAddress::new(sender, ern);

    (receiver, address)
}

/// Serializes a message to JSON for IPC response transmission.
///
/// Since `ActonMessage` doesn't require `Serialize`, we use the `Debug` representation
/// as a fallback. A more robust solution would be to add a serialize method to
/// `ActonMessage` or use the `erased-serde` crate.
///
/// For Phase 3, this provides basic response serialization that captures the message
/// type and its debug representation.
fn serialize_response(message: &dyn ActonMessage) -> serde_json::Value {
    // Since we can't directly check if a type implements Serialize at runtime,
    // we return the Debug representation as a fallback.
    // In the future, this could be extended with a proper erased serialization trait.
    serde_json::json!({
        "type": std::any::type_name_of_val(message),
        "debug": format!("{:?}", message),
    })
}

/// Process an IPC envelope and route to the target agent.
///
/// If `expects_reply` is `true`, this function creates a temporary channel
/// to receive the agent's response and waits for it (with timeout).
async fn process_envelope(
    envelope: &IpcEnvelope,
    type_registry: &Arc<IpcTypeRegistry>,
    agent_registry: &Arc<DashMap<String, AgentHandle>>,
    stats: &Arc<IpcListenerStats>,
) -> IpcResponse {
    let correlation_id = &envelope.correlation_id;

    // Look up the target agent
    let Some(entry) = agent_registry.get(&envelope.target) else {
        let err = IpcError::AgentNotFound(envelope.target.clone());
        stats.errors.fetch_add(1, Ordering::Relaxed);
        return IpcResponse::error(correlation_id, &err);
    };
    let agent_handle = entry.value().clone();

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
            &agent_handle,
            message,
            envelope.response_timeout(),
            stats,
        )
        .await
    } else {
        process_fire_and_forget(correlation_id, &agent_handle, message, stats)
    }
}

/// Process a fire-and-forget message (no response expected).
fn process_fire_and_forget(
    correlation_id: &str,
    agent_handle: &AgentHandle,
    message: Box<dyn ActonMessage + Send + Sync>,
    stats: &Arc<IpcListenerStats>,
) -> IpcResponse {
    // Send the message to the agent using backpressure-aware method
    match agent_handle.try_send_boxed(message) {
        Ok(()) => {
            stats.messages_routed.fetch_add(1, Ordering::Relaxed);
            // Return a simple acknowledgment for fire-and-forget messages
            IpcResponse::success(
                correlation_id,
                Some(serde_json::json!({ "status": "delivered" })),
            )
        }
        Err(IpcError::TargetBusy) => {
            stats.backpressure_rejections.fetch_add(1, Ordering::Relaxed);
            IpcResponse::error(correlation_id, &IpcError::TargetBusy)
        }
        Err(e) => {
            stats.errors.fetch_add(1, Ordering::Relaxed);
            IpcResponse::error(correlation_id, &e)
        }
    }
}

/// Process a request-response message (wait for agent's reply).
async fn process_request_response(
    correlation_id: &str,
    agent_handle: &AgentHandle,
    message: Box<dyn ActonMessage + Send + Sync>,
    timeout: Duration,
    stats: &Arc<IpcListenerStats>,
) -> IpcResponse {
    // Create a temporary channel to receive the response
    let (mut response_receiver, reply_to_address) = create_ipc_response_proxy(correlation_id);

    // Send the message with our proxy as the reply-to address using backpressure-aware method
    match agent_handle.try_send_boxed_with_reply_to(message, reply_to_address) {
        Ok(()) => {
            stats.messages_routed.fetch_add(1, Ordering::Relaxed);
        }
        Err(IpcError::TargetBusy) => {
            stats.backpressure_rejections.fetch_add(1, Ordering::Relaxed);
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
            // Serialize the response message
            let payload = serialize_response(response_envelope.message.as_ref());

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

        let handle = IpcListenerHandle {
            stats,
            cancel_token,
            shutdown_state,
            drain_timeout: Duration::from_secs(5),
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

        let handle = IpcListenerHandle {
            stats,
            cancel_token,
            shutdown_state: shutdown_state.clone(),
            drain_timeout: Duration::from_secs(5),
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

        let handle = IpcListenerHandle {
            stats,
            cancel_token: cancel_token.clone(),
            shutdown_state,
            drain_timeout: Duration::from_millis(100),
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
