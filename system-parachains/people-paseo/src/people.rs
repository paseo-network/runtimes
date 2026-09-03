// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;
use crate::{
	parameters::{
		AccountsApiAllowance, LiteNotificationSlotsPerPeriod, LitePersonRegistrationFee,
		LitePersonStatementLimit, LiteStmtStoreSlotsPerPeriod,
		LongTermStorageAllowanceForLitePeople, LongTermStorageAllowanceForPeople,
		LongTermStorageClaimsPerPeriod, LongTermStorageCleanupLimit, LongTermStorageGraceWindow,
		LongTermStoragePeriodDuration, NotificationAllowance, NotificationPeriodDuration,
		NotificationSlotsPerPeriod, PersonStatementLimit, StmtStoreCleanupLimit,
		StmtStoreGraceWindow, StmtStoreReplacementCooldown, StmtStoreSlotsPerPeriod,
	},
	xcm_config::LocationToAccountId,
};
use codec::{Decode, Encode, MaxEncodedLen};
use enumflags2::{bitflags, BitFlags};
use frame_support::{parameter_types, CloneNoBound, DebugNoBound, EqNoBound, PartialEqNoBound};
use pallet_identity::{Data, IdentityInformationProvider};
use parachains_common::{impls::ToParentTreasury, DAYS, MINUTES};
use scale_info::TypeInfo;
use sp_runtime::{
	traits::{AccountIdConversion, Verify},
	Debug,
};
use xcm::latest::prelude::BodyId;

parameter_types! {
	//   27 | Min encoded size of `Registration`
	// - 10 | Min encoded size of `IdentityInfo`
	// -----|
	//   17 | Min size without `IdentityInfo` (accounted for in byte deposit)
	pub const BasicDeposit: Balance = system_para_deposit(1, 17);
	pub const ByteDeposit: Balance = system_para_deposit(0, 1);
	pub const UsernameDeposit: Balance = system_para_deposit(0, 32);
	pub const SubAccountDeposit: Balance = system_para_deposit(1, 53);
	pub RelayTreasuryAccount: AccountId =
		parachains_common::TREASURY_PALLET_ID.into_account_truncating();
	pub const GeneralAdminBodyId: BodyId = BodyId::Administration;
}

pub type IdentityAdminOrigin = EitherOfDiverse<
	EnsureRoot<AccountId>,
	EitherOf<
		EnsureXcm<IsVoiceOfBody<RelayChainLocation, GeneralAdminBodyId>>,
		EnsureXcm<IsVoiceOfBody<AssetHubLocation, GeneralAdminBodyId>>,
	>,
>;

impl pallet_identity::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type BasicDeposit = BasicDeposit;
	type ByteDeposit = ByteDeposit;
	type UsernameDeposit = UsernameDeposit;
	type SubAccountDeposit = SubAccountDeposit;
	type MaxSubAccounts = ConstU32<100>;
	type IdentityInformation = IdentityInfo;
	type MaxRegistrars = ConstU32<20>;
	type Slashed = ToParentTreasury<RelayTreasuryAccount, LocationToAccountId, Runtime>;
	type ForceOrigin = EnsureRoot<Self::AccountId>;
	type RegistrarOrigin = IdentityAdminOrigin;
	type OffchainSignature = Signature;
	type SigningPublicKey = <Signature as Verify>::Signer;
	type UsernameAuthorityOrigin = IdentityAdminOrigin;
	type PendingUsernameExpiration = ConstU32<{ 7 * DAYS }>;
	type UsernameGracePeriod = ConstU32<{ 3 * DAYS }>;
	type MaxSuffixLength = ConstU32<7>;
	type MaxUsernameLength = ConstU32<32>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
	type WeightInfo = weights::pallet_identity::WeightInfo<Runtime>;
}

/// The fields that we use to identify the owner of an account with. Each corresponds to a field
/// in the `IdentityInfo` struct.
#[bitflags]
#[repr(u64)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IdentityField {
	Display,
	Legal,
	Web,
	Matrix,
	Email,
	PgpFingerprint,
	Image,
	Twitter,
	GitHub,
	Discord,
}

/// Information concerning the identity of the controller of an account.
#[derive(
	CloneNoBound,
	Encode,
	Decode,
	DecodeWithMemTracking,
	EqNoBound,
	MaxEncodedLen,
	PartialEqNoBound,
	DebugNoBound,
	TypeInfo,
)]
#[codec(mel_bound())]
pub struct IdentityInfo {
	/// A reasonable display name for the controller of the account. This should be whatever the
	/// account is typically known as and should not be confusable with other entities, given
	/// reasonable context.
	///
	/// Stored as UTF-8.
	pub display: Data,

	/// The full legal name in the local jurisdiction of the entity. This might be a bit
	/// long-winded.
	///
	/// Stored as UTF-8.
	pub legal: Data,

	/// A representative website held by the controller of the account.
	///
	/// NOTE: `https://` is automatically prepended.
	///
	/// Stored as UTF-8.
	pub web: Data,

	/// The Matrix (e.g. for Element) handle held by the controller of the account. Previously,
	/// this was called `riot`.
	///
	/// Stored as UTF-8.
	pub matrix: Data,

	/// The email address of the controller of the account.
	///
	/// Stored as UTF-8.
	pub email: Data,

	/// The PGP/GPG public key of the controller of the account.
	pub pgp_fingerprint: Option<[u8; 20]>,

	/// A graphic image representing the controller of the account. Should be a company,
	/// organization or project logo or a headshot in the case of a human.
	pub image: Data,

	/// The Twitter identity. The leading `@` character may be elided.
	pub twitter: Data,

	/// The GitHub username of the controller of the account.
	pub github: Data,

	/// The Discord username of the controller of the account.
	pub discord: Data,
}

impl IdentityInformationProvider for IdentityInfo {
	type FieldsIdentifier = u64;

	fn has_identity(&self, fields: Self::FieldsIdentifier) -> bool {
		self.fields().bits() & fields == fields
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn create_identity_info() -> Self {
		let data = Data::Raw(vec![0; 32].try_into().unwrap());

		IdentityInfo {
			display: data.clone(),
			legal: data.clone(),
			web: data.clone(),
			matrix: data.clone(),
			email: data.clone(),
			pgp_fingerprint: Some([0; 20]),
			image: data.clone(),
			twitter: data.clone(),
			github: data.clone(),
			discord: data,
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn all_fields() -> Self::FieldsIdentifier {
		use enumflags2::BitFlag;
		IdentityField::all().bits()
	}
}

impl IdentityInfo {
	pub(crate) fn fields(&self) -> BitFlags<IdentityField> {
		let mut res = <BitFlags<IdentityField>>::empty();
		if !self.display.is_none() {
			res.insert(IdentityField::Display);
		}
		if !self.legal.is_none() {
			res.insert(IdentityField::Legal);
		}
		if !self.web.is_none() {
			res.insert(IdentityField::Web);
		}
		if !self.matrix.is_none() {
			res.insert(IdentityField::Matrix);
		}
		if !self.email.is_none() {
			res.insert(IdentityField::Email);
		}
		if self.pgp_fingerprint.is_some() {
			res.insert(IdentityField::PgpFingerprint);
		}
		if !self.image.is_none() {
			res.insert(IdentityField::Image);
		}
		if !self.twitter.is_none() {
			res.insert(IdentityField::Twitter);
		}
		if !self.github.is_none() {
			res.insert(IdentityField::GitHub);
		}
		if !self.discord.is_none() {
			res.insert(IdentityField::Discord);
		}
		res
	}
}

/// A `Default` identity. This is given to users who get a username but have not set an identity.
impl Default for IdentityInfo {
	fn default() -> Self {
		IdentityInfo {
			display: Data::None,
			legal: Data::None,
			web: Data::None,
			matrix: Data::None,
			email: Data::None,
			pgp_fingerprint: None,
			image: Data::None,
			twitter: Data::None,
			github: Data::None,
			discord: Data::None,
		}
	}
}

// ---------------------------------------------------------------------------
// Individuality (Proof of Personhood) pallet configuration.
// ---------------------------------------------------------------------------

use crate::xcm_config;
use assets_common::local_and_foreign_assets::ForeignAssetReserveData;
use cumulus_primitives_core::Junction::{PalletInstance, Parachain};
use frame_support::{
	pallet_prelude::PhantomData,
	traits::{
		fungible::{HoldConsideration, ItemOf},
		ConstU128, ConstU32, ConstU64, ConstUint, ContainsPair, Get, LinearStoragePrice,
		Randomness,
	},
};
use indiv_pallet_game::PhaseDurationValues;
use indiv_pallet_origin_restriction::Allowance;
#[cfg(feature = "runtime-benchmarks")]
use indiv_support::traits::{PersonalId, RingIndex};
#[cfg(feature = "runtime-benchmarks")]
use paseo_runtime_constants::system_parachain::ASSET_HUB_ID;
use indiv_support::{
	fungibles::CombineAssetsWithHolder,
	traits::{Alias, AllocateStorage, Context},
	utils::TypedGetToGet,
};
#[cfg(feature = "runtime-benchmarks")]
use sp_runtime::BoundedVec;
use sp_runtime::{
	traits::{ConstI8, ConstU16},
	DispatchResult, MultiSignature, MultiSigner, Percent,
};
use sp_statement_store::StatementAllowance;
use verifiable::{ring::bandersnatch::BandersnatchVrfVerifiable, GenerateVerifiable};
use xcm::{
	latest::prelude::{send_xcm, OriginKind, Transact, UnpaidExecution},
	v5::{Location, WeightLimit},
};

/// External asset id as registered on Paseo Asset Hub, reused from the shared runtime
/// constants.
pub use paseo_runtime_constants::ProtectedAssetLocation as ExternalAssetLocation;

parameter_types! {
	pub const StaleAliasCleanupInterval: BlockNumber = 5 * MINUTES;
}

/// The full featured fungibles implementation with both regular and hold functionality.
pub type AssetsWithHolder = CombineAssetsWithHolder<Assets, AssetsHolder>;

/// A fungible implementation using the external asset id from Asset Hub.
pub type FungibleExternalAsset = ItemOf<AssetsWithHolder, ExternalAssetLocation, AccountId>;

// The `AccountContexts` type, which must implement `trait Contains` and return true only for the
// contexts the runtime supports.
pub struct AccountContexts;
impl frame_support::traits::Contains<Context> for AccountContexts {
	fn contains(l: &Context) -> bool {
		// individuality v0.3.1 deleted the `RESOURCES_CONTEXT` and `SCORE_CONTEXT` constants.
		// Their replacements, `Pallet::<Runtime>::resources_context()` and
		// `Pallet::<Runtime>::score_context()`, are FUNCTIONS, not consts: the contexts are now
		// derived from the runtime's network suffix rather than being fixed byte strings.
		// They must therefore be CALLED here — binding one as a value would not compile, and
		// hard-coding an old constant would silently split the context namespace.
		//
		// 🔴 Every context a pallet gates its calls on must appear in this list. A pallet whose
		// context is missing here is completely unreachable on chain — its dispatchables fail
		// with `InvalidTransaction::Call` — while its unit tests keep passing.
		l == &indiv_pallet_mob_rule::MOB_CONTEXT ||
			l == &indiv_pallet_score::Pallet::<Runtime>::score_context() ||
			l == &indiv_pallet_resources::Pallet::<Runtime>::resources_context()
	}
}

/// The contexts in which lite people may set up account aliases: the pallet's own
/// authentication context, and the score context, in which a lite person can designate an
/// account to play the game. Both are functions for the same reason as above.
pub struct LitePeopleAccountContexts;
impl frame_support::traits::Contains<Context> for LitePeopleAccountContexts {
	fn contains(l: &Context) -> bool {
		l == &indiv_pallet_people_lite::Pallet::<Runtime>::auth_context() ||
			l == &indiv_pallet_score::Pallet::<Runtime>::score_context()
	}
}

parameter_types! {
	/// Controls the ring size for the people members collection, used for anonymous ring VRF
	/// proofs. `R2e9` maps to the small Bandersnatch ring setting. The underlying `2^9`
	/// domain reserves 257 slots for proof-system overhead (blinding and internal rows),
	/// so the effective member capacity is 255.
	pub const MembersFlexibleRingExponent: indiv_support::traits::RingExponent =
		indiv_support::traits::RingExponent::R2e9;
	/// Controls the ring size for recycler rings in coinage append-only collections.
	pub const RecyclerRingExponent: indiv_support::traits::RingExponent =
		indiv_support::traits::RingExponent::R2e10;
	/// Controls the ring size for paid unload token rings in coinage append-only collections.
	pub const PaidUnloadTokenRingExponent: indiv_support::traits::RingExponent =
		indiv_support::traits::RingExponent::R2e10;
	/// The owner of the people collection. This is set to the people pallet's own location.
	pub PeopleCollectionOwner: Location = Location::new(0, [PalletInstance(51)]);
	/// The owner of the lite people collection. This matches the `PeopleLite` pallet index.
	pub LitePeopleCollectionOwner: Location = Location::new(0, [PalletInstance(62)]);
	/// Ring exponent for lite people collection.
	pub const LitePeopleRingExponent: indiv_support::traits::RingExponent =
		indiv_support::traits::RingExponent::R2e9;
	/// Onboarding size for lite people collection.
	pub const LitePeopleOnboardingSize: u32 = 3;
	/// Receives `register_with_fee` payments. Matches upstream's `plitefee`.
	pub const LitePeoplePotId: frame_support::PalletId = frame_support::PalletId(*b"plitefee");
	/// The page size for chunks manager.
	pub const ChunkPageSize: u32 = 255;
	/// Self-inclusion delay: 60 minutes.
	pub const SelfInclusionDelayValue: u64 = 3600;
}

impl indiv_pallet_chunks_manager::Config for Runtime {
	type WeightInfo = indiv_pallet_chunks_manager::weights::SubstrateWeight<Runtime>;
	type Chunk = <BandersnatchVrfVerifiable as GenerateVerifiable>::StaticChunk;
	type PageSize = ChunkPageSize;
	type ManagerOrigin = EnsureRoot<Self::AccountId>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = benchmark_utils::ChunksManagerBenchHelper;
}

impl indiv_pallet_members::Config for Runtime {
	type WeightInfo = indiv_pallet_members::weights::SubstrateWeight<Runtime>;
	type Crypto = verifiable::ring::bandersnatch::BandersnatchVrfVerifiable;
	type Location = xcm::v5::Location;
	type ChunksManager = ChunksManager;
	type Clock = Timestamp;
	type MaxCollections = ConstU32<100>;
	type OnboardingQueuePageSize = ConstU32<255>;
	type MaxFlexibleRingExponent = MembersFlexibleRingExponent;
	type RingBuildingMemberLimit = ConstU32<100>;
	/// 10 minutes in seconds for old root retention.
	type OldRootRetentionDuration = ConstU64<600>;
	type OnRingRootChange = MembersNotifier;
	type OffchainWorkerInterval = ConstU32<1>;
	type ManagerOrigin = EnsureRoot<AccountId>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = benchmark_utils::MembersBenchHelper;
}

parameter_types! {
	pub const RingBakingInterval: BlockNumber = MINUTES;
	pub const QueuePageMergingInterval: BlockNumber = 5 * MINUTES;
	pub const MaxTaskLifespan: BlockNumber = 5 * MINUTES;
}

impl indiv_pallet_people::Config for Runtime {
	type WeightInfo = indiv_pallet_people::weights::SubstrateWeight<Runtime>;
	type MemberService = Members;
	type RingExponent = MembersFlexibleRingExponent;
	type CollectionOwner = PeopleCollectionOwner;
	type AccountContexts = AccountContexts;
	type OnboardingQueuePageSize = ConstU32<30>;
	type StaleAliasCleanupInterval = StaleAliasCleanupInterval;
	type SelfInclusionDelay = SelfInclusionDelayValue;
	type ManagerOrigin = EnsureRoot<AccountId>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = benchmark_utils::PeopleBenchHelper;
}

impl indiv_pallet_dummy_dim::Config for Runtime {
	type WeightInfo = indiv_pallet_dummy_dim::weights::SubstrateWeight<Runtime>;
	type UpdateOrigin = EnsureRoot<AccountId>;
	type MaxPersonBatchSize = ConstU32<1000>;
	type People = People;
}

/// Shared benchmark helpers for individuality pallets.
#[cfg(feature = "runtime-benchmarks")]
pub mod benchmark_utils {
	use super::*;
	use alloc::vec::Vec;
	use frame_support::{
		pallet_prelude::PalletInfoAccess,
		traits::fungibles::{Create, Inspect},
	};
	use indiv_support::genesis::ring_verifier_builder_params;
	use verifiable::ring::RingDomainSize;

	pub fn member_from_seed(
		seed: u64,
	) -> <BandersnatchVrfVerifiable as GenerateVerifiable>::Member {
		let mut entropy = [0u8; 32];
		entropy[..8].copy_from_slice(&seed.to_le_bytes()[..]);
		let secret = BandersnatchVrfVerifiable::new_secret(entropy);
		BandersnatchVrfVerifiable::member_from_secret(&secret)
	}

	pub fn ensure_external_asset_exists() {
		if !Assets::asset_exists(ExternalAssetLocation::get()) {
			<Assets as Create<_>>::create(
				ExternalAssetLocation::get(),
				ParaId::new(<Assets as PalletInfoAccess>::index() as u32).into_account_truncating(),
				true,
				1u32.into(),
			)
			.expect("Failed to create asset");
		}
	}

	pub fn initialize_chunks(
		domain_size: RingDomainSize,
	) -> Vec<<BandersnatchVrfVerifiable as GenerateVerifiable>::StaticChunk> {
		ring_verifier_builder_params(domain_size)
	}

	pub struct ChunksManagerBenchHelper;

	impl
		indiv_pallet_chunks_manager::BenchmarkHelper<
			<BandersnatchVrfVerifiable as GenerateVerifiable>::StaticChunk,
		> for ChunksManagerBenchHelper
	{
		fn chunk_page() -> Vec<<BandersnatchVrfVerifiable as GenerateVerifiable>::StaticChunk> {
			let chunks = ring_verifier_builder_params(RingDomainSize::Domain16);
			chunks.into_iter().take(ChunkPageSize::get() as usize).collect()
		}
	}

	pub struct MembersBenchHelper;

	impl
		indiv_pallet_members::BenchmarkHelper<
			<BandersnatchVrfVerifiable as GenerateVerifiable>::StaticChunk,
		> for MembersBenchHelper
	{
		fn initialize_chunks(
			ring_size: indiv_support::traits::RingExponent,
		) -> Vec<<BandersnatchVrfVerifiable as GenerateVerifiable>::StaticChunk> {
			let domain_size: RingDomainSize =
				ring_size.try_into().expect("ring_size should be convertible to RingDomainSize");
			ring_verifier_builder_params(domain_size)
		}
		fn set_time(now: core::time::Duration) {
			pallet_timestamp::Now::<Runtime>::put(now.as_millis() as u64);
		}
		fn set_valid_time() {
			let duration = core::time::Duration::from_secs(5);
			pallet_timestamp::Now::<Runtime>::put(duration.as_millis() as u64);
		}
	}

	pub struct PeopleBenchHelper;

	impl
		indiv_pallet_people::BenchmarkHelper<
			<BandersnatchVrfVerifiable as GenerateVerifiable>::StaticChunk,
		> for PeopleBenchHelper
	{
		fn valid_account_context() -> Context {
			// Identity aliases are removed, and this runtime no longer accepts any
			// storage-backed alias contexts through `AccountContexts`.
			indiv_pallet_mob_rule::MOB_CONTEXT
		}
		fn initialize_chunks() -> Vec<<BandersnatchVrfVerifiable as GenerateVerifiable>::StaticChunk>
		{
			initialize_chunks(RingDomainSize::Domain11)
		}
	}

	pub struct ResourcesBenchHelper;

	impl indiv_pallet_resources::benchmarking::BenchmarkHelper<Runtime> for ResourcesBenchHelper {
		fn set_time(now: core::time::Duration) {
			pallet_timestamp::Now::<Runtime>::put(now.as_millis() as u64);
		}

		fn sign_message(message: &[u8]) -> (sp_runtime::AccountId32, MultiSignature) {
			use sp_core::Pair;
			use sp_runtime::traits::IdentifyAccount;
			let entropy = [1u8; 32];
			let pair = sp_core::ed25519::Pair::from_seed(&entropy);
			let account = pair.public().into_account().into();
			let secret = ed25519_zebra::SigningKey::from(entropy);
			let signature = sp_core::ed25519::Signature::from_raw(secret.sign(message).into());
			(account, signature.into())
		}
	}

	/// Benchmark helper for members_notifier pallet.
	pub struct MembersNotifierBenchHelper;

	impl indiv_pallet_members_notifier::benchmarking::BenchmarkHelper<Runtime>
		for MembersNotifierBenchHelper
	{
		fn init() {
			use cumulus_pallet_parachain_system::RelevantMessagingState;
			use cumulus_primitives_core::relay_chain::AbridgedHrmpChannel;

			// Timestamp must exceed ReplayCooldownSeconds (60s) so
			// request_replay benchmark passes the cooldown check.
			pallet_timestamp::Now::<Runtime>::put(120_000u64);

			// Fake HRMP egress channels so benchmarks that send XCM succeed.
			// Benchmarks use para_ids 0..MaxSubscribers and 1000..1000+MaxSubscribers.
			let max_subscribers =
				<Runtime as indiv_pallet_members_notifier::Config>::MaxSubscribers::get();
			let channel = AbridgedHrmpChannel {
				max_capacity: 1000,
				max_total_size: 1_000_000,
				max_message_size: 100_000,
				msg_count: 0,
				total_size: 0,
				mqc_head: None,
			};
			let mut egress_channels: Vec<(ParaId, AbridgedHrmpChannel)> = (0..max_subscribers)
				.chain(1000..1000 + max_subscribers)
				.map(|i| (ParaId::from(i), channel.clone()))
				.collect();
			egress_channels.sort_by_key(|(id, _)| *id);
			egress_channels.dedup_by_key(|(id, _)| *id);

			let messaging_state =
				cumulus_pallet_parachain_system::relay_state_snapshot::MessagingStateSnapshot {
					dmq_mqc_head: Default::default(),
					relay_dispatch_queue_remaining_capacity: Default::default(),
					ingress_channels: Vec::new(),
					egress_channels,
				};
			RelevantMessagingState::<Runtime>::put(messaging_state);
		}

		fn setup_ring_roots(count: u32) {
			use indiv_support::traits::Identifier;
			use verifiable::ring::RingDomainSize;

			// Creating a valid intermediate and root using the smallest domain size.
			let intermediate = BandersnatchVrfVerifiable::start_members(RingDomainSize::Domain11);
			let root = BandersnatchVrfVerifiable::finish_members(intermediate.clone());

			// Matching the test_identifier helper from the notifier benchmarking module.
			fn test_identifier(index: u32) -> Identifier {
				let mut id = [0u8; 32];
				id[..4].copy_from_slice(&index.to_be_bytes());
				id
			}
			assert_eq!(
				test_identifier(0xDEADBEEF),
				hex_literal::hex!(
					"deadbeef00000000000000000000000000000000000000000000000000000000"
				),
				"test_identifier drifted — sync with pallets/members-notifier/src/benchmarking.rs",
			);

			// Populating ring roots for all identifiers that benchmarks may reference.
			// Benchmarks spread pending updates across MaxCollections identifiers.
			let max_collections =
				<Runtime as indiv_pallet_members_notifier::Config>::MaxCollections::get();
			for coll in 0..max_collections {
				let identifier = test_identifier(coll);
				for i in 0..count {
					let ring_root = indiv_pallet_members::RingRoot::<Runtime> {
						root: root.clone(),
						revision: 0,
						intermediate: intermediate.clone(),
					};
					indiv_pallet_members::Root::<Runtime>::insert(identifier, i, ring_root);
				}
				indiv_pallet_members::CurrentRingIndex::<Runtime>::insert(identifier, count - 1);
			}
		}

		fn set_max_message_size(size: u32) {
			use cumulus_pallet_parachain_system::RelevantMessagingState;

			// Shrinking each egress channel's max_message_size triggers the worst-case
			// chunking path in send_batch. init() must have run before this.
			let mut state = RelevantMessagingState::<Runtime>::get()
				.expect("BenchmarkHelper::init must run before set_max_message_size");
			for (_, channel) in state.egress_channels.iter_mut() {
				channel.max_message_size = size;
			}
			RelevantMessagingState::<Runtime>::put(state);
		}
	}
}

parameter_types! {
	pub const MobRulePotId: PalletId = PalletId(*b"MobRwrds");
	pub const MinTurnoutPercentage: Percent = Percent::from_percent(10);
	pub const VotingPenaltyDuration: BlockNumber = DAYS;
	pub const OffchainWorkInterval: BlockNumber = 5 * MINUTES;
	pub const MinimumVoterThreshold: u32 = 3;
}

impl indiv_pallet_mob_rule::Config for Runtime {
	type WeightInfo = indiv_pallet_mob_rule::weights::SubstrateWeight<Runtime>;
	type Currency = FungibleExternalAsset;
	type CurrencyLocationInfo = ExternalAssetLocation;
	// 24 hours
	type Clock = Timestamp;
	type EnsurePerson = indiv_pallet_people::EnsurePersonalAliasInContext<Runtime>;
	type MaxVoteClaimDuration = ConstU64<86_400>;
	type MinCaseDuration = ConstU32<{ 10 * 60 }>;
	type MaxVotingDuration = ConstU32<{ 20 * 60 }>;
	type MinTurnoutNominal = ConstU32<1>;
	type MinTurnoutPercentage = MinTurnoutPercentage;
	type MaxPayoutRoundSchedules = ConstU32<5>;
	type VotingPenaltyDuration = VotingPenaltyDuration;
	type InterventionOrigin = EnsureRoot<AccountId>;
	type PotId = MobRulePotId;
	type MaxVotesClaimable = ConstU32<10>;
	type OffchainWorkInterval = OffchainWorkInterval;
	type CleanVotesBatchSize = ConstU32<1000>;
	type VotesOpenForClaimsDuration = ConstU32<{ 10 * 60 }>;
	type MinimumVoterThreshold = MinimumVoterThreshold;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = MobRuleBenchHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct MobRuleBenchHelper;

#[cfg(feature = "runtime-benchmarks")]
impl indiv_pallet_mob_rule::benchmarking::BenchmarkHelper<Runtime> for MobRuleBenchHelper {
	fn set_valid_time() {
		// Needed to allow for all time-based constraints.
		// Max voting duration (20 minutes) + Max claim duration (86_400s == 24h) + buffer.
		let sufficient_time = (20 * 60 + 86_400 + 3600) * 1000u64; // in ms
		pallet_timestamp::Now::<Runtime>::put(sufficient_time);
	}

	fn setup_currency() {
		benchmark_utils::ensure_external_asset_exists();
	}
}

parameter_types! {
	pub const ProofOfInkBaseDeposit: Balance = 100 * CENTS;
	// One cent: $10,000 / MB
	pub const ProofOfInkByteDeposit: Balance = CENTS;
	pub const ProofOfInkHoldReason: RuntimeHoldReason = RuntimeHoldReason::ProofOfInk(indiv_pallet_proof_of_ink::HoldReason::ProofOfInk);
	pub const ProofOfInkPotId: PalletId = PalletId(*b"PoIPot__");
}

#[cfg(feature = "runtime-benchmarks")]
pub struct PoIBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
use indiv_pallet_proof_of_ink::ReferralTicket;

#[cfg(feature = "runtime-benchmarks")]
use alloc::vec;

#[cfg(feature = "runtime-benchmarks")]
impl indiv_pallet_proof_of_ink::BenchmarkHelper<Runtime> for PoIBenchmarkHelper {
	fn create_tickets(seed: u64) -> BoundedVec<ReferralTicket<AccountId>, ConstU32<10>> {
		let (_, ticket) = Self::create_ticket(seed);

		BoundedVec::<ReferralTicket<AccountId>, ConstU32<10>>::try_from(vec![ReferralTicket {
			ticket,
		}])
		.unwrap()
	}

	fn create_ticket(seed: u64) -> (MultiSigner, AccountId) {
		use sp_core::Pair;
		use sp_runtime::traits::IdentifyAccount;
		let mut entropy = [0u8; 32];
		entropy[..8].copy_from_slice(&seed.to_le_bytes()[..]);
		let pair = sp_core::ed25519::Pair::from_seed(&entropy);
		let account = pair.public().into_account().into();
		let signer: MultiSigner = pair.public().into();
		(signer, account)
	}

	fn sign(seed: u64, msg: &[u8]) -> MultiSignature {
		let mut entropy = [0u8; 32];
		entropy[..8].copy_from_slice(&seed.to_le_bytes()[..]);
		// sp-core doesn't expose the signing for the runtime, so we use the underlying library
		let secret = ed25519_zebra::SigningKey::from(entropy);
		sp_core::ed25519::Signature::from_raw(secret.sign(msg).into()).into()
	}

	fn build_person_origin(personal_id: PersonalId) -> RuntimeOrigin {
		indiv_pallet_people::Origin::PersonalIdentity(personal_id).into()
	}

	fn setup_currency() {
		benchmark_utils::ensure_external_asset_exists();
	}
}

impl indiv_pallet_proof_of_ink::Config for Runtime {
	type WeightInfo = indiv_pallet_proof_of_ink::weights::SubstrateWeight<Runtime>;
	type Deposit = HoldConsideration<
		AccountId,
		Balances,
		ProofOfInkHoldReason,
		LinearStoragePrice<ProofOfInkBaseDeposit, ProofOfInkByteDeposit, Balance>,
	>;
	type People = People;
	type EnsurePerson = indiv_pallet_people::EnsurePersonalIdentity<Runtime>;
	type TicketSignature = MultiSignature;
	type TicketPublic = MultiSigner;
	type Ticket = AccountId;
	type Oracle = MobRule;
	type Randomness = SubjectBlockRandommess<Runtime>;
	type DataStore = BulletinDataStore;
	type MaxActiveReferrals = ConstU32<10>;
	type MaxRetryAttempts = ConstU32<1>;
	type MaxReimbursementValues = ConstU32<50>;
	type Currency = FungibleExternalAsset;
	type PotId = ProofOfInkPotId;
	type InvitationsOrigin = EnsureRoot<Self::AccountId>;
	type ManagerOrigin = EnsureRoot<Self::AccountId>;
	type Crypto = BandersnatchVrfVerifiable;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = PoIBenchmarkHelper;
}

parameter_types! {
	pub const ScorePotId: PalletId = PalletId(*b"scorepot");
}

#[cfg(feature = "runtime-benchmarks")]
pub struct ScoreBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl indiv_pallet_score::benchmarking::BenchmarkHelper<Runtime> for ScoreBenchmarkHelper {
	fn create_member(seed: u64) -> indiv_pallet_score::MemberOf<Runtime> {
		benchmark_utils::member_from_seed(seed)
	}
	fn setup_currency() {
		benchmark_utils::ensure_external_asset_exists();
	}
}

impl indiv_pallet_score::Config for Runtime {
	type WeightInfo = indiv_pallet_score::weights::SubstrateWeight<Runtime>;
	// Same source of truth as every other `Suffix` binding in this runtime: the on-chain
	// `NetworkSuffix` pallet, whose default is the shared `system-parachains-constants` value.
	type Suffix = NetworkSuffix;
	type EnsurePerson = indiv_pallet_people::EnsurePersonalAliasInContext<Runtime>;
	type ScorePotId = ScorePotId;
	type Currency = FungibleExternalAsset;
	type CurrencyLocationInfo = ExternalAssetLocation;
	type ManagerOrigin = EnsureRoot<Self::AccountId>;
	type MaxPayoutRoundSchedules = ConstU32<10>;
	type OffchainWorkInterval = ConstU32<2>;
	type People = People;
	type Crypto = BandersnatchVrfVerifiable;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ScoreBenchmarkHelper;
}

parameter_types! {
	pub const PlayDepositReason: RuntimeHoldReason =
		RuntimeHoldReason::Game(indiv_pallet_game::HoldReason::PlayDeposit);
	pub const PlayDepositDefault: Balance = 2 * UNITS;
	// TODO: Find a reasonable value
	pub PlayerStatementLimit: StatementAllowance = StatementAllowance {
		max_size: 1_000_000,
		max_count: 1_000_000,
	};
	pub GameAirdropSource: AccountId = PalletId(*b"pop/gads").into_account_truncating();
}

impl indiv_pallet_game::Config for Runtime {
	const TESTNET: bool = true;
	type WeightInfo = indiv_pallet_game::weights::SubstrateWeight<Runtime>;
	type MaxGroupSize = ConstU32<6>;
	type UnixTime = Timestamp;
	type MaxRounds = ConstU32<3>;
	type ManagerOrigin = EnsureRoot<Self::AccountId>;
	type InviteIssuer = EnsureRoot<Self::AccountId>;
	type EnsureLiteAlias = indiv_pallet_people_lite::EnsureLiteAliasInContext<Runtime>;
	type NonPlayingKickoutTime = ConstU32<{ 90 * DAYS }>;
	type NativeFungible = Balances;
	type PlayDeposit = HoldConsideration<
		AccountId,
		Balances,
		PlayDepositReason,
		sp_runtime::traits::Identity,
		Balance,
	>;
	type DefaultPlayDeposit = PlayDepositDefault;
	type TicketSignature = MultiSignature;
	// Weekly game cadence on devnet: 26 lets a single `schedule_games` sudo call queue
	// ~6 months ahead, so the top-up keeper runs at most twice a year.
	type MaxGameSchedules = ConstU32<26>;
	type MaxAttendanceHistoryDepth = ConstU32<12>;
	// This runtime plays games but mints nothing from them, which is the case upstream
	// documents for `()`: "A runtime that plays games without minting anything sets this to
	// `()`" (`pallets/game/src/lib.rs`). `indiv-pallet-nft-credits` and the Asset Hub
	// `indiv-pallet-nft-claims` half of that pipeline are not adopted here.
	type NftClaimCredits = ();
	type DefaultPhaseDurations = GamePhaseDurations;
	type AccountSignature = Signature;
	type PlayerStatementLimit = PlayerStatementLimit;
	type PeopleVoteWeight = ConstUint<2>;
	type CandidateVoteWeight = ConstUint<1>;
	// Production-realistic minimum: a group must hold at least 2 players for mutual
	// verification to mean anything. Kept at the production floor (not 0) so devnet games
	// exercise the same grouping behaviour as mainnet. Note: with a low active-player count
	// this can leave weekly games unable to form a valid group — the DummyDim-recognized
	// cohort is externally recognized and immune, so only the organic game-onboarding path
	// is affected.
	type MinGroupSize = ConstUint<2>;
	type AirdropAssetId = <Runtime as pallet_assets::Config>::AssetId;
	type AirdropAssetBalance = Balance;
	type Airdrop = Airdrop;
	type AirdropSource = GameAirdropSource;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = GamePalletBenchmarkHelper;
}

parameter_types! {
	pub const HonourPointFreezeDuration: indiv_pallet_honour::Seconds = 24 * 60 * 60;
	pub const HonourCallMortality: indiv_pallet_honour::Seconds = 5 * 60;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct HonourBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl indiv_pallet_honour::benchmarking::BenchmarkHelper<Runtime> for HonourBenchmarkHelper {
	fn set_time(now: indiv_pallet_honour::Seconds) {
		pallet_timestamp::Now::<Runtime>::put(now.saturating_mul(1_000));
	}

	fn seed_and_create_proof(
		vote: &indiv_pallet_honour::VoteData,
		message: &[u8],
	) -> indiv_pallet_honour::RingProofOf<Runtime> {
		use alloc::{vec, vec::Vec};
		use indiv_support::traits::{AppendOnlyMembers, RingMode};
		use verifiable::ring::RingDomainSize;

		let ring_exponent = <Runtime as indiv_pallet_people::Config>::RingExponent::get();
		let ring_index: RingIndex = 0;

		// Build a one-member people ring in the configured member service. Mirrors the targeted
		// setup used by `indiv_pallet_people`'s own proof benchmarks (`create_collection` +
		// `add_members` + `onboard_all_and_build_ring`) rather than the full `process_maintenance`
		// sweep, which is heavier and runs once per benchmark repeat.
		Members::create_collection(
			PeopleCollectionOwner::get(),
			indiv_pallet_people::PEOPLE_MEMBER_IDENTIFIER,
			1,
			RingMode::Flexible,
			ring_exponent,
			None,
		)
		.expect("benchmark: people collection must be created");

		let secret =
			BandersnatchVrfVerifiable::new_secret(sp_core::twox_256(b"honour-bench-voter"));
		let member = BandersnatchVrfVerifiable::member_from_secret(&secret);

		Members::add_members(indiv_pallet_people::PEOPLE_MEMBER_IDENTIFIER, vec![member])
			.expect("benchmark: ring member must be added");
		Members::initialize_chunks(ring_exponent);
		Members::onboard_all_and_build_ring(
			indiv_pallet_people::PEOPLE_MEMBER_IDENTIFIER,
			ring_index,
		)
		.expect("benchmark: people ring must be built");

		// Open a commitment against the ring members the member service just baked in.
		let ring_members =
			Members::ring_members(indiv_pallet_people::PEOPLE_MEMBER_IDENTIFIER, ring_index);
		let domain: RingDomainSize =
			ring_exponent.try_into().expect("people ring exponent maps to a domain size");
		let commitment = BandersnatchVrfVerifiable::open(domain, &member, ring_members.into_iter())
			.expect("benchmark: commitment must open");

		let contexts = vote.get_contexts();
		let contexts: Vec<&[u8]> = contexts.iter().map(|c| &c[..]).collect();
		let (proof, _) = BandersnatchVrfVerifiable::create_multi_context(
			commitment, &secret, &contexts, message,
		)
		.expect("benchmark: proof creation must succeed");
		proof
	}
}

impl indiv_pallet_honour::Config for Runtime {
	type WeightInfo = indiv_pallet_honour::weights::SubstrateWeight<Runtime>;
	type MemberService = Members;
	type Clock = Timestamp;
	type PointFreezeDuration = HonourPointFreezeDuration;
	type CallMortality = HonourCallMortality;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = HonourBenchmarkHelper;
}

parameter_types! {
	pub const AirdropPalletId: PalletId = PalletId(*b"pop/adrp");
}

impl indiv_pallet_airdrop::Config for Runtime {
	type WeightInfo = indiv_pallet_airdrop::weights::SubstrateWeight<Runtime>;
	type MemberService = Members;
	type Fungibles = AssetsWithHolder;
	type ManagerOrigin = EnsureRoot<Self::AccountId>;
	type PalletId = AirdropPalletId;
	type UnixTime = Timestamp;
	// Real relay-chain randomness, replacing the grindable `ParentHashRandomness` placeholder.
	// This is HALF the fix: it only produces a value if `pallet-relay-randomness` is actually
	// fed by `OnSystemEvent` in lib.rs. Without that, this compiles, runs, never populates,
	// and every airdrop stalls in `AwaitingEntropy` forever with no error.
	type Randomness = indiv_pallet_relay_randomness::RelayBlockRandomness<Runtime>;
	type AccountIdToPublic = AccountIdToSr25519Public;
	type ClearLimit = ConstU32<100>;
	type DrawLimit = ConstU32<100>;
	type OffchainWorkerInterval = ConstU32<1>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = AirdropBenchmarkHelper;
}

// `ParentHashRandomness` was DELETED here, not re-shimmed.
//
// It implemented `indiv_support::traits::CurrentBlockRandomness`, a trait individuality v0.3.1
// removed. It was always a placeholder: the parent block hash is chosen by the parachain block
// author and is grindable, so it was never safe for the airdrop draw it fed. The real relay-chain
// randomness now arrives via `pallet-relay-randomness` (see `RelayBlockRandomness` below and
// `OnSystemEvent` in lib.rs). Do not reintroduce a parent-hash shim to "make it compile".

/// Direct byte-level reinterpretation of an `AccountId32` as an sr25519 public key.
pub struct AccountIdToSr25519Public;
impl sp_runtime::traits::TryConvert<AccountId, sp_core::sr25519::Public>
	for AccountIdToSr25519Public
{
	fn try_convert(account: AccountId) -> Result<sp_core::sr25519::Public, AccountId> {
		let raw: [u8; 32] = account.clone().into();
		Ok(sp_core::sr25519::Public::from_raw(raw))
	}
}

#[cfg(feature = "runtime-benchmarks")]
pub struct AirdropBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl indiv_pallet_airdrop::benchmarking::BenchmarkHelper<Runtime> for AirdropBenchmarkHelper {
	fn set_unix_time(now: core::time::Duration) {
		pallet_timestamp::Now::<Runtime>::put(now.as_millis() as u64);
	}

	fn create_asset_id_parameter(id: u32) -> <Runtime as pallet_assets::Config>::AssetId {
		// Mirror `AssetsBenchmarkHelper::create_asset_id_parameter` and ensure the asset exists
		// so the airdrop pallet's pot can hold/transfer.
		use frame_support::traits::fungibles::Create;
		let location = xcm::latest::Location::new(
			1,
			[
				xcm::latest::Junction::Parachain(ASSET_HUB_ID),
				xcm::latest::Junction::PalletInstance(50),
				xcm::latest::Junction::GeneralIndex(id as u128),
			],
		);
		if !<Assets as frame_support::traits::fungibles::Inspect<AccountId>>::asset_exists(
			location.clone(),
		) {
			let owner: AccountId =
				parachain_info::Pallet::<Runtime>::parachain_id().into_account_truncating();
			<Assets as Create<AccountId>>::create(location.clone(), owner, true, 1u32.into())
				.expect("create asset for airdrop bench");
		}
		location
	}

	fn build_membership_proof(
		context: &indiv_support::traits::Context,
		message: &[u8],
		member_seed: u32,
	) -> (indiv_pallet_airdrop::ProofOf<Runtime>, indiv_support::traits::Alias) {
		use indiv_support::{
			genesis::ring_verifier_builder_params,
			traits::{RingMode, PEOPLE_IDENTIFIER},
		};
		use verifiable::ring::{
			ark_vrf::suites::bandersnatch::BandersnatchSha512Ell2, RingDomainSize,
		};

		type Crypto = BandersnatchVrfVerifiable;

		let ring_exponent = MembersFlexibleRingExponent::get();
		let domain: RingDomainSize =
			ring_exponent.try_into().expect("RingExponent → RingDomainSize");
		let chunks = ring_verifier_builder_params::<BandersnatchSha512Ell2>(domain);

		let mut entropy = [0u8; 32];
		entropy[..4].copy_from_slice(&member_seed.to_le_bytes());
		let secret = Crypto::new_secret(entropy);
		let member = Crypto::member_from_secret(&secret);

		// Build a single-member ring with `member`. The resulting `members` value is the on-chain
		// ring root we seed below so verification at `(PEOPLE_IDENTIFIER, ring=0, rev=0)` succeeds.
		let mut intermediate = Crypto::start_members(domain);
		Crypto::push_members(&mut intermediate, core::iter::once(member), |range| {
			Ok(chunks[range].to_vec())
		})
		.expect("push_members for single bench member");
		let members = Crypto::finish_members(intermediate.clone());

		// Seed the Members pallet so `verify_membership_at_rev(PEOPLE_IDENTIFIER, 0, 0, ...)`
		// works.
		if indiv_pallet_members::Collections::<Runtime>::get(PEOPLE_IDENTIFIER).is_none() {
			indiv_pallet_members::Collections::<Runtime>::insert(
				PEOPLE_IDENTIFIER,
				indiv_pallet_members::types::CollectionInfo {
					owner: indiv_pallet_members::types::CollectionOwner::External(
						PeopleCollectionOwner::get(),
					),
					mode: RingMode::Flexible,
					ring_size: ring_exponent,
					self_inclusion_delay: Some(SelfInclusionDelayValue::get()),
				},
			);
		}
		indiv_pallet_members::Root::<Runtime>::insert(
			PEOPLE_IDENTIFIER,
			0u32,
			indiv_pallet_members::types::RingRoot {
				root: members.clone(),
				revision: 0,
				intermediate,
			},
		);

		let commitment =
			Crypto::open(domain, &member, core::iter::once(member)).expect("open commitment");
		let (proof, _aliases) =
			Crypto::create_multi_context(commitment, &secret, &[&context[..]], message)
				.expect("create membership proof");
		let alias = Crypto::alias_in_context(&secret, &context[..]).expect("alias_in_context");
		(proof, alias)
	}

	fn account_keypair_for(seed: u32) -> (AccountId, sp_core::sr25519::Pair) {
		use sp_core::Pair as _;
		use sp_runtime::traits::IdentifyAccount;
		let mut entropy = [0u8; 32];
		entropy[..4].copy_from_slice(&seed.to_le_bytes());
		let pair = sp_core::sr25519::Pair::from_seed(&entropy);
		let account_id: AccountId = MultiSigner::Sr25519(pair.public()).into_account();
		(account_id, pair)
	}
}

#[cfg(feature = "runtime-benchmarks")]
pub struct GamePalletBenchmarkHelper {}

#[cfg(feature = "runtime-benchmarks")]
impl GamePalletBenchmarkHelper {
	fn sign(seed: u64, msg: &[u8]) -> Signature {
		let mut entropy = [0u8; 32];
		entropy[..8].copy_from_slice(&seed.to_le_bytes()[..]);
		// sp-core doesn't expose the signing for the runtime, so we use the underlying library
		let secret = ed25519_zebra::SigningKey::from(entropy);
		sp_core::ed25519::Signature::from_raw(secret.sign(msg).into()).into()
	}

	fn create_account_id(seed: u64) -> AccountId {
		use sp_core::Pair;
		use sp_runtime::traits::IdentifyAccount;
		let mut entropy = [0u8; 32];
		entropy[..8].copy_from_slice(&seed.to_le_bytes()[..]);
		let pair = sp_core::ed25519::Pair::from_seed(&entropy);
		pair.public().into_account().into()
	}
}

#[cfg(feature = "runtime-benchmarks")]
impl
	indiv_pallet_game::BenchmarkHelper<
		Signature,
		MultiSignature,
		AccountId,
		AccountId,
		<Runtime as pallet_assets::Config>::AssetId,
	> for GamePalletBenchmarkHelper
{
	fn create_account(seed: u64) -> AccountId {
		Self::create_account_id(seed)
	}

	fn sign_account(seed: u64, msg: &[u8]) -> Signature {
		Self::sign(seed, msg)
	}

	fn create_ticket(seed: u64) -> AccountId {
		Self::create_account_id(seed)
	}

	fn sign_ticket(seed: u64, msg: &[u8]) -> MultiSignature {
		Self::sign(seed, msg)
	}

	fn set_valid_time() {
		Timestamp::set_timestamp(1u32.into());
	}

	fn set_time(now: core::time::Duration) {
		// We don't call `set_timestamp` directly because it triggers checks such as aura slot
		pallet_timestamp::Now::<Runtime>::put(now.as_millis() as u64);
	}

	fn fund_account(acc: AccountId) {
		use frame_support::traits::Currency;
		let balance = 1_000_000_000_000_000u128;
		let _ = Balances::make_free_balance_be(&acc, balance);
	}

	fn airdrop_asset_id() -> <Runtime as pallet_assets::Config>::AssetId {
		ExternalAssetLocation::get()
	}
}

pub struct GamePhaseDurations;
impl Get<PhaseDurationValues> for GamePhaseDurations {
	fn get() -> PhaseDurationValues {
		PhaseDurationValues {
			registration: 5 * 60,
			shuffle: 60,
			post_shuffle_margin: 30,
			reporting: 10 * 60,
			player_process: 60,
		}
	}
}

/// Parachain ID of the Bulletin Chain used for data storage via XCM.
pub const BULLETIN_CHAIN_PARA_ID: u32 = 1010;

parameter_types! {
	pub StorageInitializationFundingAccount: AccountId =
		AccountId::from(hex_literal::hex!("d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d"));
	pub StorageInitializationInviteRecipient: AccountId =
		AccountId::from(hex_literal::hex!("8eaf04151687736326c9fea17e25fc5287613693c912909cb226aa4794f26a48"));
	pub PeopleChainSovereignAccount: AccountId = {
		parachain_info::Pallet::<Runtime>::parachain_id().into_account_truncating()
	};
	pub PeopleChainParaId: ParaId = parachain_info::Pallet::<Runtime>::parachain_id();
	/// XCM destination location for the Bulletin Chain.
	pub BulletinChainLocation: Location = Location::new(1, [Parachain(BULLETIN_CHAIN_PARA_ID)]);
}

impl indiv_pallet_storage_initialization::Config for Runtime {
	type WeightInfo = indiv_pallet_storage_initialization::weights::SubstrateWeight<Runtime>;
	type Assets = Assets;
	type ReserveData = ForeignAssetReserveData;
	type ReserveSetter = Assets;
	type InvitesRecipient = StorageInitializationInviteRecipient;
	type AssetsDestAccount = PeopleChainSovereignAccount;
	type AssetHubTransferAmount = ConstU128<5_000_000_000_000>;
	type XcmTimeout = ConstU32<100>;
	#[cfg(not(feature = "runtime-benchmarks"))]
	type XcmSender = crate::xcm_config::XcmRouter;
	#[cfg(feature = "runtime-benchmarks")]
	type XcmSender = bench_xcm_sender::BenchXcmSender;
	type ParachainInfo = PeopleChainParaId;
	type TransferAssetForeignId = ExternalAssetLocation;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct StorageInitBenchmarkHelper;
#[cfg(feature = "runtime-benchmarks")]
impl indiv_pallet_storage_initialization::benchmarking::BenchmarkHelper
	for StorageInitBenchmarkHelper
{
	fn set_unix_time(d: core::time::Duration) {
		pallet_timestamp::Now::<Runtime>::put(d.as_millis() as u64);
	}
}

#[cfg(feature = "runtime-benchmarks")]
mod bench_xcm_sender {
	use xcm::prelude::*;

	/// Bench-only `SendXcm` that proxies `validate` to the real `XcmRouter` (so
	/// the router's version-discovery / fee-factor reads are still captured in
	/// the measured weight) but short-circuits `deliver` to always succeed —
	/// the real `deliver` fails in the bench harness because there's no relay /
	/// HRMP routing set up, which would cause the success-path writes
	/// (`OnPollStatus`, `XcmTransferInitiatedAt`) to be missed.
	pub struct BenchXcmSender;
	impl SendXcm for BenchXcmSender {
		type Ticket = <crate::xcm_config::XcmRouter as SendXcm>::Ticket;

		fn validate(
			dest: &mut Option<Location>,
			xcm: &mut Option<Xcm<()>>,
		) -> SendResult<Self::Ticket> {
			<crate::xcm_config::XcmRouter as SendXcm>::validate(dest, xcm)
		}

		fn deliver(_ticket: Self::Ticket) -> Result<XcmHash, SendError> {
			Ok([0u8; 32])
		}
	}
}

parameter_types! {
	/// The suffix spliced into every product context on this chain.
	///
	/// Bound to the shared `system-parachains-constants` value, NOT to a literal, and to the
	/// SAME constant Asset Hub binds. If the two chains disagree, the same human derives a
	/// different alias on each and the personhood namespace splits. Upstream's default is
	/// `b"paseo"`; Paseo uses `b"dot"`.
	pub DefaultNetworkSuffix: indiv_support::context::ProductContextNetworkSuffix =
		system_parachains_constants::paseo::individuality::NETWORK_SUFFIX
			.to_vec()
			.try_into()
			.expect("NETWORK_SUFFIX fits MAX_NETWORK_SUFFIX_LENGTH; qed");
}

impl indiv_pallet_network_suffix::Config for Runtime {
	type UpdateOrigin = EnsureRoot<Self::AccountId>;
	type DefaultSuffix = DefaultNetworkSuffix;
	type WeightInfo = indiv_pallet_network_suffix::weights::SubstrateWeight<Runtime>;
}

impl indiv_pallet_relay_randomness::Config for Runtime {
	type WeightInfo = indiv_pallet_relay_randomness::weights::SubstrateWeight<Runtime>;
}

impl indiv_pallet_people_lite::Config for Runtime {
	type WeightInfo = indiv_pallet_people_lite::weights::SubstrateWeight<Runtime>;
	type Currency = Balances;
	type PotId = LitePeoplePotId;
	// POLICY, FLAGGED. Gates `register_with_fee` (call index 3), which is NEW in v0.3.1 -- it
	// did not exist on Paseo before, so no existing behaviour is being made costly. The value
	// is upstream's. It is the only thing rate-limiting a brand-new PERMISSIONLESS path into
	// the lite-people ring, on a chain whose native token is faucet-dispensed. See the
	// remaining-risk list in RUNTIME_WIRING_RESULT.md before enactment.
	type RegistrationFee = LitePersonRegistrationFee;
	type Suffix = NetworkSuffix;
	type AttestationAllowanceManager = EnsureRoot<Self::AccountId>;
	type MemberService = Members;
	type CollectionOwner = LitePeopleCollectionOwner;
	type LiteRingExponent = LitePeopleRingExponent;
	type LiteOnboardingSize = LitePeopleOnboardingSize;
	type AttestationSignature = Signature;
	type LiteConsumerRegistrar = Resources;
	type AccountContexts = LitePeopleAccountContexts;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
}

// The eighteen statement-store and long-term-storage limits that used to live here are now
// `pallet_parameters` dynamic parameters, declared in `crate::parameters` with defaults
// byte-identical to the values this block held. See that module.
parameter_types! {
	pub const MinUsernameLength: u32 = 6;
	pub const PersonAuthDuration: u32 = 2 * 24 * 60 * 60; // 2 days
	pub const MinPersonAuthUpdateInterval: u32 = 24 * 60 * 60; // 1 day
	pub const MaxReservationQueueLength: u32 = 10;
}

impl indiv_pallet_resources::Config for Runtime {
	type WeightInfo = indiv_pallet_resources::weights::SubstrateWeight<Runtime>;
	// Same source of truth as Asset Hub's `pgas`/`dotns-gateway` bindings: the on-chain
	// `NetworkSuffix` pallet, whose default is the shared `system-parachains-constants` value.
	type Suffix = NetworkSuffix;
	type MemberService = Members;
	// `MaxUsernameLength` was removed from the pallet's `Config` in individuality v0.3.1.
	type MinUsernameLength = MinUsernameLength;
	type PersonAuthDuration = PersonAuthDuration;
	type AccountsApiAllowance = AccountsApiAllowance;
	type StmtStoreSlotsPerPeriod = StmtStoreSlotsPerPeriod;
	type LiteStmtStoreSlotsPerPeriod = LiteStmtStoreSlotsPerPeriod;
	type StmtStoreCleanupLimit = StmtStoreCleanupLimit;
	type StmtStoreReplacementCooldown = StmtStoreReplacementCooldown;
	type StmtStoreGraceWindow = StmtStoreGraceWindow;
	// individuality v0.3.1 renamed the "friend request" allowance family to "notification".
	// The rename is nominal only: every value below is byte-identical to the `FriendRequest*`
	// value Paseo runs today, AND to upstream's own v0.3.1 value, so no behaviour changes.
	// `FriendRequestGraceWindow` and `FriendRequestRetentionDuration` have no `Notification*`
	// counterpart — they were dropped from `Config` outright, not renamed.
	type NotificationAllowance = NotificationAllowance;
	type NotificationSlotsPerPeriod = NotificationSlotsPerPeriod;
	type LiteNotificationSlotsPerPeriod = LiteNotificationSlotsPerPeriod;
	type NotificationPeriodDuration = NotificationPeriodDuration;
	type OffchainWorkerInterval = ConstU32<1>;
	type MinPersonAuthUpdateInterval = MinPersonAuthUpdateInterval;
	type EnsurePerson = indiv_pallet_people::EnsurePersonalAliasInContext<Runtime>;
	type EnsureLitePerson = indiv_pallet_people_lite::EnsureLitePerson<Runtime>;
	type Clock = Timestamp;
	type OffchainSignature = Signature;
	type LitePersonStatementLimit = LitePersonStatementLimit;
	type PersonStatementLimit = PersonStatementLimit;
	type MaxReservationQueueLength = MaxReservationQueueLength;
	type ManagerOrigin = EnsureRoot<AccountId>;
	type LongTermStoragePeriodDuration = LongTermStoragePeriodDuration;
	type LongTermStorageGraceWindow = LongTermStorageGraceWindow;
	type LongTermStorageClaimsPerPeriod = LongTermStorageClaimsPerPeriod;
	type LongTermStorageAllowanceForPeople = LongTermStorageAllowanceForPeople;
	type LongTermStorageAllowanceForLitePeople = LongTermStorageAllowanceForLitePeople;
	type LongTermStorageDataStore = BulletinDataStore;
	type LongTermStorageCleanupLimit = LongTermStorageCleanupLimit;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = benchmark_utils::ResourcesBenchHelper;
}

parameter_types! {
	pub const CoinagePalletId: PalletId = PalletId(*b"coinage ");
	pub CoinageCollectionOwner: Location = Location::new(0, [PalletInstance(68)]);
}

/// The asset amount of a coin of denomination zero, used only when benchmarks create the coinage
/// instance. The external asset (Asset Hub asset `50_000_413`) has 6 decimals, so this is $0.01.
///
/// Ported verbatim from individuality v0.3.1's `next-people-paseo`, which wraps the very same
/// Asset Hub asset id.
#[cfg(feature = "runtime-benchmarks")]
pub const COINAGE_ASSET_UNIT: Balance = 10u128.pow(4);

// `LitePeopleProof` and `PeopleProof` were DELETED here.
//
// They were runtime-local wrappers implementing `indiv_pallet_coinage::ValidateProof`, bound to
// the pallet's old `LitePeopleProof` / `PeopleProof` config pair. individuality v0.3.1 replaced
// that pair with a single `Config::MembershipProof`, which this runtime binds to the `People`
// pallet's own validator. `ValidateProof::validate_proof` also gained a fourth parameter, so
// these impls no longer match the trait. Upstream has no counterpart to them.

#[cfg(feature = "runtime-benchmarks")]
pub struct CoinageBenchHelper;

#[cfg(feature = "runtime-benchmarks")]
impl indiv_pallet_coinage::BenchmarkHelper<Runtime> for CoinageBenchHelper {
	fn setup_assets() {
		use frame_support::traits::fungibles::{Inspect, Mutate};
		benchmark_utils::ensure_external_asset_exists();
		// v0.3.1 replaced the single `UnderlyingAssetId` storage value with per-asset instances,
		// so the benchmark set-up now has to create the instance rather than write a storage item.
		if indiv_pallet_coinage::AssetToInstance::<Runtime>::iter_key_prefix(
			ExternalAssetLocation::get(),
		)
		.next()
		.is_none()
		{
			// What governance is expected to do before creating an instance: give the pallet
			// account a balance buffer so fee flows that empty its free balance cannot kill it.
			<AssetsWithHolder as Mutate<_>>::mint_into(
				ExternalAssetLocation::get(),
				&indiv_pallet_coinage::Pallet::<Runtime>::pallet_account(),
				<AssetsWithHolder as Inspect<_>>::minimum_balance(ExternalAssetLocation::get()),
			)
			.expect("minting the pallet account buffer should succeed");
			Coinage::create_sufficient_instance(
				RuntimeOrigin::root(),
				ExternalAssetLocation::get(),
				COINAGE_ASSET_UNIT,
			)
			.expect("create_sufficient_instance should succeed");
		}
	}

	fn setup_asset_without_instance() -> Location {
		use frame_support::traits::fungibles::{Inspect, Mutate};
		benchmark_utils::ensure_external_asset_exists();
		// What governance does before `create_sufficient_instance`: the pallet account's balance
		// buffer.
		<AssetsWithHolder as Mutate<_>>::mint_into(
			ExternalAssetLocation::get(),
			&indiv_pallet_coinage::Pallet::<Runtime>::pallet_account(),
			<AssetsWithHolder as Inspect<_>>::minimum_balance(ExternalAssetLocation::get()),
		)
		.expect("minting the pallet account buffer should succeed");
		ExternalAssetLocation::get()
	}

	fn fund_account(who: &AccountId, amount: u128) {
		use frame_support::traits::fungibles::Mutate;
		<AssetsWithHolder as Mutate<_>>::mint_into(ExternalAssetLocation::get(), who, amount)
			.expect("Failed to fund account");
	}

	fn create_extra_asset(seed: u32, who: &AccountId) -> Location {
		use frame_support::traits::{
			fungibles::{Create, Inspect, Mutate},
			PalletInfoAccess,
		};
		let location = Self::extra_asset_id(seed);
		if !<Assets as Inspect<_>>::asset_exists(location.clone()) {
			<Assets as Create<_>>::create(
				location.clone(),
				ParaId::new(<Assets as PalletInfoAccess>::index() as u32).into_account_truncating(),
				true,
				1u32.into(),
			)
			.expect("Failed to create extra asset");
		}
		<AssetsWithHolder as Mutate<_>>::mint_into(location.clone(), who, 1_000_000 * UNITS)
			.expect("Failed to fund extra asset");
		location
	}

	fn extra_asset_id(seed: u32) -> Location {
		Location::new(
			1,
			[
				xcm::latest::Junction::Parachain(ASSET_HUB_ID),
				xcm::latest::Junction::PalletInstance(50),
				xcm::latest::Junction::GeneralIndex(1_000_000u128 + seed as u128),
			],
		)
	}

	fn set_time(now: core::time::Duration) {
		pallet_timestamp::Now::<Runtime>::put(now.as_millis() as u64);
	}

	fn setup_fee_conversion() {
		use frame_support::traits::fungible::Mutate as _;

		let native = crate::xcm_config::RelayLocation::get();
		let asset = ExternalAssetLocation::get();
		if pallet_asset_conversion::Pools::<Runtime>::contains_key((native.clone(), asset.clone()))
		{
			return;
		}

		// Native has 10 decimals, the external asset has 6, and 1 raw asset ($10^-6) is worth
		// 10^4 raw native ($10^-10), so the pool holds that ratio — the same 10^4 rate the
		// deleted `setup_conversion_rate` wrote into `pallet_asset_rate`. The depth is far above
		// any benchmarked fee so that the conversions do not move the price.
		let native_liquidity: Balance = 1_000 * UNITS;
		let asset_liquidity: Balance = native_liquidity / 10_000;

		let provider: AccountId = [42u8; 32].into();
		Balances::mint_into(&provider, native_liquidity.saturating_mul(2))
			.expect("failed to fund the liquidity provider with native");
		Self::fund_account(&provider, asset_liquidity.saturating_mul(2));

		let origin = RuntimeOrigin::signed(provider.clone());
		crate::AssetConversion::create_pool(
			origin.clone(),
			alloc::boxed::Box::new(native.clone()),
			alloc::boxed::Box::new(asset.clone()),
		)
		.expect("failed to create the fee conversion pool");
		crate::AssetConversion::add_liquidity(
			origin,
			alloc::boxed::Box::new(native),
			alloc::boxed::Box::new(asset),
			native_liquidity,
			asset_liquidity,
			1,
			1,
			provider,
		)
		.expect("failed to add liquidity to the fee conversion pool");
	}

	fn create_people_proof(
		context: &[u8],
		msg: &[u8],
		_alias: Alias,
	) -> indiv_pallet_people::MembershipProof<Runtime> {
		use frame_support::dispatch::RawOrigin;
		use indiv_support::traits::{AddOnlyPeopleTrait, AppendOnlyMembers};
		use verifiable::ring::RingDomainSize;

		// Initialize the people collection and chunks if not already created
		indiv_pallet_people::Pallet::<Runtime>::initialize_people_collection();
		let ring_exponent = <Runtime as indiv_pallet_people::Config>::RingExponent::get();
		indiv_pallet_members::Pallet::<Runtime>::initialize_chunks(ring_exponent);

		let entropy = sp_core::twox_256(b"people_for_coinage:42");
		let secret = BandersnatchVrfVerifiable::new_secret(entropy);
		let member = BandersnatchVrfVerifiable::member_from_secret(&secret);

		// Set onboarding size so members get onboarded immediately
		indiv_pallet_members::OnboardingSize::<Runtime>::insert(
			indiv_pallet_people::PEOPLE_MEMBER_IDENTIFIER,
			1,
		);

		// Use force_recognize_personhood to add member
		indiv_pallet_people::Pallet::<Runtime>::force_recognize_personhood(
			RawOrigin::Root.into(),
			vec![member],
		)
		.expect("should recognize personhood");

		// Onboard all members and build the ring
		indiv_pallet_members::Pallet::<Runtime>::process_maintenance();

		// Get ring keys from members pallet (page 0)
		let ring_index: RingIndex = 0;
		let ring_keys = indiv_pallet_members::RingKeys::<Runtime>::get((
			indiv_pallet_people::PEOPLE_MEMBER_IDENTIFIER,
			ring_index,
			0u32,
		));

		let commitment = BandersnatchVrfVerifiable::open(
			RingDomainSize::Domain11,
			&member,
			ring_keys.into_iter(),
		)
		.expect("should open commitment");
		let (proof, _alias) = BandersnatchVrfVerifiable::create(commitment, &secret, context, msg)
			.expect("should create proof");

		indiv_pallet_people::MembershipProof { proof, ring: ring_index, revision: 0 }
	}

	fn create_lite_people_proof(
		context: &[u8],
		msg: &[u8],
		_alias: Alias,
	) -> indiv_pallet_people::MembershipProof<Runtime> {
		use indiv_support::traits::AppendOnlyMembers as _;
		use sp_core::Pair;
		use sp_runtime::traits::IdentifyAccount;
		use verifiable::ring::RingDomainSize;

		let ring_exponent = LitePeopleRingExponent::get();
		indiv_pallet_members::Pallet::<Runtime>::initialize_chunks(ring_exponent);

		let entropy = [77u8; 32];
		let pair = sp_core::ed25519::Pair::from_seed(&entropy);
		let account: AccountId = pair.public().into_account().into();

		let ring_secret = BandersnatchVrfVerifiable::new_secret([88u8; 32]);
		let ring_member = BandersnatchVrfVerifiable::member_from_secret(&ring_secret);

		indiv_pallet_people_lite::LitePeople::<Runtime>::insert(
			&account,
			indiv_pallet_people_lite::types::LitePersonInfo {
				ring_vrf_key: ring_member,
				method: indiv_pallet_people_lite::types::RecognitionMethod::UniqueDevice(
					account.clone(),
				),
			},
		);
		frame_system::Pallet::<Runtime>::inc_sufficients(&account);
		Members::create_collection(
			LitePeopleCollectionOwner::get(),
			indiv_pallet_people_lite::LITE_PEOPLE_MEMBER_IDENTIFIER,
			LitePeopleOnboardingSize::get(),
			indiv_support::traits::RingMode::AppendOnly,
			LitePeopleRingExponent::get(),
			None,
		)
		.expect("benchmark: lite people collection must be created");
		Members::add_members(
			indiv_pallet_people_lite::LITE_PEOPLE_MEMBER_IDENTIFIER,
			vec![ring_member],
		)
		.expect("benchmark: lite people member must be added");
		indiv_pallet_members::OnboardingSize::<Runtime>::insert(
			indiv_pallet_people_lite::LITE_PEOPLE_MEMBER_IDENTIFIER,
			1,
		);
		indiv_pallet_members::Pallet::<Runtime>::process_maintenance();

		let ring_index: RingIndex = 0;
		let ring_keys = indiv_pallet_members::RingKeys::<Runtime>::get((
			indiv_pallet_people_lite::LITE_PEOPLE_MEMBER_IDENTIFIER,
			ring_index,
			0u32,
		));
		let commitment = BandersnatchVrfVerifiable::open(
			RingDomainSize::Domain11,
			&ring_member,
			ring_keys.into_iter(),
		)
		.expect("should open commitment");
		let (proof, _) = BandersnatchVrfVerifiable::create(commitment, &ring_secret, context, msg)
			.expect("should create lite proof");

		indiv_pallet_people::MembershipProof { proof, ring: ring_index, revision: 0 }
	}
}

/// Union of the native balance and the chain's assets, so a coinage instance can be denominated
/// in the native token as well as in an asset. Mirrors upstream's `NativeAndAssets`.
///
/// Required by v0.3.1: `Config::Fungibles` now has to cover whatever `NativeAssetKind` names,
/// because the fee paths short-circuit to a plain `Fungibles::transfer` when an instance's asset
/// IS the native one. With the old assets-only binding that transfer could not be performed.
pub type NativeAndAssets = frame_support::traits::fungible::UnionOf<
	Balances,
	AssetsWithHolder,
	assets_common::local_and_foreign_assets::TargetFromLeft<crate::xcm_config::RelayLocation, Location>,
	Location,
	AccountId,
>;

pub struct CoinageInstanceCreationPrice;
impl sp_runtime::traits::Convert<frame_support::traits::Footprint, Balance>
	for CoinageInstanceCreationPrice
{
	fn convert(footprint: frame_support::traits::Footprint) -> Balance {
		system_para_deposit(footprint.count as u32, footprint.size as u32)
	}
}

parameter_types! {
	pub const CoinageInstanceCreationHoldReason: RuntimeHoldReason =
		RuntimeHoldReason::Coinage(indiv_pallet_coinage::HoldReason::InstanceCreationDeposit);
	pub storage CoinageLoadDeposit: (Location, Balance) =
		(crate::xcm_config::RelayLocation::get(), system_para_deposit(4, 300));
	/// POLICY, FLAGGED — DELIBERATELY `false`, WHERE UPSTREAM USES `true`.
	///
	/// Gates `create_sponsored_instance`, which is NEW in individuality v0.3.1. Paseo has no
	/// sponsored instances today because the call did not exist, so `false` is the setting that
	/// preserves current on-chain behaviour. Setting it `true` at enactment would open
	/// permissionless instance creation on a live chain in the same block as the upgrade,
	/// silently and with no extrinsic. Root can turn it on deliberately afterwards.
	pub storage CoinageEnablePermissionless: bool = false;
}

impl indiv_pallet_coinage::Config for Runtime {
	type MemberService = Members;
	type RecyclerRingExponent = RecyclerRingExponent;
	type PaidUnloadTokenRingExponent = PaidUnloadTokenRingExponent;
	type UnixTime = Timestamp;
	type PalletId = CoinagePalletId;
	type WeightInfo = indiv_pallet_coinage::weights::SubstrateWeight<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = CoinageBenchHelper;
	type MaximumAge = ConstU16<16>;
	type NativeFungible = Balances;
	// Widened from `AssetsWithHolder`; see `NativeAndAssets` above.
	type Fungibles = NativeAndAssets;
	// Replaces `UnderlyingAssetIdManager`, which was also `EnsureRoot`.
	type AdminOrigin = EnsureRoot<AccountId>;
	type SponsorOrigin = frame_system::EnsureSigned<AccountId>;
	type EnablePermissionless = CoinageEnablePermissionless;
	type LoadDeposit = CoinageLoadDeposit;
	type InstanceCreationDeposit = frame_support::traits::fungible::HoldConsideration<
		AccountId,
		Balances,
		CoinageInstanceCreationHoldReason,
		CoinageInstanceCreationPrice,
	>;
	type MinimumExponent = ConstI8<0>;
	type MaximumExponent = ConstI8<14>;
	type MinimumExponentForOutputUnloadFee = ConstI8<0>;
	type MaxSplitOutputs = ConstU32<32>;
	type MaxConsolidation = ConstU32<64>;
	type MaxBatchUnpaidLoad = ConstU32<10>;
	type RecyclerExpirationTime = ConstU32<{ 90 * 24 * 60 * 60 }>; // ~3 months
	type UnloadTokenTimePeriodPeopleLitePeople = ConstU32<{ 24 * 60 * 60 }>; // 1 day

	// ####################################################################################
	// POLICY, FLAGGED — SILENT DENOMINATION CHANGE. DO NOT "RESTORE" THE OLD NUMBERS.
	//
	// v0.3.1 changed the bound on these two from `Get<FungiblesBalanceOf>` to
	// `Get<NativeBalanceOf>`; the pallet doc went from "expressed in the underlying asset" to
	// "expressed in the native currency". Both are `u128`, so Paseo's old values
	// (`200 * 10^4` and `50 * 10^4`, i.e. $2.00 and $0.50 against a 10^6-decimal asset) still
	// COMPILE — they would just mean 0.0002 PAS and 0.00005 PAS, far below any real unload fee,
	// giving every person and lite person ZERO free unloads with no error anywhere.
	//
	// The values below are upstream's. They are NOT a translation of Paseo's old economics:
	// Paseo's people:lite ratio was 4:1 ($2.00 / $0.50); upstream's is 2:1. Nobody has
	// calibrated a PAS-denominated free-unload budget for Paseo. MUST be sanity-checked
	// against the actual native unload fee before enactment.
	// ####################################################################################
	// Allowance of 20 whole native tokens per period (fee is dynamic based on multiplier).
	type UnloadTokenAllowancePerTimePeriodForPeople = ConstU128<{ 20 * UNITS }>;
	// Allowance of 10 whole native tokens per period (fee is dynamic based on multiplier).
	type UnloadTokenAllowancePerTimePeriodForLitePeople = ConstU128<{ 10 * UNITS }>;
	// Bumped temporarily; revisit once the wallet handles `maxFee` and the
	// "user ran out of free unloads" UX.
	type MaxFreeUnloadTokensPerTimePeriod = ConstU32<1000>;
	// Replaces the separate `LitePeopleProof` / `PeopleProof` pair.
	type MembershipProof = People;
	// Bound to the on-chain AMM, as both reference integrations do: individuality v0.3.1's
	// `next-people-paseo` and polkadot-fellows' `people-polkadot` each bind their own
	// `AssetConversion`. Reached only when a coin instance's asset is NOT the native asset;
	// native-denominated instances short-circuit to a plain transfer.
	// 🔴 A pool with no liquidity behaves exactly like the fail-closed adapter this replaces:
	// paid unloads in a non-native asset fail until a native/asset pool is seeded.
	type FeeConversion = AssetConversion;
	type NativeAssetKind = crate::xcm_config::RelayLocation;
	type WeightToFee = TransactionPayment;
	type PaidUnloadTokenTimePeriod = ConstU32<{ 3 * 24 * 60 * 60 }>; // 3 days
	type PaidUnloadTokenRingExpirationTime = ConstU32<{ 4 * 24 * 60 * 60 }>; // 4 days
	type FeeDestination = TypedGetToGet<pallet_collator_selection::StakingPotAccountId<Runtime>>;
	type OffchainWorkerInterval = ConstU32<4>; // higher in prod
	type CoinFailureLockPeriod = ConstU64<60>;
}

/// Origin check that validates the caller is a sibling parachain and extracts its `ParaId`.
pub struct EnsureSiblingParachain;
impl frame_support::traits::EnsureOrigin<RuntimeOrigin> for EnsureSiblingParachain {
	type Success = cumulus_primitives_core::ParaId;

	fn try_origin(o: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
		match o.clone().into() {
			Ok(cumulus_pallet_xcm::Origin::SiblingParachain(id)) => Ok(id),
			_ => Err(o),
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
		Ok(cumulus_pallet_xcm::Origin::SiblingParachain(ASSET_HUB_ID.into()).into())
	}
}

impl indiv_pallet_members_notifier::Config for Runtime {
	type WeightInfo = indiv_pallet_members_notifier::weights::SubstrateWeight<Runtime>;
	type XcmRouter = crate::xcm_config::XcmRouter;
	type ChannelInfo = ParachainSystem;
	type ManageOrigin = EnsureRoot<AccountId>;
	type EnsureSubscriberOrigin = EnsureSiblingParachain;
	type Crypto = BandersnatchVrfVerifiable;
	type RingRootsProvider = Members;
	type Clock = Timestamp;
	type MaxSubscribers = ConstU32<10>;
	type MaxUpdatesPerBatch = ConstU32<10>;
	type MaxCollectionsPerSubscriber = ConstU32<3>;
	type MaxCollections = ConstU32<100>;
	type UpdateTriggerBlocks = ConstU32<1>;
	type UpdateTriggerThreshold = ConstU32<1>;
	type RequestReplayRemoteWeight = ConstantWeight;
	type OffchainWorkerInterval = ConstU32<1>;
	type StuckBatchTimeout = ConstU32<100>;
	type ReplayCooldownSeconds = ConstU64<60>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = benchmark_utils::MembersNotifierBenchHelper;
}

parameter_types! {
	pub ConstantWeight: Weight = Weight::from_parts(10_000, 0);
}

pub struct SubjectBlockRandommess<Runtime>(PhantomData<Runtime>);
impl<Runtime> Randomness<[u8; 32], u32> for SubjectBlockRandommess<Runtime>
where
	Runtime: frame_system::Config,
	[u8; 32]: From<<Runtime as frame_system::Config>::Hash>,
{
	fn random(subject: &[u8]) -> ([u8; 32], u32) {
		// hash subject into 32 bytes
		let subject_hash = subject.using_encoded(sp_io::hashing::blake2_256);

		// hash current block into 32 bytes
		let block_number = frame_system::Pallet::<Runtime>::block_number();
		let block_hash = frame_system::Pallet::<Runtime>::block_hash(block_number);
		let block_hash: [u8; 32] = block_hash.into();

		// bitwise XOR on subject hash and block hash
		let mut randomness = [0u8; 32];
		for byte in 0..subject_hash.len() {
			randomness[byte] = subject_hash[byte] ^ block_hash[byte];
		}
		(randomness, 0)
	}
}

/// A type containing the encoding of the Bulletin Chain pallets in its runtime. Used to construct
/// remote calls. The codec index must correspond to the index of `TransactionStorage` in the
/// `construct_runtime` of the remote chain.
#[derive(Encode, Decode)]
enum BulletinPallets<AccountId: Encode> {
	// transaction storage: 40
	#[codec(index = 40)]
	TransactionStorage(TransactionStorageCalls<AccountId>),
}

/// Call encoding for the bulletin TransactionStorage calls invoked over XCM.
#[derive(Encode, Decode)]
enum TransactionStorageCalls<AccountId: Encode> {
	// call index: 3
	// pub fn authorize_account(
	// 	origin: OriginFor<T>,
	// 	who: T::AccountId,
	// 	transactions: u32,
	// 	bytes: u64,
	// )
	#[codec(index = 3)]
	AuthorizeAccount(AccountId, u32, u64),
	// call index: 7
	// pub fn refresh_account_authorization(
	// 	origin: OriginFor<T>,
	// 	who: T::AccountId,
	// )
	#[codec(index = 7)]
	RefreshAccountAuthorization(AccountId),
}

#[allow(unused)]
pub struct BulletinDataStore;
impl AllocateStorage<AccountId> for BulletinDataStore {
	fn allocate_storage(who: &AccountId, len: u64, count: u32) -> DispatchResult {
		use crate::people::TransactionStorageCalls::AuthorizeAccount;

		let destination = BulletinChainLocation::get();
		let authorize = BulletinPallets::<AccountId>::TransactionStorage(AuthorizeAccount(
			who.clone(),
			count,
			len,
		));

		// The program to execute on the Bulletin Chain.
		let program = alloc::vec![
			UnpaidExecution { weight_limit: WeightLimit::Unlimited, check_origin: None },
			Transact {
				origin_kind: OriginKind::Xcm,
				fallback_max_weight: None,
				call: authorize.encode().into(),
			},
		]
		.into();

		// send
		#[allow(clippy::bind_instead_of_map)]
		send_xcm::<xcm_config::XcmRouter>(destination, program)
			.map(|_| ())
			.or_else(|e| {
				// Ignore errors during benchmarks.
				// TODO: maybe revisit
				#[cfg(feature = "runtime-benchmarks")]
				{
					let _ = e;
					Ok::<(), ()>(())
				}

				#[cfg(not(feature = "runtime-benchmarks"))]
				{
					Err(e)
				}
			})
			.map_err(|_| pallet_xcm::Error::<Runtime>::SendFailure)?;

		Ok(())
	}

	fn refresh_allocation(who: &AccountId) -> DispatchResult {
		use crate::people::TransactionStorageCalls::RefreshAccountAuthorization;

		let destination = BulletinChainLocation::get();
		let refresh = BulletinPallets::<AccountId>::TransactionStorage(
			RefreshAccountAuthorization(who.clone()),
		);

		// The program to execute on the Bulletin Chain.
		let program = alloc::vec![
			UnpaidExecution { weight_limit: WeightLimit::Unlimited, check_origin: None },
			Transact {
				origin_kind: OriginKind::Xcm,
				fallback_max_weight: None,
				call: refresh.encode().into(),
			},
		]
		.into();

		// send
		#[allow(clippy::bind_instead_of_map)]
		send_xcm::<xcm_config::XcmRouter>(destination, program)
			.map(|_| ())
			.or_else(|e| {
				// Ignore errors during benchmarks.
				// TODO: maybe revisit
				#[cfg(feature = "runtime-benchmarks")]
				{
					let _ = e;
					Ok::<(), ()>(())
				}

				#[cfg(not(feature = "runtime-benchmarks"))]
				{
					Err(e)
				}
			})
			.map_err(|_| pallet_xcm::Error::<Runtime>::SendFailure)?;
		Ok(())
	}
}

// ---------------------------------------------------------------------------
// Origin restriction, signature verification and feeless payment.
// ---------------------------------------------------------------------------

// TODO: choose good value
const PEOPLE_IDENTITY_AND_ALIAS_ALLOWANCE_MAX: Balance = UNITS;
const PEOPLE_IDENTITY_AND_ALIAS_ALLOWANCE_RECOVERY: Balance = CENTS;
const POI_CANDIDATE_RECOVERY: Balance = CENTS;
const ACCOUNT_PARTICIPANT_RECOVERY: Balance = CENTS;
const LITE_PERSON_ALLOWANCE_MAX: Balance = UNITS;
const LITE_PERSON_ALLOWANCE_RECOVERY: Balance = MILLICENTS;

#[derive(
	Clone,
	Encode,
	Decode,
	Debug,
	MaxEncodedLen,
	scale_info::TypeInfo,
	Eq,
	PartialEq,
	DecodeWithMemTracking,
)]
pub enum RestrictedEntity {
	PersonalAlias(Alias),
	PersonalIdentity(u64),
	ReferredCandidate(AccountId),
	AccountParticipant(AccountId),
	InvitedCandidate(AccountId),
	LitePerson(AccountId),
}

impl indiv_pallet_origin_restriction::RestrictedEntity<OriginCaller, Balance> for RestrictedEntity {
	fn allowance(&self) -> indiv_pallet_origin_restriction::Allowance<Balance> {
		match self {
			RestrictedEntity::PersonalAlias(_) | RestrictedEntity::PersonalIdentity(_) =>
				Allowance {
					max: PEOPLE_IDENTITY_AND_ALIAS_ALLOWANCE_MAX,
					recovery_per_block: PEOPLE_IDENTITY_AND_ALIAS_ALLOWANCE_RECOVERY,
				},
			RestrictedEntity::ReferredCandidate(_) =>
				Allowance { max: 0, recovery_per_block: POI_CANDIDATE_RECOVERY },
			RestrictedEntity::InvitedCandidate(_) =>
				Allowance { max: 0, recovery_per_block: POI_CANDIDATE_RECOVERY },
			RestrictedEntity::AccountParticipant(_) =>
				Allowance { max: 0, recovery_per_block: ACCOUNT_PARTICIPANT_RECOVERY },
			RestrictedEntity::LitePerson(_) => Allowance {
				max: LITE_PERSON_ALLOWANCE_MAX,
				recovery_per_block: LITE_PERSON_ALLOWANCE_RECOVERY,
			},
		}
	}
	fn restricted_entity(origin_caller: &OriginCaller) -> Option<Self> {
		use indiv_pallet_people::Origin::*;
		use indiv_pallet_people_lite::Origin::*;
		use indiv_pallet_proof_of_ink::Origin::*;
		use indiv_pallet_score::Origin::*;
		use OriginCaller::*;
		match origin_caller {
			People(PersonalIdentity(id)) => Some(RestrictedEntity::PersonalIdentity(*id)),
			People(PersonalAlias(rev_ca)) => Some(RestrictedEntity::PersonalAlias(rev_ca.ca.alias)),
			ProofOfInk(ReferredCandidate(account_id)) =>
				Some(RestrictedEntity::ReferredCandidate(account_id.clone())),
			Score(AccountParticipant(account_id)) =>
				Some(RestrictedEntity::AccountParticipant(account_id.clone())),
			PeopleLite(LitePerson(account_id)) =>
				Some(RestrictedEntity::LitePerson(account_id.clone())),
			_ => None,
		}
	}
}

pub struct OperationAllowedOneTimeExcess;
impl ContainsPair<RestrictedEntity, RuntimeCall> for OperationAllowedOneTimeExcess {
	fn contains(entity: &RestrictedEntity, call: &RuntimeCall) -> bool {
		use indiv_pallet_game::Call::*;
		use indiv_pallet_proof_of_ink::Call::*;
		use indiv_pallet_score::Call::*;
		match entity {
			RestrictedEntity::ReferredCandidate(_) => {
				matches!(
					call,
					RuntimeCall::ProofOfInk(submit_evidence { .. }) |
						RuntimeCall::ProofOfInk(commit { .. }) |
						RuntimeCall::ProofOfInk(allocate_full { .. }) |
						RuntimeCall::ProofOfInk(flakeout { .. }) |
						RuntimeCall::ProofOfInk(register_referred { .. })
				)
			},
			RestrictedEntity::InvitedCandidate(_) => {
				matches!(
					call,
					RuntimeCall::ProofOfInk(submit_evidence { .. }) |
						RuntimeCall::ProofOfInk(commit { .. }) |
						RuntimeCall::ProofOfInk(allocate_full { .. }) |
						RuntimeCall::ProofOfInk(flakeout { .. }) |
						RuntimeCall::ProofOfInk(register_non_referred { .. })
				)
			},
			RestrictedEntity::AccountParticipant(_) => {
				matches!(
					call,
					RuntimeCall::Score(cash_out { .. }) |
						RuntimeCall::Score(redeem_credit { .. }) |
						RuntimeCall::Score(register { .. }) |
						RuntimeCall::Game(sign_up_with_account { .. }) |
						RuntimeCall::Game(report { .. }) |
						RuntimeCall::Game(offboard { .. }) |
						RuntimeCall::Game(claim_airdrop { .. })
				)
			},
			RestrictedEntity::PersonalAlias(_) | RestrictedEntity::PersonalIdentity(_) => false,
			RestrictedEntity::LitePerson(_) => false,
		}
	}
}

#[cfg(feature = "runtime-benchmarks")]
pub struct OriginRestrictionBenchmarkHelper;
#[cfg(feature = "runtime-benchmarks")]
impl indiv_pallet_origin_restriction::BenchmarkHelper<OriginCaller, RuntimeCall>
	for OriginRestrictionBenchmarkHelper
{
	fn excess_pair() -> (OriginCaller, RuntimeCall) {
		(
			OriginCaller::Score(indiv_pallet_score::Origin::AccountParticipant(
				sp_runtime::AccountId32::new([0u8; 32]),
			)),
			RuntimeCall::Score(indiv_pallet_score::Call::cash_out {}),
		)
	}
}

impl indiv_pallet_origin_restriction::Config for Runtime {
	type WeightInfo = indiv_pallet_origin_restriction::weights::SubstrateWeight<Runtime>;
	// individuality v0.3.1 moved `Usages.at_block` from the local para clock to the relay
	// clock. The type is unchanged (`u32`), so this compiles either way and the live values
	// silently become far-future timestamps — see `migrations::RebaseOriginRestrictionUsages`,
	// which rebases them. Do not enact one without the other.
	type BlockNumberProvider = RelaychainDataProvider<Runtime>;
	type RestrictedEntity = RestrictedEntity;
	type OperationAllowedOneTimeExcess = OperationAllowedOneTimeExcess;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = OriginRestrictionBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct VerifySignatureBenchmarkHelper;
#[cfg(feature = "runtime-benchmarks")]
impl pallet_verify_signature::BenchmarkHelper<MultiSignature, AccountId>
	for VerifySignatureBenchmarkHelper
{
	fn create_signature(_entropy: &[u8], msg: &[u8]) -> (MultiSignature, AccountId) {
		use sp_io::crypto::{sr25519_generate, sr25519_sign};
		use sp_runtime::traits::IdentifyAccount;
		let public = sr25519_generate(0.into(), None);
		let who_account: AccountId = MultiSigner::Sr25519(public).into_account();
		let signature = MultiSignature::Sr25519(sr25519_sign(0.into(), &public, msg).unwrap());
		(signature, who_account)
	}
}

impl pallet_verify_signature::Config for Runtime {
	type Signature = MultiSignature;
	type AccountIdentifier = MultiSigner;
	type WeightInfo = ();
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = VerifySignatureBenchmarkHelper;
}

impl pallet_skip_feeless_payment::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
}
