#!/usr/bin/env bash
# IPC Fruit Market Example - Start Script
#
# Starts all components of the fruit market demo in a tmux session
# with a split-pane layout so you can see all outputs at once.
#
# Usage: ./start.sh
# Press 'q' or ESC in the keyboard pane to quit, then the session will close.
#
# Requires: tmux

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SESSION_NAME="ipc_fruit_market"

# Check for tmux
if ! command -v tmux &> /dev/null; then
    echo "Error: tmux is required but not installed."
    echo "Install with: sudo pacman -S tmux (Arch) or sudo apt install tmux (Debian/Ubuntu)"
    exit 1
fi

# Kill existing session if it exists
tmux kill-session -t "$SESSION_NAME" 2>/dev/null || true

echo "========================================"
echo "  IPC Fruit Market Example"
echo "========================================"
echo
echo "Building examples..."
cd "$PROJECT_ROOT"
cargo build --examples --features ipc --quiet
echo "Build complete!"
echo
echo "Starting tmux session: $SESSION_NAME"
echo

# Create new tmux session with the server in the first pane
tmux new-session -d -s "$SESSION_NAME" -n "ipc_fruit_market" \
    "echo '=== SERVER ===' && cargo run --example ipc_fruit_market_server --features ipc; read -p 'Press enter to close...'"

# Wait for the server to bind its socket
sleep 2

# Split horizontally for the display client (right side)
tmux split-window -h -t "$SESSION_NAME" \
    "echo '=== DISPLAY CLIENT ===' && cargo run --example ipc_fruit_market_display --features ipc; read -p 'Press enter to close...'"

# Give the display client time to subscribe before scans start
sleep 1

# Split the left pane vertically for the keyboard client
tmux select-pane -t "$SESSION_NAME:0.0"
tmux split-window -v -t "$SESSION_NAME" \
    "echo '=== KEYBOARD CLIENT (Press s to scan, ? for help, q/ESC to quit) ===' && cargo run --example ipc_fruit_market_keyboard --features ipc; tmux kill-session -t $SESSION_NAME"

# Select the keyboard client pane so typing goes there
tmux select-pane -t "$SESSION_NAME:0.1"

echo "Attaching to tmux session..."
echo "Layout: Server and display client in split panes"
echo "Type in the KEYBOARD CLIENT pane. Press 's' to scan, 'q' or ESC to quit."
echo
tmux attach-session -t "$SESSION_NAME"
