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

//! Airdrop pallet with claims based on VRF entropy.
//!
//! Each active event is indexed twice in storage:
//! - [`Events`] keyed by `EventId` carries the event's state.
//! - [`ActionSchedule`] is a `(BigEndianU64 timestamp, EventId)` map under `Identity` hashers, so
//!   iteration order matches numerical order of the timestamps.
//!
//! An offchain worker walks [`ActionSchedule`] in timestamp order and submits
//! the appropriate `authorize`d lifecycle extrinsic for each due event.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod types;
pub mod vrf;
pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub use indiv_support::utils::BigEndianU256;
pub use pallet::*;
pub use types::*;
pub use weights::WeightInfo;

use alloc::vec::Vec;
use codec::Encode;
use frame_support::{
	pallet_prelude::*,
	storage::with_storage_layer,
	traits::{
		fungibles::{Inspect, Mutate as _, MutateHold as _},
		tokens::{fungibles, Precision, Preservation},
		UnixTime,
	},
	PalletId,
};
use frame_system::{
	offchain::{CreateAuthorizedTransaction, SubmitTransaction},
	pallet_prelude::*,
};
use indiv_support::{
	traits::{
		Context, CurrentBlockRandomness, MembershipProver, RevisionIndex, RingIndex,
		PEOPLE_IDENTIFIER,
	},
	utils::BigEndianU64,
};
use sp_core::{blake2_256, hexdisplay::HexDisplay, sr25519};
use sp_runtime::{
	traits::{AccountIdConversion, TryConvert},
	Saturating,
};
use verifiable::GenerateVerifiable;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::dispatch::PostDispatchInfo;
	use sp_core::sr25519::vrf::VrfSignature;
	use sp_runtime::transaction_validity::{
		InvalidTransaction, TransactionSource, TransactionValidityError, ValidTransaction,
	};

	/// The log target for the pallet.
	pub(crate) const LOG_TARGET: &str = "runtime::indiv-pallet-airdrop";

	/// Base label for the per-event personhood context. The full context is
	/// `blake2_256(AIRDROP_CONTEXT_BASE ++ event_id)`.
	pub const AIRDROP_CONTEXT_BASE: &[u8] = b"pop:polkadot.network/airdrop";

	/// Compute the per-event context for the VRF.
	pub fn context_for_event(event_id: &EventId) -> Context {
		let mut buf = Vec::with_capacity(AIRDROP_CONTEXT_BASE.len() + size_of::<EventId>());
		buf.extend_from_slice(AIRDROP_CONTEXT_BASE);
		buf.extend_from_slice(event_id);
		blake2_256(&buf)
	}

	/// Maximum number of winners per event.
	pub const MAX_WINNERS: u32 = 10_000;

	/// Registered airdrop events, keyed by their identifier.
	#[pallet::storage]
	pub type Events<T: Config> = StorageMap<_, Twox64Concat, EventId, ActiveEventOf<T>>;

	/// Pending lifecycle actions for registered events, ordered by the timestamp at which the
	/// action should take place.
	#[pallet::storage]
	pub type ActionSchedule<T: Config> =
		StorageDoubleMap<_, Identity, BigEndianTimestamp, Identity, EventId, ()>;

	/// Per-event participant registrations, keyed by 32-byte entropy slot
	/// in big-endian order under `Identity`.
	#[pallet::storage]
	pub type Registrations<T: Config> =
		StorageDoubleMap<_, Twox64Concat, EventId, Identity, BigEndianU256, RegistrationEntryOf<T>>;

	/// Winners for an event, keyed by the event identifier and their registration entry. This maps
	/// to the participant's ticket for offchain lookup purposes.
	#[pallet::storage]
	pub type Winners<T: Config> = StorageDoubleMap<
		_,
		Twox64Concat,
		EventId,
		Blake2_128Concat,
		RegistrationEntryOf<T>,
		BigEndianU256,
	>;

	/// Entropy seed captured at draw time, per event. This serves as a partition point in the
	/// participation ticket ordered list in order to pick winners.
	#[pallet::storage]
	pub type EventEntropy<T: Config> = StorageMap<_, Twox64Concat, EventId, BigEndianU256>;

	/// Per-asset enablement gate. An asset is usable for scheduling events iff it is present in
	/// this map; the stored value is the amount the pallet transferred into its pot to keep the
	/// pot's asset account alive (the asset's ED at the time of enabling). `enable_asset` /
	/// `disable_asset` are the only ways to mutate this.
	#[pallet::storage]
	pub type SupportedAssets<T: Config> =
		StorageMap<_, Twox64Concat, AssetIdOf<T>, AssetBalanceOf<T>>;

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + CreateAuthorizedTransaction<Call<Self>> {
		type WeightInfo: WeightInfo;

		/// Ring-membership prover used by alias-based participation.
		type MemberService: MembershipProver<
			Crypto: GenerateVerifiable<Proof: Send + Sync, Signature: Send + Sync>,
		>;

		/// Fungible token interface.
		type Fungibles: fungibles::Mutate<Self::AccountId>
			+ fungibles::MutateHold<Self::AccountId, Reason: From<HoldReason>>;

		/// Governance/admin origin allowed to schedule events directly via the `schedule_event`
		/// extrinsic.
		type ManagerOrigin: EnsureOrigin<Self::RuntimeOrigin>;

		/// Pallet id used to derive the prize pot account.
		#[pallet::constant]
		type PalletId: Get<PalletId>;

		/// Unix time source.
		type UnixTime: UnixTime;

		/// Per-block randomness source used to seed the winner draw.
		type Randomness: CurrentBlockRandomness;

		/// Try to derive an sr25519 public key from a runtime `AccountId`.
		///
		/// The account-based participation flow takes only an `AccountId` (the calling pallet has
		/// authenticated the user); the pallet needs the sr25519 public key to verify the
		/// participant's VRF signature. The conversion is fallible because the AccountId might not
		/// correspond to an sr25519 key. For now, only accounts which are sr25519 can participate
		/// in events.
		type AccountIdToPublic: TryConvert<Self::AccountId, sr25519::Public>;

		/// Maximum number of registration entries processed per
		/// `clean_up_{registrations,winners}_authorized` invocation.
		#[pallet::constant]
		type ClearLimit: Get<u32>;

		/// Maximum number of winner slots inserted per `draw_winners_authorized` call.
		#[pallet::constant]
		type DrawLimit: Get<u32>;

		/// How frequently the offchain worker fires (in blocks).
		#[pallet::constant]
		type OffchainWorkerInterval: Get<BlockNumberFor<Self>>;

		/// Benchmark helper.
		#[cfg(feature = "runtime-benchmarks")]
		type BenchmarkHelper: crate::benchmarking::BenchmarkHelper<Self>;
	}

	#[pallet::extra_constants]
	impl<T: Config> Pallet<T> {
		/// Account id of the prize pot.
		pub fn airdrop_pot_id() -> T::AccountId {
			T::PalletId::get().into_account_truncating()
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		EventScheduled {
			event_id: EventId,
		},
		ScheduledEventRemoved {
			event_id: EventId,
		},
		EventCancelled {
			event_id: EventId,
		},
		RegistrationStarted {
			event_id: EventId,
		},
		AliasRegistered {
			event_id: EventId,
			slot: BigEndianU256,
			participant_origin: RegistrationEntryOf<T>,
		},
		AccountRegistered {
			event_id: EventId,
			slot: BigEndianU256,
			account_id: T::AccountId,
		},
		DrawingWinners {
			event_id: EventId,
			effective_winners: u32,
		},
		ClaimingStarted {
			event_id: EventId,
			effective_winners: u32,
		},
		EventCanceled {
			event_id: EventId,
		},
		PrizeClaimed {
			event_id: EventId,
			slot: BigEndianU256,
			beneficiary: T::AccountId,
			asset_id: AssetIdOf<T>,
			amount: AssetBalanceOf<T>,
		},
		ClearingRegistrations {
			event_id: EventId,
			unclaimed: u32,
		},
		ClearingWinners {
			event_id: EventId,
		},
		FinalizingEvent {
			event_id: EventId,
		},
		EventCompleted {
			event_id: EventId,
		},
		AssetEnabled {
			asset_id: AssetIdOf<T>,
			funded: AssetBalanceOf<T>,
		},
		AssetDisabled {
			asset_id: AssetIdOf<T>,
			refunded: AssetBalanceOf<T>,
		},
	}

	#[pallet::error]
	pub enum Error<T> {
		PrizeBelowMinBalance,
		NoWinnersConfigured,
		TooManyWinners,
		InvalidEventTimes,
		DuplicateEventId,
		NoScheduledEvent,
		UnknownEvent,
		/// Operation requires a specific status the event isn't in.
		WrongStatus,
		NotAcceptingRegistrations,
		NotClaiming,
		/// Claim attempted after the event's `end_time`.
		ClaimingWindowClosed,
		EntropySlotTaken,
		InvalidVrfProof,
		/// Supplied account id does not correspond to any sr25519 public key.
		UnsupportedAccountKey,
		InvalidMembershipProof,
		NoSuchWinner,
		ParticipantOverflow,
		PrizeAllocationOverflow,
		/// The prize asset has not been enabled via `enable_asset`.
		AssetNotEnabled,
		/// `enable_asset` was called for an asset that is already enabled.
		AssetAlreadyEnabled,
	}

	/// Custom transaction-validity errors.
	#[repr(u8)]
	pub enum AuthorizeInvalidity {
		/// Transaction source is not local or in block.
		TransactionNotLocal = 200,
		/// `event_id` is not in [`Events`].
		UnknownEvent = 201,
		/// Event status doesn't permit this action.
		WrongStatus = 202,
		/// Action is not yet due (timestamp gate).
		NotYetDue = 203,
	}

	impl From<AuthorizeInvalidity> for TransactionValidityError {
		fn from(e: AuthorizeInvalidity) -> Self {
			InvalidTransaction::Custom(e as u8).into()
		}
	}

	#[pallet::composite_enum]
	pub enum HoldReason {
		Airdrop,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn integrity_test() {
			assert!(
				T::DrawLimit::get() > 0,
				"airdrop: `DrawLimit` must be > 0 so `draw_winners_authorized` can make progress",
			);
			assert!(
				T::ClearLimit::get() > 0,
				"airdrop: `ClearLimit` must be > 0 so clean-up phases can make progress",
			);
			assert!(
				!T::OffchainWorkerInterval::get().is_zero(),
				"airdrop: `OffchainWorkerInterval` must be > 0",
			);
		}

		fn offchain_worker(block_number: BlockNumberFor<T>) {
			use sp_runtime::traits::Zero;
			if !(block_number % T::OffchainWorkerInterval::get()).is_zero() {
				return;
			}

			let now = T::UnixTime::now().as_secs();
			// Iterate the schedule in (timestamp, event_id) order.
			for (timestamp, event_id, _) in ActionSchedule::<T>::iter() {
				// Stop at the first entry whose timestamp is in the future.
				if timestamp.0 > now {
					break;
				}
				let Some(event) = Events::<T>::get(event_id) else {
					continue;
				};
				let discriminator = indiv_support::utils::ocw_random_u32();
				let call = match event.status {
					Status::Scheduled =>
						Call::start_registration_authorized { event_id, discriminator },
					Status::Registering { .. } =>
						Call::close_registration_authorized { event_id, discriminator },
					Status::DrawWinners { winners_added, effective_winners, .. } =>
						if winners_added == effective_winners {
							Call::close_drawing_authorized { event_id, discriminator }
						} else {
							Call::draw_winners_authorized { event_id, discriminator }
						},
					Status::Claiming { .. } =>
						Call::close_claiming_authorized { event_id, discriminator },
					Status::ClearingRegistrations { .. } =>
						Call::clean_up_registrations_authorized { event_id, discriminator },
					Status::ClearingWinners { .. } =>
						Call::clean_up_winners_authorized { event_id, discriminator },
					Status::Finalizing { .. } =>
						Call::finalize_authorized { event_id, discriminator },
				};
				Self::submit_authorized_transaction(call);
			}
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Schedule a new airdrop event. Origin must be `ManagerOrigin`. The prize allocation is
		/// held in the pallet's pot account. The pot is assumed to be pre-funded.
		///
		/// Cross-pallet callers should use the [`crate::types::Airdrop::schedule`] trait method
		/// instead, which debits a caller-supplied `source` account.
		#[pallet::call_index(0)]
		#[pallet::weight(T::WeightInfo::schedule_event())]
		pub fn schedule_event(
			origin: OriginFor<T>,
			event_id: EventId,
			info: EventInfoOf<T>,
		) -> DispatchResult {
			T::ManagerOrigin::ensure_origin(origin)?;
			Self::do_schedule(event_id, info)
		}

		/// Remove a previously scheduled event. The event must not have already
		/// started, otherwise this call will fail.
		#[pallet::call_index(1)]
		#[pallet::weight(T::WeightInfo::remove_scheduled_event())]
		pub fn remove_scheduled_event(origin: OriginFor<T>, event_id: EventId) -> DispatchResult {
			T::ManagerOrigin::ensure_origin(origin)?;
			Self::do_remove_scheduled(event_id)
		}

		/// Enable an asset for use in airdrop events. The origin must be `ManagerOrigin` only.
		///
		/// Transfers the asset's current minimum balance from `source` to the pallet's pot so the
		/// pot's asset account stays alive while events hold prize funds against it.
		#[pallet::call_index(10)]
		#[pallet::weight(T::WeightInfo::enable_asset())]
		pub fn enable_asset(
			origin: OriginFor<T>,
			asset_id: AssetIdOf<T>,
			source: T::AccountId,
		) -> DispatchResult {
			T::ManagerOrigin::ensure_origin(origin)?;
			ensure!(
				!SupportedAssets::<T>::contains_key(&asset_id),
				Error::<T>::AssetAlreadyEnabled,
			);
			let min_balance = T::Fungibles::minimum_balance(asset_id.clone());
			let pot = Self::airdrop_pot_id();
			T::Fungibles::transfer(
				asset_id.clone(),
				&source,
				&pot,
				min_balance,
				Preservation::Expendable,
			)?;
			SupportedAssets::<T>::insert(&asset_id, min_balance);
			Self::deposit_event(Event::<T>::AssetEnabled { asset_id, funded: min_balance });
			Ok(())
		}

		/// Disable an asset previously enabled with `enable_asset`. Refunds the originally-funded
		/// amount from the pot to `beneficiary`. The origin must be `ManagerOrigin` only.
		///
		/// The manager is responsible for ensuring no events still reference this asset before
		/// disabling, but this is safe since scheduling an event is permissioned.
		#[pallet::call_index(11)]
		#[pallet::weight(T::WeightInfo::disable_asset())]
		pub fn disable_asset(
			origin: OriginFor<T>,
			asset_id: AssetIdOf<T>,
			beneficiary: T::AccountId,
		) -> DispatchResult {
			T::ManagerOrigin::ensure_origin(origin)?;
			let funded = SupportedAssets::<T>::get(&asset_id).ok_or(Error::<T>::AssetNotEnabled)?;
			let pot = Self::airdrop_pot_id();
			T::Fungibles::transfer(
				asset_id.clone(),
				&pot,
				&beneficiary,
				funded,
				Preservation::Expendable,
			)?;
			SupportedAssets::<T>::remove(&asset_id);
			Self::deposit_event(Event::<T>::AssetDisabled { asset_id, refunded: funded });
			Ok(())
		}

		/// OCW-driven: transition `Scheduled → Registering` when
		/// `registration_starts` is reached.
		///
		/// `discriminator` is any `u32`; it lets the OCW send a different transaction when a
		/// previous one is banned by the pool because it was validated against a different state
		/// after a re-org. As the accepted transaction source is only local, it cannot be used to
		/// spam the pool.
		#[pallet::authorize(Pallet::<T>::authorize_start_registration)]
		#[pallet::call_index(2)]
		#[pallet::weight(T::WeightInfo::start_registration())]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_lifecycle_call())]
		pub fn start_registration_authorized(
			origin: OriginFor<T>,
			event_id: EventId,
			_discriminator: u32,
		) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;
			let mut event = Events::<T>::get(event_id).ok_or(Error::<T>::UnknownEvent)?;
			Self::transition(&mut event, Status::Registering { total_participants: 0 });
			Events::<T>::insert(event_id, event);
			Self::deposit_event(Event::<T>::RegistrationStarted { event_id });
			Ok(Pays::No.into())
		}

		/// OCW-driven: at `draw_time`:
		/// - close registration
		/// - capture randomness
		/// - compute the target winner count
		/// - release the unused-slot prize allocation up-front
		/// - transition `Registering → DrawWinners`
		///
		/// The draw itself is performed in batches by `draw_winners_authorized`.
		///
		/// `discriminator` is any `u32`; it lets the OCW send a different transaction when a
		/// previous one is banned by the pool because it was validated against a different state
		/// after a re-org. As the accepted transaction source is only local, it cannot be used to
		/// spam the pool.
		#[pallet::authorize(Pallet::<T>::authorize_close_registration)]
		#[pallet::call_index(3)]
		#[pallet::weight(T::WeightInfo::close_registration())]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_lifecycle_call())]
		pub fn close_registration_authorized(
			origin: OriginFor<T>,
			event_id: EventId,
			_discriminator: u32,
		) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;
			let mut event = Events::<T>::get(event_id).ok_or(Error::<T>::UnknownEvent)?;
			let Status::Registering { total_participants } = event.status else {
				defensive!("close_registration: authorize verified Status::Registering");
				return Err(Error::<T>::WrongStatus.into());
			};

			if total_participants == 0 {
				// Nothing to draw and no registrations to clean up, so we refund the entire prize
				// hold and tear the event down.
				Self::refund_and_drop_event(event);
				Self::deposit_event(Event::<T>::EventCanceled { event_id });
				return Ok(Pays::No.into());
			}

			// Seed the draw. If no usable randomness is available we leave the event in
			// `Registering` and let the OCW retry on the next block. This may mean that the event
			// doesn't actually happen if randomness is unavailable for a long time.
			let Some(entropy) = T::Randomness::randomness() else {
				log::warn!(
					target: LOG_TARGET,
					"airdrop: no randomness available for event 0x{}; retrying next block",
					HexDisplay::from(&event_id),
				);
				return Ok(Pays::No.into());
			};
			let target = BigEndianU256::from(entropy);

			let capped = event.info.prize.winner_cap.mul_floor(total_participants);
			let effective_winners = event.info.prize.max_winners.min(capped).max(1);

			// Release the prize allocation for slots we now know won't be awarded (excess of
			// `max_winners` over `effective_winners`).
			let unused_winner_slots =
				event.info.prize.max_winners.saturating_sub(effective_winners);
			if unused_winner_slots > 0 {
				Self::release_remaining_prizes(&event.info.prize, unused_winner_slots);
			}

			EventEntropy::<T>::insert(event_id, target);
			Self::transition(
				&mut event,
				Status::DrawWinners {
					total_participants,
					effective_winners,
					winners_added: 0,
					from_winner_key: target,
				},
			);
			Events::<T>::insert(event_id, event);

			Self::deposit_event(Event::<T>::DrawingWinners { event_id, effective_winners });
			Ok(Pays::No.into())
		}

		/// OCW-driven: draw up to `DrawLimit` winners per call.
		///
		/// After all the winners are drawn, the transition to `Claiming` is performed by the
		/// separate `close_drawing_authorized`.
		///
		/// `discriminator` is any `u32`; it lets the OCW send a different transaction when a
		/// previous one is banned by the pool because it was validated against a different state
		/// after a re-org. As the accepted transaction source is only local, it cannot be used to
		/// spam the pool.
		#[pallet::authorize(Pallet::<T>::authorize_draw_winners)]
		#[pallet::call_index(4)]
		#[pallet::weight(T::WeightInfo::draw_winners(T::DrawLimit::get()))]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_lifecycle_call())]
		pub fn draw_winners_authorized(
			origin: OriginFor<T>,
			event_id: EventId,
			_discriminator: u32,
		) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;
			let mut event = Events::<T>::get(event_id).ok_or(Error::<T>::UnknownEvent)?;
			let Status::DrawWinners {
				effective_winners,
				mut winners_added,
				mut from_winner_key,
				total_participants,
			} = event.status
			else {
				defensive!("draw_winners: authorize verified Status::DrawWinners");
				return Err(Error::<T>::WrongStatus.into());
			};

			let remaining = effective_winners.saturating_sub(winners_added);
			let capped_remaining = remaining.min(T::DrawLimit::get());
			let mut new_winners_added = 0;

			let from = Registrations::<T>::hashed_key_for(event_id, from_winner_key);
			let iter = Registrations::<T>::iter_prefix_from(event_id, from)
				.chain(Registrations::<T>::iter_prefix(event_id))
				.take(capped_remaining as usize);
			for (next_winner, registration) in iter {
				Winners::<T>::insert(event_id, registration, next_winner);
				from_winner_key = next_winner;
				new_winners_added.saturating_inc();
			}
			defensive_assert!(
				new_winners_added == capped_remaining,
				"iterator exhausted before capped_remaining"
			);

			winners_added = winners_added.saturating_add(new_winners_added);
			event.status = Status::DrawWinners {
				effective_winners,
				winners_added,
				from_winner_key,
				total_participants,
			};
			Events::<T>::insert(event_id, event);

			let actual_weight = T::WeightInfo::draw_winners(new_winners_added);
			Ok(PostDispatchInfo { actual_weight: Some(actual_weight), pays_fee: Pays::No })
		}

		/// OCW-driven: once `draw_winners_authorized` has filled the winner set, transition the
		/// event from `DrawWinners` to `Claiming`.
		///
		/// `discriminator` is any `u32`; it lets the OCW send a different transaction when a
		/// previous one is banned by the pool because it was validated against a different state
		/// after a re-org. As the accepted transaction source is only local, it cannot be used to
		/// spam the pool.
		#[pallet::authorize(Pallet::<T>::authorize_close_drawing)]
		#[pallet::call_index(5)]
		#[pallet::weight(T::WeightInfo::close_drawing())]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_lifecycle_call())]
		pub fn close_drawing_authorized(
			origin: OriginFor<T>,
			event_id: EventId,
			_discriminator: u32,
		) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;
			let mut event = Events::<T>::get(event_id).ok_or(Error::<T>::UnknownEvent)?;
			let Status::DrawWinners { total_participants, effective_winners, .. } = event.status
			else {
				defensive!("close_drawing: authorize verified Status::DrawWinners");
				return Err(Error::<T>::WrongStatus.into());
			};

			Self::transition(
				&mut event,
				Status::Claiming { total_participants, effective_winners, claimed: 0 },
			);
			Events::<T>::insert(event_id, event);
			Self::deposit_event(Event::<T>::ClaimingStarted { event_id, effective_winners });
			Ok(Pays::No.into())
		}

		/// OCW-driven: at `end_time` close claiming and enter the first clean-up phase.
		///
		/// `discriminator` is any `u32`; it lets the OCW send a different transaction when a
		/// previous one is banned by the pool because it was validated against a different state
		/// after a re-org. As the accepted transaction source is only local, it cannot be used to
		/// spam the pool.
		#[pallet::authorize(Pallet::<T>::authorize_close_claiming)]
		#[pallet::call_index(6)]
		#[pallet::weight(T::WeightInfo::close_claiming())]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_lifecycle_call())]
		pub fn close_claiming_authorized(
			origin: OriginFor<T>,
			event_id: EventId,
			_discriminator: u32,
		) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;
			let mut event = Events::<T>::get(event_id).ok_or(Error::<T>::UnknownEvent)?;
			let Status::Claiming { total_participants, effective_winners, claimed } = event.status
			else {
				defensive!("close_claiming: authorize verified Status::Claiming");
				return Err(Error::<T>::WrongStatus.into());
			};

			let unclaimed = effective_winners.saturating_sub(claimed);
			Self::transition(
				&mut event,
				Status::ClearingRegistrations {
					total_participants,
					effective_winners,
					claimed,
					cleaned_registrations: 0,
				},
			);
			Events::<T>::insert(event_id, event);

			Self::deposit_event(Event::<T>::ClearingRegistrations { event_id, unclaimed });
			Ok(Pays::No.into())
		}

		/// OCW-driven: First step of clean-up is to clear up to `ClearLimit` entries from
		/// `Registrations`. When the storage is fully drained, transitions to `ClearingWinners`.
		///
		/// `discriminator` is any `u32`; it lets the OCW send a different transaction when a
		/// previous one is banned by the pool because it was validated against a different state
		/// after a re-org. As the accepted transaction source is only local, it cannot be used to
		/// spam the pool.
		#[pallet::authorize(Pallet::<T>::authorize_clean_up_registrations)]
		#[pallet::call_index(7)]
		#[pallet::weight(
			T::WeightInfo::clean_up_registrations(T::ClearLimit::get())
				.saturating_add(T::WeightInfo::transition_clean_up_phase())
		)]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_lifecycle_call())]
		pub fn clean_up_registrations_authorized(
			origin: OriginFor<T>,
			event_id: EventId,
			_discriminator: u32,
		) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;
			Self::clean_up_registrations_inner(event_id, T::ClearLimit::get())
		}

		/// OCW-driven: Second step of clean-up is to clear up to `ClearLimit` entries from
		/// `Winners`. When the storage is fully drained, transitions to `Finalizing`.
		///
		/// `discriminator` is any `u32`; it lets the OCW send a different transaction when a
		/// previous one is banned by the pool because it was validated against a different state
		/// after a re-org. As the accepted transaction source is only local, it cannot be used to
		/// spam the pool.
		#[pallet::authorize(Pallet::<T>::authorize_clean_up_winners)]
		#[pallet::call_index(8)]
		#[pallet::weight(
			T::WeightInfo::clean_up_winners(T::ClearLimit::get())
				.saturating_add(T::WeightInfo::transition_clean_up_phase())
		)]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_lifecycle_call())]
		pub fn clean_up_winners_authorized(
			origin: OriginFor<T>,
			event_id: EventId,
			_discriminator: u32,
		) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;
			Self::clean_up_winners_inner(event_id, T::ClearLimit::get())
		}

		/// OCW-driven: Third step of clean-up is to release the unclaimed prize allocation and
		/// remove the event.
		///
		/// `discriminator` is any `u32`; it lets the OCW send a different transaction when a
		/// previous one is banned by the pool because it was validated against a different state
		/// after a re-org. As the accepted transaction source is only local, it cannot be used to
		/// spam the pool.
		#[pallet::authorize(Pallet::<T>::authorize_finalize)]
		#[pallet::call_index(9)]
		#[pallet::weight(T::WeightInfo::finalize())]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_lifecycle_call())]
		pub fn finalize_authorized(
			origin: OriginFor<T>,
			event_id: EventId,
			_discriminator: u32,
		) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;
			let event = Events::<T>::get(event_id).ok_or(Error::<T>::UnknownEvent)?;
			let Status::Finalizing { effective_winners, claimed } = event.status else {
				defensive!("finalize: authorize verified Status::Finalizing");
				return Err(Error::<T>::WrongStatus.into());
			};

			let unclaimed = effective_winners.saturating_sub(claimed);
			if unclaimed > 0 {
				Self::release_remaining_prizes(&event.info.prize, unclaimed);
			}
			let timestamp = Self::next_action_scheduled_at(&event.status, &event.info);
			ActionSchedule::<T>::remove(BigEndianU64(timestamp), event_id);
			EventEntropy::<T>::remove(event_id);
			Events::<T>::remove(event_id);
			Self::deposit_event(Event::<T>::EventCompleted { event_id });
			Ok(Pays::No.into())
		}
	}

	impl<T: Config> Pallet<T> {
		pub(crate) fn authorize_start_registration(
			source: TransactionSource,
			event_id: &EventId,
			_discriminator: &u32,
		) -> TransactionValidityWithRefund {
			Self::ensure_local(source)?;
			let event = Events::<T>::get(event_id).ok_or(AuthorizeInvalidity::UnknownEvent)?;
			if !matches!(event.status, Status::Scheduled) {
				return Err(AuthorizeInvalidity::WrongStatus.into());
			}
			let now = T::UnixTime::now().as_secs();
			if now < Self::next_action_scheduled_at(&event.status, &event.info) {
				return Err(AuthorizeInvalidity::NotYetDue.into());
			}
			Self::valid_for("airdrop:start", event_id)
		}

		pub(crate) fn authorize_close_registration(
			source: TransactionSource,
			event_id: &EventId,
			_discriminator: &u32,
		) -> TransactionValidityWithRefund {
			Self::ensure_local(source)?;
			let event = Events::<T>::get(event_id).ok_or(AuthorizeInvalidity::UnknownEvent)?;
			if !matches!(event.status, Status::Registering { .. }) {
				return Err(AuthorizeInvalidity::WrongStatus.into());
			}
			let now = T::UnixTime::now().as_secs();
			if now < Self::next_action_scheduled_at(&event.status, &event.info) {
				return Err(AuthorizeInvalidity::NotYetDue.into());
			}
			Self::valid_for("airdrop:close-reg", event_id)
		}

		pub(crate) fn authorize_draw_winners(
			source: TransactionSource,
			event_id: &EventId,
			_discriminator: &u32,
		) -> TransactionValidityWithRefund {
			Self::ensure_local(source)?;
			let event = Events::<T>::get(event_id).ok_or(AuthorizeInvalidity::UnknownEvent)?;
			let Status::DrawWinners { winners_added, .. } = event.status else {
				return Err(AuthorizeInvalidity::WrongStatus.into());
			};
			// Tag the tx with `winners_added` so consecutive OCW submissions for the same event are
			// distinct in the tx pool.
			Self::valid_for("airdrop:draw-winners", (event_id, winners_added))
		}

		pub(crate) fn authorize_close_drawing(
			source: TransactionSource,
			event_id: &EventId,
			_discriminator: &u32,
		) -> TransactionValidityWithRefund {
			Self::ensure_local(source)?;
			let event = Events::<T>::get(event_id).ok_or(AuthorizeInvalidity::UnknownEvent)?;
			let Status::DrawWinners { winners_added, effective_winners, .. } = event.status else {
				return Err(AuthorizeInvalidity::WrongStatus.into());
			};
			if winners_added != effective_winners {
				return Err(AuthorizeInvalidity::WrongStatus.into());
			}
			Self::valid_for("airdrop:close-drawing", event_id)
		}

		pub(crate) fn authorize_close_claiming(
			source: TransactionSource,
			event_id: &EventId,
			_discriminator: &u32,
		) -> TransactionValidityWithRefund {
			Self::ensure_local(source)?;
			let event = Events::<T>::get(event_id).ok_or(AuthorizeInvalidity::UnknownEvent)?;
			if !matches!(event.status, Status::Claiming { .. }) {
				return Err(AuthorizeInvalidity::WrongStatus.into());
			}
			let now = T::UnixTime::now().as_secs();
			if now < Self::next_action_scheduled_at(&event.status, &event.info) {
				return Err(AuthorizeInvalidity::NotYetDue.into());
			}
			Self::valid_for("airdrop:close-claim", event_id)
		}

		pub(crate) fn authorize_clean_up_registrations(
			source: TransactionSource,
			event_id: &EventId,
			_discriminator: &u32,
		) -> TransactionValidityWithRefund {
			Self::ensure_local(source)?;
			let event = Events::<T>::get(event_id).ok_or(AuthorizeInvalidity::UnknownEvent)?;
			let Status::ClearingRegistrations { cleaned_registrations, .. } = event.status else {
				return Err(AuthorizeInvalidity::WrongStatus.into());
			};
			// Tag the tx with `cleaned_registrations` so consecutive OCW submissions for the same
			// event are distinct in the tx pool.
			Self::valid_for("airdrop:clean-up-regs", (event_id, cleaned_registrations))
		}

		pub(crate) fn authorize_clean_up_winners(
			source: TransactionSource,
			event_id: &EventId,
			_discriminator: &u32,
		) -> TransactionValidityWithRefund {
			Self::ensure_local(source)?;
			let event = Events::<T>::get(event_id).ok_or(AuthorizeInvalidity::UnknownEvent)?;
			let Status::ClearingWinners { cleaned_winners, .. } = event.status else {
				return Err(AuthorizeInvalidity::WrongStatus.into());
			};
			Self::valid_for("airdrop:clean-up-winners", (event_id, cleaned_winners))
		}

		pub(crate) fn authorize_finalize(
			source: TransactionSource,
			event_id: &EventId,
			_discriminator: &u32,
		) -> TransactionValidityWithRefund {
			Self::ensure_local(source)?;
			let event = Events::<T>::get(event_id).ok_or(AuthorizeInvalidity::UnknownEvent)?;
			if !matches!(event.status, Status::Finalizing { .. }) {
				return Err(AuthorizeInvalidity::WrongStatus.into());
			}
			Self::valid_for("airdrop:finalize", event_id)
		}

		/// Schedule an event.
		pub(crate) fn do_schedule(event_id: EventId, info: EventInfoOf<T>) -> DispatchResult {
			ensure!(info.prize.max_winners > 0, Error::<T>::NoWinnersConfigured);
			ensure!(info.prize.max_winners <= MAX_WINNERS, Error::<T>::TooManyWinners);
			ensure!(
				info.registration_starts < info.draw_time && info.draw_time < info.end_time,
				Error::<T>::InvalidEventTimes,
			);
			ensure!(
				SupportedAssets::<T>::contains_key(&info.prize.asset_id),
				Error::<T>::AssetNotEnabled,
			);
			ensure!(
				info.prize.asset_amount >=
					T::Fungibles::minimum_balance(info.prize.asset_id.clone()),
				Error::<T>::PrizeBelowMinBalance,
			);
			ensure!(!Events::<T>::contains_key(event_id), Error::<T>::DuplicateEventId);

			let pot = Self::airdrop_pot_id();
			let to_hold = info
				.prize
				.asset_amount
				.checked_mul(&info.prize.max_winners.into())
				.ok_or(Error::<T>::PrizeAllocationOverflow)?;
			T::Fungibles::hold(
				info.prize.asset_id.clone(),
				&HoldReason::Airdrop.into(),
				&pot,
				to_hold,
			)?;

			let next_timestamp = info.registration_starts;
			Events::<T>::insert(
				event_id,
				ActiveEvent { id: event_id, info, status: Status::Scheduled },
			);
			ActionSchedule::<T>::insert(BigEndianU64(next_timestamp), event_id, ());

			Self::deposit_event(Event::<T>::EventScheduled { event_id });
			Ok(())
		}

		/// Remove a scheduled event.
		pub(crate) fn do_remove_scheduled(event_id: EventId) -> DispatchResult {
			let event = Events::<T>::get(event_id).ok_or(Error::<T>::NoScheduledEvent)?;
			ensure!(matches!(event.status, Status::Scheduled), Error::<T>::WrongStatus);

			Self::refund_and_drop_event(event);
			Self::deposit_event(Event::<T>::ScheduledEventRemoved { event_id });
			Ok(())
		}

		/// Cancel an event in any state.
		///
		/// - `Scheduled`: no registrations exist yet, so the full hold is released and the event
		///   dropped immediately.
		/// - `Registering`: `close_registration` never ran, so no slots have been released. The
		///   full prize allocation is released here at the cancel transition (mirroring the
		///   unused-slot release `close_registration` would have performed); the event then enters
		///   `ClearingRegistrations` with `effective_winners: 0`, so `Finalizing` releases 0 more.
		/// - `DrawWinners` / `Claiming`: `close_registration` already released the unused slots, so
		///   only `effective_winners - claimed` remains held. The cancel routes the event into
		///   `ClearingRegistrations` and the `Finalizing` step releases that remainder. From this
		///   point `claim` is rejected (wrong status), and the pallet's offchain worker drains
		///   `Registrations`/`Winners`.
		/// - Already terminating or unknown: no-op.
		pub(crate) fn do_cancel(event_id: EventId) -> DispatchResult {
			let Some(mut event) = Events::<T>::get(event_id) else {
				return Ok(());
			};

			let clearing = match event.status {
				Status::Scheduled => {
					Self::refund_and_drop_event(event);
					Self::deposit_event(Event::<T>::EventCancelled { event_id });
					return Ok(());
				},
				Status::ClearingRegistrations { .. } |
				Status::ClearingWinners { .. } |
				Status::Finalizing { .. } => return Ok(()),
				// `close_registration` never ran — release the full allocation here so funds are
				// freed promptly rather than waiting on the clean-up pipeline.
				Status::Registering { total_participants } => {
					Self::release_remaining_prizes(&event.info.prize, event.info.prize.max_winners);
					Status::ClearingRegistrations {
						total_participants,
						effective_winners: 0,
						claimed: 0,
						cleaned_registrations: 0,
					}
				},
				Status::DrawWinners { total_participants, effective_winners, .. } =>
					Status::ClearingRegistrations {
						total_participants,
						effective_winners,
						claimed: 0,
						cleaned_registrations: 0,
					},
				Status::Claiming { total_participants, effective_winners, claimed } =>
					Status::ClearingRegistrations {
						total_participants,
						effective_winners,
						claimed,
						cleaned_registrations: 0,
					},
			};

			Self::transition(&mut event, clearing);
			Events::<T>::insert(event_id, event);
			Self::deposit_event(Event::<T>::EventCancelled { event_id });
			Ok(())
		}

		/// Release the entire still-held prize allocation, drop the event's `ActionSchedule` entry,
		/// and remove the event from `Events`.
		///
		/// Useful when an event terminates without ever distributing prizes.
		fn refund_and_drop_event(event: ActiveEventOf<T>) {
			let event_id = event.id;
			Self::release_remaining_prizes(&event.info.prize, event.info.prize.max_winners);
			let timestamp = Self::next_action_scheduled_at(&event.status, &event.info);
			ActionSchedule::<T>::remove(BigEndianU64(timestamp), event_id);
			Events::<T>::remove(event_id);
		}

		/// Timestamp at which an event in `status` has its next lifecycle
		/// action scheduled.
		pub(crate) fn next_action_scheduled_at(status: &Status, info: &EventInfoOf<T>) -> u64 {
			match status {
				Status::Scheduled => info.registration_starts,
				Status::Registering { .. } | Status::DrawWinners { .. } => info.draw_time,
				Status::Claiming { .. } |
				Status::ClearingRegistrations { .. } |
				Status::ClearingWinners { .. } |
				Status::Finalizing { .. } => info.end_time,
			}
		}

		/// Apply a status transition, moving the event's `ActionSchedule`
		/// entry if the corresponding timestamp changes.
		fn transition(event: &mut ActiveEventOf<T>, new_status: Status) {
			let old_timestamp = Self::next_action_scheduled_at(&event.status, &event.info);
			let new_timestamp = Self::next_action_scheduled_at(&new_status, &event.info);
			event.status = new_status;
			if old_timestamp != new_timestamp {
				ActionSchedule::<T>::remove(BigEndianU64(old_timestamp), event.id);
				ActionSchedule::<T>::insert(BigEndianU64(new_timestamp), event.id, ());
			}
		}

		/// Body of `clean_up_registrations_authorized`, but parameterised by `clear_count` so
		/// benchmarks can drive `clear_prefix` directly. The extrinsic always passes
		/// `T::ClearLimit::get()`.
		pub(crate) fn clean_up_registrations_inner(
			event_id: EventId,
			clear_count: u32,
		) -> DispatchResultWithPostInfo {
			let mut event = Events::<T>::get(event_id).ok_or(Error::<T>::UnknownEvent)?;
			let Status::ClearingRegistrations {
				total_participants,
				effective_winners,
				claimed,
				cleaned_registrations,
			} = event.status
			else {
				defensive!("clean_up_registrations: authorize verified status");
				return Err(Error::<T>::WrongStatus.into());
			};

			let res = Registrations::<T>::clear_prefix(event_id, clear_count, None);
			let transitioned = res.maybe_cursor.is_none();

			let mut actual_weight = T::WeightInfo::clean_up_registrations(res.unique);
			if transitioned {
				Self::transition_clean_up_phase(
					event,
					Status::ClearingWinners {
						total_participants,
						effective_winners,
						claimed,
						cleaned_winners: 0,
					},
				);
				actual_weight =
					actual_weight.saturating_add(T::WeightInfo::transition_clean_up_phase());
			} else {
				event.status = Status::ClearingRegistrations {
					total_participants,
					effective_winners,
					claimed,
					cleaned_registrations: cleaned_registrations.saturating_add(res.unique),
				};
				Events::<T>::insert(event_id, event);
			}
			Ok(PostDispatchInfo { actual_weight: Some(actual_weight), pays_fee: Pays::No })
		}

		/// Body of `clean_up_winners_authorized`, but parameterised by `clear_count` so benchmarks
		/// can drive `clear_prefix` directly. The extrinsic always passes `T::ClearLimit::get()`.
		pub(crate) fn clean_up_winners_inner(
			event_id: EventId,
			clear_count: u32,
		) -> DispatchResultWithPostInfo {
			let mut event = Events::<T>::get(event_id).ok_or(Error::<T>::UnknownEvent)?;
			let Status::ClearingWinners {
				total_participants,
				effective_winners,
				claimed,
				cleaned_winners,
			} = event.status
			else {
				defensive!("clean_up_winners: authorize verified status");
				return Err(Error::<T>::WrongStatus.into());
			};

			let res = Winners::<T>::clear_prefix(event_id, clear_count, None);
			let transitioned = res.maybe_cursor.is_none();

			let mut actual_weight = T::WeightInfo::clean_up_winners(res.unique);
			if transitioned {
				Self::transition_clean_up_phase(
					event,
					Status::Finalizing { effective_winners, claimed },
				);
				actual_weight =
					actual_weight.saturating_add(T::WeightInfo::transition_clean_up_phase());
			} else {
				event.status = Status::ClearingWinners {
					total_participants,
					effective_winners,
					claimed,
					cleaned_winners: cleaned_winners.saturating_add(res.unique),
				};
				Events::<T>::insert(event_id, event);
			}
			Ok(PostDispatchInfo { actual_weight: Some(actual_weight), pays_fee: Pays::No })
		}

		/// Flip status to the next clean-up phase, persist, and deposit the matching event.
		///
		/// Used in benchmarks to account for the separate weight of the transition in the
		/// registrations and winners clean-up phases.
		pub(crate) fn transition_clean_up_phase(mut event: ActiveEventOf<T>, next_status: Status) {
			let event_id = event.id;
			let ev = match &next_status {
				Status::ClearingWinners { .. } => Event::<T>::ClearingWinners { event_id },
				Status::Finalizing { .. } => Event::<T>::FinalizingEvent { event_id },
				_ => {
					defensive!(
						"airdrop: transition_clean_up_phase called with non-clean-up status"
					);
					return;
				},
			};
			Self::transition(&mut event, next_status);
			Events::<T>::insert(event_id, event);
			Self::deposit_event(ev);
		}

		/// Ensure the source of the transaction is `Local`.
		pub(crate) fn ensure_local(
			source: TransactionSource,
		) -> Result<(), TransactionValidityError> {
			if !matches!(source, TransactionSource::InBlock | TransactionSource::Local) {
				return Err(AuthorizeInvalidity::TransactionNotLocal.into());
			}
			Ok(())
		}

		/// Build tags for the OCW transactions.
		pub(crate) fn valid_for(
			tag_prefix: &'static str,
			info: impl Encode,
		) -> Result<(ValidTransaction, Weight), TransactionValidityError> {
			let v = ValidTransaction::with_tag_prefix(tag_prefix).and_provides(info).build()?;
			Ok((v, Weight::zero()))
		}

		/// Register a participant to an event.
		///
		/// IMPORTANT! This function assumes a participant origin, account or alias, can always only
		/// produce the associated `slot` entropy for the same context of a particular event.
		fn register_slot(
			event_id: EventId,
			slot: BigEndianU256,
			entry: RegistrationEntryOf<T>,
		) -> DispatchResult {
			let mut event = Events::<T>::get(event_id).ok_or(Error::<T>::UnknownEvent)?;
			let Status::Registering { total_participants } = event.status else {
				return Err(Error::<T>::NotAcceptingRegistrations.into());
			};
			ensure!(
				!Registrations::<T>::contains_key(event_id, slot),
				Error::<T>::EntropySlotTaken
			);
			event.status = Status::Registering {
				total_participants: total_participants
					.checked_add(1)
					.ok_or(Error::<T>::ParticipantOverflow)?,
			};
			Registrations::<T>::insert(event_id, slot, entry);
			Events::<T>::insert(event_id, event);
			Ok(())
		}

		/// Register with a personal alias and a proof of membership in the People collection. The
		/// proof is verified against the specific context of this event and generates a unique
		/// alias which serves as the participant's ticket.
		///
		/// The participant alias origin is authorized by the caller of this function to participate
		/// in the lottery and it is the caller's responsibility to ensure it's eligible.
		pub(crate) fn do_participate_with_alias(
			event_id: EventId,
			participant_origin: RegistrationEntryOf<T>,
			proof: ProofOf<T>,
			ring_index: RingIndex,
			revision: RevisionIndex,
		) -> DispatchResult {
			let context = context_for_event(&event_id);
			let msg = participant_origin.encode();
			let event_alias = T::MemberService::verify_membership_at_rev(
				PEOPLE_IDENTIFIER,
				&proof,
				ring_index,
				revision,
				context,
				&msg,
			)
			.map_err(|_| Error::<T>::InvalidMembershipProof)?
			.alias;
			let slot = BigEndianU256::from(event_alias);
			Self::register_slot(event_id, slot, participant_origin.clone())?;
			Self::deposit_event(Event::<T>::AliasRegistered { event_id, slot, participant_origin });
			Ok(())
		}

		/// Register with an account and an sr25519 VRF signature of the account. This enforces that
		/// the account must map to an sr25519 address. The signature is verified against the
		/// specific context of this event and generates a VRF output which serves as the
		/// participant's ticket.
		///
		/// The participant account origin is authorized by the caller of this function to
		/// participate in the lottery and it is the caller's responsibility to ensure it's
		/// eligible.
		pub(crate) fn do_participate_with_account(
			account_id: T::AccountId,
			event_id: EventId,
			signature: VrfSignature,
		) -> DispatchResult {
			let public = T::AccountIdToPublic::try_convert(account_id.clone())
				.map_err(|_| Error::<T>::UnsupportedAccountKey)?;
			let entropy = vrf::verify_and_extract_entropy(&public, &event_id, &signature)
				.ok_or(Error::<T>::InvalidVrfProof)?;
			let slot = BigEndianU256::from(entropy);
			Self::register_slot(
				event_id,
				slot,
				RegistrationEntry::Account { account_id: account_id.clone() },
			)?;
			Self::deposit_event(Event::<T>::AccountRegistered { event_id, slot, account_id });
			Ok(())
		}

		/// Claim a participant's winning ticket in the account of their choice.
		pub(crate) fn do_claim(
			event_id: EventId,
			registrant: RegistrationEntryOf<T>,
			beneficiary: T::AccountId,
		) -> DispatchResult {
			let mut event = Events::<T>::get(event_id).ok_or(Error::<T>::UnknownEvent)?;
			let Status::Claiming { total_participants, effective_winners, claimed } = event.status
			else {
				return Err(Error::<T>::NotClaiming.into());
			};
			// Reject claims once the event's claim window has elapsed. The status-driven
			// lifecycle eventually transitions out of `Claiming` via the OCW, but a late OCW
			// must not let a claim sneak in past `end_time`.
			ensure!(
				T::UnixTime::now().as_secs() < event.info.end_time,
				Error::<T>::ClaimingWindowClosed,
			);
			// Refuse to pay out from an asset which was disabled.
			ensure!(
				SupportedAssets::<T>::contains_key(&event.info.prize.asset_id),
				Error::<T>::AssetNotEnabled,
			);

			let slot = Winners::<T>::get(event_id, &registrant).ok_or(Error::<T>::NoSuchWinner)?;

			let pot = Self::airdrop_pot_id();
			with_storage_layer::<_, DispatchError, _>(|| {
				let released = T::Fungibles::release(
					event.info.prize.asset_id.clone(),
					&HoldReason::Airdrop.into(),
					&pot,
					event.info.prize.asset_amount,
					Precision::Exact,
				)?;
				defensive_assert!(
					released == event.info.prize.asset_amount,
					"airdrop: release matches prize"
				);
				T::Fungibles::transfer(
					event.info.prize.asset_id.clone(),
					&pot,
					&beneficiary,
					event.info.prize.asset_amount,
					Preservation::Expendable,
				)?;
				Ok(())
			})?;

			Winners::<T>::remove(event_id, &registrant);
			// Keep track of the claims so the `Finalizing` step refunds the
			// remainder computed by `effective_winners - claimed`.
			event.status = Status::Claiming {
				total_participants,
				effective_winners,
				claimed: claimed.saturating_add(1),
			};
			let asset_id = event.info.prize.asset_id.clone();
			let amount = event.info.prize.asset_amount;
			Events::<T>::insert(event_id, event);

			Self::deposit_event(Event::<T>::PrizeClaimed {
				event_id,
				slot,
				beneficiary,
				asset_id,
				amount,
			});
			Ok(())
		}

		fn release_remaining_prizes(prize: &AirdropPrizeOf<T>, count: u32) {
			if count == 0 {
				return;
			}
			let to_release = prize.asset_amount.saturating_mul(count.into());
			let pot = Self::airdrop_pot_id();
			match T::Fungibles::release(
				prize.asset_id.clone(),
				&HoldReason::Airdrop.into(),
				&pot,
				to_release,
				Precision::BestEffort,
			) {
				Ok(released) if released < to_release => log::warn!(
					target: LOG_TARGET,
					"airdrop: released {released:?} of {to_release:?} held prize funds; \
					 expected to release the full amount",
				),
				Err(e) => log::warn!(
					target: LOG_TARGET,
					"airdrop: failed to release {to_release:?} of held prize funds: {e:?}",
				),
				_ => {},
			}
		}

		pub(crate) fn submit_authorized_transaction(call: Call<T>) {
			let (name, event_id) = match &call {
				Call::start_registration_authorized { event_id, .. } =>
					("start_registration_authorized", *event_id),
				Call::close_registration_authorized { event_id, .. } =>
					("close_registration_authorized", *event_id),
				Call::draw_winners_authorized { event_id, .. } =>
					("draw_winners_authorized", *event_id),
				Call::close_drawing_authorized { event_id, .. } =>
					("close_drawing_authorized", *event_id),
				Call::close_claiming_authorized { event_id, .. } =>
					("close_claiming_authorized", *event_id),
				Call::clean_up_registrations_authorized { event_id, .. } =>
					("clean_up_registrations_authorized", *event_id),
				Call::clean_up_winners_authorized { event_id, .. } =>
					("clean_up_winners_authorized", *event_id),
				Call::finalize_authorized { event_id, .. } => ("finalize_authorized", *event_id),
				_ => {
					defensive!("submit_authorized_transaction received a non-OCW call");
					return;
				},
			};
			let display_id = HexDisplay::from(&event_id);
			let tx = T::create_authorized_transaction(call.into());
			match SubmitTransaction::<T, _>::submit_transaction(tx) {
				Ok(()) => log::debug!(
					target: LOG_TARGET,
					"airdrop: submitted `{name}` for event 0x{display_id}",
				),
				Err(()) => log::warn!(
					target: LOG_TARGET,
					"airdrop: failed to submit `{name}` for event 0x{display_id}",
				),
			}
		}
	}

	impl<T: Config> crate::types::Airdrop<T::AccountId, AssetIdOf<T>, AssetBalanceOf<T>> for Pallet<T> {
		type EventInfo = EventInfoOf<T>;
		type Proof = ProofOf<T>;

		fn schedule(
			source: T::AccountId,
			event_id: EventId,
			info: Self::EventInfo,
		) -> DispatchResult {
			// Trait callers (other pallets) bring their own prize funds: transfer the prize
			// allocation from `source` to the pallet's pot, then schedule.
			let pot = Self::airdrop_pot_id();
			let to_transfer = info
				.prize
				.asset_amount
				.checked_mul(&info.prize.max_winners.into())
				.ok_or(Error::<T>::PrizeAllocationOverflow)?;
			let asset_id = info.prize.asset_id.clone();
			// Transfer can fail or the scheduling can fail, revert both if either fails.
			with_storage_layer::<_, DispatchError, _>(|| {
				T::Fungibles::transfer(
					asset_id,
					&source,
					&pot,
					to_transfer,
					Preservation::Expendable,
				)?;
				Self::do_schedule(event_id, info)
			})
		}

		fn remove_scheduled(event_id: EventId) {
			let _ = Self::do_remove_scheduled(event_id);
		}

		fn cancel(event_id: EventId) {
			let _ = Self::do_cancel(event_id);
		}

		fn participate_with_alias(
			event_id: EventId,
			participant_origin: RegistrationEntryOf<T>,
			proof: Self::Proof,
			ring_index: RingIndex,
			revision: RevisionIndex,
		) -> DispatchResult {
			Self::do_participate_with_alias(
				event_id,
				participant_origin,
				proof,
				ring_index,
				revision,
			)
		}

		fn participate_with_account(
			account_id: T::AccountId,
			event_id: EventId,
			signature: VrfSignature,
		) -> DispatchResult {
			Self::do_participate_with_account(account_id, event_id, signature)
		}

		fn claim(
			event_id: EventId,
			registrant: RegistrationEntryOf<T>,
			beneficiary: T::AccountId,
		) -> DispatchResult {
			Self::do_claim(event_id, registrant, beneficiary)
		}
	}
}
