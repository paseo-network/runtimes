#!/bin/bash

# Script to apply a patch and run revert_unwanted_changes.py on new files
# Usage: ./apply_patch_and_revert.sh <patch_file> <config.json>

set -e

if [ $# -ne 2 ]; then
    echo "Usage: $0 <patch_file> <config.json>"
    exit 1
fi

PATCH_FILE="$1"
CONFIG_FILE="$2"

# Apply the patch
echo "Applying patch: $PATCH_FILE"
git apply "$PATCH_FILE"

# Run the revert script (it now handles both tracked and untracked files)
echo "Running revert_unwanted_changes.py..."
python scripts/revert_unwanted_changes.py "$CONFIG_FILE"

echo "Done! Patch applied and unwanted changes reverted."