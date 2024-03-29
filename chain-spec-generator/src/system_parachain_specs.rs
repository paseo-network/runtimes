// Copyright (C) Parity Technologies and the various Polkadot contributors, see Contributions.md
// for a list of specific contributors.
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

use crate::common::{get_account_id_from_seed, get_from_seed, testnet_accounts};
use cumulus_primitives_core::ParaId;
use parachains_common::{AccountId, AuraId, Balance};
use sc_chain_spec::{ChainSpec, ChainSpecExtension, ChainSpecGroup, ChainType};
use serde::{Deserialize, Serialize};
use sp_core::{crypto::UncheckedInto, sr25519};//use sp_keyring::AccountKeyring::{Alice, Bob, Charlie};
use hex_literal::hex;

/// Generic extensions for Parachain ChainSpecs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ChainSpecGroup, ChainSpecExtension)]
#[serde(deny_unknown_fields)]
pub struct Extensions {
	/// The relay chain of the Parachain.
	pub relay_chain: String,
	/// The id of the Parachain.
	pub para_id: u32,
}

pub type AssetHubPaseoChainSpec =
	sc_chain_spec::GenericChainSpec<asset_hub_paseo_runtime::RuntimeGenesisConfig, Extensions>;

const AH_PROTOCOL_ID: &str = "ah-pas";

// We allow the polkadot assethub
const ASSET_HUB_PASEO_ED: Balance = parachains_common::polkadot::currency::EXISTENTIAL_DEPOSIT;

/// The default XCM version to set in genesis config.
const SAFE_XCM_VERSION: u32 = xcm::prelude::XCM_VERSION;

pub fn invulnerables_asset_hub_paseo_local() -> Vec<(AccountId, AuraId)> {
		vec![
			(
				get_account_id_from_seed::<sr25519::Public>("Alice"),
				get_from_seed::<AuraId>("Alice"),
			),
			(
				get_account_id_from_seed::<sr25519::Public>("Bob"),
				get_from_seed::<AuraId>("Bob"),
			),
		]
}

pub fn invulnerables_asset_hub_paseo() -> Vec<(AccountId, AuraId)> {
	vec![
		(
			hex!("0efe248e3ddcfcb4f29675b70fc0a8e2db66b65381c45d299427b60d05f76108").into(),
			hex!("3a73f2eed5d458e4b427be3afbd20e6d7a0b6972f5295a0b1062a96fe6d83610").unchecked_into(),
		),
		(
			hex!("8eef6710734f5d1e7d2eb303fa8f04e9bef65fb680647b24624723f95b868964").into(),
			hex!("de05506c73f35cf0bd50652b719369c2e20be9bf2c8522bd6cb61059a0cb0033").unchecked_into(),
		),
	]
}

/// Generate the session keys from individual elements.
///
/// The input must be a tuple of individual keys (a single arg for now since we have just one key).
pub fn asset_hub_paseo_session_keys(
	keys: AuraId,
) -> asset_hub_paseo_runtime::SessionKeys {
	asset_hub_paseo_runtime::SessionKeys { aura: keys }
}

pub fn ah_endowed_accounts() -> Vec<AccountId> {
	invulnerables_asset_hub_paseo().iter().map(
		|collator| collator.0.clone()
	).collect::<Vec<_>>()
}


// AssetHubPolkadot
fn asset_hub_paseo_genesis(
	wasm_binary: &[u8],
	invulnerables: Vec<(AccountId, AuraId)>,
	endowed_accounts: Vec<AccountId>,
	id: ParaId,
) -> asset_hub_paseo_runtime::RuntimeGenesisConfig {
	asset_hub_paseo_runtime::RuntimeGenesisConfig {
		system: asset_hub_paseo_runtime::SystemConfig {
			code: wasm_binary.to_vec(),
			..Default::default()
		},
		balances: asset_hub_paseo_runtime::BalancesConfig {
			balances: endowed_accounts
				.iter()
				.cloned()
				.map(|k| (k, ASSET_HUB_PASEO_ED * 4096))
				.collect(),
		},
		parachain_info: asset_hub_paseo_runtime::ParachainInfoConfig {
			parachain_id: id,
			..Default::default()
		},
		collator_selection: asset_hub_paseo_runtime::CollatorSelectionConfig {
			invulnerables: invulnerables.iter().cloned().map(|(acc, _)| acc).collect(),
			candidacy_bond: ASSET_HUB_PASEO_ED * 16,
			..Default::default()
		},
		session: asset_hub_paseo_runtime::SessionConfig {
			keys: invulnerables
				.into_iter()
				.map(|(acc, aura)| {
					(
						acc.clone(),                           // account id
						acc,                                   // validator id
						asset_hub_paseo_session_keys(aura), // session keys
					)
				})
				.collect(),
		},
		// no need to pass anything to aura, in fact it will panic if we do. Session will take care
		// of this.
		aura: Default::default(),
		aura_ext: Default::default(),
		parachain_system: Default::default(),
		polkadot_xcm: asset_hub_paseo_runtime::PolkadotXcmConfig {
			safe_xcm_version: Some(SAFE_XCM_VERSION),
			..Default::default()
		},
	}
}

fn asset_hub_paseo_local_genesis(
	wasm_binary: &[u8],
) -> asset_hub_paseo_runtime::RuntimeGenesisConfig {
	asset_hub_paseo_genesis(
		// initial collators.
		wasm_binary,
		invulnerables_asset_hub_paseo_local(),
		testnet_accounts(),
		1000.into(),
	)
}

pub fn asset_hub_paseo_local_testnet_config() -> Result<Box<dyn ChainSpec>, String> {
	let mut properties = sc_chain_spec::Properties::new();
	properties.insert("ss58Format".into(), 42.into());
	properties.insert("tokenSymbol".into(), "PAS".into());
	properties.insert("tokenDecimals".into(), 10.into());

	let wasm_binary =
		asset_hub_paseo_runtime::WASM_BINARY.ok_or("AssetHubPaseo wasm not available")?;

	Ok(Box::new(AssetHubPaseoChainSpec::from_genesis(
		// Name
		"Paseo Asset Hub Local",
		// ID
		"asset-hub-paseo-local",
		ChainType::Local,
		move || asset_hub_paseo_local_genesis(wasm_binary),
		Vec::new(),
		None,
		Some(AH_PROTOCOL_ID),
		None,
		Some(properties),
		Extensions { relay_chain: "paseo-local".into(), para_id: 1000 },
	)))
}


fn asset_hub_paseo_config_genesis(
	wasm_binary: &[u8],
) -> asset_hub_paseo_runtime::RuntimeGenesisConfig {
	asset_hub_paseo_genesis(
		// initial collators.
		wasm_binary,
		invulnerables_asset_hub_paseo(),
		ah_endowed_accounts(),
		1000.into(),
	)
}

pub fn asset_hub_paseo_testnet_config() -> Result<Box<dyn ChainSpec>, String> {
	let mut properties = sc_chain_spec::Properties::new();
	properties.insert("ss58Format".into(), 42.into());
	properties.insert("tokenSymbol".into(), "PAS".into());
	properties.insert("tokenDecimals".into(), 10.into());

	let wasm_binary =
		asset_hub_paseo_runtime::WASM_BINARY.ok_or("AssetHubPaseo wasm not available")?;

	Ok(Box::new(AssetHubPaseoChainSpec::from_genesis(
		// Name
		"Paseo Asset Hub",
		// ID
		"asset-hub-paseo",
		ChainType::Live,
		move || asset_hub_paseo_config_genesis(wasm_binary),
		Vec::new(),
		None,
		Some(AH_PROTOCOL_ID),
		None,
		Some(properties),
		Extensions { relay_chain: "paseo".into(), para_id: 1000 },
	)))
}