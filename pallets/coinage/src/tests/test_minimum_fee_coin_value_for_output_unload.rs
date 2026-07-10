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

//! Tests for `MinimumExponentForOutputUnloadFee` configuration parameter.
//!
//! These tests verify the behavior of `AsUnloadTokenFromOutput` extension validation
//! with respect to the minimum fee coin value requirement.

use crate::{mock::*, *};
use frame_support::{traits::fungibles::Inspect, BoundedVec};
use verifiable::GenerateVerifiable;

// ============================================================================
// Extension validation tests for MinimumExponentForOutputUnloadFee
// ============================================================================

#[test]
fn validation_succeeds_when_fee_recycler_value_equals_minimum() {
	// Test `AsUnloadTokenFromOutput` validation succeeds when
	// `fee_recycler_value` == `MinimumExponentForOutputUnloadFee`.
	new_test_ext().execute_with(|| {
		setup_balances();

		// Set minimum to 0, then use coin value 0 (== minimum)
		MinimumExponentForOutputUnloadFee::set(&0);

		let value: CoinValue = 0; // $1 coin, equals MinimumExponentForOutputUnloadFee
		let dest = CHARLIE;

		let (secrets, index, revision) = setup_recycler(value, 2, 0);

		// Get alias for the call
		let alias = CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let aliases = BoundedVec::try_from(vec![alias]).unwrap();

		let charlie_external_asset_before = AssetsWithHolder::balance(10, &dest);

		// Build call and extrinsic
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			aliases,
			value,
			index,
			revision,
			to: dest,
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);

		// Apply the extrinsic - should succeed because value (0) == minimum (0)
		Executive::apply_extrinsic(ext).expect("valid").expect("successful");

		// Check transfer succeeded (value=0 means 1000 units, minus fee of 2)
		let charlie_external_asset_after = AssetsWithHolder::balance(10, &dest);
		assert_eq!(charlie_external_asset_after - charlie_external_asset_before, 998);
	});
}

#[test]
fn validation_succeeds_when_fee_recycler_value_greater_than_minimum() {
	// Test `AsUnloadTokenFromOutput` validation succeeds when
	// `fee_recycler_value` > `MinimumExponentForOutputUnloadFee`.
	new_test_ext().execute_with(|| {
		setup_balances();

		// Set minimum to -2, then use coin value 0 (> minimum)
		MinimumExponentForOutputUnloadFee::set(&-2);

		let value: CoinValue = 0; // $1 coin, greater than MinimumExponentForOutputUnloadFee (-2)
		let dest = CHARLIE;

		let (secrets, index, revision) = setup_recycler(value, 2, 0);

		// Get alias for the call
		let alias = CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let aliases = BoundedVec::try_from(vec![alias]).unwrap();

		let charlie_external_asset_before = AssetsWithHolder::balance(10, &dest);

		// Build call and extrinsic
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			aliases,
			value,
			index,
			revision,
			to: dest,
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);

		// Apply the extrinsic - should succeed because value (0) > minimum (-2)
		Executive::apply_extrinsic(ext).expect("valid").expect("successful");

		// Check transfer succeeded (value=0 means 1000 units, minus fee of 2)
		let charlie_external_asset_after = AssetsWithHolder::balance(10, &dest);
		assert_eq!(charlie_external_asset_after - charlie_external_asset_before, 998);
	});
}

#[test]
fn validation_fails_with_fee_recycler_coin_too_small() {
	// Test `AsUnloadTokenFromOutput` validation fails with `FeeCoinBelowMinimum`
	// when `fee_recycler_value` < `MinimumExponentForOutputUnloadFee`.
	new_test_ext().execute_with(|| {
		setup_balances();

		// Set minimum to 0, then try to use coin value -2 (< minimum)
		MinimumExponentForOutputUnloadFee::set(&0);

		let value: CoinValue = -2; // $0.25 coin, less than MinimumExponentForOutputUnloadFee (0)
		let dest = CHARLIE;

		let (secrets, index, revision) = setup_recycler(value, 2, 0);

		// Get alias for the call
		let alias = CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let aliases = BoundedVec::try_from(vec![alias]).unwrap();

		// Build call and extrinsic
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			aliases,
			value,
			index,
			revision,
			to: dest,
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);

		// Apply the extrinsic - should fail with FeeCoinBelowMinimum
		assert_invalid(ext, CustomInvalidity::FeeCoinBelowMinimum);
	});
}

// ============================================================================
// Spam penalty test
// ============================================================================

#[test]
fn failed_call_keeps_first_alias_marked_as_unloaded_spam_penalty() {
	// Test that if the `unload_recycler_into_external_asset` call fails
	// (e.g., total unloaded amount < fee), the first alias remains marked as
	// unloaded (spam penalty).
	new_test_ext().execute_with(|| {
		setup_balances();

		// Set fee higher than the coin value to cause failure
		MockPaidUnloadTokenFeeOverride::set(&Some(2000)); // Fee = 2000, coin = 1000

		let value: CoinValue = 0; // $1 coin = 1000 underlying units
		let dest = CHARLIE;

		let (secrets, index, revision) = setup_recycler(value, 2, 0);

		// Get alias for the call
		let alias = CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let aliases = BoundedVec::try_from(vec![alias]).unwrap();

		// Verify alias is not marked as unloaded before
		assert!(
			!RecyclersUnloaded::<Test>::contains_key((value, index, alias)),
			"Alias should not be marked as unloaded before the extrinsic"
		);

		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(), 0);

		// Build call and extrinsic
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			aliases,
			value,
			index,
			revision,
			to: dest,
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);

		// Apply the extrinsic - dispatch should fail (fee > coin value)
		let result = Executive::apply_extrinsic(ext);
		// The extrinsic validates successfully but dispatch fails
		assert!(matches!(result, Ok(Err(_))), "Dispatch should fail: {result:?}");

		// Verify first alias remains marked as unloaded (spam penalty applied in prepare)
		assert!(
			RecyclersUnloaded::<Test>::contains_key((value, index, alias)),
			"First alias should remain marked as unloaded as spam penalty"
		);

		// Verify destroyed value is tracked (post_dispatch penalty)
		// fee_recycler_value=0 -> 1000 underlying units
		assert_eq!(
			TotalValueOfDestroyedCoins::<Test>::get(),
			1000,
			"Destroyed coin value should be tracked"
		);
	});
}

// ============================================================================
// Weight calculation test
// ============================================================================

#[test]
fn weight_for_unload_recycler_paying_using_output_is_non_zero() {
	// Test that `weight_for_unload_recycler_paying_using_output` returns a non-zero weight.
	new_test_ext().execute_with(|| {
		let weight = Pallet::<Test>::weight_for_unload_recycler_paying_using_output();

		assert!(
			weight.ref_time() > 0,
			"weight_for_unload_recycler_paying_using_output ref_time should be non-zero"
		);
		assert!(
			weight.proof_size() > 0,
			"weight_for_unload_recycler_paying_using_output proof_size should be non-zero"
		);
	});
}

// ============================================================================
// Fee deduction from combined output test
// ============================================================================

#[test]
fn successful_unload_deducts_fee_from_combined_output_amount() {
	// Test successful `unload_recycler_into_external_asset` with `FromOutput` fee,
	// ensuring the unload token fee is properly deducted from the combined output amount.
	//
	// Scenario: First coin value (250) is higher than the unload token fee (2). Verifies
	// that only the unload token fee amount is deducted, not the entire first coin.
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		// Set minimum to -2 (smallest coin), fee to 2
		MinimumExponentForOutputUnloadFee::set(&-2);
		MockPaidUnloadTokenFeeOverride::set(&Some(2));

		let value: CoinValue = -2; // $0.25 coin = 250 underlying units each
		let dest = CHARLIE;

		// Setup recycler with 4 members
		let (secrets, index, revision) = setup_recycler(value, 4, 0);

		// Get aliases for all 4 members
		let alias0 = CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let alias1 = CryptoOf::<Test>::alias_in_context(
			&secrets[1],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let alias2 = CryptoOf::<Test>::alias_in_context(
			&secrets[2],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let alias3 = CryptoOf::<Test>::alias_in_context(
			&secrets[3],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();

		let aliases = BoundedVec::try_from(vec![alias0, alias1, alias2, alias3]).unwrap();

		let charlie_external_asset_before = AssetsWithHolder::balance(10, &dest);
		let fee_dest_before = AssetsWithHolder::balance(10, &FEE_DESTINATION);

		// Build call and extrinsic with all 4 aliases
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			aliases,
			value,
			index,
			revision,
			to: dest,
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..4]);

		// Apply the extrinsic
		Executive::apply_extrinsic(ext).expect("valid").expect("successful");

		// Total output = 4 * 250 = 1000 units
		// Fee = 2 units
		// Charlie should receive: 1000 - 2 = 998 units
		let charlie_external_asset_after = AssetsWithHolder::balance(10, &dest);
		let fee_dest_after = AssetsWithHolder::balance(10, &FEE_DESTINATION);

		assert_eq!(
			charlie_external_asset_after - charlie_external_asset_before,
			998,
			"Charlie should receive total output minus fee"
		);
		assert_eq!(fee_dest_after - fee_dest_before, 2, "Fee destination should receive the fee");

		// Verify the event shows correct values
		System::assert_has_event(
			crate::Event::<Test>::RecyclerUnloadedIntoExternalAsset {
				to: dest,
				value,
				input_count: 4,
				amount: 998,
			}
			.into(),
		);
	});
}

#[test]
fn fee_deducted_from_total_not_first_coin() {
	// Test that unload token fee is deducted from total combined output, not just the first coin.
	//
	// Scenario: First coin value (250) is lower than the unload token fee (300). Verifies that
	// multiple coins are consolidated and the unload token fee is deducted from the total.
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		// Use default minimum (-2) and a fee higher than a single coin
		MinimumExponentForOutputUnloadFee::set(&-2);
		MockPaidUnloadTokenFeeOverride::set(&Some(300)); // Fee = 300 units (> single coin of 250)

		let value: CoinValue = -2; // $0.25 coin = 250 underlying units each
		let dest = CHARLIE;

		// Setup recycler with 3 members
		// Total = 3 * 250 = 750 units
		// Fee = 300 units
		// Output = 450 units
		let (secrets, index, revision) = setup_recycler(value, 3, 0);

		let alias0 = CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let alias1 = CryptoOf::<Test>::alias_in_context(
			&secrets[1],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let alias2 = CryptoOf::<Test>::alias_in_context(
			&secrets[2],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();

		let aliases = BoundedVec::try_from(vec![alias0, alias1, alias2]).unwrap();

		let charlie_external_asset_before = AssetsWithHolder::balance(10, &dest);
		let fee_dest_before = AssetsWithHolder::balance(10, &FEE_DESTINATION);

		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			aliases,
			value,
			index,
			revision,
			to: dest,
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..3]);

		Executive::apply_extrinsic(ext).expect("valid").expect("successful");

		let charlie_external_asset_after = AssetsWithHolder::balance(10, &dest);
		let fee_dest_after = AssetsWithHolder::balance(10, &FEE_DESTINATION);

		// First coin alone (250) cannot cover the fee (300).
		// But with combined output (750), we get 750 - 300 = 450.
		// This proves fee is deducted from total, not just first coin.
		assert_eq!(
			charlie_external_asset_after - charlie_external_asset_before,
			450,
			"Charlie should receive combined output minus fee (3*250 - 300 = 450)"
		);
		assert_eq!(fee_dest_after - fee_dest_before, 300, "Fee destination should receive the fee");

		System::assert_has_event(
			crate::Event::<Test>::RecyclerUnloadedIntoExternalAsset {
				to: dest,
				value,
				input_count: 3,
				amount: 450,
			}
			.into(),
		);
	});
}
