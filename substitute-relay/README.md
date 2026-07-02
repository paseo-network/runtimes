# Substitute Paseo relay — launch & staking-handover runbook

Stand up a **fresh Paseo relay from block 0** that takes over once the current relay is wound
down, then hand validator election to Asset Hub (AH) without losing relay liveness.

> Paseo is **post-AHM**: `Session::SessionManager = NoteHistoricalRoot<Self, StakingAhClient>`
> (`relay/paseo/src/lib.rs`). The relay's validator set comes from
> `pallet_staking_async_ah_client`, with local `pallet_staking` only as a **fallback**. The whole
> plan hinges on `ah_client`'s operating mode:
>
> | Mode | new_session source | session reports → AH | accepts AH set | use |
> |---|---|---|---|---|
> | **Passive** (default) | local `pallet_staking` (Fallback) | no | no (`ReceivedValidatorSetWhilePassive`) | bootstrap |
> | **Buffered** | none → current set **frozen** | no (dropped) | no (`Blocked`) | brief staging step |
> | **Active** | AH-provided `ValidatorSet` | yes | yes (≥ `MinimumValidatorSetSize`) | steady state |
>
> Transitions are **forward-only**: `Passive → Buffered → Active` (`ah-client/src/lib.rs`).

## What's in this directory

- `../relay/paseo/src/genesis_config_presets.rs` → `paseo_substitute_genesis()`, preset id
  **`substitute`** — the genesis below.
- `zombienet-launch.toml` — boot the fresh relay (4 bootstrap validators, Passive) to confirm it
  self-elects and finalizes.
- `chopsticks/` — dry-run the `Passive → Buffered → Active` handover + the `MinimumValidatorSetSize`
  gate against a forked relay, sudo-driven, **no AH/XCM needed**.
- `tools/format-operator-keys.mjs` — turn community-operator key submissions into the Rust
  `substitute_authority(...)` block to paste into the preset (see **Operator key intake** below).
- `tools/derive-session-keys.mjs` — generate session keys + keystore-insert commands (team/dev keys).

---

## Genesis (the `substitute` preset)

Minimal, by design. Built from the existing testnet genesis with these substitutions:

| Section | Value | Why |
|---|---|---|
| `session.keys` | 4 bootstrap authorities (**Alice/Bob/Charlie/Dave — placeholders**) | seat the relay in Passive |
| `staking` | 4 self-bonded stakers, `validatorCount=4`, `forceEra: ForceNone` | fallback election input; `ForceNone` freezes the set (no EPM churn) |
| `sudo.key` | `13uYxsEfJL5FYbJ1E7cW85ihp5LckYTyZT6Bqpc7tS4NAArK` | **current on-chain relay sudo**, reused |
| `configuration.config` | faithful snapshot of the **live** `configuration.activeConfig`, `num_cores` 56 → 2 | match the running chain; only `num_cores` is deliberately overridden (raise via `coretime.request_core_count`) |
| `ah_client` | omitted → `Mode = Passive` | bootstrap without AH |
| paras / hrmp | none | onboard AH after launch |

> ⚠️ The 4 authorities in `paseo_substitute_genesis()` are well-known **DEV keys** (Alice/Bob/Charlie/Dave),
> fine for zombienet/chopsticks only. Replace them with real community-operator keys via the
> **Operator key intake** process below before any real launch.

## Operator key intake

The 4 bootstrap validators are run by **community providers**. Each one supplies **public keys only**
(never secrets), and you bake them into the `substitute` preset via `substitute_authority(...)`.

### What each operator sends you

1. **`stash`** — their validator account, SS58 or `0x`-hex (32 bytes). Funded at genesis + self-bonded.
2. **`sessionKeys`** — the `0x` blob from **`author_rotateKeys`** run on their own node:
   ```bash
   # on the operator's node (keys are generated into ITS keystore; only the public blob leaves)
   curl -H 'Content-Type: application/json' \
     -d '{"id":1,"jsonrpc":"2.0","method":"author_rotateKeys","params":[]}' \
     http://127.0.0.1:9944
   ```
   The blob is 193 bytes = `babe`(32) + `grandpa`(32) + `para_validator`(32) + `para_assignment`(32) +
   `authority_discovery`(32) + `beefy`(33), in `SessionKeys` order. **All six are required** — `babe`
   alone gives blocks but no finality and no parachain backing. The operator keeps the matching private
   keys in the keystore of the node they will run for the substitute relay.

### Turning submissions into genesis

Collect them into `operators.json` (order = the 4 genesis slots):

```json
[
  { "name": "provider-a", "stash": "13...", "sessionKeys": "0x<193-byte blob>" },
  { "name": "provider-b", "stash": "0x..", "sessionKeys": "0x..." },
  { "name": "provider-c", "stash": "13...", "sessionKeys": "0x..." },
  { "name": "provider-d", "stash": "13...", "sessionKeys": "0x..." }
]
```

```bash
cd substitute-relay && npm i
node tools/format-operator-keys.mjs operators.json   # validates lengths, splits the blob
```

It prints a ready-to-paste `let initial_authorities = vec![ substitute_authority(...) ... ];` block.
Replace the placeholder block in `paseo_substitute_genesis()` with it. (Operators who'd rather send the
six keys individually can supply `babe`/`grandpa`/`paraValidator`/`paraAssignment`/`authorityDiscovery`/
`beefy` fields instead of `sessionKeys`; a `seed` field is accepted for dev/team keys only.)

After substituting, rebuild + re-run the fmt checks (`cargo +nightly fmt`, `taplo format --check`).

### Build the chain spec

```bash
# 1. Build the runtime wasm (from repo root)
cargo build --release -p paseo-runtime
# 2. Generate a raw chain spec from the `substitute` preset
chain-spec-builder create -r target/release/wbuild/paseo-runtime/paseo_runtime.compact.compressed.wasm \
  named-preset substitute
# -> chain_spec.json ; convert to raw with `chain-spec-builder ... -s` / `--raw` as needed
```

---

## Phase 1 — launch the fresh relay (Passive, no paras)

1. Distribute the raw chain spec + each bootstrap node's keystore (session keys for its authority).
2. Start the 4 validators. Expect: local-staking fallback seats all 4, blocks author, **GRANDPA
   finalizes**. With `num_cores=2` the scheduler forms 2 backing groups of 2.
3. Verify (see `zombienet-launch.toml` for a local version):
   - `session.validators()` → the 4 stashes.
   - `stakingAhClient.mode()` → `Passive`.
   - finalized head advancing.

**Passive is indefinitely safe** — the relay does not depend on AH. Take as long as you need for
the next phases; getting them wrong never threatens relay liveness until you call `buffer()`.

## Phase 2 — onboard Asset Hub (deferred state-move; mechanics out of scope here)

Onboard AH on the running relay (sudo): `registrar.force_register(1000, head, code)` → seat it on a
core via **relay-side** `coretime.assign_core` (no Coretime chain in this design — all assignment is
relay-side) → start AH collators on a DB whose genesis state-root matches the registered head.

Raise cores as you go: `coretime.request_core_count(N)` (sudo; Root-gated) sets
`scheduler_params.num_cores`.

## Phase 3 — preconditions before the staking handover

Do **not** touch `ah_client` mode until ALL of these hold:

1. Relay live & finalizing on the 4 (Passive).
2. AH live on the new relay (registered, cored, collating).
3. **Stake on AH:** ≥ `MinimumValidatorSetSize` accounts have done `bond` → `validate` on AH
   (AH order is `bond → validate → set_keys`, the reverse of the old relay). `validatorCount` set.
4. **Keys on the new relay:** every such validator ran `StakingRcClient.set_keys` (AH) → forwarded
   to relay `ah_client.set_keys_from_ah` → `Session::set_keys`. **Verify `Session::NextKeys`** for
   each — an elected-but-keyless validator silently fails to author. (Works while Passive, so
   pre-register.)
5. **Lower the gate:** `parameters.set_parameter` → `MinimumValidatorSetSize` ≤ your first cohort
   (default is **100**, which would reject anything smaller). Raise it again as you scale.
6. Transport: relay↔AH UMP/DMP only — available once AH is registered. **No HRMP channel needed**
   for staking (validator-set report = UMP; session reports / set_keys = DMP).

## Phase 4 — execute the handover

```text
sudo( stakingAhClient.set_mode(Buffered) )  # Passive -> Buffered : fallback stops, 4 frozen, no reports
sudo( stakingAhClient.set_mode(Active)   )  # Buffered -> Active  : relay sends session reports to AH
                                            #                       and will accept AH's validator set
```

> The runtime exposes **`stakingAhClient.set_mode(mode)`** (call_index 1, `AdminOrigin = Root`); the
> `buffer()`/`activate()` names in the pallet source are internal helpers, not extrinsics. Transitions
> are still forward-only (`Passive → Buffered → Active`), enforced inside `set_mode`.

After `set_mode(Active)`: relay session reports flow to AH → AH's era clock advances → AH election runs →
once its export gate (`ValidatorSetExportSession`) is met, AH exports the set → relay accepts it
(size ≥ lowered min) → **applies at the next session boundary** → the bootstrap 4 are replaced.
Liveness stays on the frozen 4 across the gap.

### Watch (relay events)
| Event | Meaning |
|---|---|
| `stakingAhClient.ValidatorSetReceived` | AH set accepted ✅ |
| `stakingAhClient.SetTooSmallAndDropped` | set < min-size → lower `MinimumValidatorSetSize` or grow the cohort |
| `stakingAhClient.ReceivedValidatorSetWhilePassive` | you flipped out of order / never left Passive |
| `session.NewSession` then `session.validators()` changes | the new set actually seated |

## Phase 5 — scale to target

Raise AH `validatorCount` and relay `MinimumValidatorSetSize` **together** as more validators
complete `bond → validate → set_keys`; each AH era re-elects the larger set. Bump
`num_cores` toward **20** in step (`coretime.request_core_count`) to hold ~3 validators/core →
**60 validators / 20 cores**.

## Failure modes

- **`set_mode` rejected** — transitions are forward-only; you can't skip or go back. To reset to
  Passive in a test, force `StakingAhClient::Mode` via `system.set_storage` (needs the **master**
  sudo key — `SafeSudo` proxy blocks `set_storage`).
- **Set dropped as too small** — `MinimumValidatorSetSize` still ≥ cohort. Lower it.
- **New validators don't author after seating** — missing `Session::NextKeys`; they never ran
  `set_keys` against *this* relay. Re-run it (works in any mode).
- **AH never exports** — its era clock only advances on relay session reports, which start at
  `activate()`. If still stuck, check `ValidatorSetExportSession` gating and that AH actually elected.

## Dry-run before committing

Run `chopsticks/` first — it exercises `buffer → activate`, the min-size gate (reject then accept),
and the session rotation against forked state, sudo-driven, with no AH dependency. See
`chopsticks/README.md`.
