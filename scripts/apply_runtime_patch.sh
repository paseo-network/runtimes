#!/bin/bash
set -e

# Check if the correct number of arguments is provided
if [ "$#" -ne 1 ]; then
    echo "Usage: $0 <patch_file_path>"
    exit 1
fi

PATCH_FILE=$1

# Check if the patch file exists
if [ ! -f "$PATCH_FILE" ]; then
    echo "Error: Patch file '$PATCH_FILE' does not exist."
    exit 1
fi

# Print success message only if the patch was applied correctly
if git apply --3way $PATCH_FILE; then
    echo "Patch applied successfully!"
else
    echo "Failed to apply patch."
    exit 1
fi
