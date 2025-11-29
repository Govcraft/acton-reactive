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
///   "payload": { "symbol": "AAPL", "price": 150.25 }
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
}

impl IpcEnvelope {
    /// Creates a new IPC envelope with a generated correlation ID.
    ///
    /// Uses the MTI crate to generate a unique, time-ordered correlation ID
    /// in the format `req_<uuid_v7>`.
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
        }
    }

    /// Creates a new IPC envelope with a specified correlation ID.
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
        }
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
}
