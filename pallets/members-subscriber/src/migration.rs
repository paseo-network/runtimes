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

//! PASEO-LOCAL storage migrations for `indiv-pallet-members-subscriber`.
//!
//! This file does not exist upstream. Upstream's `next-asset-hub-paseo` genesis-es the v0.3.x
//! storage shape, so it never has to migrate into it. `asset-hub-paseo` does: it has been
//! running the pre-v0.3 shape live since spec `2_004_002`, with an `Active` subscription and a
//! `ProcessingState.last_processed_sequence` in the low thousands.
//!
//! # What v0.3.x changed, and why each part is load-bearing
//!
//! Three independent breaks land at once. All three must be repaired in ONE migration, because
//! repairing any subset leaves `RingRoots` in a state that is worse than not migrating at all.
//!
//! 1. **`RingRoots` gains a leading `Generation` key.** It went from
//!    `StorageDoubleMap<Blake2_128Concat Identifier, Blake2_128Concat RingIndex>` to
//!    `StorageNMap<(Twox64Concat Generation, Blake2_128Concat Identifier,
//!    Blake2_128Concat RingIndex)>`. Same *item* prefix, 12 more bytes of key. Every live root
//!    becomes unreachable.
//!
//! 2. **`RingCollectionState` gains `next_scan_index: u32` MID-STRUCT**, between
//!    `next_ring_index` and `missing_indices`. Live values are 10 bytes; the new type needs at
//!    least 14. `unhashed::get` swallows the `codec::Error` and returns `None`, and `ValueQuery`
//!    then substitutes `Default`. The failure is completely silent.
//!
//! 3. **The `verifiable` bump reshapes the ring commitment itself.** This one is NOT in
//!    `INDIVIDUALITY_MIGRATIONS_DESIGN.md` §3 and it invalidates that document's instruction to
//!    copy `RingRoots` values verbatim. See [`v1`] for the byte-level detail.
//!
//! Consequence of getting this wrong: `asset-hub-paseo` verifies personhood ring proofs against
//! these roots (`indiv_pallet_alias_accounts::ProofOf<Runtime>`, the dotNS-gateway PoP paths).
//! A mis-migrated `RingRoots` breaks every ring-proof-gated call on AssetHub, and — because
//! `RingCommitmentRecord`'s hand-written `Decode` routes `root` through `DecodeUnchecked` — it
//! can do so while still *appearing* to decode. Hence the byte-level integrity check below.

extern crate alloc;

use crate::{
	types::{Generation, Identifier, RingIndex},
	Config, CurrentGeneration, Pallet, RingCollectionStates, RingRoots,
};
use frame_support::{
	migrations::VersionedMigration, pallet_prelude::*, traits::UncheckedOnRuntimeUpgrade,
	CloneNoBound, DebugNoBound, DefaultNoBound, EqNoBound, PartialEqNoBound,
};

const LOG_TARGET: &str = "runtime::indiv-pallet-members-subscriber::migration";

/// Repairs `RingRoots` and `RingCollectionStates` for the individuality v0.3.1 storage shape.
///
/// `VersionedMigration` is mandatory here, not stylistic: [`v1::MigrateRingRootsToGeneration`]
/// is **not idempotent**. A second run would find the already-migrated 288-byte roots, fail the
/// canonical-prefix check, and abort — loudly, but only because of the guard in
/// [`v1`]. The storage version is the real protection.
pub type MigrateV0ToV1<T> = VersionedMigration<
	0,
	1,
	v1::MigrateRingRootsToGeneration<T>,
	Pallet<T>,
	<T as frame_system::Config>::DbWeight,
>;

pub mod v1 {
	use super::*;
	use crate::types::{MembersOf, RingCommitmentRecord, RingCollectionState, SequenceNumber};
	use alloc::vec::Vec;
	use indiv_support::traits::RevisionIndex;
	use sp_io::hashing::blake2_256;

	/// Encoded length of `<T::Crypto as GenerateVerifiable>::Members` under the `verifiable` git
	/// rev `93464a6` that produced the live bytes. It wrapped `ark_vrf::ring::RingVerifierKey`,
	/// which ark-serializes (uncompressed, no length prefix) as
	/// `kzg_verifier_key (480) || ring_commitment (288)`.
	pub const OLD_MEMBERS_LEN: usize = 768;

	/// Encoded length of the same associated type under `verifiable` 0.3.0. Upstream commit
	/// `d41329f` "Store only the ring commitment in `GenerateVerifiable::MembersCommitment`"
	/// changed the wrapped type to `ark_vrf::ring::RingCommitment`, dropping the KZG verifier
	/// key. `MEMBERS_COMMITMENT_SIZE` went 768 -> 288 in the same commit.
	pub const NEW_MEMBERS_LEN: usize = 288;

	/// The bytes that go away: the leading KZG verifier key.
	///
	/// **The surviving data is the SUFFIX, not a prefix.** `new == old[480..768]`, and
	/// `new != old[0..288]`. Truncating from the wrong end produces a value that
	/// `DecodeUnchecked` accepts and that verifies nothing.
	pub const KZG_VERIFIER_KEY_LEN: usize = OLD_MEMBERS_LEN - NEW_MEMBERS_LEN;

	/// `blake2_256` of the 480-byte KZG verifier key that prefixes every live ring commitment.
	///
	/// It is a constant: the BLS12-381 `g1` and `g2` generators plus the Zcash-ceremony `tau*g2`
	/// (`RawKzgVerifierKey { g1, g2, tau_in_g2 }`). Verified read-only against
	/// `asset-hub-paseo` spec `2004002` and `people-paseo` spec `2004003`: **33 live ring
	/// commitments** (3 `MembersSubscriber::RingRoots` entries holding 8 records, 24
	/// `Members::Root`, 1 `Members::OldRoots`) are all 768 bytes and all share this exact
	/// prefix — one distinct value across the whole population.
	///
	/// Checking it is what makes the transform safe. Without it the migration cannot tell an
	/// old-format value from a new-format one, and `DecodeUnchecked` will not tell it either:
	/// fed 768 old bytes it *succeeds*, consuming 288 and yielding garbage.
	pub const CANONICAL_KZG_VERIFIER_KEY_HASH: [u8; 32] = [
		0x41, 0x28, 0x21, 0xb3, 0x81, 0x35, 0xd4, 0x44, 0x55, 0x3b, 0x70, 0x9c, 0x44, 0x99, 0xc9,
		0x6d, 0x58, 0x7b, 0xa4, 0xa5, 0x28, 0x6a, 0xa5, 0x99, 0xab, 0x6b, 0xf9, 0xf9, 0xc8, 0x63,
		0x9d, 0xef,
	];

	/// Sanity bound on the number of `RingRoots` entries this single-block migration expects.
	///
	/// Live count at design time is **3**. `MaxCollections` is 10 on `asset-hub-paseo` and ring
	/// indices track the notifier's ring count, so a realistic ceiling is well under this. The
	/// bound exists so that a chain that grew unexpectedly between design and enactment shows up
	/// as a loud `pre_upgrade` failure under try-runtime rather than as an oversized block.
	pub const EXPECTED_MAX_RING_ROOT_ENTRIES: u32 = 512;

	/// A `RingCommitmentRecord` as stored before the `verifiable` bump.
	///
	/// `root` is deliberately kept as raw bytes rather than typed as `MembersOf<T>`. Decoding it
	/// through the new type is exactly the mistake this migration exists to avoid.
	#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
	pub struct OldRingCommitmentRecord {
		/// `kzg_verifier_key (480) || ring_commitment (288)`.
		pub root: [u8; OLD_MEMBERS_LEN],
		pub revision: RevisionIndex,
		pub source_time: u64,
		pub source_sequence: SequenceNumber,
	}

	/// `RingCollectionState` as stored before `next_scan_index` was inserted.
	#[derive(
		Encode, Decode, MaxEncodedLen, TypeInfo, CloneNoBound, PartialEqNoBound, EqNoBound,
		DebugNoBound, DefaultNoBound,
	)]
	#[scale_info(skip_type_params(MaxMissing, MaxDeleted))]
	pub struct OldRingCollectionState<MaxMissing: Get<u32>, MaxDeleted: Get<u32>> {
		pub ring_count: u32,
		pub next_ring_index: u32,
		pub missing_indices: BoundedBTreeMap<RingIndex, u32, MaxMissing>,
		pub deleted_indices: BoundedBTreeSet<RingIndex, MaxDeleted>,
	}

	/// The pre-v0.3 layouts, at their original prefixes.
	///
	/// Note both aliases resolve to the SAME storage prefixes the new items use — only the key
	/// suffix (`RingRoots`) and the value encoding (`RingCollectionStates`) differ. That is why
	/// the migration must read everything before it writes anything.
	pub mod old {
		use super::*;

		#[frame_support::storage_alias]
		pub type RingRoots<T: Config> = StorageDoubleMap<
			Pallet<T>,
			Blake2_128Concat,
			Identifier,
			Blake2_128Concat,
			RingIndex,
			BoundedVec<OldRingCommitmentRecord, <T as Config>::MaxRecentRootsPerRing>,
			OptionQuery,
		>;

		#[frame_support::storage_alias]
		pub type RingCollectionStates<T: Config> = StorageMap<
			Pallet<T>,
			Blake2_128Concat,
			Identifier,
			OldRingCollectionState<
				<T as Config>::MaxMissingRootsPerCollection,
				<T as Config>::MaxDeletedRingsPerCollection,
			>,
			ValueQuery,
		>;
	}

	/// The pure byte-level half of the reshape: drop the leading KZG verifier key.
	///
	/// Split out from [`convert_root`] so it can be unit-tested against real chain bytes without
	/// standing up a runtime — the mock's `Crypto` is not the Bandersnatch suite, so the typed
	/// path cannot be exercised with production data.
	///
	/// Returns `None` unless the input is exactly [`OLD_MEMBERS_LEN`] bytes whose first
	/// [`KZG_VERIFIER_KEY_LEN`] hash to [`CANONICAL_KZG_VERIFIER_KEY_HASH`]. An
	/// already-migrated 288-byte value is therefore rejected, not silently re-truncated.
	pub fn strip_kzg_verifier_key(old_root: &[u8]) -> Option<&[u8]> {
		if old_root.len() != OLD_MEMBERS_LEN {
			log::error!(
				target: LOG_TARGET,
				"ring root is {} bytes, expected the pre-v0.3 {OLD_MEMBERS_LEN}",
				old_root.len(),
			);
			return None;
		}
		if blake2_256(&old_root[..KZG_VERIFIER_KEY_LEN]) != CANONICAL_KZG_VERIFIER_KEY_HASH {
			log::error!(
				target: LOG_TARGET,
				"ring root does not carry the canonical KZG verifier key prefix; refusing to \
				 reshape it. This value is either already migrated or not in the expected \
				 pre-v0.3 format.",
			);
			return None;
		}
		// The SUFFIX survives. Taking `[..NEW_MEMBERS_LEN]` here would produce a value that
		// `DecodeUnchecked` accepts and that verifies nothing.
		Some(&old_root[KZG_VERIFIER_KEY_LEN..])
	}

	/// Converts one stored root from the old 768-byte layout to the new 288-byte one.
	///
	/// Returns `None` — never panics, never truncates blindly — if the leading 480 bytes are not
	/// the canonical KZG verifier key, or if the surviving 288 bytes do not decode as a valid
	/// `Members` value, or if decoding leaves bytes unconsumed.
	///
	/// The decode here is the CHECKED `Decode`, not `DecodeUnchecked`. On a one-shot migration
	/// the arkworks curve-point validation is worth paying for: it is the only thing standing
	/// between a byte-splicing bug and a silently unverifiable ring root.
	pub fn convert_root<T: Config>(old_root: &[u8; OLD_MEMBERS_LEN]) -> Option<MembersOf<T>> {
		let mut tail = strip_kzg_verifier_key(old_root)?;
		let decoded = <MembersOf<T> as Decode>::decode(&mut tail)
			.inspect_err(|e| {
				log::error!(
					target: LOG_TARGET,
					"ring commitment tail failed checked decode: {e:?}",
				);
			})
			.ok()?;
		if !tail.is_empty() {
			log::error!(
				target: LOG_TARGET,
				"ring commitment tail left {} bytes unconsumed; expected exactly {}",
				tail.len(),
				NEW_MEMBERS_LEN,
			);
			return None;
		}
		Some(decoded)
	}

	/// Use [`super::MigrateV0ToV1`] rather than this directly.
	///
	/// # Behaviour
	///
	/// - `CurrentGeneration` is set to `0` explicitly. `ValueQuery` already defaults to `0`, but
	///   writing it makes the invariant auditable and the migration self-documenting: the
	///   generation the migrated roots live under is stated, not implied.
	/// - Every `RingRoots` entry is re-keyed under generation `0` AND its records' roots are
	///   re-encoded from 768 to 288 bytes. The old key is removed.
	/// - Every `RingCollectionStates` value is re-encoded with `next_scan_index` inserted.
	/// - `QueuedRingPurge` is left unset: `OptionQuery` `None` means "no purge in flight", which
	///   is correct — there are no stale generations to purge.
	/// - `Subscription`, `ProcessingState` and `RingCollectionExponents` are NOT touched.
	///
	/// # `next_scan_index` seed: `0`
	///
	/// The field is "the lowest ring index the gap scan has not examined yet", and per its own
	/// doc two kinds of index below it are *never revisited*. Seeding it to `next_ring_index`
	/// would be cheaper but would permanently skip any index the notifier deleted below the
	/// cursor. Seeding it to `0` costs one full re-scan — 2 and 1 indices on the two live
	/// collections — and cannot skip anything. There is no reason to take the "never revisited"
	/// risk to save three reads.
	///
	/// # Failure policy: all-or-nothing
	///
	/// Phase 1 reads and converts everything into memory, writing nothing. Phase 2 writes only
	/// if every conversion succeeded. A partially re-keyed `RingRoots` is the worst reachable
	/// state — some ring proofs would verify and others would not, with no error anywhere — so
	/// the migration would rather do nothing and leave a loud log line.
	///
	/// This means an aborted run still bumps the storage version (that is `VersionedMigration`'s
	/// unconditional behaviour) and will not retry. That is deliberate: an abort means the live
	/// bytes were not what this code was written against, and the correct response is a human
	/// looking at the chain, not an automatic retry.
	pub struct MigrateRingRootsToGeneration<T>(PhantomData<T>);

	impl<T: Config> UncheckedOnRuntimeUpgrade for MigrateRingRootsToGeneration<T> {
		fn on_runtime_upgrade() -> Weight {
			let mut reads = 0u64;
			let mut writes = 0u64;

			// ---- Phase 1: read and convert. No writes. ----
			let mut converted_roots: Vec<(
				Identifier,
				RingIndex,
				BoundedVec<RingCommitmentRecord<T>, T::MaxRecentRootsPerRing>,
			)> = Vec::new();

			for (identifier, ring_index, old_records) in old::RingRoots::<T>::iter() {
				reads = reads.saturating_add(1);
				let mut new_records: BoundedVec<
					RingCommitmentRecord<T>,
					T::MaxRecentRootsPerRing,
				> = BoundedVec::new();
				for old_record in old_records.iter() {
					let Some(root) = convert_root::<T>(&old_record.root) else {
						log::error!(
							target: LOG_TARGET,
							"ABORTING: could not reshape a ring commitment for collection \
							 {identifier:?} ring {ring_index}. NOTHING has been written; \
							 RingRoots is untouched and still holds the pre-v0.3 layout.",
						);
						return T::DbWeight::get().reads(reads);
					};
					if new_records
						.try_push(RingCommitmentRecord::<T> {
							root,
							revision: old_record.revision,
							source_time: old_record.source_time,
							source_sequence: old_record.source_sequence,
						})
						.is_err()
					{
						// Unreachable: the source BoundedVec has the same bound. Handled rather
						// than `expect`ed because a panic in a single-block migration takes the
						// block with it.
						log::error!(
							target: LOG_TARGET,
							"ABORTING: MaxRecentRootsPerRing overflow rebuilding collection \
							 {identifier:?} ring {ring_index}. NOTHING has been written.",
						);
						return T::DbWeight::get().reads(reads);
					}
				}
				converted_roots.push((identifier, ring_index, new_records));
			}

			let entries = converted_roots.len() as u32;
			if entries > EXPECTED_MAX_RING_ROOT_ENTRIES {
				// Not fatal — a correct migration of a bigger map is still a correct migration —
				// but an operator should see that the single-block weight assumption was based
				// on a smaller chain than the one that enacted it.
				log::error!(
					target: LOG_TARGET,
					"RingRoots holds {entries} entries, above the {EXPECTED_MAX_RING_ROOT_ENTRIES} \
					 this single-block migration was sized for; the block may be heavy.",
				);
			}

			let mut converted_states: Vec<(
				Identifier,
				RingCollectionState<
					T::MaxMissingRootsPerCollection,
					T::MaxDeletedRingsPerCollection,
				>,
			)> = Vec::new();
			for (identifier, old_state) in old::RingCollectionStates::<T>::iter() {
				reads = reads.saturating_add(1);
				converted_states.push((
					identifier,
					RingCollectionState {
						ring_count: old_state.ring_count,
						next_ring_index: old_state.next_ring_index,
						// See the type-level doc: seeded to 0, deliberately.
						next_scan_index: 0,
						missing_indices: old_state.missing_indices,
						deleted_indices: old_state.deleted_indices,
					},
				));
			}

			// ---- Phase 2: write. ----
			CurrentGeneration::<T>::put(Generation::from(0u32));
			writes = writes.saturating_add(1);

			for (identifier, ring_index, records) in converted_roots {
				// Remove first: the old double-map key and the new NMap key live under the SAME
				// item prefix, so an un-removed old key would sit inside the new map's keyspace.
				old::RingRoots::<T>::remove(identifier, ring_index);
				RingRoots::<T>::insert((Generation::from(0u32), identifier, ring_index), records);
				writes = writes.saturating_add(2);
			}

			let states = converted_states.len() as u32;
			for (identifier, state) in converted_states {
				// Same key, new value encoding — an overwrite in place.
				RingCollectionStates::<T>::insert(identifier, state);
				writes = writes.saturating_add(1);
			}

			log::info!(
				target: LOG_TARGET,
				"re-keyed {entries} RingRoots entries under generation 0, reshaped their ring \
				 commitments from {OLD_MEMBERS_LEN} to {NEW_MEMBERS_LEN} bytes, and translated \
				 {states} RingCollectionStates",
			);

			T::DbWeight::get().reads_writes(reads, writes)
		}

		#[cfg(feature = "try-runtime")]
		fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
			use crate::{ProcessingState, RingCollectionExponents, Subscription};

			ensure!(
				CurrentGeneration::<T>::get() == 0,
				"members-subscriber: CurrentGeneration is already non-zero"
			);
			ensure!(
				crate::QueuedRingPurge::<T>::get().is_none(),
				"members-subscriber: QueuedRingPurge is already set"
			);

			// Capture the roots as the bytes the migration is REQUIRED to produce, so
			// `post_upgrade` can byte-compare rather than structurally compare. Structural
			// comparison is not good enough here: `RingCommitmentRecord::root` decodes through
			// `DecodeUnchecked`, so a structurally-equal compare can pass on a corrupted curve
			// point.
			//
			// NOTE this deliberately contradicts INDIVIDUALITY_MIGRATIONS_DESIGN.md §3.5
			// assertion 2, which says the new value must be byte-identical to the OLD value.
			// It must not be: the ring commitment is reshaped. Asserting old-byte-identity
			// would fail on a correct migration and pass on one that skipped the reshape.
			let mut roots: Vec<(Identifier, RingIndex, Vec<(Vec<u8>, RevisionIndex, u64, u64)>)> =
				Vec::new();
			for (identifier, ring_index, records) in old::RingRoots::<T>::iter() {
				let mut expected = Vec::new();
				for r in records.iter() {
					ensure!(
						blake2_256(&r.root[..KZG_VERIFIER_KEY_LEN]) ==
							CANONICAL_KZG_VERIFIER_KEY_HASH,
						"members-subscriber: a live ring root does not carry the canonical KZG \
						 verifier key prefix — the stored `Members` layout is not what this \
						 migration was written against"
					);
					expected.push((
						r.root[KZG_VERIFIER_KEY_LEN..].to_vec(),
						r.revision,
						r.source_time,
						r.source_sequence,
					));
				}
				roots.push((identifier, ring_index, expected));
			}
			ensure!(
				roots.len() as u32 <= EXPECTED_MAX_RING_ROOT_ENTRIES,
				"members-subscriber: RingRoots is larger than this single-block migration was \
				 sized for — re-cost it as an MBM before enacting"
			);

			let mut states: Vec<(Identifier, u32, u32, Vec<(RingIndex, u32)>, Vec<RingIndex>)> =
				Vec::new();
			for (identifier, s) in old::RingCollectionStates::<T>::iter() {
				states.push((
					identifier,
					s.ring_count,
					s.next_ring_index,
					s.missing_indices.iter().map(|(k, v)| (*k, *v)).collect(),
					s.deleted_indices.iter().copied().collect(),
				));
			}

			let exponents: Vec<_> = RingCollectionExponents::<T>::iter().collect();

			log::info!(
				target: LOG_TARGET,
				"pre_upgrade: {} RingRoots entries, {} RingCollectionStates, {} exponents",
				roots.len(), states.len(), exponents.len(),
			);

			Ok((roots, states, Subscription::<T>::get(), ProcessingState::<T>::get(), exponents)
				.encode())
		}

		#[cfg(feature = "try-runtime")]
		fn post_upgrade(state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
			use crate::{
				types::{SubscriptionStatus, UpdatesProcessingState},
				ProcessingState, RingCollectionExponents, Subscription,
			};
			use indiv_support::traits::RingExponent;

			type Captured = (
				Vec<(Identifier, RingIndex, Vec<(Vec<u8>, RevisionIndex, u64, u64)>)>,
				Vec<(Identifier, u32, u32, Vec<(RingIndex, u32)>, Vec<RingIndex>)>,
				SubscriptionStatus,
				UpdatesProcessingState,
				Vec<(Identifier, RingExponent)>,
			);
			let (roots, states, subscription, processing, exponents) =
				<Captured>::decode(&mut &state[..]).map_err(|_| {
					sp_runtime::TryRuntimeError::Other("pre_upgrade state failed to decode")
				})?;

			// 1. Generation is pinned at 0 and no purge was queued.
			ensure!(
				CurrentGeneration::<T>::get() == 0,
				"members-subscriber: CurrentGeneration is not 0"
			);
			ensure!(
				crate::QueuedRingPurge::<T>::get().is_none(),
				"members-subscriber: QueuedRingPurge was set by the migration"
			);

			// 2. Every captured root exists under generation 0, and each record's ring
			//    commitment is BYTE-IDENTICAL to the 288-byte tail of the old 768-byte value.
			for (identifier, ring_index, expected) in &roots {
				let got = RingRoots::<T>::get((0u32, *identifier, *ring_index)).ok_or(
					sp_runtime::TryRuntimeError::Other(
						"members-subscriber: a ring root is missing under generation 0",
					),
				)?;
				ensure!(
					got.len() == expected.len(),
					"members-subscriber: ring root record count changed"
				);
				for (g, (want_root, want_rev, want_time, want_seq)) in got.iter().zip(expected) {
					ensure!(
						g.root.encode() == *want_root,
						"members-subscriber: reshaped ring commitment is not the 288-byte tail \
						 of the stored 768-byte value"
					);
					ensure!(
						g.root.encode().len() == NEW_MEMBERS_LEN,
						"members-subscriber: reshaped ring commitment is not 288 bytes"
					);
					ensure!(
						g.revision == *want_rev &&
							g.source_time == *want_time &&
							g.source_sequence == *want_seq,
						"members-subscriber: ring root metadata changed"
					);
				}
			}

			// 3. No entry is left at the pre-v0.3 key layout.
			ensure!(
				old::RingRoots::<T>::iter_keys().next().is_none(),
				"members-subscriber: an entry remains at the pre-v0.3 RingRoots key layout"
			);
			// ...and the new map holds exactly what was captured, nothing invented.
			ensure!(
				RingRoots::<T>::iter_keys().count() == roots.len(),
				"members-subscriber: RingRoots entry count changed"
			);

			// 4. Collection states round-trip, with next_scan_index seeded to 0.
			ensure!(
				RingCollectionStates::<T>::iter().count() == states.len(),
				"members-subscriber: RingCollectionStates entry count changed"
			);
			for (identifier, ring_count, next_ring_index, missing, deleted) in &states {
				let s = RingCollectionStates::<T>::get(identifier);
				ensure!(
					s.ring_count == *ring_count,
					"members-subscriber: ring_count changed"
				);
				ensure!(
					s.next_ring_index == *next_ring_index,
					"members-subscriber: next_ring_index changed"
				);
				ensure!(
					s.next_scan_index == 0,
					"members-subscriber: next_scan_index was not seeded to 0"
				);
				ensure!(
					s.missing_indices.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>() ==
						*missing,
					"members-subscriber: missing_indices changed"
				);
				ensure!(
					s.deleted_indices.iter().copied().collect::<Vec<_>>() == *deleted,
					"members-subscriber: deleted_indices changed"
				);
			}

			// 5. The three untouched items really were untouched. This is what catches an
			//    accidental kill_prefix over the whole pallet.
			ensure!(
				Subscription::<T>::get() == subscription,
				"members-subscriber: Subscription changed"
			);
			ensure!(
				ProcessingState::<T>::get() == processing,
				"members-subscriber: ProcessingState changed"
			);
			ensure!(
				RingCollectionExponents::<T>::iter().collect::<Vec<_>>() == exponents,
				"members-subscriber: RingCollectionExponents changed"
			);

			log::info!(
				target: LOG_TARGET,
				"post_upgrade: {} RingRoots entries and {} RingCollectionStates verified",
				roots.len(), states.len(),
			);
			Ok(())
		}
	}
}

#[cfg(test)]
mod tests {
	use super::v1::*;
	use crate::{
		migration::MigrateV0ToV1,
		mock::{new_test_ext, Test},
		types::{Identifier, RingCollectionState, RingIndex},
		CurrentGeneration, RingCollectionStates, RingRoots,
	};
	use codec::{Decode, Encode};
	use frame_support::{
		pallet_prelude::*, traits::OnRuntimeUpgrade, BoundedVec,
	};

	// ---------------------------------------------------------------------------------------
	// Real bytes, read read-only from asset-hub-paseo at spec 2004002.
	//
	// These are the actual stored ring commitments the migration has to reshape. Synthesised
	// data would prove nothing here: the whole point of the `MembersCommitment` finding is that
	// the on-chain layout is not what the design document assumed.
	// ---------------------------------------------------------------------------------------

	/// The 480-byte KZG verifier key that prefixes every live 768-byte ring commitment.
	/// Identical across all 33 live values on both chains.
	const LIVE_KZG_PREFIX_HEX: &str = concat!(
		"17f1d3a73197d7942695638c4fa9ac0fc3688c4f9774b905a14e3a3f171bac58",
		"6c55e83ff97a1aeffb3af00adb22c6bb08b3f481e3aaa0f1a09e30ed741d8ae4",
		"fcf5e095d5d00af600db18cb2c04b3edd03cc744a2888ae40caa232946c5e7e1",
		"13e02b6052719f607dacd3a088274f65596bd0d09920b61ab5da61bbdc7f5049",
		"334cf11213945d57e5ac7d055d042b7e024aa2b2f08f0a91260805272dc51051",
		"c6e47ad4fa403b02b4510b647ae3d1770bac0326a805bbefd48056c8c121bdb8",
		"0606c4a02ea734cc32acd2b02bc28b99cb3e287e85a763af267492ab572e99ab",
		"3f370d275cec1da1aaa9075ff05f79be0ce5d527727d6e118cc9cdc6da2e351a",
		"adfd9baa8cbdd3a76d429a695160d12c923ac9cc3baca289e193548608b82801",
		"116b2250fc4b3098ccadf9ec58526aecf307d21e233f65110554ee04460f72a6",
		"16f4d070941b0796de5defda29196e40013645b8518e5568e75d320da8b83a87",
		"e12406956b9c74491ee5ecf06dc297ba1186016209d78fd832c16ef62fc49eff",
		"0bc832fb878c2f66addd846aa85954b856c3bc70d87344f9006889e58eb0ec57",
		"34151f99b9a5b653e991247ad506d0fb12db3e2e8318b647537f00e6b702fc85",
		"26e8c9101a7b7cbc410fbf8f6021f2e45393c8696431c1a6fcc7f5a7d90ab999",
	);

	/// The 288-byte ring commitments that must survive, one per live `RingRoots` key.
	const LIVE_TAIL_0_HEX: &str = concat!(
		"0a737333dff3b0193e75b6a8a6f9302b9a143303afa54dfa2d2a06a0bfc0d572",
		"2d63952f3ba2810f8b122473394f83c91468f9462c03bbe4f44c0b5c4b61cb34",
		"c7eed7017ffb11f17f523e21601847f61a1c79f8cc23c322c0ffc4bb096e2ed0",
		"0640cc933cc5c5ad7a3885c962e218d8c60b9fa9c24decc0d7931abee0ae961b",
		"55a2d09dbc6804c2fa4ea7d314496c1a083d9c3e983cdb5f6a0e5917d6df0850",
		"ef5c4e16e9ddb2fb6e9d75c4f480955c94c837a083de5e08a9162b1680601b5c",
		"12e630ae2b14e758ab0960e372172203f4c9a41777dadd529971d7ab9d23ab29",
		"fe0e9c85ec450505dde7f5ac038274cf02d5c1577a78c98f04769305d149b19f",
		"83477d2cf1e1ee2fa41862301251395ef6ad7637a8d01fa8eaa5d99959363cfd",
	);
	const LIVE_TAIL_1_HEX: &str = concat!(
		"0d88b4f930efe4ee2f6ccb6b39a6015f6c3337de42b801c5f0fbc86565814a50",
		"b38686f8ff47cc8fbf373736844eb48f1687ed6e25f7cbad6446cab8e1ccb819",
		"3212f5e2b30736e4ccac8f4a5a6307b5e0c381fccb4dddaba11962b2ce7dab8c",
		"1796337bd10bca6bdddeea2dd96fc070ec4184ac4713b15b6ea831089d9f750b",
		"f794c8fd337e3ab9bcfbdacd4daf99810d2fc6005a55a5786058421cbad1de65",
		"3f2e431ae7523b4c2767847f4a7b77bc70859edc34ed758accd0e69398159dec",
		"12e630ae2b14e758ab0960e372172203f4c9a41777dadd529971d7ab9d23ab29",
		"fe0e9c85ec450505dde7f5ac038274cf02d5c1577a78c98f04769305d149b19f",
		"83477d2cf1e1ee2fa41862301251395ef6ad7637a8d01fa8eaa5d99959363cfd",
	);
	const LIVE_TAIL_2_HEX: &str = concat!(
		"0d82b7e1ddf78384be6402ca3fb65ff0b0377c0a269c423299b2085344f7e670",
		"f79c9a97df3821721f21ae8c72f7e87219df6c0591ce989f9b1519a351928727",
		"7975ec0787d9e0b970ceba685be4ef9d63c5fd933f2ab99f850614b2161d04b3",
		"13f2495ad29990351a03182099b1e813df83eacfb08971a19e5905bf0710a37e",
		"bcdf4145d2b14ca0d0d4d705a70fa9d9086444bd753eae0354b7d288905e54aa",
		"12c1adc007ec7619c73a869dd4e6ad24784abd77583b29a1831d77add7aa43e0",
		"12e630ae2b14e758ab0960e372172203f4c9a41777dadd529971d7ab9d23ab29",
		"fe0e9c85ec450505dde7f5ac038274cf02d5c1577a78c98f04769305d149b19f",
		"83477d2cf1e1ee2fa41862301251395ef6ad7637a8d01fa8eaa5d99959363cfd",
	);

	/// The two live `RingCollectionStates` values, in the pre-v0.3 4-field shape.
	/// `people-lite` = {ring_count 2, next_ring_index 2, {}, {}}; `people` = {1, 1, {}, {}}.
	const LIVE_STATE_PEOPLE_LITE_HEX: &str = "02000000020000000000";
	const LIVE_STATE_PEOPLE_HEX: &str = "01000000010000000000";

	fn unhex(s: &str) -> Vec<u8> {
		(0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
	}

	fn live_old_root(tail_hex: &str) -> [u8; OLD_MEMBERS_LEN] {
		let mut v = unhex(LIVE_KZG_PREFIX_HEX);
		v.extend_from_slice(&unhex(tail_hex));
		assert_eq!(v.len(), OLD_MEMBERS_LEN);
		v.try_into().unwrap()
	}

	// ---------------------------------------------------------------------------------------
	// 1. The byte transform, against production data.
	// ---------------------------------------------------------------------------------------

	#[test]
	fn live_roots_are_the_old_768_byte_layout() {
		for tail in [LIVE_TAIL_0_HEX, LIVE_TAIL_1_HEX, LIVE_TAIL_2_HEX] {
			let old = live_old_root(tail);
			assert_eq!(old.len(), OLD_MEMBERS_LEN);
			assert_eq!(unhex(tail).len(), NEW_MEMBERS_LEN);
		}
		assert_eq!(KZG_VERIFIER_KEY_LEN, 480);
	}

	#[test]
	fn strip_returns_the_suffix_not_the_prefix() {
		for tail_hex in [LIVE_TAIL_0_HEX, LIVE_TAIL_1_HEX, LIVE_TAIL_2_HEX] {
			let old = live_old_root(tail_hex);
			let got = strip_kzg_verifier_key(&old).expect("live root must be accepted");
			assert_eq!(got.len(), NEW_MEMBERS_LEN);
			// The surviving 288 bytes are the TAIL...
			assert_eq!(got, &unhex(tail_hex)[..]);
			// ...and are NOT the leading 288 bytes. This is the assertion that would have
			// caught a "just truncate it" implementation.
			assert_ne!(got, &old[..NEW_MEMBERS_LEN]);
		}
	}

	#[test]
	fn strip_rejects_an_already_migrated_value() {
		// 288 bytes in, i.e. a value that has already been through the migration.
		let already = unhex(LIVE_TAIL_0_HEX);
		assert!(strip_kzg_verifier_key(&already).is_none());
	}

	#[test]
	fn strip_rejects_a_foreign_prefix() {
		let mut old = live_old_root(LIVE_TAIL_0_HEX);
		old[0] ^= 0x01; // one bit of the g1 generator
		assert!(strip_kzg_verifier_key(&old).is_none());
	}

	#[test]
	fn strip_rejects_a_truncated_value() {
		let old = live_old_root(LIVE_TAIL_0_HEX);
		assert!(strip_kzg_verifier_key(&old[..OLD_MEMBERS_LEN - 1]).is_none());
		assert!(strip_kzg_verifier_key(&[]).is_none());
	}

	#[test]
	fn canonical_prefix_hash_matches_live_bytes() {
		use sp_io::hashing::blake2_256;
		assert_eq!(
			blake2_256(&unhex(LIVE_KZG_PREFIX_HEX)),
			CANONICAL_KZG_VERIFIER_KEY_HASH,
		);
	}

	// ---------------------------------------------------------------------------------------
	// 2. `RingCollectionState`: the silent-decode-failure half.
	// ---------------------------------------------------------------------------------------

	#[test]
	fn live_collection_state_fails_to_decode_as_the_new_type() {
		type New = RingCollectionState<ConstU32<255>, ConstU32<100>>;
		type Old = OldRingCollectionState<ConstU32<255>, ConstU32<100>>;
		for hex in [LIVE_STATE_PEOPLE_LITE_HEX, LIVE_STATE_PEOPLE_HEX] {
			let raw = unhex(hex);
			assert_eq!(raw.len(), 10, "live values are 10 bytes");
			// This is the bug: the live value does NOT decode as the v0.3.x type...
			assert!(New::decode(&mut &raw[..]).is_err());
			// ...but does decode as the pre-v0.3 one.
			assert!(Old::decode(&mut &raw[..]).is_ok());
		}
	}

	#[test]
	fn collection_state_translation_preserves_fields_and_seeds_scan_to_zero() {
		type Old = OldRingCollectionState<ConstU32<255>, ConstU32<100>>;
		let raw = unhex(LIVE_STATE_PEOPLE_LITE_HEX);
		let old = Old::decode(&mut &raw[..]).unwrap();
		assert_eq!(old.ring_count, 2);
		assert_eq!(old.next_ring_index, 2);
		assert!(old.missing_indices.is_empty());
		assert!(old.deleted_indices.is_empty());

		let new = RingCollectionState::<ConstU32<255>, ConstU32<100>> {
			ring_count: old.ring_count,
			next_ring_index: old.next_ring_index,
			next_scan_index: 0,
			missing_indices: old.missing_indices.clone(),
			deleted_indices: old.deleted_indices.clone(),
		};
		// 10 bytes in, 14 out: the 4-byte `next_scan_index` lands between `next_ring_index`
		// and `missing_indices`, not at the end.
		let encoded = new.encode();
		assert_eq!(encoded.len(), 14);
		assert_eq!(&encoded[..8], &raw[..8], "the two leading u32s are untouched");
		assert_eq!(&encoded[8..12], &[0, 0, 0, 0], "next_scan_index seeded to 0");
		assert_eq!(&encoded[12..], &raw[8..], "the two collections follow, unchanged");
	}

	// ---------------------------------------------------------------------------------------
	// 3. Key relocation. Independent of `Crypto`, so the mock runtime is fine here.
	// ---------------------------------------------------------------------------------------

	#[test]
	fn new_key_is_the_old_key_with_a_generation_segment_spliced_in() {
		new_test_ext().execute_with(|| {
			let id: Identifier = [7u8; 32];
			let ring: RingIndex = 3;
			let old_key = old::RingRoots::<Test>::hashed_key_for(id, ring);
			let new_key = RingRoots::<Test>::hashed_key_for((0u32, id, ring));

			// Same item prefix (32 bytes = twox128(pallet) ++ twox128("RingRoots")).
			assert_eq!(&new_key[..32], &old_key[..32]);
			// The new key is 12 bytes longer: twox64concat(u32) = 8 + 4.
			assert_eq!(new_key.len(), old_key.len() + 12);
			// ...and everything after the generation segment is the old key's suffix verbatim.
			assert_eq!(&new_key[44..], &old_key[32..]);
			// Matches the live layout: 100 -> 112 bytes.
			assert_eq!(old_key.len(), 100);
			assert_eq!(new_key.len(), 112);
		});
	}

	// ---------------------------------------------------------------------------------------
	// 4. End-to-end through `VersionedMigration`, in the mock runtime.
	//
	// `RingRoots` is left empty because the mock's `Crypto` is `verifiable::mock`, whose
	// `Members` is not the 768/288-byte Bandersnatch commitment — the typed reshape is covered
	// by the real-bytes tests above instead.
	// ---------------------------------------------------------------------------------------

	#[test]
	fn migration_translates_states_and_pins_generation_zero() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<Test>>();

			let a: Identifier = [1u8; 32];
			let b: Identifier = [2u8; 32];
			let mut missing = BoundedBTreeMap::new();
			missing.try_insert(5u32, 2u32).unwrap();
			let mut deleted = BoundedBTreeSet::new();
			deleted.try_insert(9u32).unwrap();

			old::RingCollectionStates::<Test>::insert(
				a,
				OldRingCollectionState {
					ring_count: 2,
					next_ring_index: 7,
					missing_indices: missing.clone(),
					deleted_indices: deleted.clone(),
				},
			);
			old::RingCollectionStates::<Test>::insert(
				b,
				OldRingCollectionState {
					ring_count: 1,
					next_ring_index: 1,
					missing_indices: Default::default(),
					deleted_indices: Default::default(),
				},
			);

			MigrateV0ToV1::<Test>::on_runtime_upgrade();

			assert_eq!(CurrentGeneration::<Test>::get(), 0);
			assert_eq!(StorageVersion::get::<crate::Pallet<Test>>(), 1);

			let sa = RingCollectionStates::<Test>::get(a);
			assert_eq!(sa.ring_count, 2);
			assert_eq!(sa.next_ring_index, 7);
			assert_eq!(sa.next_scan_index, 0);
			assert_eq!(sa.missing_indices, missing);
			assert_eq!(sa.deleted_indices, deleted);

			let sb = RingCollectionStates::<Test>::get(b);
			assert_eq!(sb.ring_count, 1);
			assert_eq!(sb.next_ring_index, 1);
			assert_eq!(sb.next_scan_index, 0);
		});
	}

	#[test]
	fn migration_is_a_no_op_when_already_at_version_one() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(1).put::<crate::Pallet<Test>>();
			let id: Identifier = [3u8; 32];
			// A value already in the NEW shape, with a non-zero scan cursor.
			RingCollectionStates::<Test>::insert(
				id,
				RingCollectionState {
					ring_count: 4,
					next_ring_index: 4,
					next_scan_index: 4,
					missing_indices: Default::default(),
					deleted_indices: Default::default(),
				},
			);

			MigrateV0ToV1::<Test>::on_runtime_upgrade();

			// Untouched: a second run must not reset the cursor it already advanced.
			assert_eq!(RingCollectionStates::<Test>::get(id).next_scan_index, 4);
		});
	}

	#[test]
	fn migration_writes_nothing_when_a_root_cannot_be_reshaped() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<Test>>();
			let id: Identifier = [4u8; 32];

			// A root that is 768 bytes but does NOT carry the canonical KZG prefix.
			let mut bad = live_old_root(LIVE_TAIL_0_HEX);
			bad[0] ^= 0xff;
			let records: BoundedVec<
				OldRingCommitmentRecord,
				<Test as crate::Config>::MaxRecentRootsPerRing,
			> =
				vec![OldRingCommitmentRecord {
					root: bad,
					revision: 1,
					source_time: 42,
					source_sequence: 7,
				}]
				.try_into()
				.unwrap();
			old::RingRoots::<Test>::insert(id, 0u32, records);
			old::RingCollectionStates::<Test>::insert(
				id,
				OldRingCollectionState {
					ring_count: 1,
					next_ring_index: 1,
					missing_indices: Default::default(),
					deleted_indices: Default::default(),
				},
			);

			MigrateV0ToV1::<Test>::on_runtime_upgrade();

			// All-or-nothing: the abort happens in phase 1, before any write.
			assert!(old::RingRoots::<Test>::contains_key(id, 0u32));
			assert!(RingRoots::<Test>::get((0u32, id, 0u32)).is_none());
			assert!(!CurrentGeneration::<Test>::exists());
		});
	}
}
