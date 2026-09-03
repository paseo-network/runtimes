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
//! - Verifies ring proofs through [`Config::MemberService`]
//!
//! A call reads an account's alias through [`AccountToAlias`] or [`Config::MemberService`]. Nothing
//! here promotes a signed origin to an alias origin: an origin that authenticates as a person and
//! is not signed pays no fee, so the caller of such a call would have to be bounded separately.
//!
//! ## Retiring stale mappings
//!
//! A mapping goes stale when [`Config::MemberService`] stops accepting its revision, but a consumer
//! that reads [`AccountToAlias`] without checking the revision keeps resolving it, so removal waits
//! out [`Config::MappingRetention`]. Retiring one therefore runs in two phases:
//! [`Pallet::report_stale_aliases`] stamps [`StaleSince`], and [`Pallet::retire_stale_aliases`]
//! removes once the retention has run out. [`Pallet::clear_stale_alias_reports`] drops the stamp of
//! a mapping that verifies again.
//!
//! The offchain worker submits all three. They take no other origin. Each one carries a batch of
//! mappings, and `authorize` checks that every mapping in it is in the state that call applies to.
//!
//! The deadline could instead be derived on the spot from the ring's newest root, and for a rebuilt
//! ring the stamp is exactly that value. The stamp exists for the cases where that derivation has
//! nothing to work with: a deleted ring has no root left, so removal would be immediate, and a ring
//! deleted partway through the retention would collapse whatever wait remained. Recording the date
//! once fixes the deadline against later changes to the ring's history.
//!
//! A stamp is not permanent. Every path that rewrites [`AccountToAlias`] drops it, and so does a
//! sweep that finds the revision verifying again, which a collection torn down and re-created under
//! the same identifier can cause by restarting its revisions at zero. That last case is only
//! noticed when a sweep runs, so it bounds how far the stamp can be trusted rather than removing
//! the problem: a mapping that goes stale, resolves again and goes stale once more between two
//! sweeps still carries its first stamp.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod types;
pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub use indiv_support::traits::{PEOPLE_IDENTIFIER, PEOPLE_LITE_IDENTIFIER};
pub use pallet::*;
pub use types::*;
pub use weights::WeightInfo;

const LOG_TARGET: &str = "runtime::indiv-pallet-alias-accounts";

/// Blocks a sweep transaction stays valid in the pool.
///
/// A sweep is validated against the mappings it names, so one that waits longer than this is
/// better rebuilt by the next sweep than kept: the accounts it carries may have been retired by
/// the sweep that beat it into a block.
const STALE_ALIAS_TX_LONGEVITY: u64 = 64;

#[frame_support::pallet]
pub mod pallet {

	use super::*;
	use alloc::vec::Vec;
	use frame_support::{
		defensive,
		pallet_prelude::*,
		traits::{
			fungibles::{self, Mutate as _},
			tokens::{Fortitude, Precision, Preservation},
			UnixTime,
		},
	};
	use frame_system::{
		offchain::{CreateAuthorizedTransaction, SubmitTransaction},
		pallet_prelude::*,
	};
	use indiv_support::{
		traits::{Context, Identifier, MembershipProver, PersonhoodLookup, RingExponent},
		tx_priority,
		weight_budget::OcwWeightBudget,
	};
	use sp_runtime::traits::Zero;
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
	pub trait Config: frame_system::Config + CreateAuthorizedTransaction<Call<Self>> {
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

		/// How long (in seconds) a mapping is kept, counted from [`StaleSince`], after which
		/// [`Pallet::retire_stale_aliases`] may remove it. A rebuilt ring stamps the newest root's
		/// source time, which [`Config::MemberService`] also measures its own retention from, so
		/// the mapping outlives the revision only by the difference. Must exceed that retention.
		#[pallet::constant]
		type MappingRetention: Get<u64>;

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

		/// PGAS fee burned on each alias registration or account swap.
		/// `None` fails every registration with [`Error::AliasFeeUnset`].
		type AliasFee: Get<Option<BalanceOf<Self>>>;

		/// Blocks between the offchain worker's sweeps for stale mappings. Must be greater than
		/// zero.
		///
		/// One sweep reads every mapping, so this is what that scan costs a node. Retiring a
		/// mapping is deferrable by [`Config::MappingRetention`], which is measured in months, so
		/// hours between sweeps cost nothing in timeliness.
		type OffchainWorkerInterval: Get<BlockNumberFor<Self>>;

		/// The most mappings one sweep transaction carries. Must be greater than zero.
		///
		/// A sweep submits one transaction per action, so this bounds each of them. Every mapping
		/// in a batch is verified twice, once in `authorize` and once in the call, and the
		/// `integrity_test` holds the pair to the block.
		#[pallet::constant]
		type MaxStaleAliasBatch: Get<u32>;
	}

	/// Why a sweep transaction was rejected before dispatch.
	///
	/// Reported through [`InvalidTransaction::Custom`], so a node operator reading a rejected
	/// sweep sees which precondition failed rather than a bare `BadProof`.
	#[repr(u8)]
	pub enum AuthorizeInvalidity {
		/// The transaction did not come from this node, and a sweep is only ever submitted by an
		/// offchain worker.
		TransactionNotLocal = 200,
		/// The batch is empty, so the transaction would do nothing.
		EmptyBatch = 201,
		/// The batch is not in strictly ascending account order, so it may name an account twice.
		UnsortedBatch = 202,
		/// An account in the batch is not in the state the call retires it from.
		WrongStaleState = 203,
	}

	impl From<AuthorizeInvalidity> for TransactionValidityError {
		fn from(e: AuthorizeInvalidity) -> Self {
			InvalidTransaction::Custom(e as u8).into()
		}
	}

	/// Which sweep call applies to a mapping, given its revision and its [`StaleSince`] stamp.
	#[derive(PartialEq, Eq, Debug, Clone, Copy)]
	pub enum StaleAliasAction {
		/// The revision no longer verifies and the mapping carries no stamp, so a stamp starts
		/// [`Config::MappingRetention`].
		Report,
		/// The stamp is [`Config::MappingRetention`] old, so the mapping can be removed.
		Retire,
		/// The revision verifies again while the mapping still carries a stamp, so the stamp is
		/// void.
		ClearReport,
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

	/// When a mapping was first reported stale, which is what [`Config::MappingRetention`] is
	/// counted from.
	///
	/// The first [`Pallet::report_stale_aliases`] for a stale mapping stamps it and removes
	/// nothing; a later call does the removal. The stamp is the ring's newest root time where that
	/// is still known and the call's own time otherwise, so a deleted ring keeps the full
	/// retention. Every path that writes [`AccountToAlias`] clears it, as does a call that finds
	/// the revision verifying again, so a mapping known to be valid has no stamp.
	#[pallet::storage]
	pub type StaleSince<T: Config> =
		StorageMap<_, Blake2_128Concat, T::AccountId, u64, OptionQuery>;

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
		/// A stale alias mapping was removed.
		StaleAliasRemoved {
			/// The account that was unlinked.
			account: T::AccountId,
			/// The collection identifier.
			collection: Identifier,
			/// The contextual alias.
			alias: Alias,
		},
		/// A mapping was reported stale, which starts [`Config::MappingRetention`]. It is still
		/// stored, and a call from `removable_at` on removes it.
		StaleAliasReported {
			/// The account whose mapping was reported.
			account: T::AccountId,
			/// The collection identifier.
			collection: Identifier,
			/// The contextual alias.
			alias: Alias,
			/// The second from which the mapping can be removed.
			removable_at: u64,
		},
		/// A mapping reported stale verifies again, so its report was dropped. A later staleness
		/// starts [`Config::MappingRetention`] over.
		StaleAliasReportCleared {
			/// The account whose report was dropped.
			account: T::AccountId,
			/// The collection identifier.
			collection: Identifier,
			/// The contextual alias.
			alias: Alias,
		},
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

	// ========== Hooks ==========

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn integrity_test() {
			assert!(
				T::ProofValidityWindow::get() > 0,
				"ProofValidityWindow must be greater than zero"
			);
			assert!(T::MappingRetention::get() > 0, "MappingRetention must be greater than zero");
			// Both are measured from the newest root's source time. Equal retentions make the
			// mapping retirable the moment it first becomes reportable.
			assert!(
				T::MappingRetention::get() > T::MemberService::old_root_retention(),
				"MappingRetention must exceed the member service's old root retention"
			);
			assert!(
				!T::OffchainWorkerInterval::get().is_zero(),
				"OffchainWorkerInterval must be greater than zero"
			);
			assert!(
				T::MaxStaleAliasBatch::get() > 0,
				"MaxStaleAliasBatch must be greater than zero"
			);

			// A sweep is an offchain-worker transaction, so a worst-case batch above the budget is
			// dropped by the transaction pool and the sweep stalls with no sign of why. Both the
			// call and its `authorize` read every mapping in the batch, so both are charged.
			let batch = T::MaxStaleAliasBatch::get();
			let budget = OcwWeightBudget::from_normal_max::<T>();
			budget.assert_fits(
				"report_stale_aliases",
				<T as Config>::WeightInfo::report_stale_aliases(batch).saturating_add(
					<T as Config>::WeightInfo::authorize_report_stale_aliases(batch),
				),
			);
			budget.assert_fits(
				"retire_stale_aliases",
				<T as Config>::WeightInfo::retire_stale_aliases(batch).saturating_add(
					<T as Config>::WeightInfo::authorize_retire_stale_aliases(batch),
				),
			);
			budget.assert_fits(
				"clear_stale_alias_reports",
				<T as Config>::WeightInfo::clear_stale_alias_reports(batch).saturating_add(
					<T as Config>::WeightInfo::authorize_clear_stale_alias_reports(batch),
				),
			);
		}

		fn offchain_worker(block_number: BlockNumberFor<T>) {
			Self::sweep_stale_aliases(block_number);
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
			StaleSince::<T>::remove(&who);
			frame_system::Pallet::<T>::dec_sufficients(&who);

			Self::deposit_event(Event::AliasAccountUnset { account: who });

			Ok(())
		}

		/// Link an account to a ring alias, on payment of a PGAS fee.
		///
		/// The origin must be signed; the signer becomes the bound account. The PGAS fee
		/// ([`Config::AliasFee`]) is burned from the signer's PGAS balance. The alias must verify
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

			let fee = T::AliasFee::get().ok_or(Error::<T>::AliasFeeUnset)?;

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
					StaleSince::<T>::remove(old);
				}
				frame_system::Pallet::<T>::inc_sufficients(&who);
			}

			AccountToAlias::<T>::insert(&who, &alias_info);
			AliasToAccount::<T>::insert(collection, &alias_info.ca, &who);
			// The mapping is valid again, so any report of it is void.
			StaleSince::<T>::remove(&who);

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
			// The mapping is valid again, so any report of it is void.
			StaleSince::<T>::remove(&who);

			Self::deposit_event(Event::AliasAccountSet {
				account: who,
				collection: new_info.collection,
				alias: new_info.ca.alias,
			});

			Ok(Pays::No.into())
		}

		/// Stamp [`StaleSince`] on each mapping in `accounts`, starting
		/// [`Config::MappingRetention`].
		///
		/// A mapping is stale once [`Config::MemberService`] stops accepting its revision, which is
		/// when the ring was rebuilt and the old revision ran out of retention there, or when the
		/// ring was deleted. Stamping removes nothing, because a consumer that reads
		/// [`AccountToAlias`] without checking the revision still resolves the mapping;
		/// [`Pallet::retire_stale_aliases`] removes it once the retention has run out.
		///
		/// `accounts` must be in strictly ascending order, so one account cannot be stamped twice
		/// in a batch, and every one of them must hold a stale mapping with no stamp yet.
		#[pallet::authorize(|source, accounts| {
			Self::authorize_sweep(source, accounts, StaleAliasAction::Report)
		})]
		#[pallet::call_index(6)]
		#[pallet::weight(<T as Config>::WeightInfo::report_stale_aliases(accounts.len() as u32))]
		#[pallet::weight_of_authorize(
			<T as Config>::WeightInfo::authorize_report_stale_aliases(accounts.len() as u32)
		)]
		pub fn report_stale_aliases(
			origin: OriginFor<T>,
			accounts: BoundedVec<T::AccountId, T::MaxStaleAliasBatch>,
		) -> DispatchResult {
			ensure_authorized(origin)?;

			let now = T::UnixTime::now().as_secs();
			for account in &accounts {
				let Some(info) = AccountToAlias::<T>::get(account) else {
					// `authorize` read the same mapping on the same state.
					defensive!("indiv-pallet-alias-accounts: swept account must hold a mapping");
					continue;
				};

				// The ring's newest root dates the mapping where the member service dates it, so
				// the retention covers the time it has already been stale, and this block dates it
				// otherwise. A source time is another chain's clock, so a value ahead of this one
				// would push the deadline out and is capped.
				let stale_since =
					T::MemberService::latest_root_source_time(&info.collection, info.ring)
						.map_or(now, |source_time| source_time.min(now));
				StaleSince::<T>::insert(account, stale_since);

				Self::deposit_event(Event::StaleAliasReported {
					account: account.clone(),
					collection: info.collection,
					alias: info.ca.alias,
					removable_at: stale_since.saturating_add(T::MappingRetention::get()),
				});
			}

			Ok(())
		}

		/// Remove each mapping in `accounts`, whose [`Config::MappingRetention`] has run out.
		///
		/// Both directions of the mapping go, along with the stamp and the account's sufficient
		/// reference. `accounts` must be in strictly ascending order, and every one of them must
		/// hold a stale mapping stamped [`Config::MappingRetention`] ago or longer.
		#[pallet::authorize(|source, accounts| {
			Self::authorize_sweep(source, accounts, StaleAliasAction::Retire)
		})]
		#[pallet::call_index(7)]
		#[pallet::weight(<T as Config>::WeightInfo::retire_stale_aliases(accounts.len() as u32))]
		#[pallet::weight_of_authorize(
			<T as Config>::WeightInfo::authorize_retire_stale_aliases(accounts.len() as u32)
		)]
		pub fn retire_stale_aliases(
			origin: OriginFor<T>,
			accounts: BoundedVec<T::AccountId, T::MaxStaleAliasBatch>,
		) -> DispatchResult {
			ensure_authorized(origin)?;

			for account in &accounts {
				let Some(info) = AccountToAlias::<T>::take(account) else {
					defensive!("indiv-pallet-alias-accounts: swept account must hold a mapping");
					continue;
				};
				AliasToAccount::<T>::remove(info.collection, &info.ca);
				StaleSince::<T>::remove(account);
				frame_system::Pallet::<T>::dec_sufficients(account);

				Self::deposit_event(Event::StaleAliasRemoved {
					account: account.clone(),
					collection: info.collection,
					alias: info.ca.alias,
				});
			}

			Ok(())
		}

		/// Drop the [`StaleSince`] stamp of each mapping in `accounts`, which verifies again.
		///
		/// A revision can verify again after it stopped: a collection torn down and re-created
		/// under the same identifier restarts its revisions at zero, so a stored revision can be
		/// reissued. Dropping the stamp keeps the next staleness to a full
		/// [`Config::MappingRetention`] rather than letting it remove the mapping on the spot.
		///
		/// `accounts` must be in strictly ascending order, and every one of them must hold a
		/// stamped mapping whose revision verifies.
		#[pallet::authorize(|source, accounts| {
			Self::authorize_sweep(source, accounts, StaleAliasAction::ClearReport)
		})]
		#[pallet::call_index(8)]
		#[pallet::weight(
			<T as Config>::WeightInfo::clear_stale_alias_reports(accounts.len() as u32)
		)]
		#[pallet::weight_of_authorize(
			<T as Config>::WeightInfo::authorize_clear_stale_alias_reports(accounts.len() as u32)
		)]
		pub fn clear_stale_alias_reports(
			origin: OriginFor<T>,
			accounts: BoundedVec<T::AccountId, T::MaxStaleAliasBatch>,
		) -> DispatchResult {
			ensure_authorized(origin)?;

			for account in &accounts {
				let Some(info) = AccountToAlias::<T>::get(account) else {
					defensive!("indiv-pallet-alias-accounts: swept account must hold a mapping");
					continue;
				};
				StaleSince::<T>::remove(account);

				Self::deposit_event(Event::StaleAliasReportCleared {
					account: account.clone(),
					collection: info.collection,
					alias: info.ca.alias,
				});
			}

			Ok(())
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
			if !T::MemberService::is_revision_valid(&info.collection, info.ring, info.revision) {
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

			let res = T::MemberService::verify_membership(
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
		/// Which sweep call applies to `account`'s mapping at `now`, or `None` when none does.
		///
		/// `None` covers a missing mapping, one whose revision verifies and carries no stamp, and
		/// one waiting out [`Config::MappingRetention`]. The offchain worker skips those and
		/// `authorize` rejects them, so the two agree on what a sweep may carry.
		pub fn stale_alias_action(account: &T::AccountId, now: u64) -> Option<StaleAliasAction> {
			let info = AccountToAlias::<T>::get(account)?;
			let stamped = StaleSince::<T>::get(account);

			if T::MemberService::is_revision_valid(&info.collection, info.ring, info.revision) {
				return stamped.is_some().then_some(StaleAliasAction::ClearReport);
			}

			match stamped {
				None => Some(StaleAliasAction::Report),
				Some(stale_since) => (now >=
					stale_since.saturating_add(T::MappingRetention::get()))
				.then_some(StaleAliasAction::Retire),
			}
		}

		/// Validates a sweep transaction for `action`.
		///
		/// A sweep only ever comes from this pallet's own offchain worker, so it is restricted to
		/// local and in-block sources. Every account is held to `action` here, which is what lets
		/// the call itself mutate without re-reading the state it depends on.
		///
		/// The batch must be in strictly ascending order. Repeats are what that rules out: the
		/// same account twice would be stamped or removed twice, and the second pass reads state
		/// the first one changed.
		///
		/// A success checks the whole batch, so it returns no unspent weight to refund.
		pub fn authorize_sweep(
			source: TransactionSource,
			accounts: &BoundedVec<T::AccountId, T::MaxStaleAliasBatch>,
			action: StaleAliasAction,
		) -> Result<(ValidTransaction, Weight), TransactionValidityError> {
			if !matches!(source, TransactionSource::InBlock | TransactionSource::Local) {
				return Err(AuthorizeInvalidity::TransactionNotLocal.into());
			}
			if accounts.is_empty() {
				return Err(AuthorizeInvalidity::EmptyBatch.into());
			}
			if accounts.windows(2).any(|pair| pair[0] >= pair[1]) {
				return Err(AuthorizeInvalidity::UnsortedBatch.into());
			}

			let now = T::UnixTime::now().as_secs();
			if accounts
				.iter()
				.any(|account| Self::stale_alias_action(account, now) != Some(action))
			{
				return Err(AuthorizeInvalidity::WrongStaleState.into());
			}

			let tag = match action {
				StaleAliasAction::Report => "alias-accounts:report-stale",
				StaleAliasAction::Retire => "alias-accounts:retire-stale",
				StaleAliasAction::ClearReport => "alias-accounts:clear-stale-report",
			};

			// A finite longevity lets a sweep that never makes it into a block self-evict rather
			// than linger against state it no longer matches. Propagation is off because peers
			// validate gossiped transactions with a source of `External`, which this rejects.
			let validity = ValidTransaction::with_tag_prefix(tag)
				.and_provides(accounts)
				.priority(tx_priority::CLEANUP)
				.longevity(STALE_ALIAS_TX_LONGEVITY)
				.propagate(false)
				.build()
				.expect("tag prefix is not empty; qed");

			Ok((validity, Weight::zero()))
		}

		/// Submits the sweep transactions for the stale mappings this node can see.
		///
		/// Runs every [`Config::OffchainWorkerInterval`] blocks and reads every mapping, sorting
		/// them into the three actions and submitting one transaction per action that has work.
		/// A batch stops at [`Config::MaxStaleAliasBatch`], so a chain with more stale mappings
		/// than that retires them over as many sweeps as it takes.
		pub(crate) fn sweep_stale_aliases(block_number: BlockNumberFor<T>) {
			let interval = T::OffchainWorkerInterval::get();
			if interval.is_zero() {
				// The `integrity_test` holds the interval above zero, so a runtime that reaches
				// this configured it past that check.
				log::error!(
					target: LOG_TARGET,
					"offchain worker: `OffchainWorkerInterval` is zero, no sweep can run",
				);
				return;
			}
			if !(block_number % interval).is_zero() {
				return;
			}

			let now = T::UnixTime::now().as_secs();
			let limit = T::MaxStaleAliasBatch::get() as usize;
			let mut batches: [Vec<T::AccountId>; 3] = Default::default();

			for account in AccountToAlias::<T>::iter_keys() {
				let Some(action) = Self::stale_alias_action(&account, now) else { continue };
				let batch = &mut batches[action as usize];
				if batch.len() < limit {
					batch.push(account);
				}
				if batches.iter().all(|batch| batch.len() >= limit) {
					break;
				}
			}

			for (action, mut accounts) in
				[StaleAliasAction::Report, StaleAliasAction::Retire, StaleAliasAction::ClearReport]
					.into_iter()
					.zip(batches)
			{
				if accounts.is_empty() {
					continue;
				}
				// `authorize` holds a batch to ascending order, which the map's iteration order
				// does not give.
				accounts.sort_unstable();
				let Ok(accounts) = BoundedVec::try_from(accounts) else {
					defensive!("indiv-pallet-alias-accounts: batch is built within its bound");
					continue;
				};

				let call = match action {
					StaleAliasAction::Report => Call::<T>::report_stale_aliases { accounts },
					StaleAliasAction::Retire => Call::<T>::retire_stale_aliases { accounts },
					StaleAliasAction::ClearReport =>
						Call::<T>::clear_stale_alias_reports { accounts },
				};
				Self::submit_sweep(call);
			}
		}

		/// Submits one sweep transaction, logging what the transaction pool did with it.
		///
		/// A rejection is expected rather than exceptional: a sweep that is already in the pool
		/// from the previous interval is byte-identical and deduplicated there.
		fn submit_sweep(call: Call<T>) {
			let tx = T::create_authorized_transaction(call.into());
			match SubmitTransaction::<T, Call<T>>::submit_transaction(tx) {
				Ok(()) => log::debug!(
					target: LOG_TARGET,
					"offchain worker: submitted a stale-alias sweep",
				),
				Err(()) => log::warn!(
					target: LOG_TARGET,
					"offchain worker: failed to submit a stale-alias sweep",
				),
			}
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
				T::MemberService::is_revision_valid(collection, ring_index, revision),
				Error::<T>::StaleRevision
			);

			let ca = T::MemberService::verify_membership(
				collection, proof, ring_index, revision, *context, msg,
			)
			.map_err(|_| Error::<T>::BadProof)?;

			Ok(RevisedContextualAlias { revision, ring: ring_index, ca })
		}
	}
}
