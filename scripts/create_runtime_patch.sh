#!/bin/bash
set -e

# Define colors
RED=$(tput setaf 1)
GREEN=$(tput setaf 2)
BLUE=$(tput setaf 4)
WHITE=$(tput setaf 7)
RESET=$(tput sgr0)

# Define patch directory relative to the root of the repo
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
ROOT_DIR="$( cd "$SCRIPT_DIR/.." && pwd )"
PATCH_DIR="${ROOT_DIR}/patches"

# Function to print messages with colors
print_message() {
    local message=$1
    local color=$2
    echo "${color}${message}${RESET}"
}

# Initialize default values
PASEO_BRANCH="main"
PROCESS_PARACHAINS="false"

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --paseo-ref-branch)
            PASEO_BRANCH="$2"
            shift 2
            ;;
        --parachains)
            PROCESS_PARACHAINS="true"
            shift
            ;;
        *)
            if [ -z "$NEXT_TAG" ]; then
                NEXT_TAG="$1"
            else
                echo "Error: Unexpected argument '$1'"
                exit 1
            fi
            shift
            ;;
    esac
done

# Check if required argument is provided
if [ -z "$NEXT_TAG" ]; then
    echo "Usage: $0 <new_polkadot_runtime_version> [--paseo-ref-branch <branch>] [--parachains]"
    echo "--paseo-ref-branch: Optional. Specify the branch to clone for Paseo runtime. Defaults to 'main'."
    echo "--parachains: Optional. Process parachains if specified."
    exit 1
fi

POLKADOT_NEXT_TAG=v${NEXT_TAG}

print_message "========================================" "${GREEN}"
print_message "Creating patches for Polkadot ${POLKADOT_NEXT_TAG}" "${GREEN}"
print_message "Paseo reference branch: ${PASEO_BRANCH}" "${GREEN}"
print_message "Parachains processing: ${PROCESS_PARACHAINS}" "${GREEN}"
print_message "========================================" "${GREEN}"

rm -rf .tmp_runtime
mkdir .tmp_runtime
cd .tmp_runtime

print_message "----- Cloning repositories -----" "${BLUE}"
print_message "Cloning polkadot-fellows/runtimes branch: ${POLKADOT_NEXT_TAG}" "${BLUE}"
git clone -q --depth 1 --branch ${POLKADOT_NEXT_TAG} https://github.com/polkadot-fellows/runtimes.git polkadot_runtime_next

print_message "Cloning paseo-network/runtimes branch: ${PASEO_BRANCH}" "${BLUE}"
git clone -q --depth 1 --branch ${PASEO_BRANCH} https://github.com/paseo-network/runtimes.git paseo_runtime

cd paseo_runtime

print_message "----- Copying new Polkadot runtime to Paseo -----" "${BLUE}"
rm -rf relay/paseo/*
rm -rf relay/common/*
cp -rf ../polkadot_runtime_next/relay/polkadot/* relay/paseo/.
cp -rf ../polkadot_runtime_next/relay/common/* relay/common/.
cp -f ../polkadot_runtime_next/Cargo.toml ./

print_message "----- Copying new Polkadot chain-spec-generator to Paseo -----" "${BLUE}"
rm -rf chain-spec-generator/*
cp -rf ../polkadot_runtime_next/chain-spec-generator/* chain-spec-generator/.

print_message "----- Copying new Polkadot integration tests to Paseo -----" "${BLUE}"
rm -rf integration-tests/*
mkdir -p integration-tests/emulated/chains/relays/paseo
mkdir -p integration-tests/emulated/chains/parachains/assets/asset-hub-paseo
mkdir -p integration-tests/emulated/chains/parachains/bridges/bridge-hub-paseo
mkdir -p integration-tests/emulated/chains/parachains/people/people-paseo
mkdir -p integration-tests/emulated/chains/parachains/coretime/coretime-paseo
mkdir -p integration-tests/emulated/chains/parachains/testing/penpal
mkdir -p integration-tests/emulated/helpers
mkdir -p integration-tests/emulated/tests/coretime/coretime-paseo
mkdir -p integration-tests/emulated/tests/bridges/bridge-hub-paseo
mkdir -p integration-tests/emulated/networks/paseo-system

cp -rf ../polkadot_runtime_next/integration-tests/emulated/chains/relays/polkadot/* integration-tests/emulated/chains/relays/paseo
cp -rf ../polkadot_runtime_next/integration-tests/emulated/chains/parachains/assets/asset-hub-polkadot/* integration-tests/emulated/chains/parachains/assets/asset-hub-paseo
cp -rf ../polkadot_runtime_next/integration-tests/emulated/chains/parachains/bridges/bridge-hub-polkadot/* integration-tests/emulated/chains/parachains/bridges/bridge-hub-paseo
cp -rf ../polkadot_runtime_next/integration-tests/emulated/chains/parachains/people/people-polkadot/* integration-tests/emulated/chains/parachains/people/people-paseo
cp -rf ../polkadot_runtime_next/integration-tests/emulated/chains/parachains/coretime/coretime-polkadot/* integration-tests/emulated/chains/parachains/coretime/coretime-paseo
cp -rf ../polkadot_runtime_next/integration-tests/emulated/chains/parachains/testing/penpal/* integration-tests/emulated/chains/parachains/testing/penpal
cp -rf ../polkadot_runtime_next/integration-tests/emulated/helpers/* integration-tests/emulated/helpers
cp -rf ../polkadot_runtime_next/integration-tests/emulated/tests/coretime/coretime-polkadot/* integration-tests/emulated/tests/coretime/coretime-paseo
cp -rf ../polkadot_runtime_next/integration-tests/emulated/tests/bridges/bridge-hub-polkadot/* integration-tests/emulated/tests/bridges/bridge-hub-paseo
cp -rf ../polkadot_runtime_next/integration-tests/emulated/networks/polkadot-system/* integration-tests/emulated/networks/paseo-system

if [ "$PROCESS_PARACHAINS" = "true" ]; then
    print_message "----- Copying system-parachains files -----" "${BLUE}"
    cp ../polkadot_runtime_next/system-parachains/constants/Cargo.toml system-parachains/constants
    cp ../polkadot_runtime_next/system-parachains/constants/src/polkadot.rs system-parachains/constants/src/paseo.rs
    cp ../polkadot_runtime_next/system-parachains/constants/src/lib.rs system-parachains/constants/src/

    print_message "----- Copying specified parachains -----" "${BLUE}"
    PARACHAINS=(
        # Parachain name | source directory | destination directory
        "asset_hub  asset-hubs/asset-hub-polkadot   asset-hub-paseo"
        "bridge_hub bridge-hubs/bridge-hub-polkadot bridge-hub-paseo"
        "people     people/people-polkadot          people-paseo"
        "coretime   coretime/coretime-polkadot     coretime-paseo"
    )
    for parachain in "${PARACHAINS[@]}"; do
        read -r parachain_name source_dir dest_dir <<< "$parachain"
        source_dir="../polkadot_runtime_next/system-parachains/${source_dir}"
        dest_dir="system-parachains/${dest_dir}"
        if [ -d "$source_dir" ]; then
            mkdir -p "$dest_dir"
            cp -rf "$source_dir"/* "$dest_dir/"
            print_message "Copied ${parachain_name} from ${source_dir} to ${dest_dir}" "${WHITE}"
        else
            print_message "Warning: ${source_dir} not found for ${parachain_name}" "${RED}"
        fi

        # Create separate patch for each system-parachains
        outDir="${PATCH_DIR}/system-parachains/0001-update-${parachain_name}-${NEXT_TAG}.patch"
        mkdir -p "$(dirname "${outDir}")"
        git format-patch -1 HEAD --stdout -- system-parachains > "${outDir}"
        print_message "Created patch for system-parachains: ${outDir}" "${WHITE}"
    done
fi

print_message "----- Committing changes -----" "${BLUE}"
git add .
git commit -m "Update to Polkadot ${NEXT_TAG} runtime"

print_message "----- Creating patch files for Polkadot ${NEXT_TAG} runtime -----" "${WHITE}"
mkdir -p ${PATCH_DIR}

# Create targeted patch files
print_message "Creating targeted patch files..." "${WHITE}"

# Patch for relay/paseo
git format-patch -1 HEAD --stdout --root relay/paseo relay/common Cargo.toml > "${PATCH_DIR}/0001-update-relay-paseo-${NEXT_TAG}.patch"
print_message "Created patch for relay/paseo: ${PATCH_DIR}/0001-update-relay-paseo-${NEXT_TAG}.patch" "${WHITE}"

# Patch for chain-spec-generator
git format-patch -1 HEAD --stdout --root chain-spec-generator > "${PATCH_DIR}/0001-update-chain-spec-generator-${NEXT_TAG}.patch"
print_message "Created patch for chain-spec-generator: ${PATCH_DIR}/0001-update-chain-spec-generator-${NEXT_TAG}.patch" "${WHITE}"

# Patch for integration-tests
git format-patch -1 HEAD --stdout --root integration-tests > "${PATCH_DIR}/0001-update-integration-tests-${NEXT_TAG}.patch"
print_message "Created patch for integration-tests: ${PATCH_DIR}/0001-update-integration-tests-${NEXT_TAG}.patch" "${WHITE}"

if [ "$PROCESS_PARACHAINS" = "true" ]; then
    # Create separate patches for each system-parachain
    for parachain in "${PARACHAINS[@]}"; do
        read -r parachain_name source_dir dest_dir <<< "$parachain"
        dest_dir="system-parachains/${dest_dir}"
        outDir="${PATCH_DIR}/system-parachains/0001-update-${parachain_name}-${NEXT_TAG}.patch"
        mkdir -p "$(dirname "${outDir}")"
        git format-patch -1 HEAD --stdout --root "${dest_dir}" > "${outDir}"
        print_message "Created patch for ${parachain_name}: ${outDir}" "${WHITE}"
    done
fi

print_message "--------------------" "${BLUE}"
print_message "----- Patch files created in patches/ directory -----" "${WHITE}"
print_message "--------------------" "${BLUE}"