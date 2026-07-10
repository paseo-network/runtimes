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
use frame_support::assert_ok;
use indiv_support::traits::AppendOnlyMembers;
use sp_runtime::{
	bounded_vec,
	transaction_validity::{InvalidTransaction, TransactionSource, TransactionValidityError},
};
use verifiable::GenerateVerifiable;

// The value tested for.
const VALUE: crate::pallet::CoinValue = 0;

// Lifecycle test for recyclers with pallet-members architecture.
//
// The story covers:
// 1. Load r0 with 1 asset → verify collection created and member mapped.
// 2. Trigger Members::process_maintenance() to build ring → verify via ring_status.
// 3. Unload 1 item.
// 4. Load 10 more members into the ring.
// 5. Trigger Members::process_maintenance() to build → verify revision updated.
// 6. Unload from r0.
// 7. Advance time past expiration (immutable_since + RecyclerExpirationTime).
// 8. Verify clean_recycler fails before expiration (InvalidTransaction::Future).
// 9. Advance to expiration time.
// 10. Execute clean_recycler → verify ring removed (RecyclersLastRemovedRingIndex updated).
// 11. Check accounting.
//
// Throughout the test, the accounting invariant is verified:
// Pallet Balance = Active Coins + Active Recyclers Value + Destroyed Value.
#[test]
fn test_recycler_lifecycle_granular() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let asset_id = 10;
		let alice = ALICE;
		let dest_user = 5000u64;

		// Fund Alice
		assert_ok!(Assets::mint(
			RuntimeOrigin::signed(1),
			asset_id,
			alice,
			1_000_000 * UNDERLYING_ASSET_UNIT
		));

		check_accounting();

		// ====================================================================
		// 1. Load r0 with 1 asset
		// ====================================================================
		let secret_r0_0 = get_unique_secret();
		let member_r0_0 = CryptoOf::<Test>::member_from_secret(&secret_r0_0);
		let proof_r0_0 = CryptoOf::<Test>::sign(&secret_r0_0, &alice.encode()).unwrap();

		assert_ok!(Coinage::load_recycler_with_external_asset(
			RuntimeOrigin::signed(alice),
			CodecPreservation::Expendable,
			VALUE,
			member_r0_0,
			proof_r0_0
		));

		// Verify collection created and member mapped
		assert!(RecyclerCollectionCreated::<Test>::contains_key(VALUE));
		assert!(RecyclersCoinToRecycler::<Test>::contains_key(member_r0_0));

		// ====================================================================
		// 2. Build ring via Members::process_maintenance()
		// ====================================================================
		Members::process_maintenance();

		let identifier = Coinage::recycler_collection_identifier(VALUE);
		let status_r0 =
			<Test as Config>::MemberService::ring_status(&identifier, 0).expect("ring 0 exists");
		assert_eq!(status_r0.total, 1);
		assert_eq!(status_r0.included, 1);

		let r0_revision_v1 =
			<Test as Config>::MemberService::ring_revision(&identifier, 0).expect("ring 0 built");
		assert_eq!(r0_revision_v1, 0);

		check_accounting();

		// ====================================================================
		// 3. Unload 1 item (using Output Fee for simplicity)
		// ====================================================================
		let alias_r0_0 =
			CryptoOf::<Test>::alias_in_context(&secret_r0_0, UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();

		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			aliases: bounded_vec![alias_r0_0],
			value: VALUE,
			index: 0,
			revision: r0_revision_v1,
			to: dest_user,
		};
		let ext = build_unload_from_output_ext(
			call,
			VALUE,
			0,
			r0_revision_v1,
			std::slice::from_ref(&secret_r0_0),
		);
		Executive::apply_extrinsic(ext).unwrap().unwrap();

		// Verify unloaded
		assert!(RecyclersUnloaded::<Test>::contains_key((VALUE, 0u32, alias_r0_0)));

		check_accounting();

		// ====================================================================
		// 4. Load 10 more members
		// ====================================================================
		let mut secrets_r0_rest = Vec::new();
		for _ in 0..10 {
			let secret = get_unique_secret();
			let member = CryptoOf::<Test>::member_from_secret(&secret);
			let proof = CryptoOf::<Test>::sign(&secret, &alice.encode()).unwrap();
			secrets_r0_rest.push(secret);

			assert_ok!(Coinage::load_recycler_with_external_asset(
				RuntimeOrigin::signed(alice),
				CodecPreservation::Expendable,
				VALUE,
				member,
				proof
			));
		}

		// ====================================================================
		// 5. Build ring → verify revision updated
		// ====================================================================
		Members::process_maintenance();

		let status_r0 =
			<Test as Config>::MemberService::ring_status(&identifier, 0).expect("ring 0 exists");
		assert_eq!(status_r0.total, 11); // 1 original + 10 new
		assert_eq!(status_r0.included, 11);

		let r0_revision_v2 =
			<Test as Config>::MemberService::ring_revision(&identifier, 0).expect("ring 0 built");
		assert!(r0_revision_v2 > r0_revision_v1, "revision should have increased");

		check_accounting();

		// ====================================================================
		// 6. Unload 1 from r0 (from the second batch)
		// ====================================================================
		{
			let secret = secrets_r0_rest[0].clone();
			let alias =
				CryptoOf::<Test>::alias_in_context(&secret, UNLOADING_RECYCLER_CONTEXT.as_ref())
					.unwrap();

			let call = crate::Call::<Test>::unload_recycler_into_external_asset {
				aliases: bounded_vec![alias],
				value: VALUE,
				index: 0,
				revision: r0_revision_v2,
				to: dest_user,
			};
			let ext = build_unload_from_output_ext(call, VALUE, 0, r0_revision_v2, &[secret]);
			Executive::apply_extrinsic(ext).unwrap().unwrap();
		}

		check_accounting();

		// ====================================================================
		// 7. Fill ring to capacity to trigger immutable_since
		// ====================================================================
		// Ring capacity is 767 (R2e10). We have 11 members. Load 756 more to fill it.
		let ring_capacity = R2E10_RING_CAPACITY;
		let to_fill = ring_capacity - 11;
		for _ in 0..to_fill {
			let secret = get_unique_secret();
			let member = CryptoOf::<Test>::member_from_secret(&secret);
			let proof = CryptoOf::<Test>::sign(&secret, &alice.encode()).unwrap();

			assert_ok!(Coinage::load_recycler_with_external_asset(
				RuntimeOrigin::signed(alice),
				CodecPreservation::Expendable,
				VALUE,
				member,
				proof
			));
		}

		// Build to include all members and mark ring as immutable (full).
		for _ in 0..10 {
			Members::process_maintenance();
		}

		let status_r0 =
			<Test as Config>::MemberService::ring_status(&identifier, 0).expect("ring 0 exists");
		assert_eq!(status_r0.total, ring_capacity);
		// immutable_since should be set now that the ring is full and a new ring has started.
		// Note: immutable_since is set when the ring becomes full AND a new member triggers
		// advancing to the next ring. Let's load one more member to ensure the ring advances.

		// If immutable_since is not yet set, it means the ring is full but hasn't been
		// "sealed" because no new member triggered the transition. Load one more member
		// to push into ring 1.
		if status_r0.immutable_since.is_none() {
			let secret = get_unique_secret();
			let member = CryptoOf::<Test>::member_from_secret(&secret);
			let proof = CryptoOf::<Test>::sign(&secret, &alice.encode()).unwrap();

			assert_ok!(Coinage::load_recycler_with_external_asset(
				RuntimeOrigin::signed(alice),
				CodecPreservation::Expendable,
				VALUE,
				member,
				proof
			));

			Members::process_maintenance();
		}

		// Ensure ring 1 is populated before asserting its state.
		if <Test as Config>::MemberService::ring_status(&identifier, 1)
			.map(|s| s.total)
			.unwrap_or(0) ==
			0
		{
			let secret = get_unique_secret();
			let member = CryptoOf::<Test>::member_from_secret(&secret);
			let proof = CryptoOf::<Test>::sign(&secret, &alice.encode()).unwrap();

			assert_ok!(Coinage::load_recycler_with_external_asset(
				RuntimeOrigin::signed(alice),
				CodecPreservation::Expendable,
				VALUE,
				member,
				proof
			));

			Members::process_maintenance();
		}

		let status_r0 =
			<Test as Config>::MemberService::ring_status(&identifier, 0).expect("ring 0 exists");
		assert_eq!(status_r0.total, ring_capacity);
		let r0_immutable_since =
			status_r0.immutable_since.expect("ring 0 should be immutable (full)") as u32;
		let status_r1 =
			<Test as Config>::MemberService::ring_status(&identifier, 1).expect("ring 1 exists");
		assert!(
			status_r1.total >= 1,
			"ring 1 should contain at least one member once ring 0 is full"
		);

		check_accounting();

		// ====================================================================
		// 8. Verify clean_recycler fails before expiration
		// ====================================================================
		let expiration = get_u32::<<Test as crate::Config>::RecyclerExpirationTime>();
		let target_time_0 = r0_immutable_since + expiration;
		advance_until_time(target_time_0.saturating_sub(2)); // -2 because one block is 2s

		let clean_recycler_ext = build_authorized_ext(crate::Call::clean_recycler { value: VALUE });
		assert_eq!(
			Executive::validate_transaction(
				TransactionSource::Local,
				clean_recycler_ext,
				Default::default(),
			),
			Err(TransactionValidityError::Invalid(InvalidTransaction::Future))
		);

		// ====================================================================
		// 9. Advance to expiration time
		// ====================================================================
		advance_until_time(target_time_0);

		// ====================================================================
		// 10. Execute clean_recycler
		// ====================================================================
		let clean_recycler_ext = build_authorized_ext(crate::Call::clean_recycler { value: VALUE });
		Executive::apply_extrinsic(clean_recycler_ext).unwrap().unwrap();

		let clean_dust_ext = build_authorized_ext(crate::Call::clean_recycler_dust {});
		Executive::apply_extrinsic(clean_dust_ext).unwrap().unwrap();

		// Verify ring removed
		assert_eq!(RecyclersLastRemovedRingIndex::<Test>::get(VALUE), Some(0));

		// Verify unloaded entries cleared for ring 0
		assert!(!RecyclersUnloaded::<Test>::contains_key((VALUE, 0u32, alias_r0_0)));

		// ====================================================================
		// 11. Check destroyed value and accounting
		// ====================================================================
		// Ring 0 had 767 members total. 2 were unloaded (step 3 and step 6).
		// clean_unchecked now correctly subtracts unloaded count, so destroyed = 765 * 1000.
		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(), (ring_capacity as u64 - 2) * 1000);

		check_accounting();
	});
}
