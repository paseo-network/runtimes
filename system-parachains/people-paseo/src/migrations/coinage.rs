// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! PASEO-LOCAL storage migrations for `indiv-pallet-coinage`.
//!
//! # Why these exist and upstream ships none
//!
//! individuality v0.3.1 rewrites `coinage` into a multi-instance registry. Upstream needs no
//! migration for it because its `next-*` runtimes are launched from genesis presets and never
//! carry pre-v0.3 state — verified: `next-people-paseo` is at spec `3000000` and its
//! `Coinage::UnderlyingAssetId` reads `None`. **Paseo is the only chain upgrading into v0.3.x
//! while holding live coinage state**, so the migration set is Paseo's alone.
//!
//! # These live in the runtime, not in the pallet
//!
//! The design this follows proposed adding a `STORAGE_VERSION` to the vendored `coinage` crate so
//! the units could be wrapped in `VersionedMigration`. That is avoided here: every type these
//! touch is `pub` at the coinage crate root, so the migrations sit in the runtime and the vendored
//! pallet stays **byte-identical to the v0.3.1 tag**. Guarding is by other means —
//! [`SeedCoinageInstanceZero`] is idempotent on its own writes, and the multi-block units are
//! tracked by `pallet_migrations` against their `id()`.
//!
//! # 🔴 Failure behaviour on this chain
//!
//! People binds `FailedMigrationHandler = FreezeChainOnFailedMigration` (`lib.rs`), where Asset Hub
//! binds `ForceUnstuckOnFailedMigration`. **A panic in any of these freezes the People chain.**
//! Everything here is written to be panic-free: saturating arithmetic, no `expect`, no unbounded
//! allocation, and no assumption that a decode succeeds.
//!
//! # Live state these were designed against
//!
//! Read read-only at People block **6,457,055**. Counts are recorded for orientation only —
//! **nothing here hardcodes them**, and that matters: `RecyclersCoinToRecycler` was 5,711 when the
//! design was written and is 5,735 now.
//!
//! | item | keys |
//! | --- | --- |
//! | `CoinsByOwner` | 816 |
//! | `LockedCoins` | 17 |
//! | `RecyclersCoinToRecycler` | 5,735 |
//! | `RecyclersUnloaded` | 385 (the anti-replay set) |
//! | `RecyclerCollectionCreated` | 15 (denominations 0..=14) |
//! | `RecyclersLastRemovedRingIndex`, `RecyclersDusting`, `PaidUnloadTokenMembers` | 0 |
//! | `Instances` | 0 — nothing seeded yet |

use crate::{AccountId, Runtime, Weight};
use frame_support::{
	migrations::{MigrationId, SteppedMigration, SteppedMigrationError},
	pallet_prelude::*,
	traits::OnRuntimeUpgrade,
	weights::WeightMeter,
};
use indiv_pallet_coinage::{
	AliasState, AssetToInstance, Coin, Config as CoinageConfig, FungiblesAssetIdOf, InstanceMode,
	InstanceRecord, Instances, LockInfo, MemberOf, NextInstanceId, RecyclerAliasStates,
	RecyclerCollectionCreated, RecyclersCoinToRecycler,
};
use indiv_support::traits::{Alias, RingIndex};

/// The asset amount of a denomination-zero coin for the pre-existing instance.
///
/// Inherited verbatim from the baseline runtime's `Config::UnderlyingAssetUnit`
/// (`ConstUint<{ 10u128.pow(4) }>` — "$0.01, the unit is 10^6"), which individuality v0.3.1 moved
/// out of `Config` and into [`InstanceRecord::asset_unit`]. Seeding anything else would silently
/// reprice every coin already on the chain.
pub const LEGACY_ASSET_UNIT: u128 = 10u128.pow(4);

/// The instance id the pre-existing coin population is adopted into.
pub const LEGACY_INSTANCE_ID: u32 = 0;

/// Storage as the **currently deployed** runtime declares it. Only items whose declaration changed
/// or was removed need an alias; anything unchanged is read through the live types.
pub mod old {
	use super::*;
	use indiv_pallet_coinage::Pallet;

	/// Removed in v0.3.1. It is the seed input for [`InstanceRecord::asset_id`], so it must be
	/// read before it is killed.
	#[frame_support::storage_alias]
	pub type UnderlyingAssetId<T: CoinageConfig> =
		StorageValue<Pallet<T>, FungiblesAssetIdOf<T>, OptionQuery>;

	/// Removed in v0.3.1; a stale marker with no successor.
	#[frame_support::storage_alias]
	pub type InitializePalletAccount<T: CoinageConfig> = StorageValue<Pallet<T>, (), OptionQuery>;

	/// Baseline shape: a single map keyed by denomination alone. v0.3.1 makes it a double map
	/// keyed `(InstanceId, Denomination)` **at the same storage prefix**, so the old keys land
	/// inside the new map's keyspace and must be removed, not merely superseded.
	#[frame_support::storage_alias]
	pub type RecyclerCollectionCreated<T: CoinageConfig> =
		StorageMap<Pallet<T>, Twox64Concat, i8, (), OptionQuery>;

	/// Baseline `Coin`: `{ value: i8, age: u16 }`, 3 bytes. v0.3.1 prepends `instance_id: u32`,
	/// making it 7. The old bytes decode to nothing under the new type, and `OptionQuery` turns
	/// that into "no coin" — which is why 816 balances would read as zero unmigrated.
	#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
	pub struct OldCoin {
		pub value: i8,
		pub age: u16,
	}

	#[frame_support::storage_alias]
	pub type CoinsByOwner<T: CoinageConfig> = StorageMap<
		Pallet<T>,
		Twox64Concat,
		<T as frame_system::Config>::AccountId,
		OldCoin,
		OptionQuery,
	>;

	/// Baseline `LockedCoin` and v0.3.1 `LockInfo` are field-identical
	/// (`{ reason: LockReason, until: u64 }`, and `LockReason` is unchanged), so the value bytes
	/// carry over verbatim and this alias reads them straight into the live type. Only the hasher
	/// moves.
	#[frame_support::storage_alias]
	pub type LockedCoins<T: CoinageConfig> = StorageMap<
		Pallet<T>,
		Twox64Concat,
		<T as frame_system::Config>::AccountId,
		LockInfo,
		OptionQuery,
	>;

	/// Baseline value is a bare `CoinValue` (1 byte); v0.3.1 widens it to
	/// `(InstanceId, Denomination)` (5 bytes).
	#[frame_support::storage_alias]
	pub type RecyclersCoinToRecycler<T: CoinageConfig> =
		StorageMap<Pallet<T>, Twox64Concat, MemberOf<T>, i8, OptionQuery>;

	/// 🔴 The anti-replay set. Removed in v0.3.1 with no successor; its role is taken by
	/// `RecyclerAliasStates`, which starts empty. Left unmigrated, a previously-consumed alias
	/// reads as available and an old ring-VRF proof can unload the same position twice.
	#[frame_support::storage_alias]
	pub type RecyclersUnloaded<T: CoinageConfig> = StorageNMap<
		Pallet<T>,
		(NMapKey<Twox64Concat, i8>, NMapKey<Twox64Concat, RingIndex>, NMapKey<Twox64Concat, Alias>),
		(),
		OptionQuery,
	>;
}

/// Unit A — adopt the pre-existing coin population into instance 0.
///
/// Single-block: it writes a bounded, live-state-independent number of keys (one instance record,
/// one counter, one asset index entry, and one per existing denomination — 15 today).
///
/// **Must run before the multi-block units.** Both of those phrase their invariants against
/// `Instances[0]` existing, and every coin operation reads it, so a partially-migrated chain with
/// no instance record would reject every call.
///
/// Idempotent: re-running is a no-op once `Instances[0]` exists, so no pallet storage version is
/// needed to guard it.
pub struct SeedCoinageInstanceZero;

impl OnRuntimeUpgrade for SeedCoinageInstanceZero {
	fn on_runtime_upgrade() -> Weight {
		let mut reads = 1u64;
		let mut writes = 0u64;

		if Instances::<Runtime>::contains_key(LEGACY_INSTANCE_ID) {
			log::info!(
				target: "runtime::coinage-migration",
				"instance {LEGACY_INSTANCE_ID} already seeded; skipping",
			);
			return <Runtime as frame_system::Config>::DbWeight::get().reads(reads);
		}

		reads = reads.saturating_add(1);
		let Some(asset_id) = old::UnderlyingAssetId::<Runtime>::get() else {
			// Nothing to adopt: a chain that never set the underlying asset has no coins either.
			// Not an error, and deliberately not a panic — People freezes on a failed migration.
			log::warn!(
				target: "runtime::coinage-migration",
				"no UnderlyingAssetId set; nothing to adopt into an instance",
			);
			return <Runtime as frame_system::Config>::DbWeight::get().reads(reads);
		};

		Instances::<Runtime>::insert(
			LEGACY_INSTANCE_ID,
			InstanceRecord::<Runtime> {
				asset_id: asset_id.clone(),
				asset_unit: LEGACY_ASSET_UNIT,
				// `Sufficient` is the shape that reproduces baseline behaviour: no pot, loads take
				// no deposit, and no `InstanceCreationDeposit` ticket to release. `Sponsored`
				// would demand a funded pot account that does not exist on this chain.
				mode: InstanceMode::Sufficient,
				current_load_deposit: None,
				old_load_deposit: None,
				creator: None,
			},
		);
		NextInstanceId::<Runtime>::put(LEGACY_INSTANCE_ID.saturating_add(1));
		AssetToInstance::<Runtime>::insert(&asset_id, LEGACY_INSTANCE_ID, ());
		writes = writes.saturating_add(3);

		// Re-key the per-denomination collection markers into the double map.
		//
		// 🔴 COLLECT BEFORE WRITING. The baseline single map and the v0.3.1 double map share this
		// storage prefix, so inserting while iterating feeds the alias's `Twox64Concat` reverse
		// decode its own 21-byte suffixes, which it happily reads back as more `i8`
		// denominations. Doing it in one pass re-keyed 25 markers for 15 real denominations.
		//
		// The old keys are dropped by the multi-block unit, which owns every removal here.
		let legacy: alloc::vec::Vec<i8> =
			old::RecyclerCollectionCreated::<Runtime>::iter_keys().collect();
		let denominations = legacy.len() as u32;
		for denomination in legacy {
			RecyclerCollectionCreated::<Runtime>::insert(LEGACY_INSTANCE_ID, denomination, ());
		}
		reads = reads.saturating_add(denominations.into());
		writes = writes.saturating_add(denominations.into());

		// Both are removed in v0.3.1 and have no successor.
		old::UnderlyingAssetId::<Runtime>::kill();
		old::InitializePalletAccount::<Runtime>::kill();
		writes = writes.saturating_add(2);

		log::info!(
			target: "runtime::coinage-migration",
			"seeded instance {LEGACY_INSTANCE_ID} (asset_unit {LEGACY_ASSET_UNIT}) and re-keyed \
			 {denominations} recycler collection marker(s)",
		);

		<Runtime as frame_system::Config>::DbWeight::get().reads_writes(reads, writes)
	}

	#[cfg(feature = "try-runtime")]
	fn pre_upgrade() -> Result<alloc::vec::Vec<u8>, sp_runtime::TryRuntimeError> {
		use codec::Encode;

		ensure!(
			Instances::<Runtime>::iter().next().is_none(),
			"coinage: instances already exist; this runtime has been migrated already",
		);
		let asset_id = old::UnderlyingAssetId::<Runtime>::get();
		ensure!(asset_id.is_some(), "coinage: no UnderlyingAssetId to seed instance 0 from",);

		let denominations: alloc::vec::Vec<i8> =
			old::RecyclerCollectionCreated::<Runtime>::iter_keys().collect();
		log::info!(
			target: "runtime::coinage-migration",
			"pre_upgrade: seeding from asset {asset_id:?}; {} denomination(s) to re-key",
			denominations.len(),
		);
		Ok((asset_id, denominations).encode())
	}

	#[cfg(feature = "try-runtime")]
	fn post_upgrade(state: alloc::vec::Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
		use codec::Decode;

		let (expected_asset, expected_denominations): (
			Option<FungiblesAssetIdOf<Runtime>>,
			alloc::vec::Vec<i8>,
		) = Decode::decode(&mut &state[..])
			.map_err(|_| "coinage: could not decode the pre_upgrade capture")?;
		let expected_asset =
			expected_asset.ok_or("coinage: pre_upgrade captured no underlying asset")?;

		let record = Instances::<Runtime>::get(LEGACY_INSTANCE_ID)
			.ok_or("coinage: instance 0 was not seeded")?;
		ensure!(record.asset_id == expected_asset, "coinage: instance 0 has the wrong asset");
		ensure!(
			record.asset_unit == LEGACY_ASSET_UNIT,
			"coinage: instance 0 would reprice every existing coin",
		);
		ensure!(record.mode == InstanceMode::Sufficient, "coinage: instance 0 is not Sufficient");
		ensure!(
			record.creator.is_none() &&
				record.current_load_deposit.is_none() &&
				record.old_load_deposit.is_none(),
			"coinage: instance 0 must carry no deposit ledger",
		);
		ensure!(
			NextInstanceId::<Runtime>::get() == LEGACY_INSTANCE_ID.saturating_add(1),
			"coinage: NextInstanceId was not advanced past the seeded instance",
		);
		ensure!(
			AssetToInstance::<Runtime>::contains_key(&expected_asset, LEGACY_INSTANCE_ID),
			"coinage: the asset is not indexed to instance 0",
		);

		// Every denomination that existed must be reachable under the new key shape. Compared
		// against the pre_upgrade capture, never against a documented count: the live figures
		// have already drifted once between the design and now.
		for denomination in &expected_denominations {
			ensure!(
				RecyclerCollectionCreated::<Runtime>::contains_key(
					LEGACY_INSTANCE_ID,
					denomination
				),
				"coinage: a recycler collection marker was not re-keyed",
			);
		}
		// Exactly those, and no others. Presence alone missed a real bug: writing into a prefix
		// while iterating it produced 25 markers for 15 denominations, and every expected one was
		// still present, so a presence-only check passed.
		let rekeyed =
			RecyclerCollectionCreated::<Runtime>::iter_key_prefix(LEGACY_INSTANCE_ID).count();
		ensure!(
			rekeyed == expected_denominations.len(),
			"coinage: the re-keyed marker count does not match the denominations captured",
		);

		ensure!(
			old::UnderlyingAssetId::<Runtime>::get().is_none(),
			"coinage: the removed UnderlyingAssetId key was left behind",
		);
		ensure!(
			old::InitializePalletAccount::<Runtime>::get().is_none(),
			"coinage: the removed InitializePalletAccount key was left behind",
		);

		log::info!(
			target: "runtime::coinage-migration",
			"post_upgrade: instance 0 seeded and {} denomination(s) verified",
			expected_denominations.len(),
		);
		Ok(())
	}
}

/// Declared `proof_size` charged per migrated entry, in bytes.
///
/// 🔴 THIS IS THE NUMBER THAT PROTECTS BLOCK PRODUCTION, and it must be an OVER-estimate.
///
/// `RuntimeDbWeight::reads_writes()` declares `proof_size = 0`. On a parachain PoV is the binding
/// constraint, not ref-time, so a step weight built from `DbWeight` alone tells the `WeightMeter`
/// that entries are free in PoV. The meter would then let a step process thousands of them while
/// the real proof grows past `MAX_POV_SIZE`, the relay rejects the block, and **block production
/// stalls**. Under-declaring here is the single most dangerous thing this file could do.
///
/// A trie proof node set for one key is roughly 0.5–2 KB. 4 KB is deliberately about double the
/// top of that range. Against the ~4 MB the MBM budget allows per block
/// (`MbmServiceWeight` = 80% of `max_block`), that admits ~1,000 entries per block:
///
/// | unit | entries | blocks at 4 KB |
/// | --- | --- | --- |
/// | B | ~6,900 | ~7 |
/// | C | ~12,000 | ~12 |
///
/// Being wrong in the generous direction costs extra blocks. Being wrong in the other direction
/// costs the chain. Do not lower this without measuring a real proof on a real snapshot.
const DECLARED_PROOF_SIZE_PER_ENTRY: u64 = 4 * 1024;

/// The weight one migrated entry is charged: a read, two writes, and a deliberately generous
/// `proof_size` allowance the `DbWeight` figures do not include.
fn per_entry_weight() -> Weight {
	<Runtime as frame_system::Config>::DbWeight::get()
		.reads_writes(1, 2)
		.saturating_add(Weight::from_parts(0, DECLARED_PROOF_SIZE_PER_ENTRY))
}

/// Upper bound on `step()` invocations before `pallet_migrations` gives up.
///
/// Expected is ~20 for B and C together. 10,000 is ~500x that: it can only be reached by a genuine
/// bug such as a cursor that fails to advance, and it bounds a runaway rather than letting one spin
/// forever.
///
/// ⚠️ On this chain exceeding it is not a soft failure: People binds
/// `FreezeChainOnFailedMigration`. The bound is set far above any legitimate run for that reason.
const MAX_MIGRATION_STEPS: u32 = 10_000;

/// The longest raw storage key any phase resumes from. The widest is the `RecyclersUnloaded`
/// n-map: 32-byte item prefix + three `Twox64Concat` segments, the largest being an `Alias`.
const MAX_CURSOR_KEY: u32 = 160;

/// Which map [`MigrateCoinageToInstances`] is working through, and where it got to.
///
/// Phases run in the listed order and each carries the raw key it last handled, so a step resumes
/// exactly where the previous one stopped.
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub enum Phase {
	/// 816 balances: rehash `Twox64Concat` -> `Blake2_128Concat` and widen `Coin`.
	CoinsByOwner(Option<BoundedVec<u8, ConstU32<MAX_CURSOR_KEY>>>),
	/// 17 entries: rehash only; the value bytes are already correct.
	LockedCoins(Option<BoundedVec<u8, ConstU32<MAX_CURSOR_KEY>>>),
	/// 5,735 entries: rehash and widen the value to `(InstanceId, Denomination)`.
	RecyclersCoinToRecycler(Option<BoundedVec<u8, ConstU32<MAX_CURSOR_KEY>>>),
	/// 🔴 385 entries: rebuild the anti-replay set into `RecyclerAliasStates`.
	RecyclersUnloaded(Option<BoundedVec<u8, ConstU32<MAX_CURSOR_KEY>>>),
	/// Drop the 15 baseline `RecyclerCollectionCreated` keys, which sit inside the new double
	/// map's prefix. Unit A already wrote their replacements.
	OldCollectionMarkers(Option<BoundedVec<u8, ConstU32<MAX_CURSOR_KEY>>>),
}

/// Unit B — move every per-owner and per-recycler key into its instance-aware shape.
///
/// Multi-block by necessity: ~6,900 entries, and the binding constraint is PoV, not ref-time —
/// each read contributes trie proof nodes against a ~5 MB budget.
///
/// 🔴 Every step must be panic-free. People binds `FreezeChainOnFailedMigration`, so a panic here
/// freezes the chain. Entries whose old value does not decode are counted and skipped rather than
/// asserted on; `post_upgrade` is where a non-zero count becomes an error, by which point the
/// migration has already finished and the chain is live.
pub struct MigrateCoinageToInstances;

impl SteppedMigration for MigrateCoinageToInstances {
	type Cursor = Phase;
	type Identifier = MigrationId<32>;

	fn id() -> Self::Identifier {
		MigrationId {
			pallet_id: *b"paseo-coinage-to-instances------",
			version_from: 0,
			version_to: 1,
		}
	}

	fn max_steps() -> Option<u32> {
		Some(MAX_MIGRATION_STEPS)
	}

	fn step(
		cursor: Option<Self::Cursor>,
		meter: &mut WeightMeter,
	) -> Result<Option<Self::Cursor>, SteppedMigrationError> {
		let required = per_entry_weight();
		if meter.remaining().any_lt(required) {
			return Err(SteppedMigrationError::InsufficientWeight { required });
		}

		let mut phase = cursor.unwrap_or(Phase::CoinsByOwner(None));

		loop {
			if meter.try_consume(required).is_err() {
				return Ok(Some(phase));
			}

			// `Done` and `Halt` both leave the current prefix, but only `Halt` means something
			// went wrong; the leftover keys make it detectable in `post_upgrade`.
			phase = match phase {
				Phase::CoinsByOwner(last) => match step_coins_by_owner(last) {
					StepOutcome::Next(k) => Phase::CoinsByOwner(Some(k)),
					StepOutcome::Done | StepOutcome::Halt => Phase::LockedCoins(None),
				},
				Phase::LockedCoins(last) => match step_locked_coins(last) {
					StepOutcome::Next(k) => Phase::LockedCoins(Some(k)),
					StepOutcome::Done | StepOutcome::Halt => Phase::RecyclersCoinToRecycler(None),
				},
				Phase::RecyclersCoinToRecycler(last) => match step_coin_to_recycler(last) {
					StepOutcome::Next(k) => Phase::RecyclersCoinToRecycler(Some(k)),
					StepOutcome::Done | StepOutcome::Halt => Phase::RecyclersUnloaded(None),
				},
				Phase::RecyclersUnloaded(last) => match step_recyclers_unloaded(last) {
					StepOutcome::Next(k) => Phase::RecyclersUnloaded(Some(k)),
					StepOutcome::Done | StepOutcome::Halt => Phase::OldCollectionMarkers(None),
				},
				Phase::OldCollectionMarkers(last) => match step_old_markers(last) {
					StepOutcome::Next(k) => Phase::OldCollectionMarkers(Some(k)),
					StepOutcome::Done | StepOutcome::Halt => return Ok(None),
				},
			};
		}
	}
}

/// What one entry-level step produced.
///
/// The point of this enum is that [`Self::Done`] and [`Self::Halt`] are different. Both stop the
/// current prefix, but `Halt` means *we could not continue*, which leaves old keys behind — and
/// `post_upgrade`'s emptiness assertions are guaranteed to catch that. Collapsing the two into a
/// bare `None`, as an earlier revision did, is what would turn a cursor failure into silent data
/// loss.
enum StepOutcome {
	/// Migrated one entry; resume after this raw key.
	Next(BoundedVec<u8, ConstU32<MAX_CURSOR_KEY>>),
	/// No entries left under this prefix.
	Done,
	/// Could not continue. Logged, and detectable afterwards by the leftover keys.
	Halt,
}

/// Bound a raw key for the cursor.
///
/// 🔴 Every key these phases produce has a fixed shape, so this cannot fail in practice — but if
/// it did, returning a value the caller reads as "prefix finished" would **silently skip every
/// remaining entry under it**. That is data loss, and invisible until a balance is missing. So a
/// failure is logged loudly and surfaced as an error the caller must handle by stopping.
fn bound_key(raw: alloc::vec::Vec<u8>) -> Result<BoundedVec<u8, ConstU32<MAX_CURSOR_KEY>>, ()> {
	BoundedVec::try_from(raw).map_err(|k: alloc::vec::Vec<u8>| {
		log::error!(
			target: "runtime::coinage-migration",
			"cursor key of {} bytes exceeds MAX_CURSOR_KEY ({MAX_CURSOR_KEY}); stopping this phase",
			k.len(),
		);
	})
}

fn resume<'a>(last: &'a Option<BoundedVec<u8, ConstU32<MAX_CURSOR_KEY>>>) -> Option<&'a [u8]> {
	last.as_ref().map(|k| k.as_slice())
}

// ---------------------------------------------------------------------------------------------
// 🔴 Why every phase below walks RAW keys instead of the typed legacy alias
//
// For `CoinsByOwner`, `LockedCoins`, `RecyclersCoinToRecycler` and `RecyclerCollectionCreated`,
// v0.3.1 changed only the HASHER (or the map arity) — pallet and item names are unchanged, so
// **source and destination share one storage prefix**. A freshly written destination key lands
// inside the range a legacy-typed iterator walks.
//
// And it is NOT skipped. Reversing `Twox64Concat` over a 48-byte `Blake2_128Concat` suffix yields
// 8 hash bytes and 40 remaining; `AccountId32` decodes from the first 32 and SCALE ignores the 8
// trailing, so it succeeds with a garbage account. The migration then reads its own output back as
// input, writes another key from the garbage account, and never terminates. Measured: unit B ran
// 500 simulated blocks against a real snapshot without finishing.
//
// Legacy and destination keys differ in LENGTH, and that is what separates them:
//
// | item | legacy suffix | destination suffix |
// | --- | --- | --- |
// | `CoinsByOwner`, `LockedCoins`, `RecyclersCoinToRecycler` | 8 + 32 = 40 | 16 + 32 = 48 |
// | `RecyclerCollectionCreated` | 8 + 1 = 9 | (8+4) + (8+1) = 21 |
//
// `RecyclersUnloaded` -> `RecyclerAliasStates` is the one safe case: different item names, so
// different prefixes, so no overlap.
// ---------------------------------------------------------------------------------------------

/// `twox128("Coinage") ++ twox128(item)`.
fn coinage_item_prefix(item: &str) -> alloc::vec::Vec<u8> {
	let mut key = alloc::vec::Vec::with_capacity(32);
	key.extend_from_slice(&sp_io::hashing::twox_128(b"Coinage"));
	key.extend_from_slice(&sp_io::hashing::twox_128(item.as_bytes()));
	key
}

/// Legacy suffix length for a `Twox64Concat` map over a fixed-size key: 8 hash bytes + the key.
const fn twox64_suffix(key_len: usize) -> usize {
	8 + key_len
}

/// Next raw key under `item_prefix` after `from` whose suffix length is exactly
/// `legacy_suffix_len`, skipping everything else — which is this migration's own output.
///
/// Bounded: stops at the end of the prefix or after `SCAN_LIMIT` non-matching keys, so it can
/// never spin. Hitting the limit ends the phase, and `post_upgrade`'s emptiness assertion catches
/// whatever was left.
fn next_legacy_key(
	item_prefix: &[u8],
	from: &[u8],
	legacy_suffix_len: usize,
) -> Option<alloc::vec::Vec<u8>> {
	const SCAN_LIMIT: u32 = 10_000;
	let mut cursor = from.to_vec();
	for _ in 0..SCAN_LIMIT {
		let next = sp_io::storage::next_key(&cursor)?;
		if !next.starts_with(item_prefix) {
			return None;
		}
		if next.len() == item_prefix.len().saturating_add(legacy_suffix_len) {
			return Some(next);
		}
		cursor = next;
	}
	log::error!(
		target: "runtime::coinage-migration",
		"scan limit reached looking for a legacy key; ending this phase",
	);
	None
}

/// Decode a map key out of a raw storage key whose suffix is `hash ++ encoded_key`.
fn key_from_raw<K: Decode>(raw: &[u8], item_prefix_len: usize, hash_len: usize) -> Option<K> {
	let start = item_prefix_len.saturating_add(hash_len);
	let mut encoded = raw.get(start..)?;
	K::decode(&mut encoded).ok()
}

/// One `CoinsByOwner` entry: rehash the key and widen `Coin` with `instance_id = 0`.
fn step_coins_by_owner(last: Option<BoundedVec<u8, ConstU32<MAX_CURSOR_KEY>>>) -> StepOutcome {
	let prefix = coinage_item_prefix("CoinsByOwner");
	let from = resume(&last).map(|k| k.to_vec()).unwrap_or_else(|| prefix.clone());
	let Some(raw) = next_legacy_key(&prefix, &from, twox64_suffix(32)) else {
		return StepOutcome::Done;
	};
	let Some(account) = key_from_raw::<AccountId>(&raw, prefix.len(), 8) else {
		log::error!(target: "runtime::coinage-migration", "undecodable CoinsByOwner key; halting");
		return StepOutcome::Halt;
	};
	let Some(old_coin) = old::CoinsByOwner::<Runtime>::get(&account) else {
		// A key that is present but whose value will not decode. Do NOT clear it: leaving it makes
		// `post_upgrade`'s emptiness assertion fail loudly instead of losing a balance quietly.
		log::error!(
			target: "runtime::coinage-migration",
			"CoinsByOwner value did not decode; leaving the key for post_upgrade to catch",
		);
		return StepOutcome::Halt;
	};

	// `instance_id` is prepended, so the widened value strictly extends the old one.
	indiv_pallet_coinage::CoinsByOwner::<Runtime>::insert(
		&account,
		Coin { instance_id: LEGACY_INSTANCE_ID, value: old_coin.value, age: old_coin.age },
	);
	sp_io::storage::clear(&raw);
	match bound_key(raw) {
		Ok(k) => StepOutcome::Next(k),
		Err(()) => StepOutcome::Halt,
	}
}

/// One `LockedCoins` entry: the value bytes are already correct, only the hasher moves.
fn step_locked_coins(last: Option<BoundedVec<u8, ConstU32<MAX_CURSOR_KEY>>>) -> StepOutcome {
	let prefix = coinage_item_prefix("LockedCoins");
	let from = resume(&last).map(|k| k.to_vec()).unwrap_or_else(|| prefix.clone());
	let Some(raw) = next_legacy_key(&prefix, &from, twox64_suffix(32)) else {
		return StepOutcome::Done;
	};
	let Some(account) = key_from_raw::<AccountId>(&raw, prefix.len(), 8) else {
		log::error!(target: "runtime::coinage-migration", "undecodable LockedCoins key; halting");
		return StepOutcome::Halt;
	};

	// `LockedCoin` and `LockInfo` are field-identical, so the value bytes are copied verbatim
	// rather than decoded and re-encoded.
	if let Some(value) = sp_io::storage::get(&raw) {
		let dest = indiv_pallet_coinage::LockedCoins::<Runtime>::hashed_key_for(&account);
		sp_io::storage::set(&dest, &value);
	}
	sp_io::storage::clear(&raw);
	match bound_key(raw) {
		Ok(k) => StepOutcome::Next(k),
		Err(()) => StepOutcome::Halt,
	}
}

/// One `RecyclersCoinToRecycler` entry: rehash, and widen the bare denomination into
/// `(instance_id, denomination)`.
fn step_coin_to_recycler(last: Option<BoundedVec<u8, ConstU32<MAX_CURSOR_KEY>>>) -> StepOutcome {
	let prefix = coinage_item_prefix("RecyclersCoinToRecycler");
	let from = resume(&last).map(|k| k.to_vec()).unwrap_or_else(|| prefix.clone());
	let Some(raw) = next_legacy_key(&prefix, &from, twox64_suffix(32)) else {
		return StepOutcome::Done;
	};
	let Some(member) = key_from_raw::<MemberOf<Runtime>>(&raw, prefix.len(), 8) else {
		log::error!(
			target: "runtime::coinage-migration",
			"undecodable RecyclersCoinToRecycler key; halting",
		);
		return StepOutcome::Halt;
	};
	let Some(denomination) = old::RecyclersCoinToRecycler::<Runtime>::get(&member) else {
		log::error!(
			target: "runtime::coinage-migration",
			"RecyclersCoinToRecycler value did not decode; leaving the key",
		);
		return StepOutcome::Halt;
	};

	RecyclersCoinToRecycler::<Runtime>::insert(&member, (LEGACY_INSTANCE_ID, denomination));
	sp_io::storage::clear(&raw);
	match bound_key(raw) {
		Ok(k) => StepOutcome::Next(k),
		Err(()) => StepOutcome::Halt,
	}
}

/// 🔴 One anti-replay entry, rebuilt into `RecyclerAliasStates`.
///
/// The value must be `AliasState::Unloaded`, which is **variant index 1** — encoding `0x01`.
/// Writing `0x00` would be a truncated `Locked`, which fails to decode, which `OptionQuery` turns
/// into `None`, which the replay guard reads as *available*. A mis-encoded entry is
/// indistinguishable from a missing one and reintroduces exactly the double-unload this exists to
/// prevent.
///
/// Deliberately does **not** write `RecyclersUnloadedCount`. individuality v0.3.1 added that
/// counter and documents the migrated case: "a ring that already had alias states when
/// `RecyclersUnloadedCount` was introduced is never counted, so a caller that needs its number has
/// to scan `RecyclerAliasStates`". Seeding it would risk a `defensive!` mismatch at ring removal.
fn step_recyclers_unloaded(last: Option<BoundedVec<u8, ConstU32<MAX_CURSOR_KEY>>>) -> StepOutcome {
	let mut iter = match resume(&last) {
		Some(k) => old::RecyclersUnloaded::<Runtime>::iter_keys_from(k.to_vec()),
		None => old::RecyclersUnloaded::<Runtime>::iter_keys(),
	};
	let Some((denomination, ring, alias)) = iter.next() else { return StepOutcome::Done };
	let raw = old::RecyclersUnloaded::<Runtime>::hashed_key_for((denomination, ring, alias));

	RecyclerAliasStates::<Runtime>::insert(
		(LEGACY_INSTANCE_ID, denomination, ring, alias),
		AliasState::Unloaded,
	);
	old::RecyclersUnloaded::<Runtime>::remove((denomination, ring, alias));
	match bound_key(raw) {
		Ok(k) => StepOutcome::Next(k),
		Err(()) => StepOutcome::Halt,
	}
}

/// One baseline `RecyclerCollectionCreated` key. Unit A already wrote the double-map replacement;
/// this drops the old key, which would otherwise sit inside the new map's own prefix.
fn step_old_markers(last: Option<BoundedVec<u8, ConstU32<MAX_CURSOR_KEY>>>) -> StepOutcome {
	// 🔴 Length discrimination is not optional here. The legacy single-map suffix is 9 bytes and
	// unit A's double-map replacement is 21; a typed legacy iterator decodes the 21-byte key as an
	// `i8` plus ignored trailing bytes, so it would DELETE the markers unit A just wrote.
	let prefix = coinage_item_prefix("RecyclerCollectionCreated");
	let from = resume(&last).map(|k| k.to_vec()).unwrap_or_else(|| prefix.clone());
	let Some(raw) = next_legacy_key(&prefix, &from, twox64_suffix(1)) else {
		return StepOutcome::Done;
	};
	sp_io::storage::clear(&raw);
	match bound_key(raw) {
		Ok(k) => StepOutcome::Next(k),
		Err(()) => StepOutcome::Halt,
	}
}

/// Count raw keys under a Coinage item whose suffix length matches the LEGACY shape.
///
/// 🔴 `post_upgrade` must use this, not the typed legacy alias. The alias misreads the
/// destination keys — which share the prefix — as legacy ones (a 48-byte `Blake2_128Concat`
/// suffix reverse-decodes as a `Twox64Concat` key with a garbage account), so an
/// `iter_keys().next().is_none()` check on it reports leftovers that do not exist. That produced a
/// false failure on the first clean run of the migration.
#[cfg(feature = "try-runtime")]
fn legacy_keys_remaining(item: &str, legacy_suffix_len: usize) -> usize {
	let prefix = coinage_item_prefix(item);
	let mut cursor = prefix.clone();
	let mut found = 0usize;
	while let Some(next) = sp_io::storage::next_key(&cursor) {
		if !next.starts_with(&prefix) {
			break;
		}
		if next.len() == prefix.len().saturating_add(legacy_suffix_len) {
			found = found.saturating_add(1);
		}
		cursor = next;
	}
	found
}

#[cfg(feature = "try-runtime")]
mod unit_b_checks {
	use super::*;
	use codec::{Decode, Encode};

	/// What `pre_upgrade` hands to `post_upgrade`: the exact population to account for.
	#[derive(Encode, Decode)]
	pub struct Captured {
		pub coins: u32,
		pub locked: u32,
		pub coin_to_recycler: u32,
		pub unloaded: u32,
		pub markers: u32,
	}

	impl MigrateCoinageToInstances {
		pub fn capture() -> Captured {
			Captured {
				coins: legacy_keys_remaining("CoinsByOwner", twox64_suffix(32)) as u32,
				locked: legacy_keys_remaining("LockedCoins", twox64_suffix(32)) as u32,
				coin_to_recycler: legacy_keys_remaining(
					"RecyclersCoinToRecycler",
					twox64_suffix(32),
				) as u32,
				// `RecyclersUnloaded` has its own item name, so nothing shares its prefix and the
				// typed alias is safe here.
				unloaded: old::RecyclersUnloaded::<Runtime>::iter_keys().count() as u32,
				markers: legacy_keys_remaining("RecyclerCollectionCreated", twox64_suffix(1))
					as u32,
			}
		}
	}
}

#[cfg(feature = "try-runtime")]
impl MigrateCoinageToInstances {
	/// Counts taken from live state, never from a documented figure — the design's
	/// `RecyclersCoinToRecycler` number was already stale by 24 entries when re-read.
	pub fn pre_upgrade_state() -> Result<alloc::vec::Vec<u8>, sp_runtime::TryRuntimeError> {
		use codec::Encode;

		ensure!(
			Instances::<Runtime>::contains_key(LEGACY_INSTANCE_ID),
			"coinage: unit A must run before unit B; instance 0 is not seeded",
		);

		let captured = Self::capture();
		log::info!(
			target: "runtime::coinage-migration",
			"pre_upgrade: {} coins, {} locked, {} coin->recycler, {} unloaded (anti-replay), \
			 {} legacy markers",
			captured.coins, captured.locked, captured.coin_to_recycler, captured.unloaded,
			captured.markers,
		);
		Ok(captured.encode())
	}

	/// The load-bearing check. Two independent things must hold:
	///
	/// 1. **Every old key is gone.** `PrefixIterator` silently skips an entry whose value fails to
	///    decode, leaving it in storage — so an emptiness assertion is the only way that surfaces.
	///    It fires after the chain is live rather than freezing it mid-step.
	/// 2. **Every entry landed**, counted against the `pre_upgrade` capture rather than a literal.
	pub fn post_upgrade_state(
		state: alloc::vec::Vec<u8>,
	) -> Result<(), sp_runtime::TryRuntimeError> {
		use codec::Decode;
		use unit_b_checks::Captured;

		let before: Captured = Decode::decode(&mut &state[..])
			.map_err(|_| "coinage: could not decode the pre_upgrade capture")?;

		// Counted by KEY SHAPE, because the destination keys share these prefixes.
		ensure!(
			legacy_keys_remaining("CoinsByOwner", twox64_suffix(32)) == 0,
			"coinage: a baseline CoinsByOwner key survived — an entry was skipped, so a balance \
			 was not migrated",
		);
		ensure!(
			legacy_keys_remaining("LockedCoins", twox64_suffix(32)) == 0,
			"coinage: a baseline LockedCoins key survived",
		);
		ensure!(
			legacy_keys_remaining("RecyclersCoinToRecycler", twox64_suffix(32)) == 0,
			"coinage: a baseline RecyclersCoinToRecycler key survived",
		);
		ensure!(
			legacy_keys_remaining("RecyclerCollectionCreated", twox64_suffix(1)) == 0,
			"coinage: a baseline RecyclerCollectionCreated key survived inside the new double map",
		);
		// Safe through the typed alias: nothing shares this item's prefix.
		ensure!(
			old::RecyclersUnloaded::<Runtime>::iter_keys().next().is_none(),
			"coinage: a baseline RecyclersUnloaded key survived — the anti-replay set is not \
			 fully rebuilt",
		);

		let coins = indiv_pallet_coinage::CoinsByOwner::<Runtime>::iter_keys().count() as u32;
		ensure!(coins == before.coins, "coinage: CoinsByOwner lost or gained entries");

		let locked = indiv_pallet_coinage::LockedCoins::<Runtime>::iter_keys().count() as u32;
		ensure!(locked == before.locked, "coinage: LockedCoins lost or gained entries");

		let c2r = RecyclersCoinToRecycler::<Runtime>::iter_keys().count() as u32;
		ensure!(
			c2r == before.coin_to_recycler,
			"coinage: RecyclersCoinToRecycler lost or gained entries",
		);

		// 🔴 The anti-replay rebuild. Every previously-consumed alias must now read as `Unloaded`;
		// one that reads `None` is spendable again.
		let states = RecyclerAliasStates::<Runtime>::iter().count() as u32;
		ensure!(
			states == before.unloaded,
			"coinage: RecyclerAliasStates does not account for every previously-unloaded alias",
		);
		for (_, state) in RecyclerAliasStates::<Runtime>::iter() {
			ensure!(
				state == AliasState::Unloaded,
				"coinage: a rebuilt alias state is not Unloaded",
			);
		}

		// Every coin must belong to the seeded instance; a stray id would be unspendable.
		for (_, coin) in indiv_pallet_coinage::CoinsByOwner::<Runtime>::iter() {
			ensure!(
				coin.instance_id == LEGACY_INSTANCE_ID,
				"coinage: a migrated coin is not in the seeded instance",
			);
		}

		log::info!(
			target: "runtime::coinage-migration",
			"post_upgrade: {coins} coins, {locked} locked, {c2r} coin->recycler and {states} \
			 anti-replay entries verified against the pre_upgrade capture",
		);
		Ok(())
	}
}

/// The 16 `pallet-members` storage items keyed by `Identifier`, all `Identity`-hashed.
///
/// `Identity` means the 32 identifier bytes appear verbatim in the key, so relocating a collection
/// is a pure byte splice — `dest = item_prefix ++ new_id ++ tail` — with no value ever decoded.
/// `IdentifiersOf` is deliberately absent: it is keyed by *owner*, not identifier, and is handled
/// separately.
const MEMBERS_ITEMS: [&str; 16] = [
	"Collections",
	"SuspendedCollections",
	"Root",
	"OldRoots",
	"CurrentRingIndex",
	"OnboardingSize",
	"RingKeys",
	"RingKeysStatus",
	"PendingSuspensions",
	"ActiveMembers",
	"Members",
	"RingsState",
	"StaleRings",
	"QueuePageIndices",
	"OnboardingQueue",
	"RingDeletionQueue",
];

/// Baseline identifier: `b"coinage/recycler" ++ [denomination] ++ [0u8; 15]`.
fn legacy_recycler_identifier(denomination: i8) -> [u8; 32] {
	let mut id = [0u8; 32];
	id[0..16].copy_from_slice(&indiv_pallet_coinage::RECYCLER_COLLECTION_PREFIX);
	id[16] = denomination as u8;
	id
}

/// v0.3.1 identifier: the instance id is spliced in at `[16..20]` and the denomination moves to
/// `[20]`. Even at instance 0 this differs from the baseline for every denomination but zero.
fn instanced_recycler_identifier(denomination: i8) -> [u8; 32] {
	let mut id = [0u8; 32];
	id[0..16].copy_from_slice(&indiv_pallet_coinage::RECYCLER_COLLECTION_PREFIX);
	id[16..20].copy_from_slice(&LEGACY_INSTANCE_ID.to_le_bytes());
	id[20] = denomination as u8;
	id
}

/// `twox128("Members") ++ twox128(item) ++ identifier`.
fn members_item_prefix(item: &str, identifier: &[u8; 32]) -> alloc::vec::Vec<u8> {
	let mut key = alloc::vec::Vec::with_capacity(64 + 32);
	key.extend_from_slice(&sp_io::hashing::twox_128(b"Members"));
	key.extend_from_slice(&sp_io::hashing::twox_128(item.as_bytes()));
	key.extend_from_slice(identifier);
	key
}

/// Where [`RelocateCoinageRecyclerCollections`] has got to.
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub enum Relocation {
	/// Splicing raw keys, item by item, denomination by denomination.
	Keys {
		/// Index into [`MEMBERS_ITEMS`].
		item: u8,
		/// The denomination being relocated.
		denomination: i8,
		/// The raw key last spliced, or `None` to start this prefix.
		last: Option<BoundedVec<u8, ConstU32<MAX_CURSOR_KEY>>>,
	},
	/// Rewriting each relocated collection's recorded owner.
	Owners { denomination: i8 },
	/// Re-pointing the owner index.
	Index,
}

/// Unit C — relocate the 15 live recycler collections inside `pallet-members`.
///
/// v0.3.1 changed the identifier derivation from `id[16] = denomination` to
/// `id[16..20] = instance_id; id[20] = denomination`, so every existing recycler ring sits at an
/// address the new code never looks at. Unmigrated, loading or unloading recreates empty
/// collections beside the old ones and the existing member rows are stranded.
///
/// `pallet-members` hashes `Identifier` with `Identity` in all 16 of the items below, so the
/// identifier bytes appear verbatim in the key and relocation is a pure byte splice — no value is
/// ever decoded, which is what makes this cheaper per row than unit B despite being larger.
///
/// The two things that are *not* prefix-movable are handled in their own phases: the owner
/// recorded inside `CollectionInfo`, and `IdentifiersOf`, which is keyed by owner rather than by
/// identifier.
pub struct RelocateCoinageRecyclerCollections;

impl SteppedMigration for RelocateCoinageRecyclerCollections {
	type Cursor = Relocation;
	type Identifier = MigrationId<32>;

	fn id() -> Self::Identifier {
		MigrationId {
			pallet_id: *b"paseo-coinage-relocate-recycler-",
			version_from: 0,
			version_to: 1,
		}
	}

	fn max_steps() -> Option<u32> {
		Some(MAX_MIGRATION_STEPS)
	}

	fn step(
		cursor: Option<Self::Cursor>,
		meter: &mut WeightMeter,
	) -> Result<Option<Self::Cursor>, SteppedMigrationError> {
		let required = per_entry_weight();
		if meter.remaining().any_lt(required) {
			return Err(SteppedMigrationError::InsufficientWeight { required });
		}

		let mut state =
			cursor.unwrap_or(Relocation::Keys { item: 0, denomination: i8::MIN, last: None });

		loop {
			if meter.try_consume(required).is_err() {
				return Ok(Some(state));
			}

			state = match state {
				Relocation::Keys { item, denomination, last } =>
					step_relocate_keys(item, denomination, last),
				Relocation::Owners { denomination } => step_relocate_owner(denomination),
				Relocation::Index =>
					return {
						relocate_identifier_index();
						Ok(None)
					},
			};
		}
	}
}

/// Advance the (item, denomination) walk, wrapping denomination into the next item.
fn advance(item: u8, denomination: i8) -> Relocation {
	match denomination.checked_add(1) {
		Some(next) => Relocation::Keys { item, denomination: next, last: None },
		// Denominations exhausted for this item; move to the next one.
		None => match item.checked_add(1) {
			Some(next) if (next as usize) < MEMBERS_ITEMS.len() =>
				Relocation::Keys { item: next, denomination: i8::MIN, last: None },
			_ => Relocation::Owners { denomination: i8::MIN },
		},
	}
}

/// Splice one raw key from a collection's legacy prefix to its instanced prefix.
fn step_relocate_keys(
	item: u8,
	denomination: i8,
	last: Option<BoundedVec<u8, ConstU32<MAX_CURSOR_KEY>>>,
) -> Relocation {
	let Some(name) = MEMBERS_ITEMS.get(item as usize) else {
		return Relocation::Owners { denomination: i8::MIN };
	};

	// Only denominations this chain actually has a collection for. Derived from the double map
	// unit A seeded, so it survives unit B dropping the legacy markers.
	if !RecyclerCollectionCreated::<Runtime>::contains_key(LEGACY_INSTANCE_ID, denomination) {
		return advance(item, denomination);
	}

	// 🔴 Denomination 0's identifiers are BYTE-IDENTICAL between the two schemes: the legacy
	// layout writes `id[16] = 0` and the instanced one writes `id[16..20] = 0u32` then
	// `id[20] = 0`, and both leave the tail zeroed. Relocating it would mean `set(dest)` followed
	// by `clear(src)` on the same key — deleting the collection. Measured against a real snapshot:
	// 4 `pallet-members` rows vanished, exactly denomination 0's row count.
	//
	// It is already at its destination, so there is nothing to move.
	let legacy_id = legacy_recycler_identifier(denomination);
	let instanced_id = instanced_recycler_identifier(denomination);
	if legacy_id == instanced_id {
		return advance(item, denomination);
	}

	let src = members_item_prefix(name, &legacy_id);
	let dst = members_item_prefix(name, &instanced_id);

	// 🔴 Seven of the sixteen items are `StorageMap<_, Identity, Identifier, V>`, where the
	// identifier IS the entire key — so the stored key equals `src` exactly. `next_key(src)`
	// returns the key strictly AFTER it, which would skip the entry itself and strand the
	// collection at its legacy address. Handle that exact key before walking.
	let key = match last {
		None if sp_io::storage::get(&src).is_some() => src.clone(),
		last => {
			let from = last.map(|k| k.to_vec()).unwrap_or_else(|| src.clone());
			match sp_io::storage::next_key(&from) {
				Some(k) if k.starts_with(&src) => k,
				_ => return advance(item, denomination),
			}
		},
	};

	// Pure splice: the identifier occupies a fixed 32-byte slot, so everything after the source
	// prefix is the untouched remainder of the key.
	let mut moved = dst;
	moved.extend_from_slice(&key[src.len()..]);
	if let Some(value) = sp_io::storage::get(&key) {
		sp_io::storage::set(&moved, &value);
		sp_io::storage::clear(&key);
	}

	match bound_key(key) {
		Ok(k) => Relocation::Keys { item, denomination, last: Some(k) },
		// Cannot advance the cursor, so leave this prefix rather than loop. The un-relocated
		// keys are what `post_upgrade` detects.
		Err(()) => advance(item, denomination),
	}
}

/// Rewrite one relocated collection's recorded owner from the pallet account to the
/// instance-scoped one, matching `Coinage::recycler_collection_owner(0)`.
fn step_relocate_owner(denomination: i8) -> Relocation {
	if !RecyclerCollectionCreated::<Runtime>::contains_key(LEGACY_INSTANCE_ID, denomination) {
		return match denomination.checked_add(1) {
			Some(next) => Relocation::Owners { denomination: next },
			None => Relocation::Index,
		};
	}

	// The owner rewrite applies to every relocated collection INCLUDING denomination 0, whose
	// keys never moved (its two identifiers are equal) but whose recorded owner still changes.
	let id = instanced_recycler_identifier(denomination);
	indiv_pallet_members::Collections::<Runtime>::mutate(id, |maybe| {
		if let Some(info) = maybe {
			info.owner =
				indiv_pallet_members::CollectionOwner::External(indiv_pallet_coinage::Pallet::<
					Runtime,
				>::recycler_collection_owner(
					LEGACY_INSTANCE_ID
				));
		}
	});

	match denomination.checked_add(1) {
		Some(next) => Relocation::Owners { denomination: next },
		None => Relocation::Index,
	}
}

/// Move the relocated identifiers from the pallet-account owner index to the instance-scoped one,
/// leaving the paid-token collections — which did **not** move — where they are.
///
/// 🔴 This is the one place unit C can fail on a bound: both resulting lists are
/// `BoundedVec<_, MaxCollections>`. Overflow is logged and the entry left untouched rather than
/// panicking, because People freezes on a failed migration.
fn relocate_identifier_index() {
	use indiv_pallet_members::{CollectionOwner, IdentifiersOf};

	let old_owner = CollectionOwner::External(xcm::v5::Location::new(
		0,
		[xcm::v5::Junction::PalletInstance(
			<indiv_pallet_coinage::Pallet<Runtime> as frame_support::traits::PalletInfoAccess>::index() as u8,
		)],
	));
	let new_owner = CollectionOwner::External(
		indiv_pallet_coinage::Pallet::<Runtime>::recycler_collection_owner(LEGACY_INSTANCE_ID),
	);

	let relocated: alloc::vec::Vec<[u8; 32]> =
		RecyclerCollectionCreated::<Runtime>::iter_key_prefix(LEGACY_INSTANCE_ID)
			.map(instanced_recycler_identifier)
			.collect();
	let legacy: alloc::vec::Vec<[u8; 32]> =
		RecyclerCollectionCreated::<Runtime>::iter_key_prefix(LEGACY_INSTANCE_ID)
			.map(legacy_recycler_identifier)
			.collect();

	// Denomination 0's identifier is the same in both schemes, so it appears in `legacy` AND in
	// `relocated`. Removing it from the old owner and adding it to the new one is still correct —
	// the collection moved owners even though its key did not — but it must not be dropped from
	// `relocated` or the new owner would never list it.
	IdentifiersOf::<Runtime>::mutate(&old_owner, |maybe| {
		if let Some(list) = maybe {
			list.retain(|id| !legacy.contains(id));
		}
	});

	IdentifiersOf::<Runtime>::mutate(&new_owner, |maybe| {
		let list = maybe.get_or_insert_with(Default::default);
		for id in &relocated {
			if list.contains(id) {
				continue;
			}
			if list.try_push(*id).is_err() {
				log::error!(
					target: "runtime::coinage-migration",
					"IdentifiersOf overflow for the instanced owner; collection left unindexed",
				);
			}
		}
	});
}
