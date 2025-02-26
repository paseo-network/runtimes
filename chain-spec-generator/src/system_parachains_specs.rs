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

use sc_chain_spec::{ChainSpec, ChainSpecExtension, ChainSpecGroup, ChainType};
use sc_network::config::MultiaddrWithPeerId;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Generic extensions for Parachain ChainSpecs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ChainSpecGroup, ChainSpecExtension)]
#[serde(deny_unknown_fields)]
pub struct Extensions {
	/// The relay chain of the Parachain.
	pub relay_chain: String,
	/// The id of the Parachain.
	pub para_id: u32,
}

pub type AssetHubPaseoChainSpec = sc_chain_spec::GenericChainSpec<Extensions>;

pub type BridgeHubPaseoChainSpec = sc_chain_spec::GenericChainSpec<Extensions>;

pub type PeoplePaseoChainSpec = sc_chain_spec::GenericChainSpec<Extensions>;

pub type CoretimePaseoChainSpec = sc_chain_spec::GenericChainSpec<Extensions>;
pub type CollectivesPaseoChainSpec = sc_chain_spec::GenericChainSpec<Extensions>;

pub fn asset_hub_paseo_local_testnet_config() -> Result<Box<dyn ChainSpec>, String> {
	let mut properties = sc_chain_spec::Properties::new();
	properties.insert("ss58Format".into(), 0.into());
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
		.with_genesis_config_preset_name("local_testnet")
		.with_properties(properties)
		.build(),
	))
}

pub fn bridge_hub_paseo_local_testnet_config() -> Result<Box<dyn ChainSpec>, String> {
	let mut properties = sc_chain_spec::Properties::new();
	properties.insert("ss58Format".into(), 0.into());
	properties.insert("tokenSymbol".into(), "PAS".into());
	properties.insert("tokenDecimals".into(), 10.into());

	Ok(Box::new(
		BridgeHubPaseoChainSpec::builder(
			bridge_hub_paseo_runtime::WASM_BINARY
				.expect("BridgeHubPaseo wasm not available!"),
			Extensions { relay_chain: "paseo-local".into(), para_id: 1002 },
		)
		.with_name("Paseo Bridge Hub Local")
		.with_id("paseo-bridge-hub-local")
		.with_chain_type(ChainType::Local)
		.with_protocol_id("bh-pas")
		.with_genesis_config_patch(
		    bridge_hub_paseo_runtime::genesis_config_presets::bridge_hub_paseo_local_testnet_genesis(
				1002.into()
			),
		)
		.with_properties(properties)
		.build(),
	))
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
		.with_genesis_config_patch(
			people_paseo_runtime::genesis_config_presets::people_paseo_local_testnet_genesis(
				1004.into(),
			),
		)
		.with_properties(properties)
		.build(),
	))
}

pub fn coretime_paseo_local_testnet_config() -> Result<Box<dyn ChainSpec>, String> {
	let mut properties = sc_chain_spec::Properties::new();
	properties.insert("ss58Format".into(), 0.into());
	properties.insert("tokenSymbol".into(), "PAS".into());
	properties.insert("tokenDecimals".into(), 10.into());

	Ok(Box::new(
		CoretimePaseoChainSpec::builder(
			coretime_paseo_runtime::WASM_BINARY.expect("CoretimePaseo wasm not available!"),
			Extensions { relay_chain: "paseo-local".into(), para_id: 1005 },
		)
		.with_name("Paseo Coretime Local")
		.with_id("paseo-coretime-local")
		.with_chain_type(ChainType::Local)
		.with_protocol_id("ct-pas")
		.with_genesis_config_preset_name("local_testnet")
		.with_properties(properties)
		.build(),
	))
}

pub fn coretime_paseo_tot_config() -> Result<Box<dyn ChainSpec>, String> {
	let mut properties = sc_chain_spec::Properties::new();
	properties.insert("ss58Format".into(), 0.into());
	properties.insert("tokenSymbol".into(), "PAS".into());
	properties.insert("tokenDecimals".into(), 10.into());

	Ok(Box::new(
		CoretimePaseoChainSpec::builder(
			coretime_paseo_runtime::WASM_BINARY.expect("CoretimePaseo wasm not available!"),
			Extensions { relay_chain: "paseo".into(), para_id: 1005 },
		)
		.with_name("Paseo Coretime Local")
		.with_id("paseo-coretime-tot")
		.with_chain_type(ChainType::Live)
		.with_protocol_id("ct-pas")
		.with_genesis_config_preset_name("tot")
		.with_properties(properties)
		.build(),
	))
}

pub fn coretime_paseo_config() -> Result<Box<dyn ChainSpec>, String> {
	let mut properties = sc_chain_spec::Properties::new();
	properties.insert("ss58Format".into(), 0.into());
	properties.insert("tokenSymbol".into(), "PAS".into());
	properties.insert("tokenDecimals".into(), 10.into());

	Ok(Box::new(
		CoretimePaseoChainSpec::builder(
			coretime_paseo_runtime::WASM_BINARY.expect("Paseo Coretime wasm not available!"),
			Extensions { relay_chain: "paseo".into(), para_id: 1005 },
		)
		.with_name("Paseo Coretime")
		.with_id("paseo-coretime")
		.with_chain_type(ChainType::Live)
		.with_protocol_id("ct-pas")
		.with_genesis_config_preset_name("live")
		.with_properties(properties)
		.build(),
	))
}

pub fn collectives_paseo_local_config() -> Result<Box<dyn ChainSpec>, String> {
	let mut properties = sc_chain_spec::Properties::new();
	properties.insert("ss58Format".into(), 0.into());
	properties.insert("tokenSymbol".into(), "PAS".into());
	properties.insert("tokenDecimals".into(), 10.into());

	Ok(Box::new(
		CollectivesPaseoChainSpec::builder(
			collectives_paseo_runtime::WASM_BINARY.expect("Collectives wasm not available!"),
			Extensions { relay_chain: "collectives-local".into(), para_id: 1001 },
		)
		.with_name("Paseo Collectives Local")
		.with_id("paseo-collectives-local")
		.with_chain_type(ChainType::Local)
		.with_protocol_id("col-pas")
		.with_genesis_config_preset_name("local_testnet")
		.with_properties(properties)
		.build(),
	))
}
