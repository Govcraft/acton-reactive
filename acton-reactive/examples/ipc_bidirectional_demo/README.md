# IPC Bidirectional Communication Example

This example demonstrates the **request-response pattern** in acton-reactive IPC, where external clients can send queries and receive responses from agents.

## Components

- **server.rs**: The IPC server that hosts calculator and key-value store services
- **client.rs**: An external client that sends requests and receives responses

## Features

- **Request-Response Pattern**: External process sends a query, agent processes it, and returns a response via the IPC proxy channel
- **Calculator Service**: A simple arithmetic service demonstrating synchronous response generation
- **Key-Value Store**: A stateful service demonstrating queries that may or may not have results

## How It Works

1. IPC client sends `IpcEnvelope` with `expects_reply: true`
2. IPC listener creates a temporary MPSC channel (proxy)
3. Message is sent to agent with proxy as `reply_to` address
4. Agent handler uses `envelope.reply_envelope().send(response)` to reply
5. Listener receives response on proxy channel and serializes it back to client

## Running the Example

### Step 1: Start the Server

In one terminal:

```bash
cargo run --example ipc_bidirectional_server --features ipc
```

The server will start and display:
- Available services (calculator, kv_store)
- Socket path for client connections
- Instructions for running the client

### Step 2: Run the Client

In another terminal:

```bash
cargo run --example ipc_bidirectional_client --features ipc
```

The client will:
1. Connect to the server
2. Send arithmetic requests to the calculator service
3. Send get/set requests to the key-value store
4. Display responses from each request

## Example Output

**Server:**
```
[Calculator] Add: 5 + 3 = 8 (op #1)
[Calculator] Multiply: 7 x 6 = 42 (op #2)
[KV Store] Set: username = "alice"
[KV Store] Get: username = "alice"
```

**Client:**
```
Test 1: Addition (5 + 3)
Response (success):
   result: 8
   operation: "5 + 3"
```

## Available Services

### Calculator (`calculator`)

| Message Type | Description |
|-------------|-------------|
| `AddRequest` | Add two numbers (`a`, `b`) |
| `MultiplyRequest` | Multiply two numbers (`a`, `b`) |

### Key-Value Store (`kv_store`)

| Message Type | Description |
|-------------|-------------|
| `SetValue` | Store a key-value pair (`key`, `value`) |
| `GetValue` | Retrieve a value by key (`key`) |
