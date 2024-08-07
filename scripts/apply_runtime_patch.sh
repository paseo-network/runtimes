#!/bin/bash
set -e

# Function to display usage
usage() {
    echo "Usage: $0 [--check] <patch_file_path>"
    exit 1
}

# Check if the correct number of arguments is provided
if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    usage
fi

# Initialize variables
CHECK_FLAG=""
PATCH_FILE=""

# Parse arguments
if [ "$#" -eq 2 ]; then
    if [ "$1" == "--check" ]; then
        CHECK_FLAG="--check"
        PATCH_FILE=$2
    else
        usage
    fi
else
    PATCH_FILE=$1
fi

# Check if the patch file exists
if [ ! -f "$PATCH_FILE" ]; then
    echo "Error: Patch file '$PATCH_FILE' does not exist."
    exit 1
fi

# Print success message only if the patch was applied correctly
if git apply --3way $CHECK_FLAG $PATCH_FILE; then
    echo "Patch applied successfully!"
else
    echo "Failed to apply patch."
    exit 1
fi

# Unstage all files
echo "Unstaging all files..."
if git reset; then
    echo "All files unstaged successfully."
else
    echo "Failed to unstage files."
    exit 1
fi