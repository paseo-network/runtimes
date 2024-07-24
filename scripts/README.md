# Runtime Patch Scripts

This directory contains scripts for managing runtime patches:

1. `create_runtime_patch.sh`: Creates patch files with the new code from the Polkadot repo.
2. `apply_runtime_patch.py`: Applies patches while preserving specific content.
3. `revert_unwanted_changes.py`: Reverts specific changes in files.

## Creating the runtime patch files

Creates patch files for Paseo-specific modifications based on the differences between Polkadot runtime versions.

Usage:

```bash
./scripts/create_runtime_patch.sh <current_version> <new_version> [process_parachains]
```

Parameters:

- `current_version`: The current Paseo runtime version
- `new_version`: The new Polkadot runtime version to update to
- `process_parachains`: Optional. Set to 'true' to process parachains. Defaults to 'false'.

Example:

```bash
# Without processing parachains
./scripts/create_runtime_patch.sh 1.2.3 1.2.4

# With processing parachains
./scripts/create_runtime_patch.sh 1.2.3 1.2.4 true
```

This script will create the following patch files in the `patches` directory:

- `relay_polkadot.patch`: Contains changes for the relay chain and Cargo.toml
- `parachain_<name>.patch`: Created for each specified parachain if `process_parachains` is set to true
- `system_parachains_common.patch`: Contains changes for the `system-parachains/constants` directory and `system-parachains/Cargo.toml` file

## Apply the patch

Usage:

```bash
python apply_runtime_patch.py [--check] <patch_file>

The --check option performs a dry run of the patch application process
```

Examples:

```bash
# Apply a patch
python apply_runtime_patch.py ../patches/relay_polkadot.patch

# Check if a patch can be applied
python apply_runtime_patch.py --check ../patches/relay_polkadot.patch
```

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

Both `apply_runtime_patch.py` and `revert_unwanted_changes.py` scripts log their actions to `apply_patch.log` and `revert_unwanted_changes.log` respectively in the current directory.
