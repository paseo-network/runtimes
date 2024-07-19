#!/bin/bash
set -e

# Define colors
RED=$(tput setaf 1)
WHITE=$(tput setaf 7)
BLUE=$(tput setaf 4)
RESET=$(tput sgr0)

# Define patch directory and file names
PATCH_DIR="../../patches"
RELAY_PATCH_FILE="${PATCH_DIR}/relay_polkadot.patch"

# Function to print messages with colors
print_message() {
    local message=$1
    local color=$2
    echo "${color}${message}${RESET}"
}

# Check if the correct number of arguments is provided
if [ "$#" -lt 2 ] || [ "$#" -gt 4 ]; then
    echo "Usage: $0 <current_paseo_runtime_version> <new_polkadot_runtime_version> [paseo_branch] [process_parachains]"
    echo "paseo_branch: Optional. Specify the branch to clone for Paseo runtime. Defaults to 'main'."
    echo "process_parachains: Optional. Set to 'true' to process parachains. Defaults to 'false'."
    exit 1
fi

# Define list of parachains to copy
# Format: "parachain_name origin_dir dest_dir"
# parachain_name: name of the parachain
# origin_dir: relative path in polkadot_runtime_next/system-parachains/
# dest_dir: relative path in paseo_runtime/system-parachains/
PARACHAINS=(
    "asset_hub asset-hubs/asset-hub-polkadot asset-hub-paseo"
)


CURRENT_TAG=$1
NEXT_TAG=$2
PASEO_BRANCH=${3:-main}
PROCESS_PARACHAINS=${4:-false}
POLKADOT_CURRENT_TAG=v${CURRENT_TAG}
POLKADOT_NEXT_TAG=v${NEXT_TAG}


print_message "========================================" "${GREEN}"
print_message "Creating patches from tag ${POLKADOT_CURRENT_TAG} to ${POLKADOT_NEXT_TAG}" "${GREEN}"
print_message "Parachains processing: ${PROCESS_PARACHAINS}" "${GREEN}"
print_message "========================================" "${GREEN}"

rm -rf tmp_runtime
mkdir tmp_runtime
cd tmp_runtime

print_message "----- Cloning repositories -----" "${BLUE}"
print_message "Cloning paseo-network/runtimes branch: ${PASEO_BRANCH}" "${BLUE}"
git clone -q --depth 1 --branch ${PASEO_BRANCH} https://github.com/paseo-network/runtimes.git paseo_runtime

print_message "Cloning polkadot-fellows/runtimes branch: ${POLKADOT_CURRENT_TAG}" "${BLUE}"
git clone -q --depth 1 --branch ${POLKADOT_CURRENT_TAG} https://github.com/polkadot-fellows/runtimes.git polkadot_runtime_current

print_message "Cloning polkadot-fellows/runtimes branch: ${POLKADOT_NEXT_TAG}" "${BLUE}"
git clone -q --depth 1 --branch ${POLKADOT_NEXT_TAG} https://github.com/polkadot-fellows/runtimes.git polkadot_runtime_next

print_message "----- Copying current Polkadot runtime to Paseo -----" "${BLUE}"
cp -fr polkadot_runtime_current/relay/polkadot/* paseo_runtime/relay/paseo/.

print_message "----- Creating temporary branch in Paseo repo -----" "${BLUE}"
cd paseo_runtime
git switch -q -c tmp/${CURRENT_TAG}-runtime
git add .
git commit -m "Revert to Polkadot ${CURRENT_TAG} runtime"

print_message "----- Reverting changes to keep Paseo-specific modifications -----" "${RED}"
git revert --no-edit HEAD
LATEST_COMMIT=$(git rev-parse HEAD)

print_message "----- Creating new branch for updated runtime -----" "${BLUE}"
git switch -q -c release/${NEXT_TAG}-runtime

print_message "----- Copying new Polkadot runtime to Paseo -----" "${BLUE}"
rm -rf relay/paseo/*
cp -rf ../polkadot_runtime_next/relay/polkadot/* relay/paseo/.
cp -f ../polkadot_runtime_next/Cargo.toml ./

if [ "$PROCESS_PARACHAINS" = "true" ]; then
    print_message "----- Copying system-parachains files -----" "${BLUE}"
    cp ../polkadot_runtime_next/system-parachains/constants/Cargo.toml system-parachains/constants
    cp ../polkadot_runtime_next/system-parachains/constants/src/polkadot.rs system-parachains/constants/src/paseo.rs
    cp ../polkadot_runtime_next/system-parachains/constants/src/lib.rs system-parachains/constants/src/

    print_message "Copied system-parachains files:" "${WHITE}"
    print_message "- Cargo.toml" "${WHITE}"
    print_message "- constants/src/paseo.rs (renamed from polkadot.rs)" "${WHITE}"
    print_message "- constants/src/lib.rs" "${WHITE}"


    print_message "----- Copying specified parachains -----" "${BLUE}"
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
    done
fi

git add .
git commit -m "Update to Polkadot ${NEXT_TAG} runtime"
if [ "$PROCESS_PARACHAINS" = "true" ]; then
    git commit --amend -m "Update to Polkadot ${NEXT_TAG} runtime and copy specified parachains"
fi

print_message "----- Creating patch files for Polkadot ${NEXT_TAG} runtime -----" "${WHITE}"
mkdir -p ${PATCH_DIR}

# Create patches for relay/paseo and Cargo.toml
if git format-patch -o ${PATCH_DIR} ${LATEST_COMMIT}..HEAD -- relay/paseo Cargo.toml; then
    print_message "Successfully created relay patch files in: ${PATCH_DIR}" "${WHITE}"
else
    print_message "Failed to create relay patch files" "${RED}"
fi

if [ "$PROCESS_PARACHAINS" = "true" ]; then
    # Create patches for each parachain
    for parachain in "${PARACHAINS[@]}"; do
        read -r parachain_name _ dest_dir <<< "$parachain"
        parachain_dir="system-parachains/${dest_dir}"
        if [ -d "$parachain_dir" ]; then
            if git format-patch -o ${PATCH_DIR} --prefix="${parachain_name}/" ${LATEST_COMMIT}..HEAD -- "$parachain_dir"; then
                print_message "Created patches for ${parachain_name}" "${WHITE}"
            else
                print_message "Failed to create patches for ${parachain_name}" "${RED}"
            fi
        else
            print_message "Warning: ${dest_dir} not found for ${parachain_name}, skipping patch creation" "${RED}"
        fi
    done

    # Create patches for system-parachains/constants and system-parachains/Cargo.toml
    if git format-patch -o ${PATCH_DIR} --prefix="system-parachains/" ${LATEST_COMMIT}..HEAD -- system-parachains/constants system-parachains/Cargo.toml; then
        print_message "Created patches for system-parachains/constants" "${WHITE}"
    else
        print_message "Failed to create patches for system-parachains/constants" "${RED}"
    fi
fi

print_message "--------------------" "${BLUE}"
print_message "----- Patch files created in patches/ directory -----" "${WHITE}"
print_message "----- Apply these patch files to integrate Paseo-specific changes -----" "${WHITE}"
print_message "--------------------" "${BLUE}"