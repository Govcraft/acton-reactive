# IPC Fruit Market Example

An advanced IPC example demonstrating a multi-process point-of-sale system where each component runs as a separate process, communicating via Unix domain sockets.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              SERVER PROCESS                                 │
│                                                                             │
│  ┌─────────────────────┐              ┌─────────────────────────────────┐   │
│  │   PriceService      │              │        Message Broker           │   │
│  │      Actor          │─────────────>│    (broadcasts to clients)      │   │
│  │                     │              │                                 │   │
│  │  - Handles ScanItem │              │  Broadcasts:                    │   │
│  │  - Looks up prices  │              │  - ItemScanned                  │   │
│  │  - Handles ToggleHelp             │  - PriceUpdate                  │   │
│  └─────────────────────┘              │  - HelpToggled                  │   │
│            ▲                          └─────────────────────────────────┘   │
│            │                                         │                      │
│  ┌─────────┴─────────┐               ┌───────────────┴───────────────────┐  │
│  │   IPC Listener    │◄──────────────│      Subscription Manager        │  │
│  │                   │               │   (routes broadcasts to clients)  │  │
│  └─────────┬─────────┘               └───────────────────────────────────┘  │
└────────────┼────────────────────────────────────────────────────────────────┘
             │
             │ Unix Domain Socket
             │
     ┌───────┴───────┐
     │               │
┌────▼────┐    ┌─────▼─────┐
│Keyboard │    │  Display  │
│ Client  │    │  Client   │
│         │    │           │
│Commands:│    │Subscribes:│
│- ScanItem    │- ItemScanned
│- ToggleHelp  │- PriceUpdate
│         │    │- HelpToggled
└─────────┘    └───────────┘
```

## Components

### Server (`server.rs`)

The central coordinator containing the PriceService actor that:
- Receives `ScanItem` commands from the keyboard client
- Simulates price lookups (100ms delay with random prices)
- Broadcasts `ItemScanned` and `PriceUpdate` messages to subscribed clients
- Handles `ToggleHelp` commands

### Keyboard Client (`keyboard_client.rs`)

An input-only client that:
- Captures raw keyboard input using crossterm
- Sends fire-and-forget commands to the server
- Controls: `s` to scan, `?` to toggle help, `q`/`ESC` to quit

### Display Client (`display_client.rs`)

A UI client that:
- Subscribes to `ItemScanned`, `PriceUpdate`, and `HelpToggled` messages
- Maintains local cart state
- Renders a colorful terminal receipt UI
- Updates in real-time as items are scanned and priced

## Message Flow

1. User presses `s` in the keyboard client
2. Keyboard client sends `ScanItem` to server (fire-and-forget)
3. Server's PriceService broadcasts `ItemScanned` immediately
4. Display client receives `ItemScanned`, adds item to cart (pending price)
5. Server simulates price lookup (100ms delay)
6. Server broadcasts `PriceUpdate` with the price
7. Display client receives `PriceUpdate`, updates item price and total

## Running the Example

### Option 1: Using tmux launcher (recommended)

```bash
cd acton-reactive/examples/ipc_fruit_market
./start.sh
```

This starts all components in a split-pane tmux session.

### Option 2: Manual startup

Open three terminals:

**Terminal 1 - Server:**
```bash
cargo run --example ipc_fruit_market_server --features ipc
```

**Terminal 2 - Display Client:**
```bash
cargo run --example ipc_fruit_market_display --features ipc
```

**Terminal 3 - Keyboard Client:**
```bash
cargo run --example ipc_fruit_market_keyboard --features ipc
```

## IPC Patterns Demonstrated

1. **Fire-and-Forget Commands**: Keyboard client sends commands without waiting for responses
2. **Push Subscriptions**: Display client subscribes to message types and receives broadcasts
3. **Broker Broadcasting**: Server broadcasts messages to all subscribed clients
4. **State Synchronization**: Display client maintains local state updated by server broadcasts

## Key Concepts

- **Separate Processes**: Each component is a separate binary/process
- **No Shared Code**: Message types are duplicated in each process (IPC convention)
- **Reactive UI**: Display updates reactively as messages arrive
- **Decoupled Architecture**: Components can start/stop independently

## Shutdown

- Press `q` or `ESC` in the keyboard client to quit
- Press `Ctrl+C` in the server to shut down
- Use `./stop.sh` to kill all processes if using tmux
