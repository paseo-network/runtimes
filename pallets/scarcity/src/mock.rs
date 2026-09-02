// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Mock runtime for scarcity pallet tests.

use crate as pallet_scarcity;
use frame_support::{
	derive_impl, parameter_types,
	traits::{
		fungible::{self, HoldConsideration},
		ConstU32, ConstU64, LinearStoragePrice, UnixTime,
	},
	weights::{constants::RocksDbWeight, Weight},
};
use sp_runtime::{
	traits::{Identity, IdentityLookup},
	BuildStorage,
};

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
	pub enum Test {
		System: frame_system = 0,
		Balances: pallet_balances = 1,
		Scarcity: pallet_scarcity = 2,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Nonce = u64;
	type Block = Block;
	type BlockHashCount = ConstU64<250>;
	type DbWeight = RocksDbWeight;
	type AccountId = u64;
	type Lookup = IdentityLookup<Self::AccountId>;
	type AccountData = pallet_balances::AccountData<u64>;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
	type Balance = u64;
	type ExistentialDeposit = ConstU64<1>;
	type AccountStore = System;
	type RuntimeHoldReason = RuntimeHoldReason;
}

parameter_types! {
	pub static MockNow: u64 = 0;
}

/// Test-controlled Unix time source.
pub struct MockUnixTime;
impl UnixTime for MockUnixTime {
	fn now() -> core::time::Duration {
		core::time::Duration::from_secs(MockNow::get())
	}
}

type TestStoragePrice = LinearStoragePrice<ConstU64<1>, ConstU64<1>, u64>;

parameter_types! {
	/// Metadata entries one instance may carry. The integrity tests raise it.
	pub storage MaxInstanceMetadata: u32 = 3;
	/// Weight the mock policy charges per metadata pair. The integrity tests raise it.
	pub storage PolicyWeightPerPair: Weight = Weight::from_parts(11, 22);
	/// Weight the mock deletion hook charges. The integrity tests raise it.
	pub storage DeletionHookWeight: Weight = Weight::from_parts(123, 45);
}

parameter_types! {
	pub const ScarcityHoldReason: RuntimeHoldReason =
		RuntimeHoldReason::Scarcity(crate::HoldReason::StorageDeposit);
}

type TestConsideration = HoldConsideration<u64, Balances, ScarcityHoldReason, Identity, u64>;

impl crate::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = ();
	type UnixTime = MockUnixTime;
	type Balance = u64;
	type Consideration = TestConsideration;
	type CollectionDeposit = TestStoragePrice;
	type ItemDeposit = TestStoragePrice;
	type InstanceDeposit = TestStoragePrice;
	type MetadataDeposit = TestStoragePrice;
	type MaxKeyLen = ConstU32<32>;
	type MaxValueLen = ConstU32<256>;
	type MaxInstanceMetadata = MaxInstanceMetadata;
	type LockPeriod = ConstU64<60>;
	type MaxTransferPriority = ConstU64<1_000_000>;
	type OnCollectionDeleted = RecordCollectionDeletion;
	type OnPurseOccupied = RecordPurseOccupancy;
	type MetadataPolicy = RejectReservedValue;
}

parameter_types! {
	/// The last collection the deletion hook was called for, so tests can assert it fired.
	pub storage LastDeletedCollection: Option<crate::CollectionId> = None;
}

/// Records the collection passed to the deletion hook and charges a distinct weight, so the
/// wiring and the weight added to `delete_collection` are both observable in tests.
pub struct RecordCollectionDeletion;
impl crate::OnCollectionDeleted for RecordCollectionDeletion {
	fn on_collection_deleted(collection: crate::CollectionId) {
		LastDeletedCollection::set(&Some(collection));
	}

	fn on_delete_weight() -> Weight {
		DeletionHookWeight::get()
	}
}

parameter_types! {
	/// Every purse key the occupancy hook was called for, so tests can assert which paths fire.
	pub storage OccupiedPurses: alloc::vec::Vec<u64> = alloc::vec![];
}

/// Records the purse passed to the occupancy hook and charges a distinct weight, so the wiring
/// and the weight added to `mint` are both observable in tests.
pub struct RecordPurseOccupancy;
impl crate::OnPurseOccupied<u64> for RecordPurseOccupancy {
	fn on_purse_occupied(purse: &u64) {
		let mut purses = OccupiedPurses::get();
		purses.push(*purse);
		OccupiedPurses::set(&purses);
	}

	fn on_purse_occupied_weight() -> frame_support::weights::Weight {
		frame_support::weights::Weight::from_parts(678, 90)
	}
}

/// The metadata key the mock policy constrains.
pub const POLICED_KEY: &[u8] = b"policed";
/// The value the mock policy refuses under [`POLICED_KEY`].
pub const REJECTED_VALUE: &[u8] = b"no";

/// Stands in for a runtime policy that types one key, charging a distinct weight so both the
/// rejection and the weight added to every metadata write are observable in tests.
pub struct RejectReservedValue;
impl<Key: AsRef<[u8]>, Value: AsRef<[u8]>> crate::ValidateMetadata<Key, Value>
	for RejectReservedValue
{
	fn validate(key: &Key, value: &Value) -> Result<(), sp_runtime::DispatchError> {
		if key.as_ref() == POLICED_KEY && value.as_ref() == REJECTED_VALUE {
			return Err(sp_runtime::DispatchError::Other("policed value"));
		}
		Ok(())
	}

	fn validate_weight(pairs: u32) -> Weight {
		PolicyWeightPerPair::get().saturating_mul(pairs as u64)
	}
}

pub fn scarcity_hold_reason() -> RuntimeHoldReason {
	RuntimeHoldReason::Scarcity(crate::HoldReason::StorageDeposit)
}

pub fn held(who: u64) -> u64 {
	<Balances as fungible::InspectHold<u64>>::balance_on_hold(&scarcity_hold_reason(), &who)
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	let storage = RuntimeGenesisConfig {
		system: Default::default(),
		balances: pallet_balances::GenesisConfig::<Test> {
			balances: (1..=10).map(|account| (account, 1_000_000)).collect(),
			..Default::default()
		},
	}
	.build_storage()
	.unwrap();
	let mut ext: sp_io::TestExternalities = storage.into();
	ext.execute_with(|| {
		MockNow::set(0);
		System::set_block_number(1);
	});
	ext
}
