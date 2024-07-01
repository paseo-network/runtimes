# Runtimes

Work In Progress runtime for Paseo Polkadot community testnet.

## Structure

Each leaf folder contains one runtime crate:

<!-- Run "tree -I 'target' -d -L 3" and then delete some folders from Paseo. -->

```pre
├── relay
│   ├── paseo
└── system-parachains
    ├── asset-hub-paseo
```


### Scripts

#### Creating the runtime patch file

1. Run:

   ```bash
   ./scripts/create_runtime_patch.sh <current_version> <new_version>
   ```

Example:

   ```bash
   ./scripts/create_runtime_patch.sh 1.2.3 1.4.0
   ```

#### Applying the runtime patch file

1. Run:

   ```bash
   ./scripts/apply_runtime_patch.sh <patch_file_path>
   ```

Example:

   ```bash
   ./scripts/apply_runtime_patch.sh ./patches/paseo_specific_changes.patch
   ```
