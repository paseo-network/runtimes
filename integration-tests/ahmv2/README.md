# AHM v2 migration integration tests

Rust dry-run harness for the AHM v2 migration: moving account, proxy, registrar and HRMP state
off the Relay Chain.

Upstream (polkadot-fellows/runtimes) that state goes to the Coretime chain. Paseo's Coretime
chain is dead, so on Paseo the receiving side of the migration (`pallet-ct-migrator`) runs on
Asset Hub, and this suite loads the Relay Chain and Asset Hub.

The two chains are loaded from `try-runtime` snapshots of real network state. Blocks are produced
by calling hooks directly and DMP/UMP messages are shuttled between the chains by hand (no nodes
or networking).

## The `ahm-v2` feature

The migrator pallets are wired into the runtimes behind each runtime's `ahm-v2` feature, which
this crate enables on both runtimes. The runtimes gate them on
`all(feature = "ahm-v2", not(feature = "on-chain-release-build"))`, so a release build drops the
pallets even when `ahm-v2` is on.

## Running

```bash
cd integration-tests/ahmv2
just test
```

That is all: it creates any missing snapshot and runs the test suite. Extra arguments are passed
through to `cargo test` (e.g. `just test rc_and_asset_hub`). Snapshot creation needs the
[try-runtime CLI](https://github.com/paritytech/try-runtime-cli).

Snapshots land in `snapshots/paseo/` (gitignored) and are kept until you delete them, so runs do
not re-create them. `SNAP_DIR` overrides the parent directory: point it at a tree that already
holds `paseo/snap_rc.snap` and `paseo/snap_ah.snap`. The default endpoints are public; override
them with `RC_URI` / `AH_URI` to scrape from your own nodes.

To run against specific snapshot files, bypass the justfile:

```bash
SNAP_RC=... SNAP_AH=... cargo test -p paseo-integration-tests-ahmv2 -- --test-threads=1
```

Keep `--test-threads=1`: the Asset Hub snapshot is several GB and every test hydrates its own
copy of it, so running the tests in parallel exhausts the memory of a developer machine.

Snapshots are cached in memory per test process and re-hydrated per test, so each test gets fresh
externalities without reloading from disk.

Every produced block asserts that consumed weight stays below 80% of the block limit and, unless
a test opts out to judge one message itself, that no `MessageQueue` failure event was emitted.
Every shuttled message is decoded against the receiving runtime's `RuntimeCall` (catches
encode/decode drift between the chains).
