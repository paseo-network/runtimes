# Runtime Patch Scripts

This directory contains scripts for managing runtime patches:

1. `create_runtime_patch.sh`: Creates patch files with the new code from the Polkadot repo.
2. `apply_runtime_patch.sh`: Applies patches.
3. `revert_unwanted_changes.py`: Reverts specific changes in files.

## Creating the runtime patch files

Creates patch files for Paseo-specific modifications based on the differences between Polkadot runtime versions.

Usage:

```bash
./scripts/create_runtime_patch.sh <current_version> <new_version> [--paseo-ref-branch <branch>] [--parachains]
```

Parameters:

- `current_version`: The current Paseo runtime version
- `new_version`: The new Polkadot runtime version to update to
- `--paseo-ref-branch`: Optional. Specify the branch to clone for Paseo runtime. Defaults to 'main'.
- `--parachains`: Optional. Process parachains if specified.

Example:

```bash
# Without processing parachains
./scripts/create_runtime_patch.sh 1.2.3 1.2.4

# With processing parachains
./scripts/create_runtime_patch.sh 1.2.3 1.2.4 --parachains
```

This script will create the following patch files in the `patches` directory:

- `0001-Update-to-polkadot-relay-${NEXT_TAG}.patch`: Contains changes for the relay chain and Cargo.toml
- `system-parachains/${parachain_name}/0001-update-to-${parachain_name}-${NEXT_TAG}.patch`: Created for each specified parachain if `--parachains` is set
- `system-parachains/0001-update-to-parachains-${NEXT_TAG}.patch`: Contains changes for the `system-parachains/constants` directory and `system-parachains/Cargo.toml` file

## Apply the patch

Usage:

```bash
./scripts/apply_runtime_patch.sh [--check] <patch_file>
```

Parameters:
- `--check`: Optional. Performs a dry run of the patch application process.
- `<patch_file>`: Path to the patch file to be applied.

Examples:

```bash
# Apply a patch
./scripts/apply_runtime_patch.sh ../patches/relay_polkadot.patch

# Check if a patch can be applied (dry run)
./scripts/apply_runtime_patch.sh --check ../patches/relay_polkadot.patch
```

This script will attempt to apply the specified patch file using Git's patch application functionality. If successful, it will unstage all changes, allowing you to review and commit them manually.

## Revert unwanted changes

Reverts specific changes in files based on predefined rules.

Usage:

```bash
python revert_unwanted_changes.py <config.json>
```

Example:

```bash
python revert_unwanted_changes.py replacement_config.json
```

The `revert_unwanted_changes.py` script logs its actions to `revert_unwanted_changes.log` in the current directory.
