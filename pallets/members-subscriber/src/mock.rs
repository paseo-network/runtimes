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

//! Mock runtime for testing the members-subscriber pallet.

extern crate alloc;

use alloc::vec::Vec;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use core::{cell::RefCell, ops::Range, time::Duration};
use frame_support::{derive_impl, parameter_types, traits::OffchainWorker};
use frame_system::{
	offchain::{CreateAuthorizedTransaction, CreateTransaction, CreateTransactionBase},
	AuthorizeCall,
};
use scale_info::TypeInfo;
use sp_core::{ConstU32, ConstU64};
use sp_runtime::{
	offchain::{
		testing::{PoolState, TestOffchainExt, TestTransactionPoolExt},
		OffchainDbExt, OffchainWorkerExt, TransactionPoolExt,
	},
	BuildStorage,
};
use std::sync::Arc;
use verifiable::{
	Alias, AliasVec, BatchProofItem, Entropy, Error as VerifiableError, GenerateVerifiable,
};
use xcm::v5::{Assets, Location, SendError, SendResult, SendXcm, Xcm, XcmHash};

use crate::types::NotifierEndpoint;

pub type Header = sp_runtime::generic::Header<u64, sp_runtime::traits::BlakeTwo256>;
pub type TxExtension = (AuthorizeCall<Test>,);

pub type Extrinsic = sp_runtime::generic::UncheckedExtrinsic<
	u64,
	RuntimeCall,
	sp_runtime::testing::UintAuthorityId,
	TxExtension,
>;
pub type Block = sp_runtime::generic::Block<Header, Extrinsic>;

frame_support::construct_runtime!(
	pub enum Test
	{
		System: frame_system,
		MembersSubscriber: crate,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Block = Block;
}

parameter_types! {
	pub const RingRootsNotifier: NotifierEndpoint = NotifierEndpoint {
		location: Location::parent(),
		pallet_index: 50,
	};
	pub const SelfParaId: u32 = 1000;
	pub const MaxMissingRootsPerCollection: u32 = 255;
	pub const MaxDeletedRingsPerCollection: u32 = 100;
	pub const MaxRingRootsPerCollection: u32 = 100;
	pub const MaxCollections: u32 = 10;
	pub const ReplayCooldownSeconds: u64 = 60;
	pub const MaxUpdatesPerBatch: u32 = 10;
	pub const ReplayWarningThreshold: u32 = 5;
	pub const ReplayAbandonThreshold: u32 = 10;
	pub const MaxRecentRootsPerRing: u32 = 2;
	pub const OffchainWorkerInterval: u64 = 1;
}

// ========== XCM Tracking ==========

thread_local! {
	/// Tracks XCM messages sent during tests for verification.
	pub static SENT_XCMS: RefCell<Vec<(Location, Vec<u8>)>> = const { RefCell::new(Vec::new()) };
	/// Controllable time source for OCW cooldown testing.
	pub static TIME: RefCell<Duration> = const { RefCell::new(Duration::from_secs(1_700_000_000)) };
	/// Transaction pool state for OCW testing.
	pub static TRANSACTION_POOL: RefCell<Arc<parking_lot::RwLock<PoolState>>> =
		RefCell::new(Arc::new(parking_lot::RwLock::new(PoolState {
			transactions: Vec::new(),
		})));
}

pub fn clear_sent_xcms() {
	SENT_XCMS.with(|x| x.borrow_mut().clear());
}

pub fn get_sent_xcms() -> Vec<(Location, Vec<u8>)> {
	SENT_XCMS.with(|x| x.borrow().clone())
}

/// Mock XCM sender for testing.
pub struct MockXcmSender;

impl SendXcm for MockXcmSender {
	type Ticket = (Location, Vec<u8>);

	fn validate(
		destination: &mut Option<Location>,
		message: &mut Option<Xcm<()>>,
	) -> SendResult<Self::Ticket> {
		let dest = destination.take().unwrap_or(Location::here());
		let msg = message.take().map(|m| m.encode()).unwrap_or_default();
		Ok(((dest, msg), Assets::new()))
	}

	fn deliver(ticket: Self::Ticket) -> Result<XcmHash, SendError> {
		SENT_XCMS.with(|x| x.borrow_mut().push(ticket));
		Ok([0u8; 32])
	}
}

pub struct MockEnsureNotifierOrigin;

impl frame_support::traits::EnsureOrigin<RuntimeOrigin> for MockEnsureNotifierOrigin {
	type Success = ();

	fn try_origin(o: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
		match o.clone().into() {
			Ok(frame_system::RawOrigin::Root) => Ok(()),
			_ => Err(o),
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
		Ok(RuntimeOrigin::root())
	}
}

/// Mock Unix time source for testing. Reads from the `TIME` thread-local.
pub struct MockUnixTime;

impl frame_support::traits::UnixTime for MockUnixTime {
	fn now() -> Duration {
		TIME.with(|t| *t.borrow())
	}
}

pub fn set_time_secs(secs: u64) {
	TIME.with(|t| *t.borrow_mut() = Duration::from_secs(secs));
}

// ========== CreateAuthorizedTransaction ==========

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

impl crate::Config for Test {
	type WeightInfo = ();
	type Crypto = TestVerifiable;
	type XcmSender = MockXcmSender;
	type RingRootsNotifier = RingRootsNotifier;
	type SelfParaId = SelfParaId;
	type MaxMissingRootsPerCollection = MaxMissingRootsPerCollection;
	type MaxDeletedRingsPerCollection = MaxDeletedRingsPerCollection;
	type MaxRingRootsPerCollection = MaxRingRootsPerCollection;
	type MaxUpdatesPerBatch = MaxUpdatesPerBatch;
	type EnsureNotifierOrigin = MockEnsureNotifierOrigin;
	type EnsureTerminationOrigin = frame_system::EnsureRoot<u64>;
	type MaxCollections = MaxCollections;
	type UnixTime = MockUnixTime;
	type ReplayCooldownSeconds = ReplayCooldownSeconds;
	type ReplayWarningThreshold = ReplayWarningThreshold;
	type ReplayAbandonThreshold = ReplayAbandonThreshold;
	type MaxRecentRootsPerRing = MaxRecentRootsPerRing;
	type OffchainWorkerInterval = ConstU64<1>;
}

// ========== Mock Verifiable Implementation ==========

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
		let mut r = [0u8; 32];
		r[0..self.context.len().min(32)].copy_from_slice(&self.context);
		r[0] ^= self.member.0 as u8;
		r
	}
}

#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, TypeInfo, DecodeWithMemTracking)]
pub struct TestSignature {
	pub member: TestMemberKey,
	pub message: Vec<u8>,
}

pub type TestMembers = verifiable::mock::MockMembers<u64, ConstU32<16>>;

pub struct TestVerifiable;

impl GenerateVerifiable for TestVerifiable {
	type Members = TestMembers;
	type Intermediate = TestMembers;
	type Member = TestMemberKey;
	type Secret = TestMemberKey;
	type Commitment = (Self::Member, Vec<u64>);
	type Proof = TestProof;
	type Signature = TestSignature;
	type StaticChunk = ();
	type Config = ();

	fn start_members(_config: Self::Config) -> Self::Intermediate {
		TestMembers::default()
	}

	fn push_members(
		inter: &mut Self::Intermediate,
		members: impl Iterator<Item = Self::Member>,
		_lookup: impl Fn(Range<usize>) -> Result<Vec<Self::StaticChunk>, ()>,
	) -> Result<(), VerifiableError> {
		for member in members {
			inter.try_push(member.0).map_err(|_| VerifiableError::SetFull)?
		}
		Ok(())
	}

	fn finish_members(inter: Self::Intermediate) -> Self::Members {
		inter
	}

	fn new_secret(entropy: Entropy) -> Self::Secret {
		TestMemberKey(entropy[0].into())
	}

	fn member_from_secret(secret: &Self::Secret) -> Self::Member {
		secret.clone()
	}

	fn open(
		_config: Self::Config,
		member: &Self::Member,
		members: impl Iterator<Item = Self::Member>,
	) -> Result<Self::Commitment, VerifiableError> {
		let set = members.map(|x| x.0).collect::<Vec<_>>();
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
		_config: Self::Config,
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
		proofs: &[BatchProofItem<Self::Proof>],
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

	fn is_member_valid(_member: &Self::Member) -> bool {
		true
	}
}

// ========== Test Helpers ==========

#[allow(dead_code)]
pub fn new_test_ext() -> sp_io::TestExternalities {
	let storage = frame_system::GenesisConfig::<Test>::default().build_storage().unwrap();
	let mut ext: sp_io::TestExternalities = storage.into();
	let (offchain, _state) = TestOffchainExt::new();
	let (pool, state) = TestTransactionPoolExt::new();
	TRANSACTION_POOL.set(state);
	ext.register_extension(OffchainDbExt::new(offchain.clone()));
	ext.register_extension(OffchainWorkerExt::new(offchain));
	ext.register_extension(TransactionPoolExt::new(pool));
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

/// Drains transactions submitted by the offchain worker and executes them.
pub fn drain_ocw_transactions() {
	let transactions =
		{ TRANSACTION_POOL.with_borrow_mut(|pool| std::mem::take(&mut pool.write().transactions)) };
	for tx in transactions {
		let tx = Decode::decode(&mut &tx[..]).unwrap();
		Executive::apply_extrinsic(tx).expect("tx valid").expect("tx succeeds");
	}
}

/// Returns the number of pending transactions in the OCW pool.
pub fn pending_ocw_tx_count() -> usize {
	TRANSACTION_POOL.with_borrow(|pool| pool.read().transactions.len())
}

/// Advances the chain by one block, triggering the offchain worker.
#[allow(dead_code)]
pub fn advance_block() {
	let current = frame_system::Pallet::<Test>::block_number();
	AllPalletsWithSystem::offchain_worker(current);
	let next = current.saturating_add(1u64);
	frame_system::Pallet::<Test>::initialize(&next, &Default::default(), &Default::default());
}

// ========== Benchmark Helper ==========

#[cfg(feature = "runtime-benchmarks")]
impl crate::benchmarking::BenchmarkHelper<Test> for Test {
	fn mock_ring_root(seed: u32) -> crate::types::MembersOf<Test> {
		let mut members = TestMembers::default();
		members.try_push(seed as u64).ok();
		members
	}
}
