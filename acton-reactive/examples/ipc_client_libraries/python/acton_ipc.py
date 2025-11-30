#!/usr/bin/env python3
"""
Acton IPC Client Library for Python

A Python client for communicating with acton-reactive IPC servers.
Supports JSON and MessagePack serialization, request-response,
streaming, subscriptions, and service discovery.

Protocol: v2 with 7-byte header
- 4 bytes: payload length (big-endian u32)
- 1 byte: protocol version (0x02)
- 1 byte: message type
- 1 byte: format (0x01 = JSON, 0x02 = MessagePack)

Requirements:
    pip install msgpack  # Optional, for MessagePack support
"""

from __future__ import annotations

import asyncio
import json
import os
import socket
import struct
import time
import uuid
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from enum import IntEnum
from pathlib import Path
from typing import Any, AsyncIterator, Callable, TypeVar

# Optional MessagePack support
try:
    import msgpack

    HAS_MSGPACK = True
except ImportError:
    HAS_MSGPACK = False
    msgpack = None


# =============================================================================
# Protocol Constants
# =============================================================================


class ProtocolVersion(IntEnum):
    """IPC protocol version."""

    V1 = 0x01  # Legacy: 6-byte header, JSON only
    V2 = 0x02  # Current: 7-byte header, multi-format


class MessageType(IntEnum):
    """Wire protocol message types."""

    REQUEST = 0x01
    RESPONSE = 0x02
    ERROR = 0x03
    HEARTBEAT = 0x04
    PUSH = 0x05
    SUBSCRIBE = 0x06
    UNSUBSCRIBE = 0x07
    DISCOVER = 0x08
    STREAM = 0x09


class Format(IntEnum):
    """Serialization format."""

    JSON = 0x01
    MESSAGEPACK = 0x02


# Header sizes
HEADER_SIZE_V1 = 6  # length(4) + version(1) + msg_type(1)
HEADER_SIZE_V2 = 7  # length(4) + version(1) + msg_type(1) + format(1)

# Limits
MAX_FRAME_SIZE = 16 * 1024 * 1024  # 16 MiB
DEFAULT_TIMEOUT_MS = 30000


# =============================================================================
# Error Types
# =============================================================================


class IpcError(Exception):
    """Base IPC error."""

    pass


class ConnectionError(IpcError):
    """Connection-related errors."""

    pass


class ProtocolError(IpcError):
    """Protocol violation errors."""

    pass


class TimeoutError(IpcError):
    """Request timeout errors."""

    pass


class ServerError(IpcError):
    """Server-side errors."""

    def __init__(self, message: str, code: str | None = None):
        super().__init__(message)
        self.code = code


# =============================================================================
# Correlation ID Generation (MTI-compatible)
# =============================================================================


def generate_correlation_id(prefix: str = "req") -> str:
    """
    Generate a correlation ID compatible with MTI Type IDs.
    Format: <prefix>_<uuid_v7_hex>

    Note: Python's uuid module doesn't support v7 natively yet,
    so we use v4 with a timestamp prefix for ordering.
    """
    # Use timestamp + random for ordering similar to UUIDv7
    timestamp_ms = int(time.time() * 1000)
    random_part = uuid.uuid4().hex[:16]
    id_part = f"{timestamp_ms:012x}{random_part}"
    return f"{prefix}_{id_part}"


# =============================================================================
# Serialization
# =============================================================================


class Serializer(ABC):
    """Abstract serializer interface."""

    @property
    @abstractmethod
    def format_byte(self) -> int:
        """Return the format byte for this serializer."""
        pass

    @abstractmethod
    def serialize(self, data: Any) -> bytes:
        """Serialize data to bytes."""
        pass

    @abstractmethod
    def deserialize(self, data: bytes) -> Any:
        """Deserialize bytes to data."""
        pass


class JsonSerializer(Serializer):
    """JSON serializer."""

    @property
    def format_byte(self) -> int:
        return Format.JSON

    def serialize(self, data: Any) -> bytes:
        return json.dumps(data, separators=(",", ":")).encode("utf-8")

    def deserialize(self, data: bytes) -> Any:
        return json.loads(data.decode("utf-8"))


class MessagePackSerializer(Serializer):
    """MessagePack serializer (requires msgpack package)."""

    def __init__(self):
        if not HAS_MSGPACK:
            raise ImportError(
                "msgpack package required for MessagePack support. "
                "Install with: pip install msgpack"
            )

    @property
    def format_byte(self) -> int:
        return Format.MESSAGEPACK

    def serialize(self, data: Any) -> bytes:
        return msgpack.packb(data, use_bin_type=True)

    def deserialize(self, data: bytes) -> Any:
        return msgpack.unpackb(data, raw=False)


# =============================================================================
# Frame Encoding/Decoding
# =============================================================================


@dataclass
class Frame:
    """A wire protocol frame."""

    version: ProtocolVersion
    message_type: MessageType
    format: Format
    payload: bytes

    @classmethod
    def encode(
        cls,
        message_type: MessageType,
        payload: bytes,
        format_: Format = Format.JSON,
        version: ProtocolVersion = ProtocolVersion.V2,
    ) -> bytes:
        """Encode a frame to wire format."""
        payload_len = len(payload)

        if payload_len > MAX_FRAME_SIZE:
            raise ProtocolError(f"Payload too large: {payload_len} > {MAX_FRAME_SIZE}")

        if version == ProtocolVersion.V2:
            # V2: 7-byte header
            header = struct.pack(
                ">IBBB",
                payload_len,
                version,
                message_type,
                format_,
            )
        else:
            # V1: 6-byte header (JSON only)
            header = struct.pack(
                ">IBB",
                payload_len,
                version,
                message_type,
            )

        return header + payload

    @classmethod
    async def decode_async(cls, reader: asyncio.StreamReader) -> "Frame":
        """Decode a frame from an async stream."""
        # Read first 6 bytes (common to v1 and v2)
        header_start = await reader.readexactly(6)
        payload_len, version, message_type = struct.unpack(">IBB", header_start)

        if payload_len > MAX_FRAME_SIZE:
            raise ProtocolError(f"Frame too large: {payload_len}")

        # Determine format based on version
        if version == ProtocolVersion.V2:
            # Read format byte
            format_byte = await reader.readexactly(1)
            format_ = Format(struct.unpack("B", format_byte)[0])
        else:
            # V1 is always JSON
            format_ = Format.JSON

        # Read payload
        payload = await reader.readexactly(payload_len)

        return cls(
            version=ProtocolVersion(version),
            message_type=MessageType(message_type),
            format=format_,
            payload=payload,
        )

    @classmethod
    def decode_sync(cls, sock: socket.socket) -> "Frame":
        """Decode a frame from a synchronous socket."""

        def recv_exact(n: int) -> bytes:
            data = b""
            while len(data) < n:
                chunk = sock.recv(n - len(data))
                if not chunk:
                    raise ConnectionError("Connection closed")
                data += chunk
            return data

        # Read first 6 bytes
        header_start = recv_exact(6)
        payload_len, version, message_type = struct.unpack(">IBB", header_start)

        if payload_len > MAX_FRAME_SIZE:
            raise ProtocolError(f"Frame too large: {payload_len}")

        # Determine format
        if version == ProtocolVersion.V2:
            format_byte = recv_exact(1)
            format_ = Format(struct.unpack("B", format_byte)[0])
        else:
            format_ = Format.JSON

        # Read payload
        payload = recv_exact(payload_len)

        return cls(
            version=ProtocolVersion(version),
            message_type=MessageType(message_type),
            format=format_,
            payload=payload,
        )


# =============================================================================
# Message Types
# =============================================================================


@dataclass
class IpcEnvelope:
    """Request envelope sent to the server."""

    correlation_id: str
    target: str
    message_type: str
    payload: Any
    expects_reply: bool = True
    expects_stream: bool = False
    response_timeout_ms: int = DEFAULT_TIMEOUT_MS

    def to_dict(self) -> dict:
        return {
            "correlation_id": self.correlation_id,
            "target": self.target,
            "message_type": self.message_type,
            "payload": self.payload,
            "expects_reply": self.expects_reply,
            "expects_stream": self.expects_stream,
            "response_timeout_ms": self.response_timeout_ms,
        }


@dataclass
class IpcResponse:
    """Response from the server."""

    correlation_id: str
    success: bool
    payload: Any = None
    error: str | None = None
    error_code: str | None = None

    @classmethod
    def from_dict(cls, data: dict) -> "IpcResponse":
        return cls(
            correlation_id=data.get("correlation_id", ""),
            success=data.get("success", False),
            payload=data.get("payload"),
            error=data.get("error"),
            error_code=data.get("error_code"),
        )


@dataclass
class StreamFrame:
    """A streaming response frame."""

    correlation_id: str
    sequence: int
    payload: Any = None
    is_final: bool = False
    error: str | None = None
    error_code: str | None = None

    @classmethod
    def from_dict(cls, data: dict) -> "StreamFrame":
        return cls(
            correlation_id=data.get("correlation_id", ""),
            sequence=data.get("sequence", 0),
            payload=data.get("payload"),
            is_final=data.get("is_final", False),
            error=data.get("error"),
            error_code=data.get("error_code"),
        )


@dataclass
class PushNotification:
    """Push notification from subscriptions."""

    notification_id: str
    message_type: str
    payload: Any
    source_actor: str | None = None
    timestamp_ms: int = 0

    @classmethod
    def from_dict(cls, data: dict) -> "PushNotification":
        return cls(
            notification_id=data.get("notification_id", ""),
            message_type=data.get("message_type", ""),
            payload=data.get("payload"),
            source_actor=data.get("source_actor"),
            timestamp_ms=data.get("timestamp_ms", 0),
        )


@dataclass
class DiscoveryResponse:
    """Service discovery response."""

    protocol_version: dict
    actors: list[dict]
    message_types: list[str]

    @classmethod
    def from_dict(cls, data: dict) -> "DiscoveryResponse":
        return cls(
            protocol_version=data.get("protocol_version", {}),
            actors=data.get("actors", []),
            message_types=data.get("message_types", []),
        )


# =============================================================================
# Socket Path Resolution
# =============================================================================


def get_default_socket_path(app_name: str) -> Path:
    """
    Get the default socket path following XDG conventions.

    Path: $XDG_RUNTIME_DIR/acton/<app_name>/ipc.sock
    Fallback: /tmp/acton/<app_name>/ipc.sock
    """
    runtime_dir = os.environ.get("XDG_RUNTIME_DIR")
    if runtime_dir:
        base = Path(runtime_dir)
    else:
        base = Path("/tmp")

    return base / "acton" / app_name / "ipc.sock"


def socket_exists(path: Path | str) -> bool:
    """Check if a socket file exists."""
    return Path(path).exists()


async def socket_is_alive(path: Path | str, timeout: float = 1.0) -> bool:
    """Check if a socket is responding via heartbeat."""
    try:
        async with ActonIpcClient(str(path)) as client:
            await asyncio.wait_for(client.heartbeat(), timeout=timeout)
            return True
    except Exception:
        return False


# =============================================================================
# Async IPC Client
# =============================================================================


class ActonIpcClient:
    """
    Async IPC client for acton-reactive.

    Usage:
        async with ActonIpcClient('/path/to/socket') as client:
            response = await client.request('actor', 'MessageType', {'key': 'value'})
    """

    def __init__(
        self,
        socket_path: str | Path,
        *,
        use_messagepack: bool = False,
        protocol_version: ProtocolVersion = ProtocolVersion.V2,
    ):
        self.socket_path = Path(socket_path)
        self._reader: asyncio.StreamReader | None = None
        self._writer: asyncio.StreamWriter | None = None
        self._pending: dict[str, asyncio.Future] = {}
        self._stream_queues: dict[str, asyncio.Queue] = {}
        self._push_handlers: list[Callable[[PushNotification], None]] = []
        self._receive_task: asyncio.Task | None = None
        self._connected = False
        self._protocol_version = protocol_version

        # Set up serializer
        if use_messagepack:
            self._serializer = MessagePackSerializer()
        else:
            self._serializer = JsonSerializer()

    async def __aenter__(self) -> "ActonIpcClient":
        await self.connect()
        return self

    async def __aexit__(self, *args) -> None:
        await self.close()

    async def connect(self) -> None:
        """Connect to the IPC server."""
        if self._connected:
            return

        if not self.socket_path.exists():
            raise ConnectionError(f"Socket not found: {self.socket_path}")

        self._reader, self._writer = await asyncio.open_unix_connection(
            str(self.socket_path)
        )
        self._connected = True

        # Start receive loop
        self._receive_task = asyncio.create_task(self._receive_loop())

    async def close(self) -> None:
        """Close the connection."""
        if self._receive_task:
            self._receive_task.cancel()
            try:
                await self._receive_task
            except asyncio.CancelledError:
                pass

        if self._writer:
            self._writer.close()
            await self._writer.wait_closed()

        self._connected = False
        self._reader = None
        self._writer = None

    async def _receive_loop(self) -> None:
        """Background task to receive and dispatch frames."""
        try:
            while self._connected and self._reader:
                frame = await Frame.decode_async(self._reader)
                await self._handle_frame(frame)
        except asyncio.CancelledError:
            pass
        except Exception as e:
            # Fail all pending requests
            for future in self._pending.values():
                if not future.done():
                    future.set_exception(ConnectionError(str(e)))
            self._pending.clear()

    async def _handle_frame(self, frame: Frame) -> None:
        """Handle a received frame."""
        # Deserialize based on format
        if frame.format == Format.MESSAGEPACK and HAS_MSGPACK:
            data = MessagePackSerializer().deserialize(frame.payload)
        else:
            data = JsonSerializer().deserialize(frame.payload)

        if frame.message_type == MessageType.RESPONSE:
            self._handle_response(data)
        elif frame.message_type == MessageType.ERROR:
            self._handle_error(data)
        elif frame.message_type == MessageType.STREAM:
            await self._handle_stream_frame(data)
        elif frame.message_type == MessageType.PUSH:
            self._handle_push(data)
        elif frame.message_type == MessageType.HEARTBEAT:
            pass  # Heartbeat handled in request

    def _handle_response(self, data: dict) -> None:
        """Handle a response frame."""
        correlation_id = data.get("correlation_id", "")
        if correlation_id in self._pending:
            future = self._pending.pop(correlation_id)
            if not future.done():
                future.set_result(IpcResponse.from_dict(data))

    def _handle_error(self, data: dict) -> None:
        """Handle an error frame."""
        correlation_id = data.get("correlation_id", "")
        if correlation_id in self._pending:
            future = self._pending.pop(correlation_id)
            if not future.done():
                future.set_exception(
                    ServerError(
                        data.get("error", "Unknown error"),
                        data.get("error_code"),
                    )
                )

    async def _handle_stream_frame(self, data: dict) -> None:
        """Handle a streaming response frame."""
        correlation_id = data.get("correlation_id", "")
        if correlation_id in self._stream_queues:
            await self._stream_queues[correlation_id].put(StreamFrame.from_dict(data))

    def _handle_push(self, data: dict) -> None:
        """Handle a push notification."""
        notification = PushNotification.from_dict(data)
        for handler in self._push_handlers:
            try:
                handler(notification)
            except Exception:
                pass  # Don't let handler errors break the receive loop

    async def _send_frame(
        self,
        message_type: MessageType,
        data: Any,
    ) -> None:
        """Send a frame to the server."""
        if not self._writer:
            raise ConnectionError("Not connected")

        payload = self._serializer.serialize(data)
        frame = Frame.encode(
            message_type,
            payload,
            Format(self._serializer.format_byte),
            self._protocol_version,
        )

        self._writer.write(frame)
        await self._writer.drain()

    async def request(
        self,
        target: str,
        message_type: str,
        payload: Any,
        *,
        timeout_ms: int = DEFAULT_TIMEOUT_MS,
    ) -> IpcResponse:
        """
        Send a request and wait for a response.

        Args:
            target: Target actor name
            message_type: Message type name
            payload: Message payload
            timeout_ms: Request timeout in milliseconds

        Returns:
            IpcResponse with the server's response
        """
        correlation_id = generate_correlation_id("req")

        envelope = IpcEnvelope(
            correlation_id=correlation_id,
            target=target,
            message_type=message_type,
            payload=payload,
            expects_reply=True,
            response_timeout_ms=timeout_ms,
        )

        # Create future for response
        future: asyncio.Future[IpcResponse] = asyncio.get_event_loop().create_future()
        self._pending[correlation_id] = future

        try:
            await self._send_frame(MessageType.REQUEST, envelope.to_dict())
            return await asyncio.wait_for(
                future,
                timeout=timeout_ms / 1000.0,
            )
        except asyncio.TimeoutError:
            self._pending.pop(correlation_id, None)
            raise TimeoutError(f"Request timed out after {timeout_ms}ms")

    async def fire_and_forget(
        self,
        target: str,
        message_type: str,
        payload: Any,
    ) -> None:
        """
        Send a message without waiting for a response.

        Args:
            target: Target actor name
            message_type: Message type name
            payload: Message payload
        """
        correlation_id = generate_correlation_id("req")

        envelope = IpcEnvelope(
            correlation_id=correlation_id,
            target=target,
            message_type=message_type,
            payload=payload,
            expects_reply=False,
        )

        await self._send_frame(MessageType.REQUEST, envelope.to_dict())

    async def stream(
        self,
        target: str,
        message_type: str,
        payload: Any,
        *,
        timeout_ms: int = DEFAULT_TIMEOUT_MS,
    ) -> AsyncIterator[StreamFrame]:
        """
        Send a streaming request and yield response frames.

        Args:
            target: Target actor name
            message_type: Message type name
            payload: Message payload
            timeout_ms: Total stream timeout in milliseconds

        Yields:
            StreamFrame objects until is_final=True
        """
        correlation_id = generate_correlation_id("str")

        envelope = IpcEnvelope(
            correlation_id=correlation_id,
            target=target,
            message_type=message_type,
            payload=payload,
            expects_reply=False,
            expects_stream=True,
            response_timeout_ms=timeout_ms,
        )

        # Create queue for stream frames
        queue: asyncio.Queue[StreamFrame] = asyncio.Queue()
        self._stream_queues[correlation_id] = queue

        try:
            await self._send_frame(MessageType.REQUEST, envelope.to_dict())

            start_time = time.time()
            while True:
                remaining = timeout_ms / 1000.0 - (time.time() - start_time)
                if remaining <= 0:
                    raise TimeoutError("Stream timed out")

                try:
                    frame = await asyncio.wait_for(
                        queue.get(),
                        timeout=remaining,
                    )
                except asyncio.TimeoutError:
                    raise TimeoutError("Stream timed out")

                yield frame

                if frame.is_final:
                    break
                if frame.error:
                    raise ServerError(frame.error, frame.error_code)
        finally:
            self._stream_queues.pop(correlation_id, None)

    async def subscribe(self, message_types: list[str]) -> bool:
        """
        Subscribe to message types for push notifications.

        Args:
            message_types: List of message type names to subscribe to

        Returns:
            True if subscription was successful
        """
        correlation_id = generate_correlation_id("sub")

        data = {
            "correlation_id": correlation_id,
            "message_types": message_types,
        }

        future: asyncio.Future[IpcResponse] = asyncio.get_event_loop().create_future()
        self._pending[correlation_id] = future

        await self._send_frame(MessageType.SUBSCRIBE, data)

        try:
            response = await asyncio.wait_for(future, timeout=5.0)
            return response.success
        except asyncio.TimeoutError:
            self._pending.pop(correlation_id, None)
            return False

    async def unsubscribe(self, message_types: list[str]) -> bool:
        """
        Unsubscribe from message types.

        Args:
            message_types: List of message type names to unsubscribe from

        Returns:
            True if unsubscription was successful
        """
        correlation_id = generate_correlation_id("unsub")

        data = {
            "correlation_id": correlation_id,
            "message_types": message_types,
        }

        future: asyncio.Future[IpcResponse] = asyncio.get_event_loop().create_future()
        self._pending[correlation_id] = future

        await self._send_frame(MessageType.UNSUBSCRIBE, data)

        try:
            response = await asyncio.wait_for(future, timeout=5.0)
            return response.success
        except asyncio.TimeoutError:
            self._pending.pop(correlation_id, None)
            return False

    def on_push(self, handler: Callable[[PushNotification], None]) -> None:
        """
        Register a handler for push notifications.

        Args:
            handler: Callback function that receives PushNotification objects
        """
        self._push_handlers.append(handler)

    async def discover(self) -> DiscoveryResponse:
        """
        Discover available actors and message types.

        Returns:
            DiscoveryResponse with available actors and types
        """
        correlation_id = generate_correlation_id("disc")

        data = {
            "correlation_id": correlation_id,
            "include_actors": True,
            "include_message_types": True,
        }

        future: asyncio.Future[IpcResponse] = asyncio.get_event_loop().create_future()
        self._pending[correlation_id] = future

        await self._send_frame(MessageType.DISCOVER, data)

        response = await asyncio.wait_for(future, timeout=5.0)

        # The response payload contains discovery info
        return DiscoveryResponse.from_dict(response.payload or {})

    async def heartbeat(self) -> None:
        """Send a heartbeat to verify connection."""
        await self._send_frame(MessageType.HEARTBEAT, {})


# =============================================================================
# Synchronous Client (for simpler use cases)
# =============================================================================


class ActonIpcClientSync:
    """
    Synchronous IPC client for simpler use cases.

    Usage:
        with ActonIpcClientSync('/path/to/socket') as client:
            response = client.request('actor', 'MessageType', {'key': 'value'})
    """

    def __init__(
        self,
        socket_path: str | Path,
        *,
        use_messagepack: bool = False,
    ):
        self.socket_path = Path(socket_path)
        self._sock: socket.socket | None = None

        if use_messagepack:
            self._serializer = MessagePackSerializer()
        else:
            self._serializer = JsonSerializer()

    def __enter__(self) -> "ActonIpcClientSync":
        self.connect()
        return self

    def __exit__(self, *args) -> None:
        self.close()

    def connect(self) -> None:
        """Connect to the IPC server."""
        if not self.socket_path.exists():
            raise ConnectionError(f"Socket not found: {self.socket_path}")

        self._sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._sock.connect(str(self.socket_path))

    def close(self) -> None:
        """Close the connection."""
        if self._sock:
            self._sock.close()
            self._sock = None

    def request(
        self,
        target: str,
        message_type: str,
        payload: Any,
        *,
        timeout_ms: int = DEFAULT_TIMEOUT_MS,
    ) -> IpcResponse:
        """Send a request and wait for a response."""
        if not self._sock:
            raise ConnectionError("Not connected")

        correlation_id = generate_correlation_id("req")

        envelope = IpcEnvelope(
            correlation_id=correlation_id,
            target=target,
            message_type=message_type,
            payload=payload,
            expects_reply=True,
            response_timeout_ms=timeout_ms,
        )

        # Send request
        frame_data = self._serializer.serialize(envelope.to_dict())
        frame = Frame.encode(
            MessageType.REQUEST,
            frame_data,
            Format(self._serializer.format_byte),
        )
        self._sock.sendall(frame)

        # Set timeout
        self._sock.settimeout(timeout_ms / 1000.0)

        try:
            # Receive response
            response_frame = Frame.decode_sync(self._sock)

            if response_frame.format == Format.MESSAGEPACK and HAS_MSGPACK:
                data = MessagePackSerializer().deserialize(response_frame.payload)
            else:
                data = JsonSerializer().deserialize(response_frame.payload)

            if response_frame.message_type == MessageType.ERROR:
                raise ServerError(
                    data.get("error", "Unknown error"),
                    data.get("error_code"),
                )

            return IpcResponse.from_dict(data)
        except socket.timeout:
            raise TimeoutError(f"Request timed out after {timeout_ms}ms")

    def fire_and_forget(
        self,
        target: str,
        message_type: str,
        payload: Any,
    ) -> None:
        """Send a message without waiting for a response."""
        if not self._sock:
            raise ConnectionError("Not connected")

        correlation_id = generate_correlation_id("req")

        envelope = IpcEnvelope(
            correlation_id=correlation_id,
            target=target,
            message_type=message_type,
            payload=payload,
            expects_reply=False,
        )

        frame_data = self._serializer.serialize(envelope.to_dict())
        frame = Frame.encode(
            MessageType.REQUEST,
            frame_data,
            Format(self._serializer.format_byte),
        )
        self._sock.sendall(frame)


# =============================================================================
# Example Usage
# =============================================================================


async def example_usage():
    """Example demonstrating client usage."""
    socket_path = get_default_socket_path("my_app")

    print(f"Connecting to: {socket_path}")

    async with ActonIpcClient(socket_path) as client:
        # Discover available services
        discovery = await client.discover()
        print(f"Available actors: {discovery.actors}")
        print(f"Message types: {discovery.message_types}")

        # Send a request
        response = await client.request(
            target="calculator",
            message_type="Add",
            payload={"a": 5, "b": 3},
        )
        print(f"Response: {response.payload}")

        # Subscribe to notifications
        await client.subscribe(["PriceUpdate"])

        # Register push handler
        def on_price(notification: PushNotification):
            print(f"Price update: {notification.payload}")

        client.on_push(on_price)

        # Stream results
        async for frame in client.stream(
            target="search",
            message_type="Query",
            payload={"q": "test"},
        ):
            print(f"Stream frame {frame.sequence}: {frame.payload}")
            if frame.is_final:
                break


if __name__ == "__main__":
    print("Acton IPC Client Library for Python")
    print("=====================================")
    print()
    print("This is a client library for communicating with acton-reactive IPC servers.")
    print()
    print("Usage:")
    print("  from acton_ipc import ActonIpcClient")
    print()
    print("  async with ActonIpcClient('/path/to/socket') as client:")
    print(
        "      response = await client.request('actor', 'MessageType', {'data': 'value'})"
    )
    print()
    print("For MessagePack support, install: pip install msgpack")
