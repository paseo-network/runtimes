#!/bin/bash
set -e

# Define colors
RED=$(tput setaf 1)
WHITE=$(tput setaf 7)
BLUE=$(tput setaf 4)
RESET=$(tput sgr0)

# Function to print messages with colors
print_message() {
    local message=$1
    local color=$2
    echo "${color}${message}${RESET}"
}

# Check if the correct number of arguments is provided
if [ "$#" -ne 2 ]; then
    echo "Usage: $0 <current_paseo_runtime_version> <new_polkadot_runtime_version>"
    exit 1
fi

CURRENT_TAG=$1
NEXT_TAG=$2
POLKADOT_CURRENT_TAG=v${CURRENT_TAG}
POLKADOT_NEXT_TAG=v${NEXT_TAG}

rm -rf tmp_runtime
mkdir tmp_runtime
cd tmp_runtime

print_message "----- Cloning repositories -----" "${BLUE}"
git clone --depth 1 https://github.com/paseo-network/runtimes.git paseo_runtime
git clone --depth 1 --branch ${POLKADOT_CURRENT_TAG} https://github.com/polkadot-fellows/runtimes.git polkadot_runtime_current
git clone --depth 1 --branch ${POLKADOT_NEXT_TAG} https://github.com/polkadot-fellows/runtimes.git polkadot_runtime_next

print_message "----- Copying current Polkadot runtime to Paseo -----" "${BLUE}"
cp -fr polkadot_runtime_current/relay/polkadot/* paseo_runtime/relay/paseo/.

print_message "----- Creating temporary branch in Paseo repo -----" "${BLUE}"
cd paseo_runtime
git switch -c tmp/${CURRENT_TAG}-runtime
git add .
git commit -m "Revert to Polkadot ${CURRENT_TAG} runtime"

print_message "----- Reverting changes to keep Paseo-specific modifications -----" "${RED}"
git revert --no-edit HEAD
LATEST_COMMIT=$(git rev-parse HEAD)

print_message "----- Creating new branch for updated runtime -----" "${BLUE}"
git switch main
git switch -c release/${NEXT_TAG}-runtime

print_message "----- Copying new Polkadot runtime to Paseo -----" "${BLUE}"
rm -rf relay/paseo/*
cp -rf ../polkadot_runtime_next/relay/polkadot/* relay/paseo/.
cp -f ../polkadot_runtime_next/Cargo.toml ./
git add .
git commit -m "Update to Polkadot ${NEXT_TAG} runtime"

print_message "----- Creating patch file for Paseo-specific modifications -----" "${WHITE}"
mkdir -p ../../patches
git diff ${LATEST_COMMIT} HEAD > ../../patches/paseo_specific_changes.patch

print_message "--------------------" "${BLUE}"
print_message "----- Patch file created: patches/paseo_specific_changes.patch -----" "${WHITE}"
print_message "----- Apply this patch file to integrate Paseo-specific changes -----" "${WHITE}"
print_message "--------------------" "${BLUE}"