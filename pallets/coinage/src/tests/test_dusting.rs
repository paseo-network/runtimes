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
use frame_support::{assert_ok, traits::UnixTime};
use indiv_support::traits::AppendOnlyMembers;
use sp_runtime::bounded_vec;
use verifiable::GenerateVerifiable;

// Tests the gradual cleanup of unloaded aliases when a recycler ring is removed.
//
// Scenario:
// 1. Fill a recycler ring to capacity and trigger ring building.
// 2. Unload a few coins, creating entries in `RecyclersUnloaded`.
// 3. Advance time past the ring's expiration.
// 4. Automatically trigger `clean_recycler` via offchain worker — this removes the ring and queues
//    the (value, ring_index) in `RecyclersDusting` for gradual cleanup.
// 5. Automatically trigger `clean_recycler_dust` via offchain worker — this clears the unloaded
//    aliases from `RecyclersUnloaded` and removes the entry from `RecyclersDusting`.
//
// Verifies:
// - After `clean_recycler`: ring is removed, dusting entry exists, aliases still present.
// - After `clean_recycler_dust`: dusting entry removed, all aliases cleaned up.
#[test]
fn test_removed_recyclers_rings_dusting() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let value = 0;
		let ring_capacity = R2E10_RING_CAPACITY;

		// 1. Load coins to fill ring 0 completely and start ring 1
		let (secrets, _index, _revision) = setup_recycler(value, ring_capacity + 1, 0);

		// Ensure all 768 members are processed
		for _ in 0..10 {
			Members::process_maintenance();
		}

		// Ensure that ring 0 has been built and is marked as immutable
		let identifier = Coinage::recycler_collection_identifier(value);
		let status = <Test as Config>::MemberService::ring_status(&identifier, 0).unwrap();
		assert_eq!(status.total, ring_capacity);
		assert!(status.immutable_since.is_some());

		let immutable_since = status.immutable_since.unwrap() as u32;
		let r0_revision = <Test as Config>::MemberService::ring_revision(&identifier, 0).unwrap();

		// 2. Unload a few coins from ring 0 to create "dust" in `RecyclersUnloaded`
		let dest = 5000u64;
		let mut aliases = Vec::new();

		// Unload first 5 coins
		#[allow(clippy::needless_range_loop)]
		for i in 0..5 {
			let secret = &secrets[i];
			let alias = CryptoOf::<Test>::alias_in_context(
				secret,
				crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
			)
			.unwrap();
			aliases.push(alias);
		}

		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			aliases: aliases.clone().try_into().unwrap(),
			value,
			index: 0,
			revision: r0_revision,
			to: dest,
		};
		let ext = build_unload_from_output_ext(call, value, 0, r0_revision, &secrets[0..5]);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Build another unload using the old revision before the recycler is cleaned. This should
		// become invalid once ring 0 is removed, even though the old root is still retained.
		let stale_alias = CryptoOf::<Test>::alias_in_context(
			&secrets[5],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let build_stale_unload_ext = || {
			build_unload_from_output_ext(
				crate::Call::<Test>::unload_recycler_into_external_asset {
					aliases: bounded_vec![stale_alias],
					value,
					index: 0,
					revision: r0_revision,
					to: dest + 1,
				},
				value,
				0,
				r0_revision,
				&secrets[5..6],
			)
		};

		for alias in &aliases {
			assert!(RecyclersUnloaded::<Test>::contains_key((value, 0u32, *alias)));
		}

		// Verify that RecyclersCoinToRecycler entries exist for all ring 0 members
		let r0_members: Vec<_> = secrets[..ring_capacity as usize]
			.iter()
			.map(CryptoOf::<Test>::member_from_secret)
			.collect();
		for member in &r0_members {
			assert!(RecyclersCoinToRecycler::<Test>::contains_key(member));
		}

		// 3. Advance time to expire the ring
		let expiration = get_u32::<<Test as crate::Config>::RecyclerExpirationTime>();
		advance_until_time(immutable_since + expiration);

		// The members pallet still retains the historical root during the grace period, but coinage
		// must reject recycler proofs as soon as the recycler itself has expired.
		assert!(
			<Test as Config>::MemberService::is_revision_valid(&identifier, 0, r0_revision,),
			"the old recycler revision should still be retained by members at expiry time"
		);
		assert!(!RecyclerManager::<Test>::validate_recycler_revision(value, 0, r0_revision));
		assert_invalid(build_stale_unload_ext(), CustomInvalidity::InvalidRecyclerRevision);

		// 4. Run offchain worker, which should trigger `clean_recycler`
		advance_block();

		// Check that the ring was removed and added to `RecyclersDusting` queue
		assert_eq!(RecyclersLastRemovedRingIndex::<Test>::get(value), Some(0));
		assert!(RecyclersDusting::<Test>::contains_key((value, 0u32)));
		assert!(!RecyclerManager::<Test>::validate_recycler_revision(value, 0, r0_revision));

		// Verify that RecyclersCoinToRecycler entries for ring 0 members have been cleaned
		for member in &r0_members {
			assert!(!RecyclersCoinToRecycler::<Test>::contains_key(member));
		}

		// Verify that the unloaded aliases have NOT been removed yet (dusting handles it)
		for alias in &aliases {
			assert!(RecyclersUnloaded::<Test>::contains_key((value, 0u32, *alias)));
		}

		// Verify TotalValueOfDestroyedCoins accounts for remaining (non-unloaded) coins
		let unit = Coinage::coin_value_to_asset_amount(value).unwrap();
		let expected_destroyed = unit * (ring_capacity - 5) as u64; // 767 total - 5 unloaded
		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(), expected_destroyed);
		System::assert_has_event(
			crate::Event::<Test>::RecyclerCleaned {
				value,
				remaining_coins: ring_capacity - 5,
				destroyed_amount: expected_destroyed,
			}
			.into(),
		);

		// 5. Run offchain worker again, which should trigger `clean_recycler_dust`
		advance_block();

		// Check that `RecyclersDusting` queue for this ring is cleared
		assert!(!RecyclersDusting::<Test>::contains_key((value, 0u32)));

		// Verify that all unloaded aliases for this ring have been removed
		for alias in &aliases {
			assert!(!RecyclersUnloaded::<Test>::contains_key((value, 0u32, *alias)));
		}
		System::assert_has_event(crate::Event::<Test>::RecyclerDustCleaned.into());
	});
}

// Tests the gradual cleanup of consumed paid unload tokens when a period expires.
//
// Scenario:
// 1. Load 768 paid unload tokens (enough to fill multiple rings).
// 2. Trigger ring building for the paid tokens.
// 3. Consume a few tokens via real unload calls, and mark the rest as consumed directly.
// 4. Advance time past the period's expiration.
// 5. Automatically trigger `clean_paid_unload_token_ring` via offchain worker for each ring — this
//    removes ring members one ring at a time.
// 6. Automatically trigger `delete_expired_paid_unload_token_collection` via offchain worker — this
//    deletes the collection and queues the period in `PaidUnloadTokenDusting`.
// 7. Automatically trigger `clean_paid_unload_token_dust` via offchain worker — this clears the
//    consumed tokens from `PaidUnloadTokenConsumed` and removes the entry from
//    `PaidUnloadTokenDusting`.
//
// Verifies:
// - After ring cleanup: members removed, collection still exists.
// - After collection deletion: collection removed, dusting entry exists, consumed tokens still
//   present.
// - After `clean_paid_unload_token_dust` completes: dusting entry removed, all consumed tokens
//   cleaned up.
#[test]
fn test_paid_unload_token_dusting_flow() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let ring_capacity = R2E10_RING_CAPACITY as usize;
		let num_keys = ring_capacity + 1;
		let value = 0;

		let now_secs = MockTime::now().as_secs() as u32;
		let period_duration = get_u32::<<Test as Config>::PaidUnloadTokenTimePeriod>();
		let period = now_secs / period_duration;

		// 1. Load 768 paid unload tokens
		let mut paid_secrets = Vec::new();
		for i in 0..num_keys {
			let user = 10000 + i as u64;
			fund_native(user, 1000);

			let secret = get_unique_secret();
			let member = CryptoOf::<Test>::member_from_secret(&secret);
			let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

			assert_ok!(Coinage::pay_for_recycler_unload_fee_token_with_native(
				RuntimeOrigin::signed(user),
				member,
				proof
			));
			paid_secrets.push(secret);
		}

		// Trigger ring building for paid unload tokens
		for _ in 0..10 {
			Members::process_maintenance();
		}

		// 2. Unload them
		let mut ctx = [0u8; 32];
		ctx[..28].copy_from_slice(pallet::PAID_UNLOAD_TOKEN_CONTEXT_BASE.as_ref());
		ctx[28..32].copy_from_slice(&period.to_le_bytes());

		// We do 5 real unload calls to simulate the "usual flow"
		let (r_secrets, r_idx, r_rev) = setup_recycler(value, 5, 0);
		let identifier = Coinage::paid_token_collection_identifier(period);

		for i in 0..5 {
			let secret = &paid_secrets[i as usize];
			let ring_index = (i as usize / ring_capacity) as u32;
			let paid_ring_revision =
				<Test as Config>::MemberService::ring_revision(&identifier, ring_index).unwrap();

			let dest_user = 20000 + i as u64;
			let alias = CryptoOf::<Test>::alias_in_context(
				&r_secrets[i as usize],
				crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
			)
			.unwrap();
			let call = crate::Call::<Test>::unload_recycler_into_coin {
				aliases: bounded_vec![alias],
				value,
				index: r_idx,
				revision: r_rev,
				to: dest_user,
			};

			let uxt = build_unload_paid_ext(
				call.clone(),
				secret,
				ring_index,
				paid_ring_revision,
				period,
				&[r_secrets[i as usize].clone()],
				value,
				r_idx,
			);

			assert_eq!(Executive::apply_extrinsic(uxt), Ok(Ok(())));
		}

		// For the rest, directly mark them as consumed to speed up the test
		#[allow(clippy::needless_range_loop)]
		for i in 5..num_keys {
			let secret = &paid_secrets[i];
			let ring_index = (i / ring_capacity) as u32;
			let alias = CryptoOf::<Test>::alias_in_context(secret, &ctx).unwrap();
			PaidTknManager::<Test>::mark_token_consumed(period, ring_index, alias);
		}

		assert_eq!(
			pallet::PaidUnloadTokenConsumed::<Test>::iter_prefix((BigEndianPeriod::from(period),))
				.count(),
			num_keys
		);

		// 3. Advance time to just before expiration
		let expiration_time = (period + 1)
			.saturating_mul(period_duration)
			.saturating_add(get_u32::<<Test as Config>::PaidUnloadTokenRingExpirationTime>());

		advance_until_time(expiration_time);

		// Verify collection still exists before clean
		assert!(pallet::PaidTokenCollectionsCreated::<Test>::contains_key(BigEndianPeriod::from(
			period
		)));

		// Verify PaidUnloadTokenMembers entries exist for all members
		let paid_members: Vec<_> =
			paid_secrets.iter().map(CryptoOf::<Test>::member_from_secret).collect();
		for member in &paid_members {
			assert!(pallet::PaidUnloadTokenMembers::<Test>::contains_key(member));
		}

		// 4. Run offchain worker to trigger `clean_paid_unload_token_ring` for ring 0
		advance_block();
		// Collection should still exist — only ring 0 was cleaned
		assert!(pallet::PaidTokenCollectionsCreated::<Test>::contains_key(BigEndianPeriod::from(
			period
		)));
		System::assert_has_event(
			crate::Event::<Test>::PaidUnloadTokenRingCleaned { period, ring_index: 0 }.into(),
		);

		// Verify PaidUnloadTokenMembers entries for ring 0 members have been cleaned
		for member in &paid_members[..ring_capacity] {
			assert!(!pallet::PaidUnloadTokenMembers::<Test>::contains_key(member));
		}

		// 5. Run offchain worker to trigger `clean_paid_unload_token_ring` for ring 1
		advance_block();
		// Collection should still exist — rings cleaned but collection not yet deleted
		assert!(pallet::PaidTokenCollectionsCreated::<Test>::contains_key(BigEndianPeriod::from(
			period
		)));
		System::assert_has_event(
			crate::Event::<Test>::PaidUnloadTokenRingCleaned { period, ring_index: 1 }.into(),
		);

		// Verify PaidUnloadTokenMembers entries for ring 1 members have been cleaned
		for member in &paid_members[ring_capacity..] {
			assert!(!pallet::PaidUnloadTokenMembers::<Test>::contains_key(member));
		}

		// 6. Run offchain worker to trigger `delete_expired_paid_unload_token_collection`
		advance_block();
		assert!(!pallet::PaidTokenCollectionsCreated::<Test>::contains_key(BigEndianPeriod::from(
			period
		)));
		assert!(pallet::PaidUnloadTokenDusting::<Test>::contains_key(BigEndianPeriod::from(
			period
		)));
		// Consumed tokens should still all be there after collection deletion
		assert_eq!(
			pallet::PaidUnloadTokenConsumed::<Test>::iter_prefix((BigEndianPeriod::from(period),))
				.count(),
			num_keys
		);
		System::assert_has_event(
			crate::Event::<Test>::ExpiredPaidUnloadTokenCollectionDeleted { period }.into(),
		);

		// 7. Run offchain workers to complete dusting
		advance_block();

		// Verify all consumed tokens have been cleaned
		assert_eq!(
			pallet::PaidUnloadTokenConsumed::<Test>::iter_prefix((BigEndianPeriod::from(period),))
				.count(),
			0
		);
		assert!(!pallet::PaidUnloadTokenDusting::<Test>::contains_key(BigEndianPeriod::from(
			period
		)));
		System::assert_has_event(crate::Event::<Test>::PaidUnloadTokenDustCleaned.into());
	});
}
