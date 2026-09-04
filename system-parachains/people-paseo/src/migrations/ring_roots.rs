// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Individuality.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! PASEO-LOCAL. People's half of the v0.3.1 ring-commitment reshape.
//!
//! # What changed
//!
//! v0.3.1 forces `verifiable` from Paseo's git rev `93464a6` to crates.io `0.3.0`
//! (`indiv-pallet-members` does not compile against the old rev). Upstream commit `d41329f`,
//! *"Store only the ring commitment in `GenerateVerifiable::MembersCommitment`"*, changed the
//! wrapped type from `ark_vrf::ring::RingVerifierKey` to `ark_vrf::ring::RingCommitment` and
//! `MEMBERS_COMMITMENT_SIZE` from 768 to 288. The live bytes are
//! `kzg_verifier_key (480) || ring_commitment (288)`.
//!
//! Two People storage items embed `MembersOf<T>` and are therefore stale:
//!
//! | item | v0.3.0 | v0.3.1 |
//! | --- | --- | --- |
//! | `Members::Root` = `root ++ revision:u32 ++ intermediate:[u8; 848]` | 1620 | 1140 |
//! | `Members::OldRoots` = `root ++ archived_at:u64` | 776 | 296 |
//!
//! `Members::OldRoots` is easy to overlook: `OldRootRetentionDuration` is 600s, so it is usually
//! empty and was empty in the snapshot this was developed against. It is **not always** empty, and
//! an entry present at enactment would be corrupted exactly as a `Root` would.
//!
//! # Why it is silent, and why it is total
//!
//! Both `RingRoot` and `OldRoot` hand-write `Decode` through `DecodeUnchecked` over fixed-size
//! arrays, and SCALE ignores trailing bytes. Fed 768 old bytes the new type *succeeds*, consuming
//! 288 and yielding garbage. Measured across the 24 live `Members::Root` records, the leading 480
//! bytes are **byte-identical in every ring** — so unmigrated, every ring presents the same root
//! and the rings stop being distinguishable. Proof verification reads `root`.
//!
//! # Why dropping 480 bytes is lossless
//!
//! The discarded prefix is `RawKzgVerifierKey { g1, g2, tau_in_g2 }` — the BLS12-381 generators
//! plus the Zcash-ceremony `tau*g2`, 96 + 192 + 192. It is a constant, and v0.3.1 re-derives it
//! with `verifiable::ring::make_canonical_pcs_vk()` instead of storing it.
//!
//! Verified against all 24 live records before this was written: the prefix equals
//! `make_canonical_pcs_vk()` 24/24, the surviving 288 bytes decode **checked** (as real curve
//! points) into a v0.3.1 `MembersCommitment` 24/24, and re-encoding round-trips byte-identically
//! 24/24.
//!
//! ⭐ **The surviving data is the SUFFIX.** `new == old[480..768]`, and `new != old[0..288]`.
//! Truncating from the wrong end produces a value `DecodeUnchecked` accepts and that verifies
//! nothing.
//!
//! # Relationship to Asset Hub
//!
//! `indiv_pallet_members_subscriber::migration::MigrateV0ToV1` performs the same reshape for
//! `MembersSubscriber::RingRoots` on Asset Hub, and owns the canonical write-up of the constant.
//! The two runtimes share no local crate — `system-parachains-common` is a fellowship git
//! dependency we do not own — so the hash below is duplicated deliberately and pinned by a test in
//! each runtime. Upstream ships no migration for either side: `next-people-paseo` genesis-es the
//! v0.3.x shape and never carries legacy ring roots.

use crate::{Runtime, Weight};
#[cfg(any(feature = "try-runtime", test))]
use codec::{Decode, Encode};
#[cfg(feature = "try-runtime")]
use frame_support::ensure;
use frame_support::traits::OnRuntimeUpgrade;
use sp_io::hashing::{blake2_256, twox_128};

/// Encoded length of `MembersOf<T>` under the `verifiable` git rev that produced the live bytes.
pub const OLD_MEMBERS_LEN: usize = 768;
/// Encoded length of the same associated type under `verifiable` 0.3.0.
pub const NEW_MEMBERS_LEN: usize = 288;
/// The bytes that go away: the leading KZG verifier key.
pub const KZG_VERIFIER_KEY_LEN: usize = OLD_MEMBERS_LEN - NEW_MEMBERS_LEN;

/// `blake2_256` of the 480-byte KZG verifier key that prefixes every live ring commitment.
///
/// Must equal `indiv_pallet_members_subscriber::migration::old::CANONICAL_KZG_VERIFIER_KEY_HASH`.
/// Derived twice independently — from Asset Hub's live state when that migration was written, and
/// again from People's 24 live `Members::Root` records here — with identical results.
///
/// Checking it is what makes the transform safe: without it the migration cannot tell an
/// old-format value from a new-format one, and `DecodeUnchecked` will not tell it either.
pub const CANONICAL_KZG_VERIFIER_KEY_HASH: [u8; 32] = [
	0x41, 0x28, 0x21, 0xb3, 0x81, 0x35, 0xd4, 0x44, 0x55, 0x3b, 0x70, 0x9c, 0x44, 0x99, 0xc9, 0x6d,
	0x58, 0x7b, 0xa4, 0xa5, 0x28, 0x6a, 0xa5, 0x99, 0xab, 0x6b, 0xf9, 0xf9, 0xc8, 0x63, 0x9d, 0xef,
];

/// `proof_size` charged per entry, which `DbWeight` does not model.
///
/// `RocksDbWeight` on this runtime declares **ref_time only** (`read: 25_000ns`,
/// `write: 100_000ns`) and a `proof_size` of **zero**. On a parachain the PoV is the binding
/// constraint, so a migration that touches storage and declares `reads_writes()` alone is
/// understating its real cost to nothing.
///
/// 4 KiB per entry is deliberately generous: the values are 1620 bytes read and 1140 written
/// (`Members::Root`) or 776/296 (`Members::OldRoots`), so this leaves >2x headroom for the trie
/// nodes the proof carries. Measured for comparison with `try-runtime on-runtime-upgrade` over a
/// live `people-paseo` 2004003 snapshot: the **whole** single-block migration set produced a
/// 12.6 KB compressed PoV, against the 96 KiB this declares for its 24 entries.
///
/// Same figure, and same reasoning, as `coinage::DECLARED_PROOF_SIZE_PER_ENTRY` in this runtime:
/// being wrong in the generous direction costs block space; being wrong in the other direction
/// costs the chain.
const DECLARED_PROOF_SIZE_PER_ENTRY: u64 = 4 * 1024;

/// Sanity bound on the number of entries this **single-block** migration expects to touch.
///
/// Live count at design time is **24** (`Members::Root`) + **0** (`Members::OldRoots`).
/// `OldRoots` is the open-ended one: `OldRootRetentionDuration` is 600s, so it is empty only
/// because nothing archived recently, and a burst of ring-root revisions shortly before enactment
/// would leave entries behind.
///
/// The bound is **not** a truncation point — stopping half-way would leave the map in exactly the
/// mixed state this migration exists to remove. It is a tripwire: `pre_upgrade` fails loudly under
/// try-runtime, and `on_runtime_upgrade` logs an error while still completing the work correctly.
///
/// 512 x [`DECLARED_PROOF_SIZE_PER_ENTRY`] is 2 MiB, still inside a parachain PoV budget, so even
/// the worst case this admits stays safe.
const EXPECTED_MAX_ENTRIES: u32 = 512;

/// The weight one entry is charged: a read, a write, and the `proof_size` allowance `DbWeight`
/// omits. `ref_time` deliberately keeps coming from `DbWeight`, whose constants are the
/// **reference machine's**, not whatever host happens to build or run this.
fn per_entry_weight() -> Weight {
	<Runtime as frame_system::Config>::DbWeight::get()
		.reads_writes(1, 1)
		.saturating_add(Weight::from_parts(0, DECLARED_PROOF_SIZE_PER_ENTRY))
}

/// The `Members` storage items whose value begins with a `MembersOf<T>`.
const ITEMS: [&str; 2] = ["Root", "OldRoots"];

/// `twox128("Members") ++ twox128(item)`.
fn item_prefix(item: &str) -> alloc::vec::Vec<u8> {
	let mut key = alloc::vec::Vec::with_capacity(32);
	key.extend_from_slice(&twox_128(b"Members"));
	key.extend_from_slice(&twox_128(item.as_bytes()));
	key
}

/// Every raw key under `item`, collected before any rewriting.
///
/// Collected up front deliberately: rewriting in place while walking `next_key` over the same
/// prefix is the mutation-during-iteration trap that bit unit A of the coinage migration.
fn keys_under(item: &str) -> alloc::vec::Vec<alloc::vec::Vec<u8>> {
	let prefix = item_prefix(item);
	let mut keys = alloc::vec::Vec::new();
	let mut cursor = prefix.clone();
	while let Some(next) = sp_io::storage::next_key(&cursor) {
		if !next.starts_with(&prefix) {
			break;
		}
		keys.push(next.clone());
		cursor = next;
	}
	keys
}

/// Drops the leading KZG verifier key, or returns `None` if this value is not old-format.
///
/// Never panics and never truncates blindly. An already-migrated value is rejected rather than
/// silently re-truncated, which is what makes the migration idempotent.
pub fn strip_kzg_verifier_key(old: &[u8]) -> Option<&[u8]> {
	if old.len() < OLD_MEMBERS_LEN {
		return None;
	}
	if blake2_256(&old[..KZG_VERIFIER_KEY_LEN]) != CANONICAL_KZG_VERIFIER_KEY_HASH {
		return None;
	}
	Some(&old[KZG_VERIFIER_KEY_LEN..])
}

/// Reshapes `Members::Root` and `Members::OldRoots` into the v0.3.1 commitment-only layout.
pub struct MigrateRingRootsToCommitmentOnly;

impl OnRuntimeUpgrade for MigrateRingRootsToCommitmentOnly {
	fn on_runtime_upgrade() -> Weight {
		let mut reads = 0u64;
		let mut writes = 0u64;
		let mut untouched = 0u64;

		for item in ITEMS {
			for key in keys_under(item) {
				reads = reads.saturating_add(1);
				let Some(value) = sp_io::storage::get(&key) else { continue };
				match strip_kzg_verifier_key(&value) {
					Some(new) => {
						sp_io::storage::set(&key, new);
						writes = writes.saturating_add(1);
					},
					// Already current, or a shape this migration does not recognise. Either way,
					// leaving it alone is the safe action; `post_upgrade` re-checks.
					None => untouched = untouched.saturating_add(1),
				}
			}
		}

		if reads > EXPECTED_MAX_ENTRIES as u64 {
			// Not fatal, and deliberately not a truncation: a correct migration of a larger map is
			// still correct, whereas stopping half-way would leave the mixed state this migration
			// exists to remove. Loud so an operator sees that the single-block weight assumption
			// was sized against a smaller chain than the one that enacted it.
			log::error!(
				target: "runtime::ring_roots",
				"touched {reads} entries, above the {EXPECTED_MAX_ENTRIES} this single-block \
				 migration was sized for; the block may be heavy.",
			);
		}

		log::info!(
			target: "runtime::ring_roots",
			"Members ring commitments: {writes} reshaped, {untouched} left as-is, \
			 {reads} entries visited",
		);
		per_entry_weight().saturating_mul(reads)
	}

	#[cfg(feature = "try-runtime")]
	fn pre_upgrade() -> Result<alloc::vec::Vec<u8>, sp_runtime::TryRuntimeError> {
		let mut legacy = 0u32;
		for item in ITEMS {
			for key in keys_under(item) {
				if let Some(v) = sp_io::storage::get(&key) {
					if strip_kzg_verifier_key(&v).is_some() {
						legacy += 1;
					}
				}
			}
		}
		// Tripwire, not a limit: this is a single-block migration whose declared weight is sized
		// for `EXPECTED_MAX_ENTRIES`. If the chain grew past that between design and enactment we
		// want a loud try-runtime failure here, not an oversized block on a live network.
		let visited = ITEMS.iter().map(|i| keys_under(i).len()).sum::<usize>() as u32;
		ensure!(
			visited <= EXPECTED_MAX_ENTRIES,
			"ring_roots: more entries than this single-block migration is sized for",
		);
		log::info!(
			target: "runtime::ring_roots",
			"pre_upgrade: {legacy} legacy ring commitments, {visited}/{EXPECTED_MAX_ENTRIES} entries",
		);
		Ok(legacy.encode())
	}

	#[cfg(feature = "try-runtime")]
	fn post_upgrade(state: alloc::vec::Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
		let expected: u32 = Decode::decode(&mut &state[..])
			.map_err(|_| sp_runtime::TryRuntimeError::Other("bad pre_upgrade state"))?;
		let mut remaining = 0u32;
		let mut total = 0u32;
		for item in ITEMS {
			for key in keys_under(item) {
				if let Some(v) = sp_io::storage::get(&key) {
					total += 1;
					if strip_kzg_verifier_key(&v).is_some() {
						remaining += 1;
					}
					ensure!(
						v.len() >= NEW_MEMBERS_LEN,
						"Members ring commitment shorter than a commitment",
					);
				}
			}
		}
		ensure!(remaining == 0, "a legacy-format ring commitment survived the migration");
		log::info!(
			target: "runtime::ring_roots",
			"post_upgrade: {expected} reshaped, {total} ring commitments now current",
		);
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use alloc::vec::Vec;

	/// The 768-byte `MembersOf` of a real `people-paseo` spec-2004003 `Members::Root` record.
	/// Public chain state, used as ground truth so the transform is tested against what is
	/// actually stored rather than against a synthetic value.
	const LIVE_ROOT_HEX: &[&str] = &[
		"17f1d3a73197d7942695638c4fa9ac0fc3688c4f9774b905a14e3a3f171bac586c55e83ff97a1aeffb3af00a",
		"db22c6bb08b3f481e3aaa0f1a09e30ed741d8ae4fcf5e095d5d00af600db18cb2c04b3edd03cc744a2888ae4",
		"0caa232946c5e7e113e02b6052719f607dacd3a088274f65596bd0d09920b61ab5da61bbdc7f5049334cf112",
		"13945d57e5ac7d055d042b7e024aa2b2f08f0a91260805272dc51051c6e47ad4fa403b02b4510b647ae3d177",
		"0bac0326a805bbefd48056c8c121bdb80606c4a02ea734cc32acd2b02bc28b99cb3e287e85a763af267492ab",
		"572e99ab3f370d275cec1da1aaa9075ff05f79be0ce5d527727d6e118cc9cdc6da2e351aadfd9baa8cbdd3a7",
		"6d429a695160d12c923ac9cc3baca289e193548608b82801116b2250fc4b3098ccadf9ec58526aecf307d21e",
		"233f65110554ee04460f72a616f4d070941b0796de5defda29196e40013645b8518e5568e75d320da8b83a87",
		"e12406956b9c74491ee5ecf06dc297ba1186016209d78fd832c16ef62fc49eff0bc832fb878c2f66addd846a",
		"a85954b856c3bc70d87344f9006889e58eb0ec5734151f99b9a5b653e991247ad506d0fb12db3e2e8318b647",
		"537f00e6b702fc8526e8c9101a7b7cbc410fbf8f6021f2e45393c8696431c1a6fcc7f5a7d90ab999006ef202",
		"30befe4e7ac499ae28ec1dc8cc23d20b7baac108845fb60b0482c7429da0406dd1984cc896c21e87876b6a11",
		"0fc02b3c78b89f5426127632d2786de9a225d2c756409ab071ee36f411e2e9b4692cb432c7ba64a345ad85c1",
		"fed513af134b249820596390f2608a0a658f285dc808f3df04cc1f60737916876b3740464b8ac64e14867320",
		"cb61e41e5d860c9f03fd18abe3205e50a702469acdd37b43b4b532609e862b2d03731d1cd3fbba18bd47709a",
		"1c729bfd438ad2a9aa908f3a11c7ea6ca6ea24fe6c5a61ebf4e8c7c053faad1b12d999bd655418b4fbd892ac",
		"4804841e5b94345c06a45b5d22d59076093f5596fb3d43b85539a24ff26ec0d4f2ab61482549d738134b42f8",
		"a3b35d712eeaeee7074a53e95ebd3dfcff7df576",
	];

	fn live_root() -> Vec<u8> {
		let hex: alloc::string::String = LIVE_ROOT_HEX.concat();
		(0..hex.len())
			.step_by(2)
			.map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
			.collect()
	}

	#[test]
	fn live_record_carries_the_canonical_verifier_key() {
		let old = live_root();
		assert_eq!(old.len(), OLD_MEMBERS_LEN);
		assert_eq!(blake2_256(&old[..KZG_VERIFIER_KEY_LEN]), CANONICAL_KZG_VERIFIER_KEY_HASH);
	}

	/// The surviving data is the SUFFIX, and it is a real commitment: `verifiable` 0.3.0 decodes
	/// it and re-encodes it byte-identically.
	#[test]
	fn surviving_suffix_is_a_valid_commitment() {
		let old = live_root();
		let new = strip_kzg_verifier_key(&old).expect("live record must convert");
		assert_eq!(new.len(), NEW_MEMBERS_LEN);
		assert_eq!(new, &old[KZG_VERIFIER_KEY_LEN..]);

		type Commitment = verifiable::ring::MembersCommitment<
			verifiable::ring::bandersnatch::BandersnatchSha512Ell2,
		>;
		let decoded = Commitment::decode(&mut &new[..]).expect("suffix must decode");
		assert_eq!(decoded.encode(), new, "commitment must round-trip byte-identically");
	}

	/// Truncating from the wrong end is the failure mode this migration exists to prevent.
	/// The guard must refuse such a value rather than treat it as convertible.
	#[test]
	fn wrong_end_truncation_is_refused() {
		let old = live_root();
		let wrong = &old[..NEW_MEMBERS_LEN];
		assert!(
			strip_kzg_verifier_key(wrong).is_none(),
			"a prefix-truncated value must not be accepted as legacy",
		);
	}

	/// The declared worst case must stay inside a parachain PoV budget. If someone raises
	/// `EXPECTED_MAX_ENTRIES` or `DECLARED_PROOF_SIZE_PER_ENTRY`, this is what catches an
	/// unshippable combination.
	#[test]
	fn declared_worst_case_fits_a_pov() {
		let worst = per_entry_weight().saturating_mul(EXPECTED_MAX_ENTRIES as u64);
		// 5 MiB is the relay's per-candidate PoV ceiling; stay an order of magnitude under it.
		assert!(
			worst.proof_size() <= 2 * 1024 * 1024,
			"declared worst-case proof_size {} exceeds 2 MiB",
			worst.proof_size(),
		);
		// And the live population must sit far below the bound it is sized against.
		assert!(24 * 4 < EXPECTED_MAX_ENTRIES, "bound leaves too little headroom");
	}

	/// Running twice must be a no-op, not a second truncation.
	#[test]
	fn is_idempotent() {
		let old = live_root();
		let once = strip_kzg_verifier_key(&old).expect("first pass converts").to_vec();
		assert!(strip_kzg_verifier_key(&once).is_none(), "second pass must refuse");
	}
}
