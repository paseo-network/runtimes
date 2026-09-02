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
	extension::{AsCoinage, Pre},
	mock::*,
	pallet::{CustomInvalidity, DenominationToAssetAmountError},
	*,
};
use codec::Encode;
use frame_support::{assert_ok, traits::UnixTime};
use sp_runtime::{traits::TransactionExtension as _, DispatchError};
use verifiable::GenerateVerifiable;

#[test]
fn failed_coin_extrinsic_restores_coin() {
	// When a coin extrinsic fails dispatch, the coin consumed in prepare should be restored.
	new_test_ext().execute_with(|| {
		let value: Denomination = 0; // exponent 0 → 1000 underlying units
		let coin = Coin { instance_id: TEST_INSTANCE_ID, value, age: 0 };
		let coin_id = 1;
		let current_block = frame_system::Pallet::<Test>::block_number();
		let expected_lock_until =
			current_block.saturating_add(get_u64::<<Test as Config>::CoinFailureLockPeriod>());

		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(TEST_INSTANCE_ID), 0);
		assert!(!CoinsByOwner::<Test>::contains_key(coin_id));

		let pre = Pre::<Test>::UsingCoin { coin_id, coin };
		let info = Default::default();
		let post_info = Default::default();
		let err = Err(DispatchError::Other("test"));

		let result = AsCoinage::<Test>::post_dispatch_details(pre, &info, &post_info, 0, &err);
		assert_ok!(result);

		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(TEST_INSTANCE_ID), 0);
		assert_eq!(
			CoinsByOwner::<Test>::get(coin_id),
			Some(Coin { instance_id: TEST_INSTANCE_ID, value, age: 0 })
		);
		assert_eq!(
			LockedCoins::<Test>::get(coin_id),
			Some(LockInfo {
				reason: LockReason::FailedDispatch { retries: 0 },
				until: expected_lock_until
			})
		);
		assert_eq!(Coinage::get_coin_lock_until(coin_id), Some(expected_lock_until));
	});
}

#[test]
fn successful_coin_extrinsic_clears_existing_lock() {
	new_test_ext().execute_with(|| {
		let coin_id = 1u64;
		let coin = Coin { instance_id: TEST_INSTANCE_ID, value: 0, age: 0 };

		LockedCoins::<Test>::insert(
			coin_id,
			LockInfo { reason: LockReason::FailedDispatch { retries: 0 }, until: 100u64 },
		);
		assert_eq!(
			LockedCoins::<Test>::get(coin_id),
			Some(LockInfo { reason: LockReason::FailedDispatch { retries: 0 }, until: 100u64 })
		);

		let pre = Pre::<Test>::UsingCoin { coin_id, coin };
		let info = Default::default();
		let post_info = Default::default();
		let ok = Ok(());

		let result = AsCoinage::<Test>::post_dispatch_details(pre, &info, &post_info, 0, &ok);
		assert_ok!(result);

		assert_eq!(LockedCoins::<Test>::get(coin_id), None);
		assert_eq!(Coinage::get_coin_lock_until(coin_id), None);
		assert!(!CoinsByOwner::<Test>::contains_key(coin_id));
	});
}

#[test]
fn get_coin_lock_until_returns_none_for_expired_lock() {
	new_test_ext().execute_with(|| {
		let coin_id = 1u64;
		let current_block = frame_system::Pallet::<Test>::block_number();
		LockedCoins::<Test>::insert(
			coin_id,
			LockInfo { reason: LockReason::FailedDispatch { retries: 0 }, until: current_block },
		);

		assert_eq!(Coinage::get_coin_lock_until(coin_id), None);
	});
}

#[test]
fn denomination_to_asset_amount_valid_values() {
	// Mock config: MinimumExponent=-2, MaximumExponent=7
	new_test_ext().execute_with(|| {
		assert_eq!(Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, -2), Ok(250)); // 1000 >> 2
		assert_eq!(Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, -1), Ok(500)); // 1000 >> 1
		assert_eq!(Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, 0), Ok(1000)); // 1000
		assert_eq!(Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, 1), Ok(2000)); // 1000 << 1
		assert_eq!(Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, 2), Ok(4000)); // 1000 << 2
		assert_eq!(Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, 7), Ok(128_000)); // 1000 << 7
	});
}

#[test]
fn denomination_to_asset_amount_all_default_exponents() {
	// With the default mock config (MinimumExponent=-2, MaximumExponent=7),
	// every exponent must convert losslessly.
	new_test_ext().execute_with(|| {
		let min_exp = <Test as crate::Config>::MinimumExponent::get();
		let max_exp = <Test as crate::Config>::MaximumExponent::get();
		let asset_unit = UNDERLYING_ASSET_UNIT;

		for value in min_exp..=max_exp {
			let amount = Coinage::denomination_to_asset_amount(asset_unit, value)
				.unwrap_or_else(|e| panic!("denomination {value} should convert, got {e:?}"));
			assert!(amount > 0, "denomination {value} must produce a non-zero amount");

			// Verify the amount matches the expected power-of-2 scaling of the asset unit.
			let expected = if value < 0 {
				asset_unit >> value.unsigned_abs() as u32
			} else {
				asset_unit << value as u32
			};
			assert_eq!(amount, expected, "denomination {value}: got {amount}, expected {expected}");
		}
	});
}

#[test]
fn denomination_to_asset_amount_rejects_lossy_exponents() {
	// UNDERLYING_ASSET_UNIT=1000: can be divided by 2 exactly 3 times (1000 → 500 → 250 → 125).
	// The 4th division loses precision (125 / 2 = 62.5, truncated to 62).
	new_test_ext().execute_with(|| {
		// Widen the allowed range to -10 so we can test exponents that would
		// normally be rejected by the bounds check. 1000 can be divided by 2
		// exactly 3 times (1000 → 500 → 250 → 125), so only -1, -2, -3 are
		// lossless. Exponents -4 through -10 lose precision.
		MinimumExponent::set(&-10);
		for value in -10..=-3 {
			let result = Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, value);
			if value >= -3 {
				assert_eq!(result, Ok(125), "denomination {value} should be lossless");
			} else {
				assert_eq!(
					result,
					Err(DenominationToAssetAmountError::LossyDenominationConversion),
					"denomination {value} should be lossy"
				);
			}
		}
	});
}

#[test]
fn denomination_to_asset_amount_rejects_overwide_shifts() {
	new_test_ext().execute_with(|| {
		// Allow the full i8 range so we can test shifts that exceed u64's
		// 64-bit width. These would normally be rejected by the bounds check.
		MinimumExponent::set(&i8::MIN);

		// Shifts of 64..=128 bits exceed u64 width → DenominationTooSmall.
		for value in i8::MIN..=-64 {
			assert_eq!(
				Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, value),
				Err(DenominationToAssetAmountError::DenominationTooSmall),
				"denomination {value} should exceed u64 bit width"
			);
		}

		// Shift of 63 bits fits in u64 but is lossy for unit=1000.
		assert_eq!(
			Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, -63),
			Err(DenominationToAssetAmountError::LossyDenominationConversion),
		);
	});
}

#[test]
fn denomination_to_asset_amount_out_of_bounds() {
	new_test_ext().execute_with(|| {
		assert_eq!(
			Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, -3),
			Err(DenominationToAssetAmountError::DenominationOutOfBound)
		);
		assert_eq!(
			Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, 8),
			Err(DenominationToAssetAmountError::DenominationOutOfBound)
		);
		assert_eq!(
			Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, i8::MIN),
			Err(DenominationToAssetAmountError::DenominationOutOfBound)
		);
		assert_eq!(
			Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, i8::MAX),
			Err(DenominationToAssetAmountError::DenominationOutOfBound)
		);
	});
}

#[test]
fn denomination_to_asset_amount_i8_min_does_not_panic() {
	new_test_ext().execute_with(|| {
		// Allow i8::MIN (-128) to pass the bounds check so we can verify
		// that unsigned_abs() handles it without panicking. The old code
		// used -value which overflows on -128.
		MinimumExponent::set(&-128);
		assert_eq!(
			Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, i8::MIN),
			Err(DenominationToAssetAmountError::DenominationTooSmall)
		);
	});
}

/// Verifies that left-shifting `UNDERLYING_ASSET_UNIT` by more bits than fit in u64 returns
/// `DenominationTooBig` instead of silently truncating.
#[test]
fn denomination_to_asset_amount_rejects_left_shift_overflow() {
	new_test_ext().execute_with(|| {
		// With unit=1000, we need ~10 bits. Max safe left-shift is 54 (64 - 10).
		// A shift of 55 causes overflow: 1000 << 55 needs 65 bits, exceeding u64.

		// Set MaximumExponent high enough to trigger overflow
		MaximumExponent::set(&60);

		// First verify the math: unit=1000 has 54 leading zeros
		let asset_unit = UNDERLYING_ASSET_UNIT;
		assert_eq!(asset_unit.leading_zeros(), 54, "unit=1000 should have 54 leading zeros");

		// Shift of 55 should be detected as overflow and return DenominationTooBig.
		assert_eq!(
			Coinage::denomination_to_asset_amount(asset_unit, 55),
			Err(DenominationToAssetAmountError::DenominationTooBig),
			"Left-shift overflow should be rejected"
		);

		// Also test shift of 60 (even more overflow)
		assert_eq!(
			Coinage::denomination_to_asset_amount(asset_unit, 60),
			Err(DenominationToAssetAmountError::DenominationTooBig),
			"Left-shift overflow should be rejected"
		);

		// Shift of 54 should still work (max safe shift)
		MaximumExponent::set(&54);
		assert!(
			Coinage::denomination_to_asset_amount(asset_unit, 54).is_ok(),
			"Max safe shift (54) should succeed"
		);
	});
}

/// Helper to build a pay_for_recycler_unload_fee_token_with_coin extrinsic that will fail
/// dispatch when the coin has no asset backing.
fn build_pay_for_token_ext(signer: u64) -> Extrinsic {
	let secret = get_secret(signer as u8);
	let member = CryptoOf::<Test>::member_from_secret(&secret);
	let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();
	build_signed_as_coin_ext(
		signer,
		crate::Call::pay_for_recycler_unload_fee_token_with_coin {
			member_key: member,
			proof_of_ownership: proof,
		},
		true,
	)
}

/// Helper to build a transfer extrinsic.
fn build_transfer_ext(signer: u64, to: u64) -> Extrinsic {
	build_signed_as_coin_ext(signer, crate::Call::transfer { to }, true)
}

#[test]
fn exponential_lock_duration_on_consecutive_failures_and_cleared_on_success() {
	// Test scenario:
	// 1. A transaction using a coin fails → locked for 2^0 * base_period = 5 seconds
	// 2. Lock expires, another transaction using the same coin fails → locked for 2^1 * base_period
	//    = 10 seconds
	// 3. Lock expires, a transaction succeeds → lock is completely removed
	//
	// We use pay_for_recycler_unload_fee_token_with_coin with a coin that has no asset backing
	// to trigger dispatch failures. The transfer call is used for the successful case.
	new_test_ext().execute_with(|| {
		setup_asset();
		let coin_owner = ALICE;
		let dest = BOB;
		let denomination: Denomination = 0;
		let base_lock_period = COIN_FAILURE_LOCK_PERIOD;

		// Insert coin directly WITHOUT asset backing - this will cause dispatch to fail
		// when trying to release/transfer the underlying asset.
		CoinsByOwner::<Test>::insert(
			coin_owner,
			Coin { instance_id: TEST_INSTANCE_ID, value: denomination, age: 0 },
		);

		// Initially coin exists and no lock.
		assert!(CoinsByOwner::<Test>::contains_key(coin_owner));
		assert!(LockedCoins::<Test>::get(coin_owner).is_none());

		// --- First failure ---
		let current_time = MockTime::now().as_secs();
		let first_lock_duration = base_lock_period; // 2^0 * 5 = 5 seconds
		let first_lock_until = current_time + first_lock_duration;

		let ext = build_pay_for_token_ext(coin_owner);
		let res = Executive::apply_extrinsic(ext);
		// Transaction is valid but dispatch fails (no asset backing).
		assert!(matches!(res, Ok(Err(_))), "Dispatch should fail: {res:?}");

		// Coin is restored and locked with retries=0.
		assert_eq!(
			CoinsByOwner::<Test>::get(coin_owner),
			Some(Coin { instance_id: TEST_INSTANCE_ID, value: denomination, age: 0 })
		);
		assert_eq!(
			LockedCoins::<Test>::get(coin_owner),
			Some(LockInfo {
				reason: LockReason::FailedDispatch { retries: 0 },
				until: first_lock_until
			})
		);
		assert_eq!(Coinage::get_coin_lock_until(coin_owner), Some(first_lock_until));

		// Trying to use the coin while locked should fail validation.
		let ext = build_pay_for_token_ext(coin_owner);
		assert_invalid(ext, CustomInvalidity::CoinTemporarilyLocked);

		// --- Advance past the first lock period ---
		advance_until_time(first_lock_until as u32);

		// Lock should be considered expired now.
		assert_eq!(Coinage::get_coin_lock_until(coin_owner), None);

		// --- Second failure ---
		let current_time = MockTime::now().as_secs();
		let second_lock_duration = 2 * base_lock_period; // 2^1 * 5 = 10 seconds
		let second_lock_until = current_time + second_lock_duration;

		let ext = build_pay_for_token_ext(coin_owner);
		let res = Executive::apply_extrinsic(ext);
		// Transaction is valid but dispatch fails (still no asset backing).
		assert!(matches!(res, Ok(Err(_))), "Dispatch should fail: {res:?}");

		// Coin is restored and locked with retries=1 (exponential increase).
		assert_eq!(
			CoinsByOwner::<Test>::get(coin_owner),
			Some(Coin { instance_id: TEST_INSTANCE_ID, value: denomination, age: 0 })
		);
		assert_eq!(
			LockedCoins::<Test>::get(coin_owner),
			Some(LockInfo {
				reason: LockReason::FailedDispatch { retries: 1 },
				until: second_lock_until
			})
		);
		assert_eq!(Coinage::get_coin_lock_until(coin_owner), Some(second_lock_until));

		// --- Advance past the second lock period ---
		advance_until_time(second_lock_until as u32);

		// Lock should be considered expired now.
		assert_eq!(Coinage::get_coin_lock_until(coin_owner), None);

		// --- Successful transaction ---
		// Transfer doesn't need asset backing, it just moves the coin.
		let ext = build_transfer_ext(coin_owner, dest);
		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.unwrap());

		// Coin is transferred (not at original owner) and lock is completely removed.
		assert!(!CoinsByOwner::<Test>::contains_key(coin_owner));
		assert!(LockedCoins::<Test>::get(coin_owner).is_none());
		assert_eq!(Coinage::get_coin_lock_until(coin_owner), None);
		// Destination received the coin with incremented age.
		assert_eq!(
			CoinsByOwner::<Test>::get(dest),
			Some(Coin { instance_id: TEST_INSTANCE_ID, value: denomination, age: 1 })
		);
	});
}

// Each dual-mode unload call charges the component-wise maximum of its two fee-mode paths up
// front (the fee mode lives in the origin, which the `#[pallet::weight]` annotation cannot see),
// then refunds down to the mode actually run via `PostDispatchInfo`. This checks the charged
// worst case is `max(prepaid, from_output)` and dominates each mode, so it is a true upper bound
// and the two modes genuinely differ (otherwise the refund would be a no-op).
#[test]
fn unload_call_charge_is_max_of_modes() {
	new_test_ext().execute_with(|| {
		// coins: a = 2 aliases, d = 2 destinations.
		let (a, d) = (2usize, 2u32);
		assert_eq!(
			Coinage::unload_recycler_into_coins_max_weight(a, d),
			Coinage::unload_recycler_into_coins_prepaid_weight(a, d)
				.max(Coinage::unload_recycler_into_coins_from_output_weight(a, d)),
		);

		// external asset: n = 2 aliases. The `_max_weight` helper feeds the annotation.
		assert_eq!(
			Coinage::unload_recycler_into_external_asset_max_weight(a),
			Coinage::unload_recycler_into_external_asset_prepaid_weight(a)
				.max(Coinage::unload_recycler_into_external_asset_from_output_weight(a)),
		);

		// loaded_coins: a = 2 aliases, d = 0 loaded_coins.
		assert_eq!(
			Coinage::unload_recycler_into_external_asset_and_loaded_coins_max_weight(a, 0),
			Coinage::unload_recycler_into_external_asset_and_loaded_coins_prepaid_weight(a, 0).max(
				Coinage::unload_recycler_into_external_asset_and_loaded_coins_from_output_weight(
					a, 0
				)
			),
		);
	});
}
