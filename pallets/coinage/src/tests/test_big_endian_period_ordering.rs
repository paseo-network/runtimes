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

//! Tests that `PaidTokenCollectionsCreated` iterates in numeric period order.
//!
//! With little-endian encoding (the old approach), period 256 (`00 01 00 00`)
//! sorts before period 1 (`01 00 00 00`), so `.iter().next()` would return
//! the wrong period. `BigEndianPeriod` fixes this.

use crate::{mock::*, pallet::BigEndianPeriod, *};

#[test]
fn iter_returns_oldest_period_across_byte_boundaries() {
	new_test_ext().execute_with(|| {
		// Insert periods that expose the LE vs BE ordering difference.
		// LE bytes: 1 = [01,00,00,00], 256 = [00,01,00,00], 512 = [00,02,00,00]
		// LE order:  256, 512, 1  (wrong)
		// BE order:  1, 256, 512  (correct)
		PaidTokenCollectionsCreated::<Test>::insert(BigEndianPeriod::from(256), ());
		PaidTokenCollectionsCreated::<Test>::insert(BigEndianPeriod::from(1), ());
		PaidTokenCollectionsCreated::<Test>::insert(BigEndianPeriod::from(512), ());

		let (first, _) = PaidTokenCollectionsCreated::<Test>::iter().next().unwrap();
		assert_eq!(first.0, 1, "iter().next() must return the numerically smallest period");

		// Verify full ordering.
		let periods = PaidTokenCollectionsCreated::<Test>::iter()
			.map(|(p, _)| p.0)
			.collect::<Vec<_>>();
		assert_eq!(periods, vec![1, 256, 512]);
	});
}

#[test]
fn iter_returns_oldest_period_within_same_high_byte() {
	new_test_ext().execute_with(|| {
		PaidTokenCollectionsCreated::<Test>::insert(BigEndianPeriod::from(258), ());
		PaidTokenCollectionsCreated::<Test>::insert(BigEndianPeriod::from(2), ());
		PaidTokenCollectionsCreated::<Test>::insert(BigEndianPeriod::from(65536), ());

		let (first, _) = PaidTokenCollectionsCreated::<Test>::iter().next().unwrap();
		assert_eq!(first.0, 2);

		let periods = PaidTokenCollectionsCreated::<Test>::iter()
			.map(|(p, _)| p.0)
			.collect::<Vec<_>>();
		assert_eq!(periods, vec![2, 258, 65536]);
	});
}

#[test]
fn dusting_iterates_in_numeric_order() {
	new_test_ext().execute_with(|| {
		PaidUnloadTokenDusting::<Test>::insert(BigEndianPeriod::from(300), ());
		PaidUnloadTokenDusting::<Test>::insert(BigEndianPeriod::from(1), ());
		PaidUnloadTokenDusting::<Test>::insert(BigEndianPeriod::from(256), ());

		let periods = PaidUnloadTokenDusting::<Test>::iter_keys().map(|p| p.0).collect::<Vec<_>>();
		assert_eq!(periods, vec![1, 256, 300]);
	});
}

/// Integration test: create paid token collections in two periods that diverge
/// under LE vs BE ordering, expire both, and verify the offchain worker deletes
/// the numerically oldest period first — using only the public `MemberService`
/// API and `advance_to_block` (which runs the OCW and applies its transactions).
///
/// Period 1 starts at t=100s, period 256 starts at t=25600s (period duration = 100s).
/// Under LE, 256 (`00 01 00 00`) sorts before 1 (`01 00 00 00`), so the OCW
/// would incorrectly try to clean period 256 first.
#[test]
fn ocw_cleans_oldest_period_first_across_byte_boundary() {
	use frame_support::weights::WeightMeter;
	use std::time::Duration;

	new_test_ext().execute_with(|| {
		setup_asset();

		let period_duration = get_u32::<<Test as Config>::PaidUnloadTokenTimePeriod>();
		let expiration = get_u32::<<Test as Config>::PaidUnloadTokenRingExpirationTime>();

		// Create collection for period 1 via on_poll.
		TIME.with(|t| *t.borrow_mut() = Duration::from_secs((1 * period_duration) as u64));
		let mut meter = WeightMeter::new();
		Coinage::on_poll(0, &mut meter);

		// Create collection for period 256 via on_poll.
		TIME.with(|t| *t.borrow_mut() = Duration::from_secs((256 * period_duration) as u64));
		let mut meter = WeightMeter::new();
		Coinage::on_poll(0, &mut meter);

		// Both collections exist (verified via the public MemberService API).
		let id_1 = Coinage::paid_token_collection_identifier(1);
		let id_256 = Coinage::paid_token_collection_identifier(256);
		assert!(
			<Test as Config>::MemberService::ring_status(&id_1, 0).is_some(),
			"period 1 collection must exist"
		);
		assert!(
			<Test as Config>::MemberService::ring_status(&id_256, 0).is_some(),
			"period 256 collection must exist"
		);

		// Advance time past expiration for both periods.
		// Period 256 expires at: (256 + 1) * 100 + 200 = 25900
		let both_expired = (257 * period_duration) + expiration;
		TIME.with(|t| *t.borrow_mut() = Duration::from_secs(both_expired as u64));

		// Advance one block — OCW runs and submits a cleanup transaction, which
		// is then applied. Since neither collection has members, the OCW submits
		// a delete_expired_paid_unload_token_collection for the oldest period.
		let next = frame_system::Pallet::<Test>::block_number() + 1;
		advance_to_block(next);

		// Period 1 should have been deleted (numerically oldest).
		assert!(
			<Test as Config>::MemberService::ring_status(&id_1, 0).is_none(),
			"period 1 collection must be deleted first"
		);
		// Period 256 should still exist.
		assert!(
			<Test as Config>::MemberService::ring_status(&id_256, 0).is_some(),
			"period 256 collection must still exist"
		);
	});
}
