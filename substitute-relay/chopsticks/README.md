# Chopsticks dry-run: staking handover

Exercises the `ah_client` operating-mode FSM and the `MinimumValidatorSetSize` gate against a
**fork of the live Paseo relay** — relay-only, sudo-driven, **no Asset Hub and no XCM**. This is
the cheap way to watch `Passive → Buffered → Active` accept (and reject) a validator set before
you commit any of it to a real genesis.

## Why this works without AH/XCM

`ah_client.validator_set(report)` authorizes via `ensure_origin_or_root` — so a `sudo` (Root) call
feeds it a synthetic set directly. The receive path then enforces, in order:

1. `Mode::can_accept_validator_set()` → **Active only** (else `Error::Blocked`),
2. `len(set) >= MinimumValidatorSetSize` (else `SetTooSmallAndDropped`),
3. otherwise `ValidatorSetReceived` → `ValidatorSet` stored → seated at the next session.

The driver replays all three. The "accept" step reuses the live validator set (its accounts already
have `Session` keys and it is ≥ the default min of 100), so no parameter surgery is needed.

## Run

```bash
cd ..               # substitute-relay/
npm i               # chopsticks + @polkadot/* (package.json pins yargs@17 via overrides)

# Work around a chopsticks 1.5.0 packaging bug under Node 20 (its CJS bin require()s ESM-only
# deps). Remove its nested ESM yargs (so it resolves the top-level v17) and run the ESM cli:
rm -rf node_modules/@acala-network/chopsticks/node_modules/yargs

# terminal 1: fork the live relay (serves ws://localhost:8000)
node node_modules/@acala-network/chopsticks/dist/esm/cli.js -c chopsticks/relay.yml

# terminal 2: drive the handover (funds //Alice on the fork automatically)
node chopsticks/drive-handover.mjs
```

## Verified output (Paseo spec 2003001, 2026-06-30)

```
stakingAhClient tx methods: validatorSet, setMode, forceOnMigrationEnd, setKeysFromAh, purgeKeysFromAh
Sudo.Key: 15oF4uVJ...  (== //Alice in ss58=0)
0. live session.validators: 152
0b. funded Alice
1. reset -> Mode Passive, ValidatorSet cleared
2. validator_set@Passive   -> sudo.Sudid: Err {Module index 42, error 0}   # StakingAhClient::Blocked
3. -> Buffered             -> Mode Buffered
4. -> Active               -> Mode Active
5. validator_set(4)@Active -> stakingAhClient.SetTooSmallAndDropped ; ValidatorSet stored? false
6. validator_set(152)      -> stakingAhClient.ValidatorSetReceived  ; ValidatorSet stored? true
```

This confirms: the mode FSM is forward-only (`set_mode`), the receive call is `Blocked` outside
Active, the `MinimumValidatorSetSize` gate drops the 4-validator set (default min 100) and accepts
the 152-validator set.

## Notes / caveats

- The mode-transition extrinsic is **`stakingAhClient.set_mode(mode)`** — the source `buffer()`/
  `activate()` are internal helpers, not dispatchables. The driver uses `setMode` (falls back from
  `buffer`/`activate` if absent).
- `Sudo.Key` is overridden to `//Alice` in `relay.yml`; the driver funds Alice on the fork (else
  txs are rejected `1010: invalid payment`).
- To observe the *actual* validator rotation (not just `ValidatorSet` stored), build ~one epoch of
  blocks (`dev_newBlock { count }`) until `session.NewSession`, then re-read `session.validators()`.
- To dry-run the **full** AH→relay path (real election + XCM export), use chopsticks XCM mode with
  `relay.yml` + an `asset-hub.yml`, or zombienet — heavier, out of scope for this gate check.
