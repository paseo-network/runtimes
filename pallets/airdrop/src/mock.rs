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
use core::{cell::RefCell, ops::Range, time::Duration};
use frame_support::{
	derive_impl, parameter_types,
	traits::{AsEnsureOriginWithArg, UnixTime},
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
use sp_runtime::{
	offchain::{
		testing::{PoolState, TestOffchainExt, TestTransactionPoolExt},
		OffchainDbExt, OffchainWorkerExt, TransactionPoolExt,
	},
	testing::UintAuthorityId,
	traits::TryConvert,
	BuildStorage, DispatchError,
};
use verifiable::{mock::Mock, AliasVec, Entropy, Error, GenerateVerifiable};

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
		Airdrop: crate,
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

parameter_types! {
	/// The mock source's observation lookahead, covered by its commitment moment.
	pub storage RandomnessLookahead: u32 = 0;
}

pub struct MockRandomness;
impl indiv_support::traits::MomentRandomness<u32> for MockRandomness {
	fn randomness() -> Option<([u8; 32], u32)> {
		RANDOMNESS.with(|r| *r.borrow())
	}

	fn current_moment() -> u32 {
		RANDOMNESS.with(|r| r.borrow().map_or(0, |(_, moment)| moment)) + RandomnessLookahead::get()
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
			*r = Some((value, moment.saturating_sub(RandomnessLookahead::get())));
		});
	}
}

pub fn set_draw_limit(limit: u32) {
	DrawLimitValue::set(&limit);
}

/// Thin wrapper around the [`Mock`] crypto. Same shape as `Mock` for every method except
/// `validate_multi_context` / `create_multi_context`, which carry an extra "chosen alias"
/// alongside the underlying mock proof so tests can place participants at specific slot keys.
pub struct MockCrypto;

/// `MockCrypto::Proof = (MockProof, chosen_alias)`. The wrapped inner proof is what `Mock`
/// validates; the chosen alias is what the mock prover surfaces back to the pallet as the
/// contextual alias (i.e. the slot key).
pub type MockProof = (<Mock as GenerateVerifiable>::Proof, Alias);

impl GenerateVerifiable for MockCrypto {
	type Members = <Mock as GenerateVerifiable>::Members;
	type Intermediate = <Mock as GenerateVerifiable>::Intermediate;
	type Member = <Mock as GenerateVerifiable>::Member;
	type Secret = <Mock as GenerateVerifiable>::Secret;
	type Commitment = <Mock as GenerateVerifiable>::Commitment;
	type Proof = MockProof;
	type Signature = <Mock as GenerateVerifiable>::Signature;
	type StaticChunk = <Mock as GenerateVerifiable>::StaticChunk;
	type Config = <Mock as GenerateVerifiable>::Config;

	fn start_members(config: Self::Config) -> Self::Intermediate {
		Mock::start_members(config)
	}
	fn push_members(
		inter: &mut Self::Intermediate,
		members: impl Iterator<Item = Self::Member>,
		lookup: impl Fn(Range<usize>) -> Result<Vec<Self::StaticChunk>, ()>,
	) -> Result<(), Error> {
		Mock::push_members(inter, members, lookup)
	}
	fn finish_members(inter: Self::Intermediate) -> Self::Members {
		Mock::finish_members(inter)
	}
	fn new_secret(entropy: Entropy) -> Self::Secret {
		Mock::new_secret(entropy)
	}
	fn member_from_secret(secret: &Self::Secret) -> Self::Member {
		Mock::member_from_secret(secret)
	}
	fn open(
		config: Self::Config,
		member: &Self::Member,
		members: impl Iterator<Item = Self::Member>,
	) -> Result<Self::Commitment, Error> {
		Mock::open(config, member, members)
	}
	fn create_multi_context(
		commitment: Self::Commitment,
		secret: &Self::Secret,
		contexts: &[&[u8]],
		message: &[u8],
	) -> Result<(Self::Proof, AliasVec), Error> {
		// Default chosen-alias is whatever the inner Mock derives for the first context; tests
		// that want a specific slot key call `mock_proof` directly.
		let (inner_proof, aliases) =
			Mock::create_multi_context(commitment, secret, contexts, message)?;
		let chosen = aliases.first().copied().ok_or(Error::ContextCountMismatch)?;
		Ok(((inner_proof, chosen), aliases))
	}
	fn validate_multi_context(
		config: Self::Config,
		proof: &Self::Proof,
		members: &Self::Members,
		contexts: &[&[u8]],
		message: &[u8],
	) -> Result<AliasVec, Error> {
		// Reuse Mock's verification (binds context+message via SHA-256 tag); on success surface
		// the chosen alias instead of Mock's member-derived one so the slot key is what the test
		// picked.
		Mock::validate_multi_context(config, &proof.0, members, contexts, message)?;
		Ok(core::iter::once(proof.1).collect())
	}
	fn sign(secret: &Self::Secret, message: &[u8]) -> Result<Self::Signature, Error> {
		Mock::sign(secret, message)
	}
	fn verify_signature(
		signature: &Self::Signature,
		message: &[u8],
		member: &Self::Member,
	) -> bool {
		Mock::verify_signature(signature, message, member)
	}
	fn alias_in_context(secret: &Self::Secret, context: &[u8]) -> Result<Alias, Error> {
		Mock::alias_in_context(secret, context)
	}
	fn is_member_valid(member: &Self::Member) -> bool {
		Mock::is_member_valid(member)
	}
}

/// Fixed secret used to back every mock proof — the actual member identity doesn't matter for
/// these tests, only that the inner Mock proof over `(context, msg)` is valid.
const MOCK_SECRET: [u8; 32] = [42u8; 32];

/// Build a `MockProof` over the per-event context derived from `event_id`, committing to `msg`
/// (typically `participant_origin.encode()`), and tagged with `chosen_alias` so the mock prover
/// surfaces that exact alias as the contextual alias / slot key.
pub fn mock_proof(event_id: &crate::EventId, chosen_alias: Alias, msg: &[u8]) -> MockProof {
	let context = crate::context_for_event(event_id);
	let member = Mock::member_from_secret(&MOCK_SECRET);
	let mut inter = Mock::start_members(());
	Mock::push_members(&mut inter, core::iter::once(member), |_| Ok(Vec::new()))
		.expect("single member fits");
	let members = Mock::finish_members(inter);
	let commitment = Mock::open((), &member, members.iter().copied()).expect("open ok");
	let (inner_proof, _) =
		Mock::create_multi_context(commitment, &MOCK_SECRET, &[&context], msg).expect("sign ok");
	(inner_proof, chosen_alias)
}

/// One-element members set holding the proof's underlying mock pubkey, so `Mock::validate_*`
/// accepts it without test-side ring registration.
fn members_for_proof(proof: &MockProof) -> <Mock as GenerateVerifiable>::Members {
	let mut inter = Mock::start_members(());
	Mock::push_members(&mut inter, core::iter::once(proof.0.member), |_| Ok(Vec::new()))
		.expect("single member fits");
	Mock::finish_members(inter)
}

pub struct MockMemberService;

impl MembershipProver for MockMemberService {
	type Crypto = MockCrypto;

	fn verify_membership(
		_identifier: &Identifier,
		proof: &MockProof,
		_ring_index: RingIndex,
		_revision: RevisionIndex,
		context: Context,
		msg: &[u8],
	) -> Result<ContextualAlias, DispatchError> {
		let alias = MockCrypto::validate((), proof, &members_for_proof(proof), &context, msg)
			.map_err(|_| DispatchError::Other("mock proof invalid"))?;
		Ok(ContextualAlias { context, alias })
	}

	fn verify_memberships_in_ring(
		_identifier: &Identifier,
		_ring_index: RingIndex,
		_revision: RevisionIndex,
		_items: &[RingMembershipProof<MockProof>],
	) -> Result<Vec<ContextualAlias>, DispatchError> {
		unimplemented!()
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

// Inverse direction (AccountId → sr25519::Public) can't be reconstructed for
// a `u64` account id since the public key is 32 bytes. Tests register their
// pair's public key here before exercising the account-participation path.
parameter_types! {
	pub storage AccountToPublic: alloc::collections::BTreeMap<u64, sr25519::Public> =
		alloc::collections::BTreeMap::new();
}

pub fn register_account_pubkey(account_id: u64, public: sr25519::Public) {
	let mut map = AccountToPublic::get();
	map.insert(account_id, public);
	AccountToPublic::set(&map);
}

/// Forward helper used by tests to compute the `u64` account id corresponding
/// to a `sr25519::Public` (low 8 bytes interpreted little-endian).
pub fn account_id_for(public: sr25519::Public) -> u64 {
	u64::from_le_bytes(public.0[0..8].try_into().expect("sr25519::Public is 32 bytes"))
}

pub struct AccountToPub;
impl TryConvert<u64, sr25519::Public> for AccountToPub {
	fn try_convert(account_id: u64) -> Result<sr25519::Public, u64> {
		AccountToPublic::get().get(&account_id).copied().ok_or(account_id)
	}
}

parameter_types! {
	pub AirdropPalletId: frame_support::PalletId = frame_support::PalletId(*b"pop/adrp");
	pub storage ClearLimitValue: u32 = 100;
	pub storage DrawLimitValue: u32 = 100;
	pub const OcwInterval: u64 = 1;
}

impl crate::Config for Test {
	type WeightInfo = ();
	type MemberService = MockMemberService;
	type Fungibles = indiv_support::fungibles::CombineAssetsWithHolder<Assets, AssetsHolder>;
	type ManagerOrigin = EnsureRoot<u64>;
	type PalletId = AirdropPalletId;
	type UnixTime = MockTime;
	type Randomness = MockRandomness;
	type AccountIdToPublic = AccountToPub;
	type ClearLimit = ClearLimitValue;
	type DrawLimit = DrawLimitValue;
	type OffchainWorkerInterval = OcwInterval;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = AirdropBenchmarkHelper;
}

/// Benchmark setup hooks for the mock runtime.
#[cfg(feature = "runtime-benchmarks")]
pub struct AirdropBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl crate::benchmarking::BenchmarkHelper<Test> for AirdropBenchmarkHelper {
	fn set_unix_time(now: core::time::Duration) {
		TIME.with(|t| *t.borrow_mut() = now);
	}

	fn create_asset_id_parameter(id: u32) -> u32 {
		id
	}

	fn build_membership_proof(
		context: &indiv_support::traits::Context,
		message: &[u8],
		member_seed: u32,
	) -> (crate::ProofOf<Test>, Alias) {
		// Mock-only: derive the alias directly from `member_seed`. The mock prover trusts whatever
		// alias is tagged onto the proof, so we don't need to wire `member_seed` through a real
		// keypair → alias derivation. A production runtime would.
		let mut alias = [0u8; 32];
		alias[28..32].copy_from_slice(&member_seed.to_le_bytes());
		let member = Mock::member_from_secret(&MOCK_SECRET);
		let mut inter = Mock::start_members(());
		Mock::push_members(&mut inter, core::iter::once(member), |_| Ok(Vec::new()))
			.expect("single member fits");
		let members = Mock::finish_members(inter);
		let commitment = Mock::open((), &member, members.iter().copied()).expect("open ok");
		let (inner_proof, _) =
			Mock::create_multi_context(commitment, &MOCK_SECRET, &[context], message)
				.expect("sign ok");
		((inner_proof, alias), alias)
	}

	fn account_keypair_for(seed: u32) -> (u64, sp_core::sr25519::Pair) {
		use sp_core::Pair as _;
		let mut entropy = [0u8; 32];
		entropy[28..32].copy_from_slice(&seed.to_le_bytes());
		let pair = sp_core::sr25519::Pair::from_seed(&entropy);
		let public = pair.public();
		let account_id = account_id_for(public);
		register_account_pubkey(account_id, public);
		(account_id, pair)
	}
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	set_now_secs(0);
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

/// Run the pallet's offchain worker for the current block, then drain the
/// transaction pool and apply every submitted authorized transaction —
/// equivalent to one block's worth of OCW activity. Each call corresponds to
/// a single OCW tick: an event whose lifecycle needs N transitions to reach
/// its next phase boundary needs N calls (e.g. multi-batch draws and clean-ups
/// take multiple ticks each). Each tick also advances the mock randomness by
/// one block, mirroring the fresh relay VRF the real chain sees every block.
pub fn run_to_next_ocw() {
	use frame_support::traits::Hooks;
	advance_randomness();
	let block = frame_system::Pallet::<Test>::block_number();
	crate::Pallet::<Test>::offchain_worker(block);
	let transactions =
		TRANSACTION_POOL.with(|pool| core::mem::take(&mut pool.borrow().write().transactions));
	for raw in transactions {
		let tx = Extrinsic::decode(&mut &raw[..]).expect("ocw tx decodes");
		Executive::apply_extrinsic(tx)
			.expect("ocw tx valid for block")
			.expect("ocw tx dispatch ok");
	}
}
