#!/usr/bin/env python3

import os
import sys
import json
import shlex
import shutil
import argparse
import tempfile
import threading
import subprocess
from concurrent.futures import ThreadPoolExecutor, as_completed

import _help

_HelpAction = _help._HelpAction

# Paths are resolved relative to this script rather than the working directory: CI runs the
# `main` copy of the tooling against a checkout of the PR branch, so the two can differ.
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
MATRIX_PATH = os.path.join(SCRIPT_DIR, '..', '..', 'workflows', 'runtimes-matrix.json')
HEADER_PATH = os.path.join(SCRIPT_DIR, 'file_header.txt')

with open(MATRIX_PATH, 'r') as f:
    runtimesMatrix = json.load(f)

runtimeNames = list(map(lambda x: x['name'], runtimesMatrix))

common_args = {
    '--continue-on-fail': {"action": "store_true", "help": "Won't exit on the first failed pallet and continue with the "
                                                          "next ones. Always on when no --pallet is given, so one "
                                                          "broken pallet cannot sink a full sweep"},
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

 > runs every pallet of people-paseo except the two listed

 %(prog)s --runtime people-paseo --exclude-pallets indiv_pallet_coinage pallet_proxy

 > runs four pallets at a time - faster, but concurrent runs perturb each other's timings

 %(prog)s --runtime people-paseo --jobs 4

 '''

parser_bench = subparsers.add_parser('bench', help='Runs benchmarks', epilog=bench_example,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)

for arg, config in common_args.items():
    parser_bench.add_argument(arg, **config)

parser_bench.add_argument('--runtime', help='Runtime(s) space separated', choices=runtimeNames, nargs='*',
                          default=runtimeNames)
parser_bench.add_argument('--pallet', help='Pallet(s) space separated', nargs='*', default=[])
parser_bench.add_argument('--exclude-pallets', help='Pallet(s) space separated to skip, on top of the runtime\'s '
                                                    'benchmarks_exclude_pallets', nargs='*', default=[])
parser_bench.add_argument('--profile', help='Cargo profile used to build the runtimes', default='production')
parser_bench.add_argument('--steps', help='Number of steps across component ranges', default='50')
parser_bench.add_argument('--repeat', help='Number of times the benchmark repeats per step', default='20')
parser_bench.add_argument('--jobs', help='Number of pallets benchmarked at the same time. frame-omni-bencher is '
                                         'single-threaded, so this is how a run uses more than one core. Pallets '
                                         'running side by side compete for caches and memory bandwidth, which '
                                         'inflates their weights: keep 1 for weights meant to ship',
                          type=int, default=1)
parser_bench.add_argument('--no-smoke', help='Skip the quick `--steps 2 --repeat 1` pass that weeds out broken '
                                             'benchmarks before the real run', action='store_true')
parser_bench.add_argument('--commit', help='Commit each pallet\'s weights as soon as the pallet finishes, one '
                                           'commit per pallet (used by CI)', action='store_true')
parser_bench.add_argument('--summary', help='Write the per-pallet outcome as JSON to this file')
parser_bench.add_argument('--check-args', help='Only validate the arguments, then exit', action='store_true')

# `parse_args`, not `parse_known_args`: an option this version does not know must fail loudly. A
# silently dropped `--exclude-pallets` once cost a 53-hour run.
args = parser.parse_args()

print(f'args: {args}')

if args.command != 'bench':
    parser.print_help()
    sys.exit(1)

if args.jobs < 1:
    parser.error('--jobs must be at least 1')

if args.check_args:
    sys.exit(0)

# An explicit `--pallet` list is usually a quick targeted run where the first failure is worth
# stopping for. A full sweep is not: keep going and keep what succeeded.
continue_on_fail = args.continue_on_fail or not args.pallet

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

# Unlike `benchmarks_exclude_pallets` in the matrix, this is an explicit request, so it applies
# even alongside `--pallet`.
if args.exclude_pallets:
    print(f'Excluding pallets: {args.exclude_pallets}')
    runtime_pallets_map = {
        runtime: kept
        for runtime, pallets in runtime_pallets_map.items()
        if (kept := [p for p in pallets if p not in args.exclude_pallets])
    }

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

print_lock = threading.Lock()


def log(message):
    with print_lock:
        print(message, flush=True)


def output_path(config, pallet):
    default_path = f"./{config['path']}/src/weights"
    xcm_path = f"./{config['path']}/src/weights/xcm"
    return default_path if not pallet.startswith("pallet_xcm_benchmarks") else xcm_path


def bench_cmd(runtime, pallet, out_dir, smoke):
    config = runtimesMatrix[runtime]
    templates = config.get("benchmarks_templates", {}) or {}
    template = templates.get(pallet)
    excluded_extrinsics = config.get("benchmarks_exclude_extrinsics", {}) or {}
    excluded = excluded_extrinsics.get(pallet, [])
    excluded_string = ",".join(f"{pallet}::{e}" for e in excluded)

    cmd = [
        "frame-omni-bencher", "v1", "benchmark", "pallet",
        "--extrinsic=*",
        f"--runtime={wasm_path(config)}",
        f"--pallet={pallet}",
        "--genesis-builder=runtime",
        f"--genesis-builder-preset={config.get('genesis_builder_preset', 'local_testnet')}",
        "--wasm-execution=compiled",
        "--heap-pages=4096",
        "--min-duration", "1",
        "--quiet",
    ]
    if smoke:
        # Only whether every benchmark executes matters here. Two samples per component are too
        # few to fit a weight when a benchmark skips one (`only has 1 unique value(s)`), so the
        # smoke pass writes no weight file and runs no regression.
        cmd += ["--steps=2", "--repeat=1", "--no-median-slopes", "--no-min-squares"]
    else:
        cmd += [f"--steps={args.steps}", f"--repeat={args.repeat}", f"--header={HEADER_PATH}",
                f"--output={out_dir}"]
        if template:
            cmd.append(f"--template={template}")
    if excluded_string:
        cmd.append(f"--exclude-extrinsics={excluded_string}")
    return cmd


def run_bench(runtime, pallet, smoke, stop, fail_fast):
    """Benchmarks one pallet into a private temp dir, so pallets running in parallel never see each
    other's files. Returns (exit status, temp dir holding the generated files)."""
    if stop.is_set():
        return None, None
    out_dir = tempfile.mkdtemp(prefix='bench-')
    cmd = bench_cmd(runtime, pallet, out_dir, smoke)
    tag = f'[{runtime}/{pallet}]'
    log(f'{tag} running: {shlex.join(cmd)}')
    # Prefix every line, or the logs of parallel pallets are impossible to tell apart.
    proc = subprocess.Popen(cmd, env={**os.environ, "RUNTIME_LOG": "off"}, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True, errors='replace')
    for line in proc.stdout:
        log(f'{tag} {line.rstrip()}')
    status = proc.wait()
    if status != 0 and fail_fast:
        # Set here rather than by the caller, or this worker's next pallet starts before it sees it.
        # Pallets already running are left to finish - killing them would throw their work away.
        stop.set()
    return status, out_dir


def run_all(work, smoke, jobs, on_done, fail_fast):
    """Runs `work` (a list of (runtime, pallet)) on `jobs` workers and calls `on_done` from this
    thread, one pallet at a time, as they finish."""
    stop = threading.Event()
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        futures = {pool.submit(run_bench, r, p, smoke, stop, fail_fast): (r, p) for r, p in work}
        for future in as_completed(futures):
            runtime, pallet = futures[future]
            status, out_dir = future.result()
            if status is None:
                continue  # never started, see `stop`
            try:
                on_done(runtime, pallet, status, out_dir)
            finally:
                shutil.rmtree(out_dir, ignore_errors=True)


def write_summary():
    """Rewritten after every pallet, so a run killed at the timeout still leaves an accurate one."""
    if not args.summary:
        return
    os.makedirs(os.path.dirname(os.path.abspath(args.summary)), exist_ok=True)
    with open(args.summary, 'w') as f:
        json.dump({'successful': successful_benchmarks, 'failed': failed_benchmarks}, f, indent=2)


def git(*cmd):
    subprocess.run(["git", *cmd], check=True)


work = [(runtime, pallet) for runtime, pallets in runtime_pallets_map.items() for pallet in pallets]

# Smoke pass: every benchmark at `--steps 2 --repeat 1`, i.e. the lowest and highest value of each
# component once. It takes minutes, and catches the benchmarks that cannot run at all - which
# frame-omni-bencher otherwise reports only after running every other benchmark of the pallet in
# full, and then writes no weights for it. Its timings are thrown away, so it uses every core.
if not args.no_smoke:
    smoke_failed = []

    def on_smoke_done(runtime, pallet, status, out_dir):
        if status != 0:
            log(f'-- smoke: {pallet} in {runtime} is broken, skipping it')
            smoke_failed.append((runtime, pallet))

    log(f'-- smoke-testing {len(work)} pallets')
    run_all(work, True, os.cpu_count() or 1, on_smoke_done, fail_fast=False)

    for runtime, pallet in smoke_failed:
        failed_benchmarks[runtime] = failed_benchmarks.get(runtime, []) + [pallet]
    write_summary()
    if smoke_failed and not continue_on_fail:
        print(f'Broken benchmarks, not starting the real run: {smoke_failed}')
        work = []
    else:
        work = [w for w in work if w not in smoke_failed]


def on_bench_done(runtime, pallet, status, out_dir):
    if status != 0:
        log(f'Failed to benchmark {pallet} in {runtime}')
        failed_benchmarks[runtime] = failed_benchmarks.get(runtime, []) + [pallet]
        write_summary()
        return

    dest_dir = output_path(runtimesMatrix[runtime], pallet)
    os.makedirs(dest_dir, exist_ok=True)
    written = []
    for name in sorted(os.listdir(out_dir)):
        dest = os.path.join(dest_dir, name)
        shutil.move(os.path.join(out_dir, name), dest)
        written.append(dest)
    log(f'-- {pallet} in {runtime} done: {written}')
    successful_benchmarks[runtime] = successful_benchmarks.get(runtime, []) + [pallet]
    write_summary()

    if args.commit and written:
        git("add", "--", *written)
        # Nothing staged when the weights came out identical.
        if subprocess.run(["git", "diff", "--cached", "--quiet"]).returncode != 0:
            git("commit", "-q", "-m", f"Update {runtime} weights: {pallet}")


log(f'-- benchmarking {len(work)} pallets, {args.jobs} at a time')
run_all(work, False, args.jobs, on_bench_done, fail_fast=not continue_on_fail)

if failed_benchmarks:
    print('❌ Failed benchmarks of runtimes/pallets:')
    for runtime, pallets in failed_benchmarks.items():
        print(f'-- {runtime}: {pallets}')

if successful_benchmarks:
    print('✅ Successful benchmarks of runtimes/pallets:')
    for runtime, pallets in successful_benchmarks.items():
        print(f'-- {runtime}: {pallets}')

if failed_benchmarks:
    sys.exit(1)
