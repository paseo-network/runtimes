# Runtime Patch Scripts

This directory contains scripts for managing runtime patches:

1. `create_runtime_patch.sh`: Creates a patch file with the new code from polkadot repo.
2. `apply_runtime_patch.py`: Applies patches while preserving specific content.
3. `revert_unwanted_changes.py`: Reverts specific changes in files.

## Creating the runtime patch file

Creates a patch file for Paseo-specific modifications based on the differences between Polkadot runtime versions.

   ```bash
   ./scripts/create_runtime_patch.sh <current_version> <new_version>
   ```

Example:

   ```bash
   ./scripts/create_runtime_patch.sh 1.2.3 1.2.4
   ```

This script will create a patch file named `paseo_specific_changes.patch` in the `patches` directory.

## Apply the patch

Usage:

```bash
python apply_runtime_patch.py [--check] <patch_file>

The --check option performs a dry run of the patch application process
```

Examples:

```bash
# Apply a patch
python apply_runtime_patch.py ../patches/paseo_specific_changes

# Check if a patch can be applied
python apply_runtime_patch.py --check ../patches/paseo_specific_changes
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
