#!/usr/bin/env node
// Turn community-operator key submissions into the Rust to paste into the `substitute` genesis
// preset (`relay/paseo/src/genesis_config_presets.rs`, `paseo_substitute_genesis`).
//
// Each operator submits (see substitute-relay/README.md "Operator key intake"):
//   - `stash`: their validator account (SS58 or 0x-hex, 32 bytes), and
//   - `sessionKeys`: the 0x blob returned by `author_rotateKeys` on THEIR node (193 bytes =
//      babe.32 + grandpa.32 + para_validator.32 + para_assignment.32 + authority_discovery.32 +
//      beefy.33, in SessionKeys order),
//   OR the six public keys individually (babe/grandpa/paraValidator/paraAssignment/
//      authorityDiscovery/beefy), OR a `seed` (dev/team-generated only — never for real keys).
//
// Usage:
//   npm i                                    # in substitute-relay/
//   node tools/format-operator-keys.mjs operators.json
//
// operators.json is a JSON array, e.g.:
//   [
//     { "name": "acme",  "stash": "13...", "sessionKeys": "0x<193 bytes>" },
//     { "name": "globex", "stash": "0x..", "babe":"0x..", "grandpa":"0x..", "paraValidator":"0x..",
//        "paraAssignment":"0x..", "authorityDiscovery":"0x..", "beefy":"0x.." },
//     { "name": "dev-alice", "seed": "//Alice" }
//   ]

import { readFileSync } from 'node:fs';
import { Keyring } from '@polkadot/keyring';
import { cryptoWaitReady, decodeAddress } from '@polkadot/util-crypto';
import { hexToU8a, u8aToHex, isHex } from '@polkadot/util';
import { ed25519, RistrettoPoint } from '@noble/curves/ed25519';
import { secp256k1 } from '@noble/curves/secp256k1';

await cryptoWaitReady();

// --- crypto-type verification ---------------------------------------------------------------
// Each session key must be the crypto the runtime expects. A wrong-type key (e.g. an ed25519 key
// submitted as `babe`, or a grandpa/babe swap) would leave the validator unable to author/finalize.
// We check the public key actually lies on the expected curve. `stash` is an account (any 32 bytes),
// so it is not curve-checked.
const _Ed = ed25519.Point || ed25519.ExtendedPoint;
const onEd25519 = (u8) => { try { _Ed.fromHex(u8aToHex(u8, undefined, false)); return true; } catch { return false; } };
const onRistretto = (u8) => { try { RistrettoPoint.fromHex(u8aToHex(u8, undefined, false)); return true; } catch { return false; } };
const onSecp256k1 = (u8) => { try { secp256k1.Point.fromHex(u8aToHex(u8, undefined, false)); return true; } catch { return false; } };
const SCHEME = {
  babe: ['sr25519', onRistretto],
  grandpa: ['ed25519', onEd25519],
  paraValidator: ['sr25519', onRistretto],
  paraAssignment: ['sr25519', onRistretto],
  authorityDiscovery: ['sr25519', onRistretto],
  beefy: ['ecdsa/secp256k1', onSecp256k1],
};
function verifyCrypto(name, k) {
  const bad = [];
  for (const [field, [scheme, ok]] of Object.entries(SCHEME)) {
    if (!ok(k[field]))
      bad.push(`${field} is not a valid ${scheme} public key (0x${u8aToHex(k[field], undefined, false)})`);
  }
  if (bad.length) {
    const hint = bad.some((b) => b.startsWith('babe') || b.startsWith('grandpa'))
      ? ' — likely a babe<->grandpa swap (runtime SessionKeys is grandpa-first).'
      : '';
    throw new Error(`${name}: crypto-type check failed:\n    - ${bad.join('\n    - ')}${hint}`);
  }
}

const path = process.argv[2] || 'operators.json';
const operators = JSON.parse(readFileSync(path, 'utf8'));
if (!Array.isArray(operators)) throw new Error('expected a JSON array of operators');

// SessionKeys layout (declaration order) + byte sizes — MUST match the runtime's SessionKeys.
// The Paseo relay declares GRANDPA FIRST, then babe (relay/paseo/src/lib.rs). So an
// `author_rotateKeys` blob is grandpa-first; splitting it babe-first swaps babe<->grandpa
// (a real submission hit exactly this). This order is authoritative for blob-splitting.
const LAYOUT = [
  ['grandpa', 32],
  ['babe', 32],
  ['paraValidator', 32],
  ['paraAssignment', 32],
  ['authorityDiscovery', 32],
  ['beefy', 33],
];
const BLOB_LEN = LAYOUT.reduce((n, [, s]) => n + s, 0); // 193

const hx = (u8) => u8aToHex(u8, undefined, false); // no 0x prefix, for hex!["..."]
const want = (u8, n, label) => {
  if (u8.length !== n) throw new Error(`${label}: expected ${n} bytes, got ${u8.length}`);
  return u8;
};

function deriveFromSeed(seed) {
  const sr = new Keyring({ type: 'sr25519' });
  const ed = new Keyring({ type: 'ed25519' });
  const ec = new Keyring({ type: 'ecdsa' });
  return {
    stash: sr.addFromUri(`${seed}//stash`).publicKey,
    babe: sr.addFromUri(seed).publicKey,
    grandpa: ed.addFromUri(seed).publicKey,
    paraValidator: sr.addFromUri(seed).publicKey,
    paraAssignment: sr.addFromUri(seed).publicKey,
    authorityDiscovery: sr.addFromUri(seed).publicKey,
    beefy: ec.addFromUri(seed).publicKey,
  };
}

function resolve(op) {
  // dev/team only: derive everything (incl. stash) from a seed
  if (op.seed) {
    return deriveFromSeed(op.seed);
  }

  // stash accepts SS58 or 0x-hex
  const stash = want(isHex(op.stash) ? hexToU8a(op.stash) : decodeAddress(op.stash), 32, `${op.name} stash`);

  if (op.sessionKeys) {
    const blob = hexToU8a(op.sessionKeys);
    if (blob.length !== BLOB_LEN)
      throw new Error(`${op.name} sessionKeys: expected ${BLOB_LEN} bytes (author_rotateKeys blob), got ${blob.length}`);
    const out = { stash };
    let off = 0;
    for (const [name, size] of LAYOUT) {
      out[name] = blob.slice(off, off + size);
      off += size;
    }
    return out;
  }
  // individual keys
  const out = { stash };
  for (const [name, size] of LAYOUT) {
    if (!op[name]) throw new Error(`${op.name}: missing ${name} (provide sessionKeys blob, the six keys, or a seed)`);
    out[name] = want(hexToU8a(op[name]), size, `${op.name} ${name}`);
  }
  return out;
}

const snippets = [];
for (const op of operators) {
  const k = resolve(op);
  verifyCrypto(op.name, k); // hard-fail on wrong-crypto keys (e.g. babe<->grandpa swap)
  // arg order matches substitute_authority(): stash, babe, grandpa, para_validator,
  // para_assignment, authority_discovery, beefy.
  snippets.push(
    `\t\t// ${op.name}\n` +
      `\t\tsubstitute_authority(\n` +
      `\t\t\thex!["${hx(k.stash)}"],\n` +
      `\t\t\thex!["${hx(k.babe)}"],\n` +
      `\t\t\thex!["${hx(k.grandpa)}"],\n` +
      `\t\t\thex!["${hx(k.paraValidator)}"],\n` +
      `\t\t\thex!["${hx(k.paraAssignment)}"],\n` +
      `\t\t\thex!["${hx(k.authorityDiscovery)}"],\n` +
      `\t\t\thex!["${hx(k.beefy)}"],\n` +
      `\t\t),`,
  );
}

console.log(`// ${operators.length} authorit${operators.length === 1 ? 'y' : 'ies'} — paste into the initial_authorities vec\n`);
console.log(`let initial_authorities = vec![\n${snippets.join('\n')}\n\t];`);
console.error(`\n[ok] formatted ${operators.length} operator(s) from ${path}`);
