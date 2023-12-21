use hex_literal::hex;
use pallet_im_online::sr25519::AuthorityId as ImOnlineId;
use pallet_staking::Forcing;
use paseo_runtime_constants::currency::UNITS as PAS;
use polkadot_primitives::{AccountId, AccountPublic, AssignmentId, ValidatorId};
use polkadot_runtime_parachains::configuration::HostConfiguration;
use sc_chain_spec::{ChainSpec, ChainType, NoExtension};
use sc_consensus_grandpa::AuthorityId as GrandpaId;
use sp_authority_discovery::AuthorityId as AuthorityDiscoveryId;
use sp_consensus_babe::AuthorityId as BabeId;
use sp_consensus_beefy::ecdsa_crypto::AuthorityId as BeefyId;
use sp_core::{crypto::UncheckedInto, sr25519, Pair, Public};
use sp_runtime::{traits::IdentifyAccount, AccountId32, Perbill};

pub type PaseoChainSpec =
    sc_chain_spec::GenericChainSpec<paseo_runtime::RuntimeGenesisConfig, NoExtension>;

const DEFAULT_PROTOCOL_ID: &str = "pas";

/// Returns the properties for the [`PaseoChainSpec`].
pub fn paseo_chain_spec_properties() -> serde_json::map::Map<String, serde_json::Value> {
    serde_json::json!({
        "tokenDecimals": 10,
        "ss58Format": 42,
        "tokenSymbol": "PAS"
    })
    .as_object()
    .expect("Map given; qed")
    .clone()
}

fn default_parachains_host_configuration() -> HostConfiguration<polkadot_primitives::BlockNumber> {
    use polkadot_primitives::{MAX_CODE_SIZE, MAX_POV_SIZE};

    polkadot_runtime_parachains::configuration::HostConfiguration {
        validation_upgrade_cooldown: 2u32,
        validation_upgrade_delay: 2,
        code_retention_period: 1200,
        max_code_size: MAX_CODE_SIZE,
        max_pov_size: MAX_POV_SIZE,
        max_head_data_size: 32 * 1024,
        group_rotation_frequency: 20,
        paras_availability_period: 4,
        max_upward_queue_count: 8,
        max_upward_queue_size: 1024 * 1024,
        max_downward_message_size: 1024 * 1024,
        max_upward_message_size: 50 * 1024,
        max_upward_message_num_per_candidate: 5,
        hrmp_sender_deposit: 0,
        hrmp_recipient_deposit: 0,
        hrmp_channel_max_capacity: 8,
        hrmp_channel_max_total_size: 8 * 1024,
        hrmp_max_parachain_inbound_channels: 4,
        hrmp_channel_max_message_size: 1024 * 1024,
        hrmp_max_parachain_outbound_channels: 4,
        hrmp_max_message_num_per_candidate: 5,
        dispute_period: 6,
        no_show_slots: 2,
        n_delay_tranches: 25,
        needed_approvals: 2,
        relay_vrf_modulo_samples: 2,
        zeroth_delay_tranche_width: 0,
        minimum_validation_upgrade_delay: 5,
        ..Default::default()
    }
}
fn paseo_session_keys(
    babe: BabeId,
    grandpa: GrandpaId,
    im_online: ImOnlineId,
    para_validator: ValidatorId,
    para_assignment: AssignmentId,
    authority_discovery: AuthorityDiscoveryId,
) -> paseo_runtime::SessionKeys {
    paseo_runtime::SessionKeys {
        babe,
        grandpa,
        im_online,
        para_validator,
        para_assignment,
        authority_discovery,
    }
}
/// Helper function to generate a crypto pair from seed
pub fn get_from_seed<TPublic: Public>(seed: &str) -> <TPublic::Pair as Pair>::Public {
    TPublic::Pair::from_string(&format!("//{}", seed), None)
        .expect("static values are valid; qed")
        .public()
}

/// Helper function to generate an account ID from seed
pub fn get_account_id_from_seed<TPublic: Public>(seed: &str) -> AccountId
where
    AccountPublic: From<<TPublic::Pair as Pair>::Public>,
{
    AccountPublic::from(get_from_seed::<TPublic>(seed)).into_account()
}

/// Helper function to generate stash, controller and session key from seed
pub fn get_authority_keys_from_seed(
    seed: &str,
) -> (
    AccountId,
    AccountId,
    BabeId,
    GrandpaId,
    ImOnlineId,
    ValidatorId,
    AssignmentId,
    AuthorityDiscoveryId,
    BeefyId,
) {
    let keys = get_authority_keys_from_seed_no_beefy(seed);
    (
        keys.0,
        keys.1,
        keys.2,
        keys.3,
        keys.4,
        keys.5,
        keys.6,
        keys.7,
        get_from_seed::<BeefyId>(seed),
    )
}

/// Helper function to generate stash, controller and session key from seed
pub fn get_authority_keys_from_seed_no_beefy(
    seed: &str,
) -> (
    AccountId,
    AccountId,
    BabeId,
    GrandpaId,
    ImOnlineId,
    ValidatorId,
    AssignmentId,
    AuthorityDiscoveryId,
) {
    (
        get_account_id_from_seed::<sr25519::Public>(&format!("{}//stash", seed)),
        get_account_id_from_seed::<sr25519::Public>(seed),
        get_from_seed::<BabeId>(seed),
        get_from_seed::<GrandpaId>(seed),
        get_from_seed::<ImOnlineId>(seed),
        get_from_seed::<ValidatorId>(seed),
        get_from_seed::<AssignmentId>(seed),
        get_from_seed::<AuthorityDiscoveryId>(seed),
    )
}

/// Helper function to generate stash, controller and session key from seed
pub fn generate_paseo_session_keys(
    seed: &str,
) -> (
    AccountId,
    AccountId,
    BabeId,
    GrandpaId,
    ImOnlineId,
    ValidatorId,
    AssignmentId,
    AuthorityDiscoveryId,
) {
    (
        get_account_id_from_seed::<sr25519::Public>(&format!("{}//stash", seed)),
        get_account_id_from_seed::<sr25519::Public>(seed),
        get_from_seed::<BabeId>(seed),
        get_from_seed::<GrandpaId>(seed),
        get_from_seed::<ImOnlineId>(seed),
        get_from_seed::<ValidatorId>(seed),
        get_from_seed::<AssignmentId>(seed),
        get_from_seed::<AuthorityDiscoveryId>(seed),
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

pub fn paseo_genesis(
    wasm_binary: &[u8],
    initial_authorities: Vec<(
        AccountId,
        AccountId,
        BabeId,
        GrandpaId,
        ImOnlineId,
        ValidatorId,
        AssignmentId,
        AuthorityDiscoveryId,
    )>,
    root_key: AccountId,
    endowed_accounts: Option<Vec<AccountId>>,
) -> paseo_runtime::RuntimeGenesisConfig {
    let endowed_accounts: Vec<AccountId> = endowed_accounts.unwrap_or_else(testnet_accounts);

    const ENDOWMENT: u128 = 1_000_000_000_000_000 * PAS;
    const STASH: u128 = 1_000_000_000 * PAS;

    paseo_runtime::RuntimeGenesisConfig {
        system: paseo_runtime::SystemConfig {
            code: wasm_binary.to_vec(),
            ..Default::default()
        },
        indices: paseo_runtime::IndicesConfig { indices: vec![] },
        balances: paseo_runtime::BalancesConfig {
            balances: endowed_accounts
                .iter()
                .map(|k| (k.clone(), ENDOWMENT))
                .collect(),
        },
        session: paseo_runtime::SessionConfig {
            keys: initial_authorities
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
        staking: paseo_runtime::StakingConfig {
            minimum_validator_count: 1,
            validator_count: initial_authorities.len() as u32,
            stakers: initial_authorities
                .iter()
                .map(|x| {
                    (
                        x.0.clone(),
                        x.0.clone(),
                        STASH,
                        paseo_runtime::StakerStatus::Validator,
                    )
                })
                .collect(),
            invulnerables: initial_authorities.iter().map(|x| x.0.clone()).collect(),
            force_era: Forcing::NotForcing,
            slash_reward_fraction: Perbill::from_percent(10),
            ..Default::default()
        },
        babe: paseo_runtime::BabeConfig {
            authorities: Default::default(),
            epoch_config: Some(paseo_runtime::BABE_GENESIS_EPOCH_CONFIG),
            ..Default::default()
        },
        grandpa: Default::default(),
        im_online: Default::default(),
        authority_discovery: paseo_runtime::AuthorityDiscoveryConfig {
            keys: vec![],
            ..Default::default()
        },
        claims: paseo_runtime::ClaimsConfig {
            claims: vec![],
            vesting: vec![],
        },
        vesting: paseo_runtime::VestingConfig { vesting: vec![] },
        treasury: Default::default(),
        hrmp: Default::default(),
        configuration: paseo_runtime::ConfigurationConfig {
            config: default_parachains_host_configuration(),
        },
        paras: Default::default(),
        xcm_pallet: Default::default(),
        nomination_pools: Default::default(),
        sudo: paseo_runtime::SudoConfig {
            key: Some(root_key),
        },
    }
}

fn paseo_config_genesis(wasm_binary: &[u8]) -> paseo_runtime::RuntimeGenesisConfig {
    type Sessions = (
        AccountId,
        AccountId,
        BabeId,
        GrandpaId,
        ImOnlineId,
        ValidatorId,
        AssignmentId,
        AuthorityDiscoveryId,
    );

    let stash_paradox: AccountId32 =
        hex!("043393e76c137dfdc403a6fd9a2d6129d470d51c5a67bd40517378030c87170d").into();
    let stash_stake_plus: AccountId32 =
        hex!("82c3105dbd4bb206428d8a8b7ea1f19965a0668dd583b06c3b75daa181fe654c").into();
    let stash_amforc: AccountId32 =
        hex!("32eebacd223f4aef33d98a667a68f9e371f40384257c6d31030952b9d94e1152").into();

    let paradox: Sessions = (
        stash_paradox, // stash account (sr25519/1)
        stash_paradox, // stash account  (sr25519/1)
        hex!("b07d600e3487e2712dcc3879c7b17c9b29cd2243b45f0d9343c591b89cf82a65").unchecked_into(), // babe key (sr25519/2)
        hex!("c8caee6f6eddc41c6cc55e554343392cbc13d2a8a57b97f6f85fc965bdd20ce8").unchecked_into(), // grandpa key (ed25519)
        hex!("0edf2a41cb81178704560b02c35f5e01a5a97a568ebc10c025ade18b6ab2fa1d").unchecked_into(), // im online key (sr25519/2)
        hex!("161d0af40e6efc165c17d0189bd2d770bdfa0a9b8393cb89113f473a2e948c68").unchecked_into(), // validator key (sr25519/2)
        hex!("def964eed9a73f8a6610f1a0373378dca6f277eb7787869ed5841893105ad930").unchecked_into(), // assignment key (sr25519/2)
        hex!("f89c97bf5b2c07c05c84eebce4ffc7b28766946c03741fd1a71fdae0942e8768").unchecked_into(), // authority discovery key (sr25519/2)
    );

    let stake_plus: Sessions = (
        stash_stake_plus, // stash account (sr25519/1)
        stash_stake_plus, // stash account  (sr25519/1)
        hex!("74b6c7c8fdf1e3bfd09c0bdd8216f3c4073f66e2687f99b3c50a564ea5b87f58").unchecked_into(), // babe key (sr25519/2)
        hex!("a9641167f560ad26ebadb38939bd4e0bc2143d4e8b60286dd291d08b323fc684").unchecked_into(), // grandpa key (ed25519)
        hex!("74b6c7c8fdf1e3bfd09c0bdd8216f3c4073f66e2687f99b3c50a564ea5b87f58").unchecked_into(), // im online key (sr25519/2)
        hex!("74b6c7c8fdf1e3bfd09c0bdd8216f3c4073f66e2687f99b3c50a564ea5b87f58").unchecked_into(), // validator key (sr25519/2)
        hex!("74b6c7c8fdf1e3bfd09c0bdd8216f3c4073f66e2687f99b3c50a564ea5b87f58").unchecked_into(), // assignment key (sr25519/2)
        hex!("74b6c7c8fdf1e3bfd09c0bdd8216f3c4073f66e2687f99b3c50a564ea5b87f58").unchecked_into(), // authority discovery key (sr25519/2)
    );

    let amforc: Sessions = (
        stash_amforc, // stash account (sr25519/1)
        stash_amforc, // stash account  (sr25519/1)
        hex!("58108e1651614afc6a535c426fc013945e93533faa33819fe4e69423fe323302").unchecked_into(), // babe key (sr25519/2)
        hex!("8270a62b61639ee56113834aecec01de6cda91413a5111b89f74d6585da34f50").unchecked_into(), // grandpa key (ed25519)
        hex!("58108e1651614afc6a535c426fc013945e93533faa33819fe4e69423fe323302").unchecked_into(), // im online key (sr25519/2)
        hex!("58108e1651614afc6a535c426fc013945e93533faa33819fe4e69423fe323302").unchecked_into(), // validator key (sr25519/2)
        hex!("58108e1651614afc6a535c426fc013945e93533faa33819fe4e69423fe323302").unchecked_into(), // assignment key (sr25519/2)
        hex!("58108e1651614afc6a535c426fc013945e93533faa33819fe4e69423fe323302").unchecked_into(), // authority discovery key (sr25519/2)
    );

    /* const DWELLIR: Sessions = (
        hex!("9492b8c38442c79061bdbb8d38dcd28138938a7fd476edf89ecdec06a5a9d20f").into(), // stash account (sr25519/1)
        hex!("9492b8c38442c79061bdbb8d38dcd28138938a7fd476edf89ecdec06a5a9d20f").into(), // stash account  (sr25519/1)
        hex!(NOT_PROVIDED_YET).unchecked_into(), // babe key (sr25519/2)
        hex!(NOT_PROVIDED_YET).unchecked_into(), // grandpa key (ed25519)
        hex!(NOT_PROVIDED_YET).unchecked_into(), // im online key (sr25519/2)
        hex!(NOT_PROVIDED_YET).unchecked_into(), // validator key (sr25519/2)
        hex!(NOT_PROVIDED_YET).unchecked_into(), // assignment key (sr25519/2)
        hex!(NOT_PROVIDED_YET).unchecked_into(), // authority discovery key (sr25519/2)
    ); */

    let initial_paseo_validators: Vec<Sessions> = vec![paradox, stake_plus, amforc];

    paseo_genesis(
        wasm_binary,
        // initial authorities
        initial_paseo_validators,
        //root key
        get_account_id_from_seed::<sr25519::Public>("Alice"),
        // endowed accounts
        Some(vec![stash_paradox, stash_stake_plus, stash_amforc]),
    )
}

/// Paseo config
pub fn paseo_config() -> Result<Box<dyn ChainSpec>, String> {
    let wasm_binary = paseo_runtime::WASM_BINARY.ok_or("Paseo wasm not available")?;

    Ok(Box::new(PaseoChainSpec::from_genesis(
        "Paseo Testnet",
        "paseo",
        ChainType::Live,
        move || paseo_config_genesis(wasm_binary),
        vec![],
        None,
        Some(DEFAULT_PROTOCOL_ID),
        None,
        Some(paseo_chain_spec_properties()),
        Default::default(),
    )))
}

fn paseo_local_genesis(wasm_binary: &[u8]) -> paseo_runtime::RuntimeGenesisConfig {
    paseo_genesis(
        wasm_binary,
        // initial authorities
        vec![
            get_authority_keys_from_seed_no_beefy("Alice"),
            get_authority_keys_from_seed_no_beefy("Bob"),
        ],
        //root key
        get_account_id_from_seed::<sr25519::Public>("Alice"),
        // endowed accounts
        None,
    )
}

/// Paseo local config (multivalidator Alice + Bob)
pub fn paseo_local_config() -> Result<Box<dyn ChainSpec>, String> {
    let wasm_binary = paseo_runtime::WASM_BINARY.ok_or("Paseo development wasm not available")?;

    Ok(Box::new(PaseoChainSpec::from_genesis(
        "Paseo Local Testnet",
        "paseo_local",
        ChainType::Local,
        move || paseo_local_genesis(wasm_binary),
        vec![],
        None,
        Some(DEFAULT_PROTOCOL_ID),
        None,
        Some(paseo_chain_spec_properties()),
        Default::default(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_parachains_host_configuration_is_consistent() {
        default_parachains_host_configuration().panic_if_not_consistent();
    }
}
