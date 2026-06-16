// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Genesis configs presets for the AssetHubPaseo runtime

use crate::{xcm_config::UniversalLocation, *};
use alloc::vec::Vec;
use pallet_revive::AddressMapper;
use parachains_common::AuraId;
use sp_core::sr25519;
use sp_genesis_builder::PresetId;
use system_parachains_constants::genesis_presets::*;
use xcm::latest::prelude::*;
use xcm_builder::GlobalConsensusConvertsFor;
use xcm_executor::traits::ConvertLocation;

const ASSET_HUB_POLKADOT_ED: Balance = ExistentialDeposit::get();

/// Invulnerable Collators for the particular case of AssetHubPaseo
pub fn invulnerables_asset_hub_paseo() -> Vec<(AccountId, AuraId)> {
	vec![
		(get_account_id_from_seed::<sr25519::Public>("Alice"), get_from_seed::<AuraId>("Alice")),
		(get_account_id_from_seed::<sr25519::Public>("Bob"), get_from_seed::<AuraId>("Bob")),
	]
}

fn asset_hub_paseo_genesis(
	invulnerables: Vec<(AccountId, AuraId)>,
	endowed_accounts: Vec<AccountId>,
	id: ParaId,
	foreign_assets: Vec<(Location, AccountId, Balance)>,
	foreign_assets_endowed_accounts: Vec<(Location, AccountId, Balance)>,
) -> serde_json::Value {
	let mut balances: Vec<(AccountId, Balance)> = endowed_accounts
		.iter()
		.cloned()
		.map(|k| (k, ASSET_HUB_POLKADOT_ED * 4096 * 4096))
		.collect();
	// Ensure the DAP buffer and staging accounts hold at least the existential
	// deposit, but skip any account a preset has already endowed (the dev preset
	// seeds the staging account explicitly). Pushing a duplicate account makes the
	// `balances` genesis builder panic with "duplicate balances in genesis", which
	// silently breaks benchmarking (`frame-omni-bencher` builds a genesis preset)
	// and dev chain-spec generation.
	for account in [Dap::buffer_account(), Dap::staging_account()] {
		if !balances.iter().any(|(who, _)| *who == account) {
			balances.push((account, ASSET_HUB_POLKADOT_ED));
		}
	}

	serde_json::json!({
		"balances": BalancesConfig {
			balances,
			dev_accounts: None,
		},
		"parachainInfo": ParachainInfoConfig {
			parachain_id: id,
			..Default::default()
		},
		"collatorSelection": CollatorSelectionConfig {
			invulnerables: invulnerables.iter().cloned().map(|(acc, _)| acc).collect(),
			candidacy_bond: ASSET_HUB_POLKADOT_ED * 16,
			..Default::default()
		},
		"session": SessionConfig {
			keys: invulnerables
				.into_iter()
				.map(|(acc, aura)| {
					(
						acc.clone(),                           // account id
						acc,                                   // validator id
						SessionKeys { aura }, 	// session keys
					)
				})
				.collect(),
			..Default::default()
		},
		"sudo": {
			"key": Some(get_account_id_from_seed::<sr25519::Public>("Alice"))
		},
		"polkadotXcm": {
			"safeXcmVersion": Some(SAFE_XCM_VERSION),
		},
		"staking": {
			"validatorCount": 100,
			"devStakers": Some((2_000, 25_000)),
		},
		"foreignAssets": ForeignAssetsConfig {
			assets: foreign_assets
				.into_iter()
				.map(|asset| (asset.0, asset.1, false, asset.2))
				.collect(),
			accounts: foreign_assets_endowed_accounts
				.into_iter()
				.map(|asset| (asset.0, asset.1, asset.2))
				.collect(),
			..Default::default()
		},
		"revive": ReviveConfig {
			mapped_accounts: endowed_accounts.iter().filter(|x| ! <Runtime as pallet_revive::Config>::AddressMapper::is_mapped(x)).cloned().collect(),
			accounts: Vec::new(),
			debug_settings: None,
		},
		// no need to pass anything to aura, in fact it will panic if we do. Session will take care
		// of this. `aura: Default::default()`
	})
}

pub fn asset_hub_paseo_local_testnet_genesis(para_id: ParaId) -> serde_json::Value {
	asset_hub_paseo_genesis(
		invulnerables_asset_hub_paseo(),
		testnet_accounts(),
		para_id,
		vec![
			// bridged KSM
			(
				Location::new(2, [GlobalConsensus(Kusama)]),
				GlobalConsensusConvertsFor::<UniversalLocation, AccountId>::convert_location(
					&Location { parents: 2, interior: [GlobalConsensus(Kusama)].into() },
				)
				.unwrap(),
				10000000,
			),
		],
		vec![
			// bridged KSM to Bob
			(
				Location::new(2, [GlobalConsensus(Kusama)]),
				get_account_id_from_seed::<sp_core::sr25519::Public>("Bob"),
				10000000 * 4096 * 4096,
			),
		],
	)
}

fn asset_hub_paseo_development_genesis(para_id: ParaId) -> serde_json::Value {
	asset_hub_paseo_genesis(
		invulnerables_asset_hub_paseo(),
		testnet_accounts_with([
			// Make sure the DAP staging account is funded for benchmarking purposes.
			Dap::staging_account(),
		]),
		para_id,
		vec![],
		vec![],
	)
}

/// Provides the names of the predefined genesis configs for this runtime.
pub fn preset_names() -> Vec<PresetId> {
	vec![
		PresetId::from(sp_genesis_builder::DEV_RUNTIME_PRESET),
		PresetId::from(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET),
	]
}

/// Provides the JSON representation of predefined genesis config for given `id`.
pub fn get_preset(id: &PresetId) -> Option<Vec<u8>> {
	let patch = match id.as_ref() {
		sp_genesis_builder::DEV_RUNTIME_PRESET => asset_hub_paseo_development_genesis(1000.into()),
		sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET =>
			asset_hub_paseo_local_testnet_genesis(1000.into()),
		_ => return None,
	};
	Some(
		serde_json::to_string(&patch)
			.expect("serialization to json is expected to work. qed.")
			.into_bytes(),
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use frame_support::genesis_builder_helper::build_state;
	use sp_genesis_builder::{DEV_RUNTIME_PRESET, LOCAL_TESTNET_RUNTIME_PRESET};

	// Building a genesis preset must not panic. The `balances` genesis builder panics
	// on duplicate accounts ("duplicate balances in genesis"); this previously broke
	// asset-hub benchmarking (`frame-omni-bencher` builds the `local_testnet` preset)
	// and dev chain-spec generation.
	/// Recursively merge `patch` into `base`, as `sc-chain-spec` does when it
	/// applies a preset patch on top of the default genesis config.
	fn json_merge(base: &mut serde_json::Value, patch: serde_json::Value) {
		match (base, patch) {
			(serde_json::Value::Object(base), serde_json::Value::Object(patch)) =>
				for (k, v) in patch {
					json_merge(base.entry(k).or_insert(serde_json::Value::Null), v);
				},
			(base, patch) => *base = patch,
		}
	}

	fn assert_preset_builds(id: &str) {
		sp_io::TestExternalities::default().execute_with(|| {
			// Preset generation itself reads storage (`AddressMapper::is_mapped`),
			// so it must also run inside the externalities environment.
			let preset = get_preset(&PresetId::from(id))
				.unwrap_or_else(|| panic!("preset `{id}` is not defined"));
			let patch = serde_json::from_slice(&preset).expect("preset is valid JSON; qed");
			let mut config = serde_json::to_value(crate::RuntimeGenesisConfig::default())
				.expect("default genesis config serializes; qed");
			json_merge(&mut config, patch);
			build_state::<crate::RuntimeGenesisConfig>(
				serde_json::to_vec(&config).expect("merged config serializes; qed"),
			)
			.unwrap_or_else(|e| panic!("preset `{id}` failed to build: {e}"));
		});
	}

	#[test]
	fn local_testnet_genesis_preset_builds() {
		assert_preset_builds(LOCAL_TESTNET_RUNTIME_PRESET);
	}

	#[test]
	fn development_genesis_preset_builds() {
		assert_preset_builds(DEV_RUNTIME_PRESET);
	}
}
