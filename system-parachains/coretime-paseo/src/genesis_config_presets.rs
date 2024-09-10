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

fn coretime_paseo_development_genesis(para_id: ParaId) -> serde_json::Value {
	coretime_paseo_genesis(invulnerables_tot(), testnet_accounts(), para_id)
}

fn coretime_paseo_live_genesis(para_id: ParaId) -> serde_json::Value {
	coretime_paseo_genesis(
		vec![
			// Parity polkadot-coretime-collator-a-0
			// 13umUoWwGb765EPzMUrMmYTcEjKfNJiNyCDwdqAvCMzteGzi
			(
				hex!("80b6f570f356fef7b891afa2e1c30fca89bc7a2cddd545fd8a173106fce3a11f").into(),
				hex!("4a69b6ec0eda668471d806db625681a147efc35a4baeacf0bca95d12d13cd942")
					.unchecked_into(),
			),
			// Parity polkadot-coretime-collator-a-1
			// 13NAwtroa2efxgtih1oscJqjxcKpWJeQF8waWPTArBewi2CQ
			(
				hex!("689e1a66fa33b75f66415021aacc4fa23f49306a3c21407748b8b2d39b4abf63").into(),
				hex!("f0d0e90c36f95605510f00a9f0821675bc0c7b70e5c8d113b0426c21d627773b")
					.unchecked_into(),
			)
		],
		Vec::new(),
		para_id,
	)
}

pub(super) fn preset_names() -> Vec<PresetId> {
	vec![PresetId::from("live"), PresetId::from("development"), PresetId::from("local_testnet")]
}

/// Provides the JSON representation of predefined genesis config for given `id`.
pub fn get_preset(id: &sp_genesis_builder::PresetId) -> Option<sp_std::vec::Vec<u8>> {
	let patch = match id.try_into() {
		Ok("live") => coretime_paseo_live_genesis(1005.into()),
		Ok("development") => coretime_paseo_development_genesis(1005.into()),
		Ok("local_testnet") => coretime_paseo_local_testnet_genesis(1005.into()),
		_ => return None,
	};
	Some(
		serde_json::to_string(&patch)
			.expect("serialization to json is expected to work. qed.")
			.into_bytes(),
	)
}
