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
    beefy: BeefyId,
) -> paseo_runtime::SessionKeys {
    paseo_runtime::SessionKeys {
        babe,
        grandpa,
        im_online,
        para_validator,
        para_assignment,
        authority_discovery,
        beefy,
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
	(keys.0, keys.1, keys.2, keys.3, keys.4, keys.5, keys.6, keys.7, get_from_seed::<BeefyId>(seed))
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
        BeefyId
    )>,
    root_key: AccountId,
    endowed_accounts: Option<Vec<AccountId>>,
) -> paseo_runtime::RuntimeGenesisConfig {
    let endowed_accounts: Vec<AccountId> = endowed_accounts.unwrap_or_else(testnet_accounts);

    const ENDOWMENT: u128 = 1_000_000 * PAS; // 1M PAS
    const ROOT_ENDOWMENT: u128 = 100_000_000 * PAS; // 100M PAS
    const STASH: u128 = 1_000_00 * PAS; // 100k PAS

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
                            x.8.clone()
                        ),
                    )
                })
                .collect::<Vec<_>>(),
        },
        staking: paseo_runtime::StakingConfig {
            minimum_validator_count: 2,
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
            min_nominator_bond: 2_500_000_000_000, // 250 PAS
            min_validator_bond: STASH,
            max_validator_count: Some(200),
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
        beefy: Default::default()
    }
}

fn paseo_local_genesis(wasm_binary: &[u8]) -> paseo_runtime::RuntimeGenesisConfig {
    paseo_genesis(
        wasm_binary,
        // initial authorities
        vec![get_authority_keys_from_seed("Alice"),get_authority_keys_from_seed("Charlie"),get_authority_keys_from_seed("Bob")],
        //root key
        get_account_id_from_seed::<sr25519::Public>("Alice"),
        // endowed accounts
        None,
    )
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
        BeefyId
    );

    let root_key: AccountId32 =
        hex!("7e939ef17e229e9a29210d95cb0b607e0030d54899c05f791a62d5c6f4557659").into();

    let initial_paseo_validators: Vec<Sessions> = vec![
        (
            hex!("94c4156ed6a101ae478a3de3ba70a05fce8a3d67be6fb85f33bfcf2777ab6b10").into(), // stash account (sr25519/1)
            hex!("94c4156ed6a101ae478a3de3ba70a05fce8a3d67be6fb85f33bfcf2777ab6b10").into(), // stash account  (sr25519/1)
            hex!("18bd0f67d77f04f1a92400421813d8927fad109b40a8689254a5f0c8b346857c").unchecked_into(), // babe key (sr25519/2)
            hex!("6e60f1e253735fb113c183fa603f591e4456435171f387c0849001b428b5ccb1").unchecked_into(), // grandpa key (ed25519)
            hex!("d28145a7cde195a4c834276730d30f074b212a150e770931ee9470e853e7d224").unchecked_into(), // im online key (sr25519/2)
            hex!("02683131f96baec9121383995904c49a02ce2c2451f8038291e5db2dce66663e").unchecked_into(), // validator key (sr25519/2)
            hex!("92ac14f8ad1811cc83861afadf12f3191cca1391f1f3af705977faa2fa2bf46a").unchecked_into(), // assignment key (sr25519/2)
            hex!("629f9fd0dd7279c7af7470472d1208a13e33239b484974d47cffce4ad4785644").unchecked_into(), // authority discovery key (sr25519/2)
            get_from_seed::<BeefyId>("1")
        ),
        (
            hex!("82c3105dbd4bb206428d8a8b7ea1f19965a0668dd583b06c3b75daa181fe654c").into(), // stash account (sr25519/1)
            hex!("82c3105dbd4bb206428d8a8b7ea1f19965a0668dd583b06c3b75daa181fe654c").into(), // stash account  (sr25519/1)
            hex!("facb2f987caac6c1290a9784b1efdba78343d39aed805addb12945efbe444000").unchecked_into(), // babe key (sr25519/2)
            hex!("4c669b04865e9acaf7b72bdfcb0099d70d9ec63c8c2d6b8cb0552815d7b50a0a").unchecked_into(), // grandpa key (ed25519)
            hex!("ca3c2703db1633a27eff681d979967988c3a6752c669fd41f1abde10f3b05446").unchecked_into(), // im online key (sr25519/2)
            hex!("2253ee3c02d89582602ca5b0570cfc01dc82cc8d1b9d2071eb5db6318749124b").unchecked_into(), // validator key (sr25519/2)
            hex!("f0e6c42698fffc28f9fc769fddcdf165af54c171cde43690cc8f73c853de1f04").unchecked_into(), // assignment key (sr25519/2)
            hex!("26e2fc857945d01520797a75388c58e710c9fefedd28387af70880f1682be41e").unchecked_into(), // authority discovery key (sr25519/2)
            get_from_seed::<BeefyId>("2")
        ),
        (
            hex!("6ed774481ff68097867271bc8ecaeee3500817ccefdfda74ceeafd32a2259627").into(), // stash account (sr25519/1)
            hex!("6ed774481ff68097867271bc8ecaeee3500817ccefdfda74ceeafd32a2259627").into(), // stash account  (sr25519/1)
            hex!("b2efe7e70daf44b3466c63ccbf4487f42c6a9f6fbb7050b849691e36ce92e347").unchecked_into(), // babe key (sr25519/2)
            hex!("0509f9caf32fda5584343c473b386c433acb99fd9400724b8cf3c618d840133f").unchecked_into(), // grandpa key (ed25519)
            hex!("bef47a9e4b47ed57461e1d28cac7da327a52ebcd64d74080d31deb3ac7a7645e").unchecked_into(), // im online key (sr25519/2)
            hex!("4e1a59090261a7e6bd82544df1eebd96dc87b4eb1211346645fa44d6b932960b").unchecked_into(), // validator key (sr25519/2)
            hex!("e4ca45c68b0248885d190d22068c6628ee2f00d9fa0706d5a5c1c8456369f03e").unchecked_into(), // assignment key (sr25519/2)
            hex!("aa3955187f755708cd6a8104314b962ff5043e36efa3ec5d84df40c58b442221").unchecked_into(), // authority discovery key (sr25519/2)
            get_from_seed::<BeefyId>("3")
        ),
        (
            hex!("1e30aa51ad68b8918d2c46e914986818c111bee03582610cbc9fb73fe0e4c413").into(), // stash account (sr25519/1)
            hex!("1e30aa51ad68b8918d2c46e914986818c111bee03582610cbc9fb73fe0e4c413").into(), // stash account  (sr25519/1)
            hex!("74dacbca0cdb5099afef67e622c147614198e669931cebc605629da332632473").unchecked_into(), // babe key (sr25519/2)
            hex!("5fafb6219eb8d463bec0370b2aab69f45fc780959fc2eddbc7703760aa342022").unchecked_into(), // grandpa key (ed25519)
            hex!("b414aa148096a92a1831309f758f944725653363ccbaeb21817b7df5784b8d46").unchecked_into(), // im online key (sr25519/2)
            hex!("c45a6d0878f808b1baaa85dcfb4e930ae06e3205bb38855527aee6f259e3327b").unchecked_into(), // validator key (sr25519/2)
            hex!("2e2f75472708a497d1743f52b04edf26c250d9e6d220f3bae3d176f02f8e586c").unchecked_into(), // assignment key (sr25519/2)
            hex!("2ada042fb4bbfd9b6d8c48293ffc4a7722632c843a67e608554c41d06aabc413").unchecked_into(), // authority discovery key (sr25519/2)
            get_from_seed::<BeefyId>("4")
        ),
        (
            hex!("0efe248e3ddcfcb4f29675b70fc0a8e2db66b65381c45d299427b60d05f76108").into(), // stash account (sr25519/1)
            hex!("0efe248e3ddcfcb4f29675b70fc0a8e2db66b65381c45d299427b60d05f76108").into(), // stash account  (sr25519/1)
            hex!("5440add43e5388a81aef665c9086d386c0be0ce75e4f8a4a3d8168e976ea821f").unchecked_into(), // babe key (sr25519/2)
            hex!("83f43ee3e4521b55de0fe830847fda88a6b017b87979af1a41b180c39da1e4b0").unchecked_into(), // grandpa key (ed25519)
            hex!("98aab6f52520022d011a6eba2dca1c6327edbbcd753c170dcf0e9118b5f0f25b").unchecked_into(), // im online key (sr25519/2)
            hex!("d0d052eca7d732d9f560ba970ca48f67387b899e76958ea6ed342a3a553ef022").unchecked_into(), // validator key (sr25519/2)
            hex!("3e547f5cc3455a61d0404d7296ceec7375cbe20322109d118c5e725b1a5cbf04").unchecked_into(), // assignment key (sr25519/2)
            hex!("cce3ec06252cf7cdad47fe1265047a9bbddb9059ee4bdc6dec83b67249b4a934").unchecked_into(), // authority discovery key (sr25519/2)
            get_from_seed::<BeefyId>("5")
        ),
        (
            hex!("043393e76c137dfdc403a6fd9a2d6129d470d51c5a67bd40517378030c87170d").into(), // stash account (sr25519/1)
            hex!("043393e76c137dfdc403a6fd9a2d6129d470d51c5a67bd40517378030c87170d").into(), // stash account  (sr25519/1)
            hex!("b07d600e3487e2712dcc3879c7b17c9b29cd2243b45f0d9343c591b89cf82a65").unchecked_into(), // babe key (sr25519/2)
            hex!("c8caee6f6eddc41c6cc55e554343392cbc13d2a8a57b97f6f85fc965bdd20ce8").unchecked_into(), // grandpa key (ed25519)
            hex!("0edf2a41cb81178704560b02c35f5e01a5a97a568ebc10c025ade18b6ab2fa1d").unchecked_into(), // im online key (sr25519/2)
            hex!("161d0af40e6efc165c17d0189bd2d770bdfa0a9b8393cb89113f473a2e948c68").unchecked_into(), // validator key (sr25519/2)
            hex!("def964eed9a73f8a6610f1a0373378dca6f277eb7787869ed5841893105ad930").unchecked_into(), // assignment key (sr25519/2)
            hex!("f89c97bf5b2c07c05c84eebce4ffc7b28766946c03741fd1a71fdae0942e8768").unchecked_into(), // authority discovery key (sr25519/2)
            get_from_seed::<BeefyId>("6")
        ),
        (
            hex!("d41a8a9678862ebe9d1a1d59cac1c8430ef31e282f9cb391cf6f2b4d9ce2fd3d").into(), // stash account (sr25519/1)
            hex!("d41a8a9678862ebe9d1a1d59cac1c8430ef31e282f9cb391cf6f2b4d9ce2fd3d").into(), // stash account  (sr25519/1)
            hex!("1ab03b1b3277edfedd24ef3d3359b449b64bd95ed82a04e7f9fbaab7b71dc015").unchecked_into(), // babe key (sr25519/2)
            hex!("64adb43a7628139f6c02100f6a5465dbd33422418426c572b12547c5a665008c").unchecked_into(), // grandpa key (ed25519)
            hex!("2cd12c731d91441f0114b08d314cd3f9a9f7fd0240d467fe54adefbee4d90762").unchecked_into(), // im online key (sr25519/2)
            hex!("441629077e228528f91ca7dc17051bb437408a5ae272d0950e58961846a8fc2e").unchecked_into(), // validator key (sr25519/2)
            hex!("c22ce14ba0e59aa974d4a05c9208ba5ae18674c6a23c9998d91e7d1ebea7e06b").unchecked_into(), // assignment key (sr25519/2)
            hex!("9a86227e204a2d003399c2a3b50c2c869c4380c195a014a02f6d2e7048941237").unchecked_into(), // authority discovery key (sr25519/2)
            get_from_seed::<BeefyId>("7")
        ),
        (
            hex!("68728d12a90fb1a9f5b0f0b2814d730401d314964113554e66ff19e7067d7c69").into(), // stash account (sr25519/1)
            hex!("68728d12a90fb1a9f5b0f0b2814d730401d314964113554e66ff19e7067d7c69").into(), // stash account  (sr25519/1)
            hex!("c44b3e8efe854419ccd5801a82ada22d39cfccdbcece382304cdfeac81ebe402").unchecked_into(), // babe key (sr25519/2)
            hex!("6e309dfa4c8de814cad140b8612a9e41bfba244f9ab1468e1b5d9b3cc1f5e565").unchecked_into(), // grandpa key (ed25519)
            hex!("a8a03d86e6c0dbe180cadfc7994121f462b28f7a8cb1be7e0e354147624be734").unchecked_into(), // im online key (sr25519/2)
            hex!("0a988fb965b156a07debf072fd9d34a9c7c0fc0e0ff5bd63ef766afb76e2b328").unchecked_into(), // validator key (sr25519/2)
            hex!("9ef622d2467ed115fa0c6c86303e1ef6a08a0609c97e616aa69b026a6d3f2663").unchecked_into(), // assignment key (sr25519/2)
            hex!("729053f28155071474b4686323db5f7a318cb3f088b76660cc8ff5e3e11ec32e").unchecked_into(), // authority discovery key (sr25519/2)
            get_from_seed::<BeefyId>("8")
        ),
        (
            hex!("d0b4896a63b672a7a74f5234bf9ec8567ff3c5bb8f93795e15e2c498b48d327c").into(), // stash account (sr25519/1)
            hex!("d0b4896a63b672a7a74f5234bf9ec8567ff3c5bb8f93795e15e2c498b48d327c").into(), // stash account  (sr25519/1)
            hex!("6248d87bd2a640ffe26d6b831735887c24e2076a3a0f3a74f7ae7568c2760408").unchecked_into(), // babe key (sr25519/2)
            hex!("c8de3a01502422b59dfa601c9c3a04a98d2bfbd79dd0810d1d1250feab4241ee").unchecked_into(), // grandpa key (ed25519)
            hex!("7c03ca47a3201455f8f89defda4aa909cb1d25dd9ddb7fd62a940606f79b5663").unchecked_into(), // im online key (sr25519/2)
            hex!("90032c39c968f486f77f8764301a8479206f063d49eeb9f6d499333e2a1be045").unchecked_into(), // validator key (sr25519/2)
            hex!("165f3b255dc17054e6d4447c4005f689eb5ed2f99fe201f4ff799bf088495850").unchecked_into(), // assignment key (sr25519/2)
            hex!("047277f22b9ef92a8b99618f4c86c2412f0e3b08a4f965f842775672043d1e25").unchecked_into(), // authority discovery key (sr25519/2)
            get_from_seed::<BeefyId>("9")
        ),
        (
            hex!("32eebacd223f4aef33d98a667a68f9e371f40384257c6d31030952b9d94e1152").into(), // stash account (sr25519/1)
            hex!("32eebacd223f4aef33d98a667a68f9e371f40384257c6d31030952b9d94e1152").into(), // stash account  (sr25519/1)
            hex!("58108e1651614afc6a535c426fc013945e93533faa33819fe4e69423fe323302").unchecked_into(), // babe key (sr25519/2)
            hex!("8270a62b61639ee56113834aecec01de6cda91413a5111b89f74d6585da34f50").unchecked_into(), // grandpa key (ed25519)
            hex!("74bd654c470ed9b94972c1f997593fab7bdd4d6b85e3cf49401265668142584e").unchecked_into(), // im online key (sr25519/2)
            hex!("ad90a2c3fa2c756f974628dd279adb87935f7ea509856276e3b86f759b22451c").unchecked_into(), // validator key (sr25519/2)
            hex!("c083b0d0bd7d6ffd14562b4c9e28738b087ccc32262170c633c18359ff848779").unchecked_into(), // assignment key (sr25519/2)
            hex!("92cb05c48fc643f057626c669604675c5ad5a836266f260ae7030c6fdc17a543").unchecked_into(), // authority discovery key (sr25519/2)
            get_from_seed::<BeefyId>("10")
        ),
        (
            hex!("8eef6710734f5d1e7d2eb303fa8f04e9bef65fb680647b24624723f95b868964").into(), // stash account (sr25519/1)
            hex!("8eef6710734f5d1e7d2eb303fa8f04e9bef65fb680647b24624723f95b868964").into(), // stash account  (sr25519/1)
            hex!("68a9ec74fa35b3425eaf503dd36294ba8e758e7b8084c4d6bfd547f8c6b58274").unchecked_into(), // babe key (sr25519/2)
            hex!("e41426f7465c13c48335771c5450bf61c50a9cf28b9274f170c7421eea7974f1").unchecked_into(), // grandpa key (ed25519)
            hex!("60fcc9d094d21fe17cfb7426501f50cb3d75c4c9395a3140e0f255443f660d3b").unchecked_into(), // im online key (sr25519/2)
            hex!("9e065eea4143325fbbd26967c26a228d51a3a8384062f7434973f15d1da2c010").unchecked_into(), // validator key (sr25519/2)
            hex!("68f7a83678a377701b46a5e6a4637e99868186ff4835fc0e3914cc56a76a3601").unchecked_into(), // assignment key (sr25519/2)
            hex!("64ffc83f4f86cc595e607a00b977eeb6641e02a4e6e556c24ab163aecd7d146c").unchecked_into(), // authority discovery key (sr25519/2)
            get_from_seed::<BeefyId>("11")
        ),
        (
            hex!("9492b8c38442c79061bdbb8d38dcd28138938a7fd476edf89ecdec06a5a9d20f").into(), // stash account (sr25519/1)
            hex!("9492b8c38442c79061bdbb8d38dcd28138938a7fd476edf89ecdec06a5a9d20f").into(), // stash account  (sr25519/1)
            hex!("ae240842b74e5dd778972e451558134f434c7a1d8a52bc70519f38054e245533").unchecked_into(), // babe key (sr25519/2)
            hex!("c9a68a26e9aa37ba6334f1a20275e3be7d3a9d4aa988627eadac8ea0d0a2dfbf").unchecked_into(), // grandpa key (ed25519)
            hex!("06bd8fd81e50cda2bd67bf6893d921d1aae5cb08409ae43e0bff4d54e1830e58").unchecked_into(), // im online key (sr25519/2)
            hex!("ea9400f05e7fb75a3f7a92febbf58e5a3060dd06132ed6d5d68a3d75ec452826").unchecked_into(), // validator key (sr25519/2)
            hex!("bed3b452f869d187be58a4ba98588611084283810728fa75981e792beaec4151").unchecked_into(), // assignment key (sr25519/2)
            hex!("763d070989ead31f265b40cc7a0cd29d47799b766d6a7f084e44c82baedfc01e").unchecked_into(), // authority discovery key (sr25519/2)
            get_from_seed::<BeefyId>("12")
        ),
        (
            hex!("9037d1020ed699c2f538d3ffcf0eb98087ee11ca4bd07bfddb0d68633806af74").into(), // stash account (sr25519/1)
            hex!("9037d1020ed699c2f538d3ffcf0eb98087ee11ca4bd07bfddb0d68633806af74").into(), // stash account  (sr25519/1)
            hex!("7c8348ec95a0faad6a638ef74864761028c53221bde07e9ff7c81a3f427abf3f").unchecked_into(), // babe key (sr25519/2)
            hex!("ace46e899b90e75199549d8fa2ae7097e896ab3c845217e3155f99b6ffb88803").unchecked_into(), // grandpa key (ed25519)
            hex!("30c40adee5476157ef3c2a26e10cab95ec7d54b62dd220738f5a474d5f86874e").unchecked_into(), // im online key (sr25519/2)
            hex!("0ce84accce1ced0de223aa72f760f1b3d13ddfd267938cd63e25308378d32008").unchecked_into(), // validator key (sr25519/2)
            hex!("eed33645cda7812cd343bbaef9131b2794812f2fd37701ccb6cddf9c1e293d38").unchecked_into(), // assignment key (sr25519/2)
            hex!("7aeb767131602e6612e607a9eb8e26b4ce4fa4765593d032bc923ce8acadda42").unchecked_into(), // authority discovery key (sr25519/2)
            get_from_seed::<BeefyId>("13")
        ),
        (
            hex!("5270ec35ba01254d8bff046a1a58f16d3ae615c235efd6e99a35f233b2d9df2c").into(), // stash account (sr25519/1)
            hex!("5270ec35ba01254d8bff046a1a58f16d3ae615c235efd6e99a35f233b2d9df2c").into(), // stash account  (sr25519/1)
            hex!("50412bd7d3f1075e8f8e3b682d05ea20b391c287d8c849a0e49a78f568553e69").unchecked_into(), // babe key (sr25519/2)
            hex!("622c382187c0b2c61ecfb17443294d11a9d2ab770ae6f1fb49184a43906d59fe").unchecked_into(), // grandpa key (ed25519)
            hex!("94848b8cf2cbc9e6fd72db8d80676591b5be4d1ec68972ada48cf6fd01228712").unchecked_into(), // im online key (sr25519/2)
            hex!("303094c583e253794c9db14803585baaa74472f4ecba846defefc8aecfb6214a").unchecked_into(), // validator key (sr25519/2)
            hex!("b400c4164d016282b202c1d42d9dc8ede28cbe4b751d463bab5f23fef42b295a").unchecked_into(), // assignment key (sr25519/2)
            hex!("8c15606f4c121376097ff0e96c2a33ea7b024d812b42fe2c741c8b8cee17e63d").unchecked_into(), // authority discovery key (sr25519/2)
            get_from_seed::<BeefyId>("14")
        ),
    ];

    let mut endowed_accounts = initial_paseo_validators.iter()
    .map(|validator| validator.0.clone()) 
    .collect::<Vec<_>>(); 

    // Add Faucet
    endowed_accounts.push(hex!("e21bb02f2a82cb1113ff10693093377672925b23f047624c0cfa7a24a8609841").into());


    paseo_genesis(
        wasm_binary,
        // initial authorities
        initial_paseo_validators,
        //root key
        root_key.clone(),
        // endowed accounts
        Some(endowed_accounts),
    )
}

/// Paseo local config (multivalidator Alice + Bob)
pub fn paseo_local_config() -> Result<Box<dyn ChainSpec>, String> {
    let wasm_binary = paseo_runtime::WASM_BINARY.ok_or("Paseo development wasm not available")?;

    Ok(Box::new(PaseoChainSpec::from_genesis(
        "Paseo Local Testnet",
        "paseo-local",
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_parachains_host_configuration_is_consistent() {
        default_parachains_host_configuration().panic_if_not_consistent();
    }
}
