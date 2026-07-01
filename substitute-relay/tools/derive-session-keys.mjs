#!/usr/bin/env node
// Derive a bootstrap authority's accounts + 6 session keys from a secret seed, in the exact
// shape the `substitute` genesis preset expects (mirrors `get_authority_keys_from_seed`).
//
// Use this to REPLACE the well-known Alice/Bob/Charlie/Dave placeholders with real, team-generated
// bootstrap keys before launch:
//   - keep the secret seeds OUT of source; load them into each node's keystore (commands printed),
//   - paste only the PUBLIC keys into the preset (snippet printed).
//
// Usage:
//   npm i                                  # in ../  (substitute-relay/)
//   node tools/derive-session-keys.mjs "//Alice" "//Bob" "//Charlie" "//Dave"
//   node tools/derive-session-keys.mjs "<bip39 mnemonic>"        # one real validator
//
// SS58 prefix 0 (Paseo) is used for the printed addresses.

import { Keyring } from '@polkadot/keyring';
import { cryptoWaitReady, encodeAddress } from '@polkadot/util-crypto';
import { u8aToHex } from '@polkadot/util';

const SS58 = 0; // Paseo
await cryptoWaitReady();

const seeds = process.argv.slice(2);
if (seeds.length === 0) {
  console.error('usage: node derive-session-keys.mjs "<seed1>" ["<seed2>" ...]');
  process.exit(1);
}

const sr = new Keyring({ type: 'sr25519', ss58Format: SS58 });
const ed = new Keyring({ type: 'ed25519', ss58Format: SS58 });
const ec = new Keyring({ type: 'ecdsa', ss58Format: SS58 });

const pub = (pair) => u8aToHex(pair.publicKey);

for (const seed of seeds) {
  const stash = sr.addFromUri(`${seed}//stash`);
  const ctrl = sr.addFromUri(seed);
  const babe = sr.addFromUri(seed);
  const grandpa = ed.addFromUri(seed);
  const paraVal = sr.addFromUri(seed); // ValidatorId
  const paraAsg = sr.addFromUri(seed); // AssignmentId
  const audi = sr.addFromUri(seed);    // AuthorityDiscovery
  const beefy = ec.addFromUri(seed);   // ecdsa (33-byte compressed)

  console.log(`\n=== authority for seed "${seed}" ===`);
  console.log(`stash      (sr25519) ${stash.address}`);
  console.log(`controller (sr25519) ${ctrl.address}`);
  console.log(`babe                 ${pub(babe)}`);
  console.log(`grandpa    (ed25519) ${pub(grandpa)}`);
  console.log(`para_validator       ${pub(paraVal)}`);
  console.log(`para_assignment      ${pub(paraAsg)}`);
  console.log(`authority_discovery  ${pub(audi)}`);
  console.log(`beefy      (ecdsa)   ${pub(beefy)}`);

  console.log(`\n  # load this authority's keystore on its node:`);
  for (const [t, key, pair] of [
    ['babe', 'babe', babe], ['gran', 'gran', grandpa], ['para', 'para', paraVal],
    ['asgn', 'asgn', paraAsg], ['audi', 'audi', audi], ['beef', 'beef', beefy],
  ]) {
    console.log(`  polkadot key insert --key-type ${key} --scheme ${pair.type} --suri "${seed}" --base-path <node-base>`);
  }
}

console.log(`\n# To substitute these in relay/paseo/src/genesis_config_presets.rs, replace the`);
console.log(`# get_authority_keys_from_seed("Alice"/...) entries with a constructor built from the`);
console.log(`# PUBLIC keys above (e.g. AccountId/BabeId/... ::from(hex), keeping secrets out of source).`);
