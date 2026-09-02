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

use crate::*;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use frame_support::{
	assert_ok, derive_impl,
	dispatch::{DispatchErrorWithPostInfo, GetDispatchInfo},
	pallet_prelude::{Get, ValidTransaction},
	parameter_types,
	storage::with_transaction,
	traits::{AsEnsureOriginWithArg, Everything, OnIdle, OnPoll, OriginTrait},
	PalletId,
};
use frame_system::{
	offchain::{CreateAuthorizedTransaction, CreateBare, CreateTransaction, CreateTransactionBase},
	EnsureRoot, RunToBlockHooks,
};
use indiv_pallet_people::Origin::PersonalAlias;
use indiv_pallet_score::{AccountOrPerson, SCORE_CONTEXT};
use indiv_support::traits::{ContextualAlias, RevisedContextualAlias, RingExponent};
use scale_info::TypeInfo;
use sp_core::{ConstU32, ConstU64, ConstUint, H256};
use sp_runtime::{
	testing::TestSignature,
	traits::{
		Applyable, BlakeTwo256, Checkable, DispatchInfoOf, IdentityLookup,
		TransactionExtension as TransactionExtensionTrait, ValidateResult,
	},
	transaction_validity::{InvalidTransaction, TransactionSource, TransactionValidityError},
	AccountId32, BuildStorage, DispatchError, Permill, TransactionOutcome, Weight,
};
use sp_statement_store::{runtime_api::StatementSource, Statement, Topic};
use std::{
	cell::RefCell,
	sync::{Arc, LazyLock, Mutex},
	time::Duration,
};
use verifiable::{mock::Mock, Alias, GenerateVerifiable};
use xcm::v5::Location;

pub(crate) const DEFAULT_IDENTIFIER_KEY: CommunicationIdentifier = [42u8; 65];

/// Convert a `u64` to an `AccountId32`.
pub fn id_to_account(id: u64) -> AccountId32 {
	let mut bytes = [0; 32];
	bytes[..8].copy_from_slice(&id.to_le_bytes());
	AccountId32::new(bytes)
}

/// Convert a `u64` to an `Alias`.
pub fn id_to_alias(id: u64) -> Alias {
	let mut bytes = [0; 32];
	bytes[..8].copy_from_slice(&id.to_le_bytes());
	bytes
}

/// A signature type that is always successful for a given account
#[derive(
	PartialEq,
	Eq,
	Clone,
	Encode,
	Decode,
	DecodeWithMemTracking,
	Debug,
	Hash,
	PartialOrd,
	Ord,
	MaxEncodedLen,
	TypeInfo,
)]
pub struct AccountAuthority(pub AccountId32);

impl IdentifyAccount for AccountAuthority {
	type AccountId = AccountId32;
	fn into_account(self) -> Self::AccountId {
		self.0
	}
}

impl Verify for AccountAuthority {
	type Signer = Self;

	fn verify<L: sp_runtime::traits::Lazy<[u8]>>(
		&self,
		_msg: L,
		signer: &<Self::Signer as IdentifyAccount>::AccountId,
	) -> bool {
		self.0 == *signer
	}
}

/// A simple mock pallet that implements some mock deposit.
#[frame_support::pallet]
pub mod deposit {
	use frame_support::{pallet_prelude::*, traits::Consideration, DefaultNoBound};
	use sp_runtime::AccountId32;

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[derive(
		Encode,
		Decode,
		MaxEncodedLen,
		TypeInfo,
		Clone,
		Debug,
		Eq,
		PartialEq,
		DecodeWithMemTracking,
		DefaultNoBound,
	)]
	pub struct Deposit<AccountId> {
		pub counter: u64,
		pub active: BoundedVec<(AccountId, MockConsideration), ConstU32<100_000>>,
		pub dropped: BoundedVec<MockConsideration, ConstU32<1000>>,
		pub burned: BoundedVec<MockConsideration, ConstU32<100_000>>,
	}
	#[pallet::storage]
	pub type DepositStorage<T: Config> = StorageValue<_, Deposit<T::AccountId>, ValueQuery>;

	#[pallet::config]
	pub trait Config: frame_system::Config {}

	#[derive(
		Encode,
		Decode,
		MaxEncodedLen,
		TypeInfo,
		Clone,
		Copy,
		Debug,
		Eq,
		PartialEq,
		DecodeWithMemTracking,
		Default,
	)]
	pub struct MockConsideration(pub u64);
	impl Consideration<AccountId32, u64> for MockConsideration {
		fn new(who: &AccountId32, _new: u64) -> Result<Self, DispatchError> {
			DepositStorage::<super::Test>::mutate(|deposit| {
				let id = MockConsideration(deposit.counter);
				deposit.counter += 1;
				deposit.active.try_push((who.clone(), id)).unwrap();
				Ok(id)
			})
		}
		fn update(self, _who: &AccountId32, _new: u64) -> Result<Self, DispatchError> {
			DepositStorage::<super::Test>::mutate(|deposit| {
				deposit.active.iter().position(|(_, u)| *u == self).ok_or("not found")?;
				Ok(self)
			})
		}
		fn drop(self, _who: &AccountId32) -> Result<(), DispatchError> {
			DepositStorage::<super::Test>::mutate(|deposit| {
				let index =
					deposit.active.iter().position(|(_, u)| *u == self).ok_or("not found")?;
				let (_, id) = deposit.active.remove(index);
				deposit.dropped.try_push(id).unwrap();

				Ok(())
			})
		}
		fn burn(self, _: &AccountId32) {
			DepositStorage::<super::Test>::mutate(|deposit| {
				let index = deposit.active.iter().position(|(_, u)| *u == self).expect("not found");
				let (_, id) = deposit.active.remove(index);
				deposit.burned.try_push(id).unwrap();
			});
		}
		#[cfg(feature = "runtime-benchmarks")]
		fn ensure_successful(_who: &AccountId32, _amount: u64) {}
	}
}

pub const NOT_FUNDED_ACCOUNT: AccountId32 = AccountId32::new(*b"4321                            ");

#[derive(Clone, Eq, PartialEq, Encode, Decode, TypeInfo, DecodeWithMemTracking, Debug)]
pub struct DenyNotFundedAccount;

impl TransactionExtensionTrait<RuntimeCall> for DenyNotFundedAccount {
	const IDENTIFIER: &'static str = "DenyNotFundedAccount";
	type Implicit = ();
	type Val = ();
	type Pre = ();

	fn weight(&self, _: &RuntimeCall) -> sp_runtime::Weight {
		Default::default()
	}

	fn validate(
		&self,
		origin: RuntimeOrigin,
		_: &RuntimeCall,
		_: &DispatchInfoOf<RuntimeCall>,
		_: usize,
		_: (),
		_: &impl Encode,
		_: TransactionSource,
	) -> ValidateResult<(), RuntimeCall> {
		match origin.caller() {
			OriginCaller::system(frame_system::RawOrigin::Signed(NOT_FUNDED_ACCOUNT)) =>
				Err(InvalidTransaction::Payment.into()),
			_ => Ok((ValidTransaction::default(), (), origin)),
		}
	}

	fn prepare(
		self,
		_: Self::Val,
		_: &RuntimeOrigin,
		_: &RuntimeCall,
		_: &DispatchInfoOf<RuntimeCall>,
		_: usize,
	) -> Result<Self::Pre, TransactionValidityError> {
		Ok(())
	}
}

pub type TransactionExtension = (
	crate::GameAsInvited<Test>,
	indiv_pallet_score::ScoreAsParticipant<Test>,
	DenyNotFundedAccount,
);

pub type Header = sp_runtime::generic::Header<u64, sp_runtime::traits::BlakeTwo256>;
pub type Block = sp_runtime::generic::Block<Header, Extrinsic>;
pub type Extrinsic = sp_runtime::generic::UncheckedExtrinsic<
	AccountId32,
	RuntimeCall,
	AccountAuthority,
	TransactionExtension,
>;

impl<LocalCall> CreateBare<LocalCall> for Test
where
	RuntimeCall: From<LocalCall>,
{
	fn create_bare(call: Self::RuntimeCall) -> Self::Extrinsic {
		Extrinsic::new_bare(call)
	}
}

impl<LocalCall> CreateTransactionBase<LocalCall> for Test
where
	RuntimeCall: From<LocalCall>,
{
	type Extrinsic = Extrinsic;
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
		Extrinsic::new_transaction(call, extension)
	}
}

impl<LocalCall> CreateAuthorizedTransaction<LocalCall> for Test
where
	RuntimeCall: From<LocalCall>,
{
	fn create_extension() -> Self::Extension {
		(
			crate::GameAsInvited::new(None),
			indiv_pallet_score::ScoreAsParticipant::new(None),
			DenyNotFundedAccount,
		)
	}
}

// Configure a mock runtime to test the pallet.
frame_support::construct_runtime!(
	pub enum Test
	{
		System: frame_system,
		ChunksManager: indiv_pallet_chunks_manager,
		Members: indiv_pallet_members,
		Game: crate,
		Score: indiv_pallet_score,
		Balances: pallet_balances,
		Assets: pallet_assets,
		AssetsHolder: pallet_assets_holder,
		Airdrop: indiv_pallet_airdrop,
		People: indiv_pallet_people,
		Deposit: deposit,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type BaseCallFilter = frame_support::traits::Everything;
	type BlockWeights = ();
	type BlockLength = ();
	type DbWeight = ();
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	type Nonce = u64;
	type Hash = H256;
	type Hashing = BlakeTwo256;
	type AccountId = AccountId32;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Block = Block;
	type RuntimeEvent = RuntimeEvent;
	type BlockHashCount = ConstUint<250>;
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = pallet_balances::AccountData<u64>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ConstUint<42>;
	type OnSetCode = ();
	type MaxConsumers = frame_support::traits::ConstU32<16>;
}

impl indiv_pallet_chunks_manager::Config for Test {
	type WeightInfo = ();
	type Chunk = <Mock as GenerateVerifiable>::StaticChunk;
	type PageSize = ConstU32<1024>;
	type ManagerOrigin = EnsureRoot<Self::AccountId>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ChunksBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct ChunksBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl indiv_pallet_chunks_manager::BenchmarkHelper<()> for ChunksBenchmarkHelper {
	fn chunk_page() -> Vec<()> {
		vec![(); 1024]
	}
}

parameter_types! {
	pub const FlexibleRingExp: RingExponent = RingExponent::R2e9;
	pub const MockCollectionOwner: u32 = 1;
}

impl indiv_pallet_members::Config for Test {
	type WeightInfo = ();
	type Crypto = Mock;
	type Location = u32;
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
	type BenchmarkHelper = ();
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
	type Balance = u64;
	type RuntimeEvent = RuntimeEvent;
	type DustRemoval = ();
	type AccountStore = System;
	type WeightInfo = ();
	type MaxLocks = ();
	type ReserveIdentifier = [u8; 8];
	type RuntimeFreezeReason = RuntimeFreezeReason;
	type FreezeIdentifier = ();
}

#[derive_impl(pallet_assets::config_preludes::TestDefaultConfig)]
impl pallet_assets::Config for Test {
	type Balance = u128;
	type AssetId = u32;
	type AssetIdParameter = u32;
	type Currency = Balances;
	type CreateOrigin = AsEnsureOriginWithArg<frame_system::EnsureSigned<AccountId32>>;
	type ForceOrigin = EnsureRoot<AccountId32>;
	type Holder = AssetsHolder;
}

impl pallet_assets_holder::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeHoldReason = RuntimeHoldReason;
}

parameter_types! {
	pub PlayerStatementLimit: StatementAllowance = StatementAllowance {
		max_size: 1000,
		max_count: 1000,
	};
}

pub struct MockWeightInfo;
impl WeightInfo for MockWeightInfo {
	fn new_game() -> Weight {
		Weight::zero()
	}
	fn get_game() -> Weight {
		Weight::zero()
	}
	fn get_game_schedules(_n: u32) -> Weight {
		Weight::zero()
	}
	fn unix_time() -> Weight {
		Weight::zero()
	}
	fn put_game() -> Weight {
		Weight::zero()
	}
	fn put_game_schedules() -> Weight {
		Weight::zero()
	}
	fn shuffles_base() -> Weight {
		Weight::from_parts(15, 15)
	}
	fn shuffle_step_insert(_n: u32) -> Weight {
		Weight::from_parts(10, 10)
	}
	fn shuffle_step_retrieve(_n: u32) -> Weight {
		Weight::from_parts(10, 10)
	}
	fn shuffle_step_compute_weights(_n: u32, _r: u32) -> Weight {
		Weight::from_parts(10, 10)
	}
	fn shuffle_step_start_session() -> Weight {
		Weight::from_parts(10, 10)
	}
	fn player_process_step1() -> Weight {
		Weight::from_parts(15, 15)
	}
	fn player_process_step1_attended_player() -> Weight {
		Weight::from_parts(10, 10)
	}
	fn player_process_step1_not_attended_player() -> Weight {
		Weight::from_parts(10, 10)
	}
	fn process_cancelling() -> Weight {
		Weight::zero()
	}
	fn process_cancelling_step(_n: u32) -> Weight {
		Weight::zero()
	}

	fn sign_up_with_invite() -> Weight {
		Weight::zero()
	}

	fn sign_up_with_account_new() -> Weight {
		Weight::zero()
	}

	fn sign_up_with_account_recognized() -> Weight {
		Weight::zero()
	}

	fn sign_up_with_alias() -> Weight {
		Weight::zero()
	}

	fn report(p: u32) -> Weight {
		Weight::from_parts(p as u64, p as u64)
	}

	fn offboard_account() -> Weight {
		Weight::zero()
	}
	fn offboard_person() -> Weight {
		Weight::zero()
	}

	fn kickout() -> Weight {
		Weight::zero()
	}

	fn grant_invites() -> Weight {
		Weight::zero()
	}

	fn remove_available_and_pending_invites(_n: u32) -> Weight {
		Weight::zero()
	}

	fn set_invite_ticket() -> Weight {
		Weight::zero()
	}

	fn cancel_invite_ticket() -> Weight {
		Weight::zero()
	}

	fn schedule_games(_n: u32) -> Weight {
		Weight::zero()
	}

	fn remove_scheduled_game() -> Weight {
		Weight::zero()
	}

	fn set_play_deposit() -> Weight {
		Weight::zero()
	}

	fn as_invited_tx_ext() -> Weight {
		Weight::zero()
	}

	fn process_reporting() -> Weight {
		Weight::zero()
	}

	fn insert_attendance_history() -> Weight {
		Weight::zero()
	}

	fn player_process_step2() -> Weight {
		Weight::from_parts(10, 10)
	}
	fn player_process_step2_inner_loop() -> Weight {
		Weight::from_parts(PLAYER_PROCESS_STEP2_CHUNK as u64, PLAYER_PROCESS_STEP2_CHUNK as u64)
	}
	fn kill_current_game() -> Weight {
		Weight::zero()
	}
	fn set_game_phases() -> Weight {
		Weight::zero()
	}
	fn claim_airdrop() -> Weight {
		Weight::zero()
	}
	fn on_game_cancelled() -> Weight {
		Weight::zero()
	}
}

parameter_types! {
	pub storage PeopleVoteWeight: u8 = 1;
	pub storage CandidateVoteWeight: u8 = 1;
	pub const PlayDepositDefault: u64 = 2;
}

thread_local! {
	pub(crate) static AIRDROP_RANDOMNESS: RefCell<Option<([u8; 32], u32)>> =
		const { RefCell::new(Some(([42u8; 32], 1))) };
	pub(crate) static AIRDROP_ACCOUNT_TO_PUBLIC:
		RefCell<alloc::collections::BTreeMap<AccountId32, sp_core::sr25519::Public>> =
		const { RefCell::new(alloc::collections::BTreeMap::new()) };
}

pub fn register_account_pubkey(account_id: AccountId32, public: sp_core::sr25519::Public) {
	AIRDROP_ACCOUNT_TO_PUBLIC.with(|m| {
		m.borrow_mut().insert(account_id, public);
	});
}

pub struct AccountToPub;
impl sp_runtime::traits::TryConvert<AccountId32, sp_core::sr25519::Public> for AccountToPub {
	fn try_convert(account_id: AccountId32) -> Result<sp_core::sr25519::Public, AccountId32> {
		AIRDROP_ACCOUNT_TO_PUBLIC
			.with(|m| m.borrow().get(&account_id).copied())
			.ok_or(account_id)
	}
}

pub struct MockAirdropRandomness;
impl indiv_support::traits::MomentRandomness<u32> for MockAirdropRandomness {
	fn randomness() -> Option<([u8; 32], u32)> {
		AIRDROP_RANDOMNESS.with(|r| *r.borrow())
	}

	fn current_moment() -> u32 {
		AIRDROP_RANDOMNESS.with(|r| r.borrow().map_or(0, |(_, moment)| moment))
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn set_randomness(randomness: [u8; 32], moment: u32) {
		AIRDROP_RANDOMNESS.with(|r| *r.borrow_mut() = Some((randomness, moment)));
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn set_current_moment(moment: u32) {
		AIRDROP_RANDOMNESS.with(|r| {
			let mut r = r.borrow_mut();
			let value = r.map_or([0u8; 32], |(value, _)| value);
			*r = Some((value, moment));
		});
	}
}

/// Advance the mock airdrop randomness by one block, so a draw awaiting entropy sees a
/// value produced after registration closed.
pub fn advance_airdrop_randomness() {
	AIRDROP_RANDOMNESS.with(|r| {
		let mut r = r.borrow_mut();
		if let Some((value, block_number)) = *r {
			*r = Some((value, block_number + 1));
		}
	});
}

/// Mock membership service.
pub struct MockAirdropMemberService;
impl indiv_support::traits::MembershipProver for MockAirdropMemberService {
	type Crypto = Mock;

	fn verify_membership(
		_identifier: &indiv_support::traits::Identifier,
		_proof: &<Mock as GenerateVerifiable>::Proof,
		_ring_index: indiv_support::traits::RingIndex,
		_revision: indiv_support::traits::RevisionIndex,
		context: indiv_support::traits::Context,
		msg: &[u8],
	) -> Result<indiv_support::traits::ContextualAlias, DispatchError> {
		let alias = alias_from_message(msg);
		Ok(indiv_support::traits::ContextualAlias { context, alias })
	}

	fn verify_memberships_in_ring(
		_identifier: &indiv_support::traits::Identifier,
		_ring_index: indiv_support::traits::RingIndex,
		_revision: indiv_support::traits::RevisionIndex,
		_items: &[indiv_support::traits::RingMembershipProof<
			<Mock as GenerateVerifiable>::Proof,
		>],
	) -> Result<Vec<indiv_support::traits::ContextualAlias>, DispatchError> {
		unimplemented!()
	}

	fn ring_revision(
		_identifier: &indiv_support::traits::Identifier,
		_ring_index: indiv_support::traits::RingIndex,
	) -> Option<indiv_support::traits::RevisionIndex> {
		Some(0)
	}

	fn is_revision_valid(
		_identifier: &indiv_support::traits::Identifier,
		_ring_index: indiv_support::traits::RingIndex,
		_revision: indiv_support::traits::RevisionIndex,
	) -> bool {
		true
	}

	fn revision_source_time(
		_identifier: &indiv_support::traits::Identifier,
		_ring_index: indiv_support::traits::RingIndex,
		_revision: indiv_support::traits::RevisionIndex,
	) -> Option<u64> {
		None
	}

	fn old_root_retention() -> u64 {
		// This mock keeps no root history, so nothing is ever superseded.
		0
	}
}

/// Derive a deterministic 32-byte alias from the message bytes.
///
/// Used in mock membership service.
fn alias_from_message(msg: &[u8]) -> indiv_support::traits::Alias {
	sp_io::hashing::blake2_256(msg)
}

parameter_types! {
	pub AirdropSource: AccountId32 = id_to_account(0xa1d_700);
	pub AirdropPalletId: PalletId = PalletId(*b"pop/adrp");
	pub storage AirdropClearLimit: u32 = 100;
	pub storage AirdropDrawLimit: u32 = 100;
}

/// `Assets + AssetsHolder` combined.
pub type AssetsWithHolder = indiv_support::fungibles::CombineAssetsWithHolder<Assets, AssetsHolder>;

impl indiv_pallet_airdrop::Config for Test {
	type WeightInfo = ();
	type MemberService = MockAirdropMemberService;
	type Fungibles = AssetsWithHolder;
	type ManagerOrigin = EnsureRoot<AccountId32>;
	type PalletId = AirdropPalletId;
	type UnixTime = Test;
	type Randomness = MockAirdropRandomness;
	type AccountIdToPublic = AccountToPub;
	type ClearLimit = AirdropClearLimit;
	type DrawLimit = AirdropDrawLimit;
	type OffchainWorkerInterval = ConstU64<1>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = TestAirdropBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct TestAirdropBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl indiv_pallet_airdrop::benchmarking::BenchmarkHelper<Test> for TestAirdropBenchmarkHelper {
	fn set_unix_time(now: Duration) {
		MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = now);
	}

	fn create_asset_id_parameter(id: u32) -> u32 {
		id
	}

	fn build_membership_proof(
		_context: &indiv_support::traits::Context,
		_message: &[u8],
		_member_seed: u32,
	) -> (indiv_pallet_airdrop::ProofOf<Test>, indiv_support::traits::Alias) {
		// `MockAirdropMemberService` is permissive: any proof verifies, and the surfaced
		// alias is derived from the message. A default proof is therefore enough.
		(<Mock as GenerateVerifiable>::Proof::default(), [0u8; 32])
	}

	fn account_keypair_for(seed: u32) -> (AccountId32, sp_core::sr25519::Pair) {
		use sp_core::Pair as _;
		let mut entropy = [0u8; 32];
		entropy[28..32].copy_from_slice(&seed.to_le_bytes());
		let pair = sp_core::sr25519::Pair::from_seed(&entropy);
		let public = pair.public();
		let account_id: AccountId32 = id_to_account(seed as u64);
		register_account_pubkey(account_id.clone(), public);
		(account_id, pair)
	}
}

/// Default airdrop prize used by test schedules.
pub fn test_airdrop_prize() -> indiv_pallet_airdrop::types::AirdropPrize<u32, u128> {
	indiv_pallet_airdrop::types::AirdropPrize {
		asset_id: TEST_AIRDROP_ASSET_ID,
		asset_amount: 1_000,
		max_winners: 100,
		winner_cap: Permill::from_percent(50),
	}
}

/// Default claim window used by test schedules.
pub const TEST_AIRDROP_CLAIM_WINDOW: u64 = 7 * 24 * 60 * 60;

impl crate::Config for Test {
	type WeightInfo = MockWeightInfo;
	type UnixTime = Test;
	type MaxRounds = ConstUint<10>;
	type MaxGroupSize = ConstUint<10>;
	type MinGroupSize = ConstUint<0>;
	type ManagerOrigin = EnsureRoot<Self::AccountId>;
	type InviteIssuer = EnsureRoot<Self::AccountId>;
	type NonPlayingKickoutTime = ConstUint<1000>;
	type NativeFungible = Balances;
	type PlayDeposit = deposit::MockConsideration;
	type DefaultPlayDeposit = PlayDepositDefault;
	type DefaultPhaseDurations = GamePhaseDurations;
	type MaxGameSchedules = ConstUint<5>;
	type MaxAttendanceHistoryDepth = ConstUint<2>;
	type TicketSignature = TestSignature;
	type PlayerStatementLimit = PlayerStatementLimit;
	type AccountSignature = AccountAuthority;
	type PeopleVoteWeight = PeopleVoteWeight;
	type CandidateVoteWeight = CandidateVoteWeight;
	type AirdropAssetId = u32;
	type AirdropAssetBalance = u128;
	type Airdrop = Airdrop;
	type AirdropSource = AirdropSource;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = BenchHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct BenchHelper {}

#[cfg(feature = "runtime-benchmarks")]
use frame_benchmarking::account;

#[cfg(feature = "runtime-benchmarks")]
impl BenchmarkHelper<AccountAuthority, TestSignature, u64, AccountId32, u32> for BenchHelper {
	fn create_account(seed: u64) -> AccountId32 {
		account("acc", 0, seed as u32)
	}

	fn sign_account(seed: u64, _msg: &[u8]) -> AccountAuthority {
		AccountAuthority(Self::create_account(seed))
	}

	fn create_ticket(seed: u64) -> u64 {
		seed
	}

	fn sign_ticket(seed: u64, msg: &[u8]) -> TestSignature {
		TestSignature(seed, msg.to_vec())
	}

	fn set_valid_time() {
		MOCK_UNIX_TIME.with(|mock| *mock.borrow_mut() = Default::default());
	}

	fn set_time(now: core::time::Duration) {
		MOCK_UNIX_TIME.with(|mock| *mock.borrow_mut() = now);
	}

	fn fund_account(acc: AccountId32) {
		use frame_support::traits::Currency;
		let _ = Balances::make_free_balance_be(&acc, u64::MAX / 2);
	}

	fn airdrop_asset_id() -> u32 {
		TEST_AIRDROP_ASSET_ID
	}
}

pub struct GamePhaseDurations;
impl Get<PhaseDurationValues> for GamePhaseDurations {
	fn get() -> PhaseDurationValues {
		PhaseDurationValues {
			registration: 2,
			shuffle: 1,
			post_shuffle_margin: 1,
			reporting: 2,
			player_process: 2,
			airdrop_claim_window: TEST_AIRDROP_CLAIM_WINDOW as u32,
		}
	}
}

impl deposit::Config for Test {}

impl indiv_pallet_people::Config for Test {
	type WeightInfo = ();
	type AccountContexts = Everything;
	type OnboardingQueuePageSize = ConstUint<512>;
	type MemberService = Members;
	type CollectionOwner = MockCollectionOwner;
	type RingExponent = FlexibleRingExp;
	type StaleAliasCleanupInterval = ConstUint<5>;
	type SelfInclusionDelay = ConstUint<3600>;
	type ManagerOrigin = EnsureRoot<Self::AccountId>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
}

parameter_types! {
	pub const ScorePotId: PalletId = PalletId(*b"scorepot");
	pub const BalancesLocation: Location = Location::here();
}

#[cfg(feature = "runtime-benchmarks")]
impl indiv_pallet_score::benchmarking::BenchmarkHelper<Test> for Test {
	fn create_member(_seed: u64) -> [u8; 32] {
		unimplemented!()
	}
	fn setup_currency() {}
}

impl indiv_pallet_score::Config for Test {
	type WeightInfo = ();
	type EnsurePerson = indiv_pallet_people::EnsurePersonalAliasInContext<Test>;
	type ScorePotId = ScorePotId;
	type Currency = Balances;
	type CurrencyLocationInfo = BalancesLocation;
	type ManagerOrigin = EnsureRoot<Self::AccountId>;
	type MaxPayoutRoundSchedules = ConstUint<10>;
	type OffchainWorkInterval = ConstUint<1>;
	type People = People;
	type Crypto = Mock;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = Test;
}

thread_local! {
	pub static MOCK_UNIX_TIME: RefCell<Duration> = RefCell::new(Default::default());
}
impl frame_support::traits::UnixTime for Test {
	fn now() -> Duration {
		MOCK_UNIX_TIME.with(|mock| *mock.borrow())
	}
}

pub struct MockStatementStore(pub Mutex<Vec<(sp_statement_store::Hash, Statement)>>);

impl sp_statement_store::StatementStore for MockStatementStore {
	fn posted(
		&self,
		_match_all_topics: &[Topic],
		_dest: [u8; 32],
	) -> sp_statement_store::Result<Vec<Vec<u8>>> {
		unimplemented!();
	}
	fn submit(
		&self,
		_statement: Statement,
		_source: StatementSource,
	) -> sp_statement_store::SubmitResult {
		unimplemented!();
	}
	fn remove(&self, hash: &sp_statement_store::Hash) -> sp_statement_store::Result<()> {
		self.0.lock().unwrap().retain(|(h, _)| h != hash);
		Ok(())
	}
	fn statement(
		&self,
		_hash: &sp_statement_store::Hash,
	) -> sp_statement_store::Result<Option<Statement>> {
		unimplemented!();
	}
	fn statements(&self) -> sp_statement_store::Result<Vec<(sp_statement_store::Hash, Statement)>> {
		Ok(self.0.lock().unwrap().clone())
	}
	fn broadcasts(&self, _match_all_topics: &[Topic]) -> sp_statement_store::Result<Vec<Vec<u8>>> {
		unimplemented!();
	}
	fn posted_clear(
		&self,
		_match_all_topics: &[Topic],
		_dest: [u8; 32],
	) -> sp_statement_store::Result<Vec<Vec<u8>>> {
		unimplemented!();
	}
	fn posted_stmt(
		&self,
		_match_all_topics: &[Topic],
		_dest: [u8; 32],
	) -> sp_statement_store::Result<Vec<Vec<u8>>> {
		unimplemented!();
	}
	fn broadcasts_stmt(
		&self,
		_match_all_topics: &[Topic],
	) -> sp_statement_store::Result<Vec<Vec<u8>>> {
		unimplemented!();
	}
	fn posted_clear_stmt(
		&self,
		_match_all_topics: &[Topic],
		_dest: [u8; 32],
	) -> sp_statement_store::Result<Vec<Vec<u8>>> {
		unimplemented!();
	}
	fn remove_by(&self, who: [u8; 32]) -> sp_statement_store::Result<()> {
		use sp_statement_store::Proof;
		let mut guard = self.0.lock().unwrap();
		guard.retain(|(_, stmt)| {
			match stmt.proof() {
				Some(Proof::Ed25519 { signer, .. }) => *signer != who,
				Some(Proof::Sr25519 { signer, .. }) => *signer != who,
				// For ECDSA or None, conservatively keep the statement.
				_ => true,
			}
		});
		Ok(())
	}

	fn take_recent_statements(
		&self,
	) -> sp_statement_store::Result<Vec<(sp_statement_store::Hash, Statement)>> {
		unimplemented!();
	}

	fn has_statement(&self, _: &sp_statement_store::Hash) -> bool {
		unimplemented!();
	}
	fn statement_hashes(&self) -> Vec<sp_statement_store::Hash> {
		unimplemented!();
	}
	fn statements_by_hashes(
		&self,
		_hashes: &[sp_statement_store::Hash],
		_filter: &mut dyn FnMut(
			&sp_statement_store::Hash,
			&[u8],
			&Statement,
		) -> sp_statement_store::FilterDecision,
	) -> sp_statement_store::Result<(Vec<(sp_statement_store::Hash, Statement)>, usize)> {
		unimplemented!();
	}
}

impl MockStatementStore {
	pub fn add_stmt(&self, stmt: Statement) {
		self.0.lock().unwrap().push((stmt.hash(), stmt));
	}
	pub fn clear(&self) {
		self.0.lock().unwrap().clear();
	}
}

pub static MOCK_STATEMENT_STORE: LazyLock<Arc<MockStatementStore>> =
	LazyLock::new(|| Arc::new(MockStatementStore(Mutex::new(Default::default()))));

pub fn new_test_ext() -> sp_io::TestExternalities {
	use codec::Encode;

	// Create a page of chunks and compute its hash
	let chunks: Vec<<Mock as GenerateVerifiable>::StaticChunk> = [(); 1024].to_vec();
	let encoded_chunks = chunks.encode();
	let page_hash = sp_io::hashing::blake2_256(&encoded_chunks);

	let c = RuntimeGenesisConfig {
		system: Default::default(),
		chunks_manager: indiv_pallet_chunks_manager::GenesisConfig::<Test> {
			encoded_chunk_page_hashes: vec![(RingExponent::R2e9.exponent(), vec![page_hash])],
			..Default::default()
		},
		..Default::default()
	}
	.build_storage()
	.unwrap();
	let mut ext = sp_io::TestExternalities::from(c);
	ext.register_extension(sp_statement_store::runtime_api::StatementStoreExt::new(
		MOCK_STATEMENT_STORE.clone(),
	));
	// 100k active members yields a personhood threshold of 21 and absence grace ratio (1, 6).
	ext.execute_with(|| {
		indiv_pallet_members::ActiveMembers::<Test>::insert(
			indiv_pallet_people::PEOPLE_MEMBER_IDENTIFIER,
			100_000,
		);
		indiv_pallet_score::PersonhoodThreshold::<Test>::put(21);
		indiv_pallet_score::AbsenceGraceRatio::<Test>::put((1u8, 6u8));
	});
	// Setup the airdrop environment.
	ext.execute_with(|| {
		use frame_support::traits::fungibles::{Create, Inspect, Mutate};
		let asset_id: u32 = TEST_AIRDROP_ASSET_ID;
		let pot = indiv_pallet_airdrop::Pallet::<Test>::airdrop_pot_id();
		<Assets as Create<AccountId32>>::create(asset_id, pot.clone(), true, 1u128)
			.expect("create airdrop asset");
		let min = <Assets as Inspect<AccountId32>>::minimum_balance(asset_id);
		<Assets as Mutate<AccountId32>>::mint_into(asset_id, &pot, min).expect("seed pot ED");
		indiv_pallet_airdrop::SupportedAssets::<Test>::insert(asset_id, min);
		<Assets as Mutate<AccountId32>>::mint_into(asset_id, &AirdropSource::get(), u128::MAX / 2)
			.expect("fund airdrop source");
	});
	ext
}

/// Asset id used by [`test_airdrop_prize`].
pub const TEST_AIRDROP_ASSET_ID: u32 = 7777;

/// We gather both error into a single type in order to do `assert_ok` and `assert_err` safely.
/// Otherwise, we can easily miss the inner error in a `Resut<Resut<_, _>, _>`.
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum TransactionExecutionError {
	Validity(TransactionValidityError),
	// This ignores the post info.
	Dispatch(DispatchErrorWithPostInfo),
}

impl TransactionExecutionError {
	#[allow(unused)]
	pub fn unwrap_dispatch(self) -> DispatchErrorWithPostInfo {
		let Self::Dispatch(error) = self else {
			panic!("validity error unwrapped as dispatch");
		};
		error
	}
}

impl From<DispatchErrorWithPostInfo> for TransactionExecutionError {
	fn from(e: DispatchErrorWithPostInfo) -> Self {
		TransactionExecutionError::Dispatch(e)
	}
}

impl From<TransactionValidityError> for TransactionExecutionError {
	fn from(e: TransactionValidityError) -> Self {
		TransactionExecutionError::Validity(e)
	}
}

impl From<DispatchError> for TransactionExecutionError {
	fn from(e: DispatchError) -> Self {
		TransactionExecutionError::Dispatch(e.into())
	}
}

impl From<InvalidTransaction> for TransactionExecutionError {
	fn from(e: InvalidTransaction) -> Self {
		TransactionExecutionError::Validity(e.into())
	}
}

/// Execute a bare extrinsic with the given call.
pub fn exec_tx(x: Extrinsic) -> Result<(), TransactionExecutionError> {
	let info = x.get_dispatch_info();
	let len = x.encoded_size();

	let checked = Checkable::check(x, &frame_system::ChainContext::<Test>::default())?;

	// validation is always rollbacked in production.
	with_transaction(|| {
		let valid = checked.validate::<Test>(TransactionSource::External, &info, len);

		TransactionOutcome::Rollback(Result::<_, DispatchError>::Ok(valid))
	})
	.unwrap()?;

	checked.apply::<Test>(&info, len)??;

	Ok(())
}

/// Execute a bare extrinsic with the given call.
#[allow(unused)]
pub fn exec_bare_tx(call: impl Into<RuntimeCall>) -> Result<(), TransactionExecutionError> {
	let x = Extrinsic::new_bare(call.into());

	exec_tx(x)
}

/// Execute a signed extrinsic with the invited transaction extension and the given call.
pub fn exec_invited_tx(
	account: AccountId32,
	tx_ext: crate::GameAsInvitedData<Test>,
	call: impl Into<RuntimeCall>,
) -> Result<(), TransactionExecutionError> {
	let x = Extrinsic::new_signed(
		call.into(),
		account.clone(),
		AccountAuthority(account),
		(
			crate::GameAsInvited::<Test>::new(Some(tx_ext)),
			indiv_pallet_score::ScoreAsParticipant::<Test>::new(None),
			DenyNotFundedAccount,
		),
	);

	exec_tx(x)
}

/// Execute a signed extrinsic with the given call.
pub fn exec_signed_tx(
	account: AccountId32,
	call: impl Into<RuntimeCall>,
) -> Result<(), TransactionExecutionError> {
	let x = Extrinsic::new_signed(
		call.into(),
		account.clone(),
		AccountAuthority(account),
		(
			crate::GameAsInvited::<Test>::new(None),
			indiv_pallet_score::ScoreAsParticipant::<Test>::new(None),
			DenyNotFundedAccount,
		),
	);

	exec_tx(x)
}

/// Execute a signed extrinsic with the participant transaction extension and the given call.
pub fn exec_participant_tx(
	account: AccountId32,
	nonce: u64,
	call: impl Into<RuntimeCall>,
) -> Result<(), TransactionExecutionError> {
	let x = Extrinsic::new_signed(
		call.into(),
		account.clone(),
		AccountAuthority(account),
		(
			crate::GameAsInvited::<Test>::new(None),
			indiv_pallet_score::ScoreAsParticipant::<Test>::new(Some(
				indiv_pallet_score::ScoreAsParticipantData { nonce },
			)),
			DenyNotFundedAccount,
		),
	);

	exec_tx(x)
}

#[allow(clippy::too_many_arguments)]
pub fn run_game_scenario<F>(
	schedule: GameSchedule<u32, u128>,
	players: &[AccountOrPerson<AccountId32>],
	report_generator: F,
) where
	F: Fn(&AccountOrPerson<AccountId32>) -> Option<FullReport<Test>> + Copy,
{
	run_game_scenario_with_phase(
		schedule,
		|| {
			for p in players {
				match p {
					AccountOrPerson::Account(acc) => {
						assert_ok!(Game::sign_up_with_account(
							RuntimeOrigin::signed(acc.clone()),
							DEFAULT_IDENTIFIER_KEY,
							None,
						));
					},
					AccountOrPerson::Person(alias) => {
						let account = AccountId32::new(*alias);
						assert_ok!(Game::sign_up_with_alias(
							runtime_origin_for_alias(alias),
							DEFAULT_IDENTIFIER_KEY,
							account.clone(),
							AccountAuthority(account),
							None,
						));
					},
				}
			}
		},
		|| {
			for p in players {
				if let Some(full_report) = report_generator(p) {
					match p {
						AccountOrPerson::Account(acc) => {
							assert_ok!(Game::report(
								RuntimeOrigin::signed(acc.clone()),
								full_report
							));
						},
						AccountOrPerson::Person(alias) => {
							assert_ok!(Game::report(runtime_origin_for_alias(alias), full_report));
						},
					}
				}
			}
		},
	)
}

// A helper that runs a full game scenario: "new_game" -> "sign_up" -> "report" -> "process".
#[allow(clippy::too_many_arguments)]
pub fn run_game_scenario_with_phase<FS, FR>(
	schedule: GameSchedule<u32, u128>,
	sign_up_fn: FS,
	report_fn: FR,
) where
	FS: FnOnce(),
	FR: FnOnce(),
{
	// Create a new game
	assert_ok!(Game::new_game(&schedule));

	let registration_ends = GameTimes::<Test>::registration_end(&schedule);
	let report_ends = GameTimes::<Test>::reporting_end(&schedule);

	// Execute the sign-up phase using the user-supplied closure
	sign_up_fn();

	// Move time -> end registration -> transition
	crate::mock::MOCK_UNIX_TIME
		.with(|t| *t.borrow_mut() = Duration::from_secs((registration_ends + 1) as u64));
	advance_process(); // registration to shuffle
	advance_process(); // shuffle to report

	// Now do your `report_fn` (the user-supplied closure)
	report_fn();

	// Move time beyond `report_ends` -> transition -> process
	crate::mock::MOCK_UNIX_TIME
		.with(|t| *t.borrow_mut() = Duration::from_secs((report_ends + 1) as u64));
	advance_process(); // report to player process step1
	advance_process(); // step1 to step2
	advance_process(); // step2 to step3
	advance_process(); // step3 to done
}

pub fn block_skipped() -> bool {
	System::block_number().is_multiple_of(GAME_PROCESS_SKIPPED_BLOCK as u64)
}

pub fn advance_process_with_weights(on_poll: Weight, on_idle: Weight) {
	let bn = System::block_number() + 1;
	System::run_to_block_with::<AllPalletsWithSystem>(
		bn,
		RunToBlockHooks::default().after_initialize(|bn| {
			AllPalletsWithSystem::on_poll(bn, &mut WeightMeter::with_limit(on_poll));
			AllPalletsWithSystem::on_idle(bn, on_idle);
		}),
	);
	if block_skipped() {
		advance_process_with_weights(on_poll, on_idle);
	}
}

pub fn advance_process_with_on_poll_only() {
	let bn = System::block_number() + 1;
	System::run_to_block_with::<AllPalletsWithSystem>(
		bn,
		RunToBlockHooks::default().after_initialize(|bn| {
			AllPalletsWithSystem::on_poll(bn, &mut WeightMeter::with_limit(Weight::MAX));
		}),
	);
	if block_skipped() {
		advance_process_with_on_poll_only();
	}
}

pub fn advance_process() {
	advance_process_with_weights(Weight::MAX, Weight::zero());
}

pub fn run_game_scenario_with_hooks<F>(
	schedule: GameSchedule<u32, u128>,
	players: &[AccountOrPerson<AccountId32>],
	report_generator: F,
) where
	F: Fn(&AccountOrPerson<AccountId32>) -> Option<FullReport<Test>> + Copy,
{
	let registration_ends = GameTimes::<Test>::registration_end(&schedule);
	let game_play_time = GameTimes::<Test>::game_play_time(&schedule);
	let report_ends = GameTimes::<Test>::reporting_end(&schedule);

	// Sign up
	for p in players {
		match p {
			AccountOrPerson::Account(acc) => {
				assert_ok!(Game::sign_up_with_account(
					RuntimeOrigin::signed(acc.clone()),
					DEFAULT_IDENTIFIER_KEY,
					None,
				));
			},
			AccountOrPerson::Person(alias) => {
				let account = AccountId32::new(*alias);
				assert_ok!(Game::sign_up_with_alias(
					runtime_origin_for_alias(alias),
					DEFAULT_IDENTIFIER_KEY,
					account.clone(),
					AccountAuthority(account),
					None,
				));
			},
		}
	}

	// Move time, end registration, shuffle
	MOCK_UNIX_TIME.with(|v| *v.borrow_mut() = Duration::from_secs(registration_ends as u64 + 1));
	advance_process(); // register to shuffle
	advance_process(); // shuffle to report

	// Move time to game day
	MOCK_UNIX_TIME.with(|v| *v.borrow_mut() = Duration::from_secs(game_play_time as u64));

	// Each player does some report
	for p in players {
		if let AccountOrPerson::Account(acc) = p {
			if let Some(full_report) = report_generator(p) {
				assert_ok!(Game::report(RuntimeOrigin::signed(acc.clone()), full_report));
			}
		}
	}

	// Move time to after report_ends, transition, process
	MOCK_UNIX_TIME.with(|v| *v.borrow_mut() = Duration::from_secs(report_ends as u64 + 1));
	advance_process(); // report to player process step1
	advance_process(); // step1 to step2
	advance_process(); // step2 to step3
	advance_process(); // step3 to done
	advance_process(); // done to next game
}

pub fn run_basic_game_scenario_with_hooks(
	schedule: GameSchedule<u32, u128>,
	players: &[AccountOrPerson<AccountId32>],
) {
	run_game_scenario_with_hooks(schedule, players, |_player| {
		Some(vec![vec![Report::Person].try_into().unwrap()].try_into().unwrap())
	});
}

/// Create a valid runtime origin as a personal alias in the context of SCORE_CONTEXT.
pub fn runtime_origin_for_alias(alias: &Alias) -> RuntimeOrigin {
	PersonalAlias(RevisedContextualAlias {
		revision: 0,
		ring: 0,
		ca: ContextualAlias { context: SCORE_CONTEXT, alias: *alias },
	})
	.into()
}

/// Generate a mock key pair for testing
pub fn mock_key(
	id: u64,
) -> (<Mock as GenerateVerifiable>::Member, <Mock as GenerateVerifiable>::Secret) {
	let mut entropy = [0u8; 32];
	entropy[0..8].copy_from_slice(&id.to_le_bytes());
	let secret = Mock::new_secret(entropy);
	let member = Mock::member_from_secret(&secret);
	(member, secret)
}
