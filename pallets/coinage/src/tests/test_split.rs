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

use crate::{mock::*, pallet::CustomInvalidity, *};
use frame_support::{assert_err, assert_ok};
use sp_runtime::{transaction_validity::TransactionSource, DispatchError};

/// Helper to build a split extrinsic.
pub fn build_split_ext(
	signer: u64,
	split_into: Vec<(Denomination, Vec<u64>)>,
	as_coin: bool,
) -> Extrinsic {
	let split_into = split_into
		.into_iter()
		.map(|(v, dests)| (v, dests.try_into().unwrap()))
		.collect::<Vec<_>>()
		.try_into()
		.unwrap();
	build_signed_as_coin_ext(signer, crate::Call::split { split_into }, as_coin)
}

#[test]
fn split_bad_origin_fail() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		// Construct transaction without AsCoin extension info.
		// The extension will validate as "NotUsing" and set origin to Signed(signer).
		let ext = build_split_ext(signer, vec![(0, vec![2])], false);

		// Validation passes because the extension allows pass-through.
		assert_ok!(Executive::validate_transaction(
			TransactionSource::External,
			ext.clone(),
			Default::default(),
		));

		// Dispatch fails because the pallet expects Origin::Coin.
		// This represents the "fail" scenario (valid transaction, failed execution).
		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref()); // Ensure validity
		assert_err!(res.unwrap(), DispatchError::BadOrigin);
	});
}

#[test]
fn split_coin_not_exist_invalid() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		// Signer has no coin in storage.
		let ext = build_split_ext(signer, vec![(0, vec![2, 3])], true);
		assert_invalid(ext, CustomInvalidity::NoCoin);
	});
}

#[test]
fn split_coin_max_age_invalid() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		let max_age = get_u16::<<Test as Config>::MaximumAge>();
		// Insert coin with max age.
		CoinsByOwner::<Test>::insert(
			signer,
			Coin { instance_id: TEST_INSTANCE_ID, value: 1, age: max_age },
		);

		let ext = build_split_ext(signer, vec![(0, vec![2, 3])], true);
		assert_invalid(ext, CustomInvalidity::CoinTooOld);
	});
}

#[test]
fn split_into_value_too_big_invalid() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		CoinsByOwner::<Test>::insert(
			signer,
			Coin { instance_id: TEST_INSTANCE_ID, value: 1, age: 0 },
		);

		let max_exponent = <Test as Config>::MaximumExponent::get();

		// Split into value > MaxExponent
		let ext = build_split_ext(signer, vec![(max_exponent + 1, vec![2])], true);
		assert_invalid(ext, CustomInvalidity::SplitExponentTooBig);
	});
}

#[test]
fn split_into_value_too_small_invalid() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		CoinsByOwner::<Test>::insert(
			signer,
			Coin { instance_id: TEST_INSTANCE_ID, value: 1, age: 0 },
		);

		let min_exponent = <Test as Config>::MinimumExponent::get();

		// Split into value < MinExponent
		let ext = build_split_ext(signer, vec![(min_exponent - 1, vec![2])], true);
		assert_invalid(ext, CustomInvalidity::SplitExponentTooSmall);
	});
}

#[test]
fn split_dest_has_coin_invalid() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		let dest = 2;
		CoinsByOwner::<Test>::insert(
			signer,
			Coin { instance_id: TEST_INSTANCE_ID, value: 1, age: 0 },
		);
		CoinsByOwner::<Test>::insert(
			dest,
			Coin { instance_id: TEST_INSTANCE_ID, value: 0, age: 0 },
		);

		let ext = build_split_ext(signer, vec![(0, vec![dest])], true);
		assert_invalid(ext, CustomInvalidity::AddressAlreadyHasCoin);
	});
}

#[test]
fn split_dest_has_coin_in_other_instance_invalid() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		let dest = 2;
		let other_instance_id = setup_sponsored_instance();
		assert_ne!(other_instance_id, TEST_INSTANCE_ID);

		CoinsByOwner::<Test>::insert(
			signer,
			Coin { instance_id: TEST_INSTANCE_ID, value: 1, age: 0 },
		);

		// Destination already holds a coin in a different instance.
		CoinsByOwner::<Test>::insert(
			dest,
			Coin { instance_id: other_instance_id, value: 0, age: 0 },
		);

		let ext = build_split_ext(signer, vec![(0, vec![dest])], true);
		assert_invalid(ext, CustomInvalidity::AddressAlreadyHasCoin);
	});
}

#[test]
fn split_sum_mismatch_invalid() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		CoinsByOwner::<Test>::insert(
			signer,
			Coin { instance_id: TEST_INSTANCE_ID, value: 1, age: 0 },
		);

		// Coin Value 1 = 8 units (if min=-2).
		// Split into one Value 0 = 4 units. Mismatch.
		let ext = build_split_ext(signer, vec![(0, vec![2])], true);
		assert_invalid(ext, CustomInvalidity::InvalidSplit);
	});
}

#[test]
fn split_too_many_outputs_one_value_invalid() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		// High value to accommodate many splits without sum issues first
		CoinsByOwner::<Test>::insert(
			signer,
			Coin { instance_id: TEST_INSTANCE_ID, value: 7, age: 0 },
		);

		let max_split_outputs = get_u32::<<Test as Config>::MaxSplitOutputs>();

		// Try max + 1 outputs.
		let dests: Vec<u64> = (100..(100 + max_split_outputs + 1)).map(|x| x as u64).collect();

		let ext = build_split_ext(
			signer,
			vec![
				(-2, dests[..max_split_outputs as usize].to_vec()),
				(-1, vec![dests[max_split_outputs as usize]]),
			],
			true,
		);
		assert_invalid(ext, CustomInvalidity::TooManySplits);
	});
}

#[test]
fn split_too_many_outputs_multiple_values_invalid() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		CoinsByOwner::<Test>::insert(
			signer,
			Coin { instance_id: TEST_INSTANCE_ID, value: 7, age: 0 },
		);

		// 20 outputs of val 0, 20 outputs of val 1. Total 40 > 32 (MaxSplitOutputs).
		let dests1: Vec<u64> = (100..120).collect();
		let dests2: Vec<u64> = (200..220).collect();

		let ext = build_split_ext(signer, vec![(0, dests1), (1, dests2)], true);
		assert_invalid(ext, CustomInvalidity::TooManySplits);
	});
}

#[test]
fn split_not_sorted_invalid() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		CoinsByOwner::<Test>::insert(
			signer,
			Coin { instance_id: TEST_INSTANCE_ID, value: 5, age: 0 },
		);

		// Order: Value 1 then Value 0. Descending.
		let ext = build_split_ext(signer, vec![(1, vec![2]), (0, vec![3])], true);
		assert_invalid(ext, CustomInvalidity::SplitIntoNotSorted);
	});
}

#[test]
fn split_valid_success() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let signer = 1;
		// Value 1.
		CoinsByOwner::<Test>::insert(
			signer,
			Coin { instance_id: TEST_INSTANCE_ID, value: 1, age: 0 },
		);

		// Split 1 -> 0, 0.
		let dest1 = 2;
		let dest2 = 3;
		let ext = build_split_ext(signer, vec![(0, vec![dest1, dest2])], true);

		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		assert!(!CoinsByOwner::<Test>::contains_key(signer));
		assert_eq!(
			CoinsByOwner::<Test>::get(dest1).unwrap(),
			Coin { instance_id: TEST_INSTANCE_ID, value: 0, age: 1 }
		);
		assert_eq!(
			CoinsByOwner::<Test>::get(dest2).unwrap(),
			Coin { instance_id: TEST_INSTANCE_ID, value: 0, age: 1 }
		);
		System::assert_has_event(
			crate::Event::<Test>::CoinSplit { instance_id: TEST_INSTANCE_ID, output_count: 2 }
				.into(),
		);
	});
}

#[test]
fn split_edge_cases_high_output_count_and_value_invalid_sum() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		let max_exponent = <Test as Config>::MaximumExponent::get();
		// Insert coin with MaxExponent.
		CoinsByOwner::<Test>::insert(
			signer,
			Coin { instance_id: TEST_INSTANCE_ID, value: max_exponent, age: 0 },
		);

		assert_eq!(get_u32::<<Test as Config>::MaxSplitOutputs>(), 32);

		// Create a split with max outputs (32) of max value (7).
		// The sum will be huge (32 * 2^9 = 16384 units).
		// But the origin coin (value 7) is only 512 units.
		// This ensures arithmetic doesn't overflow/panic and correctly identifies InvalidSplit (sum
		// mismatch).

		let dests: Vec<u64> = (100..132).collect(); // 32 destinations
		let ext = build_split_ext(signer, vec![(max_exponent, dests)], true);

		assert_invalid(ext, CustomInvalidity::InvalidSplit);
	});
}

#[test]
fn split_duplicate_destination_invalid() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		let dest = 2;

		CoinsByOwner::<Test>::insert(
			signer,
			Coin { instance_id: TEST_INSTANCE_ID, value: 5, age: 0 },
		);

		// Duplicate dest in same value list
		let ext = build_split_ext(signer, vec![(4, vec![dest, dest])], true);
		assert_invalid(ext, CustomInvalidity::DuplicateDestinationsInSplit);
	});

	new_test_ext().execute_with(|| {
		let signer = 1;
		let dest = 2;
		let other_dest = 3;

		CoinsByOwner::<Test>::insert(
			signer,
			Coin { instance_id: TEST_INSTANCE_ID, value: 5, age: 0 },
		);

		// Duplicate dest across different value lists
		let ext = build_split_ext(signer, vec![(3, vec![dest, other_dest]), (4, vec![dest])], true);
		assert_invalid(ext, CustomInvalidity::DuplicateDestinationsInSplit);
	});
}

#[test]
fn split_empty_outputs_invalid() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		CoinsByOwner::<Test>::insert(
			signer,
			Coin { instance_id: TEST_INSTANCE_ID, value: 0, age: 0 },
		);

		// Empty destination list for a value
		let ext = build_split_ext(signer, vec![(0, vec![])], true);
		assert_invalid(ext, CustomInvalidity::EmptySplit);
	});
}

/// Verify that validation stops early when destination count exceeds MaxSplitOutputs,
/// preventing `MaxSplitOutputs^2` storage reads.
#[test]
fn split_too_many_outputs_stops_early() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		CoinsByOwner::<Test>::insert(
			signer,
			Coin { instance_id: TEST_INSTANCE_ID, value: 7, age: 0 },
		);

		let max_split_outputs = get_u32::<<Test as Config>::MaxSplitOutputs>();

		// Create max + 2 destinations. The destination at index max + 1 already has a coin.
		// If validation was unbounded, it would check all destinations and return
		// AddressAlreadyHasCoin. With the fix, it should return TooManySplits first because
		// the count check happens before the storage reads for destinations beyond the limit.

		let num_dests = max_split_outputs as usize + 2;

		let dests: Vec<u64> = (100u64..).take(num_dests).collect();
		let dest_with_coin = dests[max_split_outputs as usize];
		CoinsByOwner::<Test>::insert(
			dest_with_coin,
			Coin { instance_id: TEST_INSTANCE_ID, value: 0, age: 0 },
		);

		let split_into = vec![
			(-2, dests[0..max_split_outputs as usize].to_vec()),
			(-1, dests[max_split_outputs as usize..].to_vec()),
		];
		let ext = build_split_ext(signer, split_into, true);

		// Should get TooManySplits, NOT AddressAlreadyHasCoin, proving validation stopped early.
		assert_invalid(ext, CustomInvalidity::TooManySplits);
	});
}
