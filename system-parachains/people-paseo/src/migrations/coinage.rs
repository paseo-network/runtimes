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

use crate::{Runtime, Weight};
use frame_support::{
	pallet_prelude::*,
	traits::{Get, OnRuntimeUpgrade},
};
use indiv_pallet_coinage::{
	AssetToInstance, Config as CoinageConfig, FungiblesAssetIdOf, InstanceMode, InstanceRecord,
	Instances, NextInstanceId, RecyclerCollectionCreated,
};

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

		// Re-key the per-denomination collection markers into the double map. The old keys are
		// dropped by the multi-block unit, which owns every removal from this prefix.
		let mut denominations = 0u32;
		for denomination in old::RecyclerCollectionCreated::<Runtime>::iter_keys() {
			RecyclerCollectionCreated::<Runtime>::insert(LEGACY_INSTANCE_ID, denomination, ());
			denominations = denominations.saturating_add(1);
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
