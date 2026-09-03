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

pub use super::*;

use frame_support::{
	derive_impl, parameter_types,
	traits::{
		fungible::HoldConsideration, ConstU32, ConstU64, Currency, LinearStoragePrice, UnixTime,
	},
};
use pallet_revive::precompiles::AddressMapper;
use sp_runtime::{traits::Identity, AccountId32, BuildStorage};

type Block = frame_system::mocking::MockBlock<Test>;

/// Address prefix of the per-collection precompile in this mock.
pub const COLLECTION_PREFIX: u16 = 0x0520;
/// Fixed address index of the factory precompile in this mock.
pub const FACTORY_INDEX: u16 = 0x0521;

frame_support::construct_runtime!(
	pub enum Test
	{
		System: frame_system,
		Balances: pallet_balances,
		Scarcity: pallet_scarcity,
		Revive: pallet_revive,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Block = Block;
	type AccountId = AccountId32;
	type Lookup = sp_runtime::traits::IdentityLookup<Self::AccountId>;
	type AccountData = pallet_balances::AccountData<u64>;
	// Mirrors the runtime, so tests see the address semantics production has.
	type OnNewAccount = pallet_revive::AutoMapper<Test>;
	type OnKilledAccount = pallet_revive::AutoMapper<Test>;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
	type AccountStore = System;
	type MaxFreezes = frame_support::traits::VariantCountOf<RuntimeFreezeReason>;
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

parameter_types! {
	pub const ScarcityHoldReason: RuntimeHoldReason =
		RuntimeHoldReason::Scarcity(pallet_scarcity::HoldReason::StorageDeposit);
}

type StoragePrice = LinearStoragePrice<ConstU64<1>, ConstU64<1>, u64>;
type Consideration = HoldConsideration<AccountId32, Balances, ScarcityHoldReason, Identity, u64>;

impl pallet_scarcity::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = ();
	type UnixTime = MockUnixTime;
	type Balance = u64;
	type Consideration = Consideration;
	type CollectionDeposit = StoragePrice;
	type ItemDeposit = StoragePrice;
	type InstanceDeposit = StoragePrice;
	type MetadataDeposit = StoragePrice;
	type MaxKeyLen = ConstU32<32>;
	type MaxValueLen = ConstU32<256>;
	type MaxInstanceMetadata = ConstU32<3>;
	type LockPeriod = ConstU64<60>;
	type MaxTransferPriority = ConstU64<1_000_000>;
	// Nothing in this mock keys state by a collection, so deletion needs no cleanup hook.
	type OnCollectionDeleted = ();
	// Mirrors the runtime: a mint makes its purse key addressable to the contract environment.
	type OnPurseOccupied = crate::MapPurseKey<Test>;
	type MetadataPolicy = crate::Erc721MetadataPolicy<Test>;
}

#[derive_impl(pallet_revive::config_preludes::TestDefaultConfig)]
impl pallet_revive::Config for Test {
	type AddressMapper = pallet_revive::AccountId32Mapper<Self>;
	type AutoMap = frame_support::traits::ConstBool<true>;
	type Balance = u64;
	type Currency = Balances;
	type Precompiles =
		(ScarcityCollection<Self, COLLECTION_PREFIX>, ScarcityFactory<Self, FACTORY_INDEX>);
	type UploadOrigin = frame_system::EnsureSigned<AccountId32>;
	type InstantiateOrigin = frame_system::EnsureSigned<AccountId32>;
}

/// The per-collection precompile address of `collection` under [`COLLECTION_PREFIX`].
pub fn collection_address(collection: CollectionId) -> H160 {
	let mut address = [0u8; 20];
	address[0..4].copy_from_slice(&collection.to_be_bytes());
	address[16..18].copy_from_slice(&COLLECTION_PREFIX.to_be_bytes());
	H160(address)
}

/// The factory precompile's fixed address under [`FACTORY_INDEX`].
pub fn factory_address() -> H160 {
	let mut address = [0u8; 20];
	address[16..18].copy_from_slice(&FACTORY_INDEX.to_be_bytes());
	H160(address)
}

/// Fund `account` and register its H160↔AccountId32 mapping with `pallet-revive`.
pub fn map_account(account: &AccountId32) {
	Balances::make_free_balance_be(account, u64::MAX / 2);
	let _ = <Test as pallet_revive::Config>::AddressMapper::map(account);
}

pub fn id_to_account(id: u64) -> AccountId32 {
	let mut bytes = [0u8; 32];
	bytes[..8].copy_from_slice(&id.to_le_bytes());
	AccountId32::new(bytes)
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	let t = RuntimeGenesisConfig {
		system: Default::default(),
		balances: Default::default(),
		revive: Default::default(),
	}
	.build_storage()
	.unwrap();

	let mut ext: sp_io::TestExternalities = t.into();
	ext.execute_with(|| {
		System::set_block_number(1);
		MockNow::set(1_700_000_000);
	});
	ext
}
