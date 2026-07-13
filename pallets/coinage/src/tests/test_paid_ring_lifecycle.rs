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
use frame_support::{assert_ok, traits::UnixTime, weights::WeightMeter};
use indiv_support::traits::AppendOnlyMembers;
use sp_runtime::{
	bounded_vec,
	transaction_validity::{InvalidTransaction, TransactionSource, TransactionValidityError},
};
use verifiable::GenerateVerifiable;

// Full lifecycle test for Paid Unload Tokens using pallet-members.
//
// Steps:
// 1. Users (Alice, Bob, Charlie) pay for unload tokens using different methods.
// 2. Trigger Members::process_maintenance() to build Ring 0, verify via ring_status.
// 3. Alice uses her token from Ring 0 to unload a recycler.
// 4. More users join (10 additional).
// 5. Trigger Members::process_maintenance() to rebuild Ring 0 (now 13 members).
// 6. New user joins, rebuild Ring 0 again (now 14 members, still fits).
// 7. New user uses their token, Bob uses his token.
// 8. Advance to next period, then to expiration.
// 9. Verify clean fails before expiration.
// 10. Execute clean after expiration — verify PaidTokenCollectionsCreated removed.
#[test]
fn paid_ring_lifecycle() {
	new_test_ext().execute_with(|| {
		setup_asset();
		check_accounting();

		let alice = 1u64;
		let bob = 2u64;
		let charlie = 3u64;
		let asset_id = TEST_ASSET_ID;

		// Configuration
		let coin_val = 0;
		let fund_amount = 10_000;

		// Fund Users
		assert_ok!(Assets::mint(RuntimeOrigin::signed(alice), asset_id, alice, fund_amount));
		assert_ok!(Assets::mint(RuntimeOrigin::signed(alice), asset_id, bob, fund_amount));
		// Charlie (native/coin user) funded below

		// ====================
		// 1. Insert into paid ring (All 3 methods)
		// ====================

		// Alice: Native
		fund_native(alice, fund_amount);
		let secret_alice = get_unique_secret();
		let member_alice = CryptoOf::<Test>::member_from_secret(&secret_alice);
		let proof_alice = CryptoOf::<Test>::sign(&secret_alice, &alice.encode()).unwrap();

		let call_alice = crate::Call::pay_for_recycler_unload_fee_token_with_native {
			member_key: member_alice,
			proof_of_ownership: proof_alice,
		};
		let ext_alice = build_signed_ext(alice, crate::Call::from(call_alice));
		Executive::apply_extrinsic(ext_alice).unwrap().unwrap();

		// Bob: ExternalAsset
		let secret_bob = get_unique_secret();
		let member_bob = CryptoOf::<Test>::member_from_secret(&secret_bob);
		let proof_bob = CryptoOf::<Test>::sign(&secret_bob, &bob.encode()).unwrap();

		let call_bob = crate::Call::pay_for_recycler_unload_fee_token_with_external_asset {
			member_key: member_bob,
			proof_of_ownership: proof_bob,
		};
		let ext_bob = build_signed_ext(bob, crate::Call::from(call_bob));
		Executive::apply_extrinsic(ext_bob).unwrap().unwrap();

		// Charlie: Coin
		create_coin(charlie, 0, 0);

		let secret_charlie = get_unique_secret();
		let member_charlie = CryptoOf::<Test>::member_from_secret(&secret_charlie);
		let proof_charlie = CryptoOf::<Test>::sign(&secret_charlie, &charlie.encode()).unwrap();

		let call_charlie = crate::Call::pay_for_recycler_unload_fee_token_with_coin {
			member_key: member_charlie,
			proof_of_ownership: proof_charlie,
		};
		let ext_charlie = build_signed_as_coin_ext(charlie, call_charlie, true);
		Executive::apply_extrinsic(ext_charlie).unwrap().unwrap();

		check_accounting();

		// ====================
		// 2. Build Ring 0 via Members::process_maintenance()
		// ====================
		let now_secs = MockTime::now().as_secs() as u32;
		let period_duration = get_u32::<<Test as Config>::PaidUnloadTokenTimePeriod>();
		let period = now_secs / period_duration;
		let ring_index = 0u32;

		// Collection should exist
		assert!(PaidTokenCollectionsCreated::<Test>::contains_key(BigEndianPeriod::from(period)));

		// Trigger ring building
		Members::process_maintenance();

		// Verify ring was built via ring_status
		let identifier = Coinage::paid_token_collection_identifier(period);
		let status = <Test as Config>::MemberService::ring_status(&identifier, ring_index).unwrap();
		assert_eq!(status.included, 3);

		// ====================
		// 3. Alice uses her token from Ring 0
		// ====================
		let (r_secrets, r_idx, r_rev) = setup_recycler(coin_val, 1, 50);
		check_accounting();

		let dest_coin = 4000u64;
		let alias =
			CryptoOf::<Test>::alias_in_context(&r_secrets[0], UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();

		let unload_call = crate::Call::unload_recycler_into_coin {
			aliases: bounded_vec![alias],
			value: coin_val,
			index: r_idx,
			revision: r_rev,
			to: dest_coin,
		};

		let paid_ring_revision =
			<Test as Config>::MemberService::ring_revision(&identifier, ring_index).unwrap();

		let uxt = build_unload_paid_ext(
			unload_call,
			&secret_alice,
			ring_index,
			paid_ring_revision,
			period,
			&r_secrets,
			coin_val,
			r_idx,
		);
		Executive::apply_extrinsic(uxt).unwrap().unwrap();
		check_accounting();

		// ====================
		// 4. More users join (10 additional)
		// ====================
		for i in 3..13u32 {
			let u = 5000 + i as u64;
			fund_native(u, 1000);
			let s = get_unique_secret();
			let m = CryptoOf::<Test>::member_from_secret(&s);
			let p = CryptoOf::<Test>::sign(&s, &u.encode()).unwrap();
			let c = crate::Call::pay_for_recycler_unload_fee_token_with_native {
				member_key: m,
				proof_of_ownership: p,
			};
			let ext = build_signed_ext(u, crate::Call::from(c));
			Executive::apply_extrinsic(ext).unwrap().unwrap();
		}

		// ====================
		// 5. Rebuild Ring 0 (now 13 members)
		// ====================
		Members::process_maintenance();

		// Check that all 13 members are included in ring 0
		let status = <Test as Config>::MemberService::ring_status(&identifier, ring_index).unwrap();
		assert_eq!(status.included, 13);

		// ====================
		// 6. New user joins, rebuild Ring 0 again (now 14 members, still fits)
		// ====================
		let user_new = 6000u64;
		fund_native(user_new, 1000);
		let secret_new = get_unique_secret();
		let member_new = CryptoOf::<Test>::member_from_secret(&secret_new);
		let proof_new = CryptoOf::<Test>::sign(&secret_new, &user_new.encode()).unwrap();
		let call_new = crate::Call::pay_for_recycler_unload_fee_token_with_native {
			member_key: member_new,
			proof_of_ownership: proof_new,
		};
		let ext_new = build_signed_ext(user_new, crate::Call::from(call_new));
		Executive::apply_extrinsic(ext_new).unwrap().unwrap();

		// Build again to include the new member
		Members::process_maintenance();

		// The new user may land in ring 0 (if not full) or ring 1 depending on capacity.
		// With RingExponent::R2e10 capacity 767, all 14 members fit in ring 0.
		let status = <Test as Config>::MemberService::ring_status(&identifier, ring_index).unwrap();
		assert_eq!(status.included, 14);

		// ====================
		// 7. New user uses their token, Bob uses his token
		// ====================

		// New user uses token
		let (r_secrets_3, r_idx_3, r_rev_3) = setup_recycler(coin_val, 1, 70);
		let dest_coin_3 = 4002u64;
		let alias_3 = CryptoOf::<Test>::alias_in_context(
			&r_secrets_3[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let unload_call_3 = crate::Call::unload_recycler_into_coin {
			aliases: bounded_vec![alias_3],
			value: coin_val,
			index: r_idx_3,
			revision: r_rev_3,
			to: dest_coin_3,
		};

		// Get latest revision after builds
		let paid_ring_revision =
			<Test as Config>::MemberService::ring_revision(&identifier, ring_index).unwrap();
		let uxt_3 = build_unload_paid_ext(
			unload_call_3,
			&secret_new,
			ring_index,
			paid_ring_revision,
			period,
			&r_secrets_3,
			coin_val,
			r_idx_3,
		);
		Executive::apply_extrinsic(uxt_3).unwrap().unwrap();
		check_accounting();

		// Bob uses his token from Ring 0 (still valid)
		let (r_secrets_2, r_idx_2, r_rev_2) = setup_recycler(coin_val, 1, 60);
		let dest_coin_2 = 4001u64;
		let alias_2 = CryptoOf::<Test>::alias_in_context(
			&r_secrets_2[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();

		let unload_call_2 = crate::Call::unload_recycler_into_coin {
			aliases: bounded_vec![alias_2],
			value: coin_val,
			index: r_idx_2,
			revision: r_rev_2,
			to: dest_coin_2,
		};

		let uxt_2 = build_unload_paid_ext(
			unload_call_2,
			&secret_bob,
			ring_index,
			paid_ring_revision,
			period,
			&r_secrets_2,
			coin_val,
			r_idx_2,
		);
		Executive::apply_extrinsic(uxt_2).unwrap().unwrap();
		check_accounting();

		// ====================
		// 8. Wait for next period
		// ====================
		let next_period_start =
			(period + 1) * get_u32::<<Test as Config>::PaidUnloadTokenTimePeriod>();
		advance_until_time(next_period_start);
		check_accounting();

		// ====================
		// 9. Verify clean fails before expiration
		// ====================
		let expiry_time =
			next_period_start + get_u32::<<Test as Config>::PaidUnloadTokenRingExpirationTime>();

		advance_until_time(expiry_time - 2);

		// Build an authorized extrinsic for clean_paid_unload_token_ring and validate it
		// fails (because we're still before expiration)
		let clean_ring_ext = build_authorized_ext(crate::Call::clean_paid_unload_token_ring {
			period,
			ring_index: 0,
		});
		assert_eq!(
			Executive::validate_transaction(
				TransactionSource::Local,
				clean_ring_ext,
				Default::default(),
			),
			Err(TransactionValidityError::Invalid(InvalidTransaction::Future))
		);

		// ====================
		// 10. Wait for expiry and clean (2-step: clean ring then delete collection)
		// ====================
		advance_until_time(expiry_time);

		// Collection still exists before clean
		assert!(PaidTokenCollectionsCreated::<Test>::contains_key(BigEndianPeriod::from(period)));

		// Step 1: Clean ring 0 members
		let clean_ring_ext = build_authorized_ext(crate::Call::clean_paid_unload_token_ring {
			period,
			ring_index: 0,
		});
		Executive::apply_extrinsic(clean_ring_ext).unwrap().unwrap();

		// Collection should still exist — only the ring was cleaned
		assert!(PaidTokenCollectionsCreated::<Test>::contains_key(BigEndianPeriod::from(period)));

		// Step 2: Delete the collection
		let delete_ext =
			build_authorized_ext(crate::Call::delete_expired_paid_unload_token_collection {
				period,
			});
		Executive::apply_extrinsic(delete_ext).unwrap().unwrap();

		let clean_dust_ext = build_authorized_ext(crate::Call::clean_paid_unload_token_dust {});
		Executive::apply_extrinsic(clean_dust_ext).unwrap().unwrap();

		// Verify collection is removed
		assert!(!PaidTokenCollectionsCreated::<Test>::contains_key(BigEndianPeriod::from(period)));

		check_accounting();
	});
}

#[test]
fn paid_collection_is_created_on_poll_period_boundary() {
	new_test_ext().execute_with(|| {
		setup_asset();

		let period_duration = get_u32::<<Test as Config>::PaidUnloadTokenTimePeriod>();

		// Period 0 gets created proactively by on_poll, without any paid token tx.
		let mut meter = WeightMeter::new();
		Coinage::on_poll(frame_system::Pallet::<Test>::block_number(), &mut meter);
		assert!(PaidTokenCollectionsCreated::<Test>::contains_key(BigEndianPeriod::from(0)));

		let identifier_0 = Coinage::paid_token_collection_identifier(0);
		assert!(<Test as Config>::MemberService::ring_status(&identifier_0, 0).is_some());

		// Re-running in the same period should not create duplicates.
		let mut meter = WeightMeter::new();
		Coinage::on_poll(frame_system::Pallet::<Test>::block_number(), &mut meter);
		assert_eq!(PaidTokenCollectionsCreated::<Test>::iter_keys().count(), 1);

		// At the next period boundary, period 1 should be created proactively as well.
		advance_until_time(period_duration);
		let mut meter = WeightMeter::new();
		Coinage::on_poll(frame_system::Pallet::<Test>::block_number(), &mut meter);
		assert!(PaidTokenCollectionsCreated::<Test>::contains_key(BigEndianPeriod::from(1)));

		let identifier_1 = Coinage::paid_token_collection_identifier(1);
		assert!(<Test as Config>::MemberService::ring_status(&identifier_1, 0).is_some());
	});
}

#[test]
fn paid_ring_multi_ring_cleanup() {
	new_test_ext().execute_with(|| {
		setup_asset();
		check_accounting();

		let ring_capacity = R2E10_RING_CAPACITY;
		let total_members = ring_capacity + 1;
		let mut members = Vec::with_capacity(total_members as usize);

		for i in 0..total_members {
			let user = 10_000 + i as u64;
			fund_native(user, 1000);

			let secret = get_unique_secret();
			let member = CryptoOf::<Test>::member_from_secret(&secret);
			let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

			let call = crate::Call::pay_for_recycler_unload_fee_token_with_native {
				member_key: member,
				proof_of_ownership: proof,
			};
			let ext = build_signed_ext(user, crate::Call::from(call));
			Executive::apply_extrinsic(ext).unwrap().unwrap();
			members.push(member);
		}

		let now_secs = MockTime::now().as_secs() as u32;
		let period_duration = get_u32::<<Test as Config>::PaidUnloadTokenTimePeriod>();
		let period = now_secs / period_duration;
		let identifier = Coinage::paid_token_collection_identifier(period);

		// Ensure all 768 members are processed and ring 1 is populated.
		let expected_ring1_total = total_members - ring_capacity;
		for _ in 0..30 {
			Members::process_maintenance();
			let status1_total = <Test as Config>::MemberService::ring_status(&identifier, 1)
				.map(|status| status.total)
				.unwrap_or(0);
			if status1_total == expected_ring1_total {
				break;
			}
		}

		let status0 = <Test as Config>::MemberService::ring_status(&identifier, 0).unwrap();
		assert_eq!(status0.total, ring_capacity);
		let status1 = <Test as Config>::MemberService::ring_status(&identifier, 1).unwrap();
		assert_eq!(status1.total, expected_ring1_total);

		let next_period_start = (period + 1) * period_duration;
		let expiry_time =
			next_period_start + get_u32::<<Test as Config>::PaidUnloadTokenRingExpirationTime>();
		advance_until_time(expiry_time);

		// Clean ring 0
		let clean_r0 = build_authorized_ext(crate::Call::clean_paid_unload_token_ring {
			period,
			ring_index: 0,
		});
		Executive::apply_extrinsic(clean_r0).unwrap().unwrap();

		// Collection still exists — ring 1 not cleaned yet
		assert!(PaidTokenCollectionsCreated::<Test>::contains_key(BigEndianPeriod::from(period)));

		// Clean ring 1
		let clean_r1 = build_authorized_ext(crate::Call::clean_paid_unload_token_ring {
			period,
			ring_index: 1,
		});
		Executive::apply_extrinsic(clean_r1).unwrap().unwrap();

		// Delete the collection
		let delete_ext =
			build_authorized_ext(crate::Call::delete_expired_paid_unload_token_collection {
				period,
			});
		Executive::apply_extrinsic(delete_ext).unwrap().unwrap();

		assert!(!PaidTokenCollectionsCreated::<Test>::contains_key(BigEndianPeriod::from(period)));
		for member in &members {
			assert!(!PaidUnloadTokenMembers::<Test>::contains_key(member));
		}

		check_accounting();
	});
}
