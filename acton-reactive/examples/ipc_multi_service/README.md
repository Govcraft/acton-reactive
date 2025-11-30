# IPC Multi-Service Example

This example demonstrates exposing **multiple actors via IPC**, each providing a different service, with a real-time dashboard that updates in place.

## Components

- **server.rs**: The IPC server with a live dashboard showing service status and statistics
- **client.rs**: A client that sends messages to different services and monitors responses

## Features

- Multiple actors with different responsibilities
- IPC routing to different services by logical name
- Real-time dashboard with in-place terminal updates
- Listener statistics monitoring
- Graceful shutdown with Ctrl+C handling
- Heartbeat/ping for connection health checks

## Services Provided

| Service | Description |
|---------|-------------|
| **counter** | A simple counter that tracks increments/decrements |
| **logger** | A logging service that accepts log messages |
| **config** | A configuration store for key-value pairs |

## Running the Example

### Step 1: Start the Server

In one terminal:

```bash
cargo run --example ipc_multi_service_server --features ipc
```

The server will display a live dashboard showing:
- Service status (running/stopped)
- IPC statistics (connections, messages received/routed, errors)
- Activity log of recent operations
- Socket path for client connections

### Step 2: Run the Client

In another terminal:

```bash
cargo run --example ipc_multi_service_client --features ipc
```

The client will:
1. Check socket availability
2. Send a heartbeat to verify connection health
3. Send messages to each service
4. Display responses

## Client Options

Connect to the default server:
```bash
cargo run --example ipc_multi_service_client --features ipc
```

Connect to a different server by name:
```bash
cargo run --example ipc_multi_service_client --features ipc -- --server ipc_basic
```

Connect using a custom socket path:
```bash
cargo run --example ipc_multi_service_client --features ipc -- /path/to/socket.sock
```

## Available Message Types

### Counter Service (`counter`)

| Message Type | Description |
|-------------|-------------|
| `Increment` | Increase counter by `amount` |
| `Decrement` | Decrease counter by `amount` |

### Logger Service (`logger`)

| Message Type | Description |
|-------------|-------------|
| `LogMessage` | Log a message with `level` and `message` |

### Config Service (`config`)

| Message Type | Description |
|-------------|-------------|
| `SetConfig` | Store a config value (`key`, `value`) |
| `GetConfig` | Retrieve a config value by `key` |

## Dashboard Preview

```
═══════════════════════════════════════════════════════════
  IPC Multi-Actor Server Dashboard
═══════════════════════════════════════════════════════════
  Socket: /run/user/1000/acton/ipc_multi_service_server.sock

┌─ Services ─────────────────────────────────────────────────┐
│        ●  Counter        15 value         3 ops           │
│        ●  Logger         5 entries                        │
│        ●  Config         2 keys                           │
└────────────────────────────────────────────────────────────┘

┌─ IPC Statistics ───────────────────────────────────────────┐
│        1 active           3 total                         │
│       10 received        10 routed          0 errors      │
└────────────────────────────────────────────────────────────┘
```

## Notes

- The server requires the `crossterm` crate for terminal UI
- Press Ctrl+C on either server or client for graceful shutdown
- The socket is automatically cleaned up when the server stops
