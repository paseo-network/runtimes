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

//! Genesis configs presets for the PeoplePolkadot runtime

use crate::*;
use hex_literal::hex;
use sp_core::{crypto::UncheckedInto, sr25519};
use sp_genesis_builder::PresetId;
use system_parachains_constants::genesis_presets::*;

const LIVE_RUNTIME_PRESET: &str = "live";
const PEOPLE_POLKADOT_ED: Balance = ExistentialDeposit::get();
const PARA_ID: u32 = 1044;

fn people_paseo_genesis(
	invulnerables: Vec<(AccountId, parachains_common::AuraId)>,
	endowed_accounts: Vec<AccountId>,
	id: ParaId,
	sudo: AccountId,
) -> serde_json::Value {
	let mut endowed_accounts = endowed_accounts;
	endowed_accounts.push(sudo.clone());

	serde_json::json!({
		"balances": BalancesConfig {
			balances: endowed_accounts
				.iter()
				.cloned()
				.map(|k| (k, PEOPLE_POLKADOT_ED * 4096 * 4096))
				.collect(),
			dev_accounts: None,
		},
		"parachainInfo": ParachainInfoConfig {
			parachain_id: id,
			..Default::default()
		},
		"collatorSelection": CollatorSelectionConfig {
			invulnerables: invulnerables.iter().cloned().map(|(acc, _)| acc).collect(),
			candidacy_bond: PEOPLE_POLKADOT_ED * 16,
			..Default::default()
		},
		"session": SessionConfig {
			keys: invulnerables
				.into_iter()
				.map(|(acc, aura)| {
					(
						acc.clone(),                         // account id
						acc,                                 // validator id
						SessionKeys { aura },			// session keys
					)
				})
				.collect(),
			..Default::default()
		},
		"sudo": {
			"key": Some(sudo)
		},
		"polkadotXcm": {
			"safeXcmVersion": Some(SAFE_XCM_VERSION),
		},
		// no need to pass anything to aura, in fact it will panic if we do. Session will take care
		// of this. `aura: Default::default()`
	})
}

pub fn people_paseo_local_testnet_genesis(para_id: ParaId) -> serde_json::Value {
	people_paseo_genesis(
		invulnerables(),
		testnet_accounts(),
		para_id,
		get_account_id_from_seed::<sr25519::Public>("Alice"),
	)
}

fn people_paseo_development_genesis(para_id: ParaId) -> serde_json::Value {
	people_paseo_genesis(
		invulnerables(),
		testnet_accounts_with([
			// Make sure `StakingPot` is funded for benchmarking purposes.
			StakingPot::get(),
		]),
		para_id,
		get_account_id_from_seed::<sr25519::Public>("Alice"),
	)
}

fn people_paseo_live_genesis(para_id: ParaId) -> serde_json::Value {
	people_paseo_genesis(
		Vec::from([
			(
				// Stash
				hex!("c4a649d9ddfa50130085a322b9adfe684888df6a6212dab0ef81193011d13119").into(),
				// Aura key
				hex!("c4a649d9ddfa50130085a322b9adfe684888df6a6212dab0ef81193011d13119")
					.unchecked_into(),
			),
			(
				// Stash
				hex!("9eb379b09a33013b839ed290a6d73cc31b138b1f6c178ba51406a45503801265").into(),
				// Aura key
				hex!("9eb379b09a33013b839ed290a6d73cc31b138b1f6c178ba51406a45503801265")
					.unchecked_into(),
			),
		]),
		testnet_accounts_with([
			// Make sure `StakingPot` is funded for benchmarking purposes.
			StakingPot::get(),
		]),
		para_id,
		hex!("bcc61f8c1a75aa26fa7dc56001ff41c1fcae46f9271e37a95804f150bd837242").into(),
	)
}

/// Provides the names of the predefined genesis configs for this runtime.
pub fn preset_names() -> Vec<PresetId> {
	vec![
		PresetId::from(sp_genesis_builder::DEV_RUNTIME_PRESET),
		PresetId::from(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET),
		PresetId::from(LIVE_RUNTIME_PRESET),
	]
}

/// Provides the JSON representation of predefined genesis config for given `id`.
pub fn get_preset(id: &PresetId) -> Option<Vec<u8>> {
	let patch = match id.as_ref() {
		sp_genesis_builder::DEV_RUNTIME_PRESET => people_paseo_development_genesis(PARA_ID.into()),
		sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET =>
			people_paseo_local_testnet_genesis(PARA_ID.into()),
		LIVE_RUNTIME_PRESET => people_paseo_live_genesis(PARA_ID.into()),

		_ => return None,
	};
	Some(
		serde_json::to_string(&patch)
			.expect("serialization to json is expected to work. qed.")
			.into_bytes(),
	)
}
