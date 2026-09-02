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
	extension::{CustomValidity, PgasCollection},
	mock::*,
	pallet::{Day, PGAS_DAY_GRACE_WINDOW},
	ClaimedGasAliases, Event, Pallet,
};
use frame_support::{
	assert_noop, assert_ok,
	dispatch::GetDispatchInfo,
	traits::{fungibles::Inspect, OffchainWorker},
};
use frame_system::RawOrigin as SystemOrigin;
use sp_runtime::{
	traits::{DispatchTransaction, Dispatchable},
	transaction_validity::{InvalidTransaction, TransactionSource, TransactionValidityError},
	AccountId32,
};

const DAY: u64 = 86400;

/// Permissionless asset creation as used by all claim tests.
fn setup_pgas_asset() {
	assert_ok!(Pallet::<Test>::create_pgas_asset(SystemOrigin::Authorized.into()));
}

/// Run a claim through the full `AsPgas` validate → dispatch pipeline.
fn submit_claim(
	member_id: u64,
	ring_index: u32,
	collection: PgasCollection,
	slot_index: u32,
	target: AccountId32,
	day: u32,
) -> sp_runtime::DispatchResult {
	let (call, tx_ext) = build_claim_tx(member_id, ring_index, collection, slot_index, target, day);
	let info = call.get_dispatch_info();
	let (_, _val, origin) = tx_ext
		.validate_only(SystemOrigin::None.into(), &call, &info, 0, TransactionSource::External, 0)
		.map_err(|_| sp_runtime::DispatchError::Other("validate_only failed"))?;
	// Dispatch using the mutated origin produced by the extension.
	call.dispatch(origin).map(|_| ()).map_err(|e| e.error)
}

// ==================== Asset creation ====================

#[test]
fn create_pgas_asset_success() {
	new_test_ext().execute_with(|| {
		assert!(!<Assets as Inspect<AccountId32>>::asset_exists(PgasAssetId::get()));
		setup_pgas_asset();
		assert!(<Assets as Inspect<AccountId32>>::asset_exists(PgasAssetId::get()));
		System::assert_has_event(Event::<Test>::PgasAssetCreated.into());
	});
}

#[test]
fn create_pgas_asset_fails_if_already_exists() {
	new_test_ext().execute_with(|| {
		setup_pgas_asset();
		assert_noop!(
			Pallet::<Test>::create_pgas_asset(SystemOrigin::Authorized.into()),
			pallet_assets::Error::<Test>::InUse
		);
	});
}

#[test]
fn claim_pgas_rejected_before_asset_created() {
	new_test_ext().execute_with(|| {
		set_time_sec(DAY);
		// Deliberately do NOT create the asset.

		let target = id_to_account(42);
		let (call, tx_ext) = build_claim_tx(100, 0, PgasCollection::People, 0, target, 1);
		let info = call.get_dispatch_info();
		let result = tx_ext.validate_only(
			SystemOrigin::None.into(),
			&call,
			&info,
			0,
			TransactionSource::External,
			0,
		);
		assert!(matches!(
			result,
			Err(TransactionValidityError::Invalid(InvalidTransaction::Custom(code)))
				if code == CustomValidity::PgasAssetNotCreated as u8
		));
	});
}

// ==================== Successful claims ====================

#[test]
fn claim_pgas_people_success() {
	new_test_ext().execute_with(|| {
		set_time_sec(DAY);
		setup_pgas_asset();

		let target = id_to_account(42);
		assert_ok!(submit_claim(100, 0, PgasCollection::People, 0, target.clone(), 1));

		assert_eq!(
			<Assets as Inspect<AccountId32>>::balance(PgasAssetId::get(), &target),
			PgasClaimAmount::get()
		);
	});
}

#[test]
fn claim_pgas_lite_success_with_lower_slot_bound() {
	new_test_ext().execute_with(|| {
		set_time_sec(DAY);
		setup_pgas_asset();

		// MaxClaimsPerPeriodPerLitePerson = 2, so slot_index 0 and 1 are valid.
		for slot in 0..MaxClaimsPerPeriodPerLitePerson::get() {
			let target = id_to_account(700 + slot as u64);
			assert_ok!(submit_claim(
				500 + slot as u64,
				0,
				PgasCollection::LitePeople,
				slot,
				target,
				1
			));
		}
	});
}

// ==================== Slot bounds ====================

#[test]
fn claim_pgas_people_invalid_slot_rejected_in_validate() {
	new_test_ext().execute_with(|| {
		set_time_sec(DAY);
		setup_pgas_asset();

		let target = id_to_account(42);
		let slot = MaxClaimsPerPeriodPerPerson::get();
		let (call, tx_ext) = build_claim_tx(100, 0, PgasCollection::People, slot, target, 1);
		let info = call.get_dispatch_info();
		let result = tx_ext.validate_only(
			SystemOrigin::None.into(),
			&call,
			&info,
			0,
			TransactionSource::External,
			0,
		);
		assert!(matches!(
			result,
			Err(TransactionValidityError::Invalid(InvalidTransaction::Custom(code)))
				if code == CustomValidity::InvalidClaimSlot as u8
		));
	});
}

#[test]
fn claim_pgas_lite_slot_above_lite_bound_but_below_people_bound_rejected() {
	// Demonstrates the per-collection slot bound: people can go up to 4 slots but lite only 2.
	new_test_ext().execute_with(|| {
		set_time_sec(DAY);
		setup_pgas_asset();

		let people_ok_but_lite_too_big = MaxClaimsPerPeriodPerLitePerson::get();
		assert!(people_ok_but_lite_too_big < MaxClaimsPerPeriodPerPerson::get());

		let target = id_to_account(42);
		let (call, tx_ext) = build_claim_tx(
			100,
			0,
			PgasCollection::LitePeople,
			people_ok_but_lite_too_big,
			target,
			1,
		);
		let info = call.get_dispatch_info();
		let result = tx_ext.validate_only(
			SystemOrigin::None.into(),
			&call,
			&info,
			0,
			TransactionSource::External,
			0,
		);
		assert!(matches!(
			result,
			Err(TransactionValidityError::Invalid(InvalidTransaction::Custom(code)))
				if code == CustomValidity::InvalidClaimSlot as u8
		));
	});
}

// ==================== Uniqueness ====================

#[test]
fn claim_pgas_already_claimed_same_alias_rejected_in_validate() {
	new_test_ext().execute_with(|| {
		set_time_sec(DAY);
		setup_pgas_asset();

		let target = id_to_account(42);
		assert_ok!(submit_claim(100, 0, PgasCollection::People, 0, target.clone(), 1));

		// Second attempt with the same member + slot + day → same alias.
		let (call, tx_ext) = build_claim_tx(100, 0, PgasCollection::People, 0, target, 1);
		let info = call.get_dispatch_info();
		let result = tx_ext.validate_only(
			SystemOrigin::None.into(),
			&call,
			&info,
			0,
			TransactionSource::External,
			0,
		);
		assert!(matches!(
			result,
			Err(TransactionValidityError::Invalid(InvalidTransaction::Custom(code)))
				if code == CustomValidity::AlreadyClaimed as u8
		));
	});
}

// ==================== Bad proof ====================

#[test]
fn claim_pgas_bad_proof_rejected() {
	new_test_ext().execute_with(|| {
		set_time_sec(DAY);
		setup_pgas_asset();

		// Build a proof for slot 0 but attempt to claim slot 1 — the proof's context (embedded
		// in the proof) will not match the context the extension derives from the call, so
		// `TestVerifiable::validate` returns an error.
		let target = id_to_account(42);
		let (good_call, bad_tx_ext) =
			build_claim_tx(100, 0, PgasCollection::People, 0, target.clone(), 1);
		let (bad_call, _) = build_claim_tx(100, 0, PgasCollection::People, 1, target, 1);
		let _ = good_call; // unused, only the extension's proof is reused with a different call.
		let info = bad_call.get_dispatch_info();
		let result = bad_tx_ext.validate_only(
			SystemOrigin::None.into(),
			&bad_call,
			&info,
			0,
			TransactionSource::External,
			0,
		);
		assert!(matches!(
			result,
			Err(TransactionValidityError::Invalid(InvalidTransaction::BadProof))
		));
	});
}

// ==================== Grace window ====================

#[test]
fn claim_pgas_grace_day_accepted_near_rollover() {
	new_test_ext().execute_with(|| {
		// Place time just after the day-2 boundary, within the grace window back to day 1.
		let t = DAY * 2 + 60; // 1 minute past day-2 midnight
		set_time_sec(t);
		setup_pgas_asset();
		assert_eq!(Pallet::<Test>::current_day(), 2);
		assert_eq!(Pallet::<Test>::grace_day(), 1);

		// Proof constructed for day 1 (the grace day) still verifies.
		let target = id_to_account(42);
		assert_ok!(submit_claim(100, 0, PgasCollection::People, 0, target.clone(), 1));

		// The record is stored under the proof's day (1), not the current day.
		assert_eq!(ClaimedGasAliases::<Test>::iter_prefix(Day::from(1u32)).count(), 1);
		assert_eq!(ClaimedGasAliases::<Test>::iter_prefix(Day::from(2u32)).count(), 0);
	});
}

#[test]
fn claim_pgas_outside_grace_rejected() {
	new_test_ext().execute_with(|| {
		// Well past the grace window (current_day = 3, grace_day = 3).
		set_time_sec(DAY * 3 + PGAS_DAY_GRACE_WINDOW + 1);
		setup_pgas_asset();
		assert_eq!(Pallet::<Test>::current_day(), 3);
		assert_eq!(Pallet::<Test>::grace_day(), 3);

		// Proof built for day 1 is no longer in the accepted window; the extension rejects
		// the day before running proof verification.
		let target = id_to_account(42);
		let (call, tx_ext) = build_claim_tx(100, 0, PgasCollection::People, 0, target, 1);
		let info = call.get_dispatch_info();
		let result = tx_ext.validate_only(
			SystemOrigin::None.into(),
			&call,
			&info,
			0,
			TransactionSource::External,
			0,
		);
		assert!(matches!(
			result,
			Err(TransactionValidityError::Invalid(InvalidTransaction::Custom(code)))
				if code == CustomValidity::InvalidClaimDay as u8
		));
	});
}

#[test]
fn claim_pgas_future_day_rejected() {
	new_test_ext().execute_with(|| {
		// Currently on day 1 with the grace window still referring to day 1.
		set_time_sec(DAY + PGAS_DAY_GRACE_WINDOW + 1);
		setup_pgas_asset();
		assert_eq!(Pallet::<Test>::current_day(), 1);
		assert_eq!(Pallet::<Test>::grace_day(), 1);

		// Proof for day 2 (future) is not accepted.
		let target = id_to_account(42);
		let (call, tx_ext) = build_claim_tx(100, 0, PgasCollection::People, 0, target, 2);
		let info = call.get_dispatch_info();
		let result = tx_ext.validate_only(
			SystemOrigin::None.into(),
			&call,
			&info,
			0,
			TransactionSource::External,
			0,
		);
		assert!(matches!(
			result,
			Err(TransactionValidityError::Invalid(InvalidTransaction::Custom(code)))
				if code == CustomValidity::InvalidClaimDay as u8
		));
	});
}

// ==================== OCW cleanup ====================

#[test]
fn offchain_worker_submits_cleanup_for_stale_day() {
	new_test_ext().execute_with(|| {
		// Populate a claim on day 1.
		set_time_sec(DAY);
		setup_pgas_asset();
		let target = id_to_account(42);
		assert_ok!(submit_claim(100, 0, PgasCollection::People, 0, target, 1));
		assert_eq!(ClaimedGasAliases::<Test>::iter_prefix(Day::from(1u32)).count(), 1);

		// First moment day 1 is fully outside the grace window: `grace_day` flips to 2 once
		// `now >= DAY * 2 + PGAS_DAY_GRACE_WINDOW`.
		set_time_sec(DAY * 2 + PGAS_DAY_GRACE_WINDOW + 1);
		Pgas::offchain_worker(System::block_number());

		assert_eq!(pending_ocw_tx_count(), 1);
		drain_ocw_transactions();

		assert_eq!(ClaimedGasAliases::<Test>::iter_prefix(Day::from(1u32)).count(), 0);
	});
}

#[test]
fn offchain_worker_picks_oldest_day_first_with_big_endian_ordering() {
	// `Day = BigEndianU32` is the whole reason `Identity`-hashed iteration over
	// `ClaimedGasAliases` yields days in ascending chronological order. To prove the encoding
	// matters, we seed three days whose default little-endian byte order would *not* match
	// numeric order — specifically picking 1, 2, and 256:
	//
	//   little-endian (broken):  256 < 1 < 2   (`[00,01,00,00]` < `[01,00,00,00]` <
	// `[02,00,00,00]`)   big-endian   (correct):   1 < 2 < 256  (`[00,00,00,01]` < `[00,00,00,02]`
	// < `[00,00,01,00]`)
	//
	// Then we run the OCW three times (committing the deletions between rounds so the limit
	// math doesn't go through the overlay short-circuit) and assert it processes day 1, then
	// day 2, then day 256.
	let mut ext = new_test_ext();
	ext.execute_with(|| {
		set_time_sec(DAY);
		setup_pgas_asset();

		// Insert one record under each day. The day of insertion determines the prefix; the
		// alias bytes don't matter for ordering. We deliberately insert 256 *first* and 1 last
		// to defeat any insertion-order luck in iteration.
		for day_index in [256u32, 1u32, 2u32] {
			let mut alias = [0u8; 32];
			alias[28..32].copy_from_slice(&day_index.to_le_bytes());
			ClaimedGasAliases::<Test>::insert(Day::from(day_index), alias, ());
		}
	});
	ext.commit_all().expect("commit overlay to backend");

	// Move past the grace window for day 256 so all three days are eligible. `grace_day`
	// must be > 256, i.e. `now >= DAY * 257 + PGAS_DAY_GRACE_WINDOW`.
	let cleanup_time = DAY * 257 + PGAS_DAY_GRACE_WINDOW + 1;

	for expected_day in [1u32, 2u32, 256u32] {
		ext.execute_with(|| {
			set_time_sec(cleanup_time);
			Pgas::offchain_worker(System::block_number());
			assert_eq!(pending_ocw_tx_count(), 1);
			drain_ocw_transactions();

			// The day we just cleaned should now be empty; later days (if any) untouched.
			assert_eq!(
				ClaimedGasAliases::<Test>::iter_prefix(Day::from(expected_day)).count(),
				0,
				"expected day {expected_day} to be cleaned this round",
			);
		});
		ext.commit_all().expect("commit deletions to backend");
	}

	// All three days drained; OCW has nothing left to do.
	ext.execute_with(|| {
		set_time_sec(cleanup_time);
		Pgas::offchain_worker(System::block_number());
		assert_eq!(pending_ocw_tx_count(), 0);
	});
}

#[test]
fn offchain_worker_partial_cleanup_leaves_remainder_for_next_round() {
	// Exercise the OCW resubmission cycle: a single cleanup call clears at most
	// `MaxPgasClaimRecordCleanupPerCall` backend entries, leaving the rest for the next round.
	//
	// Substrate's `clear_prefix` limit only counts *backend* deletions — overlay-resident keys
	// are cleared without counting (sp_io docs). Under the usual `execute_with` everything stays
	// in the overlay, so we use `commit_all()` between phases to push records into the backend
	// and let the limit actually bite.
	let mut ext = new_test_ext();
	let day_index = 1u32;
	let day = Day::from(day_index);
	let limit = MaxPgasClaimRecordCleanupPerCall::get();
	let total = limit + 2;

	// Phase 1: insert `limit + 2` records, then commit them to the backend.
	ext.execute_with(|| {
		set_time_sec(DAY);
		setup_pgas_asset();
		for i in 0..total {
			let mut alias = [0u8; 32];
			alias[0..4].copy_from_slice(&i.to_le_bytes());
			ClaimedGasAliases::<Test>::insert(day, alias, ());
		}
		assert_eq!(ClaimedGasAliases::<Test>::iter_prefix(day).count(), total as usize);
	});
	ext.commit_all().expect("commit overlay to backend");

	// Phase 2: advance past grace, OCW submits the cleanup tx, drain applies it, and we expect
	// `limit` removals leaving `total - limit` records for the next round. First moment day 1
	// leaves the grace window is `DAY * 2 + PGAS_DAY_GRACE_WINDOW`.
	ext.execute_with(|| {
		set_time_sec(DAY * 2 + PGAS_DAY_GRACE_WINDOW + 1);
		Pgas::offchain_worker(System::block_number());
		assert_eq!(pending_ocw_tx_count(), 1, "OCW should submit a cleanup tx");
		drain_ocw_transactions();
		assert_eq!(
			ClaimedGasAliases::<Test>::iter_prefix(day).count() as u32,
			total - limit,
			"first round should leave the surplus",
		);
	});
	ext.commit_all().expect("commit deletions to backend");

	// Phase 3: the next OCW pass clears the rest (only `total - limit < limit` records remain).
	ext.execute_with(|| {
		Pgas::offchain_worker(System::block_number());
		assert_eq!(pending_ocw_tx_count(), 1, "OCW should submit a follow-up cleanup tx");
		drain_ocw_transactions();
		assert_eq!(ClaimedGasAliases::<Test>::iter_prefix(day).count(), 0);
	});
	ext.commit_all().expect("commit final deletions to backend");

	// Phase 4: with the prefix empty, the OCW has nothing to do and stops submitting.
	ext.execute_with(|| {
		Pgas::offchain_worker(System::block_number());
		assert_eq!(pending_ocw_tx_count(), 0, "OCW should not submit once the prefix is empty",);
	});
}

#[test]
fn offchain_worker_skips_days_within_grace() {
	new_test_ext().execute_with(|| {
		// Claim on day 1.
		set_time_sec(DAY);
		setup_pgas_asset();
		let target = id_to_account(42);
		assert_ok!(submit_claim(100, 0, PgasCollection::People, 0, target, 1));

		// Still within grace for day 1 (set time just after day-2 midnight).
		set_time_sec(DAY * 2 + 60);
		Pgas::offchain_worker(System::block_number());

		assert_eq!(pending_ocw_tx_count(), 0);
		assert_eq!(ClaimedGasAliases::<Test>::iter_prefix(Day::from(1u32)).count(), 1);
	});
}

// ==================== Scenarios ported from pgas-attempt-5 ====================

#[test]
fn build_gas_context_is_deterministic_and_injective() {
	// No storage needed; just verifies the context builder.
	let ctx1 = Pallet::<Test>::build_gas_context(1, 0);
	let ctx2 = Pallet::<Test>::build_gas_context(1, 0);
	assert_eq!(ctx1, ctx2, "same (day, slot) must yield the same context");

	let ctx_diff_day = Pallet::<Test>::build_gas_context(2, 0);
	assert_ne!(ctx1, ctx_diff_day, "different days must yield different contexts");

	let ctx_diff_slot = Pallet::<Test>::build_gas_context(1, 1);
	assert_ne!(ctx1, ctx_diff_slot, "different slots must yield different contexts");
}

#[test]
fn claim_pgas_same_person_different_days() {
	new_test_ext().execute_with(|| {
		setup_pgas_asset();

		let target = id_to_account(42);
		let member_id = 100u64;
		let slot_index = 0u32;

		// Day 1 claim.
		set_time_sec(DAY);
		assert_ok!(submit_claim(
			member_id,
			0,
			PgasCollection::People,
			slot_index,
			target.clone(),
			1
		));

		// Day 2 claim by the same member at the same slot — a new day means a new context and
		// therefore a new alias, so the claim is fresh rather than a replay.
		set_time_sec(DAY * 2);
		assert_ok!(submit_claim(
			member_id,
			0,
			PgasCollection::People,
			slot_index,
			target.clone(),
			2
		));

		assert_eq!(
			<Assets as Inspect<AccountId32>>::balance(PgasAssetId::get(), &target),
			PgasClaimAmount::get() * 2,
		);
		assert_eq!(ClaimedGasAliases::<Test>::iter_prefix(Day::from(1u32)).count(), 1);
		assert_eq!(ClaimedGasAliases::<Test>::iter_prefix(Day::from(2u32)).count(), 1);
	});
}

#[test]
fn claim_pgas_current_day_proof_still_works_during_grace_window() {
	new_test_ext().execute_with(|| {
		// Just past the day-2 midnight, so we're still inside the grace window for day 1 — both
		// the current day (2) and the grace day (1) proofs are accepted. This test covers the
		// "current day" branch explicitly.
		set_time_sec(DAY * 2 + 60);
		setup_pgas_asset();
		assert_eq!(Pallet::<Test>::current_day(), 2);
		assert_eq!(Pallet::<Test>::grace_day(), 1);

		let target = id_to_account(42);
		assert_ok!(submit_claim(100, 0, PgasCollection::People, 0, target, 2));

		// Record is stored against the proof's day (2, the current day), not the grace day.
		assert_eq!(ClaimedGasAliases::<Test>::iter_prefix(Day::from(2u32)).count(), 1);
		assert_eq!(ClaimedGasAliases::<Test>::iter_prefix(Day::from(1u32)).count(), 0);
	});
}

#[test]
fn claim_pgas_same_alias_different_target_second_attempt_rejected() {
	new_test_ext().execute_with(|| {
		set_time_sec(DAY);
		setup_pgas_asset();

		// First claim lands for target A.
		let target_a = id_to_account(42);
		assert_ok!(submit_claim(100, 0, PgasCollection::People, 0, target_a, 1));

		// Same (member, day, slot) ⇒ same alias regardless of target, so a fresh proof bound to
		// a different target still hits the already-claimed record.
		let target_b = id_to_account(99);
		let (call, tx_ext) = build_claim_tx(100, 0, PgasCollection::People, 0, target_b, 1);
		let info = call.get_dispatch_info();
		let result = tx_ext.validate_only(
			SystemOrigin::None.into(),
			&call,
			&info,
			0,
			TransactionSource::External,
			0,
		);
		assert!(matches!(
			result,
			Err(TransactionValidityError::Invalid(InvalidTransaction::Custom(code)))
				if code == CustomValidity::AlreadyClaimed as u8
		));
	});
}

#[test]
fn claim_pgas_direct_dispatch_requires_claim_alias_origin() {
	new_test_ext().execute_with(|| {
		set_time_sec(DAY);
		setup_pgas_asset();
		let target = id_to_account(42);

		// Calling `claim_pgas` directly (i.e. skipping the `AsPgas` extension that produces the
		// `ClaimAlias` origin) is rejected regardless of what the outer origin is.
		assert_noop!(
			Pallet::<Test>::claim_pgas(
				SystemOrigin::Signed(id_to_account(1)).into(),
				0,
				target.clone(),
			),
			sp_runtime::DispatchError::BadOrigin,
		);
		assert_noop!(
			Pallet::<Test>::claim_pgas(SystemOrigin::Root.into(), 0, target.clone()),
			sp_runtime::DispatchError::BadOrigin,
		);
		assert_noop!(
			Pallet::<Test>::claim_pgas(SystemOrigin::None.into(), 0, target),
			sp_runtime::DispatchError::BadOrigin,
		);
	});
}

#[test]
fn person_can_claim_all_slots_in_a_day() {
	new_test_ext().execute_with(|| {
		setup_pgas_asset();
		set_time_sec(DAY);

		let target = id_to_account(42);
		let slots = MaxClaimsPerPeriodPerPerson::get();
		assert!(slots > 0, "test requires a non-zero slot count");

		// Our mock derives alias from (context, member), and the context varies by slot_index,
		// so the same member produces a distinct alias per slot — every slot is claimable in one
		// day.
		for slot in 0..slots {
			assert_ok!(submit_claim(100, 0, PgasCollection::People, slot, target.clone(), 1));
		}

		assert_eq!(
			<Assets as Inspect<AccountId32>>::balance(PgasAssetId::get(), &target),
			PgasClaimAmount::get() * slots as u64,
		);
	});
}

#[test]
fn multi_person_multi_day_claim_and_issuance() {
	new_test_ext().execute_with(|| {
		setup_pgas_asset();

		let target_a = id_to_account(42);
		let target_b = id_to_account(43);

		// Day 1: two distinct members each claim slot 0.
		set_time_sec(DAY);
		assert_ok!(submit_claim(100, 0, PgasCollection::People, 0, target_a.clone(), 1));
		assert_ok!(submit_claim(200, 0, PgasCollection::People, 0, target_b.clone(), 1));

		// Day 2: member 100 claims again.
		set_time_sec(DAY * 2);
		assert_ok!(submit_claim(100, 0, PgasCollection::People, 0, target_a.clone(), 2));

		assert_eq!(
			<Assets as Inspect<AccountId32>>::balance(PgasAssetId::get(), &target_a),
			PgasClaimAmount::get() * 2,
		);
		assert_eq!(
			<Assets as Inspect<AccountId32>>::balance(PgasAssetId::get(), &target_b),
			PgasClaimAmount::get(),
		);
		assert_eq!(
			<Assets as Inspect<AccountId32>>::total_issuance(PgasAssetId::get()),
			PgasClaimAmount::get() * 3,
		);
	});
}

#[test]
fn clean_pgas_claim_records_removes_all_records_for_the_day() {
	new_test_ext().execute_with(|| {
		set_time_sec(DAY);
		setup_pgas_asset();

		// Insert `limit` records directly (as many as the pallet will remove in one call). We
		// bypass the claim flow so the test doesn't hinge on the claim pipeline — only on the
		// authorize + `clear_prefix` path.
		let day_index = 1u32;
		let day_be = Day::from(day_index);
		let count = MaxPgasClaimRecordCleanupPerCall::get();
		for i in 0..count {
			let mut alias = [0u8; 32];
			alias[0..4].copy_from_slice(&i.to_le_bytes());
			ClaimedGasAliases::<Test>::insert(day_be, alias, ());
		}
		assert_eq!(ClaimedGasAliases::<Test>::iter_prefix(day_be).count(), count as usize);

		// Move past the grace window so the authorize closure accepts the day. First moment
		// day 1 is out of grace: `now >= DAY * 2 + PGAS_DAY_GRACE_WINDOW`.
		set_time_sec(DAY * 2 + PGAS_DAY_GRACE_WINDOW + 1);

		let first_alias = ClaimedGasAliases::<Test>::iter_key_prefix(day_be)
			.next()
			.expect("records were just inserted");
		assert_ok!(Pallet::<Test>::clean_pgas_claim_records(
			SystemOrigin::Authorized.into(),
			day_index,
			first_alias,
		));
		assert_eq!(ClaimedGasAliases::<Test>::iter_prefix(day_be).count(), 0);
		assert!(System::events().iter().any(|r| matches!(
			&r.event,
			RuntimeEvent::Pgas(Event::PgasClaimRecordsCleaned { day_index: d, .. }) if *d == day_index,
		)));
	});
}

#[test]
fn create_pgas_asset_authorize_rejects_when_asset_exists() {
	new_test_ext().execute_with(|| {
		setup_pgas_asset();

		// The authorize closure is the pool-level gate; it returns `Stale` so a second creation
		// attempt never reaches dispatch. `create_pgas_asset_fails_if_already_exists` above
		// covers the dispatch-side `InUse` path.
		let result = Pallet::<Test>::authorize_create_pgas_asset();
		assert!(matches!(
			result,
			Err(TransactionValidityError::Invalid(InvalidTransaction::Stale))
		));
	});
}

#[test]
fn integrity_test_passes() {
	new_test_ext().execute_with(|| {
		<Pallet<Test> as frame_support::traits::Hooks<u64>>::integrity_test();
	});
}
