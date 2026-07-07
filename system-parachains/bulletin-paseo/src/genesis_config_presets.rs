// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-only

//! # Bulletin Paseo Runtime genesis config presets

use crate::{UNITS as PAS, *};
use alloc::{vec, vec::Vec};
use cumulus_primitives_core::ParaId;
use frame_support::build_struct_json_patch;
use hex_literal::hex;
use parachains_common::{AccountId, AuraId};
use sp_core::crypto::UncheckedInto;
use sp_genesis_builder::PresetId;
use sp_keyring::Sr25519Keyring;

const BULLETIN_PASEO_ED: Balance = ExistentialDeposit::get();
pub const BULLETIN_PARA_ID: ParaId = ParaId::new(1010);

fn bulletin_paseo_genesis(
	invulnerables: Vec<(AccountId, AuraId)>,
	endowed_accounts: Vec<AccountId>,
	endowment: Balance,
	id: ParaId,
	sudo_account: Option<AccountId>,
	account_authorizations: Vec<(AccountId, u32, u64)>,
	allowed_authorizers: Vec<(AccountId, u32, u64)>,
) -> serde_json::Value {
	build_struct_json_patch!(RuntimeGenesisConfig {
		balances: BalancesConfig {
			balances: endowed_accounts.iter().cloned().map(|k| (k, endowment)).collect(),
		},
		parachain_info: ParachainInfoConfig { parachain_id: id },
		collator_selection: CollatorSelectionConfig {
			invulnerables: invulnerables.iter().cloned().map(|(acc, _)| acc).collect(),
			candidacy_bond: BULLETIN_PASEO_ED * 16,
		},
		session: SessionConfig {
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
		polkadot_xcm: PolkadotXcmConfig { safe_xcm_version: Some(SAFE_XCM_VERSION) },
		sudo: SudoConfig { key: sudo_account },
		transaction_storage: TransactionStorageConfig {
			account_authorizations,
			allowed_authorizers,
			..Default::default()
		},
	})
}

/// Genesis for the live Bulletin Paseo deployment (para 1010 on the Paseo relay).
///
/// The two genesis invulnerables are PLACEHOLDERS using well-known dev keys; swap in the
/// collator providers' real account/Aura keys (the `hex!` pairs below) before generating the
/// launch chain spec.
fn bulletin_paseo_live_genesis() -> serde_json::Value {
	bulletin_paseo_genesis(
		// Initial collators (invulnerables).
		vec![
			// PLACEHOLDER: provider 1 — currently Alice (well-known dev key).
			// 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
			(
				hex!("d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d").into(),
				hex!("d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d")
					.unchecked_into(),
			),
			// PLACEHOLDER: provider 2 — currently Bob (well-known dev key).
			// 5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty
			(
				hex!("8eaf04151687736326c9fea17e25fc5287613693c912909cb226aa4794f26a48").into(),
				hex!("8eaf04151687736326c9fea17e25fc5287613693c912909cb226aa4794f26a48")
					.unchecked_into(),
			),
		],
		// Endow the sudo account so it exists and can pay fees for regular calls.
		vec![hex!("808cd36029a4142ad7d255cd504e826156fee86f453841d398f874467c7f6e0b").into()],
		PAS * 1_000,
		BULLETIN_PARA_ID,
		// Sudo: 5EyFpXybSYon74HVGUZVyvtYxTLy4EuqUxMhgXcmLM2qz1BL
		Some(hex!("808cd36029a4142ad7d255cd504e826156fee86f453841d398f874467c7f6e0b").into()),
		// Account authorizations (account, transactions_allowance, bytes_allowance): the sudo
		// account can store from genesis. Note: subject to `AuthorizationPeriod` (14 days),
		// renewable on-chain.
		vec![(
			hex!("808cd36029a4142ad7d255cd504e826156fee86f453841d398f874467c7f6e0b").into(),
			1_000,
			10 * 1024 * 1024 * 1024,
		)],
		// Additional authorizers (account, transactions budget, bytes budget): the sudo
		// account can also grant authorizations via plain signed calls, without wrapping
		// them in `sudo.sudo`.
		vec![(
			hex!("808cd36029a4142ad7d255cd504e826156fee86f453841d398f874467c7f6e0b").into(),
			100_000,
			100 * 1024 * 1024 * 1024,
		)],
	)
}

/// Provides the JSON representation of predefined genesis config for given `id`.
pub fn get_preset(id: &PresetId) -> Option<Vec<u8>> {
	let patch = match id.as_ref() {
		"live" => bulletin_paseo_live_genesis(),
		sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET => bulletin_paseo_genesis(
			// initial collators.
			vec![
				(Sr25519Keyring::Alice.to_account_id(), Sr25519Keyring::Alice.public().into()),
				(Sr25519Keyring::Bob.to_account_id(), Sr25519Keyring::Bob.public().into()),
			],
			Sr25519Keyring::well_known().map(|k| k.to_account_id()).collect(),
			PAS * 1_000_000,
			BULLETIN_PARA_ID,
			// Sudo
			Some(Sr25519Keyring::Alice.to_account_id()),
			// Account authorizations (account, transactions_allowance, bytes_allowance).
			vec![(Sr25519Keyring::Alice.to_account_id(), 100, 10 * 1024 * 1024)],
			// Additional account authorizers (account, transactions budget, bytes budget).
			vec![(Sr25519Keyring::Eve.to_account_id(), 100_000, 100 * 1024 * 1024 * 1024)],
		),
		sp_genesis_builder::DEV_RUNTIME_PRESET => bulletin_paseo_genesis(
			// initial collators.
			vec![(Sr25519Keyring::Alice.to_account_id(), Sr25519Keyring::Alice.public().into())],
			vec![
				Sr25519Keyring::Alice.to_account_id(),
				Sr25519Keyring::Bob.to_account_id(),
				Sr25519Keyring::AliceStash.to_account_id(),
				Sr25519Keyring::BobStash.to_account_id(),
			],
			PAS * 1_000_000,
			BULLETIN_PARA_ID,
			// Sudo
			Some(Sr25519Keyring::Alice.to_account_id()),
			// Account authorizations (account, transactions_allowance, bytes_allowance).
			vec![(Sr25519Keyring::Alice.to_account_id(), 100, 10 * 1024 * 1024)],
			// Additional account authorizers (account, transactions budget, bytes budget).
			vec![(Sr25519Keyring::Eve.to_account_id(), 100_000, 100 * 1024 * 1024 * 1024)],
		),
		_ => return None,
	};

	Some(
		serde_json::to_string(&patch)
			.expect("serialization to json is expected to work. qed.")
			.into_bytes(),
	)
}

/// List of supported presets.
pub fn preset_names() -> Vec<PresetId> {
	vec![
		PresetId::from("live"),
		PresetId::from(sp_genesis_builder::DEV_RUNTIME_PRESET),
		PresetId::from(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET),
	]
}

#[cfg(test)]
mod tests {
	use super::*;
	use frame_support::genesis_builder_helper::build_state;
	use sp_genesis_builder::{DEV_RUNTIME_PRESET, LOCAL_TESTNET_RUNTIME_PRESET};

	/// Recursively merge `patch` into `base`, as `sc-chain-spec` does when it
	/// applies a preset patch on top of the default genesis config.
	fn json_merge(base: &mut serde_json::Value, patch: serde_json::Value) {
		match (base, patch) {
			(serde_json::Value::Object(base), serde_json::Value::Object(patch)) =>
				for (k, v) in patch {
					json_merge(base.entry(k).or_insert(serde_json::Value::Null), v);
				},
			(base, patch) => *base = patch,
		}
	}

	fn assert_preset_builds(id: &str) {
		sp_io::TestExternalities::default().execute_with(|| {
			let preset = get_preset(&PresetId::from(id))
				.unwrap_or_else(|| panic!("preset `{id}` is not defined"));
			let patch = serde_json::from_slice(&preset).expect("preset is valid JSON; qed");
			let mut config = serde_json::to_value(crate::RuntimeGenesisConfig::default())
				.expect("default genesis config serializes; qed");
			json_merge(&mut config, patch);
			build_state::<crate::RuntimeGenesisConfig>(
				serde_json::to_vec(&config).expect("merged config serializes; qed"),
			)
			.unwrap_or_else(|e| panic!("preset `{id}` failed to build: {e}"));
		});
	}

	#[test]
	fn live_genesis_preset_builds() {
		assert_preset_builds("live");
	}

	#[test]
	fn local_testnet_genesis_preset_builds() {
		assert_preset_builds(LOCAL_TESTNET_RUNTIME_PRESET);
	}

	#[test]
	fn development_genesis_preset_builds() {
		assert_preset_builds(DEV_RUNTIME_PRESET);
	}
}
