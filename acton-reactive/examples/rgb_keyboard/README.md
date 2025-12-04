# RGB Keyboard - Advanced IPC Example

This advanced example demonstrates multiple acton-reactive IPC patterns working together
to create a colorful keyboard visualization system.

## Architecture

```
                           ┌─────────────────────────┐
                           │      RGB Server         │
                           │  (KeystrokeProcessor)   │
                           └─────────────────────────┘
                                      │
            ┌─────────────────────────┼─────────────────────────┐
            │                         │                         │
            ▼                         ▼                         ▼
   ┌────────────────┐       ┌────────────────┐       ┌────────────────┐
   │   R Client     │       │   G Client     │       │   B Client     │
   │ (subscribes to │       │ (subscribes to │       │ (subscribes to │
   │ ColorRequest)  │       │ ColorRequest)  │       │ ColorRequest)  │
   └────────────────┘       └────────────────┘       └────────────────┘
            │                         │                         │
            │       ColorResponse     │       ColorResponse     │
            └─────────────────────────┼─────────────────────────┘
                                      │
                           ┌──────────▼──────────┐
                           │      RGB Server     │
                           │  (correlates RGB)   │
                           └─────────────────────┘
                                      │
            ┌─────────────────────────┼─────────────────────────┐
            │                         │                         │
            ▼                         ▼                         ▼
   ┌────────────────┐       ┌────────────────┐       ┌────────────────┐
   │  Input Client  │       │ Output Client  │       │  (broadcasts   │
   │ (keyboard →)   │       │ (← displays)   │       │ ColoredChar)   │
   └────────────────┘       └────────────────┘       └────────────────┘
```

## Components

### Server (`server.rs`)
- Central hub with `KeystrokeProcessor` actor
- Receives keystrokes from input client (fire-and-forget)
- Broadcasts `ColorRequest` with correlation ID to R/G/B clients
- Correlates responses from all three color clients
- Broadcasts completed `ColoredCharacter` to output client

### Color Clients (`color_client.rs`)
- Three instances: R, G, and B (run with `--component R|G|B`)
- Subscribe to `ColorRequest` push notifications
- Generate random 0-255 value for their color component
- Send `ColorResponse` back to server via request-response

### Input Client (`input_client.rs`)
- Captures raw keyboard input using crossterm
- Streams each keystroke to the server
- Fire-and-forget pattern (no response expected)

### Output Client (`output_client.rs`)
- Subscribes to `ColoredCharacter` push notifications
- Displays characters with true RGB ANSI color codes
- Shows the character colored by the randomly generated RGB values

## IPC Patterns Demonstrated

1. **Fire-and-Forget**: Input client sends keystrokes without waiting for response
2. **Broadcast/Subscription**: Server broadcasts to R/G/B clients; output client subscribes
3. **Request-Response with Correlation**: Color clients respond with matching correlation IDs
4. **Push Notifications**: Server pushes colored characters to output client

## Requirements

- **tmux** (recommended) - For the convenience scripts that launch all components in a split-pane view
  ```bash
  # Arch Linux
  sudo pacman -S tmux

  # Debian/Ubuntu
  sudo apt install tmux

  # macOS
  brew install tmux
  ```

## Running the Example

### Quick Start with tmux (Recommended)

The easiest way to run this example is with the provided scripts, which launch
all 6 components in a single tmux session with split panes:

```bash
cd acton-reactive/examples/rgb_keyboard

# Start all services in a tmux split-pane layout
./start.sh

# Navigate between panes: Ctrl+b then arrow keys
# Type in the INPUT CLIENT pane to see colored output
# Press ESC in the input pane to quit (closes the session)

# Or from another terminal, force stop everything:
./stop.sh
```

### Manual Start (Without tmux)

If you don't have tmux, you can start each component manually in **6 separate
terminal windows**. Start them in this order:

```bash
# Terminal 1: Start the server first
cargo run --example rgb_keyboard_server --features ipc

# Terminal 2: Start the R color client
cargo run --example rgb_keyboard_color_client --features ipc -- --component R

# Terminal 3: Start the G color client
cargo run --example rgb_keyboard_color_client --features ipc -- --component G

# Terminal 4: Start the B color client
cargo run --example rgb_keyboard_color_client --features ipc -- --component B

# Terminal 5: Start the output client (displays colored characters)
cargo run --example rgb_keyboard_output_client --features ipc

# Terminal 6: Start the input client (type here!)
cargo run --example rgb_keyboard_input_client --features ipc
```

Press `Ctrl+C` in each terminal to stop the components when done.

## Expected Behavior

1. Type characters in the input client terminal
2. Each keystroke triggers the following flow:
   - Input client sends keystroke to server
   - Server broadcasts `ColorRequest` to all color clients
   - R, G, B clients each respond with random 0-255 values
   - Server correlates all three responses
   - Server broadcasts `ColoredCharacter` to output client
3. Output client displays each character with the random RGB color
4. Press `Esc` to quit the input client

## Message Flow

```
InputClient                Server               R/G/B Clients        OutputClient
     │                        │                       │                    │
     │──Keystroke('a')───────>│                       │                    │
     │                        │                       │                    │
     │                        │──ColorRequest(id)────>│                    │
     │                        │      (broadcast)      │                    │
     │                        │                       │                    │
     │                        │<──ColorResponse(R)────│                    │
     │                        │<──ColorResponse(G)────│                    │
     │                        │<──ColorResponse(B)────│                    │
     │                        │                       │                    │
     │                        │──ColoredChar('a',RGB)─────────────────────>│
     │                        │     (broadcast)       │                    │
     │                        │                       │           [displays 'a']
```

## Technical Details

- Uses Unix domain sockets for IPC
- MTI library generates time-ordered UUIDv7 correlation IDs
- Pending requests timeout after 5 seconds
- True RGB colors using ANSI escape sequences (`\x1b[38;2;R;G;Bm`)
