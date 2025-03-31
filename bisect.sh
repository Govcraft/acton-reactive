#!/bin/bash

# Set PATH to ensure all necessary commands are available
export PATH="/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$HOME/.cargo/bin"

# Setup logging
PROJECT_ROOT=$(git rev-parse --show-toplevel)
LOG_FILE="${PROJECT_ROOT}/test_progress.log"

# Logging function - only writes to log file, not stdout
log_message() {
  local timestamp=$(date '+%Y-%m-%d %H:%M:%S')
  echo "[${timestamp}] $1" >>"$LOG_FILE"
}

# Get the current commit hash for logging
CURRENT_COMMIT=$(git rev-parse HEAD)
log_message "Starting test for commit ${CURRENT_COMMIT:0:8}"

# Try to find Cargo.toml using git ls-tree
CARGO_TOML_PATH=""
if CARGO_TOML_FILE=$(git ls-tree -r --name-only HEAD | grep -m 1 'Cargo.toml$'); then
  CARGO_TOML_PATH="$PROJECT_ROOT/$CARGO_TOML_FILE"
else
  printf "\r❌ Cargo.toml not found in any expected location for commit ${CURRENT_COMMIT:0:8}\n"
  log_message "Cargo.toml not found for commit ${CURRENT_COMMIT:0:8}"
  exit 125 # Exit with 125 to tell git bisect to skip this commit
fi

# Attempt to build the specific project in the workspace
printf "\r🛠️ Checking acton-reactive project at $CARGO_TOML_PATH..."
log_message "Starting cargo check for acton-reactive at $CARGO_TOML_PATH"

if ! cargo check --quiet --manifest-path "$CARGO_TOML_PATH" --target-dir "${PROJECT_ROOT}/target"; then
  printf "\r❌ Build failed for commit ${CURRENT_COMMIT:0:8}\n"
  log_message "Build failed for commit ${CURRENT_COMMIT:0:8}"
  exit 1 # Mark this commit as bad for bisecting
fi

printf "\r✅ Build successful for commit ${CURRENT_COMMIT:0:8}\n"
log_message "Build successful for commit ${CURRENT_COMMIT:0:8}"

# Run tests for the specific project
printf "\r🔄 Running tests for acton-reactive at $CARGO_TOML_PATH..."
log_message "Starting cargo test for acton-reactive at $CARGO_TOML_PATH"

if cargo test --release --quiet --manifest-path "$CARGO_TOML_PATH"; then
  printf "\r✅ Tests passed for commit ${CURRENT_COMMIT:0:8}\n"
  log_message "Tests passed for commit ${CURRENT_COMMIT:0:8}"
  exit 0 # Mark this commit as good for bisecting
else
  printf "\r❌ Tests failed for commit ${CURRENT_COMMIT:0:8}\n"
  log_message "Tests failed for commit ${CURRENT_COMMIT:0:8}"
  exit 1 # Mark this commit as bad for bisecting
fi
