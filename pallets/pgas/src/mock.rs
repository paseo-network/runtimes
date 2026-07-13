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

#![allow(unused)]

extern crate alloc;

use crate as pallet_pgas;
use alloc::{collections::BTreeMap, vec::Vec};
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use core::{cell::RefCell, ops::Range, time::Duration};
use frame_support::{
	derive_impl, parameter_types,
	traits::{AsEnsureOriginWithArg, UnixTime},
};
use frame_system::{
	offchain::{CreateAuthorizedTransaction, CreateBare, CreateTransaction, CreateTransactionBase},
	AuthorizeCall,
};
use indiv_support::traits::{
	Alias, BatchProofItem, Context, ContextualAlias, Identifier, MembershipProver,
	RevisedContextualAlias, RevisionIndex, RingIndex,
};
use scale_info::TypeInfo;
use sp_core::ConstU32;
use sp_runtime::{
	offchain::{
		testing::{PoolState, TestOffchainExt, TestTransactionPoolExt},
		OffchainDbExt, OffchainWorkerExt, TransactionPoolExt,
	},
	traits::{IdentifyAccount, Verify},
	AccountId32, BoundedVec, BuildStorage, DispatchError,
};
use std::sync::Arc;
use verifiable::{AliasVec, Entropy, Error as VerifiableError, GenerateVerifiable};

pub type Header = sp_runtime::generic::Header<u64, sp_runtime::traits::BlakeTwo256>;
pub type Block = sp_runtime::generic::Block<Header, Extrinsic>;
pub type TxExtension = (pallet_pgas::AsPgas<Test>, AuthorizeCall<Test>);
pub type Extrinsic = sp_runtime::generic::UncheckedExtrinsic<
	AccountId32,
	RuntimeCall,
	AccountAuthority,
	TxExtension,
>;

// ---- Signature stub (required by UncheckedExtrinsic) ---------------------------------------

#[derive(
	Clone, Eq, PartialEq, Debug, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
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

// ---- Runtime -------------------------------------------------------------------------------

frame_support::construct_runtime!(
	pub enum Test {
		System: frame_system,
		Balances: pallet_balances,
		Assets: pallet_assets,
		Pgas: pallet_pgas,
	}
);

impl<LocalCall> CreateBare<LocalCall> for Test
where
	RuntimeCall: From<LocalCall>,
{
	fn create_bare(call: RuntimeCall) -> Extrinsic {
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
	type Extension = TxExtension;
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
		(pallet_pgas::AsPgas::new(None), AuthorizeCall::new())
	}
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Block = Block;
	type AccountId = AccountId32;
	type Lookup = sp_runtime::traits::IdentityLookup<AccountId32>;
	type AccountData = pallet_balances::AccountData<u64>;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
	type AccountStore = System;
}

#[derive_impl(pallet_assets::config_preludes::TestDefaultConfig)]
impl pallet_assets::Config for Test {
	type Currency = Balances;
	type CreateOrigin = AsEnsureOriginWithArg<frame_system::EnsureSigned<AccountId32>>;
	type ForceOrigin = frame_system::EnsureRoot<AccountId32>;
	type Holder = ();
}

// ---- Clock ---------------------------------------------------------------------------------

parameter_types! {
	pub static MockUnixTime: Duration = Duration::ZERO;
	pub static TransactionPool: Arc<parking_lot::RwLock<PoolState>> =
		Arc::new(parking_lot::RwLock::new(PoolState { transactions: Vec::new() }));
}

pub struct TestClock;
impl UnixTime for TestClock {
	fn now() -> Duration {
		MockUnixTime::get()
	}
}

pub fn set_time_sec(secs: u64) {
	MockUnixTime::set(Duration::from_secs(secs));
}

// ---- Mock crypto ---------------------------------------------------------------------------
//
// Minimal `GenerateVerifiable` implementation: proofs carry their own context/member/members/
// message; `validate` checks they match. Alias is derived deterministically from context +
// member so tests can assert on it.

#[derive(
	Clone, PartialEq, Eq, Debug, Encode, Decode, TypeInfo, MaxEncodedLen, DecodeWithMemTracking,
)]
pub struct TestMemberKey(pub u64);

#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, TypeInfo, DecodeWithMemTracking)]
pub struct TestProof {
	pub context: Vec<u8>,
	pub member: TestMemberKey,
	pub members: Vec<u64>,
	pub message: Vec<u8>,
}

impl TestProof {
	pub fn alias(&self) -> Alias {
		// Seed alias with the context so different contexts yield different aliases;
		// XOR the member id into the first byte so different members get distinct aliases.
		let mut r = [0u8; 32];
		let ctx_len = self.context.len().min(32);
		r[..ctx_len].copy_from_slice(&self.context[..ctx_len]);
		r[0] ^= self.member.0 as u8;
		r
	}
}

#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, TypeInfo, DecodeWithMemTracking)]
pub struct TestSignature {
	pub member: TestMemberKey,
	pub message: Vec<u8>,
}

pub struct TestVerifiable;

impl GenerateVerifiable for TestVerifiable {
	type Members = BoundedVec<u64, ConstU32<256>>;
	type Intermediate = BoundedVec<u64, ConstU32<256>>;
	type Member = TestMemberKey;
	type Secret = TestMemberKey;
	type Commitment = (Self::Member, Vec<u64>);
	type Proof = TestProof;
	type Signature = TestSignature;
	type StaticChunk = ();
	type Config = ();

	fn start_members(_: Self::Config) -> Self::Intermediate {
		BoundedVec::new()
	}

	fn push_members(
		inter: &mut Self::Intermediate,
		members: impl Iterator<Item = Self::Member>,
		_lookup: impl Fn(Range<usize>) -> Result<Vec<Self::StaticChunk>, ()>,
	) -> Result<(), VerifiableError> {
		for m in members {
			inter.try_push(m.0).map_err(|_| VerifiableError::SetFull)?;
		}
		Ok(())
	}

	fn finish_members(inter: Self::Intermediate) -> Self::Members {
		inter
	}

	fn new_secret(entropy: Entropy) -> Self::Secret {
		// Use first 8 bytes as a little-endian u64 so callers can pick a friendly id.
		let mut bytes = [0u8; 8];
		bytes.copy_from_slice(&entropy[..8]);
		TestMemberKey(u64::from_le_bytes(bytes))
	}

	fn member_from_secret(secret: &Self::Secret) -> Self::Member {
		secret.clone()
	}

	fn open(
		_: Self::Config,
		member: &Self::Member,
		members: impl Iterator<Item = Self::Member>,
	) -> Result<Self::Commitment, VerifiableError> {
		let set = members.map(|m| m.0).collect::<Vec<_>>();
		if !set.contains(&member.0) {
			return Err(VerifiableError::NotInRing);
		}
		Ok((member.clone(), set))
	}

	fn create_multi_context(
		(member, members): Self::Commitment,
		secret: &Self::Secret,
		contexts: &[&[u8]],
		message: &[u8],
	) -> Result<(Self::Proof, AliasVec), VerifiableError> {
		if contexts.len() != 1 || &member != secret {
			return Err(VerifiableError::NotInRing);
		}
		let proof =
			TestProof { context: contexts[0].to_vec(), member, members, message: message.to_vec() };
		let alias = proof.alias();
		Ok((proof, core::iter::once(alias).collect()))
	}

	fn validate_multi_context(
		_: Self::Config,
		proof: &Self::Proof,
		members: &Self::Members,
		contexts: &[&[u8]],
		message: &[u8],
	) -> Result<AliasVec, VerifiableError> {
		if contexts.len() == 1 &&
			proof.context == contexts[0] &&
			proof.members[..] == members[..] &&
			proof.message == message
		{
			Ok(core::iter::once(proof.alias()).collect())
		} else {
			Err(VerifiableError::VerificationFailed)
		}
	}

	fn batch_validate(
		capacity: Self::Config,
		members: &Self::Members,
		proofs: &[verifiable::BatchProofItem<Self::Proof>],
	) -> Result<Vec<Alias>, VerifiableError> {
		proofs
			.iter()
			.map(|item| {
				Self::validate(capacity, &item.proof, members, &item.context, &item.message)
			})
			.collect()
	}

	fn sign(secret: &Self::Secret, message: &[u8]) -> Result<Self::Signature, VerifiableError> {
		Ok(TestSignature { member: Self::member_from_secret(secret), message: message.to_vec() })
	}

	fn verify_signature(
		signature: &Self::Signature,
		message: &[u8],
		member: &Self::Member,
	) -> bool {
		&signature.member == member && signature.message == message
	}

	fn alias_in_context(_secret: &Self::Secret, _context: &[u8]) -> Result<Alias, VerifiableError> {
		unimplemented!()
	}

	fn is_member_valid(_: &Self::Member) -> bool {
		true
	}

	fn is_valid(
		capacity: Self::Config,
		proof: &Self::Proof,
		members: &Self::Members,
		context: &[u8],
		alias: &Alias,
		message: &[u8],
	) -> bool {
		Self::validate(capacity, proof, members, context, message)
			.map(|a| &a == alias)
			.unwrap_or(false)
	}
}

// ---- MockProver: MembershipProver<Crypto = TestVerifiable> ---------------------------------

thread_local! {
	/// Registered rings keyed by (identifier, ring_index) -> list of (revision, members).
	/// The newest revision is the last element.
	static RING_REGISTRY: RefCell<BTreeMap<(Identifier, RingIndex), Vec<(RevisionIndex, Vec<u64>)>>> =
		const { RefCell::new(BTreeMap::new()) };
}

pub struct MockProver;

impl MockProver {
	/// Register (or overwrite) the members of a ring. Resets revisions to `[0]`.
	pub fn set_ring_members(identifier: &Identifier, ring_index: RingIndex, members: Vec<u64>) {
		RING_REGISTRY.with(|r| {
			r.borrow_mut().insert((*identifier, ring_index), alloc::vec![(0u32, members)]);
		});
	}

	/// Return the stored members for the newest revision of a ring.
	pub fn ring_members(identifier: &Identifier, ring_index: RingIndex) -> Vec<u64> {
		RING_REGISTRY.with(|r| {
			r.borrow()
				.get(&(*identifier, ring_index))
				.and_then(|rs| rs.last().cloned())
				.map(|(_, m)| m)
				.unwrap_or_default()
		})
	}

	fn members_for(
		identifier: &Identifier,
		ring_index: RingIndex,
	) -> Option<<TestVerifiable as GenerateVerifiable>::Members> {
		let raw = Self::ring_members(identifier, ring_index);
		if raw.is_empty() {
			return None;
		}
		let mut bv = BoundedVec::new();
		for v in raw {
			bv.try_push(v).ok()?;
		}
		Some(bv)
	}
}

impl MembershipProver for MockProver {
	type Crypto = TestVerifiable;

	fn verify_membership(
		identifier: &Identifier,
		proof: &<Self::Crypto as GenerateVerifiable>::Proof,
		ring_index: RingIndex,
		context: Context,
		msg: &[u8],
	) -> Result<RevisedContextualAlias, DispatchError> {
		let members = Self::members_for(identifier, ring_index)
			.ok_or(DispatchError::Other("ring not registered"))?;
		let alias = TestVerifiable::validate((), proof, &members, &context[..], msg)
			.map_err(|_| DispatchError::Other("invalid proof"))?;
		Ok(RevisedContextualAlias {
			revision: 0,
			ring: ring_index,
			ca: ContextualAlias { alias, context },
		})
	}

	fn verify_membership_at_rev(
		identifier: &Identifier,
		proof: &<Self::Crypto as GenerateVerifiable>::Proof,
		ring_index: RingIndex,
		_revision: RevisionIndex,
		context: Context,
		msg: &[u8],
	) -> Result<ContextualAlias, DispatchError> {
		Self::verify_membership(identifier, proof, ring_index, context, msg).map(|r| r.ca)
	}

	fn verify_memberships_in_ring(
		_identifier: &Identifier,
		_ring_index: RingIndex,
		_items: &[BatchProofItem<<Self::Crypto as GenerateVerifiable>::Proof>],
	) -> Result<Vec<RevisedContextualAlias>, DispatchError> {
		unimplemented!("pgas mock does not use batch verification")
	}

	fn verify_memberships_in_ring_at_rev(
		_identifier: &Identifier,
		_ring_index: RingIndex,
		_revision: RevisionIndex,
		_items: &[BatchProofItem<<Self::Crypto as GenerateVerifiable>::Proof>],
	) -> Result<Vec<ContextualAlias>, DispatchError> {
		unimplemented!("pgas mock does not use batch verification")
	}

	fn ring_revision(identifier: &Identifier, ring_index: RingIndex) -> Option<RevisionIndex> {
		RING_REGISTRY.with(|r| {
			r.borrow()
				.get(&(*identifier, ring_index))
				.and_then(|rs| rs.last().map(|(rev, _)| *rev))
		})
	}

	fn is_revision_valid(
		identifier: &Identifier,
		ring_index: RingIndex,
		revision: RevisionIndex,
	) -> bool {
		RING_REGISTRY.with(|r| {
			r.borrow()
				.get(&(*identifier, ring_index))
				.map(|rs| rs.iter().any(|(rev, _)| *rev == revision))
				.unwrap_or(false)
		})
	}

	fn revision_source_time(
		_identifier: &Identifier,
		_ring_index: RingIndex,
		_revision: RevisionIndex,
	) -> Option<u64> {
		None
	}
}

// ---- Config + helpers ----------------------------------------------------------------------

parameter_types! {
	pub PgasAssetId: u32 = 1;
	pub PgasClaimAmount: u64 = 1000;
	pub const MaxClaimsPerPeriodPerPerson: u32 = 4;
	pub const MaxClaimsPerPeriodPerLitePerson: u32 = 2;
	pub const MaxPgasClaimRecordCleanupPerCall: u32 = 3;
	pub PgasAdmin: AccountId32 = AccountId32::new([0xaa; 32]);
	pub PgasMinBalance: u64 = 1;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct BenchmarkHelper;
#[cfg(feature = "runtime-benchmarks")]
impl pallet_pgas::benchmarking::BenchmarkHelper<Test> for BenchmarkHelper {
	fn set_time(duration: Duration) {
		MockUnixTime::set(duration);
	}

	fn seed_and_create_proof(
		identifier: &Identifier,
		ring_index: RingIndex,
		context: &Context,
		message: &[u8],
	) -> <TestVerifiable as GenerateVerifiable>::Proof {
		// Deterministic test member — benchmarks want a fresh, reproducible setup.
		let secret = TestVerifiable::new_secret([0u8; 32]);
		let member = TestVerifiable::member_from_secret(&secret);
		register_member(identifier, ring_index, member.0);
		let members = MockProver::ring_members(identifier, ring_index);
		let commitment = TestVerifiable::open((), &member, members.into_iter().map(TestMemberKey))
			.expect("commitment opens on test crypto");
		let (proof, _) = TestVerifiable::create(commitment, &secret, context, message)
			.expect("proof creation on test crypto is infallible");
		proof
	}
}

impl pallet_pgas::Config for Test {
	type WeightInfo = ();
	type MembershipProver = MockProver;
	type Clock = TestClock;
	type Fungibles = Assets;
	type PgasAssetId = PgasAssetId;
	type PgasClaimAmount = PgasClaimAmount;
	type MaxClaimsPerPeriodPerPerson = MaxClaimsPerPeriodPerPerson;
	type MaxClaimsPerPeriodPerLitePerson = MaxClaimsPerPeriodPerLitePerson;
	type MaxPgasClaimRecordCleanupPerCall = MaxPgasClaimRecordCleanupPerCall;
	type PgasAdmin = PgasAdmin;
	type PgasMinBalance = PgasMinBalance;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = BenchmarkHelper;
}

// ---- Test helpers --------------------------------------------------------------------------

pub fn id_to_account(id: u64) -> AccountId32 {
	let mut bytes = [0; 32];
	bytes[..8].copy_from_slice(&id.to_le_bytes());
	AccountId32::new(bytes)
}

/// Register `member_id` as a member of `(identifier, ring_index)`.
pub fn register_member(identifier: &Identifier, ring_index: RingIndex, member_id: u64) {
	RING_REGISTRY.with(|r| {
		let mut reg = r.borrow_mut();
		let entry = reg
			.entry((*identifier, ring_index))
			.or_insert_with(|| alloc::vec![(0u32, Vec::new())]);
		let (_, members) = entry.last_mut().expect("at least one revision");
		if !members.contains(&member_id) {
			members.push(member_id);
		}
	});
}

/// Compute the inherited-implication message that `AsPgas` validates the proof against.
pub fn proof_message_for(call: &RuntimeCall, extension_version: u8) -> [u8; 32] {
	sp_runtime::traits::TxBaseImplication((extension_version, call))
		.using_encoded(sp_io::hashing::blake2_256)
}

/// Build a proof and `AsPgas` extension for a claim.
pub fn build_claim_tx(
	member_id: u64,
	ring_index: RingIndex,
	collection: pallet_pgas::PgasCollection,
	slot_index: u32,
	target: AccountId32,
	day: u32,
) -> (RuntimeCall, pallet_pgas::AsPgas<Test>) {
	register_member(&collection.identifier(), ring_index, member_id);
	let secret = TestVerifiable::new_secret({
		let mut e = [0u8; 32];
		e[..8].copy_from_slice(&member_id.to_le_bytes());
		e
	});
	let member = TestVerifiable::member_from_secret(&secret);
	let members = MockProver::ring_members(&collection.identifier(), ring_index);
	let commitment = TestVerifiable::open((), &member, members.into_iter().map(TestMemberKey))
		.expect("commitment should open");

	let call = RuntimeCall::Pgas(pallet_pgas::Call::claim_pgas { slot_index, target });
	let msg = proof_message_for(&call, 0);
	let context = pallet_pgas::Pallet::<Test>::build_gas_context(day, slot_index);
	let (proof, _) =
		TestVerifiable::create(commitment, &secret, &context, &msg).expect("proof should build");

	let revision = MockProver::ring_revision(&collection.identifier(), ring_index).unwrap_or(0);

	let tx_ext = pallet_pgas::AsPgas::<Test>::new(Some(pallet_pgas::AsPgasInfo::Claim {
		proof,
		ring_index,
		revision,
		collection,
		day,
	}));
	(call, tx_ext)
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	let t = frame_system::GenesisConfig::<Test>::default().build_storage().unwrap();
	let mut ext = sp_io::TestExternalities::new(t);
	let (offchain, _) = TestOffchainExt::new();
	let (pool, state) = TestTransactionPoolExt::new();
	TransactionPool::set(state);
	ext.register_extension(OffchainDbExt::new(offchain.clone()));
	ext.register_extension(OffchainWorkerExt::new(offchain));
	ext.register_extension(TransactionPoolExt::new(pool));
	ext.execute_with(|| {
		System::set_block_number(1);
		MockUnixTime::set(Duration::ZERO);
		RING_REGISTRY.with(|r| r.borrow_mut().clear());
	});
	ext
}

pub type Executive = frame_executive::Executive<
	Test,
	Block,
	frame_system::ChainContext<Test>,
	Test,
	AllPalletsWithSystem,
	(),
>;

/// Drain transactions that OCW submitted into the pool and apply them.
pub fn drain_ocw_transactions() {
	let transactions = std::mem::take(&mut TransactionPool::get().write().transactions);
	for tx in transactions {
		let tx = Decode::decode(&mut &tx[..]).unwrap();
		Executive::apply_extrinsic(tx)
			.expect("tx should be valid")
			.expect("tx should dispatch successfully");
	}
}

pub fn pending_ocw_tx_count() -> usize {
	TransactionPool::get().read().transactions.len()
}
