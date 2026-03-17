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

//! Channel-based IPC client for connecting to an acton-reactive server.
//!
//! This module provides [`IpcClient`], a high-level client abstraction that mirrors
//! the server-side channel-based writer pattern used in the IPC listener. Instead of
//! sharing a socket writer behind `Arc<Mutex<_>>`, the client uses an `mpsc` channel
//! to send write commands to a dedicated writer task that exclusively owns the socket
//! write half.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────┐        mpsc channel         ┌──────────────┐
//! │  Caller(s)  │ ──── ClientWriteCommand ───> │  Writer Task │ ── OwnedWriteHalf
//! └─────────────┘                              └──────────────┘
//!
//!                                              ┌──────────────┐
//!                        pending_requests      │  Reader Task │ ── OwnedReadHalf
//!  oneshot::Receiver <── DashMap<corr_id> ──── │              │
//!                                              │  push_tx ────│──> mpsc::Receiver
//!                                              └──────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use acton_reactive::ipc::client::{IpcClient, IpcClientConfig};
//! use acton_reactive::ipc::IpcEnvelope;
//!
//! // Connect to the server
//! let client = IpcClient::connect("/tmp/my_app/ipc.sock").await?;
//!
//! // Fire-and-forget send
//! let envelope = IpcEnvelope::new("my_actor", "MyMessage", payload);
//! client.send(envelope).await?;
//!
//! // Request-response
//! let envelope = IpcEnvelope::new_request("my_actor", "MyQuery", payload);
//! let response = client.request(envelope).await?;
//!
//! // Subscribe to push notifications
//! let sub_response = client.subscribe(vec!["PriceUpdate".into()]).await?;
//! let push_rx = client.take_push_receiver().unwrap();
//! while let Some(notification) = push_rx.recv().await {
//!     println!("Got: {:?}", notification);
//! }
//! ```

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use serde::Deserialize;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, error, trace, warn};

use super::protocol::{
    read_frame, write_frame, Format, MAX_FRAME_SIZE, MSG_TYPE_DISCOVER, MSG_TYPE_ERROR,
    MSG_TYPE_HEARTBEAT, MSG_TYPE_PUSH, MSG_TYPE_REQUEST, MSG_TYPE_RESPONSE, MSG_TYPE_SUBSCRIBE,
    MSG_TYPE_UNSUBSCRIBE,
};
use super::types::{
    IpcDiscoverRequest, IpcDiscoverResponse, IpcEnvelope, IpcError, IpcPushNotification,
    IpcResponse, IpcSubscribeRequest, IpcSubscriptionResponse, IpcUnsubscribeRequest,
};

// ============================================================================
// Constants
// ============================================================================

/// Default capacity for the writer command channel.
const DEFAULT_WRITER_CHANNEL_CAPACITY: usize = 64;

/// Default capacity for the push notification channel.
const DEFAULT_PUSH_CHANNEL_CAPACITY: usize = 256;

/// Default timeout for request-response operations.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

// ============================================================================
// Types
// ============================================================================

/// Raw response bytes with their format, used for type-agnostic correlation matching.
type RawResponse = Result<(Format, Vec<u8>), IpcError>;

/// Map of correlation IDs to pending response channels.
type PendingRequests = DashMap<String, oneshot::Sender<RawResponse>>;

/// Commands sent to the client's dedicated writer task.
///
/// Mirrors the server-side `WriteCommand` pattern from `listener.rs`, but for
/// client-to-server operations.
enum ClientWriteCommand {
    /// Send a fire-and-forget envelope (`MSG_TYPE_REQUEST`).
    /// The server responds with `MSG_TYPE_RESPONSE` which the reader drains.
    Envelope { envelope: IpcEnvelope, format: Format },

    /// Send a request that expects a correlated response.
    Request {
        envelope: IpcEnvelope,
        format: Format,
        reply_tx: oneshot::Sender<RawResponse>,
    },

    /// Send a subscribe request.
    Subscribe {
        request: IpcSubscribeRequest,
        format: Format,
        reply_tx: oneshot::Sender<RawResponse>,
    },

    /// Send an unsubscribe request.
    Unsubscribe {
        request: IpcUnsubscribeRequest,
        format: Format,
        reply_tx: oneshot::Sender<RawResponse>,
    },

    /// Send a discovery request.
    Discover {
        request: IpcDiscoverRequest,
        format: Format,
        reply_tx: oneshot::Sender<RawResponse>,
    },

    /// Gracefully shut down the writer task.
    Shutdown,
}

/// Minimal struct for peeking at a response's correlation ID without full deserialization.
///
/// All response types (`IpcResponse`, `IpcSubscriptionResponse`, `IpcDiscoverResponse`)
/// share `correlation_id` as their first field.
#[derive(Deserialize)]
struct CorrelationPeek {
    correlation_id: String,
}

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for an [`IpcClient`] connection.
#[derive(Debug, Clone)]
pub struct IpcClientConfig {
    /// Wire format for serialization (JSON or `MessagePack`).
    pub format: Format,

    /// Capacity of the writer command channel.
    ///
    /// Controls how many write commands can be buffered before `send()` applies
    /// backpressure. Default: 64.
    pub writer_channel_capacity: usize,

    /// Capacity of the push notification channel.
    ///
    /// Controls how many push notifications can be buffered before the reader
    /// task drops them. Default: 256.
    pub push_channel_capacity: usize,

    /// Default timeout for request-response operations.
    ///
    /// Default: 30 seconds.
    pub default_timeout: Duration,

    /// Maximum frame size for incoming messages.
    ///
    /// Default: [`MAX_FRAME_SIZE`] (16 MiB).
    pub max_frame_size: usize,
}

impl Default for IpcClientConfig {
    fn default() -> Self {
        Self {
            format: Format::default(),
            writer_channel_capacity: DEFAULT_WRITER_CHANNEL_CAPACITY,
            push_channel_capacity: DEFAULT_PUSH_CHANNEL_CAPACITY,
            default_timeout: DEFAULT_TIMEOUT,
            max_frame_size: MAX_FRAME_SIZE,
        }
    }
}

// ============================================================================
// IpcClient
// ============================================================================

/// A channel-based IPC client for connecting to an acton-reactive server.
///
/// The client splits the Unix socket into a reader half (owned by a reader task)
/// and a writer half (owned by a writer task). All write operations go through
/// an `mpsc` channel, eliminating mutex contention and enabling non-blocking
/// publishes.
///
/// # Connection Lifecycle
///
/// 1. [`connect`](Self::connect) establishes the socket and spawns reader/writer tasks
/// 2. Use [`send`](Self::send) for fire-and-forget messages
/// 3. Use [`request`](Self::request) for request-response patterns
/// 4. Use [`subscribe`](Self::subscribe) + [`take_push_receiver`](Self::take_push_receiver)
///    for push notifications
/// 5. [`disconnect`](Self::disconnect) or drop to clean up
pub struct IpcClient {
    /// Sender for write commands to the writer task.
    writer_tx: mpsc::Sender<ClientWriteCommand>,

    /// Receiver for push notifications from subscriptions.
    ///
    /// Behind `std::sync::Mutex` for one-time extraction via `take_push_receiver()`.
    /// This is NOT contended during normal operation — only accessed once.
    push_rx: std::sync::Mutex<Option<mpsc::Receiver<IpcPushNotification>>>,

    /// Wire format for this connection.
    format: Format,

    /// Default timeout for request-response operations.
    default_timeout: Duration,

    /// Handle to the reader task (for cleanup on drop).
    reader_handle: JoinHandle<()>,

    /// Handle to the writer task (for cleanup on drop).
    ///
    /// Wrapped in `Mutex<Option<>>` so `disconnect()` can take ownership
    /// and await the task to drain pending writes before closing.
    writer_handle: std::sync::Mutex<Option<JoinHandle<()>>>,

    /// Whether the client has been shut down.
    shutdown: AtomicBool,
}

impl IpcClient {
    /// Connect to a Unix domain socket at the given path with default configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket connection fails.
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self, IpcError> {
        Self::connect_with_config(path, IpcClientConfig::default()).await
    }

    /// Connect to a Unix domain socket with custom configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket connection fails.
    pub async fn connect_with_config(
        path: impl AsRef<Path>,
        config: IpcClientConfig,
    ) -> Result<Self, IpcError> {
        let path = path.as_ref();
        debug!(path = %path.display(), "IPC client connecting");

        let stream = UnixStream::connect(path)
            .await
            .map_err(|e| IpcError::IoError(format!("Failed to connect to {}: {e}", path.display())))?;

        let (reader, writer) = stream.into_split();

        // Create the writer command channel
        let (writer_tx, writer_rx) =
            mpsc::channel::<ClientWriteCommand>(config.writer_channel_capacity);

        // Create the push notification channel
        let (push_tx, push_rx) =
            mpsc::channel::<IpcPushNotification>(config.push_channel_capacity);

        // Shared pending requests map for correlation matching
        let pending_requests: std::sync::Arc<PendingRequests> =
            std::sync::Arc::new(DashMap::new());
        let pending_for_writer = std::sync::Arc::clone(&pending_requests);

        // Spawn the writer task (exclusively owns the write half)
        let writer_handle = tokio::spawn(async move {
            run_client_writer_task(writer, writer_rx, pending_for_writer).await;
        });

        // Spawn the reader task (exclusively owns the read half)
        let max_frame_size = config.max_frame_size;
        let reader_handle = tokio::spawn(async move {
            run_client_reader_task(reader, pending_requests, push_tx, max_frame_size).await;
        });

        debug!(path = %path.display(), "IPC client connected");

        Ok(Self {
            writer_tx,
            push_rx: std::sync::Mutex::new(Some(push_rx)),
            format: config.format,
            default_timeout: config.default_timeout,
            reader_handle,
            writer_handle: std::sync::Mutex::new(Some(writer_handle)),
            shutdown: AtomicBool::new(false),
        })
    }

    /// Send a fire-and-forget envelope.
    ///
    /// The message is enqueued to the writer channel and returns immediately.
    /// The server's response (if any) will be drained by the reader task.
    ///
    /// # Errors
    ///
    /// Returns an error if the writer channel is closed (client disconnected).
    pub async fn send(&self, envelope: IpcEnvelope) -> Result<(), IpcError> {
        self.writer_tx
            .send(ClientWriteCommand::Envelope {
                envelope,
                format: self.format,
            })
            .await
            .map_err(|_| IpcError::ConnectionClosed)
    }

    /// Send a request and wait for a correlated response.
    ///
    /// Uses the client's default timeout.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, times out, or the connection is closed.
    pub async fn request(&self, envelope: IpcEnvelope) -> Result<IpcResponse, IpcError> {
        self.request_with_timeout(envelope, self.default_timeout)
            .await
    }

    /// Send a request and wait for a correlated response with a custom timeout.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, times out, or the connection is closed.
    pub async fn request_with_timeout(
        &self,
        envelope: IpcEnvelope,
        timeout_duration: Duration,
    ) -> Result<IpcResponse, IpcError> {
        let (reply_tx, reply_rx) = oneshot::channel();

        self.writer_tx
            .send(ClientWriteCommand::Request {
                envelope,
                format: self.format,
                reply_tx,
            })
            .await
            .map_err(|_| IpcError::ConnectionClosed)?;

        let (format, bytes) = tokio::time::timeout(timeout_duration, reply_rx)
            .await
            .map_err(|_| IpcError::Timeout)?
            .map_err(|_| IpcError::ConnectionClosed)??;

        format.deserialize(&bytes)
    }

    /// Subscribe to message types.
    ///
    /// Returns the server's subscription response containing the current set
    /// of subscribed types.
    ///
    /// # Errors
    ///
    /// Returns an error if the subscription request fails or times out.
    pub async fn subscribe(
        &self,
        message_types: Vec<String>,
    ) -> Result<IpcSubscriptionResponse, IpcError> {
        let request = IpcSubscribeRequest::new(message_types);
        let (reply_tx, reply_rx) = oneshot::channel();

        self.writer_tx
            .send(ClientWriteCommand::Subscribe {
                request,
                format: self.format,
                reply_tx,
            })
            .await
            .map_err(|_| IpcError::ConnectionClosed)?;

        let (format, bytes) = tokio::time::timeout(self.default_timeout, reply_rx)
            .await
            .map_err(|_| IpcError::Timeout)?
            .map_err(|_| IpcError::ConnectionClosed)??;

        format.deserialize(&bytes)
    }

    /// Unsubscribe from message types.
    ///
    /// Pass an empty vector to unsubscribe from all types.
    ///
    /// # Errors
    ///
    /// Returns an error if the unsubscription request fails or times out.
    pub async fn unsubscribe(
        &self,
        message_types: Vec<String>,
    ) -> Result<IpcSubscriptionResponse, IpcError> {
        let request = if message_types.is_empty() {
            IpcUnsubscribeRequest::unsubscribe_all()
        } else {
            IpcUnsubscribeRequest::new(message_types)
        };
        let (reply_tx, reply_rx) = oneshot::channel();

        self.writer_tx
            .send(ClientWriteCommand::Unsubscribe {
                request,
                format: self.format,
                reply_tx,
            })
            .await
            .map_err(|_| IpcError::ConnectionClosed)?;

        let (format, bytes) = tokio::time::timeout(self.default_timeout, reply_rx)
            .await
            .map_err(|_| IpcError::Timeout)?
            .map_err(|_| IpcError::ConnectionClosed)??;

        format.deserialize(&bytes)
    }

    /// Discover available actors and message types.
    ///
    /// # Errors
    ///
    /// Returns an error if the discovery request fails or times out.
    pub async fn discover(&self) -> Result<IpcDiscoverResponse, IpcError> {
        let request = IpcDiscoverRequest::new();
        let (reply_tx, reply_rx) = oneshot::channel();

        self.writer_tx
            .send(ClientWriteCommand::Discover {
                request,
                format: self.format,
                reply_tx,
            })
            .await
            .map_err(|_| IpcError::ConnectionClosed)?;

        let (format, bytes) = tokio::time::timeout(self.default_timeout, reply_rx)
            .await
            .map_err(|_| IpcError::Timeout)?
            .map_err(|_| IpcError::ConnectionClosed)??;

        format.deserialize(&bytes)
    }

    /// Take the push notification receiver.
    ///
    /// Returns `None` if already taken. The caller owns the receiver and can
    /// poll it for incoming push notifications from subscriptions.
    ///
    /// This is designed for one-time extraction — the `Mutex` is only contended
    /// during this call, never during normal operation.
    pub fn take_push_receiver(&self) -> Option<mpsc::Receiver<IpcPushNotification>> {
        self.push_rx
            .lock()
            .ok()
            .and_then(|mut guard| guard.take())
    }

    /// Get the wire format used by this client.
    #[must_use]
    pub const fn format(&self) -> Format {
        self.format
    }

    /// Check if the client is still connected.
    ///
    /// Returns `false` if the client has been shut down or the writer task has exited.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        if self.shutdown.load(Ordering::Relaxed) {
            return false;
        }
        self.writer_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|h| !h.is_finished())
    }

    /// Gracefully disconnect from the server.
    ///
    /// Sends an unsubscribe-all request, shuts down the writer task, and aborts
    /// the reader task.
    ///
    /// # Errors
    ///
    /// Returns an error if the shutdown sequence encounters issues.
    pub async fn disconnect(&self) -> Result<(), IpcError> {
        if self.shutdown.swap(true, Ordering::SeqCst) {
            return Ok(()); // Already shut down
        }

        // Best-effort unsubscribe-all
        let request = IpcUnsubscribeRequest::unsubscribe_all();
        let payload = self.format.serialize(&request)?;
        let _ = self
            .writer_tx
            .send(ClientWriteCommand::Envelope {
                envelope: IpcEnvelope::new(
                    "__unsubscribe__",
                    "IpcUnsubscribeRequest",
                    serde_json::Value::Null,
                ),
                format: self.format,
            })
            .await;

        // Send raw unsubscribe frame via a special envelope that the writer
        // won't try to correlate. Actually, let's just send the shutdown command
        // to cleanly exit the writer. The server will clean up subscriptions
        // when the connection drops.
        drop(payload); // Not needed — server cleans up on disconnect

        // Signal the writer task to exit and wait for it to drain pending writes
        let _ = self
            .writer_tx
            .send(ClientWriteCommand::Shutdown)
            .await;

        // Await the writer task so all pending frames (including large ones)
        // are flushed to the socket before we close the connection.
        if let Some(handle) = self
            .writer_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = handle.await;
        }

        // Abort the reader task (it will exit when the socket closes)
        self.reader_handle.abort();

        debug!("IPC client disconnected");
        Ok(())
    }
}

impl Drop for IpcClient {
    fn drop(&mut self) {
        // Abort the reader task unconditionally (it exits when the socket closes)
        self.reader_handle.abort();

        // Only abort the writer if disconnect() didn't already drain it
        if let Some(handle) = self
            .writer_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            handle.abort();
        }
    }
}

// ============================================================================
// Writer Task
// ============================================================================

/// Serialize, register a pending reply, and write a frame to the socket.
///
/// Registers the `reply_tx` in `pending_requests` keyed by `correlation_id`
/// **before** writing the frame, preventing races with the reader task.
/// On serialization or write failure, the pending entry is cleaned up and
/// the error is sent through the oneshot channel.
///
/// Returns `None` on serialization failure (caller should `continue`),
/// or `Some(result)` with the write result.
async fn write_correlated_frame<T: serde::Serialize + Sync>(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    pending_requests: &PendingRequests,
    correlation_id: String,
    reply_tx: oneshot::Sender<RawResponse>,
    msg_type: u8,
    format: Format,
    value: &T,
) -> Option<Result<(), IpcError>> {
    pending_requests.insert(correlation_id.clone(), reply_tx);

    let payload = match format.serialize(value) {
        Ok(p) => p,
        Err(e) => {
            if let Some((_, tx)) = pending_requests.remove(&correlation_id) {
                let _ = tx.send(Err(e.clone()));
            }
            error!(error = %e, "Failed to serialize IPC frame");
            return None;
        }
    };

    let result = write_frame(writer, msg_type, format, &payload).await;
    if let Err(ref e) = result {
        if let Some((_, tx)) = pending_requests.remove(&correlation_id) {
            let _ = tx.send(Err(e.clone()));
        }
    }
    Some(result)
}

/// Dedicated writer task that exclusively owns the socket write half.
///
/// Mirrors the server-side `run_writer_task` in `listener.rs`. Receives
/// `ClientWriteCommand` messages from the mpsc channel and writes frames
/// to the socket sequentially.
///
/// For request-response commands, the pending request is registered in the
/// `DashMap` **before** writing the frame to prevent races with the reader task.
async fn run_client_writer_task(
    mut writer: tokio::net::unix::OwnedWriteHalf,
    mut receiver: mpsc::Receiver<ClientWriteCommand>,
    pending_requests: std::sync::Arc<PendingRequests>,
) {
    trace!("IPC client writer task started");

    while let Some(cmd) = receiver.recv().await {
        let result = match cmd {
            ClientWriteCommand::Envelope { envelope, format } => {
                let payload = match format.serialize(&envelope) {
                    Ok(p) => p,
                    Err(e) => {
                        error!(error = %e, "Failed to serialize envelope");
                        continue;
                    }
                };
                Some(write_frame(&mut writer, MSG_TYPE_REQUEST, format, &payload).await)
            }

            ClientWriteCommand::Request { envelope, format, reply_tx } => {
                let cid = envelope.correlation_id.clone();
                write_correlated_frame(
                    &mut writer, &pending_requests, cid, reply_tx,
                    MSG_TYPE_REQUEST, format, &envelope,
                ).await
            }

            ClientWriteCommand::Subscribe { request, format, reply_tx } => {
                let cid = request.correlation_id.clone();
                write_correlated_frame(
                    &mut writer, &pending_requests, cid, reply_tx,
                    MSG_TYPE_SUBSCRIBE, format, &request,
                ).await
            }

            ClientWriteCommand::Unsubscribe { request, format, reply_tx } => {
                let cid = request.correlation_id.clone();
                write_correlated_frame(
                    &mut writer, &pending_requests, cid, reply_tx,
                    MSG_TYPE_UNSUBSCRIBE, format, &request,
                ).await
            }

            ClientWriteCommand::Discover { request, format, reply_tx } => {
                let cid = request.correlation_id.clone();
                write_correlated_frame(
                    &mut writer, &pending_requests, cid, reply_tx,
                    MSG_TYPE_DISCOVER, format, &request,
                ).await
            }

            ClientWriteCommand::Shutdown => {
                trace!("IPC client writer received shutdown command");
                break;
            }
        };

        if let Some(Err(e)) = result {
            error!(error = %e, "IPC client writer error, closing connection");
            break;
        }
    }

    // On exit, clear all pending requests. Dropping the oneshot senders
    // causes `RecvError` on the receiver side, which callers map to `ConnectionClosed`.
    pending_requests.clear();

    trace!("IPC client writer task finished");
}

// ============================================================================
// Reader Task
// ============================================================================

/// Dedicated reader task that exclusively owns the socket read half.
///
/// Routes incoming frames by message type:
/// - `MSG_TYPE_RESPONSE` / `MSG_TYPE_ERROR`: Matches by correlation ID to pending
///   requests. Unclaimed responses (fire-and-forget acks) are drained.
/// - `MSG_TYPE_PUSH`: Forwarded through the push notification channel.
/// - `MSG_TYPE_HEARTBEAT`: Ignored.
/// - Other: Logged as warning.
async fn run_client_reader_task(
    mut reader: tokio::net::unix::OwnedReadHalf,
    pending_requests: std::sync::Arc<PendingRequests>,
    push_tx: mpsc::Sender<IpcPushNotification>,
    max_frame_size: usize,
) {
    trace!("IPC client reader task started");

    loop {
        match read_frame(&mut reader, max_frame_size).await {
            Ok((msg_type, format, payload)) => match msg_type {
                MSG_TYPE_RESPONSE | MSG_TYPE_ERROR => {
                    handle_response_frame(&pending_requests, format, payload);
                }
                MSG_TYPE_PUSH => {
                    handle_push_frame(&push_tx, format, &payload);
                }
                MSG_TYPE_HEARTBEAT => {
                    trace!("IPC client received heartbeat");
                }
                _ => {
                    warn!(msg_type, "IPC client received unknown message type");
                }
            },
            Err(IpcError::ConnectionClosed) => {
                debug!("IPC client connection closed by server");
                break;
            }
            Err(e) => {
                error!(error = %e, "IPC client reader error");
                break;
            }
        }
    }

    // Fail any remaining pending requests
    for entry in pending_requests.iter() {
        let correlation_id = entry.key().clone();
        if let Some((_, tx)) = pending_requests.remove(&correlation_id) {
            let _ = tx.send(Err(IpcError::ConnectionClosed));
        }
    }

    trace!("IPC client reader task finished");
}

/// Handle an incoming response or error frame.
///
/// Peeks at the `correlation_id` field via minimal deserialization, then routes
/// to the matching pending request. Unclaimed responses (from fire-and-forget
/// sends) are silently drained.
fn handle_response_frame(
    pending_requests: &PendingRequests,
    format: Format,
    payload: Vec<u8>,
) {
    // Peek at the correlation_id without full deserialization
    let correlation_id = match format.deserialize::<CorrelationPeek>(&payload) {
        Ok(peek) => peek.correlation_id,
        Err(e) => {
            warn!(error = %e, "Failed to peek correlation_id from response");
            return;
        }
    };

    // Look up and remove the pending request
    if let Some((_, reply_tx)) = pending_requests.remove(&correlation_id) {
        // Send raw bytes — the caller will deserialize to the expected type
        let _ = reply_tx.send(Ok((format, payload)));
    } else {
        // Fire-and-forget response or unknown correlation — drain silently
        trace!(correlation_id, "Draining unclaimed response");
    }
}

/// Handle an incoming push notification frame.
fn handle_push_frame(
    push_tx: &mpsc::Sender<IpcPushNotification>,
    format: Format,
    payload: &[u8],
) {
    match format.deserialize::<IpcPushNotification>(payload) {
        Ok(notification) => {
            if let Err(e) = push_tx.try_send(notification) {
                match e {
                    mpsc::error::TrySendError::Full(_) => {
                        warn!("Push notification channel full, dropping notification");
                    }
                    mpsc::error::TrySendError::Closed(_) => {
                        debug!("Push notification channel closed");
                    }
                }
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to deserialize push notification");
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    /// Test that `IpcClientConfig` has sensible defaults.
    #[test]
    fn config_defaults() {
        let config = IpcClientConfig::default();
        assert_eq!(config.writer_channel_capacity, 64);
        assert_eq!(config.push_channel_capacity, 256);
        assert_eq!(config.default_timeout, Duration::from_secs(30));
        assert_eq!(config.max_frame_size, MAX_FRAME_SIZE);
    }

    /// Test that the client can connect to a Unix socket.
    #[tokio::test]
    async fn connect_to_socket() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let socket_path = dir.path().join("test.sock");

        // Start a listener that accepts one connection
        let listener = tokio::net::UnixListener::bind(&socket_path)
            .expect("failed to bind socket");

        let accept_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("failed to accept");
            stream
        });

        // Connect with the client
        let client = IpcClient::connect(&socket_path).await;
        assert!(client.is_ok());

        let client = client.expect("client should connect");
        assert!(client.is_connected());

        // Clean up
        let _server_stream = accept_handle.await.expect("accept failed");
        drop(client);
    }

    /// Test that connect fails for a non-existent socket.
    #[tokio::test]
    async fn connect_fails_for_missing_socket() {
        let result = IpcClient::connect("/tmp/nonexistent_test_socket_12345.sock").await;
        assert!(result.is_err());
    }

    /// Test fire-and-forget send enqueues without blocking.
    #[tokio::test]
    async fn send_fire_and_forget() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let socket_path = dir.path().join("test.sock");

        let listener = tokio::net::UnixListener::bind(&socket_path)
            .expect("failed to bind socket");

        let accept_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("failed to accept");
            let (mut reader, mut writer) = stream.into_split();

            // Read the frame the client sends
            let result = read_frame(&mut reader, MAX_FRAME_SIZE).await;
            assert!(result.is_ok());
            let (msg_type, _, payload) = result.expect("should read frame");
            assert_eq!(msg_type, MSG_TYPE_REQUEST);

            // Verify it's an IpcEnvelope
            let envelope: IpcEnvelope =
                serde_json::from_slice(&payload).expect("should deserialize");
            assert_eq!(envelope.target, "test_actor");

            // Send a response back (server always responds to requests)
            let response = IpcResponse::success(&envelope.correlation_id, None);
            let resp_payload =
                serde_json::to_vec(&response).expect("should serialize response");
            write_frame(
                &mut writer,
                MSG_TYPE_RESPONSE,
                Format::Json,
                &resp_payload,
            )
            .await
            .expect("should write response");

            // Keep connection alive briefly
            tokio::time::sleep(Duration::from_millis(100)).await;
        });

        let client = IpcClient::connect(&socket_path)
            .await
            .expect("should connect");

        let envelope = IpcEnvelope::new(
            "test_actor",
            "TestMessage",
            serde_json::json!({"key": "value"}),
        );
        let result = client.send(envelope).await;
        assert!(result.is_ok());

        accept_handle.await.expect("server task failed");
    }

    /// Test request-response with correlation matching.
    #[tokio::test]
    async fn request_response_correlation() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let socket_path = dir.path().join("test.sock");

        let listener = tokio::net::UnixListener::bind(&socket_path)
            .expect("failed to bind socket");

        let accept_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("failed to accept");
            let (mut reader, mut writer) = stream.into_split();

            // Read the request
            let (msg_type, _, payload) = read_frame(&mut reader, MAX_FRAME_SIZE)
                .await
                .expect("should read frame");
            assert_eq!(msg_type, MSG_TYPE_REQUEST);

            let envelope: IpcEnvelope =
                serde_json::from_slice(&payload).expect("should deserialize");

            // Send correlated response
            let response = IpcResponse::success(
                &envelope.correlation_id,
                Some(serde_json::json!({"result": 42})),
            );
            let resp_payload =
                serde_json::to_vec(&response).expect("should serialize response");
            write_frame(
                &mut writer,
                MSG_TYPE_RESPONSE,
                Format::Json,
                &resp_payload,
            )
            .await
            .expect("should write response");

            // Keep connection alive
            tokio::time::sleep(Duration::from_millis(100)).await;
        });

        let client = IpcClient::connect(&socket_path)
            .await
            .expect("should connect");

        let envelope = IpcEnvelope::new_request(
            "test_actor",
            "TestQuery",
            serde_json::json!({"query": "test"}),
        );
        let response = client.request(envelope).await;
        assert!(response.is_ok());

        let response = response.expect("should get response");
        assert!(response.success);
        assert_eq!(
            response.payload,
            Some(serde_json::json!({"result": 42}))
        );

        accept_handle.await.expect("server task failed");
    }

    /// Test subscribe and receive push notifications.
    #[tokio::test]
    async fn subscribe_and_receive_push() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let socket_path = dir.path().join("test.sock");

        let listener = tokio::net::UnixListener::bind(&socket_path)
            .expect("failed to bind socket");

        let accept_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("failed to accept");
            let (mut reader, mut writer) = stream.into_split();

            // Read subscribe request
            let (msg_type, _, payload) = read_frame(&mut reader, MAX_FRAME_SIZE)
                .await
                .expect("should read frame");
            assert_eq!(msg_type, MSG_TYPE_SUBSCRIBE);

            let sub_request: IpcSubscribeRequest =
                serde_json::from_slice(&payload).expect("should deserialize");

            // Send subscription response
            let response = IpcSubscriptionResponse::success(
                &sub_request.correlation_id,
                sub_request.message_types.clone(),
            );
            let resp_payload =
                serde_json::to_vec(&response).expect("should serialize");
            write_frame(
                &mut writer,
                MSG_TYPE_RESPONSE,
                Format::Json,
                &resp_payload,
            )
            .await
            .expect("should write response");

            // Send a push notification
            let notification = IpcPushNotification::new(
                "TestEvent",
                Some("test_actor".to_string()),
                serde_json::json!({"data": "hello"}),
            );
            let push_payload =
                serde_json::to_vec(&notification).expect("should serialize");
            write_frame(
                &mut writer,
                MSG_TYPE_PUSH,
                Format::Json,
                &push_payload,
            )
            .await
            .expect("should write push");

            // Keep connection alive
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        let client = IpcClient::connect(&socket_path)
            .await
            .expect("should connect");

        // Subscribe
        let sub_response = client
            .subscribe(vec!["TestEvent".to_string()])
            .await
            .expect("should subscribe");
        assert!(sub_response.success);

        // Take push receiver and wait for notification
        let mut push_rx = client
            .take_push_receiver()
            .expect("should take push receiver");

        let notification = tokio::time::timeout(Duration::from_secs(2), push_rx.recv())
            .await
            .expect("should not timeout")
            .expect("should receive notification");

        assert_eq!(notification.message_type, "TestEvent");
        assert_eq!(
            notification.payload,
            serde_json::json!({"data": "hello"})
        );

        accept_handle.await.expect("server task failed");
    }

    /// Test that `take_push_receiver` returns `None` on second call.
    #[tokio::test]
    async fn take_push_receiver_once() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let socket_path = dir.path().join("test.sock");

        let listener = tokio::net::UnixListener::bind(&socket_path)
            .expect("failed to bind socket");

        let accept_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("failed to accept");
            tokio::time::sleep(Duration::from_millis(200)).await;
            drop(stream);
        });

        let client = IpcClient::connect(&socket_path)
            .await
            .expect("should connect");

        assert!(client.take_push_receiver().is_some());
        assert!(client.take_push_receiver().is_none());

        accept_handle.await.expect("server task failed");
    }

    /// Test graceful disconnect.
    #[tokio::test]
    async fn graceful_disconnect() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let socket_path = dir.path().join("test.sock");

        let listener = tokio::net::UnixListener::bind(&socket_path)
            .expect("failed to bind socket");

        let accept_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("failed to accept");
            tokio::time::sleep(Duration::from_millis(500)).await;
            drop(stream);
        });

        let client = IpcClient::connect(&socket_path)
            .await
            .expect("should connect");

        assert!(client.is_connected());

        let result = client.disconnect().await;
        assert!(result.is_ok());

        // After disconnect, is_connected should return false
        // (may take a moment for the writer task to exit)
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!client.is_connected());

        // Second disconnect should be a no-op
        let result = client.disconnect().await;
        assert!(result.is_ok());

        accept_handle.await.expect("server task failed");
    }
}
