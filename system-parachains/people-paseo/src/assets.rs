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

use super::*;

use assets_common::local_and_foreign_assets::ForeignAssetReserveData;
use frame_support::{
	parameter_types,
	traits::{
		fungibles::Balanced, AsEnsureOriginWithArg, ConstU128, ConstU32,
	},
	PalletId,
};
use frame_system::{EnsureNever, EnsureRoot};
use frame_support::traits::tokens::imbalance::ResolveAssetTo;
use sp_runtime::Permill;
use xcm::latest::{Asset, AssetId, Junction::*, Location};

parameter_types! {
	pub const AssetDeposit: Balance = UNITS;
	pub const AssetAccountDeposit: Balance = system_para_deposit(1, 16);
	pub const ApprovalDeposit: Balance = SYSTEM_PARA_EXISTENTIAL_DEPOSIT;
	pub const AssetsStringLimit: u32 = 50;
	pub const MetadataDepositBase: Balance = system_para_deposit(1, 68);
	pub const MetadataDepositPerByte: Balance = system_para_deposit(0, 1);
}

impl pallet_assets::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Balance = Balance;
	type AssetId = Location;
	type AssetIdParameter = Location;
	type Currency = Balances;
	// Assets can only be force created by root.
	type CreateOrigin = EnsureNever<AccountId>;
	type ForceOrigin = EnsureRoot<AccountId>;
	type AssetDeposit = AssetDeposit;
	type MetadataDepositBase = MetadataDepositBase;
	type MetadataDepositPerByte = MetadataDepositPerByte;
	type ApprovalDeposit = ApprovalDeposit;
	type StringLimit = AssetsStringLimit;
	type Holder = AssetsHolder;
	type Freezer = ();
	type Extra = ();
	type WeightInfo = weights::pallet_assets::WeightInfo<Runtime>;
	type CallbackHandle = ();
	type AssetAccountDeposit = AssetAccountDeposit;
	type ReserveData = ForeignAssetReserveData;
	type RemoveItemsLimit = frame_support::traits::ConstU32<1000>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = xcm_config::XcmBenchmarkHelper;
}

/// Handles crediting transaction fees to the staking pot.
pub struct CreditToStakingPot;
impl pallet_asset_tx_payment::HandleCredit<AccountId, Assets> for CreditToStakingPot {
	fn handle_credit(credit: frame_support::traits::fungibles::Credit<AccountId, Assets>) {
		use sp_core::TypedGet;
		let staking_pot = pallet_collator_selection::StakingPotAccountId::<Runtime>::get();
		let _ = Assets::resolve(&staking_pot, credit);
	}
}

type OnChargeStableTransaction =
	pallet_asset_tx_payment::FungiblesAdapter<AssetRate, CreditToStakingPot>;

#[cfg(feature = "runtime-benchmarks")]
pub struct AssetTxPaymentBenchmarkHelper;
#[cfg(feature = "runtime-benchmarks")]
impl pallet_asset_tx_payment::BenchmarkHelperTrait<AccountId, Location, Location>
	for AssetTxPaymentBenchmarkHelper
{
	fn create_asset_id_parameter(id: u32) -> (Location, Location) {
		assert_eq!(id, 1);
		let l = Location::new(
			1,
			[
				xcm::latest::Junction::Parachain(1000),
				xcm::latest::Junction::PalletInstance(50),
				xcm::latest::Junction::GeneralIndex(1337),
			],
		);
		(l.clone(), l)
	}

	fn setup_balances_and_pool(asset_id: Location, account: AccountId) {
		use alloc::boxed::Box;
		use frame_support::traits::{
			fungible::Mutate as _,
			fungibles::{Inspect as _, Mutate as _},
		};

		AssetRate::create(RuntimeOrigin::root(), Box::new(asset_id.clone()), 1.into()).unwrap();
		if !Assets::asset_exists(asset_id.clone()) {
			Assets::force_create(
				RuntimeOrigin::root(),
				asset_id.clone(),
				account.clone().into(),
				true,
				1,
			)
			.unwrap();
		}
		Assets::mint_into(asset_id, &account, 10_000 * UNITS).unwrap();
		Balances::mint_into(&account, 10_000 * UNITS).unwrap();
	}
}

impl pallet_asset_tx_payment::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Fungibles = Assets;
	type OnChargeAssetTransaction = OnChargeStableTransaction;
	type WeightInfo = weights::pallet_asset_tx_payment::WeightInfo<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = AssetTxPaymentBenchmarkHelper;
}

impl pallet_asset_rate::Config for Runtime {
	type WeightInfo = weights::pallet_asset_rate::WeightInfo<Runtime>;
	type RuntimeEvent = RuntimeEvent;
	type CreateOrigin = EnsureRoot<AccountId>;
	type RemoveOrigin = EnsureRoot<AccountId>;
	type UpdateOrigin = EnsureRoot<AccountId>;
	type Currency = Balances;
	type AssetKind = <Runtime as pallet_assets::Config>::AssetId;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = AssetRateBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct AssetRateBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl pallet_asset_rate::AssetKindFactory<Location> for AssetRateBenchmarkHelper {
	fn create_asset_kind(seed: u32) -> Location {
		Location::new(
			1,
			[
				xcm::latest::Junction::Parachain(1000),
				xcm::latest::Junction::GeneralIndex(seed as u128),
			],
		)
	}
}

impl pallet_assets_holder::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeHoldReason = RuntimeHoldReason;
}

/// Module that holds everything related to the HOLLAR asset.
pub mod hollar {
	use super::*;
	use frame_support::traits::ContainsPair;

	/// The parachain id of the Hydration DEX.
	pub const HYDRATION_PARA_ID: u32 = 2034;

	/// The asset id of HOLLAR.
	pub const HOLLAR_ASSET_ID: u128 = 222;

	/// A unit of HOLLAR consists of 10^18 plancks.
	pub const HOLLAR_UNITS: u128 = 1_000_000_000_000_000_000u128;

	parameter_types! {
		pub HydrationLocation: Location = Location::new(1, [Parachain(HYDRATION_PARA_ID)]);
		pub HollarLocation: Location = Location::new(1, [Parachain(HYDRATION_PARA_ID), GeneralIndex(HOLLAR_ASSET_ID)]);
		pub HollarId: AssetId = AssetId(HollarLocation::get());
		pub Hollar: Asset = (HollarId::get(), 10 * HOLLAR_UNITS).into();
	}

	/// A type that matches the pair `(Hollar, Hydration)`,
	/// used in the XCM configuration's `IsReserve`.
	pub struct HollarFromHydration;
	impl ContainsPair<Asset, Location> for HollarFromHydration {
		fn contains(asset: &Asset, origin: &Location) -> bool {
			let is_hydration = matches!(origin.unpack(), (1, [Parachain(para_id)]) if *para_id == HYDRATION_PARA_ID);
			let is_hollar = matches!(
				asset.id.0.unpack(),
				(1, [Parachain(para_id), GeneralIndex(asset_id)])
				if *para_id == HYDRATION_PARA_ID && *asset_id == HOLLAR_ASSET_ID
			);

			is_hydration && is_hollar
		}
	}
}

// ---------------------------------------------------------------------------------------------
// Asset conversion (AMM).
//
// Adopted so `indiv_pallet_coinage::Config::FeeConversion` can be bound to a real AMM, which is
// what both reference integrations do: individuality v0.3.1's own `next-people-paseo`
// (`AssetConversion = 18`, `PoolAssets = 19`) and polkadot-fellows' `people-polkadot`
// (`AssetConversion = 17`, `PoolAssets = 18`). This runtime uses 18/19; 17 is taken by
// `OriginRestriction`.
//
// Modelled on the fellowship's setup. The values below are theirs unless noted.
// ---------------------------------------------------------------------------------------------

/// The pool-token instance of `pallet-assets`, owned by `pallet-asset-conversion`.
pub type PoolAssetsInstance = pallet_assets::Instance1;

impl pallet_assets::Config<PoolAssetsInstance> for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Balance = Balance;
	type RemoveItemsLimit = ConstU32<1000>;
	type AssetId = u32;
	type AssetIdParameter = u32;
	type ReserveData = ();
	type Currency = Balances;
	// Only the AMM creates pool tokens. Outside benchmarks nothing else may.
	#[cfg(feature = "runtime-benchmarks")]
	type CreateOrigin =
		AsEnsureOriginWithArg<frame_system::EnsureSignedBy<AssetConversionOrigin, AccountId>>;
	#[cfg(not(feature = "runtime-benchmarks"))]
	type CreateOrigin =
		AsEnsureOriginWithArg<frame_support::traits::NeverEnsureOrigin<AccountId>>;
	type ForceOrigin = EnsureRoot<AccountId>;
	type AssetDeposit = ConstU128<0>;
	type AssetAccountDeposit = AssetAccountDeposit;
	type MetadataDepositBase = ConstU128<0>;
	type MetadataDepositPerByte = ConstU128<0>;
	type ApprovalDeposit = ApprovalDeposit;
	type StringLimit = AssetsStringLimit;
	type Freezer = ();
	type Holder = ();
	type Extra = ();
	type CallbackHandle = ();
	// ⚠️ In-crate reference weights: this runtime has no benchmarked `pallet_assets_pool` module.
	type WeightInfo = pallet_assets::weights::SubstrateWeight<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
}

parameter_types! {
	pub const AssetConversionPalletId: PalletId = PalletId(*b"py/ascon");
	pub const LiquidityWithdrawalFee: Permill = Permill::from_percent(0);
	pub StakingPotAccount: AccountId =
		<pallet_collator_selection::StakingPotAccountId<Runtime> as frame_support::traits::TypedGet>::get();
	pub LpFee: Permill = Permill::from_rational(3u32, 1_000u32);
	pub const PoolSetupFee: Balance = system_para_deposit(1, 4) + AssetDeposit::get();
}

#[cfg(feature = "runtime-benchmarks")]
parameter_types! {
	pub AssetsPalletIndex: u32 = <Assets as frame_support::traits::PalletInfoAccess>::index() as u32;
}

#[cfg(feature = "runtime-benchmarks")]
frame_support::ord_parameter_types! {
	pub const AssetConversionOrigin: AccountId =
		sp_runtime::traits::AccountIdConversion::<AccountId>::into_account_truncating(
			&AssetConversionPalletId::get(),
		);
}

pub type PoolIdToAccountId =
	pallet_asset_conversion::AccountIdConverter<AssetConversionPalletId, (Location, Location)>;

impl pallet_asset_conversion::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Balance = Balance;
	type HigherPrecisionBalance = sp_core::U256;
	type AssetKind = Location;
	type Assets = crate::people::NativeAndAssets;
	type PoolId = (Self::AssetKind, Self::AssetKind);
	type PoolLocator = pallet_asset_conversion::WithFirstAsset<
		crate::xcm_config::RelayLocation,
		AccountId,
		Self::AssetKind,
		PoolIdToAccountId,
	>;
	type PoolAssetId = u32;
	type PoolAssets = PoolAssets;
	type PoolSetupFee = PoolSetupFee;
	type PoolSetupFeeAsset = crate::xcm_config::RelayLocation;
	type PoolSetupFeeTarget = ResolveAssetTo<StakingPotAccount, Self::Assets>;
	type LiquidityWithdrawalFee = LiquidityWithdrawalFee;
	type LPFee = LpFee;
	type PalletId = AssetConversionPalletId;
	type MaxSwapPathLength = ConstU32<3>;
	type MintMinLiquidity = ConstU128<100>;
	// ⚠️ In-crate reference weights, not benchmarked on this runtime.
	type WeightInfo = pallet_asset_conversion::weights::SubstrateWeight<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = assets_common::benchmarks::AssetPairFactory<
		crate::xcm_config::RelayLocation,
		parachain_info::Pallet<Runtime>,
		AssetsPalletIndex,
		Self::AssetKind,
	>;
}
