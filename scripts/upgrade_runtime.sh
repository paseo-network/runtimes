#/bin/bash

set -e

read -p "Enter the current paseo runtime version (without v as prefix): " CURRENT_TAG
read -p "Enter the new polkadot runtime version (without v as prefix): " NEXT_TAG

rm -rf tmp_runtime
mkdir tmp_runtime
cd tmp_runtime

echo "\n----- Cloning paseo runtime repo -----"
git clone https://github.com/paseo-network/runtimes.git paseo_runtime

echo "\n----- Cloning polkadot runtime repo -----"
git clone https://github.com/polkadot-fellows/runtimes.git polkadot_runtime

cd polkadot_runtime
POLKADOT_CURRENT_TAG=v${CURRENT_TAG}
echo "\n----- Checking out tag ${POLKADOT_CURRENT_TAG} -----"
git checkout tags/${POLKADOT_CURRENT_TAG}


echo "\n----- Copying runtime files to paseo folder -----"
cp -fr relay/polkadot/* ../paseo_runtime/relay/paseo/.

POLKADOT_NEXT_TAG=v${NEXT_TAG}
echo "\n----- Checking out tag ${POLKADOT_NEXT_TAG} -----"
git checkout tags/${POLKADOT_NEXT_TAG}

echo "\n----- Commiting current polkadot runtime on paseo repo -----"
cd ../paseo_runtime
git switch -c tmp/${CURRENT_TAG}-runtime
git add .
git commit -m "go back from paseo to polkadot"

echo "\n----- Revert and commit changes, leaving paseo specific changes -----"
git revert --no-edit HEAD
LATEST_COMMIT=$(git rev-parse HEAD)


echo "\n----- Creating new branch from main on paseo repo -----"
git switch main
git switch -c feat/${NEXT_TAG}-runtime


echo "\n----- Commiting new polkadot runtime into paseo repo -----"
rm -rf relay/paseo/*
cp -rf ../polkadot_runtime/relay/polkadot/* relay/paseo/.
git add .
git commit -m "initial polkadot ${NEXT_TAG} code"

echo "\n----- Cherry picking paseo specific modifications from tmp branch -----"
git cherry-pick ${LATEST_COMMIT}

echo "\n--------------------"
echo "----- Now resolve conflicts, commit them, push branch, and create PR to main -----"
echo "--------------------"