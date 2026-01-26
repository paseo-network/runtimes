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

pub use codec::Encode;

// Substrate
pub use frame_support::{
	assert_err, assert_ok,
	pallet_prelude::Weight,
	sp_runtime::{AccountId32, DispatchError, DispatchResult},
	traits::fungibles::Inspect,
};

// Paseo
pub use xcm::{
	prelude::{AccountId32 as AccountId32Junction, *},
	v3::{Error, NetworkId::Polkadot as PolkadotId},
};

// Cumulus
pub use asset_test_utils::xcm_helpers;
pub use emulated_integration_tests_common::{
	xcm_emulator::{
		assert_expected_events, bx, helpers::weight_within_threshold, Chain, Parachain as Para,
		RelayChain as Relay, Test, TestArgs, TestContext, TestExt,
	},
	xcm_helpers::{xcm_transact_paid_execution, xcm_transact_unpaid_execution},
	PROOF_SIZE_THRESHOLD, REF_TIME_THRESHOLD, XCM_V3,
};
pub use parachains_common::{AccountId, Balance};
pub use paseo_system_emulated_network::{
	asset_hub_paseo_emulated_chain::{
		genesis::ED as ASSET_HUB_POLKADOT_ED, AssetHubPaseoParaPallet as AssetHubPaseoPallet,
	},
	bridge_hub_paseo_emulated_chain::BridgeHubPaseoParaPallet as BridgeHubPaseoPallet,
	collectives_paseo_emulated_chain::CollectivesPaseoParaPallet as CollectivesPaseoPallet,
	coretime_paseo_emulated_chain::CoretimePaseoParaPallet as CoretimePaseoPallet,
	paseo_emulated_chain::{genesis::ED as POLKADOT_ED, PaseoRelayPallet as PaseoPallet},
	penpal_emulated_chain::{PenpalAParaPallet as PenpalAPallet, PenpalAssetOwner},
	people_paseo_emulated_chain::{
		genesis::ED as PEOPLE_POLKADOT_ED, PeoplePaseoParaPallet as PeoplePaseoPallet,
	},
	AssetHubPaseoPara as AssetHubPaseo, AssetHubPaseoParaReceiver as AssetHubPaseoReceiver,
	AssetHubPaseoParaSender as AssetHubPaseoSender, BridgeHubPaseoPara as BridgeHubPaseo,
	CollectivesPaseoPara as CollectivesPaseo, CoretimePaseoPara as CoretimePaseo,
	PaseoRelay as Paseo, PaseoRelayReceiver as PaseoReceiver, PaseoRelaySender as PaseoSender,
	PenpalAPara as PenpalA, PeoplePaseoPara as PeoplePaseo,
	PeoplePaseoParaReceiver as PeoplePaseoReceiver, PeoplePaseoParaSender as PeoplePaseoSender,
};
pub use people_paseo_runtime::{
	assets::hollar::{
		HollarId, HollarLocation, HydrationLocation, HOLLAR_UNITS, HYDRATION_PARA_ID,
	},
	ExistentialDeposit as PeoplePolkadotExistentialDeposit,
};

pub type RelayToSystemParaTest = Test<Paseo, PeoplePaseo>;
pub type RelayToParaTest = Test<Paseo, PenpalA>;
pub type SystemParaToRelayTest = Test<PeoplePaseo, Paseo>;
pub type SystemParaToParaTest = Test<PeoplePaseo, PenpalA>;
pub type ParaToSystemParaTest = Test<PenpalA, PeoplePaseo>;

#[cfg(test)]
mod tests;
