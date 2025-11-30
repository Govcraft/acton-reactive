#!/usr/bin/env python3
"""
Example client demonstrating the Acton IPC client library.

This example shows how to:
1. Connect to an acton-reactive IPC server
2. Send request-response messages
3. Use streaming responses
4. Subscribe to push notifications
5. Discover available services

Usage:
    # First, start the example server (from the Rust side):
    cargo run --example ipc_client_libraries_server

    # Then run this client:
    python example_client.py
"""

import asyncio
import sys
from pathlib import Path

# Import from the local library
from acton_ipc import (
    ActonIpcClient,
    ActonIpcClientSync,
    PushNotification,
    get_default_socket_path,
    socket_exists,
    ServerError,
    TimeoutError,
    ConnectionError,
)


async def demo_basic_request(client: ActonIpcClient):
    """Demonstrate basic request-response pattern."""
    print("\n=== Basic Request-Response ===")

    try:
        response = await client.request(
            target='calculator',
            message_type='Add',
            payload={'a': 42, 'b': 8},
            timeout_ms=5000,
        )

        if response.success:
            print(f"  Result: {response.payload}")
        else:
            print(f"  Error: {response.error}")

    except ServerError as e:
        print(f"  Server error: {e} (code: {e.code})")
    except TimeoutError as e:
        print(f"  Timeout: {e}")


async def demo_fire_and_forget(client: ActonIpcClient):
    """Demonstrate fire-and-forget pattern."""
    print("\n=== Fire and Forget ===")

    await client.fire_and_forget(
        target='logger',
        message_type='LogEvent',
        payload={
            'level': 'info',
            'message': 'Hello from Python client!',
        },
    )
    print("  Message sent (no response expected)")


async def demo_streaming(client: ActonIpcClient):
    """Demonstrate streaming response pattern."""
    print("\n=== Streaming Response ===")

    try:
        frame_count = 0
        async for frame in client.stream(
            target='search',
            message_type='SearchQuery',
            payload={'query': 'test', 'limit': 5},
            timeout_ms=10000,
        ):
            frame_count += 1
            if frame.error:
                print(f"  Stream error: {frame.error}")
                break

            print(f"  Frame {frame.sequence}: {frame.payload}")

            if frame.is_final:
                print(f"  Stream complete ({frame_count} frames)")
                break

    except TimeoutError as e:
        print(f"  Stream timeout: {e}")


async def demo_discovery(client: ActonIpcClient):
    """Demonstrate service discovery."""
    print("\n=== Service Discovery ===")

    try:
        discovery = await client.discover()

        print(f"  Protocol: v{discovery.protocol_version.get('current', '?')}")
        print(f"  Capabilities: {discovery.protocol_version.get('capabilities', {})}")
        print(f"  Agents: {[a.get('name') for a in discovery.agents]}")
        print(f"  Message Types: {discovery.message_types}")

    except Exception as e:
        print(f"  Discovery failed: {e}")


async def demo_subscriptions(client: ActonIpcClient):
    """Demonstrate push notifications via subscriptions."""
    print("\n=== Subscriptions ===")

    received_notifications = []

    def on_notification(notification: PushNotification):
        received_notifications.append(notification)
        print(f"  [PUSH] {notification.message_type}: {notification.payload}")

    client.on_push(on_notification)

    # Subscribe to message types
    success = await client.subscribe(['PriceUpdate', 'StatusChange'])
    print(f"  Subscription result: {'success' if success else 'failed'}")

    # In a real scenario, we would wait for push notifications here
    # For demo purposes, we'll just show that we're subscribed
    print("  Waiting for notifications (3 seconds)...")
    await asyncio.sleep(3)

    print(f"  Received {len(received_notifications)} notifications")

    # Unsubscribe
    await client.unsubscribe(['PriceUpdate', 'StatusChange'])
    print("  Unsubscribed")


async def main_async():
    """Async main entry point."""
    # Determine socket path
    socket_path = get_default_socket_path('ipc_client_example')

    # Allow override via command line
    if len(sys.argv) > 1:
        socket_path = Path(sys.argv[1])

    print(f"Acton IPC Python Client Example")
    print(f"================================")
    print(f"Socket path: {socket_path}")

    # Check if socket exists
    if not socket_exists(socket_path):
        print(f"\nError: Socket not found at {socket_path}")
        print("\nMake sure the example server is running:")
        print("  cargo run --example ipc_client_libraries_server")
        sys.exit(1)

    # Connect and run demos
    try:
        async with ActonIpcClient(socket_path) as client:
            print("\nConnected to server!")

            await demo_discovery(client)
            await demo_basic_request(client)
            await demo_fire_and_forget(client)
            await demo_streaming(client)
            await demo_subscriptions(client)

            print("\n=== All demos complete ===")

    except ConnectionError as e:
        print(f"\nConnection error: {e}")
        sys.exit(1)


def main_sync():
    """Synchronous main entry point (alternative)."""
    socket_path = get_default_socket_path('ipc_client_example')

    if len(sys.argv) > 1:
        socket_path = Path(sys.argv[1])

    print(f"Acton IPC Python Client (Sync)")
    print(f"==============================")
    print(f"Socket path: {socket_path}")

    if not socket_exists(socket_path):
        print(f"\nError: Socket not found at {socket_path}")
        sys.exit(1)

    with ActonIpcClientSync(socket_path) as client:
        print("\nConnected!")

        # Basic request
        response = client.request(
            target='calculator',
            message_type='Add',
            payload={'a': 10, 'b': 20},
        )
        print(f"Result: {response.payload}")


if __name__ == '__main__':
    # Use async version by default
    asyncio.run(main_async())
