#/bin/bash
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

rm -rf tmp_runtime
mkdir tmp_runtime
cd tmp_runtime

print_message "----- Cloning paseo runtime repo -----" "${BLUE}"
git clone https://github.com/paseo-network/runtimes.git paseo_runtime

print_message "----- Cloning polkadot runtime repo -----" "${BLUE}"
git clone https://github.com/polkadot-fellows/runtimes.git polkadot_runtime

cd polkadot_runtime
POLKADOT_CURRENT_TAG=v${CURRENT_TAG}
print_message "----- Checking out tag ${POLKADOT_CURRENT_TAG} -----" "${WHITE}"
git checkout tags/${POLKADOT_CURRENT_TAG}

print_message "----- Copying runtime files to paseo folder -----" "${BLUE}"
cp -fr relay/polkadot/* ../paseo_runtime/relay/paseo/.

POLKADOT_NEXT_TAG=v${NEXT_TAG}
print_message "----- Checking out tag ${POLKADOT_NEXT_TAG} -----" "${WHITE}"
git checkout tags/${POLKADOT_NEXT_TAG}

print_message "----- Committing current polkadot runtime on paseo repo -----" "${BLUE}"
cd ../paseo_runtime
git switch -c tmp/${CURRENT_TAG}-runtime
git add .
git commit -m "go back from paseo to polkadot"

print_message "----- Revert and commit changes, leaving paseo specific changes -----" "${RED}"
git revert --no-edit HEAD
LATEST_COMMIT=$(git rev-parse HEAD)

print_message "----- Creating new branch from main on paseo repo -----" "${BLUE}"
git switch main
git switch -c release/${NEXT_TAG}-runtime

print_message "----- Committing new polkadot runtime into paseo repo -----" "${BLUE}"
rm -rf relay/paseo/*
cp -rf ../polkadot_runtime/relay/polkadot/* relay/paseo/.
git add .
git commit -m "initial polkadot ${NEXT_TAG} code"

print_message "----- Creating patch file for paseo specific modifications -----" "${WHITE}"
mkdir -p ../../patches
git diff ${LATEST_COMMIT} HEAD > ../../patches/paseo_specific_changes.patch

print_message "--------------------" "${RED}"
print_message "----- Patch file created: patches/paseo_specific_changes.patch -----" "${RED}"
print_message "----- Apply this patch file to integrate paseo specific changes -----" "${RED}"
print_message "--------------------" "${RED}"
