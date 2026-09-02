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

//! Integration tests exercising the precompile with the real `AliasAccounts` pallet
//! instead of a mock `PersonhoodLookup`. Validates the full chain:
//!
//! AccountToAlias storage → PersonhoodLookup trait → precompile → ABI-encoded uint8

use super::*;
use crate::{
	mock::{map_account, ALICE_ALIAS, BOB_ALIAS, CHARLIE_ALIAS, DAVE_ALIAS, PRECOMPILE_ADDR},
	DEFAULT_CONTEXT_ALIAS,
};

use alloc::{vec, vec::Vec};
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use core::{cell::RefCell, ops::Range};
use frame_support::{derive_impl, parameter_types, traits::Get};
use indiv_pallet_alias_accounts::types::AliasAccountInfo;
use indiv_pallet_members_subscriber::types::NotifierEndpoint;
use indiv_support::traits::{
	Alias, Context, ContextualAlias, Identifier, RevisionIndex, RingExponent, RingIndex,
	PEOPLE_IDENTIFIER, PEOPLE_LITE_IDENTIFIER,
};
use pallet_revive::{
	precompiles::{alloy::sol_types::SolCall, AddressMapper},
	ExecConfig, TransactionLimits,
};
use scale_info::TypeInfo;
use sp_core::{ConstU32, ConstU64};
use sp_runtime::{AccountId32, BuildStorage, Weight};
use verifiable::{AliasVec, Entropy, Error as VerifiableError, GenerateVerifiable};
use xcm::v5::{Assets, Location, SendError, SendResult, SendXcm, Xcm, XcmHash};

type Block = frame_system::mocking::MockBlock<IntegrationTest>;

frame_support::construct_runtime!(
	pub enum IntegrationTest
	{
		System: frame_system,
		Balances: pallet_balances,
		PalletAssets: pallet_assets,
		MembersSubscriber: indiv_pallet_members_subscriber,
		AliasAccounts: indiv_pallet_alias_accounts,
		Revive: pallet_revive,
	}
);

parameter_types! {
	pub const TestDbWeight: frame_support::weights::RuntimeDbWeight =
		frame_support::weights::RuntimeDbWeight { read: 1_000, write: 5_000 };
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for IntegrationTest {
	type Block = Block;
	type AccountId = AccountId32;
	type Lookup = sp_runtime::traits::IdentityLookup<Self::AccountId>;
	type AccountData = pallet_balances::AccountData<u64>;
	type DbWeight = TestDbWeight;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for IntegrationTest {
	type AccountStore = System;
	type MaxFreezes = frame_support::traits::VariantCountOf<RuntimeFreezeReason>;
}

#[derive_impl(pallet_assets::config_preludes::TestDefaultConfig)]
impl pallet_assets::Config for IntegrationTest {
	type Balance = u64;
	type Currency = Balances;
	type CreateOrigin =
		frame_support::traits::AsEnsureOriginWithArg<frame_system::EnsureSigned<AccountId32>>;
	type ForceOrigin = frame_system::EnsureRoot<AccountId32>;
	type Holder = ();
}

#[derive_impl(pallet_revive::config_preludes::TestDefaultConfig)]
impl pallet_revive::Config for IntegrationTest {
	type AddressMapper = pallet_revive::AccountId32Mapper<Self>;
	type Balance = u64;
	type Currency = Balances;
	type Precompiles = (PersonhoodCheck<Self>,);
	type UploadOrigin = frame_system::EnsureSigned<AccountId32>;
	type InstantiateOrigin = frame_system::EnsureSigned<AccountId32>;
}

impl Config for IntegrationTest {
	type Proof = indiv_pallet_alias_accounts::ProofOf<IntegrationTest>;
	type PersonhoodResolver = indiv_pallet_alias_accounts::Pallet<IntegrationTest>;
}

parameter_types! {
	pub const ProofValidityWindow: u64 = 100;
	pub const MappingRetention: u64 = 86_400;
	pub const PeopleLiteRingExp: RingExponent = RingExponent::R2e9;
	pub const PeopleRingExp: RingExponent = RingExponent::R2e9;
	pub const RingRootsNotifier: NotifierEndpoint = NotifierEndpoint {
		location: Location::parent(),
		pallet_index: 50,
	};
	pub const SelfParaId: u32 = 1000;
	pub const MaxMissingRootsPerCollection: u32 = 255;
	pub const MaxDeletedRingsPerCollection: u32 = 100;
	pub const MaxGapScanPerBatch: u32 = 32;
	pub const PurgePageSize: u32 = 100;
	pub const MaxCollections: u32 = 10;
	pub const ReplayCooldownSeconds: u64 = 60;
	pub const MaxUpdatesPerBatch: u32 = 10;
	pub const ReplayWarningThreshold: u32 = 5;
	pub const ReplayAbandonThreshold: u32 = 10;
}

pub const TEST_CONTEXT: Context = *b"pop:polkadot.network/people-aa  ";
pub const TEST_LITE_CONTEXT: Context = *b"pop:polkadot.network/plite-aa   ";

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
		Ok(DEFAULT_CONTEXT_ALIAS)
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

thread_local! {
	static MOCK_NOW: RefCell<u64> = const { RefCell::new(1_700_000_000) };
}

pub struct MockUnixTime;
impl frame_support::traits::UnixTime for MockUnixTime {
	fn now() -> core::time::Duration {
		core::time::Duration::from_secs(MOCK_NOW.with(|v| *v.borrow()))
	}
}

#[derive(
	Clone, PartialEq, Eq, Debug, Encode, Decode, TypeInfo, MaxEncodedLen, DecodeWithMemTracking,
)]
pub struct MockProof {
	pub alias: Alias,
	pub valid: bool,
}

pub struct TestVerifiable;
impl GenerateVerifiable for TestVerifiable {
	type Members = verifiable::mock::MockMembers<u64, ConstU32<16>>;
	type Intermediate = verifiable::mock::MockMembers<u64, ConstU32<16>>;
	type Member = u64;
	type Secret = u64;
	type Commitment = (u64, Vec<u64>);
	type Proof = MockProof;
	type Signature = ();
	type StaticChunk = ();
	type Config = ();

	fn start_members(_: Self::Config) -> Self::Intermediate {
		verifiable::mock::MockMembers::default()
	}
	fn push_members(
		inter: &mut Self::Intermediate,
		members: impl Iterator<Item = Self::Member>,
		_: impl Fn(Range<usize>) -> Result<Vec<Self::StaticChunk>, ()>,
	) -> Result<(), VerifiableError> {
		for m in members {
			inter.try_push(m).map_err(|_| VerifiableError::SetFull)?;
		}
		Ok(())
	}
	fn finish_members(inter: Self::Intermediate) -> Self::Members {
		inter
	}
	fn new_secret(e: Entropy) -> Self::Secret {
		e[0] as u64
	}
	fn member_from_secret(s: &Self::Secret) -> Self::Member {
		*s
	}
	fn open(
		_: Self::Config,
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
		_: Self::Commitment,
		_: &Self::Secret,
		_: &[&[u8]],
		_: &[u8],
	) -> Result<(Self::Proof, AliasVec), VerifiableError> {
		unimplemented!()
	}
	fn validate_multi_context(
		_: Self::Config,
		proof: &Self::Proof,
		_: &Self::Members,
		contexts: &[&[u8]],
		_: &[u8],
	) -> Result<AliasVec, VerifiableError> {
		if proof.valid {
			Ok(core::iter::repeat_n(proof.alias, contexts.len()).collect())
		} else {
			Err(VerifiableError::VerificationFailed)
		}
	}
	fn sign(_: &Self::Secret, _: &[u8]) -> Result<Self::Signature, VerifiableError> {
		Ok(())
	}
	fn verify_signature(_: &Self::Signature, _: &[u8], _: &Self::Member) -> bool {
		true
	}
	fn alias_in_context(_: &Self::Secret, _: &[u8]) -> Result<verifiable::Alias, VerifiableError> {
		unimplemented!()
	}
	fn is_member_valid(_: &Self::Member) -> bool {
		true
	}
}

type TxExtension = (frame_system::AuthorizeCall<IntegrationTest>,);
type Extrinsic = sp_runtime::generic::UncheckedExtrinsic<
	AccountId32,
	RuntimeCall,
	sp_runtime::testing::UintAuthorityId,
	TxExtension,
>;

impl<C> frame_system::offchain::CreateTransactionBase<C> for IntegrationTest
where
	RuntimeCall: From<C>,
{
	type RuntimeCall = RuntimeCall;
	type Extrinsic = Extrinsic;
}

impl<C> frame_system::offchain::CreateTransaction<C> for IntegrationTest
where
	RuntimeCall: From<C>,
{
	type Extension = TxExtension;
	fn create_transaction(
		call: <Self as frame_system::offchain::CreateTransactionBase<C>>::RuntimeCall,
		extension: Self::Extension,
	) -> Self::Extrinsic {
		Extrinsic::new_transaction(call, extension)
	}
}

impl<C> frame_system::offchain::CreateAuthorizedTransaction<C> for IntegrationTest
where
	RuntimeCall: From<C>,
{
	fn create_extension() -> Self::Extension {
		(frame_system::AuthorizeCall::new(),)
	}
}

impl indiv_pallet_members_subscriber::Config for IntegrationTest {
	type WeightInfo = ();
	type Crypto = TestVerifiable;
	type XcmSender = MockXcmSender;
	type RingRootsNotifier = RingRootsNotifier;
	type SelfParaId = SelfParaId;
	type MaxMissingRootsPerCollection = MaxMissingRootsPerCollection;
	type MaxDeletedRingsPerCollection = MaxDeletedRingsPerCollection;
	type MaxGapScanPerBatch = MaxGapScanPerBatch;
	type PurgePageSize = PurgePageSize;
	type MaxUpdatesPerBatch = MaxUpdatesPerBatch;
	type EnsureNotifierOrigin = MockEnsureNotifierOrigin;
	type EnsureTerminationOrigin = frame_system::EnsureRoot<AccountId32>;
	type MaxCollections = MaxCollections;
	type UnixTime = MockUnixTime;
	type ReplayCooldownSeconds = ReplayCooldownSeconds;
	type ReplayWarningThreshold = ReplayWarningThreshold;
	type ReplayAbandonThreshold = ReplayAbandonThreshold;
	type MaxRecentRootsPerRing = ConstU32<2>;
	type OldRootRetentionDuration = ConstU64<3600>;
	type OffchainWorkerInterval = ConstU64<1>;
}

parameter_types! {
	pub PgasAssetId: u32 = 1;
	pub storage AliasFee: Option<u64> = None;
}

impl indiv_pallet_alias_accounts::Config for IntegrationTest {
	type WeightInfo = ();
	type MemberService = MembersSubscriber;
	type UnixTime = MockUnixTime;
	type ProofValidityWindow = ProofValidityWindow;
	type MappingRetention = MappingRetention;
	type PeopleLiteRingExponent = PeopleLiteRingExp;
	type PeopleRingExponent = PeopleRingExp;
	type Fungibles = PalletAssets;
	type PgasAssetId = PgasAssetId;
	type AliasFee = AliasFee;
	type OffchainWorkerInterval = ConstU64<1>;
	type MaxStaleAliasBatch = ConstU32<4>;
}

fn id_to_account(id: u64) -> AccountId32 {
	let mut bytes = DEFAULT_CONTEXT_ALIAS;
	bytes[..8].copy_from_slice(&id.to_le_bytes());
	AccountId32::new(bytes)
}

fn seed_ring_root(collection: Identifier, ring: RingIndex, revision: RevisionIndex) {
	let now = MOCK_NOW.with(|v| *v.borrow());
	let record = indiv_pallet_members_subscriber::types::RingCommitmentRecord::<IntegrationTest> {
		root: verifiable::mock::MockMembers::default(),
		revision,
		source_time: now,
		source_sequence: 0,
	};
	let mut roots = frame_support::BoundedVec::new();
	roots.try_push(record).expect("MaxRecentRootsPerRing > 0");
	indiv_pallet_members_subscriber::Pallet::<IntegrationTest>::set_current_ring_roots(
		&collection,
		ring,
		roots,
	);
}

fn seed_alias(account: &AccountId32, collection: Identifier, alias: Alias, context: Context) {
	seed_alias_at_revision(account, collection, alias, context, 1);
}

fn seed_alias_at_revision(
	account: &AccountId32,
	collection: Identifier,
	alias: Alias,
	context: Context,
	revision: RevisionIndex,
) {
	let info =
		AliasAccountInfo { collection, revision, ring: 0, ca: ContextualAlias { alias, context } };
	indiv_pallet_alias_accounts::AccountToAlias::<IntegrationTest>::insert(account, info);
}

/// Append a ring-roots record with an explicit `source_time` without clearing existing records.
/// Used to build a multi-revision history where the latest record is not the one the alias
/// is pinned to.
fn push_ring_root(
	collection: Identifier,
	ring: RingIndex,
	revision: RevisionIndex,
	source_time: u64,
) {
	let record = indiv_pallet_members_subscriber::types::RingCommitmentRecord::<IntegrationTest> {
		root: verifiable::mock::MockMembers::default(),
		revision,
		source_time,
		source_sequence: 0,
	};
	let mut roots = indiv_pallet_members_subscriber::Pallet::<IntegrationTest>::current_ring_roots(
		&collection,
		ring,
	)
	.unwrap_or_default();
	roots.try_push(record).expect("MaxRecentRootsPerRing capacity exceeded");
	indiv_pallet_members_subscriber::Pallet::<IntegrationTest>::set_current_ring_roots(
		&collection,
		ring,
		roots,
	);
}

const MOCK_NOW_INIT: u64 = 1_700_000_000;

fn new_integration_ext() -> sp_io::TestExternalities {
	let t = RuntimeGenesisConfig {
		system: Default::default(),
		balances: Default::default(),
		pallet_assets: Default::default(),
		revive: Default::default(),
	}
	.build_storage()
	.unwrap();

	let mut ext: sp_io::TestExternalities = t.into();
	ext.execute_with(|| {
		System::set_block_number(1);
		indiv_pallet_members_subscriber::RingCollectionExponents::<IntegrationTest>::insert(
			*PEOPLE_IDENTIFIER,
			RingExponent::R2e9,
		);
		indiv_pallet_members_subscriber::RingCollectionExponents::<IntegrationTest>::insert(
			*PEOPLE_LITE_IDENTIFIER,
			RingExponent::R2e9,
		);
		seed_ring_root(*PEOPLE_IDENTIFIER, 0, 1);
		seed_ring_root(*PEOPLE_LITE_IDENTIFIER, 0, 1);
	});
	ext
}

fn bare_call_precompile(
	caller: u64,
	target: &AccountId32,
	context: &Context,
) -> pallet_revive::ContractResult<pallet_revive::ExecReturnValue, u64> {
	let caller_account = id_to_account(caller);
	map_account::<IntegrationTest>(&caller_account);

	let target_address =
		<IntegrationTest as pallet_revive::Config>::AddressMapper::to_address(target);

	let input = IPersonhood::personhoodStatusCall {
		account: target_address.0.into(),
		context: (*context).into(),
	}
	.abi_encode();

	pallet_revive::Pallet::<IntegrationTest>::bare_call(
		RuntimeOrigin::signed(caller_account),
		PRECOMPILE_ADDR,
		0u32.into(),
		TransactionLimits::WeightAndDeposit { weight_limit: Weight::MAX, deposit_limit: u64::MAX },
		input,
		&ExecConfig::new_substrate_tx(),
	)
}

fn call_precompile(
	caller: u64,
	target: &AccountId32,
	context: &Context,
) -> IPersonhood::PersonhoodInfo {
	let data = bare_call_precompile(caller, target, context)
		.result
		.expect("precompile call should succeed")
		.data;
	IPersonhood::personhoodStatusCall::abi_decode_returns(&data).unwrap()
}

#[test]
fn precompile_returns_full_via_real_alias_accounts() {
	new_integration_ext().execute_with(|| {
		let target = id_to_account(10);
		map_account::<IntegrationTest>(&target);

		seed_alias(&target, *PEOPLE_IDENTIFIER, ALICE_ALIAS, TEST_CONTEXT);

		let info = call_precompile(1, &target, &TEST_CONTEXT);
		assert_eq!(info.status as u8, FULL_STATUS);
		assert_eq!(info.contextAlias.0, ALICE_ALIAS);
	});
}

#[test]
fn precompile_returns_lite_via_real_alias_accounts() {
	new_integration_ext().execute_with(|| {
		let target = id_to_account(20);
		map_account::<IntegrationTest>(&target);

		seed_alias(&target, *PEOPLE_LITE_IDENTIFIER, BOB_ALIAS, TEST_LITE_CONTEXT);

		let info = call_precompile(1, &target, &TEST_LITE_CONTEXT);
		assert_eq!(info.status as u8, LITE_STATUS);
		assert_eq!(info.contextAlias.0, BOB_ALIAS);
	});
}

#[test]
fn precompile_returns_none_when_no_alias_registered() {
	new_integration_ext().execute_with(|| {
		let target = id_to_account(99);
		let info = call_precompile(1, &target, &TEST_CONTEXT);
		assert_eq!(info.status as u8, NO_STATUS);
		assert_eq!(info.contextAlias.0, DEFAULT_CONTEXT_ALIAS);
	});
}

#[test]
fn precompile_returns_none_for_wrong_context() {
	new_integration_ext().execute_with(|| {
		let target = id_to_account(30);
		map_account::<IntegrationTest>(&target);

		seed_alias(&target, *PEOPLE_IDENTIFIER, CHARLIE_ALIAS, TEST_CONTEXT);

		let wrong_context = [0xFFu8; 32];
		let info = call_precompile(1, &target, &wrong_context);
		assert_eq!(info.status as u8, NO_STATUS);
	});
}

#[test]
fn gas_refund_none_path_cheaper_than_some_path() {
	new_integration_ext().execute_with(|| {
		let found_target = id_to_account(50);
		map_account::<IntegrationTest>(&found_target);
		seed_alias(&found_target, *PEOPLE_IDENTIFIER, DAVE_ALIAS, TEST_CONTEXT);

		let missing_target = id_to_account(99);

		let result_found = bare_call_precompile(2, &found_target, &TEST_CONTEXT);
		let result_missing = bare_call_precompile(3, &missing_target, &TEST_CONTEXT);

		assert!(result_found.result.is_ok());
		assert!(result_missing.result.is_ok());

		// The early-`None` path (AccountToAlias miss) reports a lower `actual_weight` from the
		// resolver than the `Some` path. This proves the precompile is forwarding the resolver's
		// reported weight via `env.adjust_gas` rather than ignoring it.
		let weight_found = result_found.weight_consumed;
		let weight_missing = result_missing.weight_consumed;
		assert!(
			weight_missing.ref_time() < weight_found.ref_time(),
			"None path ({weight_missing:?}) should consume less weight than Some path ({weight_found:?})",
		);
	});
}

/// Exercises the non-latest-revision branch: the account is pinned to an older revision that
/// the member service still retains, so the lookup must return Full.
#[test]
fn precompile_returns_full_for_stale_but_retained_revision() {
	new_integration_ext().execute_with(|| {
		// `new_integration_ext` seeded revision 1 at `MOCK_NOW_INIT`. Push revision 2 as the
		// new latest; with `MaxRecentRootsPerRing = 2` the window is now full.
		push_ring_root(*PEOPLE_IDENTIFIER, 0, 2, MOCK_NOW_INIT + 1);

		let target = id_to_account(40);
		map_account::<IntegrationTest>(&target);

		seed_alias_at_revision(&target, *PEOPLE_IDENTIFIER, ALICE_ALIAS, TEST_CONTEXT, 1);

		// `MOCK_NOW` is still `MOCK_NOW_INIT`, well within the member service's retention of
		// revision 1, which runs from revision 2's source time.
		let info = call_precompile(1, &target, &TEST_CONTEXT);
		assert_eq!(info.status as u8, FULL_STATUS);
		assert_eq!(info.contextAlias.0, ALICE_ALIAS);
	});
}

fn bare_call_proof_precompile(
	caller: u64,
	expected_status: u8,
	proof_bytes: Vec<u8>,
	expected_alias: Alias,
	ring_index: RingIndex,
	context: &Context,
	revision: RevisionIndex,
	message: Vec<u8>,
) -> pallet_revive::ContractResult<pallet_revive::ExecReturnValue, u64> {
	let caller_account = id_to_account(caller);
	map_account::<IntegrationTest>(&caller_account);

	let input = IPersonhood::personhoodInfoByProofCall {
		request: IPersonhood::ProofVerificationRequest {
			expectedStatus: expected_status,
			proof: proof_bytes.into(),
			expectedAlias: expected_alias.into(),
			ringIndex: ring_index,
			context: (*context).into(),
			revision,
			message: message.into(),
		},
	}
	.abi_encode();

	pallet_revive::Pallet::<IntegrationTest>::bare_call(
		RuntimeOrigin::signed(caller_account),
		PRECOMPILE_ADDR,
		0u32.into(),
		TransactionLimits::WeightAndDeposit { weight_limit: Weight::MAX, deposit_limit: u64::MAX },
		input,
		&ExecConfig::new_substrate_tx(),
	)
}

fn call_proof_precompile_status(
	caller: u64,
	expected_status: u8,
	proof_bytes: Vec<u8>,
	expected_alias: Alias,
	ring_index: RingIndex,
	context: &Context,
	revision: RevisionIndex,
	message: Vec<u8>,
) -> bool {
	let data = bare_call_proof_precompile(
		caller,
		expected_status,
		proof_bytes,
		expected_alias,
		ring_index,
		context,
		revision,
		message,
	)
	.result
	.expect("precompile call should succeed")
	.data;
	IPersonhood::personhoodInfoByProofCall::abi_decode_returns(&data).unwrap()
}

#[test]
fn proof_precompile_returns_full_for_valid_people_proof() {
	new_integration_ext().execute_with(|| {
		let proof = MockProof { alias: ALICE_ALIAS, valid: true }.encode();

		let ok = call_proof_precompile_status(
			1,
			FULL_STATUS,
			proof,
			ALICE_ALIAS,
			0,
			&TEST_CONTEXT,
			1,
			vec![],
		);
		assert!(ok);
	});
}

#[test]
fn proof_precompile_returns_none_for_invalid_proof() {
	new_integration_ext().execute_with(|| {
		let proof = MockProof { alias: ALICE_ALIAS, valid: false }.encode();

		let ok = call_proof_precompile_status(
			1,
			FULL_STATUS,
			proof,
			ALICE_ALIAS,
			0,
			&TEST_CONTEXT,
			1,
			vec![],
		);
		assert!(!ok);
	});
}

#[test]
fn proof_precompile_returns_none_for_alias_mismatch() {
	new_integration_ext().execute_with(|| {
		let proof = MockProof { alias: ALICE_ALIAS, valid: true }.encode();

		let ok = call_proof_precompile_status(
			1,
			FULL_STATUS,
			proof,
			BOB_ALIAS,
			0,
			&TEST_CONTEXT,
			1,
			vec![],
		);
		assert!(!ok);
	});
}

#[test]
fn proof_precompile_returns_none_for_unknown_revision() {
	new_integration_ext().execute_with(|| {
		let proof = MockProof { alias: ALICE_ALIAS, valid: true }.encode();

		let ok = call_proof_precompile_status(
			1,
			FULL_STATUS,
			proof,
			ALICE_ALIAS,
			0,
			&TEST_CONTEXT,
			99,
			vec![],
		);
		assert!(!ok);
	});
}

#[test]
fn proof_precompile_returns_none_for_malformed_proof_bytes() {
	new_integration_ext().execute_with(|| {
		let result = bare_call_proof_precompile(
			1,
			FULL_STATUS,
			vec![0xFF],
			ALICE_ALIAS,
			0,
			&TEST_CONTEXT,
			1,
			vec![],
		);
		assert!(result.result.is_ok(), "decode failure must not revert");
		let ok = IPersonhood::personhoodInfoByProofCall::abi_decode_returns(
			&result.result.unwrap().data,
		)
		.unwrap();
		assert!(!ok);
	});
}

#[test]
fn proof_precompile_decode_failure_charges_full_weight() {
	new_integration_ext().execute_with(|| {
		let valid_proof = MockProof { alias: ALICE_ALIAS, valid: true }.encode();
		let mut malformed_proof = valid_proof.clone();
		*malformed_proof.last_mut().expect("encoded proof is not empty") = 0xFF;

		let result_valid = bare_call_proof_precompile(
			1,
			FULL_STATUS,
			valid_proof,
			ALICE_ALIAS,
			0,
			&TEST_CONTEXT,
			1,
			vec![],
		);
		let result_malformed = bare_call_proof_precompile(
			1,
			FULL_STATUS,
			malformed_proof,
			ALICE_ALIAS,
			0,
			&TEST_CONTEXT,
			1,
			vec![],
		);

		assert!(result_valid.result.is_ok());
		let ok_malformed = IPersonhood::personhoodInfoByProofCall::abi_decode_returns(
			&result_malformed.result.as_ref().expect("precompile call should succeed").data,
		)
		.unwrap();
		assert!(!ok_malformed);

		let weight_valid = result_valid.weight_consumed;
		let weight_malformed = result_malformed.weight_consumed;
		assert_eq!(
			weight_malformed, weight_valid,
			"decode-failure path ({weight_malformed:?}) must not refund below the verifying path ({weight_valid:?})",
		);
	});
}

#[test]
fn proof_precompile_oversized_proof_refunds_gas() {
	new_integration_ext().execute_with(|| {
		let valid_proof = MockProof { alias: ALICE_ALIAS, valid: true }.encode();

		let result_valid = bare_call_proof_precompile(
			1,
			FULL_STATUS,
			valid_proof,
			ALICE_ALIAS,
			0,
			&TEST_CONTEXT,
			1,
			vec![],
		);
		// Oversized: longer than `MockProof::max_encoded_len()`, rejected before decode.
		let oversize_len =
			<<IntegrationTest as crate::Config>::Proof as MaxEncodedLen>::max_encoded_len() + 1;
		let result_oversized = bare_call_proof_precompile(
			2,
			FULL_STATUS,
			vec![0xAA; oversize_len],
			ALICE_ALIAS,
			0,
			&TEST_CONTEXT,
			1,
			vec![],
		);

		assert!(result_valid.result.is_ok());
		assert!(result_oversized.result.is_ok());

		let ok_oversized = IPersonhood::personhoodInfoByProofCall::abi_decode_returns(
			&result_oversized.result.as_ref().unwrap().data,
		)
		.unwrap();
		assert!(!ok_oversized);

		let weight_valid = result_valid.weight_consumed;
		let weight_oversized = result_oversized.weight_consumed;
		assert!(
			weight_oversized.ref_time() < weight_valid.ref_time(),
			"oversized-proof path ({weight_oversized:?}) should refund vs valid path ({weight_valid:?})",
		);
	});
}

#[test]
fn proof_precompile_returns_none_for_unsupported_status() {
	new_integration_ext().execute_with(|| {
		let proof = MockProof { alias: ALICE_ALIAS, valid: true }.encode();

		let ok =
			call_proof_precompile_status(1, 99, proof, ALICE_ALIAS, 0, &TEST_CONTEXT, 1, vec![]);
		assert!(!ok);
	});
}

/// Exercises the expired-revision `None` path: the alias points at a non-latest revision that
/// the member service no longer retains, so the lookup must return None.
#[test]
fn precompile_returns_none_for_alias_at_expired_revision() {
	new_integration_ext().execute_with(|| {
		push_ring_root(*PEOPLE_IDENTIFIER, 0, 2, MOCK_NOW_INIT + 1);

		let target = id_to_account(41);
		map_account::<IntegrationTest>(&target);

		seed_alias_at_revision(&target, *PEOPLE_IDENTIFIER, ALICE_ALIAS, TEST_CONTEXT, 1);

		// Advance to the retention deadline for revision 1, measured from revision 2's source time.
		let retention = <<IntegrationTest as indiv_pallet_members_subscriber::Config>::OldRootRetentionDuration as Get<u64>>::get();
		MOCK_NOW.with(|v| *v.borrow_mut() = MOCK_NOW_INIT + 1 + retention);

		let info = call_precompile(1, &target, &TEST_CONTEXT);
		assert_eq!(info.status as u8, NO_STATUS);
		assert_eq!(info.contextAlias.0, DEFAULT_CONTEXT_ALIAS);
	});
}
