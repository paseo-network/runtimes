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

//! # Members Subscriber Pallet
//!
//! This pallet receives ring root updates from the notifier
//! (via the members-notifier pallet)
//! and stores them locally to enable personhood verification on subscriber chains.
//!
//! ## Subscription Lifecycle
//!
//! 1. Subscription starts with a governance call to `subscribe` on notifier. The call parameters
//!    specify subscriber parachain id.
//! 2. Notifier sends initial ring roots via XCM Transact that call `initialize_ring_roots` on the
//!    chosen subscriber.
//! 3. Ring root updates (new/updated/deleted rings) are sent periodically from the notifier and
//!    received by the subscriber via `process_ring_updates`.
//! 4. Subscription ends via a call to `terminate_subscription` on the subscriber that also ends the
//!    subscription on the notifier side.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::{collections::BTreeSet, vec, vec::Vec};

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
pub mod types;
pub mod weights;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub use pallet::*;
pub use types::*;
pub use weights::WeightInfo;

use verifiable::GenerateVerifiable;

#[frame_support::pallet]
pub mod pallet {

	use super::*;
	use frame_support::{
		pallet_prelude::*,
		traits::{Defensive, EnsureOrigin, UnixTime},
	};
	use frame_system::{
		offchain::{CreateAuthorizedTransaction, SubmitTransaction},
		pallet_prelude::*,
	};
	use indiv_support::traits::{
		BatchProofItem, Context, ContextualAlias, MembershipMultiProver, MembershipProver,
		RevisedContextualAlias, RingExponent,
	};
	use xcm::v5::{Location, SendXcm};

	const LOG_TARGET: &str = "pallet-members-subscriber";

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + CreateAuthorizedTransaction<Call<Self>> {
		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;

		/// Cryptographic implementation for ring membership verification.
		type Crypto: GenerateVerifiable<
			Member: DecodeWithMemTracking,
			Members: DecodeWithMemTracking + verifiable::DecodeUnchecked,
			Proof: Send + Sync + DecodeWithMemTracking,
			Signature: Send + Sync + DecodeWithMemTracking,
			Config: TryFrom<RingExponent>,
		>;

		/// XCM sender for communicating with the notifier.
		type XcmSender: SendXcm;

		/// Endpoint configuration for the notifier.
		/// Includes the XCM location and pallet index.
		#[pallet::constant]
		type RingRootsNotifier: Get<NotifierEndpoint>;

		/// This chain's parachain ID, included in replay requests to the notifier.
		#[pallet::constant]
		type SelfParaId: Get<u32>;

		/// Maximum number of missing ring roots per collection.
		#[pallet::constant]
		type MaxMissingRootsPerCollection: Get<u32>;

		/// Maximum number of deleted ring indices tracked per collection.
		/// Prevents falsely marking deleted rings as missing.
		#[pallet::constant]
		type MaxDeletedRingsPerCollection: Get<u32>;

		/// Maximum number of ring roots per collection.
		/// Used to bound `clear` operations during re-initialization.
		#[pallet::constant]
		type MaxRingRootsPerCollection: Get<u32>;

		// TODO: look for a way to check for XCM origin
		// as per this comment: https://github.com/paritytech/individuality/pull/523#discussion_r2720568424
		/// Origin check for XCM messages from the notifier.
		type EnsureNotifierOrigin: EnsureOrigin<Self::RuntimeOrigin>;

		/// Origin authorized to terminate the subscription (root or governance).
		type EnsureTerminationOrigin: EnsureOrigin<Self::RuntimeOrigin>;

		/// Maximum number of ring collections that can be tracked.
		#[pallet::constant]
		type MaxCollections: Get<u32>;

		/// Unix time source for timestamps.
		type UnixTime: UnixTime;

		/// Cooldown period (in seconds) after receiving a batch before sending replay requests.
		/// Allows time for multi-part batches to arrive via XCM.
		#[pallet::constant]
		type ReplayCooldownSeconds: Get<u64>;

		/// Maximum number of ring root updates that can be processed in a single batch.
		///
		/// **Important:** This value MUST be greater than or equal to the notifier's
		/// `MaxUpdatesPerBatch`. If the notifier sends batches larger
		/// than this limit, decoding will fail silently and updates will be lost.
		#[pallet::constant]
		type MaxUpdatesPerBatch: Get<u32>;

		/// Number of replay request attempts for a given ring index before emitting warning events.
		#[pallet::constant]
		type ReplayWarningThreshold: Get<u32>;

		/// Threshold for abandoning replay attempts for a given ring index.
		/// After this many attempts, the missing index is removed from tracking.
		#[pallet::constant]
		type ReplayAbandonThreshold: Get<u32>;

		/// Maximum number of recent ring roots stored per (collection, ring_index).
		/// When a ring root is updated, the new root is pushed into a sliding window;
		/// proofs built against any root in the window still verify. Oldest root is
		/// evicted when the window is full.
		#[pallet::constant]
		type MaxRecentRootsPerRing: Get<u32>;

		/// Block interval between offchain worker executions.
		#[pallet::constant]
		type OffchainWorkerInterval: Get<BlockNumberFor<Self>>;
	}

	// ========== Storage Items ==========

	/// Recent ring roots received from the notifier.
	/// Maps (identifier, ring_index) to a sliding window of ring roots.
	/// Proofs built against any root in the window are accepted. The oldest root
	/// is evicted when the window reaches `MaxRecentRootsPerRing`.
	#[pallet::storage]
	pub type RingRoots<T: Config> = StorageDoubleMap<
		_,
		Blake2_128Concat,
		Identifier,
		Blake2_128Concat,
		RingIndex,
		BoundedVec<RingCommitmentRecord<T>, T::MaxRecentRootsPerRing>,
		OptionQuery,
	>;

	/// State of each ring collection including rings count and missing rings tracking.
	#[pallet::storage]
	pub type RingCollectionStates<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		Identifier,
		RingCollectionState<T::MaxMissingRootsPerCollection, T::MaxDeletedRingsPerCollection>,
		ValueQuery,
	>;

	/// Ring exponent recorded per collection at initialization.
	///
	/// The subscriber needs the exponent to derive the crypto's `Config` when verifying
	/// proofs, since ring roots on their own do not carry that information. The entry is
	/// written by `initialize_ring_roots` and is read during proof verification via the
	/// `MembershipProver` impl.
	#[pallet::storage]
	pub type RingCollectionExponents<T: Config> =
		StorageMap<_, Blake2_128Concat, Identifier, RingExponent>;

	/// Current subscription status.
	#[pallet::storage]
	pub type Subscription<T: Config> = StorageValue<_, SubscriptionStatus, ValueQuery>;

	/// State for tracking updates processing timestamps and sequence numbers.
	#[pallet::storage]
	pub type ProcessingState<T: Config> = StorageValue<_, UpdatesProcessingState, ValueQuery>;

	// ========== Events ==========

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// Ring roots have been initialized from the notifier.
		RingRootsInitialized {
			/// Number of ring roots initialized.
			count: u32,
			/// Sequence number of the initialization batch.
			sequence: SequenceNumber,
		},
		/// Ring root updates have been processed.
		RingRootsUpdated {
			/// Number of updates processed.
			count: u32,
			/// Sequence number of the batch.
			sequence: SequenceNumber,
		},
		/// Subscription has been terminated.
		SubscriptionTerminated {
			/// Whether the unsubscribe XCM was successfully queued.
			notifier_notified: bool,
		},
		/// New missing ring roots detected during batch processing.
		MissingRingsDetected {
			/// Ring collection identifier.
			identifier: Identifier,
			/// Number of newly detected missing ring indices.
			count: u32,
		},
		/// Replay request successfully sent to notifier.
		ReplayRequestSent {
			/// Ring collection identifier.
			identifier: Identifier,
			/// Number of missing indices in this chunk.
			indices_count: u32,
		},
		/// Missing ring scan skipped because deleted_indices reached capacity.
		DeletedIndicesAtCapacity {
			/// Ring collection identifier.
			identifier: Identifier,
		},
	}

	// ========== Errors ==========

	#[pallet::error]
	pub enum Error<T> {
		/// XCM message send failed.
		XcmSendFailed,
		/// Subscription is currently inactive.
		SubscriptionInactive,
		/// Subscription is in terminated state.
		SubscriptionTerminated,
		/// Subscription is active with a different sequence; must be terminated first.
		SubscriptionAlreadyActive,
		/// Collection with the given identifier is not tracked.
		CollectionNotFound,
		/// No ring root stored for the given ring index.
		NoRoot,
		/// Proof failed to verify against any stored ring root.
		InvalidProof,
		/// Stored ring exponent does not convert into the crypto's capacity.
		InvalidRingExponent,
		/// Requested revision is not present in the stored sliding window.
		RevisionNotFound,
	}

	// ========== Hooks ==========

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn offchain_worker(block_number: BlockNumberFor<T>) {
			// Limiting execution frequency
			if !(block_number % T::OffchainWorkerInterval::get()).is_zero() {
				return;
			}

			if !matches!(Subscription::<T>::get(), SubscriptionStatus::Active { .. }) {
				return;
			}

			let now = T::UnixTime::now().as_secs();
			let cooldown = T::ReplayCooldownSeconds::get();
			let processing_state = ProcessingState::<T>::get();

			// Cooldown 1: Giving some time for multi-part batches to arrive
			if now.saturating_sub(processing_state.last_batch_received_time) < cooldown {
				return;
			}

			// Cooldown 2: So as not to send replay requests too frequently
			if now.saturating_sub(processing_state.last_replay_request_time) < cooldown {
				return;
			}

			// Sending replay transaction for each collection with missing indices
			for (identifier, state) in RingCollectionStates::<T>::iter() {
				if state.missing_indices.is_empty() {
					continue;
				}

				if state.deleted_indices.len() as u32 == T::MaxDeletedRingsPerCollection::get() {
					log::warn!(
						target: LOG_TARGET,
						"Skipped replay: deleted_indices at capacity for collection {identifier:?}",
					);
					continue;
				}

				let indices: BoundedVec<_, T::MaxMissingRootsPerCollection> =
					BoundedVec::truncate_from(state.missing_indices.keys().copied().collect());

				let call = Call::replay_missing_roots { identifier, indices };
				Self::submit_authorized_transaction(call, "Replay Missing Roots");
			}
		}

		fn integrity_test() {
			assert_ne!(
				T::RingRootsNotifier::get().location,
				Location::here(),
				"RingRootsNotifier location cannot be Here"
			);

			assert!(
				T::MaxMissingRootsPerCollection::get() > 0,
				"MaxMissingRootsPerCollection must be greater than 0"
			);

			assert!(
				T::ReplayCooldownSeconds::get() > 0,
				"ReplayCooldownSeconds must be greater than 0"
			);

			assert!(T::MaxUpdatesPerBatch::get() > 0, "MaxUpdatesPerBatch must be greater than 0");

			assert!(T::MaxCollections::get() > 0, "MaxCollections must be greater than 0");

			assert!(
				T::MaxDeletedRingsPerCollection::get() > 0,
				"MaxDeletedRingsPerCollection must be greater than 0"
			);

			assert!(
				T::MaxRingRootsPerCollection::get() > 0,
				"MaxRingRootsPerCollection must be greater than 0"
			);

			assert!(
				T::ReplayAbandonThreshold::get() > T::ReplayWarningThreshold::get(),
				"ReplayAbandonThreshold must be greater than ReplayWarningThreshold"
			);

			assert!(
				T::MaxRecentRootsPerRing::get() > 0,
				"MaxRecentRootsPerRing must be greater than 0"
			);

			assert!(
				T::OffchainWorkerInterval::get() > 0u32.into(),
				"OffchainWorkerInterval must be greater than 0"
			);

			// `replay_missing_roots` is submitted by offchain worker as an authorized transaction.
			// If weight exceeds Normal.max_extrinsic, it is silently dropped and the
			// replay flow stalls.
			let block_weights = <T as frame_system::Config>::BlockWeights::get();
			let normal_max = block_weights
				.per_class
				.get(DispatchClass::Normal)
				.max_extrinsic
				.expect("Normal class must have max_extrinsic configured");

			let replay_weight = Self::replay_missing_roots_worst_case_weight().saturating_add(
				T::WeightInfo::authorize_replay_missing_roots(
					T::MaxMissingRootsPerCollection::get(),
				),
			);
			assert!(
				replay_weight.all_lte(normal_max),
				"`replay_missing_roots` worst-case weight {replay_weight:?} exceeds \
				 Normal max_extrinsic {normal_max:?} — lower MaxMissingRootsPerCollection",
			);
		}
	}

	// ========== Extrinsics ==========

	#[pallet::call(weight = <T as Config>::WeightInfo)]
	impl<T: Config> Pallet<T> {
		/// Stores the initial ring roots received from the notifier upon subscription start.
		/// Accepts multi-part continuations (same sequence). Rejects calls with a different
		/// sequence while subscription is `Active` — a subscription must be terminated first.
		/// Can only be called by notifier XCM origin (via `EnsureNotifierOrigin`).
		///
		/// ## Parameters
		/// - `origin`: Notifier XCM origin.
		/// - `roots`: Initial batch of ring roots.
		#[pallet::call_index(0)]
		#[pallet::weight(
			T::WeightInfo::initialize_ring_roots(roots.updates.len() as u32)
				.saturating_add(T::WeightInfo::detect_missing_in_range(roots.next_ring_index))
				// Worst case when re-init from Active state clears all existing ring data.
				// Using terminate_subscription() as upper bound for clear_all_ring_data().
				.saturating_add(T::WeightInfo::terminate_subscription())
		)]
		pub fn initialize_ring_roots(
			origin: OriginFor<T>,
			ring_exponent: RingExponent,
			roots: RingRootUpdatesBatch<T>,
		) -> DispatchResultWithPostInfo {
			T::EnsureNotifierOrigin::ensure_origin(origin)?;

			// Rejecting stale re-init attempts (lower sequence).
			// Higher sequence triggers re-initialization from the notifier.
			// Same sequence continues multi-part initialization.
			let mut re_init = false;
			if let SubscriptionStatus::Active { initialized_at_sequence } = Subscription::<T>::get()
			{
				if roots.sequence < initialized_at_sequence {
					return Err(Error::<T>::SubscriptionAlreadyActive.into());
				}
				if roots.sequence > initialized_at_sequence {
					// Re-initialization from notifier — clearing stale ring data
					Self::clear_all_ring_data();
					re_init = true;
				}
			}

			let old_next_ring_index =
				RingCollectionStates::<T>::get(roots.identifier).next_ring_index;

			Self::store_ring_roots(&roots);
			Self::detect_missing_rings_in_batch(&roots, old_next_ring_index);
			Self::record_batch_processed(roots.sequence);

			Subscription::<T>::put(SubscriptionStatus::Active {
				initialized_at_sequence: roots.sequence,
			});
			RingCollectionExponents::<T>::insert(roots.identifier, ring_exponent);

			let count = roots.updates.len() as u32;
			Self::deposit_event(Event::RingRootsInitialized { count, sequence: roots.sequence });

			// Refunding overcharged weight
			let delta = roots.next_ring_index.saturating_sub(old_next_ring_index);
			let mut actual_weight = T::WeightInfo::initialize_ring_roots(count)
				.saturating_add(T::WeightInfo::detect_missing_in_range(delta));
			if re_init {
				actual_weight =
					actual_weight.saturating_add(T::WeightInfo::terminate_subscription());
			}
			Ok(Some(actual_weight).into())
		}

		/// Process ring roots updates received from the notifier.
		///
		/// ## Parameters
		/// - `origin`: Must be the XCM origin from the notifier.
		/// - `batch`: Batch of ring root updates to process.
		#[pallet::call_index(1)]
		#[pallet::weight(
			T::WeightInfo::process_ring_updates(batch.updates.len() as u32)
				.saturating_add(T::WeightInfo::detect_missing_in_range(batch.next_ring_index))
		)]
		pub fn process_ring_updates(
			origin: OriginFor<T>,
			batch: RingRootUpdatesBatch<T>,
		) -> DispatchResultWithPostInfo {
			T::EnsureNotifierOrigin::ensure_origin(origin)?;

			Self::ensure_subscription_active()?;

			// To ignore old batches but to allow replays (sequence equal to the last one).
			// Refunding unused weight since no processing occurs.
			if batch.sequence < ProcessingState::<T>::get().last_processed_sequence {
				return Ok(Some(T::WeightInfo::process_ring_updates_stale_batch()).into());
			}

			let old_next_ring_index =
				RingCollectionStates::<T>::get(batch.identifier).next_ring_index;

			Self::store_ring_roots(&batch);
			Self::detect_missing_rings_in_batch(&batch, old_next_ring_index);
			Self::record_batch_processed(batch.sequence);

			let count = batch.updates.len() as u32;
			Self::deposit_event(Event::RingRootsUpdated { count, sequence: batch.sequence });

			// Refunding overcharged detect_missing_rings_in_batch weight: annotation uses
			// next_ring_index but actual work is proportional to the delta
			let delta = batch.next_ring_index.saturating_sub(old_next_ring_index);
			let actual_weight = T::WeightInfo::process_ring_updates(count)
				.saturating_add(T::WeightInfo::detect_missing_in_range(delta));
			Ok(Some(actual_weight).into())
		}

		/// Terminates the subscription.
		///
		/// Accepts either notifier origin (`EnsureNotifierOrigin`) or local governance
		/// origin (`EnsureTerminationOrigin`). When called locally, sends an XCM
		/// unsubscribe message to the notifier. When called from the notifier (e.g. on
		/// governance unsubscribe), no XCM is sent back. Idempotent if already
		/// terminated.
		///
		/// ## Parameters
		/// - `origin`: Notifier XCM origin or root/governance origin.
		#[pallet::call_index(2)]
		#[pallet::weight(T::WeightInfo::terminate_subscription())]
		pub fn terminate_subscription(origin: OriginFor<T>) -> DispatchResult {
			let from_notifier = match T::EnsureNotifierOrigin::try_origin(origin) {
				Ok(_) => true,
				Err(origin) => {
					T::EnsureTerminationOrigin::ensure_origin(origin)?;
					false
				},
			};

			match Subscription::<T>::get() {
				SubscriptionStatus::Inactive => return Err(Error::<T>::SubscriptionInactive.into()),
				SubscriptionStatus::Terminated => return Ok(()),
				SubscriptionStatus::Active { .. } => {},
			}

			let notifier_notified = if from_notifier {
				true
			} else {
				let call = NotifierCall::Unsubscribe { subscriber_parachain_id: None };
				let ok = Self::send_to_notifier(call).is_ok();
				if !ok {
					log::error!(
						target: LOG_TARGET,
						"Failed to send unsubscribe XCM to notifier"
					);
				}
				ok
			};

			Self::clear_all_ring_data();
			Subscription::<T>::put(SubscriptionStatus::Terminated);

			Self::deposit_event(Event::SubscriptionTerminated { notifier_notified });
			Ok(())
		}

		/// Sends replay requests to the notifier for missing ring roots.
		///
		/// Submitted by the offchain worker as an authorized transaction. Validates
		/// that the subscription is active and that the provided indices are actually
		/// missing before sending XCM replay requests.
		#[pallet::authorize(Pallet::<T>::authorize_replay_missing_roots)]
		#[pallet::call_index(3)]
		#[pallet::weight(Pallet::<T>::replay_missing_roots_worst_case_weight())]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_replay_missing_roots(indices.len() as u32))]
		pub fn replay_missing_roots(
			origin: OriginFor<T>,
			identifier: Identifier,
			indices: BoundedVec<RingIndex, T::MaxMissingRootsPerCollection>,
		) -> DispatchResult {
			ensure_authorized(origin)?;

			let indices_set: BTreeSet<RingIndex> = indices.iter().copied().collect();

			if Self::process_collection_replay(identifier, &indices_set) {
				ProcessingState::<T>::mutate(|s| {
					s.last_replay_request_time = T::UnixTime::now().as_secs();
				});
			}

			Ok(())
		}
	}

	// ========== Call Declarations ==========

	/// Call declaration for members-notifier pallet on the notifier chain.
	#[derive(codec::Encode)]
	enum NotifierCall {
		/// `subscriber_parachain_id` is always `None` here because the notifier derives
		/// the parachain ID from the XCM origin. When called via governance on the notifier
		/// side, it can be set explicitly. The field must remain for encoding compatibility
		/// with the notifier's call interface.
		#[codec(index = 1)]
		Unsubscribe { subscriber_parachain_id: Option<u32> },
		#[codec(index = 2)]
		RequestReplay {
			subscriber_parachain_id: u32,
			identifier: Identifier,
			ring_root_indices: Vec<RingIndex>,
		},
	}

	// ========== Authorization ==========

	impl<T: Config> Pallet<T> {
		/// Validates that a replay request is authorized.
		///
		/// Checks that the transaction is local/in-block, the subscription is active,
		/// and at least one of the provided indices is actually missing.
		pub fn authorize_replay_missing_roots(
			source: TransactionSource,
			identifier: &Identifier,
			indices: &BoundedVec<RingIndex, T::MaxMissingRootsPerCollection>,
		) -> TransactionValidityWithRefund {
			if !matches!(source, TransactionSource::InBlock | TransactionSource::Local) {
				return Err(InvalidTransaction::Call.into());
			}

			if !matches!(Subscription::<T>::get(), SubscriptionStatus::Active { .. }) {
				return Err(InvalidTransaction::Call.into());
			}

			// To prevent replay spam
			let now = T::UnixTime::now().as_secs();
			let cooldown = T::ReplayCooldownSeconds::get();
			let ps = ProcessingState::<T>::get();
			if now.saturating_sub(ps.last_replay_request_time) < cooldown {
				return Err(InvalidTransaction::Stale.into());
			}

			let state = RingCollectionStates::<T>::get(identifier);
			// At least one of the provided indices must be missing
			let has_missing = indices.iter().any(|idx| state.missing_indices.contains_key(idx));
			if !has_missing {
				return Err(InvalidTransaction::Stale.into());
			}

			let validity = ValidTransaction::with_tag_prefix("members-subscriber:replay")
				.and_provides(identifier)
				.longevity(3)
				.propagate(false)
				.into();
			Ok((validity, T::WeightInfo::authorize_replay_missing_roots(indices.len() as u32)))
		}
	}

	// ========== Helper Functions ==========

	impl<T: Config> Pallet<T> {
		/// Clears all ring root data: RingRoots, RingCollectionStates,
		/// RingCollectionExponents, and ProcessingState.
		fn clear_all_ring_data() {
			let max_roots =
				T::MaxCollections::get().saturating_mul(T::MaxRingRootsPerCollection::get());
			let _ = RingRoots::<T>::clear(max_roots, None);
			let _ = RingCollectionStates::<T>::clear(T::MaxCollections::get(), None);
			let _ = RingCollectionExponents::<T>::clear(T::MaxCollections::get(), None);

			ProcessingState::<T>::kill();
		}

		/// Computes worst-case weight for the `replay_missing_roots` extrinsic.
		pub(crate) fn replay_missing_roots_worst_case_weight() -> Weight {
			let max_n = T::MaxMissingRootsPerCollection::get();
			let chunks = max_n.div_ceil(T::MaxUpdatesPerBatch::get());
			T::WeightInfo::replay_missing_roots(max_n)
				.saturating_add(T::WeightInfo::send_replay_request().saturating_mul(chunks.into()))
		}

		/// Sends an XCM message to the notifier.
		/// Encodes the call with the notifier's pallet index and sends via XCM Transact.
		fn send_to_notifier(call: NotifierCall) -> Result<(), Error<T>> {
			use codec::Encode;
			use xcm::prelude::*;

			let endpoint = T::RingRootsNotifier::get();
			let encoded_call = (endpoint.pallet_index, call).encode();

			let message: Xcm<()> = Xcm(vec![
				UnpaidExecution { weight_limit: Unlimited, check_origin: None },
				Transact {
					origin_kind: OriginKind::SovereignAccount,
					call: encoded_call.into(),
					fallback_max_weight: None,
				},
			]);

			send_xcm::<T::XcmSender>(endpoint.location, message)
				.map_err(|_| Error::<T>::XcmSendFailed)?;

			Ok(())
		}

		/// Stores ring roots from a batch and updates the collection state.
		///
		/// For `Built` ops: inserts/updates the ring and recovers from missing/deleted tracking.
		/// For `Deleted` ops: removes the ring, decrements count, and tracks the index as deleted.
		/// Also removes recovered indices from `missing_indices`.
		pub(crate) fn store_ring_roots(batch: &RingRootUpdatesBatch<T>) {
			let identifier = batch.identifier;
			let mut state = RingCollectionStates::<T>::get(identifier);

			state.next_ring_index = state.next_ring_index.max(batch.next_ring_index);

			for update in batch.updates.iter() {
				// Clearing from missing regardless of op type
				state.missing_indices.remove(&update.ring_index);

				match &update.op {
					RingRootOp::Built { revision, root } => {
						let mut roots =
							RingRoots::<T>::get(identifier, update.ring_index).unwrap_or_default();
						if roots.is_empty() {
							state.ring_count = state.ring_count.saturating_add(1);
						}
						// If this index was previously deleted, un-deleting it
						state.deleted_indices.remove(&update.ring_index);
						// Evicting oldest root when the window is full
						if roots.is_full() {
							roots.remove(0);
						}
						let _ = roots
							.try_push(RingCommitmentRecord {
								root: root.clone(),
								revision: *revision,
								source_time: batch.source_time,
								source_sequence: batch.sequence,
							})
							.defensive_proof(
								"room guaranteed: either was not full or oldest was evicted",
							);
						RingRoots::<T>::insert(identifier, update.ring_index, roots);
					},
					RingRootOp::Deleted => {
						let was_stored =
							RingRoots::<T>::take(identifier, update.ring_index).is_some();
						if was_stored {
							state.ring_count = state.ring_count.saturating_sub(1);
						}
						if state.deleted_indices.try_insert(update.ring_index).is_err() {
							log::warn!(
								target: LOG_TARGET,
								"deleted_indices at capacity for collection {identifier:?}, \
								 cannot track ring index {}",
								update.ring_index,
							);
						}
					},
				}
			}

			RingCollectionStates::<T>::insert(identifier, state);
		}

		/// Detects and records missing ring roots for the collection in the batch.
		/// Only scans the delta range `old_next_ring_index..next_ring_index` for new gaps,
		/// avoiding a full scan on every batch.
		pub(crate) fn detect_missing_rings_in_batch(
			batch: &RingRootUpdatesBatch<T>,
			old_next_ring_index: u32,
		) {
			let state = RingCollectionStates::<T>::get(batch.identifier);

			let accounted_for = state.ring_count.saturating_add(state.deleted_indices.len() as u32);

			if accounted_for >= state.next_ring_index {
				RingCollectionStates::<T>::mutate(batch.identifier, |state| {
					state.missing_indices.clear();
				});
				return;
			}

			// Skipping the scan when deleted_indices at capacity to avoid false positives
			if state.deleted_indices.len() as u32 >= T::MaxDeletedRingsPerCollection::get() {
				Self::deposit_event(Event::DeletedIndicesAtCapacity {
					identifier: batch.identifier,
				});
				return;
			}

			let mut new_missing_count = 0u32;
			RingCollectionStates::<T>::mutate(batch.identifier, |state| {
				for idx in old_next_ring_index..state.next_ring_index {
					if !RingRoots::<T>::contains_key(batch.identifier, idx) &&
						!state.missing_indices.contains_key(&idx) &&
						!state.deleted_indices.contains(&idx)
					{
						if state.missing_indices.try_insert(idx, 0).is_err() {
							log::warn!(
								target: LOG_TARGET,
								"missing_indices at capacity for collection {:?}, \
								 stopping scan at index {idx}",
								batch.identifier,
							);
							break;
						}
						new_missing_count += 1;
					}
				}
			});

			if new_missing_count > 0 {
				Self::deposit_event(Event::MissingRingsDetected {
					identifier: batch.identifier,
					count: new_missing_count,
				});
			}
		}

		/// Updates timestamps and sequence after processing a batch.
		fn record_batch_processed(sequence: SequenceNumber) {
			ProcessingState::<T>::mutate(|s| {
				s.last_batch_received_time = T::UnixTime::now().as_secs();
				s.last_processed_sequence = sequence;
			});
		}

		/// Ensures the subscription is in Active state.
		fn ensure_subscription_active() -> DispatchResult {
			match Subscription::<T>::get() {
				SubscriptionStatus::Active { .. } => Ok(()),
				SubscriptionStatus::Inactive => Err(Error::<T>::SubscriptionInactive.into()),
				SubscriptionStatus::Terminated => Err(Error::<T>::SubscriptionTerminated.into()),
			}
		}

		/// Sends replay requests to the notifier for missing ring roots of a single collection.
		///
		/// Increments attempt counts and abandons indices that exceed the retry threshold.
		/// Remaining indices are chunked into `MaxUpdatesPerBatch`-sized batches, each sent
		/// as a separate XCM message. Stops on first XCM failure.
		/// Returns `true` if at least one chunk was sent successfully.
		pub(crate) fn process_collection_replay(
			identifier: Identifier,
			indices: &BTreeSet<RingIndex>,
		) -> bool {
			if indices.is_empty() {
				return false;
			}

			let mut state = RingCollectionStates::<T>::get(identifier);
			if state.missing_indices.is_empty() {
				return false;
			}

			let warning_threshold = T::ReplayWarningThreshold::get();
			let abandon_threshold = T::ReplayAbandonThreshold::get();

			let mut to_send = Vec::new();
			for &idx in indices {
				let Some(replay_attempts) = state.missing_indices.get_mut(&idx) else {
					continue;
				};

				*replay_attempts = replay_attempts.saturating_add(1);

				if *replay_attempts >= abandon_threshold {
					log::error!(
						target: LOG_TARGET,
						"Replay request abandoned: identifier={identifier:?}, \
						 index={idx}, attempts={replay_attempts}",
					);
					state.missing_indices.remove(&idx);
					continue;
				}

				if *replay_attempts >= warning_threshold {
					log::warn!(
						target: LOG_TARGET,
						"Replay request warning: identifier={identifier:?}, \
						 index={idx}, attempts={replay_attempts}",
					);
				}

				to_send.push(idx);
			}

			RingCollectionStates::<T>::insert(identifier, state);

			if to_send.is_empty() {
				return false;
			}

			let max_per_chunk = T::MaxUpdatesPerBatch::get() as usize;

			let mut any_sent = false;
			for chunk in to_send.chunks(max_per_chunk) {
				if Self::send_replay_request(identifier, chunk).is_err() {
					log::error!(
						target: LOG_TARGET,
						"Replay request failed: identifier={identifier:?}, chunk={chunk:?}",
					);
					break;
				}

				any_sent = true;
				Self::deposit_event(Event::ReplayRequestSent {
					identifier,
					indices_count: chunk.len() as u32,
				});
			}

			any_sent
		}

		/// Submits an authorized transaction from the offchain worker and logs the result.
		fn submit_authorized_transaction(call: Call<T>, description: &str) {
			let tx = T::create_authorized_transaction(call.into());
			match SubmitTransaction::<T, _>::submit_transaction(tx) {
				Ok(()) => log::debug!(
					target: LOG_TARGET,
					"offchain worker: submitted authorized transaction for `{description}`"
				),
				Err(()) => log::warn!(
					target: LOG_TARGET,
					"offchain worker: failed to submit authorized transaction for \
					 `{description}`"
				),
			}
		}

		/// Sends a replay request to the notifier via XCM.
		pub(crate) fn send_replay_request(
			identifier: Identifier,
			indices: &[RingIndex],
		) -> Result<(), Error<T>> {
			let call = NotifierCall::RequestReplay {
				subscriber_parachain_id: T::SelfParaId::get(),
				identifier,
				ring_root_indices: indices.to_vec(),
			};
			Self::send_to_notifier(call)
		}

		/// Gather the information needed to verify a proof against a ring: the crypto
		/// capacity derived from the collection's stored exponent, and the sliding window
		/// of recent roots for the given ring index.
		fn ring_proving_information(
			identifier: &Identifier,
			ring_index: RingIndex,
		) -> Result<
			(
				<T::Crypto as GenerateVerifiable>::Config,
				BoundedVec<RingCommitmentRecord<T>, T::MaxRecentRootsPerRing>,
			),
			DispatchError,
		> {
			let ring_exponent = RingCollectionExponents::<T>::get(identifier)
				.ok_or(Error::<T>::CollectionNotFound)?;
			let capacity: <T::Crypto as GenerateVerifiable>::Config =
				ring_exponent.try_into().map_err(|_| Error::<T>::InvalidRingExponent)?;
			let roots = RingRoots::<T>::get(identifier, ring_index).ok_or(Error::<T>::NoRoot)?;
			Ok((capacity, roots))
		}
	}

	impl<T: Config> MembershipProver for Pallet<T> {
		type Crypto = T::Crypto;

		fn verify_membership(
			identifier: &Identifier,
			proof: &<T::Crypto as GenerateVerifiable>::Proof,
			ring_index: RingIndex,
			context: Context,
			msg: &[u8],
		) -> Result<RevisedContextualAlias, DispatchError> {
			let (capacity, roots) = Self::ring_proving_information(identifier, ring_index)?;

			for record in roots.iter().rev() {
				if let Ok(alias) =
					T::Crypto::validate(capacity, proof, &record.root, &context[..], msg)
				{
					return Ok(RevisedContextualAlias {
						revision: record.revision,
						ring: ring_index,
						ca: ContextualAlias { alias, context },
					});
				}
			}
			Err(Error::<T>::InvalidProof.into())
		}

		fn verify_membership_at_rev(
			identifier: &Identifier,
			proof: &<T::Crypto as GenerateVerifiable>::Proof,
			ring_index: RingIndex,
			revision: RevisionIndex,
			context: Context,
			msg: &[u8],
		) -> Result<ContextualAlias, DispatchError> {
			let (capacity, roots) = Self::ring_proving_information(identifier, ring_index)?;
			let record = roots
				.iter()
				.find(|r| r.revision == revision)
				.ok_or(Error::<T>::RevisionNotFound)?;
			let alias = T::Crypto::validate(capacity, proof, &record.root, &context[..], msg)
				.map_err(|_| Error::<T>::InvalidProof)?;
			Ok(ContextualAlias { alias, context })
		}

		fn verify_memberships_in_ring(
			identifier: &Identifier,
			ring_index: RingIndex,
			items: &[BatchProofItem<<T::Crypto as GenerateVerifiable>::Proof>],
		) -> Result<Vec<RevisedContextualAlias>, DispatchError> {
			let (capacity, roots) = Self::ring_proving_information(identifier, ring_index)?;

			for record in roots.iter().rev() {
				let Ok(aliases) = T::Crypto::batch_validate(capacity, &record.root, items) else {
					continue;
				};
				debug_assert_eq!(aliases.len(), items.len());
				return aliases
					.into_iter()
					.zip(items.iter().map(|item| item.context.as_slice()))
					.map(|(alias, context_bytes)| {
						let context: Context =
							context_bytes.try_into().map_err(|_| Error::<T>::InvalidProof)?;
						Ok(RevisedContextualAlias {
							revision: record.revision,
							ring: ring_index,
							ca: ContextualAlias { alias, context },
						})
					})
					.collect::<Result<Vec<_>, DispatchError>>();
			}
			Err(Error::<T>::InvalidProof.into())
		}

		fn verify_memberships_in_ring_at_rev(
			identifier: &Identifier,
			ring_index: RingIndex,
			revision: RevisionIndex,
			items: &[BatchProofItem<<T::Crypto as GenerateVerifiable>::Proof>],
		) -> Result<Vec<ContextualAlias>, DispatchError> {
			let (capacity, roots) = Self::ring_proving_information(identifier, ring_index)?;
			let record = roots
				.iter()
				.find(|r| r.revision == revision)
				.ok_or(Error::<T>::RevisionNotFound)?;

			let aliases = T::Crypto::batch_validate(capacity, &record.root, items)
				.map_err(|_| Error::<T>::InvalidProof)?;

			debug_assert_eq!(aliases.len(), items.len());

			aliases
				.into_iter()
				.zip(items.iter().map(|item| item.context.as_slice()))
				.map(|(alias, context_bytes)| {
					let context: Context =
						context_bytes.try_into().map_err(|_| Error::<T>::InvalidProof)?;
					Ok(ContextualAlias { alias, context })
				})
				.collect::<Result<Vec<_>, DispatchError>>()
		}

		fn ring_revision(identifier: &Identifier, ring_index: RingIndex) -> Option<RevisionIndex> {
			RingRoots::<T>::get(identifier, ring_index)?.last().map(|r| r.revision)
		}

		fn is_revision_valid(
			identifier: &Identifier,
			ring_index: RingIndex,
			revision: RevisionIndex,
		) -> bool {
			let Some(roots) = RingRoots::<T>::get(identifier, ring_index) else {
				return false;
			};
			roots.iter().any(|r| r.revision == revision)
		}

		fn revision_source_time(
			identifier: &Identifier,
			ring_index: RingIndex,
			revision: RevisionIndex,
		) -> Option<u64> {
			RingRoots::<T>::get(identifier, ring_index)?
				.iter()
				.find(|r| r.revision == revision)
				.map(|r| r.source_time)
		}
	}

	impl<T: Config> MembershipMultiProver for Pallet<T> {
		fn verify_membership_multi_context(
			identifier: &Identifier,
			proof: &<T::Crypto as GenerateVerifiable>::Proof,
			ring_index: RingIndex,
			contexts: &[Context],
			msg: &[u8],
		) -> Result<Vec<RevisedContextualAlias>, DispatchError> {
			let (capacity, roots) = Self::ring_proving_information(identifier, ring_index)?;
			let context_slices: Vec<&[u8]> = contexts.iter().map(|c| &c[..]).collect();

			for record in roots.iter().rev() {
				if let Ok(aliases) = T::Crypto::validate_multi_context(
					capacity,
					proof,
					&record.root,
					&context_slices,
					msg,
				) {
					debug_assert_eq!(aliases.len(), contexts.len());
					return Ok(aliases
						.into_iter()
						.zip(contexts.iter().copied())
						.map(|(alias, context)| RevisedContextualAlias {
							revision: record.revision,
							ring: ring_index,
							ca: ContextualAlias { alias, context },
						})
						.collect());
				}
			}
			Err(Error::<T>::InvalidProof.into())
		}

		fn verify_membership_multi_context_at_rev(
			identifier: &Identifier,
			proof: &<T::Crypto as GenerateVerifiable>::Proof,
			ring_index: RingIndex,
			revision: RevisionIndex,
			contexts: &[Context],
			msg: &[u8],
		) -> Result<Vec<ContextualAlias>, DispatchError> {
			let (capacity, roots) = Self::ring_proving_information(identifier, ring_index)?;
			let record = roots
				.iter()
				.find(|r| r.revision == revision)
				.ok_or(Error::<T>::RevisionNotFound)?;

			let context_slices: Vec<&[u8]> = contexts.iter().map(|c| &c[..]).collect();
			let aliases = T::Crypto::validate_multi_context(
				capacity,
				proof,
				&record.root,
				&context_slices,
				msg,
			)
			.map_err(|_| Error::<T>::InvalidProof)?;
			debug_assert_eq!(aliases.len(), contexts.len());

			Ok(aliases
				.into_iter()
				.zip(contexts.iter().copied())
				.map(|(alias, context)| ContextualAlias { alias, context })
				.collect())
		}
	}
}
