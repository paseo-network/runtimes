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

//! Genesis configs presets for the Polkadot Coretime runtime

use crate::*;
use hex_literal::hex;
use sp_core::crypto::UncheckedInto;
use sp_genesis_builder::PresetId;
use sp_std::vec::Vec;
use sp_core::sr25519;
use system_parachains_constants::genesis_presets::*;

const CORETIME_POLKADOT_ED: Balance = ExistentialDeposit::get();

fn coretime_paseo_genesis(
	invulnerables: Vec<(AccountId, AuraId)>,
	endowed_accounts: Vec<AccountId>,
	id: ParaId,
) -> serde_json::Value {
	serde_json::json!({
		"balances": BalancesConfig {
			balances: endowed_accounts
				.iter()
				.cloned()
				.map(|k| (k, CORETIME_POLKADOT_ED * 4096 * 4096))
				.collect(),
		},
		"parachainInfo": ParachainInfoConfig {
			parachain_id: id,
			..Default::default()
		},
		"collatorSelection": CollatorSelectionConfig {
			invulnerables: invulnerables.iter().cloned().map(|(acc, _)| acc).collect(),
			candidacy_bond: CORETIME_POLKADOT_ED * 16,
			..Default::default()
		},
		"session": SessionConfig {
			keys: invulnerables
				.into_iter()
				.map(|(acc, aura)| {
					(
						acc.clone(),          // account id
						acc,                  // validator id
						SessionKeys { aura }, // session keys
					)
				})
				.collect(),
		},
		"sudo": {
			"key": Some(get_account_id_from_seed::<sr25519::Public>("Alice"))
		},
		"polkadotXcm": {
			"safeXcmVersion": Some(SAFE_XCM_VERSION),
		},
		// no need to pass anything to aura, in fact it will panic if we do. Session will take care
		// of this. `aura: Default::default()`
	})
}

fn coretime_paseo_local_testnet_genesis(para_id: ParaId) -> serde_json::Value {
	coretime_paseo_genesis(invulnerables(), testnet_accounts(), para_id)
}

fn coretime_paseo_tot_genesis(para_id: ParaId) -> serde_json::Value {
	coretime_paseo_genesis(invulnerables_tot(), testnet_accounts(), para_id)
}

fn coretime_paseo_live_genesis(para_id: ParaId) -> serde_json::Value {
	coretime_paseo_genesis(
		vec![
			// Paradox
			// 16WWmr2Xqgy5fna35GsNHXMU7vDBM12gzHCFGibQjSmKpAN
			(
				hex!("043393e76c137dfdc403a6fd9a2d6129d470d51c5a67bd40517378030c87170d").into(),
				hex!("0a2cee67864d1d4c9433bfd45324b8f72425f096e01041546be48c5d3bc9a746")
					.unchecked_into(),
			),
			// Mile
			// 13NAwtroa2efxgtih1oscJqjxcKpWJeQF8waWPTArBewi2CQ
			(
				hex!("0cf6762e28ed1505f5595a7845d153b1853b026d0b620a70a564378043c33b18").into(),
				hex!("7e126fa970a75ae2cd371d01ee32e9387f0b256832e408ca8ea7b254e6bcde7d")
					.unchecked_into(),
			)
		],
		Vec::new(),
		para_id,
	)
}

pub(super) fn preset_names() -> Vec<PresetId> {
	vec![PresetId::from("live"), PresetId::from("tot"), PresetId::from("local_testnet")]
}

/// Provides the JSON representation of predefined genesis config for given `id`.
pub fn get_preset(id: &sp_genesis_builder::PresetId) -> Option<sp_std::vec::Vec<u8>> {
	let patch = match id.try_into() {
		Ok("live") => coretime_paseo_live_genesis(1005.into()),
		Ok("tot") => coretime_paseo_tot_genesis(1005.into()),
		Ok("local_testnet") => coretime_paseo_local_testnet_genesis(1005.into()),
		_ => return None,
	};
	Some(
		serde_json::to_string(&patch)
			.expect("serialization to json is expected to work. qed.")
			.into_bytes(),
	)
}
