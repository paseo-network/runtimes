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

//! Mock runtime for testing the alias-accounts pallet.

extern crate alloc;

use alloc::{collections::BTreeMap, vec::Vec};
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use core::{cell::RefCell, ops::Range};
use frame_support::{
	derive_impl,
	dispatch::{GetDispatchInfo, PostDispatchInfo},
	parameter_types,
	traits::{fungibles::Mutate as _, AsEnsureOriginWithArg},
	weights::Weight,
};
use frame_system::{
	offchain::{CreateAuthorizedTransaction, CreateTransaction, CreateTransactionBase},
	AuthorizeCall,
};
use indiv_support::traits::{Alias, ContextualAlias, MembershipProver, RingMembershipProof};
pub use indiv_support::traits::{Context, Identifier, RevisionIndex, RingExponent, RingIndex};
use scale_info::TypeInfo;
use sp_core::ConstU32;
use sp_runtime::{
	offchain::{
		testing::{PoolState, TestOffchainExt, TestTransactionPoolExt},
		OffchainDbExt, OffchainWorkerExt, TransactionPoolExt,
	},
	traits::{Applyable, Checkable},
	transaction_validity::TransactionValidityError,
	BoundedVec, BuildStorage, DispatchError, DispatchErrorWithPostInfo,
};
use verifiable::{AliasVec, Entropy, Error as VerifiableError, GenerateVerifiable};

use crate::types::AliasAccountInfo;

frame_support::construct_runtime!(
	pub enum Test
	{
		System: frame_system,
		Balances: pallet_balances,
		PalletAssets: pallet_assets,
		AliasAccounts: crate,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Block = frame_system::mocking::MockBlock<Test>;
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
	type Holder = ();
}

parameter_types! {
	pub const ProofValidityWindow: u64 = 100;
	pub storage MappingRetention: u64 = 86_400;
	pub const PeopleLiteCollection: Identifier = *crate::PEOPLE_LITE_IDENTIFIER;
	pub const PeopleLiteRingExp: RingExponent = RingExponent::R2e9;
	pub const PeopleCollection: Identifier = *crate::PEOPLE_IDENTIFIER;
	pub const PeopleRingExp: RingExponent = RingExponent::R2e9;
	pub const PgasAssetId: u32 = 1;
	pub storage AliasFee: Option<u64> = None;
	/// Every block, so a test drives one sweep per block it advances.
	pub storage OffchainWorkerInterval: u64 = 1;
	/// Small, so a test can fill a batch without seeding dozens of mappings.
	pub storage MaxStaleAliasBatch: u32 = 4;
}

#[cfg(feature = "runtime-benchmarks")]
pub const MAX_MOCK_RING_REVISIONS: u32 = 3;

// ========== Mock Time ==========

/// Reference Unix timestamp used as the initial mock time. All tests start with
/// `MockUnixTime::now()` returning this value, and may advance from it.
pub const MOCK_GENESIS_TIME: u64 = 1_700_000_000;

thread_local! {
	pub static MOCK_NOW: RefCell<u64> = const { RefCell::new(MOCK_GENESIS_TIME) };
}

pub fn set_mock_time(t: u64) {
	MOCK_NOW.with(|v| *v.borrow_mut() = t);
}

pub struct MockUnixTime;

impl frame_support::traits::UnixTime for MockUnixTime {
	fn now() -> core::time::Duration {
		core::time::Duration::from_secs(MOCK_NOW.with(|v| *v.borrow()))
	}
}

// ========== Mock Contexts and Collections ==========

pub const PEOPLE_CONTEXT: Context = *b"pop:polkadot.network/people-aa  ";
pub const PEOPLE_LITE_CONTEXT: Context = *b"pop:polkadot.network/plite-aa   ";
pub const INVALID_CONTEXT: Context = [99u8; 32];
pub const INVALID_COLLECTION: Identifier = [99u8; 32];

// ========== Mock Proof ==========

/// Mock proof type for testing.
#[derive(
	Clone, PartialEq, Eq, Debug, Encode, Decode, TypeInfo, MaxEncodedLen, DecodeWithMemTracking,
)]
pub struct MockProof {
	pub alias: Alias,
	pub valid: bool,
}

// ========== Mock Member Service ==========

/// One revision record stored by `MockMemberService`.
#[derive(Clone, Debug)]
struct MockRingRecord {
	revision: RevisionIndex,
	source_time: u64,
}

thread_local! {
	/// Per-ring revisions, ordered oldest-to-newest. The last entry is the latest revision.
	static MOCK_RING_ROOTS: RefCell<BTreeMap<(Identifier, RingIndex), Vec<MockRingRecord>>> =
		const { RefCell::new(BTreeMap::new()) };
	/// Collections marked as "configured" — proof verification only succeeds for these.
	/// Mirrors `RingCollectionExponents` membership in the production `MemberService`.
	static MOCK_KNOWN_COLLECTIONS: RefCell<BTreeMap<Identifier, ()>> =
		const { RefCell::new(BTreeMap::new()) };
}

/// Marks `identifier` as a configured collection so `MockMemberService` will accept proofs
/// against it.
pub fn seed_collection_exponent(identifier: Identifier) {
	MOCK_KNOWN_COLLECTIONS.with(|c| {
		c.borrow_mut().insert(identifier, ());
	});
}

fn collection_is_known(identifier: &Identifier) -> bool {
	MOCK_KNOWN_COLLECTIONS.with(|c| c.borrow().contains_key(identifier))
}

/// Replaces all records at `(identifier, ring_index)` with a single record at `revision`,
/// stamped with the current mock time.
pub fn set_mock_ring_revision(
	identifier: Identifier,
	ring_index: RingIndex,
	revision: RevisionIndex,
) {
	let now = MOCK_NOW.with(|v| *v.borrow());
	MOCK_RING_ROOTS.with(|m| {
		m.borrow_mut().insert(
			(identifier, ring_index),
			alloc::vec![MockRingRecord { revision, source_time: now }],
		);
	});
}

/// Appends a new ring root revision without clearing existing ones.
pub fn push_mock_ring_revision(
	identifier: Identifier,
	ring_index: RingIndex,
	revision: RevisionIndex,
) {
	let now = MOCK_NOW.with(|v| *v.borrow());
	push_record(identifier, ring_index, MockRingRecord { revision, source_time: now });
}

fn push_record(identifier: Identifier, ring_index: RingIndex, record: MockRingRecord) {
	MOCK_RING_ROOTS.with(|m| {
		m.borrow_mut().entry((identifier, ring_index)).or_default().push(record);
	});
}

pub fn remove_mock_ring_root(identifier: Identifier, ring_index: RingIndex) {
	MOCK_RING_ROOTS.with(|m| {
		m.borrow_mut().remove(&(identifier, ring_index));
	});
}

/// Retention applied by [`MockMemberService`], mirroring the member service's
/// `OldRootRetentionDuration`: a superseded revision expires this many seconds after its
/// successor's source time. The latest revision never expires.
pub const MOCK_OLD_ROOT_RETENTION: u64 = 3600;

pub struct MockMemberService;

impl MockMemberService {
	fn ring_records(identifier: &Identifier, ring_index: RingIndex) -> Option<Vec<MockRingRecord>> {
		MOCK_RING_ROOTS.with(|m| m.borrow().get(&(*identifier, ring_index)).cloned())
	}

	fn record_is_retained(records: &[MockRingRecord], index: usize) -> bool {
		let Some(successor) = records.get(index.saturating_add(1)) else {
			return true;
		};
		let now = MOCK_NOW.with(|v| *v.borrow());
		now < successor.source_time.saturating_add(MOCK_OLD_ROOT_RETENTION)
	}
}

impl MembershipProver for MockMemberService {
	type Crypto = TestVerifiable;

	fn verify_membership(
		identifier: &Identifier,
		proof: &<Self::Crypto as GenerateVerifiable>::Proof,
		ring_index: RingIndex,
		revision: RevisionIndex,
		context: Context,
		msg: &[u8],
	) -> Result<ContextualAlias, DispatchError> {
		if !collection_is_known(identifier) {
			return Err(DispatchError::Other("mock collection not configured"));
		}
		let records = Self::ring_records(identifier, ring_index)
			.ok_or(DispatchError::Other("mock ring missing"))?;
		let index = records
			.iter()
			.position(|r| r.revision == revision)
			.ok_or(DispatchError::Other("mock revision missing"))?;
		if !Self::record_is_retained(&records, index) {
			return Err(DispatchError::Other("mock revision expired"));
		}
		let alias = TestVerifiable::validate((), proof, &BoundedVec::new(), &context[..], msg)
			.map_err(|_| DispatchError::Other("invalid proof"))?;
		Ok(ContextualAlias { alias, context })
	}

	fn verify_memberships_in_ring(
		_identifier: &Identifier,
		_ring_index: RingIndex,
		_revision: RevisionIndex,
		_items: &[RingMembershipProof<<Self::Crypto as GenerateVerifiable>::Proof>],
	) -> Result<Vec<ContextualAlias>, DispatchError> {
		unimplemented!("alias-accounts mock does not use batch verification")
	}

	fn ring_revision(identifier: &Identifier, ring_index: RingIndex) -> Option<RevisionIndex> {
		Self::ring_records(identifier, ring_index)?.last().map(|r| r.revision)
	}

	fn is_revision_valid(
		identifier: &Identifier,
		ring_index: RingIndex,
		revision: RevisionIndex,
	) -> bool {
		Self::ring_records(identifier, ring_index).is_some_and(|rs| {
			rs.iter()
				.position(|r| r.revision == revision)
				.is_some_and(|index| Self::record_is_retained(&rs, index))
		})
	}

	fn revision_source_time(
		identifier: &Identifier,
		ring_index: RingIndex,
		revision: RevisionIndex,
	) -> Option<u64> {
		Self::ring_records(identifier, ring_index)?
			.into_iter()
			.find(|r| r.revision == revision)
			.map(|r| r.source_time)
	}

	fn old_root_retention() -> u64 {
		MOCK_OLD_ROOT_RETENTION
	}
}

// ========== Mock Crypto (TestVerifiable) ==========

pub struct TestVerifiable;

impl GenerateVerifiable for TestVerifiable {
	type Members = BoundedVec<u64, ConstU32<16>>;
	type Intermediate = BoundedVec<u64, ConstU32<16>>;
	type Member = u64;
	type Secret = u64;
	type Commitment = (u64, Vec<u64>);
	type Proof = MockProof;
	type Signature = ();
	type StaticChunk = ();
	type Config = ();

	fn start_members(_config: Self::Config) -> Self::Intermediate {
		BoundedVec::new()
	}

	fn push_members(
		inter: &mut Self::Intermediate,
		members: impl Iterator<Item = Self::Member>,
		_lookup: impl Fn(Range<usize>) -> Result<Vec<Self::StaticChunk>, ()>,
	) -> Result<(), VerifiableError> {
		for m in members {
			inter.try_push(m).map_err(|_| VerifiableError::SetFull)?;
		}
		Ok(())
	}

	fn finish_members(inter: Self::Intermediate) -> Self::Members {
		inter
	}

	fn new_secret(entropy: Entropy) -> Self::Secret {
		entropy[0] as u64
	}

	fn member_from_secret(secret: &Self::Secret) -> Self::Member {
		*secret
	}

	fn open(
		_config: Self::Config,
		member: &Self::Member,
		members: impl Iterator<Item = Self::Member>,
	) -> Result<Self::Commitment, VerifiableError> {
		let set: Vec<_> = members.collect();
		if !set.contains(member) {
			return Err(VerifiableError::NotInRing);
		}
		Ok((*member, set))
	}

	fn create_multi_context(
		_commitment: Self::Commitment,
		secret: &Self::Secret,
		contexts: &[&[u8]],
		_message: &[u8],
	) -> Result<(Self::Proof, AliasVec), VerifiableError> {
		let first = contexts.first().ok_or(VerifiableError::ContextCountMismatch)?;
		let alias = Self::alias_in_context(secret, first)?;
		Ok((
			MockProof { alias, valid: true },
			core::iter::repeat_n(alias, contexts.len()).collect(),
		))
	}

	fn validate_multi_context(
		_config: Self::Config,
		proof: &Self::Proof,
		_members: &Self::Members,
		contexts: &[&[u8]],
		_message: &[u8],
	) -> Result<AliasVec, VerifiableError> {
		if proof.valid {
			Ok(core::iter::repeat_n(proof.alias, contexts.len()).collect())
		} else {
			Err(VerifiableError::VerificationFailed)
		}
	}

	fn sign(_secret: &Self::Secret, _message: &[u8]) -> Result<Self::Signature, VerifiableError> {
		Ok(())
	}

	fn verify_signature(
		_signature: &Self::Signature,
		_message: &[u8],
		_member: &Self::Member,
	) -> bool {
		true
	}

	fn alias_in_context(
		secret: &Self::Secret,
		context: &[u8],
	) -> Result<verifiable::Alias, VerifiableError> {
		// Deterministic: derive a 32-byte alias from (secret, context) so `create` and
		// `validate` agree, and the bench can compute the resulting alias up front.
		let mut bytes = [0u8; 32];
		bytes[..8].copy_from_slice(&secret.to_le_bytes());
		let ctx_len = context.len().min(24);
		bytes[8..8 + ctx_len].copy_from_slice(&context[..ctx_len]);
		Ok(bytes)
	}

	fn is_member_valid(_member: &Self::Member) -> bool {
		true
	}
}

// ========== Authorized Transaction Support ==========

pub type TxExtension = (AuthorizeCall<Test>,);

pub type Extrinsic = sp_runtime::generic::UncheckedExtrinsic<
	u64,
	RuntimeCall,
	sp_runtime::testing::UintAuthorityId,
	TxExtension,
>;

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

// ========== Ring Aliases Config ==========

/// Distinct, non-zero weights for the three sweep calls, so a test can tell one call's charge
/// from another's, and a per-item term so a batch is charged for its size. Every other call
/// delegates to `()`, the substrate weights the tests used before the split.
pub struct MockWeightInfo;

impl crate::WeightInfo for MockWeightInfo {
	fn report_stale_aliases(n: u32) -> Weight {
		Weight::from_parts(10_000, 100)
			.saturating_add(Weight::from_parts(1_000, 10).saturating_mul(n.into()))
	}

	fn authorize_report_stale_aliases(n: u32) -> Weight {
		Weight::from_parts(1_000, 10)
			.saturating_add(Weight::from_parts(100, 1).saturating_mul(n.into()))
	}

	fn retire_stale_aliases(n: u32) -> Weight {
		Weight::from_parts(30_000, 300)
			.saturating_add(Weight::from_parts(3_000, 30).saturating_mul(n.into()))
	}

	fn authorize_retire_stale_aliases(n: u32) -> Weight {
		Weight::from_parts(3_000, 30)
			.saturating_add(Weight::from_parts(300, 3).saturating_mul(n.into()))
	}

	fn clear_stale_alias_reports(n: u32) -> Weight {
		Weight::from_parts(5_000, 50)
			.saturating_add(Weight::from_parts(500, 5).saturating_mul(n.into()))
	}

	fn authorize_clear_stale_alias_reports(n: u32) -> Weight {
		Weight::from_parts(500, 5)
			.saturating_add(Weight::from_parts(50, 1).saturating_mul(n.into()))
	}

	fn set_alias_account() -> Weight {
		<() as crate::WeightInfo>::set_alias_account()
	}

	fn unset_alias_account() -> Weight {
		<() as crate::WeightInfo>::unset_alias_account()
	}

	fn personhood_info() -> Weight {
		<() as crate::WeightInfo>::personhood_info()
	}

	fn personhood_info_by_proof() -> Weight {
		<() as crate::WeightInfo>::personhood_info_by_proof()
	}

	fn reprove_alias_account() -> Weight {
		<() as crate::WeightInfo>::reprove_alias_account()
	}
}

impl crate::Config for Test {
	type WeightInfo = MockWeightInfo;
	type MemberService = MockMemberService;
	type UnixTime = MockUnixTime;
	type ProofValidityWindow = ProofValidityWindow;
	type MappingRetention = MappingRetention;
	type PeopleLiteRingExponent = PeopleLiteRingExp;
	type PeopleRingExponent = PeopleRingExp;
	type Fungibles = PalletAssets;
	type PgasAssetId = PgasAssetId;
	type AliasFee = AliasFee;
	type OffchainWorkerInterval = OffchainWorkerInterval;
	type MaxStaleAliasBatch = MaxStaleAliasBatch;
}

// ========== Test Helpers ==========

pub fn new_test_ext() -> sp_io::TestExternalities {
	let t = frame_system::GenesisConfig::<Test>::default().build_storage().unwrap();
	let mut ext = sp_io::TestExternalities::new(t);
	// The offchain worker submits the sweeps as authorized transactions, so it needs somewhere to
	// submit them to.
	let (offchain, _offchain_state) = TestOffchainExt::new();
	let (pool, pool_state) = TestTransactionPoolExt::new();
	TRANSACTION_POOL.with_borrow_mut(|slot| *slot = pool_state);
	ext.register_extension(OffchainDbExt::new(offchain.clone()));
	ext.register_extension(OffchainWorkerExt::new(offchain));
	ext.register_extension(TransactionPoolExt::new(pool));
	ext.execute_with(|| {
		// Reset thread-local state shared across tests.
		MOCK_RING_ROOTS.with(|m| m.borrow_mut().clear());
		MOCK_KNOWN_COLLECTIONS.with(|c| c.borrow_mut().clear());
		set_mock_time(MOCK_GENESIS_TIME);
		seed_collection_exponent(PeopleCollection::get());
		set_mock_ring_revision(PeopleCollection::get(), 0, 1);
	});
	ext
}

/// Account that owns the PGAS asset in tests.
pub const PGAS_ADMIN: u64 = 999;

/// Create the PGAS asset and mint `amount` to `who`. Asset is sufficient with min balance 1.
pub fn setup_pgas_for(who: u64, amount: u64) {
	use frame_support::traits::fungibles::{Create, Inspect};
	if !<PalletAssets as Inspect<u64>>::asset_exists(PgasAssetId::get()) {
		<PalletAssets as Create<u64>>::create(PgasAssetId::get(), PGAS_ADMIN, true, 1)
			.expect("create pgas asset");
	}
	PalletAssets::mint_into(PgasAssetId::get(), &who, amount).expect("mint pgas");
}

/// Read the PGAS balance of `who`.
pub fn pgas_balance(who: u64) -> u64 {
	use frame_support::traits::fungibles::Inspect;
	<PalletAssets as Inspect<u64>>::balance(PgasAssetId::get(), &who)
}

/// Helper to create an AliasAccountInfo with custom collection, revision, and ring.
pub fn make_alias_info_for(
	collection: Identifier,
	alias: Alias,
	context: Context,
	revision: RevisionIndex,
	ring: RingIndex,
) -> AliasAccountInfo {
	AliasAccountInfo { collection, revision, ring, ca: ContextualAlias { alias, context } }
}

/// Helper to create an AliasAccountInfo for the People collection with default
/// revision (1) and ring (0), matching `new_test_ext` setup.
pub fn make_alias_info(alias: Alias, context: Context) -> AliasAccountInfo {
	make_alias_info_for(PeopleCollection::get(), alias, context, 1, 0)
}

// ========== Benchmark Helpers ==========

#[cfg(feature = "runtime-benchmarks")]
impl crate::benchmarking::BenchmarkHelper<Test> for Test {
	fn set_time(seconds: u64) {
		set_mock_time(seconds);
	}

	fn allowed_context() -> Context {
		PEOPLE_CONTEXT
	}

	fn mock_proof(seed: u32, _context: Context, _msg: &[u8]) -> (MockProof, Alias) {
		let alias: Alias = [seed as u8; 32];
		(MockProof { alias, valid: true }, alias)
	}

	fn create_proof_for_revision(
		_identifier: &Identifier,
		_ring_index: RingIndex,
		_revision: RevisionIndex,
		_context: &Context,
		_message: &[u8],
	) -> <TestVerifiable as GenerateVerifiable>::Proof {
		// `TestVerifiable::validate` is permissive (ignores members/context/message
		// and only checks the `valid` flag), so we can return a hand-crafted proof
		// without going through `Crypto::open` + `Crypto::create`. Production
		// runtimes should follow the convention established in `pallet-pgas`'s
		// mock: register a member, build the ring, and produce a real proof via
		// `Crypto::create`.
		MockProof { alias: [0x42u8; 32], valid: true }
	}

	fn setup_pgas_asset() {
		use frame_support::traits::fungibles::{Create, Inspect};
		if !<PalletAssets as Inspect<u64>>::asset_exists(PgasAssetId::get()) {
			<PalletAssets as Create<u64>>::create(PgasAssetId::get(), PGAS_ADMIN, true, 1)
				.expect("create pgas asset");
		}
	}

	fn set_alias_fee(fee: u64) {
		AliasFee::set(&Some(fee));
	}

	fn max_ring_revisions() -> u32 {
		MAX_MOCK_RING_REVISIONS
	}

	fn seed_ring(collection: Identifier, ring: RingIndex, revisions: u32, source_time: u64) {
		seed_collection_exponent(collection);
		// Replace any existing records for a clean worst-case state.
		MOCK_RING_ROOTS.with(|m| {
			m.borrow_mut().remove(&(collection, ring));
		});
		for revision in 0..revisions {
			push_record(collection, ring, MockRingRecord { revision, source_time });
		}
	}
}

thread_local! {
	/// The pool the offchain worker submits its sweeps to, replaced by every `new_test_ext`.
	static TRANSACTION_POOL: core::cell::RefCell<std::sync::Arc<parking_lot::RwLock<PoolState>>> =
		core::cell::RefCell::new(std::sync::Arc::new(parking_lot::RwLock::new(PoolState {
			transactions: Vec::new(),
		})));
}

/// The calls of the transactions the offchain worker has submitted, without draining them.
pub fn submitted_calls() -> Vec<RuntimeCall> {
	TRANSACTION_POOL
		.with_borrow(|pool| pool.read().transactions.clone())
		.into_iter()
		.map(|transaction| {
			Extrinsic::decode(&mut &transaction[..]).expect("transaction decodes").function
		})
		.collect::<Vec<_>>()
}

/// Empties the transaction pool, so a later assertion only sees fresh submissions.
pub fn clear_pool() {
	TRANSACTION_POOL.with_borrow_mut(|pool| pool.write().transactions.clear());
}

/// Applies `call` as an authorized transaction, the way a block does.
///
/// A test that calls the dispatchable directly hands it the `Authorized` origin and skips
/// `authorize`. This runs the whole pipeline instead, so the sweep is only dispatched if
/// `authorize` admits it, which is what gates the sweeps on chain.
pub fn apply_authorized(
	call: RuntimeCall,
) -> Result<
	Result<PostDispatchInfo, DispatchErrorWithPostInfo<PostDispatchInfo>>,
	TransactionValidityError,
> {
	let xt = Extrinsic::new_transaction(call, (AuthorizeCall::new(),));
	let info = xt.get_dispatch_info();
	let len = xt.encoded_size();
	let checked = Checkable::check(xt, &frame_system::ChainContext::<Test>::default())?;

	checked.apply::<Test>(&info, len)
}
