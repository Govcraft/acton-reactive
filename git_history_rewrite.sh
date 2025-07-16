#!/bin/bash

# Default configuration
BAD_COMMITS_FILE="bad_commits.txt"
DEBUG=${DEBUG:-false}
FORCE_YES=${FORCE_YES:-false}
TEMP_DIR=".git-rewrite-tmp"
FILTER_SCRIPT="$TEMP_DIR/filter.py" # Changed back to .py extension

# Helper function for debug output
debug() {
  if [ "$DEBUG" = "true" ]; then
    echo "DEBUG: $1" >&2
  fi
}

# Helper function for progress output
progress() {
  local current_time=$(date '+%H:%M:%S')
  echo "[$current_time] $1"
}

# Parse command line arguments
for arg in "$@"; do
  case "$arg" in
  --debug)
    DEBUG=true
    ;;
  --yes | -y)
    FORCE_YES=true
    ;;
  *)
    if [ "$arg" != "${arg#-}" ]; then
      echo "Error: Unknown option: $arg"
      exit 1
    fi
    BAD_COMMITS_FILE="$arg"
    ;;
  esac
done

debug "Starting script with options:"
debug "DEBUG=$DEBUG"
debug "BAD_COMMITS_FILE=$BAD_COMMITS_FILE"

# Check if we're in a git repository
if ! git rev-parse --git-dir >/dev/null 2>&1; then
  echo "Error: Not in a git repository"
  exit 1
fi

# Check if bad_commits.txt exists and isn't empty
if [ ! -f "$BAD_COMMITS_FILE" ]; then
  echo "Error: $BAD_COMMITS_FILE not found"
  exit 1
fi

if [ ! -s "$BAD_COMMITS_FILE" ]; then
  echo "Error: $BAD_COMMITS_FILE is empty"
  exit 1
fi

# Create temp directory
mkdir -p "$TEMP_DIR"

# Create a backup branch of current state
current_branch=$(git rev-parse --abbrev-ref HEAD)
backup_branch="backup_$(date +%Y%m%d_%H%M%S)"
progress "Creating backup branch: $backup_branch"
git branch "$backup_branch"

# Count total commits
total_bad_commits=$(grep -v '^#' "$BAD_COMMITS_FILE" | grep -v '^[[:space:]]*$' | wc -l)
progress "Found $total_bad_commits commits to remove"

# Create Python filter script
cat >"$FILTER_SCRIPT" <<'EOF'
#!/usr/bin/env python3

import os
import subprocess
import sys
from git_filter_repo import FastExportFilter, Commit, RepoFilter

def load_bad_commits():
    bad = set()
    with open('bad_commits.txt', 'r') as f:
        for line in f:
            line = line.strip()
            if line and not line.startswith('#'):
                bad.add(line)
    return bad

BAD_COMMITS = load_bad_commits()

def skip_commit(commit):
    if commit.original_id.hex() in BAD_COMMITS:
        debug = os.environ.get('DEBUG') == 'true'
        if debug:
            print(f"Skipping commit: {commit.original_id.hex()}", file=sys.stderr)
        return None
    return commit

args = RepoFilter.parse_args(['--force'])
filter = RepoFilter(args, commit_callback=skip_commit)
filter.run()
EOF

chmod +x "$FILTER_SCRIPT"

debug "Starting filter operation..."

if [ "$DEBUG" = "true" ]; then
  progress "Running filter with debug output"
  DEBUG=true uv python "$FILTER_SCRIPT"
else
  progress "Running filter..."
  uv python "$FILTER_SCRIPT"
fi

# Verify the rewrite
new_history_count=$(git rev-list HEAD --count)
progress "History rewrite complete:"
progress "- Original bad commits: $total_bad_commits"
progress "- Total commits now: $new_history_count"
progress "- Backup branch created: $backup_branch"

echo
echo "Next steps:"
echo "1. Verify the new history looks correct"
echo "2. Push changes: git push -f origin $current_branch"
echo "3. Delete backup if all is well: git branch -D $backup_branch"
echo
echo "To restore from backup if needed:"
echo "git reset --hard $backup_branch"

# Clean up
rm -rf "$TEMP_DIR"
