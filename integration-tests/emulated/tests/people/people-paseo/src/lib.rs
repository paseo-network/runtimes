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
pub use frame_support::{assert_ok, traits::fungibles};
pub use sp_runtime;

// Cumulus
pub use emulated_integration_tests_common::macros::AssetTransferFilter;
pub use emulated_integration_tests_common::xcm_emulator::{Chain, Parachain as Para, TestExt};

// Paseo
pub use xcm::prelude::*;
pub use xcm_executor;

pub use paseo_system_emulated_network::{
	asset_hub_paseo_emulated_chain::genesis::ED as ASSET_HUB_POLKADOT_ED,
	people_paseo_emulated_chain::PeoplePaseoParaPallet as PeoplePaseoPallet,
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

pub use sp_io;

#[cfg(test)]
mod tests;
