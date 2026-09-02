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
	pallet::{AirdropEventInfoOf, DrawSalts, NextDrawIndex},
};
use alloc::vec::Vec;
use frame_support::{
	assert_noop, assert_ok,
	dispatch::Pays,
	traits::{fungibles::Inspect, ConstU32, Hooks},
	BoundedVec,
};
use frame_system::RawOrigin;
use indiv_pallet_airdrop::{
	pallet::{Events as AirdropEvents, Registrations, Winners},
	types::{AirdropPrize, EventId, RegistrationEntry, Status},
};
use sp_runtime::{
	transaction_validity::{InvalidTransaction, TransactionSource, TransactionValidityError},
	DispatchError, Permill, TokenError,
};

const ASSET_ID: u32 = 42;
const PRIZE_VALUE: u64 = 1_000;
const MIN_BALANCE: u64 = 1;
const SOURCE_FUNDING: u64 = 10_000_000;
/// Matches the mock's `PrizeSourceAccount`.
const PRIZE_SOURCE: u64 = 7;

#[test]
fn people_airdrops_context_uses_paseo_allocation() {
	assert_eq!(
		crate::Pallet::<Test>::people_airdrops_context(),
		indiv_support::context::build_product_context(
			indiv_support::context::personhood::PRODUCT_NAME,
			b"paseo",
			indiv_support::context::personhood::PEOPLE_AIRDROPS,
		),
	);
}

fn fund_prize_source(amount: u64) {
	use frame_support::traits::fungibles::{Create, Mutate};
	if !<Assets as Inspect<u64>>::asset_exists(ASSET_ID) {
		<Assets as Create<u64>>::create(ASSET_ID, PRIZE_SOURCE, true, MIN_BALANCE)
			.expect("create asset");
	}
	Assets::mint_into(ASSET_ID, &PRIZE_SOURCE, amount).expect("mint into prize source");
	assert_ok!(indiv_pallet_airdrop::Pallet::<Test>::enable_asset(
		RuntimeOrigin::root(),
		ASSET_ID,
		PRIZE_SOURCE,
	));
}

fn source_balance() -> u64 {
	<Assets as Inspect<u64>>::balance(ASSET_ID, &PRIZE_SOURCE)
}

fn default_info(max_winners: u32) -> AirdropEventInfoOf<Test> {
	AirdropEventInfoOf::<Test> {
		prize: AirdropPrize {
			asset_id: ASSET_ID,
			asset_amount: PRIZE_VALUE,
			max_winners,
			winner_cap: Permill::one(),
		},
		registration_starts: 100,
		draw_time: 200,
		end_time: 300,
	}
}

fn draws(infos: &[AirdropEventInfoOf<Test>]) -> BoundedVec<AirdropEventInfoOf<Test>, ConstU32<8>> {
	infos.to_vec().try_into().expect("bounded by MaxScheduleBatch")
}

fn ids(event_ids: &[EventId]) -> BoundedVec<EventId, ConstU32<8>> {
	event_ids.to_vec().try_into().expect("bounded by MaxRegisterBatch")
}

/// Register `who` for `event_ids`, asserting the successful call is free.
fn register_ok(who: u64, event_ids: &[EventId]) {
	let post = crate::Pallet::<Test>::register(RuntimeOrigin::signed(who), ids(event_ids))
		.expect("register succeeds");
	assert_eq!(post.pays_fee, Pays::No);
}

/// Claim `who`'s prize in `event_id` to `destination`, asserting the successful call is free.
fn claim_ok(who: u64, event_id: EventId, destination: u64) {
	let post = crate::Pallet::<Test>::claim(RuntimeOrigin::signed(who), event_id, destination)
		.expect("claim succeeds");
	assert_eq!(post.pays_fee, Pays::No);
}

/// Schedule `n` one-winner draws and open their registration.
fn schedule_and_open(n: u64) -> Vec<EventId> {
	let infos = (0..n).map(|_| default_info(1)).collect::<Vec<_>>();
	assert_ok!(crate::Pallet::<Test>::schedule_draws(RuntimeOrigin::root(), draws(&infos)));
	set_now_secs(150);
	run_to_next_ocw();
	let event_ids = (0..n).map(crate::Pallet::<Test>::draw_event_id).collect::<Vec<_>>();
	for id in &event_ids {
		assert!(matches!(
			AirdropEvents::<Test>::get(id).expect("draw scheduled").status,
			Status::Registering { .. }
		));
	}
	event_ids
}

#[test]
fn schedule_draws_assigns_ids_salts_and_debits_source() {
	new_test_ext().execute_with(|| {
		fund_prize_source(SOURCE_FUNDING);
		let funded = source_balance();

		assert_ok!(crate::Pallet::<Test>::schedule_draws(
			RuntimeOrigin::root(),
			draws(&[default_info(2), default_info(3)]),
		));

		let id0 = crate::Pallet::<Test>::draw_event_id(0);
		let id1 = crate::Pallet::<Test>::draw_event_id(1);
		assert_eq!(NextDrawIndex::<Test>::get(), 2);
		assert_eq!(DrawSalts::<Test>::get(id0), Some(([42u8; 32], 300)));
		assert_eq!(DrawSalts::<Test>::get(id1), Some(([42u8; 32], 300)));
		assert!(AirdropEvents::<Test>::contains_key(id0));
		assert!(AirdropEvents::<Test>::contains_key(id1));
		// The full prize allocation of both draws left the source.
		assert_eq!(source_balance(), funded - 5 * PRIZE_VALUE);

		// A later batch continues the index sequence.
		assert_ok!(crate::Pallet::<Test>::schedule_draws(
			RuntimeOrigin::root(),
			draws(&[default_info(1)]),
		));
		assert!(AirdropEvents::<Test>::contains_key(crate::Pallet::<Test>::draw_event_id(2)));
	});
}

#[test]
fn schedule_draws_checks_origin_randomness_and_is_all_or_nothing() {
	new_test_ext().execute_with(|| {
		// Fund exactly one one-winner draw so the second draw in the batch must fail.
		fund_prize_source(MIN_BALANCE + PRIZE_VALUE);

		assert_noop!(
			crate::Pallet::<Test>::schedule_draws(
				RuntimeOrigin::signed(1),
				draws(&[default_info(1)]),
			),
			DispatchError::BadOrigin,
		);

		set_randomness(None);
		assert_noop!(
			crate::Pallet::<Test>::schedule_draws(RuntimeOrigin::root(), draws(&[default_info(1)])),
			crate::Error::<Test>::RandomnessUnavailable,
		);
		set_randomness(Some(([42u8; 32], 1)));

		// The first draw is fundable, the second is not: the whole batch reverts.
		assert_noop!(
			crate::Pallet::<Test>::schedule_draws(
				RuntimeOrigin::root(),
				draws(&[default_info(1), default_info(1)]),
			),
			TokenError::FundsUnavailable,
		);
		assert_eq!(NextDrawIndex::<Test>::get(), 0);
		assert!(!AirdropEvents::<Test>::contains_key(crate::Pallet::<Test>::draw_event_id(0)));
	});
}

#[test]
fn register_enters_every_open_draw() {
	new_test_ext().execute_with(|| {
		fund_prize_source(SOURCE_FUNDING);
		let event_ids = schedule_and_open(2);
		let alias = alias_of(1);

		register_ok(1, &event_ids);
		for id in &event_ids {
			let (salt, _end_time) = DrawSalts::<Test>::get(id).expect("scheduled");
			let slot = crate::Pallet::<Test>::slot_for(id, &salt, &alias);
			assert_eq!(
				Registrations::<Test>::get(id, slot),
				Some(RegistrationEntry::Alias { alias }),
			);
		}

		// One entry per person and draw: registering again collides on the same slot and, being an
		// error path, pays.
		assert_noop!(
			crate::Pallet::<Test>::register(RuntimeOrigin::signed(1), ids(&event_ids[..1])),
			indiv_pallet_airdrop::Error::<Test>::EntropySlotTaken,
		);
		let err = crate::Pallet::<Test>::register(RuntimeOrigin::signed(1), ids(&event_ids[..1]))
			.unwrap_err();
		assert_eq!(err.post_info.pays_fee, Pays::Yes);
		// A non-person origin cannot register.
		assert_noop!(
			crate::Pallet::<Test>::register(
				RuntimeOrigin::signed(MAX_PERSON_ACCOUNT),
				ids(&event_ids[..1]),
			),
			DispatchError::BadOrigin,
		);
		// Unknown draws and empty batches are rejected.
		assert_noop!(
			crate::Pallet::<Test>::register(RuntimeOrigin::signed(2), ids(&[[9u8; 32]])),
			crate::Error::<Test>::UnknownDraw,
		);
		assert_noop!(
			crate::Pallet::<Test>::register(RuntimeOrigin::signed(2), ids(&[])),
			crate::Error::<Test>::EmptyRegistration,
		);
	});
}

#[test]
fn register_is_all_or_nothing() {
	new_test_ext().execute_with(|| {
		fund_prize_source(SOURCE_FUNDING);
		let event_ids = schedule_and_open(2);
		let alias = alias_of(1);

		register_ok(1, &event_ids[..1]);
		// The batch fails on the already-registered draw, so the fresh draw is not entered.
		assert_noop!(
			crate::Pallet::<Test>::register(
				RuntimeOrigin::signed(1),
				ids(&[event_ids[1], event_ids[0]]),
			),
			indiv_pallet_airdrop::Error::<Test>::EntropySlotTaken,
		);
		let (salt, _end_time) = DrawSalts::<Test>::get(event_ids[1]).expect("scheduled");
		let slot = crate::Pallet::<Test>::slot_for(&event_ids[1], &salt, &alias);
		assert_eq!(Registrations::<Test>::get(event_ids[1], slot), None);
	});
}

#[test]
fn winner_claims_to_destination() {
	new_test_ext().execute_with(|| {
		fund_prize_source(SOURCE_FUNDING);
		let event_ids = schedule_and_open(1);
		let id = event_ids[0];
		register_ok(1, &event_ids);

		set_now_secs(200);
		run_to_next_ocw(); // Registering → AwaitingEntropy
		run_to_next_ocw(); // capture_entropy → DrawWinners
		run_to_next_ocw(); // draw_winners fills the single slot
		run_to_next_ocw(); // close_drawing → Claiming
		assert!(matches!(
			AirdropEvents::<Test>::get(id).expect("live").status,
			Status::Claiming { .. }
		));
		assert_eq!(
			Winners::<Test>::get(id, RegistrationEntry::Alias { alias: alias_of(1) }),
			Some(crate::Pallet::<Test>::slot_for(
				&id,
				&DrawSalts::<Test>::get(id).expect("scheduled").0,
				&alias_of(1),
			)),
		);

		// A registered non-winner and an unregistered person cannot claim.
		assert_noop!(
			crate::Pallet::<Test>::claim(RuntimeOrigin::signed(2), id, 99),
			indiv_pallet_airdrop::Error::<Test>::NoSuchWinner,
		);

		let destination = 99u64;
		claim_ok(1, id, destination);
		assert_eq!(<Assets as Inspect<u64>>::balance(ASSET_ID, &destination), PRIZE_VALUE);

		// One claim per person and draw, and the repeated call is an error path, so it pays.
		assert_noop!(
			crate::Pallet::<Test>::claim(RuntimeOrigin::signed(1), id, destination),
			indiv_pallet_airdrop::Error::<Test>::NoSuchWinner,
		);
		let err =
			crate::Pallet::<Test>::claim(RuntimeOrigin::signed(1), id, destination).unwrap_err();
		assert_eq!(err.post_info.pays_fee, Pays::Yes);
	});
}

#[test]
fn remove_scheduled_draw_refunds_and_cancel_closes_participation() {
	new_test_ext().execute_with(|| {
		fund_prize_source(SOURCE_FUNDING);
		let funded = source_balance();

		// Remove before opening: the allocation returns to the prize source.
		assert_ok!(crate::Pallet::<Test>::schedule_draws(
			RuntimeOrigin::root(),
			draws(&[default_info(1)]),
		));
		let id0 = crate::Pallet::<Test>::draw_event_id(0);
		assert_eq!(source_balance(), funded - PRIZE_VALUE);
		assert_ok!(crate::Pallet::<Test>::remove_scheduled_draw(RuntimeOrigin::root(), id0));
		assert!(!AirdropEvents::<Test>::contains_key(id0));
		assert_eq!(source_balance(), funded);

		// Unknown draws are rejected, and the manager origin is required.
		assert_noop!(
			crate::Pallet::<Test>::remove_scheduled_draw(RuntimeOrigin::root(), [9u8; 32]),
			crate::Error::<Test>::UnknownDraw,
		);
		assert_noop!(
			crate::Pallet::<Test>::cancel_draw(RuntimeOrigin::signed(1), id0),
			DispatchError::BadOrigin,
		);

		// Cancel an open draw: participation closes immediately.
		assert_ok!(crate::Pallet::<Test>::schedule_draws(
			RuntimeOrigin::root(),
			draws(&[default_info(1)]),
		));
		let id1 = crate::Pallet::<Test>::draw_event_id(1);
		set_now_secs(150);
		run_to_next_ocw();
		register_ok(1, &[id1]);
		assert_ok!(crate::Pallet::<Test>::cancel_draw(RuntimeOrigin::root(), id1));
		assert_noop!(
			crate::Pallet::<Test>::register(RuntimeOrigin::signed(2), ids(&[id1])),
			indiv_pallet_airdrop::Error::<Test>::NotAcceptingRegistrations,
		);
	});
}

#[test]
fn clean_up_draw_salt_requires_authorized_origin_and_known_draw() {
	new_test_ext().execute_with(|| {
		fund_prize_source(SOURCE_FUNDING);
		let id = schedule_and_open(1)[0];

		// The call is OCW-only: signed and root origins are rejected.
		assert_noop!(
			crate::Pallet::<Test>::clean_up_draw_salt(RuntimeOrigin::signed(200), id),
			DispatchError::BadOrigin,
		);
		assert_noop!(
			crate::Pallet::<Test>::clean_up_draw_salt(RuntimeOrigin::root(), id),
			DispatchError::BadOrigin,
		);
		// Unknown draws are rejected.
		assert_noop!(
			crate::Pallet::<Test>::clean_up_draw_salt(RawOrigin::Authorized.into(), [9u8; 32]),
			crate::Error::<Test>::UnknownDraw,
		);

		// The end-time gate lives in `authorize_clean_up_draw_salt`; dispatch removes the entry.
		set_now_secs(301);
		assert_ok!(crate::Pallet::<Test>::clean_up_draw_salt(RawOrigin::Authorized.into(), id));
		assert_eq!(DrawSalts::<Test>::get(id), None);

		// The entry is gone, so a second clean-up is rejected.
		assert_noop!(
			crate::Pallet::<Test>::clean_up_draw_salt(RawOrigin::Authorized.into(), id),
			crate::Error::<Test>::UnknownDraw,
		);
	});
}

#[test]
fn clean_up_draw_salt_authorize_gates_source_known_draws_and_end_time() {
	new_test_ext().execute_with(|| {
		fund_prize_source(SOURCE_FUNDING);
		let id = schedule_and_open(1)[0];
		set_now_secs(301);

		// External sources are rejected: the transaction is local-only.
		assert!(matches!(
			crate::Pallet::<Test>::authorize_clean_up_draw_salt(TransactionSource::External, &id),
			Err(TransactionValidityError::Invalid(InvalidTransaction::Custom(code)))
				if code == crate::pallet::AuthorizeInvalidity::TransactionNotLocal as u8
		));
		// Unknown draws are rejected.
		assert!(matches!(
			crate::Pallet::<Test>::authorize_clean_up_draw_salt(
				TransactionSource::Local,
				&[9u8; 32],
			),
			Err(TransactionValidityError::Invalid(InvalidTransaction::Custom(code)))
				if code == crate::pallet::AuthorizeInvalidity::UnknownDraw as u8
		));
		// A still-live entry is future-valid, so the pool retains the transaction.
		set_now_secs(300);
		assert!(matches!(
			crate::Pallet::<Test>::authorize_clean_up_draw_salt(TransactionSource::Local, &id),
			Err(TransactionValidityError::Invalid(InvalidTransaction::Future))
		));
		// Strictly after the end time local and in-block sources are valid.
		set_now_secs(301);
		for source in [TransactionSource::Local, TransactionSource::InBlock] {
			let (validity, _refund) =
				crate::Pallet::<Test>::authorize_clean_up_draw_salt(source, &id)
					.expect("authorize succeeds");
			assert!(!validity.propagate);
		}
	});
}

#[test]
fn offchain_worker_cleans_up_stale_salts() {
	new_test_ext().execute_with(|| {
		fund_prize_source(SOURCE_FUNDING);
		let event_ids = schedule_and_open(2);

		// While the draws are live the offchain worker submits nothing.
		crate::Pallet::<Test>::offchain_worker(System::block_number());
		assert_eq!(pending_ocw_tx_count(), 0);

		// Past the end time it submits one clean-up transaction per stale salt.
		set_now_secs(301);
		crate::Pallet::<Test>::offchain_worker(System::block_number());
		assert_eq!(pending_ocw_tx_count(), 2);
		drain_ocw_transactions();
		for id in &event_ids {
			assert_eq!(DrawSalts::<Test>::get(id), None);
		}

		// With the entries gone the offchain worker has nothing left to submit.
		crate::Pallet::<Test>::offchain_worker(System::block_number());
		assert_eq!(pending_ocw_tx_count(), 0);
	});
}

#[test]
fn slot_depends_on_event_salt_and_alias() {
	let event_a = [1u8; 32];
	let event_b = [2u8; 32];
	let salt_a = [3u8; 32];
	let salt_b = [4u8; 32];
	let alias_a = [5u8; 32];
	let alias_b = [6u8; 32];

	let slot = crate::Pallet::<Test>::slot_for(&event_a, &salt_a, &alias_a);
	assert_ne!(slot, crate::Pallet::<Test>::slot_for(&event_b, &salt_a, &alias_a));
	assert_ne!(slot, crate::Pallet::<Test>::slot_for(&event_a, &salt_b, &alias_a));
	assert_ne!(slot, crate::Pallet::<Test>::slot_for(&event_a, &salt_a, &alias_b));
}
