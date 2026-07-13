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

//! # Alias Accounts Pallet
//!
//! Manages alias authentication for the People and People Lite ring membership collections.
//! No other collections are accepted. Provides:
//!
//! - Bidirectional alias-to-account mappings (set up via `set_paid_alias_account`, paid in PGAS)
//! - A transaction extension that promotes signed alias-bound accounts into `Origin::RingAlias` for
//!   downstream pallets
//! - Verifies ring proofs through [`Config::MemberService`]
//!
//! ## Extension Variant
//!
//! - `WithAccount(nonce)`: signed call by an alias-bound account; the extension swaps the origin to
//!   `Origin::RingAlias` so downstream pallets can consume the alias identity.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod extension;
pub mod origin;
pub mod types;
pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub use extension::{AsRingAlias, AsRingAliasInfo, CustomValidity};
pub use indiv_support::traits::{PEOPLE_IDENTIFIER, PEOPLE_LITE_IDENTIFIER};
pub use pallet::*;
pub use types::*;
pub use weights::WeightInfo;

#[frame_support::pallet]
pub mod pallet {

	use super::*;
	use frame_support::{
		pallet_prelude::*,
		traits::{
			fungibles::{self, Mutate as _},
			tokens::{Fortitude, Precision, Preservation},
			EnsureOrigin, IsSubType, OriginTrait, UnixTime,
		},
	};
	use frame_system::pallet_prelude::*;
	use indiv_support::traits::{
		Context, Identifier, MembershipProver, PersonhoodLookup, RingExponent,
	};
	use verifiable::GenerateVerifiable;

	/// Asset id type used by the pallet's `Fungibles` backend.
	pub type AssetIdOf<T> = <<T as Config>::Fungibles as fungibles::Inspect<
		<T as frame_system::Config>::AccountId,
	>>::AssetId;
	/// Balance type used by the pallet's `Fungibles` backend.
	pub type BalanceOf<T> = <<T as Config>::Fungibles as fungibles::Inspect<
		<T as frame_system::Config>::AccountId,
	>>::Balance;

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config:
		frame_system::Config<
		RuntimeOrigin: From<Origin>
		                   + OriginTrait<
			PalletsOrigin: From<Origin>
			                   + TryInto<
				Origin,
				Error = <Self::RuntimeOrigin as OriginTrait>::PalletsOrigin,
			>,
		>,
		RuntimeCall: Parameter + IsSubType<Call<Self>>,
	>
	{
		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;

		/// Service used to verify membership proofs against ring collections, query ring
		/// revisions and look up revision source timestamps.
		type MemberService: MembershipProver<
			Crypto: GenerateVerifiable<Config: TryFrom<RingExponent>>,
		>;

		/// Time provider used for proof-validity windows and grace-period checks.
		type UnixTime: UnixTime;

		/// Time tolerance (in seconds) for proof-based transactions.
		/// Proofs are valid from `proof_valid_at` to `proof_valid_at + ProofValidityWindow`.
		/// Must be greater than zero.
		#[pallet::constant]
		type ProofValidityWindow: Get<u64>;

		/// Grace period (in seconds) before a stale alias mapping can be cleaned up.
		/// After a ring revision changes, alias holders have this long to update
		/// via `reprove_alias_account` before anyone can call `clean_up_stale_alias`.
		/// Must be greater than zero.
		#[pallet::constant]
		type CleanupGracePeriod: Get<u64>;

		/// Ring exponent for People Lite.
		#[pallet::constant]
		type PeopleLiteRingExponent: Get<RingExponent>;

		/// Ring exponent for People.
		#[pallet::constant]
		type PeopleRingExponent: Get<RingExponent>;

		/// The fungibles implementation used to charge the PGAS fee.
		type Fungibles: fungibles::Mutate<Self::AccountId> + fungibles::Inspect<Self::AccountId>;

		/// The asset id of the PGAS token used to pay for alias registrations.
		type PgasAssetId: Get<AssetIdOf<Self>>;

		/// Origin allowed to set the PGAS fee charged on alias registrations.
		type FeeManagerOrigin: EnsureOrigin<Self::RuntimeOrigin>;
	}

	// ========== Storage Items ==========

	/// Maps (collection, contextual_alias) to account.
	/// Uses `DoubleMap` because the same alias value could exist in different collections.
	#[pallet::storage]
	pub type AliasToAccount<T: Config> = StorageDoubleMap<
		_,
		Blake2_128Concat,
		Identifier,
		Blake2_128Concat,
		ContextualAlias,
		T::AccountId,
		OptionQuery,
	>;

	/// Maps account to full alias info (collection + revised contextual alias).
	#[pallet::storage]
	pub type AccountToAlias<T: Config> =
		StorageMap<_, Blake2_128Concat, T::AccountId, AliasAccountInfo, OptionQuery>;

	/// PGAS fee burned on each alias registration / account swap.
	///
	/// Set by [`Config::FeeManagerOrigin`] via [`Pallet::set_alias_fee`].
	/// While unset, alias registrations fail with [`Error::AliasFeeUnset`].
	#[pallet::storage]
	pub type AliasFee<T: Config> = StorageValue<_, BalanceOf<T>, OptionQuery>;

	// ========== Events ==========

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// An alias account has been set or updated.
		AliasAccountSet {
			/// The account that was linked.
			account: T::AccountId,
			/// The collection identifier.
			collection: Identifier,
			/// The contextual alias.
			alias: Alias,
		},
		/// An alias account has been removed.
		AliasAccountUnset {
			/// The account that was unlinked.
			account: T::AccountId,
		},
		/// A stale alias mapping was cleaned up.
		StaleAliasCleanedUp {
			/// The account that was unlinked.
			account: T::AccountId,
			/// The collection identifier.
			collection: Identifier,
			/// The contextual alias.
			alias: Alias,
		},
		/// The PGAS fee for alias registrations was changed.
		AliasFeeSet { fee: BalanceOf<T> },
	}

	// ========== Errors ==========

	#[pallet::error]
	pub enum Error<T> {
		/// The collection is not accepted.
		InvalidCollection,
		/// The account is already in use under another alias.
		AccountInUse,
		/// The account is not known.
		InvalidAccount,
		/// Alias <-> Account is already set and up to date.
		AliasAccountAlreadySet,
		/// Call is too late or too early.
		TimeOutOfRange,
		/// The alias mapping is not stale; the revision still matches the current ring root.
		AliasNotStale,
		/// The cleanup grace period has not elapsed yet; the alias holder still has time to
		/// update.
		CleanupTooEarly,
		/// Storage is in an inconsistent state. This should never happen under normal operation.
		InconsistentState,
		/// The PGAS fee for alias registrations has not been set yet.
		AliasFeeUnset,
		/// The proof passed to `reprove_alias_account` produced an alias / context
		/// that does not match the stored mapping for the signer.
		ReproveMismatch,
		/// No ring root was found for the requested collection / ring index.
		RingRootNotFound,
		/// The requested ring revision is no longer accepted for this ring.
		StaleRevision,
		/// Ring-VRF proof verification failed.
		BadProof,
		/// The configured ring capacity is invalid for this collection.
		InvalidRingCapacity,
	}

	// ========== Origin ==========

	/// Custom origin for ring alias authentication.
	///
	/// Set by the transaction extension after verifying a ring membership proof
	/// or looking up an existing alias-to-account mapping.
	#[pallet::origin]
	#[derive(
		Clone,
		PartialEq,
		Eq,
		Debug,
		codec::Encode,
		codec::Decode,
		codec::MaxEncodedLen,
		scale_info::TypeInfo,
		codec::DecodeWithMemTracking,
	)]
	pub enum Origin {
		/// Origin authenticated via ring alias.
		RingAlias(AliasAccountInfo),
	}

	// ========== Hooks ==========

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn integrity_test() {
			assert!(
				T::ProofValidityWindow::get() > 0,
				"ProofValidityWindow must be greater than zero"
			);
			assert!(
				T::CleanupGracePeriod::get() > 0,
				"CleanupGracePeriod must be greater than zero"
			);
		}
	}

	// ========== Extrinsics ==========

	#[pallet::call(weight = <T as Config>::WeightInfo)]
	impl<T: Config> Pallet<T> {
		/// Remove the alias mapping for the signer.
		///
		/// The origin must be signed by the account currently bound to a ring alias. The
		/// mapping is removed in both directions.
		#[pallet::call_index(1)]
		#[pallet::weight(<T as Config>::WeightInfo::unset_alias_account())]
		pub fn unset_alias_account(origin: OriginFor<T>) -> DispatchResult {
			let who = ensure_signed(origin)?;
			let alias_info = AccountToAlias::<T>::take(&who).ok_or(Error::<T>::InvalidAccount)?;
			AliasToAccount::<T>::remove(alias_info.collection, &alias_info.ca);
			frame_system::Pallet::<T>::dec_sufficients(&who);

			Self::deposit_event(Event::AliasAccountUnset { account: who });

			Ok(())
		}

		/// Link an account to a ring alias, on payment of a PGAS fee.
		///
		/// The origin must be signed; the signer becomes the bound account. The PGAS fee
		/// ([`AliasFee`]) is burned from the signer's PGAS balance. The alias must verify
		/// against the supplied `collection`/`ring_index`/`ring_revision` in `context`. The
		/// collection must still be People or People Lite.
		///
		/// If a different account is already bound to the same alias under this context, that
		/// mapping is dropped and the swap is paid for by the new signer.
		///
		/// Re-proving the same alias under a fresher ring revision should instead use
		/// [`Pallet::reprove_alias_account`], which is free.
		#[pallet::call_index(3)]
		#[pallet::weight(<T as Config>::WeightInfo::set_alias_account())]
		pub fn set_alias_account(
			origin: OriginFor<T>,
			proof: ProofOf<T>,
			collection: Identifier,
			ring_index: RingIndex,
			ring_revision: RevisionIndex,
			context: Context,
			proof_valid_at: u64,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;

			let fee = AliasFee::<T>::get().ok_or(Error::<T>::AliasFeeUnset)?;

			let now = T::UnixTime::now().as_secs();
			let window = T::ProofValidityWindow::get();
			ensure!(
				proof_valid_at <= now && now <= proof_valid_at.saturating_add(window),
				Error::<T>::TimeOutOfRange,
			);

			let validated_rca = Self::verify_proof(
				&proof,
				&collection,
				ring_index,
				ring_revision,
				&context,
				&Self::proof_message(&who, proof_valid_at),
			)?;

			let alias_info = AliasAccountInfo::from_validated(collection, &validated_rca);
			let old_account = AliasToAccount::<T>::get(collection, &alias_info.ca);
			let is_new_account = old_account.as_ref() != Some(&who);

			// Replay/no-op: same alias, same account, same revision/ring already stored.
			ensure!(
				is_new_account || AccountToAlias::<T>::get(&who).as_ref() != Some(&alias_info),
				Error::<T>::AliasAccountAlreadySet
			);

			// One-account-one-alias: signer must not already hold a different alias.
			if is_new_account {
				ensure!(!AccountToAlias::<T>::contains_key(&who), Error::<T>::AccountInUse);
			}

			// Burn the PGAS fee up-front; if this fails the call aborts before any
			// storage mutation.
			T::Fungibles::burn_from(
				T::PgasAssetId::get(),
				&who,
				fee,
				Preservation::Expendable,
				Precision::Exact,
				Fortitude::Polite,
			)?;

			if is_new_account {
				if let Some(old) = &old_account {
					frame_system::Pallet::<T>::dec_sufficients(old);
					AccountToAlias::<T>::remove(old);
				}
				frame_system::Pallet::<T>::inc_sufficients(&who);
			}

			AccountToAlias::<T>::insert(&who, &alias_info);
			AliasToAccount::<T>::insert(collection, &alias_info.ca, &who);

			Self::deposit_event(Event::AliasAccountSet {
				account: who,
				collection,
				alias: alias_info.ca.alias,
			});

			Ok(())
		}

		/// Re-prove the holder's alias against a fresher ring revision, free of charge in case of a
		/// successful operations.
		///
		/// The origin must be signed and must already hold a alias mapping. The
		/// proof is verified against the stored context; the resulting alias must
		/// match the stored alias. Only the `revision` and `ring` fields of the
		/// stored mapping change; collection, context, and alias are preserved.
		#[pallet::call_index(4)]
		#[pallet::weight(<T as Config>::WeightInfo::reprove_alias_account())]
		pub fn reprove_alias_account(
			origin: OriginFor<T>,
			proof: ProofOf<T>,
			ring_index: RingIndex,
			ring_revision: RevisionIndex,
			proof_valid_at: u64,
		) -> DispatchResultWithPostInfo {
			let who = ensure_signed(origin)?;

			let now = T::UnixTime::now().as_secs();
			let window = T::ProofValidityWindow::get();
			ensure!(
				proof_valid_at <= now && now <= proof_valid_at.saturating_add(window),
				Error::<T>::TimeOutOfRange,
			);

			let old_info = AccountToAlias::<T>::get(&who).ok_or(Error::<T>::InvalidAccount)?;

			let validated_rca = Self::verify_proof(
				&proof,
				&old_info.collection,
				ring_index,
				ring_revision,
				&old_info.ca.context,
				&Self::proof_message(&who, proof_valid_at),
			)?;

			ensure!(validated_rca.ca.alias == old_info.ca.alias, Error::<T>::ReproveMismatch);

			// On the same ring, the new revision must be strictly greater than the
			// stored one — re-prove must move the mapping forward, never sideways
			// or backwards. A different ring index (e.g. after a ring merge) is
			// always accepted: ring revisions are independent across rings, so
			// there's no meaningful ordering to enforce.
			ensure!(
				validated_rca.ring != old_info.ring || validated_rca.revision > old_info.revision,
				Error::<T>::AliasAccountAlreadySet
			);

			let new_info = AliasAccountInfo {
				collection: old_info.collection,
				revision: validated_rca.revision,
				ring: validated_rca.ring,
				ca: old_info.ca.clone(),
			};
			AccountToAlias::<T>::insert(&who, &new_info);

			Self::deposit_event(Event::AliasAccountSet {
				account: who,
				collection: new_info.collection,
				alias: new_info.ca.alias,
			});

			Ok(Pays::No.into())
		}

		/// Set the PGAS fee charged by [`Pallet::set_alias_account`].
		///
		/// Origin must be [`Config::FeeManagerOrigin`]. The new fee replaces the
		/// previous value (if any). There is no minimum — set to zero to
		/// effectively disable the burn while keeping the path open.
		#[pallet::call_index(5)]
		#[pallet::weight(<T as Config>::WeightInfo::set_alias_fee())]
		pub fn set_alias_fee(origin: OriginFor<T>, fee: BalanceOf<T>) -> DispatchResult {
			T::FeeManagerOrigin::ensure_origin(origin)?;
			AliasFee::<T>::put(fee);
			Self::deposit_event(Event::AliasFeeSet { fee });
			Ok(())
		}

		/// Remove a stale alias <-> account mapping.
		///
		/// Anyone can call this. A mapping is stale when:
		/// 1. The ring revision no longer matches the account alias (i.e., ring was rebuilt).
		/// 2. The account alias refers to a deleted ring.
		///
		/// For (1), the mapping is only considered stale after a grace period has elapsed.
		///
		/// The transaction fee is refunded on success to incentivize cleanup.
		#[pallet::call_index(2)]
		#[pallet::weight(<T as Config>::WeightInfo::clean_up_stale_alias())]
		pub fn clean_up_stale_alias(
			origin: OriginFor<T>,
			collection: Identifier,
			ca: ContextualAlias,
		) -> DispatchResultWithPostInfo {
			ensure_signed(origin)?;

			let account =
				AliasToAccount::<T>::get(collection, &ca).ok_or(Error::<T>::InvalidAccount)?;
			let alias_info =
				AccountToAlias::<T>::get(&account).ok_or(Error::<T>::InvalidAccount)?;

			ensure!(alias_info.collection == collection, Error::<T>::InconsistentState);
			Self::ensure_alias_is_stale(&alias_info)?;

			AliasToAccount::<T>::remove(collection, &ca);
			AccountToAlias::<T>::remove(&account);
			frame_system::Pallet::<T>::dec_sufficients(&account);

			Self::deposit_event(Event::StaleAliasCleanedUp {
				account,
				collection,
				alias: ca.alias,
			});

			Ok(Pays::No.into())
		}
	}

	// ========== Extra Constants ==========

	#[pallet::extra_constants]
	impl<T: Config> Pallet<T> {
		#[pallet::constant_name(PeopleCollectionIdentifier)]
		fn people_collection() -> Identifier {
			*PEOPLE_IDENTIFIER
		}

		#[pallet::constant_name(PeopleLiteCollectionIdentifier)]
		fn people_lite_collection() -> Identifier {
			*PEOPLE_LITE_IDENTIFIER
		}
	}

	// ========== Personhood Lookup ==========

	impl<T: Config> PersonhoodLookup<T::AccountId, ProofOf<T>> for Pallet<T> {
		fn personhood_info_weight() -> Weight {
			<T as Config>::WeightInfo::personhood_info()
		}

		fn personhood_info(
			account: &T::AccountId,
			context: &Context,
		) -> (Option<(Identifier, Alias)>, Weight) {
			let max = Self::personhood_info_weight();

			let saved_one_read = T::DbWeight::get().reads(1);
			let Some(info) = AccountToAlias::<T>::get(account) else {
				return (None, max.saturating_sub(saved_one_read));
			};
			if info.ca.context != *context {
				return (None, max.saturating_sub(saved_one_read));
			}
			if !Self::is_revision_in_grace(&info.collection, info.ring, info.revision) {
				return (None, max);
			}
			(Some((info.collection, info.ca.alias)), max)
		}

		fn personhood_info_by_proof_weight() -> Weight {
			<T as Config>::WeightInfo::personhood_info_by_proof()
		}

		/// Verifies the proof in `request` against the collection it specifies.
		/// Returns `false` when the ring/revision is unknown, the proof is invalid,
		/// or the derived alias differs from the claimed one.
		fn personhood_info_by_proof(
			request: indiv_support::traits::PersonhoodProofRequest<'_, ProofOf<T>>,
		) -> (bool, Weight) {
			let max = Self::personhood_info_by_proof_weight();

			let res = T::MemberService::verify_membership_at_rev(
				&request.identifier,
				&request.proof,
				request.ring_index,
				request.revision,
				request.context,
				request.message,
			);
			let matched = matches!(res, Ok(ca) if ca.alias == request.alias);
			(matched, max)
		}
	}

	// ========== Helper Functions ==========

	impl<T: Config> Pallet<T> {
		/// Extract `AliasAccountInfo` from the custom origin.
		pub fn ensure_ring_alias(
			origin: T::RuntimeOrigin,
		) -> Result<AliasAccountInfo, DispatchError> {
			match origin.into_caller().try_into() {
				Ok(Origin::RingAlias(info)) => Ok(info),
				_ => Err(DispatchError::BadOrigin),
			}
		}

		/// Verify that an alias mapping is stale and eligible for cleanup.
		///
		/// An alias is stale when any of the following is true:
		/// - Its ring revision is outdated and the grace period has elapsed.
		/// - Its ring root has been deleted.
		fn ensure_alias_is_stale(alias_info: &AliasAccountInfo) -> DispatchResult {
			// If the revision is still valid under the alias-accounts grace policy, the alias is
			// not stale.
			if Self::is_revision_in_grace(
				&alias_info.collection,
				alias_info.ring,
				alias_info.revision,
			) {
				return Err(Error::<T>::AliasNotStale.into());
			}

			// Ring deleted — immediately stale.
			let Some(latest_revision) =
				T::MemberService::ring_revision(&alias_info.collection, alias_info.ring)
			else {
				return Ok(());
			};

			// Revision invalid — enforcing cleanup cooldown against the latest root's source time.
			let newest_source_time = T::MemberService::revision_source_time(
				&alias_info.collection,
				alias_info.ring,
				latest_revision,
			)
			.unwrap_or(0);
			let now = T::UnixTime::now().as_secs();
			let grace = T::CleanupGracePeriod::get();
			ensure!(now >= newest_source_time.saturating_add(grace), Error::<T>::CleanupTooEarly);

			Ok(())
		}

		pub fn ensure_valid_collection(
			collection: &Identifier,
		) -> Result<RingExponent, DispatchError> {
			if collection == PEOPLE_LITE_IDENTIFIER {
				Ok(T::PeopleLiteRingExponent::get())
			} else if collection == PEOPLE_IDENTIFIER {
				Ok(T::PeopleRingExponent::get())
			} else {
				Err(Error::<T>::InvalidCollection.into())
			}
		}

		/// Message bound into an alias proof to prevent a stolen proof from being
		/// reused by a different signer and a expiry.
		pub fn proof_message(account: &T::AccountId, proof_valid_at: u64) -> [u8; 32] {
			(b"alias-accounts", account, proof_valid_at).using_encoded(sp_io::hashing::blake2_256)
		}

		/// Verifies a ring-VRF proof for the given collection / ring / revision and returns
		/// the resulting revised contextual alias.
		pub fn verify_proof(
			proof: &ProofOf<T>,
			collection: &Identifier,
			ring_index: RingIndex,
			revision: RevisionIndex,
			context: &Context,
			msg: &[u8],
		) -> Result<RevisedContextualAlias, DispatchError> {
			Self::ensure_valid_collection(collection)?;

			ensure!(
				Self::is_revision_in_grace(collection, ring_index, revision),
				Error::<T>::StaleRevision
			);

			let ca = T::MemberService::verify_membership_at_rev(
				collection, proof, ring_index, revision, *context, msg,
			)
			.map_err(|_| Error::<T>::BadProof)?;

			Ok(RevisedContextualAlias { revision, ring: ring_index, ca })
		}

		/// Determines whether `revision` is currently considered valid for the given ring under
		/// alias-accounts' grace policy: the revision must be retained by [`Config::MemberService`]
		/// and, when it is no longer the latest, must still be within
		/// [`Config::CleanupGracePeriod`] since it was committed.
		pub(crate) fn is_revision_in_grace(
			identifier: &Identifier,
			ring_index: RingIndex,
			revision: RevisionIndex,
		) -> bool {
			if !T::MemberService::is_revision_valid(identifier, ring_index, revision) {
				return false;
			}
			if T::MemberService::ring_revision(identifier, ring_index) == Some(revision) {
				return true;
			}
			let Some(source_time) =
				T::MemberService::revision_source_time(identifier, ring_index, revision)
			else {
				return false;
			};
			let now = T::UnixTime::now().as_secs();
			now <= source_time.saturating_add(T::CleanupGracePeriod::get())
		}
	}
}
