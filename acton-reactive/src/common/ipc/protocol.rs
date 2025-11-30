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
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

// ============================================================================
// Protocol Versioning
// ============================================================================

/// Current protocol version byte.
pub const PROTOCOL_VERSION: u8 = 0x02;

/// Minimum supported protocol version for backward compatibility.
pub const MIN_SUPPORTED_VERSION: u8 = 0x01;

/// Maximum supported protocol version.
pub const MAX_SUPPORTED_VERSION: u8 = 0x02;

/// Protocol capabilities represented as individual features.
///
/// Used for querying what features a specific protocol version supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolCapability {
    /// Support for format byte in header (allows multi-format encoding).
    FormatByte,
    /// Support for `MessagePack` binary serialization.
    MessagePack,
    /// Support for streaming responses (multiple frames per request).
    Streaming,
    /// Support for push notifications (server-initiated messages).
    Push,
    /// Support for service discovery.
    Discovery,
}

/// Capability flags stored as a bitfield for const compatibility.
///
/// This is an internal representation that allows `ProtocolVersion` constants
/// to be defined at compile time while still providing a clean API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityFlags(u8);

impl CapabilityFlags {
    /// No capabilities.
    pub const NONE: Self = Self(0);

    /// Bit flag for format byte support.
    const FORMAT_BYTE: u8 = 1 << 0;
    /// Bit flag for `MessagePack` support.
    const MESSAGEPACK: u8 = 1 << 1;
    /// Bit flag for streaming support.
    const STREAMING: u8 = 1 << 2;
    /// Bit flag for push notification support.
    const PUSH: u8 = 1 << 3;
    /// Bit flag for discovery support.
    const DISCOVERY: u8 = 1 << 4;

    /// All v2 capabilities.
    pub const V2_ALL: Self = Self(
        Self::FORMAT_BYTE | Self::MESSAGEPACK | Self::STREAMING | Self::PUSH | Self::DISCOVERY,
    );

    /// Check if a capability is supported.
    #[must_use]
    pub const fn supports(self, capability: ProtocolCapability) -> bool {
        let flag = match capability {
            ProtocolCapability::FormatByte => Self::FORMAT_BYTE,
            ProtocolCapability::MessagePack => Self::MESSAGEPACK,
            ProtocolCapability::Streaming => Self::STREAMING,
            ProtocolCapability::Push => Self::PUSH,
            ProtocolCapability::Discovery => Self::DISCOVERY,
        };
        self.0 & flag != 0
    }

    /// Get all capabilities as an iterator-friendly array.
    #[must_use]
    pub const fn as_array(self) -> [Option<ProtocolCapability>; 5] {
        [
            if self.0 & Self::FORMAT_BYTE != 0 {
                Some(ProtocolCapability::FormatByte)
            } else {
                None
            },
            if self.0 & Self::MESSAGEPACK != 0 {
                Some(ProtocolCapability::MessagePack)
            } else {
                None
            },
            if self.0 & Self::STREAMING != 0 {
                Some(ProtocolCapability::Streaming)
            } else {
                None
            },
            if self.0 & Self::PUSH != 0 {
                Some(ProtocolCapability::Push)
            } else {
                None
            },
            if self.0 & Self::DISCOVERY != 0 {
                Some(ProtocolCapability::Discovery)
            } else {
                None
            },
        ]
    }
}

/// Protocol version information with capability flags.
///
/// This struct provides detailed information about a specific protocol version,
/// including what features it supports and its wire format characteristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersion {
    /// Major version number (wire format byte).
    pub major: u8,
    /// Capability flags for this version.
    capabilities: CapabilityFlags,
    /// Header size in bytes for this version.
    pub header_size: u8,
}

impl ProtocolVersion {
    /// Protocol v1 (original release).
    ///
    /// Wire format: `[length:4][version:1][msg_type:1][payload...]`
    ///
    /// # Features
    /// - 6-byte header (no format byte)
    /// - JSON serialization only
    /// - Message types: Request, Response, Error, Heartbeat
    /// - No streaming, push, or discovery support
    pub const V1: Self = Self {
        major: 0x01,
        capabilities: CapabilityFlags::NONE,
        header_size: 6,
    };

    /// Protocol v2 (current version).
    ///
    /// Wire format: `[length:4][version:1][msg_type:1][format:1][payload...]`
    ///
    /// # Features
    /// - 7-byte header (includes format byte)
    /// - JSON and `MessagePack` serialization
    /// - Message types: Request, Response, Error, Heartbeat, Push, Subscribe,
    ///   Unsubscribe, Discover, Stream
    /// - Full streaming, push, and discovery support
    pub const V2: Self = Self {
        major: 0x02,
        capabilities: CapabilityFlags::V2_ALL,
        header_size: 7,
    };

    /// Current protocol version used by this library.
    pub const CURRENT: Self = Self::V2;

    /// Check if this version supports a specific capability.
    #[must_use]
    pub const fn supports(&self, capability: ProtocolCapability) -> bool {
        self.capabilities.supports(capability)
    }

    /// Check if format byte is supported.
    #[must_use]
    pub const fn supports_format_byte(&self) -> bool {
        self.capabilities.supports(ProtocolCapability::FormatByte)
    }

    /// Check if `MessagePack` is supported.
    #[must_use]
    pub const fn supports_messagepack(&self) -> bool {
        self.capabilities.supports(ProtocolCapability::MessagePack)
    }

    /// Check if streaming is supported.
    #[must_use]
    pub const fn supports_streaming(&self) -> bool {
        self.capabilities.supports(ProtocolCapability::Streaming)
    }

    /// Check if push notifications are supported.
    #[must_use]
    pub const fn supports_push(&self) -> bool {
        self.capabilities.supports(ProtocolCapability::Push)
    }

    /// Check if discovery is supported.
    #[must_use]
    pub const fn supports_discovery(&self) -> bool {
        self.capabilities.supports(ProtocolCapability::Discovery)
    }

    /// Get the capability flags for this version.
    #[must_use]
    pub const fn capabilities(&self) -> CapabilityFlags {
        self.capabilities
    }

    /// Get version info from a version byte.
    #[must_use]
    pub const fn from_byte(version: u8) -> Option<Self> {
        match version {
            0x01 => Some(Self::V1),
            0x02 => Some(Self::V2),
            _ => None,
        }
    }

    /// Check if this version is supported by the current implementation.
    #[must_use]
    pub const fn is_supported(version: u8) -> bool {
        version >= MIN_SUPPORTED_VERSION && version <= MAX_SUPPORTED_VERSION
    }

    /// Negotiate the best common version between client and server.
    ///
    /// Returns the highest version both sides support, or None if incompatible.
    #[must_use]
    pub const fn negotiate(client_version: u8, server_version: u8) -> Option<u8> {
        if client_version < MIN_SUPPORTED_VERSION || server_version < MIN_SUPPORTED_VERSION {
            return None;
        }
        // Use the lower of the two versions (both can speak it)
        let negotiated = if client_version < server_version {
            client_version
        } else {
            server_version
        };
        if negotiated <= MAX_SUPPORTED_VERSION {
            Some(negotiated)
        } else {
            Some(MAX_SUPPORTED_VERSION)
        }
    }

    /// Get a human-readable description of this version.
    #[must_use]
    pub const fn description(&self) -> &'static str {
        match self.major {
            0x01 => "v1 (JSON only, basic messaging)",
            0x02 => "v2 (multi-format, streaming, push, discovery)",
            _ => "unknown version",
        }
    }

    /// List all supported versions.
    #[must_use]
    pub const fn supported_versions() -> &'static [Self] {
        &[Self::V1, Self::V2]
    }
}

impl Default for ProtocolVersion {
    fn default() -> Self {
        Self::CURRENT
    }
}

impl std::fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{} ({})", self.major, self.description())
    }
}

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

/// Message type: Discovery request (client → server, for actor/type discovery).
pub const MSG_TYPE_DISCOVER: u8 = 0x08;

/// Message type: Stream frame (server → client, for streaming responses).
pub const MSG_TYPE_STREAM: u8 = 0x09;

/// Frame header size for v1: 4 bytes length + 1 byte version + 1 byte type.
pub const HEADER_SIZE_V1: usize = 6;

/// Frame header size for v2: 4 bytes length + 1 byte version + 1 byte type + 1 byte format.
pub const HEADER_SIZE_V2: usize = 7;

/// Frame header size (current version).
pub const HEADER_SIZE: usize = HEADER_SIZE_V2;

/// Maximum frame size (16 MiB hard limit).
pub const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

/// Message types supported in v1 protocol.
const V1_MESSAGE_TYPES: &[u8] = &[
    MSG_TYPE_REQUEST,
    MSG_TYPE_RESPONSE,
    MSG_TYPE_ERROR,
    MSG_TYPE_HEARTBEAT,
];

/// Message types supported in v2 protocol.
const V2_MESSAGE_TYPES: &[u8] = &[
    MSG_TYPE_REQUEST,
    MSG_TYPE_RESPONSE,
    MSG_TYPE_ERROR,
    MSG_TYPE_HEARTBEAT,
    MSG_TYPE_PUSH,
    MSG_TYPE_SUBSCRIBE,
    MSG_TYPE_UNSUBSCRIBE,
    MSG_TYPE_DISCOVER,
    MSG_TYPE_STREAM,
];

/// Validate message type for a given protocol version.
fn validate_message_type(version: u8, msg_type: u8) -> Result<(), IpcError> {
    let valid_types = match version {
        0x01 => V1_MESSAGE_TYPES,
        0x02 => V2_MESSAGE_TYPES,
        _ => {
            return Err(IpcError::ProtocolError(format!(
                "Unknown protocol version: {version:#04x}"
            )))
        }
    };

    if valid_types.contains(&msg_type) {
        Ok(())
    } else {
        Err(IpcError::ProtocolError(format!(
            "Unknown message type {msg_type:#04x} for protocol v{version}"
        )))
    }
}

/// Read a frame header from the stream with backward compatibility.
///
/// Supports both v1 (6-byte) and v2 (7-byte) headers. For v1, format defaults to JSON.
///
/// Returns `(payload_length, protocol_version, message_type, format)`.
async fn read_header<R>(reader: &mut R) -> Result<(u32, u8, u8, Format), IpcError>
where
    R: AsyncRead + Unpin,
{
    // Read the base header (6 bytes - common to v1 and v2)
    let mut base_header = [0u8; HEADER_SIZE_V1];
    reader.read_exact(&mut base_header).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            IpcError::ConnectionClosed
        } else {
            IpcError::IoError(e.to_string())
        }
    })?;

    let length = u32::from_be_bytes([
        base_header[0],
        base_header[1],
        base_header[2],
        base_header[3],
    ]);
    let version = base_header[4];
    let msg_type = base_header[5];

    // Validate protocol version is supported
    if !ProtocolVersion::is_supported(version) {
        return Err(IpcError::UnsupportedProtocolVersion {
            received: version,
            min_supported: MIN_SUPPORTED_VERSION,
            max_supported: MAX_SUPPORTED_VERSION,
        });
    }

    // Validate message type for this version
    validate_message_type(version, msg_type)?;

    // Handle version-specific format byte
    let format = if version >= 0x02 {
        // v2+: read format byte
        let mut format_byte = [0u8; 1];
        reader.read_exact(&mut format_byte).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                IpcError::ConnectionClosed
            } else {
                IpcError::IoError(e.to_string())
            }
        })?;

        Format::from_byte(format_byte[0]).ok_or_else(|| {
            IpcError::ProtocolError(format!(
                "Unknown serialization format: {:#04x}",
                format_byte[0]
            ))
        })?
    } else {
        // v1: JSON only
        Format::Json
    };

    Ok((length, version, msg_type, format))
}

/// Write a frame header to the stream using the current protocol version.
async fn write_header<W>(
    writer: &mut W,
    payload_length: u32,
    msg_type: u8,
    format: Format,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    write_header_with_version(writer, payload_length, msg_type, format, PROTOCOL_VERSION).await
}

/// Write a frame header with a specific protocol version.
///
/// For v1: writes 6-byte header (no format byte, format ignored)
/// For v2+: writes 7-byte header (includes format byte)
async fn write_header_with_version<W>(
    writer: &mut W,
    payload_length: u32,
    msg_type: u8,
    format: Format,
    version: u8,
) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
{
    if version >= 0x02 {
        // v2+ header: 7 bytes
        let mut header = [0u8; HEADER_SIZE_V2];
        header[..4].copy_from_slice(&payload_length.to_be_bytes());
        header[4] = version;
        header[5] = msg_type;
        header[6] = format.to_byte();
        writer
            .write_all(&header)
            .await
            .map_err(|e| IpcError::IoError(e.to_string()))
    } else {
        // v1 header: 6 bytes (no format byte)
        let mut header = [0u8; HEADER_SIZE_V1];
        header[..4].copy_from_slice(&payload_length.to_be_bytes());
        header[4] = version;
        header[5] = msg_type;
        writer
            .write_all(&header)
            .await
            .map_err(|e| IpcError::IoError(e.to_string()))
    }
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
pub async fn write_envelope<W>(writer: &mut W, envelope: &IpcEnvelope) -> Result<(), IpcError>
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
pub async fn read_response<R>(reader: &mut R, max_size: usize) -> Result<IpcResponse, IpcError>
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
pub async fn write_response<W>(writer: &mut W, response: &IpcResponse) -> Result<(), IpcError>
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
/// This is used by IPC clients to discover available actors and message types.
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
            "actor",
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
        assert_eq!(read_envelope.target, "actor");
        assert_eq!(read_envelope.message_type, "TestMessage");
    }

    #[tokio::test]
    async fn test_write_read_response() {
        let mut buffer = Vec::new();
        let response =
            IpcResponse::success("test_123", Some(serde_json::json!({ "result": "ok" })));

        // Write response
        write_response(&mut buffer, &response).await.unwrap();

        // Read response
        let mut reader = Cursor::new(buffer);
        let read_response = read_response(&mut reader, 1024).await.unwrap();

        assert!(read_response.success);
        assert_eq!(read_response.correlation_id, "test_123");
    }

    #[tokio::test]
    async fn test_unsupported_protocol_version() {
        // Craft a frame with unsupported protocol version
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&4u32.to_be_bytes()); // Length
        buffer.push(0xFF); // Unsupported version
        buffer.push(MSG_TYPE_REQUEST);
        buffer.push(Format::JSON_BYTE); // Format
        buffer.extend_from_slice(b"test");

        let mut reader = Cursor::new(buffer);
        let result = read_frame(&mut reader, 1024).await;

        // Should return UnsupportedProtocolVersion error
        assert!(matches!(
            result,
            Err(IpcError::UnsupportedProtocolVersion {
                received: 0xFF,
                min_supported: MIN_SUPPORTED_VERSION,
                max_supported: MAX_SUPPORTED_VERSION,
            })
        ));
    }

    #[test]
    fn test_protocol_version_from_byte() {
        assert_eq!(ProtocolVersion::from_byte(0x01), Some(ProtocolVersion::V1));
        assert_eq!(ProtocolVersion::from_byte(0x02), Some(ProtocolVersion::V2));
        assert_eq!(ProtocolVersion::from_byte(0x03), None);
        assert_eq!(ProtocolVersion::from_byte(0xFF), None);
    }

    #[test]
    fn test_protocol_version_is_supported() {
        assert!(ProtocolVersion::is_supported(0x01));
        assert!(ProtocolVersion::is_supported(0x02));
        assert!(!ProtocolVersion::is_supported(0x00));
        assert!(!ProtocolVersion::is_supported(0x03));
        assert!(!ProtocolVersion::is_supported(0xFF));
    }

    #[test]
    fn test_protocol_version_negotiate() {
        // Client v1, server v2 -> negotiate to v1
        assert_eq!(ProtocolVersion::negotiate(0x01, 0x02), Some(0x01));
        // Client v2, server v1 -> negotiate to v1
        assert_eq!(ProtocolVersion::negotiate(0x02, 0x01), Some(0x01));
        // Client v2, server v2 -> negotiate to v2
        assert_eq!(ProtocolVersion::negotiate(0x02, 0x02), Some(0x02));
        // Client v0 (unsupported) -> None
        assert_eq!(ProtocolVersion::negotiate(0x00, 0x02), None);
        // Server v0 (unsupported) -> None
        assert_eq!(ProtocolVersion::negotiate(0x01, 0x00), None);
    }

    #[test]
    fn test_protocol_version_capabilities() {
        // V1 capabilities
        let v1 = ProtocolVersion::V1;
        assert!(!v1.supports_format_byte());
        assert!(!v1.supports_messagepack());
        assert!(!v1.supports_streaming());
        assert!(!v1.supports_push());
        assert!(!v1.supports_discovery());
        assert_eq!(v1.header_size, 6);

        // V2 capabilities
        let v2 = ProtocolVersion::V2;
        assert!(v2.supports_format_byte());
        assert!(v2.supports_messagepack());
        assert!(v2.supports_streaming());
        assert!(v2.supports_push());
        assert!(v2.supports_discovery());
        assert_eq!(v2.header_size, 7);
    }

    #[test]
    fn test_protocol_capability_enum() {
        use super::ProtocolCapability;

        let v2 = ProtocolVersion::V2;
        assert!(v2.supports(ProtocolCapability::FormatByte));
        assert!(v2.supports(ProtocolCapability::MessagePack));
        assert!(v2.supports(ProtocolCapability::Streaming));
        assert!(v2.supports(ProtocolCapability::Push));
        assert!(v2.supports(ProtocolCapability::Discovery));

        let v1 = ProtocolVersion::V1;
        assert!(!v1.supports(ProtocolCapability::FormatByte));
        assert!(!v1.supports(ProtocolCapability::MessagePack));
        assert!(!v1.supports(ProtocolCapability::Streaming));
        assert!(!v1.supports(ProtocolCapability::Push));
        assert!(!v1.supports(ProtocolCapability::Discovery));
    }

    #[test]
    fn test_protocol_version_constants() {
        assert_eq!(MIN_SUPPORTED_VERSION, 0x01);
        assert_eq!(MAX_SUPPORTED_VERSION, 0x02);
        assert_eq!(PROTOCOL_VERSION, 0x02);
        assert_eq!(HEADER_SIZE_V1, 6);
        assert_eq!(HEADER_SIZE_V2, 7);
        assert_eq!(HEADER_SIZE, HEADER_SIZE_V2);
    }

    #[test]
    fn test_protocol_version_display() {
        let v1 = ProtocolVersion::V1;
        let v2 = ProtocolVersion::V2;
        assert!(v1.to_string().contains("v1"));
        assert!(v2.to_string().contains("v2"));
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
        assert_eq!(
            Format::from_byte(Format::Json.to_byte()),
            Some(Format::Json)
        );
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
        assert_eq!(
            read_notification.source_actor,
            Some("price_service".to_string())
        );
        assert_eq!(read_notification.payload["price"], 100.50);
    }

    #[tokio::test]
    async fn test_write_subscribe_request() {
        use super::super::types::IpcSubscribeRequest;

        let mut buffer = Vec::new();
        let request =
            IpcSubscribeRequest::new(vec!["PriceUpdate".to_string(), "OrderStatus".to_string()]);

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
