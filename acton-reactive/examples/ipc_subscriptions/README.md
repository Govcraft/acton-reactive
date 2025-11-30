# IPC Subscriptions Example

This example demonstrates **subscription-based push notifications** in acton-reactive IPC, where external clients can subscribe to specific message types and receive real-time push notifications when those messages are broadcast internally.

## Components

- **server.rs**: IPC server that hosts a price feed service broadcasting market data
- **client.rs**: External client that subscribes to message types and receives push notifications

## Features

- **Subscription-Based Push**: Clients subscribe to specific message types
- **Real-Time Notifications**: Server pushes messages to subscribed clients immediately
- **Selective Subscription**: Subscribe/unsubscribe to individual message types
- **Connection-Specific**: Each client maintains its own subscription set

## How It Works

```
┌─────────────┐                      ┌──────────────────┐
│   Client    │                      │     Server       │
└─────────────┘                      └──────────────────┘
       │                                      │
       │  MSG_TYPE_SUBSCRIBE                  │
       │  ["PriceUpdate", "TradeExecuted"]    │
       │─────────────────────────────────────>│
       │                                      │
       │  MSG_TYPE_RESPONSE                   │
       │  {subscribed_types: [...]}           │
       │<─────────────────────────────────────│
       │                                      │
       │                        ┌─────────────┴────────────┐
       │                        │ Internal actor broadcasts │
       │                        │ PriceUpdate via broker    │
       │                        └─────────────┬────────────┘
       │                                      │
       │  MSG_TYPE_PUSH                       │
       │  {message_type: "PriceUpdate", ...}  │
       │<─────────────────────────────────────│
       │                                      │
```

## Message Types

| Type | Description |
|------|-------------|
| `PriceUpdate` | Stock price changes (every 2 seconds) |
| `TradeExecuted` | Trade execution events (every 5 seconds) |
| `SystemStatus` | System status notifications |

## Running the Example

### Step 1: Start the Server

```bash
cargo run --example ipc_subscriptions_server --features ipc
```

The server will:
- Start a price feed service
- Broadcast `PriceUpdate` messages every 2 seconds
- Broadcast `TradeExecuted` messages every 5 seconds
- Broadcast `SystemStatus` on startup and shutdown

### Step 2: Run the Client

In another terminal:

```bash
cargo run --example ipc_subscriptions_client --features ipc
```

The client will run three demos:
1. **Subscribe to ALL** - Receive all message types for 15 seconds
2. **Prices Only** - Subscribe only to `PriceUpdate` for 10 seconds
3. **Trades Only** - Subscribe only to `TradeExecuted` for 12 seconds

## Example Output

**Server:**
```
====================================================================
       IPC Subscriptions Example (Server)
====================================================================

Registered 3 IPC message types for subscriptions
  - PriceUpdate: Stock price changes
  - TradeExecuted: Trade execution events
  - SystemStatus: System status notifications

Price feed service started
Exposed actors: price_feed
IPC listener started
Socket ready: /run/user/1000/acton/ipc_subscriptions_server/ipc.sock

====================================================================
  Server is ready for subscription-based IPC!
====================================================================

  [PriceFeed] Broadcasting: AAPL @ $175.50 (+0.00)
  [PriceFeed] Broadcasting: GOOGL @ $142.31 (+0.01)
  [PriceFeed] Broadcasting: MSFT @ $378.92 (+0.02)
  [PriceFeed] Broadcasting: AMZN @ $178.28 (+0.03)
  [PriceFeed] Broadcasting trade: SELL 100 AAPL @ $150.50 (TRD000001)
```

**Client:**
```
====================================================================
       IPC Subscriptions Example (Client)
====================================================================

Socket path: /run/user/1000/acton/ipc_subscriptions_server/ipc.sock
Connecting to server...
Connected successfully!

====================================================================
  Demo 1: Subscribe to ALL Message Types
====================================================================

Subscribing to: ["PriceUpdate", "TradeExecuted", "SystemStatus"]
  Subscribed to: ["PriceUpdate", "TradeExecuted", "SystemStatus"]

Receiving notifications for 15 seconds...

  ^ AAPL $175.50 (+0.00)
  ^ GOOGL $142.31 (+0.01)
  ^ MSFT $378.92 (+0.02)
  ^ AMZN $178.28 (+0.03)
  [TRADE] TRD000001 SELL AAPL @ $150.50 (100)
```

## Wire Protocol

The subscription system uses three message types in the IPC wire protocol:

| Message Type | Value | Direction | Description |
|-------------|-------|-----------|-------------|
| `MSG_TYPE_SUBSCRIBE` | `0x06` | Client → Server | Subscribe request |
| `MSG_TYPE_UNSUBSCRIBE` | `0x07` | Client → Server | Unsubscribe request |
| `MSG_TYPE_PUSH` | `0x05` | Server → Client | Push notification |

### Subscribe Request

```json
{
  "correlation_id": "sub_01h9xz...",
  "message_types": ["PriceUpdate", "TradeExecuted"]
}
```

### Subscription Response

```json
{
  "correlation_id": "sub_01h9xz...",
  "success": true,
  "subscribed_types": ["PriceUpdate", "TradeExecuted"]
}
```

### Push Notification

```json
{
  "notification_id": "push_01h9xz...",
  "message_type": "PriceUpdate",
  "source_actor": "price_feed",
  "payload": {
    "symbol": "AAPL",
    "price": 175.50,
    "change": 0.25
  },
  "timestamp_ms": 1699999999999
}
```

## Configuration

The push notification buffer size can be configured in `~/.config/acton/ipc.toml`:

```toml
[limits]
push_buffer_size = 100  # Max pending push notifications per connection
```

If a client is slow to read notifications and the buffer fills up, notifications will be dropped.
