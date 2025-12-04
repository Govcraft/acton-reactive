#!/usr/bin/env bash
# RGB Keyboard Example - Start Script
#
# Starts all components of the RGB keyboard demo in a tmux session
# with a nice split-pane layout so you can see all outputs.
#
# Usage: ./start.sh
# Press ESC in the input pane to quit, then the session will close.
#
# Requires: tmux

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SESSION_NAME="rgb_keyboard"

# Check for tmux
if ! command -v tmux &> /dev/null; then
    echo "Error: tmux is required but not installed."
    echo "Install with: sudo pacman -S tmux (Arch) or sudo apt install tmux (Debian/Ubuntu)"
    exit 1
fi

# Kill existing session if it exists
tmux kill-session -t "$SESSION_NAME" 2>/dev/null || true

echo "========================================"
echo "  RGB Keyboard Example"
echo "========================================"
echo
echo "Building examples..."
cd "$PROJECT_ROOT"
cargo build --examples --features ipc --quiet
echo "Build complete!"
echo
echo "Starting tmux session: $SESSION_NAME"
echo

# Create new tmux session with server in first pane
tmux new-session -d -s "$SESSION_NAME" -n "rgb_keyboard" \
    "echo '=== SERVER ===' && cargo run --example rgb_keyboard_server --features ipc; read -p 'Press enter to close...'"

# Wait for server to start
sleep 1

# Split horizontally for R client (top-right area)
tmux split-window -h -t "$SESSION_NAME" \
    "echo '=== R CLIENT ===' && cargo run --example rgb_keyboard_color_client --features ipc -- --component R; read -p 'Press enter to close...'"

# Split the right pane vertically for G client
tmux split-window -v -t "$SESSION_NAME" \
    "echo '=== G CLIENT ===' && cargo run --example rgb_keyboard_color_client --features ipc -- --component G; read -p 'Press enter to close...'"

# Split again for B client
tmux split-window -v -t "$SESSION_NAME" \
    "echo '=== B CLIENT ===' && cargo run --example rgb_keyboard_color_client --features ipc -- --component B; read -p 'Press enter to close...'"

# Wait for color clients to connect
sleep 0.5

# Go back to the left pane (server) and split it
tmux select-pane -t "$SESSION_NAME:0.0"
tmux split-window -v -t "$SESSION_NAME" \
    "echo '=== OUTPUT CLIENT ===' && cargo run --example rgb_keyboard_output_client --features ipc; read -p 'Press enter to close...'"

# Split again for input client at the bottom-left
tmux split-window -v -t "$SESSION_NAME" \
    "echo '=== INPUT CLIENT (Type here! Press ESC to quit) ===' && cargo run --example rgb_keyboard_input_client --features ipc; tmux kill-session -t $SESSION_NAME"

# Select the input client pane
tmux select-pane -t "$SESSION_NAME:0.5"

# Attach to the session
echo "Attaching to tmux session..."
echo "Layout: Server and clients in split panes"
echo "Type in the INPUT CLIENT pane. Press ESC to quit."
echo
tmux attach-session -t "$SESSION_NAME"
