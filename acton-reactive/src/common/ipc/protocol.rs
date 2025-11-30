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

//! Wire protocol implementation for IPC message framing.
//!
//! This module defines the binary framing protocol used for IPC communication.
//! Messages are length-prefixed with a header containing protocol version,
//! message type, and serialization format.
//!
//! # Wire Format (Protocol v2)
//!
//! ```text
//! ┌───────────────────────────────────────────────────────────────┐
//! │ Frame Length (4 bytes, big-endian u32, excludes header)       │
//! ├───────────────────────────────────────────────────────────────┤
//! │ Protocol Version (1 byte, currently 0x02)                     │
//! ├───────────────────────────────────────────────────────────────┤
//! │ Message Type (1 byte)                                         │
//! │   0x01 = Request                                              │
//! │   0x02 = Response                                             │
//! │   0x03 = Error                                                │
//! │   0x04 = Heartbeat                                            │
//! ├───────────────────────────────────────────────────────────────┤
//! │ Format (1 byte)                                               │
//! │   0x01 = JSON                                                 │
//! │   0x02 = MessagePack                                          │
//! ├───────────────────────────────────────────────────────────────┤
//! │ Payload (remaining bytes, encoding depends on format)         │
//! └───────────────────────────────────────────────────────────────┘
//! ```

use super::types::{IpcEnvelope, IpcError, IpcResponse};
use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Protocol version byte.
pub const PROTOCOL_VERSION: u8 = 0x02;

// ============================================================================
// Serialization Format
// ============================================================================

/// Serialization format for IPC payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    /// JSON format (UTF-8 encoded, human-readable).
    #[default]
    Json,
    /// `MessagePack` format (binary, compact).
    #[cfg(feature = "ipc-messagepack")]
    MessagePack,
}

impl Format {
    /// Format byte for JSON.
    pub const JSON_BYTE: u8 = 0x01;
    /// Format byte for `MessagePack`.
    pub const MESSAGEPACK_BYTE: u8 = 0x02;

    /// Convert format to wire byte.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        match self {
            Self::Json => Self::JSON_BYTE,
            #[cfg(feature = "ipc-messagepack")]
            Self::MessagePack => Self::MESSAGEPACK_BYTE,
        }
    }

    /// Parse format from wire byte.
    #[must_use]
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            Self::JSON_BYTE => Some(Self::Json),
            #[cfg(feature = "ipc-messagepack")]
            Self::MESSAGEPACK_BYTE => Some(Self::MessagePack),
            _ => None,
        }
    }

    /// Serialize a value using this format.
    pub fn serialize<T: Serialize>(self, value: &T) -> Result<Vec<u8>, IpcError> {
        match self {
            Self::Json => serde_json::to_vec(value).map_err(IpcError::from),
            #[cfg(feature = "ipc-messagepack")]
            Self::MessagePack => rmp_serde::to_vec(value).map_err(|e| {
                IpcError::SerializationError(format!("MessagePack serialization failed: {e}"))
            }),
        }
    }

    /// Deserialize a value using this format.
    pub fn deserialize<T: DeserializeOwned>(self, bytes: &[u8]) -> Result<T, IpcError> {
        match self {
            Self::Json => serde_json::from_slice(bytes).map_err(IpcError::from),
            #[cfg(feature = "ipc-messagepack")]
            Self::MessagePack => rmp_serde::from_slice(bytes).map_err(|e| {
                IpcError::SerializationError(format!("MessagePack deserialization failed: {e}"))
            }),
        }
    }
}

/// Message type: Request (client → server).
pub const MSG_TYPE_REQUEST: u8 = 0x01;

/// Message type: Response (server → client).
pub const MSG_TYPE_RESPONSE: u8 = 0x02;

/// Message type: Error response (server → client).
pub const MSG_TYPE_ERROR: u8 = 0x03;

/// Message type: Heartbeat (bidirectional).
pub const MSG_TYPE_HEARTBEAT: u8 = 0x04;

/// Message type: Push notification (server → client, for broker subscription forwarding).
pub const MSG_TYPE_PUSH: u8 = 0x05;

/// Message type: Subscribe request (client → server, for broker subscriptions).
pub const MSG_TYPE_SUBSCRIBE: u8 = 0x06;

/// Message type: Unsubscribe request (client → server, for broker subscriptions).
pub const MSG_TYPE_UNSUBSCRIBE: u8 = 0x07;

/// Message type: Discovery request (client → server, for agent/type discovery).
pub const MSG_TYPE_DISCOVER: u8 = 0x08;

/// Message type: Stream frame (server → client, for streaming responses).
pub const MSG_TYPE_STREAM: u8 = 0x09;

/// Frame header size: 4 bytes length + 1 byte version + 1 byte type + 1 byte format.
pub const HEADER_SIZE: usize = 7;

/// Maximum frame size (16 MiB hard limit).
pub const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

/// Read a frame header from the stream.
///
/// Returns `(payload_length, protocol_version, message_type, format)`.
async fn read_header<R>(reader: &mut R) -> Result<(u32, u8, u8, Format), IpcError>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0u8; HEADER_SIZE];
    reader.read_exact(&mut header).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            IpcError::ConnectionClosed
        } else {
            IpcError::IoError(e.to_string())
        }
    })?;

    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    let version = header[4];
    let msg_type = header[5];
    let format_byte = header[6];

    // Validate protocol version
    if version != PROTOCOL_VERSION {
        return Err(IpcError::ProtocolError(format!(
            "Unsupported protocol version: {version}, expected {PROTOCOL_VERSION}"
        )));
    }

    // Validate message type
    if !matches!(
        msg_type,
        MSG_TYPE_REQUEST
            | MSG_TYPE_RESPONSE
            | MSG_TYPE_ERROR
            | MSG_TYPE_HEARTBEAT
            | MSG_TYPE_PUSH
            | MSG_TYPE_SUBSCRIBE
            | MSG_TYPE_UNSUBSCRIBE
            | MSG_TYPE_DISCOVER
            | MSG_TYPE_STREAM
    ) {
        return Err(IpcError::ProtocolError(format!(
            "Unknown message type: {msg_type:#04x}"
        )));
    }

    // Parse format
    let format = Format::from_byte(format_byte).ok_or_else(|| {
        IpcError::ProtocolError(format!("Unknown serialization format: {format_byte:#04x}"))
    })?;

    Ok((length, version, msg_type, format))
}

/// Write a frame header to the stream.
async fn write_header<W>(
    writer: &mut W,
    payload_length: u32,
    msg_type: u8,
    format: Format,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    let mut header = [0u8; HEADER_SIZE];
    header[..4].copy_from_slice(&payload_length.to_be_bytes());
    header[4] = PROTOCOL_VERSION;
    header[5] = msg_type;
    header[6] = format.to_byte();

    writer
        .write_all(&header)
        .await
        .map_err(|e| IpcError::IoError(e.to_string()))
}

/// Read a complete frame from the stream.
///
/// Returns the message type, format, and payload bytes.
pub async fn read_frame<R>(
    reader: &mut R,
    max_size: usize,
) -> Result<(u8, Format, Vec<u8>), IpcError>
where
    R: AsyncRead + Unpin,
{
    let (length, _version, msg_type, format) = read_header(reader).await?;

    let length_usize = length as usize;

    // Validate frame size
    if length_usize > max_size {
        return Err(IpcError::ProtocolError(format!(
            "Frame size {length_usize} exceeds maximum {max_size}"
        )));
    }

    if length_usize > MAX_FRAME_SIZE {
        return Err(IpcError::ProtocolError(format!(
            "Frame size {length_usize} exceeds hard limit {MAX_FRAME_SIZE}"
        )));
    }

    // Read payload
    let mut payload = vec![0u8; length_usize];
    reader.read_exact(&mut payload).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            IpcError::ConnectionClosed
        } else {
            IpcError::IoError(e.to_string())
        }
    })?;

    Ok((msg_type, format, payload))
}

/// Write a frame to the stream using the specified format.
pub async fn write_frame<W>(
    writer: &mut W,
    msg_type: u8,
    format: Format,
    payload: &[u8],
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    let length: u32 = payload
        .len()
        .try_into()
        .map_err(|_| IpcError::ProtocolError("Payload too large for u32".to_string()))?;

    write_header(writer, length, msg_type, format).await?;
    writer
        .write_all(payload)
        .await
        .map_err(|e| IpcError::IoError(e.to_string()))?;
    writer
        .flush()
        .await
        .map_err(|e| IpcError::IoError(e.to_string()))?;

    Ok(())
}

/// Read an IPC envelope (request) from the stream.
///
/// Returns the envelope and the format it was encoded in (so responses can match).
pub async fn read_envelope<R>(
    reader: &mut R,
    max_size: usize,
) -> Result<(IpcEnvelope, Format), IpcError>
where
    R: AsyncRead + Unpin,
{
    let (msg_type, format, payload) = read_frame(reader, max_size).await?;

    if msg_type != MSG_TYPE_REQUEST {
        return Err(IpcError::ProtocolError(format!(
            "Expected request message type ({MSG_TYPE_REQUEST:#04x}), got {msg_type:#04x}"
        )));
    }

    let envelope = format.deserialize(&payload)?;
    Ok((envelope, format))
}

/// Write an IPC envelope (request) to the stream using JSON format.
pub async fn write_envelope<W>(
    writer: &mut W,
    envelope: &IpcEnvelope,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    write_envelope_with_format(writer, envelope, Format::Json).await
}

/// Write an IPC envelope (request) to the stream using the specified format.
pub async fn write_envelope_with_format<W>(
    writer: &mut W,
    envelope: &IpcEnvelope,
    format: Format,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    let payload = format.serialize(envelope)?;
    write_frame(writer, MSG_TYPE_REQUEST, format, &payload).await
}

/// Read an IPC response from the stream.
pub async fn read_response<R>(
    reader: &mut R,
    max_size: usize,
) -> Result<IpcResponse, IpcError>
where
    R: AsyncRead + Unpin,
{
    let (msg_type, format, payload) = read_frame(reader, max_size).await?;

    if !matches!(msg_type, MSG_TYPE_RESPONSE | MSG_TYPE_ERROR) {
        return Err(IpcError::ProtocolError(format!(
            "Expected response message type, got {msg_type:#04x}"
        )));
    }

    format.deserialize(&payload)
}

/// Write an IPC response to the stream using JSON format.
pub async fn write_response<W>(
    writer: &mut W,
    response: &IpcResponse,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    write_response_with_format(writer, response, Format::Json).await
}

/// Write an IPC response to the stream using the specified format.
pub async fn write_response_with_format<W>(
    writer: &mut W,
    response: &IpcResponse,
    format: Format,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    let msg_type = if response.success {
        MSG_TYPE_RESPONSE
    } else {
        MSG_TYPE_ERROR
    };
    let payload = format.serialize(response)?;
    write_frame(writer, msg_type, format, &payload).await
}

/// Write a heartbeat message to the stream.
pub async fn write_heartbeat<W>(writer: &mut W) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    write_frame(writer, MSG_TYPE_HEARTBEAT, Format::Json, &[]).await
}

/// Check if a message type is a heartbeat.
#[must_use]
pub const fn is_heartbeat(msg_type: u8) -> bool {
    msg_type == MSG_TYPE_HEARTBEAT
}

/// Check if a message type is a subscribe request.
#[must_use]
pub const fn is_subscribe(msg_type: u8) -> bool {
    msg_type == MSG_TYPE_SUBSCRIBE
}

/// Check if a message type is an unsubscribe request.
#[must_use]
pub const fn is_unsubscribe(msg_type: u8) -> bool {
    msg_type == MSG_TYPE_UNSUBSCRIBE
}

/// Write a push notification to the stream using JSON format.
///
/// Push notifications are sent from the server to clients for broker subscription
/// forwarding. They carry messages that were broadcast internally and match the
/// client's subscriptions.
pub async fn write_push<W>(
    writer: &mut W,
    push: &super::types::IpcPushNotification,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    write_push_with_format(writer, push, Format::Json).await
}

/// Write a push notification to the stream using the specified format.
pub async fn write_push_with_format<W>(
    writer: &mut W,
    push: &super::types::IpcPushNotification,
    format: Format,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    let payload = format.serialize(push)?;
    write_frame(writer, MSG_TYPE_PUSH, format, &payload).await
}

/// Read a push notification from the stream.
///
/// This is used by IPC clients to receive push notifications from the server.
#[allow(dead_code)]
pub async fn read_push<R>(
    reader: &mut R,
    max_size: usize,
) -> Result<super::types::IpcPushNotification, IpcError>
where
    R: AsyncRead + Unpin,
{
    let (msg_type, format, payload) = read_frame(reader, max_size).await?;

    if msg_type != MSG_TYPE_PUSH {
        return Err(IpcError::ProtocolError(format!(
            "Expected push message type ({MSG_TYPE_PUSH:#04x}), got {msg_type:#04x}"
        )));
    }

    format.deserialize(&payload)
}

/// Write a subscribe request to the stream using JSON format.
///
/// This is used by IPC clients to send subscription requests to the server.
#[allow(dead_code)]
pub async fn write_subscribe<W>(
    writer: &mut W,
    request: &super::types::IpcSubscribeRequest,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    write_subscribe_with_format(writer, request, Format::Json).await
}

/// Write a subscribe request to the stream using the specified format.
#[allow(dead_code)]
pub async fn write_subscribe_with_format<W>(
    writer: &mut W,
    request: &super::types::IpcSubscribeRequest,
    format: Format,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    let payload = format.serialize(request)?;
    write_frame(writer, MSG_TYPE_SUBSCRIBE, format, &payload).await
}

/// Write an unsubscribe request to the stream using JSON format.
///
/// This is used by IPC clients to send unsubscription requests to the server.
#[allow(dead_code)]
pub async fn write_unsubscribe<W>(
    writer: &mut W,
    request: &super::types::IpcUnsubscribeRequest,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    write_unsubscribe_with_format(writer, request, Format::Json).await
}

/// Write an unsubscribe request to the stream using the specified format.
#[allow(dead_code)]
pub async fn write_unsubscribe_with_format<W>(
    writer: &mut W,
    request: &super::types::IpcUnsubscribeRequest,
    format: Format,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    let payload = format.serialize(request)?;
    write_frame(writer, MSG_TYPE_UNSUBSCRIBE, format, &payload).await
}

/// Write a subscription response to the stream using JSON format.
///
/// Subscription responses are sent in reply to subscribe/unsubscribe requests.
/// They use the standard `MSG_TYPE_RESPONSE` message type since they are
/// responses to client-initiated requests.
pub async fn write_subscription_response<W>(
    writer: &mut W,
    response: &super::types::IpcSubscriptionResponse,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    write_subscription_response_with_format(writer, response, Format::Json).await
}

/// Write a subscription response to the stream using the specified format.
pub async fn write_subscription_response_with_format<W>(
    writer: &mut W,
    response: &super::types::IpcSubscriptionResponse,
    format: Format,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    let msg_type = if response.success {
        MSG_TYPE_RESPONSE
    } else {
        MSG_TYPE_ERROR
    };
    let payload = format.serialize(response)?;
    write_frame(writer, msg_type, format, &payload).await
}

/// Check if a message type is a discovery request.
#[must_use]
pub const fn is_discover(msg_type: u8) -> bool {
    msg_type == MSG_TYPE_DISCOVER
}

/// Write a discovery request to the stream using JSON format.
///
/// This is used by IPC clients to discover available agents and message types.
#[allow(dead_code)]
pub async fn write_discover<W>(
    writer: &mut W,
    request: &super::types::IpcDiscoverRequest,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    write_discover_with_format(writer, request, Format::Json).await
}

/// Write a discovery request to the stream using the specified format.
#[allow(dead_code)]
pub async fn write_discover_with_format<W>(
    writer: &mut W,
    request: &super::types::IpcDiscoverRequest,
    format: Format,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    let payload = format.serialize(request)?;
    write_frame(writer, MSG_TYPE_DISCOVER, format, &payload).await
}

/// Write a discovery response to the stream using JSON format.
///
/// Discovery responses are sent in reply to discovery requests.
/// They use the standard `MSG_TYPE_RESPONSE` message type since they are
/// responses to client-initiated requests.
pub async fn write_discovery_response<W>(
    writer: &mut W,
    response: &super::types::IpcDiscoverResponse,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    write_discovery_response_with_format(writer, response, Format::Json).await
}

/// Write a discovery response to the stream using the specified format.
pub async fn write_discovery_response_with_format<W>(
    writer: &mut W,
    response: &super::types::IpcDiscoverResponse,
    format: Format,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    let msg_type = if response.success {
        MSG_TYPE_RESPONSE
    } else {
        MSG_TYPE_ERROR
    };
    let payload = format.serialize(response)?;
    write_frame(writer, msg_type, format, &payload).await
}

// ============================================================================
// Stream Frame Functions
// ============================================================================

/// Check if a message type is a stream frame.
#[must_use]
pub const fn is_stream(msg_type: u8) -> bool {
    msg_type == MSG_TYPE_STREAM
}

/// Write a stream frame to the stream using JSON format.
///
/// Stream frames are used for streaming responses where a single request
/// results in multiple response messages.
pub async fn write_stream_frame<W>(
    writer: &mut W,
    frame: &super::types::IpcStreamFrame,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    write_stream_frame_with_format(writer, frame, Format::Json).await
}

/// Write a stream frame to the stream using the specified format.
pub async fn write_stream_frame_with_format<W>(
    writer: &mut W,
    frame: &super::types::IpcStreamFrame,
    format: Format,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    let payload = format.serialize(frame)?;
    write_frame(writer, MSG_TYPE_STREAM, format, &payload).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn test_write_read_frame() {
        let mut buffer = Vec::new();
        let payload = b"test payload";

        // Write frame
        write_frame(&mut buffer, MSG_TYPE_REQUEST, Format::Json, payload)
            .await
            .unwrap();

        // Read frame
        let mut reader = Cursor::new(buffer);
        let (msg_type, format, read_payload) = read_frame(&mut reader, 1024).await.unwrap();

        assert_eq!(msg_type, MSG_TYPE_REQUEST);
        assert_eq!(format, Format::Json);
        assert_eq!(read_payload, payload);
    }

    #[tokio::test]
    async fn test_write_read_envelope() {
        let mut buffer = Vec::new();
        let envelope = IpcEnvelope::with_correlation_id(
            "test_123",
            "agent",
            "TestMessage",
            serde_json::json!({ "value": 42 }),
        );

        // Write envelope
        write_envelope(&mut buffer, &envelope).await.unwrap();

        // Read envelope
        let mut reader = Cursor::new(buffer);
        let (read_envelope, format) = read_envelope(&mut reader, 1024).await.unwrap();

        assert_eq!(format, Format::Json);
        assert_eq!(read_envelope.correlation_id, "test_123");
        assert_eq!(read_envelope.target, "agent");
        assert_eq!(read_envelope.message_type, "TestMessage");
    }

    #[tokio::test]
    async fn test_write_read_response() {
        let mut buffer = Vec::new();
        let response = IpcResponse::success(
            "test_123",
            Some(serde_json::json!({ "result": "ok" })),
        );

        // Write response
        write_response(&mut buffer, &response).await.unwrap();

        // Read response
        let mut reader = Cursor::new(buffer);
        let read_response = read_response(&mut reader, 1024).await.unwrap();

        assert!(read_response.success);
        assert_eq!(read_response.correlation_id, "test_123");
    }

    #[tokio::test]
    async fn test_invalid_protocol_version() {
        // Craft a frame with invalid protocol version
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&4u32.to_be_bytes()); // Length
        buffer.push(0xFF); // Invalid version
        buffer.push(MSG_TYPE_REQUEST);
        buffer.push(Format::JSON_BYTE); // Format
        buffer.extend_from_slice(b"test");

        let mut reader = Cursor::new(buffer);
        let result = read_frame(&mut reader, 1024).await;

        assert!(matches!(result, Err(IpcError::ProtocolError(_))));
    }

    #[tokio::test]
    async fn test_invalid_message_type() {
        // Craft a frame with invalid message type
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&4u32.to_be_bytes()); // Length
        buffer.push(PROTOCOL_VERSION);
        buffer.push(0xFF); // Invalid type
        buffer.push(Format::JSON_BYTE); // Format
        buffer.extend_from_slice(b"test");

        let mut reader = Cursor::new(buffer);
        let result = read_frame(&mut reader, 1024).await;

        assert!(matches!(result, Err(IpcError::ProtocolError(_))));
    }

    #[tokio::test]
    async fn test_invalid_format() {
        // Craft a frame with invalid format
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&4u32.to_be_bytes()); // Length
        buffer.push(PROTOCOL_VERSION);
        buffer.push(MSG_TYPE_REQUEST);
        buffer.push(0xFF); // Invalid format
        buffer.extend_from_slice(b"test");

        let mut reader = Cursor::new(buffer);
        let result = read_frame(&mut reader, 1024).await;

        assert!(matches!(result, Err(IpcError::ProtocolError(_))));
    }

    #[tokio::test]
    async fn test_frame_too_large() {
        // Craft a frame that exceeds max size
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&10000u32.to_be_bytes()); // Large length
        buffer.push(PROTOCOL_VERSION);
        buffer.push(MSG_TYPE_REQUEST);
        buffer.push(Format::JSON_BYTE); // Format
        // Don't add payload - we'll fail on size check

        let mut reader = Cursor::new(buffer);
        let result = read_frame(&mut reader, 100).await; // Small max size

        assert!(matches!(result, Err(IpcError::ProtocolError(_))));
    }

    #[tokio::test]
    async fn test_heartbeat() {
        let mut buffer = Vec::new();

        // Write heartbeat
        write_heartbeat(&mut buffer).await.unwrap();

        // Read frame
        let mut reader = Cursor::new(buffer);
        let (msg_type, _format, payload) = read_frame(&mut reader, 1024).await.unwrap();

        assert!(is_heartbeat(msg_type));
        assert!(payload.is_empty());
    }

    #[tokio::test]
    async fn test_connection_closed_on_partial_read() {
        // Empty buffer simulates connection closed
        let mut reader = Cursor::new(Vec::<u8>::new());
        let result = read_frame(&mut reader, 1024).await;

        assert!(matches!(result, Err(IpcError::ConnectionClosed)));
    }

    #[test]
    fn test_header_constants() {
        assert_eq!(HEADER_SIZE, 7);
        assert_eq!(PROTOCOL_VERSION, 0x02);
        assert_eq!(MSG_TYPE_REQUEST, 0x01);
        assert_eq!(MSG_TYPE_RESPONSE, 0x02);
        assert_eq!(MSG_TYPE_ERROR, 0x03);
        assert_eq!(MSG_TYPE_HEARTBEAT, 0x04);
    }

    #[test]
    fn test_format_roundtrip() {
        assert_eq!(Format::from_byte(Format::Json.to_byte()), Some(Format::Json));
        #[cfg(feature = "ipc-messagepack")]
        assert_eq!(
            Format::from_byte(Format::MessagePack.to_byte()),
            Some(Format::MessagePack)
        );
        assert_eq!(Format::from_byte(0xFF), None);
    }

    #[test]
    fn test_subscription_message_type_helpers() {
        assert!(is_subscribe(MSG_TYPE_SUBSCRIBE));
        assert!(!is_subscribe(MSG_TYPE_REQUEST));
        assert!(!is_subscribe(MSG_TYPE_UNSUBSCRIBE));

        assert!(is_unsubscribe(MSG_TYPE_UNSUBSCRIBE));
        assert!(!is_unsubscribe(MSG_TYPE_REQUEST));
        assert!(!is_unsubscribe(MSG_TYPE_SUBSCRIBE));
    }

    #[tokio::test]
    async fn test_write_read_push_notification() {
        use super::super::types::IpcPushNotification;

        let mut buffer = Vec::new();
        let notification = IpcPushNotification::new(
            "PriceUpdate",
            Some("price_service".to_string()),
            serde_json::json!({ "price": 100.50 }),
        );

        // Write push notification
        write_push(&mut buffer, &notification).await.unwrap();

        // Read push notification
        let mut reader = Cursor::new(buffer);
        let read_notification = read_push(&mut reader, 1024).await.unwrap();

        assert_eq!(read_notification.message_type, "PriceUpdate");
        assert_eq!(read_notification.source_agent, Some("price_service".to_string()));
        assert_eq!(read_notification.payload["price"], 100.50);
    }

    #[tokio::test]
    async fn test_write_subscribe_request() {
        use super::super::types::IpcSubscribeRequest;

        let mut buffer = Vec::new();
        let request = IpcSubscribeRequest::new(vec![
            "PriceUpdate".to_string(),
            "OrderStatus".to_string(),
        ]);

        // Write subscribe request
        write_subscribe(&mut buffer, &request).await.unwrap();

        // Verify the frame was written with correct message type
        let mut reader = Cursor::new(buffer);
        let (msg_type, format, payload) = read_frame(&mut reader, 1024).await.unwrap();

        assert!(is_subscribe(msg_type));
        assert_eq!(format, Format::Json);
        let parsed: IpcSubscribeRequest = serde_json::from_slice(&payload).unwrap();
        assert_eq!(parsed.message_types.len(), 2);
        assert!(parsed.message_types.contains(&"PriceUpdate".to_string()));
    }

    #[tokio::test]
    async fn test_write_unsubscribe_request() {
        use super::super::types::IpcUnsubscribeRequest;

        let mut buffer = Vec::new();
        let request = IpcUnsubscribeRequest::new(vec!["PriceUpdate".to_string()]);

        // Write unsubscribe request
        write_unsubscribe(&mut buffer, &request).await.unwrap();

        // Verify the frame was written with correct message type
        let mut reader = Cursor::new(buffer);
        let (msg_type, format, payload) = read_frame(&mut reader, 1024).await.unwrap();

        assert!(is_unsubscribe(msg_type));
        assert_eq!(format, Format::Json);
        let parsed: IpcUnsubscribeRequest = serde_json::from_slice(&payload).unwrap();
        assert_eq!(parsed.message_types.len(), 1);
        assert_eq!(parsed.message_types[0], "PriceUpdate");
    }

    #[tokio::test]
    async fn test_unsubscribe_all() {
        use super::super::types::IpcUnsubscribeRequest;

        let mut buffer = Vec::new();
        let request = IpcUnsubscribeRequest::unsubscribe_all();

        write_unsubscribe(&mut buffer, &request).await.unwrap();

        let mut reader = Cursor::new(buffer);
        let (msg_type, format, payload) = read_frame(&mut reader, 1024).await.unwrap();

        assert!(is_unsubscribe(msg_type));
        assert_eq!(format, Format::Json);
        let parsed: IpcUnsubscribeRequest = serde_json::from_slice(&payload).unwrap();
        assert!(parsed.message_types.is_empty());
    }
}
