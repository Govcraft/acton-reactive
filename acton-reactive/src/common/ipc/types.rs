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

//! IPC-specific types for message serialization and error handling.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Error types for IPC operations.
///
/// These errors can occur during message serialization, deserialization,
/// routing, or transport operations.
#[derive(Debug, Clone)]
pub enum IpcError {
    /// Message type not registered in the type registry.
    ///
    /// Before an IPC message can be deserialized, its type must be registered
    /// with [`IpcTypeRegistry::register`](super::IpcTypeRegistry::register).
    UnknownMessageType(String),

    /// Target agent not found in the agent registry.
    ///
    /// The target agent must be exposed via
    /// [`AgentRuntime::ipc_expose`](crate::common::AgentRuntime::ipc_expose)
    /// before it can receive IPC messages.
    AgentNotFound(String),

    /// Serialization or deserialization failure.
    ///
    /// Contains the underlying error message from the serialization library.
    SerializationError(String),

    /// Target agent's inbox is full.
    ///
    /// The agent's message channel has reached capacity. The sender should
    /// implement backoff and retry logic.
    TargetBusy,

    /// Connection was closed unexpectedly.
    ConnectionClosed,

    /// Protocol error (invalid frame, unsupported version, etc.).
    ProtocolError(String),

    /// Socket or I/O error.
    IoError(String),

    /// Request timeout exceeded.
    Timeout,

    /// Rate limit exceeded for connection.
    ///
    /// The connection has exceeded its allowed request rate. The client should
    /// implement backoff and retry logic or reduce request frequency.
    RateLimited {
        /// Time in milliseconds until the rate limit resets.
        retry_after_ms: u64,
    },

    /// Server is shutting down.
    ///
    /// The server is gracefully shutting down and not accepting new requests.
    ShuttingDown,
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownMessageType(t) => write!(f, "Unknown message type: {t}"),
            Self::AgentNotFound(a) => write!(f, "Agent not found: {a}"),
            Self::SerializationError(e) => write!(f, "Serialization error: {e}"),
            Self::TargetBusy => write!(f, "Target agent inbox is full"),
            Self::ConnectionClosed => write!(f, "Connection closed"),
            Self::ProtocolError(e) => write!(f, "Protocol error: {e}"),
            Self::IoError(e) => write!(f, "I/O error: {e}"),
            Self::Timeout => write!(f, "Request timeout"),
            Self::RateLimited { retry_after_ms } => {
                write!(f, "Rate limit exceeded, retry after {retry_after_ms}ms")
            }
            Self::ShuttingDown => write!(f, "Server is shutting down"),
        }
    }
}

impl std::error::Error for IpcError {}

impl From<serde_json::Error> for IpcError {
    fn from(err: serde_json::Error) -> Self {
        Self::SerializationError(err.to_string())
    }
}

impl From<std::io::Error> for IpcError {
    fn from(err: std::io::Error) -> Self {
        Self::IoError(err.to_string())
    }
}

/// Inbound IPC message envelope.
///
/// This is the standard format for messages sent to an acton-reactive application
/// from external processes. The envelope contains all metadata needed to route
/// and deserialize the message.
///
/// # Wire Format
///
/// When serialized to JSON:
///
/// ```json
/// {
///   "correlation_id": "req_01h9xz7n2e5p6q8r3t1u2v3w4x",
///   "target": "price_service",
///   "message_type": "PriceUpdate",
///   "payload": { "symbol": "AAPL", "price": 150.25 },
///   "expects_reply": true,
///   "response_timeout_ms": 5000
/// }
/// ```
///
/// # Fields
///
/// - `correlation_id`: Unique identifier for request-response correlation.
///   Uses MTI format (e.g., `req_<uuid_v7>`) for time-ordered, unique IDs.
///
/// - `target`: The logical name of the target agent (as registered via
///   [`AgentRuntime::ipc_expose`](crate::common::AgentRuntime::ipc_expose)),
///   or a full ERN string for direct addressing.
///
/// - `message_type`: The registered type name used to look up the deserializer
///   in [`IpcTypeRegistry`](super::IpcTypeRegistry).
///
/// - `payload`: The serialized message data as a JSON value.
///
/// - `expects_reply` (optional): Whether the client expects a response from the
///   agent. When `true`, the IPC listener will wait for the agent to send a
///   reply using `reply_envelope.send()` and forward it back to the client.
///   Defaults to `false` for fire-and-forget messages.
///
/// - `response_timeout_ms` (optional): Maximum time in milliseconds to wait for
///   a response when `expects_reply` is `true`. Defaults to 30000 (30 seconds).
///   If the timeout expires, an error response is returned.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IpcEnvelope {
    /// Correlation ID for request-response tracking (MTI format recommended).
    pub correlation_id: String,

    /// Target agent (logical name or ERN string).
    pub target: String,

    /// Message type name (for deserialization lookup).
    pub message_type: String,

    /// Serialized payload.
    pub payload: serde_json::Value,

    /// Whether the client expects a response from the agent.
    ///
    /// When `true`, the IPC listener creates a temporary channel to receive
    /// the agent's response and waits for it (up to `response_timeout_ms`).
    /// The agent should use `reply_envelope.send(response)` to send the reply.
    ///
    /// Defaults to `false` for fire-and-forget messages.
    #[serde(default)]
    pub expects_reply: bool,

    /// Maximum time in milliseconds to wait for a response.
    ///
    /// Only used when `expects_reply` is `true`. Defaults to 30000 (30 seconds).
    #[serde(default = "default_response_timeout")]
    pub response_timeout_ms: u64,
}

/// Default response timeout: 30 seconds.
const fn default_response_timeout() -> u64 {
    30_000
}

impl IpcEnvelope {
    /// Creates a new fire-and-forget IPC envelope with a generated correlation ID.
    ///
    /// Uses the MTI crate to generate a unique, time-ordered correlation ID
    /// in the format `req_<uuid_v7>`. This creates a fire-and-forget message
    /// that does not expect a reply.
    ///
    /// # Arguments
    ///
    /// * `target` - The logical name or ERN of the target agent.
    /// * `message_type` - The registered type name of the message.
    /// * `payload` - The serialized message payload.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let envelope = IpcEnvelope::new(
    ///     "price_service",
    ///     "PriceUpdate",
    ///     serde_json::json!({ "symbol": "AAPL", "price": 150.25 }),
    /// );
    /// ```
    #[must_use]
    pub fn new(target: impl Into<String>, message_type: impl Into<String>, payload: serde_json::Value) -> Self {
        use mti::prelude::*;
        Self {
            correlation_id: "req".create_type_id::<V7>().to_string(),
            target: target.into(),
            message_type: message_type.into(),
            payload,
            expects_reply: false,
            response_timeout_ms: default_response_timeout(),
        }
    }

    /// Creates a new request-response IPC envelope with a generated correlation ID.
    ///
    /// Uses the MTI crate to generate a unique, time-ordered correlation ID
    /// in the format `req_<uuid_v7>`. This creates a request that expects a
    /// reply from the target agent.
    ///
    /// The agent should use `reply_envelope.send(response)` in their handler
    /// to send a reply back to the IPC client.
    ///
    /// # Arguments
    ///
    /// * `target` - The logical name or ERN of the target agent.
    /// * `message_type` - The registered type name of the message.
    /// * `payload` - The serialized message payload.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let envelope = IpcEnvelope::new_request(
    ///     "price_service",
    ///     "GetPrice",
    ///     serde_json::json!({ "symbol": "AAPL" }),
    /// );
    /// // The IPC listener will wait for the agent's response
    /// ```
    #[must_use]
    pub fn new_request(
        target: impl Into<String>,
        message_type: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        use mti::prelude::*;
        Self {
            correlation_id: "req".create_type_id::<V7>().to_string(),
            target: target.into(),
            message_type: message_type.into(),
            payload,
            expects_reply: true,
            response_timeout_ms: default_response_timeout(),
        }
    }

    /// Creates a new request-response IPC envelope with a custom timeout.
    ///
    /// # Arguments
    ///
    /// * `target` - The logical name or ERN of the target agent.
    /// * `message_type` - The registered type name of the message.
    /// * `payload` - The serialized message payload.
    /// * `timeout_ms` - Maximum time in milliseconds to wait for a response.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let envelope = IpcEnvelope::new_request_with_timeout(
    ///     "price_service",
    ///     "GetPrice",
    ///     serde_json::json!({ "symbol": "AAPL" }),
    ///     5000, // 5 second timeout
    /// );
    /// ```
    #[must_use]
    pub fn new_request_with_timeout(
        target: impl Into<String>,
        message_type: impl Into<String>,
        payload: serde_json::Value,
        timeout_ms: u64,
    ) -> Self {
        use mti::prelude::*;
        Self {
            correlation_id: "req".create_type_id::<V7>().to_string(),
            target: target.into(),
            message_type: message_type.into(),
            payload,
            expects_reply: true,
            response_timeout_ms: timeout_ms,
        }
    }

    /// Creates a new IPC envelope with a specified correlation ID (fire-and-forget).
    ///
    /// Use this when you need to control the correlation ID, such as when
    /// forwarding messages or implementing custom correlation schemes.
    #[must_use]
    pub fn with_correlation_id(
        correlation_id: impl Into<String>,
        target: impl Into<String>,
        message_type: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            correlation_id: correlation_id.into(),
            target: target.into(),
            message_type: message_type.into(),
            payload,
            expects_reply: false,
            response_timeout_ms: default_response_timeout(),
        }
    }

    /// Creates a new request-response IPC envelope with a specified correlation ID.
    ///
    /// Use this when you need to control the correlation ID while also expecting
    /// a response from the target agent.
    #[must_use]
    pub fn with_correlation_id_request(
        correlation_id: impl Into<String>,
        target: impl Into<String>,
        message_type: impl Into<String>,
        payload: serde_json::Value,
        timeout_ms: u64,
    ) -> Self {
        Self {
            correlation_id: correlation_id.into(),
            target: target.into(),
            message_type: message_type.into(),
            payload,
            expects_reply: true,
            response_timeout_ms: timeout_ms,
        }
    }

    /// Returns `true` if this envelope expects a reply from the target agent.
    #[must_use]
    pub const fn expects_reply(&self) -> bool {
        self.expects_reply
    }

    /// Returns the response timeout as a `Duration`.
    #[must_use]
    pub const fn response_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.response_timeout_ms)
    }
}

/// IPC response envelope.
///
/// This is the standard format for responses sent back to external processes.
/// The correlation ID matches the original request's correlation ID for
/// request-response pairing.
///
/// # Wire Format
///
/// Success response:
/// ```json
/// {
///   "correlation_id": "req_01h9xz7n2e5p6q8r3t1u2v3w4x",
///   "success": true,
///   "payload": { "acknowledged": true }
/// }
/// ```
///
/// Error response:
/// ```json
/// {
///   "correlation_id": "req_01h9xz7n2e5p6q8r3t1u2v3w4x",
///   "success": false,
///   "error": "Agent not found: price_service",
///   "error_code": "AGENT_NOT_FOUND"
/// }
/// ```
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IpcResponse {
    /// Correlation ID matching the request.
    pub correlation_id: String,

    /// Whether the request was successful.
    pub success: bool,

    /// Error message (if `success` is `false`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Machine-readable error code (if `success` is `false`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,

    /// Response payload (if `success` is `true`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

// ============================================================================
// Broker Subscription Types
// ============================================================================

/// Request to subscribe to broker broadcasts of specific message types.
///
/// When an IPC client subscribes to message types, any messages of those types
/// that are broadcast via the internal broker will be forwarded to the client
/// as [`IpcPushNotification`] messages.
///
/// # Wire Format
///
/// ```json
/// {
///   "correlation_id": "sub_01h9xz7n2e5p6q8r3t1u2v3w4x",
///   "message_types": ["PriceUpdate", "OrderStatus"]
/// }
/// ```
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IpcSubscribeRequest {
    /// Correlation ID for the subscribe request.
    pub correlation_id: String,

    /// List of message type names to subscribe to.
    ///
    /// These must match the names registered with [`IpcTypeRegistry`](super::IpcTypeRegistry).
    pub message_types: Vec<String>,
}

impl IpcSubscribeRequest {
    /// Creates a new subscribe request with a generated correlation ID.
    ///
    /// Used by IPC clients to create subscription requests.
    #[must_use]
    #[allow(dead_code)]
    pub fn new(message_types: Vec<String>) -> Self {
        use mti::prelude::*;
        Self {
            correlation_id: "sub".create_type_id::<V7>().to_string(),
            message_types,
        }
    }

    /// Creates a new subscribe request with a specified correlation ID.
    ///
    /// Used by IPC clients to create subscription requests with custom correlation IDs.
    #[must_use]
    #[allow(dead_code)]
    pub fn with_correlation_id(correlation_id: impl Into<String>, message_types: Vec<String>) -> Self {
        Self {
            correlation_id: correlation_id.into(),
            message_types,
        }
    }
}

/// Request to unsubscribe from broker broadcasts of specific message types.
///
/// # Wire Format
///
/// ```json
/// {
///   "correlation_id": "unsub_01h9xz7n2e5p6q8r3t1u2v3w4x",
///   "message_types": ["PriceUpdate"]
/// }
/// ```
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IpcUnsubscribeRequest {
    /// Correlation ID for the unsubscribe request.
    pub correlation_id: String,

    /// List of message type names to unsubscribe from.
    ///
    /// If empty, unsubscribes from all message types.
    pub message_types: Vec<String>,
}

impl IpcUnsubscribeRequest {
    /// Creates a new unsubscribe request with a generated correlation ID.
    ///
    /// Used by IPC clients to create unsubscription requests.
    #[must_use]
    #[allow(dead_code)]
    pub fn new(message_types: Vec<String>) -> Self {
        use mti::prelude::*;
        Self {
            correlation_id: "unsub".create_type_id::<V7>().to_string(),
            message_types,
        }
    }

    /// Creates a new unsubscribe request to unsubscribe from all message types.
    ///
    /// Used by IPC clients to unsubscribe from all broker broadcasts.
    #[must_use]
    #[allow(dead_code)]
    pub fn unsubscribe_all() -> Self {
        use mti::prelude::*;
        Self {
            correlation_id: "unsub".create_type_id::<V7>().to_string(),
            message_types: Vec::new(),
        }
    }
}

/// Push notification sent from server to client for broker subscription forwarding.
///
/// When internal agents broadcast messages via the broker, and an IPC client has
/// subscribed to that message type, the message is forwarded to the client as
/// a push notification.
///
/// # Wire Format
///
/// ```json
/// {
///   "notification_id": "push_01h9xz7n2e5p6q8r3t1u2v3w4x",
///   "message_type": "PriceUpdate",
///   "source_agent": "price_service",
///   "payload": { "symbol": "AAPL", "price": 150.25 }
/// }
/// ```
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IpcPushNotification {
    /// Unique ID for this notification (MTI format).
    pub notification_id: String,

    /// The message type name.
    pub message_type: String,

    /// The source agent that broadcast the message (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_agent: Option<String>,

    /// The serialized message payload.
    pub payload: serde_json::Value,

    /// Timestamp when the message was broadcast (Unix milliseconds).
    pub timestamp_ms: u64,
}

impl IpcPushNotification {
    /// Creates a new push notification.
    #[must_use]
    pub fn new(
        message_type: impl Into<String>,
        source_agent: Option<String>,
        payload: serde_json::Value,
    ) -> Self {
        use mti::prelude::*;
        use std::time::{SystemTime, UNIX_EPOCH};

        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));

        Self {
            notification_id: "push".create_type_id::<V7>().to_string(),
            message_type: message_type.into(),
            source_agent,
            payload,
            timestamp_ms,
        }
    }
}

/// Response to subscribe/unsubscribe requests.
///
/// # Wire Format
///
/// ```json
/// {
///   "correlation_id": "sub_01h9xz7n2e5p6q8r3t1u2v3w4x",
///   "success": true,
///   "subscribed_types": ["PriceUpdate", "OrderStatus"]
/// }
/// ```
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IpcSubscriptionResponse {
    /// Correlation ID matching the request.
    pub correlation_id: String,

    /// Whether the subscription operation succeeded.
    pub success: bool,

    /// Error message if `success` is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// List of message types the client is currently subscribed to after this operation.
    pub subscribed_types: Vec<String>,
}

impl IpcSubscriptionResponse {
    /// Creates a successful subscription response.
    #[must_use]
    pub fn success(correlation_id: impl Into<String>, subscribed_types: Vec<String>) -> Self {
        Self {
            correlation_id: correlation_id.into(),
            success: true,
            error: None,
            subscribed_types,
        }
    }

    /// Creates an error subscription response.
    #[must_use]
    pub fn error(correlation_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            correlation_id: correlation_id.into(),
            success: false,
            error: Some(error.into()),
            subscribed_types: Vec::new(),
        }
    }
}

impl IpcResponse {
    /// Creates a successful response with an optional payload.
    #[must_use]
    pub fn success(correlation_id: impl Into<String>, payload: Option<serde_json::Value>) -> Self {
        Self {
            correlation_id: correlation_id.into(),
            success: true,
            error: None,
            error_code: None,
            payload,
        }
    }

    /// Creates an error response from an [`IpcError`].
    #[must_use]
    pub fn error(correlation_id: impl Into<String>, err: &IpcError) -> Self {
        let (error_code, error_message) = match err {
            IpcError::UnknownMessageType(_) => ("UNKNOWN_MESSAGE_TYPE", err.to_string()),
            IpcError::AgentNotFound(_) => ("AGENT_NOT_FOUND", err.to_string()),
            IpcError::SerializationError(_) => ("SERIALIZATION_ERROR", err.to_string()),
            IpcError::TargetBusy => ("TARGET_BUSY", err.to_string()),
            IpcError::ConnectionClosed => ("CONNECTION_CLOSED", err.to_string()),
            IpcError::ProtocolError(_) => ("PROTOCOL_ERROR", err.to_string()),
            IpcError::IoError(_) => ("IO_ERROR", err.to_string()),
            IpcError::Timeout => ("TIMEOUT", err.to_string()),
            IpcError::RateLimited { .. } => ("RATE_LIMITED", err.to_string()),
            IpcError::ShuttingDown => ("SHUTTING_DOWN", err.to_string()),
        };

        Self {
            correlation_id: correlation_id.into(),
            success: false,
            error: Some(error_message),
            error_code: Some(error_code.to_string()),
            payload: None,
        }
    }

    /// Creates an error response with a custom message.
    #[must_use]
    pub fn error_with_message(
        correlation_id: impl Into<String>,
        error_code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            correlation_id: correlation_id.into(),
            success: false,
            error: Some(message.into()),
            error_code: Some(error_code.into()),
            payload: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipc_envelope_creation() {
        let envelope = IpcEnvelope::new(
            "price_service",
            "PriceUpdate",
            serde_json::json!({ "symbol": "AAPL", "price": 150.25 }),
        );

        assert!(envelope.correlation_id.starts_with("req_"));
        assert_eq!(envelope.target, "price_service");
        assert_eq!(envelope.message_type, "PriceUpdate");
        assert!(!envelope.expects_reply);
        assert_eq!(envelope.response_timeout_ms, 30_000);
    }

    #[test]
    fn test_ipc_envelope_request() {
        let envelope = IpcEnvelope::new_request(
            "price_service",
            "GetPrice",
            serde_json::json!({ "symbol": "AAPL" }),
        );

        assert!(envelope.correlation_id.starts_with("req_"));
        assert_eq!(envelope.target, "price_service");
        assert_eq!(envelope.message_type, "GetPrice");
        assert!(envelope.expects_reply);
        assert_eq!(envelope.response_timeout_ms, 30_000);
    }

    #[test]
    fn test_ipc_envelope_request_with_timeout() {
        let envelope = IpcEnvelope::new_request_with_timeout(
            "price_service",
            "GetPrice",
            serde_json::json!({ "symbol": "AAPL" }),
            5000,
        );

        assert!(envelope.expects_reply);
        assert_eq!(envelope.response_timeout_ms, 5000);
        assert_eq!(envelope.response_timeout(), std::time::Duration::from_millis(5000));
    }

    #[test]
    fn test_ipc_envelope_serialization() {
        let envelope = IpcEnvelope::with_correlation_id(
            "test_123",
            "agent",
            "Ping",
            serde_json::json!({ "value": 42 }),
        );

        let json = serde_json::to_string(&envelope).unwrap();
        let deserialized: IpcEnvelope = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.correlation_id, "test_123");
        assert_eq!(deserialized.target, "agent");
        assert_eq!(deserialized.message_type, "Ping");
        assert!(!deserialized.expects_reply);
    }

    #[test]
    fn test_ipc_envelope_deserialization_defaults() {
        // Test that expects_reply defaults to false when not present in JSON
        let json = r#"{
            "correlation_id": "test_123",
            "target": "agent",
            "message_type": "Ping",
            "payload": {}
        }"#;

        let deserialized: IpcEnvelope = serde_json::from_str(json).unwrap();
        assert!(!deserialized.expects_reply);
        assert_eq!(deserialized.response_timeout_ms, 30_000);
    }

    #[test]
    fn test_ipc_envelope_deserialization_with_expects_reply() {
        let json = r#"{
            "correlation_id": "test_123",
            "target": "agent",
            "message_type": "Query",
            "payload": {"q": "test"},
            "expects_reply": true,
            "response_timeout_ms": 5000
        }"#;

        let deserialized: IpcEnvelope = serde_json::from_str(json).unwrap();
        assert!(deserialized.expects_reply);
        assert_eq!(deserialized.response_timeout_ms, 5000);
    }

    #[test]
    fn test_ipc_response_success() {
        let response = IpcResponse::success("test_123", Some(serde_json::json!({ "ok": true })));

        assert!(response.success);
        assert!(response.error.is_none());
        assert!(response.payload.is_some());
    }

    #[test]
    fn test_ipc_response_error() {
        let err = IpcError::AgentNotFound("test_agent".to_string());
        let response = IpcResponse::error("test_123", &err);

        assert!(!response.success);
        assert_eq!(response.error_code, Some("AGENT_NOT_FOUND".to_string()));
        assert!(response.error.is_some());
        assert!(response.payload.is_none());
    }

    #[test]
    fn test_ipc_error_display() {
        let err = IpcError::UnknownMessageType("MyMessage".to_string());
        assert_eq!(err.to_string(), "Unknown message type: MyMessage");

        let err = IpcError::TargetBusy;
        assert_eq!(err.to_string(), "Target agent inbox is full");
    }

    // --- Subscription types tests ---

    #[test]
    fn test_ipc_subscribe_request_new() {
        let request = IpcSubscribeRequest::new(vec![
            "PriceUpdate".to_string(),
            "OrderStatus".to_string(),
        ]);

        assert!(request.correlation_id.starts_with("sub_"));
        assert_eq!(request.message_types.len(), 2);
        assert!(request.message_types.contains(&"PriceUpdate".to_string()));
        assert!(request.message_types.contains(&"OrderStatus".to_string()));
    }

    #[test]
    fn test_ipc_subscribe_request_with_correlation_id() {
        let request = IpcSubscribeRequest::with_correlation_id(
            "custom_123",
            vec!["MyMessage".to_string()],
        );

        assert_eq!(request.correlation_id, "custom_123");
        assert_eq!(request.message_types.len(), 1);
    }

    #[test]
    fn test_ipc_subscribe_request_serialization() {
        let request = IpcSubscribeRequest::with_correlation_id(
            "test_sub",
            vec!["TypeA".to_string(), "TypeB".to_string()],
        );

        let json = serde_json::to_string(&request).unwrap();
        let deserialized: IpcSubscribeRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.correlation_id, "test_sub");
        assert_eq!(deserialized.message_types, request.message_types);
    }

    #[test]
    fn test_ipc_unsubscribe_request_new() {
        let request = IpcUnsubscribeRequest::new(vec!["PriceUpdate".to_string()]);

        assert!(request.correlation_id.starts_with("unsub_"));
        assert_eq!(request.message_types.len(), 1);
    }

    #[test]
    fn test_ipc_unsubscribe_request_unsubscribe_all() {
        let request = IpcUnsubscribeRequest::unsubscribe_all();

        assert!(request.correlation_id.starts_with("unsub_"));
        assert!(request.message_types.is_empty());
    }

    #[test]
    fn test_ipc_unsubscribe_request_serialization() {
        let request = IpcUnsubscribeRequest::unsubscribe_all();

        let json = serde_json::to_string(&request).unwrap();
        let deserialized: IpcUnsubscribeRequest = serde_json::from_str(&json).unwrap();

        assert!(deserialized.message_types.is_empty());
    }

    #[test]
    fn test_ipc_push_notification_new() {
        let notification = IpcPushNotification::new(
            "PriceUpdate",
            Some("price_service".to_string()),
            serde_json::json!({ "symbol": "AAPL", "price": 150.25 }),
        );

        assert!(notification.notification_id.starts_with("push_"));
        assert_eq!(notification.message_type, "PriceUpdate");
        assert_eq!(notification.source_agent, Some("price_service".to_string()));
        assert!(notification.timestamp_ms > 0);
    }

    #[test]
    fn test_ipc_push_notification_no_source() {
        let notification = IpcPushNotification::new(
            "SystemEvent",
            None,
            serde_json::json!({ "event": "startup" }),
        );

        assert_eq!(notification.message_type, "SystemEvent");
        assert!(notification.source_agent.is_none());
    }

    #[test]
    fn test_ipc_push_notification_serialization() {
        let notification = IpcPushNotification::new(
            "TestMessage",
            Some("test_agent".to_string()),
            serde_json::json!({ "data": 123 }),
        );

        let json = serde_json::to_string(&notification).unwrap();
        let deserialized: IpcPushNotification = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.message_type, "TestMessage");
        assert_eq!(deserialized.source_agent, Some("test_agent".to_string()));
        assert_eq!(deserialized.payload["data"], 123);
    }

    #[test]
    fn test_ipc_subscription_response_success() {
        let response = IpcSubscriptionResponse::success(
            "sub_123",
            vec!["TypeA".to_string(), "TypeB".to_string()],
        );

        assert!(response.success);
        assert_eq!(response.correlation_id, "sub_123");
        assert!(response.error.is_none());
        assert_eq!(response.subscribed_types.len(), 2);
    }

    #[test]
    fn test_ipc_subscription_response_error() {
        let response = IpcSubscriptionResponse::error(
            "sub_456",
            "Connection not registered",
        );

        assert!(!response.success);
        assert_eq!(response.correlation_id, "sub_456");
        assert_eq!(response.error, Some("Connection not registered".to_string()));
        assert!(response.subscribed_types.is_empty());
    }

    #[test]
    fn test_ipc_subscription_response_serialization() {
        let response = IpcSubscriptionResponse::success(
            "test_corr",
            vec!["PriceUpdate".to_string()],
        );

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: IpcSubscriptionResponse = serde_json::from_str(&json).unwrap();

        assert!(deserialized.success);
        assert_eq!(deserialized.correlation_id, "test_corr");
        assert_eq!(deserialized.subscribed_types, vec!["PriceUpdate".to_string()]);
    }
}
