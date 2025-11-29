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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use super::config::IpcConfig;
use super::protocol::{
    is_heartbeat, read_frame, write_heartbeat, write_response, MSG_TYPE_REQUEST,
};
use super::registry::IpcTypeRegistry;
use super::types::{IpcEnvelope, IpcError, IpcResponse};
use crate::common::AgentHandle;
use crate::traits::AgentHandleInterface;

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
}

/// IPC listener handle for managing the listener lifecycle.
pub struct IpcListenerHandle {
    /// Statistics for the listener.
    pub stats: Arc<IpcListenerStats>,
    /// Cancellation token for graceful shutdown.
    cancel_token: CancellationToken,
}

impl IpcListenerHandle {
    /// Request the listener to stop.
    pub fn stop(&self) {
        self.cancel_token.cancel();
    }

    /// Check if the listener has been cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
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

    // Clone values for the spawned task
    let stats_clone = stats.clone();
    let cancel_token_clone = cancel_token.clone();
    let socket_path_clone = socket_path.clone();

    // Spawn the accept loop
    tokio::spawn(async move {
        accept_loop(
            listener,
            config,
            type_registry,
            agent_registry,
            cancel_token_clone,
            connection_semaphore,
            stats_clone,
        )
        .await;

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
    })
}

/// Main accept loop for the listener.
async fn accept_loop(
    listener: UnixListener,
    config: IpcConfig,
    type_registry: Arc<IpcTypeRegistry>,
    agent_registry: Arc<DashMap<String, AgentHandle>>,
    cancel_token: CancellationToken,
    connection_semaphore: Arc<Semaphore>,
    stats: Arc<IpcListenerStats>,
) {
    loop {
        tokio::select! {
            biased;

            () = cancel_token.cancelled() => {
                info!("IPC listener received shutdown signal");
                break;
            }

            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, _addr)) => {
                        // Try to acquire a connection permit
                        let Ok(permit) = connection_semaphore.clone().try_acquire_owned() else {
                            warn!("Maximum concurrent connections reached, rejecting connection");
                            stats.errors.fetch_add(1, Ordering::Relaxed);
                            continue;
                        };

                        stats.connections_accepted.fetch_add(1, Ordering::Relaxed);
                        stats.connections_active.fetch_add(1, Ordering::Relaxed);

                        let conn_id = stats.connections_accepted.load(Ordering::Relaxed);
                        trace!("Accepted connection #{}", conn_id);

                        // Spawn connection handler
                        let config_clone = config.clone();
                        let type_registry_clone = type_registry.clone();
                        let agent_registry_clone = agent_registry.clone();
                        let cancel_token_clone = cancel_token.clone();
                        let stats_clone = stats.clone();

                        tokio::spawn(async move {
                            handle_connection(
                                stream,
                                conn_id,
                                config_clone,
                                type_registry_clone,
                                agent_registry_clone,
                                cancel_token_clone,
                                stats_clone,
                            )
                            .await;

                            // Release the permit when done
                            drop(permit);
                        });
                    }
                    Err(e) => {
                        error!("Failed to accept connection: {}", e);
                        stats.errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
    }
}

/// Handle a single client connection.
async fn handle_connection(
    stream: UnixStream,
    conn_id: usize,
    config: IpcConfig,
    type_registry: Arc<IpcTypeRegistry>,
    agent_registry: Arc<DashMap<String, AgentHandle>>,
    cancel_token: CancellationToken,
    stats: Arc<IpcListenerStats>,
) {
    let (mut reader, mut writer) = stream.into_split();
    let max_message_size = config.limits.max_message_size;

    debug!("Connection #{} handler started", conn_id);

    loop {
        tokio::select! {
            biased;

            () = cancel_token.cancelled() => {
                trace!("Connection #{} received shutdown signal", conn_id);
                break;
            }

            frame_result = read_frame(&mut reader, max_message_size) => {
                match frame_result {
                    Ok((msg_type, payload)) => {
                        // Handle heartbeat
                        if is_heartbeat(msg_type) {
                            trace!("Connection #{} received heartbeat", conn_id);
                            if let Err(e) = write_heartbeat(&mut writer).await {
                                error!("Connection #{} failed to send heartbeat response: {}", conn_id, e);
                                break;
                            }
                            continue;
                        }

                        // Handle request
                        if msg_type != MSG_TYPE_REQUEST {
                            warn!("Connection #{} received unexpected message type: {:#04x}", conn_id, msg_type);
                            continue;
                        }

                        stats.messages_received.fetch_add(1, Ordering::Relaxed);

                        // Parse envelope
                        let envelope: IpcEnvelope = match serde_json::from_slice(&payload) {
                            Ok(env) => env,
                            Err(e) => {
                                let response = IpcResponse::error_with_message(
                                    "unknown",
                                    "SERIALIZATION_ERROR",
                                    format!("Failed to parse envelope: {e}"),
                                );
                                if let Err(e) = write_response(&mut writer, &response).await {
                                    error!("Connection #{} failed to send error response: {}", conn_id, e);
                                    break;
                                }
                                stats.errors.fetch_add(1, Ordering::Relaxed);
                                continue;
                            }
                        };

                        trace!(
                            "Connection #{} received request: correlation_id={}, target={}, type={}",
                            conn_id,
                            envelope.correlation_id,
                            envelope.target,
                            envelope.message_type
                        );

                        // Process the envelope and send response
                        let response = process_envelope(
                            &envelope,
                            &type_registry,
                            &agent_registry,
                            &stats,
                        )
                        .await;

                        if let Err(e) = write_response(&mut writer, &response).await {
                            error!("Connection #{} failed to send response: {}", conn_id, e);
                            break;
                        }
                    }
                    Err(IpcError::ConnectionClosed) => {
                        debug!("Connection #{} closed by client", conn_id);
                        break;
                    }
                    Err(e) => {
                        error!("Connection #{} error: {}", conn_id, e);
                        stats.errors.fetch_add(1, Ordering::Relaxed);
                        break;
                    }
                }
            }
        }
    }

    stats.connections_active.fetch_sub(1, Ordering::Relaxed);
    debug!("Connection #{} handler finished", conn_id);
}

/// Process an IPC envelope and route to the target agent.
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

    // Send the message to the agent
    // Note: We use send_raw which accepts Box<dyn ActonMessage>
    if let Err(e) = agent_handle.send_boxed(message).await {
        let err = IpcError::IoError(format!("Failed to send message to agent: {e}"));
        stats.errors.fetch_add(1, Ordering::Relaxed);
        return IpcResponse::error(correlation_id, &err);
    }

    stats.messages_routed.fetch_add(1, Ordering::Relaxed);

    // For now, return a simple acknowledgment
    // Phase 3 will add request-response correlation
    IpcResponse::success(
        correlation_id,
        Some(serde_json::json!({ "status": "delivered" })),
    )
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
    fn test_listener_handle_cancel() {
        let stats = Arc::new(IpcListenerStats::new());
        let cancel_token = CancellationToken::new();

        let handle = IpcListenerHandle {
            stats: stats.clone(),
            cancel_token: cancel_token.clone(),
        };

        assert!(!handle.is_cancelled());
        handle.stop();
        assert!(handle.is_cancelled());
    }

    #[test]
    fn test_socket_exists() {
        // Non-existent path should return false
        assert!(!socket_exists(Path::new("/nonexistent/path/socket.sock")));
    }
}
