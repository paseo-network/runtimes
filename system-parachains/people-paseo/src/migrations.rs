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
use alloc::vec::Vec;
use assets_common::{
	local_and_foreign_assets::ForeignAssetReserveData,
	migrations::foreign_assets_reserves::ForeignAssetsReservesProvider,
};
use xcm::v5::{Junction::Parachain, Location};

/// Unreleased migrations. Add new ones here:
pub type Unreleased = (
	cumulus_pallet_xcmp_queue::migration::v6::MigrateV5ToV6<Runtime>,
	cumulus_pallet_parachain_system::migration::Migration<Runtime>,
	// ---- individuality v0.3.1 ----
	//
	// MANDATORY, and FIRST of the individuality entries. Writes the `NetworkSuffix` key into
	// state. The pallet's `ValueQuery` default already answers every on-chain read, so nothing
	// in the runtime needs this -- but `state_getStorage` over an unwritten key returns `null`,
	// and the Android client reads that key directly and throws on `null`. Ordered first so any
	// later migration that derives a product context sees a materialised suffix.
	// Idempotent; never clobbers a suffix governance has changed.
	indiv_pallet_network_suffix::migration::SeedNetworkSuffix<Runtime>,
	//
	// 🔴 MUST RUN BEFORE THE TWO COLLECTION-CREATION MIGRATIONS BELOW.
	//
	// PASEO-LOCAL. v0.3.1 adds `SubscribedCollections` and makes
	// `OnRingRootChange::on_ring_root_change` early-return for any collection not in it.
	// Upstream ships no migration for that map, correctly: `next-people-paseo` launches from a
	// genesis preset with no subscribers, so an empty map is accurate there. This chain has one
	// live subscriber — para 1000 (Asset Hub), two collections — and leaving the map empty stops
	// ring-root updates flowing to Asset Hub permanently, with no error and no event.
	//
	// The ordering is what makes it non-obvious: `CreatePeopleCollection` and
	// `CreateLitePeopleCollection` write ring roots, so they fire `on_ring_root_change`. Any root
	// change fired before this seed is dropped on the floor and cannot be recovered — the update
	// is not queued, it is discarded.
	indiv_pallet_members_notifier::migration_paseo::MigrateV0ToV1<Runtime>,
	// Creates the on-chain collections the v0.3.1 people pallets expect. Both are self-guarding
	// (they no-op when the collection already exists).
	indiv_pallet_people::migration::CreatePeopleCollection<Runtime>,
	indiv_pallet_people_lite::migration::CreateLitePeopleCollection<Runtime>,
	//
	// PASEO-LOCAL. `Participant.score` narrows `u32` -> `u8` and `Streak(u32)` -> `Streak(u8)`.
	// The live 31-byte value DECODES SUCCESSFULLY under the new layout into wrong values —
	// `unhashed::get` uses `Decode::decode`, not `decode_all` — so without this nothing errors
	// and the corruption is silent, including a fabricated hold-backed `credit`.
	// Order-independent of the entries above: `score` shares no storage with them.
	indiv_pallet_score::migration::MigrateV0ToV1<Runtime>,
	//
	// PASEO-LOCAL. `Usages.at_block` keeps type `u32` but changes meaning: local para block ->
	// relay block (`Config::BlockNumberProvider` is now `RelaychainDataProvider`). Paseo's relay
	// height is far below People's, so `saturating_sub` yields an elapsed time of 0 forever and
	// the live entry never rewrites its own stamp. This rebases the clock.
	// 🔴 This migration and the `BlockNumberProvider` binding in `people.rs` are one change.
	// Enacting either without the other is a permanent user lockout.
	indiv_pallet_origin_restriction::migration::MigrateV0ToV1<Runtime>,
	//
	// ================================================================================
	// SLOT: coinage migrations. OWNED BY ANOTHER AGENT -- do not write them here.
	//
	// Required ordering when they land:
	//   1. AFTER `SeedNetworkSuffix` above. Coinage aliases are derived from product
	//      contexts, which splice the network suffix; the suffix must be materialised in
	//      state before anything reads it back out.
	//   2. AFTER both collection-creation migrations above. Coinage's recycler and
	//      paid-unload-token rings hang off the people / lite-people collections, so those
	//      must exist first.
	//   3. BEFORE any migration that reads `Instances`. The coinage migration is what
	//      decides each existing coin's instance and, critically, that instance's
	//      DENOMINATION -- see `CoinageFeeConversion` in people.rs: if instances end up
	//      non-native, every paid-unload fee path needs an AMM this chain does not have.
	// ================================================================================
);

/// Migrations/checks that do not need to be versioned and can run on every update.
pub type Permanent = pallet_xcm::migration::MigrateToLatestXcmVersion<Runtime>;

/// All migrations that will run on the next runtime upgrade.
pub type SingleBlockMigrations = (Unreleased, Permanent);

/// MBM migrations to apply on runtime upgrade.
///
/// `pallet_assets::Config::ReserveData` changed from `()` to `ForeignAssetReserveData`, so the
/// per-asset reserve entries must be backfilled from the previously hardcoded XCM rules.
pub type MbmMigrations =
	assets_common::migrations::foreign_assets_reserves::ForeignAssetsReservesMigration<
		Runtime,
		(),
		PeoplePaseoAssetsReservesProvider,
	>;

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
