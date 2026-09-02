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

use crate::{pallet::BigEndianPeriod, *};
use alloc::vec;
use frame_support::traits::UnixTime;
use indiv_support::{
	traits::{AppendOnlyMembers, MembershipProver, RingMode},
	tx_priority,
};
use sp_runtime::{
	transaction_validity::{InvalidTransaction, TransactionValidityError, ValidTransaction},
	DispatchError,
};
use verifiable::GenerateVerifiable;

/// Manager for handling paid unload token-related storage operations.
///
/// Paid unload tokens are gathered in ring-VRF rings, grouped by time period. The actual token
/// is the alias produced in the unload token context, which is derived independently of the payer's
/// account identity.
///
/// Ring-VRF ring management (members, pending members, ring building, member cleanup) is
/// delegated to [`Config::MemberService`] (pallet-members). This manager handles the
/// coinage-specific storage: collection creation tracking, member-to-period mapping, alias
/// double-spend tracking, and period expiration/cleanup.
pub struct PaidTknManager<T>(core::marker::PhantomData<T>);

impl<T: Config> PaidTknManager<T> {
	/// Compute the current paid unload token period from Unix time.
	fn current_period() -> Period {
		let now = T::UnixTime::now().as_secs() as u32;
		now.checked_div(T::PaidUnloadTokenTimePeriod::get()).unwrap_or(0)
	}

	/// Compute the expiration time for a given paid token period.
	fn period_expiration_time(period: Period) -> u32 {
		(period + 1)
			.saturating_mul(T::PaidUnloadTokenTimePeriod::get())
			.saturating_add(T::PaidUnloadTokenRingExpirationTime::get())
	}

	/// Ensure a paid token collection exists for the given period.
	///
	/// Returns `Ok(true)` when a new collection is created, `Ok(false)` if it already exists.
	fn ensure_collection_exists(period: Period) -> Result<bool, DispatchError> {
		if PaidTokenCollectionsCreated::<T>::contains_key(BigEndianPeriod::from(period)) {
			return Ok(false);
		}

		let identifier = Pallet::<T>::paid_token_collection_identifier(period);
		T::MemberService::create_collection(
			Pallet::<T>::paid_token_collection_owner(),
			&identifier,
			PAID_UNLOAD_TOKEN_ONBOARDING_SIZE,
			RingMode::AppendOnly,
			T::PaidUnloadTokenRingExponent::get(),
			None,
		)?;

		PaidTokenCollectionsCreated::<T>::insert(BigEndianPeriod::from(period), ());
		Ok(true)
	}

	/// Ensure the current period collection exists.
	///
	/// This is intended for proactive maintenance in `on_poll`.
	pub fn ensure_current_period_collection_exists() -> Result<bool, DispatchError> {
		Self::ensure_collection_exists(Self::current_period())
	}

	/// Add a member key to the paid unload token system.
	///
	/// Ensures the member key is valid and not already used, then verifies the proof of ownership.
	/// It computes the current time period, ensures the collection exists, and delegates to
	/// `Config::MemberService::add_members`. The on-poll proactive path creates the
	/// collection ahead of time, while this remains as a fallback.
	pub fn add_member(
		caller: T::AccountId,
		member: MemberOf<T>,
		proof_of_ownership: <CryptoOf<T> as GenerateVerifiable>::Signature,
	) -> DispatchResult {
		if pallet::PaidUnloadTokenMembers::<T>::contains_key(&member) {
			return Err(Error::<T>::MemberKeyAlreadyUsed.into());
		}
		if !CryptoOf::<T>::is_member_valid(&member) {
			return Err(Error::<T>::InvalidMemberKey.into());
		}
		ensure!(
			CryptoOf::<T>::verify_signature(&proof_of_ownership, &caller.encode()[..], &member),
			Error::<T>::InvalidProofOfOwnership
		);

		let period = Self::current_period();

		let _ = Self::ensure_collection_exists(period)?;

		let identifier = Pallet::<T>::paid_token_collection_identifier(period);
		T::MemberService::add_members(&identifier, vec![member.clone()])?;

		pallet::PaidUnloadTokenMembers::<T>::insert(member, ());

		Ok(())
	}

	/// Checks if a specific ring in an expired paid token period can be cleaned.
	///
	/// A ring is eligible for cleanup when:
	/// - The period exists in `PaidTokenCollectionsCreated`
	/// - The period has expired
	/// - `ring_index` matches the next ring to clean (sequential cleanup)
	/// - The ring exists (i.e. we haven't yet advanced past the last populated ring)
	///
	/// Note: since we don't call `remove_ring`, every ring that was ever populated keeps its
	/// original `total > 0` in `ring_status`. A `total == 0` means we've reached a ring index
	/// that was never populated — i.e. all rings have been processed.
	/// We assume members have been onboarded into rings because the onboarding size
	/// is 1 and time has passed since the collection doesn't accept new members.
	///
	/// Returns the transaction validity info on success.
	pub fn ensure_can_clean_ring(
		period: Period,
		ring_index: RingIndex,
	) -> Result<(ValidTransaction, Weight), TransactionValidityError> {
		if !pallet::PaidTokenCollectionsCreated::<T>::contains_key(BigEndianPeriod::from(period)) {
			return Err(pallet::CustomInvalidity::NothingToBuild.into());
		}

		let now = T::UnixTime::now().as_secs() as u32;
		if now < Self::period_expiration_time(period) {
			return Err(InvalidTransaction::Future.into());
		}

		let next_ring =
			pallet::PaidUnloadTokenNextRingToClean::<T>::get(BigEndianPeriod::from(period))
				.unwrap_or(0);
		if ring_index != next_ring {
			return Err(InvalidTransaction::Stale.into());
		}

		// Check that the ring was ever populated. Since we don't call `remove_ring`, a
		// populated ring always keeps `total > 0`. A `total == 0` means we've passed the
		// last ring and should proceed to collection deletion instead.
		let identifier = Pallet::<T>::paid_token_collection_identifier(period);
		let ring_exists =
			T::MemberService::ring_status(&identifier, ring_index).is_some_and(|s| s.total > 0);
		if !ring_exists {
			return Err(InvalidTransaction::Stale.into());
		}

		let validity = ValidTransaction::with_tag_prefix("coinage:clean-paid-token-ring")
			.and_provides((period, ring_index))
			.priority(tx_priority::CLEANUP)
			.into();
		Ok((validity, Weight::zero()))
	}

	/// Cleans a single ring of an expired paid unload token collection.
	///
	/// Removes all `PaidUnloadTokenMembers` entries for the ring's members and advances the
	/// next-ring-to-clean tracker. Actual ring removal in pallet-members is handled by
	/// `delete_collection`.
	pub fn clean_ring_unchecked(
		period: Period,
		ring_index: RingIndex,
	) -> Result<u32, DispatchError> {
		let identifier = Pallet::<T>::paid_token_collection_identifier(period);

		let members = T::MemberService::ring_members(&identifier, ring_index);
		let count = members.len() as u32;
		for member in members {
			pallet::PaidUnloadTokenMembers::<T>::remove(&member);
		}

		pallet::PaidUnloadTokenNextRingToClean::<T>::insert(
			BigEndianPeriod::from(period),
			ring_index + 1,
		);

		Ok(count)
	}

	/// Checks if an expired paid token collection can be deleted.
	///
	/// A collection is eligible for deletion when:
	/// - The period exists in `PaidTokenCollectionsCreated`
	/// - The period has expired
	/// - All rings with members have been cleaned (the next ring to clean has `total == 0`)
	///
	/// Returns the transaction validity info on success.
	pub fn ensure_can_delete_collection(
		period: Period,
	) -> Result<(ValidTransaction, Weight), TransactionValidityError> {
		if !pallet::PaidTokenCollectionsCreated::<T>::contains_key(BigEndianPeriod::from(period)) {
			return Err(pallet::CustomInvalidity::NothingToBuild.into());
		}

		let now = T::UnixTime::now().as_secs() as u32;
		if now < Self::period_expiration_time(period) {
			return Err(InvalidTransaction::Future.into());
		}

		// All rings must have been processed: the next ring to clean must be past the last
		// populated ring (i.e. `total == 0`, meaning it was never created).
		let next_ring =
			pallet::PaidUnloadTokenNextRingToClean::<T>::get(BigEndianPeriod::from(period))
				.unwrap_or(0);
		let identifier = Pallet::<T>::paid_token_collection_identifier(period);
		let more_rings_to_clean =
			T::MemberService::ring_status(&identifier, next_ring).is_some_and(|s| s.total > 0);
		if more_rings_to_clean {
			return Err(InvalidTransaction::Stale.into());
		}

		let validity = ValidTransaction::with_tag_prefix("coinage:delete-paid-token-collection")
			.and_provides(period)
			.priority(tx_priority::CLEANUP)
			.into();
		Ok((validity, Weight::zero()))
	}

	/// Deletes an expired paid unload token collection after all rings have been cleaned.
	///
	/// Delegates collection removal to `MemberService::delete_collection()`, removes the
	/// period tracking entries, and queues consumed tokens for dusting if needed.
	pub fn delete_collection_unchecked(period: Period) -> DispatchResult {
		let identifier = Pallet::<T>::paid_token_collection_identifier(period);

		T::MemberService::delete_collection(
			Pallet::<T>::paid_token_collection_owner(),
			&identifier,
		)?;

		pallet::PaidTokenCollectionsCreated::<T>::remove(BigEndianPeriod::from(period));
		pallet::PaidUnloadTokenNextRingToClean::<T>::remove(BigEndianPeriod::from(period));

		// Queue consumed tokens for dusting (only if any exist).
		if pallet::PaidUnloadTokenConsumed::<T>::iter_prefix((BigEndianPeriod::from(period),))
			.next()
			.is_some()
		{
			pallet::PaidUnloadTokenDusting::<T>::insert(BigEndianPeriod::from(period), ());
		}

		Ok(())
	}

	/// Validate the proof for the consumption of a paid unload token.
	///
	/// This checks that the collection has not expired, constructs the period-specific context,
	/// delegates proof verification to `T::MemberService::verify_membership`, and
	/// checks that the alias has not already been consumed.
	pub fn validate_token_consumption_proof(
		period: Period,
		ring_index: RingIndex,
		revision: RevisionIndex,
		proof: &ProofOf<T>,
		msg: &[u8],
	) -> Result<Alias, TransactionValidityError> {
		let identifier = Pallet::<T>::paid_token_collection_identifier(period);

		// Accept the current root or any retained old root within the grace period.
		if !T::MemberService::is_revision_valid(&identifier, ring_index, revision) {
			return Err(pallet::CustomInvalidity::InvalidPaidTokenRingRevision.into());
		}

		// Check that the collection still exists (not expired).
		let now = T::UnixTime::now().as_secs() as u32;
		if now >= Self::period_expiration_time(period) {
			return Err(TransactionValidityError::Invalid(InvalidTransaction::Stale));
		}

		let context = {
			let mut c = [0u8; 32];
			c[..28].copy_from_slice(pallet::PAID_UNLOAD_TOKEN_CONTEXT_BASE.as_ref());
			c[28..32].copy_from_slice(&period.to_le_bytes());
			c
		};

		let result = T::MemberService::verify_membership(
			&identifier,
			proof,
			ring_index,
			revision,
			context,
			msg,
		)
		.map_err(|_| pallet::CustomInvalidity::InvalidUnloadTokenProof)?;

		let alias = result.alias;

		if pallet::PaidUnloadTokenConsumed::<T>::contains_key((
			BigEndianPeriod::from(period),
			ring_index,
			alias,
		)) {
			return Err(pallet::CustomInvalidity::UnloadTokenAlreadyConsumed.into());
		}

		Ok(alias)
	}

	/// Mark a paid unload token as consumed.
	pub fn mark_token_consumed(period: Period, ring_index: RingIndex, alias: Alias) {
		pallet::PaidUnloadTokenConsumed::<T>::insert(
			(BigEndianPeriod::from(period), ring_index, alias),
			(),
		);
	}

	/// Checks whether there is paid unload token dust to clean.
	///
	/// Returns the transaction validity info on success.
	pub fn ensure_can_clean_dust() -> Result<(ValidTransaction, Weight), TransactionValidityError> {
		let first = pallet::PaidUnloadTokenDusting::<T>::iter_keys()
			.next()
			.ok_or(InvalidTransaction::Stale)?;
		let validity =
			ValidTransaction::with_tag_prefix(pallet::CLEAN_PAID_UNLOAD_TOKEN_DUST_TX_TAG_PREFIX)
				.and_provides(first.0)
				.priority(tx_priority::CLEANUP)
				.into();
		Ok((validity, Weight::zero()))
	}

	/// Cleans paid unload token dust.
	///
	/// Removes up to DUST_CLEANUP_BATCH_SIZE consumed token entries per call to bound the
	/// operation.
	pub fn clean_dust_unchecked() -> u32 {
		if let Some(period) = pallet::PaidUnloadTokenDusting::<T>::iter_keys().next() {
			let res = pallet::PaidUnloadTokenConsumed::<T>::clear_prefix(
				(period,),
				pallet::DUST_CLEANUP_BATCH_SIZE,
				None,
			);
			if res.maybe_cursor.is_none() {
				pallet::PaidUnloadTokenDusting::<T>::remove(period);
			}
			res.unique
		} else {
			0
		}
	}
}
