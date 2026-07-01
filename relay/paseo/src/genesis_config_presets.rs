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

//! Genesis configs presets for the Polkadot runtime

extern crate alloc;

use crate::*;
#[cfg(not(feature = "std"))]
use alloc::format;
use babe_primitives::AuthorityId as BabeId;
use hex_literal::hex;
use pallet_staking::{Forcing, StakerStatus};
use paseo_runtime_constants::currency::UNITS as PAS;
use polkadot_primitives::{
	node_features::FeatureIndex,
	AccountPublic, AssignmentId, AsyncBackingParams,
	ExecutorParam::{MaxMemoryPages, PvfExecTimeout},
	PvfExecKind,
};
use runtime_parachains::configuration::HostConfiguration;
use sp_core::{crypto::UncheckedInto, sr25519, Pair, Public};
use sp_genesis_builder::PresetId;
use sp_runtime::{traits::IdentifyAccount, Perbill};

/// Helper function to generate a crypto pair from seed
fn get_from_seed<TPublic: Public>(seed: &str) -> <TPublic::Pair as Pair>::Public {
	TPublic::Pair::from_string(&format!("//{seed}"), None)
		.expect("static values are valid; qed")
		.public()
}

/// Helper function to generate an account ID from seed
fn get_account_id_from_seed<TPublic: Public>(seed: &str) -> AccountId
where
	AccountPublic: From<<TPublic::Pair as Pair>::Public>,
{
	AccountPublic::from(get_from_seed::<TPublic>(seed)).into_account()
}

/// Helper function to generate stash, controller and session key from seed
fn get_authority_keys_from_seed(
	seed: &str,
) -> (
	AccountId,
	AccountId,
	BabeId,
	GrandpaId,
	ValidatorId,
	AssignmentId,
	AuthorityDiscoveryId,
	BeefyId,
) {
	(
		get_account_id_from_seed::<sr25519::Public>(&format!("{seed}//stash")),
		get_account_id_from_seed::<sr25519::Public>(seed),
		get_from_seed::<BabeId>(seed),
		get_from_seed::<GrandpaId>(seed),
		get_from_seed::<ValidatorId>(seed),
		get_from_seed::<AssignmentId>(seed),
		get_from_seed::<AuthorityDiscoveryId>(seed),
		get_from_seed::<BeefyId>(seed),
	)
}

/// Build a bootstrap authority from an operator's **public** keys (no secrets in source).
///
/// `stash` is the validator account (sr25519 public, 32 bytes); it is reused as the controller.
/// The six session keys are the public halves of the operator's keystore entries — taken from
/// `author_rotateKeys` on their node and split with
/// `substitute-relay/tools/format-operator-keys.mjs`, or generated with
/// `substitute-relay/tools/derive-session-keys.mjs`. Crypto per key: `babe` sr25519, `grandpa`
/// ed25519, `para_validator`/`para_assignment`/`authority_discovery` sr25519, `beefy` ecdsa
/// (33-byte compressed).
#[allow(clippy::type_complexity)]
fn substitute_authority(
	stash: [u8; 32],
	babe: [u8; 32],
	grandpa: [u8; 32],
	para_validator: [u8; 32],
	para_assignment: [u8; 32],
	authority_discovery: [u8; 32],
	beefy: [u8; 33],
) -> (
	AccountId,
	AccountId,
	BabeId,
	GrandpaId,
	ValidatorId,
	AssignmentId,
	AuthorityDiscoveryId,
	BeefyId,
) {
	let stash = AccountId::from(stash);
	(
		stash.clone(),
		stash,
		babe.unchecked_into(),
		grandpa.unchecked_into(),
		para_validator.unchecked_into(),
		para_assignment.unchecked_into(),
		authority_discovery.unchecked_into(),
		beefy.unchecked_into(),
	)
}

fn testnet_accounts() -> Vec<AccountId> {
	vec![
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		get_account_id_from_seed::<sr25519::Public>("Bob"),
		get_account_id_from_seed::<sr25519::Public>("Charlie"),
		get_account_id_from_seed::<sr25519::Public>("Dave"),
		get_account_id_from_seed::<sr25519::Public>("Eve"),
		get_account_id_from_seed::<sr25519::Public>("Ferdie"),
		get_account_id_from_seed::<sr25519::Public>("Alice//stash"),
		get_account_id_from_seed::<sr25519::Public>("Bob//stash"),
		get_account_id_from_seed::<sr25519::Public>("Charlie//stash"),
		get_account_id_from_seed::<sr25519::Public>("Dave//stash"),
		get_account_id_from_seed::<sr25519::Public>("Eve//stash"),
		get_account_id_from_seed::<sr25519::Public>("Ferdie//stash"),
	]
}

fn default_parachains_host_configuration() -> HostConfiguration<polkadot_primitives::BlockNumber> {
	use polkadot_primitives::{MAX_CODE_SIZE, MAX_POV_SIZE};

	let executor_parameteres = ExecutorParams::from(
		&[
			MaxMemoryPages(8192),
			PvfExecTimeout(PvfExecKind::Backing, 2500),
			PvfExecTimeout(PvfExecKind::Approval, 15000),
		][..],
	);

	runtime_parachains::configuration::HostConfiguration {
		validation_upgrade_cooldown: 2u32,
		validation_upgrade_delay: 2,
		code_retention_period: 1200,
		max_code_size: MAX_CODE_SIZE,
		max_pov_size: MAX_POV_SIZE,
		max_head_data_size: 32 * 1024,
		max_upward_queue_count: 174172,
		max_upward_queue_size: 1024 * 1024,
		max_downward_message_size: 1024 * 1024,
		max_upward_message_size: 50 * 1024,
		max_upward_message_num_per_candidate: 16,
		hrmp_sender_deposit: 0,
		hrmp_recipient_deposit: 0,
		hrmp_channel_max_capacity: 1000,
		hrmp_channel_max_total_size: 100 * 1024,
		hrmp_max_parachain_inbound_channels: 10,
		hrmp_channel_max_message_size: 1024 * 1024,
		hrmp_max_parachain_outbound_channels: 10,
		hrmp_max_message_num_per_candidate: 10,
		dispute_period: 6,
		no_show_slots: 2,
		n_delay_tranches: 25,
		needed_approvals: 2,
		relay_vrf_modulo_samples: 2,
		zeroth_delay_tranche_width: 0,
		minimum_validation_upgrade_delay: 5,
		scheduler_params: polkadot_primitives::vstaging::SchedulerParams {
			group_rotation_frequency: 20,
			paras_availability_period: 4,
			lookahead: 3,
			..Default::default()
		},
		dispute_post_conclusion_acceptance_period: 100u32,
		minimum_backing_votes: 1,
		node_features: NodeFeatures::from_element(
			1u8 << (FeatureIndex::ElasticScalingMVP as usize) |
				1u8 << (FeatureIndex::EnableAssignmentsV2 as usize),
		),
		async_backing_params: AsyncBackingParams {
			max_candidate_depth: 3,
			allowed_ancestry_len: 2,
		},
		executor_params: executor_parameteres,
		max_validators: None,
		pvf_voting_ttl: 2,
		approval_voting_params: ApprovalVotingParams { max_approval_coalesce_count: 1 },
		max_relay_parent_session_age: 0,
	}
}

#[allow(clippy::type_complexity)]
fn paseo_testnet_genesis(
	initial_authorities: Vec<(
		AccountId,
		AccountId,
		BabeId,
		GrandpaId,
		ValidatorId,
		AssignmentId,
		AuthorityDiscoveryId,
		BeefyId,
	)>,
	root_key: AccountId,
	endowed_accounts: Option<Vec<AccountId>>,
) -> serde_json::Value {
	let endowed_accounts: Vec<AccountId> = endowed_accounts.unwrap_or_else(testnet_accounts);

	const ENDOWMENT: u128 = 1_000_000 * PAS;
	const STASH: u128 = 100 * PAS;

	serde_json::json!({
		"balances": {
			"balances": endowed_accounts.iter().map(|k| (k.clone(), ENDOWMENT)).collect::<Vec<_>>(),
		},
		"session": {
			"keys": initial_authorities
				.iter()
				.map(|x| {
					(
						x.0.clone(),
						x.0.clone(),
						paseo_session_keys(
							x.2.clone(),
							x.3.clone(),
							x.4.clone(),
							x.5.clone(),
							x.6.clone(),
							x.7.clone(),
						),
					)
				})
				.collect::<Vec<_>>(),
		},
		"staking": {
			"minimumValidatorCount": 1,
			"validatorCount": initial_authorities.len() as u32,
			"stakers": initial_authorities
				.iter()
				.map(|x| (x.0.clone(), x.0.clone(), STASH, StakerStatus::<AccountId>::Validator))
				.collect::<Vec<_>>(),
			"invulnerables": initial_authorities.iter().map(|x| x.0.clone()).collect::<Vec<_>>(),
			"forceEra": Forcing::NotForcing,
			"slashRewardFraction": Perbill::from_percent(10),
		},
		"sudo": {
			"key": Some(root_key),
		},
		"babe": {
			"epochConfig": Some(BABE_GENESIS_EPOCH_CONFIG),
		},
		"configuration": {
			"config": default_parachains_host_configuration(),
		},
	})
}

fn paseo_session_keys(
	babe: BabeId,
	grandpa: GrandpaId,
	para_validator: ValidatorId,
	para_assignment: AssignmentId,
	authority_discovery: AuthorityDiscoveryId,
	beefy: BeefyId,
) -> SessionKeys {
	SessionKeys { babe, grandpa, para_validator, para_assignment, authority_discovery, beefy }
}

pub fn paseo_local_testnet_genesis() -> serde_json::Value {
	paseo_testnet_genesis(
		vec![get_authority_keys_from_seed("Alice"), get_authority_keys_from_seed("Bob")],
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		None,
	)
}

pub fn paseo_development_config_genesis() -> serde_json::Value {
	paseo_testnet_genesis(
		vec![get_authority_keys_from_seed("Alice")],
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		None,
	)
}

/// Preset id for the substitute relay (fresh, from block 0).
pub const SUBSTITUTE_RUNTIME_PRESET: &str = "substitute";

/// Raw public bytes of the **current on-chain Paseo relay sudo key**, reused as the
/// substitute relay's genesis sudo. SS58 (Paseo, prefix 0):
/// `13uYxsEfJL5FYbJ1E7cW85ihp5LckYTyZT6Bqpc7tS4NAArK` (surveyed live 2026-06-30).
const SUBSTITUTE_SUDO: [u8; 32] = [
	0x80, 0x8c, 0xd3, 0x60, 0x29, 0xa4, 0x14, 0x2a, 0xd7, 0xd2, 0x55, 0xcd, 0x50, 0x4e, 0x82, 0x61,
	0x56, 0xfe, 0xe8, 0x6f, 0x45, 0x38, 0x41, 0xd3, 0x98, 0xf8, 0x74, 0x46, 0x7c, 0x7f, 0x6e, 0x0b,
];

/// Bootstrap core count for the substitute relay. With only 4 bootstrap validators a large
/// core count produces empty backing groups (groups are formed one-per-core), so start small
/// and raise post-launch via `coretime.request_core_count` as validators scale. Target:
/// 20 cores at 3 validators each (= 60 validators), driven by Asset Hub once staking is handed
/// over.
const SUBSTITUTE_BOOTSTRAP_CORES: u32 = 2;

/// Host configuration for the substitute relay.
///
/// This is a faithful **snapshot of the live Paseo relay `configuration.activeConfig`** (surveyed
/// at spec_version 2_003_001 on 2026-06-30), NOT the runtime's
/// `default_parachains_host_configuration` — the default has drifted from what governance has since
/// set on-chain (validation upgrade delays, HRMP channel limits, scheduler params, node features,
/// ...). We bring the running values over and override only what we deliberately change to
/// bootstrap from scratch, so the substitute relay behaves identically to the chain it replaces.
///
/// Deliberate override:
/// - `scheduler_params.num_cores`: live is 56; we start at [`SUBSTITUTE_BOOTSTRAP_CORES`] and raise
///   it post-launch via `coretime.request_core_count` as validators scale (4 bootstrap validators
///   cannot back 56 cores). `max_validators_per_core` is kept at the live value of 3 — already the
///   target ratio (60 validators / 20 cores).
fn substitute_host_configuration() -> HostConfiguration<polkadot_primitives::BlockNumber> {
	let executor_params = ExecutorParams::from(
		&[
			MaxMemoryPages(8192),
			PvfExecTimeout(PvfExecKind::Backing, 2500),
			PvfExecTimeout(PvfExecKind::Approval, 15000),
		][..],
	);

	runtime_parachains::configuration::HostConfiguration {
		validation_upgrade_cooldown: 60,
		validation_upgrade_delay: 60,
		code_retention_period: 1200,
		max_code_size: 3_500_000,
		max_pov_size: 10_485_760,
		max_head_data_size: 32 * 1024,
		max_upward_queue_count: 174_762,
		max_upward_queue_size: 1024 * 1024,
		max_downward_message_size: 51_200,
		max_upward_message_size: 65_531,
		max_upward_message_num_per_candidate: 16,
		hrmp_sender_deposit: 0,
		hrmp_recipient_deposit: 0,
		hrmp_channel_max_capacity: 1000,
		hrmp_channel_max_total_size: 102_400,
		hrmp_max_parachain_inbound_channels: 30,
		hrmp_channel_max_message_size: 102_400,
		hrmp_max_parachain_outbound_channels: 30,
		hrmp_max_message_num_per_candidate: 10,
		dispute_period: 6,
		dispute_post_conclusion_acceptance_period: 100,
		no_show_slots: 2,
		n_delay_tranches: 25,
		zeroth_delay_tranche_width: 0,
		needed_approvals: 2,
		relay_vrf_modulo_samples: 2,
		pvf_voting_ttl: 2,
		minimum_validation_upgrade_delay: 5,
		minimum_backing_votes: 1,
		node_features: NodeFeatures::from_element(
			1u8 << (FeatureIndex::EnableAssignmentsV2 as usize) |
				1u8 << (FeatureIndex::ElasticScalingMVP as usize) |
				1u8 << (FeatureIndex::CandidateReceiptV2 as usize),
		),
		approval_voting_params: ApprovalVotingParams { max_approval_coalesce_count: 1 },
		async_backing_params: AsyncBackingParams {
			max_candidate_depth: 3,
			allowed_ancestry_len: 2,
		},
		executor_params,
		max_validators: None,
		max_relay_parent_session_age: 0,
		scheduler_params: polkadot_primitives::vstaging::SchedulerParams {
			group_rotation_frequency: 10,
			paras_availability_period: 4,
			max_validators_per_core: Some(3),
			lookahead: 5,
			// Override: live is 56; bootstrap small, raise via `coretime.request_core_count`.
			num_cores: SUBSTITUTE_BOOTSTRAP_CORES,
			on_demand_queue_max_size: 100,
			on_demand_target_queue_utilization: Perbill::from_parts(250_000_000),
			on_demand_fee_variability: Perbill::from_parts(30_000_000),
			on_demand_base_fee: 10_000_000,
		},
	}
}

/// Genesis for the **substitute Paseo relay** — a fresh relay started from block 0 that will
/// take over once the current relay is wound down.
///
/// Boots in `pallet_staking_async_ah_client::OperatingMode::Passive` (the pallet default —
/// `StakingAhClient` genesis is intentionally omitted, so `Mode` falls back to `Passive`).
/// In Passive mode the relay self-elects the bootstrap authorities from the local
/// `pallet_staking` fallback; Asset Hub plays no part until the operator drives the staking
/// handover (`ah_client.set_mode(Buffered)` → `set_mode(Active)`). `forceEra = ForceNone` freezes
/// the bootstrap set for the whole Passive window so `ElectionProviderMultiPhase` never has to run
/// over the tiny set.
///
/// ⚠️ The four authorities are WELL-KNOWN dev keys (Alice/Bob/Charlie/Dave) — **substitute them
/// with real operator session keys before any real launch** (each operator derives their own and
/// the public keys replace the `get_authority_keys_from_seed(...)` entries below).
pub fn paseo_substitute_genesis() -> serde_json::Value {
	// Bootstrap authorities. Operators 1-2 (turboflakes) are real community providers; operators
	// 3-4 below are still well-known DEV keys (Charlie/Dave) — ⚠️ replace them with the remaining
	// providers' submitted keys before any real launch. Regenerate entries from a provider's stash
	// + `author_rotateKeys` blob with `substitute-relay/tools/format-operator-keys.mjs` (arg
	// order: stash, babe, grandpa, para_validator, para_assignment, authority_discovery, beefy).
	let initial_authorities = vec![
		// operator 1: turboflakes (turboflakes.io)
		substitute_authority(
			hex!["0efe248e3ddcfcb4f29675b70fc0a8e2db66b65381c45d299427b60d05f76108"],
			hex!["7a412f2591679f757469c1cce3cbb489b58a65c88bec143580a6b08467c0d66f"],
			hex!["8fabe46fdd0f4df9a72ce250df69dc52afd5e63cf44d2abf0ddde4b1fc2d5568"],
			hex!["18607bb3dd6531be191d48d47bb1c01d2443edbd7a92ffa46dc26dc1d8dccc48"],
			hex!["b082cc08a2ca975d4a2cc8f6005ad04d7c60bd9412268eacc97123ec2f4f5945"],
			hex!["40a020e8bc4f7ad6ba890557e915c581b98da880302c34d8c7c0ba483a933a33"],
			hex!["02cc3ddd8bbe9038048a5803f093ff00770d6ecc14dcad38f11b1496a8abd15704"],
		),
		// operator 2: turboflakes.io/01
		substitute_authority(
			hex!["784ab3c8324d8d957ccd8be0b3ccaf006b77b503dc0b89194657c4dc6ecc222b"],
			hex!["8819d545b5476790fd97a58a4f705208ac52c57cccee2237fbf37579fa4ea547"],
			hex!["119a070623bc5088013c1a3f9037e177285c654c748136a4c6d85e03ca0e64e8"],
			hex!["f4511ed75a8c202d33cc6022df5a9e5420ca02f6f738f0e9a670c4b40edaff4d"],
			hex!["921fa6442cfc257aa8d213e88e5351540f7148bda0e8b71248b33adbabd3c155"],
			hex!["7c0a746c40f2f8a81203ebc56de2d4f9c38b0dcc54437ec26d3ba53e6f4e0c7a"],
			hex!["0358681e51bad908404c2c8108f70928dd34568e63553aa1a781fccfca364b4991"],
		),
		// dev-charlie (PLACEHOLDER — replace with operator 3)
		substitute_authority(
			hex!["1e07379407fecc4b89eb7dbd287c2c781cfb1907a96947a3eb18e4f8e7198625"],
			hex!["90b5ab205c6974c9ea841be688864633dc9ca8a357843eeacf2314649965fe22"],
			hex!["439660b36c6c03afafca027b910b4fecf99801834c62a5e6006f27d978de234f"],
			hex!["90b5ab205c6974c9ea841be688864633dc9ca8a357843eeacf2314649965fe22"],
			hex!["90b5ab205c6974c9ea841be688864633dc9ca8a357843eeacf2314649965fe22"],
			hex!["90b5ab205c6974c9ea841be688864633dc9ca8a357843eeacf2314649965fe22"],
			hex!["0389411795514af1627765eceffcbd002719f031604fadd7d188e2dc585b4e1afb"],
		),
		// dev-dave (PLACEHOLDER — replace with operator 4)
		substitute_authority(
			hex!["e860f1b1c7227f7c22602f53f15af80747814dffd839719731ee3bba6edc126c"],
			hex!["306721211d5404bd9da88e0204360a1a9ab8b87c66c1bc2fcdd37f3c2222cc20"],
			hex!["5e639b43e0052c47447dac87d6fd2b6ec50bdd4d0f614e4299c665249bbd09d9"],
			hex!["306721211d5404bd9da88e0204360a1a9ab8b87c66c1bc2fcdd37f3c2222cc20"],
			hex!["306721211d5404bd9da88e0204360a1a9ab8b87c66c1bc2fcdd37f3c2222cc20"],
			hex!["306721211d5404bd9da88e0204360a1a9ab8b87c66c1bc2fcdd37f3c2222cc20"],
			hex!["03bc9d0ca094bd5b8b3225d7651eac5d18c1c04bf8ae8f8b263eebca4e1410ed0c"],
		),
	];
	// Reuse the current on-chain relay sudo key.
	let root_key: AccountId = AccountId::from(SUBSTITUTE_SUDO);

	const ENDOWMENT: u128 = 1_000_000 * PAS;
	const STASH: u128 = 100 * PAS;

	// Minimal endowment: sudo + each authority's stash and controller account.
	let mut endowed_accounts: Vec<AccountId> =
		initial_authorities.iter().flat_map(|x| [x.0.clone(), x.1.clone()]).collect();
	endowed_accounts.push(root_key.clone());
	endowed_accounts.sort();
	endowed_accounts.dedup();

	serde_json::json!({
		"balances": {
			"balances": endowed_accounts.iter().map(|k| (k.clone(), ENDOWMENT)).collect::<Vec<_>>(),
		},
		"session": {
			"keys": initial_authorities
				.iter()
				.map(|x| {
					(
						x.0.clone(),
						x.0.clone(),
						paseo_session_keys(
							x.2.clone(),
							x.3.clone(),
							x.4.clone(),
							x.5.clone(),
							x.6.clone(),
							x.7.clone(),
						),
					)
				})
				.collect::<Vec<_>>(),
		},
		"staking": {
			"minimumValidatorCount": 1,
			"validatorCount": initial_authorities.len() as u32,
			"stakers": initial_authorities
				.iter()
				.map(|x| (x.0.clone(), x.0.clone(), STASH, StakerStatus::<AccountId>::Validator))
				.collect::<Vec<_>>(),
			"invulnerables": initial_authorities.iter().map(|x| x.0.clone()).collect::<Vec<_>>(),
			"forceEra": Forcing::ForceNone,
			"slashRewardFraction": Perbill::from_percent(10),
		},
		"sudo": {
			"key": Some(root_key),
		},
		"babe": {
			"epochConfig": Some(BABE_GENESIS_EPOCH_CONFIG),
		},
		"configuration": {
			"config": substitute_host_configuration(),
		},
	})
}

/// Provides the names of the predefined genesis configs for this runtime.
pub fn preset_names() -> Vec<PresetId> {
	vec![
		PresetId::from(sp_genesis_builder::DEV_RUNTIME_PRESET),
		PresetId::from(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET),
		PresetId::from(SUBSTITUTE_RUNTIME_PRESET),
	]
}

/// Provides the JSON representation of predefined genesis config for given `id`.
pub fn get_preset(id: &sp_genesis_builder::PresetId) -> Option<Vec<u8>> {
	let patch = match id.as_ref() {
		sp_genesis_builder::DEV_RUNTIME_PRESET => paseo_development_config_genesis(),
		sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET => paseo_local_testnet_genesis(),
		SUBSTITUTE_RUNTIME_PRESET => paseo_substitute_genesis(),
		_ => return None,
	};
	Some(
		serde_json::to_string(&patch)
			.expect("serialization to json is expected to work. qed.")
			.into_bytes(),
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn default_parachains_host_configuration_is_consistent() {
		default_parachains_host_configuration().panic_if_not_consistent();
	}
}
