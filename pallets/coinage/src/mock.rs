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

use crate::{
	extension::{AsCoinage, AsCoinageInfo},
	*,
};
use alloc::sync::Arc;
use codec::Encode;
use core::{
	cell::{Cell, RefCell},
	time::Duration,
};
use frame_support::{
	assert_ok, derive_impl, parameter_types,
	traits::{
		fungibles::{InspectHold, MutateHold},
		AsEnsureOriginWithArg, ConstU16, ConstU32, ConstU64, Currency, OffchainWorker, UnixTime,
	},
	BoundedVec,
};
use frame_system::{
	offchain::{CreateAuthorizedTransaction, CreateTransaction, CreateTransactionBase},
	AuthorizeCall,
};
use indiv_support::{
	fungibles::CombineAssetsWithHolder,
	traits::{AppendOnlyMembers, RingExponent},
};
use sp_runtime::{
	offchain::{
		testing::{PoolState, TestOffchainExt, TestTransactionPoolExt},
		OffchainDbExt, OffchainWorkerExt, TransactionPoolExt,
	},
	testing::UintAuthorityId,
	transaction_validity::{InvalidTransaction, TransactionSource, TransactionValidityError},
	BuildStorage,
};
use verifiable::{
	ring::{bandersnatch::BandersnatchVrfVerifiable, RingDomainSize},
	GenerateVerifiable,
};

pub type TransactionExtension = (AuthorizeCall<Test>, crate::extension::AsCoinage<Test>);

pub type Header = sp_runtime::generic::Header<u64, sp_runtime::traits::BlakeTwo256>;
pub type Block = sp_runtime::generic::Block<Header, Extrinsic>;
pub type Extrinsic = sp_runtime::generic::UncheckedExtrinsic<
	u64,
	RuntimeCall,
	UintAuthorityId,
	TransactionExtension,
>;

pub type Secret = <CryptoOf<Test> as verifiable::GenerateVerifiable>::Secret;
pub type Member = <CryptoOf<Test> as verifiable::GenerateVerifiable>::Member;
pub type Proof = <CryptoOf<Test> as verifiable::GenerateVerifiable>::Proof;

pub const ALICE: u64 = 1;
pub const BOB: u64 = 2;
pub const CHARLIE: u64 = 3;

/// Asset id used across mock test helpers. The pallet's `UnderlyingAssetId` storage is set to
/// this value inside [`setup_asset`] (and the benchmark helper) before any coinage operation.
pub const TEST_ASSET_ID: u32 = 10;

frame_support::construct_runtime!(
	pub enum Test {
		System: frame_system,
		Balances: pallet_balances,
		Assets: pallet_assets,
		AssetsHolder: pallet_assets_holder,
		ChunksManager: indiv_pallet_chunks_manager,
		Members: indiv_pallet_members,
		Coinage: crate,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Block = Block;
	type AccountData = pallet_balances::AccountData<u64>;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
	type AccountStore = System;
}

#[derive_impl(pallet_assets::config_preludes::TestDefaultConfig)]
impl pallet_assets::Config for Test {
	type Currency = Balances;
	type CreateOrigin = AsEnsureOriginWithArg<frame_system::EnsureSigned<u64>>;
	type ForceOrigin = frame_system::EnsureRoot<u64>;
	type Holder = AssetsHolder;
}

impl pallet_assets_holder::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeHoldReason = RuntimeHoldReason;
}

pub type AssetsWithHolder = CombineAssetsWithHolder<Assets, AssetsHolder>;

impl indiv_pallet_chunks_manager::Config for Test {
	type WeightInfo = ();
	type Chunk = <BandersnatchVrfVerifiable as GenerateVerifiable>::StaticChunk;
	type PageSize = ConstU32<1024>;
	type ManagerOrigin = frame_system::EnsureRoot<Self::AccountId>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ChunksBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct ChunksBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl
	indiv_pallet_chunks_manager::BenchmarkHelper<
		<BandersnatchVrfVerifiable as GenerateVerifiable>::StaticChunk,
	> for ChunksBenchmarkHelper
{
	fn chunk_page() -> Vec<<BandersnatchVrfVerifiable as GenerateVerifiable>::StaticChunk> {
		let chunks = indiv_support::genesis::ring_verifier_builder_params(
			verifiable::ring::RingDomainSize::Domain16,
		);
		chunks.into_iter().take(1024).collect()
	}
}

parameter_types! {
	pub const FlexibleRingExp: RingExponent = RingExponent::R2e9;
	pub const MockCollectionOwner: u32 = 1;
}

impl indiv_pallet_members::Config for Test {
	type WeightInfo = ();
	type Crypto = BandersnatchVrfVerifiable;
	type Location = u32;
	type ChunksManager = ChunksManager;
	type Clock = MockTime;
	type MaxCollections = ConstU32<20>;
	type OnboardingQueuePageSize = ConstU32<40>;
	type MaxFlexibleRingExponent = FlexibleRingExp;
	type RingBuildingMemberLimit = ConstU32<100>;
	type OldRootRetentionDuration = ConstU64<600>;
	type OnRingRootChange = ();
	type OffchainWorkerInterval = ConstU64<1>;
	type ManagerOrigin = frame_system::EnsureRoot<Self::AccountId>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
}

parameter_types! {
	pub const CoinagePalletId: PalletId = PalletId(*b"pop/coin");
	// Largest coin: 1000 * 128 (2^7) = 128,000
	pub storage MaximumExponent: i8 = 7;
	// Smallest coin: 1000 / 4 (2^2) = 250
	pub storage MinimumExponent: i8 = -2;
	pub const TestRecyclerRingExponent: RingExponent = RingExponent::R2e10;
	pub const TestPaidTokenRingExponent: RingExponent = RingExponent::R2e10;
	pub storage MinimumExponentForOutputUnloadFee: i8 = 0;
	pub storage TestUnderlyingAssetUnit: u64 = UNDERLYING_ASSET_UNIT;
}

pub const MAXIMUM_AGE: u16 = 20;
pub const UNDERLYING_ASSET_UNIT: u64 = 1_000;
// RingExponent::R2e10 currently yields a usable ring capacity of 767 members.
pub const R2E10_RING_CAPACITY: u32 = 767;
pub const MAX_SPLIT_OUTPUTS: u32 = 32;
pub const RECYCLER_EXPIRATION_TIME: u32 = 100;
pub const PAID_UNLOAD_TOKEN_RING_EXPIRATION_TIME: u32 = 200;
pub const UNLOAD_TOKEN_ALLOWANCE_PER_TIME_PERIOD_FOR_PEOPLE: u64 = 10;
pub const UNLOAD_TOKEN_ALLOWANCE_PER_TIME_PERIOD_FOR_LITE_PEOPLE: u64 = 4;
pub const UNLOAD_TOKEN_TIME_PERIOD_PEOPLE_LITE_PEOPLE: u32 = 100;
pub const MAX_FREE_UNLOAD_TOKENS_PER_TIME_PERIOD: u32 = 8;
pub const COIN_FAILURE_LOCK_PERIOD: u64 = 5;
pub const MAX_CONSOLIDATION: u32 = 16;
pub const MAX_BATCH_UNPAID_LOAD: u32 = 10;
pub const FEE_DESTINATION: u64 = 999;

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
		(AuthorizeCall::new(), crate::extension::AsCoinage::new(None))
	}
}

parameter_types! {
	/// Overrides the normal token fee that is calculated using weight.
	/// When `Some(fee)`, `MockWeightToFee` returns `fee * 2` (to account for the mock
	/// `ConversionToAssetBalance` which divides by 2). When `None`, falls back to weight's
	/// `ref_time()`.
	pub storage MockPaidUnloadTokenFeeOverride: Option<u64> = Some(2);
	/// Monotonic counter used to keep benchmark recycler setups distinct inside one externalities
	/// context.
	pub storage MockBatchVerifyCounter: u32 = 0;
}

pub struct MockWeightToFee;
impl sp_runtime::traits::Convert<frame_support::weights::Weight, u64> for MockWeightToFee {
	fn convert(w: frame_support::weights::Weight) -> u64 {
		if let Some(fee) = MockPaidUnloadTokenFeeOverride::get() {
			fee * 2
		} else {
			w.ref_time()
		}
	}
}

impl crate::Config for Test {
	type MemberService = Members;
	type CollectionOwner = MockCollectionOwner;
	type RecyclerRingExponent = TestRecyclerRingExponent;
	type PaidUnloadTokenRingExponent = TestPaidTokenRingExponent;
	type UnixTime = MockTime;
	type PalletId = CoinagePalletId;
	type WeightInfo = ();
	type MaximumAge = ConstU16<MAXIMUM_AGE>;
	type Fungibles = AssetsWithHolder;
	type NativeFungible = Balances;
	type UnderlyingAssetUnit = TestUnderlyingAssetUnit;
	type UnderlyingAssetIdManager = frame_system::EnsureRoot<u64>;
	type MinimumExponent = MinimumExponent;
	type MaximumExponent = MaximumExponent;
	type MinimumExponentForOutputUnloadFee = MinimumExponentForOutputUnloadFee;
	type LitePeopleProof = LitePeopleProof;
	type PeopleProof = PeopleProof;
	type MaxSplitOutputs = ConstU32<MAX_SPLIT_OUTPUTS>;
	type RecyclerExpirationTime = ConstU32<RECYCLER_EXPIRATION_TIME>;
	type UnloadTokenAllowancePerTimePeriodForPeople =
		ConstU64<UNLOAD_TOKEN_ALLOWANCE_PER_TIME_PERIOD_FOR_PEOPLE>;
	type UnloadTokenTimePeriodPeopleLitePeople =
		ConstU32<UNLOAD_TOKEN_TIME_PERIOD_PEOPLE_LITE_PEOPLE>;
	type UnloadTokenAllowancePerTimePeriodForLitePeople =
		ConstU64<UNLOAD_TOKEN_ALLOWANCE_PER_TIME_PERIOD_FOR_LITE_PEOPLE>;
	type MaxFreeUnloadTokensPerTimePeriod = ConstU32<MAX_FREE_UNLOAD_TOKENS_PER_TIME_PERIOD>;
	type MaxConsolidation = ConstU32<MAX_CONSOLIDATION>;
	type MaxBatchUnpaidLoad = ConstU32<MAX_BATCH_UNPAID_LOAD>;
	type PaidUnloadTokenRingExpirationTime = ConstU32<PAID_UNLOAD_TOKEN_RING_EXPIRATION_TIME>;
	type PaidUnloadTokenTimePeriod = ConstU32<100>;
	type ConversionToAssetBalance = TestConversionToAssetBalance;
	type WeightToFee = MockWeightToFee;
	type FeeDestination = ConstU64<FEE_DESTINATION>;
	type OffchainWorkerInterval = ConstU64<1>;
	type CoinFailureLockPeriod = ConstU64<COIN_FAILURE_LOCK_PERIOD>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = TestBenchmarkHelper;
}

thread_local! {
	/// When set to `Some(n)`, `TestConversionToAssetBalance` will fail starting from the
	/// n-th call (0-indexed). The counter resets each time this is set via
	/// `set_conversion_to_asset_fail_at`.
	static CONVERSION_TO_ASSET_FAIL_AT: Cell<Option<u32>> = const { Cell::new(None) };
	static CONVERSION_TO_ASSET_CALL_COUNT: Cell<u32> = const { Cell::new(0) };
}

/// Configure `TestConversionToAssetBalance` to fail starting from the n-th call (0-indexed).
/// Pass `None` to disable. Resets the internal call counter.
pub fn set_conversion_to_asset_fail_at(n: Option<u32>) {
	CONVERSION_TO_ASSET_FAIL_AT.set(n);
	CONVERSION_TO_ASSET_CALL_COUNT.set(0);
}

pub struct TestConversionToAssetBalance;
impl ConversionToAssetBalance<u64, u32, u64> for TestConversionToAssetBalance {
	type Error = DispatchError;
	fn to_asset_balance(balance: u64, _asset_id: u32) -> Result<u64, Self::Error> {
		if let Some(fail_at) = CONVERSION_TO_ASSET_FAIL_AT.get() {
			let count = CONVERSION_TO_ASSET_CALL_COUNT.with(|c| {
				let v = c.get();
				c.set(v + 1);
				v
			});
			if count >= fail_at {
				return Err(DispatchError::Other("conversion failed"));
			}
		}
		Ok(balance / 2)
	}
}

#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
pub struct LitePeopleProof {
	pub context: Vec<u8>,
	pub msg: Vec<u8>,
	pub alias: Alias,
}
impl ValidateProof for LitePeopleProof {
	type Proof = LitePeopleProof;
	fn validate_proof(proof: &Self::Proof, context: &[u8], msg: &[u8]) -> Result<Alias, ()> {
		if proof.context == context && proof.msg == msg {
			Ok(proof.alias)
		} else {
			Err(())
		}
	}
}

#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
pub struct PeopleProof {
	pub context: Vec<u8>,
	pub msg: Vec<u8>,
	pub alias: Alias,
}
impl ValidateProof for PeopleProof {
	type Proof = PeopleProof;
	fn validate_proof(proof: &Self::Proof, context: &[u8], msg: &[u8]) -> Result<Alias, ()> {
		if proof.context == context && proof.msg == msg {
			Ok(proof.alias)
		} else {
			Err(())
		}
	}
}

thread_local! {
	pub static TIME: RefCell<Duration> = RefCell::new(Duration::default());
}

pub struct MockTime;
impl UnixTime for MockTime {
	fn now() -> Duration {
		TIME.with(|t| *t.borrow())
	}
}

// Tests are executed in their own thread and only use one thread. This sets up a global variable
// for each test. If we ever need multi-threaded tests, this will need to be reworked.
thread_local! {
	static TRANSACTION_POOL: RefCell<Arc<parking_lot::RwLock<PoolState>>> =
		RefCell::new(Arc::new(parking_lot::RwLock::new(PoolState {
			transactions: Vec::new(),
		})));
}

/// Initialize chunks in the ChunksManager for ring-VRF operations.
/// Must be called before any ring operations (loading members, building, verifying).
pub fn initialize_chunks() {
	initialize_chunks_for_ring(FlexibleRingExp::get());
	initialize_chunks_for_ring(TestRecyclerRingExponent::get());
}

fn initialize_chunks_for_ring(ring_exp: RingExponent) {
	use verifiable::ring::RingDomainSize;

	let domain_size: RingDomainSize = ring_exp
		.try_into()
		.expect("mock ring exponent should convert to a ring domain size");
	let chunks = indiv_support::genesis::ring_verifier_builder_params(domain_size);

	let page_size = 1024usize; // matches ConstU32<1024> PageSize
	for (page_idx, chunk_page) in chunks.chunks(page_size).enumerate() {
		let bounded: BoundedVec<_, _> = chunk_page
			.iter()
			.cloned()
			.map(indiv_pallet_chunks_manager::UncheckedChunk)
			.collect::<Vec<_>>()
			.try_into()
			.expect("page size matches");
		indiv_pallet_chunks_manager::Chunks::<Test>::insert(ring_exp, page_idx as u32, bounded);
	}
}

/// Default test externalities: matches production where `UnderlyingAssetId` is set by
/// governance before any Coinage extrinsic is admitted. Use [`new_test_ext_no_asset_id`]
/// for tests that exercise the unset state.
#[allow(dead_code)]
pub fn new_test_ext() -> sp_io::TestExternalities {
	let mut ext = new_test_ext_no_asset_id();
	ext.execute_with(setup_asset);
	ext
}

/// Test externalities without `UnderlyingAssetId` set — for tests that explicitly exercise
/// the pre-governance state (e.g. the setter's own tests, "asset id unset" rejection tests).
#[allow(dead_code)]
pub fn new_test_ext_no_asset_id() -> sp_io::TestExternalities {
	let storage = RuntimeGenesisConfig::default().build_storage().unwrap();

	let mut ext: sp_io::TestExternalities = storage.into();
	let (offchain, _state) = TestOffchainExt::new();
	let (pool, state) = TestTransactionPoolExt::new();
	TRANSACTION_POOL.set(state);
	ext.register_extension(OffchainDbExt::new(offchain.clone()));
	ext.register_extension(OffchainWorkerExt::new(offchain));
	ext.register_extension(TransactionPoolExt::new(pool));
	ext.execute_with(initialize_chunks);
	ext
}

/// Test externalities for benchmarks that require the underlying asset to exist.
#[cfg(feature = "runtime-benchmarks")]
pub fn new_test_ext_bench() -> sp_io::TestExternalities {
	new_test_ext()
}

/// Advance the chain to `target_block`.
pub fn advance_to_block(target_block: frame_system::pallet_prelude::BlockNumberFor<Test>) {
	loop {
		let current = frame_system::Pallet::<Test>::block_number();
		if current >= target_block {
			break;
		}

		// Execute offchain worker.
		AllPalletsWithSystem::offchain_worker(current);

		// Advance time by 2 seconds (2000 ms).
		TIME.with(|t| {
			*t.borrow_mut() += Duration::from_millis(2_000);
		});

		// Advance block number by 1.
		let next = current.saturating_add(1u64);
		frame_system::Pallet::<Test>::initialize(&next, &Default::default(), &Default::default());

		// Run transactions submitted by the offchain worker.
		let transactions = {
			TRANSACTION_POOL.with_borrow_mut(|pool| std::mem::take(&mut pool.write().transactions))
		};
		for tx in transactions {
			let tx = Decode::decode(&mut &tx[..]).unwrap();
			Executive::apply_extrinsic(tx).expect("tx valid").expect("tx succeeds");
		}
	}
}

/// Executive: handles dispatch to the various modules.
pub type Executive = frame_executive::Executive<
	Test,
	Block,
	frame_system::ChainContext<Test>,
	Test,
	AllPalletsWithSystem,
	(),
>;

/// Advance exactly one block.
#[allow(dead_code)]
pub fn advance_block() {
	let next_block = frame_system::Pallet::<Test>::block_number().saturating_add(1u64);
	advance_to_block(next_block);
}

/// Advance until the on-chain Unix time (seconds) reaches or exceeds `target_s`.
#[allow(dead_code)]
pub fn advance_until_time(target_s: u32) {
	loop {
		let now_s: u64 = MockTime::now().as_secs();
		// Fixed comparison: now_s is seconds, target_s is seconds. Removed * 1000.
		if now_s >= target_s as u64 {
			break;
		}
		let next_block = frame_system::Pallet::<Test>::block_number().saturating_add(1u64);
		advance_to_block(next_block);
	}
}

pub fn get_u16<T: Get<u16>>() -> u16 {
	T::get()
}
pub fn get_u32<T: Get<u32>>() -> u32 {
	T::get()
}
pub fn get_u64<T: Get<u64>>() -> u64 {
	T::get()
}
#[allow(unused)]
pub fn get_i8<T: Get<i8>>() -> i8 {
	T::get()
}

/// Checks that the transaction fails validation with the expected custom invalidity.
#[track_caller]
pub fn assert_invalid(ext: Extrinsic, error: CustomInvalidity) {
	// fails validation
	let res = Executive::validate_transaction(
		TransactionSource::External,
		ext.clone(),
		Default::default(),
	);
	assert_eq!(
		res,
		Err(TransactionValidityError::Invalid(InvalidTransaction::Custom(error.clone() as u8)))
	);

	// fails validation even if it would have been applied
	let res = Executive::apply_extrinsic(ext);
	assert_eq!(
		res,
		Err(TransactionValidityError::Invalid(InvalidTransaction::Custom(error as u8)))
	);
}

/// Helper to build an extrinsic.
///
/// If `as_coin` is true, the extension is configured with `AsCoinageInfo::AsCoin`.
/// Otherwise, it is configured with `None` (passthrough), which should result in a BadOrigin
/// when the pallet expects a Coin origin.
pub fn build_signed_as_coin_ext(signer: u64, call: crate::Call<Test>, as_coin: bool) -> Extrinsic {
	let info = if as_coin { Some(AsCoinageInfo::AsCoin) } else { None };
	let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(info));
	Extrinsic::new_signed(call.into(), signer, UintAuthorityId(signer), extension)
}

thread_local! {
	static UNIQUE_SECRET_COUNTER: std::cell::Cell<usize> = const { std::cell::Cell::new(10000) };
}

/// Create a unique secret.
pub fn get_unique_secret() -> Secret {
	let seed = UNIQUE_SECRET_COUNTER.with(|c| {
		let v = c.get();
		c.set(v + 1);
		v
	});
	CryptoOf::<Test>::new_secret(seed.to_le_bytes().repeat(4).try_into().unwrap())
}

/// Create a secret from a seed byte.
pub fn get_secret(seed: u8) -> Secret {
	CryptoOf::<Test>::new_secret([seed; 32])
}

/// Force-create [`TEST_ASSET_ID`] in `pallet-assets` if it doesn't yet exist, and write it
/// into the coinage pallet's `UnderlyingAssetId` storage if unset.
///
/// Idempotent. Every test helper that touches coinage operations must go through this so the
/// pallet's "asset id must be set by governance" contract is satisfied in one place.
fn ensure_underlying_asset_id_set() {
	if !pallet_assets::Asset::<Test>::contains_key(TEST_ASSET_ID) {
		assert_ok!(Assets::force_create(RuntimeOrigin::root(), TEST_ASSET_ID, ALICE, true, 1));
	}
	if !crate::UnderlyingAssetId::<Test>::exists() {
		crate::UnderlyingAssetId::<Test>::put(TEST_ASSET_ID);
	}
}

/// Create a coin for an owner, ensuring asset backing is correct.
/// Mints asset to owner, transfers to pallet hold, inserts coin.
pub fn create_coin(owner: u64, value: CoinValue, age: u16) {
	let amount = Coinage::coin_value_to_asset_amount(value).unwrap();
	let asset_id = TEST_ASSET_ID;

	ensure_underlying_asset_id_set();

	// Mint to owner
	assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), asset_id, owner, amount));

	// Transfer and hold to pallet
	assert_ok!(AssetsWithHolder::transfer_and_hold(
		asset_id,
		&crate::pallet::HoldReason::Wrapped.into(),
		&owner,
		&Coinage::pallet_account(),
		amount,
		frame_support::traits::tokens::Precision::Exact,
		frame_support::traits::tokens::Preservation::Expendable,
		frame_support::traits::tokens::Fortitude::Polite,
	));

	// Insert coin
	CoinsByOwner::<Test>::insert(owner, crate::pallet::Coin { value, age });
}

/// Setup a recycler with `count` members loaded.
/// Returns the secrets and the ring index and revision.
/// `seed_offset` allows creating multiple recyclers in the same test with different seeds.
///
/// This ensures assets are minted and backed properly by calling the pallet extrinsic.
/// After loading, triggers `Members::process_maintenance()` to build the ring.
pub fn setup_recycler(
	value: CoinValue,
	count: u32,
	seed_offset: u8,
) -> (Vec<Secret>, RingIndex, RevisionIndex) {
	let mut secrets = Vec::new();
	let asset_id = TEST_ASSET_ID;
	let amount = Coinage::coin_value_to_asset_amount(value).unwrap();

	ensure_underlying_asset_id_set();

	for i in 0..count {
		let user = 10000 + (seed_offset as u64) * 100 + i as u64;
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), asset_id, user, amount));

		let secret = get_unique_secret();
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

		assert_ok!(Coinage::load_recycler_with_external_asset(
			RuntimeOrigin::signed(user),
			crate::pallet::CodecPreservation::Expendable,
			value,
			member,
			proof
		));
		secrets.push(secret);
	}

	// Trigger pallet-members to build rings from the onboarding queue.
	Members::process_maintenance();

	// Find the ring index and revision for the loaded members.
	let identifier = Coinage::recycler_collection_identifier(value);
	// The first member should be in ring 0 after building.
	let ring_index = 0u32;
	let revision =
		<Test as Config>::MemberService::ring_revision(&identifier, ring_index).unwrap_or(0);

	(secrets, ring_index, revision)
}

/// Setup the underlying external asset for tests.
///
/// Also populates the pallet's [`crate::UnderlyingAssetId`] storage with [`TEST_ASSET_ID`] so
/// that coin operations don't bail with `Error::AssetIdNotSet`.
pub fn setup_asset() {
	ensure_underlying_asset_id_set();
}

/// Setup balances for accounts (native and external asset).
/// Used by non-anonymous unload tests.
pub fn setup_balances() {
	// Give accounts some native balance
	Balances::make_free_balance_be(&ALICE, 10_000);
	Balances::make_free_balance_be(&BOB, 10_000);
	Balances::make_free_balance_be(&CHARLIE, 10_000);
	Balances::make_free_balance_be(&FEE_DESTINATION, 100);

	// Create the underlying asset
	setup_asset();

	let asset_id = TEST_ASSET_ID;
	// Mint external assets to accounts
	assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), asset_id, ALICE, 100_000));
	assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), asset_id, BOB, 100_000));
	assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), asset_id, CHARLIE, 100_000));
	assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), asset_id, FEE_DESTINATION, 1_000));

	// Mint to pallet account for holding (extra for ED)
	let pallet_account = Coinage::pallet_account();
	assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), asset_id, pallet_account, 200_000));

	// Hold assets on pallet account (simulates loaded recyclers)
	assert_ok!(AssetsWithHolder::hold(
		asset_id,
		&HoldReason::Wrapped.into(),
		&pallet_account,
		100_000,
	));
}

pub fn fund_native(who: u64, amount: u64) {
	let _ = Balances::mint_into(&who, amount);
}

/// Check the accounting invariant:
/// Total Funds (Held) == Coins + Active Recyclers (Net) + Destroyed
#[track_caller]
pub fn check_accounting() {
	let asset_id = TEST_ASSET_ID;
	let pallet_acc = Coinage::pallet_account();

	// Total funds held by the pallet account (User funds are always held)
	let total_held =
		AssetsWithHolder::balance_on_hold(asset_id, &HoldReason::Wrapped.into(), &pallet_acc);

	// 1. Value of all Coins
	let coins_value: u64 = CoinsByOwner::<Test>::iter_values()
		.map(|c| Coinage::coin_value_to_asset_amount(c.value).unwrap())
		.sum();

	// 2. Net Value of Active Recyclers
	// Count members in recycler collections via MemberService.
	let mut recyclers_value: u64 = 0;
	for (value, ()) in RecyclerCollectionCreated::<Test>::iter() {
		let unit = crate::Pallet::<Test>::coin_value_to_asset_amount(value).unwrap();
		let identifier = Coinage::recycler_collection_identifier(value);
		let active = <Test as Config>::MemberService::active_count(&identifier) as u64;
		// Subtract unloaded aliases for all rings.
		// For simplicity, count total unloaded across all rings for this value.
		let unloaded: u64 = RecyclersUnloaded::<Test>::iter_prefix((value,)).count() as u64;
		let net_members = active.saturating_sub(unloaded);
		recyclers_value += net_members * unit;
	}

	// 3. Total Value of Destroyed Coins
	let destroyed_value = TotalValueOfDestroyedCoins::<Test>::get();

	let total_in_pallet = coins_value + recyclers_value + destroyed_value;

	assert_eq!(
		total_held, total_in_pallet,
		"Invariant violation: Held {total_held} != In pallet {total_in_pallet} \n\
		detail: Coins: {coins_value}, Recyclers: {recyclers_value}, Destroyed: {destroyed_value}",
	);
}

pub fn recycler_ring_domain_size() -> RingDomainSize {
	<Test as crate::Config>::RecyclerRingExponent::get()
		.try_into()
		.expect("mock recycler ring exponent should map to a ring domain size")
}

pub fn recycler_ring_size() -> <CryptoOf<Test> as GenerateVerifiable>::Config {
	recycler_ring_domain_size().into()
}

pub fn paid_token_ring_domain_size() -> RingDomainSize {
	<Test as crate::Config>::PaidUnloadTokenRingExponent::get()
		.try_into()
		.expect("mock paid token ring exponent should map to a ring domain size")
}

pub fn paid_token_ring_size() -> <CryptoOf<Test> as GenerateVerifiable>::Config {
	paid_token_ring_domain_size().into()
}

/// Create a proof for unloading from a recycler.
pub fn create_unload_proof(
	secret: &Secret,
	members: &[MemberOf<Test>],
	proven_msg: &[u8; 32],
) -> (Proof, indiv_support::traits::Alias) {
	let member = CryptoOf::<Test>::member_from_secret(secret);
	let commitment = CryptoOf::<Test>::open(recycler_ring_size(), &member, members.iter().cloned())
		.expect("should open");
	CryptoOf::<Test>::create(
		commitment,
		secret,
		UNLOADING_RECYCLER_CONTEXT.as_ref(),
		proven_msg.as_ref(),
	)
	.expect("should create proof")
}

/// Build an unsigned extrinsic with `AsUnloadTokenFromOutput` extension.
/// Computes the correct inherited_implication and creates proofs that sign over it.
pub fn build_unload_from_output_ext(
	call: crate::Call<Test>,
	fee_recycler_value: CoinValue,
	fee_recycler_index: RingIndex,
	fee_recycler_revision: RevisionIndex,
	secrets: &[Secret],
) -> Extrinsic {
	let runtime_call: RuntimeCall = call.into();

	let inherited_implication = ((0u8, &runtime_call), (), ());
	let proven_msg = sp_io::hashing::blake2_256(&inherited_implication.encode());

	let ring_members = Coinage::get_recycler_members(fee_recycler_value, fee_recycler_index);

	// Create the other alias proofs (all secrets except the first).
	let mut other_alias_proofs_vec = Vec::new();
	for secret in &secrets[1..] {
		let member = CryptoOf::<Test>::member_from_secret(secret);
		let commitment =
			CryptoOf::<Test>::open(recycler_ring_size(), &member, ring_members.clone().into_iter())
				.expect("should open");
		let (proof, _alias) = CryptoOf::<Test>::create(
			commitment,
			secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
			&proven_msg,
		)
		.expect("should create proof");
		other_alias_proofs_vec.push(proof);
	}

	// Create first alias proof signing (other_alias_proofs ++ inherited_implication)
	let intent_msg = sp_io::hashing::blake2_256(
		&[other_alias_proofs_vec.encode(), inherited_implication.encode()].concat(),
	);
	let first_member = CryptoOf::<Test>::member_from_secret(&secrets[0]);
	let first_commitment =
		CryptoOf::<Test>::open(recycler_ring_size(), &first_member, ring_members.into_iter())
			.expect("should open");
	let (first_alias_proof, _alias) = CryptoOf::<Test>::create(
		first_commitment,
		&secrets[0],
		UNLOADING_RECYCLER_CONTEXT.as_ref(),
		&intent_msg,
	)
	.expect("should create first alias proof");

	let mut alias_proofs_vec = vec![first_alias_proof];
	alias_proofs_vec.extend(other_alias_proofs_vec);

	let info = Some(AsCoinageInfo::AsUnloadTokenFromOutput {
		fee_recycler_value,
		fee_recycler_index,
		fee_recycler_revision,
		alias_proofs: alias_proofs_vec.try_into().expect("proofs should fit in bounds"),
	});
	let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(info));
	Extrinsic::new_transaction(runtime_call, extension)
}

/// Helper to build the unload extrinsic using a Paid Unload Token.
pub fn build_unload_paid_ext(
	call: crate::Call<Test>,
	paid_token_secret: &Secret,
	paid_token_ring_index: u32,
	paid_token_ring_revision: u32,
	period: u32,
	recycler_secrets: &[Secret],
	value: CoinValue,
	index: u32,
) -> Extrinsic {
	let runtime_call: RuntimeCall = call.into();

	// 1. Calculate Implication
	let inherited_implication = ((0u8, &runtime_call), (), ());
	let proven_msg = sp_io::hashing::blake2_256(&inherited_implication.encode());

	// 2. Generate Alias Proofs (must be created before the paid token proof so we can include them
	//    in the intent message)
	let mut alias_proofs_vec = Vec::new();
	let ring_members = Coinage::get_recycler_members(value, index);

	for secret in recycler_secrets {
		let (proof, _) = create_unload_proof(secret, &ring_members, &proven_msg);
		alias_proofs_vec.push(proof);
	}

	let alias_proofs = BoundedVec::try_from(alias_proofs_vec).unwrap();

	// 3. Generate Paid Unload Token Proof with intent message
	let intent_msg = sp_io::hashing::blake2_256(
		&[alias_proofs.encode(), inherited_implication.encode()].concat(),
	);

	let mut paid_token_context = [0u8; 32];
	paid_token_context[..28].copy_from_slice(PAID_UNLOAD_TOKEN_CONTEXT_BASE.as_ref());
	paid_token_context[28..32].copy_from_slice(&period.to_le_bytes());

	let paid_token_member = CryptoOf::<Test>::member_from_secret(paid_token_secret);
	let paid_token_members = Coinage::get_paid_token_ring_members(period, paid_token_ring_index);

	let commitment = CryptoOf::<Test>::open(
		paid_token_ring_size(),
		&paid_token_member,
		paid_token_members.into_iter(),
	)
	.expect("Paid token member should be in the ring");

	let (paid_token_proof, _) =
		CryptoOf::<Test>::create(commitment, paid_token_secret, &paid_token_context, &intent_msg)
			.unwrap();

	let info = AsCoinageInfo::AsUnloadTokenPaid {
		proof: paid_token_proof,
		period,
		paid_token_ring_index,
		paid_token_ring_revision,
		alias_proofs,
	};

	let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(info)));
	Extrinsic::new_signed(runtime_call, 0, UintAuthorityId(0), extension)
}

/// Build a signed extrinsic for non-anonymous unload calls.
/// The signer pays the fee, and the extension is configured with None (passthrough).
pub fn build_signed_ext(signer: u64, call: crate::Call<Test>) -> Extrinsic {
	let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(None));
	Extrinsic::new_signed(call.into(), signer, UintAuthorityId(signer), extension)
}

/// Helper to build an authorized extrinsic (for clean_* calls).
pub fn build_authorized_ext(call: crate::Call<Test>) -> Extrinsic {
	let runtime_call: RuntimeCall = call.into();
	let extension = <Test as CreateAuthorizedTransaction<crate::Call<Test>>>::create_extension();
	Extrinsic::new_transaction(runtime_call, extension)
}

#[cfg(feature = "runtime-benchmarks")]
pub struct TestBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl crate::BenchmarkHelper<Test> for TestBenchmarkHelper {
	fn setup_assets() {
		ensure_underlying_asset_id_set();
	}

	fn fund_account(who: &u64, amount: u64) {
		use frame_support::{assert_ok, traits::fungibles::Mutate};
		let asset_id = TEST_ASSET_ID;
		assert_ok!(AssetsWithHolder::mint_into(asset_id, who, amount));
	}

	fn set_time(now: core::time::Duration) {
		TIME.with(|t| {
			*t.borrow_mut() = now;
		});
	}

	fn setup_conversion_rate() {
		// TestConversionFromAssetBalance always succeeds, no setup needed
	}

	fn create_people_proof(
		context: &[u8],
		msg: &[u8],
		alias: indiv_support::traits::Alias,
	) -> PeopleProof {
		PeopleProof { context: context.to_vec(), msg: msg.to_vec(), alias }
	}

	fn create_lite_people_proof(
		context: &[u8],
		msg: &[u8],
		alias: indiv_support::traits::Alias,
	) -> LitePeopleProof {
		LitePeopleProof { context: context.to_vec(), msg: msg.to_vec(), alias }
	}

	fn setup_batch_verify(
		count: u32,
	) -> Result<
		(
			CoinValue,
			indiv_support::traits::RingIndex,
			Vec<indiv_support::traits::Alias>,
			Vec<ProofOf<Test>>,
			[u8; 32],
		),
		frame_benchmarking::BenchmarkError,
	> {
		// Keep benchmark setups distinct inside one externalities context because the benchmark
		// runner may call this helper multiple times with different proof counts.
		let offset = MockBatchVerifyCounter::get();
		MockBatchVerifyCounter::set(&offset.saturating_add(1));
		let min = <Test as crate::Config>::MinimumExponent::get();
		let max = <Test as crate::Config>::MaximumExponent::get();
		let value_span = u32::try_from(i16::from(max) - i16::from(min) + 1)
			.map_err(|_| frame_benchmarking::BenchmarkError::Weightless)?;
		let value_offset = i16::try_from(offset % value_span)
			.map_err(|_| frame_benchmarking::BenchmarkError::Weightless)?;
		let value = i8::try_from(i16::from(min) + value_offset)
			.map_err(|_| frame_benchmarking::BenchmarkError::Weightless)?;
		let seed_offset = u8::try_from(offset % (u32::from(u8::MAX) + 1))
			.map_err(|_| frame_benchmarking::BenchmarkError::Weightless)?;
		let (secrets, ring_index, _revision) = setup_recycler(value, count, seed_offset);
		let identifier = Coinage::recycler_collection_identifier(value);
		let ring_members = Coinage::get_recycler_members(value, ring_index);
		let ring_exp = <Test as crate::Config>::RecyclerRingExponent::get();
		let capacity = <CryptoOf<Test> as GenerateVerifiable>::Config::try_from(ring_exp)
			.map_err(|_| frame_benchmarking::BenchmarkError::Weightless)?;
		if <Test as Config>::MemberService::ring_revision(&identifier, ring_index).is_none() {
			use verifiable::ring::RingDomainSize;

			let domain_size: RingDomainSize = ring_exp
				.try_into()
				.map_err(|_| frame_benchmarking::BenchmarkError::Weightless)?;
			let builder_params: Vec<<CryptoOf<Test> as GenerateVerifiable>::StaticChunk> =
				indiv_support::genesis::ring_verifier_builder_params(domain_size);
			let get_many = |range| {
				builder_params
					.get(range)
					.map(|chunks: &[<CryptoOf<Test> as GenerateVerifiable>::StaticChunk]| {
						chunks.to_vec()
					})
					.ok_or(())
			};
			let mut intermediate = CryptoOf::<Test>::start_members(capacity);
			CryptoOf::<Test>::push_members(
				&mut intermediate,
				ring_members.iter().copied(),
				get_many,
			)
			.map_err(|_| frame_benchmarking::BenchmarkError::Weightless)?;
			let root = CryptoOf::<Test>::finish_members(intermediate.clone());
			indiv_pallet_members::Root::<Test>::insert(
				identifier,
				ring_index,
				indiv_pallet_members::types::RingRoot::<Test> { root, revision: 0, intermediate },
			);
		}

		let proven_msg = [42u8; 32];
		let mut aliases = Vec::new();
		let mut proofs = Vec::new();

		for secret in &secrets {
			let member = CryptoOf::<Test>::member_from_secret(secret);
			let commitment =
				CryptoOf::<Test>::open(capacity, &member, ring_members.clone().into_iter())
					.map_err(|_| frame_benchmarking::BenchmarkError::Weightless)?;

			let (proof, alias) = CryptoOf::<Test>::create(
				commitment,
				secret,
				crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
				&proven_msg,
			)
			.map_err(|_| frame_benchmarking::BenchmarkError::Weightless)?;

			aliases.push(alias);
			proofs.push(proof);
		}

		Ok((value, ring_index, aliases, proofs, proven_msg))
	}
}
