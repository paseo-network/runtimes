// Copyright (C) Parity Technologies and the various Paseo contributors, see Contributions.md
// for a list of specific contributors.
// This file is part of Paseo.

// Paseo is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Paseo is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Paseo.  If not, see <http://www.gnu.org/licenses/>.

use sc_chain_spec::{ChainSpec, ChainType, NoExtension};

pub type PaseoChainSpec = sc_chain_spec::GenericChainSpec<NoExtension>;

const DEFAULT_PROTOCOL_ID: &str = "pas";

/// Returns the properties for the [`PaseoChainSpec`].
pub fn paseo_chain_spec_properties() -> serde_json::map::Map<String, serde_json::Value> {
	serde_json::json!({
		"tokenDecimals": 10,
	})
	.as_object()
	.expect("Map given; qed")
	.clone()
}

/// Paseo development config (single validator Alice)
pub fn paseo_development_config() -> Result<Box<dyn ChainSpec>, String> {
	Ok(Box::new(
		PaseoChainSpec::builder(
			paseo_runtime::WASM_BINARY.ok_or("Paseo development wasm not available")?,
			Default::default(),
		)
		.with_name("Paseo Dev")
		.with_id("paseo-dev")
		.with_chain_type(ChainType::Development)
		.with_genesis_config_patch(
			paseo_runtime::genesis_config_presets::paseo_development_config_genesis(),
		)
		.with_protocol_id(DEFAULT_PROTOCOL_ID)
		.with_properties(paseo_chain_spec_properties())
		.build(),
	))
}


/// Paseo local testnet config (multivalidator Alice + Bob)
pub fn paseo_local_testnet_config() -> Result<Box<dyn ChainSpec>, String> {
	Ok(Box::new(
		PaseoChainSpec::builder(
			paseo_runtime::WASM_BINARY.ok_or("Paseo development wasm not available")?,
			Default::default(),
		)
		.with_name("Paseo Local Testnet")
		.with_id("paseo-local")
		.with_chain_type(ChainType::Local)
		.with_genesis_config_patch(
			paseo_runtime::genesis_config_presets::paseo_local_testnet_genesis(),
		)
		.with_protocol_id(DEFAULT_PROTOCOL_ID)
		.with_properties(paseo_chain_spec_properties())
		.build(),
	))
}
