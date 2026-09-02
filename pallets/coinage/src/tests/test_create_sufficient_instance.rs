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

//! Tests for the `create_sufficient_instance` extrinsic.
//!
//! Every test starts from [`new_test_ext_no_instance`], because [`new_test_ext`] already creates
//! the instance wrapping [`TEST_ASSET_ID`].

use crate::{mock::*, *};
use codec::Encode;
use frame_support::{assert_noop, assert_ok, traits::fungibles::Inspect};
use indiv_support::traits::AppendOnlyMembers;
use sp_runtime::DispatchError;
use verifiable::GenerateVerifiable;

/// A second asset, used by the test that creates two instances.
const TEST_ASSET_ID_2: u32 = TEST_ASSET_ID + 1;

/// Give the pallet account the minimum-balance buffer `create_sufficient_instance` requires
/// someone to have provided beforehand.
fn fund_pallet_account_buffer(asset_id: u32) {
	assert_ok!(Assets::mint(
		RuntimeOrigin::signed(ALICE),
		asset_id,
		Coinage::pallet_account(),
		Assets::minimum_balance(asset_id)
	));
}

#[test]
fn create_sufficient_instance_bad_origin_fail() {
	new_test_ext_no_instance().execute_with(|| {
		create_asset(TEST_ASSET_ID);

		assert_noop!(
			Coinage::create_sufficient_instance(
				RuntimeOrigin::signed(ALICE),
				TEST_ASSET_ID,
				UNDERLYING_ASSET_UNIT
			),
			DispatchError::BadOrigin
		);
		assert_noop!(
			Coinage::create_sufficient_instance(
				RuntimeOrigin::none(),
				TEST_ASSET_ID,
				UNDERLYING_ASSET_UNIT
			),
			DispatchError::BadOrigin
		);

		assert!(!Instances::<Test>::contains_key(TEST_INSTANCE_ID));
	});
}

#[test]
fn create_sufficient_instance_stores_record_and_emits_event() {
	new_test_ext_no_instance().execute_with(|| {
		System::set_block_number(1);
		create_asset(TEST_ASSET_ID);
		fund_pallet_account_buffer(TEST_ASSET_ID);

		assert!(!Instances::<Test>::contains_key(TEST_INSTANCE_ID));

		assert_ok!(Coinage::create_sufficient_instance(
			RuntimeOrigin::root(),
			TEST_ASSET_ID,
			UNDERLYING_ASSET_UNIT
		));

		let record = Instances::<Test>::get(TEST_INSTANCE_ID).expect("instance was created");
		assert_eq!(record.asset_id, TEST_ASSET_ID);
		assert_eq!(record.asset_unit, UNDERLYING_ASSET_UNIT);
		assert_eq!(record.mode, InstanceMode::Sufficient);
		assert!(record.creator.is_none());

		assert_eq!(Coinage::get_instance_ids(TEST_ASSET_ID), vec![TEST_INSTANCE_ID]);
		assert_eq!(NextInstanceId::<Test>::get(), TEST_INSTANCE_ID + 1);

		System::assert_has_event(
			crate::Event::<Test>::InstanceCreated {
				instance_id: TEST_INSTANCE_ID,
				asset_id: TEST_ASSET_ID,
				asset_unit: UNDERLYING_ASSET_UNIT,
				mode: InstanceMode::Sufficient,
			}
			.into(),
		);
	});
}

#[test]
fn create_sufficient_instance_requires_touched_pallet_account_for_non_sufficient_asset() {
	new_test_ext_no_instance().execute_with(|| {
		// A non-sufficient asset: the pallet account cannot receive it until it is touched.
		assert_ok!(Assets::force_create(RuntimeOrigin::root(), TEST_ASSET_ID, ALICE, false, 1));

		assert_noop!(
			Coinage::create_sufficient_instance(
				RuntimeOrigin::root(),
				TEST_ASSET_ID,
				UNDERLYING_ASSET_UNIT
			),
			Error::<Test>::PalletAccountNotTouched
		);

		fund_native(ALICE, 1_000);
		assert_ok!(Assets::touch_other(
			RuntimeOrigin::signed(ALICE),
			TEST_ASSET_ID,
			Coinage::pallet_account()
		));

		// Touched but still empty: the account is one free-balance drain away from dusting the
		// coins' backing, so creation is refused until the buffer is provided.
		assert_noop!(
			Coinage::create_sufficient_instance(
				RuntimeOrigin::root(),
				TEST_ASSET_ID,
				UNDERLYING_ASSET_UNIT
			),
			Error::<Test>::PalletAccountBelowMinimumBalance
		);

		fund_pallet_account_buffer(TEST_ASSET_ID);
		assert_ok!(Coinage::create_sufficient_instance(
			RuntimeOrigin::root(),
			TEST_ASSET_ID,
			UNDERLYING_ASSET_UNIT
		));
	});
}

#[test]
fn create_sufficient_instance_initializes_collections_and_mints_nothing() {
	new_test_ext_no_instance().execute_with(|| {
		create_asset(TEST_ASSET_ID);
		fund_pallet_account_buffer(TEST_ASSET_ID);

		let pallet_account = Coinage::pallet_account();
		let buffer = Assets::minimum_balance(TEST_ASSET_ID);
		assert_eq!(
			<AssetsWithHolder as Inspect<_>>::balance(TEST_ASSET_ID, &pallet_account),
			buffer
		);

		assert_ok!(Coinage::create_sufficient_instance(
			RuntimeOrigin::root(),
			TEST_ASSET_ID,
			UNDERLYING_ASSET_UNIT
		));

		// The call created one recycler collection per denomination in the configured range.
		let min_exp = <Test as Config>::MinimumExponent::get();
		let max_exp = <Test as Config>::MaximumExponent::get();
		for value in min_exp..=max_exp {
			let identifier = Coinage::recycler_collection_identifier(TEST_INSTANCE_ID, value);
			assert!(
				<Test as Config>::MemberService::ring_status(&identifier, 0).is_some(),
				"denomination {value} must have a recycler collection"
			);
			assert!(
				RecyclerCollectionCreated::<Test>::contains_key(TEST_INSTANCE_ID, value),
				"denomination {value} must have its recycler collection tracked"
			);
		}

		// The call minted nothing: the pallet account still holds only the buffer governance
		// provided beforehand.
		assert_eq!(
			<AssetsWithHolder as Inspect<_>>::balance(TEST_ASSET_ID, &pallet_account),
			buffer
		);
	});
}

#[test]
fn create_sufficient_instance_wraps_one_asset_any_number_of_times() {
	new_test_ext_no_instance().execute_with(|| {
		create_asset(TEST_ASSET_ID);
		fund_pallet_account_buffer(TEST_ASSET_ID);

		// The same asset at another unit, and even at the same one: an instance is never
		// exclusive over its asset, and its creator pays a permanent deposit for it.
		for asset_unit in [UNDERLYING_ASSET_UNIT, UNDERLYING_ASSET_UNIT * 2, UNDERLYING_ASSET_UNIT]
		{
			assert_ok!(Coinage::create_sufficient_instance(
				RuntimeOrigin::root(),
				TEST_ASSET_ID,
				asset_unit
			));
		}

		let mut instances = Coinage::get_instance_ids(TEST_ASSET_ID);
		instances.sort();
		assert_eq!(instances, vec![TEST_INSTANCE_ID, TEST_INSTANCE_ID + 1, TEST_INSTANCE_ID + 2]);
		let unit_of = |instance_id| {
			Instances::<Test>::get(instance_id).expect("instance was created").asset_unit
		};
		assert_eq!(unit_of(TEST_INSTANCE_ID), UNDERLYING_ASSET_UNIT);
		assert_eq!(unit_of(TEST_INSTANCE_ID + 1), UNDERLYING_ASSET_UNIT * 2);
		assert_eq!(unit_of(TEST_INSTANCE_ID + 2), UNDERLYING_ASSET_UNIT);
	});
}

#[test]
fn create_sufficient_instance_unknown_asset_fail() {
	new_test_ext_no_instance().execute_with(|| {
		assert!(!<AssetsWithHolder as Inspect<_>>::asset_exists(TEST_ASSET_ID));

		assert_noop!(
			Coinage::create_sufficient_instance(
				RuntimeOrigin::root(),
				TEST_ASSET_ID,
				UNDERLYING_ASSET_UNIT
			),
			Error::<Test>::UnknownAsset
		);

		assert!(!Instances::<Test>::contains_key(TEST_INSTANCE_ID));
	});
}

#[test]
fn create_sufficient_instance_allocates_sequential_ids_for_distinct_assets() {
	new_test_ext_no_instance().execute_with(|| {
		create_asset(TEST_ASSET_ID);
		create_asset(TEST_ASSET_ID_2);
		fund_pallet_account_buffer(TEST_ASSET_ID);
		fund_pallet_account_buffer(TEST_ASSET_ID_2);

		assert_ok!(Coinage::create_sufficient_instance(
			RuntimeOrigin::root(),
			TEST_ASSET_ID,
			UNDERLYING_ASSET_UNIT
		));
		assert_ok!(Coinage::create_sufficient_instance(
			RuntimeOrigin::root(),
			TEST_ASSET_ID_2,
			UNDERLYING_ASSET_UNIT * 2
		));

		assert_eq!(Coinage::get_instance_ids(TEST_ASSET_ID), vec![TEST_INSTANCE_ID]);
		assert_eq!(Coinage::get_instance_ids(TEST_ASSET_ID_2), vec![TEST_INSTANCE_ID + 1]);
		assert_eq!(NextInstanceId::<Test>::get(), TEST_INSTANCE_ID + 2);

		// Each instance keeps its own asset id and asset unit.
		let first = Instances::<Test>::get(TEST_INSTANCE_ID).expect("first instance");
		let second = Instances::<Test>::get(TEST_INSTANCE_ID + 1).expect("second instance");
		assert_eq!(first.asset_id, TEST_ASSET_ID);
		assert_eq!(first.asset_unit, UNDERLYING_ASSET_UNIT);
		assert_eq!(second.asset_id, TEST_ASSET_ID_2);
		assert_eq!(second.asset_unit, UNDERLYING_ASSET_UNIT * 2);

		// Recycler collections are per instance, so both have one for the same denomination.
		assert!(RecyclerCollectionCreated::<Test>::contains_key(TEST_INSTANCE_ID, 0));
		assert!(RecyclerCollectionCreated::<Test>::contains_key(TEST_INSTANCE_ID + 1, 0));

		// Buffers are per asset: neither creation minted, burned or moved the other asset's.
		let pallet_account = Coinage::pallet_account();
		for asset_id in [TEST_ASSET_ID, TEST_ASSET_ID_2] {
			assert_eq!(
				<AssetsWithHolder as Inspect<_>>::balance(asset_id, &pallet_account),
				<AssetsWithHolder as Inspect<_>>::minimum_balance(asset_id)
			);
		}
	});
}

#[test]
fn create_sufficient_instance_invalid_asset_unit_fail() {
	new_test_ext_no_instance().execute_with(|| {
		create_asset(TEST_ASSET_ID);

		// A zero asset unit would make every coin worth nothing.
		assert_noop!(
			Coinage::create_sufficient_instance(RuntimeOrigin::root(), TEST_ASSET_ID, 0),
			Error::<Test>::InvalidAssetUnit
		);

		// `MinimumExponent` is -2 in the mock, so an asset unit must be divisible by 4;
		// 2 truncates to 0.
		assert_noop!(
			Coinage::create_sufficient_instance(RuntimeOrigin::root(), TEST_ASSET_ID, 2),
			Error::<Test>::InvalidAssetUnit
		);

		// Divisible by 4, but shifting up to `MaximumExponent` overflows the balance type.
		assert_noop!(
			Coinage::create_sufficient_instance(RuntimeOrigin::root(), TEST_ASSET_ID, 1u64 << 57),
			Error::<Test>::InvalidAssetUnit
		);

		assert!(!Instances::<Test>::contains_key(TEST_INSTANCE_ID));
	});
}

#[test]
fn create_sufficient_instance_next_id_overflow_fail() {
	new_test_ext_no_instance().execute_with(|| {
		create_asset(TEST_ASSET_ID);

		NextInstanceId::<Test>::put(InstanceId::MAX);

		assert_noop!(
			Coinage::create_sufficient_instance(
				RuntimeOrigin::root(),
				TEST_ASSET_ID,
				UNDERLYING_ASSET_UNIT
			),
			Error::<Test>::InternalError
		);
	});
}

#[test]
fn create_sufficient_instance_enables_load_recycler() {
	new_test_ext_no_instance().execute_with(|| {
		let user = BOB;
		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();
		let amount = Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, 0)
			.expect("denomination 0 must convert");

		// No instance exists, so the load fails.
		assert!(!Instances::<Test>::contains_key(TEST_INSTANCE_ID));
		assert_noop!(
			Coinage::load_recycler_with_external_asset(
				RuntimeOrigin::signed(user),
				TEST_INSTANCE_ID,
				CodecPreservation::Expendable,
				0,
				member,
				proof
			),
			Error::<Test>::InstanceNotFound
		);

		// Create the asset and its instance, then fund the caller with the amount the load needs.
		create_asset(TEST_ASSET_ID);
		fund_pallet_account_buffer(TEST_ASSET_ID);
		assert_ok!(Coinage::create_sufficient_instance(
			RuntimeOrigin::root(),
			TEST_ASSET_ID,
			UNDERLYING_ASSET_UNIT
		));
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), TEST_ASSET_ID, user, amount));

		// The same call now succeeds.
		assert_ok!(Coinage::load_recycler_with_external_asset(
			RuntimeOrigin::signed(user),
			TEST_INSTANCE_ID,
			CodecPreservation::Expendable,
			0,
			member,
			proof
		));
	});
}
