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

//! # PGAS Pallet
//!
//! PGAS ("People Gas") is a sufficient, burnable fee asset. This pallet manages the asset minting
//! to recognized (lite) persons once per claim slot per day. The pallet verifies the (lite)
//! personhood of the minter through a generic `MembershipProver` interface, which makes it chain
//! agnostic.
//!
//! ## Claim flow
//!
//! A claim is submitted as general transaction (without any particular origin) carrying the
//! [`AsPgas`] transaction extension. The extension:
//!
//! 1. Reads the `slot_index` from [`Call::claim_pgas`].
//! 2. Builds the PGAS context for the current day (with a one-hour grace window covering the
//!    previous day's context around rollovers).
//! 3. Verifies the ring-VRF proof against the configured people / lite-people ring root via the
//!    [`MembershipProver`] trait, binding the proof to `blake2_256(encode(inherited_implication))`
//!    as the message.
//! 4. Mutates the origin to [`Origin::ClaimAlias`]; the dispatch then records the alias and mints
//!    [`Config::PgasClaimAmount`] into the `target` account.
//!
//! Per (day, alias) uniqueness is enforced authoritatively in dispatch and pre-checked in
//! validate for transaction pool hygiene. Records for elapsed days are pruned by a permissionless
//! authorized cleanup call submitted by an offchain worker.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
pub mod extension;
pub mod migration;
pub mod weights;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub use extension::{AsPgas, AsPgasInfo, CustomValidity, PgasCollection};
pub use pallet::*;
pub use weights::WeightInfo;

use frame_support::{
	dispatch::DispatchResultWithPostInfo,
	traits::{fungibles, IsSubType, OriginTrait, UnixTime},
};
use indiv_support::{
	traits::{Alias, Context, MembershipProver},
	utils::BigEndianU32,
};
use sp_runtime::SaturatedConversion;
use verifiable::GenerateVerifiable;

/// Ring-VRF proof type for [`Config::MembershipProver`].
pub type ProofOf<T> =
	<<<T as Config>::MembershipProver as MembershipProver>::Crypto as GenerateVerifiable>::Proof;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use fungibles::{Create as _, Inspect as _, Mutate as _};

	/// Prefix for PGAS claim contexts.
	pub const PGAS_CONTEXT_PREFIX: [u8; 8] = *b"pop:gas:";

	/// Day index type. Uses big-endian encoding so that `Identity`-hashed storage
	/// iteration yields days in ascending chronological order.
	pub type Day = BigEndianU32;
	/// Grace window (in seconds) around day boundaries during which the previous day's
	/// context is also accepted. This gives transactions time to propagate before a day
	/// rollover invalidates in-flight proofs.
	pub const PGAS_DAY_GRACE_WINDOW: u64 = 3600;

	/// Number of seconds in a day — divisor used to map a UNIX timestamp to a day index.
	pub const SECS_PER_DAY: u64 = 86400;

	const LOG_TARGET: &str = "runtime::indiv-pallet-pgas";

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config:
		frame_system::Config<
			RuntimeOrigin: From<Origin>
			                   + From<<Self::RuntimeOrigin as OriginTrait>::PalletsOrigin>
			                   + OriginTrait<
				PalletsOrigin: From<Origin>
				                   + TryInto<
					Origin,
					Error = <Self::RuntimeOrigin as OriginTrait>::PalletsOrigin,
				>,
			>,
			RuntimeCall: From<Call<Self>> + IsSubType<Call<Self>>,
		> + frame_system::offchain::CreateAuthorizedTransaction<Call<Self>>
		+ Send
		+ Sync
	{
		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;

		/// Source of ring-VRF proof verification against subscribed ring roots.
		///
		/// On Asset Hub this is typically `pallet-members-subscriber`, which tracks the
		/// people and lite-people ring roots received from People chain via XCM.
		type MembershipProver: MembershipProver<
			Crypto: GenerateVerifiable<
				Proof: Parameter + Send + Sync + DecodeWithMemTracking,
				Signature: Parameter + Send + Sync + DecodeWithMemTracking,
				Member: DecodeWithMemTracking,
				Config: TryFrom<indiv_support::traits::RingExponent>,
			>,
		>;

		/// The source of time.
		type Clock: UnixTime;

		/// The fungibles implementation for minting PGAS.
		type Fungibles: fungibles::Mutate<Self::AccountId>
			+ fungibles::Inspect<Self::AccountId>
			+ fungibles::Create<Self::AccountId>;

		/// The asset ID of the PGAS token.
		#[pallet::constant]
		type PgasAssetId: Get<
			<<Self as Config>::Fungibles as fungibles::Inspect<Self::AccountId>>::AssetId,
		>;

		/// The amount of PGAS minted per claim.
		#[pallet::constant]
		type PgasClaimAmount: Get<
			<<Self as Config>::Fungibles as fungibles::Inspect<Self::AccountId>>::Balance,
		>;

		/// Maximum number of PGAS claims a full person may perform per period (day).
		#[pallet::constant]
		type MaxClaimsPerPeriodPerPerson: Get<u32>;

		/// Maximum number of PGAS claims a lite person may perform per period (day).
		///
		/// Typically lower than [`MaxClaimsPerPeriodPerPerson`](Self::MaxClaimsPerPeriodPerPerson)
		/// since lite personhood offers weaker sybil resistance than full personhood.
		#[pallet::constant]
		type MaxClaimsPerPeriodPerLitePerson: Get<u32>;

		/// Maximum number of PGAS claim records that can be cleaned up in a single call.
		#[pallet::constant]
		type MaxPgasClaimRecordCleanupPerCall: Get<u32>;

		/// Admin account for the PGAS asset.
		///
		/// Should resolve to the sovereign account of this pallet's XCM location so the
		/// pallet itself owns the asset.
		type PgasAdmin: Get<Self::AccountId>;

		/// The minimum balance for the PGAS asset.
		type PgasMinBalance: Get<
			<<Self as Config>::Fungibles as fungibles::Inspect<Self::AccountId>>::Balance,
		>;

		/// Benchmark helper trait.
		#[cfg(feature = "runtime-benchmarks")]
		type BenchmarkHelper: benchmarking::BenchmarkHelper<Self>;
	}

	/// Local origin produced by the [`AsPgas`] transaction extension once a
	/// ring-VRF proof has been verified.
	#[pallet::origin]
	#[derive(
		Clone, PartialEq, Eq, Debug, Encode, Decode, MaxEncodedLen, TypeInfo, DecodeWithMemTracking,
	)]
	pub enum Origin {
		/// A verified claim-slot alias. `day` is the day the proof's context was built for
		/// (either the current day or the grace day).
		ClaimAlias { alias: Alias, day: Day, collection: PgasCollection },
	}

	/// Aliases that have been used to claim PGAS, keyed by (day, alias).
	/// `Day` uses big-endian encoding with `Identity` hashing so iteration yields days
	/// in ascending order, allowing the offchain worker to find the oldest stale day
	/// without scanning.
	#[pallet::storage]
	pub type ClaimedGasAliases<T: Config> =
		StorageDoubleMap<_, Identity, Day, Blake2_128Concat, Alias, (), OptionQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// PGAS was claimed by a person.
		PgasClaimed {
			alias: Alias,
			target: T::AccountId,
			amount: <<T as Config>::Fungibles as fungibles::Inspect<T::AccountId>>::Balance,
			collection: PgasCollection,
			day: u32,
		},
		/// The PGAS asset was created.
		PgasAssetCreated,
		/// Old PGAS claim records were cleaned up.
		PgasClaimRecordsCleaned { day_index: u32, count: u32 },
	}

	#[pallet::error]
	pub enum Error<T> {
		/// This alias has already been used to claim PGAS in this period.
		AlreadyClaimed,
		/// The PGAS asset does not exist or minting failed.
		PgasMintFailed,
		/// `clean_pgas_claim_records` was called with a day that has no stored records.
		NoRecordsForDay,
		/// The `first_alias` passed to `clean_pgas_claim_records` does not match the first alias
		/// currently stored under the prefix.
		FirstAliasMismatch,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn offchain_worker(_block_number: BlockNumberFor<T>) {
			use frame_system::offchain::SubmitTransaction;

			// Only clean days that are fully outside the grace window.
			let grace_day = Self::grace_day();

			// Iterate days in ascending order. First key in the iterator should be a key that
			// belongs to the oldest day.
			let Some((day, first_alias)) = ClaimedGasAliases::<T>::iter_keys().next() else {
				return;
			};
			let day_u32: u32 = day.into();
			if day_u32 >= grace_day {
				return;
			}
			let call = Call::clean_pgas_claim_records { day_index: day_u32, first_alias };
			let tx = T::create_authorized_transaction(call.into());
			match SubmitTransaction::<T, _>::submit_transaction(tx) {
				Ok(()) => log::debug!(
					target: LOG_TARGET,
					"pgas: submitted clean_pgas_claim_records for day {day_u32}"
				),
				Err(()) => log::warn!(
					target: LOG_TARGET,
					"pgas: failed to submit clean_pgas_claim_records for day {day_u32}"
				),
			}
		}

		fn integrity_test() {
			assert!(
				T::PgasClaimAmount::get() >= T::PgasMinBalance::get(),
				"`PgasClaimAmount` must be >= `PgasMinBalance`, otherwise the first claim to a \
				 fresh account would fail the asset's existential-deposit check",
			);
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Mint PGAS for a verified claim slot.
		///
		/// Must be submitted with the [`AsPgas`] transaction extension, which
		/// verifies the ring-VRF proof and produces an [`Origin::ClaimAlias`]. The outer origin
		/// must be `None` (the extension replaces it with the local origin); any other origin is
		/// rejected.
		///
		/// `slot_index` is part of the call payload so the extension can derive the claim context
		/// on-chain and so the proof binds to the requested slot via the inherited implication.
		#[pallet::call_index(0)]
		#[pallet::weight(<T as Config>::WeightInfo::claim_pgas())]
		pub fn claim_pgas(
			origin: OriginFor<T>,
			_slot_index: u32,
			target: T::AccountId,
		) -> DispatchResultWithPostInfo {
			let (alias, day, collection) = Self::ensure_claim_alias(origin)?;

			// Already checked in extension, but we will do the write anyway so checking here as
			// well.
			ensure!(!ClaimedGasAliases::<T>::contains_key(day, alias), Error::<T>::AlreadyClaimed);

			let amount = T::PgasClaimAmount::get();
			T::Fungibles::mint_into(T::PgasAssetId::get(), &target, amount)
				.map_err(|_| Error::<T>::PgasMintFailed)?;
			ClaimedGasAliases::<T>::insert(day, alias, ());

			Self::deposit_event(Event::PgasClaimed {
				alias,
				target,
				amount,
				collection,
				day: day.into(),
			});

			Ok(Pays::No.into())
		}

		/// Create the PGAS asset. This is a permissionless authorized call that can only succeed
		/// if the PGAS asset does not already exist.
		#[pallet::call_index(1)]
		#[pallet::authorize(|_source| Self::authorize_create_pgas_asset())]
		#[pallet::weight_of_authorize(<T as Config>::WeightInfo::authorize_create_pgas_asset())]
		#[pallet::weight(<T as Config>::WeightInfo::create_pgas_asset())]
		pub fn create_pgas_asset(origin: OriginFor<T>) -> DispatchResult {
			ensure_authorized(origin)?;
			Self::do_create_pgas_asset()
		}

		/// Remove old PGAS claim records for a specific `day_index`.
		///
		/// This is an authorized extrinsic submitted by the offchain worker.
		/// Only records from days that have fully elapsed (outside the grace window) can
		/// be cleaned. Up to [`Config::MaxPgasClaimRecordCleanupPerCall`] entries are
		/// removed per call.
		///
		/// `first_alias` is the first alias currently stored for `day_index` and is included in the
		/// tags for transaction uniqueness.
		#[pallet::call_index(2)]
		#[pallet::authorize(|source, day_index, first_alias| {
			Self::authorize_clean_pgas_claim_records(source, *day_index, *first_alias)
		})]
		#[pallet::weight_of_authorize(<T as Config>::WeightInfo::authorize_clean_pgas_claim_records())]
		#[pallet::weight(<T as Config>::WeightInfo::clean_pgas_claim_records(T::MaxPgasClaimRecordCleanupPerCall::get()))]
		pub fn clean_pgas_claim_records(
			origin: OriginFor<T>,
			day_index: u32,
			first_alias: Alias,
		) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;

			let day = Day::from(day_index);
			let actual_first = ClaimedGasAliases::<T>::iter_key_prefix(day)
				.next()
				.ok_or(Error::<T>::NoRecordsForDay)?;
			ensure!(actual_first == first_alias, Error::<T>::FirstAliasMismatch);

			let limit = T::MaxPgasClaimRecordCleanupPerCall::get();
			let result = ClaimedGasAliases::<T>::clear_prefix(day, limit, None);

			Self::deposit_event(Event::PgasClaimRecordsCleaned { day_index, count: result.unique });

			Ok(Some(<T as Config>::WeightInfo::clean_pgas_claim_records(result.unique)).into())
		}
	}

	impl<T: Config> Pallet<T> {
		/// Extract a verified [`Origin::ClaimAlias`] from a runtime origin.
		pub fn ensure_claim_alias(
			origin: OriginFor<T>,
		) -> Result<(Alias, Day, PgasCollection), sp_runtime::DispatchError> {
			match origin.into_caller().try_into() {
				Ok(Origin::ClaimAlias { alias, day, collection }) => Ok((alias, day, collection)),
				_ => Err(sp_runtime::DispatchError::BadOrigin),
			}
		}

		/// Create the PGAS asset and emit [`Event::PgasAssetCreated`].
		///
		/// Shared between the [`Call::create_pgas_asset`] extrinsic and the
		/// [`migration::CreatePgasAsset`] runtime upgrade.
		pub fn do_create_pgas_asset() -> DispatchResult {
			T::Fungibles::create(
				T::PgasAssetId::get(),
				T::PgasAdmin::get(),
				true, // sufficient
				T::PgasMinBalance::get(),
			)?;

			Self::deposit_event(Event::PgasAssetCreated);
			Ok(())
		}

		/// Authorize the creation of the PGAS asset.
		///
		/// Rejects with [`InvalidTransaction::Stale`] if the asset already exists.
		pub(crate) fn authorize_create_pgas_asset(
		) -> Result<(ValidTransaction, Weight), TransactionValidityError> {
			if T::Fungibles::asset_exists(T::PgasAssetId::get()) {
				return Err(InvalidTransaction::Stale.into());
			}
			ValidTransaction::with_tag_prefix("PgasAssetCreation")
				.and_provides(b"create_pgas")
				.propagate(true)
				.build()
				.map(|v| (v, Weight::zero()))
		}

		/// Authorize the cleanup of PGAS claim records for a given day.
		///
		/// Only local/in-block sources are accepted. Rejects if the day is still within
		/// the grace window, if there are no records to clean, or if `first_alias` does not
		/// match the first alias currently stored under the day's prefix.
		pub(crate) fn authorize_clean_pgas_claim_records(
			source: TransactionSource,
			day_index: u32,
			first_alias: Alias,
		) -> Result<(ValidTransaction, Weight), TransactionValidityError> {
			if !matches!(source, TransactionSource::InBlock | TransactionSource::Local) {
				return Err(InvalidTransaction::Call.into());
			}
			if day_index >= Self::grace_day() {
				return Err(InvalidTransaction::Future.into());
			}
			let day = Day::from(day_index);
			let actual_first = ClaimedGasAliases::<T>::iter_key_prefix(day)
				.next()
				.ok_or(InvalidTransaction::Stale)?;
			if actual_first != first_alias {
				return Err(CustomValidity::FirstAliasMismatch.into());
			}
			ValidTransaction::with_tag_prefix("pgas:clean-pgas-claims")
				.and_provides((day_index, first_alias))
				.propagate(false)
				.build()
				.map(|v| (v, <T as Config>::WeightInfo::authorize_clean_pgas_claim_records()))
		}

		/// Build the context bytes for a PGAS claim.
		///
		/// Format: [`PGAS_CONTEXT_PREFIX`] (8 bytes) + day (u32 LE, 4 bytes) + slot_index (u32
		/// LE, 4 bytes) + zeros (16 bytes).
		pub fn build_gas_context(day: u32, slot_index: u32) -> Context {
			let mut context = [0u8; 32];
			context[..8].copy_from_slice(&PGAS_CONTEXT_PREFIX);
			context[8..12].copy_from_slice(&day.to_le_bytes());
			context[12..16].copy_from_slice(&slot_index.to_le_bytes());
			context
		}

		/// The current day index derived from [`Config::Clock`].
		pub fn current_day() -> u32 {
			(T::Clock::now().as_secs() / SECS_PER_DAY).saturated_into()
		}

		/// The previous-day index accepted by the grace window (equals [`Self::current_day`]
		/// outside the grace window).
		pub fn grace_day() -> u32 {
			(T::Clock::now().as_secs().saturating_sub(PGAS_DAY_GRACE_WINDOW) / SECS_PER_DAY)
				.saturated_into()
		}

		/// Per-collection maximum slot index.
		pub fn max_claims_for(collection: PgasCollection) -> u32 {
			match collection {
				PgasCollection::People => T::MaxClaimsPerPeriodPerPerson::get(),
				PgasCollection::LitePeople => T::MaxClaimsPerPeriodPerLitePerson::get(),
			}
		}
	}
}
