# Runtime Patch Scripts

These scripts move Paseo runtime changes between Polkadot SDK versions.

- `create_runtime_patch.sh`: Create patches from Polkadot runtime changes.
- `apply_runtime_patch.sh`: Apply one patch file.
- `revert_unwanted_changes.py`: Revert configured changes that should not land in Paseo.

## Create Runtime Patches

Create patch files for Paseo-specific changes between two Polkadot runtime
versions.

Usage:

```bash
./scripts/create_runtime_patch.sh <current_version> <new_version> [--paseo-ref-branch <branch>] [--parachains]
```

Parameters:

- `current_version`: The current Paseo runtime version
- `new_version`: The new Polkadot runtime version to update to
- `--paseo-ref-branch`: Branch to clone for the Paseo runtime. Defaults to `main`.
- `--parachains`: Also process system parachains.

Example:

```bash
# Without processing parachains
./scripts/create_runtime_patch.sh 1.2.3 1.2.4

# With processing parachains
./scripts/create_runtime_patch.sh 1.2.3 1.2.4 --parachains
```

The script writes these files under `patches`:

- `0001-Update-to-polkadot-relay-${NEXT_TAG}.patch`: Relay runtime and `Cargo.toml` changes.
- `system-parachains/${parachain_name}/0001-update-to-${parachain_name}-${NEXT_TAG}.patch`: Per-parachain patch when `--parachains` is set.
- `system-parachains/0001-update-to-parachains-${NEXT_TAG}.patch`: Shared system parachain constants and `Cargo.toml` changes.

## Apply a Patch

Usage:

```bash
./scripts/apply_runtime_patch.sh [--check] <patch_file>
```

Parameters:

- `--check`: Dry run. Validate the patch without changing files.
- `<patch_file>`: Patch file to apply.

Examples:

```bash
# Apply a patch
./scripts/apply_runtime_patch.sh ../patches/relay_polkadot.patch

# Check if a patch can be applied (dry run)
./scripts/apply_runtime_patch.sh --check ../patches/relay_polkadot.patch
```

After a successful apply, the script unstages the changes so they can be
reviewed before commit.

## Revert Unwanted Changes

Revert configured changes that should not be carried into Paseo.

Usage:

```bash
python revert_unwanted_changes.py <config.json>
```

Example:

```bash
python revert_unwanted_changes.py replacement_config.json
```

The script writes its log to `revert_unwanted_changes.log` in the current
directory.
