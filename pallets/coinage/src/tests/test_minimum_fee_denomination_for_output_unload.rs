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
//! with respect to the minimum fee denomination requirement.

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

		// Set minimum to 0, then use denomination 0 (== minimum)
		MinimumExponentForOutputUnloadFee::set(&0);

		let value: Denomination = 0; // $1 coin, equals MinimumExponentForOutputUnloadFee
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
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
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

		// Set minimum to -2, then use denomination 0 (> minimum)
		MinimumExponentForOutputUnloadFee::set(&-2);

		let value: Denomination = 0; // $1 coin, greater than MinimumExponentForOutputUnloadFee (-2)
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
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
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

		// Set minimum to 0, then try to use denomination -2 (< minimum)
		MinimumExponentForOutputUnloadFee::set(&0);

		let value: Denomination = -2; // $0.25 coin, less than MinimumExponentForOutputUnloadFee (0)
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
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);

		// Apply the extrinsic - should fail with FeeCoinBelowMinimum
		assert_invalid(ext, CustomInvalidity::FeeCoinBelowMinimum);
	});
}

// ============================================================================
// Failed-dispatch lock test
// ============================================================================

#[test]
fn failed_call_locks_first_alias_instead_of_destroying_it() {
	// Test that if the `unload_recycler_into_external_asset` call fails
	// (e.g., total unloaded amount < fee), the first alias is preserved behind a temporary
	// failed-dispatch lock instead of being consumed permanently.
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: Denomination = 0; // $1 coin = 1000 underlying units
		let dest = CHARLIE;
		let max_fee = unload_token_fee_in_asset();
		// Force a dispatch-time failure: validation quotes the conversion and accepts the call,
		// then the swap takes more of the asset than the quote bounding it allowed.
		set_fee_conversion_swap_surcharge(1);

		let (secrets, index, revision) = setup_recycler(value, 2, 0);

		// Use a real recycler alias so the extension follows the normal reserve-on-prepare path.
		let alias = CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let aliases = BoundedVec::try_from(vec![alias]).unwrap();

		// Verify alias is not marked as unloaded before
		assert!(
			!matches!(
				RecyclerAliasStates::<Test>::get((TEST_INSTANCE_ID, value, index, alias)),
				Some(AliasState::Unloaded),
			),
			"Alias should not be marked as unloaded before the extrinsic"
		);

		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(TEST_INSTANCE_ID), 0);

		// Build call and extrinsic
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value,
			index,
			revision,
			to: dest,
			max_fee,
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);

		// Apply the extrinsic - dispatch should fail (the conversion is gone by then)
		let result = Executive::apply_extrinsic(ext);
		// The extrinsic validates successfully but dispatch fails
		assert!(matches!(result, Ok(Err(_))), "Dispatch should fail: {result:?}");

		// The failed call must roll back the temporary "unloaded" marker so the alias is not
		// treated as permanently spent.
		assert!(
			!matches!(
				RecyclerAliasStates::<Test>::get((TEST_INSTANCE_ID, value, index, alias)),
				Some(AliasState::Unloaded),
			),
			"First alias should be restored after failed dispatch"
		);
		// Instead of being destroyed, the alias is parked behind a retry lock for a while.
		let lock_until = super::get_recycler_alias_lock_until(value, index, alias)
			.expect("failed dispatch should lock the first alias");
		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(TEST_INSTANCE_ID), 0);

		// A second submission with the same alias should now be rejected during validation until
		// the lock expires, with the market back in line so the lock is what rejects it.
		set_fee_conversion_swap_surcharge(0);
		let ext_locked = build_unload_from_output_ext(
			crate::Call::<Test>::unload_recycler_into_external_asset {
				instance_id: TEST_INSTANCE_ID,
				aliases: BoundedVec::try_from(vec![alias]).unwrap(),
				value,
				index,
				revision,
				to: dest,
				max_fee,
			},
			value,
			index,
			revision,
			&secrets[0..1],
		);
		assert_invalid(ext_locked, CustomInvalidity::AliasTemporarilyLocked);

		// Once the timeout passes the alias becomes usable again.
		advance_until_time(lock_until as u32);
		assert_eq!(super::get_recycler_alias_lock_until(value, index, alias), None);
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
	// Scenario: First denomination (250) is higher than the unload token fee (2). Verifies
	// that only the unload token fee amount is deducted, not the entire first coin.
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		// Set minimum to -2 (smallest coin), fee to 2
		MinimumExponentForOutputUnloadFee::set(&-2);
		MockPaidUnloadTokenFeeOverride::set(&Some(2));

		let value: Denomination = -2; // $0.25 coin = 250 underlying units each
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
		let market_before = AssetsWithHolder::balance(10, &MOCK_MARKET);

		// Build call and extrinsic with all 4 aliases
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..4]);

		// Apply the extrinsic
		Executive::apply_extrinsic(ext).expect("valid").expect("successful");

		// Total output = 4 * 250 = 1000 units
		// Fee = 2 units
		// Charlie should receive: 1000 - 2 = 998 units
		let charlie_external_asset_after = AssetsWithHolder::balance(10, &dest);
		let market_after = AssetsWithHolder::balance(10, &MOCK_MARKET);

		assert_eq!(
			charlie_external_asset_after - charlie_external_asset_before,
			998,
			"Charlie should receive total output minus fee"
		);
		assert_eq!(
			market_after - market_before,
			2,
			"The market should receive the asset the fee cost"
		);

		// Verify the event shows correct values
		System::assert_has_event(
			crate::Event::<Test>::RecyclerUnloadedIntoExternalAsset {
				instance_id: TEST_INSTANCE_ID,
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
	// Scenario: First denomination (250) is lower than the unload token fee (300). Verifies that
	// multiple coins are consolidated and the unload token fee is deducted from the total.
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		// Use default minimum (-2) and a fee higher than a single coin
		MinimumExponentForOutputUnloadFee::set(&-2);
		MockPaidUnloadTokenFeeOverride::set(&Some(300)); // Fee = 300 units (> single coin of 250)

		let value: Denomination = -2; // $0.25 coin = 250 underlying units each
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
		let market_before = AssetsWithHolder::balance(10, &MOCK_MARKET);

		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..3]);

		Executive::apply_extrinsic(ext).expect("valid").expect("successful");

		let charlie_external_asset_after = AssetsWithHolder::balance(10, &dest);
		let market_after = AssetsWithHolder::balance(10, &MOCK_MARKET);

		// First coin alone (250) cannot cover the fee (300).
		// But with combined output (750), we get 750 - 300 = 450.
		// This proves fee is deducted from total, not just first coin.
		assert_eq!(
			charlie_external_asset_after - charlie_external_asset_before,
			450,
			"Charlie should receive combined output minus fee (3*250 - 300 = 450)"
		);
		assert_eq!(
			market_after - market_before,
			300,
			"The market should receive the asset the fee cost"
		);

		System::assert_has_event(
			crate::Event::<Test>::RecyclerUnloadedIntoExternalAsset {
				instance_id: TEST_INSTANCE_ID,
				to: dest,
				value,
				input_count: 3,
				amount: 450,
			}
			.into(),
		);
	});
}
