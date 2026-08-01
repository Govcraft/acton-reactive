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
//!                        active_streams        │              │
//!  mpsc::Receiver    <── DashMap<corr_id> ──── │              │
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
//! // Request-stream (multiple frames per request)
//! let envelope = IpcEnvelope::new_stream_request("my_actor", "MyStreamQuery", payload);
//! let mut stream_rx = client.request_stream(envelope).await?;
//! while let Some(frame) = stream_rx.recv().await {
//!     println!("Got frame #{}: {:?}", frame.sequence, frame.payload);
//!     if frame.is_final {
//!         break;
//!     }
//! }
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

use super::remote_ask::RemoteActorRef;
use super::protocol::{
    read_frame, write_frame, Format, MAX_FRAME_SIZE, MSG_TYPE_DISCOVER, MSG_TYPE_ERROR,
    MSG_TYPE_HEARTBEAT, MSG_TYPE_PUSH, MSG_TYPE_REQUEST, MSG_TYPE_RESPONSE, MSG_TYPE_STREAM,
    MSG_TYPE_SUBSCRIBE, MSG_TYPE_UNSUBSCRIBE,
};
use super::types::{
    IpcDiscoverRequest, IpcDiscoverResponse, IpcEnvelope, IpcError, IpcPushNotification,
    IpcResponse, IpcStreamFrame, IpcSubscribeRequest, IpcSubscriptionResponse,
    IpcUnsubscribeRequest, CONNECTION_REJECTED_CORRELATION_ID,
};

// ============================================================================
// Constants
// ============================================================================

/// Default capacity for the writer command channel.
const DEFAULT_WRITER_CHANNEL_CAPACITY: usize = 64;

/// Default capacity for the push notification channel.
const DEFAULT_PUSH_CHANNEL_CAPACITY: usize = 256;

/// Default capacity for each per-stream frame channel.
const DEFAULT_STREAM_CHANNEL_CAPACITY: usize = 64;

/// Default timeout for request-response operations.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

// ============================================================================
// Types
// ============================================================================

/// Raw response bytes with their format, used for type-agnostic correlation matching.
type RawResponse = Result<(Format, Vec<u8>), IpcError>;

/// Map of correlation IDs to pending response channels.
type PendingRequests = DashMap<String, oneshot::Sender<RawResponse>>;

/// Map of correlation IDs to active stream frame channels.
type ActiveStreams = DashMap<String, mpsc::Sender<IpcStreamFrame>>;

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

    /// Send a streaming request (`MSG_TYPE_REQUEST` with `expects_stream`).
    /// The reader routes each `MSG_TYPE_STREAM` frame to the per-correlation-id
    /// channel registered in `active_streams`.
    StreamRequest { envelope: IpcEnvelope, format: Format },

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
/// 4. Use [`request_stream`](Self::request_stream) for request-stream patterns
/// 5. Use [`subscribe`](Self::subscribe) + [`take_push_receiver`](Self::take_push_receiver)
///    for push notifications
/// 6. [`disconnect`](Self::disconnect) or drop to clean up
pub struct IpcClient {
    /// Sender for write commands to the writer task.
    writer_tx: mpsc::Sender<ClientWriteCommand>,

    /// Receiver for push notifications from subscriptions.
    ///
    /// Behind `std::sync::Mutex` for one-time extraction via `take_push_receiver()`.
    /// This is NOT contended during normal operation — only accessed once.
    push_rx: std::sync::Mutex<Option<mpsc::Receiver<IpcPushNotification>>>,

    /// Map of correlation IDs to active stream frame channels.
    ///
    /// Shared with the reader task, which routes incoming `MSG_TYPE_STREAM`
    /// frames to the matching channel.
    active_streams: std::sync::Arc<ActiveStreams>,

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

    /// Why the server refused this connection, if it did.
    ///
    /// A connection-level rejection is a write-once fact about the connection, so
    /// this is a `OnceLock` rather than shared mutable state. The reader task
    /// records the reason before the socket closes; every path that would
    /// otherwise report a bare [`IpcError::ConnectionClosed`] consults it first,
    /// so the cause survives to the caller regardless of which task observes the
    /// shutdown first.
    rejection: std::sync::Arc<ConnectionRejection>,
}

/// Write-once record of a server-initiated connection-level rejection.
type ConnectionRejection = std::sync::OnceLock<IpcError>;

/// Resolve every outstanding request with `failure`.
///
/// Keys are collected before removal on purpose: `DashMap::iter` holds a lock on
/// the shard it is walking, and removing a key from that same shard while the
/// guard is live deadlocks the shard — which on a current-thread runtime wedges
/// the whole executor, timers included.
fn fail_pending_requests(pending_requests: &PendingRequests, failure: &IpcError) {
    let correlation_ids: Vec<String> = pending_requests
        .iter()
        .map(|entry| entry.key().clone())
        .collect();

    for correlation_id in correlation_ids {
        if let Some((_, tx)) = pending_requests.remove(&correlation_id) {
            let _ = tx.send(Err(failure.clone()));
        }
    }
}

/// The error to report for a connection that is no longer usable.
///
/// Prefers a recorded server rejection over the generic
/// [`IpcError::ConnectionClosed`], so "the server refused you at its connection
/// limit" is not flattened into "the connection closed".
fn connection_error(rejection: &ConnectionRejection) -> IpcError {
    rejection
        .get()
        .cloned()
        .unwrap_or(IpcError::ConnectionClosed)
}

impl IpcClient {
    /// The error to report when this connection is unusable.
    ///
    /// Returns the server's stated refusal reason when there is one, so callers
    /// see "connection limit reached (N)" instead of a bare "connection closed".
    fn connection_error(&self) -> IpcError {
        connection_error(&self.rejection)
    }

    /// The reason the server refused this connection, if it did.
    ///
    /// `None` for a connection that was accepted normally. This lets a caller
    /// distinguish a refusal from an ordinary disconnect without inspecting the
    /// error of a failed request.
    #[must_use]
    pub fn rejection_reason(&self) -> Option<IpcError> {
        self.rejection.get().cloned()
    }

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

        // Shared active streams map for stream frame routing
        let active_streams: std::sync::Arc<ActiveStreams> = std::sync::Arc::new(DashMap::new());
        let streams_for_writer = std::sync::Arc::clone(&active_streams);
        let streams_for_reader = std::sync::Arc::clone(&active_streams);

        // Records a server-side refusal so it survives the connection closing.
        let rejection: std::sync::Arc<ConnectionRejection> =
            std::sync::Arc::new(std::sync::OnceLock::new());
        let rejection_for_writer = std::sync::Arc::clone(&rejection);
        let rejection_for_reader = std::sync::Arc::clone(&rejection);

        // Spawn the writer task (exclusively owns the write half)
        let writer_handle = tokio::spawn(async move {
            run_client_writer_task(
                writer,
                writer_rx,
                pending_for_writer,
                streams_for_writer,
                rejection_for_writer,
            )
            .await;
        });

        // Spawn the reader task (exclusively owns the read half)
        let max_frame_size = config.max_frame_size;
        let reader_handle = tokio::spawn(async move {
            run_client_reader_task(
                reader,
                pending_requests,
                streams_for_reader,
                push_tx,
                max_frame_size,
                rejection_for_reader,
            )
            .await;
        });

        debug!(path = %path.display(), "IPC client connected");

        Ok(Self {
            writer_tx,
            push_rx: std::sync::Mutex::new(Some(push_rx)),
            active_streams,
            format: config.format,
            default_timeout: config.default_timeout,
            reader_handle,
            writer_handle: std::sync::Mutex::new(Some(writer_handle)),
            shutdown: AtomicBool::new(false),
            rejection,
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
            .map_err(|_| self.connection_error())
    }

    /// Names an actor in the peer process, so it can be
    /// [`ask`](crate::common::ipc::RemoteActorRef::ask)ed.
    ///
    /// The typed counterpart to building an [`IpcEnvelope`] by hand and calling
    /// [`request`](Self::request): where that deals in type-name strings and
    /// `serde_json::Value`, this gives back the request's declared reply type, and gives
    /// the same call as a local actor handle.
    ///
    /// ```rust,ignore
    /// let client = IpcClient::connect("/run/app.sock").await?;
    /// let count: Count = client.actor("counter").ask(GetCount).await?;
    /// ```
    ///
    /// `name` is the string the actor was exposed under with
    /// [`ActorRuntime::ipc_expose`](crate::common::ActorRuntime::ipc_expose) in the peer.
    /// Nothing is sent here and the name is not checked; an unknown one surfaces as
    /// [`AskError::PeerRejected`](crate::common::AskError::PeerRejected) when asked.
    #[must_use]
    pub const fn actor<'client>(&'client self, name: &'client str) -> RemoteActorRef<'client> {
        RemoteActorRef::new(self, name)
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
            .map_err(|_| self.connection_error())?;

        let (format, bytes) = tokio::time::timeout(timeout_duration, reply_rx)
            .await
            .map_err(|_| IpcError::Timeout)?
            .map_err(|_| self.connection_error())??;

        format.deserialize(&bytes)
    }

    /// Send a streaming request and receive the response frames through a channel.
    ///
    /// The server replies with multiple [`IpcStreamFrame`]s for a single request.
    /// Every frame is delivered through the returned receiver, in order and
    /// without gaps. The channel closes after a frame with `is_final: true` is
    /// delivered — server errors arrive as a final frame with the `error` field
    /// set, whether the actor's stream failed mid-flight or the server rejected
    /// the request before dispatching it (shutdown drain, rate limiting).
    ///
    /// Uses the client's default timeout as the maximum time to wait **between
    /// consecutive frames** (and for the first frame). If the channel closes
    /// before an `is_final` frame arrives, the stream terminated abnormally:
    /// either the inter-frame timeout elapsed, the connection was closed, or
    /// the client was dropped.
    ///
    /// The envelope's `expects_stream` flag is set automatically, so any
    /// envelope constructor works; [`IpcEnvelope::new_stream_request`] is the
    /// idiomatic choice.
    ///
    /// # Backpressure
    ///
    /// Frames are never dropped. When the receiver's buffer fills, the client's
    /// shared connection reader blocks until the consumer catches up, applying
    /// backpressure through the connection exactly like TCP flow control. A
    /// slow stream consumer therefore stalls **everything** the reader
    /// delivers: responses to concurrent [`request`](Self::request) calls and
    /// push notifications queue behind the stalled stream. Drain the receiver
    /// promptly (or from a dedicated task) — in particular, don't await a
    /// `request()` on the same client while leaving a mid-flight stream
    /// undrained, or that request may time out waiting for a response the
    /// reader can't reach. The same applies to other concurrent streams on
    /// this client: their frames also queue behind the stalled stream, so a
    /// healthy stream can hit its inter-frame timeout and terminate without
    /// an error frame while another stream is left undrained.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is closed before the request is enqueued.
    pub async fn request_stream(
        &self,
        envelope: IpcEnvelope,
    ) -> Result<mpsc::Receiver<IpcStreamFrame>, IpcError> {
        self.request_stream_with_timeout(envelope, self.default_timeout)
            .await
    }

    /// Send a streaming request with a custom inter-frame timeout.
    ///
    /// See [`request_stream`](Self::request_stream) for the stream and
    /// backpressure semantics. `frame_timeout` bounds the wait between
    /// consecutive frames, not the total stream duration — the server bounds
    /// the latter via the envelope's `response_timeout_ms`.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is closed before the request is enqueued.
    pub async fn request_stream_with_timeout(
        &self,
        mut envelope: IpcEnvelope,
        frame_timeout: Duration,
    ) -> Result<mpsc::Receiver<IpcStreamFrame>, IpcError> {
        // The server only streams when the envelope asks for it.
        envelope.expects_stream = true;
        envelope.expects_reply = false;

        let correlation_id = envelope.correlation_id.clone();
        let (frame_tx, frame_rx) = mpsc::channel(DEFAULT_STREAM_CHANNEL_CAPACITY);
        let (out_tx, out_rx) = mpsc::channel(DEFAULT_STREAM_CHANNEL_CAPACITY);

        // Register the routing entry **before** writing the frame, preventing
        // a race with the reader task (mirrors `write_correlated_frame`).
        self.active_streams.insert(correlation_id.clone(), frame_tx);

        if self
            .writer_tx
            .send(ClientWriteCommand::StreamRequest {
                envelope,
                format: self.format,
            })
            .await
            .is_err()
        {
            self.active_streams.remove(&correlation_id);
            return Err(self.connection_error());
        }

        // The forwarder applies the inter-frame timeout and completes the
        // stream on the final frame or on abnormal termination.
        tokio::spawn(run_stream_forwarder(
            correlation_id,
            frame_rx,
            out_tx,
            frame_timeout,
            std::sync::Arc::clone(&self.active_streams),
        ));

        Ok(out_rx)
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
            .map_err(|_| self.connection_error())?;

        let (format, bytes) = tokio::time::timeout(self.default_timeout, reply_rx)
            .await
            .map_err(|_| IpcError::Timeout)?
            .map_err(|_| self.connection_error())??;

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
            .map_err(|_| self.connection_error())?;

        let (format, bytes) = tokio::time::timeout(self.default_timeout, reply_rx)
            .await
            .map_err(|_| IpcError::Timeout)?
            .map_err(|_| self.connection_error())??;

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
            .map_err(|_| self.connection_error())?;

        let (format, bytes) = tokio::time::timeout(self.default_timeout, reply_rx)
            .await
            .map_err(|_| IpcError::Timeout)?
            .map_err(|_| self.connection_error())??;

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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        // are flushed to the socket before we close the connection. Take the
        // handle out of the mutex first so the guard is dropped before awaiting.
        let writer_handle = self
            .writer_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(handle) = writer_handle {
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

        // Only abort the writer if disconnect() didn't already drain it.
        // Take the handle out of the mutex first so the guard is dropped
        // before aborting.
        let writer_handle = self
            .writer_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(handle) = writer_handle {
            handle.abort();
        }

        // Aborted tasks never reach their post-loop cleanup, so terminate any
        // in-flight streams here. Dropping the frame senders makes each
        // forwarder observe end-of-stream and close its consumer's channel
        // promptly instead of waiting out the inter-frame timeout.
        self.active_streams.clear();
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
    active_streams: std::sync::Arc<ActiveStreams>,
    rejection: std::sync::Arc<ConnectionRejection>,
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

            ClientWriteCommand::StreamRequest { envelope, format } => {
                let payload = match format.serialize(&envelope) {
                    Ok(p) => p,
                    Err(e) => {
                        error!(error = %e, "Failed to serialize stream request");
                        // Drop the routing entry so the stream terminates
                        // instead of waiting for frames that will never come.
                        active_streams.remove(&envelope.correlation_id);
                        continue;
                    }
                };
                let result = write_frame(&mut writer, MSG_TYPE_REQUEST, format, &payload).await;
                if result.is_err() {
                    active_streams.remove(&envelope.correlation_id);
                }
                Some(result)
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

    // On exit, fail all pending requests. Sending the reason explicitly (rather
    // than dropping the senders and relying on `RecvError`) preserves a server
    // refusal, which a bare `RecvError` would flatten into `ConnectionClosed`.
    fail_pending_requests(&pending_requests, &connection_error(&rejection));

    // Terminate all active streams. Dropping the frame senders closes the
    // per-stream channels so consumers observe end-of-stream instead of hanging.
    active_streams.clear();

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
/// - `MSG_TYPE_STREAM`: Matches by correlation ID to active streams.
/// - `MSG_TYPE_PUSH`: Forwarded through the push notification channel.
/// - `MSG_TYPE_HEARTBEAT`: Ignored.
/// - Other: Logged as warning.
async fn run_client_reader_task(
    mut reader: tokio::net::unix::OwnedReadHalf,
    pending_requests: std::sync::Arc<PendingRequests>,
    active_streams: std::sync::Arc<ActiveStreams>,
    push_tx: mpsc::Sender<IpcPushNotification>,
    max_frame_size: usize,
    rejection: std::sync::Arc<ConnectionRejection>,
) {
    trace!("IPC client reader task started");

    loop {
        match read_frame(&mut reader, max_frame_size).await {
            Ok((msg_type, format, payload)) => match msg_type {
                MSG_TYPE_RESPONSE | MSG_TYPE_ERROR => {
                    handle_response_frame(
                        &pending_requests,
                        &active_streams,
                        format,
                        payload,
                        &rejection,
                    )
                    .await;
                }
                MSG_TYPE_STREAM => {
                    handle_stream_frame(&active_streams, format, &payload).await;
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

    // Fail any remaining pending requests, reporting the server's refusal reason
    // when it gave one rather than a bare "connection closed".
    fail_pending_requests(&pending_requests, &connection_error(&rejection));

    // Terminate any streams still in flight so consumers don't hang.
    active_streams.clear();

    trace!("IPC client reader task finished");
}

/// Handle an incoming response or error frame.
///
/// Peeks at the `correlation_id` field via minimal deserialization, then routes
/// to the matching pending request. When the correlation ID instead matches an
/// active stream, the server rejected the stream request before dispatching it
/// (shutdown drain, rate limiting, or an envelope-parse failure) — the stream
/// is terminated by delivering a synthesized final [`IpcStreamFrame`] carrying
/// the response's error (or payload). Unclaimed responses (from fire-and-forget
/// sends) are silently drained.
async fn handle_response_frame(
    pending_requests: &PendingRequests,
    active_streams: &ActiveStreams,
    format: Format,
    payload: Vec<u8>,
    rejection: &ConnectionRejection,
) {
    // Peek at the correlation_id without full deserialization
    let correlation_id = match format.deserialize::<CorrelationPeek>(&payload) {
        Ok(peek) => peek.correlation_id,
        Err(e) => {
            warn!(error = %e, "Failed to peek correlation_id from response");
            return;
        }
    };

    // A connection-level rejection belongs to no request, so it would otherwise
    // fall through to the drain below and be lost. Record it instead: the socket
    // is about to close, and this is the only statement of why.
    if correlation_id == CONNECTION_REJECTED_CORRELATION_ID {
        record_connection_rejection(format, &payload, rejection);
        return;
    }

    // Look up and remove the pending request
    if let Some((_, reply_tx)) = pending_requests.remove(&correlation_id) {
        // Send raw bytes — the caller will deserialize to the expected type
        let _ = reply_tx.send(Ok((format, payload)));
        return;
    }

    // A plain response for an active stream is a pre-dispatch rejection —
    // terminate the stream instead of leaving the consumer to time out.
    // Clone the sender out of the map so no shard lock is held while sending.
    if let Some(frame_tx) = active_streams
        .get(&correlation_id)
        .map(|entry| entry.value().clone())
    {
        debug!(correlation_id, "Server rejected stream request, terminating stream");
        let frame = synthesize_stream_termination(&correlation_id, format, &payload);
        let _ = frame_tx.send(frame).await;
        active_streams.remove(&correlation_id);
        return;
    }

    // Fire-and-forget response or unknown correlation — drain silently
    trace!(correlation_id, "Draining unclaimed response");
}

/// Decode a connection-level rejection and latch it as this connection's failure
/// reason.
///
/// Failing to decode is not fatal: the connection is closing either way, and the
/// caller falls back to [`IpcError::ConnectionClosed`].
fn record_connection_rejection(
    format: Format,
    payload: &[u8],
    rejection: &ConnectionRejection,
) {
    let Ok(response) = format.deserialize::<IpcResponse>(payload) else {
        warn!("Failed to decode connection rejection from server");
        return;
    };

    let Some(error) = response.as_connection_rejection() else {
        return;
    };

    warn!(error = %error, "Server refused the IPC connection");
    let _ = rejection.set(error);
}

/// Build the final [`IpcStreamFrame`] that terminates a stream whose request
/// the server rejected with a plain response frame.
///
/// The response's error and payload are carried over so the consumer observes
/// why the stream ended (e.g. `RATE_LIMITED`, `SHUTTING_DOWN`). If the response
/// cannot be deserialized, a generic error frame is synthesized instead.
fn synthesize_stream_termination(
    correlation_id: &str,
    format: Format,
    payload: &[u8],
) -> IpcStreamFrame {
    match format.deserialize::<IpcResponse>(payload) {
        Ok(response) if response.success => IpcStreamFrame {
            correlation_id: correlation_id.to_string(),
            sequence: 0,
            is_final: true,
            error: None,
            error_code: None,
            payload: response.payload,
        },
        Ok(response) => IpcStreamFrame {
            correlation_id: correlation_id.to_string(),
            sequence: 0,
            is_final: true,
            error: Some(response.error.unwrap_or_else(|| {
                "Server rejected the stream request".to_string()
            })),
            error_code: response.error_code,
            payload: response.payload,
        },
        Err(e) => IpcStreamFrame::error(
            correlation_id,
            0,
            format!("Server rejected the stream request with an undecodable response: {e}"),
        ),
    }
}

/// Handle an incoming stream frame.
///
/// Routes the frame to the matching active stream by correlation ID. Frames
/// for unknown (or expired) correlation IDs are warned about and dropped.
/// When the receiving side is gone, the routing entry is removed so late
/// frames don't accumulate.
///
/// The send is **awaited**: when the per-stream channel is full, the reader
/// task blocks here until the consumer drains it, applying backpressure
/// through the connection exactly like TCP flow control. Frames are never
/// silently dropped — a slow stream consumer instead stalls the shared
/// connection reader (see [`IpcClient::request_stream`]).
async fn handle_stream_frame(active_streams: &ActiveStreams, format: Format, payload: &[u8]) {
    let frame = match format.deserialize::<IpcStreamFrame>(payload) {
        Ok(frame) => frame,
        Err(e) => {
            warn!(error = %e, "Failed to deserialize stream frame");
            return;
        }
    };

    let correlation_id = frame.correlation_id.clone();
    let is_final = frame.is_final;

    // Clone the sender out of the map so no shard lock is held while sending
    let Some(frame_tx) = active_streams
        .get(&correlation_id)
        .map(|entry| entry.value().clone())
    else {
        warn!(correlation_id, "Received stream frame for unknown correlation_id, dropping");
        return;
    };

    if frame_tx.send(frame).await.is_err() {
        // The forwarder exited (receiver dropped or timed out) — clean up
        debug!(correlation_id, "Stream frame channel closed, removing stream");
        active_streams.remove(&correlation_id);
        return;
    }

    if is_final {
        // The stream is complete — release the routing entry
        active_streams.remove(&correlation_id);
    }
}

/// Per-stream forwarder that applies the inter-frame timeout.
///
/// Receives frames routed by the reader task and forwards them to the
/// consumer's channel, exiting (and closing the consumer's channel) when:
/// - a frame with `is_final: true` has been delivered (clean completion),
/// - no frame arrives within `frame_timeout` (client-side read timeout),
/// - the consumer drops its receiver (stream cancelled), or
/// - the routing entry is dropped (connection closed or client shut down).
///
/// On exit, the routing entry is removed so late frames for this correlation
/// ID are warned about and dropped rather than buffered forever.
async fn run_stream_forwarder(
    correlation_id: String,
    mut frame_rx: mpsc::Receiver<IpcStreamFrame>,
    out_tx: mpsc::Sender<IpcStreamFrame>,
    frame_timeout: Duration,
    active_streams: std::sync::Arc<ActiveStreams>,
) {
    trace!(correlation_id, "Stream forwarder started");

    loop {
        match tokio::time::timeout(frame_timeout, frame_rx.recv()).await {
            Ok(Some(frame)) => {
                let is_final = frame.is_final;
                if out_tx.send(frame).await.is_err() {
                    // Consumer dropped the receiver mid-stream
                    debug!(correlation_id, "Stream receiver dropped, cancelling stream");
                    break;
                }
                if is_final {
                    trace!(correlation_id, "Stream completed with final frame");
                    break;
                }
            }
            Ok(None) => {
                // All senders dropped: connection closed or client shut down
                debug!(correlation_id, "Stream terminated before final frame");
                break;
            }
            Err(_) => {
                warn!(
                    correlation_id,
                    timeout = ?frame_timeout,
                    "Stream timed out waiting for next frame"
                );
                break;
            }
        }
    }

    active_streams.remove(&correlation_id);
    trace!(correlation_id, "Stream forwarder finished");
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

    /// Test that stream frames are routed to the `request_stream` receiver and
    /// the channel closes cleanly after the final frame.
    #[tokio::test]
    async fn request_stream_receives_frames_until_final() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let socket_path = dir.path().join("test.sock");

        let listener = tokio::net::UnixListener::bind(&socket_path)
            .expect("failed to bind socket");

        let accept_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("failed to accept");
            let (mut reader, mut writer) = stream.into_split();

            // Read the stream request
            let (msg_type, _, payload) = read_frame(&mut reader, MAX_FRAME_SIZE)
                .await
                .expect("should read frame");
            assert_eq!(msg_type, MSG_TYPE_REQUEST);

            let envelope: IpcEnvelope =
                serde_json::from_slice(&payload).expect("should deserialize");
            assert!(envelope.expects_stream);

            // Send three data frames followed by a final frame
            for sequence in 0..3_u32 {
                let frame = IpcStreamFrame::data(
                    &envelope.correlation_id,
                    sequence,
                    serde_json::json!({ "n": sequence }),
                );
                let frame_payload =
                    serde_json::to_vec(&frame).expect("should serialize frame");
                write_frame(&mut writer, MSG_TYPE_STREAM, Format::Json, &frame_payload)
                    .await
                    .expect("should write frame");
            }

            let final_frame = IpcStreamFrame::final_frame(&envelope.correlation_id, 3, None);
            let final_payload =
                serde_json::to_vec(&final_frame).expect("should serialize final frame");
            write_frame(&mut writer, MSG_TYPE_STREAM, Format::Json, &final_payload)
                .await
                .expect("should write final frame");

            // Keep connection alive
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        let client = IpcClient::connect(&socket_path)
            .await
            .expect("should connect");

        let envelope = IpcEnvelope::new_stream_request(
            "test_actor",
            "TestStream",
            serde_json::json!({}),
        );
        let mut stream_rx = client
            .request_stream(envelope)
            .await
            .expect("should start stream");

        let mut frames = Vec::new();
        while let Some(frame) =
            tokio::time::timeout(Duration::from_secs(2), stream_rx.recv())
                .await
                .expect("should not timeout")
        {
            frames.push(frame);
        }

        assert_eq!(frames.len(), 4);
        for (expected_sequence, frame) in (0_u32..).zip(frames.iter()) {
            assert_eq!(frame.sequence, expected_sequence);
        }
        assert!(frames.last().expect("frames should not be empty").is_final);
        assert!(frames[..3].iter().all(|f| !f.is_final));

        // The routing entry is released after the final frame
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(client.active_streams.is_empty());

        accept_handle.await.expect("server task failed");
    }

    /// Test that the stream terminates when no frame arrives within the timeout.
    #[tokio::test]
    async fn request_stream_times_out_without_frames() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let socket_path = dir.path().join("test.sock");

        let listener = tokio::net::UnixListener::bind(&socket_path)
            .expect("failed to bind socket");

        let accept_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("failed to accept");
            let (mut reader, _writer) = stream.into_split();

            // Read the request but never respond
            let _ = read_frame(&mut reader, MAX_FRAME_SIZE).await;
            tokio::time::sleep(Duration::from_millis(500)).await;
        });

        let client = IpcClient::connect(&socket_path)
            .await
            .expect("should connect");

        let envelope = IpcEnvelope::new_stream_request(
            "test_actor",
            "TestStream",
            serde_json::json!({}),
        );
        let mut stream_rx = client
            .request_stream_with_timeout(envelope, Duration::from_millis(100))
            .await
            .expect("should start stream");

        // The channel closes without an `is_final` frame once the timeout elapses
        let result = tokio::time::timeout(Duration::from_secs(2), stream_rx.recv())
            .await
            .expect("stream should terminate before the outer timeout");
        assert!(result.is_none());

        // The routing entry is cleaned up
        assert!(client.active_streams.is_empty());

        accept_handle.await.expect("server task failed");
    }

    /// Test that a stream terminates (rather than hanging) when the connection
    /// closes mid-stream.
    #[tokio::test]
    async fn request_stream_terminates_on_connection_close() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let socket_path = dir.path().join("test.sock");

        let listener = tokio::net::UnixListener::bind(&socket_path)
            .expect("failed to bind socket");

        let accept_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("failed to accept");
            let (mut reader, mut writer) = stream.into_split();

            let (_, _, payload) = read_frame(&mut reader, MAX_FRAME_SIZE)
                .await
                .expect("should read frame");
            let envelope: IpcEnvelope =
                serde_json::from_slice(&payload).expect("should deserialize");

            // Send two data frames, then drop the connection without a final frame
            for sequence in 0..2_u32 {
                let frame = IpcStreamFrame::data(
                    &envelope.correlation_id,
                    sequence,
                    serde_json::json!({ "n": sequence }),
                );
                let frame_payload =
                    serde_json::to_vec(&frame).expect("should serialize frame");
                write_frame(&mut writer, MSG_TYPE_STREAM, Format::Json, &frame_payload)
                    .await
                    .expect("should write frame");
            }
        });

        let client = IpcClient::connect(&socket_path)
            .await
            .expect("should connect");

        let envelope = IpcEnvelope::new_stream_request(
            "test_actor",
            "TestStream",
            serde_json::json!({}),
        );
        let mut stream_rx = client
            .request_stream(envelope)
            .await
            .expect("should start stream");

        let mut frames = Vec::new();
        while let Some(frame) =
            tokio::time::timeout(Duration::from_secs(2), stream_rx.recv())
                .await
                .expect("stream should terminate, not hang")
        {
            frames.push(frame);
        }

        // Both data frames arrived, then the channel closed without `is_final`
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|f| !f.is_final));

        accept_handle.await.expect("server task failed");
    }

    /// Test that dropping the stream receiver cleans up the routing entry.
    #[tokio::test]
    async fn request_stream_receiver_drop_cleans_up() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let socket_path = dir.path().join("test.sock");

        let listener = tokio::net::UnixListener::bind(&socket_path)
            .expect("failed to bind socket");

        let accept_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("failed to accept");
            let (mut reader, mut writer) = stream.into_split();

            let (_, _, payload) = read_frame(&mut reader, MAX_FRAME_SIZE)
                .await
                .expect("should read frame");
            let envelope: IpcEnvelope =
                serde_json::from_slice(&payload).expect("should deserialize");

            // Send a frame after the client has dropped its receiver
            tokio::time::sleep(Duration::from_millis(100)).await;
            let frame = IpcStreamFrame::data(
                &envelope.correlation_id,
                0,
                serde_json::json!({ "n": 0 }),
            );
            let frame_payload =
                serde_json::to_vec(&frame).expect("should serialize frame");
            write_frame(&mut writer, MSG_TYPE_STREAM, Format::Json, &frame_payload)
                .await
                .expect("should write frame");

            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        let client = IpcClient::connect(&socket_path)
            .await
            .expect("should connect");

        let envelope = IpcEnvelope::new_stream_request(
            "test_actor",
            "TestStream",
            serde_json::json!({}),
        );
        let stream_rx = client
            .request_stream(envelope)
            .await
            .expect("should start stream");
        assert_eq!(client.active_streams.len(), 1);

        // Drop the receiver mid-stream
        drop(stream_rx);

        // Once the forwarder notices the dropped receiver, the entry is removed
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(client.active_streams.is_empty());

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

    /// Test that a slow consumer receives every frame, in order and without
    /// gaps, via reader backpressure rather than frame drops.
    #[tokio::test]
    async fn request_stream_slow_consumer_receives_all_frames() {
        // Well beyond the combined per-stream channel capacities, so the
        // reader task must block on the full channel while the consumer lags.
        const FRAME_COUNT: u32 = 300;

        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let socket_path = dir.path().join("test.sock");

        let listener = tokio::net::UnixListener::bind(&socket_path)
            .expect("failed to bind socket");

        let accept_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("failed to accept");
            let (mut reader, mut writer) = stream.into_split();

            let (_, _, payload) = read_frame(&mut reader, MAX_FRAME_SIZE)
                .await
                .expect("should read frame");
            let envelope: IpcEnvelope =
                serde_json::from_slice(&payload).expect("should deserialize");

            // Flood the client with frames as fast as the socket accepts them
            for sequence in 0..FRAME_COUNT - 1 {
                let frame = IpcStreamFrame::data(
                    &envelope.correlation_id,
                    sequence,
                    serde_json::json!({ "n": sequence }),
                );
                let frame_payload =
                    serde_json::to_vec(&frame).expect("should serialize frame");
                write_frame(&mut writer, MSG_TYPE_STREAM, Format::Json, &frame_payload)
                    .await
                    .expect("should write frame");
            }

            let final_frame =
                IpcStreamFrame::final_frame(&envelope.correlation_id, FRAME_COUNT - 1, None);
            let final_payload =
                serde_json::to_vec(&final_frame).expect("should serialize final frame");
            write_frame(&mut writer, MSG_TYPE_STREAM, Format::Json, &final_payload)
                .await
                .expect("should write final frame");

            // Keep connection alive until the consumer has drained everything
            tokio::time::sleep(Duration::from_secs(2)).await;
        });

        let client = IpcClient::connect(&socket_path)
            .await
            .expect("should connect");

        let envelope = IpcEnvelope::new_stream_request(
            "test_actor",
            "TestStream",
            serde_json::json!({}),
        );
        let mut stream_rx = client
            .request_stream(envelope)
            .await
            .expect("should start stream");

        // Lag behind the server long enough for every buffer to fill
        tokio::time::sleep(Duration::from_millis(300)).await;

        let mut frames = Vec::new();
        while let Some(frame) =
            tokio::time::timeout(Duration::from_secs(5), stream_rx.recv())
                .await
                .expect("stream should terminate, not hang")
        {
            frames.push(frame);
        }

        // Every frame arrived, in order, with no sequence gaps
        assert_eq!(frames.len(), FRAME_COUNT as usize);
        for (expected_sequence, frame) in (0_u32..).zip(frames.iter()) {
            assert_eq!(frame.sequence, expected_sequence);
        }
        assert!(frames.last().expect("frames should not be empty").is_final);

        accept_handle.await.expect("server task failed");
    }

    /// Test that a pre-dispatch server rejection (e.g. rate limiting) reaches
    /// the stream consumer as a final error frame instead of stalling the
    /// stream until the inter-frame timeout.
    #[tokio::test]
    async fn request_stream_terminated_by_rate_limit_rejection() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let socket_path = dir.path().join("test.sock");

        let listener = tokio::net::UnixListener::bind(&socket_path)
            .expect("failed to bind socket");

        let accept_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("failed to accept");
            let (mut reader, mut writer) = stream.into_split();

            let (_, _, payload) = read_frame(&mut reader, MAX_FRAME_SIZE)
                .await
                .expect("should read frame");
            let envelope: IpcEnvelope =
                serde_json::from_slice(&payload).expect("should deserialize");

            // Reject the stream request exactly like the listener's rate
            // limiter does: a plain error response on the same correlation ID.
            let response = IpcResponse::error(
                &envelope.correlation_id,
                &IpcError::RateLimited { retry_after_ms: 100 },
            );
            let resp_payload =
                serde_json::to_vec(&response).expect("should serialize response");
            write_frame(&mut writer, MSG_TYPE_ERROR, Format::Json, &resp_payload)
                .await
                .expect("should write response");

            // Keep connection alive
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        let client = IpcClient::connect(&socket_path)
            .await
            .expect("should connect");

        let envelope = IpcEnvelope::new_stream_request(
            "test_actor",
            "TestStream",
            serde_json::json!({}),
        );
        let mut stream_rx = client
            .request_stream(envelope)
            .await
            .expect("should start stream");

        // The rejection arrives as a synthesized final error frame
        let frame = tokio::time::timeout(Duration::from_secs(2), stream_rx.recv())
            .await
            .expect("stream should terminate, not hang")
            .expect("should receive the rejection frame");
        assert!(frame.is_final);
        assert_eq!(frame.error_code.as_deref(), Some("RATE_LIMITED"));
        assert!(frame.error.is_some());

        // The channel closes and the routing entry is released
        let next = tokio::time::timeout(Duration::from_secs(2), stream_rx.recv())
            .await
            .expect("stream should terminate, not hang");
        assert!(next.is_none());
        assert!(client.active_streams.is_empty());

        accept_handle.await.expect("server task failed");
    }

    /// Test that dropping the client terminates in-flight streams promptly
    /// instead of leaving consumers to wait out the inter-frame timeout.
    #[tokio::test]
    async fn drop_client_closes_stream_channel() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let socket_path = dir.path().join("test.sock");

        let listener = tokio::net::UnixListener::bind(&socket_path)
            .expect("failed to bind socket");

        let accept_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("failed to accept");
            let (mut reader, _writer) = stream.into_split();

            // Read the request but never send any frames
            let _ = read_frame(&mut reader, MAX_FRAME_SIZE).await;
            tokio::time::sleep(Duration::from_millis(500)).await;
        });

        let client = IpcClient::connect(&socket_path)
            .await
            .expect("should connect");

        let envelope = IpcEnvelope::new_stream_request(
            "test_actor",
            "TestStream",
            serde_json::json!({}),
        );
        let mut stream_rx = client
            .request_stream(envelope)
            .await
            .expect("should start stream");

        // Drop the client with the stream still in flight. The default
        // inter-frame timeout is 30s, so a prompt close proves the Drop
        // cleanup (not the timeout) terminated the stream.
        drop(client);

        let result = tokio::time::timeout(Duration::from_secs(2), stream_rx.recv())
            .await
            .expect("stream should close promptly after drop, not hang");
        assert!(result.is_none());

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
