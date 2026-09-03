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
	extension::{AsCoinage, AsCoinageInfo, FreeTokenKind},
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
		fungible::{HoldConsideration, Mutate as _},
		fungibles::{self, Inspect, InspectHold, MutateHold},
		tokens::Preservation,
		AsEnsureOriginWithArg, ConstU32, ConstU64, ConstantStoragePrice, Currency, OffchainWorker,
		UnixTime,
	},
	BoundedVec,
};
use frame_system::{
	offchain::{CreateAuthorizedTransaction, CreateTransaction, CreateTransactionBase},
	AuthorizeCall,
};
use indiv_support::{
	crypto::{BandersnatchVrfVerifiable, GenerateVerifiable},
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
use verifiable::ring::RingDomainSize;

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

/// Asset id used across mock test helpers. The coinage instance wrapping it is created inside
/// [`setup_asset`] (and the benchmark helper) before any coinage operation.
pub const TEST_ASSET_ID: u32 = 10;

/// The instance wrapping [`TEST_ASSET_ID`], which every mock test helper operates on.
pub const TEST_INSTANCE_ID: InstanceId = 0;

/// Base id for the extra assets of `BenchmarkHelper::create_extra_asset` and the deposit
/// currencies derived from them.
pub const EXTRA_ASSET_ID_BASE: u32 = 1_000;

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

/// The [`crate::Config::Fungibles`] id of the native token in tests, usable both as a deposit
/// currency and as the asset an instance's coins wrap.
pub const NATIVE_DEPOSIT_ID: u32 = u32::MAX;

/// Criterion routing [`NATIVE_DEPOSIT_ID`] to `Balances` and every other id to the assets.
pub struct NativeFromDepositId;
impl sp_runtime::traits::Convert<u32, sp_runtime::Either<(), u32>> for NativeFromDepositId {
	fn convert(id: u32) -> sp_runtime::Either<(), u32> {
		if id == NATIVE_DEPOSIT_ID {
			sp_runtime::Either::Left(())
		} else {
			sp_runtime::Either::Right(id)
		}
	}
}

/// Native plus assets, the mock's [`crate::Config::Fungibles`].
pub type NativeAndAssets = frame_support::traits::fungible::UnionOf<
	Balances,
	AssetsWithHolder,
	NativeFromDepositId,
	u32,
	u64,
>;

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
}

impl indiv_pallet_members::Config for Test {
	type WeightInfo = ();
	type Crypto = BandersnatchVrfVerifiable;
	type Location = xcm::v5::Location;
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
	/// The native amount [`InstanceCreationDeposit`] takes per instance, whatever the footprint.
	pub storage InstanceCreationDepositAmount: u64 = 100;
	pub const CoinageInstanceCreationHoldReason: RuntimeHoldReason =
		RuntimeHoldReason::Coinage(crate::HoldReason::InstanceCreationDeposit);
	/// The chain-wide load deposit, changed at will by [`set_load_deposit`].
	pub storage LoadDeposit: (u32, u64) = (NATIVE_DEPOSIT_ID, 10);
	/// Whether sponsored instances can be created, flipped by [`set_enable_permissionless`].
	pub storage EnablePermissionless: bool = true;
}

/// The mock's [`crate::Config::InstanceCreationDeposit`]: a native hold of a fixed amount,
/// ignoring the footprint so tests can predict it from [`InstanceCreationDepositAmount`].
pub type InstanceCreationDeposit = HoldConsideration<
	u64,
	Balances,
	CoinageInstanceCreationHoldReason,
	ConstantStoragePrice<InstanceCreationDepositAmount, u64>,
>;

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
	/// When `Some(fee)`, `MockWeightToFee` returns it. When `None`, falls back to weight's
	/// `ref_time()`.
	pub storage MockPaidUnloadTokenFeeOverride: Option<u64> = Some(2);
	/// Monotonic counter used to keep benchmark recycler setups distinct inside one externalities
	/// context.
	pub storage MockBatchVerifyCounter: u32 = 0;
	pub storage MaximumAge: u16 = MAXIMUM_AGE;
	pub storage UnloadTokenAllowancePerTimePeriodForPeople: u64 =
		UNLOAD_TOKEN_ALLOWANCE_PER_TIME_PERIOD_FOR_PEOPLE;
	pub storage UnloadTokenAllowancePerTimePeriodForLitePeople: u64 =
		UNLOAD_TOKEN_ALLOWANCE_PER_TIME_PERIOD_FOR_LITE_PEOPLE;
	pub storage MaxFreeUnloadTokensPerTimePeriod: u32 = MAX_FREE_UNLOAD_TOKENS_PER_TIME_PERIOD;
}

pub struct MockWeightToFee;
impl sp_runtime::traits::Convert<frame_support::weights::Weight, u64> for MockWeightToFee {
	fn convert(w: frame_support::weights::Weight) -> u64 {
		if let Some(fee) = MockPaidUnloadTokenFeeOverride::get() {
			fee
		} else {
			w.ref_time()
		}
	}
}

impl crate::Config for Test {
	type MemberService = Members;
	type RecyclerRingExponent = TestRecyclerRingExponent;
	type PaidUnloadTokenRingExponent = TestPaidTokenRingExponent;
	type UnixTime = MockTime;
	type PalletId = CoinagePalletId;
	type WeightInfo = ();
	type MaximumAge = MaximumAge;
	type Fungibles = NativeAndAssets;
	type NativeFungible = Balances;
	type AdminOrigin = frame_system::EnsureRoot<u64>;
	type SponsorOrigin = frame_system::EnsureSigned<u64>;
	type EnablePermissionless = EnablePermissionless;
	type LoadDeposit = LoadDeposit;
	type InstanceCreationDeposit = InstanceCreationDeposit;
	type MinimumExponent = MinimumExponent;
	type MaximumExponent = MaximumExponent;
	type MinimumExponentForOutputUnloadFee = MinimumExponentForOutputUnloadFee;
	type MembershipProof = MembershipProof;
	type MaxSplitOutputs = ConstU32<MAX_SPLIT_OUTPUTS>;
	type RecyclerExpirationTime = ConstU32<RECYCLER_EXPIRATION_TIME>;
	type UnloadTokenAllowancePerTimePeriodForPeople = UnloadTokenAllowancePerTimePeriodForPeople;
	type UnloadTokenTimePeriodPeopleLitePeople =
		ConstU32<UNLOAD_TOKEN_TIME_PERIOD_PEOPLE_LITE_PEOPLE>;
	type UnloadTokenAllowancePerTimePeriodForLitePeople =
		UnloadTokenAllowancePerTimePeriodForLitePeople;
	type MaxFreeUnloadTokensPerTimePeriod = MaxFreeUnloadTokensPerTimePeriod;
	type MaxConsolidation = ConstU32<MAX_CONSOLIDATION>;
	type MaxBatchUnpaidLoad = ConstU32<MAX_BATCH_UNPAID_LOAD>;
	type PaidUnloadTokenRingExpirationTime = ConstU32<PAID_UNLOAD_TOKEN_RING_EXPIRATION_TIME>;
	type PaidUnloadTokenTimePeriod = ConstU32<100>;
	type FeeConversion = TestFeeConversion;
	type NativeAssetKind = NativeAssetKind;
	type WeightToFee = MockWeightToFee;
	type FeeDestination = ConstU64<FEE_DESTINATION>;
	type OffchainWorkerInterval = ConstU64<1>;
	type CoinFailureLockPeriod = ConstU64<COIN_FAILURE_LOCK_PERIOD>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = TestBenchmarkHelper;
}

thread_local! {
	/// When set to `Some(n)`, [`TestFeeConversion`] reports the conversion as unavailable starting
	/// from the n-th quote (0-indexed). The counter resets each time this is set via
	/// `set_fee_conversion_unavailable_at`.
	static FEE_CONVERSION_FAIL_AT: Cell<Option<u32>> = const { Cell::new(None) };
	static FEE_CONVERSION_QUOTE_COUNT: Cell<u32> = const { Cell::new(0) };
	/// When set to `Some((n, surcharge))`, [`TestFeeConversion`] quotes `surcharge` above its normal
	/// rate from the n-th quote (0-indexed) on, so that tests can move the price between the quote
	/// validation takes and the one the dispatch takes. Set via
	/// `set_fee_conversion_quote_surcharge_at`.
	static FEE_CONVERSION_QUOTE_SURCHARGE_AT: Cell<Option<(u32, u64)>> = const { Cell::new(None) };
	/// How much less of the asset a swap takes than the quote said it would, so that tests can
	/// reach the paths the pallet keeps for a market that undercharges a quote.
	static FEE_CONVERSION_SWAP_DISCOUNT: Cell<u64> = const { Cell::new(0) };
	/// How much more of the asset a swap takes than the quote said it would, so that tests can
	/// reach the dispatch failure of a market that moved past the bound the quote set.
	static FEE_CONVERSION_SWAP_SURCHARGE: Cell<u64> = const { Cell::new(0) };
	/// When set, the market prices an asset against its own reserve of it, as a real pool does:
	/// whatever grows that reserve between a quote and the swap the quote bounds makes the swap
	/// cost more than the quote said.
	static FEE_CONVERSION_RESERVE_PRICING: Cell<bool> = const { Cell::new(false) };
}

/// Configure [`TestFeeConversion`] to report the conversion as unavailable starting from the n-th
/// quote (0-indexed). Pass `None` to disable. Resets the internal quote counter.
pub fn set_fee_conversion_unavailable_at(n: Option<u32>) {
	FEE_CONVERSION_FAIL_AT.set(n);
	FEE_CONVERSION_QUOTE_COUNT.set(0);
}

/// Configure [`TestFeeConversion`] to quote `surcharge` above its normal rate from the n-th quote
/// (0-indexed) on, so that the price validation quoted has moved by the time the dispatch quotes it
/// again. Pass `None` to disable. Resets the internal quote counter.
///
/// Only quotes are surcharged, not swaps: the paths this reaches reject the quote before swapping
/// on it. Use [`set_fee_conversion_swap_surcharge`] for a swap that costs more than its quote.
pub fn set_fee_conversion_quote_surcharge_at(n_and_surcharge: Option<(u32, u64)>) {
	FEE_CONVERSION_QUOTE_SURCHARGE_AT.set(n_and_surcharge);
	FEE_CONVERSION_QUOTE_COUNT.set(0);
}

/// Make [`TestFeeConversion`]'s swaps take `discount` less of the asset than their quote, leaving
/// the payer with asset the pallet has already set aside for the fee. Pass 0 to disable.
pub fn set_fee_conversion_swap_discount(discount: u64) {
	FEE_CONVERSION_SWAP_DISCOUNT.set(discount);
}

/// Make [`TestFeeConversion`]'s swaps take `surcharge` more of the asset than their quote, so the
/// bound the quote set no longer covers them and the swap fails. Pass 0 to disable.
pub fn set_fee_conversion_swap_surcharge(surcharge: u64) {
	FEE_CONVERSION_SWAP_SURCHARGE.set(surcharge);
}

/// Make [`TestFeeConversion`] price an asset against the market's own reserve of it, so that a
/// quote taken before the reserve grows no longer covers the swap it bounds. Pass `false` for the
/// fixed price.
pub fn set_fee_conversion_reserve_pricing(enabled: bool) {
	FEE_CONVERSION_RESERVE_PRICING.set(enabled);
}

/// Stands in for the market the asset is converted through: it receives the asset the payer gives
/// up, and the native it hands out is minted, so it has unlimited depth and a fixed price.
pub const MOCK_MARKET: u64 = 998;

/// Turns the market's reserve of an asset into the surcharge it prices that asset at, under
/// [`set_fee_conversion_reserve_pricing`].
const RESERVE_PRICE_DIVISOR: u64 = 4;

pub struct TestFeeConversion;

impl TestFeeConversion {
	/// One unit of native costs one unit of the asset, the same rate the mock used before
	/// conversion was a swap, plus the market's surcharge on its own reserve when it prices
	/// against it.
	fn asset_for_native(asset: u32, native_amount: u64) -> u64 {
		let surcharge = if FEE_CONVERSION_RESERVE_PRICING.get() {
			<AssetsWithHolder as fungibles::Inspect<_>>::balance(asset, &MOCK_MARKET) /
				RESERVE_PRICE_DIVISOR
		} else {
			0
		};
		native_amount.saturating_add(surcharge)
	}
}

/// The asset kind the mock market knows the native currency by. It is the same id
/// [`crate::Config::Fungibles`] routes to the native currency, as in a real runtime: the market
/// and the fungibles union name one asset the same way.
pub const NATIVE_ASSET_KIND: u32 = NATIVE_DEPOSIT_ID;

parameter_types! {
	pub const NativeAssetKind: u32 = NATIVE_ASSET_KIND;
}

impl QuotePrice for TestFeeConversion {
	type Balance = u64;
	type AssetKind = u32;

	fn quote_price_tokens_for_exact_tokens(
		asset1: u32,
		asset2: u32,
		amount: u64,
		_include_fee: bool,
	) -> Option<u64> {
		assert_eq!(asset2, NATIVE_ASSET_KIND, "coinage only ever quotes an asset against native");
		// A swap of nothing is not a swap, so there is no price for it. Matches
		// `pallet-asset-conversion`, and checked before the failure switch because a quote the
		// market never priced must not consume one of its failures.
		if amount == 0 {
			return None;
		}
		let count = FEE_CONVERSION_QUOTE_COUNT.with(|c| {
			let v = c.get();
			c.set(v + 1);
			v
		});
		if FEE_CONVERSION_FAIL_AT.get().is_some_and(|fail_at| count >= fail_at) {
			return None;
		}
		let surcharge = match FEE_CONVERSION_QUOTE_SURCHARGE_AT.get() {
			Some((from, surcharge)) if count >= from => surcharge,
			_ => 0,
		};
		Some(TestFeeConversion::asset_for_native(asset1, amount).saturating_add(surcharge))
	}

	fn quote_price_exact_tokens_for_tokens(
		_asset1: u32,
		_asset2: u32,
		_amount: u64,
		_include_fee: bool,
	) -> Option<u64> {
		unimplemented!("coinage only quotes for an exact amount of native out")
	}
}

impl Swap<u64> for TestFeeConversion {
	type Balance = u64;
	type AssetKind = u32;

	fn max_path_len() -> u32 {
		2
	}

	fn swap_exact_tokens_for_tokens(
		_sender: u64,
		_path: Vec<u32>,
		_amount_in: u64,
		_amount_out_min: Option<u64>,
		_send_to: u64,
		_keep_alive: bool,
	) -> Result<u64, DispatchError> {
		unimplemented!("coinage only swaps for an exact amount of native out")
	}

	fn swap_tokens_for_exact_tokens(
		sender: u64,
		path: Vec<u32>,
		amount_out: u64,
		amount_in_max: Option<u64>,
		send_to: u64,
		keep_alive: bool,
	) -> Result<u64, DispatchError> {
		let [asset, native] = path[..] else {
			panic!("coinage always swaps a single hop, got {path:?}")
		};
		assert_eq!(native, NATIVE_ASSET_KIND, "the last hop must be the native currency");

		// Deliberately not going through `quote_price_tokens_for_exact_tokens`: the failure switch
		// counts quotes, and a swap that a quote already priced must not consume another one.
		let asset_in = TestFeeConversion::asset_for_native(asset, amount_out)
			.saturating_sub(FEE_CONVERSION_SWAP_DISCOUNT.get())
			.saturating_add(FEE_CONVERSION_SWAP_SURCHARGE.get());
		if amount_in_max.is_some_and(|max| asset_in > max) {
			return Err(DispatchError::Other("fee conversion above the maximum"));
		}
		// The same mapping `pallet-asset-conversion` applies to the flag.
		let preservation =
			if keep_alive { Preservation::Preserve } else { Preservation::Expendable };
		<AssetsWithHolder as fungibles::Mutate<_>>::transfer(
			asset,
			&sender,
			&MOCK_MARKET,
			asset_in,
			preservation,
		)?;
		Balances::mint_into(&send_to, amount_out)?;
		Ok(asset_in)
	}
}

#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
pub struct MembershipProof {
	pub context: Vec<u8>,
	pub msg: Vec<u8>,
	pub alias: Alias,
}
impl ValidateProof for MembershipProof {
	type Proof = MembershipProof;
	fn validate_proof(
		_identifier: &indiv_support::traits::Identifier,
		proof: &Self::Proof,
		context: &indiv_support::traits::Context,
		msg: &[u8],
	) -> Result<Alias, ()> {
		if proof.context == *context && proof.msg == msg {
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

/// Default test externalities, with the instance wrapping [`TEST_ASSET_ID`] already created.
/// Use [`new_test_ext_no_instance`] for tests that must run without one.
#[allow(dead_code)]
pub fn new_test_ext() -> sp_io::TestExternalities {
	let mut ext = new_test_ext_no_instance();
	ext.execute_with(setup_asset);
	ext
}

/// Test externalities without any instance created.
#[allow(dead_code)]
pub fn new_test_ext_no_instance() -> sp_io::TestExternalities {
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

/// Test externalities for the benchmark suite, with no instance, so the
/// `create_sufficient_instance` benchmark can create one. The others go through `common_setup`.
#[cfg(feature = "runtime-benchmarks")]
pub fn new_test_ext_bench() -> sp_io::TestExternalities {
	new_test_ext_no_instance()
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
/// If `as_coin` is true, the extension is configured with `AsCoinageInfo::AsCoin` for
/// [`TEST_INSTANCE_ID`]. Otherwise, it is configured with `None` (passthrough), which
/// should result in a BadOrigin when the pallet expects a Coin origin.
pub fn build_signed_as_coin_ext(signer: u64, call: crate::Call<Test>, as_coin: bool) -> Extrinsic {
	let info = as_coin.then_some(AsCoinageInfo::AsCoin);
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

/// Create a coin for an owner, ensuring asset backing is correct.
/// Mints asset to owner, transfers to pallet hold, inserts coin.
pub fn create_coin(owner: u64, value: Denomination, age: u16) {
	let amount = Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, value).unwrap();
	let asset_id = TEST_ASSET_ID;

	setup_asset();

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
	CoinsByOwner::<Test>::insert(
		owner,
		crate::pallet::Coin { instance_id: TEST_INSTANCE_ID, value, age },
	);
}

/// Setup a recycler with `count` members loaded.
/// Returns the secrets and the ring index and revision.
/// `seed_offset` allows creating multiple recyclers in the same test with different seeds.
///
/// This ensures assets are minted and backed properly by calling the pallet extrinsic.
/// After loading, triggers `Members::process_maintenance()` to build the ring.
pub fn setup_recycler(
	value: Denomination,
	count: u32,
	seed_offset: u8,
) -> (Vec<Secret>, RingIndex, RevisionIndex) {
	setup_asset();
	setup_recycler_for(TEST_INSTANCE_ID, TEST_ASSET_ID, value, count, seed_offset)
}

/// Give `who` `amount` of the asset an instance's coins wrap, whichever side of
/// [`NativeAndAssets`] it lives on.
///
/// The native side is topped up with the existential deposit, which the assets side gets from
/// the asset's own minimum balance being 1 in the mock.
pub fn fund_wrapped_asset(asset_id: u32, who: u64, amount: u64) {
	if asset_id == NATIVE_DEPOSIT_ID {
		fund_native(who, amount + <Balances as Currency<u64>>::minimum_balance());
	} else {
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), asset_id, who, amount));
	}
}

/// [`setup_recycler`] for an arbitrary instance.
///
/// The instance must already exist and, if it is sponsored, its loads must be collateralizable
/// (load deposit set and pot funded), since members are loaded through the pallet extrinsic.
pub fn setup_recycler_for(
	instance_id: InstanceId,
	asset_id: u32,
	value: Denomination,
	count: u32,
	seed_offset: u8,
) -> (Vec<Secret>, RingIndex, RevisionIndex) {
	let mut secrets = Vec::new();
	let amount = Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, value).unwrap();

	for i in 0..count {
		let user = 10000 + (seed_offset as u64) * 100 + i as u64;
		fund_wrapped_asset(asset_id, user, amount);

		let secret = get_unique_secret();
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

		assert_ok!(Coinage::load_recycler_with_external_asset(
			RuntimeOrigin::signed(user),
			instance_id,
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
	let identifier = Coinage::recycler_collection_identifier(instance_id, value);
	// The first member should be in ring 0 after building.
	let ring_index = 0u32;
	let revision =
		<Test as Config>::MemberService::ring_revision(&identifier, ring_index).unwrap_or(0);

	(secrets, ring_index, revision)
}

/// Asset wrapped by the sponsored instance mock helpers create.
pub const SPONSORED_ASSET_ID: u32 = 11;

/// Creator and default funder of the sponsored mock instance.
pub const SPONSOR: u64 = 4;

/// Create the sponsored instance wrapping [`SPONSORED_ASSET_ID`], if it does not yet exist,
/// and return its id.
///
/// The privileged [`TEST_INSTANCE_ID`] is created first, so the sponsored instance always gets
/// a distinct id and the privileged paths stay exercised alongside the sponsored ones.
pub fn setup_sponsored_instance() -> InstanceId {
	setup_asset();
	create_asset(SPONSORED_ASSET_ID);
	if let Some(instance_id) =
		crate::AssetToInstance::<Test>::iter_key_prefix(SPONSORED_ASSET_ID).next()
	{
		return instance_id;
	}
	fund_native(SPONSOR, 1_000_000);
	assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), SPONSORED_ASSET_ID, SPONSOR, 1_000_000));
	let instance_id = crate::NextInstanceId::<Test>::get();
	assert_ok!(Coinage::create_sponsored_instance(
		RuntimeOrigin::signed(SPONSOR),
		SPONSORED_ASSET_ID,
		UNDERLYING_ASSET_UNIT,
		None
	));
	instance_id
}

/// Set the chain-wide load deposit, which a real chain changes through governance.
pub fn set_load_deposit(currency: u32, price: u64) {
	LoadDeposit::set(&(currency, price));
}

/// Fund the pot of `instance_id` with a tracked contribution from [`SPONSOR`], who is funded
/// with the necessary balance first.
pub fn fund_pot(instance_id: InstanceId, currency: u32, amount: u64) {
	if currency == NATIVE_DEPOSIT_ID {
		fund_native(SPONSOR, amount + 100);
	} else {
		create_asset(currency);
		// One unit above the funded amount: the funding transfers with `Protect`, so the
		// sponsor's account must survive at the asset's minimum balance.
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), currency, SPONSOR, amount + 1));
	}
	assert_ok!(Coinage::fund_pot(RuntimeOrigin::signed(SPONSOR), instance_id, currency, amount));
}

/// Check the load-deposit invariant for a sponsored instance: per currency, the pot's held
/// balance under `HoldReason::LoadDeposit` equals `Σ price * count` over that currency's tiers,
/// and the tier counts sum to `expected_keys`.
///
/// Every currency that exists is queried, not only the ones with a live tier, so collateral
/// stranded in a currency whose tiers are all gone fails the check too.
#[track_caller]
pub fn check_load_deposit_invariant(instance_id: InstanceId, expected_keys: u32) {
	let pot = Coinage::pot_account(instance_id);
	let record = crate::Instances::<Test>::get(instance_id).expect("instance exists");

	let mut held_by_currency = alloc::collections::BTreeMap::<u32, u64>::new();
	let mut keys = 0u32;
	for tier in record.old_load_deposit.iter().chain(record.current_load_deposit.iter()) {
		keys += tier.count;
		*held_by_currency.entry(tier.asset_id).or_default() += tier.price * u64::from(tier.count);
	}
	assert_eq!(keys, expected_keys, "redeemable key count mismatch");

	let currencies = pallet_assets::Asset::<Test>::iter_keys()
		.chain(core::iter::once(NATIVE_DEPOSIT_ID))
		.collect::<alloc::collections::BTreeSet<_>>();
	for currency in currencies {
		let held = <NativeAndAssets as InspectHold<_>>::balance_on_hold(
			currency,
			&HoldReason::LoadDeposit.into(),
			&pot,
		);
		let expected = held_by_currency.get(&currency).copied().unwrap_or(0);
		assert_eq!(held, expected, "held balance mismatch for currency {currency}");
	}
}

/// The tier the instance last loaded at.
pub fn current_tier(instance_id: InstanceId) -> Option<DepositTier<u32, u64>> {
	Instances::<Test>::get(instance_id)
		.expect("instance exists")
		.current_load_deposit
}

/// The instance's superseded tier, if it still holds deposits.
pub fn old_tier(instance_id: InstanceId) -> Option<DepositTier<u32, u64>> {
	Instances::<Test>::get(instance_id).expect("instance exists").old_load_deposit
}

/// Mint the load amount to a fresh user and load one member key into `instance_id` at
/// denomination 0 through the plain signed call.
pub fn try_load(
	instance_id: InstanceId,
	asset_id: u32,
	seed: u64,
) -> frame_support::dispatch::DispatchResultWithPostInfo {
	try_load_with_unit(instance_id, asset_id, UNDERLYING_ASSET_UNIT, seed)
}

/// [`try_load`] for an instance whose coin of denomination 0 is worth `asset_unit`.
pub fn try_load_with_unit(
	instance_id: InstanceId,
	asset_id: u32,
	asset_unit: u64,
	seed: u64,
) -> frame_support::dispatch::DispatchResultWithPostInfo {
	let user = 20_000 + seed;
	assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), asset_id, user, asset_unit));
	let secret = get_unique_secret();
	let member = CryptoOf::<Test>::member_from_secret(&secret);
	let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();
	Coinage::load_recycler_with_external_asset(
		RuntimeOrigin::signed(user),
		instance_id,
		CodecPreservation::Expendable,
		0,
		member,
		proof,
	)
}

/// The pot's balance held under `HoldReason::LoadDeposit` in `currency`.
pub fn pot_held(instance_id: InstanceId, currency: u32) -> u64 {
	let pot = Coinage::pot_account(instance_id);
	<NativeAndAssets as InspectHold<_>>::balance_on_hold(
		currency,
		&HoldReason::LoadDeposit.into(),
		&pot,
	)
}

/// The pot's reducible balance in `currency`, which is what a withdrawal or a fresh load
/// deposit can draw from.
pub fn pot_free(instance_id: InstanceId, currency: u32) -> u64 {
	let pot = Coinage::pot_account(instance_id);
	<NativeAndAssets as Inspect<_>>::reducible_balance(
		currency,
		&pot,
		frame_support::traits::tokens::Preservation::Preserve,
		frame_support::traits::tokens::Fortitude::Polite,
	)
}

/// Withdraw the pot's whole reducible balance in `currency` back to [`SPONSOR`], so the next
/// sponsored load cannot be collateralized.
pub fn drain_pot(instance_id: InstanceId, currency: u32) {
	let free = pot_free(instance_id, currency);
	assert_ok!(Coinage::withdraw_pot_funds(
		RuntimeOrigin::signed(SPONSOR),
		instance_id,
		currency,
		free
	));
	assert_eq!(pot_free(instance_id, currency), 0);
}

/// A sponsored instance with [`Config::LoadDeposit`] set to `(NATIVE_DEPOSIT_ID, price)`, its
/// pot funded with `funding` and a denomination-0 recycler of `count` keys, each key
/// collateralized by one load deposit from the pot.
pub fn setup_sponsored_recycler(
	price: u64,
	funding: u64,
	count: u32,
	seed_offset: u8,
) -> (InstanceId, Vec<Secret>, RingIndex, RevisionIndex) {
	let instance_id = setup_sponsored_instance();
	set_load_deposit(NATIVE_DEPOSIT_ID, price);
	fund_pot(instance_id, NATIVE_DEPOSIT_ID, funding);
	let (secrets, index, revision) =
		setup_recycler_for(instance_id, SPONSORED_ASSET_ID, 0, count, seed_offset);
	check_load_deposit_invariant(instance_id, count);
	(instance_id, secrets, index, revision)
}

/// The alias of `secret` in the recycler unloading context.
pub fn recycler_alias(secret: &Secret) -> indiv_support::traits::Alias {
	CryptoOf::<Test>::alias_in_context(secret, UNLOADING_RECYCLER_CONTEXT.as_ref())
		.expect("alias derivation succeeds")
}

/// Register `count` paid unload token member keys, paying each fee in native, and build the
/// paid-token ring. Returns the members' secrets and the `(period, ring_index, revision)`
/// coordinates needed to build an [`AsCoinageInfo::AsUnloadTokenPaid`] extension.
///
/// Call this before setting up the recyclers under test: it runs the members maintenance,
/// which would bump the revision of any ring with queued members.
pub fn setup_paid_unload_tokens(count: u32) -> (Vec<Secret>, u32, u32, u32) {
	let payer = 30_000u64;
	fund_native(payer, 1_000_000);
	fund_native(FEE_DESTINATION, 1_000);
	let mut secrets = Vec::new();
	for _ in 0..count {
		let secret = get_unique_secret();
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &payer.encode()).unwrap();
		assert_ok!(Coinage::pay_for_recycler_unload_fee_token_with_native(
			RuntimeOrigin::signed(payer),
			member,
			proof
		));
		secrets.push(secret);
	}
	Members::process_maintenance();

	let period = (MockTime::now().as_secs() as u32) /
		get_u32::<<Test as crate::Config>::PaidUnloadTokenTimePeriod>();
	let ring_index = 0u32;
	let revision = <Test as crate::Config>::MemberService::ring_revision(
		&Coinage::paid_token_collection_identifier(period),
		ring_index,
	)
	.expect("paid token ring exists");
	(secrets, period, ring_index, revision)
}

/// Create `asset_id` without the instance wrapping it, if it doesn't yet exist.
pub fn create_asset(asset_id: u32) {
	if !pallet_assets::Asset::<Test>::contains_key(asset_id) {
		assert_ok!(Assets::force_create(RuntimeOrigin::root(), asset_id, ALICE, true, 1));
	}
}

/// Create [`TEST_ASSET_ID`] and the coinage instance wrapping it, if they don't yet exist.
///
/// The only place mock helpers create the instance, so that coin operations don't bail with
/// `Error::InstanceNotFound`.
pub fn setup_asset() {
	create_asset(TEST_ASSET_ID);

	if crate::AssetToInstance::<Test>::iter_key_prefix(TEST_ASSET_ID).next().is_none() {
		// What governance is expected to do before creating an instance: give the pallet
		// account a balance buffer so fee flows that empty its free balance cannot kill it.
		let min_balance =
			<Assets as frame_support::traits::fungibles::Inspect<u64>>::minimum_balance(
				TEST_ASSET_ID,
			);
		assert_ok!(Assets::mint(
			RuntimeOrigin::signed(ALICE),
			TEST_ASSET_ID,
			Coinage::pallet_account(),
			min_balance
		));
		assert_ok!(Coinage::create_sufficient_instance(
			RuntimeOrigin::root(),
			TEST_ASSET_ID,
			UNDERLYING_ASSET_UNIT
		));
	}
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

/// The asset amount that pays exactly one unload token fee at the mock rate. Tests pass this as
/// `max_fee` when they expect the fee to be paid with the asset.
pub fn unload_token_fee_in_asset() -> u64 {
	Coinage::get_paid_unload_token_fee_in_asset(TEST_INSTANCE_ID)
		.expect("the mock fee conversion is available")
}

/// A `max_fee` bound that comfortably covers the unload token fees of a signed call, for tests
/// that assert the fee actually charged rather than the bound.
pub fn max_fee_bound() -> u64 {
	unload_token_fee_in_asset() * 8
}

/// The same bound for a call paying its fees in the native currency, i.e. with
/// [`crate::FeeCurrency::Native`].
pub fn native_max_fee_bound() -> u64 {
	Coinage::get_paid_unload_token_fee_in_native() * 8
}

pub fn fund_native(who: u64, amount: u64) {
	let _ = Balances::mint_into(&who, amount);
}

/// Check the accounting invariant:
/// Total Funds (Held) == Coins + Active Recyclers (Net) + Archived + Destroyed
#[track_caller]
pub fn check_accounting() {
	let asset_id = TEST_ASSET_ID;
	let pallet_acc = Coinage::pallet_account();

	// Total funds held by the pallet account (User funds are always held)
	let total_held =
		AssetsWithHolder::balance_on_hold(asset_id, &HoldReason::Wrapped.into(), &pallet_acc);

	// 1. Value of all Coins
	let coins_value: u64 = CoinsByOwner::<Test>::iter_values()
		.filter(|c| c.instance_id == TEST_INSTANCE_ID)
		.map(|c| Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, c.value).unwrap())
		.sum();

	// 2. Net Value of Active Recyclers
	// Count members in recycler collections via MemberService.
	let mut recyclers_value: u64 = 0;
	for (value, ()) in RecyclerCollectionCreated::<Test>::iter_prefix(TEST_INSTANCE_ID) {
		let asset_amount =
			Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, value).unwrap();
		let identifier = Coinage::recycler_collection_identifier(TEST_INSTANCE_ID, value);
		let active = <Test as Config>::MemberService::active_count(&identifier) as u64;
		// Subtract unloaded aliases for all rings.
		// For simplicity, count total unloaded across all rings for this value.
		let unloaded: u64 = RecyclerAliasStates::<Test>::iter_prefix((TEST_INSTANCE_ID, value))
			.filter(|(_, state)| matches!(state, AliasState::Unloaded))
			.count() as u64;
		let net_members = active.saturating_sub(unloaded);
		recyclers_value += net_members * asset_amount;
	}

	// 3. Value of Archived Recyclers (recoverable, not destroyed): remaining coins per archive.
	// The archive key is a single tuple rather than a double map, so this filters instead of
	// iterating an instance prefix.
	let archived_value: u64 = RecyclersArchives::<Test>::iter()
		.filter(|((instance_id, _, _), _)| *instance_id == TEST_INSTANCE_ID)
		.map(|((_, value, _ring), info)| {
			Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, value).unwrap() *
				info.remaining as u64
		})
		.sum();

	// 4. Total Value of Destroyed Coins
	let destroyed_value = TotalValueOfDestroyedCoins::<Test>::get(TEST_INSTANCE_ID);

	let total_in_pallet = coins_value + recyclers_value + archived_value + destroyed_value;

	assert_eq!(
		total_held, total_in_pallet,
		"Invariant violation: Held {total_held} != In pallet {total_in_pallet} \n\
		detail: Coins: {coins_value}, Recyclers: {recyclers_value}, Archived: {archived_value}, \
		Destroyed: {destroyed_value}",
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

/// Build an unsigned extrinsic with `AsUnloadTokenFromOutput` extension against
/// [`TEST_INSTANCE_ID`].
pub fn build_unload_from_output_ext(
	call: crate::Call<Test>,
	fee_recycler_value: Denomination,
	fee_recycler_index: RingIndex,
	fee_recycler_revision: RevisionIndex,
	secrets: &[Secret],
) -> Extrinsic {
	build_unload_from_output_ext_for(
		TEST_INSTANCE_ID,
		call,
		fee_recycler_value,
		fee_recycler_index,
		fee_recycler_revision,
		secrets,
	)
}

/// Build an unsigned extrinsic with `AsUnloadTokenFromOutput` extension against an arbitrary
/// instance. Computes the correct inherited_implication and creates proofs that sign over it.
pub fn build_unload_from_output_ext_for(
	instance_id: InstanceId,
	call: crate::Call<Test>,
	fee_recycler_value: Denomination,
	fee_recycler_index: RingIndex,
	fee_recycler_revision: RevisionIndex,
	secrets: &[Secret],
) -> Extrinsic {
	let runtime_call: RuntimeCall = call.into();

	let inherited_implication = ((0u8, &runtime_call), (), ());
	let proven_msg = sp_io::hashing::blake2_256(&inherited_implication.encode());
	let first_alias =
		CryptoOf::<Test>::alias_in_context(&secrets[0], UNLOADING_RECYCLER_CONTEXT.as_ref())
			.expect("should derive alias");
	let retry_counter = match RecyclerAliasStates::<Test>::get((
		instance_id,
		fee_recycler_value,
		fee_recycler_index,
		first_alias,
	)) {
		Some(AliasState::Locked(LockInfo {
			reason: LockReason::FailedDispatch { retries },
			..
		})) => retries.saturating_add(1),
		_ => 0,
	};

	let ring_members =
		Coinage::get_recycler_members(instance_id, fee_recycler_value, fee_recycler_index);

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

	// Create first alias proof signing the other proofs, retry counter, and inherited implication.
	let intent_msg = (&other_alias_proofs_vec, retry_counter, &inherited_implication)
		.using_encoded(sp_io::hashing::blake2_256);
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
		retry_counter,
		alias_proofs: alias_proofs_vec.try_into().expect("proofs should fit in bounds"),
	});
	let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(info));
	Extrinsic::new_transaction(runtime_call, extension)
}

/// Helper to build the unload extrinsic using a Paid Unload Token against
/// [`TEST_INSTANCE_ID`].
pub fn build_unload_paid_ext(
	call: crate::Call<Test>,
	paid_token_secret: &Secret,
	paid_token_ring_index: u32,
	paid_token_ring_revision: u32,
	period: u32,
	recycler_secrets: &[Secret],
	value: Denomination,
	index: u32,
) -> Extrinsic {
	build_unload_paid_ext_for(
		TEST_INSTANCE_ID,
		call,
		paid_token_secret,
		paid_token_ring_index,
		paid_token_ring_revision,
		period,
		recycler_secrets,
		value,
		index,
	)
}

/// Helper to build the unload extrinsic using a Paid Unload Token against an arbitrary
/// instance.
pub fn build_unload_paid_ext_for(
	instance_id: InstanceId,
	call: crate::Call<Test>,
	paid_token_secret: &Secret,
	paid_token_ring_index: u32,
	paid_token_ring_revision: u32,
	period: u32,
	recycler_secrets: &[Secret],
	value: Denomination,
	index: u32,
) -> Extrinsic {
	let runtime_call: RuntimeCall = call.into();

	// 1. Calculate Implication
	let inherited_implication = ((0u8, &runtime_call), (), ());
	let proven_msg = sp_io::hashing::blake2_256(&inherited_implication.encode());

	// 2. Generate Alias Proofs (must be created before the paid token proof so we can include them
	//    in the intent message)
	let mut alias_proofs_vec = Vec::new();
	let ring_members = Coinage::get_recycler_members(instance_id, value, index);

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

/// Build an unload extrinsic authenticated by a people or lite-people free unload token,
/// against an arbitrary instance.
///
/// The mock membership proof accepts whatever `people_alias` is given, so distinct aliases
/// simulate distinct persons.
pub fn build_unload_free_token_ext_for(
	instance_id: InstanceId,
	call: crate::Call<Test>,
	kind: FreeTokenKind,
	period: u32,
	counter: u32,
	people_alias: indiv_support::traits::Alias,
	recycler_secrets: &[Secret],
	value: Denomination,
	index: RingIndex,
) -> Extrinsic {
	let runtime_call: RuntimeCall = call.into();
	let inherited_implication = ((0u8, &runtime_call), (), ());
	let proven_msg = sp_io::hashing::blake2_256(&inherited_implication.encode());

	// The alias proofs must be created before the people proof, which signs over them.
	let ring_members = Coinage::get_recycler_members(instance_id, value, index);
	let mut alias_proofs_vec = Vec::new();
	for secret in recycler_secrets {
		let (proof, _) = create_unload_proof(secret, &ring_members, &proven_msg);
		alias_proofs_vec.push(proof);
	}
	let alias_proofs = BoundedVec::try_from(alias_proofs_vec).expect("proofs fit in bounds");

	let intent_msg = sp_io::hashing::blake2_256(
		&[alias_proofs.encode(), inherited_implication.encode()].concat(),
	);
	let context = crate::pallet::free_unload_token_context(period, counter);
	let proof = MembershipProof {
		context: context.to_vec(),
		msg: intent_msg.to_vec(),
		alias: people_alias,
	};
	let info = match kind {
		FreeTokenKind::People =>
			AsCoinageInfo::AsUnloadTokenPeople { proof, period, counter, alias_proofs },
		FreeTokenKind::LitePeople =>
			AsCoinageInfo::AsUnloadTokenLitePeople { proof, period, counter, alias_proofs },
	};
	let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(info)));
	Extrinsic::new_signed(runtime_call, 0, UintAuthorityId(0), extension)
}

/// The unload-token extension flavor a load-deposit test drives a recycler unload call with.
pub enum UnloadTokenVariant {
	People,
	LitePeople,
	Paid { secrets: Vec<Secret>, period: u32, ring_index: u32, revision: u32 },
	FromOutput,
}

/// [`UnloadTokenVariant::Paid`] with `count` freshly registered paid unload tokens.
///
/// Like [`setup_paid_unload_tokens`], call this before setting up the recyclers under test.
pub fn paid_unload_token_variant(count: u32) -> UnloadTokenVariant {
	let (secrets, period, ring_index, revision) = setup_paid_unload_tokens(count);
	UnloadTokenVariant::Paid { secrets, period, ring_index, revision }
}

impl UnloadTokenVariant {
	/// Build the `nth` unload extrinsic of this flavor for `call`, proving `recycler_secrets`
	/// against the recycler `(instance_id, value, index, revision)`.
	///
	/// `nth` distinguishes consecutive tokens of one test: the free-token flavors derive a
	/// distinct person and counter from it, and the paid flavor consumes the `nth` registered
	/// token.
	pub fn build_ext(
		&self,
		instance_id: InstanceId,
		call: crate::Call<Test>,
		recycler_secrets: &[Secret],
		value: Denomination,
		index: RingIndex,
		revision: RevisionIndex,
		nth: u32,
	) -> Extrinsic {
		match self {
			UnloadTokenVariant::People => build_unload_free_token_ext_for(
				instance_id,
				call,
				FreeTokenKind::People,
				0,
				nth,
				[(nth as u8).saturating_add(1); 32],
				recycler_secrets,
				value,
				index,
			),
			UnloadTokenVariant::LitePeople => build_unload_free_token_ext_for(
				instance_id,
				call,
				FreeTokenKind::LitePeople,
				0,
				nth,
				[(nth as u8).saturating_add(1); 32],
				recycler_secrets,
				value,
				index,
			),
			UnloadTokenVariant::Paid {
				secrets,
				period,
				ring_index,
				revision: paid_token_revision,
			} => build_unload_paid_ext_for(
				instance_id,
				call,
				&secrets[nth as usize],
				*ring_index,
				*paid_token_revision,
				*period,
				recycler_secrets,
				value,
				index,
			),
			UnloadTokenVariant::FromOutput => build_unload_from_output_ext_for(
				instance_id,
				call,
				value,
				index,
				revision,
				recycler_secrets,
			),
		}
	}
}

/// The input and proofs unloading `secrets` from the denomination-`value` recycler
/// `(instance_id, index)` non-anonymously, bound to `signer` and `to`.
///
/// The proven message covers the input, which carries the aliases, so the aliases are derived
/// first (they do not depend on the message) and the proofs rebuilt over the final input.
pub fn build_non_anonymous_unload(
	instance_id: InstanceId,
	secrets: &[Secret],
	value: Denomination,
	index: RingIndex,
	signer: u64,
	to: u64,
) -> (
	UnloadRecyclerInput<<Test as crate::Config>::MaxConsolidation>,
	BoundedVec<Proof, <Test as crate::Config>::MaxConsolidation>,
) {
	type RInput = UnloadRecyclerInput<<Test as crate::Config>::MaxConsolidation>;

	let revision = <Test as crate::Config>::MemberService::ring_revision(
		&Coinage::recycler_collection_identifier(instance_id, value),
		index,
	)
	.expect("ring exists");
	// The whole ring, not just the keys under test: earlier setups may have put members in it,
	// and the proofs are built against the ring the root commits to.
	let members = Coinage::get_recycler_members(instance_id, value, index);

	let aliases = secrets.iter().map(recycler_alias).collect::<Vec<_>>();
	let input: RInput = UnloadRecyclerInput {
		value,
		index,
		revision,
		aliases: BoundedVec::try_from(aliases).expect("aliases fit in bounds"),
	};
	let proven_msg = sp_io::hashing::blake2_256(
		&(instance_id, &alloc::vec![input.clone()], &to, &signer).encode(),
	);
	let proofs = secrets
		.iter()
		.map(|secret| create_unload_proof(secret, &members, &proven_msg).0)
		.collect::<Vec<_>>();

	(input, BoundedVec::try_from(proofs).expect("proofs fit in bounds"))
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
		setup_asset();
	}

	fn setup_asset_without_instance() -> u32 {
		use frame_support::{
			assert_ok,
			traits::fungibles::{Inspect, Mutate},
		};
		let asset_id = TEST_ASSET_ID;
		create_asset(asset_id);
		// What governance does before `create_sufficient_instance`: the pallet account's balance
		// buffer.
		assert_ok!(AssetsWithHolder::mint_into(
			asset_id,
			&Coinage::pallet_account(),
			<AssetsWithHolder as Inspect<u64>>::minimum_balance(asset_id)
		));
		asset_id
	}

	fn fund_account(who: &u64, amount: u64) {
		use frame_support::{assert_ok, traits::fungibles::Mutate};
		let asset_id = TEST_ASSET_ID;
		assert_ok!(AssetsWithHolder::mint_into(asset_id, who, amount));
	}

	fn create_extra_asset(seed: u32, who: &u64) -> u32 {
		use frame_support::{assert_ok, traits::fungibles::Mutate};
		let asset_id = EXTRA_ASSET_ID_BASE + seed;
		create_asset(asset_id);
		assert_ok!(AssetsWithHolder::mint_into(asset_id, who, 1_000_000_000));
		asset_id
	}

	fn extra_asset_id(seed: u32) -> u32 {
		EXTRA_ASSET_ID_BASE + seed
	}

	fn set_time(now: core::time::Duration) {
		TIME.with(|t| {
			*t.borrow_mut() = now;
		});
	}

	fn setup_fee_conversion() {
		// `TestFeeConversion` has a fixed rate and unlimited depth, so there is nothing to set up.
	}

	fn create_people_proof(
		context: &[u8],
		msg: &[u8],
		alias: indiv_support::traits::Alias,
	) -> MembershipProof {
		MembershipProof { context: context.to_vec(), msg: msg.to_vec(), alias }
	}

	fn create_lite_people_proof(
		context: &[u8],
		msg: &[u8],
		alias: indiv_support::traits::Alias,
	) -> MembershipProof {
		MembershipProof { context: context.to_vec(), msg: msg.to_vec(), alias }
	}

	fn setup_batch_verify(
		count: u32,
	) -> Result<
		(
			Denomination,
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
		let min_exp = <Test as crate::Config>::MinimumExponent::get();
		let max_exp = <Test as crate::Config>::MaximumExponent::get();
		let value_span = u32::try_from(i16::from(max_exp) - i16::from(min_exp) + 1)
			.map_err(|_| frame_benchmarking::BenchmarkError::Weightless)?;
		let value_offset = i16::try_from(offset % value_span)
			.map_err(|_| frame_benchmarking::BenchmarkError::Weightless)?;
		let value = i8::try_from(i16::from(min_exp) + value_offset)
			.map_err(|_| frame_benchmarking::BenchmarkError::Weightless)?;
		let seed_offset = u8::try_from(offset % (u32::from(u8::MAX) + 1))
			.map_err(|_| frame_benchmarking::BenchmarkError::Weightless)?;
		let (secrets, ring_index, _revision) = setup_recycler(value, count, seed_offset);
		let identifier = Coinage::recycler_collection_identifier(TEST_INSTANCE_ID, value);
		let ring_members = Coinage::get_recycler_members(TEST_INSTANCE_ID, value, ring_index);
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
