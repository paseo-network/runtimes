#!/usr/bin/env node
// Dry-run the substitute relay's staking handover against a chopsticks fork of the live Paseo
// relay. Relay-only, sudo-driven — NO Asset Hub and NO XCM. Exercises the exact code paths the
// handover depends on:
//
//   1. Force ah_client Mode -> Passive (simulate the fresh-relay bootstrap state).
//   2. In Passive, ah_client.validator_set(report) is rejected (Error::Blocked).
//   3. Passive -> Buffered -> Active (forward-only set_mode transitions).
//   4. In Active, a TOO-SMALL set -> SetTooSmallAndDropped (the MinimumValidatorSetSize gate).
//   5. In Active, a set >= min -> ValidatorSetReceived + ValidatorSet stored (seats next session).
//
// Synthetic sets are fed via `sudo(ah_client.validator_set(..))` because the receive call accepts
// Root. The "accept" demo reuses the live validator set (its accounts already have Session keys
// and it is >= the default min of 100), so no parameter surgery is needed.
//
// Prereqs:  npm i (in ../) ; chopsticks running per relay.yml (ws://localhost:8000).
// Usage:    node chopsticks/drive-handover.mjs

import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';
import { encodeAddress } from '@polkadot/util-crypto';

const ENDPOINT = process.env.ENDPOINT || 'ws://localhost:8000';
const log = (...a) => console.log(...a);

const provider = new WsProvider(ENDPOINT);
const api = await ApiPromise.create({ provider, noInitWarn: true });
const keyring = new Keyring({ type: 'sr25519' });
const alice = keyring.addFromUri('//Alice'); // must equal the Sudo.Key override in relay.yml

// --- chopsticks dev RPC (custom methods need provider.send, not api.rpc) -------------------
const dev = (method, ...params) => provider.send(method, params);
const newBlock = () => dev('dev_newBlock', {});
const setStorage = (entries) => dev('dev_setStorage', entries);
const mode = async () => (await api.query.stakingAhClient.mode()).toString();

// Sign+submit a sudo(call), produce a block, return the event names emitted in that block.
async function sudoCall(call, label) {
  const tx = api.tx.sudo.sudo(call);
  await tx.signAsync(alice, { nonce: -1 });
  try {
    await dev('author_submitExtrinsic', tx.toHex());
  } catch (e) {
    log(`  [${label}] submit error:`, e.message);
  }
  const blockHash = await newBlock();
  const hash = typeof blockHash === 'string' ? blockHash : blockHash?.hash || blockHash;
  const at = await api.at(hash);
  const records = await at.query.system.events();
  const events = records.map((r) => `${r.event.section}.${r.event.method}`);
  log(`  [${label}]`);
  log(`    events: ${events.join(', ') || '(none)'}`);
  // surface the inner result of sudo.Sudid (so a blocked/failed inner call is visible)
  const sudid = records.find((r) => r.event.section === 'sudo' && r.event.method === 'Sudid');
  if (sudid) {
    const res = sudid.event.data[0];
    log(`    sudo.Sudid: ${res.isErr ? 'Err ' + JSON.stringify(res.asErr.toHuman()) : 'Ok'}`);
  }
  return events;
}

function makeReport(validatorSet, id) {
  return { newValidatorSet: validatorSet, id, pruneUpTo: null, leftover: false };
}

// --- run -----------------------------------------------------------------------------------
const rv = await api.rpc.state.getRuntimeVersion();
log(`Connected ${ENDPOINT} — ${rv.specName}/${rv.specVersion}`);
log(`stakingAhClient tx methods: ${Object.keys(api.tx.stakingAhClient).join(', ')}`);
const aliceSs58 = encodeAddress(alice.publicKey, 0); // Paseo ss58 = 0
log(`Sudo.Key: ${(await api.query.sudo.key()).toString()}  (expect //Alice ${aliceSs58})\n`);

// pick the mode-transition extrinsics (prefer buffer()/activate(), fall back to set_mode)
const tx = api.tx.stakingAhClient;
const toBuffered = tx.buffer ? tx.buffer() : tx.setMode('Buffered');
const toActive = tx.activate ? tx.activate() : tx.setMode('Active');

const liveValidators = (await api.query.session.validators()).map((v) => v.toString());
log(`0. live session.validators: ${liveValidators.length}`);

// 0b. fund the sudo signer (Alice) on the fork — otherwise txs are rejected (1010 invalid payment).
await setStorage({
  System: { Account: [[[alice.address], { providers: 1, data: { free: '1000000000000000000000' } }]] },
});
const bal = await api.query.system.account(alice.address);
log(`0b. funded Alice -> free ${bal.data.free.toString()}\n`);

// 1. reset ah_client to a clean fresh-relay bootstrap state: Mode -> Passive (enum, Passive=0x00),
//    and clear any buffered set so the run is idempotent (null removes an OptionQuery key).
const modeKey = api.query.stakingAhClient.mode.key();
await setStorage([
  [modeKey, '0x00'],
  [api.query.stakingAhClient.validatorSet.key(), null],
  [api.query.stakingAhClient.incompleteValidatorSetReport.key(), null],
]);
log(`1. reset -> Mode ${await mode()} (expect Passive), ValidatorSet cleared\n`);

// 2. Passive: validator_set must be Blocked (look for ExtrinsicFailed, no ValidatorSetReceived)
await sudoCall(tx.validatorSet(makeReport(liveValidators.slice(0, 4), 1)),
  '2. validator_set@Passive (expect Sudo error / no ValidatorSetReceived)');
log('');

// 3. Passive -> Buffered
await sudoCall(toBuffered, '3. -> Buffered');
log(`    Mode -> ${await mode()}  (expect Buffered)\n`);

// 4. Buffered -> Active
await sudoCall(toActive, '4. -> Active');
log(`    Mode -> ${await mode()}  (expect Active)\n`);

// 5. Active + too-small set -> SetTooSmallAndDropped
await sudoCall(tx.validatorSet(makeReport(liveValidators.slice(0, 4), 2)),
  '5. validator_set(4 accts)@Active (expect stakingAhClient.SetTooSmallAndDropped)');
log(`    ValidatorSet stored? ${(await api.query.stakingAhClient.validatorSet()).isSome}  (expect false)\n`);

// 6. Active + full live set (>= min) -> accepted
await sudoCall(tx.validatorSet(makeReport(liveValidators, 3)),
  '6. validator_set(live set)@Active (expect stakingAhClient.ValidatorSetReceived)');
log(`    ValidatorSet stored? ${(await api.query.stakingAhClient.validatorSet()).isSome}  (expect true)`);

log('\nDone. (To watch the real rotation, build ~1 epoch of blocks then re-read session.validators.)');
await api.disconnect();
process.exit(0);
