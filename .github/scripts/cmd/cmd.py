#!/usr/bin/env python3

import os
import sys
import json
import shlex
import argparse
import subprocess
import _help

_HelpAction = _help._HelpAction

MATRIX_PATH = '.github/workflows/runtimes-matrix.json'

with open(MATRIX_PATH, 'r') as f:
    runtimesMatrix = json.load(f)

runtimeNames = list(map(lambda x: x['name'], runtimesMatrix))

common_args = {
    '--continue-on-fail': {"action": "store_true", "help": "Won't exit(1) on failed command and continue with next "
                                                          "steps. Helpful when you want to push at least successful "
                                                          "pallets, and then run failed ones separately"},
    '--quiet': {"action": "store_true", "help": "Won't print start/end/failed messages in Pull Request"},
    '--clean': {"action": "store_true", "help": "Clean up the previous bot's & author's comments in Pull Request "
                                                "which triggered /cmd"},
}

parser = argparse.ArgumentParser(prog="/cmd ", description='A command runner for the Paseo runtimes repo',
                                 add_help=False)
parser.add_argument('--help', action=_HelpAction, help='help for help if you need some help')  # help for help

subparsers = parser.add_subparsers(help='a command to run', dest='command')

"""
BENCH
"""

bench_example = '''**Examples**:

 > runs all benchmarks for all runtimes

 %(prog)s

 > runs benchmarks for pallet_balances and pallet_xcm_benchmarks::generic for all runtimes which have these pallets
 > --quiet makes it output nothing to the PR but reactions

 %(prog)s --pallet pallet_balances pallet_xcm_benchmarks::generic --quiet

 > runs bench for all pallets of the paseo relay runtime and continues even if some benchmarks fail

 %(prog)s --runtime paseo --continue-on-fail

 > does not output anything and cleans up the previous bot's & author's command triggering comments in the PR

 %(prog)s --runtime paseo people-paseo --pallet pallet_balances pallet_multisig --quiet --clean

 '''

parser_bench = subparsers.add_parser('bench', help='Runs benchmarks', epilog=bench_example,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)

for arg, config in common_args.items():
    parser_bench.add_argument(arg, **config)

parser_bench.add_argument('--runtime', help='Runtime(s) space separated', choices=runtimeNames, nargs='*',
                          default=runtimeNames)
parser_bench.add_argument('--pallet', help='Pallet(s) space separated', nargs='*', default=[])
parser_bench.add_argument('--profile', help='Cargo profile used to build the runtimes', default='production')
parser_bench.add_argument('--steps', help='Number of steps across component ranges', default='50')
parser_bench.add_argument('--repeat', help='Number of times the benchmark repeats per step', default='20')

args, unknown = parser.parse_known_args()

print(f'args: {args}')

if args.command != 'bench':
    parser.print_help()
    sys.exit(1)

runtime_pallets_map = {}
failed_benchmarks = {}
successful_benchmarks = {}

profile = args.profile

print(f'Provided runtimes: {args.runtime}')
# convert to mapped dict
runtimesMatrix = list(filter(lambda x: x['name'] in args.runtime, runtimesMatrix))
runtimesMatrix = {x['name']: x for x in runtimesMatrix}
print(f'Filtered out runtimes: {list(runtimesMatrix.keys())}')


def wasm_path(runtime):
    package = runtime['package']
    return f"target/{profile}/wbuild/{package}/{package.replace('-', '_')}.wasm"


# loop over remaining runtimes to collect available pallets
for runtime in runtimesMatrix.values():
    print(f'-- compiling the runtime {runtime["name"]}')
    features = "runtime-benchmarks"
    features_extra = runtime.get("build_extra_features")
    if features_extra:
        features += "," + features_extra
    print(f'-- with features {features}')
    result = subprocess.run(
        ["cargo", "build", "-p", runtime['package'], "--profile", profile, "-q", "--features", features])
    if result.returncode != 0:
        print(f"Failed to build {runtime['name']}")
        sys.exit(1)

    print(f'-- listing pallets for benchmark for {runtime["name"]}')
    result = subprocess.run(
        ["frame-omni-bencher", "v1", "benchmark", "pallet", "--no-csv-header", "--all", "--list",
         f"--runtime={wasm_path(runtime)}",
         "--genesis-builder=runtime",
         f"--genesis-builder-preset={runtime.get('genesis_builder_preset', 'local_testnet')}"],
        capture_output=True, text=True)
    if result.returncode != 0:
        print(f"Failed to list pallets for {runtime['name']}: {result.stderr}")
        sys.exit(1)
    raw_pallets = result.stdout.split('\n')

    all_pallets = set()
    for pallet in raw_pallets:
        if pallet:
            all_pallets.add(pallet.split(',')[0].strip())

    # Pallets that are known not to benchmark in this runtime. They are only skipped when the
    # pallet list is auto-discovered - an explicit `--pallet` always wins, so a fix can be verified.
    excluded_pallets = set(runtime.get('benchmarks_exclude_pallets', []) or [])
    if not args.pallet and excluded_pallets & all_pallets:
        print(f'-- excluding pallets in {runtime["name"]}: {sorted(excluded_pallets & all_pallets)}')
        all_pallets -= excluded_pallets

    pallets = sorted(all_pallets)
    print(f'Pallets in {runtime["name"]}: {pallets}')
    runtime_pallets_map[runtime['name']] = pallets

# filter out only the specified pallets from the collected runtimes/pallets
if args.pallet:
    print(f'Pallet: {args.pallet}')
    new_pallets_map = {}
    # keep only the specified pallets that actually exist in the runtime
    for runtime, pallets in runtime_pallets_map.items():
        matched = [p for p in args.pallet if p in pallets]
        missing = [p for p in args.pallet if p not in pallets]
        if missing:
            print(f'-- {runtime} does not have pallets: {missing}, skipping them')
        if matched:
            new_pallets_map[runtime] = matched

    runtime_pallets_map = new_pallets_map

print(f'Filtered out runtimes & pallets: {runtime_pallets_map}')

if not runtime_pallets_map:
    if args.pallet and not args.runtime:
        print(f"No pallets [{args.pallet}] found in any runtime")
    elif args.runtime and not args.pallet:
        print(f"{args.runtime} runtime does not have any pallets")
    elif args.runtime and args.pallet:
        print(f"No pallets [{args.pallet}] found in {args.runtime}")
    else:
        print('No runtimes found')
    sys.exit(0)

header_path = os.path.abspath('./.github/scripts/cmd/file_header.txt')

for runtime in runtime_pallets_map:
    for pallet in runtime_pallets_map[runtime]:
        config = runtimesMatrix[runtime]
        default_path = f"./{config['path']}/src/weights"
        xcm_path = f"./{config['path']}/src/weights/xcm"
        output_path = default_path if not pallet.startswith("pallet_xcm_benchmarks") else xcm_path
        templates = config.get("benchmarks_templates", {}) or {}
        template = templates.get(pallet)
        excluded_extrinsics = config.get("benchmarks_exclude_extrinsics", {}) or {}
        excluded = excluded_extrinsics.get(pallet, [])
        excluded_string = ",".join(f"{pallet}::{e}" for e in excluded)

        print(f'-- benchmarking {pallet} in {runtime} into {output_path} '
              f'using template {template} and excluded {excluded_string}')

        cmd = [
            "frame-omni-bencher", "v1", "benchmark", "pallet",
            "--extrinsic=*",
            f"--runtime={wasm_path(config)}",
            f"--pallet={pallet}",
            "--genesis-builder=runtime",
            f"--genesis-builder-preset={config.get('genesis_builder_preset', 'local_testnet')}",
            f"--header={header_path}",
            f"--output={output_path}",
            "--wasm-execution=compiled",
            f"--steps={args.steps}",
            f"--repeat={args.repeat}",
            "--heap-pages=4096",
            "--min-duration", "1",
            "--quiet",
        ]
        if template:
            cmd.append(f"--template={template}")
        if excluded_string:
            cmd.append(f"--exclude-extrinsics={excluded_string}")

        print(f'-- running: {shlex.join(cmd)}')
        status = subprocess.run(cmd, env={**os.environ, "RUNTIME_LOG": "off"}).returncode

        if status != 0 and not args.continue_on_fail:
            print(f'Failed to benchmark {pallet} in {runtime}')
            sys.exit(1)

        # Otherwise collect failed benchmarks and print them at the end
        if status != 0:
            failed_benchmarks[f'{runtime}'] = failed_benchmarks.get(f'{runtime}', []) + [pallet]
        else:
            successful_benchmarks[f'{runtime}'] = successful_benchmarks.get(f'{runtime}', []) + [pallet]

if failed_benchmarks:
    print('❌ Failed benchmarks of runtimes/pallets:')
    for runtime, pallets in failed_benchmarks.items():
        print(f'-- {runtime}: {pallets}')

if successful_benchmarks:
    print('✅ Successful benchmarks of runtimes/pallets:')
    for runtime, pallets in successful_benchmarks.items():
        print(f'-- {runtime}: {pallets}')
