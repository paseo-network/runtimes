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
	AsAuthorizer, CallInspector, EnsureAllowedAuthorizers, ValidTransactionParams,
	DEFAULT_MAX_BLOCK_TRANSACTIONS, DEFAULT_MAX_TRANSACTION_SIZE,
};
use pallet_xcm::EnsureXcm;
use sp_runtime::transaction_validity::{TransactionLongevity, TransactionPriority};

parameter_types! {
	/// Cap on the total bytes committed to permanent storage (via `renew`) across all
	/// authorizations on this chain. Seeded at 1 TiB; storage-backed so governance
	/// (root) can raise/lower it via `system.set_storage` without a runtime upgrade.
	pub storage MaxPermanentStorageSize: u64 = 1024 * 1024 * 1024 * 1024;
}

/// `RemoveExpired*` / `RemoveExhausted*` (permissionless cleanup) sit at the top so they always
/// run before stores compete for blockspace.
const CLEANUP_PRIORITY: TransactionPriority = TransactionPriority::MAX;
/// Base priority for `store` / `renew` / `authorize`. Picked well below
/// `TransactionPriority::MAX` so `AllowanceBasedPriority` can add its boost without saturating
/// `u64`, while still leaving plenty of headroom above generic transactions.
const STORE_PRIORITY: TransactionPriority = TransactionPriority::MAX / 4;
const TX_LONGEVITY: TransactionLongevity = crate::DAYS as TransactionLongevity;

parameter_types! {
	pub const AuthorizationPeriod: crate::BlockNumber = 14 * crate::DAYS;
	// Pool parameters, one set per call family. The tag prefixes must be pairwise distinct —
	// `integrity_test` asserts it, because `ContentHash` and `AccountId32` both encode to 32
	// bytes and a shared prefix would silently make two families dedup against each other.
	pub const StoreTxParams: ValidTransactionParams =
		ValidTransactionParams::new("TransactionStorageStore", STORE_PRIORITY, TX_LONGEVITY);
	pub const RenewTxParams: ValidTransactionParams =
		ValidTransactionParams::new("TransactionStorageRenew", STORE_PRIORITY, TX_LONGEVITY);
	pub const AuthorizeTxParams: ValidTransactionParams =
		ValidTransactionParams::new("TransactionStorageAuthorize", STORE_PRIORITY, TX_LONGEVITY);
	pub const RemoveExpiredAccountAuthorizationTxParams: ValidTransactionParams =
		ValidTransactionParams::new(
			"TransactionStorageRemoveExpiredAccountAuthorization",
			CLEANUP_PRIORITY,
			TX_LONGEVITY,
		);
	pub const RemoveExpiredPreimageAuthorizationTxParams: ValidTransactionParams =
		ValidTransactionParams::new(
			"TransactionStorageRemoveExpiredPreimageAuthorization",
			CLEANUP_PRIORITY,
			TX_LONGEVITY,
		);
	pub const RemoveExhaustedAuthorizerTxParams: ValidTransactionParams =
		ValidTransactionParams::new(
			"TransactionStorageRemoveExhaustedAuthorizer",
			CLEANUP_PRIORITY,
			TX_LONGEVITY,
		);
}

/// Tells [`pallet_bulletin_transaction_storage::extension::ValidateAuthorizedCalls`] how to find
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
	type StoreTxParams = StoreTxParams;
	type AuthorizeTxParams = AuthorizeTxParams;
	type RemoveExpiredAccountAuthorizationTxParams = RemoveExpiredAccountAuthorizationTxParams;
	type RemoveExpiredPreimageAuthorizationTxParams = RemoveExpiredPreimageAuthorizationTxParams;
	type RemoveExhaustedAuthorizerTxParams = RemoveExhaustedAuthorizerTxParams;
	// The two opaque payloads the storage pallet carries on behalf of the renewal pallet: the
	// per-entry `EntryKind` and the per-authorization `PermanentExtent`. The storage pallet has
	// no renewal vocabulary of its own — it only stores and resets these.
	type EntryMeta = pallet_bulletin_data_renewal::EntryKind;
	type AuthorizationExtra = pallet_bulletin_data_renewal::PermanentExtent;
	type OnObsoleteTransactions = crate::DataRenewal;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = pallet_bulletin_data_renewal::RenewalBenchmarkHelper;
}

/// Renewal / auto-renewal lifecycle, split out of `pallet-bulletin-transaction-storage`.
impl pallet_bulletin_data_renewal::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = crate::weights::pallet_bulletin_data_renewal::WeightInfo<Runtime>;
	type MaxPermanentStorageSize = MaxPermanentStorageSize;
	type RenewTxParams = RenewTxParams;
}

parameter_types! {
	/// Maximum allowable skew between the user's submit timestamp and the on-chain
	/// time when validating a HOP promotion: 48 hours, in milliseconds.
	pub const SubmitTimestampTolerance: u64 = 48 * 60 * 60 * 1000;
	// Lowest priority: promotion only fills blockspace stores would not have used.
	// `integrity_test` asserts `promote` sits below `store`.
	pub const PromoteTxParams: ValidTransactionParams =
		ValidTransactionParams::new("HopPromotion", 0, 5);
}

impl pallet_bulletin_hop_promotion::Config for Runtime {
	type SubmitTimestampTolerance = SubmitTimestampTolerance;
	type PromoteTxParams = PromoteTxParams;
	type WeightInfo = crate::weights::pallet_bulletin_hop_promotion::WeightInfo<Runtime>;
}
