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

use super::*;

pub mod coinage;
pub mod ring_roots;
use alloc::vec::Vec;
use assets_common::{
	local_and_foreign_assets::ForeignAssetReserveData,
	migrations::foreign_assets_reserves::ForeignAssetsReservesProvider,
};
use xcm::v5::{Junction::Parachain, Location};

/// Resets a pallet's on-chain storage version to 0.
///
/// The Paseo-local v2.5.x migrations of the individuality pallets were `VersionedMigration`s and
/// left the on-chain storage version of their pallets at 1 once they ran. Those pallets now come
/// straight from upstream `individuality-community`, which ships them **without** a
/// `#[pallet::storage_version]`, and a pallet without one fails its try-runtime `post_upgrade`
/// while its on-chain version is non-zero ("On chain storage version set, while the pallet
/// doesn't have the `#[pallet::storage_version(VERSION)]` attribute"). Upstream's
/// genesis-launched chains carry version 0 for them; this puts Paseo in the same state.
///
/// Idempotent: a single read once the version is 0, so it is safe to leave in the tuple.
pub struct ResetStorageVersion<P>(core::marker::PhantomData<P>);
impl<P: frame_support::traits::PalletInfoAccess> frame_support::traits::OnRuntimeUpgrade
	for ResetStorageVersion<P>
{
	fn on_runtime_upgrade() -> frame_support::weights::Weight {
		use frame_support::traits::StorageVersion;
		let db = <Runtime as frame_system::Config>::DbWeight::get();
		if StorageVersion::get::<P>() == StorageVersion::new(0) {
			return db.reads(1);
		}
		log::info!(
			target: "runtime::migrations",
			"resetting the on-chain storage version of {} to 0 (upstream declares none)",
			P::name(),
		);
		StorageVersion::new(0).put::<P>();
		db.reads_writes(1, 1)
	}

	#[cfg(feature = "try-runtime")]
	fn post_upgrade(_state: alloc::vec::Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
		use frame_support::traits::StorageVersion;
		frame_support::ensure!(
			StorageVersion::get::<P>() == StorageVersion::new(0),
			"ResetStorageVersion: the on-chain storage version is still non-zero"
		);
		Ok(())
	}
}

/// Unreleased migrations. Add new ones here:
pub type Unreleased = (
	cumulus_pallet_xcmp_queue::migration::v6::MigrateV5ToV6<Runtime>,
	cumulus_pallet_parachain_system::migration::Migration<Runtime>,
	//
	// 🔴 MUST RUN BEFORE ANYTHING THAT READS A RING ROOT.
	//
	// PASEO-LOCAL. v0.3.1's `verifiable` bump shrinks `Members::Root.root` from 768 to 288 bytes
	// (it stores only the ring commitment; the 480-byte KZG verifier key is now re-derived). The
	// 24 live records are in the old layout and `RingRoot`'s hand-written `Decode` reads them
	// **without error** — every ring would present the same root. Strips the inlined key.
	//
	// Ordered here because the two collection-creation migrations below fire
	// `on_ring_root_change`, which reads roots: they must see the converted form.
	ring_roots::MigrateRingRootsToCommitmentOnly,
	// Creates the on-chain collections the v0.3.1 people pallets expect. Both are self-guarding
	// (they no-op when the collection already exists).
	indiv_pallet_people::migration::CreatePeopleCollection<Runtime>,
	indiv_pallet_people_lite::migration::CreateLitePeopleCollection<Runtime>,
	//
	// PASEO-LOCAL. Unit A of the coinage migration: adopt the pre-existing coin population into
	// instance 0, seeded from the removed `UnderlyingAssetId`.
	//
	// Ordering, all load-bearing:
	//   1. AFTER `SeedNetworkSuffix`. Coinage aliases derive from product contexts, which splice
	//      the network suffix; the suffix must be in state before anything reads it back out.
	//   2. AFTER both collection-creation migrations. Coinage's recycler and paid-unload-token
	//      rings hang off the people / lite-people collections, so those must exist first.
	//   3. BEFORE units B and C, which run as MBMs. Both phrase their invariants against
	//      `Instances[0]`, and every coinage call reads it.
	//
	// The instance is seeded `Sufficient` and native-unit-preserving, so no paid-unload fee path
	// changes denomination. (The earlier note here referenced `CoinageFeeConversion`, the
	// fail-closed adapter that has since been replaced by the on-chain AMM.)
	coinage::SeedCoinageInstanceZero,
	// The v2.5.2 Paseo-local `MembersNotifier`, `Score` and `OriginRestriction` migrations have
	// run on chain and are retired; see `ResetStorageVersion` for the version they left behind.
	ResetStorageVersion<MembersNotifier>,
	ResetStorageVersion<Score>,
	ResetStorageVersion<OriginRestriction>,
	// Storage-version bootstrap for the pallet added in this release, as upstream
	// `next-people-paseo` carries it. Single use: remove once the upgrade carrying it is live.
	indiv_pallet_nft_credits::migration::MigrateV0ToV1<Runtime>,
);

/// Migrations/checks that do not need to be versioned and can run on every update.
pub type Permanent = pallet_xcm::migration::MigrateToLatestXcmVersion<Runtime>;

/// All migrations that will run on the next runtime upgrade.
pub type SingleBlockMigrations = (Unreleased, Permanent);

/// MBM migrations to apply on runtime upgrade.
///
/// `pallet_assets::Config::ReserveData` changed from `()` to `ForeignAssetReserveData`, so the
/// per-asset reserve entries must be backfilled from the previously hardcoded XCM rules.
pub type MbmMigrations = (
	assets_common::migrations::foreign_assets_reserves::ForeignAssetsReservesMigration<
		Runtime,
		(),
		PeoplePaseoAssetsReservesProvider,
	>,
	// Units B and C of the coinage migration. B rebuilds the anti-replay set and moves every
	// per-owner and per-recycler key; C relocates the 15 recycler collections inside
	// `pallet-members`. C takes its denominations from the double map unit A seeded rather than
	// from the legacy markers B drops, so B and C are order-independent between themselves — but
	// both require A, which runs single-block in `Unreleased` above.
	coinage::MigrateCoinageToInstances,
	coinage::RelocateCoinageRecyclerCollections,
);

fn reserve_data_for(asset_id: &Location) -> Option<ForeignAssetReserveData> {
	let (parents, interior) = asset_id.unpack();
	if parents != 1 {
		return None;
	}
	let reserve = match interior.first() {
		Some(Parachain(id)) => Location::new(1, [Parachain(*id)]),
		_ => return None,
	};
	Some((reserve, false).into())
}

pub struct PeoplePaseoAssetsReservesProvider;
impl ForeignAssetsReservesProvider for PeoplePaseoAssetsReservesProvider {
	type ReserveData = ForeignAssetReserveData;

	fn reserves_for(asset_id: &Location) -> Vec<Self::ReserveData> {
		reserve_data_for(asset_id).into_iter().collect()
	}

	#[cfg(feature = "try-runtime")]
	fn check_reserves_for(asset_id: &Location, reserves: Vec<Self::ReserveData>) -> bool {
		reserves == Self::reserves_for(asset_id)
	}
}
