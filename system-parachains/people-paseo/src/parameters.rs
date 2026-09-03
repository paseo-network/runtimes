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

//! Governance-mutable Individuality runtime parameters.
//!
//! Ported from individuality v0.3.1 `runtimes/next-people-paseo/src/parameters.rs`.
//!
//! Every default below reproduces the value this runtime already runs on chain, so adopting
//! `pallet_parameters` is behaviour-neutral at the upgrade block. The two exceptions are
//! `lite_personhood::RegistrationFee` and `people_airdrops::PrizeSource`, which have no
//! predecessor here: both belong to `Config` items that individuality v0.3.1 introduces.

use crate::{ExistentialDeposit, *};
use frame_support::{
	dynamic_params::{dynamic_pallet_params, dynamic_params},
	traits::{ConstU32, EnsureOrigin, EnsureOriginWithArg},
	PalletId,
};
use indiv_pallet_resources::types::LongTermStorageAllocation;
use indiv_support::parameters::{
	AtLeast, AtLeastOne, AtMost, BenchmarkMax, SaturatingSubOne, StatementAllowanceGetter,
	StatementAllowanceParameter,
};
use sp_runtime::traits::AccountIdConversion;

const SECONDS_PER_DAY: u32 = 24 * 60 * 60;

/// The largest statement-store cleanup batch covered by the current resources weights.
pub const STMT_STORE_CLEANUP_LIMIT_CAP: u32 = 50;

/// The largest long-term storage cleanup batch covered by the current resources weights.
pub const LONG_TERM_STORAGE_CLEANUP_LIMIT_CAP: u32 = 20;

/// Dynamic runtime parameters configurable on-chain through [`pallet_parameters`].
///
/// The defaults preserve the People chain economics this runtime already enforces. Governance can
/// adjust them after deployment without another runtime upgrade.
#[dynamic_params(RuntimeParameters, pallet_parameters::Parameters::<Runtime>)]
pub mod dynamic_params {
	use super::*;

	/// Per-person statement and notification storage limits.
	#[dynamic_pallet_params]
	#[codec(index = 0)]
	pub mod statement_storage {
		#[codec(index = 0)]
		pub static AccountsApiAllowance: StatementAllowanceParameter =
			StatementAllowanceParameter { max_size: 500 * 1024, max_count: 2 };
		#[codec(index = 6)]
		pub static NotificationAllowance: StatementAllowanceParameter =
			StatementAllowanceParameter { max_size: 10 * 1024, max_count: 1 };
		#[codec(index = 10)]
		pub static LitePersonStatementLimit: StatementAllowanceParameter =
			StatementAllowanceParameter { max_size: 500 * 1024, max_count: 50 };
		#[codec(index = 11)]
		pub static PersonStatementLimit: StatementAllowanceParameter =
			StatementAllowanceParameter { max_size: 1024 * 1024, max_count: 200 };
		#[codec(index = 1)]
		pub static StmtStoreSlotsPerPeriod: u32 = 20;
		#[codec(index = 2)]
		pub static LiteStmtStoreSlotsPerPeriod: u32 = 10;
		#[codec(index = 3)]
		pub static StmtStoreCleanupLimit: u32 = 50;
		#[codec(index = 4)]
		pub static StmtStoreReplacementCooldown: u32 = 60;
		#[codec(index = 5)]
		pub static StmtStoreGraceWindow: u32 = 2 * 24 * 60 * 60;
		#[codec(index = 7)]
		pub static NotificationSlotsPerPeriod: u8 = 16;
		#[codec(index = 8)]
		pub static LiteNotificationSlotsPerPeriod: u8 = 8;
		#[codec(index = 9)]
		pub static NotificationPeriodDuration: u32 = SECONDS_PER_DAY;
	}

	/// Long-term storage allocation limits.
	#[dynamic_pallet_params]
	#[codec(index = 1)]
	pub mod bulletin_storage {
		#[codec(index = 0)]
		pub static LongTermStoragePeriodDuration: u32 = 14 * 24 * 60 * 60;
		#[codec(index = 1)]
		pub static LongTermStorageGraceWindow: u32 = 60 * 60;
		#[codec(index = 2)]
		pub static LongTermStorageClaimsPerPeriod: u8 = 100;
		#[codec(index = 3)]
		pub static LongTermStorageCleanupLimit: u32 = 20;
		#[codec(index = 4)]
		pub static LongTermStorageAllowanceForPeople: LongTermStorageAllocation =
			LongTermStorageAllocation { transactions: 100, bytes: 8 * 1024 * 1024 };
		#[codec(index = 5)]
		pub static LongTermStorageAllowanceForLitePeople: LongTermStorageAllocation =
			LongTermStorageAllocation { transactions: 10, bytes: 4 * 1024 * 1024 };
	}

	/// People airdrop draw funding.
	#[dynamic_pallet_params]
	#[codec(index = 2)]
	pub mod people_airdrops {
		/// Account funding the prize allocation of scheduled draws. The airdrop pallet records
		/// the source per draw at scheduling time, so an update only affects draws scheduled
		/// after it; draws already scheduled refund to the account they were funded from.
		#[codec(index = 0)]
		pub static PrizeSource: sp_runtime::AccountId32 =
			PalletId(*b"pop/pads").into_account_truncating();
	}

	/// Lite-person registration pricing.
	#[dynamic_pallet_params]
	#[codec(index = 3)]
	pub mod lite_personhood {
		/// Non-refundable native fee required to register as a lite person.
		#[codec(index = 0)]
		pub static RegistrationFee: Balance = 75 * UNITS;
	}
}

// `pallet_parameters` validates only the origin of an update, never the stored value, and the
// pallet `integrity_test` checks run against build-time defaults only. The aliases below
// therefore clamp every read, so no stored value can violate the resources pallet invariants
// they mirror.

pub type AccountsApiAllowance =
	StatementAllowanceGetter<dynamic_params::statement_storage::AccountsApiAllowance>;

pub type NotificationAllowance =
	StatementAllowanceGetter<dynamic_params::statement_storage::NotificationAllowance>;

pub type LitePersonStatementLimit =
	StatementAllowanceGetter<dynamic_params::statement_storage::LitePersonStatementLimit>;
pub type PersonStatementLimit =
	StatementAllowanceGetter<dynamic_params::statement_storage::PersonStatementLimit>;

/// Statement-store slots per period, kept non-zero.
pub type StmtStoreSlotsPerPeriod =
	AtLeastOne<dynamic_params::statement_storage::StmtStoreSlotsPerPeriod>;

/// Lite statement-store slots per period, kept non-zero and at most [`StmtStoreSlotsPerPeriod`].
pub type LiteStmtStoreSlotsPerPeriod = AtMost<
	AtLeastOne<dynamic_params::statement_storage::LiteStmtStoreSlotsPerPeriod>,
	StmtStoreSlotsPerPeriod,
>;

/// Statement-store cleanup batch size, kept non-zero and within the benchmarked cap.
pub type StmtStoreCleanupLimit = BenchmarkMax<
	AtMost<
		AtLeastOne<dynamic_params::statement_storage::StmtStoreCleanupLimit>,
		ConstU32<STMT_STORE_CLEANUP_LIMIT_CAP>,
	>,
	ConstU32<STMT_STORE_CLEANUP_LIMIT_CAP>,
>;

/// Statement replacement cooldown, kept non-zero and at most one day, the statement-store period.
pub type StmtStoreReplacementCooldown = AtMost<
	AtLeastOne<dynamic_params::statement_storage::StmtStoreReplacementCooldown>,
	ConstU32<SECONDS_PER_DAY>,
>;

/// Statement-store grace window, kept non-zero.
pub type StmtStoreGraceWindow = AtLeastOne<dynamic_params::statement_storage::StmtStoreGraceWindow>;

/// Highest valid notification slot identifier per period. Zero is valid, leaving only slot `0`.
pub type NotificationSlotsPerPeriod = dynamic_params::statement_storage::NotificationSlotsPerPeriod;

/// Highest valid lite notification slot identifier, kept at most [`NotificationSlotsPerPeriod`].
pub type LiteNotificationSlotsPerPeriod = AtMost<
	dynamic_params::statement_storage::LiteNotificationSlotsPerPeriod,
	NotificationSlotsPerPeriod,
>;

pub type NotificationPeriodDuration = dynamic_params::statement_storage::NotificationPeriodDuration;

/// Long-term storage period duration, kept non-zero.
pub type LongTermStoragePeriodDuration =
	AtLeastOne<dynamic_params::bulletin_storage::LongTermStoragePeriodDuration>;

/// Long-term storage grace window, kept smaller than [`LongTermStoragePeriodDuration`].
pub type LongTermStorageGraceWindow = AtMost<
	dynamic_params::bulletin_storage::LongTermStorageGraceWindow,
	SaturatingSubOne<LongTermStoragePeriodDuration>,
>;

/// Long-term storage claims per period, kept non-zero.
pub type LongTermStorageClaimsPerPeriod =
	AtLeastOne<dynamic_params::bulletin_storage::LongTermStorageClaimsPerPeriod>;

/// Long-term storage cleanup batch size, kept non-zero and within the benchmarked cap.
pub type LongTermStorageCleanupLimit = BenchmarkMax<
	AtMost<
		AtLeastOne<dynamic_params::bulletin_storage::LongTermStorageCleanupLimit>,
		ConstU32<LONG_TERM_STORAGE_CLEANUP_LIMIT_CAP>,
	>,
	ConstU32<LONG_TERM_STORAGE_CLEANUP_LIMIT_CAP>,
>;

pub type LongTermStorageAllowanceForPeople =
	dynamic_params::bulletin_storage::LongTermStorageAllowanceForPeople;
pub type LongTermStorageAllowanceForLitePeople =
	dynamic_params::bulletin_storage::LongTermStorageAllowanceForLitePeople;

/// Fee required to register as a lite person without device attestation.
///
/// A stored value below the existential deposit cannot make registration free or prevent the fee
/// pot account from existing.
pub type LitePersonRegistrationFee =
	AtLeast<dynamic_params::lite_personhood::RegistrationFee, ExistentialDeposit>;

/// Any account is a valid prize source, so the value is read unclamped.
pub type PeopleAirdropsPrizeSource = dynamic_params::people_airdrops::PrizeSource;

/// The relay-chain Root origin and the Fellowship governance voice may update these parameters.
pub struct DynamicParameterOrigin;
impl EnsureOriginWithArg<RuntimeOrigin, RuntimeParametersKey> for DynamicParameterOrigin {
	type Success = ();

	fn try_origin(
		origin: RuntimeOrigin,
		_key: &RuntimeParametersKey,
	) -> Result<Self::Success, RuntimeOrigin> {
		RootOrFellows::ensure_origin(origin.clone()).map(|_| ()).map_err(|_| origin)
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn try_successful_origin(_key: &RuntimeParametersKey) -> Result<RuntimeOrigin, ()> {
		Ok(RuntimeOrigin::root())
	}
}

impl pallet_parameters::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeParameters = RuntimeParameters;
	type AdminOrigin = DynamicParameterOrigin;
	// ⚠️ `weights::pallet_parameters` currently carries the crate's REFERENCE weights, not a
	// benchmark run on this runtime. See that module.
	type WeightInfo = weights::pallet_parameters::WeightInfo<Runtime>;
}

#[cfg(feature = "runtime-benchmarks")]
impl Default for RuntimeParameters {
	fn default() -> Self {
		RuntimeParameters::StatementStorage(
			dynamic_params::statement_storage::Parameters::StmtStoreSlotsPerPeriod(
				dynamic_params::statement_storage::StmtStoreSlotsPerPeriod,
				None,
			),
		)
	}
}
