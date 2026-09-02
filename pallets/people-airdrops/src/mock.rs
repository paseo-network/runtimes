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

extern crate alloc;

use alloc::{sync::Arc, vec::Vec};
use codec::Decode;
use core::{cell::RefCell, time::Duration};
use frame_support::{
	derive_impl, parameter_types,
	traits::{AsEnsureOriginWithArg, ConstU32, EnsureOriginWithArg, UnixTime},
};
use frame_system::{
	offchain::{CreateAuthorizedTransaction, CreateTransaction, CreateTransactionBase},
	AuthorizeCall, EnsureRoot,
};
use indiv_support::traits::{
	Alias, Context, ContextualAlias, Identifier, MembershipProver, RevisionIndex, RingIndex,
	RingMembershipProof,
};
use sp_core::sr25519;
use sp_crypto_hashing::blake2_256;
use sp_runtime::{
	offchain::{
		testing::{PoolState, TestOffchainExt, TestTransactionPoolExt},
		OffchainDbExt, OffchainWorkerExt, TransactionPoolExt,
	},
	testing::UintAuthorityId,
	traits::TryConvert,
	BuildStorage, DispatchError,
};
use verifiable::{mock::Mock, GenerateVerifiable};

pub type Header = sp_runtime::generic::Header<u64, sp_runtime::traits::BlakeTwo256>;
pub type Block = sp_runtime::generic::Block<Header, Extrinsic>;
pub type TxExtension = (AuthorizeCall<Test>,);
pub type Extrinsic =
	sp_runtime::generic::UncheckedExtrinsic<u64, RuntimeCall, UintAuthorityId, TxExtension>;

frame_support::construct_runtime!(
	pub enum Test {
		System: frame_system,
		Balances: pallet_balances,
		Assets: pallet_assets,
		AssetsHolder: pallet_assets_holder,
		Airdrop: indiv_pallet_airdrop,
		PeopleAirdrops: crate,
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
	type ForceOrigin = EnsureRoot<u64>;
	type Holder = AssetsHolder;
}

impl pallet_assets_holder::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeHoldReason = RuntimeHoldReason;
}

impl<C> CreateTransactionBase<C> for Test
where
	RuntimeCall: From<C>,
{
	type RuntimeCall = RuntimeCall;
	type Extrinsic = Extrinsic;
}

impl<C> CreateTransaction<C> for Test
where
	RuntimeCall: From<C>,
{
	type Extension = TxExtension;
	fn create_transaction(
		call: <Self as CreateTransactionBase<C>>::RuntimeCall,
		extension: Self::Extension,
	) -> Self::Extrinsic {
		Extrinsic::new_transaction(call, extension)
	}
}

impl<C> CreateAuthorizedTransaction<C> for Test
where
	RuntimeCall: From<C>,
{
	fn create_extension() -> Self::Extension {
		(AuthorizeCall::new(),)
	}
}

thread_local! {
	pub static TIME: RefCell<Duration> = RefCell::new(Duration::default());
	static TRANSACTION_POOL: RefCell<Arc<parking_lot::RwLock<PoolState>>> =
		RefCell::new(Arc::new(parking_lot::RwLock::new(PoolState {
			transactions: Vec::new(),
		})));
}

pub struct MockTime;
impl UnixTime for MockTime {
	fn now() -> Duration {
		TIME.with(|t| *t.borrow())
	}
}

pub fn set_now_secs(secs: u64) {
	TIME.with(|t| *t.borrow_mut() = Duration::from_secs(secs));
}

thread_local! {
	pub static RANDOMNESS: RefCell<Option<([u8; 32], u32)>> =
		const { RefCell::new(Some(([42u8; 32], 1))) };
}

pub fn set_randomness(value: Option<([u8; 32], u32)>) {
	RANDOMNESS.with(|r| *r.borrow_mut() = value);
}

/// Advance the mock randomness by one block, mirroring the relay chain producing a fresh
/// VRF output every block. A no-op while the randomness is set to `None`.
pub fn advance_randomness() {
	RANDOMNESS.with(|r| {
		let mut r = r.borrow_mut();
		if let Some((value, block_number)) = *r {
			*r = Some((value, block_number + 1));
		}
	});
}

pub struct MockRandomness;
impl indiv_support::traits::MomentRandomness<u32> for MockRandomness {
	fn randomness() -> Option<([u8; 32], u32)> {
		RANDOMNESS.with(|r| *r.borrow())
	}

	fn current_moment() -> u32 {
		RANDOMNESS.with(|r| r.borrow().map_or(0, |(_, moment)| moment))
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn set_randomness(randomness: [u8; 32], moment: u32) {
		RANDOMNESS.with(|r| *r.borrow_mut() = Some((randomness, moment)));
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn set_current_moment(moment: u32) {
		RANDOMNESS.with(|r| {
			let mut r = r.borrow_mut();
			let value = r.map_or([0u8; 32], |(value, _)| value);
			*r = Some((value, moment));
		});
	}
}

/// Permissive membership service for the airdrop pallet's config. The proof-based participation
/// path is unused by these tests, so any proof verifies and the alias derives from the message.
pub struct MockMemberService;

impl MembershipProver for MockMemberService {
	type Crypto = Mock;

	fn verify_membership(
		_identifier: &Identifier,
		_proof: &<Mock as GenerateVerifiable>::Proof,
		_ring_index: RingIndex,
		_revision: RevisionIndex,
		context: Context,
		msg: &[u8],
	) -> Result<ContextualAlias, DispatchError> {
		Ok(ContextualAlias { context, alias: blake2_256(msg) })
	}

	fn verify_memberships_in_ring(
		_identifier: &Identifier,
		_ring_index: RingIndex,
		_revision: RevisionIndex,
		_items: &[RingMembershipProof<<Mock as GenerateVerifiable>::Proof>],
	) -> Result<Vec<ContextualAlias>, DispatchError> {
		unimplemented!("ring batch verification is unused in these tests")
	}

	fn ring_revision(_identifier: &Identifier, _ring_index: RingIndex) -> Option<RevisionIndex> {
		Some(0)
	}

	fn is_revision_valid(
		_identifier: &Identifier,
		_ring_index: RingIndex,
		_revision: RevisionIndex,
	) -> bool {
		true
	}

	fn revision_source_time(
		_identifier: &Identifier,
		_ring_index: RingIndex,
		_revision: RevisionIndex,
	) -> Option<u64> {
		None
	}

	fn old_root_retention() -> u64 {
		// This mock keeps no root history, so nothing is ever superseded.
		0
	}
}

/// The account-based participation path is unused by these tests, so the conversion always
/// fails.
pub struct AccountToPub;
impl TryConvert<u64, sr25519::Public> for AccountToPub {
	fn try_convert(account_id: u64) -> Result<sr25519::Public, u64> {
		Err(account_id)
	}
}

parameter_types! {
	pub AirdropPalletId: frame_support::PalletId = frame_support::PalletId(*b"pop/adrp");
}

impl indiv_pallet_airdrop::Config for Test {
	type WeightInfo = ();
	type MemberService = MockMemberService;
	type Fungibles = indiv_support::fungibles::CombineAssetsWithHolder<Assets, AssetsHolder>;
	type ManagerOrigin = EnsureRoot<u64>;
	type PalletId = AirdropPalletId;
	type UnixTime = MockTime;
	type Randomness = MockRandomness;
	type AccountIdToPublic = AccountToPub;
	type ClearLimit = ConstU32<100>;
	type DrawLimit = ConstU32<100>;
	type OffchainWorkerInterval = frame_support::traits::ConstU64<1>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = AirdropBenchmarkHelper;
}

/// Benchmark hooks required by the airdrop pallet's config. Its benchmarks never run against
/// this runtime, so the proof and keypair builders are permissive placeholders.
#[cfg(feature = "runtime-benchmarks")]
pub struct AirdropBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl indiv_pallet_airdrop::benchmarking::BenchmarkHelper<Test> for AirdropBenchmarkHelper {
	fn set_unix_time(now: Duration) {
		TIME.with(|t| *t.borrow_mut() = now);
	}

	fn create_asset_id_parameter(id: u32) -> u32 {
		id
	}

	fn build_membership_proof(
		_context: &Context,
		message: &[u8],
		_member_seed: u32,
	) -> (indiv_pallet_airdrop::ProofOf<Test>, Alias) {
		(<Mock as GenerateVerifiable>::Proof::default(), blake2_256(message))
	}

	fn account_keypair_for(_seed: u32) -> (u64, sr25519::Pair) {
		unimplemented!("account participation is unused in this mock")
	}
}

/// Signed accounts below this bound resolve to a person alias via [`EnsurePersonMock`].
pub const MAX_PERSON_ACCOUNT: u64 = 100;

/// The alias [`EnsurePersonMock`] resolves a signed `account` to.
pub fn alias_of(account: u64) -> Alias {
	let mut alias = [0u8; 32];
	alias[..8].copy_from_slice(&account.to_le_bytes());
	alias
}

/// Person origin mock: a signed origin with account id below [`MAX_PERSON_ACCOUNT`] resolves to
/// [`alias_of`] the account; anything else is not a person.
pub struct EnsurePersonMock;
impl EnsureOriginWithArg<RuntimeOrigin, Context> for EnsurePersonMock {
	type Success = Alias;

	fn try_origin(origin: RuntimeOrigin, context: &Context) -> Result<Alias, RuntimeOrigin> {
		match frame_system::ensure_signed(origin.clone()) {
			Ok(account)
				if account < MAX_PERSON_ACCOUNT &&
					context == &crate::Pallet::<Test>::people_airdrops_context() =>
				Ok(alias_of(account)),
			_ => Err(origin),
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn try_successful_origin(context: &Context) -> Result<RuntimeOrigin, ()> {
		(context == &crate::Pallet::<Test>::people_airdrops_context())
			.then(|| RuntimeOrigin::signed(1))
			.ok_or(())
	}
}

parameter_types! {
	pub const PrizeSourceAccount: u64 = 7;
	pub NetworkSuffix: indiv_support::context::ProductContextNetworkSuffix =
		b"paseo".to_vec().try_into().expect("network suffix fits");
}

impl crate::Config for Test {
	type WeightInfo = ();
	type Suffix = NetworkSuffix;
	type EnsurePerson = EnsurePersonMock;
	type AirdropAssetId = u32;
	type AirdropAssetBalance = u64;
	type Airdrop = Airdrop;
	type ManagerOrigin = EnsureRoot<u64>;
	type PrizeSource = PrizeSourceAccount;
	type Randomness = MockRandomness;
	type UnixTime = MockTime;
	type MaxScheduleBatch = ConstU32<8>;
	type MaxRegisterBatch = ConstU32<8>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = PeopleAirdropsBenchmarkHelper;
}

/// Benchmark hooks: places draws into lifecycle phases by writing the airdrop pallet's storage
/// directly, since the trait surface deliberately cannot.
#[cfg(feature = "runtime-benchmarks")]
pub struct PeopleAirdropsBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl crate::benchmarking::BenchmarkHelper<Test> for PeopleAirdropsBenchmarkHelper {
	fn fund_prize_source(source: &u64, draws: u32) -> Vec<crate::pallet::AirdropEventInfoOf<Test>> {
		use frame_support::traits::fungibles::{Create, Inspect, Mutate};
		use indiv_pallet_airdrop::pallet::SupportedAssets;
		const BENCH_ASSET_BASE: u32 = 42;
		const BENCH_PRIZE: u64 = 1_000;
		let pot = indiv_pallet_airdrop::Pallet::<Test>::airdrop_pot_id();
		// One distinct asset per draw so a batch schedule touches distinct asset storage per draw
		// (see the `BenchmarkHelper` trait doc).
		(0..draws)
			.map(|i| {
				let asset_id = BENCH_ASSET_BASE + i;
				if !<Assets as Inspect<u64>>::asset_exists(asset_id) {
					<Assets as Create<u64>>::create(asset_id, *source, true, 1)
						.expect("create asset");
				}
				// Mirror `enable_asset`: mark the asset supported and keep the pot's asset account
				// alive.
				if !SupportedAssets::<Test>::contains_key(asset_id) {
					Assets::mint_into(asset_id, &pot, 1).expect("fund pot ed");
					SupportedAssets::<Test>::insert(asset_id, 1u64);
				}
				Assets::mint_into(asset_id, source, BENCH_PRIZE).expect("fund source");
				crate::pallet::AirdropEventInfoOf::<Test> {
					prize: indiv_pallet_airdrop::types::AirdropPrize {
						asset_id,
						asset_amount: BENCH_PRIZE,
						max_winners: 1,
						winner_cap: sp_runtime::Permill::one(),
					},
					registration_starts: 100,
					draw_time: 200,
					end_time: 300,
				}
			})
			.collect()
	}

	fn open_registration(event_id: &crate::EventId) {
		indiv_pallet_airdrop::pallet::Events::<Test>::mutate(event_id, |event| {
			if let Some(event) = event {
				event.status =
					indiv_pallet_airdrop::types::Status::Registering { total_participants: 0 };
			}
		});
	}

	fn start_claiming(event_id: &crate::EventId) {
		use indiv_pallet_airdrop::pallet::{Registrations, Winners};
		let registrations =
			Registrations::<Test>::iter_prefix(event_id).collect::<alloc::vec::Vec<_>>();
		for (slot, entry) in &registrations {
			Winners::<Test>::insert(event_id, entry.clone(), *slot);
		}
		indiv_pallet_airdrop::pallet::Events::<Test>::mutate(event_id, |event| {
			if let Some(event) = event {
				event.status = indiv_pallet_airdrop::types::Status::Claiming {
					total_participants: registrations.len() as u32,
					effective_winners: registrations.len() as u32,
					claimed: 0,
				};
			}
		});
	}

	fn count_registrations(event_id: &crate::EventId) -> u32 {
		indiv_pallet_airdrop::pallet::Registrations::<Test>::iter_prefix(event_id).count() as u32
	}

	fn count_winners(event_id: &crate::EventId) -> u32 {
		indiv_pallet_airdrop::pallet::Winners::<Test>::iter_prefix(event_id).count() as u32
	}

	fn set_unix_time(now_secs: u64) {
		set_now_secs(now_secs);
	}
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	set_now_secs(0);
	set_randomness(Some(([42u8; 32], 1)));
	let storage = RuntimeGenesisConfig::default().build_storage().unwrap();
	let mut ext = sp_io::TestExternalities::from(storage);
	let (offchain, _state) = TestOffchainExt::new();
	let (pool, pool_state) = TestTransactionPoolExt::new();
	TRANSACTION_POOL.set(pool_state);
	ext.register_extension(OffchainDbExt::new(offchain.clone()));
	ext.register_extension(OffchainWorkerExt::new(offchain));
	ext.register_extension(TransactionPoolExt::new(pool));
	ext.execute_with(|| System::set_block_number(1));
	ext
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

/// Number of transactions currently sitting in the mock transaction pool.
pub fn pending_ocw_tx_count() -> usize {
	TRANSACTION_POOL.with(|pool| pool.borrow().read().transactions.len())
}

/// Drain the transaction pool and apply every submitted authorized transaction.
pub fn drain_ocw_transactions() {
	let transactions =
		TRANSACTION_POOL.with(|pool| core::mem::take(&mut pool.borrow().write().transactions));
	for raw in transactions {
		let tx = Extrinsic::decode(&mut &raw[..]).expect("ocw tx decodes");
		Executive::apply_extrinsic(tx)
			.expect("ocw tx valid for block")
			.expect("ocw tx dispatch ok");
	}
}

/// Run the airdrop pallet's offchain worker for the current block, then drain the transaction
/// pool and apply every submitted authorized transaction. Each call corresponds to a single OCW
/// tick and also advances the mock randomness by one block.
pub fn run_to_next_ocw() {
	use frame_support::traits::Hooks;
	advance_randomness();
	let block = frame_system::Pallet::<Test>::block_number();
	indiv_pallet_airdrop::Pallet::<Test>::offchain_worker(block);
	drain_ocw_transactions();
}
