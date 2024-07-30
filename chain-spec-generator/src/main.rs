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

use clap::Parser;
use sc_chain_spec::ChainSpec;
use std::collections::HashMap;

mod common;
mod relay_chain_specs;
mod system_parachains_specs;

#[derive(Parser)]
struct Cli {
	/// The chain spec to generate.
	chain: String,

	/// Generate the chain spec as raw?
	#[arg(long)]
	raw: bool,
}

fn main() -> Result<(), String> {
	let cli = Cli::parse();

	let supported_chains =
		HashMap::<_, Box<dyn Fn() -> Result<Box<dyn ChainSpec>, String>>>::from([
			("paseo-dev", Box::new(|| relay_chain_specs::paseo_development_config()) as Box<_>),
			("paseo-local", Box::new(|| relay_chain_specs::paseo_local_testnet_config()) as Box<_>),
			(
				"asset-hub-paseo-local",
				Box::new(|| system_parachains_specs::asset_hub_paseo_local_testnet_config())
					as Box<_>,
			),
			(
				"bridge-hub-paseo",
				Box::new(|| system_parachains_specs::bridge_hub_paseo_config()) as Box<_>,
			),
			(
				"bridge-hub-paseo-local",
				Box::new(|| system_parachains_specs::bridge_hub_paseo_local_testnet_config())
					as Box<_>,
			),
		]);

	if let Some(function) = supported_chains.get(&*cli.chain) {
		let chain_spec = (*function)()?.as_json(cli.raw)?;
		print!("{chain_spec}");
		Ok(())
	} else {
        let supported = supported_chains.keys().enumerate().fold(String::new(), |c, (n, k)| {
			let extra = if n + 1 < supported_chains.len() { ", " } else { "" };
			format!("{c}{k}{extra}")
		});
		if cli.chain.ends_with(".json") {
			let chain_spec = common::from_json_file(&cli.chain, supported)?.as_json(cli.raw)?;
			print!("{chain_spec}");
			Ok(())
		} else {
			Err(format!("Unknown chain, only supported: {supported} or a json file"))
		}
	}
}
