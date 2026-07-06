// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-only

//! # Bulletin Paseo Runtime genesis config presets

use crate::{
	paseo_constants::{currency::UNITS as PAS, xcm_version::SAFE_XCM_VERSION},
	*,
};
use alloc::{vec, vec::Vec};
use cumulus_primitives_core::ParaId;
use frame_support::build_struct_json_patch;
use parachains_common::{AccountId, AuraId};
use sp_genesis_builder::PresetId;
use sp_keyring::Sr25519Keyring;

const BULLETIN_PASEO_ED: Balance = ExistentialDeposit::get();
pub const BULLETIN_PARA_ID: ParaId = ParaId::new(1501);

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

/// Provides the JSON representation of predefined genesis config for given `id`.
pub fn get_preset(id: &PresetId) -> Option<Vec<u8>> {
	let patch = match id.as_ref() {
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
		PresetId::from(sp_genesis_builder::DEV_RUNTIME_PRESET),
		PresetId::from(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET),
	]
}
