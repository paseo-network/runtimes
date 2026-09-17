// This file is part of Cumulus.

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

//! Tests for the Paseo Asset Hub (previously known as Statemint) chain.

use asset_hub_paseo_runtime::{
	genesis_config_presets::EXTERNAL_ASSET_ID,
	xcm_config::{
		bridging, CheckingAccount, DotLocation, ExternalAssetLocation, LocationToAccountId,
		RelayChainLocation, TrustBackedAssetsPalletLocation, XcmConfig,
	},
	AllPalletsWithoutSystem, AssetDeposit, Assets, Balances, Block, Dap, ExistentialDeposit,
	ForeignAssets, ForeignAssetsInstance, MetadataDepositBase, MetadataDepositPerByte,
	ParachainSystem, PolkadotXcm, Runtime, RuntimeCall, RuntimeEvent, RuntimeOrigin, SessionKeys,
	ToKusamaXcmRouterInstance, TrustBackedAssetsInstance, XcmpQueue, SLOT_DURATION,
};
use asset_test_utils::{
	include_create_and_manage_foreign_assets_for_local_consensus_parachain_assets_works,
	include_teleports_for_foreign_assets_works, test_cases_over_bridge::TestBridgingConfig,
	CollatorSessionKey, CollatorSessionKeys, ExtBuilder, GovernanceOrigin, SlotDurations,
};
use codec::{Decode, Encode};
use frame_support::{
	assert_err, assert_ok,
	traits::{fungibles::InspectEnumerable, ContainsPair},
	weights::Weight,
};
use parachains_common::{AccountId, AssetIdForTrustBackedAssets, AuraId, Balance};
use sp_consensus_aura::SlotDuration;
use sp_core::crypto::Ss58Codec;
use sp_runtime::{traits::MaybeEquivalence, Either, TryRuntimeError};
use system_parachains_constants::paseo::{
	consensus::RELAY_CHAIN_SLOT_DURATION_MILLIS, currency::UNITS,
	fee::WeightToFee as PaseoWeightToFee,
};
use xcm::latest::{
	prelude::{Assets as XcmAssets, *},
	WESTEND_GENESIS_HASH,
};
use xcm_builder::WithLatestLocationConverter;
use xcm_runtime_apis::conversions::LocationToAccountHelper;

const ALICE: [u8; 32] = [1u8; 32];
const SOME_ASSET_ADMIN: [u8; 32] = [5u8; 32];

frame_support::parameter_types! {
	// Local OpenGov
	pub Governance: GovernanceOrigin<RuntimeOrigin> = GovernanceOrigin::Origin(RuntimeOrigin::root());
}

type AssetIdForTrustBackedAssetsConvertLatest =
	assets_common::AssetIdForTrustBackedAssetsConvert<TrustBackedAssetsPalletLocation>;
type RuntimeHelper = asset_test_utils::RuntimeHelper<Runtime, AllPalletsWithoutSystem>;
type WeightToFee = PaseoWeightToFee<Runtime>;

fn collator_session_key(account: [u8; 32]) -> CollatorSessionKey<Runtime> {
	CollatorSessionKey::new(
		AccountId::from(account),
		AccountId::from(account),
		SessionKeys { aura: AuraId::from(sp_core::sr25519::Public::from_raw(account)) },
	)
}

fn collator_session_keys() -> CollatorSessionKeys<Runtime> {
	CollatorSessionKeys::default().add(collator_session_key(ALICE))
}

fn slot_durations() -> SlotDurations {
	SlotDurations {
		relay: SlotDuration::from_millis(RELAY_CHAIN_SLOT_DURATION_MILLIS.into()),
		para: SlotDuration::from_millis(SLOT_DURATION),
	}
}

#[test]
fn test_ed_is_one_hundredth_of_relay() {
	ExtBuilder::<Runtime>::default()
		.with_tracing()
		.with_collators(vec![AccountId::from(ALICE)])
		.with_session_keys(vec![(
			AccountId::from(ALICE),
			AccountId::from(ALICE),
			SessionKeys { aura: AuraId::from(sp_core::sr25519::Public::from_raw(ALICE)) },
		)])
		.build()
		.execute_with(|| {
			let relay_ed = paseo_runtime_constants::currency::EXISTENTIAL_DEPOSIT;
			let asset_hub_ed = ExistentialDeposit::get();
			assert_eq!(relay_ed / 100, asset_hub_ed);
		});
}

#[test]
fn test_assets_balances_api_works() {
	use assets_common::runtime_api::runtime_decl_for_fungibles_api::FungiblesApi;

	ExtBuilder::<Runtime>::default()
		.with_tracing()
		.with_collators(vec![AccountId::from(ALICE)])
		.with_session_keys(vec![(
			AccountId::from(ALICE),
			AccountId::from(ALICE),
			SessionKeys { aura: AuraId::from(sp_core::sr25519::Public::from_raw(ALICE)) },
		)])
		.build()
		.execute_with(|| {
			let local_asset_id = 1;
			let foreign_asset_id_location =
				Location::new(1, [Parachain(1234), GeneralIndex(12345)]);

			// check before
			assert_eq!(Assets::balance(local_asset_id, AccountId::from(ALICE)), 0);
			assert_eq!(
				ForeignAssets::balance(foreign_asset_id_location.clone(), AccountId::from(ALICE)),
				0
			);
			assert_eq!(Balances::free_balance(AccountId::from(ALICE)), 0);
			assert!(Runtime::query_account_balances(AccountId::from(ALICE))
				.unwrap()
				.try_as::<XcmAssets>()
				.unwrap()
				.is_none());

			// Drip some balance
			use frame_support::traits::fungible::Mutate;
			let some_currency = ExistentialDeposit::get();
			Balances::mint_into(&AccountId::from(ALICE), some_currency).unwrap();

			// We need root origin to create a sufficient asset
			let minimum_asset_balance = 3333333_u128;
			assert_ok!(Assets::force_create(
				RuntimeHelper::root_origin(),
				local_asset_id.into(),
				AccountId::from(ALICE).into(),
				true,
				minimum_asset_balance
			));

			// We first mint enough asset for the account to exist for assets
			assert_ok!(Assets::mint(
				RuntimeHelper::origin_of(AccountId::from(ALICE)),
				local_asset_id.into(),
				AccountId::from(ALICE).into(),
				minimum_asset_balance
			));

			// create foreign asset
			let foreign_asset_minimum_asset_balance = 3333333_u128;
			assert_ok!(ForeignAssets::force_create(
				RuntimeHelper::root_origin(),
				foreign_asset_id_location.clone(),
				AccountId::from(SOME_ASSET_ADMIN).into(),
				false,
				foreign_asset_minimum_asset_balance
			));

			// We first mint enough asset for the account to exist for assets
			assert_ok!(ForeignAssets::mint(
				RuntimeHelper::origin_of(AccountId::from(SOME_ASSET_ADMIN)),
				foreign_asset_id_location.clone(),
				AccountId::from(ALICE).into(),
				6 * foreign_asset_minimum_asset_balance
			));

			// check after
			assert_eq!(
				Assets::balance(local_asset_id, AccountId::from(ALICE)),
				minimum_asset_balance
			);
			assert_eq!(
				ForeignAssets::balance(foreign_asset_id_location.clone(), AccountId::from(ALICE)),
				6 * minimum_asset_balance
			);
			assert_eq!(Balances::free_balance(AccountId::from(ALICE)), some_currency);

			let result: XcmAssets = Runtime::query_account_balances(AccountId::from(ALICE))
				.unwrap()
				.try_into()
				.unwrap();
			assert_eq!(result.len(), 3);

			// check currency
			assert!(result.inner().iter().any(|asset| asset.eq(
				&assets_common::fungible_conversion::convert_balance::<DotLocation, Balance>(
					some_currency
				)
				.unwrap()
			)));
			// check trusted asset
			assert!(result.inner().iter().any(|asset| asset.eq(&(
				AssetIdForTrustBackedAssetsConvertLatest::convert_back(&local_asset_id).unwrap(),
				minimum_asset_balance
			)
				.into())));
			// check foreign asset
			assert!(result.inner().iter().any(|asset| asset.eq(&(
				WithLatestLocationConverter::convert_back(&foreign_asset_id_location).unwrap(),
				6 * foreign_asset_minimum_asset_balance
			)
				.into())));
		});
}

asset_test_utils::include_teleports_for_native_asset_works!(
	Runtime,
	AllPalletsWithoutSystem,
	XcmConfig,
	CheckingAccount,
	WeightToFee,
	ParachainSystem,
	collator_session_keys(),
	slot_durations(),
	ExistentialDeposit::get(),
	Box::new(|runtime_event_encoded: Vec<u8>| {
		match RuntimeEvent::decode(&mut &runtime_event_encoded[..]) {
			Ok(RuntimeEvent::PolkadotXcm(event)) => Some(event),
			_ => None,
		}
	}),
	1000
);

include_teleports_for_foreign_assets_works!(
	Runtime,
	AllPalletsWithoutSystem,
	XcmConfig,
	CheckingAccount,
	WeightToFee,
	ParachainSystem,
	LocationToAccountId,
	ForeignAssetsInstance,
	collator_session_keys(),
	slot_durations(),
	ExistentialDeposit::get(),
	Box::new(|runtime_event_encoded: Vec<u8>| {
		match RuntimeEvent::decode(&mut &runtime_event_encoded[..]) {
			Ok(RuntimeEvent::PolkadotXcm(event)) => Some(event),
			_ => None,
		}
	}),
	Box::new(|runtime_event_encoded: Vec<u8>| {
		match RuntimeEvent::decode(&mut &runtime_event_encoded[..]) {
			Ok(RuntimeEvent::XcmpQueue(event)) => Some(event),
			_ => None,
		}
	})
);

asset_test_utils::include_asset_transactor_transfer_with_local_consensus_currency_works!(
	Runtime,
	XcmConfig,
	collator_session_keys(),
	ExistentialDeposit::get(),
	Box::new(|| {
		assert!(Assets::asset_ids().collect::<Vec<_>>().is_empty());
		assert!(ForeignAssets::asset_ids().collect::<Vec<_>>().is_empty());
	}),
	Box::new(|| {
		assert!(Assets::asset_ids().collect::<Vec<_>>().is_empty());
		assert!(ForeignAssets::asset_ids().collect::<Vec<_>>().is_empty());
	})
);

asset_test_utils::include_asset_transactor_transfer_with_pallet_assets_instance_works!(
	asset_transactor_transfer_with_trust_backed_assets_works,
	Runtime,
	XcmConfig,
	TrustBackedAssetsInstance,
	AssetIdForTrustBackedAssets,
	AssetIdForTrustBackedAssetsConvertLatest,
	collator_session_keys(),
	ExistentialDeposit::get(),
	12345,
	Box::new(|| {
		assert!(ForeignAssets::asset_ids().collect::<Vec<_>>().is_empty());
	}),
	Box::new(|| {
		assert!(ForeignAssets::asset_ids().collect::<Vec<_>>().is_empty());
	})
);

asset_test_utils::include_asset_transactor_transfer_with_pallet_assets_instance_works!(
	asset_transactor_transfer_with_foreign_assets_works,
	Runtime,
	XcmConfig,
	ForeignAssetsInstance,
	Location,
	WithLatestLocationConverter<Location>,
	collator_session_keys(),
	ExistentialDeposit::get(),
	Location::new(1, [Parachain(1313), GeneralIndex(12345)]),
	Box::new(|| {
		assert!(Assets::asset_ids().collect::<Vec<_>>().is_empty());
	}),
	Box::new(|| {
		assert!(Assets::asset_ids().collect::<Vec<_>>().is_empty());
	})
);

include_create_and_manage_foreign_assets_for_local_consensus_parachain_assets_works!(
	Runtime,
	XcmConfig,
	WeightToFee,
	LocationToAccountId,
	ForeignAssetsInstance,
	Location,
	WithLatestLocationConverter<Location>,
	collator_session_keys(),
	ExistentialDeposit::get(),
	AssetDeposit::get(),
	MetadataDepositBase::get(),
	MetadataDepositPerByte::get(),
	Box::new(|pallet_asset_call| RuntimeCall::ForeignAssets(pallet_asset_call).encode()),
	Box::new(|runtime_event_encoded: Vec<u8>| {
		match RuntimeEvent::decode(&mut &runtime_event_encoded[..]) {
			Ok(RuntimeEvent::ForeignAssets(pallet_asset_event)) => Some(pallet_asset_event),
			_ => None,
		}
	}),
	Box::new(|| {
		assert!(Assets::asset_ids().collect::<Vec<_>>().is_empty());
		assert!(ForeignAssets::asset_ids().collect::<Vec<_>>().is_empty());
	}),
	Box::new(|| {
		assert!(Assets::asset_ids().collect::<Vec<_>>().is_empty());
		assert_eq!(ForeignAssets::asset_ids().collect::<Vec<_>>().len(), 1);
	})
);

fn bridging_to_asset_hub_kusama() -> TestBridgingConfig {
	PolkadotXcm::force_xcm_version(
		RuntimeOrigin::root(),
		Box::new(bridging::to_kusama::AssetHubKusama::get()),
		XCM_VERSION,
	)
	.expect("version saved!");
	TestBridgingConfig {
		bridged_network: bridging::to_kusama::KusamaNetwork::get(),
		local_bridge_hub_para_id: bridging::SiblingBridgeHubParaId::get(),
		local_bridge_hub_location: bridging::SiblingBridgeHub::get(),
		bridged_target_location: bridging::to_kusama::AssetHubKusama::get(),
	}
}

/* // FIXME @karol FAIL-CI
#[test]
fn limited_reserve_transfer_assets_for_native_asset_to_asset_hub_kusama_works() {
	use sp_runtime::traits::Get;

	asset_test_utils::test_cases_over_bridge::limited_reserve_transfer_assets_for_native_asset_works::<
		Runtime,
		AllPalletsWithoutSystem,
		XcmConfig,
		ParachainSystem,
		XcmpQueue,
		LocationToAccountId,
	>(
		collator_session_keys(),
		slot_durations(),
		ExistentialDeposit::get(),
		AccountId::from(ALICE),
		Box::new(|runtime_event_encoded: Vec<u8>| {
			match RuntimeEvent::decode(&mut &runtime_event_encoded[..]) {
				Ok(RuntimeEvent::PolkadotXcm(event)) => Some(event),
				_ => None,
			}
		}),
		Box::new(|runtime_event_encoded: Vec<u8>| {
			match RuntimeEvent::decode(&mut &runtime_event_encoded[..]) {
				Ok(RuntimeEvent::XcmpQueue(event)) => Some(event),
				_ => None,
			}
		}),
		bridging_to_asset_hub_kusama,
		WeightLimit::Unlimited,
		Some(XcmBridgeHubRouterFeeAssetId::get()),
		Some(TreasuryAccount::get()),
	)
} */

#[test]
fn reserve_transfer_native_asset_to_non_teleport_para_works() {
	asset_test_utils::test_cases::reserve_transfer_native_asset_to_non_teleport_para_works::<
		Runtime,
		AllPalletsWithoutSystem,
		XcmConfig,
		ParachainSystem,
		XcmpQueue,
		LocationToAccountId,
	>(
		collator_session_keys(),
		slot_durations(),
		ExistentialDeposit::get(),
		AccountId::from(ALICE),
		Box::new(|runtime_event_encoded: Vec<u8>| {
			match RuntimeEvent::decode(&mut &runtime_event_encoded[..]) {
				Ok(RuntimeEvent::PolkadotXcm(event)) => Some(event),
				_ => None,
			}
		}),
		Box::new(|runtime_event_encoded: Vec<u8>| {
			match RuntimeEvent::decode(&mut &runtime_event_encoded[..]) {
				Ok(RuntimeEvent::XcmpQueue(event)) => Some(event),
				_ => None,
			}
		}),
		WeightLimit::Unlimited,
	);
}

#[test]
fn report_bridge_status_from_xcm_bridge_router_for_kusama_works() {
	asset_test_utils::test_cases_over_bridge::report_bridge_status_from_xcm_bridge_router_works::<
		Runtime,
		AllPalletsWithoutSystem,
		XcmConfig,
		LocationToAccountId,
		ToKusamaXcmRouterInstance,
	>(
		collator_session_keys(),
		bridging_to_asset_hub_kusama,
		|| bp_asset_hub_paseo::build_congestion_message(Default::default(), true).into(),
		|| bp_asset_hub_paseo::build_congestion_message(Default::default(), false).into(),
	)
}

#[test]
fn test_report_bridge_status_call_compatibility() {
	// if this test fails, make sure `bp_asset_hub_kusama` has valid encoding
	assert_eq!(
		RuntimeCall::ToKusamaXcmRouter(pallet_xcm_bridge_hub_router::Call::report_bridge_status {
			bridge_id: Default::default(),
			is_congested: true,
		})
		.encode(),
		bp_asset_hub_paseo::Call::ToKusamaXcmRouter(
			bp_asset_hub_paseo::XcmBridgeHubRouterCall::report_bridge_status {
				bridge_id: Default::default(),
				is_congested: true,
			}
		)
		.encode()
	)
}

#[test]
fn check_sane_weight_report_bridge_status() {
	use pallet_xcm_bridge_hub_router::WeightInfo;
	let actual = <Runtime as pallet_xcm_bridge_hub_router::Config<
		ToKusamaXcmRouterInstance,
	>>::WeightInfo::report_bridge_status();
	let max_weight = bp_asset_hub_paseo::XcmBridgeHubRouterTransactCallMaxWeight::get();
	assert!(
		actual.all_lte(max_weight),
		"max_weight: {max_weight:?} should be adjusted to actual {actual:?}"
	);
}

#[test]
fn change_xcm_bridge_hub_router_base_fee_by_governance_works() {
	asset_test_utils::test_cases::change_storage_constant_by_governance_works::<
		Runtime,
		bridging::XcmBridgeHubRouterBaseFee,
		Balance,
	>(
		collator_session_keys(),
		1000,
		Governance::get(),
		|| {
			log::error!(
				target: "bridges::estimate",
				"`bridging::XcmBridgeHubRouterBaseFee` actual value: {} for runtime: {}",
				bridging::XcmBridgeHubRouterBaseFee::get(),
				<Runtime as frame_system::Config>::Version::get(),
			);
			(
				bridging::XcmBridgeHubRouterBaseFee::key().to_vec(),
				bridging::XcmBridgeHubRouterBaseFee::get(),
			)
		},
		|old_value| {
			if let Some(new_value) = old_value.checked_add(1) {
				new_value
			} else {
				old_value.checked_sub(1).unwrap()
			}
		},
	)
}

#[test]
fn change_xcm_bridge_hub_router_byte_fee_by_governance_works() {
	asset_test_utils::test_cases::change_storage_constant_by_governance_works::<
		Runtime,
		bridging::XcmBridgeHubRouterByteFee,
		Balance,
	>(
		collator_session_keys(),
		1000,
		Governance::get(),
		|| {
			(
				bridging::XcmBridgeHubRouterByteFee::key().to_vec(),
				bridging::XcmBridgeHubRouterByteFee::get(),
			)
		},
		|old_value| {
			if let Some(new_value) = old_value.checked_add(1) {
				new_value
			} else {
				old_value.checked_sub(1).unwrap()
			}
		},
	)
}

#[test]
fn change_xcm_bridge_hub_ethereum_base_fee_by_governance_works() {
	asset_test_utils::test_cases::change_storage_constant_by_governance_works::<
		Runtime,
		bridging::to_ethereum::BridgeHubEthereumBaseFee,
		Balance,
	>(
		collator_session_keys(),
		1000,
		Governance::get(),
		|| {
			(
				bridging::to_ethereum::BridgeHubEthereumBaseFee::key().to_vec(),
				bridging::to_ethereum::BridgeHubEthereumBaseFee::get(),
			)
		},
		|old_value| {
			if let Some(new_value) = old_value.checked_add(1) {
				new_value
			} else {
				old_value.checked_sub(1).unwrap()
			}
		},
	)
}

#[test]
fn location_conversion_works() {
	let alice_32 =
		AccountId32 { network: None, id: polkadot_core_primitives::AccountId::from(ALICE).into() };
	let bob_20 = AccountKey20 { network: None, key: [123u8; 20] };

	// the purpose of hardcoded values is to catch an unintended location conversion logic change.
	struct TestCase {
		description: &'static str,
		location: Location,
		expected_account_id_str: &'static str,
	}

	let test_cases = vec![
		// DescribeTerminus
		TestCase {
			description: "DescribeTerminus Parent",
			location: Location::new(1, Here),
			expected_account_id_str: "5Dt6dpkWPwLaH4BBCKJwjiWrFVAGyYk3tLUabvyn4v7KtESG",
		},
		TestCase {
			description: "DescribeTerminus Sibling",
			location: Location::new(1, [Parachain(1111)]),
			expected_account_id_str: "5Eg2fnssmmJnF3z1iZ1NouAuzciDaaDQH7qURAy3w15jULDk",
		},
		// DescribePalletTerminal
		TestCase {
			description: "DescribePalletTerminal Parent",
			location: Location::new(1, [PalletInstance(50)]),
			expected_account_id_str: "5CnwemvaAXkWFVwibiCvf2EjqwiqBi29S5cLLydZLEaEw6jZ",
		},
		TestCase {
			description: "DescribePalletTerminal Sibling",
			location: Location::new(1, [Parachain(1111), PalletInstance(50)]),
			expected_account_id_str: "5GFBgPjpEQPdaxEnFirUoa51u5erVx84twYxJVuBRAT2UP2g",
		},
		// DescribeAccountId32Terminal
		TestCase {
			description: "DescribeAccountId32Terminal Parent",
			location: Location::new(1, [alice_32]),
			expected_account_id_str: "5DN5SGsuUG7PAqFL47J9meViwdnk9AdeSWKFkcHC45hEzVz4",
		},
		TestCase {
			description: "DescribeAccountId32Terminal Sibling",
			location: Location::new(1, [Parachain(1111), alice_32]),
			expected_account_id_str: "5DGRXLYwWGce7wvm14vX1Ms4Vf118FSWQbJkyQigY2pfm6bg",
		},
		// DescribeAccountKey20Terminal
		TestCase {
			description: "DescribeAccountKey20Terminal Parent",
			location: Location::new(1, [bob_20]),
			expected_account_id_str: "5CJeW9bdeos6EmaEofTUiNrvyVobMBfWbdQvhTe6UciGjH2n",
		},
		TestCase {
			description: "DescribeAccountKey20Terminal Sibling",
			location: Location::new(1, [Parachain(1111), bob_20]),
			expected_account_id_str: "5CE6V5AKH8H4rg2aq5KMbvaVUDMumHKVPPQEEDMHPy3GmJQp",
		},
		// DescribeTreasuryVoiceTerminal
		TestCase {
			description: "DescribeTreasuryVoiceTerminal Parent",
			location: Location::new(1, [Plurality { id: BodyId::Treasury, part: BodyPart::Voice }]),
			expected_account_id_str: "5CUjnE2vgcUCuhxPwFoQ5r7p1DkhujgvMNDHaF2bLqRp4D5F",
		},
		TestCase {
			description: "DescribeTreasuryVoiceTerminal Sibling",
			location: Location::new(
				1,
				[Parachain(1111), Plurality { id: BodyId::Treasury, part: BodyPart::Voice }],
			),
			expected_account_id_str: "5G6TDwaVgbWmhqRUKjBhRRnH4ry9L9cjRymUEmiRsLbSE4gB",
		},
		// DescribeBodyTerminal
		TestCase {
			description: "DescribeBodyTerminal Parent",
			location: Location::new(1, [Plurality { id: BodyId::Unit, part: BodyPart::Voice }]),
			expected_account_id_str: "5EBRMTBkDisEXsaN283SRbzx9Xf2PXwUxxFCJohSGo4jYe6B",
		},
		TestCase {
			description: "DescribeBodyTerminal Sibling",
			location: Location::new(
				1,
				[Parachain(1111), Plurality { id: BodyId::Unit, part: BodyPart::Voice }],
			),
			expected_account_id_str: "5DBoExvojy8tYnHgLL97phNH975CyT45PWTZEeGoBZfAyRMH",
		},
	];

	for tc in test_cases {
		let expected = polkadot_core_primitives::AccountId::from_string(tc.expected_account_id_str)
			.expect("Invalid AccountId string");

		let got = LocationToAccountHelper::<polkadot_core_primitives::AccountId, LocationToAccountId>::convert_location(
			tc.location.into(),
		)
			.unwrap();

		assert_eq!(got, expected, "{}", tc.description);
	}
}

#[test]
fn xcm_payment_api_works() {
	parachains_runtimes_test_utils::test_cases::xcm_payment_api_with_native_token_works::<
		Runtime,
		RuntimeCall,
		RuntimeOrigin,
		Block,
		WeightToFee,
	>();
	asset_test_utils::test_cases::xcm_payment_api_with_pools_works::<
		Runtime,
		RuntimeCall,
		RuntimeOrigin,
		Block,
		WeightToFee,
	>();
	asset_test_utils::test_cases::xcm_payment_api_foreign_asset_pool_works::<
		Runtime,
		RuntimeCall,
		RuntimeOrigin,
		LocationToAccountId,
		Block,
		WeightToFee,
	>(ExistentialDeposit::get(), WESTEND_GENESIS_HASH);
}

#[test]
fn test_xcm_v4_to_v5_works() {
	// Test some common XCM location patterns to ensure V4 -> V5 compatibility
	let test_locations_v4 = vec![
		// Relay chain
		xcm::v4::Location::new(1, xcm::v4::Junctions::Here),
		// Sibling parachain
		xcm::v4::Location::new(1, [xcm::v4::Junction::Parachain(1000)]),
		// Asset on sibling parachain
		xcm::v4::Location::new(
			1,
			[
				xcm::v4::Junction::Parachain(1000),
				xcm::v4::Junction::PalletInstance(50),
				xcm::v4::Junction::GeneralIndex(1984),
			],
		),
		// Global consensus location
		xcm::v4::Location::new(
			1,
			[xcm::v4::Junction::GlobalConsensus(xcm::v4::NetworkId::Polkadot)],
		),
	];

	for v4_location in test_locations_v4 {
		// Test V4 -> V5 conversion
		let v5_location = xcm::v5::Location::try_from(v4_location.clone())
			.map_err(|_| TryRuntimeError::Other("Failed to convert V4 location to V5"))
			.unwrap();

		// Test that we can encode/decode V5 location
		let encoded = v5_location.encode();
		let decoded = xcm::v5::Location::decode(&mut &encoded[..])
			.map_err(|_| TryRuntimeError::Other("Failed to decode V5 location"))
			.unwrap();

		assert_eq!(v5_location, decoded, "V5 location encode/decode round-trip failed");

		// Test V4 encoded -> V5 decoded compatibility
		let encoded_v4 = v4_location.encode();
		let decoded_v5 = xcm::v5::Location::decode(&mut &encoded_v4[..])
			.map_err(|_| TryRuntimeError::Other("Failed to decode V4 encoded location as V5"))
			.unwrap();

		// try-from is compatible
		assert_eq!(
			decoded_v5, v5_location,
			"V4 encoded -> V5 decoded should match try_from conversion"
		);

		// encode/decode is compatible
		assert_eq!(encoded_v4, decoded_v5.encode(), "V4 encoded should match V5 re-encoded");
	}
}

#[test]
fn authorized_aliases_work() {
	ExtBuilder::<Runtime>::default()
		.with_tracing()
		.with_collators(vec![AccountId::from(ALICE)])
		.with_session_keys(vec![(
			AccountId::from(ALICE),
			AccountId::from(ALICE),
			SessionKeys { aura: AuraId::from(sp_core::sr25519::Public::from_raw(ALICE)) },
		)])
		.build()
		.execute_with(|| {
			use frame_support::traits::fungible::Mutate;
			let alice: AccountId = ALICE.into();
			let local_alice = Location::new(0, AccountId32 { network: None, id: ALICE });
			let alice_on_sibling_para =
				Location::new(1, [Parachain(42), AccountId32 { network: None, id: ALICE }]);
			let alice_on_relay = Location::new(1, AccountId32 { network: None, id: ALICE });
			let bob_on_relay = Location::new(1, AccountId32 { network: None, id: [42_u8; 32] });

			assert_ok!(Balances::mint_into(&alice, 2 * UNITS));

			// neither `alice_on_sibling_para`, `alice_on_relay`, `bob_on_relay` are allowed to
			// alias into `local_alice`
			for aliaser in [&alice_on_sibling_para, &alice_on_relay, &bob_on_relay] {
				assert!(!<XcmConfig as xcm_executor::Config>::Aliasers::contains(
					aliaser,
					&local_alice
				));
			}

			// Alice explicitly authorizes `alice_on_sibling_para` to alias her local account
			assert_ok!(PolkadotXcm::add_authorized_alias(
				RuntimeHelper::origin_of(alice.clone()),
				Box::new(alice_on_sibling_para.clone().into()),
				None
			));

			// `alice_on_sibling_para` now explicitly allowed to alias into `local_alice`
			assert!(<XcmConfig as xcm_executor::Config>::Aliasers::contains(
				&alice_on_sibling_para,
				&local_alice
			));
			// as expected, `alice_on_relay` and `bob_on_relay` still can't alias into `local_alice`
			for aliaser in [&alice_on_relay, &bob_on_relay] {
				assert!(!<XcmConfig as xcm_executor::Config>::Aliasers::contains(
					aliaser,
					&local_alice
				));
			}

			// Alice explicitly authorizes `alice_on_relay` to alias her local account
			assert_ok!(PolkadotXcm::add_authorized_alias(
				RuntimeHelper::origin_of(alice.clone()),
				Box::new(alice_on_relay.clone().into()),
				None
			));
			// Now both `alice_on_relay` and `alice_on_sibling_para` can alias into her local
			// account
			for aliaser in [&alice_on_relay, &alice_on_sibling_para] {
				assert!(<XcmConfig as xcm_executor::Config>::Aliasers::contains(
					aliaser,
					&local_alice
				));
			}

			// Alice removes authorization for `alice_on_relay` to alias her local account
			assert_ok!(PolkadotXcm::remove_authorized_alias(
				RuntimeHelper::origin_of(alice.clone()),
				Box::new(alice_on_relay.clone().into())
			));

			// `alice_on_relay` no longer allowed to alias into `local_alice`
			assert!(!<XcmConfig as xcm_executor::Config>::Aliasers::contains(
				&alice_on_relay,
				&local_alice
			));

			// `alice_on_sibling_para` still allowed to alias into `local_alice`
			assert!(<XcmConfig as xcm_executor::Config>::Aliasers::contains(
				&alice_on_sibling_para,
				&local_alice
			));
		})
}

#[test]
fn governance_authorize_upgrade_works() {
	use paseo_runtime_constants::system_parachain::{ASSET_HUB_ID, COLLECTIVES_ID};

	// no - random non-system para
	assert_err!(
		parachains_runtimes_test_utils::test_cases::can_governance_authorize_upgrade::<
			Runtime,
			RuntimeOrigin,
		>(GovernanceOrigin::Location(Location::new(1, Parachain(12334)))),
		Either::Right(InstructionError { index: 0, error: XcmError::Barrier })
	);
	// no - random system para
	assert_err!(
		parachains_runtimes_test_utils::test_cases::can_governance_authorize_upgrade::<
			Runtime,
			RuntimeOrigin,
		>(GovernanceOrigin::Location(Location::new(1, Parachain(1765)))),
		Either::Right(InstructionError { index: 1, error: XcmError::BadOrigin })
	);
	// no - AssetHub
	assert_err!(
		parachains_runtimes_test_utils::test_cases::can_governance_authorize_upgrade::<
			Runtime,
			RuntimeOrigin,
		>(GovernanceOrigin::Location(Location::new(1, Parachain(ASSET_HUB_ID)))),
		Either::Right(InstructionError { index: 1, error: XcmError::BadOrigin })
	);
	// no - Collectives
	assert_err!(
		parachains_runtimes_test_utils::test_cases::can_governance_authorize_upgrade::<
			Runtime,
			RuntimeOrigin,
		>(GovernanceOrigin::Location(Location::new(1, Parachain(COLLECTIVES_ID)))),
		Either::Right(InstructionError { index: 1, error: XcmError::BadOrigin })
	);
	// no - Collectives Voice of Fellows plurality
	assert_err!(
		parachains_runtimes_test_utils::test_cases::can_governance_authorize_upgrade::<
			Runtime,
			RuntimeOrigin,
		>(GovernanceOrigin::LocationAndDescendOrigin(
			Location::new(1, Parachain(COLLECTIVES_ID)),
			Plurality { id: BodyId::Technical, part: BodyPart::Voice }.into()
		)),
		Either::Right(InstructionError { index: 2, error: XcmError::BadOrigin })
	);

	// ok - relaychain
	assert_ok!(parachains_runtimes_test_utils::test_cases::can_governance_authorize_upgrade::<
		Runtime,
		RuntimeOrigin,
	>(GovernanceOrigin::Location(RelayChainLocation::get())));
}

/// A Staking proxy can add/remove a StakingOperator proxy for the account it is proxying.
#[test]
fn staking_proxy_can_manage_staking_operator() {
	use asset_hub_paseo_runtime::{Proxy, ProxyType};
	use frame_support::traits::fungible::Mutate;
	use sp_runtime::traits::StaticLookup;

	ExtBuilder::<Runtime>::default()
		.with_collators(vec![AccountId::from(ALICE)])
		.with_session_keys(vec![(
			AccountId::from(ALICE),
			AccountId::from(ALICE),
			SessionKeys { aura: AuraId::from(sp_core::sr25519::Public::from_raw(ALICE)) },
		)])
		.build()
		.execute_with(|| {
			// Given: Alice, Bob, and Carol with sufficient balance
			let alice: AccountId = ALICE.into();
			let bob: AccountId = [2u8; 32].into();
			let carol: AccountId = [3u8; 32].into();

			Balances::mint_into(&alice, 100 * UNITS).unwrap();
			Balances::mint_into(&bob, 100 * UNITS).unwrap();
			Balances::mint_into(&carol, 100 * UNITS).unwrap();

			// Given: Alice has Bob as her Staking proxy
			assert_ok!(Proxy::add_proxy(
				RuntimeOrigin::signed(alice.clone()),
				<Runtime as frame_system::Config>::Lookup::unlookup(bob.clone()),
				ProxyType::Staking,
				0
			));

			// When: Bob (via proxy) adds Carol as StakingOperator for Alice
			let add_call = RuntimeCall::Proxy(pallet_proxy::Call::add_proxy {
				delegate: <Runtime as frame_system::Config>::Lookup::unlookup(carol.clone()),
				proxy_type: ProxyType::StakingOperator,
				delay: 0,
			});
			assert_ok!(Proxy::proxy(
				RuntimeOrigin::signed(bob.clone()),
				<Runtime as frame_system::Config>::Lookup::unlookup(alice.clone()),
				None,
				Box::new(add_call)
			));

			// Then: Carol is Alice's StakingOperator proxy
			let alice_proxies = pallet_proxy::Proxies::<Runtime>::get(&alice);
			assert!(
				alice_proxies
					.0
					.iter()
					.any(|p| p.delegate == carol && p.proxy_type == ProxyType::StakingOperator),
				"Carol should be Alice's StakingOperator proxy"
			);

			// When: Bob tries to add an Any proxy for Alice
			let add_any_call = RuntimeCall::Proxy(pallet_proxy::Call::add_proxy {
				delegate: <Runtime as frame_system::Config>::Lookup::unlookup(carol.clone()),
				proxy_type: ProxyType::Any,
				delay: 0,
			});
			// proxy() returns Ok(()) but inner call result is in ProxyExecuted event
			assert_ok!(Proxy::proxy(
				RuntimeOrigin::signed(bob.clone()),
				<Runtime as frame_system::Config>::Lookup::unlookup(alice.clone()),
				None,
				Box::new(add_any_call),
			));

			// Then: The ProxyExecuted event should contain CallFiltered error
			let events = frame_system::Pallet::<Runtime>::events();
			let proxy_executed = events.iter().rev().find_map(|record| {
				if let RuntimeEvent::Proxy(pallet_proxy::Event::ProxyExecuted { result }) =
					&record.event
				{
					Some(*result)
				} else {
					None
				}
			});
			assert_eq!(
				proxy_executed,
				Some(Err(frame_system::Error::<Runtime>::CallFiltered.into())),
				"Inner call should fail with CallFiltered"
			);

			// And: Carol was NOT added as Any proxy
			let alice_proxies = pallet_proxy::Proxies::<Runtime>::get(&alice);
			assert!(
				!alice_proxies
					.0
					.iter()
					.any(|p| p.delegate == carol && p.proxy_type == ProxyType::Any),
				"Carol should NOT be Alice's Any proxy - Staking proxy cannot add Any"
			);

			// When: Bob (via proxy) removes Carol as StakingOperator for Alice
			let remove_call = RuntimeCall::Proxy(pallet_proxy::Call::remove_proxy {
				delegate: <Runtime as frame_system::Config>::Lookup::unlookup(carol.clone()),
				proxy_type: ProxyType::StakingOperator,
				delay: 0,
			});
			assert_ok!(Proxy::proxy(
				RuntimeOrigin::signed(bob.clone()),
				<Runtime as frame_system::Config>::Lookup::unlookup(alice.clone()),
				None,
				Box::new(remove_call)
			));

			// Then: Carol is no longer Alice's StakingOperator proxy
			let alice_proxies = pallet_proxy::Proxies::<Runtime>::get(&alice);
			assert!(
				!alice_proxies
					.0
					.iter()
					.any(|p| p.delegate == carol && p.proxy_type == ProxyType::StakingOperator),
				"Carol should no longer be Alice's StakingOperator proxy"
			);
		});
}

/// Verifies StakingOperator filter allows validator operations and session key management,
/// but forbids fund management.
#[test]
fn staking_operator_filter_allows_validator_ops_and_session_keys() {
	use asset_hub_paseo_runtime::ProxyType;
	use frame_support::traits::InstanceFilter;
	use pallet_staking_async::{Call as StakingCall, RewardDestination, ValidatorPrefs};
	use pallet_staking_async_rc_client::Call as RcClientCall;

	let operator = ProxyType::StakingOperator;

	// StakingOperator can perform validator operations
	assert!(operator
		.filter(&RuntimeCall::Staking(StakingCall::validate { prefs: ValidatorPrefs::default() })));
	assert!(operator.filter(&RuntimeCall::Staking(StakingCall::chill {})));
	assert!(operator.filter(&RuntimeCall::Staking(StakingCall::kick { who: vec![] })));

	// StakingOperator can manage session keys
	assert!(operator.filter(&RuntimeCall::StakingRcClient(RcClientCall::set_keys {
		keys: Default::default(),
		proof: Default::default(),
		max_delivery_and_remote_execution_fee: None,
	})));
	assert!(operator.filter(&RuntimeCall::StakingRcClient(RcClientCall::purge_keys {
		max_delivery_and_remote_execution_fee: None,
	})));

	// StakingOperator can batch operations
	assert!(operator.filter(&RuntimeCall::Utility(pallet_utility::Call::batch { calls: vec![] })));
	assert!(
		operator.filter(&RuntimeCall::Utility(pallet_utility::Call::batch_all { calls: vec![] }))
	);
	assert!(
		operator.filter(&RuntimeCall::Utility(pallet_utility::Call::force_batch { calls: vec![] }))
	);

	// StakingOperator cannot use other utility calls
	assert!(!operator.filter(&RuntimeCall::Utility(pallet_utility::Call::as_derivative {
		index: 0,
		call: Box::new(RuntimeCall::System(frame_system::Call::remark { remark: vec![] })),
	})));
	assert!(!operator.filter(&RuntimeCall::Utility(pallet_utility::Call::dispatch_as {
		as_origin: Box::new(asset_hub_paseo_runtime::OriginCaller::system(
			frame_system::RawOrigin::Root,
		)),
		call: Box::new(RuntimeCall::System(frame_system::Call::remark { remark: vec![] })),
	})));
	assert!(!operator.filter(&RuntimeCall::Utility(pallet_utility::Call::with_weight {
		call: Box::new(RuntimeCall::System(frame_system::Call::remark { remark: vec![] })),
		weight: Default::default(),
	})));

	// StakingOperator cannot manage funds or nominations
	assert!(!operator.filter(&RuntimeCall::Staking(StakingCall::bond {
		value: 100,
		payee: RewardDestination::Staked
	})));
	assert!(!operator.filter(&RuntimeCall::Staking(StakingCall::unbond { value: 100 })));
	assert!(!operator.filter(&RuntimeCall::Staking(StakingCall::nominate { targets: vec![] })));
	assert!(!operator
		.filter(&RuntimeCall::Staking(StakingCall::update_payee { controller: [0u8; 32].into() })));
}

/// Test that a pure proxy stash can delegate to a StakingOperator
/// who can then call validate, chill, and manage session keys.
#[test]
fn pure_proxy_stash_can_delegate_to_staking_operator() {
	use asset_hub_paseo_runtime::ProxyType;

	let controller: AccountId = ALICE.into();
	let operator: AccountId = [2u8; 32].into();

	ExtBuilder::<Runtime>::default()
		.with_collators(vec![AccountId::from(ALICE)])
		.with_session_keys(vec![(
			AccountId::from(ALICE),
			AccountId::from(ALICE),
			SessionKeys { aura: AuraId::from(sp_core::sr25519::Public::from_raw(ALICE)) },
		)])
		.build()
		.execute_with(|| {
			use frame_support::traits::fungible::Mutate;

			// GIVEN: fund controller and operator
			assert_ok!(Balances::mint_into(&controller, 100 * UNITS));
			assert_ok!(Balances::mint_into(&operator, 100 * UNITS));

			// WHEN: controller creates a pure proxy stash with Staking proxy type
			assert_ok!(asset_hub_paseo_runtime::Proxy::create_pure(
				RuntimeOrigin::signed(controller.clone()),
				ProxyType::Staking,
				0,
				0
			));
			let pure_stash = asset_hub_paseo_runtime::Proxy::pure_account(
				&controller,
				&ProxyType::Staking,
				0,
				None,
			);

			// Fund the pure proxy stash
			assert_ok!(Balances::mint_into(&pure_stash, 100 * UNITS));

			// WHEN: controller (via Staking proxy) adds StakingOperator proxy for the operator
			let add_operator_call = RuntimeCall::Proxy(pallet_proxy::Call::add_proxy {
				delegate: operator.clone().into(),
				proxy_type: ProxyType::StakingOperator,
				delay: 0,
			});
			assert_ok!(asset_hub_paseo_runtime::Proxy::proxy(
				RuntimeOrigin::signed(controller.clone()),
				pure_stash.clone().into(),
				None,
				Box::new(add_operator_call),
			));

			// THEN: operator can call chill on behalf of pure proxy stash
			let chill_call = RuntimeCall::Staking(pallet_staking_async::Call::chill {});
			assert_ok!(asset_hub_paseo_runtime::Proxy::proxy(
				RuntimeOrigin::signed(operator.clone()),
				pure_stash.clone().into(),
				None,
				Box::new(chill_call),
			));

			// THEN: operator can call validate on behalf of pure proxy stash
			let validate_call = RuntimeCall::Staking(pallet_staking_async::Call::validate {
				prefs: Default::default(),
			});
			assert_ok!(asset_hub_paseo_runtime::Proxy::proxy(
				RuntimeOrigin::signed(operator.clone()),
				pure_stash.clone().into(),
				None,
				Box::new(validate_call),
			));

			// THEN: operator can call purge_keys (session key management on AssetHub)
			let purge_keys_call =
				RuntimeCall::StakingRcClient(pallet_staking_async_rc_client::Call::purge_keys {
					max_delivery_and_remote_execution_fee: None,
				});
			assert_ok!(asset_hub_paseo_runtime::Proxy::proxy(
				RuntimeOrigin::signed(operator.clone()),
				pure_stash.clone().into(),
				None,
				Box::new(purge_keys_call),
			));

			// THEN: operator CANNOT call bond (fund management is forbidden)
			let bond_call = RuntimeCall::Staking(pallet_staking_async::Call::bond {
				value: 10 * UNITS,
				payee: pallet_staking_async::RewardDestination::Staked,
			});
			assert_ok!(asset_hub_paseo_runtime::Proxy::proxy(
				RuntimeOrigin::signed(operator.clone()),
				pure_stash.clone().into(),
				None,
				Box::new(bond_call),
			));
			// Check that the proxied call failed due to filter (CallFiltered error)
			frame_system::Pallet::<Runtime>::assert_last_event(
				pallet_proxy::Event::ProxyExecuted {
					result: Err(frame_system::Error::<Runtime>::CallFiltered.into()),
				}
				.into(),
			);
		});
}

#[test]
fn slash_goes_to_dap_buffer_account() {
	use frame_support::traits::{
		fungible::{Balanced, Inspect},
		Hooks, OnUnbalanced,
	};
	use sp_runtime::BuildStorage;

	let dap_buffer = pallet_dap::Pallet::<Runtime>::buffer_account();
	let dap_staging = pallet_dap::Pallet::<Runtime>::staging_account();
	let ed = ExistentialDeposit::get();

	let mut t = frame_system::GenesisConfig::<Runtime>::default().build_storage().unwrap();
	pallet_balances::GenesisConfig::<Runtime> {
		balances: vec![
			(AccountId::from(ALICE), 1_000 * UNITS),
			(dap_buffer.clone(), ed),
			(dap_staging.clone(), ed),
		],
		..Default::default()
	}
	.assimilate_storage(&mut t)
	.unwrap();

	sp_io::TestExternalities::from(t).execute_with(|| {
		let dap_buffer_before = <Balances as Inspect<_>>::balance(&dap_buffer);
		let dap_staging_before = <Balances as Inspect<_>>::balance(&dap_staging);

		// When: a slash occurs (simulating staking slash via OnUnbalanced)
		let slash_amount = 100 * UNITS;
		let credit = <Balances as Balanced<AccountId>>::issue(slash_amount);
		Dap::on_unbalanced(credit);

		// Slash lands in staging first, not directly in the buffer.
		assert_eq!(
			<Balances as Inspect<_>>::balance(&dap_staging),
			dap_staging_before + slash_amount
		);
		assert_eq!(<Balances as Inspect<_>>::balance(&dap_buffer), dap_buffer_before);

		// on_idle drains staging into buffer and deactivates.
		pallet_dap::Pallet::<Runtime>::on_idle(1, Weight::MAX);
		assert_eq!(<Balances as Inspect<_>>::balance(&dap_staging), dap_staging_before);
		assert_eq!(
			<Balances as Inspect<_>>::balance(&dap_buffer),
			dap_buffer_before + slash_amount
		);

		// When: another slash occurs
		let slash_amount_2 = 50 * UNITS;
		let credit2 = <Balances as Balanced<AccountId>>::issue(slash_amount_2);
		Dap::on_unbalanced(credit2);
		// Lands in staging again.
		assert_eq!(
			<Balances as Inspect<_>>::balance(&dap_staging),
			dap_staging_before + slash_amount_2
		);
		assert_eq!(
			<Balances as Inspect<_>>::balance(&dap_buffer),
			dap_buffer_before + slash_amount
		);

		// After on_idle: staging drains into buffer and deactivates.
		pallet_dap::Pallet::<Runtime>::on_idle(2, Weight::MAX);
		assert_eq!(<Balances as Inspect<_>>::balance(&dap_staging), dap_staging_before);
		assert_eq!(
			<Balances as Inspect<_>>::balance(&dap_buffer),
			dap_buffer_before + slash_amount + slash_amount_2
		);
	});
}

#[test]
fn migrate_bounty_account_assets_moves_native_and_leaves_the_rest() {
	use asset_hub_paseo_runtime::{
		migrations::MigrateBountyAccountAssets, treasury::TreasuryPalletId,
	};
	use frame_support::traits::{
		fungible::Mutate as _, tokens::fungibles::Mutate as _, Get as _, OnRuntimeUpgrade,
		PalletInfoAccess,
	};
	use pallet_multi_asset_bounties::{Bounty, BountyStatus};
	use polkadot_runtime_common::impls::VersionedLocatableAsset;
	use sp_core::H256;
	use sp_runtime::traits::AccountIdConversion;

	const BOUNTY_ID: u32 = 7;
	const USDT_ASSET_ID: u32 = 1984;
	const USDC_ASSET_ID: u32 = 1337;
	const UNRELATED_ASSET_ID: u32 = 99; // NOT used by multi-asset bounties, must NOT move
	const DOT_AMOUNT: Balance = 100 * UNITS;
	const USDT_AMOUNT: Balance = 250_000 * 1_000_000; // 6 decimals
	const USDC_AMOUNT: Balance = 100_000 * 1_000_000; // 6 decimals
	const UNRELATED_AMOUNT: Balance = 999 * 1_000_000;

	ExtBuilder::<Runtime>::default().build().execute_with(|| {
		let bounty = Bounty::<
			AccountId,
			Balance,
			VersionedLocatableAsset,
			H256,
			xcm::v5::QueryId,
			parachains_common::pay::VersionedLocatableAccount,
		> {
			asset_kind: VersionedLocatableAsset::V5 {
				location: Location::new(0, []),
				asset_id: AssetId(Location::new(
					0,
					[
						PalletInstance(<Assets as PalletInfoAccess>::index() as u8),
						GeneralIndex(USDT_ASSET_ID.into()),
					],
				)),
			},
			value: USDT_AMOUNT,
			metadata: H256::zero(),
			status: BountyStatus::CuratorUnassigned,
		};
		pallet_multi_asset_bounties::Bounties::<Runtime>::insert(BOUNTY_ID, bounty);

		let pallet_id = TreasuryPalletId::get();
		let old: AccountId = pallet_id.into_sub_account_truncating(("mbt", BOUNTY_ID));
		let new: AccountId = pallet_id.into_sub_account_truncating((
			pallet_multi_asset_bounties::BountyAccountPrefix::get(),
			BOUNTY_ID,
		));
		assert_ne!(old, new);

		// Native DOT (Balances).
		Balances::mint_into(&old, DOT_AMOUNT).unwrap();
		// Trust-backed assets the multi-asset bounties pallet supports.
		for id in [USDT_ASSET_ID, USDC_ASSET_ID, UNRELATED_ASSET_ID] {
			assert_ok!(Assets::force_create(
				RuntimeHelper::root_origin(),
				id.into(),
				AccountId::from(SOME_ASSET_ADMIN).into(),
				true,
				1,
			));
		}
		Assets::mint_into(USDT_ASSET_ID, &old, USDT_AMOUNT).unwrap();
		Assets::mint_into(USDC_ASSET_ID, &old, USDC_AMOUNT).unwrap();
		Assets::mint_into(UNRELATED_ASSET_ID, &old, UNRELATED_AMOUNT).unwrap();

		MigrateBountyAccountAssets::on_runtime_upgrade();

		// Native PAS moves: it is the only entry in `BountyRelevantAssets`.
		assert_eq!(Balances::free_balance(&old), 0);
		assert_eq!(Balances::free_balance(&new), DOT_AMOUNT);
		// USDT and USDC do NOT move here, unlike on Polkadot Asset Hub: Paseo deliberately
		// lists native only in `treasury::BountyRelevantAssets` ("Paseo testnet only uses
		// native PAS"), and `TransferAllFungibles` sweeps exactly that list. If those assets
		// are ever added to the list, these two pairs flip to the `&new` account.
		assert_eq!(Assets::balance(USDT_ASSET_ID, &old), USDT_AMOUNT);
		assert_eq!(Assets::balance(USDT_ASSET_ID, &new), 0);
		assert_eq!(Assets::balance(USDC_ASSET_ID, &old), USDC_AMOUNT);
		assert_eq!(Assets::balance(USDC_ASSET_ID, &new), 0);
		// An asset not used by the multi-asset bounties pallet stays at the old account.
		assert_eq!(Assets::balance(UNRELATED_ASSET_ID, &old), UNRELATED_AMOUNT);
		assert_eq!(Assets::balance(UNRELATED_ASSET_ID, &new), 0);
	});
}

#[test]
fn session_keys_are_compatible_between_ah_and_rc() {
	use asset_hub_paseo_runtime::staking::RelayChainSessionKeys;
	use sp_runtime::traits::OpaqueKeys;

	// Verify the key type IDs match in order.
	// This ensures that when keys are encoded on AssetHub and decoded on Paseo (or vice versa),
	// they map to the correct key types.
	assert_eq!(
		RelayChainSessionKeys::key_ids(),
		paseo_runtime::SessionKeys::key_ids(),
		"Session key type IDs must match between AssetHub and Paseo"
	);
}

mod pgas_fees {
	use asset_hub_paseo_runtime::{
		Assets, Balances, Executive, ExistentialDeposit, PgasAssetId, PgasMinBalance, Runtime,
		RuntimeCall, RuntimeEvent, SessionKeys, System, TxExtension, UncheckedExtrinsic,
	};
	use codec::Encode;
	use frame_support::{
		assert_ok,
		dispatch::GetDispatchInfo,
		traits::{
			fungible::Inspect as FungibleInspect,
			fungibles::{Inspect as FungiblesInspect, Mutate as FungiblesMutate},
			SignedTransactionBuilder,
		},
	};
	use parachains_common::{AccountId, AuraId};
	use paseo_runtime_constants::system_parachain::ASSET_HUB_ID;
	use sp_keyring::Sr25519Keyring;
	use sp_runtime::{
		generic,
		transaction_validity::{InvalidTransaction, TransactionValidityError},
		MultiSignature,
	};

	use asset_test_utils::ExtBuilder;

	use super::ALICE;

	/// Builds a signed extrinsic whose `ChargePGAS` has the PGAS path enabled. The `dap` module's
	/// helper cannot be reused: it goes through `EthExtraImpl::get_eth_extension`, which
	/// constructs `ChargePGAS` with `new_skip_pgas`.
	fn construct_extrinsic(sender: Sr25519Keyring, call: RuntimeCall) -> UncheckedExtrinsic {
		let account_id = AccountId::from(sender.public());
		let nonce = frame_system::Pallet::<Runtime>::account(&account_id).nonce;
		let tx_ext = TxExtension::from((
			(
				(),
				indiv_pallet_scarcity::extension::AsScarcity::<Runtime>::new(None),
				frame_system::AuthorizeCall::<Runtime>::new(),
				indiv_pallet_pgas::AsPgas::<Runtime>::new(None),
				indiv_pallet_dotns_gateway::AsDotnsGateway::<Runtime>::new(None),
			),
			indiv_pallet_origin_restriction::RestrictOrigin::<Runtime>::new(true),
			frame_system::CheckNonZeroSender::<Runtime>::new(),
			frame_system::CheckSpecVersion::<Runtime>::new(),
			frame_system::CheckTxVersion::<Runtime>::new(),
			frame_system::CheckGenesis::<Runtime>::new(),
			frame_system::CheckEra::<Runtime>::from(generic::Era::Immortal),
			frame_system::CheckNonce::<Runtime>::from(nonce),
			frame_system::CheckWeight::<Runtime>::new(),
			pallet_pgas_allowance::ChargePGAS::<
				Runtime,
				pallet_asset_conversion_tx_payment::ChargeAssetTxPayment<Runtime>,
			>::from(pallet_asset_conversion_tx_payment::ChargeAssetTxPayment::<Runtime>::from(
				0, None,
			)),
			(
				polkadot_runtime_common::claims::PrevalidateAttests::<Runtime>::new(),
				frame_metadata_hash_extension::CheckMetadataHash::<Runtime>::new(false),
				pallet_revive::evm::tx_extension::SetOrigin::<Runtime>::default(),
			),
		));
		let payload = generic::SignedPayload::new(call.clone(), tx_ext.clone()).unwrap();
		let signature = payload.using_encoded(|e| sender.sign(e));
		UncheckedExtrinsic::new_signed_transaction(
			call,
			account_id.into(),
			MultiSignature::Sr25519(signature),
			tx_ext,
		)
	}

	/// Paseo deviation from upstream `next-asset-hub-paseo`: `PGASCallFilter` lets only `Revive`
	/// calls (and utility batches of them) be paid in PGAS. A Revive call from a signer holding
	/// nothing but PGAS goes through; the same signer's `remark` is refused as unpayable, and a
	/// signer one planck short of the fee in PGAS is refused too.
	#[test]
	fn pgas_pays_the_fee_of_revive_calls_only() {
		let alice = AccountId::from(ALICE);
		let bob = AccountId::from(Sr25519Keyring::Bob.public());
		let charlie = AccountId::from(Sr25519Keyring::Charlie.public());

		ExtBuilder::<Runtime>::default()
			.with_collators(vec![alice.clone()])
			.with_session_keys(vec![(
				alice.clone(),
				alice.clone(),
				SessionKeys { aura: AuraId::from(sp_core::sr25519::Public::from_raw(ALICE)) },
			)])
			.with_para_id(ASSET_HUB_ID.into())
			.build()
			.execute_with(|| {
				assert_ok!(indiv_pallet_pgas::Pallet::<Runtime>::do_create_pgas_asset());
				let pgas = PgasAssetId::get();
				let endowment = 100 * ExistentialDeposit::get();
				assert_ok!(<Assets as FungiblesMutate<AccountId>>::mint_into(
					pgas, &bob, endowment
				));

				let revive_call = RuntimeCall::Revive(pallet_revive::Call::map_account {});
				let remark = RuntimeCall::System(frame_system::Call::remark { remark: vec![] });
				let xt_bob_revive = construct_extrinsic(Sr25519Keyring::Bob, revive_call.clone());
				let xt_charlie = construct_extrinsic(Sr25519Keyring::Charlie, revive_call);

				assert_eq!(<Balances as FungibleInspect<AccountId>>::balance(&bob), 0);
				assert_eq!(<Balances as FungibleInspect<AccountId>>::balance(&charlie), 0);
				assert!(!frame_system::Pallet::<Runtime>::account_exists(&charlie));

				// Endow Charlie one planck short of the fee. The balance keeps his account alive
				let fee = pallet_transaction_payment::Pallet::<Runtime>::compute_fee(
					xt_charlie.encoded_size() as u32,
					&xt_charlie.get_dispatch_info(),
					0,
				);
				let charlie_endowment = fee - 1;
				assert!(
					charlie_endowment >= PgasMinBalance::get(),
					"the PGAS endowment must be holdable yet insufficient for the fee"
				);
				assert_ok!(<Assets as FungiblesMutate<AccountId>>::mint_into(
					pgas,
					&charlie,
					charlie_endowment
				));
				assert!(frame_system::Pallet::<Runtime>::account_exists(&charlie));

				// A Revive call is fee-payable in PGAS whatever its dispatch outcome.
				assert!(Executive::apply_extrinsic(xt_bob_revive).is_ok());

				let paid = endowment - <Assets as FungiblesInspect<AccountId>>::balance(pgas, &bob);
				assert!(paid > 0, "the fee should have been taken in PGAS");
				assert_eq!(<Balances as FungibleInspect<AccountId>>::balance(&bob), 0);
				assert!(
					System::events().iter().any(|record| matches!(
						record.event,
						RuntimeEvent::PgasAllowance(
							pallet_pgas_allowance::Event::PGASFeePaid { actual_fee, .. }
						) if actual_fee == paid
					)),
					"a PGASFeePaid event should report the fee burned"
				);

				// The filter keeps a non-Revive call off the PGAS path, and Bob holds no native.
				let xt_bob_remark = construct_extrinsic(Sr25519Keyring::Bob, remark);
				let pgas_left = <Assets as FungiblesInspect<AccountId>>::balance(pgas, &bob);
				assert_eq!(
					Executive::apply_extrinsic(xt_bob_remark),
					Err(TransactionValidityError::Invalid(InvalidTransaction::Payment))
				);
				assert_eq!(<Assets as FungiblesInspect<AccountId>>::balance(pgas, &bob), pgas_left);

				assert_eq!(
					Executive::apply_extrinsic(xt_charlie),
					Err(TransactionValidityError::Invalid(InvalidTransaction::Payment))
				);
				assert_eq!(
					<Assets as FungiblesInspect<AccountId>>::balance(pgas, &charlie),
					charlie_endowment,
					"a rejected transaction does not touch the signer's PGAS"
				);
			});
	}

	/// A signer whose PGAS balance does not cover the fee pays it in the native asset instead.
	#[test]
	fn dot_pays_the_fee_when_pgas_is_insufficient() {
		let alice = AccountId::from(ALICE);
		let bob = AccountId::from(Sr25519Keyring::Bob.public());
		let endowment = 100 * ExistentialDeposit::get();

		ExtBuilder::<Runtime>::default()
			.with_collators(vec![alice.clone()])
			.with_session_keys(vec![(
				alice.clone(),
				alice.clone(),
				SessionKeys { aura: AuraId::from(sp_core::sr25519::Public::from_raw(ALICE)) },
			)])
			.with_balances(vec![
				(bob.clone(), endowment),
				(pallet_dap::Pallet::<Runtime>::staging_account(), ExistentialDeposit::get()),
			])
			.with_para_id(ASSET_HUB_ID.into())
			.build()
			.execute_with(|| {
				assert_ok!(indiv_pallet_pgas::Pallet::<Runtime>::do_create_pgas_asset());
				let pgas = PgasAssetId::get();

				let call = RuntimeCall::Revive(pallet_revive::Call::map_account {});
				let xt = construct_extrinsic(Sr25519Keyring::Bob, call);

				let info = xt.get_dispatch_info();
				let fee = pallet_transaction_payment::Pallet::<Runtime>::compute_fee(
					xt.encoded_size() as u32,
					&info,
					0,
				);
				let pgas_endowment = fee - 1;
				assert!(
					pgas_endowment >= PgasMinBalance::get(),
					"the PGAS endowment must be holdable yet insufficient for the fee"
				);
				assert_ok!(<Assets as FungiblesMutate<AccountId>>::mint_into(
					pgas,
					&bob,
					pgas_endowment
				));

				assert!(Executive::apply_extrinsic(xt).is_ok());

				let paid = endowment - <Balances as FungibleInspect<AccountId>>::balance(&bob);
				assert!(paid > 0, "the fee should have been taken from the native balance");
				assert_eq!(
					<Assets as FungiblesInspect<AccountId>>::balance(pgas, &bob),
					pgas_endowment,
					"the insufficient PGAS balance should be left untouched"
				);
				assert!(
					System::events().iter().any(|record| matches!(
						record.event,
						RuntimeEvent::TransactionPayment(
							pallet_transaction_payment::Event::TransactionFeePaid {
								actual_fee, ..
							}
						) if actual_fee == paid
					)),
					"a TransactionFeePaid event should report the native fee"
				);
				assert!(
					!System::events().iter().any(|record| matches!(
						record.event,
						RuntimeEvent::PgasAllowance(
							pallet_pgas_allowance::Event::PGASFeePaid { .. }
						)
					)),
					"no fee should have been taken in PGAS"
				);
			});
	}
}

mod external_asset_teleport {
	// The trusted-reserve teleporter reads `ParachainInfo`, so every check runs inside
	// externalities.
	use super::*;
	use paseo_runtime_constants::system_parachain::PEOPLE_ID;

	type IsTeleporter = <XcmConfig as xcm_executor::Config>::IsTeleporter;

	fn people_origin() -> Location {
		Location::new(1, [Parachain(PEOPLE_ID)])
	}

	fn external_asset(amount: u128) -> Asset {
		(ExternalAssetLocation::get(), amount).into()
	}

	#[test]
	fn external_asset_teleport_from_people_is_accepted() {
		sp_io::TestExternalities::default().execute_with(|| {
			assert!(IsTeleporter::contains(&external_asset(1_000), &people_origin()));
		});
	}

	#[test]
	fn external_asset_teleport_from_relay_is_rejected() {
		sp_io::TestExternalities::default().execute_with(|| {
			assert!(!IsTeleporter::contains(&external_asset(1_000), &Location::parent()));
		});
	}

	#[test]
	fn external_asset_teleport_from_random_sibling_is_rejected() {
		sp_io::TestExternalities::default().execute_with(|| {
			let random_sibling = Location::new(1, [Parachain(4242)]);
			assert!(!IsTeleporter::contains(&external_asset(1_000), &random_sibling));
		});
	}

	#[test]
	fn wrong_asset_teleport_from_people_is_rejected() {
		sp_io::TestExternalities::default().execute_with(|| {
			// Different GeneralIndex (not the external asset).
			let wrong_asset: Asset = (
				Location::new(
					0,
					[PalletInstance(50), GeneralIndex((EXTERNAL_ASSET_ID + 1) as u128)],
				),
				1_000u128,
			)
				.into();
			assert!(!IsTeleporter::contains(&wrong_asset, &people_origin()));
		});
	}

	#[test]
	fn wrong_pallet_index_teleport_from_people_is_rejected() {
		sp_io::TestExternalities::default().execute_with(|| {
			// Right asset id, wrong pallet instance.
			let wrong_asset: Asset = (
				Location::new(0, [PalletInstance(99), GeneralIndex(EXTERNAL_ASSET_ID as u128)]),
				1_000u128,
			)
				.into();
			assert!(!IsTeleporter::contains(&wrong_asset, &people_origin()));
		});
	}

	#[test]
	fn dot_teleport_from_relay_still_works() {
		sp_io::TestExternalities::default().execute_with(|| {
			// Regression: pre-existing native-asset teleport rules unaffected.
			let dot: Asset = (DotLocation::get(), 1_000u128).into();
			assert!(IsTeleporter::contains(&dot, &Location::parent()));
		});
	}
}

/// The transaction pipeline is what decides whether a call can reach dispatch unpaid, so its shape
/// is an invariant of this chain, not an implementation detail.
mod tx_extension_pipeline {
	use asset_hub_paseo_runtime::{RuntimeCall, TxExtension};
	use sp_runtime::traits::TransactionExtension;

	/// Every extension of the pipeline, in the order it runs.
	///
	/// The four ahead of `RestrictOrigins` are the ones that replace the origin, and everything
	/// that charges the transaction runs after it. An extension that installs an origin the
	/// payment extensions do not charge therefore needs an allowance in
	/// `pallet-origin-restriction` to bound it, which is why an addition anywhere in this list is
	/// a deliberate change rather than an implementation detail.
	///
	/// Paseo deviation from upstream `next-asset-hub-paseo`: `PrevalidateAttests` (the claims
	/// pallet's extension) sits between `ChargeAssetTxPayment` and `CheckMetadataHash`.
	const PIPELINE: [&str; 18] = [
		"UnitTransactionExtension",
		"AsScarcity",
		"AuthorizeCall",
		"AsPgas",
		"AsDotnsGateway",
		"RestrictOrigins",
		"CheckNonZeroSender",
		"CheckSpecVersion",
		"CheckTxVersion",
		"CheckGenesis",
		"CheckMortality",
		"CheckNonce",
		"CheckWeight",
		"ChargeAssetTxPayment",
		"PrevalidateAttests",
		"CheckMetadataHash",
		"EthSetOrigin",
		"StorageWeightReclaim",
	];

	#[test]
	fn the_pipeline_is_the_expected_one() {
		let identifiers = <TxExtension as TransactionExtension<RuntimeCall>>::metadata()
			.into_iter()
			.map(|meta| meta.identifier)
			.collect::<Vec<_>>();

		assert_eq!(identifiers, PIPELINE);
	}
}

/// `EnsureCreditClaimant` is the only path from a transaction to a claimant identity, and
/// `ClaimantKind` is the only thing that selects between the two: no extension installs an alias
/// origin, so a claim resolves the person from the signer's binding.
mod credit_claimant_origin {
	use asset_hub_paseo_runtime::{
		AliasAccounts, EnsureCreditClaimant, MembersSubscriber, Runtime, RuntimeOrigin,
	};
	use frame_support::traits::{EnsureOriginWithArg, Get};
	use indiv_pallet_alias_accounts::{AccountToAlias, AliasAccountInfo, PEOPLE_IDENTIFIER};
	use indiv_pallet_members_subscriber::types::RingCommitmentRecord;
	use indiv_pallet_nft_claims::ClaimantKind;
	use indiv_support::{
		crypto::BandersnatchVrfVerifiable,
		identity::AccountOrPerson,
		traits::{Context, ContextualAlias, PersonhoodLookup},
	};
	use parachains_common::AccountId;
	use sp_runtime::BoundedVec;
	use verifiable::{ring::RingDomainSize, GenerateVerifiable};

	const ALIAS: [u8; 32] = [9u8; 32];
	const CONTEXT: Context = [3u8; 32];
	/// Time the seeded ring roots are committed at, in seconds.
	const SOURCE_TIME: u64 = 1_000_000;

	/// Window `personhood_info` keeps accepting a superseded revision for.
	fn retention() -> u64 {
		<<Runtime as indiv_pallet_members_subscriber::Config>::OldRootRetentionDuration as Get<
			u64,
		>>::get()
	}

	fn signer() -> AccountId {
		AccountId::from([8u8; 32])
	}

	/// Records `revisions` roots for ring 0 of the people collection, numbered from 0 and all
	/// committed at [`SOURCE_TIME`]. The retention check reads only the revision numbers and their
	/// source times, so an empty ring commitment stands in for the real root.
	fn seed_ring(revisions: u32) {
		let root = BandersnatchVrfVerifiable::finish_members(
			BandersnatchVrfVerifiable::start_members(RingDomainSize::Domain11),
		);
		let roots = (0..revisions)
			.map(|revision| RingCommitmentRecord {
				root: root.clone(),
				revision,
				source_time: SOURCE_TIME,
				source_sequence: 1,
			})
			.collect::<Vec<_>>();
		MembersSubscriber::set_current_ring_roots(
			PEOPLE_IDENTIFIER,
			0,
			BoundedVec::try_from(roots).expect("revisions within MaxRecentRootsPerRing"),
		);
	}

	/// Moves the clock the retention check reads to `SOURCE_TIME + offset` seconds. Writes `Now`
	/// directly, since `set_timestamp` runs Aura's `OnTimestampSet` hook, which requires the slot
	/// to match.
	fn set_now(offset: u64) {
		pallet_timestamp::Now::<Runtime>::put(
			SOURCE_TIME.saturating_add(offset).saturating_mul(1_000),
		);
	}

	/// The alias `personhood_info` resolves for `signer()` in [`CONTEXT`].
	fn personhood_alias() -> Option<[u8; 32]> {
		<AliasAccounts as PersonhoodLookup<AccountId, _>>::personhood_info(&signer(), &CONTEXT)
			.0
			.map(|(_collection, alias)| alias)
	}

	/// Binds `signer()` to [`ALIAS`], as `set_alias_account` does for a person who proved a ring
	/// membership.
	fn bind_alias() {
		AccountToAlias::<Runtime>::insert(
			signer(),
			AliasAccountInfo {
				collection: *PEOPLE_IDENTIFIER,
				ring: 0,
				revision: 0,
				ca: ContextualAlias { alias: ALIAS, context: CONTEXT },
			},
		);
	}

	/// `try_origin`'s success value, dropping the origin it hands back on failure so this does not
	/// depend on `RuntimeOrigin` being printable.
	fn claimant(origin: RuntimeOrigin, kind: ClaimantKind) -> Option<AccountOrPerson<AccountId>> {
		EnsureCreditClaimant::try_origin(origin, &kind).ok()
	}

	#[test]
	fn a_signer_claims_what_was_awarded_to_its_account() {
		sp_io::TestExternalities::default().execute_with(|| {
			let origin = RuntimeOrigin::signed(signer());
			assert_eq!(
				claimant(origin, ClaimantKind::Account),
				Some(AccountOrPerson::Account(signer()))
			);
		});
	}

	/// Revision 0 is the ring's latest, so the binding is one both lookups accept. This is the
	/// baseline [`a_stale_binding_still_resolves_to_its_person`] moves away from.
	#[test]
	fn a_signer_claims_as_the_person_its_account_is_bound_to() {
		sp_io::TestExternalities::default().execute_with(|| {
			bind_alias();
			seed_ring(1);
			set_now(0);
			assert_eq!(personhood_alias(), Some(ALIAS));

			let origin = RuntimeOrigin::signed(signer());
			assert_eq!(
				claimant(origin, ClaimantKind::Person),
				Some(AccountOrPerson::Person(ALIAS))
			);
		});
	}

	/// Claiming as a person is what the binding authorizes, so an account without one is rejected
	/// rather than falling back to claiming as itself.
	#[test]
	fn an_unbound_signer_cannot_claim_as_a_person() {
		sp_io::TestExternalities::default().execute_with(|| {
			let origin = RuntimeOrigin::signed(signer());
			assert_eq!(claimant(origin, ClaimantKind::Person), None);
		});
	}

	/// Revision 1 supersedes the binding's revision 0 and the retention has passed, so
	/// `personhood_info` refuses the binding. The credit is awarded to the alias before the claim,
	/// so the claim still resolves the same person.
	#[test]
	fn a_stale_binding_still_resolves_to_its_person() {
		sp_io::TestExternalities::default().execute_with(|| {
			bind_alias();
			seed_ring(2);
			set_now(retention() + 1);
			assert_eq!(personhood_alias(), None);

			let origin = RuntimeOrigin::signed(signer());
			assert_eq!(
				claimant(origin, ClaimantKind::Person),
				Some(AccountOrPerson::Person(ALIAS))
			);
		});
	}

	#[test]
	fn an_unsigned_origin_cannot_claim() {
		sp_io::TestExternalities::default().execute_with(|| {
			bind_alias();
			assert_eq!(claimant(RuntimeOrigin::root(), ClaimantKind::Person), None);
			assert_eq!(claimant(RuntimeOrigin::none(), ClaimantKind::Account), None);
		});
	}
}

/// Worst-case notifier-to-subscriber calls must fit the per-XCM-message weight budget.
/// The calls arrive only via XCM Transact, so their declared dispatch weight is weighed
/// into the message; a call above the MessageQueue service weight is marked permanently
/// overweight and never executes.
mod members_subscriber_xcm_budget {
	use super::*;
	use frame_support::{dispatch::GetDispatchInfo, traits::Get};
	use indiv_pallet_members_subscriber::types::{
		RingRootOp, RingRootUpdate, RingRootUpdatesBatch,
	};
	use indiv_support::{crypto::BandersnatchVrfVerifiable, traits::RingExponent};
	use sp_runtime::BoundedVec;
	use verifiable::{ring::RingDomainSize, GenerateVerifiable};

	/// A full batch whose ring counter sits at the far end of its range, proving the
	/// weight annotation is capped rather than growing with the counter.
	fn worst_case_batch() -> RingRootUpdatesBatch<Runtime> {
		let root = BandersnatchVrfVerifiable::finish_members(
			BandersnatchVrfVerifiable::start_members(RingDomainSize::Domain11),
		);
		let max_updates =
			<Runtime as indiv_pallet_members_subscriber::Config>::MaxUpdatesPerBatch::get();
		let updates = (0..max_updates)
			.map(|i| RingRootUpdate {
				ring_index: i,
				op: RingRootOp::Built { revision: 1, root: root.clone() },
			})
			.collect::<Vec<_>>();
		RingRootUpdatesBatch {
			identifier: *indiv_pallet_alias_accounts::PEOPLE_IDENTIFIER,
			sequence: 1,
			source_time: 1,
			updates: BoundedVec::try_from(updates).expect("within MaxUpdatesPerBatch"),
			next_ring_index: u32::MAX,
		}
	}

	#[test]
	fn subscriber_calls_fit_the_xcm_message_budget() {
		sp_io::TestExternalities::default().execute_with(|| {
			let budget =
				asset_hub_paseo_runtime::dynamic_params::message_queue::MaxOnInitWeight::get()
					.expect("MQ service weight configured");

			// Transact instruction overhead is negligible next to the slack this asserts.
			let calls = [
				(
					"initialize_ring_roots",
					indiv_pallet_members_subscriber::Call::<Runtime>::initialize_ring_roots {
						ring_exponent: RingExponent::R2e9,
						roots: worst_case_batch(),
					},
				),
				(
					"process_ring_updates",
					indiv_pallet_members_subscriber::Call::<Runtime>::process_ring_updates {
						batch: worst_case_batch(),
					},
				),
				(
					"terminate_subscription",
					indiv_pallet_members_subscriber::Call::<Runtime>::terminate_subscription {},
				),
			];
			for (name, call) in calls {
				let weight = call.get_dispatch_info().call_weight;
				assert!(
					weight.all_lte(budget),
					"`{name}` worst-case weight {weight:?} exceeds the XCM message budget {budget:?}",
				);
			}
		});
	}
}

/// The removal of a credit tree, from the claim that mints its last credit to the message the game
/// chain receives. These tests run against the runtime's own configuration, not the pallet's mock.
mod credit_tree_removal {
	use super::*;
	use asset_hub_paseo_runtime::{
		Balances, CreditTreeTtl, ExistentialDeposit, NftClaims, Runtime, RuntimeEvent,
		RuntimeOrigin, Scarcity, System, XcmpQueue,
	};
	use cumulus_primitives_core::XcmpMessageSource;
	use frame_support::{pallet_prelude::TransactionSource, BoundedVec};
	use indiv_pallet_nft_claims::{ClaimantKind, CreditTrees, PendingTreeDeletions, TreeExpiries};
	use indiv_support::{
		credit_trees::{
			credit_leaf, expiry_deadline, oldest_expiry, CreditTreeBlock, CreditProofNode,
			CreditTreeDelivery, ExpiryTimestamp, NftClaimCreditTree,
		},
		identity::AccountOrPerson,
	};
	use paseo_runtime_constants::system_parachain::{ASSET_HUB_ID, PEOPLE_ID};

	const BLOCK: CreditTreeBlock = 1;
	/// The wall-clock time the delivered tree commits to. The value is arbitrary, because
	/// `due_at` derives the deadline from it.
	const TIMESTAMP: u32 = 1_000_000;

	/// The first second at which the delivered tree is past its deadline.
	fn due_at() -> u64 {
		expiry_deadline(TIMESTAMP, CreditTreeTtl::get())
	}

	fn game_chain_origin() -> RuntimeOrigin {
		cumulus_pallet_xcm::Origin::SiblingParachain(PEOPLE_ID.into()).into()
	}

	fn set_now(secs: u64) {
		pallet_timestamp::Now::<Runtime>::put(secs.saturating_mul(1_000));
	}

	/// Creates the collection and item a claim mints into, and registers it for claims as its owner
	/// does beforehand. Returns the collection's identifier.
	fn prepare_collection() -> indiv_pallet_scarcity::CollectionId {
		use frame_support::traits::fungible::Mutate;
		use indiv_pallet_nft_claims::ItemSelection;

		let owner = AccountId::from([254u8; 32]);
		Balances::set_balance(&owner, 1_000 * ExistentialDeposit::get());
		let collection = indiv_pallet_scarcity::NextCollectionId::<Runtime>::get();
		assert_ok!(Scarcity::do_create_collection(owner.clone()));
		assert_ok!(Scarcity::do_define_item(
			owner.clone(),
			collection,
			indiv_pallet_scarcity::Transferability::Transferable,
			Vec::new()
		));
		assert_ok!(NftClaims::set_collection_minter(
			RuntimeOrigin::signed(owner),
			collection,
			Some(ItemSelection::Random)
		));

		collection
	}

	/// Delivers a one-leaf tree for [`BLOCK`] that commits to `claimant`'s `credit`, as the game
	/// chain does. Returns its leaf.
	fn deliver_tree(
		claimant: &AccountOrPerson<AccountId>,
		credit: [u8; 32],
	) -> indiv_support::credit_trees::NftClaimCreditLeaf {
		let leaf = credit_leaf(claimant, &credit);
		let tree = NftClaimCreditTree {
			game_index: 0,
			root: CreditProofNode(sp_io::hashing::blake2_256(&leaf.encode())),
			leaf_count: 1,
			timestamp: TIMESTAMP,
		};
		assert_ok!(NftClaims::receive_credit_trees(
			game_chain_origin(),
			indiv_support::credit_trees::CreditTreeBatch {
				source_time: TIMESTAMP as u64,
				trees: BoundedVec::truncate_from(vec![CreditTreeDelivery {
					sequence: Some(0),
					block: BLOCK,
					tree,
				}]),
			}
		));
		leaf
	}

	fn ext() -> sp_io::TestExternalities {
		let mut ext = ExtBuilder::<Runtime>::default().with_para_id(ASSET_HUB_ID.into()).build();
		ext.execute_with(|| {
			System::set_block_number(1);
			set_now(TIMESTAMP as u64);
			open_channel_to_game_chain();
			// The router refuses a destination whose XCM version it does not know.
			assert_ok!(PolkadotXcm::force_default_xcm_version(
				RuntimeOrigin::root(),
				Some(xcm::latest::VERSION)
			));
		});
		ext
	}

	/// Opens the egress HRMP channel the deletions travel over. The router checks it before it
	/// accepts a message.
	fn open_channel_to_game_chain() {
		use cumulus_pallet_parachain_system::RelevantMessagingState;
		use cumulus_primitives_core::relay_chain::AbridgedHrmpChannel;

		let channel = AbridgedHrmpChannel {
			max_capacity: 1000,
			max_total_size: 1_000_000,
			max_message_size: 102_400,
			msg_count: 0,
			total_size: 0,
			mqc_head: None,
		};
		RelevantMessagingState::<Runtime>::put(
			cumulus_pallet_parachain_system::relay_state_snapshot::MessagingStateSnapshot {
				dmq_mqc_head: Default::default(),
				relay_dispatch_queue_remaining_capacity: Default::default(),
				ingress_channels: Vec::new(),
				egress_channels: vec![(PEOPLE_ID.into(), channel)],
			},
		);
	}

	#[test]
	fn the_last_claim_of_a_tree_queues_the_game_chains_deletion() {
		ext().execute_with(|| {
			let collection = prepare_collection();
			let claimant = AccountId::from([1u8; 32]);
			let credit = [7u8; 32];
			deliver_tree(&AccountOrPerson::Account(claimant.clone()), credit);
			assert!(TreeExpiries::<Runtime>::contains_key(ExpiryTimestamp::from(TIMESTAMP), BLOCK));

			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(claimant),
				ClaimantKind::Account,
				BLOCK,
				credit,
				0,
				Default::default(),
				collection,
				AccountId::from([2u8; 32])
			));

			assert!(
				!CreditTrees::<Runtime>::contains_key(BLOCK),
				"the fully claimed tree is removed"
			);
			assert_eq!(PendingTreeDeletions::<Runtime>::get().to_vec(), vec![BLOCK]);
			// The spent leaf and the expiry entry that gets it removed outlive the tree.
			assert!(indiv_pallet_nft_claims::Pallet::<Runtime>::leaf_is_claimed(
				&indiv_pallet_nft_claims::ClaimedLeaves::<Runtime>::get(BLOCK),
				0
			));
			assert!(TreeExpiries::<Runtime>::contains_key(ExpiryTimestamp::from(TIMESTAMP), BLOCK));
		});
	}

	#[test]
	fn a_tree_past_its_deadline_is_swept_and_its_deletion_queued() {
		ext().execute_with(|| {
			let claimant = AccountId::from([1u8; 32]);
			deliver_tree(&AccountOrPerson::Account(claimant), [7u8; 32]);
			assert_eq!(oldest_expiry::<TreeExpiries<Runtime>, CreditTreeBlock>(), Some(TIMESTAMP));

			// One second before the tree falls due, the pallet's own check rejects the sweep.
			set_now(due_at() - 1);
			assert!(NftClaims::authorize_sweep_expired_trees(TransactionSource::Local, &TIMESTAMP)
				.is_err());

			set_now(due_at());
			assert!(NftClaims::authorize_sweep_expired_trees(TransactionSource::Local, &TIMESTAMP)
				.is_ok());
			assert_ok!(NftClaims::sweep_expired_trees(
				RuntimeOrigin::from(frame_system::RawOrigin::Authorized),
				TIMESTAMP,
				1
			));

			assert!(!CreditTrees::<Runtime>::contains_key(BLOCK));
			assert_eq!(PendingTreeDeletions::<Runtime>::get().to_vec(), vec![BLOCK]);
			assert_eq!(oldest_expiry::<TreeExpiries<Runtime>, CreditTreeBlock>(), None);
			assert!(System::events().iter().any(|record| matches!(
				record.event,
				RuntimeEvent::NftClaims(indiv_pallet_nft_claims::Event::CreditTreesExpired {
					count,
				}) if count == 1
			)));
		});
	}

	#[test]
	fn a_tree_delivered_past_its_deadline_is_not_stored() {
		ext().execute_with(|| {
			set_now(TIMESTAMP as u64 + CreditTreeTtl::get());

			let claimant = AccountId::from([1u8; 32]);
			deliver_tree(&AccountOrPerson::Account(claimant), [7u8; 32]);

			assert!(!CreditTrees::<Runtime>::contains_key(BLOCK));
			assert_eq!(oldest_expiry::<TreeExpiries<Runtime>, CreditTreeBlock>(), None);
		});
	}

	/// The message the deletions travel in has to name the dispatchable that receives them on the
	/// game chain. Only configuration makes the two chains agree, so this compares the encoded
	/// call against what next-people-paseo's own `RuntimeCall` encodes to.
	#[test]
	fn the_deletion_message_names_the_game_chains_dispatchable() {
		use xcm::latest::{Instruction, Xcm};

		ext().execute_with(|| {
			let claimant = AccountId::from([1u8; 32]);
			deliver_tree(&AccountOrPerson::Account(claimant), [7u8; 32]);
			set_now(due_at());
			assert_ok!(NftClaims::sweep_expired_trees(
				RuntimeOrigin::from(frame_system::RawOrigin::Authorized),
				TIMESTAMP,
				1
			));

			// The sweep queued the one tree delivered, so `BLOCK` is the front it left.
			assert_ok!(NftClaims::send_tree_deletions(
				RuntimeOrigin::from(frame_system::RawOrigin::Authorized),
				BLOCK,
				1
			));
			assert!(PendingTreeDeletions::<Runtime>::get().is_empty());

			// The call the pallet built, read back out of the message it queued.
			let encoded = XcmpQueue::take_outbound_messages(usize::MAX, &[])
				.into_iter()
				.find_map(|(para, data)| (u32::from(para) == PEOPLE_ID).then_some(data))
				.expect("a message went to the game chain");
			// The page carries a one-byte format prefix ahead of the versioned fragment.
			let mut bytes = &encoded[1..];
			let message: Xcm<()> = xcm::VersionedXcm::<()>::decode(&mut bytes)
				.expect("the fragment decodes")
				.try_into()
				.expect("the fragment is the latest version");
			let call = message
				.0
				.into_iter()
				.find_map(|instruction| match instruction {
					Instruction::Transact { call, .. } => Some(call.into_encoded()),
					_ => None,
				})
				.expect("the XCM carries a Transact");

			let expected = (57u8, 20u8, codec::Compact(1u32), BLOCK).encode();
			assert_eq!(call, expected, "the pallet and call indices are the game chain's");
		});
	}
}
