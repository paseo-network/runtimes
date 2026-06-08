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

/// Provides the initial `LastIssuanceTimestamp` for the DAP V1->V2 migration.
pub struct DapLastIssuanceTimestamp;
impl frame_support::traits::Get<u64> for DapLastIssuanceTimestamp {
	fn get() -> u64 {
		pallet_staking_async::ActiveEra::<crate::Runtime>::get()
			.and_then(|era| era.start)
			.unwrap_or(0)
	}
}

/// Default DAP budget allocation: 15% buffer, 85% staker rewards, 0% validator incentive.
///
/// Matches the previous `EraPayout` 15% treasury / 85% stakers split, now enforced at the
/// DAP drip level instead of at era-payout time.
pub struct DefaultDapBudget;
impl frame_support::traits::Get<pallet_dap::BudgetAllocationMap> for DefaultDapBudget {
	fn get() -> pallet_dap::BudgetAllocationMap {
		use sp_runtime::Perbill;
		use sp_staking::budget::BudgetRecipientList;

		let recipients = <crate::Runtime as pallet_dap::Config>::BudgetRecipients::recipients();
		// Order matches `pallet_dap::Config::BudgetRecipients`:
		// [dap (buffer), StakerRewardRecipient, ValidatorIncentiveRecipient]
		let percentages =
			[Perbill::from_percent(15), Perbill::from_percent(85), Perbill::from_percent(0)];

		let mut map = pallet_dap::BudgetAllocationMap::new();
		for ((key, _), perbill) in recipients.into_iter().zip(percentages) {
			let _ = map.try_insert(key, perbill);
		}
		map
	}
}

/// Moves `pallet-multi-asset-bounties` bounty/child-bounty funds from the old `&str`-derived
/// account to the new `[u8; 3]`-derived account (SDK PR #11052). No-op if there are no
/// multi-asset bounties.
pub struct MigrateBountyAccountAssets;
impl frame_support::traits::OnRuntimeUpgrade for MigrateBountyAccountAssets {
	fn on_runtime_upgrade() -> frame_support::weights::Weight {
		use frame_support::traits::Get;
		use pallet_bounties::TransferAllAssets;
		use sp_runtime::traits::AccountIdConversion;

		let pallet_id = <crate::Runtime as pallet_treasury::Config>::PalletId::get();
		let assets_per_bounty = crate::treasury::BountyRelevantAssets::get().len() as u64;
		type Transferer = <crate::Runtime as pallet_bounties::Config>::TransferAllAssets;
		let db_weight = <crate::Runtime as frame_system::Config>::DbWeight::get();
		let mut weight = frame_support::weights::Weight::zero();

		for bounty_id in pallet_multi_asset_bounties::Bounties::<crate::Runtime>::iter_keys() {
			let old: crate::AccountId = pallet_id.into_sub_account_truncating(("mbt", bounty_id));
			let new: crate::AccountId = pallet_id.into_sub_account_truncating((
				pallet_multi_asset_bounties::BountyAccountPrefix::get(),
				bounty_id,
			));
			let _ = Transferer::force_transfer_all_assets(&old, &new);
			weight = weight.saturating_add(
				db_weight.reads_writes(2 * assets_per_bounty, 2 * assets_per_bounty),
			);
		}

		for (parent_id, child_id) in
			pallet_multi_asset_bounties::ChildBounties::<crate::Runtime>::iter_keys()
		{
			let old: crate::AccountId =
				pallet_id.into_sub_account_truncating(("mcb", parent_id, child_id));
			let new: crate::AccountId = pallet_id.into_sub_account_truncating((
				pallet_multi_asset_bounties::ChildBountyAccountPrefix::get(),
				parent_id,
				child_id,
			));
			let _ = Transferer::force_transfer_all_assets(&old, &new);
			weight = weight.saturating_add(
				db_weight.reads_writes(2 * assets_per_bounty, 2 * assets_per_bounty),
			);
		}

		weight
	}
}

/// Unreleased migrations. Add new ones here:
pub type Unreleased = (
	cumulus_pallet_xcmp_queue::migration::v6::MigrateV5ToV6<crate::Runtime>,
	cumulus_pallet_parachain_system::migration::Migration<crate::Runtime>,
	// DAP V1->V2: seed `BudgetAllocation` + `LastIssuanceTimestamp` for the non-minting model.
	pallet_dap::migrations::MigrateV1ToV2<
		crate::Runtime,
		DapLastIssuanceTimestamp,
		DefaultDapBudget,
		crate::dynamic_params::staking_election::MaxEraDuration,
	>,
	// Move multi-asset bounty pot funds to the new [u8; 3]-derived accounts.
	MigrateBountyAccountAssets,
);

/// Migrations/checks that do not need to be versioned and can run on every update.
pub type Permanent = pallet_xcm::migration::MigrateToLatestXcmVersion<crate::Runtime>;

/// All single block migrations that will run on the next runtime upgrade.
pub type SingleBlockMigrations = (Unreleased, Permanent);

#[cfg(not(feature = "runtime-benchmarks"))]
pub use multiblock_migrations::MbmMigrations;

#[cfg(not(feature = "runtime-benchmarks"))]
mod multiblock_migrations {
	use crate::{
		xcm_config::bridging::{
			to_ethereum::EthereumLocation,
			to_kusama::{AssetHubKusama, KsmLocation},
		},
		*,
	};
	use alloc::{vec, vec::Vec};
	use assets_common::{
		local_and_foreign_assets::ForeignAssetReserveData,
		migrations::foreign_assets_reserves::ForeignAssetsReservesProvider,
	};
	use frame_support::traits::Contains;
	use xcm::v5::{Junction, Location};
	use xcm_builder::StartsWith;

	/// MBM migrations to apply on runtime upgrade.
	pub type MbmMigrations =
		assets_common::migrations::foreign_assets_reserves::ForeignAssetsReservesMigration<
			Runtime,
			ForeignAssetsInstance,
			AssetHubPaseoForeignAssetsReservesProvider,
		>;

	/// This type provides reserves information for `asset_id`. Meant to be used in a migration
	/// running on the Asset Hub Paseo upgrade which changes the Foreign Assets
	/// reserve-transfers and teleports from hardcoded rules to per-asset configured reserves.
	///
	/// The hardcoded rules (see `xcm_config.rs`) migrated here:
	/// 1. Foreign Assets native to sibling parachains are teleportable between the asset's native
	///    chain and Asset Hub ==> `ForeignAssetReserveData { reserve: "Asset's native chain",
	///    teleport: true }`
	/// 2. Foreign assets native to Ethereum Ecosystem have Ethereum as trusted reserve. ==>
	///    `ForeignAssetReserveData { reserve: "Ethereum", teleport: false }`
	/// 3. Foreign assets native to Kusama Ecosystem have Asset Hub Kusama as trusted reserve. ==>
	///    `ForeignAssetReserveData { reserve: "Asset Hub Kusama", teleport: false }`
	pub struct AssetHubPaseoForeignAssetsReservesProvider;
	impl ForeignAssetsReservesProvider for AssetHubPaseoForeignAssetsReservesProvider {
		type ReserveData = ForeignAssetReserveData;
		fn reserves_for(asset_id: &Location) -> Vec<Self::ReserveData> {
			let reserves = if StartsWith::<KsmLocation>::contains(asset_id) {
				// rule 3: Kusama asset, Asset Hub Kusama reserve, non teleportable
				vec![(AssetHubKusama::get(), false).into()]
			} else if StartsWith::<EthereumLocation>::contains(asset_id) {
				// rule 2: Ethereum asset, Ethereum reserve, non teleportable
				vec![(EthereumLocation::get(), false).into()]
			} else {
				match asset_id.unpack() {
					(1, interior) => {
						match interior.first() {
							Some(Junction::Parachain(sibling_para_id))
								if sibling_para_id.ne(
									&paseo_runtime_constants::system_parachain::ASSET_HUB_ID,
								) =>
							{
								// rule 1: sibling parachain asset, sibling parachain reserve,
								// teleportable
								vec![ForeignAssetReserveData {
									reserve: Location::new(
										1,
										Junction::Parachain(*sibling_para_id),
									),
									teleportable: true,
								}]
							},
							_ => vec![],
						}
					},
					_ => vec![],
				}
			};
			if reserves.is_empty() {
				log::error!(
					target: "runtime::AssetHubPaseoForeignAssetsReservesProvider::reserves_for",
					"unexpected asset id {asset_id:?}",
				);
			}
			reserves
		}

		#[cfg(feature = "try-runtime")]
		fn check_reserves_for(asset_id: &Location, reserves: Vec<Self::ReserveData>) -> bool {
			if StartsWith::<KsmLocation>::contains(asset_id) {
				let expected =
					ForeignAssetReserveData { reserve: AssetHubKusama::get(), teleportable: false };
				// rule 3: Kusama asset
				reserves.len() == 1 && expected.eq(reserves.get(0).unwrap())
			} else if StartsWith::<EthereumLocation>::contains(asset_id) {
				let expected = ForeignAssetReserveData {
					reserve: EthereumLocation::get(),
					teleportable: false,
				};
				// rule 2: Ethereum asset
				reserves.len() == 1 && expected.eq(reserves.get(0).unwrap())
			} else {
				match asset_id.unpack() {
					(1, interior) => {
						match interior.first() {
							Some(Junction::Parachain(sibling_para_id))
								if sibling_para_id.ne(
									&paseo_runtime_constants::system_parachain::ASSET_HUB_ID,
								) =>
							{
								let expected = ForeignAssetReserveData {
									reserve: Location::new(
										1,
										Junction::Parachain(*sibling_para_id),
									),
									teleportable: true,
								};
								// rule 1: sibling parachain asset
								reserves.len() == 1 && expected.eq(reserves.get(0).unwrap())
							},
							// unexpected asset
							_ => false,
						}
					},
					// unexpected asset
					_ => false,
				}
			}
		}
	}
}
