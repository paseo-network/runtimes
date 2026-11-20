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

//! The runtime migrations per release.

use crate::Runtime;
use frame_support::parameter_types;

parameter_types! {
	pub const AhMigratorPalletName: &'static str = "AhMigrator";
}

pub type RemoveAhMigratorPallet = frame_support::migrations::RemovePallet<
	AhMigratorPalletName,
	<Runtime as frame_system::Config>::DbWeight,
>;

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
	// Wipes the storage the retired `AhMigrator` pallet left behind. Upstream ran it before
	// v2.5.0; on Paseo it was never wired and the pallet's keys are still on chain.
	RemoveAhMigratorPallet,
	// stable2609-2: adds the registration base deposit to each stored signed-phase
	// `SubmissionMetadata`. `VersionedMigration` (storage version 0 -> 1).
	pallet_election_provider_multi_block::signed::migrations::v1::MigrateV0ToV1<Runtime>,
	// individuality v0.3.1 gave `AccountNameRecord` a `chat` field on each of `lite`/`full`.
	// Live `AccountNames` entries are in the old two-`Option<BaseLabel>` shape and would fail
	// to decode. `VersionedMigration`, so it is self-guarding and cannot run twice.
	indiv_pallet_dotns_gateway::migration::MigrateV0ToV1<Runtime>,
	// The v2.5.0 Paseo-local `OriginRestriction` migration has run on chain and is retired; see
	// `ResetStorageVersion` for the version it left behind.
	ResetStorageVersion<crate::OriginRestriction>,
	// Storage-version bootstrap for the two pallets added in this release, as upstream
	// `next-asset-hub-paseo` carries them. Both are `VersionedMigration`s over empty maps here.
	indiv_pallet_scarcity::migration::MigrateV0ToV1<Runtime>,
	indiv_pallet_scarcity::migration::MigrateV1ToV2<Runtime>,
	indiv_pallet_nft_claims::migration::MigrateV0ToV1<Runtime>,
);

/// Migrations/checks that do not need to be versioned and can run on every update.
pub type Permanent = pallet_xcm::migration::MigrateToLatestXcmVersion<crate::Runtime>;

/// All single block migrations that will run on the next runtime upgrade.
pub type SingleBlockMigrations = (Unreleased, Permanent);

/// MBM migrations to apply on runtime upgrade.
#[cfg(not(feature = "runtime-benchmarks"))]
pub type MbmMigrations = (
	// Scarcity `Instances` v2 -> v3: each value becomes a `Location`. It starts once the
	// single-block `MigrateV1ToV2` above has left the pallet at version 2, and is a no-op on
	// any other version.
	indiv_pallet_scarcity::migration::v3::MigrateToLocations<Runtime>,
);
