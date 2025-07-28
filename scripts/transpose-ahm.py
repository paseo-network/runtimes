#!/usr/bin/env python3

import os
import re
import sys
import argparse
from pathlib import Path
from typing import Dict, List, Tuple
from git import Repo

class RuntimeTransposer:
    def __init__(self, source_root: str, target_root: str):
        self.source_root = Path(source_root)
        self.target_root = Path(target_root)
        self.substitutions: Dict[str, List[Tuple[str, str]]] = {}

    def print_git_info(self):
        """Print git branch and commit of source root."""
        try:
            repo = Repo(self.source_root)
            branch = repo.active_branch.name
            commit = repo.head.commit.hexsha
            
            print(f"Copying runtimes from {branch} at commit {commit}")
            
        except Exception as e:
            print(f"Error getting git info: {e}")

    def add_substitution(self, path_pattern: str, pattern: str, replacement: str):
        """Add a substitution rule for files matching a path pattern (regex)."""
        if path_pattern not in self.substitutions:
            self.substitutions[path_pattern] = []
        self.substitutions[path_pattern].append((pattern, replacement))

    def add_substitutions(self, path_pattern: str, substitutions: List[Tuple[str, str]]):
        """Add multiple substitution rules for files matching a path pattern."""
        if path_pattern not in self.substitutions:
            self.substitutions[path_pattern] = []
        self.substitutions[path_pattern].extend(substitutions)

    def add_global_substitutions(self, substitutions: List[Tuple[str, str]]):
        """Add substitutions that apply to all files."""
        self.add_substitutions(r".*", substitutions)

    def revert_paths(self, paths: List[str]):
        """Revert specific paths in the target repository to their previous git state."""
        try:
            target_repo = Repo(self.target_root)
        except Exception as e:
            print(f"Error: Target root is not a git repository: {e}")
            return
            
        for path in paths:
            target_path = self.target_root / path
            
            try:
                # Check if the file exists in the current working tree
                if target_path.exists():
                    # Revert the file to its previous committed state
                    target_repo.git.checkout('HEAD', '--', str(target_path))
                    print(f"Reverted file to previous git state: {path}")
                else:
                    print(f"Warning: File does not exist in target: {path}")
            except Exception as e:
                print(f"Error reverting {path}: {e}")

    def _get_substitutions(self, file_path: str) -> List[Tuple[str, str]]:
        """Get all substitutions that apply to a given file path."""
        substitutions = []
        
        # Add exact file path matches
        if file_path in self.substitutions:
            substitutions.extend(self.substitutions[file_path])
        
        # Add path pattern matches (regex)
        for path_pattern, pattern_substitutions in self.substitutions.items():
            if path_pattern != file_path:  # Skip exact matches we already handled
                try:
                    if re.search(path_pattern, file_path):
                        substitutions.extend(pattern_substitutions)
                except re.error:
                    # If path_pattern is not a valid regex, skip it
                    continue
        
        return substitutions

    def copy_and_transform(self, source_path: str, target_path: str):
        """Copy a file from source to target, applying all substitutions."""
        source = self.source_root / source_path
        target = self.target_root / target_path

        if not source.exists():
            print(f"Error: Source file does not exist: {source}")
            sys.exit(1)

        if not target.parent.exists():
            print(f"Error: Target directory does not exist: {target.parent}")
            sys.exit(1)

        # Read source file
        with open(source, 'r') as f:
            content = f.read()

        # Apply substitutions
        substitutions = self._get_substitutions(source_path)
        for pattern, replacement in substitutions:
            content = re.sub(pattern, replacement, content)

        # Write transformed content to target
        with open(target, 'w') as f:
            f.write(content)

        print(f"Copied and transformed: {source_path} -> {target_path}")

    def copy_directory(self, source_dir: str, target_dir: str):
        """Copy an entire directory structure, applying substitutions to all files."""
        source = self.source_root / source_dir
        target = self.target_root / target_dir

        if not source.exists():
            print(f"Error: Source directory does not exist: {source}")
            sys.exit(1)

        if not target.exists():
            print(f"Error: Target directory does not exist: {target}")
            sys.exit(1)

        # Walk through source directory
        for root, dirs, files in os.walk(source):
            # Calculate relative path from source directory
            rel_path = Path(root).relative_to(source)
            target_subdir = target / rel_path

            # Create subdirectory if it doesn't exist
            target_subdir.mkdir(parents=True, exist_ok=True)

            # Copy and transform each file
            for file in files:
                source_file = Path(root) / file
                target_file = target_subdir / file
                
                # Get relative path for file-specific substitutions
                rel_source_path = str(source_file.relative_to(self.source_root))
                
                # Read source file
                with open(source_file, 'r') as f:
                    content = f.read()

                # Apply substitutions
                substitutions = self._get_substitutions(rel_source_path)
                for pattern, replacement in substitutions:
                    content = re.sub(pattern, replacement, content)

                # Write transformed content to target
                with open(target_file, 'w') as f:
                    f.write(content)

                print(f"Copied and transformed: {source_file} -> {target_file}")

# Hardcoded list of paths that should always be reverted - take care here not to miss true differences
DEFAULT_REVERT_PATHS = [
    "relay/paseo/src/genesis_config_presets.rs",
    "relay/paseo/src/governance/tracks.rs",
    "system-parachains/asset-hub-paseo/src/genesis_config_presets.rs",
]

def setup_transposer(source_root: str, target_root: str) -> RuntimeTransposer:
    """Set up the transposer with all substitutions configured."""
    transposer = RuntimeTransposer(source_root, target_root)

    """
    Global
    """
    transposer.add_global_substitutions([
        (r"Polkadot", "Paseo"),
        (r"polkadot", "paseo"),
        (r"POLKADOT", "PASEO"),
        (r"\bDOT\b", "PAS"),
        # Revert specific patterns that should remain as polkadot
        (r"paseo-sdk", "polkadot-sdk"),
        (r"paseo-runtime-common", "polkadot-runtime-common"),
        (r"paseo_runtime_common", "polkadot_runtime_common"),
        (r"paseo-primitives", "polkadot-primitives"),
        (r"paseo_primitives", "polkadot_primitives"),
        (r"paseo-parachain-primitives", "polkadot-parachain-primitives"),
        (r"paseo_parachain_primitives", "polkadot_parachain_primitives"),
        (r"paseo-core-primitives", "polkadot-core-primitives"),
        (r"paseo_core_primitives", "polkadot_core_primitives"),
        # (r"system-parachains-common", "system-parachains-constants"),
        # (r"system_parachains_common", "system_parachains_constants"),
        (r"collectives_paseo_runtime_constants", "collectives_polkadot_runtime_constants"),
        (r"// <https://research.web3.foundation/en/latest/paseo/BABE/Babe/#6-practical-results>", "// <https://research.web3.foundation/en/latest/polkadot/BABE/Babe/#6-practical-results>"),
        (r"type LeaseOffset = LeaseOffset;", "type LeaseOffset = ();"),
        (r"PaseoXcm", "PolkadotXcm"),
        (r"Parity Technologies and the various Paseo contributors", "Parity Technologies and the various Polkadot contributors"),
        (r" \(previously known as Statemint\)", ""),
        (r"AssetHubPaseoAuraId as AuraId", "AuraId"),
        (r"ed25519", "sr25519"),
    ])


    """
    Relay
    """
    transposer.add_substitutions(r"relay/polkadot/Cargo\.toml", [
        ("version.workspace = true", 'version = "1.6.0"'),
        ("\n\n# just for use with zombie-bite to test migration\npallet-sudo = { workspace = true, optional = true }", ''),
        ("pallet-session = { workspace = true }", "pallet-session = { workspace = true }\npallet-sudo = { workspace = true }"),
        (r"fast-runtime = \[\]", 'fast-runtime = ["paseo-runtime-constants/fast-runtime"]\n\nzombie-bite-sudo = ["dep:pallet-sudo"]'),
    ])

    transposer.add_substitutions(r"relay/polkadot/constants/src/lib.rs", [
        ("pub const BROKER_ID: u32 = 1005;", "pub const BROKER_ID: u32 = 1005;\n	/// PAssetHub (Interim AH + contracts) Chain ID.\n	pub const PASSET_HUB_ID: u32 = 1111;"),
        (r"EPOCH_DURATION_IN_SLOTS: BlockNumber = prod_or_fast!\(4 \* HOURS,", "EPOCH_DURATION_IN_SLOTS: BlockNumber = prod_or_fast!(1 * HOURS, 1 *"),
    ])

    transposer.add_substitutions(r"relay/polkadot/src/genesis_config_presets.rs", [
        ("node_features::FeatureIndex, AccountPublic, AssignmentId, AsyncBackingParams,", "node_features::FeatureIndex, AccountPublic, AssignmentId, AsyncBackingParams, ExecutorParam::{MaxMemoryPages, PvfExecTimeout}, PvfExecKind")
    ])

    transposer.add_substitutions(r"relay/polkadot/src/lib.rs", [
        (r"paseo\.subsquare\.io/referenda/1139", "polkadot.subsquare.io/referenda/1139"),
        (r"let fixed_total_issuance: i128 = 15_011_657_390_566_252_333;", "let fixed_total_issuance: i128 = 1_487_502_468_008_283_162;"),
    ])

    transposer.add_substitutions(r"relay/polkadot/src/governance/mod.rs", [
        ("pub type TreasurySpender.*", '// We just allow `Root` to spend money from the treasury, this should prevent bad actors from\n// stealing "money".\npub type TreasurySpender = EnsureRootWithSuccess<AccountId, MaxBalance>;'),
    ])

    transposer.add_substitutions(r"relay/polkadot/src/weights/mod.rs", [
        (r"pub mod pallet_staking;", "pub mod pallet_staking;\npub mod pallet_sudo;")
    ])

    """
    Asset Hub
    """
    transposer.add_substitutions(r"system-parachains/asset-hubs/asset-hub-polkadot/Cargo\.toml", [
        # (r"system-parachains-common", "system-parachains-constants"),
        ("bp-asset-hub-kusama = { workspace = true }\n",""),
        ("bp-bridge-hub-paseo = { workspace = true }", "bp-bridge-hub-paseo = { workspace = true }\nbp-bridge-hub-polkadot = { workspace = true }"),
        ("kusama-runtime-constants = { workspace = true }\n", ""),
        ("pallet-timestamp = { workspace = true }", "pallet-sudo = { workspace = true }\npallet-timestamp = { workspace = true }\nsp-debug-derive = { workspace = true }"),
        ("collectives-paseo-runtime-constants", "collectives-polkadot-runtime-constants"),
        (r"fast-runtime = \[\]", 'fast-runtime = ["paseo-runtime-constants/fast-runtime"]\nforce-debug = ["sp-debug-derive/force-debug"]'),
    ]) 

    transposer.add_substitutions(r"system-parachains/asset-hubs/asset-hub-polkadot/src/lib\.rs", [
        (r'Cow::Borrowed\("statemint"\)', 'Cow::Borrowed("asset-hub-paseo")'),
        (r"//! ## Renaming\n.*\n.*\n.*\n.*\n.*\n", ""),
    ])

    transposer.add_substitutions(r"system-parachains/asset-hubs/asset-hub-polkadot/tests/snowbridge\.rs", [
        (r"NetworkId::Ethereum { chain_id: 11155111", "NetworkId::Ethereum { chain_id: 1"),
    ])

    transposer.add_substitutions(r"system-parachains/asset-hubs/asset-hub-polkadot/tests/tests\.rs", [
        (r"bp_bridge_hub_paseo::WITH_BRIDGE_PASEO", "bp_bridge_hub_polkadot::WITH_BRIDGE_POLKADOT"),
    ])

    # This one is pretty horrible... would look better if we supported multiline replacements properly
    transposer.add_substitutions(r"system-parachains/asset-hubs/asset-hub-polkadot/src/lib\.rs", [
        (r'.*ensure_key_ss58\(\).*\n.*\n.*\n.*\n.*\n.*\n.*}', '	fn ensure_key_ss58() {\n		use frame_support::traits::SortedMembers;\n		use sp_core::crypto::Ss58Codec;\n		let acc =\n			AccountId::from_ss58check("5F4EbSkZz18X36xhbsjvDNs6NuZ82HyYtq5UiJ1h9SBHJXZD").unwrap();\n		assert_eq!(acc, MigController::sorted_members()[0]);\n	}\n\n	#[test]\n	fn aura_uses_sr25519_for_authority_id() {\n		// Ensure that AuthorityId configuration is the expected.\n		assert_eq!(\n			TypeId::of::<<Runtime as pallet_aura::Config>::AuthorityId>(),\n			TypeId::of::<sp_consensus_aura::sr25519::AuthorityId>(),\n		);\n	}'),
    ])


    return transposer

def run_copy_and_transform(args):
    """Run the copy and transform operation."""
    transposer = setup_transposer(args.source_root, args.target_root)
    
    # Print git info for the source root
    transposer.print_git_info()

    # Copy multiple directories
    for source_dir, target_dir in args.copy_pairs:
        print(f"\nCopying {source_dir} -> {target_dir}")
        transposer.copy_directory(source_dir, target_dir)

    # Combine hardcoded and command-line revert paths
    all_revert_paths = DEFAULT_REVERT_PATHS.copy()
    if args.revert_paths:
        all_revert_paths.extend(args.revert_paths)

    # Print the list of paths that will be reverted
    if all_revert_paths:
        print("\nCopy and transform complete\nThe following paths will be reverted after you inspect the diff:")
        for path in all_revert_paths:
            print(f"  - {path}")

        input("\nHit enter if you're happy to revert these files, or Ctrl+C to abort...\n")
        transposer.revert_paths(all_revert_paths)
    else:
        print("\nNo paths were reverted")

def parse_copy_pairs(copy_pairs_str):
    """Parse copy pairs from command line arguments."""
    pairs = []
    for pair_str in copy_pairs_str:
        if ':' not in pair_str:
            print(f"Error: Copy pair must be in format 'source:target', got: {pair_str}")
            sys.exit(1)
        source, target = pair_str.split(':', 1)
        pairs.append((source.strip(), target.strip()))
    return pairs

def run_revert_only(args):
    """Revert files without copying again."""
    transposer = RuntimeTransposer(args.source_root, args.target_root)
    
    # Combine hardcoded and command-line revert paths
    all_revert_paths = DEFAULT_REVERT_PATHS.copy()
    if args.revert_paths:
        all_revert_paths.extend(args.revert_paths)
    
    transposer.revert_paths(all_revert_paths)

def main():
    parser = argparse.ArgumentParser(description="Transpose and transform runtime files")
    parser.add_argument("--source-root", default="../runtimes", help="Source root directory")
    parser.add_argument("--target-root", default=".", help="Target root directory")
    
    subparsers = parser.add_subparsers(dest="command", help="Available commands")
    
    # Copy and transform command
    copy_parser = subparsers.add_parser("copy", help="Copy and transform files")
    copy_parser.add_argument("--copy-pairs", nargs="+", metavar="SOURCE:TARGET", 
                           default=["relay/polkadot:relay/paseo", "system-parachains/asset-hubs/asset-hub-polkadot:system-parachains/asset-hub-paseo", "system-parachains/common:system-parachains/common"],
                           help="Source:target directory pairs to copy (e.g., 'relay/polkadot:relay/paseo' 'system-parachains/asset-hub-polkadot:system-parachains/asset-hub-paseo')")
    copy_parser.add_argument("--revert-paths", nargs="*", help="Additional paths to revert after copy (hardcoded paths are always reverted)")
    copy_parser.set_defaults(func=run_copy_and_transform)
    
    # Revert only command
    revert_parser = subparsers.add_parser("revert", help="Revert specific paths only")
    revert_parser.add_argument("--revert-paths", nargs="+", required=True, help="Paths to revert")
    revert_parser.set_defaults(func=run_revert_only)
    
    args = parser.parse_args()
    
    if not args.command:
        parser.print_help()
        sys.exit(1)
    
    # Parse copy pairs if this is the copy command
    if args.command == "copy":
        args.copy_pairs = parse_copy_pairs(args.copy_pairs)
    
    args.func(args)

    # TODO run zepter, taplo and fmt

if __name__ == "__main__":
    main()