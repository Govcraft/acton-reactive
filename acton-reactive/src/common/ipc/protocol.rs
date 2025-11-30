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
//! Messages are length-prefixed with a header containing protocol version and
//! message type.
//!
//! # Wire Format
//!
//! ```text
//! ┌───────────────────────────────────────────────────────────────┐
//! │ Frame Length (4 bytes, big-endian u32, excludes header)       │
//! ├───────────────────────────────────────────────────────────────┤
//! │ Protocol Version (1 byte, currently 0x01)                     │
//! ├───────────────────────────────────────────────────────────────┤
//! │ Message Type (1 byte)                                         │
//! │   0x01 = Request                                              │
//! │   0x02 = Response                                             │
//! │   0x03 = Error                                                │
//! │   0x04 = Heartbeat                                            │
//! ├───────────────────────────────────────────────────────────────┤
//! │ JSON Payload (remaining bytes, UTF-8 encoded)                 │
//! └───────────────────────────────────────────────────────────────┘
//! ```

use super::types::{IpcEnvelope, IpcError, IpcResponse};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Protocol version byte.
pub const PROTOCOL_VERSION: u8 = 0x01;

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

/// Frame header size: 4 bytes length + 1 byte version + 1 byte type.
pub const HEADER_SIZE: usize = 6;

/// Maximum frame size (16 MiB hard limit).
pub const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

/// Read a frame header from the stream.
///
/// Returns `(payload_length, protocol_version, message_type)`.
async fn read_header<R>(reader: &mut R) -> Result<(u32, u8, u8), IpcError>
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
    ) {
        return Err(IpcError::ProtocolError(format!(
            "Unknown message type: {msg_type:#04x}"
        )));
    }

    Ok((length, version, msg_type))
}

/// Write a frame header to the stream.
async fn write_header<W>(
    writer: &mut W,
    payload_length: u32,
    msg_type: u8,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    let mut header = [0u8; HEADER_SIZE];
    header[..4].copy_from_slice(&payload_length.to_be_bytes());
    header[4] = PROTOCOL_VERSION;
    header[5] = msg_type;

    writer
        .write_all(&header)
        .await
        .map_err(|e| IpcError::IoError(e.to_string()))
}

/// Read a complete frame from the stream.
///
/// Returns the message type and payload bytes.
pub async fn read_frame<R>(
    reader: &mut R,
    max_size: usize,
) -> Result<(u8, Vec<u8>), IpcError>
where
    R: AsyncRead + Unpin,
{
    let (length, _version, msg_type) = read_header(reader).await?;

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

    Ok((msg_type, payload))
}

/// Write a frame to the stream.
pub async fn write_frame<W>(
    writer: &mut W,
    msg_type: u8,
    payload: &[u8],
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    let length: u32 = payload
        .len()
        .try_into()
        .map_err(|_| IpcError::ProtocolError("Payload too large for u32".to_string()))?;

    write_header(writer, length, msg_type).await?;
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
pub async fn read_envelope<R>(
    reader: &mut R,
    max_size: usize,
) -> Result<IpcEnvelope, IpcError>
where
    R: AsyncRead + Unpin,
{
    let (msg_type, payload) = read_frame(reader, max_size).await?;

    if msg_type != MSG_TYPE_REQUEST {
        return Err(IpcError::ProtocolError(format!(
            "Expected request message type ({MSG_TYPE_REQUEST:#04x}), got {msg_type:#04x}"
        )));
    }

    serde_json::from_slice(&payload).map_err(IpcError::from)
}

/// Write an IPC envelope (request) to the stream.
pub async fn write_envelope<W>(
    writer: &mut W,
    envelope: &IpcEnvelope,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    let payload = serde_json::to_vec(envelope)?;
    write_frame(writer, MSG_TYPE_REQUEST, &payload).await
}

/// Read an IPC response from the stream.
pub async fn read_response<R>(
    reader: &mut R,
    max_size: usize,
) -> Result<IpcResponse, IpcError>
where
    R: AsyncRead + Unpin,
{
    let (msg_type, payload) = read_frame(reader, max_size).await?;

    if !matches!(msg_type, MSG_TYPE_RESPONSE | MSG_TYPE_ERROR) {
        return Err(IpcError::ProtocolError(format!(
            "Expected response message type, got {msg_type:#04x}"
        )));
    }

    serde_json::from_slice(&payload).map_err(IpcError::from)
}

/// Write an IPC response to the stream.
pub async fn write_response<W>(
    writer: &mut W,
    response: &IpcResponse,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    let msg_type = if response.success {
        MSG_TYPE_RESPONSE
    } else {
        MSG_TYPE_ERROR
    };
    let payload = serde_json::to_vec(response)?;
    write_frame(writer, msg_type, &payload).await
}

/// Write a heartbeat message to the stream.
pub async fn write_heartbeat<W>(writer: &mut W) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    write_frame(writer, MSG_TYPE_HEARTBEAT, &[]).await
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

/// Write a push notification to the stream.
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
    let payload = serde_json::to_vec(push)?;
    write_frame(writer, MSG_TYPE_PUSH, &payload).await
}

/// Read a push notification from the stream.
pub async fn read_push<R>(
    reader: &mut R,
    max_size: usize,
) -> Result<super::types::IpcPushNotification, IpcError>
where
    R: AsyncRead + Unpin,
{
    let (msg_type, payload) = read_frame(reader, max_size).await?;

    if msg_type != MSG_TYPE_PUSH {
        return Err(IpcError::ProtocolError(format!(
            "Expected push message type ({MSG_TYPE_PUSH:#04x}), got {msg_type:#04x}"
        )));
    }

    serde_json::from_slice(&payload).map_err(IpcError::from)
}

/// Write a subscribe request to the stream.
pub async fn write_subscribe<W>(
    writer: &mut W,
    request: &super::types::IpcSubscribeRequest,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    let payload = serde_json::to_vec(request)?;
    write_frame(writer, MSG_TYPE_SUBSCRIBE, &payload).await
}

/// Write an unsubscribe request to the stream.
pub async fn write_unsubscribe<W>(
    writer: &mut W,
    request: &super::types::IpcUnsubscribeRequest,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    let payload = serde_json::to_vec(request)?;
    write_frame(writer, MSG_TYPE_UNSUBSCRIBE, &payload).await
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
        write_frame(&mut buffer, MSG_TYPE_REQUEST, payload)
            .await
            .unwrap();

        // Read frame
        let mut reader = Cursor::new(buffer);
        let (msg_type, read_payload) = read_frame(&mut reader, 1024).await.unwrap();

        assert_eq!(msg_type, MSG_TYPE_REQUEST);
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
        let read_envelope = read_envelope(&mut reader, 1024).await.unwrap();

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
        let (msg_type, payload) = read_frame(&mut reader, 1024).await.unwrap();

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
        assert_eq!(HEADER_SIZE, 6);
        assert_eq!(PROTOCOL_VERSION, 0x01);
        assert_eq!(MSG_TYPE_REQUEST, 0x01);
        assert_eq!(MSG_TYPE_RESPONSE, 0x02);
        assert_eq!(MSG_TYPE_ERROR, 0x03);
        assert_eq!(MSG_TYPE_HEARTBEAT, 0x04);
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
        let (msg_type, payload) = read_frame(&mut reader, 1024).await.unwrap();

        assert!(is_subscribe(msg_type));
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
        let (msg_type, payload) = read_frame(&mut reader, 1024).await.unwrap();

        assert!(is_unsubscribe(msg_type));
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
        let (msg_type, payload) = read_frame(&mut reader, 1024).await.unwrap();

        assert!(is_unsubscribe(msg_type));
        let parsed: IpcUnsubscribeRequest = serde_json::from_slice(&payload).unwrap();
        assert!(parsed.message_types.is_empty());
    }
}
