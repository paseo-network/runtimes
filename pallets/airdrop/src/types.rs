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

//! Types for the airdrop pallet.

use super::*;
use frame_support::pallet_prelude::*;
use indiv_support::{
	traits::{Alias, MembershipProver, RevisionIndex, RingIndex},
	utils::{BigEndianU256, BigEndianU64},
};
use sp_core::sr25519::vrf::VrfSignature;
use sp_runtime::Permill;
use verifiable::GenerateVerifiable;

/// A 32-byte event identifier supplied by the scheduling caller.
pub type EventId = [u8; 32];

pub type AccountIdOf<T> = <T as frame_system::Config>::AccountId;
pub type AssetIdOf<T> =
	<<T as Config>::Fungibles as frame_support::traits::tokens::fungibles::Inspect<
		AccountIdOf<T>,
	>>::AssetId;
pub type AssetBalanceOf<T> =
	<<T as Config>::Fungibles as frame_support::traits::tokens::fungibles::Inspect<
		AccountIdOf<T>,
	>>::Balance;

/// Ring-membership proof type used by alias-based participation.
pub type ProofOf<T> =
	<<<T as Config>::MemberService as MembershipProver>::Crypto as GenerateVerifiable>::Proof;

/// A unix-seconds timestamp used as a key in maps. Stored in big-endian form so we can iterate
/// storage in ascending order.
pub type BigEndianTimestamp = BigEndianU64;

/// Prize specification for an event. Each winner receives `asset_amount` of `asset_id`; the prize
/// fund holds `max_winners * asset_amount` up-front and `winner_cap` further limits the fraction of
/// participants that may win.
#[derive(
	Encode,
	Decode,
	DecodeWithMemTracking,
	Clone,
	PartialEq,
	Eq,
	Debug,
	TypeInfo,
	MaxEncodedLen,
	Default,
)]
pub struct AirdropPrize<AssetId, AssetBalance> {
	pub asset_id: AssetId,
	pub asset_amount: AssetBalance,
	pub max_winners: u32,
	pub winner_cap: Permill,
}

pub type AirdropPrizeOf<T> = AirdropPrize<AssetIdOf<T>, AssetBalanceOf<T>>;

/// Event information.
///
/// General constant information for the event.
#[derive(
	Encode, Decode, DecodeWithMemTracking, Clone, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
)]
pub struct EventInfo<AssetId, AssetBalance> {
	/// The prize specification for the event.
	pub prize: AirdropPrize<AssetId, AssetBalance>,
	/// Unix timestamp, in seconds, at which registration opens.
	pub registration_starts: u64,
	/// Unix timestamp, in seconds, at which registration closes and the draw is performed.
	pub draw_time: u64,
	/// Unix timestamp, in seconds, at which claiming closes and clean up starts.
	pub end_time: u64,
}

pub type EventInfoOf<T> = EventInfo<AssetIdOf<T>, AssetBalanceOf<T>>;

/// Lifecycle status of an active event.
#[derive(
	Encode, Decode, DecodeWithMemTracking, Clone, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
)]
pub enum Status {
	/// Scheduled to start.
	Scheduled,
	/// Accepting registrations until `draw_time`.
	Registering {
		/// Count of registered participants.
		total_participants: u32,
	},
	/// Waiting for randomness produced after the close before seeding the draw.
	AwaitingEntropy {
		/// Frozen registration total at the moment registration closed.
		total_participants: u32,
		/// Actual number of potential winners.
		effective_winners: u32,
		/// The randomness source's commitment moment recorded when registration closed:
		/// an upper bound, at that time, on the moment of every value outside observers
		/// could determine. The draw only proceeds with randomness carrying a strictly
		/// greater moment, i.e. produced after the last possible registration.
		last_moment: u32,
	},
	/// Drawing the winners for this event.
	DrawWinners {
		/// Frozen registration total at the moment draw started.
		total_participants: u32,
		/// Actual number of potential winners.
		effective_winners: u32,
		/// `Winners` entries added so far.
		winners_added: u32,
		/// Cursor for the next batch of added winners. Initialized to the entropy point on the
		/// first batch.
		from_winner_key: BigEndianU256,
	},
	/// Accepting claims until `end_time`.
	Claiming {
		/// Frozen registration total at the moment draw started.
		total_participants: u32,
		/// Actual number of potential winners.
		effective_winners: u32,
		/// Number of winners that have successfully claimed.
		claimed: u32,
	},
	/// Past `end_time`; cleaning `Registrations` in `ClearLimit`-sized batches.
	ClearingRegistrations {
		/// Frozen registration total at the moment draw started.
		total_participants: u32,
		/// Actual number of potential winners.
		effective_winners: u32,
		/// Number of winners that have successfully claimed.
		claimed: u32,
		/// Count of `Registrations` entries removed so far.
		cleaned_registrations: u32,
	},
	/// `Registrations` is empty; now draining `Winners` in batches.
	ClearingWinners {
		/// Frozen registration total at the moment draw started.
		total_participants: u32,
		/// Actual number of potential winners.
		effective_winners: u32,
		/// Number of winners that have successfully claimed.
		claimed: u32,
		/// Count of `Winners` entries removed so far.
		cleaned_winners: u32,
	},
	/// Releasing the unclaimed prize allocation and the event information.
	Finalizing { effective_winners: u32, claimed: u32 },
}

/// Active event record.
#[derive(
	Encode, Decode, DecodeWithMemTracking, Clone, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
)]
pub struct ActiveEvent<AccountId, AssetId, AssetBalance> {
	pub id: EventId,
	pub info: EventInfo<AssetId, AssetBalance>,
	pub status: Status,
	/// Funding source for source-funded events; released prize funds are refunded here. `None` for
	/// pre-funded `schedule_event` events, whose released funds stay in the pot.
	pub source: Option<AccountId>,
}

pub type ActiveEventOf<T> =
	ActiveEvent<<T as frame_system::Config>::AccountId, AssetIdOf<T>, AssetBalanceOf<T>>;

/// A registration entry, stored under the entropy slot key.
#[derive(
	Encode, Decode, DecodeWithMemTracking, Clone, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
)]
pub enum RegistrationEntry<AccountId> {
	Alias { alias: Alias },
	Account { account_id: AccountId },
}

pub type RegistrationEntryOf<T> = RegistrationEntry<<T as frame_system::Config>::AccountId>;

/// The airdrop interface.
///
/// Pallets that use this trait are expected to:
/// - Carry their own user-facing extrinsics for participation and claiming.
/// - Run their own eligibility checks before invoking `claim`.
/// - Coordinate their own event-id allocation.
pub trait Airdrop<AccountId, AssetId, Balance> {
	/// Event information carried at scheduling time.
	type EventInfo;
	/// Ring-membership proof type accepted on alias-based participation.
	type Proof;

	/// Schedule a new airdrop event.
	///
	/// `source` is the account from which the prize allocation (`max_winners × asset_amount`) is
	/// transferred into the pallet's pot account; the funds are then held on the pot under the
	/// airdrop hold reason for the lifetime of the event. The prize asset must have been enabled
	/// for use by the pallet via `enable_asset` first. `event_id` must be unique across all
	/// scheduled and active events.
	fn schedule(source: AccountId, event_id: EventId, info: Self::EventInfo) -> DispatchResult;

	/// Remove a previously scheduled (but not yet started) event and release its held funds.
	/// A no-op for an unknown or already-started event.
	fn remove_scheduled(event_id: EventId);

	/// Cancel an event: participation and claims are immediately closed. A no-op for an
	/// unknown or already-terminating event.
	fn cancel(event_id: EventId);

	/// Register a participant using a personhood ring-membership proof.
	///
	/// The proof is verified under the per-event personhood context; the alias it recovers is the
	/// participant's entropy slot for the draw.
	fn participate_with_alias(
		event_id: EventId,
		participant_origin: RegistrationEntry<AccountId>,
		proof: Self::Proof,
		ring_index: RingIndex,
		revision: RevisionIndex,
	) -> DispatchResult;

	/// Register a participant using an sr25519 VRF signature over the event-specific transcript.
	/// The deterministic 32-byte VRF output is used as the participant's entropy slot.
	///
	/// `account_id` is the participant's runtime account id and it must be sr25519 for this call to
	/// work since that is the only `VrfSignature` supported at this moment.
	fn participate_with_account(
		account_id: AccountId,
		event_id: EventId,
		signature: VrfSignature,
	) -> DispatchResult;

	/// Register a participant under a caller-supplied entropy slot.
	///
	/// The caller must derive `slot` deterministically from the participant identity and the
	/// event, so that one participant cannot occupy two slots in the same event; the pallet only
	/// rejects occupied slots. `entry` is the identifier the participant later claims with.
	fn participate_with_slot(
		event_id: EventId,
		slot: BigEndianU256,
		entry: RegistrationEntry<AccountId>,
	) -> DispatchResult;

	/// Claim a prize on behalf of a winning participant.
	///
	/// `registrant` is the participant's identifier used at registration time.
	///
	/// The registrant must have been picked as a winner by the draw and have an unclaimed entry in
	/// the `Winners` map. However, the calling pallet can impose additional eligibility checks by
	/// gating access to this call.
	fn claim(
		event_id: EventId,
		registrant: RegistrationEntry<AccountId>,
		beneficiary: AccountId,
	) -> DispatchResult;
}
