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

//! Mock runtime for testing the dotNS gateway pallet.

extern crate alloc;

use crate::{
	types::{BaseLabel, Link, ProofOf},
	AsDotnsGateway, AsDotnsGatewayInfo,
};

use alloc::vec::Vec;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use core::{cell::RefCell, ops::Range};
use frame_support::{derive_impl, dispatch::GetDispatchInfo, parameter_types};
use frame_system::{
	offchain::{CreateAuthorizedTransaction, CreateTransaction, CreateTransactionBase},
	AuthorizeCall, EnsureRoot,
};
use indiv_pallet_members_subscriber::types::NotifierEndpoint;
use indiv_support::traits::{
	Alias, Identifier, RingExponent, RingIndex, PEOPLE_IDENTIFIER, PEOPLE_LITE_IDENTIFIER,
};
use scale_info::TypeInfo;
use sp_core::{ConstU32, ConstU64, H160};
use sp_runtime::{
	testing::UintAuthorityId,
	traits::DispatchTransaction,
	transaction_validity::{TransactionValidityError, ValidTransaction},
	BoundedVec, BuildStorage, DispatchError,
};
use verifiable::{AliasVec, Entropy, Error as VerifiableError, GenerateVerifiable};
use xcm::v5::{Assets, Location, SendError, SendResult, SendXcm, Xcm, XcmHash};

frame_support::construct_runtime!(
	pub enum Test
	{
		System: frame_system,
		MembersSubscriber: indiv_pallet_members_subscriber,
		DotnsGateway: crate,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Block = frame_system::mocking::MockBlock<Test>;
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
	pub const DispatcherAddr: H160 = H160([0xd0; 20]);
}

// ========== Mock XCM Sender ==========

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

	fn deliver(_ticket: Self::Ticket) -> Result<XcmHash, SendError> {
		Ok([0u8; 32])
	}
}

// ========== Mock EnsureNotifierOrigin ==========

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

// ========== Mock Time ==========

thread_local! {
	pub static MOCK_NOW: RefCell<u64> = const { RefCell::new(1_700_000_000) };
}

pub struct MockUnixTime;

impl frame_support::traits::UnixTime for MockUnixTime {
	fn now() -> core::time::Duration {
		core::time::Duration::from_secs(MOCK_NOW.with(|v| *v.borrow()))
	}
}

// ========== Mock Contract Caller ==========

thread_local! {
	pub static CONTRACT_CALLS: RefCell<Vec<(H160, Vec<u8>, u128)>> = const { RefCell::new(Vec::new()) };
	pub static CONTRACT_CALL_RESULT: RefCell<Result<Vec<u8>, crate::ContractCallError>> = const { RefCell::new(Ok(Vec::new())) };
	pub static CONTRACT_CALL_WEIGHT: RefCell<frame_support::weights::Weight> = const { RefCell::new(frame_support::weights::Weight::zero()) };
}

pub fn set_contract_call_result(result: Result<Vec<u8>, crate::ContractCallError>) {
	CONTRACT_CALL_RESULT.with(|r| *r.borrow_mut() = result);
}

/// Configures the caller to fail with a `DispatchError` that carries no revert data
/// (simulates a low-level failure: out-of-gas, caller error, etc.).
pub fn set_contract_call_dispatch_error(err: DispatchError) {
	set_contract_call_result(Err(crate::ContractCallError::from(err)));
}

/// Configures the caller to fail with a contract revert carrying the given bytes.
pub fn set_contract_call_revert(revert_data: Vec<u8>) {
	set_contract_call_result(Err(crate::ContractCallError {
		dispatch: DispatchError::Other("revert"),
		revert_data: Some(revert_data),
	}));
}

pub fn set_contract_call_weight(weight: frame_support::weights::Weight) {
	CONTRACT_CALL_WEIGHT.with(|w| *w.borrow_mut() = weight);
}

pub fn get_contract_calls() -> Vec<(H160, Vec<u8>, u128)> {
	CONTRACT_CALLS.with(|c| c.borrow().clone())
}

pub fn clear_contract_calls() {
	CONTRACT_CALLS.with(|c| c.borrow_mut().clear());
}

pub struct MockContractCaller;

impl crate::ContractCaller for MockContractCaller {
	fn call(
		dest: H160,
		data: Vec<u8>,
		value: u128,
	) -> Result<(Vec<u8>, frame_support::weights::Weight), crate::ContractCallError> {
		CONTRACT_CALLS.with(|c| c.borrow_mut().push((dest, data, value)));
		let weight = CONTRACT_CALL_WEIGHT.with(|w| *w.borrow());
		CONTRACT_CALL_RESULT.with(|r| r.borrow().clone()).map(|data| (data, weight))
	}
}

// ========== Mock Proof ==========

#[derive(
	Clone, PartialEq, Eq, Debug, Encode, Decode, TypeInfo, MaxEncodedLen, DecodeWithMemTracking,
)]
pub struct MockProof {
	pub alias: Alias,
	pub valid: bool,
	pub message: BoundedVec<u8, ConstU32<256>>,
}

// ========== Mock Crypto (TestVerifiable) ==========

pub type TestMembers = verifiable::mock::MockMembers<u64, ConstU32<16>>;

pub struct TestVerifiable;

impl GenerateVerifiable for TestVerifiable {
	type Members = TestMembers;
	type Intermediate = TestMembers;
	type Member = u64;
	type Secret = u64;
	type Commitment = (u64, Vec<u64>);
	type Proof = MockProof;
	type Signature = ();
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
		_secret: &Self::Secret,
		_contexts: &[&[u8]],
		_message: &[u8],
	) -> Result<(Self::Proof, AliasVec), VerifiableError> {
		unimplemented!()
	}

	fn validate_multi_context(
		_config: Self::Config,
		proof: &Self::Proof,
		_members: &Self::Members,
		contexts: &[&[u8]],
		message: &[u8],
	) -> Result<AliasVec, VerifiableError> {
		if proof.valid && proof.message.as_slice() == message {
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
		_secret: &Self::Secret,
		_context: &[u8],
	) -> Result<verifiable::Alias, VerifiableError> {
		unimplemented!()
	}

	fn is_member_valid(_member: &Self::Member) -> bool {
		true
	}
}

// ========== Ring Root Helpers ==========

pub fn set_mock_ring_root(identifier: Identifier, ring_index: RingIndex) {
	let now = MOCK_NOW.with(|v| *v.borrow());
	let record = indiv_pallet_members_subscriber::types::RingCommitmentRecord::<Test> {
		root: TestMembers::default(),
		revision: 1,
		source_time: now,
		source_sequence: 0,
	};
	let mut roots = indiv_pallet_members_subscriber::RingRoots::<Test>::get(identifier, ring_index)
		.unwrap_or_default();
	roots.clear();
	roots.try_push(record).expect("MaxRecentRootsPerRing > 0");
	indiv_pallet_members_subscriber::RingRoots::<Test>::insert(identifier, ring_index, roots);
	indiv_pallet_members_subscriber::RingCollectionExponents::<Test>::insert(
		identifier,
		RingExponent::R2e9,
	);
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

// ========== Members Subscriber Config ==========

impl indiv_pallet_members_subscriber::Config for Test {
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
	type EnsureTerminationOrigin = EnsureRoot<u64>;
	type MaxCollections = MaxCollections;
	type UnixTime = MockUnixTime;
	type ReplayCooldownSeconds = ReplayCooldownSeconds;
	type ReplayWarningThreshold = ReplayWarningThreshold;
	type ReplayAbandonThreshold = ReplayAbandonThreshold;
	type MaxRecentRootsPerRing = ConstU32<2>;
	type OffchainWorkerInterval = ConstU64<1>;
}

// ========== Test Address Mapper ==========

/// Maps u64 AccountId to H160 by placing the 8-byte big-endian representation
/// in the last 8 bytes. Matches pallet_revive's `TestAccountMapper` for u64.
pub struct TestAddressMapper;

impl crate::AddressMapper<u64> for TestAddressMapper {
	fn to_address(account_id: &u64) -> H160 {
		let mut bytes = [0u8; 20];
		bytes[12..].copy_from_slice(&account_id.to_be_bytes());
		H160::from(bytes)
	}
}

// ========== Benchmark Helper ==========

#[cfg(feature = "runtime-benchmarks")]
impl crate::benchmarking::BenchmarkHelper<Test> for () {
	fn setup_ring_root(identifier: &Identifier, ring_index: RingIndex) {
		// Filling to MaxRecentRootsPerRing capacity so verify_proof iterates
		// all roots (worst-case for find_map over the sliding window).
		let now = MOCK_NOW.with(|v| *v.borrow());
		let max_recent =
			<<Test as indiv_pallet_members_subscriber::Config>::MaxRecentRootsPerRing as frame_support::traits::Get<u32>>::get();
		let mut roots = frame_support::BoundedVec::new();
		for i in 0..max_recent {
			roots
				.try_push(indiv_pallet_members_subscriber::types::RingCommitmentRecord::<Test> {
					root: TestMembers::default(),
					revision: i + 1,
					source_time: now,
					source_sequence: 0,
				})
				.expect("within MaxRecentRootsPerRing bound");
		}
		indiv_pallet_members_subscriber::RingRoots::<Test>::insert(*identifier, ring_index, roots);
		indiv_pallet_members_subscriber::RingCollectionExponents::<Test>::insert(
			*identifier,
			RingExponent::R2e9,
		);
	}

	fn valid_proof(_collection: &crate::Collection, message: &[u8]) -> MockProof {
		MockProof {
			alias: [1u8; 32],
			valid: true,
			message: BoundedVec::try_from(message.to_vec()).expect("message fits bound"),
		}
	}

	fn candidate() -> u64 {
		0
	}

	fn sign(_message: &[u8]) -> UintAuthorityId {
		UintAuthorityId(0)
	}

	fn set_time(seconds: u64) {
		MOCK_NOW.with(|v| *v.borrow_mut() = seconds);
	}
}

// ========== DotNS Gateway Config ==========

parameter_types! {
	pub const MockMaxContractCallWeight: frame_support::weights::Weight =
		frame_support::weights::Weight::from_parts(500_000_000, 50_000);
	pub const MockMaxValiditySeconds: u64 = 600;
	pub const MockMaxFutureSkewSeconds: u64 = 15;
}

impl crate::Config for Test {
	type WeightInfo = ();
	type MemberService = MembersSubscriber;
	type ContractCaller = MockContractCaller;
	type AddressMapper = TestAddressMapper;
	type MaxContractCallWeight = MockMaxContractCallWeight;
	type MaxValiditySeconds = MockMaxValiditySeconds;
	type MaxFutureSkewSeconds = MockMaxFutureSkewSeconds;
	type UnixTime = MockUnixTime;
	type AttestationAllowanceManager = EnsureRoot<u64>;
	type DispatcherAddressManager = EnsureRoot<u64>;
	type AttestationSignature = UintAuthorityId;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
}

// ========== Test Helpers ==========

pub fn new_test_ext() -> sp_io::TestExternalities {
	let mut t = frame_system::GenesisConfig::<Test>::default().build_storage().unwrap();
	crate::GenesisConfig::<Test> {
		dispatcher_address: Some(DispatcherAddr::get()),
		_phantom: core::marker::PhantomData,
	}
	.assimilate_storage(&mut t)
	.unwrap();
	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| {
		// Setting up ring roots for both collections
		set_mock_ring_root(*PEOPLE_IDENTIFIER, 0);
		set_mock_ring_root(*PEOPLE_LITE_IDENTIFIER, 0);
		clear_contract_calls();
	});
	ext
}

pub fn valid_proof(alias: Alias, message: &[u8]) -> MockProof {
	MockProof {
		alias,
		valid: true,
		message: BoundedVec::try_from(message.to_vec()).expect("message fits bound"),
	}
}

pub fn invalid_proof() -> MockProof {
	MockProof { alias: [0u8; 32], valid: false, message: BoundedVec::new() }
}

pub fn valid_candidate_signature(candidate: u64) -> UintAuthorityId {
	UintAuthorityId(candidate)
}

pub fn invalid_candidate_signature() -> UintAuthorityId {
	UintAuthorityId(u64::MAX)
}

pub fn set_attestation_allowance(attester: u64, count: u32) {
	crate::pallet::AttestationAllowance::<Test>::insert(attester, count);
}

/// Builds a `PersonRegistration`` origin.
pub fn person_registration_origin(alias: Alias) -> RuntimeOrigin {
	RuntimeOrigin::from(OriginCaller::DotnsGateway(crate::Origin::PersonRegistration(alias)))
}

/// Mock signature from `signer`
pub fn offchain_signature(signer: u64) -> UintAuthorityId {
	UintAuthorityId(signer)
}

/// Invalid signature
pub fn invalid_offchain_signature() -> UintAuthorityId {
	UintAuthorityId(u64::MAX)
}

/// Drives the pallet extension `validate_only` against the `register_name` call. Returns the
/// validation outcome along with the origin.
pub fn validate_register(
	proof: ProofOf<Test>,
	ring_index: RingIndex,
	signature: UintAuthorityId,
	who: u64,
	label: BaseLabel,
	link: Link,
) -> Result<(ValidTransaction, (), RuntimeOrigin), TransactionValidityError> {
	let tx_ext = AsDotnsGateway::<Test>::new(Some(AsDotnsGatewayInfo::RegisterFullName {
		proof,
		ring_index,
		signature,
	}));
	let call = RuntimeCall::DotnsGateway(crate::pallet::Call::register_name { who, label, link });
	let info = call.get_dispatch_info();
	tx_ext.validate_only(
		frame_system::RawOrigin::None.into(),
		&call,
		&info,
		0,
		sp_runtime::transaction_validity::TransactionSource::External,
		0,
	)
}
