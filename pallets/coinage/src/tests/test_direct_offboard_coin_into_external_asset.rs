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
	mock::{
		assert_invalid, build_signed_as_coin_ext, create_coin, get_i8, new_test_ext,
		new_test_ext_no_instance, setup_asset, AssetsWithHolder, Coinage, Executive, Extrinsic,
		System, Test, MAXIMUM_AGE, TEST_ASSET_ID, TEST_INSTANCE_ID, UNDERLYING_ASSET_UNIT,
	},
	pallet::CustomInvalidity,
	Coin, CoinsByOwner, Config, HoldReason, LockReason, LockedCoins,
};
use frame_support::{
	assert_err, assert_ok,
	traits::fungibles::{Inspect, InspectHold},
};
use sp_runtime::{transaction_validity::TransactionSource, DispatchError};

#[derive(Clone, Copy)]
pub struct Signer(pub u64);

#[derive(Clone, Copy)]
pub struct Dest(pub u64);

#[derive(Clone, Copy)]
pub struct AsCoin(pub bool);

/// Helper to build a direct offboard extrinsic.
pub fn build_direct_offboard_ext(signer: Signer, to: Dest, as_coin: AsCoin) -> Extrinsic {
	build_signed_as_coin_ext(
		signer.0,
		crate::Call::direct_offboard_coin_into_external_asset { to: to.0 },
		as_coin.0,
	)
}

#[test]
fn direct_offboard_bad_origin_fail() {
	new_test_ext().execute_with(|| {
		let ext = build_direct_offboard_ext(Signer(1), Dest(2), AsCoin(false));

		assert_ok!(Executive::validate_transaction(
			TransactionSource::External,
			ext.clone(),
			Default::default(),
		));

		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), DispatchError::BadOrigin);
	});
}

#[test]
fn direct_offboard_coin_not_exist_invalid() {
	new_test_ext().execute_with(|| {
		let ext = build_direct_offboard_ext(Signer(1), Dest(2), AsCoin(true));
		assert_invalid(ext, CustomInvalidity::NoCoin);
	});
}

#[test]
fn direct_offboard_denomination_out_of_bounds_invalid() {
	new_test_ext().execute_with(|| {
		let signer = Signer(1);
		let dest = Dest(2);
		let invalid_value = get_i8::<<Test as Config>::MaximumExponent>() + 1;
		CoinsByOwner::<Test>::insert(
			signer.0,
			Coin { instance_id: TEST_INSTANCE_ID, value: invalid_value, age: 0 },
		);

		let ext = build_direct_offboard_ext(signer, dest, AsCoin(true));
		assert_invalid(ext, CustomInvalidity::DenominationOutOfBound);
	});
}

#[test]
fn direct_offboard_invalid_keeps_coin_untouched() {
	new_test_ext().execute_with(|| {
		let signer = Signer(1);
		let dest = Dest(2);
		let invalid_value = get_i8::<<Test as Config>::MaximumExponent>() + 1;
		let coin = Coin { instance_id: TEST_INSTANCE_ID, value: invalid_value, age: 0 };
		CoinsByOwner::<Test>::insert(signer.0, coin);

		let ext = build_direct_offboard_ext(signer, dest, AsCoin(true));
		assert_invalid(ext, CustomInvalidity::DenominationOutOfBound);

		assert_eq!(CoinsByOwner::<Test>::get(signer.0), Some(coin));
	});
}

#[test]
fn direct_offboard_instance_not_found_rejected_in_validation() {
	new_test_ext_no_instance().execute_with(|| {
		assert!(!crate::Instances::<Test>::contains_key(TEST_INSTANCE_ID));
		let signer = Signer(1);
		let dest = Dest(2);
		let coin = Coin { instance_id: TEST_INSTANCE_ID, value: 0, age: 0 };
		CoinsByOwner::<Test>::insert(signer.0, coin);

		let ext = build_direct_offboard_ext(signer, dest, AsCoin(true));
		assert_invalid(ext, CustomInvalidity::InstanceNotFound);

		assert_eq!(CoinsByOwner::<Test>::get(signer.0), Some(coin));
		assert!(crate::LockedCoins::<Test>::get(signer.0).is_none());
	});
}

fn assert_direct_offboard_success(age: u16) {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let signer = Signer(1);
		let dest = Dest(2);
		let value = 0;
		let amount = Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, value).unwrap();
		let asset_id = TEST_ASSET_ID;
		let pallet_account = Coinage::pallet_account();

		create_coin(signer.0, value, age);

		let pallet_hold_before = AssetsWithHolder::balance_on_hold(
			asset_id,
			&HoldReason::Wrapped.into(),
			&pallet_account,
		);
		let dest_balance_before = <AssetsWithHolder as Inspect<_>>::balance(asset_id, &dest.0);

		let ext = build_direct_offboard_ext(signer, dest, AsCoin(true));
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		assert!(!CoinsByOwner::<Test>::contains_key(signer.0));

		let pallet_hold_after = AssetsWithHolder::balance_on_hold(
			asset_id,
			&HoldReason::Wrapped.into(),
			&pallet_account,
		);
		let dest_balance_after = <AssetsWithHolder as Inspect<_>>::balance(asset_id, &dest.0);

		assert_eq!(pallet_hold_after, pallet_hold_before - amount);
		assert_eq!(dest_balance_after, dest_balance_before + amount);
		System::assert_has_event(
			crate::Event::<Test>::CoinOffboardedIntoExternalAsset {
				instance_id: TEST_INSTANCE_ID,
				to: dest.0,
				value,
				amount,
			}
			.into(),
		);
	});
}

#[test]
fn direct_offboard_fresh_coin_success() {
	assert_direct_offboard_success(0);
}

#[test]
fn direct_offboard_aged_coin_success() {
	assert_direct_offboard_success(1);
}

#[test]
fn direct_offboard_maximum_age_coin_success() {
	assert_direct_offboard_success(MAXIMUM_AGE);
}

#[test]
fn direct_offboard_cannot_consume_another_owners_coin() {
	new_test_ext().execute_with(|| {
		let victim = Signer(1);
		let attacker = Signer(2);
		let dest = Dest(3);
		let victim_coin = Coin { instance_id: TEST_INSTANCE_ID, value: 0, age: 1 };

		create_coin(victim.0, victim_coin.value, victim_coin.age);

		assert_invalid(
			build_direct_offboard_ext(attacker, dest, AsCoin(true)),
			CustomInvalidity::NoCoin,
		);

		assert_eq!(CoinsByOwner::<Test>::get(victim.0), Some(victim_coin));
		assert!(LockedCoins::<Test>::get(attacker.0).is_none());
		assert_eq!(<AssetsWithHolder as Inspect<_>>::balance(TEST_ASSET_ID, &dest.0), 0);
	});
}

#[test]
fn direct_offboard_failed_dispatch_restores_and_locks_aged_coin() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_asset();

		let signer = Signer(1);
		let dest = Dest(2);
		let coin = Coin { instance_id: TEST_INSTANCE_ID, value: 0, age: 1 };
		let pallet_account = Coinage::pallet_account();

		// Deliberately create an inconsistent coin: it has no held asset backing.
		// Validation passes, `prepare` consumes the coin, then dispatch must fail.
		CoinsByOwner::<Test>::insert(signer.0, coin);

		let dest_balance_before = <AssetsWithHolder as Inspect<_>>::balance(TEST_ASSET_ID, &dest.0);
		let pallet_hold_before = AssetsWithHolder::balance_on_hold(
			TEST_ASSET_ID,
			&HoldReason::Wrapped.into(),
			&pallet_account,
		);

		let result =
			Executive::apply_extrinsic(build_direct_offboard_ext(signer, dest, AsCoin(true)));

		assert!(matches!(result, Ok(Err(_))), "dispatch should fail: {result:?}");
		assert_eq!(CoinsByOwner::<Test>::get(signer.0), Some(coin));
		assert!(matches!(
			LockedCoins::<Test>::get(signer.0),
			Some(lock) if matches!(lock.reason, LockReason::FailedDispatch { retries: 0 })
		));
		assert_eq!(
			<AssetsWithHolder as Inspect<_>>::balance(TEST_ASSET_ID, &dest.0),
			dest_balance_before
		);
		assert_eq!(
			AssetsWithHolder::balance_on_hold(
				TEST_ASSET_ID,
				&HoldReason::Wrapped.into(),
				&pallet_account,
			),
			pallet_hold_before
		);
	});
}
