#!/usr/bin/env bash
# RGB Keyboard Example - Stop Script
#
# Stops the RGB keyboard tmux session and all its processes.
#
# Usage: ./stop.sh

SESSION_NAME="rgb_keyboard"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "========================================"
echo "  RGB Keyboard Example - Stopping..."
echo "========================================"
echo

# Check if tmux session exists
if tmux has-session -t "$SESSION_NAME" 2>/dev/null; then
    echo -e "${YELLOW}Killing tmux session: $SESSION_NAME${NC}"
    tmux kill-session -t "$SESSION_NAME"
    echo -e "${GREEN}Session killed.${NC}"
else
    echo -e "${YELLOW}No tmux session '$SESSION_NAME' found.${NC}"
    echo "Checking for any stray rgb_keyboard processes..."

    # Fallback: find processes by name
    FOUND=0
    for pattern in rgb_keyboard_server rgb_keyboard_color_client rgb_keyboard_output_client rgb_keyboard_input_client; do
        pids=$(pgrep -f "$pattern" 2>/dev/null || true)
        if [[ -n "$pids" ]]; then
            for pid in $pids; do
                echo -e "Stopping $pattern (PID: ${RED}$pid${NC})..."
                kill "$pid" 2>/dev/null || true
                FOUND=1
            done
        fi
    done

    if [[ $FOUND -eq 0 ]]; then
        echo -e "${GREEN}No rgb_keyboard processes found running.${NC}"
    else
        sleep 1
        echo -e "${GREEN}Processes stopped.${NC}"
    fi
fi

echo
echo "========================================"
echo -e "  ${GREEN}Shutdown complete!${NC}"
echo "========================================"
