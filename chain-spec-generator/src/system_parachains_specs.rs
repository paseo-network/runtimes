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
use sp_core::{crypto::UncheckedInto, sr25519};

/// Generic extensions for Parachain ChainSpecs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ChainSpecGroup, ChainSpecExtension)]
#[serde(deny_unknown_fields)]
pub struct Extensions {
	/// The relay chain of the Parachain.
	pub relay_chain: String,
	/// The id of the Parachain.
	pub para_id: u32,
}

pub type AssetHubPaseoChainSpec = sc_chain_spec::GenericChainSpec<(), Extensions>;

pub type BridgeHubPaseoChainSpec = sc_chain_spec::GenericChainSpec<(), Extensions>;

pub type PeoplePaseoChainSpec = sc_chain_spec::GenericChainSpec<(), Extensions>;

const ASSET_HUB_PASEO_ED: Balance = asset_hub_paseo_runtime::ExistentialDeposit::get();

const BRIDGE_HUB_PASEO_ED: Balance = bridge_hub_paseo_runtime::ExistentialDeposit::get();

const PEOPLE_PASEO_ED: Balance = people_paseo_runtime::ExistentialDeposit::get();

/// The default XCM version to set in genesis config.
const SAFE_XCM_VERSION: u32 = xcm::prelude::XCM_VERSION;

/// Invulnerable Collators
pub fn invulnerables() -> Vec<(AccountId, AuraId)> {
	vec![
		(get_account_id_from_seed::<sr25519::Public>("Alice"), get_from_seed::<AuraId>("Alice")),
		(get_account_id_from_seed::<sr25519::Public>("Bob"), get_from_seed::<AuraId>("Bob")),
	]
}

/// Invulnerable Collators for the particular case of AssetHubPolkadot
pub fn invulnerables_asset_hub_paseo() -> Vec<(AccountId, AuraId)> {
	vec![
		(get_account_id_from_seed::<sr25519::Public>("Alice"), get_from_seed::<AuraId>("Alice")),
		(get_account_id_from_seed::<sr25519::Public>("Bob"), get_from_seed::<AuraId>("Bob")),
	]
}

/// Invulnerable Collators for People Chain
pub fn invulnerables_people_chain() -> Vec<(AccountId, AuraId)> {
	vec![
		// Stash: 13R4RzMu23PzK5o2BBhD19t5xV6PCpfoLjHjRUBrpdSJ4AAz / 0x6ad1e7e694a29ebf362928a0ba41d0d6d2af848f6d44511195e5f69820f36f44
		// AuraId: 0x9cd774e5f1a124e75b5973f767c88fff510a21c20eed72c8b592923561662e35
		(
			hex_literal::hex!("6ad1e7e694a29ebf362928a0ba41d0d6d2af848f6d44511195e5f69820f36f44")
				.into(),
			hex_literal::hex!("9cd774e5f1a124e75b5973f767c88fff510a21c20eed72c8b592923561662e35")
				.unchecked_into(),
		),
		// Stash: 5DvoL2BNoSm7wRt2tfZ6WW5QFrxm68GLv5SCrPQ4JBLjbvpL
		// AuraId: 0xa0f18557714e374091ee5b56180aba063f62281a7c734aed62d7f4cae3b42f0f
		(
			hex_literal::hex!("5270ec35ba01254d8bff046a1a58f16d3ae615c235efd6e99a35f233b2d9df2c")
				.into(),
			hex_literal::hex!("a0f18557714e374091ee5b56180aba063f62281a7c734aed62d7f4cae3b42f0f")
				.unchecked_into(),
		),
	]
}

/// Generate the session keys from individual elements.
///
/// The input must be a tuple of individual keys (a single arg for now since we have just one key).
pub fn asset_hub_paseo_session_keys(keys: AuraId) -> asset_hub_paseo_runtime::SessionKeys {
	asset_hub_paseo_runtime::SessionKeys { aura: keys }
}

/// Generate the session keys from individual elements.
///
/// The input must be a tuple of individual keys (a single arg for now since we have just one key).
pub fn bridge_hub_paseo_session_keys(keys: AuraId) -> bridge_hub_paseo_runtime::SessionKeys {
	bridge_hub_paseo_runtime::SessionKeys { aura: keys }
}

/// Generate the session keys from individual elements.
///
/// The input must be a tuple of individual keys (a single arg for now since we have just one key).
pub fn people_paseo_session_keys(keys: AuraId) -> people_paseo_runtime::SessionKeys {
	people_paseo_runtime::SessionKeys { aura: keys }
}

// AssetHubPaseo
fn asset_hub_paseo_genesis(
	invulnerables: Vec<(AccountId, AuraId)>,
	endowed_accounts: Vec<AccountId>,
	id: ParaId,
) -> serde_json::Value {
	serde_json::json!({
		"balances": asset_hub_paseo_runtime::BalancesConfig {
			balances: endowed_accounts
				.iter()
				.cloned()
				.map(|k| (k, ASSET_HUB_PASEO_ED * 4096 * 4096))
				.collect(),
		},
		"parachainInfo": asset_hub_paseo_runtime::ParachainInfoConfig {
			parachain_id: id,
			..Default::default()
		},
		"collatorSelection": asset_hub_paseo_runtime::CollatorSelectionConfig {
			invulnerables: invulnerables.iter().cloned().map(|(acc, _)| acc).collect(),
			candidacy_bond: ASSET_HUB_PASEO_ED * 16,
			..Default::default()
		},
		"session": asset_hub_paseo_runtime::SessionConfig {
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
		"polkadotXcm": {
			"safeXcmVersion": Some(SAFE_XCM_VERSION),
		},
		// no need to pass anything to aura, in fact it will panic if we do. Session will take care
		// of this. `aura: Default::default()`
	})
}

fn asset_hub_paseo_local_genesis(para_id: ParaId) -> serde_json::Value {
	asset_hub_paseo_genesis(
		// initial collators.
		invulnerables_asset_hub_paseo(),
		testnet_accounts(),
		para_id,
	)
}

pub fn asset_hub_paseo_local_testnet_config() -> Result<Box<dyn ChainSpec>, String> {
	let mut properties = sc_chain_spec::Properties::new();
	properties.insert("ss58Format".into(), 42.into());
	properties.insert("tokenSymbol".into(), "PAS".into());
	properties.insert("tokenDecimals".into(), 10.into());

	Ok(Box::new(
		AssetHubPaseoChainSpec::builder(
			asset_hub_paseo_runtime::WASM_BINARY.expect("AssetHubPaseo wasm not available!"),
			Extensions { relay_chain: "paseo-local".into(), para_id: 1000 },
		)
		.with_name("Asset Hub Paseo Local")
		.with_id("asset-hub-paseo-local")
		.with_chain_type(ChainType::Local)
		.with_protocol_id("ah-pas")
		.with_genesis_config_patch(asset_hub_paseo_local_genesis(1000.into()))
		.with_properties(properties)
		.build(),
	))
}

// BridgeHubPaseo
fn bridge_hub_paseo_genesis(
	invulnerables: Vec<(AccountId, AuraId)>,
	endowed_accounts: Vec<AccountId>,
	id: ParaId,
) -> serde_json::Value {
	serde_json::json!({
		"balances": bridge_hub_paseo_runtime::BalancesConfig {
			balances: endowed_accounts
				.iter()
				.cloned()
				.map(|k| (k, BRIDGE_HUB_PASEO_ED * 4096 * 4096))
				.collect(),
		},
		"parachainInfo": bridge_hub_paseo_runtime::ParachainInfoConfig {
			parachain_id: id,
			..Default::default()
		},
		"collatorSelection": bridge_hub_paseo_runtime::CollatorSelectionConfig {
			invulnerables: invulnerables.iter().cloned().map(|(acc, _)| acc).collect(),
			candidacy_bond: BRIDGE_HUB_PASEO_ED * 16,
			..Default::default()
		},
		"session": bridge_hub_paseo_runtime::SessionConfig {
			keys: invulnerables
				.into_iter()
				.map(|(acc, aura)| {
					(
						acc.clone(),                            // account id
						acc,                                    // validator id
						bridge_hub_paseo_session_keys(aura), // session keys
					)
				})
				.collect(),
		},
		"polkadotXcm": {
			"safeXcmVersion": Some(SAFE_XCM_VERSION),
		},
		"ethereumSystem": bridge_hub_paseo_runtime::EthereumSystemConfig {
			para_id: id,
			asset_hub_para_id: paseo_runtime_constants::system_parachain::ASSET_HUB_ID.into(),
			..Default::default()
		},
		// no need to pass anything to aura, in fact it will panic if we do. Session will take care
		// of this. `aura: Default::default()`
	})
}

fn bridge_hub_paseo_local_genesis(para_id: ParaId) -> serde_json::Value {
	bridge_hub_paseo_genesis(
		// initial collators.
		invulnerables(),
		testnet_accounts(),
		para_id,
	)
}

pub fn bridge_hub_paseo_config() -> Result<Box<dyn ChainSpec>, String> {
	let mut properties = sc_chain_spec::Properties::new();
	properties.insert("ss58Format".into(), 42.into());
	properties.insert("tokenSymbol".into(), "PAS".into());
	properties.insert("tokenDecimals".into(), 10.into());

	Ok(Box::new(
		BridgeHubPaseoChainSpec::builder(
			bridge_hub_paseo_runtime::WASM_BINARY.expect("BridgeHubPaseo wasm not available!"),
			Extensions { relay_chain: "paseo".into(), para_id: 1002 },
		)
		.with_name("Paseo Bridge Hub")
		.with_id("paseo-bridge-hub")
		.with_chain_type(ChainType::Live)
		.with_protocol_id("bh-pas")
		.with_genesis_config_patch(bridge_hub_paseo_local_genesis(1002.into()))
		.with_properties(properties)
		.build(),
	))
}

pub fn bridge_hub_paseo_local_testnet_config() -> Result<Box<dyn ChainSpec>, String> {
	let mut properties = sc_chain_spec::Properties::new();
	properties.insert("ss58Format".into(), 42.into());
	properties.insert("tokenSymbol".into(), "PAS".into());
	properties.insert("tokenDecimals".into(), 10.into());

	Ok(Box::new(
		BridgeHubPaseoChainSpec::builder(
			bridge_hub_paseo_runtime::WASM_BINARY.expect("BridgeHubPaseo wasm not available!"),
			Extensions { relay_chain: "paseo-local".into(), para_id: 1002 },
		)
		.with_name("Paseo Bridge Hub Local")
		.with_id("paseo-bridge-hub-local")
		.with_chain_type(ChainType::Local)
		.with_protocol_id("bh-pas")
		.with_genesis_config_patch(bridge_hub_paseo_local_genesis(1002.into()))
		.with_properties(properties)
		.build(),
	))
}

// PeoplePolkadot
pub fn people_paseo_genesis(
	invulnerables: Vec<(AccountId, AuraId)>,
	endowed_accounts: Vec<AccountId>,
	id: ParaId,
) -> serde_json::Value {
	serde_json::json!({
		"balances": people_paseo_runtime::BalancesConfig {
			balances: endowed_accounts
				.iter()
				.cloned()
				.map(|k| (k, PEOPLE_PASEO_ED * 4096 * 4096))
				.collect(),
		},
		"parachainInfo": people_paseo_runtime::ParachainInfoConfig {
			parachain_id: id,
			..Default::default()
		},
		"collatorSelection": people_paseo_runtime::CollatorSelectionConfig {
			invulnerables: invulnerables.iter().cloned().map(|(acc, _)| acc).collect(),
			candidacy_bond: PEOPLE_PASEO_ED * 16,
			..Default::default()
		},
		"session": people_paseo_runtime::SessionConfig {
			keys: invulnerables
				.into_iter()
				.map(|(acc, aura)| {
					(
						acc.clone(),                         // account id
						acc,                                 // validator id
						people_paseo_session_keys(aura), // session keys
					)
				})
				.collect(),
		},
		"polkadotXcm": {
			"safeXcmVersion": Some(SAFE_XCM_VERSION),
		},
		// no need to pass anything to aura, in fact it will panic if we do. Session will take care
		// of this. `aura: Default::default()`
	})
}

fn people_paseo_local_genesis(para_id: ParaId) -> serde_json::Value {
	crate::system_parachains_specs::people_paseo_genesis(
		// initial collators.
		invulnerables(),
		testnet_accounts(),
		para_id,
	)
}

fn people_paseo_testnet_genesis(para_id: ParaId) -> serde_json::Value {
	crate::system_parachains_specs::people_paseo_genesis(
		// Initial collators.
		invulnerables_people_chain(),
		// Endow collator stashes + sudo.
		vec![
			hex_literal::hex!("6ad1e7e694a29ebf362928a0ba41d0d6d2af848f6d44511195e5f69820f36f44")
				.into(),
			hex_literal::hex!("5270ec35ba01254d8bff046a1a58f16d3ae615c235efd6e99a35f233b2d9df2c")
				.into(),
			hex_literal::hex!("7e939ef17e229e9a29210d95cb0b607e0030d54899c05f791a62d5c6f4557659")
				.into(),
		],
		para_id,
	)
}

pub fn people_paseo_local_testnet_config() -> Result<Box<dyn ChainSpec>, String> {
	let mut properties = sc_chain_spec::Properties::new();
	properties.insert("ss58Format".into(), 0.into());
	properties.insert("tokenSymbol".into(), "PAS".into());
	properties.insert("tokenDecimals".into(), 10.into());

	Ok(Box::new(
		PeoplePaseoChainSpec::builder(
			people_paseo_runtime::WASM_BINARY.expect("PeoplePaseo wasm not available!"),
			Extensions { relay_chain: "paseo-local".into(), para_id: 1004 },
		)
		.with_name("Paseo People Local")
		.with_id("paseo-people-local")
		.with_chain_type(ChainType::Local)
		.with_protocol_id("pc-pas")
		.with_genesis_config_patch(crate::system_parachains_specs::people_paseo_local_genesis(
			1004.into(),
		))
		.with_properties(properties)
		.build(),
	))
}

pub fn people_paseo_testnet_config() -> Result<Box<dyn ChainSpec>, String> {
	let mut properties = sc_chain_spec::Properties::new();
	properties.insert("ss58Format".into(), 0.into());
	properties.insert("tokenSymbol".into(), "PAS".into());
	properties.insert("tokenDecimals".into(), 10.into());

	Ok(Box::new(
		PeoplePaseoChainSpec::builder(
			people_paseo_runtime::WASM_BINARY.expect("PeoplePaseo wasm not available!"),
			Extensions { relay_chain: "paseo".into(), para_id: 1004 },
		)
		.with_name("Paseo People")
		.with_id("paseo-people")
		.with_chain_type(ChainType::Live)
		.with_protocol_id("pc-pas")
		.with_genesis_config_patch(crate::system_parachains_specs::people_paseo_testnet_genesis(
			1004.into(),
		))
		.with_properties(properties)
		.build(),
	))
}
