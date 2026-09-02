// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Individuality.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use frame_support::{
	__private::sp_io,
	derive_impl, match_types,
	pallet_prelude::ConstU32,
	parameter_types,
	traits::{ConstU16, ConstU64, OriginTrait, ProcessMessageError, TryMapSuccess},
	PalletId,
};
use frame_system::{
	self,
	offchain::{
		CreateAuthorizedTransaction, CreateInherent, CreateTransaction, CreateTransactionBase,
	},
	EnsureRoot, EnsureSigned,
};
#[cfg(feature = "runtime-benchmarks")]
use indiv_pallet_people::BenchmarkHelper;
use indiv_pallet_people::{extension::AsPerson, Config};
#[cfg(feature = "runtime-benchmarks")]
use indiv_pallet_proof_of_ink::ReferralTicket;
#[cfg(feature = "runtime-benchmarks")]
use indiv_support::traits::PersonalId;
use indiv_support::traits::{Alias, Context, CountedMembers};
use sp_core::H256;
use sp_runtime::{
	morph_types,
	testing::{TestSignature, UintAuthorityId},
	traits::{AccountIdConversion, BlakeTwo256, IdentityLookup},
	MultiSignature, Percent,
};
use sp_statement_store::StatementAllowance;
use std::time::Duration;
use verifiable::ring::bandersnatch::BandersnatchVrfVerifiable;
use xcm::prelude::*;
use xcm_executor::{
	traits::{FeeManager, FeeReason, Properties, ShouldExecute, WeightBounds, WithOriginFilter},
	AssetsInHolding, Config as XcmConfig, XcmExecutor,
};

// Simple XCM origin converter for testing
pub struct SimpleXcmOriginConverter;
impl frame_support::traits::EnsureOrigin<RuntimeOrigin> for SimpleXcmOriginConverter {
	type Success = Location;

	fn try_origin(o: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
		match o.clone().into() {
			Ok(frame_system::RawOrigin::Root) => Ok(Location::here()),
			_ => Err(o),
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
		Ok(RuntimeOrigin::root())
	}
}
use xcm::v5::Location;

pub type TransactionExtension = (AsPerson<Test>, frame_system::CheckNonce<Test>);

pub type Header = sp_runtime::generic::Header<u64, BlakeTwo256>;
pub type Block = sp_runtime::generic::Block<Header, UncheckedExtrinsic>;
pub type UncheckedExtrinsic = sp_runtime::generic::UncheckedExtrinsic<
	AccountId32,
	RuntimeCall,
	MultiSignature,
	TransactionExtension,
>;

frame_support::construct_runtime!(
	pub enum Test
	{
		System: frame_system,
		PeoplePallet: indiv_pallet_people,
		PeopleLite: indiv_pallet_people_lite,
		ProofOfInk: indiv_pallet_proof_of_ink,
		Balances: pallet_balances,
		Assets: pallet_assets,
		AssetsHolder: pallet_assets_holder,
		Score: indiv_pallet_score,
		Game: indiv_pallet_game,
		MobRule: indiv_pallet_mob_rule,
		PalletXcm: pallet_xcm,
		ChunksManager: indiv_pallet_chunks_manager,
		MembersPallet: indiv_pallet_members,
		Airdrop: indiv_pallet_airdrop,
		IndividualityInitiator: crate,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type BaseCallFilter = frame_support::traits::Everything;
	type BlockWeights = ();
	type BlockLength = ();
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	type RuntimeTask = RuntimeTask;
	type Nonce = u64;
	type Hash = H256;
	type Hashing = BlakeTwo256;
	type AccountId = AccountId32;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Block = Block;
	type BlockHashCount = ConstU64<250>;
	type DbWeight =
		<frame_system::config_preludes::TestDefaultConfig as frame_system::DefaultConfig>::DbWeight;
	type Version = ();
	type AccountData = pallet_balances::AccountData<u32>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ConstU16<42>;
	type OnSetCode = ();
	type MaxConsumers = ConstU32<16>;
}

pub const MOCK_CONTEXT: Context = *b"pop:polkadot.network/mock       ";
match_types! {
	pub type TestAccountContexts: impl Contains<Context> = {
		&MOCK_CONTEXT
	};
}

impl<LocalCall> CreateInherent<LocalCall> for Test
where
	RuntimeCall: From<LocalCall>,
{
	fn create_bare(call: Self::RuntimeCall) -> Self::Extrinsic {
		UncheckedExtrinsic::new_bare(call)
	}
}

impl<LocalCall> CreateTransactionBase<LocalCall> for Test
where
	RuntimeCall: From<LocalCall>,
{
	type Extrinsic = UncheckedExtrinsic;
	type RuntimeCall = RuntimeCall;
}

impl<LocalCall> CreateTransaction<LocalCall> for Test
where
	RuntimeCall: From<LocalCall>,
{
	type Extension = TransactionExtension;
	fn create_transaction(
		call: <Self as CreateTransactionBase<LocalCall>>::RuntimeCall,
		extension: Self::Extension,
	) -> Self::Extrinsic {
		UncheckedExtrinsic::new_transaction(call, extension)
	}
}

impl<LocalCall> CreateAuthorizedTransaction<LocalCall> for Test
where
	RuntimeCall: From<LocalCall>,
{
	fn create_extension() -> Self::Extension {
		(AsPerson::new(None), frame_system::CheckNonce::from(0))
	}
}

parameter_types! {
	pub const PeopleCollectionOwner: MockMembersLocation = MockMembersLocation(1);
}

impl Config for Test {
	type WeightInfo = ();
	type MemberService = MembersPallet;
	type CollectionOwner = PeopleCollectionOwner;
	type AccountContexts = TestAccountContexts;
	type OnboardingQueuePageSize = ConstU32<40>;
	type RingExponent = FlexibleRingExp;
	type StaleAliasCleanupInterval = ConstU64<5>;
	type SelfInclusionDelay = ConstU64<600>;
	type ManagerOrigin = EnsureRoot<Self::AccountId>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = BenchHelper;
}

parameter_types! {
	// Mirrors `indiv-pallet-people-lite`'s own v0.3.1 mock (`pallets/people-lite/src/mock.rs`),
	// which is the closest thing to an authoritative binding for these items: this pallet is a
	// permanent Paseo fork and has no upstream mock to copy from.
	//
	// The fee is typed `u32` because this mock's `pallet_balances::Config::Balance` is `u32`
	// (people-lite's own mock uses `u64`). Nothing in `tests.rs` registers a lite person by
	// paying the fee, so neither the pot nor the amount is load-bearing here.
	pub const LitePeoplePotId: PalletId = PalletId(*b"plitefee");
	pub storage LitePersonRegistrationFee: u32 = 10;
	// The value is upstream's mock value, NOT the runtime's. The Paseo runtimes must bind
	// `Suffix` to the `dot` suffix carried by `indiv-pallet-network-suffix`; see that pallet's
	// `migration` module for why it is `dot` and never `.dot`. No test here asserts on a
	// suffix-derived context.
	pub LiteNetworkSuffix: indiv_support::context::ProductContextNetworkSuffix =
		b"paseo".to_vec().try_into().expect("network suffix fits");
}

impl indiv_pallet_people_lite::Config for Test {
	type WeightInfo = ();
	type Currency = Balances;
	type PotId = LitePeoplePotId;
	type RegistrationFee = LitePersonRegistrationFee;
	type Suffix = LiteNetworkSuffix;
	type AccountContexts = TestAccountContexts;
	type AttestationAllowanceManager = EnsureRoot<Self::AccountId>;
	type MemberService = MembersPallet;
	type CollectionOwner = PeopleCollectionOwner;
	type LiteRingExponent = FlexibleRingExp;
	type LiteOnboardingSize = ConstU32<10>;
	type AttestationSignature = MultiSignature;
	type LiteConsumerRegistrar = ();
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
}

#[cfg(feature = "runtime-benchmarks")]
pub struct BenchHelper {}

#[cfg(feature = "runtime-benchmarks")]
impl<Chunk> BenchmarkHelper<Chunk> for BenchHelper
where
	Chunk: From<<BandersnatchVrfVerifiable as verifiable::GenerateVerifiable>::StaticChunk>,
{
	fn valid_account_context() -> Context {
		MOCK_CONTEXT
	}
	fn initialize_chunks() -> Vec<Chunk> {
		vec![]
	}
}

impl indiv_pallet_chunks_manager::Config for Test {
	type WeightInfo = ();
	type Chunk = <BandersnatchVrfVerifiable as verifiable::GenerateVerifiable>::StaticChunk;
	type PageSize = ConstU32<1024>;
	type ManagerOrigin = EnsureRoot<Self::AccountId>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ChunksManagerBenchHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct ChunksManagerBenchHelper;

#[cfg(feature = "runtime-benchmarks")]
impl
	indiv_pallet_chunks_manager::BenchmarkHelper<
		<BandersnatchVrfVerifiable as verifiable::GenerateVerifiable>::StaticChunk,
	> for ChunksManagerBenchHelper
{
	fn chunk_page(
	) -> Vec<<BandersnatchVrfVerifiable as verifiable::GenerateVerifiable>::StaticChunk> {
		use indiv_support::genesis::ring_verifier_builder_params;
		use verifiable::ring::RingDomainSize;
		let chunks = ring_verifier_builder_params(RingDomainSize::Domain16);
		chunks.into_iter().take(1024).collect()
	}
}

/// Mock location type for the members pallet.
#[derive(
	Clone,
	PartialEq,
	Eq,
	codec::Encode,
	codec::Decode,
	codec::MaxEncodedLen,
	scale_info::TypeInfo,
	Default,
	codec::DecodeWithMemTracking,
	Debug,
)]
pub struct MockMembersLocation(pub u32);

parameter_types! {
	pub const FlexibleRingExp: indiv_support::traits::RingExponent = indiv_support::traits::RingExponent::R2e9;
}

impl indiv_pallet_members::Config for Test {
	type WeightInfo = ();
	type Crypto = BandersnatchVrfVerifiable;
	type Location = MockMembersLocation;
	type ChunksManager = ChunksManager;
	type Clock = Test;
	type MaxCollections = ConstU32<10>;
	type OnboardingQueuePageSize = ConstU32<40>;
	type MaxFlexibleRingExponent = FlexibleRingExp;
	type RingBuildingMemberLimit = ConstU32<100>;
	type OldRootRetentionDuration = ConstU64<600>;
	type OnRingRootChange = ();
	type OffchainWorkerInterval = ConstU64<1>;
	type ManagerOrigin = EnsureRoot<Self::AccountId>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = MembersBenchHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct MembersBenchHelper;

#[cfg(feature = "runtime-benchmarks")]
impl<Chunk> indiv_pallet_members::BenchmarkHelper<Chunk> for MembersBenchHelper
where
	Chunk: From<<BandersnatchVrfVerifiable as verifiable::GenerateVerifiable>::StaticChunk>,
{
	fn initialize_chunks(_ring_size: indiv_support::traits::RingExponent) -> Vec<Chunk> {
		vec![]
	}
	fn set_time(_now: Duration) {
		unimplemented!();
	}
	fn set_valid_time() {
		unimplemented!();
	}
}

morph_types! {
	pub type AlwaysFineAccountId32: TryMorph = |_r: AccountId32| -> Result<u64, ()> {
		Ok(1)
	};
}

parameter_types! {
	pub const PoiPotId: PalletId = PalletId(*b"PoiPotId");
	pub const MobRulePotId: PalletId = PalletId(*b"MobRwrds");
	pub const MinTurnoutPercentage: Percent = Percent::from_percent(10);
	pub const BalancesLocation: Location = Location::here();
	pub const ExistentialDeposit: u32 = 1;
	pub const FundingAccount: AccountId32 = AccountId32::new([1u8; 32]);
   pub const InviteRecipient: AccountId32 = AccountId32::new([2u8; 32]);
	pub storage PeopleVoteWeight: u8 = 1;
	pub storage CandidateVoteWeight: u8 = 1;
	pub PlayerStatementLimit: StatementAllowance = StatementAllowance {
		max_size: 1000,
		max_count: 1000,
	};
	pub const ScorePotId: PalletId = PalletId(*b"scorepot");
	pub const TestAssetHubParaId: u32 = 1000u32;
	pub const TestAssetHubTransferAmount: u32 = 5_000_000u32;
	pub const TestXcmTimeout: u64 = 100u64;
	pub const TestAssetHubAssetId: u32 = 1337u32;
	pub TestTransferAssetForeignId: Location = Location::new(
		1,
		[
			Parachain(1000),
			PalletInstance(50),
			GeneralIndex(1337),
		],
	);
	pub const TestXcmExecutionFee: u128 = 1_000_000_000u128;
}

pub struct MockRandomness;
impl frame_support::traits::Randomness<[u8; 32], u64> for MockRandomness {
	fn random(_subject: &[u8]) -> ([u8; 32], u64) {
		([0u8; 32], 0)
	}
}

impl frame_support::traits::UnixTime for Test {
	fn now() -> Duration {
		Duration::from_secs(1000)
	}
}

pub struct GamePhaseDurations;
impl frame_support::traits::Get<indiv_pallet_game::PhaseDurationValues> for GamePhaseDurations {
	fn get() -> indiv_pallet_game::PhaseDurationValues {
		indiv_pallet_game::PhaseDurationValues {
			registration: 60,
			shuffle: 30,
			post_shuffle_margin: 30,
			reporting: 30,
			player_process: 60,
			airdrop_claim_window: 7 * 24 * 60 * 60,
		}
	}
}

impl pallet_balances::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeHoldReason = RuntimeHoldReason;
	type RuntimeFreezeReason = RuntimeFreezeReason;
	type WeightInfo = ();
	type Balance = u32;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type ReserveIdentifier = [u8; 8];
	type FreezeIdentifier = ();
	type MaxLocks = ConstU32<50>;
	type MaxReserves = ConstU32<50>;
	type MaxFreezes = ConstU32<50>;
	type DoneSlashHandler = ();
}

#[derive_impl(pallet_assets::config_preludes::TestDefaultConfig)]
impl pallet_assets::Config for Test {
	type Balance = u128;
	type AssetId = u32;
	type AssetIdParameter = u32;
	type Currency = Balances;
	type CreateOrigin =
		frame_support::traits::AsEnsureOriginWithArg<frame_system::EnsureSigned<AccountId32>>;
	type ForceOrigin = EnsureRoot<AccountId32>;
	type Holder = AssetsHolder;
}

impl pallet_assets_holder::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeHoldReason = RuntimeHoldReason;
}

parameter_types! {
	pub const AirdropPalletId: PalletId = PalletId(*b"pop/adrp");
	pub GameAirdropSource: AccountId32 =
		PalletId(*b"pop/gads").into_account_truncating();
}

pub type AssetsWithHolder = indiv_support::fungibles::CombineAssetsWithHolder<Assets, AssetsHolder>;

/// Type-level stub for `indiv_pallet_airdrop::Config::Randomness`.
///
/// `indiv-support` v0.3.1 replaces `CurrentBlockRandomness` (`Option<[u8; 32]>`) with
/// `MomentRandomness<Moment>`, which pairs each value with the moment it became publicly
/// determinable. This mock keeps the previous stub's character — a constant value that no test
/// ever reads — and pairs it with a constant moment.
///
/// `Airdrop` is in this mock runtime only because `indiv_pallet_game::Config` requires it; this
/// pallet's test suite (`tests.rs`) never opens or draws an airdrop event. A test that did draw
/// would need a moment that advances past the registration-close commitment, as
/// `indiv-pallet-game`'s own mock does with its `advance_airdrop_randomness` helper.
pub struct MockAirdropRandomness;
impl indiv_support::traits::MomentRandomness<u32> for MockAirdropRandomness {
	fn randomness() -> Option<([u8; 32], u32)> {
		Some(([0u8; 32], 0))
	}

	fn current_moment() -> u32 {
		0
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn set_randomness(_randomness: [u8; 32], _moment: u32) {}

	#[cfg(feature = "runtime-benchmarks")]
	fn set_current_moment(_moment: u32) {}
}

pub struct AccountToPub;
impl sp_runtime::traits::TryConvert<AccountId32, sp_core::sr25519::Public> for AccountToPub {
	fn try_convert(_: AccountId32) -> Result<sp_core::sr25519::Public, AccountId32> {
		Ok(sp_core::sr25519::Public::from_raw([0u8; 32]))
	}
}

impl indiv_pallet_airdrop::Config for Test {
	type WeightInfo = ();
	type MemberService = MembersPallet;
	type Fungibles = AssetsWithHolder;
	type ManagerOrigin = EnsureRoot<AccountId32>;
	type PalletId = AirdropPalletId;
	type UnixTime = Test;
	type Randomness = MockAirdropRandomness;
	type AccountIdToPublic = AccountToPub;
	type ClearLimit = ConstU32<100>;
	type DrawLimit = ConstU32<100>;
	type OffchainWorkerInterval = ConstU64<1>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
}

impl indiv_pallet_proof_of_ink::Config for Test {
	type WeightInfo = ();
	type Deposit = ();
	type People = PeoplePallet;
	type EnsurePerson = TryMapSuccess<EnsureSigned<Self::AccountId>, AlwaysFineAccountId32>;
	type TicketSignature = sp_runtime::testing::TestSignature;
	type TicketPublic = UintAuthorityId;
	type Ticket = u64;
	type Oracle = ();
	type Randomness = MockRandomness;
	type DataStore = ();
	type MaxActiveReferrals = ConstU32<10>;
	type MaxRetryAttempts = ConstU32<1>;
	type MaxReimbursementValues = ConstU32<10>;
	type Currency = Balances;
	type PotId = PoiPotId;
	type InvitationsOrigin = EnsureRoot<Self::AccountId>;
	type ManagerOrigin = EnsureRoot<Self::AccountId>;
	type Crypto = BandersnatchVrfVerifiable;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = Test;
}

#[cfg(feature = "runtime-benchmarks")]
impl indiv_pallet_proof_of_ink::BenchmarkHelper<Test> for Test {
	fn create_tickets(seed: u64) -> sp_runtime::BoundedVec<ReferralTicket<u64>, ConstU32<10>> {
		sp_runtime::BoundedVec::try_from(vec![ReferralTicket { ticket: seed }]).unwrap()
	}

	fn create_ticket(seed: u64) -> (UintAuthorityId, u64) {
		(seed.into(), seed)
	}

	fn sign(seed: u64, msg: &[u8]) -> TestSignature {
		TestSignature(seed, msg.to_vec())
	}

	fn build_person_origin(_personal_id: PersonalId) -> RuntimeOrigin {
		unimplemented!();
	}

	fn setup_currency() {}
}

pub struct MockPerson;
impl frame_support::traits::EnsureOriginWithArg<RuntimeOrigin, Context> for MockPerson {
	type Success = Alias;

	fn try_origin(
		origin: RuntimeOrigin,
		_context: &Context,
	) -> Result<Self::Success, RuntimeOrigin> {
		match origin.caller() {
			OriginCaller::PeoplePallet(indiv_pallet_people::Origin::PersonalAlias(
				contextual_alias,
			)) => Ok(contextual_alias.ca.alias),
			_ => Err(origin),
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn try_successful_origin(_context: &Context) -> Result<RuntimeOrigin, ()> {
		use indiv_support::traits::{ContextualAlias, RevisedContextualAlias};
		Ok(OriginCaller::PeoplePallet(indiv_pallet_people::Origin::PersonalAlias(
			RevisedContextualAlias {
				revision: 1,
				ring: 1,
				ca: ContextualAlias {
					alias: [1; 32],
					context: *b"test                            ",
				},
			},
		))
		.into())
	}
}

impl CountedMembers for MockPerson {
	fn active_count() -> u32 {
		0
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn set_active_count(_count: u32) {}
}

#[derive(
	Clone,
	Debug,
	PartialEq,
	Eq,
	codec::Encode,
	codec::Decode,
	codec::MaxEncodedLen,
	scale_info::TypeInfo,
	Default,
)]
pub struct MockDeposit;

impl frame_support::traits::Consideration<AccountId32, u32> for MockDeposit {
	fn new(_who: &AccountId32, _amount: u32) -> Result<Self, sp_runtime::DispatchError> {
		Ok(MockDeposit)
	}

	fn update(self, _who: &AccountId32, _amount: u32) -> Result<Self, sp_runtime::DispatchError> {
		Ok(self)
	}

	fn drop(self, _who: &AccountId32) -> Result<(), sp_runtime::DispatchError> {
		Ok(())
	}

	fn burn(self, _who: &AccountId32) {
		unimplemented!();
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn ensure_successful(_who: &AccountId32, _amount: u32) {}
}

#[cfg(feature = "runtime-benchmarks")]
pub struct ScoreBenchHelper;

#[cfg(feature = "runtime-benchmarks")]
impl indiv_pallet_score::benchmarking::BenchmarkHelper<Test> for ScoreBenchHelper {
	fn create_member(_seed: u64) -> indiv_pallet_score::MemberOf<Test> {
		unimplemented!();
	}
	fn setup_currency() {}
}

impl indiv_pallet_score::Config for Test {
	type WeightInfo = ();
	type EnsurePerson = MockPerson;
	type People = PeoplePallet;
	type ScorePotId = ScorePotId;
	type Currency = Balances;
	type CurrencyLocationInfo = BalancesLocation;
	type ManagerOrigin = EnsureRoot<Self::AccountId>;
	type MaxPayoutRoundSchedules = ConstU32<10>;
	type OffchainWorkInterval = ConstU64<1>;
	type Crypto = BandersnatchVrfVerifiable;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ScoreBenchHelper;
}

parameter_types! {
	pub const GameDefaultPlayDeposit: u32 = 1;
}

impl indiv_pallet_game::Config for Test {
	type WeightInfo = ();
	type UnixTime = Test;
	type MaxRounds = ConstU32<10>;
	type MaxGroupSize = ConstU32<20>;
	type MinGroupSize = ConstU32<2>;
	type ManagerOrigin = EnsureRoot<Self::AccountId>;
	type InviteIssuer = EnsureRoot<Self::AccountId>;
	type NonPlayingKickoutTime = ConstU64<1000>;
	type NativeFungible = Balances;
	type PlayDeposit = MockDeposit;
	type DefaultPlayDeposit = GameDefaultPlayDeposit;
	type DefaultPhaseDurations = GamePhaseDurations;
	type MaxGameSchedules = ConstU32<100>;
	type MaxAttendanceHistoryDepth = ConstU32<12>;
	type TicketSignature = TestSignature;
	type PlayerStatementLimit = PlayerStatementLimit;
	type AccountSignature = MultiSignature;
	type PeopleVoteWeight = PeopleVoteWeight;
	type CandidateVoteWeight = CandidateVoteWeight;
	type AirdropAssetId = u32;
	type AirdropAssetBalance = u128;
	type Airdrop = Airdrop;
	type AirdropSource = GameAirdropSource;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = GameBenchHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct GameBenchHelper {}

#[cfg(feature = "runtime-benchmarks")]
use frame_benchmarking::account;
use frame_support::traits::UnixTime;
#[cfg(feature = "runtime-benchmarks")]
use indiv_pallet_mob_rule::benchmarking;
use sp_core::crypto::AccountId32;

#[cfg(feature = "runtime-benchmarks")]
impl indiv_pallet_game::BenchmarkHelper<MultiSignature, TestSignature, u64, AccountId32, u32>
	for GameBenchHelper
{
	fn create_account(seed: u64) -> AccountId32 {
		account("acc", 0, seed as u32)
	}

	fn sign_account(_seed: u64, _msg: &[u8]) -> MultiSignature {
		MultiSignature::Ecdsa(sp_core::ecdsa::Signature::from_raw([0u8; 65]))
	}

	fn create_ticket(seed: u64) -> u64 {
		seed
	}

	fn sign_ticket(seed: u64, msg: &[u8]) -> TestSignature {
		TestSignature(seed, msg.to_vec())
	}

	fn set_valid_time() {
		unimplemented!();
	}

	fn set_time(_now: Duration) {
		unimplemented!();
	}

	fn fund_account(_acc: AccountId32) {
		unimplemented!();
	}

	fn airdrop_asset_id() -> u32 {
		0
	}
}

parameter_types! {
	pub static Now: core::time::Duration = core::time::Duration::from_millis(0);
}

pub struct TestClock;
impl UnixTime for TestClock {
	fn now() -> core::time::Duration {
		Now::get()
	}
}

impl indiv_pallet_mob_rule::Config for Test {
	type WeightInfo = ();
	type Currency = Balances;
	type CurrencyLocationInfo = BalancesLocation;
	type Clock = TestClock;
	type EnsurePerson = MockPerson;
	type MaxVoteClaimDuration = ConstU64<7200>;
	type MinCaseDuration = ConstU32<{ 24 * 60 * 60 }>;
	type MaxVotingDuration = ConstU32<{ 14 * 24 * 60 * 60 }>;
	type MinTurnoutNominal = ConstU32<1>;
	type MinTurnoutPercentage = MinTurnoutPercentage;
	type MaxPayoutRoundSchedules = ConstU32<5>;
	type VotingPenaltyDuration = ConstU64<10>;
	type InterventionOrigin = EnsureRoot<Self::AccountId>;
	type PotId = MobRulePotId;
	type MaxVotesClaimable = ConstU32<10>;
	type OffchainWorkInterval = ConstU64<5>;
	type CleanVotesBatchSize = ConstU32<6>;
	type VotesOpenForClaimsDuration = ConstU32<{ 60 * 60 }>;
	type MinimumVoterThreshold = ConstU32<1>;

	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = MobRuleBenchHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct MobRuleBenchHelper;
#[cfg(feature = "runtime-benchmarks")]
impl benchmarking::BenchmarkHelper<Test> for MobRuleBenchHelper {
	fn set_valid_time() {
		// A reasonable time away from genesis block for benchmarks
		Now::set(Duration::from_secs(14 * 24 * 60 * 60 + 3600));
	}

	fn setup_currency() {}
}

// Simple mock for XCM sending - just succeeds without doing anything
pub struct MockXcmSender;

impl SendXcm for MockXcmSender {
	type Ticket = ();

	fn validate(
		_destination: &mut Option<Location>,
		_message: &mut Option<Xcm<()>>,
	) -> SendResult<Self::Ticket> {
		Ok(((), xcm::prelude::Assets::new()))
	}

	fn deliver(_ticket: Self::Ticket) -> Result<XcmHash, SendError> {
		Ok([0; 32])
	}
}

// Simple mock for assets that only implements what the pallet actually uses
use frame_support::traits::fungibles::{Create, Dust, Inspect, Mutate, Unbalanced};

parameter_types! {
	pub static MockAssetBalances: std::collections::BTreeMap<(Location, AccountId32), u32> = std::collections::BTreeMap::new();
}

pub struct MockAssets;

impl Inspect<AccountId32> for MockAssets {
	type AssetId = Location;
	type Balance = u32;

	fn total_issuance(_asset: Self::AssetId) -> Self::Balance {
		0
	}
	fn minimum_balance(_asset: Self::AssetId) -> Self::Balance {
		0
	}
	fn total_balance(_asset: Self::AssetId, _who: &AccountId32) -> Self::Balance {
		MockAssetBalances::get().get(&(_asset, _who.clone())).copied().unwrap_or(0)
	}
	fn balance(_asset: Self::AssetId, _who: &AccountId32) -> Self::Balance {
		MockAssetBalances::get().get(&(_asset, _who.clone())).copied().unwrap_or(0)
	}
	fn reducible_balance(
		_asset: Self::AssetId,
		_who: &AccountId32,
		_preservation: frame_support::traits::tokens::Preservation,
		_force: frame_support::traits::tokens::Fortitude,
	) -> Self::Balance {
		MockAssetBalances::get().get(&(_asset, _who.clone())).copied().unwrap_or(0)
	}
	fn can_deposit(
		_asset: Self::AssetId,
		_who: &AccountId32,
		_amount: Self::Balance,
		_provenance: frame_support::traits::tokens::Provenance,
	) -> frame_support::traits::tokens::DepositConsequence {
		frame_support::traits::tokens::DepositConsequence::Success
	}
	fn can_withdraw(
		_asset: Self::AssetId,
		_who: &AccountId32,
		_amount: Self::Balance,
	) -> frame_support::traits::tokens::WithdrawConsequence<Self::Balance> {
		frame_support::traits::tokens::WithdrawConsequence::Success
	}
	fn asset_exists(_asset: Self::AssetId) -> bool {
		true
	}
}

impl Unbalanced<AccountId32> for MockAssets {
	fn handle_dust(_dust: Dust<AccountId32, Self>) {}
	fn write_balance(
		_asset: Self::AssetId,
		_who: &AccountId32,
		amount: Self::Balance,
	) -> Result<Option<Self::Balance>, sp_runtime::DispatchError> {
		Ok(Some(amount))
	}
	fn set_total_issuance(_asset: Self::AssetId, _amount: Self::Balance) {}
}

impl Mutate<AccountId32> for MockAssets {
	fn mint_into(
		asset: Self::AssetId,
		who: &AccountId32,
		amount: Self::Balance,
	) -> Result<Self::Balance, sp_runtime::DispatchError> {
		MockAssetBalances::mutate(|balances| {
			*balances.entry((asset, who.clone())).or_insert(0) += amount;
		});
		Ok(amount)
	}
	fn burn_from(
		_asset: Self::AssetId,
		_who: &AccountId32,
		amount: Self::Balance,
		_preservation: frame_support::traits::tokens::Preservation,
		_precision: frame_support::traits::tokens::Precision,
		_force: frame_support::traits::tokens::Fortitude,
	) -> Result<Self::Balance, sp_runtime::DispatchError> {
		Ok(amount)
	}
	fn transfer(
		_asset: Self::AssetId,
		_source: &AccountId32,
		_dest: &AccountId32,
		amount: Self::Balance,
		_preservation: frame_support::traits::tokens::Preservation,
	) -> Result<Self::Balance, sp_runtime::DispatchError> {
		Ok(amount) // Always succeeds for testing
	}
}

impl Create<AccountId32> for MockAssets {
	fn create(
		_id: Self::AssetId,
		_admin: AccountId32,
		_is_sufficient: bool,
		_min_balance: Self::Balance,
	) -> sp_runtime::DispatchResult {
		Ok(())
	}
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MockReserveData {
	pub reserve: Location,
	pub teleportable: bool,
}

impl From<(Location, bool)> for MockReserveData {
	fn from((reserve, teleportable): (Location, bool)) -> Self {
		Self { reserve, teleportable }
	}
}

pub struct MockReserveSetter;
impl crate::ReserveSetter<AccountId32, Location, MockReserveData> for MockReserveSetter {
	fn set_reserves(
		_owner: &AccountId32,
		_asset_id: Location,
		_reserves: sp_runtime::BoundedVec<
			MockReserveData,
			ConstU32<{ pallet_assets::MAX_RESERVES }>,
		>,
	) -> sp_runtime::DispatchResult {
		Ok(())
	}
}

parameter_types! {
	pub const TestPeopleChainAccount: AccountId32 = AccountId32::new([3u8; 32]);
	pub const TestPeopleChainParaId: u32 = 1004u32;
}

// Minimal XCM configuration for testing
parameter_types! {
	pub const BaseXcmWeight: Weight = Weight::from_parts(1_000_000, 1_000_000);
	pub UniversalLocation: InteriorLocation = Here;
}

// Test weigher implementation based on polkadot-sdk xcm-executor mock
pub struct TestWeigher;
impl<C> WeightBounds<C> for TestWeigher {
	fn weight(_message: &mut Xcm<C>, _weight_limit: Weight) -> Result<Weight, InstructionError> {
		Ok(BaseXcmWeight::get())
	}

	fn instr_weight(_instruction: &mut Instruction<C>) -> Result<Weight, XcmError> {
		Ok(BaseXcmWeight::get())
	}
}

// Always allow XCM execution for testing
pub struct TestBarrier;
impl ShouldExecute for TestBarrier {
	fn should_execute<RuntimeCall>(
		_origin: &Location,
		_instructions: &mut [Instruction<RuntimeCall>],
		_max_weight: Weight,
		_properties: &mut Properties,
	) -> Result<(), ProcessMessageError> {
		Ok(())
	}
}

// Always waive fees for testing
pub struct AlwaysWaiveFeeManager;
impl FeeManager for AlwaysWaiveFeeManager {
	fn is_waived(_origin: Option<&Location>, _reason: FeeReason) -> bool {
		true
	}

	fn handle_fee(_fee: AssetsInHolding, _context: Option<&XcmContext>, _reason: FeeReason) {}
}

// Simplified mock XCM router that always succeeds for testing
pub struct TestXcmRouter;
impl xcm::prelude::SendXcm for TestXcmRouter {
	type Ticket = ();

	fn validate(
		_destination: &mut Option<Location>,
		_message: &mut Option<Xcm<()>>,
	) -> xcm::prelude::SendResult<Self::Ticket> {
		Ok(((), xcm::prelude::Assets::new()))
	}

	fn deliver(_ticket: Self::Ticket) -> Result<XcmHash, xcm::prelude::SendError> {
		Ok([0u8; 32].into())
	}
}

pub struct TestXcmExecutorConfig;
impl XcmConfig for TestXcmExecutorConfig {
	type RuntimeCall = RuntimeCall;
	type XcmSender = TestXcmRouter;
	type AssetTransactor = ();
	type OriginConverter = ();
	type IsReserve = ();
	type IsTeleporter = ();
	type UniversalLocation = UniversalLocation;
	type Barrier = TestBarrier;
	type Weigher = TestWeigher;
	type Trader = ();
	type ResponseHandler = ();
	type AssetTrap = ();
	type SubscriptionService = ();
	type PalletInstancesInfo = ();
	type MaxAssetsIntoHolding = frame_support::traits::ConstU32<64>;
	type AssetLocker = ();
	type AssetExchanger = ();
	type FeeManager = AlwaysWaiveFeeManager;
	type MessageExporter = ();
	type UniversalAliases = ();
	type CallDispatcher = WithOriginFilter<frame_support::traits::Everything>;
	type SafeCallFilter = frame_support::traits::Everything;
	type Aliasers = ();
	type TransactionalProcessor = ();
	type HrmpNewChannelOpenRequestHandler = ();
	type HrmpChannelAcceptedHandler = ();
	type HrmpChannelClosingHandler = ();
	type XcmRecorder = ();
	type XcmEventEmitter = ();
}

impl pallet_xcm::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type SendXcmOrigin = SimpleXcmOriginConverter;
	type XcmRouter = TestXcmRouter;
	type ExecuteXcmOrigin = frame_system::EnsureNever<Location>;
	type XcmExecuteFilter = frame_support::traits::Nothing;
	type XcmExecutor = XcmExecutor<TestXcmExecutorConfig>;
	type XcmTeleportFilter = frame_support::traits::Nothing;
	type XcmReserveTransferFilter = frame_support::traits::Nothing;
	type Weigher = TestWeigher;
	type UniversalLocation = UniversalLocation;
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	type AdvertisedXcmVersion = pallet_xcm::CurrentXcmVersion;
	type Currency = Balances;
	type CurrencyMatcher = ();
	type TrustedLockers = ();
	type SovereignAccountOf = ();
	type MaxLockers = frame_support::traits::ConstU32<8>;
	type WeightInfo = pallet_xcm::TestWeightInfo;
	type AdminOrigin = frame_system::EnsureRoot<AccountId32>;
	type MaxRemoteLockConsumers = frame_support::traits::ConstU32<0>;
	type RemoteLockConsumerIdentifier = ();
	type AuthorizedAliasConsideration = ();
	const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;
}

impl crate::Config for Test {
	type WeightInfo = ();
	type Assets = MockAssets;
	type ReserveData = MockReserveData;
	type ReserveSetter = MockReserveSetter;
	type InvitesRecipient = InviteRecipient;
	type AssetsDestAccount = TestPeopleChainAccount;
	type AssetHubTransferAmount = TestAssetHubTransferAmount;
	type XcmTimeout = TestXcmTimeout;
	type XcmSender = MockXcmSender;
	type ParachainInfo = TestPeopleChainParaId;
	type TransferAssetForeignId = TestTransferAssetForeignId;
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	use sp_runtime::BuildStorage;

	RuntimeGenesisConfig {
		system: Default::default(),
		chunks_manager: Default::default(),
		balances: pallet_balances::GenesisConfig::<Test> {
			balances: vec![(AccountId32::new([1u8; 32]), 1_000_000_000u32)],
			dev_accounts: Default::default(),
		},
		assets: Default::default(),
		pallet_xcm: Default::default(),
		// v0.3.1 gives `people-multi` and `people-lite` a genesis config whose only field is
		// `create_collection`, defaulting to `false`. `false` is deliberate here, not merely
		// convenient: this pallet's whole job is to create those collections during the
		// migration, and its tests assert on that path. Creating them at genesis instead would
		// make the tests assert against an already-initialised chain.
		//
		// Listed explicitly rather than via `..Default::default()`, matching the rest of this
		// initializer: this pallet is a permanent Paseo fork with no upstream, so a compile
		// error on the next pallet that gains genesis state is the notification we want.
		people_pallet: Default::default(),
		people_lite: Default::default(),
	}
	.build_storage()
	.unwrap()
	.into()
}
