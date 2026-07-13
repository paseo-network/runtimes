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

use crate::{extension::*, mock::*, *};
use codec::Encode;
use frame_support::{
	assert_err, assert_noop, assert_ok, traits::fungibles::InspectHold as _, BoundedVec,
};
use frame_system::AuthorizeCall;
use sp_runtime::{testing::UintAuthorityId, DispatchError, ModuleError};
use verifiable::GenerateVerifiable;

fn fund(who: u64, amount: u64) {
	// Mint assets to the user
	assert_ok!(Assets::mint(RuntimeOrigin::signed(1), TEST_ASSET_ID, who, amount));
}

/// Helper to build the load_recycler_with_external_asset extrinsic.
fn build_ext(
	signer: u64,
	preservation: CodecPreservation,
	value: CoinValue,
	member_key: MemberOf<Test>,
	proof_of_ownership: SignatureOf<Test>,
) -> Extrinsic {
	// We use as_coin = false because this call requires a Signed origin, not a Coin origin.
	build_signed_as_coin_ext(
		signer,
		crate::Call::load_recycler_with_external_asset {
			preservation,
			value,
			member_key,
			proof_of_ownership,
		},
		false,
	)
}

#[test]
fn bad_origin_fail() {
	new_test_ext().execute_with(|| {
		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &1u64.encode()).unwrap();

		// A direct call with a None origin should fail the ensure_signed check
		assert_noop!(
			Coinage::load_recycler_with_external_asset(
				RuntimeOrigin::none(),
				CodecPreservation::Expendable,
				0,
				member,
				proof
			),
			DispatchError::BadOrigin
		);
	});
}

#[test]
fn member_key_already_used_fail() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let user1 = 1;
		let user2 = 2;
		fund(user1, 10_000);
		fund(user2, 10_000);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);

		// User 1 uses the key successfully
		let proof1 = CryptoOf::<Test>::sign(&secret, &user1.encode()).unwrap();
		let ext1 = build_ext(user1, CodecPreservation::Expendable, 0, member, proof1);
		assert_eq!(Executive::apply_extrinsic(ext1), Ok(Ok(())));

		// User 2 tries to use the same key
		let proof2 = CryptoOf::<Test>::sign(&secret, &user2.encode()).unwrap();
		let ext2 = build_ext(user2, CodecPreservation::Expendable, 0, member, proof2);

		let res = Executive::apply_extrinsic(ext2);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), Error::<Test>::MemberKeyAlreadyUsed);
	});
}

#[test]
fn proof_of_ownership_invalid_fail() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let user = 1;
		fund(user, 10_000);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);

		// Proof signs a different message (e.g. user 2's ID)
		let proof = CryptoOf::<Test>::sign(&secret, &2u64.encode()).unwrap();

		let ext = build_ext(user, CodecPreservation::Expendable, 0, member, proof);

		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), Error::<Test>::InvalidProofOfOwnership);
	});
}

#[test]
fn value_out_of_bound_fail() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let user = 1;
		fund(user, 10_000);
		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

		let max_exp = <Test as Config>::MaximumExponent::get();
		// Try value > MaximumExponent
		let ext = build_ext(user, CodecPreservation::Expendable, max_exp + 1, member, proof);

		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), Error::<Test>::CoinValueOutOfBound);
	});
}

#[test]
fn signer_not_enough_balance_fail() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let user = 1;
		fund(user, 10);
		// User has 10 balance (not enough fund)

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

		// Try to load value 0 which requires 1000 units
		let ext = build_ext(user, CodecPreservation::Expendable, 0, member, proof);

		// Should fail with a funds unavailable error (from pallet-assets)
		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref());
		assert_err!(
			res.unwrap(),
			DispatchError::Module(ModuleError {
				index: 2,
				error: [0, 0, 0, 0],
				message: Some("BalanceLow")
			})
		);
	});
}

#[test]
fn all_good_success_when_no_recycler() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_asset();
		let user = 1;
		let value = 0;
		fund(user, 2000);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

		let ext = build_ext(user, CodecPreservation::Expendable, value, member, proof);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Check Recycler Collection Created
		assert!(RecyclerCollectionCreated::<Test>::contains_key(value));

		// Check Member mapping
		assert!(RecyclersCoinToRecycler::<Test>::contains_key(member));

		// Check Balance Held
		let balance = Assets::balance(TEST_ASSET_ID, user);
		assert_eq!(balance, 1000); // 2000 - 1000 held

		// Check funds are held in the Pallet Account
		let pallet_acc = Coinage::pallet_account();
		assert_eq!(AssetsWithHolder::total_balance_on_hold(TEST_ASSET_ID, &pallet_acc), 1000);
		System::assert_has_event(
			crate::Event::<Test>::RecyclerLoadedWithExternalAsset {
				who: user,
				value,
				amount: 1000,
			}
			.into(),
		);
	});
}

#[test]
fn all_good_success_when_existing_recycler() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let user1 = 1;
		let user2 = 2;
		let value = 0;
		fund(user1, 1000);
		fund(user2, 1000);

		// Load 1
		let secret1 = get_secret(1);
		let member1 = CryptoOf::<Test>::member_from_secret(&secret1);
		let proof1 = CryptoOf::<Test>::sign(&secret1, &user1.encode()).unwrap();
		let ext1 = build_ext(user1, CodecPreservation::Expendable, value, member1, proof1);
		assert_eq!(Executive::apply_extrinsic(ext1), Ok(Ok(())));

		// Load 2 (into same recycler)
		let secret2 = get_secret(2);
		let member2 = CryptoOf::<Test>::member_from_secret(&secret2);
		let proof2 = CryptoOf::<Test>::sign(&secret2, &user2.encode()).unwrap();
		let ext2 = build_ext(user2, CodecPreservation::Expendable, value, member2, proof2);
		assert_eq!(Executive::apply_extrinsic(ext2), Ok(Ok(())));

		// Check both members are mapped
		assert!(RecyclersCoinToRecycler::<Test>::contains_key(member1));
		assert!(RecyclersCoinToRecycler::<Test>::contains_key(member2));
	});
}

#[test]
fn all_good_success_when_recycler_get_full_a_new_one_is_created() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let value = 0;
		// Ring capacity is 767 (RingExponent::R2e10). Fill it to trigger a new ring.
		let ring_capacity = R2E10_RING_CAPACITY;

		// Fill the recycler to capacity
		for i in 0..ring_capacity {
			let user = 1000 + i as u64;
			fund(user, 1000);
			let secret = get_unique_secret();
			let member = CryptoOf::<Test>::member_from_secret(&secret);
			let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();
			let ext = build_ext(user, CodecPreservation::Expendable, value, member, proof);
			assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));
		}

		// Collection should exist
		assert!(RecyclerCollectionCreated::<Test>::contains_key(value));

		// Load one more -> should go into the next ring
		let user = 2000;
		fund(user, 1000);
		let secret = get_unique_secret();
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();
		let ext = build_ext(user, CodecPreservation::Expendable, value, member, proof);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Verify the new member is mapped
		assert!(RecyclersCoinToRecycler::<Test>::contains_key(member));
	});
}

#[test]
fn batch_grouped_load_success() {
	new_test_ext().execute_with(|| {
		let member_a = CryptoOf::<Test>::member_from_secret(&get_secret(51));
		let member_b = CryptoOf::<Test>::member_from_secret(&get_secret(52));
		let member_c = CryptoOf::<Test>::member_from_secret(&get_secret(53));

		assert_ok!(RecyclerManager::<Test>::load_batch_grouped(&[
			(0, member_a),
			(1, member_b),
			(0, member_c),
		]));

		assert!(RecyclerCollectionCreated::<Test>::contains_key(0));
		assert!(RecyclerCollectionCreated::<Test>::contains_key(1));
		assert_eq!(RecyclersCoinToRecycler::<Test>::get(member_a), Some(0));
		assert_eq!(RecyclersCoinToRecycler::<Test>::get(member_b), Some(1));
		assert_eq!(RecyclersCoinToRecycler::<Test>::get(member_c), Some(0));
	});
}

#[test]
fn batch_grouped_load_duplicate_member_key_fails_without_partial_mutation() {
	new_test_ext().execute_with(|| {
		let member = CryptoOf::<Test>::member_from_secret(&get_secret(61));

		let result = RecyclerManager::<Test>::load_batch_grouped(&[(0, member), (1, member)]);
		assert!(matches!(result, Err(RecyclerLoadError::MemberKeyAlreadyUsed)));

		assert!(!RecyclerCollectionCreated::<Test>::contains_key(0));
		assert!(!RecyclerCollectionCreated::<Test>::contains_key(1));
		assert_eq!(RecyclersCoinToRecycler::<Test>::get(member), None);
	});
}

// ==================== `load_recycler_with_external_asset_unpaid_batch` ====================

/// Build an `UnpaidLoadInput` for `signer` with a fresh member key.
fn make_batch_item(
	signer: u64,
	preservation: CodecPreservation,
	value: CoinValue,
) -> (MemberOf<Test>, UnpaidLoadInput<Test>) {
	let secret = get_unique_secret();
	let member_key = CryptoOf::<Test>::member_from_secret(&secret);
	let proof_of_ownership = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();
	let item = UnpaidLoadInput { preservation, value, member_key, proof_of_ownership };
	(member_key, item)
}

/// Build the `load_recycler_with_external_asset_unpaid_batch` extrinsic via the
/// `InfallibleUnpaidSigned` extension.
fn build_batch_ext(
	signer: u64,
	nonce: u32,
	items: BoundedVec<UnpaidLoadInput<Test>, <Test as Config>::MaxBatchUnpaidLoad>,
) -> Extrinsic {
	let call = crate::Call::load_recycler_with_external_asset_unpaid_batch { items };
	let info = Some(AsCoinageInfo::InfallibleUnpaidSigned { nonce });
	let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(info));
	Extrinsic::new_signed(call.into(), signer, UintAuthorityId(signer), extension)
}

#[test]
fn batch_unpaid_load_success_with_max_items() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_asset();
		Coinage::do_initialize().unwrap();

		let user = 1;
		let value = 0;
		let asset_amount = Coinage::coin_value_to_asset_amount(value).unwrap();
		let n = MAX_BATCH_UNPAID_LOAD;
		// Protect leaves enough behind to keep the asset account alive (and therefore the
		// system account, which carries the nonce).
		let total_cost = asset_amount * n as u64;
		fund(user, total_cost + 1);

		let mut members = Vec::with_capacity(n as usize);
		let mut items: Vec<UnpaidLoadInput<Test>> = Vec::with_capacity(n as usize);
		for _ in 0..n {
			let (member, item) = make_batch_item(user, CodecPreservation::Protect, value);
			members.push(member);
			items.push(item);
		}
		let bounded = BoundedVec::try_from(items)
			.expect("vec of MAX_BATCH_UNPAID_LOAD items fits in the bound");

		let ext = build_batch_ext(user, 0, bounded);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Every member key was registered into a recycler.
		for member in &members {
			assert!(RecyclersCoinToRecycler::<Test>::contains_key(member));
		}

		// Asset balance reduced by the full batch cost; 1 unit (ED) remains.
		let balance_after = Assets::balance(TEST_ASSET_ID, user);
		assert_eq!(balance_after, 1);

		// Pallet account holds the full amount.
		let pallet_acc = Coinage::pallet_account();
		assert_eq!(AssetsWithHolder::total_balance_on_hold(TEST_ASSET_ID, &pallet_acc), total_cost);

		// One nonce consumed for the whole batch.
		assert_eq!(frame_system::Pallet::<Test>::account_nonce(user), 1);

		// One event per inner item.
		let event_count = frame_system::Pallet::<Test>::events()
			.into_iter()
			.filter(|e| {
				matches!(
					e.event,
					RuntimeEvent::Coinage(crate::Event::RecyclerLoadedWithExternalAsset { .. },)
				)
			})
			.count();
		assert_eq!(event_count, n as usize);
	});
}

#[test]
fn batch_unpaid_load_duplicate_member_key_rejected_by_extension() {
	new_test_ext().execute_with(|| {
		setup_asset();
		Coinage::do_initialize().unwrap();
		let user = 1;
		fund(user, 100_000);

		// Two items in the batch share the same member key — extension must reject the
		// transaction with `MemberKeyAlreadyUsed` before any state change.
		let secret = get_secret(50);
		let member_key = CryptoOf::<Test>::member_from_secret(&secret);
		let proof_of_ownership = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();
		let item = UnpaidLoadInput {
			preservation: CodecPreservation::Expendable,
			value: 0,
			member_key,
			proof_of_ownership,
		};

		let bounded =
			BoundedVec::try_from(vec![item.clone(), item]).expect("two items fit in the bound");

		let ext = build_batch_ext(user, 0, bounded);
		assert_invalid(ext, CustomInvalidity::MemberKeyAlreadyUsed);

		// State unchanged: no recycler mapping created.
		assert!(!RecyclersCoinToRecycler::<Test>::contains_key(member_key));
	});
}
