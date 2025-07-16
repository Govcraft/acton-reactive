#!/bin/bash

# Check if GNU Parallel is installed
if ! command -v parallel &>/dev/null; then
  echo "GNU Parallel is required but not installed. Please install it first."
  exit 1
fi

# Store the current branch to return to it later
CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD)

# Store the project root directory
PROJECT_ROOT=$(git rev-parse --show-toplevel)
cd "$PROJECT_ROOT" || exit

# File to store bad commits - using absolute path
BAD_COMMITS_FILE="${PROJECT_ROOT}/bad_commits.txt"
# Log file for debugging
LOG_FILE="${PROJECT_ROOT}/check_progress.log"

# Create bad commits file if it doesn't exist
touch "$BAD_COMMITS_FILE"

# Create a semaphore directory for tracking processed commits
SEMAPHORE_DIR="${PROJECT_ROOT}/.semaphores"
mkdir -p "$SEMAPHORE_DIR"

# Mutex for appending to bad_commits file
LOCK_FILE="${PROJECT_ROOT}/.bad_commits.lock"

# Logging function - only writes to log file, doesn't print to stdout
log_message() {
  local timestamp=$(date '+%Y-%m-%d %H:%M:%S')
  echo "[${timestamp}] $1" >>"$LOG_FILE"
}

# Function to safely append to bad_commits file
append_bad_commit() {
  local commit=$1
  (
    # Try to acquire lock
    flock -x 200
    # Check if commit is already in the file
    if ! grep -q "^${commit}$" "$BAD_COMMITS_FILE"; then
      echo "$commit" >>"$BAD_COMMITS_FILE"
    fi
  ) 200>"$LOCK_FILE"
}

# Function to process a single commit
process_commit() {
  local commit=$1
  local total_commits=$2
  local index=$3
  local temp_dir="${PROJECT_ROOT}/temp_check_${commit:0:8}_$$"
  local tmp_output="${PROJECT_ROOT}/.tmp_bad_commits_${commit:0:8}"

  # Check if commit is already in bad_commits file
  if grep -q "^${commit}$" "$BAD_COMMITS_FILE"; then
    log_message "Commit ${commit:0:8} already marked as bad, skipping ($index of $total_commits)"
    printf "\r✓ %d of %d (Bad commits so far: %d)" "$index" "$total_commits" "$(wc -l <"$BAD_COMMITS_FILE")"
    return 0
  fi

  # Check if output file already exists
  if [ -f "$tmp_output" ]; then
    log_message "Output file exists for commit ${commit:0:8}, adding to bad_commits ($index of $total_commits)"
    append_bad_commit "$commit"
    rm -f "$tmp_output" # Clean up tmp file after adding to bad_commits
    printf "\r✓ %d of %d (Bad commits so far: %d)" "$index" "$total_commits" "$(wc -l <"$BAD_COMMITS_FILE")"
    return 0
  fi

  # Log start of commit processing
  log_message "Starting process for commit ${commit:0:8} ($index of $total_commits)"

  # Check if this commit has already been processed via semaphore
  if [ -f "${SEMAPHORE_DIR}/${commit}" ]; then
    log_message "Commit ${commit:0:8} already processed, skipping ($index of $total_commits)"
    return 0
  fi

  # Create semaphore file to mark this commit as being processed
  touch "${SEMAPHORE_DIR}/${commit}"

  # Create a worktree silently
  if ! git worktree add --quiet --force "$temp_dir" "$commit" 2>/dev/null; then
    log_message "Failed to create worktree for commit ${commit:0:8} ($index of $total_commits)"
    rm -f "${SEMAPHORE_DIR}/${commit}"
    return 1
  fi

  # Change to the temporary directory
  cd "$temp_dir" >/dev/null 2>&1 || exit

  # Run direnv and log its execution
  log_message "Running direnv for commit ${commit:0:8} ($index of $total_commits)"
  if DIRENV_LOG_FORMAT="" direnv allow . >/dev/null 2>&1; then
    eval "$(DIRENV_LOG_FORMAT="" direnv export bash 2>/dev/null)"
    log_message "direnv setup successful for ${commit:0:8} ($index of $total_commits)"
  else
    log_message "direnv setup failed for ${commit:0:8} ($index of $total_commits)"
  fi

  # Log cargo check start
  log_message "Starting cargo check for acton-reactive in commit ${commit:0:8} ($index of $total_commits)"

  # Run cargo check for acton-reactive project with timeout and capture output and exit code
  if ! timeout --foreground --kill-after=35s 30s cargo check --quiet --manifest-path "$PROJECT_ROOT/acton-reactive/Cargo.toml" --target-dir "${PROJECT_ROOT}/target" >"$tmp_output" 2>&1; then
    local exit_code=$?
    log_message "Cargo check output for ${commit:0:8} in acton-reactive:"
    log_message "$(cat "$tmp_output")"
    if [ "$exit_code" -ne 124 ] && [ "$exit_code" -ne 137 ]; then
      printf "\r✗ %d of %d (Bad commits so far: %d)" "$index" "$total_commits" "$(wc -l <"$BAD_COMMITS_FILE")"
      log_message "Cargo check failed for ${commit:0:8} ($index of $total_commits) in acton-reactive with exit code $exit_code"
      # Add to bad_commits file directly only if cargo check actually fails
      append_bad_commit "$commit"
    fi
  else
    printf "\r✓ %d of %d (Bad commits so far: %d)" "$index" "$total_commits" "$(wc -l <"$BAD_COMMITS_FILE")"
    log_message "Cargo check passed for ${commit:0:8} ($index of $total_commits) in acton-reactive"
  fi

  # Cleanup silently
  cd "$PROJECT_ROOT" >/dev/null 2>&1 || exit
  git worktree remove --force "$temp_dir" 2>/dev/null
  rm -rf "$temp_dir" >/dev/null 2>&1

  # Log completion
  log_message "Completed processing commit ${commit:0:8} ($index of $total_commits)"
}

export -f process_commit
export -f log_message
export -f append_bad_commit
export PROJECT_ROOT
export SEMAPHORE_DIR
export LOG_FILE
export BAD_COMMITS_FILE
export LOCK_FILE

# Start logging
log_message "Starting script execution"
log_message "Project root: $PROJECT_ROOT"

# Read commits into array silently
readarray -t COMMIT_ARRAY < <(git log --reverse --format="%H")
TOTAL_COMMITS=${#COMMIT_ARRAY[@]}
log_message "Found $TOTAL_COMMITS commits to process"

# Clean up only semaphores
rm -rf "$SEMAPHORE_DIR"
mkdir -p "$SEMAPHORE_DIR"
log_message "Cleaned up and recreated semaphore directory"

# Run the checks serially
for index in "${!COMMIT_ARRAY[@]}"; do
  commit=${COMMIT_ARRAY[$index]}
  process_commit "$commit" "$TOTAL_COMMITS" "$((index + 1))"
done

echo -e "\nCheck complete!"
log_message "Script execution completed"

# Print summary
BAD_COMMIT_COUNT=$(wc -l <"$BAD_COMMITS_FILE")
log_message "Found $BAD_COMMIT_COUNT commits that failed cargo check"
echo "Found $BAD_COMMIT_COUNT commits that failed cargo check"
if [ "$BAD_COMMIT_COUNT" -gt 0 ]; then
  echo "See $BAD_COMMITS_FILE for details"
fi

# Cleanup only semaphores at the end
rm -rf "$SEMAPHORE_DIR"
rm -f "$LOCK_FILE"
git checkout --quiet "$CURRENT_BRANCH"
log_message "Cleanup completed"
log_message "Script execution finished"
