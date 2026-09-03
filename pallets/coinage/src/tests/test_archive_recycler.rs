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

//! Tests for archiving recyclers on cleanup.

use crate::{mock::*, *};
use indiv_support::traits::{Alias, AppendOnlyMembers};
use verifiable::GenerateVerifiable;

/// Collect the aliases for the first `n` recycler secrets in the unloading context.
fn aliases_for(secrets: &[Secret], n: usize) -> Vec<Alias> {
	secrets[..n]
		.iter()
		.map(|secret| {
			CryptoOf::<Test>::alias_in_context(
				secret,
				crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
			)
			.unwrap()
		})
		.collect()
}

// A recycler ring with unloaded aliases gets archived on clean, committing to
// blake2_256(unloaded_aliases_root ++ recycler_root).
#[test]
fn archive_records_combined_root_on_clean() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let value = 0;
		let ring_capacity = R2E10_RING_CAPACITY;

		// Fill ring 0 to capacity (and start ring 1) so ring 0 becomes immutable.
		let (secrets, _index, _revision) = setup_recycler(value, ring_capacity + 1, 0);
		for _ in 0..10 {
			Members::process_maintenance();
		}

		let identifier = Coinage::recycler_collection_identifier(TEST_INSTANCE_ID, value);
		let status = <Test as Config>::MemberService::ring_status(&identifier, 0).unwrap();
		assert_eq!(status.total, ring_capacity);
		let immutable_since = status.immutable_since.unwrap() as u32;
		let r0_revision = <Test as Config>::MemberService::ring_revision(&identifier, 0).unwrap();

		// Unload the first 5 coins from ring 0, creating entries in `RecyclersUnloaded`.
		let unloaded = aliases_for(&secrets, 5);
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: unloaded.clone().try_into().unwrap(),
			value,
			index: 0,
			revision: r0_revision,
			to: 5000u64,
			max_fee: unload_token_fee_in_asset(),
		};
		let ext = build_unload_from_output_ext(call, value, 0, r0_revision, &secrets[0..5]);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Capture the recycler (ring-VRF) root before cleanup removes the ring.
		let recycler_root = Coinage::recycler_ring_root(TEST_INSTANCE_ID, value, 0)
			.expect("ring 0 has a root before cleanup");

		// Expire the ring and trigger `clean_recycler` via the offchain worker.
		let expiration = get_u32::<<Test as crate::Config>::RecyclerExpirationTime>();
		advance_until_time(immutable_since + expiration);
		advance_block();

		// The archival commitment must have been recorded, with `remaining` = not-yet-unloaded.
		let archived = RecyclersArchives::<Test>::get((TEST_INSTANCE_ID, value, 0u32))
			.expect("a recycler with recoverable coins must be archived on clean");

		// Recompute the expected commitment independently.
		let unloaded_root = RecyclerManager::<Test>::unloaded_aliases_root(&unloaded).unwrap();
		let expected = archive_commitment(unloaded_root, &recycler_root);
		assert_eq!(archived.commitment, expected);
		assert_eq!(archived.remaining, ring_capacity - 5);

		// Cleanup must not destroy the remaining (now archived) value.
		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(TEST_INSTANCE_ID), 0);

		System::assert_has_event(
			crate::Event::<Test>::RecyclerArchived {
				instance_id: TEST_INSTANCE_ID,
				value,
				ring_index: 0,
				recycler_root: recycler_root.clone(),
			}
			.into(),
		);

		// The archive survives dusting of the unloaded aliases.
		advance_block();
		assert!(!RecyclersDusting::<Test>::contains_key((TEST_INSTANCE_ID, value, 0u32)));
		for alias in &unloaded {
			assert!(!RecyclerAliasStates::<Test>::contains_key((
				TEST_INSTANCE_ID,
				value,
				0u32,
				*alias
			)));
		}
		assert_eq!(
			RecyclersArchives::<Test>::get((TEST_INSTANCE_ID, value, 0u32))
				.unwrap()
				.commitment,
			expected
		);
	});
}

// A recycler ring cleaned with no unloaded aliases is still archived (all coins recoverable),
// committing to the empty-trie unloaded root.
#[test]
fn archive_records_full_ring_with_empty_unloaded_root() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let value = 0;
		let ring_capacity = R2E10_RING_CAPACITY;

		let (_secrets, _index, _revision) = setup_recycler(value, ring_capacity + 1, 0);
		for _ in 0..10 {
			Members::process_maintenance();
		}

		let identifier = Coinage::recycler_collection_identifier(TEST_INSTANCE_ID, value);
		let status = <Test as Config>::MemberService::ring_status(&identifier, 0).unwrap();
		let immutable_since = status.immutable_since.unwrap() as u32;
		let recycler_root = Coinage::recycler_ring_root(TEST_INSTANCE_ID, value, 0)
			.expect("ring 0 has a root before cleanup");

		// No coins unloaded: the whole ring is recoverable, so it must still be archived.
		let expiration = get_u32::<<Test as crate::Config>::RecyclerExpirationTime>();
		advance_until_time(immutable_since + expiration);
		advance_block();

		assert_eq!(RecyclersLastRemovedRingIndex::<Test>::get(TEST_INSTANCE_ID, value), Some(0));
		// Nothing unloaded → no dusting queued.
		assert!(!RecyclersDusting::<Test>::contains_key((TEST_INSTANCE_ID, value, 0u32)));

		let archived = RecyclersArchives::<Test>::get((TEST_INSTANCE_ID, value, 0u32))
			.expect("a full ring with no unloads is still archived");
		let empty_root = RecyclerManager::<Test>::unloaded_aliases_root(&[]).unwrap();
		let expected = archive_commitment(empty_root, &recycler_root);
		assert_eq!(archived.commitment, expected);
		assert_eq!(archived.remaining, ring_capacity);
		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(TEST_INSTANCE_ID), 0);
	});
}

// A recycler ring whose coins were all unloaded before cleanup has nothing recoverable, so it must
// NOT be archived (the `remaining > 0` guard is skipped).
#[test]
fn fully_unloaded_ring_is_not_archived() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let value = 0;
		let ring_capacity = R2E10_RING_CAPACITY;

		let (secrets, _index, _revision) = setup_recycler(value, ring_capacity + 1, 0);
		for _ in 0..10 {
			Members::process_maintenance();
		}

		let identifier = Coinage::recycler_collection_identifier(TEST_INSTANCE_ID, value);
		let status = <Test as Config>::MemberService::ring_status(&identifier, 0).unwrap();
		assert_eq!(status.total, ring_capacity);
		let immutable_since = status.immutable_since.unwrap() as u32;

		// Drain the whole ring: mark every coin unloaded before cleanup.
		for alias in aliases_for(&secrets, ring_capacity as usize) {
			RecyclerManager::<Test>::mark_alias_unloaded(TEST_INSTANCE_ID, value, 0, alias);
		}

		let expiration = get_u32::<<Test as crate::Config>::RecyclerExpirationTime>();
		advance_until_time(immutable_since + expiration);
		advance_block();

		// The ring was removed, but with no recoverable coins no archive is recorded.
		assert_eq!(RecyclersLastRemovedRingIndex::<Test>::get(TEST_INSTANCE_ID, value), Some(0));
		assert!(RecyclersArchives::<Test>::get((TEST_INSTANCE_ID, value, 0u32)).is_none());
	});
}
