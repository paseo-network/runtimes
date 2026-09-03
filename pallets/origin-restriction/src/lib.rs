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

//! # Origin restriction pallet and transaction extension
//!
//! This pallet tracks certain origin and limits how much total "fee usage" they can accumulate.
//! Usage gradually recovers as the blocks of [`Config::BlockNumberProvider`] pass. A parachain
//! configures the relay chain block number there, so the recovery rate does not depend on the
//! local block production.
//!
//! First the entity is extracted from the restricted origin, the entity represents the granularity
//! of usage tracking.
//!
//! For example, an origin like `DaoOrigin { name: [u8; 8], tally: Percent }`
//! can have its usage tracked and restricted at the DAO level, so the tracked entity would be
//! `DaoEntity { name: [u8; 8] }`. This ensures that usage restrictions apply to the DAO as a whole,
//! independent of any particular voter percentage.
//!
//! Then when dispatching a transaction, if the entity’s new usage would exceed its max allowance,
//! the transaction is invalid, except if the call is in the set of calls permitted to exceed that
//! limit (`OperationAllowedOneTimeExcess`). In that case, as long as the entity's usage prior to
//! dispatch was zero, the transaction is valid (with respect to usage). If the entity's
//! usage is already above the limit, the transaction is always invalid. After dispatch, any call
//! flagged as `Pays::No` fully restores the consumed usage.
//!
//! To expand on `OperationAllowedOneTimeExcess`, user have to wait for the usage to completely
//! recover to zero before being able to do an operation that exceed max allowance.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
#[cfg(test)]
mod mock;
/// PASEO-LOCAL. Does not exist upstream — see the module documentation.
pub mod migration;
#[cfg(test)]
mod tests;
pub mod weights;

extern crate alloc;

pub use weights::WeightInfo;

use codec::{Decode, DecodeWithMemTracking, Encode};
use frame_support::{
	dispatch::{DispatchInfo, PostDispatchInfo},
	pallet_prelude::{Pays, Zero},
	traits::{ContainsPair, OriginTrait},
	weights::WeightToFee,
	DebugNoBound, Parameter,
};
use pallet_transaction_payment::OnChargeTransaction;
use scale_info::TypeInfo;
use sp_runtime::{
	traits::{
		AsTransactionAuthorizedOrigin, BlockNumberProvider, DispatchInfoOf, DispatchOriginOf,
		Dispatchable, Implication, PostDispatchInfoOf, TransactionExtension, ValidateResult,
	},
	transaction_validity::{
		InvalidTransaction, TransactionSource, TransactionValidityError, ValidTransaction,
	},
	Debug,
	DispatchError::BadOrigin,
	DispatchResult, SaturatedConversion, Saturating, Weight,
};

/// The allowance for an entity, defining its usage limit and recovery rate.
#[derive(Clone, Debug)]
pub struct Allowance<Balance> {
	/// The maximum usage allowed before transactions are restricted.
	pub max: Balance,
	/// The amount of usage recovered per block of [`Config::BlockNumberProvider`].
	pub recovery_per_block: Balance,
}

/// The restriction of an entity.
pub trait RestrictedEntity<OriginCaller, Balance>: Sized {
	/// The allowance given for the entity.
	fn allowance(&self) -> Allowance<Balance>;
	/// Whether the origin is restricted, and what entity it belongs to.
	fn restricted_entity(caller: &OriginCaller) -> Option<Self>;
}

/// Helper trait used by the benchmarks to construct state and inputs the bench needs
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper<OriginCaller, RuntimeCall> {
	/// Returns `(origin, call)` such that the entity derived from `origin` paired with
	/// `call` is in the excess set per [`Config::OperationAllowedOneTimeExcess`].
	///
	/// Used by the [`RestrictOrigin`] extension benchmark to drive `validate` into the
	/// `allowed_one_time_excess` closure on the success branch.
	fn excess_pair() -> (OriginCaller, RuntimeCall);
}

pub use pallet::*;
#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::{pallet_prelude::*, traits::ContainsPair};
	use frame_system::pallet_prelude::*;

	/// The usage of an entity.
	#[derive(Encode, Decode, Clone, Eq, PartialEq, Debug, TypeInfo, MaxEncodedLen)]
	pub struct Usage<Balance, BlockNumber> {
		/// The amount of usage consumed at block `at_block`.
		pub used: Balance,
		/// The block number at which the usage was last updated. (Provided by
		/// [`Config::BlockNumberProvider`]).
		pub at_block: BlockNumber,
	}

	pub(crate) type OriginCallerFor<T> =
		<<T as frame_system::Config>::RuntimeOrigin as OriginTrait>::PalletsOrigin;
	pub(crate) type BalanceOf<T> =
		<<T as pallet_transaction_payment::Config>::OnChargeTransaction as OnChargeTransaction<
			T,
		>>::Balance;
	pub(crate) type ProviderBlockNumberFor<T> =
		<<T as Config>::BlockNumberProvider as BlockNumberProvider>::BlockNumber;

	/// PASEO-LOCAL DEVIATION from individuality v0.3.1.
	///
	/// Upstream ships this pallet with no storage version, because upstream's `next-*` runtimes
	/// are launched from genesis presets and never carry a `Usages` entry stamped on the old
	/// clock. `people-paseo` does. Declaring a version is what lets
	/// [`migration::MigrateV0ToV1`] be a `VersionedMigration` and therefore self-guarding — that
	/// migration is destructive on a second run (it would discard accrued recovery) and must
	/// never run twice.
	///
	/// Keep this at `1` until a further storage change lands; bump it in lockstep with a new
	/// `VersionedMigration` in `migration.rs`.
	pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

	#[pallet::pallet]
	#[pallet::storage_version(STORAGE_VERSION)]
	pub struct Pallet<T>(_);

	/// The current usage for each entity.
	#[pallet::storage]
	pub type Usages<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		T::RestrictedEntity,
		Usage<BalanceOf<T>, ProviderBlockNumberFor<T>>,
	>;

	#[pallet::config]
	pub trait Config:
		frame_system::Config<
			RuntimeCall: Dispatchable<Info = DispatchInfo, PostInfo = PostDispatchInfo>,
			RuntimeOrigin: AsTransactionAuthorizedOrigin,
		> + pallet_transaction_payment::Config
		+ Send
		+ Sync
	{
		/// The weight information for this pallet.
		type WeightInfo: WeightInfo;

		/// The block number used to measure the usage recovery.
		///
		/// A parachain must set the relay chain block number provider, so that the recovery rate
		/// stays the same when the local block production changes.
		type BlockNumberProvider: BlockNumberProvider;

		/// The type that represent the entities tracked, its allowance and the conversion from
		/// origin is bounded in [`RestrictedEntity`].
		///
		/// This is the canonical origin from the point of view of usage tracking.
		/// Each entity is tracked separately.
		///
		/// This is different from origin as a multiple origin can represent a single entity.
		/// For example, imagine a DAO origin with a percentage of voters, we want to track the DAO
		/// entity regardless of the voter percentage.
		type RestrictedEntity: RestrictedEntity<OriginCallerFor<Self>, BalanceOf<Self>>
			+ Parameter
			+ MaxEncodedLen;

		/// For some entities, the calls that are allowed to go beyond the max allowance.
		///
		/// This must be only for call which have a reasonable maximum weight and length.
		type OperationAllowedOneTimeExcess: ContainsPair<Self::RestrictedEntity, Self::RuntimeCall>;

		/// Benchmarking helper.
		#[cfg(feature = "runtime-benchmarks")]
		type BenchmarkHelper: BenchmarkHelper<OriginCallerFor<Self>, Self::RuntimeCall>;
	}

	#[pallet::error]
	pub enum Error<T> {
		/// The origin has no usage tracked.
		NoUsage,
		/// The usage is not zero.
		NotZero,
	}

	#[pallet::event]
	#[pallet::generate_deposit(fn deposit_event)]
	pub enum Event<T: Config> {
		/// Usage for an entity is cleaned.
		UsageCleaned { entity: T::RestrictedEntity },
	}

	#[pallet::call(weight = <T as Config>::WeightInfo)]
	impl<T: Config> Pallet<T> {
		/// Allow to clean usage associated with an entity when it is zero or when there is no
		/// longer any allowance for the origin.
		// This could be an unsigned call
		#[pallet::call_index(1)]
		pub fn clean_usage(
			origin: OriginFor<T>,
			entity: T::RestrictedEntity,
		) -> DispatchResultWithPostInfo {
			// `None` origin is better to reject in general, due to being used for inherents and
			// validate unsigned.
			if ensure_none(origin.clone()).is_ok() {
				return Err(BadOrigin.into())
			}

			let Some(mut usage) = Usages::<T>::take(&entity) else {
				return Err(Error::<T>::NoUsage.into())
			};

			let now = T::BlockNumberProvider::current_block_number();
			let elapsed = now.saturating_sub(usage.at_block).saturated_into::<u32>();

			let allowance = entity.allowance();
			let receive_back = allowance.recovery_per_block.saturating_mul(elapsed.into());
			usage.used = usage.used.saturating_sub(receive_back);

			ensure!(usage.used.is_zero(), Error::<T>::NotZero);

			Self::deposit_event(Event::UsageCleaned { entity });

			Ok(Pays::No.into())
		}
	}
}

fn extrinsic_fee<T: Config>(weight: Weight, length: usize) -> BalanceOf<T> {
	let weight_fee = T::WeightToFee::weight_to_fee(&weight);
	let length_fee = T::LengthToFee::weight_to_fee(&Weight::from_parts(length as u64, 0));
	weight_fee.saturating_add(length_fee)
}

/// This transaction extension restricts some origins and prevents them from dispatching calls,
/// based on their usage and allowance.
///
/// The extension can be enabled or disabled with the inner boolean. When enabled, the restriction
/// process executes. When disabled, only the `RestrictedOrigins` check is executed.
/// You can always enable it, the only advantage of disabling it is have better pre-dispatch weight.
#[derive(Encode, Decode, Clone, Eq, PartialEq, TypeInfo, DebugNoBound, DecodeWithMemTracking)]
#[scale_info(skip_type_params(T))]
pub struct RestrictOrigin<T>(bool, core::marker::PhantomData<T>);

impl<T> RestrictOrigin<T> {
	/// Instantiates a new `RestrictOrigins` extension.
	pub fn new(enable: bool) -> Self {
		Self(enable, core::marker::PhantomData)
	}
}

/// The info passed between the validate and prepare steps for the `RestrictOrigins` extension.
#[derive(DebugNoBound)]
pub enum Val<T: Config> {
	Charge {
		fee: BalanceOf<T>,
		entity: T::RestrictedEntity,
		/// The updated usage to persist in `prepare`.
		usage: Usage<BalanceOf<T>, ProviderBlockNumberFor<T>>,
	},
	NoCharge,
}

/// The info passed between the prepare and post-dispatch steps for the `RestrictOrigins`
/// extension.
pub enum Pre<T: Config> {
	Charge {
		fee: BalanceOf<T>,
		entity: T::RestrictedEntity,
	},
	NoCharge {
		// weight initially estimated by the extension, to be refunded
		refund: Weight,
	},
}

impl<T: Config> TransactionExtension<T::RuntimeCall> for RestrictOrigin<T> {
	const IDENTIFIER: &'static str = "RestrictOrigins";
	type Implicit = ();
	type Val = Val<T>;
	type Pre = Pre<T>;

	fn weight(&self, _call: &T::RuntimeCall) -> frame_support::weights::Weight {
		if !self.0 {
			return Weight::zero()
		}

		<T as Config>::WeightInfo::restrict_origin_tx_ext()
	}

	fn validate(
		&self,
		origin: DispatchOriginOf<T::RuntimeCall>,
		call: &T::RuntimeCall,
		info: &DispatchInfoOf<T::RuntimeCall>,
		len: usize,
		_self_implicit: (),
		_inherited_implication: &impl Implication,
		_source: TransactionSource,
	) -> ValidateResult<Self::Val, T::RuntimeCall> {
		let origin_caller = origin.caller();
		let Some(entity) = T::RestrictedEntity::restricted_entity(origin_caller) else {
			return Ok((ValidTransaction::default(), Val::NoCharge, origin));
		};
		let allowance = T::RestrictedEntity::allowance(&entity);

		if !self.0 {
			// Extension is disabled, but the restriction must happen, the extension should have
			// been enabled.
			return Err(InvalidTransaction::Call.into())
		}

		let now = T::BlockNumberProvider::current_block_number();
		let mut usage = match Usages::<T>::get(&entity) {
			Some(mut usage) => {
				let elapsed = now.saturating_sub(usage.at_block).saturated_into::<u32>();
				let receive_back = allowance.recovery_per_block.saturating_mul(elapsed.into());
				usage.used = usage.used.saturating_sub(receive_back);
				usage.at_block = now;
				usage
			},
			None => Usage { used: 0u32.into(), at_block: now },
		};

		// The usage before taking into account this extrinsic.
		let usage_without_new_xt = usage.used;
		let fee = extrinsic_fee::<T>(info.total_weight(), len);
		usage.used = usage.used.saturating_add(fee);

		let allowed_one_time_excess = || {
			usage_without_new_xt == 0u32.into() &&
				T::OperationAllowedOneTimeExcess::contains(&entity, call)
		};
		if usage.used <= allowance.max || allowed_one_time_excess() {
			Ok((ValidTransaction::default(), Val::Charge { fee, entity, usage }, origin))
		} else {
			Err(InvalidTransaction::Payment.into())
		}
	}

	fn prepare(
		self,
		val: Self::Val,
		_origin: &DispatchOriginOf<T::RuntimeCall>,
		call: &T::RuntimeCall,
		_info: &DispatchInfoOf<T::RuntimeCall>,
		_len: usize,
	) -> Result<Self::Pre, TransactionValidityError> {
		match val {
			Val::Charge { fee, entity, usage } => {
				// The value `usage` was calculated in `validate` and is blindly set to the entity
				// usage here in `prepare`. No other transaction extension, and no other logic
				// should write into `Usages` for this entity in between `validate` and
				// `prepare`, otherwise the change would be simply ignored and overwritten.
				Usages::<T>::insert(&entity, &usage);
				Ok(Pre::Charge { fee, entity })
			},
			Val::NoCharge => Ok(Pre::NoCharge { refund: self.weight(call) }),
		}
	}

	fn post_dispatch_details(
		pre: Self::Pre,
		_info: &DispatchInfoOf<T::RuntimeCall>,
		post_info: &PostDispatchInfoOf<T::RuntimeCall>,
		len: usize,
		_result: &DispatchResult,
	) -> Result<Weight, TransactionValidityError> {
		match pre {
			Pre::Charge { fee, entity } => {
				Usages::<T>::mutate_exists(entity, |maybe_usage| {
					if let Some(usage) = maybe_usage {
						if post_info.pays_fee == Pays::No {
							usage.used = usage.used.saturating_sub(fee);
						} else if let Some(actual_weight) = post_info.actual_weight {
							let actual_fee = extrinsic_fee::<T>(actual_weight, len);
							usage.used = usage.used.saturating_sub(fee.saturating_sub(actual_fee));
						}

						if usage.used.is_zero() {
							*maybe_usage = None;
						}
					}
				});
				Ok(Weight::zero())
			},
			Pre::NoCharge { refund } => Ok(refund),
		}
	}
}
