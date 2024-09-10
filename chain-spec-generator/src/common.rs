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

use crate::{
	relay_chain_specs::{PaseoChainSpec},
	system_parachains_specs::{
		AssetHubPaseoChainSpec,
		BridgeHubPaseoChainSpec,CoretimePaseoChainSpec,PeoplePaseoChainSpec,
	},
	ChainSpec,
};

#[derive(Debug, serde::Deserialize)]
struct EmptyChainSpecWithId {
	id: String,
}

/* pub fn from_json_file(filepath: &str, supported: String) -> Result<Box<dyn ChainSpec>, String> {
	let path = std::path::PathBuf::from(&filepath);
	let file = std::fs::File::open(filepath).expect("Failed to open file");
	let reader = std::io::BufReader::new(file);
	let chain_spec: EmptyChainSpecWithId = serde_json::from_reader(reader)
		.expect("Failed to read 'json' file with ChainSpec configuration");
	match &chain_spec.id {
		x if x.starts_with("paseo") | x.starts_with("pas") =>
			Ok(Box::new(PaseoChainSpec::from_json_file(path)?)),
		x if x.starts_with("asset-hub-paseo") =>
			Ok(Box::new(AssetHubPaseoChainSpec::from_json_file(path)?)),
		x if x.starts_with("bridge-hub-paseo") =>
			Ok(Box::new(BridgeHubPaseoChainSpec::from_json_file(path)?)),
		x if x.starts_with("coretime-paseo") =>
			Ok(Box::new(CoretimePaseoChainSpec::from_json_file(path)?)),
		x if x.starts_with("people-paseo") =>
			Ok(Box::new(PeoplePaseoChainSpec::from_json_file(path)?)),
		_ => Err(format!("Unknown chain 'id' in json file. Only supported: {supported}'")),
	}
} */
