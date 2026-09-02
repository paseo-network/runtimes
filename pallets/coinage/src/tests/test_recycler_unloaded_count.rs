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

//! Tests for [`RecyclersUnloadedCount`], the per-ring count of unloaded aliases.
//!
//! The count exists so a reader can get the number of unloaded coins of a live recycler with one
//! storage read instead of a scan over [`RecyclerAliasStates`]. Every test therefore also asserts
//! the count against that scan.

use crate::{mock::*, *};
use indiv_support::traits::AppendOnlyMembers;
use sp_runtime::bounded_vec;
use verifiable::GenerateVerifiable;

/// The number of unloaded aliases of a ring, counted by scanning [`RecyclerAliasStates`].
///
/// This is what [`RecyclersUnloadedCount`] replaces, so the tests compare the two.
fn scan_unloaded(value: Denomination, index: RingIndex) -> u32 {
	RecyclerAliasStates::<Test>::iter_prefix((TEST_INSTANCE_ID, value, index))
		.filter(|(_, state)| matches!(state, AliasState::Unloaded))
		.count() as u32
}

fn assert_unloaded_count(value: Denomination, index: RingIndex, expected: u32) {
	assert_eq!(
		RecyclerManager::<Test>::unloaded_count(TEST_INSTANCE_ID, value, index),
		Some(expected),
		"stored unloaded count"
	);
	assert_eq!(scan_unloaded(value, index), expected, "unloaded aliases in storage");
}

fn alias_of(secret: &Secret) -> Alias {
	CryptoOf::<Test>::alias_in_context(secret, crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref())
		.unwrap()
}

/// The count follows final unloads only.
///
/// A failed dispatch leaves the fee alias premarked by `prepare` and then turned into a retry lock
/// by `post_dispatch`, which must not count. The retry then unloads both aliases and adds two: one
/// through the extension's `post_dispatch` (the fee alias) and one through
/// [`RecyclerManager::unload`].
#[test]
fn unloaded_count_only_counts_final_unloads() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let value: Denomination = 0;
		let dest = CHARLIE;
		let (secrets, index, revision) = setup_recycler(value, 2, 0);
		let alias0 = alias_of(&secrets[0]);
		let alias1 = alias_of(&secrets[1]);

		assert_unloaded_count(value, index, 0);

		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![alias0, alias1],
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
		};

		// Validation quotes the conversion and accepts the call, then the swap takes more of the
		// asset than the quote allowed, so the dispatch fails.
		set_fee_conversion_swap_surcharge(1);
		let ext =
			build_unload_from_output_ext(call.clone(), value, index, revision, &secrets[0..2]);
		let result = Executive::apply_extrinsic(ext);
		assert!(matches!(result, Ok(Err(_))), "dispatch should fail: {result:?}");

		// `alias0` carries a retry lock, `alias1` was reverted with the dispatch. Neither is a
		// successful unload, so the count stays at zero.
		assert!(
			super::get_recycler_alias_lock_until(value, index, alias0).is_some(),
			"failed dispatch should lock the fee alias for retry"
		);
		assert_unloaded_count(value, index, 0);

		// Wait out the lock and retry against a market in line with its quote.
		set_fee_conversion_swap_surcharge(0);
		let lock_until = super::get_recycler_alias_lock_until(value, index, alias0)
			.expect("failed dispatch should lock the alias");
		advance_until_time(lock_until as u32);

		let refreshed_ext =
			build_unload_from_output_ext(call, value, index, revision, &secrets[0..2]);
		let result = Executive::apply_extrinsic(refreshed_ext);
		assert!(matches!(result, Ok(Ok(_))), "retry should succeed: {result:?}");

		assert_unloaded_count(value, index, 2);
	});
}

/// A second unload of a different alias adds one, and the count never double-counts an alias that
/// was already unloaded: a repeat attempt is rejected before it can be marked again.
#[test]
fn unloaded_count_grows_once_per_alias() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let value: Denomination = 0;
		let dest = CHARLIE;
		let (secrets, index, revision) = setup_recycler(value, 3, 0);

		let unload = |secret_range: core::ops::Range<usize>, to: u64| {
			let aliases = secrets[secret_range.clone()].iter().map(alias_of).collect::<Vec<_>>();
			let call = crate::Call::<Test>::unload_recycler_into_external_asset {
				instance_id: TEST_INSTANCE_ID,
				aliases: aliases.try_into().unwrap(),
				value,
				index,
				revision,
				to,
				max_fee: unload_token_fee_in_asset(),
			};
			let ext =
				build_unload_from_output_ext(call, value, index, revision, &secrets[secret_range]);
			Executive::apply_extrinsic(ext)
		};

		assert_eq!(unload(0..2, dest), Ok(Ok(())));
		assert_unloaded_count(value, index, 2);

		assert_eq!(unload(2..3, dest + 1), Ok(Ok(())));
		assert_unloaded_count(value, index, 3);

		// The same alias again: rejected in validation, so the count is untouched.
		assert_invalid(
			build_unload_from_output_ext(
				crate::Call::<Test>::unload_recycler_into_external_asset {
					instance_id: TEST_INSTANCE_ID,
					aliases: bounded_vec![alias_of(&secrets[2])],
					value,
					index,
					revision,
					to: dest + 2,
					max_fee: unload_token_fee_in_asset(),
				},
				value,
				index,
				revision,
				&secrets[2..3],
			),
			CustomInvalidity::RecyclerAlreadyUnloaded,
		);
		assert_unloaded_count(value, index, 3);
	});
}

/// Cleaning an expired ring drops its count, which is what keeps the map bounded by the number of
/// live rings. The alias states themselves are dusted separately and later.
#[test]
fn unloaded_count_is_dropped_when_the_ring_is_cleaned() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: Denomination = 0;
		let ring_capacity = R2E10_RING_CAPACITY;
		let dest = 5000u64;

		// Fill ring 0 to capacity and start ring 1.
		let (secrets, _index, _revision) = setup_recycler(value, ring_capacity + 1, 0);
		for _ in 0..10 {
			Members::process_maintenance();
		}

		let identifier = Coinage::recycler_collection_identifier(TEST_INSTANCE_ID, value);
		let status = <Test as Config>::MemberService::ring_status(&identifier, 0).unwrap();
		assert_eq!(status.total, ring_capacity);
		let immutable_since =
			status.immutable_since.expect("a full append-only ring is immutable") as u32;
		let revision = <Test as Config>::MemberService::ring_revision(&identifier, 0).unwrap();

		let aliases = secrets[0..5].iter().map(alias_of).collect::<Vec<_>>();
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: aliases.try_into().unwrap(),
			value,
			index: 0,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
		};
		let ext = build_unload_from_output_ext(call, value, 0, revision, &secrets[0..5]);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		assert_unloaded_count(value, 0, 5);

		// `RingStatus::total` minus the count is the number of coins the ring still holds, which is
		// what the ring is archived with.
		assert_eq!(
			status.total -
				RecyclerManager::<Test>::unloaded_count(TEST_INSTANCE_ID, value, 0)
					.expect("the ring is counted"),
			ring_capacity - 5
		);

		// Expire the ring and let the offchain worker clean it.
		let expiration = get_u32::<<Test as crate::Config>::RecyclerExpirationTime>();
		advance_until_time(immutable_since + expiration);
		advance_block();

		assert_eq!(RecyclersLastRemovedRingIndex::<Test>::get(TEST_INSTANCE_ID, value), Some(0));
		assert!(
			!RecyclersUnloadedCount::<Test>::contains_key((TEST_INSTANCE_ID, value, 0u32)),
			"cleaning the ring must drop its unloaded count"
		);
		assert_eq!(
			RecyclersArchives::<Test>::get((TEST_INSTANCE_ID, value, 0u32))
				.unwrap()
				.remaining,
			ring_capacity - 5
		);

		// The alias states outlive the count until dusting clears them, which must not bring the
		// count back.
		assert_eq!(scan_unloaded(value, 0), 5);
		assert_eq!(RecyclerManager::<Test>::unloaded_count(TEST_INSTANCE_ID, value, 0), None);
	});
}

/// A ring that was already unloaded from when the count was introduced is never counted.
///
/// Its earlier unloads are only in [`RecyclerAliasStates`], so counting the later ones would give a
/// partial number that reads like a complete one. Such a ring is recognised by having alias states
/// but no count, and it keeps no count for the rest of its life.
#[test]
fn rings_that_predate_the_count_are_never_counted() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let value: Denomination = 0;
		let dest = CHARLIE;
		let (secrets, index, revision) = setup_recycler(value, 3, 0);

		let unload = |secret: usize, to: u64| {
			let call = crate::Call::<Test>::unload_recycler_into_external_asset {
				instance_id: TEST_INSTANCE_ID,
				aliases: bounded_vec![alias_of(&secrets[secret])],
				value,
				index,
				revision,
				to,
				max_fee: unload_token_fee_in_asset(),
			};
			let ext = build_unload_from_output_ext(
				call,
				value,
				index,
				revision,
				&secrets[secret..secret + 1],
			);
			Executive::apply_extrinsic(ext)
		};

		// Stand in for the state a runtime upgrade finds: a ring with an unload that was never
		// counted, because the count did not exist when it happened.
		assert_eq!(unload(0, dest), Ok(Ok(())));
		RecyclersUnloadedCount::<Test>::remove((TEST_INSTANCE_ID, value, index));

		// The ring has alias states but no count, so it stays uncounted rather than starting a
		// partial one.
		assert_eq!(unload(1, dest + 1), Ok(Ok(())));
		assert_eq!(scan_unloaded(value, index), 2);
		assert_eq!(RecyclerManager::<Test>::unloaded_count(TEST_INSTANCE_ID, value, index), None);

		// Cleaning it must not report a mismatch between the missing count and the two aliases.
		let cleaned = RecyclerAliasStates::<Test>::iter_prefix((TEST_INSTANCE_ID, value, index))
			.filter(|(_, state)| matches!(state, AliasState::Unloaded))
			.count();
		assert_eq!(cleaned, 2);
	});
}

/// A ring that has no alias state yet has had no unload, so its count can start at zero. This is
/// what lets a ring created after the count was introduced be counted without a migration.
#[test]
fn a_ring_without_alias_states_starts_counting_from_zero() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let value: Denomination = 0;
		let (secrets, index, revision) = setup_recycler(value, 2, 0);

		// No alias state and no count entry: the ring is new, not one that predates the count, so
		// it already reports zero without anything having been written for it.
		assert_eq!(scan_unloaded(value, index), 0);
		assert!(!RecyclersUnloadedCount::<Test>::contains_key((TEST_INSTANCE_ID, value, index)));
		assert_unloaded_count(value, index, 0);

		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![alias_of(&secrets[0])],
			value,
			index,
			revision,
			to: CHARLIE,
			max_fee: unload_token_fee_in_asset(),
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		assert_unloaded_count(value, index, 1);
	});
}

/// A failed dispatch is the one way a ring gets an alias state without an unload, and it must not
/// leave the ring looking like one that predates the count.
///
/// The premark starts the count and the lock takes it back down to zero, so the entry stays and the
/// next unload counts from there rather than treating the ring as uncounted.
#[test]
fn a_lock_left_by_a_failed_dispatch_keeps_the_ring_counted() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let value: Denomination = 0;
		let (secrets, index, revision) = setup_recycler(value, 2, 0);
		let alias0 = alias_of(&secrets[0]);

		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![alias0],
			value,
			index,
			revision,
			to: CHARLIE,
			max_fee: unload_token_fee_in_asset(),
		};

		set_fee_conversion_swap_surcharge(1);
		let ext =
			build_unload_from_output_ext(call.clone(), value, index, revision, &secrets[0..1]);
		assert!(matches!(Executive::apply_extrinsic(ext), Ok(Err(_))));

		// The ring now has an alias state, so the count entry has to be there already: otherwise
		// the ring would be mistaken for one that predates the count.
		assert!(super::get_recycler_alias_lock_until(value, index, alias0).is_some());
		assert_unloaded_count(value, index, 0);

		set_fee_conversion_swap_surcharge(0);
		let lock_until = super::get_recycler_alias_lock_until(value, index, alias0)
			.expect("failed dispatch should lock the alias");
		advance_until_time(lock_until as u32);

		let refreshed_ext =
			build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);
		assert!(matches!(Executive::apply_extrinsic(refreshed_ext), Ok(Ok(_))));
		assert_unloaded_count(value, index, 1);
	});
}
