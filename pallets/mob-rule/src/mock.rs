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

use crate::*;
use frame_support::{
	derive_impl, parameter_types,
	traits::{EnsureOriginWithArg, OriginTrait, UnixTime},
	PalletId,
};
use frame_system::{
	offchain::{CreateBare, CreateTransactionBase},
	pallet_prelude::ExtrinsicFor,
	EnsureRoot,
};
use indiv_support::traits::CountedMembers;
use sp_core::{ConstU16, ConstU32, ConstU64, H256};
use sp_runtime::{
	traits::{BlakeTwo256, IdentityLookup},
	BuildStorage, Percent,
};
use xcm::v5::Location;

type Block = frame_system::mocking::MockBlock<Test>;

// Configure a mock runtime to test the pallet.
frame_support::construct_runtime!(
	pub enum Test
	{
		System: frame_system,
		Balances: pallet_balances,
		MobRule: crate,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type BaseCallFilter = frame_support::traits::Everything;
	type BlockWeights = ();
	type BlockLength = ();
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	type RuntimeTask = RuntimeTask;
	type Nonce = u64;
	type Hash = H256;
	type Hashing = BlakeTwo256;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Block = Block;
	type BlockHashCount = ConstU64<250>;
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = pallet_balances::AccountData<u64>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ConstU16<42>;
	type OnSetCode = ();
	type MaxConsumers = frame_support::traits::ConstU32<16>;
}

parameter_types! {
	pub const ExistentialDeposit: u64 = 5;
	pub const MaxReserves: u32 = 50;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
	type Balance = u64;
	type RuntimeEvent = RuntimeEvent;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
	type MaxLocks = ();
	type MaxReserves = MaxReserves;
	type ReserveIdentifier = [u8; 8];
	type RuntimeFreezeReason = RuntimeFreezeReason;
	type FreezeIdentifier = ();
}

parameter_types! {
	pub static Now: core::time::Duration = core::time::Duration::from_millis(0);
}

pub struct TestClock;
impl UnixTime for TestClock {
	fn now() -> core::time::Duration {
		Now::get()
	}
}

parameter_types! {
	pub const PotId: PalletId = PalletId(*b"PotRwrds");
	pub const MinTurnoutPercentage: Percent = Percent::from_percent(10);
	pub const BalancesLocation: Location = Location::here();
}

thread_local! {
	pub static VOTER_COUNT: core::cell::RefCell<u32> = const { core::cell::RefCell::new(0) };
}

/// Ensures the origin has an alias lower than 5.
///
/// This struct is used to implement the `EnsureOriginWithArg` trait, which checks
/// if the origin's alias is less than 5. If the check passes, it returns the alias.
pub struct EnsureAliasLowerThan5;
impl EnsureOriginWithArg<RuntimeOrigin, Context> for EnsureAliasLowerThan5 {
	type Success = Alias;

	fn try_origin(o: RuntimeOrigin, _context: &Context) -> Result<Self::Success, RuntimeOrigin> {
		match o.as_signer() {
			Some(id) if *id < 5 => {
				let mut alias: Alias = [0u8; 32];
				alias[..8].copy_from_slice(&id.to_le_bytes());
				Ok(alias)
			},
			_ => Err(o),
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn try_successful_origin(_context: &Context) -> Result<RuntimeOrigin, ()> {
		use frame_system::RawOrigin;
		Ok(RawOrigin::Signed(0).into())
	}
}

impl CountedMembers for EnsureAliasLowerThan5 {
	fn active_count() -> u32 {
		VOTER_COUNT.with(|c| *c.borrow())
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn set_active_count(count: u32) {
		VOTER_COUNT.with(|c| *c.borrow_mut() = count);
	}
}

/// Utility functions of EnsureAliasLowerThan5.
impl EnsureAliasLowerThan5
where
	RuntimeOrigin: core::fmt::Debug,
{
	pub fn get_alias(origin: RuntimeOrigin) -> Alias {
		EnsureAliasLowerThan5::try_origin(origin, &Default::default()).unwrap()
	}

	pub fn set_voter_count(voter_count: u32) {
		VOTER_COUNT.with(|c| *c.borrow_mut() = voter_count);
	}
}

pub type Extrinsic = ExtrinsicFor<Test>;

impl<LocalCall> CreateBare<LocalCall> for Test
where
	RuntimeCall: From<LocalCall>,
{
	fn create_bare(call: Self::RuntimeCall) -> Self::Extrinsic {
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

impl crate::Config for Test {
	type WeightInfo = ();
	type Currency = Balances;
	type CurrencyLocationInfo = BalancesLocation;
	type Clock = TestClock;
	type EnsurePerson = EnsureAliasLowerThan5;
	type MaxVoteClaimDuration = ConstU64<7200>;
	type MinCaseDuration = ConstU32<{ 24 * 60 * 60 }>;
	type MaxVotingDuration = ConstU32<{ 14 * 24 * 60 * 60 }>;
	type MinTurnoutNominal = ConstU32<1>;
	type MinTurnoutPercentage = MinTurnoutPercentage;
	type MaxPayoutRoundSchedules = ConstU32<5>;
	type VotingPenaltyDuration = ConstU64<10>;
	type InterventionOrigin = EnsureRoot<Self::AccountId>;
	type PotId = PotId;
	type MaxVotesClaimable = ConstU32<10>;
	type OffchainWorkInterval = ConstU64<5>;
	type CleanVotesBatchSize = ConstU32<6>;
	type VotesOpenForClaimsDuration = ConstU32<{ 60 * 60 }>;
	type MinimumVoterThreshold = ConstU32<1>;

	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = BenchHelper;
}

pub fn advance_to(b: u64) {
	while System::block_number() < b {
		System::set_block_number(System::block_number() + 1);
	}
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
	let c = frame_system::GenesisConfig::<Test>::default().build_storage().unwrap();
	sp_io::TestExternalities::from(c)
}

#[cfg(feature = "runtime-benchmarks")]
pub struct BenchHelper;

#[cfg(feature = "runtime-benchmarks")]
use std::time::Duration;

#[cfg(feature = "runtime-benchmarks")]
impl benchmarking::BenchmarkHelper<Test> for BenchHelper {
	fn set_valid_time() {
		// A reasonable time away from genesis block for benchmarks
		Now::set(Duration::from_secs(14 * 24 * 60 * 60 + 3600));
	}

	fn setup_currency() {
		// not needed for this runtime
	}
}
