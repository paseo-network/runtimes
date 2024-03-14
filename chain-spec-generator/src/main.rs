use clap::Parser;
use sc_chain_spec::ChainSpec;
use std::collections::HashMap;

mod common;
mod relay_chain_specs;
mod system_parachain_specs;

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
            (
                "paseo",
                Box::new(|| relay_chain_specs::paseo_config()) as Box<_>,
            ),
            (
                "paseo-local",
                Box::new(|| relay_chain_specs::paseo_local_config()) as Box<_>,
            ),
            (
                "asset-hub-paseo",
                Box::new(|| system_parachain_specs::asset_hub_paseo_testnet_config())
					as Box<_>,
            ),
            (
				"asset-hub-paseo-local",
				Box::new(|| system_parachain_specs::asset_hub_paseo_local_testnet_config())
					as Box<_>,
			),
        ]);

    if let Some(function) = supported_chains.get(&*cli.chain) {
        let chain_spec = (*function)()?.as_json(cli.raw)?;
        print!("{chain_spec}");
        Ok(())
    } else {
        let supported = supported_chains
            .keys()
            .enumerate()
            .fold(String::new(), |c, (n, k)| {
                let extra = (n + 1 < supported_chains.len()).then(|| ", ").unwrap_or("");
                format!("{c}{k}{extra}")
            });
        if cli.chain.ends_with(".json") {
            let chain_spec = common::from_json_file(&cli.chain, supported)?.as_json(cli.raw)?;
            print!("{chain_spec}");
            Ok(())
        } else {
            Err(format!(
                "Unknown chain, only supported: {supported} or a json file"
            ))
        }
    }
}
