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

//! The recycler manager handles the storage and operations for recyclers.
//!
//! See [`RecyclerManager`] for details.

use crate::*;
use alloc::{
	collections::{BTreeMap, BTreeSet},
	vec,
	vec::Vec,
};
use codec::Encode;
use frame_support::{defensive, traits::UnixTime};
use indiv_support::{
	traits::{AppendOnlyMembers, MembershipProver, RingMembershipProof, RingMode},
	tx_priority,
};
use sp_core::H256;
use sp_runtime::{
	traits::BlakeTwo256,
	transaction_validity::{InvalidTransaction, TransactionValidityError, ValidTransaction},
};
use sp_trie::{delta_trie_root, LayoutV1, StorageProof, Trie, TrieDBBuilder};

/// The error type for the load function.
#[derive(Debug)]
pub enum RecyclerLoadError {
	/// The member key is already used in another recycler.
	MemberKeyAlreadyUsed,
	/// The member key is invalid according to the crypto scheme.
	InvalidMemberKey,
	/// An unexpected error occurred.
	InternalError,
}

impl RecyclerLoadError {
	pub fn into_pallet_error<T>(self) -> Error<T> {
		match self {
			RecyclerLoadError::MemberKeyAlreadyUsed => Error::<T>::MemberKeyAlreadyUsed,
			RecyclerLoadError::InvalidMemberKey => Error::<T>::InvalidMemberKey,
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
	/// The alias is temporarily locked after a previous failed dispatch.
	TemporarilyLocked,
	/// The recycler revision does not match (recycler may not exist or has been rebuilt).
	InvalidRevision,
}

impl From<ValidateAliasProofError> for CustomInvalidity {
	fn from(e: ValidateAliasProofError) -> Self {
		match e {
			ValidateAliasProofError::InvalidProof => CustomInvalidity::InvalidAliasProof,
			ValidateAliasProofError::AlreadyUnloaded => CustomInvalidity::RecyclerAlreadyUnloaded,
			ValidateAliasProofError::TemporarilyLocked => CustomInvalidity::AliasTemporarilyLocked,
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
			ValidateAliasProofError::TemporarilyLocked => Error::<T>::AliasTemporarilyLocked,
			ValidateAliasProofError::InvalidRevision => Error::<T>::InvalidRecyclerRevision,
		}
	}
}

/// Manager for handling recycler-related storage operations.
///
/// Recyclers are defined for a specific denomination, take coins as input, gather the member keys
/// in ring-VRF rings, and allow users to unload (given a valid unload token) new coins or assets as
/// output.
///
/// Ring-VRF ring management (members, pending members, ring building, member cleanup) is
/// delegated to [`Config::MemberService`] (pallet-members). This manager handles the
/// coinage-specific storage: collection creation tracking, member-to-value mapping, alias
/// double-spend tracking, and ring expiration/cleanup/archival.
///
/// # Lifecycle of a recycler
///
/// A ring-VRF collection is created for each denomination; a recycler is one ring of that
/// collection, and rings fill up sequentially.
///
/// Coins are loaded into the recycler (adding their member key to the ring) while, concurrently,
/// members already included in a built revision unload by proving ring membership, consuming
/// their one-time alias. Once at capacity the ring becomes immutable; unloading continues.
///
/// After being immutable for [`Config::RecyclerExpirationTime`] the ring expires and regular
/// unloads stop. The expired ring is then cleaned up: it is removed, and if not-yet-unloaded
/// coins remain it is archived in [`RecyclersArchives`]. The archive only contains commitments,
/// the recycler itself effectively goes off-trie. Members can still unload their coin from the
/// archive via [`Self::unload_archived`], by proving against those commitments.
pub struct RecyclerManager<T>(core::marker::PhantomData<T>);

impl<T: Config> RecyclerManager<T> {
	/// Get the index of the oldest ring that hasn't been cleaned yet.
	///
	/// Rings are cleaned sequentially starting from 0 because they fill up and become
	/// immutable in order, so older rings always expire first. Returns `None` only on
	/// `RingIndex` overflow (effectively unreachable).
	fn next_ring_to_clean(instance_id: InstanceId, value: Denomination) -> Option<RingIndex> {
		RecyclersLastRemovedRingIndex::<T>::get(instance_id, value)
			.map(|i| i.checked_add(1))
			.unwrap_or(Some(0))
	}

	/// Create the recycler collection for the given instance and denomination.
	pub(crate) fn create_collection(
		instance_id: InstanceId,
		value: Denomination,
	) -> DispatchResult {
		let identifier = Pallet::<T>::recycler_collection_identifier(instance_id, value);
		T::MemberService::create_collection(
			Pallet::<T>::recycler_collection_owner(instance_id),
			&identifier,
			pallet::RECYCLER_ONBOARDING_SIZE,
			RingMode::AppendOnly,
			T::RecyclerRingExponent::get(),
			None,
		)?;

		RecyclerCollectionCreated::<T>::insert(instance_id, value, ());
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
		instance_id: InstanceId,
		value: Denomination,
		index: RingIndex,
		revision: RevisionIndex,
	) -> bool {
		let identifier = Pallet::<T>::recycler_collection_identifier(instance_id, value);
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
	/// This validates the key and delegates to `MemberService::add_members()`.
	pub fn load(
		instance_id: InstanceId,
		value: Denomination,
		member_key: MemberOf<T>,
	) -> Result<(), RecyclerLoadError> {
		if Self::is_member_key_used(&member_key) {
			return Err(RecyclerLoadError::MemberKeyAlreadyUsed);
		}

		if !CryptoOf::<T>::is_member_valid(&member_key) {
			return Err(RecyclerLoadError::InvalidMemberKey);
		}

		let identifier = Pallet::<T>::recycler_collection_identifier(instance_id, value);
		T::MemberService::add_members(&identifier, vec![member_key.clone()])
			.map_err(|_| RecyclerLoadError::InternalError)?;

		RecyclersCoinToRecycler::<T>::insert(member_key, (instance_id, value));

		Ok(())
	}

	/// Push multiple member keys into recycler collections, grouped by denomination.
	///
	/// Duplicate and invalid keys are rejected before any mutation. Member-service failures are
	/// handled transactionally here so grouped recycler bookkeeping remains consistent.
	pub fn load_batch_grouped(
		instance_id: InstanceId,
		loaded_coins: &[(Denomination, MemberOf<T>)],
	) -> Result<(), RecyclerLoadError> {
		let mut seen = BTreeSet::new();
		let mut grouped: BTreeMap<Denomination, Vec<MemberOf<T>>> = BTreeMap::new();

		for (value, member_key) in loaded_coins {
			if !seen.insert(member_key.encode()) || Self::is_member_key_used(member_key) {
				return Err(RecyclerLoadError::MemberKeyAlreadyUsed);
			}

			if !CryptoOf::<T>::is_member_valid(member_key) {
				return Err(RecyclerLoadError::InvalidMemberKey);
			}

			grouped.entry(*value).or_default().push(member_key.clone());
		}

		for (value, members) in grouped {
			let identifier = Pallet::<T>::recycler_collection_identifier(instance_id, value);
			for member_key in &members {
				RecyclersCoinToRecycler::<T>::insert(member_key.clone(), (instance_id, value));
			}
			T::MemberService::add_members(&identifier, members)
				.map_err(|_| RecyclerLoadError::InternalError)?;
		}

		Ok(())
	}

	/// Unload tokens from a recycler.
	///
	/// Rejects duplicate aliases before verifying all proofs in one batch via
	/// `MemberService::verify_memberships_in_ring()`. It then sequentially checks expected aliases,
	/// already-unloaded status, and marks each alias.
	pub fn unload(
		instance_id: InstanceId,
		value: Denomination,
		index: RingIndex,
		revision: RevisionIndex,
		aliases: &[Alias],
		alias_proofs: &[ProofOf<T>],
		proven_msg: &[u8; 32],
	) -> Result<(), DispatchError> {
		ensure!(aliases.len() == alias_proofs.len(), Error::<T>::ProofAndAliasMismatch);
		ensure!(
			Self::validate_recycler_revision(instance_id, value, index, revision),
			Error::<T>::InvalidRecyclerRevision
		);
		let mut seen_aliases = BTreeSet::new();
		for alias in aliases {
			ensure!(seen_aliases.insert(*alias), Error::<T>::RecyclerAlreadyUnloaded);
		}

		let identifier = Pallet::<T>::recycler_collection_identifier(instance_id, value);

		// Build batch items: all proofs share the same message and context for a recycler input.
		let items = alias_proofs
			.iter()
			.map(|proof| RingMembershipProof {
				proof: proof.clone(),
				message: proven_msg.to_vec(),
				context: UNLOADING_RECYCLER_CONTEXT.to_vec(),
			})
			.collect::<Vec<_>>();

		// One batch verification call instead of per-proof verification.
		let results =
			T::MemberService::verify_memberships_in_ring(&identifier, index, revision, &items)
				.map_err(|_| Error::<T>::InvalidAliasProof)?;

		ensure!(results.len() == aliases.len(), Error::<T>::ProofAndAliasMismatch);

		// Sequential post-processing: alias matching, already-unloaded checks, marking.
		for (result, expected_alias) in results.iter().zip(aliases.iter()) {
			let alias = result.alias;
			ensure!(alias == *expected_alias, Error::<T>::ProofAndAliasMismatch);
			Self::ensure_alias_available(instance_id, value, index, alias)
				.map_err(ValidateAliasProofError::into_pallet_error::<T>)?;
			Self::mark_alias_unloaded(instance_id, value, index, alias);
			Pallet::<T>::deposit_event(Event::RecyclerAliasUnloaded {
				instance_id,
				value,
				ring_index: index,
				alias,
			});
		}

		Ok(())
	}

	/// Validates a single alias proof without marking it as unloaded.
	///
	/// Delegates proof verification to `T::MemberService::verify_membership` and
	/// checks that the alias has not already been unloaded from this ring.
	///
	/// Returns the validated alias on success.
	pub fn validate_alias_proof(
		instance_id: InstanceId,
		value: Denomination,
		index: RingIndex,
		revision: RevisionIndex,
		alias_proof: &ProofOf<T>,
		proven_msg: &[u8; 32],
	) -> Result<Alias, ValidateAliasProofError> {
		// Check that the ring revision matches.
		if !Self::validate_recycler_revision(instance_id, value, index, revision) {
			return Err(ValidateAliasProofError::InvalidRevision);
		}

		let identifier = Pallet::<T>::recycler_collection_identifier(instance_id, value);

		let result = T::MemberService::verify_membership(
			&identifier,
			alias_proof,
			index,
			revision,
			UNLOADING_RECYCLER_CONTEXT,
			proven_msg.as_ref(),
		)
		.map_err(|_| ValidateAliasProofError::InvalidProof)?;

		let alias = result.alias;

		Self::ensure_alias_available(instance_id, value, index, alias)?;

		Ok(alias)
	}

	/// Marks a single alias as unloaded and counts it in [`RecyclersUnloadedCount`].
	/// Should only be called after successful validation via `validate_alias_proof`.
	///
	/// Overwrites any existing temporary lock for the alias because a successful unload consumes
	/// it permanently. Use [`Self::mark_unloaded_alias_locked`] to undo the mark.
	///
	/// Does not emit [`Event::RecyclerAliasUnloaded`]: the caller must emit it if (and only if) the
	/// mark persists. In particular the transaction-extension premark is reverted in
	/// `post_dispatch` on dispatch failure, so it only emits the event on success.
	pub fn mark_alias_unloaded(
		instance_id: InstanceId,
		value: Denomination,
		index: RingIndex,
		alias: Alias,
	) {
		// Read before the write below, which would make the ring look used. Every alias state of a
		// ring is written through this function, so a ring that has state but no count is one that
		// predates the count: it stays uncounted, since only a scan could recover its number.
		let count = Self::unloaded_count(instance_id, value, index);

		let previous =
			RecyclerAliasStates::<T>::mutate((instance_id, value, index, alias), |state| {
				state.replace(AliasState::Unloaded)
			});

		if let Some(count) = count.filter(|_| previous != Some(AliasState::Unloaded)) {
			RecyclersUnloadedCount::<T>::insert(
				(instance_id, value, index),
				count.saturating_add(1),
			);
		}
	}

	/// Turns the unloaded mark of an alias into a temporary lock, and takes it out of
	/// [`RecyclersUnloadedCount`] again.
	///
	/// This undoes [`Self::mark_alias_unloaded`] for a premark whose dispatch failed, leaving the
	/// alias unloadable again once `lock` expires. The alias must still carry the premark: any
	/// other state means the caller marked and locked out of step.
	pub fn mark_unloaded_alias_locked(
		instance_id: InstanceId,
		value: Denomination,
		index: RingIndex,
		alias: Alias,
		lock: LockInfo,
	) {
		let previous =
			RecyclerAliasStates::<T>::mutate((instance_id, value, index, alias), |state| {
				state.replace(AliasState::Locked(lock))
			});

		if previous != Some(AliasState::Unloaded) {
			defensive!("coinage: locked an alias that was not marked unloaded", previous);
			return;
		}

		RecyclersUnloadedCount::<T>::mutate((instance_id, value, index), |unloaded| {
			if let Some(count) = unloaded {
				*count = count.saturating_sub(1);
			}
		});
	}

	/// The number of aliases unloaded from a recycler ring, or `None` if the ring is not counted.
	///
	/// A ring that already had alias states when [`RecyclersUnloadedCount`] was introduced is never
	/// counted, so a caller that needs its number has to scan [`RecyclerAliasStates`] over the
	/// ring's prefix. A ring with no alias state at all has had no unload, so it reports zero
	/// without ever having been written to.
	pub fn unloaded_count(
		instance_id: InstanceId,
		value: Denomination,
		index: RingIndex,
	) -> Option<u32> {
		RecyclersUnloadedCount::<T>::get((instance_id, value, index))
			.or_else(|| (!Self::has_alias_states(instance_id, value, index)).then_some(0))
	}

	fn has_alias_states(instance_id: InstanceId, value: Denomination, index: RingIndex) -> bool {
		RecyclerAliasStates::<T>::iter_prefix((instance_id, value, index))
			.next()
			.is_some()
	}

	/// Checks if the next recycler ring (by sequential index) can be cleaned.
	///
	/// A ring is eligible for cleanup when it has been immutable for longer than
	/// [`Config::RecyclerExpirationTime`].
	///
	/// Returns the transaction validity info on success.
	pub fn ensure_can_clean(
		instance_id: InstanceId,
		value: Denomination,
	) -> Result<(ValidTransaction, Weight), TransactionValidityError> {
		let identifier = Pallet::<T>::recycler_collection_identifier(instance_id, value);
		let next_ring =
			Self::next_ring_to_clean(instance_id, value).ok_or(InvalidTransaction::Stale)?;

		let status = T::MemberService::ring_status(&identifier, next_ring)
			.ok_or(InvalidTransaction::Future)?;

		let immutable_since = status.immutable_since.ok_or(InvalidTransaction::Future)? as u32;
		let now = T::UnixTime::now().as_secs() as u32;
		let expiration = immutable_since.saturating_add(T::RecyclerExpirationTime::get());
		if now < expiration {
			return Err(InvalidTransaction::Future.into());
		}

		let validity = ValidTransaction::with_tag_prefix("coinage:remove-recycler")
			.and_provides((instance_id, value, next_ring))
			.priority(tx_priority::CLEANUP)
			.into();
		Ok((validity, Weight::zero()))
	}

	/// Builds the root of a trie committing to a set of unloaded aliases.
	///
	/// Each alias is inserted as a key with an empty value into a `LayoutV1<BlakeTwo256>` trie.
	/// The resulting root is independent of insertion order (sorted Patricia–Merkle trie), so the
	/// arbitrary storage iteration order of the aliases does not affect it.
	pub(crate) fn unloaded_aliases_root(aliases: &[Alias]) -> Result<H256, DispatchError> {
		use sp_trie::{LayoutV1, TrieDBMutBuilder, TrieMut};
		type Layout = LayoutV1<BlakeTwo256>;

		// An empty proof yields an empty `MemoryDB`. This mirrors `MemoryDB::default()` but goes
		// through `into_memory_db`, whose `Default` bound resolves in the no_std runtime build.
		let mut db = StorageProof::empty().into_memory_db::<BlakeTwo256>();
		let mut root = H256::default();
		{
			let mut trie = TrieDBMutBuilder::<Layout>::new(&mut db, &mut root).build();
			for alias in aliases {
				trie.insert(&alias[..], &[]).map_err(|_| Error::<T>::InternalError)?;
			}
		}
		Ok(root)
	}

	/// Verifies a recovery from an archived recycler ring and updates its commitment.
	///
	/// `recycler_root` and `unloaded_root` are supplied by the caller (untrusted) and validated
	/// against the stored commitment `blake2_256(unloaded_root ++ recycler_root)`. Then:
	/// - `alias_proof` is verified against `recycler_root` (the deleted ring's root), yielding the
	///   caller's `alias` — only an original ring member can produce a valid proof;
	/// - `non_inclusion_proof` proves `alias` is absent from `unloaded_root` — i.e. it was never
	///   unloaded.
	///
	/// On success the recovered `alias` is inserted into the unloaded-aliases trie (proof-only
	/// recompute), the stored commitment is updated to the new root, and `remaining` is decremented
	/// (the entry is removed once it reaches zero). This makes a second recovery of the same alias
	/// impossible: the new commitment requires the new root, against which no non-inclusion proof
	/// for `alias` exists.
	///
	/// `proven_msg` must be the message the `alias_proof` was created over (binding the signer).
	pub fn unload_archived(
		instance_id: InstanceId,
		value: Denomination,
		index: RingIndex,
		recycler_root: &MembersOf<T>,
		unloaded_root: H256,
		alias_proof: &ProofOf<T>,
		non_inclusion_proof: impl IntoIterator<Item = Vec<u8>>,
		proven_msg: &[u8; 32],
	) -> Result<Alias, DispatchError> {
		type Layout = LayoutV1<BlakeTwo256>;

		let mut info = RecyclersArchives::<T>::get((instance_id, value, index))
			.ok_or(Error::<T>::ArchivedRecyclerNotFound)?;

		// Bind the supplied roots to the stored commitment. Also checked at transaction
		// validation.
		let commitment = archive_commitment(unloaded_root, recycler_root);
		ensure!(commitment == info.commitment, Error::<T>::InvalidArchivedRoots);

		// Verify ring membership against the supplied (deleted) ring root.
		let config =
			<CryptoOf<T> as GenerateVerifiable>::Config::try_from(T::RecyclerRingExponent::get())
				.map_err(|_| Error::<T>::InvalidRingExponent)?;
		let alias = CryptoOf::<T>::validate(
			config,
			alias_proof,
			recycler_root,
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
			proven_msg,
		)
		.map_err(|_| Error::<T>::InvalidAliasProof)?;

		// Reconstruct the partial unloaded-aliases trie from the caller-supplied proof. The same DB
		// backs both the non-inclusion check and the root recomputation below, so a single proof
		// (recorded over the insert of `alias`) is sufficient for both.
		let mut db = StorageProof::new(non_inclusion_proof).into_memory_db::<BlakeTwo256>();

		// Prove the alias was never unloaded: it must be absent from the committed trie. This is
		// checked with a lookup on the proof-backed DB rather than `verify_trie_proof`, because the
		// strict proof verifier rejects (`ExtraneousHashReference`) the extra nodes that the
		// `delta_trie_root` insert below legitimately requires. A lookup that returns `Ok(None)`
		// proves absence with all path nodes present; a missing node yields `Err` and is rejected.
		{
			let trie = TrieDBBuilder::<Layout>::new(&db, &unloaded_root).build();
			ensure!(
				matches!(trie.get(&alias[..]), Ok(None)),
				Error::<T>::AliasWasUnloadedOrInvalidProof,
			);
		}

		// Recompute the unloaded-aliases root after inserting the recovered alias, using only the
		// proof nodes (which cover the insertion path for this key).
		let new_unloaded_root = delta_trie_root::<Layout, _, _, _, _, _>(
			&mut db,
			unloaded_root,
			[(alias.to_vec(), Some(Vec::new()))],
			None,
			None,
		)
		.map_err(|_| Error::<T>::AliasWasUnloadedOrInvalidProof)?;

		// Update the commitment and the recoverable-coin count, removing the archive when drained.
		info.remaining = info.remaining.saturating_sub(1);
		info.commitment = archive_commitment(new_unloaded_root, recycler_root);
		if info.remaining == 0 {
			RecyclersArchives::<T>::remove((instance_id, value, index));
		} else {
			RecyclersArchives::<T>::insert((instance_id, value, index), info);
		}

		Ok(alias)
	}

	/// Cleans an expired recycler ring.
	///
	/// Delegates ring removal to `MemberService::remove_ring()`.
	///
	/// If the ring still has at least one not-yet-unloaded alias (i.e. recoverable coins remain),
	/// an archival commitment `blake2_256(unloaded_aliases_root ++ recycler_root)` is recorded in
	/// [`RecyclersArchives`] together with the number of recoverable coins. This is captured
	/// *before* `remove_ring()` so the ring-VRF root is still readable. The archived value is not
	/// destroyed: its backing asset stays held in the pallet account and can be recovered via
	/// [`Self::unload_archived`].
	///
	/// Returns `(remaining_coins, member_count, archived)` where `archived` is
	/// `Some((ring_index, recycler_root))` when an archival commitment was recorded; the ring-VRF
	/// `recycler_root` is passed back so the caller can emit it (the ring itself is removed here,
	/// so the event is its last on-chain source).
	pub fn clean_unchecked(
		instance_id: InstanceId,
		value: Denomination,
	) -> Result<(u32, u32, Option<(RingIndex, MembersOf<T>)>), DispatchError> {
		let identifier = Pallet::<T>::recycler_collection_identifier(instance_id, value);
		let next_ring =
			Self::next_ring_to_clean(instance_id, value).ok_or(Error::<T>::InternalError)?;

		let status = T::MemberService::ring_status(&identifier, next_ring)
			.ok_or(Error::<T>::InternalError)?;

		// Remove the member-to-coin-value reverse mapping for all members in this ring.
		let members = T::MemberService::ring_members(&identifier, next_ring);
		let member_count = members.len() as u32;
		for member in members {
			RecyclersCoinToRecycler::<T>::remove(&member);
		}

		// Collect the ring's unloaded aliases in a single pass (drives both archival and dusting).
		// For this 4-key NMap, iterating with a 3-key prefix yields the trailing `Alias` key.
		let unloaded_aliases =
			RecyclerAliasStates::<T>::iter_prefix((instance_id, value, next_ring))
				.filter(|(_, state)| matches!(state, AliasState::Unloaded))
				.map(|(alias, _)| alias)
				.collect::<Vec<Alias>>();
		let unloaded_count = unloaded_aliases.len() as u32;
		let remaining = status.total.saturating_sub(unloaded_count);

		// The ring goes away here, so drop its unloaded count. The scan above is the ground truth
		// for `remaining`; the count only serves readers, and a mismatch on a counted ring means
		// the two went out of step somewhere in the unload paths.
		let counted = RecyclersUnloadedCount::<T>::take((instance_id, value, next_ring));
		if counted.is_some_and(|counted| counted != unloaded_count) {
			defensive!(
				"coinage: unloaded count does not match the ring's unloaded aliases",
				(counted, unloaded_count)
			);
		}

		// Archive the recycler before removing the ring if it still has not-yet-unloaded coins, so
		// a member can later recover their value via `unload_archived`. Commit to the
		// unloaded-alias set together with the ring-VRF root, both still readable until
		// `remove_ring()`.
		let mut archived = None;
		if remaining > 0 {
			// The ring-VRF root of this ring (the "recycler root").
			let recycler_root = Pallet::<T>::recycler_ring_root(instance_id, value, next_ring)
				.ok_or(Error::<T>::InternalError)?;

			let unloaded_root = Self::unloaded_aliases_root(&unloaded_aliases)?;
			let commitment = archive_commitment(unloaded_root, &recycler_root);

			RecyclersArchives::<T>::insert(
				(instance_id, value, next_ring),
				ArchivedRecycler { commitment, remaining },
			);
			archived = Some((next_ring, recycler_root));
		}

		// Queue any deferred ring dust for bounded cleanup.
		if Self::has_alias_states(instance_id, value, next_ring) {
			pallet::RecyclersDusting::<T>::insert((instance_id, value, next_ring), ());
		}

		// Remove the ring via MemberService.
		T::MemberService::remove_ring(&identifier, next_ring)?;

		// Update last removed index.
		RecyclersLastRemovedRingIndex::<T>::insert(instance_id, value, next_ring);

		// Return the count of remaining (non-unloaded) coins, the member count, and the archived
		// root (if any).
		Ok((remaining, member_count, archived))
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
			.priority(tx_priority::CLEANUP)
			.into();
		Ok((validity, Weight::zero()))
	}

	/// Cleans recycler dust.
	///
	/// Removes up to DUST_CLEANUP_BATCH_SIZE entries from [`RecyclerAliasStates`] per call to
	/// bound the operation.
	pub fn clean_dust_unchecked() -> u32 {
		if let Some((instance_id, value, ring_index)) =
			pallet::RecyclersDusting::<T>::iter_keys().next()
		{
			let cleared = RecyclerAliasStates::<T>::clear_prefix(
				(instance_id, value, ring_index),
				pallet::DUST_CLEANUP_BATCH_SIZE,
				None,
			);
			if cleared.maybe_cursor.is_none() {
				pallet::RecyclersDusting::<T>::remove((instance_id, value, ring_index));
			}
			cleared.unique
		} else {
			0
		}
	}

	fn ensure_alias_available(
		instance_id: InstanceId,
		value: Denomination,
		index: RingIndex,
		alias: Alias,
	) -> Result<(), ValidateAliasProofError> {
		match RecyclerAliasStates::<T>::get((instance_id, value, index, alias)) {
			Some(AliasState::Unloaded) => Err(ValidateAliasProofError::AlreadyUnloaded),
			Some(AliasState::Locked(locked)) if T::UnixTime::now().as_secs() < locked.until =>
				Err(ValidateAliasProofError::TemporarilyLocked),
			Some(AliasState::Locked(_)) | None => Ok(()),
		}
	}
}
