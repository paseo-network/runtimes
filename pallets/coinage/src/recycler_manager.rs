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
use alloc::{
	collections::{BTreeMap, BTreeSet},
	vec,
	vec::Vec,
};
use codec::Encode;
use frame_support::traits::UnixTime;
use indiv_support::traits::{AppendOnlyMembers, MembershipProver, RingMode};
use sp_runtime::transaction_validity::{
	InvalidTransaction, TransactionValidityError, ValidTransaction,
};
use verifiable::BatchProofItem;

/// The error type for the load function.
#[derive(Debug)]
pub enum RecyclerLoadError {
	/// The member key is already used in another recycler.
	MemberKeyAlreadyUsed,
	/// The member key is invalid according to the crypto scheme.
	InvalidMemberKey,
	/// The recycler collection does not exist and could not be created on-demand.
	CannotCreateRecyclerCollection,
	/// An unexpected error occurred.
	InternalError,
}

impl RecyclerLoadError {
	pub fn into_pallet_error<T>(self) -> Error<T> {
		match self {
			RecyclerLoadError::MemberKeyAlreadyUsed => Error::<T>::MemberKeyAlreadyUsed,
			RecyclerLoadError::InvalidMemberKey => Error::<T>::InvalidMemberKey,
			RecyclerLoadError::CannotCreateRecyclerCollection =>
				Error::<T>::CannotCreateRecyclerCollection,
			RecyclerLoadError::InternalError => Error::<T>::InternalError,
		}
	}
}

/// The error type for the validate_alias_proof function.
#[derive(Debug)]
pub enum ValidateAliasProofError {
	/// The proof is invalid according to the MemberService.
	InvalidProof,
	/// The alias has already been unloaded from this ring.
	AlreadyUnloaded,
	/// The recycler revision does not match (recycler may not exist or has been rebuilt).
	InvalidRevision,
}

impl From<ValidateAliasProofError> for CustomInvalidity {
	fn from(e: ValidateAliasProofError) -> Self {
		match e {
			ValidateAliasProofError::InvalidProof => CustomInvalidity::InvalidAliasProof,
			ValidateAliasProofError::AlreadyUnloaded => CustomInvalidity::RecyclerAlreadyUnloaded,
			ValidateAliasProofError::InvalidRevision => CustomInvalidity::InvalidRecyclerRevision,
		}
	}
}

impl From<ValidateAliasProofError> for TransactionValidityError {
	fn from(e: ValidateAliasProofError) -> Self {
		CustomInvalidity::from(e).into()
	}
}

impl ValidateAliasProofError {
	pub fn into_pallet_error<T>(self) -> Error<T> {
		match self {
			ValidateAliasProofError::InvalidProof => Error::<T>::InvalidAliasProof,
			ValidateAliasProofError::AlreadyUnloaded => Error::<T>::RecyclerAlreadyUnloaded,
			ValidateAliasProofError::InvalidRevision => Error::<T>::InvalidRecyclerRevision,
		}
	}
}

/// Manager for handling recycler-related storage operations.
///
/// Recyclers are defined for a specific coin value, take coins as input, gather the member keys
/// in ring-VRF rings, and allow users to unload (given a valid unload token) new coins or assets as
/// output.
///
/// Ring-VRF ring management (members, pending members, ring building, member cleanup) is
/// delegated to [`Config::MemberService`] (pallet-members). This manager handles the
/// coinage-specific storage: collection creation tracking, member-to-value mapping, alias
/// double-spend tracking, and ring expiration/cleanup.
pub struct RecyclerManager<T>(core::marker::PhantomData<T>);

impl<T: Config> RecyclerManager<T> {
	/// Get the index of the oldest ring that hasn't been cleaned yet.
	///
	/// Rings are cleaned sequentially starting from 0 because they fill up and become
	/// immutable in order, so older rings always expire first. Returns `None` only on
	/// `RingIndex` overflow (effectively unreachable).
	fn next_ring_to_clean(value: CoinValue) -> Option<RingIndex> {
		RecyclersLastRemovedRingIndex::<T>::get(value)
			.map(|i| i.checked_add(1))
			.unwrap_or(Some(0))
	}

	/// Check whether a recycler collection exists for the given coin value.
	pub(crate) fn collection_exists(value: CoinValue) -> bool {
		RecyclerCollectionCreated::<T>::contains_key(value)
	}

	/// Ensure a recycler collection exists for the given coin value, creating it if needed.
	///
	/// Collections are normally created eagerly during `on_poll` initialization.
	/// This serves as a safety fallback.
	pub(crate) fn ensure_collection_exists(value: CoinValue) -> DispatchResult {
		if RecyclerCollectionCreated::<T>::contains_key(value) {
			return Ok(());
		}

		let identifier = Pallet::<T>::recycler_collection_identifier(value);
		T::MemberService::create_collection(
			T::CollectionOwner::get(),
			&identifier,
			pallet::RECYCLER_ONBOARDING_SIZE,
			RingMode::AppendOnly,
			T::RecyclerRingExponent::get(),
			None,
		)?;

		RecyclerCollectionCreated::<T>::insert(value, ());
		Ok(())
	}

	/// Check if a member key is already used in any recycler.
	pub fn is_member_key_used(member: &MemberOf<T>) -> bool {
		RecyclersCoinToRecycler::<T>::contains_key(member)
	}

	/// Validates that a recycler ring revision is valid, with expired recyclers considered invalid.
	///
	/// Returns `true` if the ring still has a current root in [`Config::MemberService`], has not
	/// yet expired, and the revision is valid (either current or an old revision within the
	/// retention period).
	///
	/// Recyclers differ from generic membership rings: once a recycler ring is cleaned, its
	/// remaining backing value is destroyed and the ring must not remain spendable just because the
	/// old root is still retained for other consumers. Expired recyclers are therefore rejected
	/// immediately, even before background cleanup removes the ring.
	pub fn validate_recycler_revision(
		value: CoinValue,
		index: RingIndex,
		revision: RevisionIndex,
	) -> bool {
		let identifier = Pallet::<T>::recycler_collection_identifier(value);
		let Some(status) = T::MemberService::ring_status(&identifier, index) else {
			return false;
		};

		if let Some(immutable_since) = status.immutable_since {
			let now = T::UnixTime::now().as_secs();
			let expiration =
				immutable_since.saturating_add(u64::from(T::RecyclerExpirationTime::get()));
			if now >= expiration {
				return false;
			}
		}

		T::MemberService::ring_revision(&identifier, index).is_some() &&
			T::MemberService::is_revision_valid(&identifier, index, revision)
	}

	/// Push a member key into the recycler system.
	///
	/// This ensures the collection exists (normally a no-op after `on_poll` initialization),
	/// validates the key, and delegates to `MemberService::add_members()`.
	pub fn load(value: CoinValue, member_key: MemberOf<T>) -> Result<(), RecyclerLoadError> {
		if Self::is_member_key_used(&member_key) {
			return Err(RecyclerLoadError::MemberKeyAlreadyUsed);
		}

		if !CryptoOf::<T>::is_member_valid(&member_key) {
			return Err(RecyclerLoadError::InvalidMemberKey);
		}

		Self::ensure_collection_exists(value)
			.map_err(|_| RecyclerLoadError::CannotCreateRecyclerCollection)?;

		let identifier = Pallet::<T>::recycler_collection_identifier(value);
		T::MemberService::add_members(&identifier, vec![member_key.clone()])
			.map_err(|_| RecyclerLoadError::InternalError)?;

		RecyclersCoinToRecycler::<T>::insert(member_key, value);

		Ok(())
	}

	/// Push multiple member keys into recycler collections, grouped by coin value.
	///
	/// Duplicate and invalid keys are rejected before any mutation. Member-service failures are
	/// handled transactionally here so grouped recycler bookkeeping remains consistent.
	pub fn load_batch_grouped(
		vouchers: &[(CoinValue, MemberOf<T>)],
	) -> Result<(), RecyclerLoadError> {
		let mut seen = BTreeSet::new();
		let mut grouped: BTreeMap<CoinValue, Vec<MemberOf<T>>> = BTreeMap::new();

		for (value, member_key) in vouchers {
			if !seen.insert(member_key.encode()) || Self::is_member_key_used(member_key) {
				return Err(RecyclerLoadError::MemberKeyAlreadyUsed);
			}

			if !CryptoOf::<T>::is_member_valid(member_key) {
				return Err(RecyclerLoadError::InvalidMemberKey);
			}

			grouped.entry(*value).or_default().push(member_key.clone());
		}

		for (value, members) in grouped {
			Self::ensure_collection_exists(value)
				.map_err(|_| RecyclerLoadError::CannotCreateRecyclerCollection)?;
			let identifier = Pallet::<T>::recycler_collection_identifier(value);
			for member_key in &members {
				RecyclersCoinToRecycler::<T>::insert(member_key.clone(), value);
			}
			T::MemberService::add_members(&identifier, members)
				.map_err(|_| RecyclerLoadError::InternalError)?;
		}

		Ok(())
	}

	/// Unload tokens from a recycler.
	///
	/// Verifies all proofs in one batch via
	/// `MemberService::verify_memberships_in_ring_at_rev()`, then sequentially checks expected
	/// aliases, already-unloaded status, and marks each alias.
	pub fn unload(
		value: CoinValue,
		index: RingIndex,
		revision: RevisionIndex,
		aliases: &[Alias],
		alias_proofs: &[ProofOf<T>],
		proven_msg: &[u8; 32],
	) -> Result<(), DispatchError> {
		ensure!(aliases.len() == alias_proofs.len(), Error::<T>::ProofAndAliasMismatch);
		ensure!(
			Self::validate_recycler_revision(value, index, revision),
			Error::<T>::InvalidRecyclerRevision
		);

		let identifier = Pallet::<T>::recycler_collection_identifier(value);

		// Build batch items: all proofs share the same message and context for a recycler input.
		let items = alias_proofs
			.iter()
			.map(|proof| BatchProofItem {
				proof: proof.clone(),
				message: proven_msg.to_vec(),
				context: UNLOADING_RECYCLER_CONTEXT.to_vec(),
			})
			.collect::<Vec<_>>();

		// One batch verification call instead of per-proof verification.
		let results = T::MemberService::verify_memberships_in_ring_at_rev(
			&identifier,
			index,
			revision,
			&items,
		)
		.map_err(|_| Error::<T>::InvalidAliasProof)?;

		ensure!(results.len() == aliases.len(), Error::<T>::ProofAndAliasMismatch);

		// Sequential post-processing: alias matching, already-unloaded checks, marking.
		for (result, expected_alias) in results.iter().zip(aliases.iter()) {
			let alias = result.alias;
			ensure!(alias == *expected_alias, Error::<T>::ProofAndAliasMismatch);
			if RecyclersUnloaded::<T>::contains_key((value, index, alias)) {
				return Err(Error::<T>::RecyclerAlreadyUnloaded.into());
			}
			Self::mark_alias_unloaded(value, index, alias);
		}

		Ok(())
	}

	/// Validates a single alias proof without marking it as unloaded.
	///
	/// Delegates proof verification to `Config::MemberService::verify_membership_at_rev` and
	/// checks that the alias has not already been unloaded from this ring.
	///
	/// Returns the validated alias on success.
	pub fn validate_alias_proof(
		value: CoinValue,
		index: RingIndex,
		revision: RevisionIndex,
		alias_proof: &ProofOf<T>,
		proven_msg: &[u8; 32],
	) -> Result<Alias, ValidateAliasProofError> {
		// Check that the ring revision matches.
		if !Self::validate_recycler_revision(value, index, revision) {
			return Err(ValidateAliasProofError::InvalidRevision);
		}

		let identifier = Pallet::<T>::recycler_collection_identifier(value);

		let result = T::MemberService::verify_membership_at_rev(
			&identifier,
			alias_proof,
			index,
			revision,
			UNLOADING_RECYCLER_CONTEXT,
			proven_msg.as_ref(),
		)
		.map_err(|_| ValidateAliasProofError::InvalidProof)?;

		let alias = result.alias;

		if RecyclersUnloaded::<T>::contains_key((value, index, alias)) {
			return Err(ValidateAliasProofError::AlreadyUnloaded);
		}

		Ok(alias)
	}

	/// Marks a single alias as unloaded.
	/// Should only be called after successful validation via `validate_alias_proof`.
	pub fn mark_alias_unloaded(value: CoinValue, index: RingIndex, alias: Alias) {
		RecyclersUnloaded::<T>::insert((value, index, alias), ());
	}

	/// Checks if the next recycler ring (by sequential index) can be cleaned.
	///
	/// A ring is eligible for cleanup when it has been immutable for longer than
	/// [`Config::RecyclerExpirationTime`].
	///
	/// Returns the transaction validity info on success.
	pub fn ensure_can_clean(
		value: CoinValue,
	) -> Result<(ValidTransaction, Weight), TransactionValidityError> {
		let identifier = Pallet::<T>::recycler_collection_identifier(value);
		let next_ring = Self::next_ring_to_clean(value).ok_or(InvalidTransaction::Stale)?;

		let status = T::MemberService::ring_status(&identifier, next_ring)
			.ok_or(InvalidTransaction::Future)?;

		let immutable_since = status.immutable_since.ok_or(InvalidTransaction::Future)? as u32;
		let now = T::UnixTime::now().as_secs() as u32;
		let expiration = immutable_since.saturating_add(T::RecyclerExpirationTime::get());
		if now < expiration {
			return Err(InvalidTransaction::Future.into());
		}

		let validity = ValidTransaction::with_tag_prefix("coinage:remove-recycler")
			.and_provides((value, next_ring))
			.into();
		Ok((validity, Weight::zero()))
	}

	/// Cleans an expired recycler ring.
	///
	/// Delegates ring removal to `MemberService::remove_ring()`.
	/// Returns `(remaining_coins, member_count)`.
	pub fn clean_unchecked(value: CoinValue) -> Result<(u32, u32), DispatchError> {
		let identifier = Pallet::<T>::recycler_collection_identifier(value);
		let next_ring = Self::next_ring_to_clean(value).ok_or(Error::<T>::InternalError)?;

		let status = T::MemberService::ring_status(&identifier, next_ring)
			.ok_or(Error::<T>::InternalError)?;

		// Remove the member-to-coin-value reverse mapping for all members in this ring.
		let members = T::MemberService::ring_members(&identifier, next_ring);
		let member_count = members.len() as u32;
		for member in members {
			RecyclersCoinToRecycler::<T>::remove(&member);
		}

		// Remove the ring via MemberService.
		T::MemberService::remove_ring(&identifier, next_ring)?;

		// Queue unloaded aliases for this ring for dusting (only if any exist).
		if RecyclersUnloaded::<T>::iter_prefix((value, next_ring)).next().is_some() {
			pallet::RecyclersDusting::<T>::insert((value, next_ring), ());
		}

		// Update last removed index.
		RecyclersLastRemovedRingIndex::<T>::insert(value, next_ring);

		// Return the count of remaining (non-unloaded) coins and the member count.
		let unloaded_count = RecyclersUnloaded::<T>::iter_prefix((value, next_ring)).count() as u32;
		Ok((status.total.saturating_sub(unloaded_count), member_count))
	}

	/// Checks if there is recycler dust to clean.
	///
	/// Returns the transaction validity info on success.
	pub fn ensure_can_clean_dust() -> Result<(ValidTransaction, Weight), TransactionValidityError> {
		let first = pallet::RecyclersDusting::<T>::iter_keys()
			.next()
			.ok_or(InvalidTransaction::Stale)?;
		let validity = ValidTransaction::with_tag_prefix("coinage:clean-recycler-dust")
			.and_provides(first)
			.into();
		Ok((validity, Weight::zero()))
	}

	/// Cleans recycler dust.
	///
	/// Removes up to DUST_CLEANUP_BATCH_SIZE unloaded alias entries per call to bound the
	/// operation.
	pub fn clean_dust_unchecked() -> u32 {
		if let Some((value, ring_index)) = pallet::RecyclersDusting::<T>::iter_keys().next() {
			let res = RecyclersUnloaded::<T>::clear_prefix(
				(value, ring_index),
				pallet::DUST_CLEANUP_BATCH_SIZE,
				None,
			);
			if res.maybe_cursor.is_none() {
				pallet::RecyclersDusting::<T>::remove((value, ring_index));
			}
			res.unique
		} else {
			0
		}
	}
}
