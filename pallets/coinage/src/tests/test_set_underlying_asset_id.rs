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

use crate::{mock::*, *};
use codec::Encode;
use frame_support::{assert_noop, assert_ok, traits::Hooks};
use sp_runtime::DispatchError;
use verifiable::GenerateVerifiable;

/// Create [`TEST_ASSET_ID`] in `pallet-assets` without populating the pallet's
/// `UnderlyingAssetId` storage, so the setter call itself is what writes it.
fn create_test_asset_in_fungibles() {
	assert_ok!(Assets::force_create(RuntimeOrigin::root(), TEST_ASSET_ID, ALICE, true, 1,));
}

#[test]
fn setter_requires_manager_origin() {
	new_test_ext_no_asset_id().execute_with(|| {
		create_test_asset_in_fungibles();
		assert_noop!(
			Coinage::set_underlying_asset_id(RuntimeOrigin::signed(ALICE), TEST_ASSET_ID),
			DispatchError::BadOrigin,
		);
		assert_noop!(
			Coinage::set_underlying_asset_id(RuntimeOrigin::none(), TEST_ASSET_ID),
			DispatchError::BadOrigin,
		);
		assert!(!UnderlyingAssetId::<Test>::exists());
	});
}

#[test]
fn setter_emits_event_and_stores_value() {
	new_test_ext_no_asset_id().execute_with(|| {
		// Events are only deposited in non-genesis blocks.
		frame_system::Pallet::<Test>::set_block_number(1);
		create_test_asset_in_fungibles();

		// Initial state: storage is empty.
		assert!(!UnderlyingAssetId::<Test>::exists());

		assert_ok!(Coinage::set_underlying_asset_id(RuntimeOrigin::root(), TEST_ASSET_ID));
		assert_eq!(UnderlyingAssetId::<Test>::get(), Some(TEST_ASSET_ID));
		System::assert_has_event(
			crate::Event::<Test>::UnderlyingAssetIdSet { asset_id: TEST_ASSET_ID }.into(),
		);
	});
}

#[test]
fn setter_rejects_second_call() {
	new_test_ext_no_asset_id().execute_with(|| {
		create_test_asset_in_fungibles();
		assert_ok!(Coinage::set_underlying_asset_id(RuntimeOrigin::root(), TEST_ASSET_ID));

		// Different value, but second call must still be rejected.
		// (Even if the new asset id were valid in pallet-assets, the single-set guard takes
		// precedence.)
		assert_noop!(
			Coinage::set_underlying_asset_id(RuntimeOrigin::root(), TEST_ASSET_ID + 1),
			Error::<Test>::AssetIdAlreadySet,
		);
		assert_eq!(UnderlyingAssetId::<Test>::get(), Some(TEST_ASSET_ID));
	});
}

#[test]
fn setter_rejects_unknown_asset() {
	new_test_ext_no_asset_id().execute_with(|| {
		// Asset id has not been created in `pallet-assets`.
		assert!(!pallet_assets::Asset::<Test>::contains_key(TEST_ASSET_ID));

		assert_noop!(
			Coinage::set_underlying_asset_id(RuntimeOrigin::root(), TEST_ASSET_ID),
			Error::<Test>::UnknownAsset,
		);
		assert!(!UnderlyingAssetId::<Test>::exists());
	});
}

#[test]
fn coin_op_asset_id_not_set_rejected_in_dispatch_handler() {
	new_test_ext_no_asset_id().execute_with(|| {
		// Don't call `setup_asset()`; storage stays empty.
		assert!(!UnderlyingAssetId::<Test>::exists());

		let user = ALICE;
		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

		assert_noop!(
			Coinage::load_recycler_with_external_asset(
				RuntimeOrigin::signed(user),
				crate::pallet::CodecPreservation::Expendable,
				0,
				member,
				proof,
			),
			Error::<Test>::AssetIdNotSet,
		);
	});
}

#[test]
fn load_recycler_round_trip_through_setter() {
	new_test_ext_no_asset_id().execute_with(|| {
		let user = ALICE;
		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();
		let amount = Coinage::coin_value_to_asset_amount(0).expect("coin value 0 must convert");

		// Step 1: storage is unset → the extrinsic refuses to run.
		assert!(!UnderlyingAssetId::<Test>::exists());
		assert_noop!(
			Coinage::load_recycler_with_external_asset(
				RuntimeOrigin::signed(user),
				crate::pallet::CodecPreservation::Expendable,
				0,
				member,
				proof,
			),
			Error::<Test>::AssetIdNotSet,
		);

		// Step 2: governance dispatches the setter (the actual extrinsic, not a storage
		// poke), and we prepare the asset + balance the extrinsic needs.
		assert_ok!(Assets::force_create(RuntimeOrigin::root(), TEST_ASSET_ID, ALICE, true, 1,));
		assert_ok!(Coinage::set_underlying_asset_id(RuntimeOrigin::root(), TEST_ASSET_ID));
		assert_eq!(UnderlyingAssetId::<Test>::get(), Some(TEST_ASSET_ID));
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), TEST_ASSET_ID, user, amount));

		// Step 3: retry the same extrinsic — it now succeeds.
		assert_ok!(Coinage::load_recycler_with_external_asset(
			RuntimeOrigin::signed(user),
			crate::pallet::CodecPreservation::Expendable,
			0,
			member,
			proof,
		));
	});
}

#[test]
fn on_poll_skips_init_until_asset_id_set() {
	use crate::WeightInfo;
	new_test_ext_no_asset_id().execute_with(|| {
		let init_weight = <Test as crate::Config>::WeightInfo::on_poll_initialize();
		let check_weight =
			<Test as crate::Config>::WeightInfo::on_poll_initialize_check_condition();
		let paid_weight =
			<Test as crate::Config>::WeightInfo::on_poll_create_paid_token_collection();

		assert!(!UnderlyingAssetId::<Test>::exists());
		assert!(!InitializePalletAccount::<Test>::exists());

		// Block 1, asset id unset: gate skips the init branch entirely.
		let mut meter = frame_support::weights::WeightMeter::new();
		<crate::Pallet<Test> as Hooks<u64>>::on_poll(1u64, &mut meter);
		assert!(
			!InitializePalletAccount::<Test>::exists(),
			"initialization must remain pending while asset id is unset",
		);
		assert_eq!(
			meter.consumed(),
			paid_weight.saturating_add(check_weight),
			"on_poll must skip init_weight while asset id is unset",
		);
		assert!(meter.consumed().any_lt(init_weight), "init_weight must not be charged");

		// Set the asset id. Next poll runs the full initialization.
		setup_asset();
		assert!(UnderlyingAssetId::<Test>::exists());

		let mut meter = frame_support::weights::WeightMeter::new();
		<crate::Pallet<Test> as Hooks<u64>>::on_poll(2u64, &mut meter);
		assert!(
			InitializePalletAccount::<Test>::exists(),
			"initialization must complete once asset id is set",
		);

		// Subsequent polls: init flag set, init branch skipped.
		let mut meter = frame_support::weights::WeightMeter::new();
		<crate::Pallet<Test> as Hooks<u64>>::on_poll(3u64, &mut meter);
		assert_eq!(
			meter.consumed(),
			paid_weight.saturating_add(check_weight),
			"on_poll must skip init_weight after initialization completes",
		);
	});
}
