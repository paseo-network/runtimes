// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-only

//! Storage-specific configurations.

use super::{
	xcm_config::IsAuthorizerParachain, Runtime, RuntimeCall, RuntimeEvent, RuntimeHoldReason,
};
use alloc::vec::Vec;
use bulletin_pallets_common::{inspect_utility_wrapper, NoCurrency};
use frame_support::{
	parameter_types,
	traits::{Contains, EitherOf},
};
use pallet_bulletin_transaction_storage::{
	AsAuthorizer, CallInspector, EnsureAllowedAuthorizers, DEFAULT_MAX_BLOCK_TRANSACTIONS,
	DEFAULT_MAX_TRANSACTION_SIZE,
};
use pallet_xcm::EnsureXcm;
use sp_runtime::transaction_validity::{TransactionLongevity, TransactionPriority};

parameter_types! {
	/// Cap on the total bytes committed to permanent storage (via `renew`) across all
	/// authorizations on this chain. Seeded at 1.7 TiB; storage-backed so governance
	/// (root) can raise/lower it via `system.set_storage` without a runtime upgrade.
	pub storage MaxPermanentStorageSize: u64 = 17 * 1024 * 1024 * 1024 * 1024 / 10;
}

parameter_types! {
	pub const AuthorizationPeriod: crate::BlockNumber = 14 * crate::DAYS;
	// Priorities and longevities used by the transaction storage pallet extrinsics.
	//
	// `RemoveExpiredAuthorization` (permissionless cleanup) sits at the top so it always
	// runs before stores compete for blockspace.
	pub const RemoveExpiredAuthorizationPriority: TransactionPriority = TransactionPriority::MAX;
	pub const RemoveExpiredAuthorizationLongevity: TransactionLongevity = crate::DAYS as TransactionLongevity;
	// Base priority for `store` / `renew`. Picked well below `TransactionPriority::MAX` so
	// `AllowanceBasedPriority` can add its boost without saturating `u64`, while still
	// leaving plenty of headroom above generic transactions.
	pub const StoreRenewPriority: TransactionPriority = TransactionPriority::MAX / 4;
	pub const StoreRenewLongevity: TransactionLongevity = crate::DAYS as TransactionLongevity;
}

/// Tells [`pallet_bulletin_transaction_storage::extension::ValidateStorageCalls`] how to find
/// storage calls inside wrapper extrinsics so it can recursively validate and consume
/// authorization.
///
/// Also implements [`Contains<RuntimeCall>`] returning `true` for storage-mutating calls
/// (store, store_with_cid_config, renew). Used with `EverythingBut` as the XCM
/// `SafeCallFilter` to block these calls from XCM dispatch — they require on-chain
/// authorization that XCM cannot provide.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct StorageCallInspector;

impl pallet_bulletin_transaction_storage::CallInspector<Runtime> for StorageCallInspector {
	fn inspect_wrapper(call: &RuntimeCall) -> Option<Vec<&RuntimeCall>> {
		match call {
			RuntimeCall::Utility(c) => inspect_utility_wrapper(c),
			// Sudo is intentionally not inspected: the sudo key holder can store
			// data via `sudo(store)` without authorization, as Root origin is
			// accepted by `ensure_authorized`.
			_ => None,
		}
	}
}

/// Returns `true` for storage-mutating TransactionStorage calls (store, store_with_cid_config,
/// renew). Recursively inspects wrapper calls (Utility) to prevent bypass via nesting.
/// Used with `EverythingBut` as the XCM `SafeCallFilter`.
impl Contains<RuntimeCall> for StorageCallInspector {
	fn contains(call: &RuntimeCall) -> bool {
		Self::is_storage_mutating_call(call, 0)
	}
}

/// The main business of the Bulletin chain.
impl pallet_bulletin_transaction_storage::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type Currency = NoCurrency<Self::AccountId, RuntimeHoldReason>;
	type RuntimeHoldReason = RuntimeHoldReason;
	type FeeDestination = ();
	type WeightInfo = crate::weights::pallet_bulletin_transaction_storage::WeightInfo<Runtime>;
	type MaxBlockTransactions = crate::ConstU32<{ DEFAULT_MAX_BLOCK_TRANSACTIONS }>;
	/// Max transaction size per block needs to be aligned with `BlockLength`.
	type MaxTransactionSize = crate::ConstU32<{ DEFAULT_MAX_TRANSACTION_SIZE }>;
	type MaxPermanentStorageSize = MaxPermanentStorageSize;
	type AuthorizationPeriod = AuthorizationPeriod;
	type AuthorizerRegistrarOrigin = frame_system::EnsureRoot<Self::AccountId>;
	type Authorizer = EitherOf<
		EitherOf<
			// Root can do whatever.
			AsAuthorizer<crate::EnsureRoot<Self::AccountId>, Self::AccountId, crate::BlockNumber>,
			// Sibling parachains listed in `AllowedParachainIds` can handle authorizations.
			AsAuthorizer<EnsureXcm<IsAuthorizerParachain>, Self::AccountId, crate::BlockNumber>,
		>,
		// Accounts registered in `AllowedAuthorizers` storage (managed via
		// `add_authorizer` / `remove_authorizer`).
		EnsureAllowedAuthorizers<Runtime>,
	>;
	type StoreRenewPriority = StoreRenewPriority;
	type StoreRenewLongevity = StoreRenewLongevity;
	type RemoveExpiredAuthorizationPriority = RemoveExpiredAuthorizationPriority;
	type RemoveExpiredAuthorizationLongevity = RemoveExpiredAuthorizationLongevity;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper =
		pallet_bulletin_transaction_storage::benchmarking::DefaultCheckProofHelper;
}

parameter_types! {
	/// Maximum allowable skew between the user's submit timestamp and the on-chain
	/// time when validating a HOP promotion: 48 hours, in milliseconds.
	pub const SubmitTimestampTolerance: u64 = 48 * 60 * 60 * 1000;
}

impl pallet_bulletin_hop_promotion::Config for Runtime {
	type SubmitTimestampTolerance = SubmitTimestampTolerance;
	type WeightInfo = crate::weights::pallet_bulletin_hop_promotion::WeightInfo<Runtime>;
}
