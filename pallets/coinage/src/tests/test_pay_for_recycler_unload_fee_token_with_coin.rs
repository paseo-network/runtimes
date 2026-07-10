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
use codec::Encode;
use frame_support::{
	assert_err, assert_ok,
	traits::{fungibles::InspectHold, UnixTime},
};
use indiv_support::traits::AppendOnlyMembers;
use sp_runtime::{bounded_vec, transaction_validity::TransactionSource, DispatchError};
use verifiable::GenerateVerifiable;

fn build_ext(
	signer: u64,
	member_key: MemberOf<Test>,
	proof_of_ownership: SignatureOf<Test>,
	as_coin: bool,
) -> Extrinsic {
	build_signed_as_coin_ext(
		signer,
		crate::Call::pay_for_recycler_unload_fee_token_with_coin { member_key, proof_of_ownership },
		as_coin,
	)
}

#[test]
fn wrong_origin_fail() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		// as_coin = false -> Signed Origin, but call expects Coin Origin
		let ext = build_ext(signer, member, proof, false);

		// Validation passes (extension is passthrough)
		assert_ok!(Executive::validate_transaction(
			TransactionSource::External,
			ext.clone(),
			Default::default(),
		));

		// Dispatch fails
		assert_err!(Executive::apply_extrinsic(ext).unwrap(), DispatchError::BadOrigin);
	});
}

#[test]
fn coin_max_age_fail() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let signer = 1;
		let max_age = get_u16::<<Test as Config>::MaximumAge>();
		// Insert manually because create_coin sets age to 0
		create_coin(signer, 0, max_age);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		let ext = build_ext(signer, member, proof, true);

		assert_invalid(ext, CustomInvalidity::CoinTooOld);
	});
}

#[test]
fn coin_value_too_low_invalid() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let signer = 1;

		// Set fee > coin value.
		// MinExp = -2 (250 units). Coin Value 0 = 1000 units.
		// Set Fee = 2000 units.
		MockPaidUnloadTokenFeeOverride::set(&Some(2000));

		let coin_value = 0;
		create_coin(signer, coin_value, 0);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		let ext = build_ext(signer, member, proof, true);

		assert_invalid(ext, CustomInvalidity::CoinValueIsLessThanFee);
	});
}

#[test]
fn member_key_already_used_fail() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let coin_value = 0;

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);

		// 1. First user pays successfully
		let user1 = 1;
		create_coin(user1, coin_value, 0);
		let proof1 = CryptoOf::<Test>::sign(&secret, &user1.encode()).unwrap();
		let ext1 = build_ext(user1, member, proof1, true);
		assert_eq!(Executive::apply_extrinsic(ext1), Ok(Ok(())));

		// 2. Second user tries same member key
		let user2 = 2;
		create_coin(user2, coin_value, 0);
		let proof2 = CryptoOf::<Test>::sign(&secret, &user2.encode()).unwrap();
		let ext2 = build_ext(user2, member, proof2, true);

		assert_invalid(ext2, CustomInvalidity::MemberKeyAlreadyUsed);
	});
}

#[test]
fn wrong_proof_of_ownership_fail() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let signer = 1;
		create_coin(signer, 0, 0);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);

		// Sign wrong user ID
		let proof = CryptoOf::<Test>::sign(&secret, &999u64.encode()).unwrap();

		let ext = build_ext(signer, member, proof, true);

		assert_invalid(ext, CustomInvalidity::InvalidProofOfOwnership);
	});
}

#[test]
fn success_accounting_and_usability() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_asset();
		let signer = 1;
		let fee_dest = get_u64::<<Test as Config>::FeeDestination>();
		let asset_id = TEST_ASSET_ID;
		let coin_value = 0; // 1000 units
		let fee = Coinage::paid_unload_token_fee_in_asset().ok().unwrap(); // 2 units

		create_coin(signer, coin_value, 0);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		let ext = build_ext(signer, member, proof, true);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// 1. Fee Transferred
		assert_eq!(Assets::balance(asset_id, fee_dest), fee);

		// 2. Pallet Hold Reduced
		// Fee is released from hold and transferred. Remainder stays held (burnt).
		let pallet_acc = Coinage::pallet_account();
		let on_hold = AssetsWithHolder::total_balance_on_hold(asset_id, &pallet_acc);
		assert_eq!(on_hold, 1000 - fee);

		// 3. Destroyed Value Tracked
		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(), 1000 - fee);

		// 4. Coin removed
		assert!(!CoinsByOwner::<Test>::contains_key(signer));

		// 5. Token Usability (Ring membership)
		assert!(PaidUnloadTokenMembers::<Test>::contains_key(member));

		let now_secs = MockTime::now().as_secs() as u32;
		let period: u32 = now_secs / get_u32::<<Test as Config>::PaidUnloadTokenTimePeriod>();
		let index = 0u32;

		// Build ring to make it usable
		Members::process_maintenance();
		let members = Coinage::get_paid_token_ring_members(period, index);
		assert!(members.contains(&member));

		// Verify Proof Logic
		let mut context = [0u8; 32];
		context[..28].copy_from_slice(PAID_UNLOAD_TOKEN_CONTEXT_BASE.as_ref());
		context[28..32].copy_from_slice(&period.to_le_bytes());
		let msg = b"usage_test";
		let commitment =
			CryptoOf::<Test>::open(paid_token_ring_size(), &member, members.into_iter()).unwrap();
		let (proof, _) =
			CryptoOf::<Test>::create(commitment, &secret, &context, msg.as_ref()).unwrap();

		let id = Coinage::paid_token_collection_identifier(period);
		let revision = <Test as Config>::MemberService::ring_revision(&id, index).unwrap();
		assert_ok!(PaidTknManager::<Test>::validate_token_consumption_proof(
			period,
			index,
			revision,
			&proof,
			msg.as_ref()
		));
		System::assert_has_event(
			crate::Event::<Test>::PaidUnloadTokenRegisteredWithCoin { fee, destroyed: 1000 - fee }
				.into(),
		);

		// 6. Perform Real Unload using the Paid Token
		let (r_secrets, r_idx, r_rev) = setup_recycler(coin_value, 1, 0);
		let dest_coin = 2000u64;
		let alias =
			CryptoOf::<Test>::alias_in_context(&r_secrets[0], UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();

		let unload_call = crate::Call::unload_recycler_into_coin {
			aliases: bounded_vec![alias],
			value: coin_value,
			index: r_idx,
			revision: r_rev,
			to: dest_coin,
		};

		let uxt = build_unload_paid_ext(
			unload_call,
			&secret,
			index,
			revision,
			period,
			&r_secrets,
			coin_value,
			r_idx,
		);
		assert_eq!(Executive::apply_extrinsic(uxt), Ok(Ok(())));

		assert!(CoinsByOwner::<Test>::contains_key(dest_coin));
	});
}

#[test]
fn success_insert_first_key() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let coin_value = 0;
		let signer = 1;
		create_coin(signer, coin_value, 0);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		let ext = build_ext(signer, member, proof, true);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Verify ring state
		let now_secs = MockTime::now().as_secs() as u32;
		let period: u32 = now_secs / get_u32::<<Test as Config>::PaidUnloadTokenTimePeriod>();
		let index = 0u32;

		// Collection should exist
		assert!(PaidTokenCollectionsCreated::<Test>::contains_key(BigEndianPeriod::from(period)));

		// Build ring, then verify
		let id = Coinage::paid_token_collection_identifier(period);
		Members::process_maintenance();
		let status = <Test as Config>::MemberService::ring_status(&id, index).unwrap();
		assert_eq!(status.total, 1);
		let revision = <Test as Config>::MemberService::ring_revision(&id, index).unwrap();
		let (recycler_secrets, r_idx, r_rev) = setup_recycler(coin_value, 1, 0);
		let alias = CryptoOf::<Test>::alias_in_context(
			&recycler_secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let unload_call = crate::Call::unload_recycler_into_coin {
			aliases: bounded_vec![alias],
			value: coin_value,
			index: r_idx,
			revision: r_rev,
			to: 2000,
		};
		let uxt = build_unload_paid_ext(
			unload_call,
			&secret,
			index,
			revision,
			period,
			&recycler_secrets,
			coin_value,
			r_idx,
		);
		assert_eq!(Executive::apply_extrinsic(uxt), Ok(Ok(())));
	});
}

#[test]
fn success_insert_ring_full_creates_new() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let coin_value = 0;
		let ring_size = R2E10_RING_CAPACITY;

		// Fill the ring
		for i in 0..ring_size {
			let user = 1000 + i as u64;
			create_coin(user, coin_value, 0);

			let secret = get_unique_secret();
			let member = CryptoOf::<Test>::member_from_secret(&secret);
			let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

			let ext = build_ext(user, member, proof, true);
			assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));
		}

		let now_secs = MockTime::now().as_secs() as u32;
		let period: u32 = now_secs / get_u32::<<Test as Config>::PaidUnloadTokenTimePeriod>();

		// Ensure all 767 members are processed
		for _ in 0..10 {
			Members::process_maintenance();
		}

		// Check first ring status
		let id = Coinage::paid_token_collection_identifier(period);
		let status0 = <Test as Config>::MemberService::ring_status(&id, 0).unwrap();
		assert_eq!(status0.total, ring_size);

		// Insert one more (use unique secret since 0..254 are already used)
		let user = 2000;
		create_coin(user, coin_value, 0);
		let secret = get_unique_secret();
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

		let ext = build_ext(user, member, proof, true);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Build again to onboard the new member into ring 1
		Members::process_maintenance();

		// Check second ring exists with 1 member
		let status1 = <Test as Config>::MemberService::ring_status(&id, 1).unwrap();
		assert_eq!(status1.total, 1);

		// Get revision for the unload
		let revision = <Test as Config>::MemberService::ring_revision(&id, 1).unwrap();
		let (recycler_secrets, r_idx, r_rev) = setup_recycler(coin_value, 1, 0);
		let alias = CryptoOf::<Test>::alias_in_context(
			&recycler_secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let unload_call = crate::Call::unload_recycler_into_coin {
			aliases: bounded_vec![alias],
			value: coin_value,
			index: r_idx,
			revision: r_rev,
			to: 3000,
		};
		let uxt = build_unload_paid_ext(
			unload_call,
			&secret,
			1,
			revision,
			period,
			&recycler_secrets,
			coin_value,
			r_idx,
		);
		assert_eq!(Executive::apply_extrinsic(uxt), Ok(Ok(())));
	});
}

#[test]
fn success_with_previous_revision() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let coin_value = 0;

		// 1. Pay for token using a coin
		let signer = 1;
		create_coin(signer, coin_value, 0);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		let ext = build_ext(signer, member, proof, true);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		let now_secs = MockTime::now().as_secs() as u32;
		let period = now_secs / get_u32::<<Test as Config>::PaidUnloadTokenTimePeriod>();
		let index: u32 = 0;

		// 2. Build Ring
		let paid_id = Coinage::paid_token_collection_identifier(period);
		indiv_pallet_members::OnboardingSize::<Test>::insert(paid_id, 1u32);
		Members::process_maintenance();

		// Capture the ring members BEFORE adding more members
		let members_v1 = Coinage::get_paid_token_ring_members(period, index);
		let rev_after_first_build = Coinage::get_paid_token_ring_revision(period, index).unwrap();

		// 3. Add another member and build again (previous_root is set)
		let signer2 = 2;
		create_coin(signer2, coin_value, 0);

		let secret2 = get_secret(2);
		let member2 = CryptoOf::<Test>::member_from_secret(&secret2);
		let proof2 = CryptoOf::<Test>::sign(&secret2, &signer2.encode()).unwrap();

		let ext2 = build_ext(signer2, member2, proof2, true);
		assert_eq!(Executive::apply_extrinsic(ext2), Ok(Ok(())));

		Members::process_maintenance();

		// Verify revision incremented
		let rev_after_second_build = Coinage::get_paid_token_ring_revision(period, index).unwrap();
		assert!(rev_after_second_build > rev_after_first_build);

		// 4. Verify proof against OLD ring members with OLD revision works
		let mut context = [0u8; 32];
		context[..28].copy_from_slice(PAID_UNLOAD_TOKEN_CONTEXT_BASE.as_ref());
		context[28..32].copy_from_slice(&period.to_le_bytes());
		let msg = b"usage_test";

		// Generate proof using OLD ring members (v1)
		let commitment =
			CryptoOf::<Test>::open(paid_token_ring_size(), &member, members_v1.into_iter())
				.unwrap();
		let (proof, _) =
			CryptoOf::<Test>::create(commitment, &secret, &context, msg.as_ref()).unwrap();

		// Should succeed with OLD revision because previous_root is valid
		assert_ok!(PaidTknManager::<Test>::validate_token_consumption_proof(
			period,
			index,
			rev_after_first_build,
			&proof,
			msg.as_ref()
		));
	});
}

#[test]
fn failed_dispatch_restores_coin() {
	// Insert a coin without asset backing so that Fungibles::release fails at dispatch.
	// The coin is consumed in prepare, but should be restored in post_dispatch on failure.
	new_test_ext().execute_with(|| {
		setup_asset();
		let signer = 1u64;
		let coin_value: CoinValue = 0; // Exponent value 0 (equals 1000 underlying units)
		let lock_period = get_u64::<<Test as Config>::CoinFailureLockPeriod>();
		let current_block = frame_system::Pallet::<Test>::block_number();
		let expected_lock_until = current_block.saturating_add(lock_period);

		// Insert coin directly without held asset backing.
		CoinsByOwner::<Test>::insert(signer, Coin { value: coin_value, age: 0 });

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(), 0);

		let ext = build_ext(signer, member, proof, true);
		let result = Executive::apply_extrinsic(ext);
		// Dispatch fails (Fungibles::release fails due to no held balance)
		assert!(matches!(result, Ok(Err(_))), "Dispatch should fail: {result:?}");

		// Coin should be restored; no destroyed value should be tracked.
		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(), 0);
		assert_eq!(CoinsByOwner::<Test>::get(signer), Some(Coin { value: coin_value, age: 0 }));
		assert_eq!(
			LockedCoins::<Test>::get(signer),
			Some(LockedCoin {
				reason: LockReason::FailedDispatch { retries: 0 },
				until: expected_lock_until
			})
		);

		// Retry during lock window should be invalid at extension validation.
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();
		let ext_locked = build_ext(signer, member, proof, true);
		assert_invalid(ext_locked, CustomInvalidity::CoinTemporarilyLocked);

		// After lock expires, validation should allow the transaction again.
		for _ in 0..lock_period {
			advance_block();
		}
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();
		let ext_after_lock = build_ext(signer, member, proof, true);
		assert!(
			Executive::validate_transaction(
				TransactionSource::External,
				ext_after_lock,
				Default::default(),
			)
			.is_ok(),
			"transaction should be valid again once coin lock expires"
		);
	});
}

#[test]
fn fee_conversion_failure_after_prepare_restores_and_locks_coin() {
	// Simulates the scenario where `paid_unload_token_fee_in_asset()` succeeds during
	// validation (tx pool) but fails during dispatch (different block). The coin is consumed
	// in `prepare`, then restored and locked in `post_dispatch`.
	//
	// Flow: validate (conversion call 0 → ok) → prepare (coin taken) →
	//       dispatch (conversion call 1 → fail) → post_dispatch (coin restored + locked)
	new_test_ext().execute_with(|| {
		setup_asset();
		let signer = 1u64;
		let coin_value: CoinValue = 0; // Exponent value 0 (equals 1000 underlying units)
		create_coin(signer, coin_value, 0);
		let current_block = frame_system::Pallet::<Test>::block_number();
		let expected_lock_until =
			current_block.saturating_add(get_u64::<<Test as Config>::CoinFailureLockPeriod>());

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(), 0);

		// Make the conversion fail on the 2nd call (index 1 = dispatch).
		// Call 0 (validate) succeeds, call 1 (dispatch) fails.
		set_conversion_to_asset_fail_at(Some(1));

		let ext = build_ext(signer, member, proof, true);
		let result = Executive::apply_extrinsic(ext);

		// Dispatch fails because fee conversion fails.
		assert!(matches!(result, Ok(Err(_))), "Dispatch should fail: {result:?}");

		// Coin was consumed in prepare, then restored and locked in post_dispatch.
		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(), 0);
		assert_eq!(CoinsByOwner::<Test>::get(signer), Some(Coin { value: coin_value, age: 0 }));
		assert_eq!(
			LockedCoins::<Test>::get(signer),
			Some(LockedCoin {
				reason: LockReason::FailedDispatch { retries: 0 },
				until: expected_lock_until
			})
		);

		// Retry should be blocked during the lock period.
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();
		let ext_locked = build_ext(signer, member, proof, true);
		assert_invalid(ext_locked, CustomInvalidity::CoinTemporarilyLocked);

		// Reset
		set_conversion_to_asset_fail_at(None);
	});
}
