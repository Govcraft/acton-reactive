#!/bin/bash

# Ensure the script stops on errors
set -e

# Define the filter-repo command with Python code inline
git filter-repo --commit-callback "
import os

# Read the bad commits from the file
with open('bad_commits.txt', 'rb') as f:
    bad_commits = set(line.strip() for line in f)

# Print the first bad commit for debugging purposes
print(f'First bad commit in set: {next(iter(bad_commits))}')

# Print the current commit details
print(f'Current commit original_id: {commit.original_id}, type: {type(commit.original_id)}')

# Check if the current commit matches any in the bad commits list
if commit.original_id in bad_commits:
    print(f'Found match: {commit.original_id}')
    commit.skip()
"
