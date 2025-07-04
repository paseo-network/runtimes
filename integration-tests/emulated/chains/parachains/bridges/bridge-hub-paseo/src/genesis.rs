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
use sp_keyring::Sr25519Keyring as Keyring;

// Cumulus
use emulated_integration_tests_common::{
	accounts, build_genesis_storage, collators, SAFE_XCM_VERSION,
};
use parachains_common::Balance;
use xcm::latest::prelude::*;

pub const ASSET_HUB_PARA_ID: u32 = 1000;
pub const PARA_ID: u32 = 1002;
pub const ED: Balance = bridge_hub_paseo_runtime::ExistentialDeposit::get();

pub fn genesis() -> sp_core::storage::Storage {
	let genesis_config = bridge_hub_paseo_runtime::RuntimeGenesisConfig {
		system: bridge_hub_paseo_runtime::SystemConfig::default(),
		balances: bridge_hub_paseo_runtime::BalancesConfig {
			balances: accounts::init_balances().iter().cloned().map(|k| (k, ED * 4096)).collect(),
			dev_accounts: None,
		},
		parachain_info: bridge_hub_paseo_runtime::ParachainInfoConfig {
			parachain_id: PARA_ID.into(),
			..Default::default()
		},
		collator_selection: bridge_hub_paseo_runtime::CollatorSelectionConfig {
			invulnerables: collators::invulnerables().iter().cloned().map(|(acc, _)| acc).collect(),
			candidacy_bond: ED * 16,
			..Default::default()
		},
		session: bridge_hub_paseo_runtime::SessionConfig {
			keys: collators::invulnerables()
				.into_iter()
				.map(|(acc, aura)| {
					(
						acc.clone(),                                       // account id
						acc,                                               // validator id
						bridge_hub_paseo_runtime::SessionKeys { aura }, // session keys
					)
				})
				.collect(),
			..Default::default()
		},
		polkadot_xcm: bridge_hub_paseo_runtime::PolkadotXcmConfig {
			safe_xcm_version: Some(SAFE_XCM_VERSION),
			..Default::default()
		},
		bridge_kusama_grandpa: bridge_hub_paseo_runtime::BridgeKusamaGrandpaConfig {
			owner: Some(Keyring::Bob.to_account_id()),
			..Default::default()
		},
		bridge_kusama_messages: bridge_hub_paseo_runtime::BridgeKusamaMessagesConfig {
			owner: Some(Keyring::Bob.to_account_id()),
			..Default::default()
		},
		ethereum_system: bridge_hub_paseo_runtime::EthereumSystemConfig {
			para_id: PARA_ID.into(),
			asset_hub_para_id: ASSET_HUB_PARA_ID.into(),
			..Default::default()
		},
		xcm_over_bridge_hub_kusama: bridge_hub_paseo_runtime::XcmOverBridgeHubKusamaConfig {
			opened_bridges: vec![
				// open PAH -> KAH bridge
				(
					Location::new(1, [Parachain(1000)]),
					Junctions::from([Kusama.into(), Parachain(1000)]),
					Some(bp_messages::LegacyLaneId([0, 0, 0, 1])),
				),
			],
			..Default::default()
		},
		..Default::default()
	};

	build_genesis_storage(
		&genesis_config,
		bridge_hub_paseo_runtime::WASM_BINARY
			.expect("WASM binary was not built, please build it!"),
	)
}
