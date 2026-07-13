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

use crate::{
	mock::*,
	pallet::{ActionSchedule, EventEntropy, Events, Registrations, SupportedAssets, Winners},
	types::Airdrop,
	AirdropPrize, BigEndianU256, EventId, EventInfo, RegistrationEntry, Status,
};
use codec::Encode;
use frame_support::{assert_err, assert_ok, traits::fungibles::Mutate};
use indiv_support::{traits::Alias, utils::BigEndianU64};
use sp_core::sr25519;
use sp_runtime::Permill;

const ASSET_ID: u32 = 42;
const PRIZE_VALUE: u64 = 1_000;
const MIN_BALANCE: u64 = 1;
const POT_FUNDING: u64 = 10_000_000;
/// Account that bankrolls the airdrop prize pool in tests. `schedule` debits
/// this account by `max_winners × PRIZE_VALUE` and credits the pallet's pot.
const SOURCE: u64 = 7;

fn pot() -> u64 {
	<crate::Pallet<Test>>::airdrop_pot_id()
}

fn fund_source() {
	use frame_support::traits::fungibles::{Create, Inspect};
	if !<Assets as Inspect<u64>>::asset_exists(ASSET_ID) {
		<Assets as Create<u64>>::create(ASSET_ID, SOURCE, true, MIN_BALANCE).expect("create asset");
	}
	Assets::mint_into(ASSET_ID, &SOURCE, POT_FUNDING).expect("mint into source");
	enable_default_asset();
}

/// Enable the default `ASSET_ID` for airdrop scheduling, funding the pot's ED from `SOURCE`.
fn enable_default_asset() {
	assert_ok!(crate::Pallet::<Test>::enable_asset(
		frame_system::RawOrigin::Root.into(),
		ASSET_ID,
		SOURCE,
	));
}

fn pot_balance() -> u64 {
	use frame_support::traits::fungibles::Inspect;
	<Assets as Inspect<u64>>::balance(ASSET_ID, &pot())
}

fn pot_balance_on_hold() -> u64 {
	use frame_support::traits::fungibles::InspectHold;
	<AssetsHolder as InspectHold<u64>>::balance_on_hold(
		ASSET_ID,
		&crate::HoldReason::Airdrop.into(),
		&pot(),
	)
}

fn source_balance() -> u64 {
	use frame_support::traits::fungibles::Inspect;
	<Assets as Inspect<u64>>::balance(ASSET_ID, &SOURCE)
}

fn default_prize(max_winners: u32, cap: Permill) -> AirdropPrize<u32, u64> {
	AirdropPrize { asset_id: ASSET_ID, asset_amount: PRIZE_VALUE, max_winners, winner_cap: cap }
}

fn default_info(max_winners: u32, cap: Permill) -> EventInfo<u32, u64> {
	EventInfo {
		prize: default_prize(max_winners, cap),
		registration_starts: 100,
		draw_time: 200,
		end_time: 300,
	}
}

fn event_id(byte: u8) -> EventId {
	[byte; 32]
}

fn alias_from(byte: u8) -> Alias {
	[byte; 32]
}

fn participate_alias(id: EventId, participant_origin: Alias, event_alias: Alias) {
	let entry = RegistrationEntry::<u64>::Alias { alias: participant_origin };
	let msg = entry.encode();
	let proof = mock_proof(&id, event_alias, &msg);
	assert_ok!(crate::Pallet::<Test>::participate_with_alias(id, entry, proof, 0, 0,));
}

/// Drive an event from `Scheduled` to `Claiming` by advancing time and
/// running the OCW.
fn drive_to_claiming(id: EventId, info: EventInfo<u32, u64>, num_participants: u32) {
	assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, id, info.clone()));
	set_now_secs(info.registration_starts as u64);
	run_to_next_ocw(); // Scheduled → Registering
	for i in 0..num_participants {
		let participant = alias_from(0x10 + i as u8);
		let event_alias = alias_from(0xA0 + i as u8);
		participate_alias(id, participant, event_alias);
	}
	set_now_secs(info.draw_time as u64);
	run_to_next_ocw(); // Registering → DrawWinners (close_registration)
	assert!(matches!(Events::<Test>::get(id).unwrap().status, Status::DrawWinners { .. }));
	// Callers of `drive_to_claiming` keep `num_participants` small, so
	// `effective_winners <= DrawLimit` and one draw_winners batch fills
	// everything. If a future caller exceeds `DrawLimit`, this helper needs
	// to loop draw_winners until `winners_added == effective_winners`.
	run_to_next_ocw(); // draw_winners fills the batch (winners_added == effective_winners)
	run_to_next_ocw(); // close_drawing → Claiming
	assert!(matches!(Events::<Test>::get(id).unwrap().status, Status::Claiming { .. }));
}

#[test]
fn schedule_requires_asset_enabled() {
	new_test_ext().execute_with(|| {
		use frame_support::traits::fungibles::{Create, Inspect};
		// Mint funds to source but DON'T enable the asset.
		if !<Assets as Inspect<u64>>::asset_exists(ASSET_ID) {
			<Assets as Create<u64>>::create(ASSET_ID, SOURCE, true, MIN_BALANCE)
				.expect("create asset");
		}
		Assets::mint_into(ASSET_ID, &SOURCE, POT_FUNDING).expect("mint into source");

		let id = event_id(1);
		assert_err!(
			crate::Pallet::<Test>::schedule(SOURCE, id, default_info(1, Permill::one())),
			crate::Error::<Test>::AssetNotEnabled,
		);
	});
}

#[test]
fn enable_then_disable_asset_round_trip() {
	new_test_ext().execute_with(|| {
		use frame_support::traits::fungibles::{Create, Inspect};
		if !<Assets as Inspect<u64>>::asset_exists(ASSET_ID) {
			<Assets as Create<u64>>::create(ASSET_ID, SOURCE, true, MIN_BALANCE)
				.expect("create asset");
		}
		Assets::mint_into(ASSET_ID, &SOURCE, POT_FUNDING).expect("mint into source");

		enable_default_asset();
		assert_eq!(SupportedAssets::<Test>::get(ASSET_ID), Some(MIN_BALANCE));
		assert_eq!(pot_balance(), MIN_BALANCE);
		assert_eq!(source_balance(), POT_FUNDING - MIN_BALANCE);

		// Re-enabling an already-enabled asset is rejected.
		assert_err!(
			crate::Pallet::<Test>::enable_asset(
				frame_system::RawOrigin::Root.into(),
				ASSET_ID,
				SOURCE,
			),
			crate::Error::<Test>::AssetAlreadyEnabled,
		);

		let beneficiary = 88u64;
		assert_ok!(crate::Pallet::<Test>::disable_asset(
			frame_system::RawOrigin::Root.into(),
			ASSET_ID,
			beneficiary,
		));
		assert!(!SupportedAssets::<Test>::contains_key(ASSET_ID));
		assert_eq!(<Assets as Inspect<u64>>::balance(ASSET_ID, &beneficiary), MIN_BALANCE);

		// Disable of a non-enabled asset is rejected.
		assert_err!(
			crate::Pallet::<Test>::disable_asset(
				frame_system::RawOrigin::Root.into(),
				ASSET_ID,
				beneficiary,
			),
			crate::Error::<Test>::AssetNotEnabled,
		);
	});
}

#[test]
fn close_registration_skips_when_randomness_unavailable() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(42);
		let info = default_info(2, Permill::one());
		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, id, info.clone()));
		set_now_secs(info.registration_starts);
		run_to_next_ocw(); // → Registering
		participate_alias(id, alias_from(0x11), alias_from(0xA0));

		// Randomness unavailable: close_registration should leave the event in `Registering` and
		// the OCW must retry on the next pass.
		set_randomness(None);
		set_now_secs(info.draw_time);
		run_to_next_ocw();
		assert!(matches!(Events::<Test>::get(id).unwrap().status, Status::Registering { .. }));
		assert!(EventEntropy::<Test>::get(id).is_none());

		// Once randomness becomes available the next OCW tick advances the event normally.
		set_randomness(Some([7u8; 32]));
		run_to_next_ocw();
		assert!(matches!(Events::<Test>::get(id).unwrap().status, Status::DrawWinners { .. }));
	});
}

#[test]
fn claim_rejected_when_asset_disabled() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(91);
		drive_to_claiming(id, default_info(1, Permill::one()), 1);
		let (registrant, _slot) = Winners::<Test>::iter_prefix(id).next().expect("a winner");

		// Simulate the asset being disabled while an event is still in `Claiming` by removing the
		// entry from `SupportedAssets` directly. The real `disable_asset` extrinsic would fail
		// here because the pot's ED can't leave while the prize hold pins the asset account; this
		// test only exercises the claim-time guard.
		SupportedAssets::<Test>::remove(ASSET_ID);
		let beneficiary = 99u64;
		assert_err!(
			crate::Pallet::<Test>::claim(id, registrant, beneficiary),
			crate::Error::<Test>::AssetNotEnabled,
		);
	});
}

#[test]
fn schedule_then_remove_releases_hold() {
	new_test_ext().execute_with(|| {
		fund_source();
		// `fund_source` enables the asset for scheduling, transferring the asset's ED from
		// `SOURCE` to the pot up front.
		assert_eq!(source_balance(), POT_FUNDING - MIN_BALANCE);
		assert_eq!(pot_balance_on_hold(), 0);
		assert_eq!(pot_balance(), MIN_BALANCE);

		let id = event_id(1);
		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, id, default_info(10, Permill::one())));
		// Source covered just the prize allocation; the pot's ED was funded once at enable time.
		assert_eq!(source_balance(), POT_FUNDING - 10 * PRIZE_VALUE - MIN_BALANCE);
		assert_eq!(pot_balance_on_hold(), 10 * PRIZE_VALUE);
		assert_eq!(pot_balance(), MIN_BALANCE);

		crate::Pallet::<Test>::remove_scheduled(id);
		// Hold released back to the pot's free balance; source is not
		// auto-refunded (callers manage refunds out-of-band if they need to).
		assert_eq!(pot_balance_on_hold(), 0);
		assert_eq!(pot_balance(), 10 * PRIZE_VALUE + MIN_BALANCE);
		assert_eq!(ActionSchedule::<Test>::iter().count(), 0);
	});
}

#[test]
fn cancel_in_registering_releases_full_hold_and_routes_to_clearing() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(1);
		let info = default_info(10, Permill::one());
		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, id, info.clone()));
		// Full allocation held while Scheduled.
		assert_eq!(pot_balance_on_hold(), 10 * PRIZE_VALUE);

		// Advance to `Registering`.
		set_now_secs(info.registration_starts);
		run_to_next_ocw();
		assert!(matches!(Events::<Test>::get(id).unwrap().status, Status::Registering { .. },));
		// `close_registration` never ran, so the full allocation is still held.
		assert_eq!(pot_balance_on_hold(), 10 * PRIZE_VALUE);

		// Cancel mid-`Registering`.
		crate::Pallet::<Test>::cancel(id);

		// The full prize allocation is released at the cancel transition — mirroring the
		// unused-slot release `close_registration` performs in the normal flow. Without this
		// release the funds would sit on the pot's hold until `Finalizing`.
		assert_eq!(pot_balance_on_hold(), 0);
		assert_eq!(pot_balance(), 10 * PRIZE_VALUE + MIN_BALANCE);

		// Event is routed into the clean-up pipeline with no winners to settle, so the
		// subsequent `Finalizing` step releases 0 more.
		let event = Events::<Test>::get(id).expect("event still present");
		assert!(matches!(
			event.status,
			Status::ClearingRegistrations { effective_winners: 0, claimed: 0, .. },
		));
	});
}

#[test]
fn cancel_in_registering_drains_registrations_and_finalizes() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(1);
		let info = default_info(10, Permill::one());
		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, id, info.clone()));

		// Advance to `Registering` and seed one registration so there's something to drain.
		set_now_secs(info.registration_starts);
		run_to_next_ocw();
		participate_alias(id, alias_from(0x11), alias_from(0xA0));
		assert_eq!(Registrations::<Test>::iter_prefix(id).count(), 1);

		crate::Pallet::<Test>::cancel(id);
		assert_eq!(pot_balance_on_hold(), 0);

		// Drive the OCW past `end_time` so the clean-up pipeline runs.
		set_now_secs(info.end_time);
		run_to_next_ocw(); // ClearingRegistrations: drain Registrations → ClearingWinners
		assert_eq!(Registrations::<Test>::iter_prefix(id).count(), 0);
		assert!(matches!(Events::<Test>::get(id).unwrap().status, Status::ClearingWinners { .. },));

		run_to_next_ocw(); // ClearingWinners: no Winners to drain → Finalizing
		assert!(matches!(
			Events::<Test>::get(id).unwrap().status,
			Status::Finalizing { effective_winners: 0, claimed: 0 },
		));

		run_to_next_ocw(); // Finalize: removes the event; release_remaining_prizes is a no-op (0).
		assert!(Events::<Test>::get(id).is_none());
		assert_eq!(ActionSchedule::<Test>::iter().count(), 0);
		// No double-release — funds already came out at cancel time.
		assert_eq!(pot_balance_on_hold(), 0);
	});
}

#[test]
fn cancel_in_draw_winners_routes_to_clearing_and_finalize_releases_remainder() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(2);
		// `max_winners = 10`, `winner_cap = 100%` ⇒ `effective_winners = min(10, n) = n`.
		let info = default_info(10, Permill::one());
		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, id, info.clone()));

		set_now_secs(info.registration_starts);
		run_to_next_ocw(); // → Registering
		let n_participants = 3u32;
		for i in 0..n_participants {
			participate_alias(id, alias_from(0x10 + i as u8), alias_from(0xA0 + i as u8));
		}

		// Transition Registering → DrawWinners. `close_registration` releases the unused
		// `max_winners - effective_winners = 10 - 3 = 7` slots upfront.
		set_now_secs(info.draw_time);
		run_to_next_ocw();
		assert!(matches!(
			Events::<Test>::get(id).unwrap().status,
			Status::DrawWinners { effective_winners: 3, .. },
		));
		assert_eq!(pot_balance_on_hold(), n_participants as u64 * PRIZE_VALUE);

		// Cancel mid-`DrawWinners` (no draw batch run yet, so no `Winners` entries).
		crate::Pallet::<Test>::cancel(id);
		assert!(matches!(
			Events::<Test>::get(id).unwrap().status,
			Status::ClearingRegistrations { effective_winners: 3, claimed: 0, .. },
		));
		// No upfront release: the remaining `effective_winners` worth stays held until
		// `Finalizing`, matching the normal-flow timing.
		assert_eq!(pot_balance_on_hold(), n_participants as u64 * PRIZE_VALUE);

		// Drive the clean-up pipeline.
		set_now_secs(info.end_time);
		run_to_next_ocw(); // drain Registrations → ClearingWinners
		assert_eq!(Registrations::<Test>::iter_prefix(id).count(), 0);
		run_to_next_ocw(); // no Winners to drain → Finalizing
		run_to_next_ocw(); // Finalize: release the remaining 3 slots, remove event
		assert!(Events::<Test>::get(id).is_none());
		assert_eq!(pot_balance_on_hold(), 0);
	});
}

#[test]
fn cancel_in_claiming_routes_to_clearing_preserving_claimed_count() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(3);
		let info = default_info(2, Permill::one());
		drive_to_claiming(id, info.clone(), 2);
		// `close_registration` released 0 unused slots (effective == max), so the full
		// `2 × PRIZE_VALUE` is held entering `Claiming`.
		assert_eq!(pot_balance_on_hold(), 2 * PRIZE_VALUE);

		// One winner claims — releases one slot.
		let (registrant, _slot) = Winners::<Test>::iter_prefix(id).next().expect("a winner");
		assert_ok!(crate::Pallet::<Test>::claim(id, registrant, 99u64));
		assert_eq!(pot_balance_on_hold(), PRIZE_VALUE);

		// Cancel mid-`Claiming`. The `claimed` count is preserved so `Finalizing` only releases
		// the unclaimed remainder.
		crate::Pallet::<Test>::cancel(id);
		assert!(matches!(
			Events::<Test>::get(id).unwrap().status,
			Status::ClearingRegistrations { effective_winners: 2, claimed: 1, .. },
		));
		// No upfront release — the unclaimed slot is settled at `Finalizing`.
		assert_eq!(pot_balance_on_hold(), PRIZE_VALUE);

		set_now_secs(info.end_time);
		run_to_next_ocw(); // drain Registrations
		run_to_next_ocw(); // drain Winners
		run_to_next_ocw(); // Finalize: release the 1 unclaimed slot
		assert!(Events::<Test>::get(id).is_none());
		assert_eq!(pot_balance_on_hold(), 0);
	});
}

#[test]
fn schedule_rejects_duplicate_id() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(1);
		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, id, default_info(1, Permill::one())));
		assert_err!(
			crate::Pallet::<Test>::schedule(SOURCE, id, default_info(1, Permill::one())),
			crate::Error::<Test>::DuplicateEventId,
		);
	});
}

#[test]
fn schedule_rejects_invalid_times_and_zero_winners() {
	new_test_ext().execute_with(|| {
		fund_source();
		let bad_times = EventInfo {
			prize: default_prize(1, Permill::one()),
			registration_starts: 200,
			draw_time: 100,
			end_time: 300,
		};
		assert_err!(
			crate::Pallet::<Test>::schedule(SOURCE, event_id(1), bad_times),
			crate::Error::<Test>::InvalidEventTimes,
		);

		let zero_winners = default_info(0, Permill::one());
		assert_err!(
			crate::Pallet::<Test>::schedule(SOURCE, event_id(2), zero_winners),
			crate::Error::<Test>::NoWinnersConfigured,
		);
	});
}

#[test]
fn scheduled_event_is_indexed_by_registration_starts() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(7);
		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, id, default_info(2, Permill::one())));
		assert!(ActionSchedule::<Test>::contains_key(BigEndianU64(100), id));
		let ev = Events::<Test>::get(id).unwrap();
		assert!(matches!(ev.status, Status::Scheduled));
	});
}

#[test]
fn start_registration_transitions_and_reschedules() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(11);
		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, id, default_info(2, Permill::one())));
		set_now_secs(150);
		run_to_next_ocw();
		let ev = Events::<Test>::get(id).unwrap();
		assert!(matches!(ev.status, Status::Registering { .. }));
		// Schedule moved from the registration_starts slot to the draw_time slot.
		assert!(!ActionSchedule::<Test>::contains_key(BigEndianU64(100), id));
		assert!(ActionSchedule::<Test>::contains_key(BigEndianU64(200), id));
	});
}

#[test]
fn invalid_proof_rejected() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(8);
		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, id, default_info(1, Permill::one())));
		set_now_secs(150);
		run_to_next_ocw();
		let msg_for_a = RegistrationEntry::<u64>::Alias { alias: alias_from(1) }.encode();
		let proof = mock_proof(&id, alias_from(0xAA), &msg_for_a);
		assert_err!(
			crate::Pallet::<Test>::participate_with_alias(
				id,
				RegistrationEntry::Alias { alias: alias_from(2) },
				proof,
				0,
				0,
			),
			crate::Error::<Test>::InvalidMembershipProof,
		);
	});
}

#[test]
fn event_entropy_set_once_and_dropped_at_finalize() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(0x55);
		let info = default_info(2, Permill::one());
		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, id, info.clone()));
		assert!(EventEntropy::<Test>::get(id).is_none(), "not set before draw opens");

		set_now_secs(info.registration_starts as u64);
		run_to_next_ocw(); // Scheduled → Registering
		participate_alias(id, alias_from(0x11), alias_from(0xA0));
		participate_alias(id, alias_from(0x12), alias_from(0xA1));
		assert!(EventEntropy::<Test>::get(id).is_none(), "still not set during Registering",);

		set_now_secs(info.draw_time as u64);
		run_to_next_ocw(); // close_registration → DrawWinners
		let seed = EventEntropy::<Test>::get(id).expect("entropy set at draw open");

		// Subsequent draw batches must NOT touch `EventEntropy`.
		run_to_next_ocw(); // draw_winners
		run_to_next_ocw(); // close_drawing → Claiming
		assert_eq!(EventEntropy::<Test>::get(id), Some(seed));

		set_now_secs(info.end_time as u64);
		// Run the rest of the clean-up chain: close_claiming, clean_up_registrations,
		// clean_up_winners, finalize.
		for _ in 0..4 {
			run_to_next_ocw();
		}
		assert!(Events::<Test>::get(id).is_none());
		assert!(EventEntropy::<Test>::get(id).is_none(), "finalize must drop the entropy entry",);
	});
}

#[test]
fn duplicate_slot_registration_rejected() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(7);
		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, id, default_info(2, Permill::one())));
		set_now_secs(150);
		run_to_next_ocw();
		participate_alias(id, alias_from(11), alias_from(0xA1));
		let entry = RegistrationEntry::<u64>::Alias { alias: alias_from(22) };
		let msg = entry.encode();
		let proof = mock_proof(&id, alias_from(0xA1), &msg);
		assert_err!(
			crate::Pallet::<Test>::participate_with_alias(id, entry, proof, 0, 0),
			crate::Error::<Test>::EntropySlotTaken,
		);
	});
}

#[test]
fn account_participation_validates_vrf_and_binding() {
	use sp_core::{crypto::VrfSecret, Pair};
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(9);
		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, id, default_info(1, Permill::one())));
		set_now_secs(150);
		run_to_next_ocw();

		let pair = sr25519::Pair::from_seed(b"00000000000000000000000000000abc");
		let public = pair.public();
		let account_id = account_id_for(public);
		register_account_pubkey(account_id, public);
		let sig = pair.vrf_sign(&crate::vrf::transcript_for_event(&id, &public).into_sign_data());

		// An account that hasn't been registered as an sr25519 key: the
		// AccountIdToPublic conversion fails.
		assert_err!(
			crate::Pallet::<Test>::participate_with_account(
				account_id.wrapping_add(1),
				id,
				sig.clone(),
			),
			crate::Error::<Test>::UnsupportedAccountKey,
		);

		// Matching account id (registered) succeeds.
		assert_ok!(crate::Pallet::<Test>::participate_with_account(account_id, id, sig.clone(),));
		assert!(matches!(
			Events::<Test>::get(id).unwrap().status,
			Status::Registering { total_participants: 1 }
		));

		// Same proof replayed → slot already taken.
		assert_err!(
			crate::Pallet::<Test>::participate_with_account(account_id, id, sig),
			crate::Error::<Test>::EntropySlotTaken,
		);
	});
}

#[test]
fn draw_marks_exactly_effective_n_winners() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(11);
		let info = default_info(3, Permill::one());
		drive_to_claiming(id, info.clone(), 3);
		let ev = Events::<Test>::get(id).unwrap();
		assert!(matches!(ev.status, Status::Claiming { effective_winners: 3, .. }));
		assert_eq!(Winners::<Test>::iter_prefix(id).count(), 3);
		// Schedule moved to end_time.
		assert!(ActionSchedule::<Test>::contains_key(BigEndianU64(info.end_time), id));
	});
}

#[test]
fn draw_with_no_participants_cancels_event() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(13);
		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, id, default_info(3, Permill::one())));
		assert_eq!(source_balance(), POT_FUNDING - 3 * PRIZE_VALUE - MIN_BALANCE);
		assert_eq!(pot_balance_on_hold(), 3 * PRIZE_VALUE);

		set_now_secs(150);
		run_to_next_ocw(); // → Registering
		set_now_secs(250);
		run_to_next_ocw(); // close+draw: 0 participants → cancel + finalize inline
					 // Event is removed; the hold is released back to the pot's free
					 // balance (callers manage source refunds separately).
		assert!(Events::<Test>::get(id).is_none());
		assert_eq!(ActionSchedule::<Test>::iter().count(), 0);
		assert_eq!(pot_balance_on_hold(), 0);
		assert_eq!(pot_balance(), 3 * PRIZE_VALUE + MIN_BALANCE);
	});
}

#[test]
fn claim_transfers_to_beneficiary_and_consumes_slot() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(17);
		drive_to_claiming(id, default_info(1, Permill::one()), 1);
		// Winners is now keyed by RegistrationEntry; iter_prefix yields
		// (registrant, slot) pairs.
		let (registrant, _slot) = Winners::<Test>::iter_prefix(id).next().expect("a winner");
		let beneficiary = 99u64;
		assert_ok!(crate::Pallet::<Test>::claim(id, registrant.clone(), beneficiary));
		assert_err!(
			crate::Pallet::<Test>::claim(id, registrant, beneficiary),
			crate::Error::<Test>::NoSuchWinner,
		);
		use frame_support::traits::fungibles::Inspect;
		assert_eq!(<Assets as Inspect<u64>>::balance(ASSET_ID, &beneficiary), PRIZE_VALUE);
	});
}

#[test]
fn claim_rejected_after_end_time() {
	// `claim` must fail once `now >= end_time` even if the event is still in `Claiming`
	// (e.g. the OCW hasn't yet advanced it to `ClearingRegistrations`). Verifies that
	// `end_time` is a hard cut-off independent of the lifecycle's status transitions.
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(53);
		let info = default_info(1, Permill::one());
		drive_to_claiming(id, info.clone(), 1);
		let (registrant, _slot) = Winners::<Test>::iter_prefix(id).next().expect("a winner");
		let beneficiary = 99u64;

		// At `end_time` the window is closed (strict `<` check).
		set_now_secs(info.end_time as u64);
		assert_err!(
			crate::Pallet::<Test>::claim(id, registrant.clone(), beneficiary),
			crate::Error::<Test>::ClaimingWindowClosed,
		);
		// Strictly later than `end_time` is also rejected.
		set_now_secs(info.end_time as u64 + 100);
		assert_err!(
			crate::Pallet::<Test>::claim(id, registrant, beneficiary),
			crate::Error::<Test>::ClaimingWindowClosed,
		);
	});
}

#[test]
fn clean_up_removes_event() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(19);
		let info = default_info(1, Permill::one());
		drive_to_claiming(id, info.clone(), 1);
		set_now_secs(info.end_time as u64);
		run_to_next_ocw(); // Claiming → ClearingRegistrations
		let ev = Events::<Test>::get(id).unwrap();
		// Nobody claimed: effective_winners=1, claimed=0.
		assert!(matches!(
			ev.status,
			Status::ClearingRegistrations { effective_winners: 1, claimed: 0, .. }
		));

		run_to_next_ocw(); // drains Registrations (1 entry) → ClearingWinners
		assert!(matches!(Events::<Test>::get(id).unwrap().status, Status::ClearingWinners { .. }));
		assert_eq!(Registrations::<Test>::iter_prefix(id).count(), 0);

		run_to_next_ocw(); // drains Winners (1 entry) → Finalizing
		assert!(matches!(Events::<Test>::get(id).unwrap().status, Status::Finalizing { .. }));
		assert_eq!(Winners::<Test>::iter_prefix(id).count(), 0);

		run_to_next_ocw(); // finalize: refund unclaimed, remove event
		assert!(Events::<Test>::get(id).is_none());
		assert_eq!(ActionSchedule::<Test>::iter().count(), 0);
		// The unclaimed prize's hold was released back into the pot's free
		// balance. The pot also
		// keeps the ED that source paid in at schedule time.
		assert_eq!(pot_balance_on_hold(), 0);
		assert_eq!(pot_balance(), PRIZE_VALUE + MIN_BALANCE);

		// Second event: 2 winners, one claims, the other doesn't. `finalize` must release only the
		// unclaimed half, so a partial refund.
		let id2 = event_id(20);
		let info2 = EventInfo {
			prize: AirdropPrize {
				asset_id: ASSET_ID,
				asset_amount: PRIZE_VALUE,
				max_winners: 2,
				winner_cap: Permill::one(),
			},
			registration_starts: 1000,
			draw_time: 2000,
			end_time: 3000,
		};
		drive_to_claiming(id2, info2.clone(), 2);

		let (registrant, _slot) = Winners::<Test>::iter_prefix(id2).next().expect("a winner");
		let beneficiary = 77u64;
		assert_ok!(crate::Pallet::<Test>::claim(id2, registrant, beneficiary));

		set_now_secs(info2.end_time as u64);
		run_to_next_ocw(); // Claiming → ClearingRegistrations
		run_to_next_ocw(); // drains Registrations (2 entries) → ClearingWinners
		run_to_next_ocw(); // drains Winners (1 remaining) → Finalizing
		run_to_next_ocw(); // finalize: refund only the unclaimed prize
		assert!(Events::<Test>::get(id2).is_none());
		// One prize went to the beneficiary, the other was refunded to the pot. Pot accumulated:
		// PRIZE_VALUE from event 1 (1 unclaimed) + PRIZE_VALUE from event 2 (1 unclaimed). Source
		// paid 3 × PRIZE_VALUE for the prize allocations plus the single ED transferred once when
		// the asset was enabled; beneficiary received 1 × PRIZE_VALUE; pot keeps the rest.
		assert_eq!(pot_balance_on_hold(), 0);
		assert_eq!(pot_balance(), 2 * PRIZE_VALUE + MIN_BALANCE);
		assert_eq!(source_balance(), POT_FUNDING - 3 * PRIZE_VALUE - MIN_BALANCE);
	});
}

#[test]
fn winner_cap_floor_with_min_one() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(29);
		// 10 max winners, 1% cap on 5 participants → floor(0.05) = 0, but the
		// pallet floors at 1 so at least one winner is drawn.
		drive_to_claiming(id, default_info(10, Permill::from_percent(1)), 5);
		assert!(matches!(
			Events::<Test>::get(id).unwrap().status,
			Status::Claiming { effective_winners: 1, .. }
		));
	});
}

#[test]
fn registrations_use_identity_order() {
	new_test_ext().execute_with(|| {
		fund_source();
		let id = event_id(31);
		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, id, default_info(3, Permill::one())));
		set_now_secs(150);
		run_to_next_ocw();
		// Aliases vary either the last byte (low end of the keyspace), the
		// first byte (top of the keyspace), or both. Under BigEndianU256 the
		// first byte is most significant, the second byte is next, and so on,
		// so the expected numerical order is:
		// 0x..01 < 0x..02 < 0x..03 < 0xAA00.. < 0xBB33.. < 0xFF00..
		let mut a1 = [0u8; 32];
		a1[31] = 0x01;
		let mut a2 = [0u8; 32];
		a2[31] = 0x02;
		let mut a3 = [0u8; 32];
		a3[31] = 0x03;
		let mut a4 = [0u8; 32];
		a4[0] = 0xAA;
		let mut a5 = [0u8; 32];
		a5[0] = 0xBB;
		a5[1] = 0x33;
		let mut a6 = [0u8; 32];
		a6[0] = 0xFF;
		let aliases: [Alias; 6] = [a1, a2, a3, a4, a5, a6];
		for a in &aliases {
			Registrations::<Test>::insert(
				id,
				BigEndianU256::from(*a),
				RegistrationEntry::Alias { alias: *a },
			);
		}
		let collected: Vec<_> = Registrations::<Test>::iter_key_prefix(id).collect();
		assert_eq!(collected, aliases.iter().map(|a| BigEndianU256::from(*a)).collect::<Vec<_>>(),);
	});
}

#[test]
fn action_schedule_orders_by_timestamp() {
	new_test_ext().execute_with(|| {
		fund_source();
		// Three events with different registration_starts; the schedule must
		// iterate in numerical timestamp order regardless of insertion order.
		let early = EventInfo {
			registration_starts: 50,
			draw_time: 100,
			end_time: 200,
			..default_info(1, Permill::one())
		};
		let mid = EventInfo {
			registration_starts: 100,
			draw_time: 200,
			end_time: 300,
			..default_info(1, Permill::one())
		};
		let late = EventInfo {
			registration_starts: 500,
			draw_time: 600,
			end_time: 700,
			..default_info(1, Permill::one())
		};

		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, event_id(2), late));
		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, event_id(1), mid));
		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, event_id(0), early));

		let order: Vec<u64> =
			ActionSchedule::<Test>::iter().map(|(timestamp, _, _)| timestamp.0).collect();
		assert_eq!(order, vec![50, 100, 500]);
	});
}

#[test]
fn draw_winners_runs_in_multiple_batches() {
	new_test_ext().execute_with(|| {
		fund_source();
		// Force a small per-call draw budget so we observe the stepping.
		set_draw_limit(2);

		let id = event_id(41);
		let info = default_info(5, Permill::one());
		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, id, info.clone()));
		set_now_secs(info.registration_starts as u64);
		run_to_next_ocw(); // → Registering
		for i in 1..=5u8 {
			let participant = alias_from(0x10 + i);
			let event_alias = alias_from(0xA0 + i);
			participate_alias(id, participant, event_alias);
		}
		set_now_secs(info.draw_time as u64);
		run_to_next_ocw(); // close_registration → DrawWinners { winners_added: 0, .. }
		assert!(matches!(
			Events::<Test>::get(id).unwrap().status,
			Status::DrawWinners { winners_added: 0, effective_winners: 5, .. }
		));

		// First step: chained `iter_prefix_from(cursor).chain(iter_prefix(event_id))`
		// walks past `entropy_target` and into the prefix, picking up 2 winners.
		run_to_next_ocw();
		assert!(matches!(
			Events::<Test>::get(id).unwrap().status,
			Status::DrawWinners { winners_added: 2, .. }
		));
		assert_eq!(Winners::<Test>::iter_prefix(id).count(), 2);

		// Second step: 2 more (total 4).
		run_to_next_ocw();
		assert!(matches!(
			Events::<Test>::get(id).unwrap().status,
			Status::DrawWinners { winners_added: 4, .. }
		));
		assert_eq!(Winners::<Test>::iter_prefix(id).count(), 4);

		// Third step: 1 more (total 5). Still in DrawWinners because
		// `close_drawing_authorized` is dispatched on the next OCW pass.
		run_to_next_ocw();
		assert!(matches!(
			Events::<Test>::get(id).unwrap().status,
			Status::DrawWinners { winners_added: 5, effective_winners: 5, .. }
		));
		assert_eq!(Winners::<Test>::iter_prefix(id).count(), 5);

		// Fourth step: `close_drawing` (winners_added == effective_winners)
		// → Claiming.
		run_to_next_ocw();
		assert!(matches!(
			Events::<Test>::get(id).unwrap().status,
			Status::Claiming { effective_winners: 5, .. }
		));
		assert_eq!(Winners::<Test>::iter_prefix(id).count(), 5);
	});
}

#[test]
fn end_to_end_lottery_lifecycle() {
	new_test_ext().execute_with(|| {
		fund_source();
		// Force a small per-call draw budget so the test actually walks
		// multiple OCW passes through the draw phase. ClearLimit stays at the
		// mock default (100) so 1000 registrations span 10 clean-up batches.
		set_draw_limit(5);

		// 1000 participants, 30 max winners, 10% winner_cap. winner_cap × 1000
		// = 100 candidate winners; max_winners=30 is the binding cap, so the
		// percentage is never the limiting factor (it isn't "reached").
		const N_PARTICIPANTS: u16 = 1000;
		const MAX_WINNERS: u32 = 30;
		const N_CLAIMED: u64 = 20;

		let id = event_id(99);
		let info = EventInfo {
			prize: AirdropPrize {
				asset_id: ASSET_ID,
				asset_amount: PRIZE_VALUE,
				max_winners: MAX_WINNERS,
				winner_cap: Permill::from_percent(10),
			},
			registration_starts: 100,
			draw_time: 5_000,
			end_time: 10_000,
		};

		assert_ok!(crate::Pallet::<Test>::schedule(SOURCE, id, info.clone()));
		// `schedule` debits `source` only for the prize allocation. The pot's free balance shows
		// the ED transferred once at `enable_asset` time, which keeps the asset account alive
		// while the hold is in place.
		assert_eq!(source_balance(), POT_FUNDING - MAX_WINNERS as u64 * PRIZE_VALUE - MIN_BALANCE,);
		assert_eq!(pot_balance_on_hold(), MAX_WINNERS as u64 * PRIZE_VALUE);
		assert_eq!(pot_balance(), MIN_BALANCE);

		// ---- Open registration ----
		set_now_secs(info.registration_starts as u64);
		run_to_next_ocw();
		assert!(matches!(
			Events::<Test>::get(id).unwrap().status,
			Status::Registering { total_participants: 0 }
		));

		// ---- Register 1000 participants ----
		for i in 0..N_PARTICIPANTS {
			let b = i.to_be_bytes();
			let mut participant = [0u8; 32];
			participant[0] = 0x10;
			participant[1..3].copy_from_slice(&b);
			let mut event_alias = [0u8; 32];
			event_alias[0] = 0xA0;
			event_alias[1..3].copy_from_slice(&b);
			participate_alias(id, participant, event_alias);
		}
		assert!(matches!(
			Events::<Test>::get(id).unwrap().status,
			Status::Registering { total_participants } if total_participants == N_PARTICIPANTS as u32
		));

		// ---- Close registration, draw winners (5 per batch), close drawing ----
		set_now_secs(info.draw_time as u64);
		run_to_next_ocw(); // close_registration → DrawWinners
		assert!(matches!(
			Events::<Test>::get(id).unwrap().status,
			Status::DrawWinners { effective_winners: MAX_WINNERS, winners_added: 0, .. }
		));
		// 30 winners / 5 per batch = 6 draw_winners calls, then 1 close_drawing.
		for _ in 0..(MAX_WINNERS / 5 + 1) {
			run_to_next_ocw();
		}
		assert!(matches!(
			Events::<Test>::get(id).unwrap().status,
			Status::Claiming { effective_winners: MAX_WINNERS, claimed: 0, .. }
		));
		assert_eq!(Winners::<Test>::iter_prefix(id).count(), MAX_WINNERS as usize);

		// ---- 20 of the 30 winners claim ----
		let winners: Vec<_> = Winners::<Test>::iter_prefix(id).collect();
		for (i, (registrant, _slot)) in winners.into_iter().take(N_CLAIMED as usize).enumerate() {
			let beneficiary = 1_000 + i as u64;
			assert_ok!(crate::Pallet::<Test>::claim(id, registrant, beneficiary));
		}
		assert!(matches!(
			Events::<Test>::get(id).unwrap().status,
			Status::Claiming { claimed, .. } if claimed == N_CLAIMED as u32
		));
		assert_eq!(
			Winners::<Test>::iter_prefix(id).count(),
			(MAX_WINNERS - N_CLAIMED as u32) as usize,
		);

		// ---- End time → clean-up → finalize ----
		set_now_secs(info.end_time as u64);
		// close_claiming + 10 clean_up_registrations (1000/ClearLimit=100) +
		// 1 clean_up_winners (10 remaining drain inside one batch) + 1 finalize.
		for _ in 0..(1 + N_PARTICIPANTS as u32 / 100 + 1 + 1) {
			run_to_next_ocw();
		}
		assert!(Events::<Test>::get(id).is_none());
		assert_eq!(ActionSchedule::<Test>::iter().count(), 0);
		assert_eq!(Registrations::<Test>::iter_prefix(id).count(), 0);
		assert_eq!(Winners::<Test>::iter_prefix(id).count(), 0);

		// ---- Pot accounting ----
		// - Source paid 30 × PRIZE_VALUE + MIN_BALANCE (ED) up front.
		// - 20 claims released their hold and transferred to beneficiaries (left the pot).
		// - Finalize released the 10 unclaimed prizes back to the pot's free balance (they stay in
		//   the pot — refunds are not auto-returned to source).
		assert_eq!(pot_balance_on_hold(), 0);
		assert_eq!(pot_balance(), (MAX_WINNERS as u64 - N_CLAIMED) * PRIZE_VALUE + MIN_BALANCE,);
		assert_eq!(source_balance(), POT_FUNDING - MAX_WINNERS as u64 * PRIZE_VALUE - MIN_BALANCE,);
		use frame_support::traits::fungibles::Inspect;
		for i in 0..N_CLAIMED {
			assert_eq!(<Assets as Inspect<u64>>::balance(ASSET_ID, &(1_000 + i)), PRIZE_VALUE,);
		}
	});
}
