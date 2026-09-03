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

//! Manager-scheduled airdrop draws for proven people.
//!
//! A manager origin schedules batches of draws through the [`Airdrop`] trait, funded from a
//! configured prize source account which also receives all refunds. Proven people, authenticated
//! per call through a person origin ([`Config::EnsurePerson`]) under
//! [`Pallet::people_airdrops_context`], register for any open draws and claim prizes to a
//! destination of their choice.
//!
//! A draw's salt entry is dead once the draw's end time has passed; the pallet's offchain worker
//! then removes it through authorized [`Pallet::clean_up_draw_salt`] transactions.
//!
//! A participant's entropy slot is `blake2_256(SLOT_DOMAIN ++ event_id ++ salt ++ alias)`. The
//! alias makes the slot unique per person by construction. The per-draw `salt`, captured from the
//! randomness source at scheduling time, makes slots unpredictable before the draw exists, so no
//! earlier secret choice (such as a personhood key) can be ground to position a slot. A person
//! chooses their alias in this context once, when they register their bandersnatch key as a
//! person. That choice fixes their slots in all draws open at that moment. This is accepted as the
//! entropy captured at draw time still decides the winners. The pallet targets frequent small
//! draws, so registration verifies a (cheaper) hash instead of a ring proof.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub use pallet::*;
pub use weights::WeightInfo;

use frame_support::pallet_prelude::*;
use frame_system::{
	offchain::{CreateAuthorizedTransaction, SubmitTransaction},
	pallet_prelude::*,
};
use indiv_pallet_airdrop::types::{Airdrop, EventId, EventInfo, RegistrationEntry};
use indiv_support::{
	traits::{Alias, Context, MomentRandomness},
	tx_priority,
	utils::BigEndianU256,
	weight_budget::OcwWeightBudget,
};
use sp_core::hexdisplay::HexDisplay;
use sp_crypto_hashing::blake2_256;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::traits::{EnsureOriginWithArg, UnixTime};

	/// The log target for the pallet.
	pub(crate) const LOG_TARGET: &str = "runtime::indiv-pallet-people-airdrops";

	/// Domain separator for entropy slot derivation.
	pub const SLOT_DOMAIN: [u8; 24] = *b"pop:people-airdrops:slot";

	/// Prefix of every draw event id (see [`Pallet::draw_event_id`]).
	pub const EVENT_ID_BASE: [u8; 24] = *b"pop:people-airdrops:    ";

	/// `EventInfo` for the airdrop API.
	pub type AirdropEventInfoOf<T> =
		EventInfo<<T as Config>::AirdropAssetId, <T as Config>::AirdropAssetBalance>;

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + CreateAuthorizedTransaction<Call<Self>> {
		type WeightInfo: WeightInfo;

		/// Runtime-wide network suffix used to derive product contexts.
		type Suffix: Get<indiv_support::context::ProductContextNetworkSuffix>;

		/// Person origin resolving to the caller's alias in the supplied context. Only people in
		/// the People collection can produce it, which is the pallet's entire airdrop eligibility
		/// check.
		type EnsurePerson: EnsureOriginWithArg<OriginFor<Self>, Context, Success = Alias>;

		/// Asset id used by [`Self::Airdrop`].
		type AirdropAssetId: Parameter + MaxEncodedLen + Default;

		/// Asset balance used by [`Self::Airdrop`].
		type AirdropAssetBalance: Parameter + MaxEncodedLen + Copy + Default + From<u32>;

		/// The airdrop implementation running the draws.
		type Airdrop: Airdrop<
			Self::AccountId,
			Self::AirdropAssetId,
			Self::AirdropAssetBalance,
			EventInfo = AirdropEventInfoOf<Self>,
		>;

		/// Origin allowed to schedule, remove and cancel draws.
		type ManagerOrigin: EnsureOrigin<Self::RuntimeOrigin>;

		/// Account funding the prize allocation of every scheduled draw. Refunds of unused and
		/// unclaimed prizes flow back to it.
		type PrizeSource: Get<Self::AccountId>;

		/// Randomness source supplying per-draw slot salts at scheduling time. The same salt can be
		/// reused in multiple airdrops.
		type Randomness: MomentRandomness<u32>;

		/// Unix time source, used to gate salt clean-up on the draw's end time.
		type UnixTime: UnixTime;

		/// Maximum number of draws scheduled per `schedule_draws` call.
		#[pallet::constant]
		type MaxScheduleBatch: Get<u32>;

		/// Maximum number of draws registered for per `register` call.
		#[pallet::constant]
		type MaxRegisterBatch: Get<u32>;

		/// Benchmark helper.
		#[cfg(feature = "runtime-benchmarks")]
		type BenchmarkHelper: crate::benchmarking::BenchmarkHelper<Self>;
	}

	#[pallet::extra_constants]
	impl<T: Config> Pallet<T> {
		/// The context used to authenticate people participating in airdrops.
		pub fn people_airdrops_context() -> Context {
			indiv_support::context::build_product_context(
				indiv_support::context::personhood::PRODUCT_NAME,
				&T::Suffix::get(),
				indiv_support::context::personhood::PEOPLE_AIRDROPS,
			)
		}
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn integrity_test() {
			let budget = OcwWeightBudget::from_normal_max::<T>();
			budget.assert_fits(
				"clean_up_draw_salt",
				T::WeightInfo::clean_up_draw_salt()
					.saturating_add(T::WeightInfo::authorize_clean_up_draw_salt()),
			);
		}

		fn offchain_worker(_block_number: BlockNumberFor<T>) {
			let now = T::UnixTime::now().as_secs();
			for (event_id, (_salt, end_time)) in DrawSalts::<T>::iter() {
				if now <= end_time {
					continue;
				}
				let display_id = HexDisplay::from(&event_id);
				let tx =
					T::create_authorized_transaction(Call::clean_up_draw_salt { event_id }.into());
				match SubmitTransaction::<T, _>::submit_transaction(tx) {
					Ok(()) => log::debug!(
						target: LOG_TARGET,
						"people-airdrops: submitted `clean_up_draw_salt` for event 0x{display_id}",
					),
					Err(()) => log::warn!(
						target: LOG_TARGET,
						"people-airdrops: failed to submit `clean_up_draw_salt` for event \
						 0x{display_id}",
					),
				}
			}
		}
	}

	/// Index of the next draw to schedule. Feeds [`Pallet::draw_event_id`].
	#[pallet::storage]
	pub type NextDrawIndex<T> = StorageValue<_, u64, ValueQuery>;

	/// Per-draw slot salt, captured from the randomness source at scheduling time, together with
	/// the draw's end time. The salt is only read during registration, which closes strictly
	/// before the end time, so an entry whose end time has passed is dead and is removed by the
	/// offchain worker via [`Pallet::clean_up_draw_salt`].
	#[pallet::storage]
	pub type DrawSalts<T> = StorageMap<_, Twox64Concat, EventId, ([u8; 32], u64)>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// A draw was scheduled. Registration and claim events are emitted by the airdrop
		/// implementation under this `event_id`.
		DrawScheduled { draw_index: u64, event_id: EventId },
		/// A scheduled draw was asked to be removed before opening.
		DrawRemoved { event_id: EventId },
		/// A draw was asked to be cancelled.
		DrawCancelled { event_id: EventId },
		/// A draw's salt entry was removed after its end time.
		DrawSaltRemoved { event_id: EventId },
	}

	#[pallet::error]
	pub enum Error<T> {
		/// The randomness source has no value to salt the scheduled draws with.
		RandomnessUnavailable,
		/// No draw with this event id was scheduled by this pallet.
		UnknownDraw,
		/// `register` was called with an empty batch.
		EmptyRegistration,
	}

	/// Custom transaction-validity errors.
	#[repr(u8)]
	pub enum AuthorizeInvalidity {
		/// Transaction source is not local or in block.
		TransactionNotLocal = 200,
		/// `event_id` has no [`DrawSalts`] entry.
		UnknownDraw = 201,
	}

	impl From<AuthorizeInvalidity> for TransactionValidityError {
		fn from(e: AuthorizeInvalidity) -> Self {
			InvalidTransaction::Custom(e as u8).into()
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Schedule a batch of draws, in order, assigning each the next draw index. The prize
		/// allocation of every draw is transferred from [`Config::PrizeSource`] into the airdrop
		/// pot.
		///
		/// If any event fails to be scheduled, the whole call fails and no events are scheduled.
		#[pallet::call_index(0)]
		#[pallet::weight(T::WeightInfo::schedule_draws(draws.len() as u32))]
		pub fn schedule_draws(
			origin: OriginFor<T>,
			draws: BoundedVec<AirdropEventInfoOf<T>, T::MaxScheduleBatch>,
		) -> DispatchResult {
			T::ManagerOrigin::ensure_origin(origin)?;
			let (salt, _moment) =
				T::Randomness::randomness().ok_or(Error::<T>::RandomnessUnavailable)?;
			for info in draws {
				let draw_index = NextDrawIndex::<T>::mutate(|index| {
					let current = *index;
					*index = index.saturating_add(1);
					current
				});
				let event_id = Self::draw_event_id(draw_index);
				let end_time = info.end_time;
				T::Airdrop::schedule(T::PrizeSource::get(), event_id, info)?;
				DrawSalts::<T>::insert(event_id, (salt, end_time));
				Self::deposit_event(Event::<T>::DrawScheduled { draw_index, event_id });
			}
			Ok(())
		}

		/// Remove a scheduled draw that has not opened yet, refunding its prize allocation to
		/// [`Config::PrizeSource`]. Best-effort: a no-op for an unknown or already-opened draw.
		/// The draw's salt is kept, since the draw may still be live (see [`DrawSalts`]).
		#[pallet::call_index(1)]
		#[pallet::weight(T::WeightInfo::remove_scheduled_draw())]
		pub fn remove_scheduled_draw(origin: OriginFor<T>, event_id: EventId) -> DispatchResult {
			T::ManagerOrigin::ensure_origin(origin)?;
			ensure!(DrawSalts::<T>::contains_key(event_id), Error::<T>::UnknownDraw);
			T::Airdrop::remove_scheduled(event_id);
			Self::deposit_event(Event::<T>::DrawRemoved { event_id });
			Ok(())
		}

		/// Cancel a draw in any state: participation and claims close immediately and the
		/// still-held prize allocation is refunded to [`Config::PrizeSource`] by the airdrop
		/// clean-up. Best-effort: a no-op for an unknown or already-terminating draw.
		#[pallet::call_index(2)]
		#[pallet::weight(T::WeightInfo::cancel_draw())]
		pub fn cancel_draw(origin: OriginFor<T>, event_id: EventId) -> DispatchResult {
			T::ManagerOrigin::ensure_origin(origin)?;
			ensure!(DrawSalts::<T>::contains_key(event_id), Error::<T>::UnknownDraw);
			T::Airdrop::cancel(event_id);
			Self::deposit_event(Event::<T>::DrawCancelled { event_id });
			Ok(())
		}

		/// Register the calling person for every draw in `event_ids`. All or nothing: any failure
		/// (unknown draw, draw not open, already registered) reverts the whole batch.
		///
		/// Free on success. Success is bounded to one registration per person and draw, so
		/// repeated calls fail and pay.
		#[pallet::call_index(3)]
		#[pallet::weight(T::WeightInfo::register(event_ids.len() as u32))]
		pub fn register(
			origin: OriginFor<T>,
			event_ids: BoundedVec<EventId, T::MaxRegisterBatch>,
		) -> DispatchResultWithPostInfo {
			let alias = T::EnsurePerson::ensure_origin(origin, &Self::people_airdrops_context())?;
			ensure!(!event_ids.is_empty(), Error::<T>::EmptyRegistration);
			for event_id in event_ids {
				let (salt, _end_time) =
					DrawSalts::<T>::get(event_id).ok_or(Error::<T>::UnknownDraw)?;
				let slot = Self::slot_for(&event_id, &salt, &alias);
				T::Airdrop::participate_with_slot(
					event_id,
					slot,
					RegistrationEntry::Alias { alias },
				)?;
			}
			Ok(Pays::No.into())
		}

		/// Claim the calling person's prize in the draw `event_id`, paying it out to
		/// `destination`. Only drawn winners hold a claimable prize, and only until the draw's
		/// claim window closes.
		///
		/// Success is bounded to one claim per person and draw, so repeated
		/// calls fail and pay.
		#[pallet::call_index(4)]
		#[pallet::weight(T::WeightInfo::claim())]
		pub fn claim(
			origin: OriginFor<T>,
			event_id: EventId,
			destination: T::AccountId,
		) -> DispatchResultWithPostInfo {
			let alias = T::EnsurePerson::ensure_origin(origin, &Self::people_airdrops_context())?;
			T::Airdrop::claim(event_id, RegistrationEntry::Alias { alias }, destination)?;
			Ok(Pays::No.into())
		}

		/// OCW-driven: remove a draw's [`DrawSalts`] entry once the draw's end time has passed. The
		/// salt is only read during registration, which closes strictly before the end time, so
		/// the entry is dead by then.
		///
		/// The transaction is only accepted from a local or in-block source, so it cannot be
		/// submitted externally.
		#[pallet::authorize(Pallet::<T>::authorize_clean_up_draw_salt)]
		#[pallet::call_index(5)]
		#[pallet::weight(T::WeightInfo::clean_up_draw_salt())]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_clean_up_draw_salt())]
		pub fn clean_up_draw_salt(origin: OriginFor<T>, event_id: EventId) -> DispatchResult {
			ensure_authorized(origin)?;
			ensure!(DrawSalts::<T>::contains_key(event_id), Error::<T>::UnknownDraw);
			DrawSalts::<T>::remove(event_id);
			Self::deposit_event(Event::<T>::DrawSaltRemoved { event_id });
			Ok(())
		}
	}

	impl<T: Config> Pallet<T> {
		/// Authorize the clean-up of a draw's [`DrawSalts`] entry.
		///
		/// Only local or in-block sources are accepted. Rejects with
		/// [`InvalidTransaction::Future`] while the draw's end time has not passed, so the pool
		/// retains the transaction for later revalidation.
		pub(crate) fn authorize_clean_up_draw_salt(
			source: TransactionSource,
			event_id: &EventId,
		) -> TransactionValidityWithRefund {
			if !matches!(source, TransactionSource::InBlock | TransactionSource::Local) {
				return Err(AuthorizeInvalidity::TransactionNotLocal.into());
			}
			let (_salt, end_time) =
				DrawSalts::<T>::get(event_id).ok_or(AuthorizeInvalidity::UnknownDraw)?;
			if T::UnixTime::now().as_secs() <= end_time {
				return Err(InvalidTransaction::Future.into());
			}
			let validity = ValidTransaction::with_tag_prefix("people-airdrops:clean-salt")
				.and_provides(event_id)
				.propagate(false)
				.priority(tx_priority::CLEANUP)
				.build()?;
			Ok((validity, Weight::zero()))
		}

		/// Deterministic event id of the draw at `draw_index`: [`EVENT_ID_BASE`] concatenated
		/// with the big-endian encoded index.
		pub fn draw_event_id(draw_index: u64) -> EventId {
			let mut event_id = [0u8; 32];
			event_id[..24].copy_from_slice(&EVENT_ID_BASE);
			event_id[24..].copy_from_slice(&draw_index.to_be_bytes());
			event_id
		}

		/// Entropy slot of `alias` in a draw. Deterministic per (person, draw), so one person can
		/// never occupy two slots; dependent on the draw's salt, so unpredictable before the draw
		/// was scheduled.
		pub fn slot_for(event_id: &EventId, salt: &[u8; 32], alias: &Alias) -> BigEndianU256 {
			let mut buf = [0u8; 120];
			buf[..24].copy_from_slice(&SLOT_DOMAIN);
			buf[24..56].copy_from_slice(event_id);
			buf[56..88].copy_from_slice(salt);
			buf[88..].copy_from_slice(alias);
			BigEndianU256::from(blake2_256(&buf))
		}
	}
}
