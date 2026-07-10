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

use codec::Encode;
use core::time::Duration;
use frame_support::{derive_impl, parameter_types, traits::UnixTime};
use frame_system::{
	offchain::{CreateAuthorizedTransaction, CreateBare, CreateTransaction, CreateTransactionBase},
	pallet_prelude::ExtrinsicFor,
	EnsureRoot,
};
use indiv_support::traits::RingExponent;
use sp_core::{ConstU16, ConstU32, ConstU64, H256};
use sp_runtime::{
	traits::{BlakeTwo256, IdentityLookup},
	BuildStorage,
};
use verifiable::{mock::Mock, GenerateVerifiable};

type Block = frame_system::mocking::MockBlock<Test>;

pub struct MockTime;
impl UnixTime for MockTime {
	fn now() -> Duration {
		Duration::from_secs(1_000_000)
	}
}

// Configure a mock runtime to test the pallet.
frame_support::construct_runtime!(
	pub enum Test
	{
		System: frame_system,
		ChunksManager: indiv_pallet_chunks_manager,
		Members: indiv_pallet_members,
		People: indiv_pallet_people,
		DummyDim: crate
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
	type AccountId = u64;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Block = Block;
	type RuntimeEvent = RuntimeEvent;
	type BlockHashCount = ConstU64<250>;
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = ();
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ConstU16<42>;
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
	type Clock = MockTime;
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

impl indiv_pallet_people::Config for Test {
	type WeightInfo = ();
	type MemberService = Members;
	type CollectionOwner = MockCollectionOwner;
	type AccountContexts = ();
	type OnboardingQueuePageSize = ConstU32<512>;
	type RingExponent = FlexibleRingExp;
	type StaleAliasCleanupInterval = ConstU64<5>;
	type SelfInclusionDelay = ConstU64<3600>;
	type ManagerOrigin = EnsureRoot<Self::AccountId>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
}

impl crate::Config for Test {
	type WeightInfo = ();
	type UpdateOrigin = EnsureRoot<Self::AccountId>;
	type MaxPersonBatchSize = ConstU32<1000>;
	type People = People;
}

#[allow(dead_code)]
pub fn advance_to(b: u64) {
	while System::block_number() < b {
		System::set_block_number(System::block_number() + 1);
	}
}

impl<LocalCall> CreateBare<LocalCall> for Test
where
	RuntimeCall: From<LocalCall>,
{
	fn create_bare(call: Self::RuntimeCall) -> Self::Extrinsic {
		ExtrinsicFor::<Test>::new_bare(call)
	}
}

impl<LocalCall> CreateTransactionBase<LocalCall> for Test
where
	RuntimeCall: From<LocalCall>,
{
	type Extrinsic = ExtrinsicFor<Test>;
	type RuntimeCall = RuntimeCall;
}

impl<LocalCall> CreateTransaction<LocalCall> for Test
where
	RuntimeCall: From<LocalCall>,
{
	type Extension = ();
	fn create_transaction(
		call: <Self as CreateTransactionBase<LocalCall>>::RuntimeCall,
		_extension: Self::Extension,
	) -> Self::Extrinsic {
		ExtrinsicFor::<Test>::new_transaction(call, ())
	}
}

impl<LocalCall> CreateAuthorizedTransaction<LocalCall> for Test
where
	RuntimeCall: From<LocalCall>,
{
	fn create_extension() -> Self::Extension {}
}

pub struct ConfigRecord;

pub fn new_config() -> ConfigRecord {
	ConfigRecord
}

pub struct TestExt(ConfigRecord);
#[allow(dead_code)]
impl TestExt {
	pub fn new() -> Self {
		Self(new_config())
	}

	pub fn execute_with<R>(self, f: impl Fn() -> R) -> R {
		new_test_ext().execute_with(f)
	}
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	// Create a page of chunks and compute its hash
	let chunks: Vec<<Mock as GenerateVerifiable>::StaticChunk> = [(); 1024].to_vec();
	let encoded_chunks = chunks.encode();
	let page_hash = sp_io::hashing::blake2_256(&encoded_chunks);

	RuntimeGenesisConfig {
		system: Default::default(),
		chunks_manager: indiv_pallet_chunks_manager::GenesisConfig::<Test> {
			encoded_chunk_page_hashes: vec![(RingExponent::R2e9.exponent(), vec![page_hash])],
			..Default::default()
		},
	}
	.build_storage()
	.unwrap()
	.into()
}
