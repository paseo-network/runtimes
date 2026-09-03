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
const VALUE: crate::pallet::Denomination = 0;

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
			TEST_INSTANCE_ID,
			CodecPreservation::Expendable,
			VALUE,
			member_r0_0,
			proof_r0_0
		));

		// Verify collection created and member mapped
		assert!(RecyclerCollectionCreated::<Test>::contains_key(TEST_INSTANCE_ID, VALUE));
		assert!(RecyclersCoinToRecycler::<Test>::contains_key(member_r0_0));

		// ====================================================================
		// 2. Build ring via Members::process_maintenance()
		// ====================================================================
		Members::process_maintenance();

		let identifier = Coinage::recycler_collection_identifier(TEST_INSTANCE_ID, VALUE);
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
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![alias_r0_0],
			value: VALUE,
			index: 0,
			revision: r0_revision_v1,
			to: dest_user,
			max_fee: unload_token_fee_in_asset(),
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
		assert!(matches!(
			RecyclerAliasStates::<Test>::get((TEST_INSTANCE_ID, VALUE, 0u32, alias_r0_0)),
			Some(AliasState::Unloaded),
		));

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
				TEST_INSTANCE_ID,
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
				instance_id: TEST_INSTANCE_ID,
				aliases: bounded_vec![alias],
				value: VALUE,
				index: 0,
				revision: r0_revision_v2,
				to: dest_user,
				max_fee: unload_token_fee_in_asset(),
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
				TEST_INSTANCE_ID,
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
				TEST_INSTANCE_ID,
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
				TEST_INSTANCE_ID,
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

		let clean_recycler_ext = build_authorized_ext(crate::Call::clean_recycler {
			instance_id: TEST_INSTANCE_ID,
			value: VALUE,
		});
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
		let clean_recycler_ext = build_authorized_ext(crate::Call::clean_recycler {
			instance_id: TEST_INSTANCE_ID,
			value: VALUE,
		});
		Executive::apply_extrinsic(clean_recycler_ext).unwrap().unwrap();

		let clean_dust_ext = build_authorized_ext(crate::Call::clean_recycler_dust {});
		Executive::apply_extrinsic(clean_dust_ext).unwrap().unwrap();

		// Verify ring removed
		assert_eq!(RecyclersLastRemovedRingIndex::<Test>::get(TEST_INSTANCE_ID, VALUE), Some(0));

		// Verify unloaded entries cleared for ring 0
		assert!(!matches!(
			RecyclerAliasStates::<Test>::get((TEST_INSTANCE_ID, VALUE, 0u32, alias_r0_0)),
			Some(AliasState::Unloaded),
		));

		// ====================================================================
		// 11. Check archived value and accounting
		// ====================================================================
		// Ring 0 had 767 members total. 2 were unloaded (step 3 and step 6). The remaining 765 are
		// no longer destroyed: they are archived and recoverable, so nothing is destroyed here.
		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(TEST_INSTANCE_ID), 0);
		assert_eq!(
			RecyclersArchives::<Test>::get((TEST_INSTANCE_ID, VALUE, 0u32))
				.unwrap()
				.remaining,
			ring_capacity - 2,
		);

		check_accounting();
	});
}

#[test]
fn recycler_alias_unloaded_event_emitted_on_success_not_on_locked_attempt_extrinsic() {
	// `RecyclerAliasUnloaded` must be emitted once per alias when an unload persists, and must
	// NOT be emitted when a failed dispatch reverts the premarked fee alias into a temporary
	// lock. The successful retry covers both emission sites: the fee alias is emitted from the
	// extension's `post_dispatch` (premark path) and the second alias from the batch unload.
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let value: Denomination = 0; // $1 coin = 1000 underlying units each
		let dest = CHARLIE;
		let (secrets, index, revision) = setup_recycler(value, 2, 0);
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

		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![alias0, alias1],
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
		};
		// Make the dispatch fail: validation quotes the conversion and accepts the call, then the
		// swap takes more of the asset than the quote bounding it allowed.
		set_fee_conversion_swap_surcharge(1);
		let ext =
			build_unload_from_output_ext(call.clone(), value, index, revision, &secrets[0..2]);

		// Extension validate passes, prepare premarks alias0 as unloaded, dispatch fails,
		// post_dispatch reverts alias0 into a temporary retry lock.
		let result = Executive::apply_extrinsic(ext);
		assert!(matches!(result, Ok(Err(_))), "dispatch should fail: {result:?}");
		assert!(
			super::get_recycler_alias_lock_until(value, index, alias0).is_some(),
			"failed dispatch should lock the fee alias for retry"
		);

		// The unload did not persist for any alias, so no event must have been emitted: not for
		// the premarked-then-locked alias0 (prepare/post_dispatch effects survive the dispatch
		// error) and not for alias1 (its unload was reverted with the failed dispatch).
		assert!(
			!System::events().iter().any(|record| matches!(
				record.event,
				RuntimeEvent::Coinage(crate::Event::RecyclerAliasUnloaded { .. })
			)),
			"no RecyclerAliasUnloaded event must be emitted for a failed dispatch"
		);

		// Wait out the lock and retry against a market in line with its quote, with a fresh retry
		// counter.
		set_fee_conversion_swap_surcharge(0);
		let lock_until = super::get_recycler_alias_lock_until(value, index, alias0)
			.expect("failed dispatch should lock the alias");
		advance_until_time(lock_until as u32);
		assert_eq!(super::get_recycler_alias_lock_until(value, index, alias0), None);

		let refreshed_ext =
			build_unload_from_output_ext(call, value, index, revision, &secrets[0..2]);
		let result = Executive::apply_extrinsic(refreshed_ext);
		assert!(matches!(result, Ok(Ok(_))), "retry should succeed: {result:?}");

		// Both aliases are now permanently unloaded and each got its event: alias0 through the
		// extension `post_dispatch` premark path, alias1 through `RecyclerManager::unload`.
		assert_eq!(
			RecyclerAliasStates::<Test>::get((TEST_INSTANCE_ID, value, index, alias0)),
			Some(AliasState::Unloaded),
		);
		assert_eq!(
			RecyclerAliasStates::<Test>::get((TEST_INSTANCE_ID, value, index, alias1)),
			Some(AliasState::Unloaded),
		);
		System::assert_has_event(
			crate::Event::<Test>::RecyclerAliasUnloaded {
				instance_id: TEST_INSTANCE_ID,
				value,
				ring_index: index,
				alias: alias0,
			}
			.into(),
		);
		System::assert_has_event(
			crate::Event::<Test>::RecyclerAliasUnloaded {
				instance_id: TEST_INSTANCE_ID,
				value,
				ring_index: index,
				alias: alias1,
			}
			.into(),
		);
		assert_eq!(
			System::events()
				.iter()
				.filter(|record| matches!(
					record.event,
					RuntimeEvent::Coinage(crate::Event::RecyclerAliasUnloaded { .. })
				))
				.count(),
			2,
			"exactly one event per unloaded alias"
		);
	});
}
