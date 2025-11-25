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

// Substrate
pub use codec::Encode;
pub use frame_support::{assert_err, assert_ok, pallet_prelude::DispatchResult, traits::fungibles};
pub use sp_runtime::DispatchError;

// Cumulus
pub use emulated_integration_tests_common::macros::{AssetTransferFilter, XcmPaymentApiV1};
pub use emulated_integration_tests_common::{
	accounts::{ALICE, BOB},
	create_pool_with_native_on,
	impls::Inspect,
	test_parachain_is_trusted_teleporter, test_parachain_is_trusted_teleporter_for_relay,
	test_relay_is_trusted_teleporter,
	xcm_emulator::{
		assert_expected_events, bx, helpers::weight_within_threshold, Chain, Parachain as Para,
		RelayChain as Relay, Test, TestArgs, TestContext, TestExt,
	},
	xcm_helpers::{xcm_transact_paid_execution, xcm_transact_unpaid_execution},
	ASSETS_PALLET_ID, PROOF_SIZE_THRESHOLD, REF_TIME_THRESHOLD, XCM_V4,
};

// Paseo
pub use xcm::{
	prelude::{AccountId32 as AccountId32Junction, *},
	v5::{self, Error},
};
pub use xcm_executor::traits::TransferType;

pub use parachains_common::{AccountId, Balance};
pub use paseo_system_emulated_network::{
	asset_hub_paseo_emulated_chain::{
		genesis::{
			AssetHubPaseoAssetOwner, PenpalATeleportableAssetLocation, ED as ASSET_HUB_POLKADOT_ED,
		},
		AssetHubPaseoParaPallet as AssetHubPaseoPallet,
	},
	bridge_hub_paseo_emulated_chain::{
		genesis::ED as BRIDGE_HUB_POLKADOT_ED, BridgeHubPaseoParaPallet as BridgeHubPaseoPallet,
	},
	people_paseo_emulated_chain::{
		PeoplePaseoParaPallet as PeoplePolkadotPallet,
		PeoplePaseoParaPallet as PeoplePaseoPallet,
	},
	paseo_emulated_chain::{
		genesis::ED as POLKADOT_ED,
		paseo_runtime::{xcm_config::UniversalLocation as PaseoRelayUniversalLocation, Dmp},
		PaseoRelayPallet as PaseoPallet,
	},
	penpal_emulated_chain::{
		penpal_runtime::xcm_config::{
			CustomizableAssetFromSystemAssetHub as PenpalCustomizableAssetFromSystemAssetHub,
			UniversalLocation as PenpalUniversalLocation,
		},
		PenpalAParaPallet as PenpalAPallet, PenpalAssetOwner, PenpalBParaPallet as PenpalBPallet,
	},
	AssetHubPaseoPara as AssetHubPaseo, AssetHubPaseoParaReceiver as AssetHubPaseoReceiver,
	AssetHubPaseoParaSender as AssetHubPaseoSender, BridgeHubPaseoPara as BridgeHubPaseo,
	BridgeHubPaseoParaReceiver as BridgeHubPaseoReceiver,
	BridgeHubPaseoParaSender as BridgeHubPaseoSender, PaseoRelay as Paseo,
	PaseoRelayReceiver as PaseoReceiver, PaseoRelaySender as PaseoSender, PenpalAPara as PenpalA,
	PenpalAParaReceiver as PenpalAReceiver, PenpalBPara as PenpalB,
	PenpalBParaReceiver as PenpalBReceiver, PenpalBParaSender as PenpalBSender,
	PeoplePaseoPara as PeoplePolkadot, PeoplePaseoParaReceiver as PeoplePolkadotReceiver,
	PeoplePaseoParaSender as PeoplePolkadotSender,
	PeoplePaseoPara as PeoplePaseo, PeoplePaseoParaReceiver as PeoplePaseoReceiver,
	PeoplePaseoParaSender as PeoplePaseoSender,
};
pub use people_paseo_runtime::{
	xcm_config::{
		hollar::{HollarId, HollarLocation, HydrationLocation, HOLLAR_UNITS, HYDRATION_PARA_ID},
		XcmConfig,
	},
	ExistentialDeposit as PeoplePolkadotExistentialDeposit,
};

pub use paseo_runtime_constants::currency::CENTS as PAS_CENTS;

// XCM executor imports
pub use xcm_executor;
pub use sp_io;

#[cfg(test)]
mod tests;
