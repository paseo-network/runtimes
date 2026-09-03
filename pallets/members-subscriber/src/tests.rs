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

//! Tests for the members-subscriber pallet.

use crate::{
	mock::{MaxDeletedRingsPerCollection, MaxMissingRootsPerCollection, *},
	pallet::{
		CurrentGeneration, Event, ProcessingState, QueuedRingPurge, RingCollectionExponents,
		RingCollectionStates, RingRoots, Subscription,
	},
	types::{
		Identifier, RingCollectionState, RingCommitmentRecord, RingPurgeProgress, RingRootOp,
		RingRootUpdate, RingRootUpdatesBatch, SubscriptionStatus,
	},
	Pallet,
};
use indiv_support::traits::RingExponent;

const TEST_RING_EXPONENT: RingExponent = RingExponent::R2e9;
use alloc::collections::BTreeSet;
use frame_support::{
	assert_noop, assert_ok,
	pallet_prelude::{BoundedBTreeMap, BoundedBTreeSet},
	traits::Hooks,
	BoundedVec,
};
use sp_runtime::{bounded_vec, DispatchError};

const PEOPLE: Identifier = [0u8; 32];
const PEOPLE_LITE: Identifier = [1u8; 32];

fn mock_ring_root(seed: u64) -> crate::mock::TestMembers {
	let mut members = crate::mock::TestMembers::default();
	members.try_push(seed).ok();
	members
}

type CollectionRingStateType =
	RingCollectionState<MaxMissingRootsPerCollection, MaxDeletedRingsPerCollection>;

fn make_collection_ring_state(
	ring_count: u32,
	next_ring_index: u32,
	missing_indices: &[(u32, u32)],
	deleted_indices: &[u32],
) -> CollectionRingStateType {
	let mut missing = BoundedBTreeMap::new();
	for (idx, count) in missing_indices {
		missing.try_insert(*idx, *count).unwrap();
	}
	let mut deleted = BoundedBTreeSet::new();
	for idx in deleted_indices {
		deleted.try_insert(*idx).unwrap();
	}
	RingCollectionState {
		ring_count,
		next_ring_index,
		missing_indices: missing,
		deleted_indices: deleted,
		..Default::default()
	}
}

fn next_scan_index(identifier: Identifier) -> u32 {
	RingCollectionStates::<Test>::get(identifier).next_scan_index
}

/// Roots stored under the current generation prefix.
fn ring_roots(
	identifier: Identifier,
	ring_index: u32,
) -> Option<BoundedVec<RingCommitmentRecord<Test>, MaxRecentRootsPerRing>> {
	Pallet::<Test>::current_ring_roots(&identifier, ring_index)
}

fn has_ring_root(identifier: Identifier, ring_index: u32) -> bool {
	ring_roots(identifier, ring_index).is_some()
}

fn is_missing(identifier: Identifier, idx: u32) -> bool {
	RingCollectionStates::<Test>::get(identifier).missing_indices.contains_key(&idx)
}

fn get_ring_count(identifier: Identifier) -> u32 {
	RingCollectionStates::<Test>::get(identifier).ring_count
}

fn total_missing_count() -> usize {
	RingCollectionStates::<Test>::iter()
		.map(|(_, state)| state.missing_indices.len())
		.sum()
}

/// Active subscription with both test collections initialized, which `process_ring_updates`
/// requires before it accepts a batch.
fn setup_active_subscription() {
	Subscription::<Test>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
	ProcessingState::<Test>::mutate(|s| s.last_processed_sequence = 1);
	for identifier in [PEOPLE, PEOPLE_LITE] {
		RingCollectionExponents::<Test>::insert(identifier, TEST_RING_EXPONENT);
	}
}

/// Dispatches purge pages until all stale ring roots are physically removed.
fn run_purge_to_completion() {
	while QueuedRingPurge::<Test>::exists() {
		assert_ok!(MembersSubscriber::purge_stale_ring_roots(RuntimeOrigin::from(
			frame_system::RawOrigin::Authorized
		)));
	}
}

/// Queues a purge of `generation` starting at `page`.
fn queue_purge(generation: u32, page: u32) {
	QueuedRingPurge::<Test>::put(RingPurgeProgress { generation, page });
}

/// The queued purge as a `(generation, page)` pair.
fn queued_purge() -> Option<(u32, u32)> {
	QueuedRingPurge::<Test>::get().map(|p| (p.generation, p.page))
}

fn mock_ring_root_updates_batch(
	sequence: u64,
	source_time: u64,
	indices: impl IntoIterator<Item = u32>,
	identifier: Identifier,
	next_ring_index: u32,
) -> RingRootUpdatesBatch<Test> {
	let mut updates = BoundedVec::new();
	for i in indices {
		updates
			.try_push(RingRootUpdate::<Test> {
				ring_index: i,
				op: RingRootOp::Built { revision: 1, root: mock_ring_root(i as u64) },
			})
			.unwrap();
	}

	RingRootUpdatesBatch::<Test> { identifier, sequence, source_time, updates, next_ring_index }
}

/// Verify that the default mock configuration passes all integrity checks,
/// including the block-fit assertion for `replay_missing_roots`.
#[test]
fn integrity_test_passes() {
	new_test_ext().execute_with(|| {
		<crate::Pallet<Test> as Hooks<u64>>::integrity_test();
	});
}

mod ring_roots_initialization {
	use super::*;

	#[test]
	fn fails_for_none_origin() {
		new_test_ext().execute_with(|| {
			let batch = RingRootUpdatesBatch::<Test>::default();
			assert_noop!(
				MembersSubscriber::initialize_ring_roots(
					RuntimeOrigin::none(),
					TEST_RING_EXPONENT,
					batch
				),
				DispatchError::BadOrigin
			);
		});
	}

	#[test]
	fn fails_for_signed_origin() {
		new_test_ext().execute_with(|| {
			let batch = RingRootUpdatesBatch::<Test>::default();
			assert_noop!(
				MembersSubscriber::initialize_ring_roots(
					RuntimeOrigin::signed(1),
					TEST_RING_EXPONENT,
					batch
				),
				DispatchError::BadOrigin
			);
		});
	}

	#[test]
	fn succeeds_with_notifier_origin() {
		new_test_ext().execute_with(|| {
			let batch = RingRootUpdatesBatch::<Test>::default();
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));
		});
	}

	#[test]
	fn rejects_re_init_with_lower_sequence_while_active() {
		new_test_ext().execute_with(|| {
			let batch1 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 5,
				source_time: 1000,
				updates: BoundedVec::new(),
				next_ring_index: 0,
			};
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch1
			));
			assert_eq!(
				Subscription::<Test>::get(),
				SubscriptionStatus::Active { initialized_at_sequence: 5 }
			);

			// Lower sequence while Active leads to error.
			let batch2 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 3,
				source_time: 2000,
				updates: BoundedVec::new(),
				next_ring_index: 0,
			};
			assert_noop!(
				MembersSubscriber::initialize_ring_roots(
					RuntimeOrigin::root(),
					TEST_RING_EXPONENT,
					batch2
				),
				crate::Error::<Test>::SubscriptionAlreadyActive
			);
			// State unchanged.
			assert_eq!(
				Subscription::<Test>::get(),
				SubscriptionStatus::Active { initialized_at_sequence: 5 }
			);
		});
	}

	#[test]
	fn reinit_with_higher_sequence_clears_old_data() {
		new_test_ext().execute_with(|| {
			// Initial initialization with ring roots.
			let updates: BoundedVec<RingRootUpdate<Test>, MaxUpdatesPerBatch> =
				bounded_vec![RingRootUpdate {
					ring_index: 0,
					op: RingRootOp::Built { revision: 1, root: mock_ring_root(42) },
				}];
			let batch1 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 1,
				source_time: 1000,
				updates,
				next_ring_index: 1,
			};
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				RingExponent::R2e9,
				batch1
			));

			// Old ring root stored.
			assert!(has_ring_root(PEOPLE, 0));
			assert_eq!(RingCollectionStates::<Test>::get(PEOPLE).ring_count, 1);
			assert_eq!(RingCollectionExponents::<Test>::get(PEOPLE), Some(RingExponent::R2e9));

			// Re-initialization with higher sequence, different data, and a different exponent.
			let new_updates: BoundedVec<RingRootUpdate<Test>, MaxUpdatesPerBatch> =
				bounded_vec![RingRootUpdate {
					ring_index: 5,
					op: RingRootOp::Built { revision: 1, root: mock_ring_root(99) },
				}];
			let batch2 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 10,
				source_time: 2000,
				updates: new_updates,
				next_ring_index: 6,
			};
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				RingExponent::R2e14,
				batch2
			));

			// Subscription updated to new sequence.
			assert_eq!(
				Subscription::<Test>::get(),
				SubscriptionStatus::Active { initialized_at_sequence: 10 }
			);

			// Old ring root logically cleared: unreachable but still under the stale prefix.
			assert!(!has_ring_root(PEOPLE, 0));
			assert_eq!(RingRoots::<Test>::iter().count(), 2);
			assert!(has_ring_root(PEOPLE, 5));
			assert_eq!(RingCollectionStates::<Test>::get(PEOPLE).ring_count, 1);

			// Purge removes the stale entry and keeps the new one.
			run_purge_to_completion();
			assert!(!has_ring_root(PEOPLE, 0));
			assert!(has_ring_root(PEOPLE, 5));

			// Stale exponent was wiped by `clear_all_ring_data` and replaced with the new one.
			assert_eq!(RingCollectionExponents::<Test>::get(PEOPLE), Some(RingExponent::R2e14));
		});
	}

	#[test]
	fn reinit_with_equal_sequence_succeeds() {
		new_test_ext().execute_with(|| {
			// Initialized with sequence 1.
			let batch1 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 1,
				source_time: 1000,
				updates: BoundedVec::new(),
				next_ring_index: 0,
			};
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch1
			));
			assert_eq!(
				Subscription::<Test>::get(),
				SubscriptionStatus::Active { initialized_at_sequence: 1 }
			);

			// Re-init with equal sequence succeeds (continuation).
			let batch2 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 1,
				source_time: 1000,
				updates: BoundedVec::new(),
				next_ring_index: 0,
			};
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch2
			));

			// Subscription remains Active with same sequence.
			assert_eq!(
				Subscription::<Test>::get(),
				SubscriptionStatus::Active { initialized_at_sequence: 1 }
			);
		});
	}

	#[test]
	fn stores_all_received_ring_roots() {
		new_test_ext().execute_with(|| {
			let mut updates = BoundedVec::new();
			for i in 0..3u32 {
				let update = RingRootUpdate::<Test> {
					ring_index: i,
					op: RingRootOp::Built { revision: 1, root: mock_ring_root(i as u64 + 100) },
				};
				updates.try_push(update).unwrap();
			}

			let batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 1,
				source_time: 12345,
				updates,
				next_ring_index: 3,
			};

			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));

			// All ring roots were stored.
			assert_eq!(RingRoots::<Test>::iter().count(), 3);

			// Each ring root has correct data.
			for i in 0..3u32 {
				let roots = ring_roots(PEOPLE, i).expect("ring root should exist");
				let record = roots.last().expect("at least one record");
				assert_eq!(record.root, mock_ring_root(i as u64 + 100));
				assert_eq!(record.revision, 1);
				assert_eq!(record.source_time, 12345);
				assert_eq!(record.source_sequence, 1);
			}

			// Sequence was updated.
			assert_eq!(ProcessingState::<Test>::get().last_processed_sequence, 1);
		});
	}

	#[test]
	fn sets_subscription_to_active() {
		new_test_ext().execute_with(|| {
			assert_eq!(Subscription::<Test>::get(), SubscriptionStatus::Inactive);

			let batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 42,
				source_time: 1000,
				updates: BoundedVec::new(),
				next_ring_index: 0,
			};

			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));

			assert_eq!(
				Subscription::<Test>::get(),
				SubscriptionStatus::Active { initialized_at_sequence: 42 }
			);
		});
	}

	#[test]
	fn updates_collection_ring_count() {
		new_test_ext().execute_with(|| {
			// Indices 0, 2, 5 (with gaps - 3 roots total)
			let batch = mock_ring_root_updates_batch(1, 1000, [0, 2, 5], PEOPLE, 6);

			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));

			// CollectionRingsCount should be 3 (number of roots stored)
			assert_eq!(get_ring_count(PEOPLE), 3);
		});
	}

	#[test]
	fn detects_missing_rings_in_received_batch() {
		new_test_ext().execute_with(|| {
			// Indices 0, 1, 3 (index 2 missing)
			let batch = mock_ring_root_updates_batch(1, 1000, [0, 1, 3], PEOPLE, 4);

			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));

			// 3 rings stored
			assert_eq!(get_ring_count(PEOPLE), 3);

			// Missing rings should only include index 2
			assert_eq!(total_missing_count(), 1);
			assert!(is_missing(PEOPLE, 2));
		});
	}

	#[test]
	fn updates_last_batch_received_time() {
		new_test_ext().execute_with(|| {
			assert_eq!(ProcessingState::<Test>::get().last_batch_received_time, 0);

			let batch = RingRootUpdatesBatch::<Test>::default();

			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));

			// MockUnixTime returns 1_700_000_000 seconds
			assert_eq!(ProcessingState::<Test>::get().last_batch_received_time, 1_700_000_000);
		});
	}

	#[test]
	fn no_missing_rings_detected_when_all_indices_present() {
		new_test_ext().execute_with(|| {
			// Rings from 0 to 4 (no missing)
			let batch = mock_ring_root_updates_batch(1, 1000, 0..5, PEOPLE, 5);

			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));

			// No missing rings should be detected
			assert_eq!(total_missing_count(), 0);
			assert_eq!(get_ring_count(PEOPLE), 5);
		});
	}

	#[test]
	fn detects_missing_rings_when_more_rings_on_notifier_side() {
		new_test_ext().execute_with(|| {
			// Rings from 0 to 4 (no missing) - but there's more rings on the notifier side
			let batch = mock_ring_root_updates_batch(1, 1000, 0..5, PEOPLE, 6);

			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));

			// One missing ring should be detected (index 5)
			assert_eq!(total_missing_count(), 1);
			assert!(is_missing(PEOPLE, 5));
			assert_eq!(get_ring_count(PEOPLE), 5);
		});
	}

	#[test]
	fn removes_recovered_roots_from_missing() {
		new_test_ext().execute_with(|| {
			// First init with gaps - indices 0, 2 provided, 1 and 3 missing
			let batch1 = mock_ring_root_updates_batch(1, 1000, [0, 2], PEOPLE, 4);

			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch1
			));

			// Indices 1 and 3 should be missing
			assert_eq!(total_missing_count(), 2);
			assert!(is_missing(PEOPLE, 1));
			assert!(is_missing(PEOPLE, 3));

			// Multi-part init continuation (same sequence) providing the missing root at index 1
			let batch2 = mock_ring_root_updates_batch(1, 1000, [1], PEOPLE, 4);

			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch2
			));

			// Index 1 should be removed from missing, only 3 remains
			assert_eq!(total_missing_count(), 1);
			assert!(is_missing(PEOPLE, 3));
			assert!(!is_missing(PEOPLE, 1));
		});
	}

	#[test]
	fn re_init_after_termination_clears_old_data() {
		new_test_ext().execute_with(|| {
			// First init with 5 rings
			let mut updates1 = BoundedVec::new();
			for i in 0..5u32 {
				updates1
					.try_push(RingRootUpdate::<Test> {
						ring_index: i,
						op: RingRootOp::Built { revision: 1, root: mock_ring_root(i as u64) },
					})
					.unwrap();
			}

			let batch1 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 1,
				source_time: 1000,
				updates: updates1,
				next_ring_index: 5,
			};

			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch1
			));
			assert_eq!(RingRoots::<Test>::iter().count(), 5);
			assert_eq!(get_ring_count(PEOPLE), 5);

			// Terminating before re-init
			assert_ok!(MembersSubscriber::terminate_subscription(RuntimeOrigin::root()));

			// Re-init with 3 different rings
			let mut updates2 = BoundedVec::new();
			for i in 10..13u32 {
				updates2
					.try_push(RingRootUpdate::<Test> {
						ring_index: i,
						op: RingRootOp::Built { revision: 2, root: mock_ring_root(i as u64 + 100) },
					})
					.unwrap();
			}

			let batch2 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 2,
				source_time: 2000,
				updates: updates2,
				next_ring_index: 3,
			};

			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch2
			));

			// Old rings logically cleared; purge removes them physically, only new 3 exist
			run_purge_to_completion();
			assert_eq!(RingRoots::<Test>::iter().count(), 3);
			assert_eq!(get_ring_count(PEOPLE), 3);

			// Old rings (0-4) don't exist
			for i in 0..5u32 {
				assert!(!has_ring_root(PEOPLE, i));
			}

			// New rings (10-12) exist
			for i in 10..13u32 {
				assert!(has_ring_root(PEOPLE, i));
			}
		});
	}

	#[test]
	fn multi_part_init_is_additive() {
		new_test_ext().execute_with(|| {
			// Part 1 of init with 3 rings
			let mut updates1 = BoundedVec::new();
			for i in 0..3u32 {
				updates1
					.try_push(RingRootUpdate::<Test> {
						ring_index: i,
						op: RingRootOp::Built { revision: 1, root: mock_ring_root(i as u64) },
					})
					.unwrap();
			}

			let batch1 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 1,
				source_time: 1000,
				updates: updates1,
				next_ring_index: 5,
			};

			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch1
			));
			assert_eq!(RingRoots::<Test>::iter().count(), 3);
			assert_eq!(get_ring_count(PEOPLE), 3);

			// Part 2 of init with 2 more rings (same sequence)
			let mut updates2 = BoundedVec::new();
			for i in 3..5u32 {
				updates2
					.try_push(RingRootUpdate::<Test> {
						ring_index: i,
						op: RingRootOp::Built { revision: 1, root: mock_ring_root(i as u64) },
					})
					.unwrap();
			}

			let batch2 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 1, // Same sequence = continuation
				source_time: 1000,
				updates: updates2,
				next_ring_index: 5,
			};

			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch2
			));

			// All 5 rings should exist
			assert_eq!(RingRoots::<Test>::iter().count(), 5);
			assert_eq!(get_ring_count(PEOPLE), 5);
		});
	}

	#[test]
	fn multi_collection_init_stores_per_identifier_exponents() {
		new_test_ext().execute_with(|| {
			// Init for PEOPLE with R2e9.
			let batch_people = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 1,
				source_time: 1000,
				updates: bounded_vec![RingRootUpdate {
					ring_index: 0,
					op: RingRootOp::Built { revision: 1, root: mock_ring_root(42) },
				}],
				next_ring_index: 1,
			};
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				RingExponent::R2e9,
				batch_people
			));

			// Init for PEOPLE_LITE with R2e14 (same sequence = continuation, no re-init clear).
			let batch_lite = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE_LITE,
				sequence: 1,
				source_time: 1000,
				updates: bounded_vec![RingRootUpdate {
					ring_index: 0,
					op: RingRootOp::Built { revision: 1, root: mock_ring_root(7) },
				}],
				next_ring_index: 1,
			};
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				RingExponent::R2e14,
				batch_lite
			));

			// Each identifier keeps its own exponent.
			assert_eq!(RingCollectionExponents::<Test>::get(PEOPLE), Some(RingExponent::R2e9));
			assert_eq!(
				RingCollectionExponents::<Test>::get(PEOPLE_LITE),
				Some(RingExponent::R2e14)
			);
		});
	}

	#[test]
	fn succeeds_when_subscription_terminated() {
		new_test_ext().execute_with(|| {
			Subscription::<Test>::put(SubscriptionStatus::Terminated);

			let batch = mock_ring_root_updates_batch(5, 1000, 0..2, PEOPLE, 2);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));
			assert_eq!(
				Subscription::<Test>::get(),
				SubscriptionStatus::Active { initialized_at_sequence: 5 }
			);
			assert_eq!(RingRoots::<Test>::iter().count(), 2);
		});
	}

	#[test]
	fn re_init_after_termination_clears_processing_state() {
		new_test_ext().execute_with(|| {
			// First init
			let batch1 = mock_ring_root_updates_batch(1, 1000, 0..3, PEOPLE, 3);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch1
			));

			// Manually set last_replay_request_time and inject stale state
			ProcessingState::<Test>::mutate(|s| {
				s.last_replay_request_time = 999;
			});
			assert_eq!(ProcessingState::<Test>::get().last_replay_request_time, 999);
			RingCollectionStates::<Test>::mutate(PEOPLE, |state| {
				state.missing_indices.try_insert(99, 1).unwrap();
				state.deleted_indices.try_insert(98).unwrap();
			});

			// Terminating before re-init
			assert_ok!(MembersSubscriber::terminate_subscription(RuntimeOrigin::root()));

			// Re-init with different sequence
			let batch2 = mock_ring_root_updates_batch(2, 2000, 0..2, PEOPLE, 2);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch2
			));

			// ProcessingState was killed and re-set by record_batch_processed
			assert_eq!(ProcessingState::<Test>::get().last_replay_request_time, 0);
			assert_eq!(ProcessingState::<Test>::get().last_processed_sequence, 2);

			// Stale missing_indices and deleted_indices were cleared by re-init
			let state = RingCollectionStates::<Test>::get(PEOPLE);
			assert!(state.missing_indices.is_empty());
			assert!(state.deleted_indices.is_empty());
		});
	}

	#[test]
	fn clears_missing_indices_when_all_accounted_for() {
		new_test_ext().execute_with(|| {
			// Init with indices 0, 2 (missing 1), next_ring_index=3
			let batch1 = mock_ring_root_updates_batch(1, 1000, [0, 2], PEOPLE, 3);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch1
			));
			assert!(is_missing(PEOPLE, 1));

			// Recovery: send index 1, making local_count=3 >= next_ring_index=3
			let batch2 = mock_ring_root_updates_batch(1, 1000, [1], PEOPLE, 3);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch2
			));

			assert_eq!(get_ring_count(PEOPLE), 3);
			// All indices in 0..3 accounted for → missing entries cleared
			assert_eq!(total_missing_count(), 0);

			// Ring roots actually stored for all indices
			assert!(has_ring_root(PEOPLE, 0));
			assert!(has_ring_root(PEOPLE, 1));
			assert!(has_ring_root(PEOPLE, 2));
		});
	}
}

mod ring_roots_updates {
	use super::*;

	#[test]
	fn fails_for_none_origin() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();
			let batch = RingRootUpdatesBatch::<Test>::default();
			assert_noop!(
				MembersSubscriber::process_ring_updates(RuntimeOrigin::none(), batch),
				DispatchError::BadOrigin
			);
		});
	}

	#[test]
	fn fails_for_signed_origin() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();
			let batch = RingRootUpdatesBatch::<Test>::default();
			assert_noop!(
				MembersSubscriber::process_ring_updates(RuntimeOrigin::signed(1), batch),
				DispatchError::BadOrigin
			);
		});
	}

	#[test]
	fn fails_when_subscription_inactive() {
		new_test_ext().execute_with(|| {
			// Subscription is Inactive by default
			let batch = RingRootUpdatesBatch::<Test> { sequence: 2, ..Default::default() };
			assert_noop!(
				MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch),
				crate::Error::<Test>::SubscriptionInactive
			);
		});
	}

	#[test]
	fn fails_when_subscription_terminated() {
		new_test_ext().execute_with(|| {
			Subscription::<Test>::put(SubscriptionStatus::Terminated);
			let batch = RingRootUpdatesBatch::<Test> { sequence: 2, ..Default::default() };
			assert_noop!(
				MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch),
				crate::Error::<Test>::SubscriptionTerminated
			);
		});
	}

	#[test]
	fn allows_replay_of_same_batch_sequence() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();
			// last_processed_sequence is 1

			let mut updates = BoundedVec::new();
			updates
				.try_push(RingRootUpdate::<Test> {
					ring_index: 0,
					op: RingRootOp::Built { revision: 1, root: mock_ring_root(42) },
				})
				.unwrap();

			let batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 1, // Same as last processed
				source_time: 1000,
				updates,
				next_ring_index: 0,
			};
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch));

			// Sequence stays the same
			assert_eq!(ProcessingState::<Test>::get().last_processed_sequence, 1);
			// Replay batch is processed — ring root is stored
			assert_eq!(RingRoots::<Test>::iter().count(), 1);
			assert!(has_ring_root(PEOPLE, 0));
		});
	}

	#[test]
	fn ignores_old_batch_sequence() {
		new_test_ext().execute_with(|| {
			Subscription::<Test>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
			ProcessingState::<Test>::mutate(|s| s.last_processed_sequence = 5);

			let batch = RingRootUpdatesBatch::<Test> {
				sequence: 3, // Older than last processed (5)
				..Default::default()
			};
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch));

			// Sequence should not be updated
			assert_eq!(ProcessingState::<Test>::get().last_processed_sequence, 5);
		});
	}

	#[test]
	fn processes_updates_successfully() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			let mut updates = BoundedVec::new();
			for i in 0..3u32 {
				let update = RingRootUpdate::<Test> {
					ring_index: i,
					op: RingRootOp::Built { revision: 2, root: mock_ring_root(i as u64 + 200) },
				};
				updates.try_push(update).unwrap();
			}

			let batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 2,
				source_time: 2000,
				updates,
				next_ring_index: 3,
			};

			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch));

			// Ring roots were stored
			assert_eq!(RingRoots::<Test>::iter().count(), 3);
			for i in 0..3u32 {
				let roots = ring_roots(PEOPLE, i).expect("ring root should exist");
				let record = roots.last().expect("at least one record");
				assert_eq!(record.root, mock_ring_root(i as u64 + 200));
				assert_eq!(record.revision, 2);
				assert_eq!(record.source_time, 2000);
				assert_eq!(record.source_sequence, 2);
			}

			// Sequence was updated
			assert_eq!(ProcessingState::<Test>::get().last_processed_sequence, 2);
		});
	}

	#[test]
	fn removes_recovered_roots_from_missing() {
		new_test_ext().execute_with(|| {
			// Initialize with indices 0, 4 (indices 1, 2, 3 will be missing out of 5)
			let init_batch = mock_ring_root_updates_batch(1, 500, [0, 4], PEOPLE, 5);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				init_batch
			));

			assert_eq!(get_ring_count(PEOPLE), 2);
			assert!(is_missing(PEOPLE, 1));
			assert!(is_missing(PEOPLE, 2));
			assert!(is_missing(PEOPLE, 3));

			// PEOPLE_LITE with a missing entry
			RingCollectionStates::<Test>::insert(
				PEOPLE_LITE,
				make_collection_ring_state(0, 0, &[(5, 0)], &[]),
			);

			// Update recovering index 2
			let mut updates = BoundedVec::new();
			updates
				.try_push(RingRootUpdate::<Test> {
					ring_index: 2,
					op: RingRootOp::Built { revision: 1, root: mock_ring_root(42) },
				})
				.unwrap();

			let batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 2,
				source_time: 2000,
				updates,
				next_ring_index: 5,
			};

			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch));

			// Ring with index 2 should be removed from PEOPLE missing
			assert_eq!(RingCollectionStates::<Test>::get(PEOPLE).missing_indices.len(), 2);
			assert!(is_missing(PEOPLE, 1));
			assert!(!is_missing(PEOPLE, 2)); // Recovered
			assert!(is_missing(PEOPLE, 3));
			// PEOPLE_LITE missing is untouched
			assert!(is_missing(PEOPLE_LITE, 5));
		});
	}

	#[test]
	fn detects_new_missing_rings() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// Batch has ring index 0, 2 but notifier reports 3 total rings
			let mut updates = BoundedVec::new();
			for i in [0u32, 2] {
				let update = RingRootUpdate::<Test> {
					ring_index: i,
					op: RingRootOp::Built { revision: 1, root: mock_ring_root(i as u64) },
				};
				updates.try_push(update).unwrap();
			}

			let batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 2,
				source_time: 2000,
				updates,
				next_ring_index: 3, // Expected 3 rings
			};

			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch));

			// Ring index 1 should be detected as missing
			assert_eq!(total_missing_count(), 1);
			assert!(is_missing(PEOPLE, 1));
		});
	}

	#[test]
	fn updates_last_batch_received_time() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();
			ProcessingState::<Test>::mutate(|s| s.last_batch_received_time = 0);

			let batch = RingRootUpdatesBatch::<Test> { sequence: 2, ..Default::default() };

			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch));

			// MockUnixTime returns 1_700_000_000 seconds
			assert_eq!(ProcessingState::<Test>::get().last_batch_received_time, 1_700_000_000);
		});
	}

	#[test]
	fn updates_collection_ring_count() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();
			// Pre-set some rings count
			RingCollectionStates::<Test>::insert(
				PEOPLE,
				make_collection_ring_state(5, 5, &[], &[]),
			);

			let mut updates = BoundedVec::new();
			// 3 new unique rings
			for i in 5..8u32 {
				let update = RingRootUpdate::<Test> {
					ring_index: i,
					op: RingRootOp::Built { revision: 1, root: mock_ring_root(i as u64) },
				};
				updates.try_push(update).unwrap();
			}

			let batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 2,
				source_time: 2000,
				updates,
				next_ring_index: 8,
			};

			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch));

			// Count should be 5 + 3 = 8
			assert_eq!(get_ring_count(PEOPLE), 8);
		});
	}

	#[test]
	fn collection_ring_count_only_increments_for_new_indices() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// Batch 1: Initial batch with indices 0, 1, 2
			let mut updates1 = BoundedVec::new();
			for i in 0..3u32 {
				updates1
					.try_push(RingRootUpdate::<Test> {
						ring_index: i,
						op: RingRootOp::Built { revision: 1, root: mock_ring_root(i as u64 + 100) },
					})
					.unwrap();
			}

			let batch1 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 2,
				source_time: 1000,
				updates: updates1,
				next_ring_index: 3,
			};
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch1));
			assert_eq!(get_ring_count(PEOPLE), 3);

			// Batch 2: Updates batch with same indices (revisions only, no new rings)
			let mut updates2 = BoundedVec::new();
			for i in 0..2u32 {
				updates2
					.try_push(RingRootUpdate::<Test> {
						ring_index: i,
						op: RingRootOp::Built { revision: 2, root: mock_ring_root(i as u64 + 200) }, // different root
					})
					.unwrap();
			}

			let batch2 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 3,
				source_time: 2000,
				updates: updates2,
				next_ring_index: 3,
			};
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch2));

			// Count should remain 3
			assert_eq!(get_ring_count(PEOPLE), 3);

			// Batch 3: Mix of existing and new indices
			let mut updates3 = BoundedVec::new();
			// Index 2 exists, indices 3 and 4 are new
			for i in 2..5u32 {
				updates3
					.try_push(RingRootUpdate::<Test> {
						ring_index: i,
						op: RingRootOp::Built { revision: 3, root: mock_ring_root(i as u64 + 300) },
					})
					.unwrap();
			}

			let batch3 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 4,
				source_time: 3000,
				updates: updates3,
				next_ring_index: 5,
			};
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch3));

			// Count should be 3 + 2 = 5 (indices 3 and 4 are new)
			assert_eq!(get_ring_count(PEOPLE), 5);

			// All expected roots exist
			for i in 0..5u32 {
				assert!(has_ring_root(PEOPLE, i), "Ring {i} should exist");
			}
		});
	}

	#[test]
	fn deletion_removes_ring_and_decrements_count() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// First, store some rings
			let batch1 = mock_ring_root_updates_batch(2, 1000, 0..3, PEOPLE, 3);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch1));
			assert_eq!(get_ring_count(PEOPLE), 3);
			assert_eq!(RingRoots::<Test>::iter().count(), 3);

			// Now send a deletion (root = None) for ring index 1
			// next_ring_index remains 3 (deletion doesn't shrink the index space)
			let mut updates = BoundedVec::new();
			updates
				.try_push(RingRootUpdate::<Test> { ring_index: 1, op: RingRootOp::Deleted })
				.unwrap();

			let batch2 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 3,
				source_time: 2000,
				updates,
				next_ring_index: 3,
			};

			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch2));

			// Ring 1 should be removed
			assert!(!has_ring_root(PEOPLE, 1));
			// Rings 0 and 2 still exist
			assert!(has_ring_root(PEOPLE, 0));
			assert!(has_ring_root(PEOPLE, 2));
			// Count decremented
			assert_eq!(get_ring_count(PEOPLE), 2);
			// Deleted index tracked
			let state = RingCollectionStates::<Test>::get(PEOPLE);
			assert!(state.deleted_indices.contains(&1));
			// Not falsely marked as missing (Bug 2 fix)
			assert!(!is_missing(PEOPLE, 1));
		});
	}

	#[test]
	fn deletion_of_nonexistent_ring_is_noop() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// Send deletion for a ring that doesn't exist
			let mut updates = BoundedVec::new();
			updates
				.try_push(RingRootUpdate::<Test> { ring_index: 99, op: RingRootOp::Deleted })
				.unwrap();

			let batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 2,
				source_time: 1000,
				updates,
				next_ring_index: 0,
			};

			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch));

			// Count remains 0
			assert_eq!(get_ring_count(PEOPLE), 0);
		});
	}
}

mod subscription_termination {
	use super::*;

	#[test]
	fn fails_for_none_origin() {
		new_test_ext().execute_with(|| {
			Subscription::<Test>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
			assert_noop!(
				MembersSubscriber::terminate_subscription(RuntimeOrigin::none()),
				DispatchError::BadOrigin
			);
		});
	}

	#[test]
	fn fails_for_signed_origin() {
		new_test_ext().execute_with(|| {
			Subscription::<Test>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
			assert_noop!(
				MembersSubscriber::terminate_subscription(RuntimeOrigin::signed(1)),
				DispatchError::BadOrigin
			);
		});
	}

	#[test]
	fn fails_when_subscription_inactive() {
		new_test_ext().execute_with(|| {
			// Subscription is Inactive by default
			assert_noop!(
				MembersSubscriber::terminate_subscription(RuntimeOrigin::root()),
				crate::Error::<Test>::SubscriptionInactive
			);
		});
	}

	#[test]
	fn idempotent_when_already_terminated() {
		new_test_ext().execute_with(|| {
			Subscription::<Test>::put(SubscriptionStatus::Terminated);
			clear_sent_xcms();
			assert_ok!(MembersSubscriber::terminate_subscription(RuntimeOrigin::root()));
			assert!(get_sent_xcms().is_empty());
		});
	}

	#[test]
	fn from_notifier_origin_does_not_send_xcm() {
		new_test_ext().execute_with(|| {
			Subscription::<Test>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
			clear_sent_xcms();

			// Root in tests matches both EnsureNotifierOrigin and EnsureTerminationOrigin,
			// but notifier is tried first, so from_notifier = true
			assert_ok!(MembersSubscriber::terminate_subscription(RuntimeOrigin::root()));
			assert_eq!(Subscription::<Test>::get(), SubscriptionStatus::Terminated);

			// No unsubscribe XCM sent
			assert!(get_sent_xcms().is_empty());
		});
	}

	#[test]
	fn terminate_then_reinitialize() {
		new_test_ext().execute_with(|| {
			// Activating subscription
			Subscription::<Test>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
			let batch = mock_ring_root_updates_batch(1, 1000, 0..3, PEOPLE, 3);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));
			assert_eq!(RingRoots::<Test>::iter().count(), 3);

			// Terminating
			assert_ok!(MembersSubscriber::terminate_subscription(RuntimeOrigin::root()));
			assert_eq!(Subscription::<Test>::get(), SubscriptionStatus::Terminated);
			run_purge_to_completion();
			assert_eq!(RingRoots::<Test>::iter().count(), 0);

			// Re-initializing with new sequence
			let batch = mock_ring_root_updates_batch(10, 2000, 0..2, PEOPLE, 2);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));
			assert_eq!(
				Subscription::<Test>::get(),
				SubscriptionStatus::Active { initialized_at_sequence: 10 }
			);
			assert_eq!(RingRoots::<Test>::iter().count(), 2);
		});
	}

	#[test]
	fn terminate_then_reinit_different_identifier_leaves_no_stale_exponent() {
		new_test_ext().execute_with(|| {
			// First subscription: register PEOPLE with R2e9.
			Subscription::<Test>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
			let batch = mock_ring_root_updates_batch(1, 1000, 0..1, PEOPLE, 1);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				RingExponent::R2e9,
				batch
			));
			assert_eq!(RingCollectionExponents::<Test>::get(PEOPLE), Some(RingExponent::R2e9));

			// Terminate the subscription.
			assert_ok!(MembersSubscriber::terminate_subscription(RuntimeOrigin::root()));
			assert_eq!(Subscription::<Test>::get(), SubscriptionStatus::Terminated);

			// Exponent from the previous subscription must not leak.
			assert!(RingCollectionExponents::<Test>::get(PEOPLE).is_none());
			assert_eq!(RingCollectionExponents::<Test>::iter().count(), 0);

			// New subscription for a different identifier with a different exponent.
			let batch = mock_ring_root_updates_batch(5, 2000, 0..1, PEOPLE_LITE, 1);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				RingExponent::R2e14,
				batch
			));

			// Only the new identifier's exponent is tracked.
			assert!(RingCollectionExponents::<Test>::get(PEOPLE).is_none());
			assert_eq!(
				RingCollectionExponents::<Test>::get(PEOPLE_LITE),
				Some(RingExponent::R2e14)
			);
		});
	}

	#[test]
	fn succeeds_and_sets_terminated_state() {
		new_test_ext().execute_with(|| {
			Subscription::<Test>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });

			// Populate storage to verify it gets cleared
			let batch = mock_ring_root_updates_batch(1, 1000, 0..3, PEOPLE, 5);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));
			assert_eq!(RingRoots::<Test>::iter().count(), 3);
			assert!(ProcessingState::<Test>::get().last_batch_received_time > 0);
			assert!(!RingCollectionStates::<Test>::get(PEOPLE).missing_indices.is_empty());

			assert_ok!(MembersSubscriber::terminate_subscription(RuntimeOrigin::root()));
			assert_eq!(Subscription::<Test>::get(), SubscriptionStatus::Terminated);

			// Small maps cleared inline; ring roots await the purge
			assert!(queued_purge().is_some());
			assert_eq!(RingCollectionStates::<Test>::iter().count(), 0);
			assert_eq!(RingCollectionExponents::<Test>::iter().count(), 0);
			assert_eq!(ProcessingState::<Test>::get(), Default::default());

			// Purge removes all stale ring roots
			run_purge_to_completion();
			assert_eq!(RingRoots::<Test>::iter().count(), 0);
		});
	}
}

mod replay_logic {
	use super::*;

	#[test]
	fn process_collection_replay_noop_when_no_missing() {
		new_test_ext().execute_with(|| {
			clear_sent_xcms();
			let state = make_collection_ring_state(0, 0, &[], &[]);
			RingCollectionStates::<Test>::insert(PEOPLE, state);
			let indices = BTreeSet::new();
			assert!(!Pallet::<Test>::process_collection_replay(PEOPLE, &indices));
			assert!(get_sent_xcms().is_empty());
		});
	}

	#[test]
	fn sends_replay_request_for_missing_indices() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			clear_sent_xcms();

			let state = make_collection_ring_state(0, 0, &[(1, 0), (2, 0)], &[]);
			RingCollectionStates::<Test>::insert(PEOPLE, state);
			let indices = BTreeSet::from([1, 2]);
			assert!(Pallet::<Test>::process_collection_replay(PEOPLE, &indices));

			// XCM sent
			let sent = get_sent_xcms();
			assert_eq!(sent.len(), 1);
		});
	}

	#[test]
	fn sends_per_collection() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			clear_sent_xcms();

			let state1 = make_collection_ring_state(0, 0, &[(1, 0), (3, 0)], &[]);
			let state2 = make_collection_ring_state(0, 0, &[(5, 0), (7, 0)], &[]);
			RingCollectionStates::<Test>::insert(PEOPLE, state1);
			RingCollectionStates::<Test>::insert(PEOPLE_LITE, state2);

			let indices1 = BTreeSet::from([1, 3]);
			let indices2 = BTreeSet::from([5, 7]);
			assert!(Pallet::<Test>::process_collection_replay(PEOPLE, &indices1));
			assert!(Pallet::<Test>::process_collection_replay(PEOPLE_LITE, &indices2));

			// 2 XCMs sent (one per collection)
			let sent = get_sent_xcms();
			assert_eq!(sent.len(), 2);
		});
	}

	#[test]
	fn keeps_index_after_warning_threshold() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			clear_sent_xcms();

			// One missing ring with count just below warning threshold == 4
			// ReplayWarningThreshold == 5
			let state = make_collection_ring_state(0, 0, &[(1, 4)], &[]);
			RingCollectionStates::<Test>::insert(PEOPLE, state);
			let indices = BTreeSet::from([1]);
			assert!(Pallet::<Test>::process_collection_replay(PEOPLE, &indices));

			// XCM sent
			assert_eq!(get_sent_xcms().len(), 1);

			// Index still in missing (warning threshold doesn't remove it)
			let stored = RingCollectionStates::<Test>::get(PEOPLE);
			assert!(stored.missing_indices.contains_key(&1));
		});
	}

	#[test]
	fn abandons_index_after_max_attempts() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			clear_sent_xcms();

			// Missing index with count just below abandon threshold == 9
			// ReplayAbandonThreshold == 10
			let state = make_collection_ring_state(0, 0, &[(1, 9)], &[]);
			RingCollectionStates::<Test>::insert(PEOPLE, state);
			let indices = BTreeSet::from([1]);
			assert!(!Pallet::<Test>::process_collection_replay(PEOPLE, &indices));

			// No XCM sent — index abandoned before sending
			assert_eq!(get_sent_xcms().len(), 0);

			// Index removed from storage after reaching abandon threshold
			let stored = RingCollectionStates::<Test>::get(PEOPLE);
			assert!(!stored.missing_indices.contains_key(&1));
		});
	}
}

mod missing_rings_detection {
	use super::*;

	#[test]
	fn deleted_index_not_falsely_marked_as_missing() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// Subscriber has rings 0, 1, 2
			let batch1 = mock_ring_root_updates_batch(2, 1000, 0..3, PEOPLE, 3);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch1));
			assert_eq!(get_ring_count(PEOPLE), 3);

			// Notifier deletes index 1 and adds index 3
			// Notifier now has {0, 2, 3}, next_ring_index=4
			let mut updates = BoundedVec::new();
			updates
				.try_push(RingRootUpdate::<Test> { ring_index: 1, op: RingRootOp::Deleted })
				.unwrap();
			updates
				.try_push(RingRootUpdate::<Test> {
					ring_index: 3,
					op: RingRootOp::Built { revision: 1, root: mock_ring_root(3) },
				})
				.unwrap();

			let batch2 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 3,
				source_time: 2000,
				updates,
				next_ring_index: 4,
			};

			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch2));

			// Index 1 not marked as missing - it was deleted
			assert!(!is_missing(PEOPLE, 1));
			// Index 1 tracked as deleted
			assert!(RingCollectionStates::<Test>::get(PEOPLE).deleted_indices.contains(&1));
			// No missing rings
			assert_eq!(total_missing_count(), 0);
		});
	}

	#[test]
	fn deletion_with_local_count_less_than_next_ring_index_does_not_false_positive() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// Subscriber has rings 0, 1, 2
			let batch1 = mock_ring_root_updates_batch(2, 1000, 0..3, PEOPLE, 3);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch1));

			// Notifier deletes index 1, next_ring_index stays 3
			// Subscriber: local_count=2 after deletion, deleted_count=1, next_ring_index=3
			let mut updates = BoundedVec::new();
			updates
				.try_push(RingRootUpdate::<Test> { ring_index: 1, op: RingRootOp::Deleted })
				.unwrap();

			let batch2 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 3,
				source_time: 2000,
				updates,
				next_ring_index: 3,
			};

			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch2));

			assert_eq!(get_ring_count(PEOPLE), 2);
			// Index 1 is not missing - it was deleted
			assert!(!is_missing(PEOPLE, 1));
			// Nothing's missing
			assert_eq!(total_missing_count(), 0);
		});
	}

	#[test]
	fn replay_of_deleted_index_does_not_readd_to_missing() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// Subscriber has rings 0, 1, and 2
			let batch1 = mock_ring_root_updates_batch(2, 1000, 0..3, PEOPLE, 3);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch1));

			// Index 1 deleted
			let mut del_updates = BoundedVec::new();
			del_updates
				.try_push(RingRootUpdate::<Test> { ring_index: 1, op: RingRootOp::Deleted })
				.unwrap();

			let del_batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 3,
				source_time: 2000,
				updates: del_updates,
				next_ring_index: 3,
			};
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), del_batch));

			// Simulating replay response: notifier sends root=None, revision=0 for deleted index 1
			let mut replay_updates = BoundedVec::new();
			replay_updates
				.try_push(RingRootUpdate::<Test> { ring_index: 1, op: RingRootOp::Deleted })
				.unwrap();

			let replay_batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 3, // Replay uses the same sequence
				source_time: 2000,
				updates: replay_updates,
				next_ring_index: 3,
			};
			assert_ok!(MembersSubscriber::process_ring_updates(
				RuntimeOrigin::root(),
				replay_batch
			));

			// Index 1 not added to missing
			assert!(!is_missing(PEOPLE, 1));
			assert_eq!(total_missing_count(), 0);
		});
	}

	#[test]
	fn deletion_then_readdition_clears_deleted_tracking() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// Subscriber gets rings 0, 1, and 2
			let batch1 = mock_ring_root_updates_batch(2, 1000, 0..3, PEOPLE, 3);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch1));

			// Index 1 is deleted
			let mut del_updates = BoundedVec::new();
			del_updates
				.try_push(RingRootUpdate::<Test> { ring_index: 1, op: RingRootOp::Deleted })
				.unwrap();
			let del_batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 3,
				source_time: 2000,
				updates: del_updates,
				next_ring_index: 3,
			};
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), del_batch));

			// Subscriber registers ring index 1 as deleted
			assert!(RingCollectionStates::<Test>::get(PEOPLE).deleted_indices.contains(&1));

			// Index 1 added again
			let mut readd_updates = BoundedVec::new();
			readd_updates
				.try_push(RingRootUpdate::<Test> {
					ring_index: 1,
					op: RingRootOp::Built { revision: 3, root: mock_ring_root(999) },
				})
				.unwrap();

			let readd_batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 4,
				source_time: 3000,
				updates: readd_updates,
				next_ring_index: 3,
			};
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), readd_batch));

			// Index 1 no longer in deleted indices
			assert!(!RingCollectionStates::<Test>::get(PEOPLE).deleted_indices.contains(&1));
			// And is correctly stored
			assert!(has_ring_root(PEOPLE, 1));
			assert_eq!(get_ring_count(PEOPLE), 3);
		});
	}

	#[test]
	fn detects_missing_rings_alongside_deletions() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// Subscriber has rings 0, 1, 2
			let batch1 = mock_ring_root_updates_batch(2, 1000, 0..3, PEOPLE, 3);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch1));

			// Notifier deletes index 1, adds indices 3 and 4
			// Notifier has {0, 2, 3, 4}, next_ring_index=5
			// Batch only includes: delete 1, add 3 (index 4 is missing from batch)
			let mut updates = BoundedVec::new();
			updates
				.try_push(RingRootUpdate::<Test> { ring_index: 1, op: RingRootOp::Deleted })
				.unwrap();
			updates
				.try_push(RingRootUpdate::<Test> {
					ring_index: 3,
					op: RingRootOp::Built { revision: 1, root: mock_ring_root(3) },
				})
				.unwrap();

			let batch2 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 3,
				source_time: 2000,
				updates,
				next_ring_index: 5,
			};

			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch2));

			// local_count=3, deleted_count=1, next_ring_index=5 → 3+1=4 < 5 → scan
			// Scan 0..5: index 1 is deleted (skip), index 4 not stored and not deleted → missing

			// Index 1 not missing but deleted
			assert!(!is_missing(PEOPLE, 1));
			assert!(RingCollectionStates::<Test>::get(PEOPLE).deleted_indices.contains(&1));

			// Index 4 registered as missing and not deleted
			assert!(is_missing(PEOPLE, 4));
			assert!(!RingCollectionStates::<Test>::get(PEOPLE).deleted_indices.contains(&4));

			// Only 1 missing ring found
			assert_eq!(total_missing_count(), 1);
		});
	}

	#[test]
	fn next_ring_index_tracks_maximum_across_batches() {
		new_test_ext().execute_with(|| {
			// Batch 1 with next_ring_index=5
			let batch1 = mock_ring_root_updates_batch(1, 1000, 0..5, PEOPLE, 5);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch1
			));
			assert_eq!(RingCollectionStates::<Test>::get(PEOPLE).next_ring_index, 5);

			// Batch 2 (continuation, same sequence) with lower next_ring_index — should not
			// decrease
			let batch2 = mock_ring_root_updates_batch(1, 1000, [2], PEOPLE, 3);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch2
			));

			// next_ring_index remains 5 (max of 5, 3)
			assert_eq!(RingCollectionStates::<Test>::get(PEOPLE).next_ring_index, 5);
		});
	}

	#[test]
	fn unrecorded_deleted_ring_is_tracked_and_not_missing() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// Nothing in the storage

			// A batch with index 0 and index 1 marked as deleted
			let mut updates = BoundedVec::new();
			updates
				.try_push(RingRootUpdate::<Test> {
					ring_index: 0,
					op: RingRootOp::Built { revision: 1, root: mock_ring_root(0) },
				})
				.unwrap();
			updates
				.try_push(RingRootUpdate::<Test> { ring_index: 1, op: RingRootOp::Deleted })
				.unwrap();
			let batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 2,
				source_time: 1000,
				updates,
				next_ring_index: 2,
			};

			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch));

			// Nothing should be recorded as missing
			assert!(!is_missing(PEOPLE, 1));
			assert_eq!(total_missing_count(), 0);

			// Ring 1 recorded as deleted
			assert!(RingCollectionStates::<Test>::get(PEOPLE).deleted_indices.contains(&1));
		});
	}

	#[test]
	fn replay_of_deleted_ring_removes_it_from_missing() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// Subscriber receives only ring 0 but there's 2 rings in the notifier
			let batch1 = mock_ring_root_updates_batch(2, 1000, 0..1, PEOPLE, 2);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch1));

			// Ring 1 recorded as missing
			assert!(is_missing(PEOPLE, 1));

			// Notifier responds to replay request with root None for index 1
			let mut updates = BoundedVec::new();
			updates
				.try_push(RingRootUpdate::<Test> { ring_index: 1, op: RingRootOp::Deleted })
				.unwrap();
			let batch2 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 2, // Replays use the same sequence
				source_time: 2000,
				updates,
				next_ring_index: 2,
			};

			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch2));

			// Ring 1 is removed from missing
			assert!(!is_missing(PEOPLE, 1));

			// and is tracked as deleted
			assert!(RingCollectionStates::<Test>::get(PEOPLE).deleted_indices.contains(&1));
		});
	}
}

mod deleted_indices_overflow {
	use super::*;

	#[test]
	fn skips_scan_when_deleted_indices_at_capacity() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();
			System::set_block_number(1);

			// Deleted indices at capacity
			let deleted: Vec<u32> = (0..MaxDeletedRingsPerCollection::get()).collect();
			RingCollectionStates::<Test>::insert(
				PEOPLE,
				make_collection_ring_state(0, 200, &[], &deleted),
			);

			// Batch to process
			let batch = mock_ring_root_updates_batch(2, 1000, [200], PEOPLE, 210);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch));

			// No missing indices added — scan was skipped
			assert_eq!(RingCollectionStates::<Test>::get(PEOPLE).missing_indices.len(), 0);

			// DeletedIndicesAtCapacity event emitted
			System::assert_has_event(
				Event::<Test>::DeletedIndicesAtCapacity { identifier: PEOPLE }.into(),
			);
		});
	}

	#[test]
	fn deleted_indices_overflow_logs_but_does_not_panic() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// Deleted indices at capacity
			let deleted: Vec<u32> = (0..MaxDeletedRingsPerCollection::get()).collect();
			RingCollectionStates::<Test>::insert(
				PEOPLE,
				make_collection_ring_state(0, 0, &[], &deleted),
			);

			// Deleting one more ring shouldn't cause a panic
			let mut updates = BoundedVec::new();
			updates
				.try_push(RingRootUpdate::<Test> { ring_index: 999, op: RingRootOp::Deleted })
				.unwrap();
			let batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 2,
				source_time: 1000,
				updates,
				next_ring_index: 0,
			};

			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch));

			// Index 999 not tracked (at capacity) but no panic
			assert!(!RingCollectionStates::<Test>::get(PEOPLE).deleted_indices.contains(&999));
		});
	}
}

mod recent_ring_roots {
	use super::*;

	#[test]
	fn stores_single_root() {
		new_test_ext().execute_with(|| {
			let batch = mock_ring_root_updates_batch(1, 1000, [0], PEOPLE, 1);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));

			let roots = ring_roots(PEOPLE, 0).expect("should exist");
			assert_eq!(roots.len(), 1);
			assert_eq!(roots[0].root, mock_ring_root(0));
			assert_eq!(roots[0].revision, 1);
		});
	}

	#[test]
	fn update_pushes_new_root_keeping_previous() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// First root at ring index 0
			let batch1 = mock_ring_root_updates_batch(2, 1000, [0], PEOPLE, 1);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch1));

			let roots = ring_roots(PEOPLE, 0).unwrap();
			assert_eq!(roots.len(), 1);

			// Second root at same ring index with different revision
			let mut updates = BoundedVec::new();
			updates
				.try_push(RingRootUpdate::<Test> {
					ring_index: 0,
					op: RingRootOp::Built { revision: 2, root: mock_ring_root(42) },
				})
				.unwrap();
			let batch2 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 3,
				source_time: 2000,
				updates,
				next_ring_index: 1,
			};
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch2));

			// Both roots should be in the vec (MaxRecentRootsPerRing = 2)
			let roots = ring_roots(PEOPLE, 0).unwrap();
			assert_eq!(roots.len(), 2);
			assert_eq!(roots[0].revision, 1); // Oldest
			assert_eq!(roots[0].root, mock_ring_root(0));
			assert_eq!(roots[1].revision, 2); // Newest
			assert_eq!(roots[1].root, mock_ring_root(42));

			// Ring count should still be 1 (same ring index)
			assert_eq!(get_ring_count(PEOPLE), 1);
		});
	}

	#[test]
	fn evicts_oldest_when_window_full() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// First root (revision 1)
			let batch1 = mock_ring_root_updates_batch(2, 1000, [0], PEOPLE, 1);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch1));

			// Second root (revision 2) — fills window (MaxRecentRootsPerRing = 2)
			let mut updates2 = BoundedVec::new();
			updates2
				.try_push(RingRootUpdate::<Test> {
					ring_index: 0,
					op: RingRootOp::Built { revision: 2, root: mock_ring_root(20) },
				})
				.unwrap();
			let batch2 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 3,
				source_time: 2000,
				updates: updates2,
				next_ring_index: 1,
			};
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch2));

			// Third root (revision 3) — should evict revision 1
			let mut updates3 = BoundedVec::new();
			updates3
				.try_push(RingRootUpdate::<Test> {
					ring_index: 0,
					op: RingRootOp::Built { revision: 3, root: mock_ring_root(30) },
				})
				.unwrap();
			let batch3 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 4,
				source_time: 3000,
				updates: updates3,
				next_ring_index: 1,
			};
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch3));

			let roots = ring_roots(PEOPLE, 0).unwrap();
			assert_eq!(roots.len(), 2);
			// Revision 1 evicted, window now contains revisions 2 and 3
			assert_eq!(roots[0].revision, 2);
			assert_eq!(roots[0].root, mock_ring_root(20));
			assert_eq!(roots[1].revision, 3);
			assert_eq!(roots[1].root, mock_ring_root(30));
		});
	}

	#[test]
	fn deletion_removes_all_recent_roots() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// Two roots at ring index 0
			let batch1 = mock_ring_root_updates_batch(2, 1000, [0], PEOPLE, 1);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch1));

			let mut updates2 = BoundedVec::new();
			updates2
				.try_push(RingRootUpdate::<Test> {
					ring_index: 0,
					op: RingRootOp::Built { revision: 2, root: mock_ring_root(42) },
				})
				.unwrap();
			let batch2 = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 3,
				source_time: 2000,
				updates: updates2,
				next_ring_index: 1,
			};
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch2));
			assert_eq!(ring_roots(PEOPLE, 0).unwrap().len(), 2);

			// Deleting the ring removes the entire BoundedVec
			let mut del_updates = BoundedVec::new();
			del_updates
				.try_push(RingRootUpdate::<Test> { ring_index: 0, op: RingRootOp::Deleted })
				.unwrap();
			let del_batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 4,
				source_time: 3000,
				updates: del_updates,
				next_ring_index: 1,
			};
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), del_batch));

			assert!(!has_ring_root(PEOPLE, 0));
			assert_eq!(get_ring_count(PEOPLE), 0);
		});
	}

	#[test]
	fn readdition_after_deletion_starts_fresh_window() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// Add, then delete ring index 0
			let batch1 = mock_ring_root_updates_batch(2, 1000, [0], PEOPLE, 1);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch1));

			let mut del = BoundedVec::new();
			del.try_push(RingRootUpdate::<Test> { ring_index: 0, op: RingRootOp::Deleted })
				.unwrap();
			let del_batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 3,
				source_time: 2000,
				updates: del,
				next_ring_index: 1,
			};
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), del_batch));
			assert!(!has_ring_root(PEOPLE, 0));

			// Re-adding starts with fresh window of length 1
			let mut readd = BoundedVec::new();
			readd
				.try_push(RingRootUpdate::<Test> {
					ring_index: 0,
					op: RingRootOp::Built { revision: 5, root: mock_ring_root(99) },
				})
				.unwrap();
			let readd_batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 4,
				source_time: 3000,
				updates: readd,
				next_ring_index: 1,
			};
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), readd_batch));

			let roots = ring_roots(PEOPLE, 0).unwrap();
			assert_eq!(roots.len(), 1);
			assert_eq!(roots[0].revision, 5);
			assert_eq!(get_ring_count(PEOPLE), 1);
		});
	}
}

mod offchain_worker {
	use super::*;

	fn setup_active_with_missing(missing: &[(u32, u32)]) {
		Subscription::<Test>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
		ProcessingState::<Test>::mutate(|s| {
			s.last_processed_sequence = 1;
			// Batch received long ago so cooldown is satisfied
			s.last_batch_received_time = 0;
			s.last_replay_request_time = 0;
		});
		RingCollectionStates::<Test>::insert(
			PEOPLE,
			make_collection_ring_state(0, 0, missing, &[]),
		);
		// Advancing time well past cooldown
		set_time_secs(1_700_000_000);
	}

	#[test]
	fn skips_when_subscription_inactive() {
		new_test_ext().execute_with(|| {
			// Subscription is Inactive by default.
			Pallet::<Test>::offchain_worker(1);
			assert_eq!(pending_ocw_tx_count(), 0);
		});
	}

	#[test]
	fn skips_when_no_missing_indices() {
		new_test_ext().execute_with(|| {
			Subscription::<Test>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
			ProcessingState::<Test>::mutate(|s| {
				s.last_batch_received_time = 0;
				s.last_replay_request_time = 0;
			});
			set_time_secs(1_700_000_000);

			Pallet::<Test>::offchain_worker(1);
			assert_eq!(pending_ocw_tx_count(), 0);
		});
	}

	#[test]
	fn skips_during_batch_cooldown() {
		new_test_ext().execute_with(|| {
			Subscription::<Test>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
			RingCollectionStates::<Test>::insert(
				PEOPLE,
				make_collection_ring_state(0, 0, &[(1, 0)], &[]),
			);
			// Batch received very recently
			let now_secs = 1_700_000_000u64;
			set_time_secs(now_secs);
			ProcessingState::<Test>::mutate(|s| {
				s.last_batch_received_time = now_secs; // Within cooldown
				s.last_replay_request_time = 0;
			});

			Pallet::<Test>::offchain_worker(1);
			assert_eq!(pending_ocw_tx_count(), 0);
		});
	}

	#[test]
	fn skips_during_replay_cooldown() {
		new_test_ext().execute_with(|| {
			Subscription::<Test>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
			RingCollectionStates::<Test>::insert(
				PEOPLE,
				make_collection_ring_state(0, 0, &[(1, 0)], &[]),
			);
			let now_secs = 1_700_000_000u64;
			set_time_secs(now_secs);
			ProcessingState::<Test>::mutate(|s| {
				s.last_batch_received_time = 0;
				s.last_replay_request_time = now_secs; // Within cooldown
			});

			Pallet::<Test>::offchain_worker(1);
			assert_eq!(pending_ocw_tx_count(), 0);
		});
	}

	#[test]
	fn submits_replay_transaction_when_missing_indices_exist() {
		new_test_ext().execute_with(|| {
			setup_active_with_missing(&[(1, 0), (3, 0)]);

			Pallet::<Test>::offchain_worker(1);

			assert_eq!(pending_ocw_tx_count(), 1);
		});
	}

	#[test]
	fn submits_replay_for_each_collection() {
		new_test_ext().execute_with(|| {
			setup_active_with_missing(&[(1, 0)]);
			RingCollectionStates::<Test>::insert(
				PEOPLE_LITE,
				make_collection_ring_state(0, 0, &[(5, 0)], &[]),
			);

			Pallet::<Test>::offchain_worker(1);

			// One transaction per collection.
			assert_eq!(pending_ocw_tx_count(), 2);
		});
	}

	#[test]
	fn submitted_transaction_executes_and_sends_xcm() {
		new_test_ext().execute_with(|| {
			setup_active_with_missing(&[(1, 0), (3, 0)]);
			clear_sent_xcms();

			// OCW submits transaction.
			Pallet::<Test>::offchain_worker(1);
			assert_eq!(pending_ocw_tx_count(), 1);

			// Executing the submitted transaction.
			drain_ocw_transactions();

			// XCM replay request sent to notifier.
			assert!(!get_sent_xcms().is_empty());
		});
	}
}

mod proof_verification {
	use super::*;
	use crate::pallet::Error;
	use indiv_support::traits::{MembershipMultiProver, MembershipProver, RingMembershipProof};

	const CTX: crate::types::Context = [9u8; 32];
	const MSG: &[u8] = b"msg";
	const RING: u32 = 0;

	/// Build a `TestProof` that validates against a ring root containing `seed`.
	fn proof_for(seed: u64) -> TestProof {
		proof_for_with(seed, &[seed], CTX, MSG)
	}

	fn proof_for_with(
		seed: u64,
		members: &[u64],
		context: crate::types::Context,
		message: &[u8],
	) -> TestProof {
		TestProof {
			context: context.to_vec(),
			member: TestMemberKey(seed),
			members: members.to_vec(),
			message: message.to_vec(),
		}
	}

	fn root_for(seeds: &[u64]) -> crate::mock::TestMembers {
		let mut members = crate::mock::TestMembers::default();
		for seed in seeds {
			members.try_push(*seed).unwrap();
		}
		members
	}

	/// Seed an active subscription with an exponent and a single root in the window.
	fn seed_single_root(identifier: Identifier, revision: u32, seed: u64) {
		seed_root(identifier, revision, root_for(&[seed]));
	}

	fn seed_root(identifier: Identifier, revision: u32, root: crate::mock::TestMembers) {
		Subscription::<Test>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
		RingCollectionExponents::<Test>::insert(identifier, TEST_RING_EXPONENT);
		let batch = RingRootUpdatesBatch::<Test> {
			identifier,
			sequence: revision as u64,
			source_time: now_secs(),
			updates: bounded_vec![RingRootUpdate {
				ring_index: RING,
				op: RingRootOp::Built { revision, root },
			}],
			next_ring_index: 1,
		};
		Pallet::<Test>::store_ring_roots(&batch);
	}

	/// Push one more revision onto an existing ring's sliding window with the current
	/// mock time as source time.
	fn push_revision(identifier: Identifier, revision: u32, seed: u64) {
		let batch = RingRootUpdatesBatch::<Test> {
			identifier,
			sequence: revision as u64,
			source_time: now_secs(),
			updates: bounded_vec![RingRootUpdate {
				ring_index: RING,
				op: RingRootOp::Built { revision, root: mock_ring_root(seed) },
			}],
			next_ring_index: 1,
		};
		Pallet::<Test>::store_ring_roots(&batch);
	}

	#[test]
	fn verify_membership_succeeds_against_latest_root() {
		new_test_ext().execute_with(|| {
			seed_single_root(PEOPLE, 1, 42);

			let ca = Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(42), RING, 1, CTX, MSG)
				.unwrap();
			assert_eq!(ca.context, CTX);
		});
	}

	#[test]
	fn verify_membership_accepts_older_root_still_in_window() {
		new_test_ext().execute_with(|| {
			// Window holds up to MaxRecentRootsPerRing = 2.
			seed_single_root(PEOPLE, 1, 42);
			push_revision(PEOPLE, 2, 99);

			// A proof built against revision 1 still validates when that revision is specified.
			let ca = Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(42), RING, 1, CTX, MSG)
				.unwrap();
			assert_eq!(ca.context, CTX);

			// A proof built against the newest revision also validates.
			let ca = Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(99), RING, 2, CTX, MSG)
				.unwrap();
			assert_eq!(ca.context, CTX);
		});
	}

	#[test]
	fn verify_membership_rejects_invalid_proof_for_present_revision() {
		new_test_ext().execute_with(|| {
			// Revision 1 is present, but the proof was built against a different alias.
			seed_single_root(PEOPLE, 1, 42);

			assert_noop!(
				Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(99), RING, 1, CTX, MSG),
				Error::<Test>::InvalidProof,
			);
		});
	}

	#[test]
	fn verify_membership_returns_revision_not_found_for_evicted_root() {
		new_test_ext().execute_with(|| {
			// Fill then overflow the window so revision 1 is evicted.
			seed_single_root(PEOPLE, 1, 42);
			push_revision(PEOPLE, 2, 99);
			push_revision(PEOPLE, 3, 7);

			// Sanity: only the two most recent revisions are retained.
			let roots = ring_roots(PEOPLE, RING).unwrap();
			assert_eq!(roots.len(), 2);
			assert_eq!(roots[0].revision, 2);
			assert_eq!(roots[1].revision, 3);

			assert_noop!(
				Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(42), RING, 1, CTX, MSG),
				Error::<Test>::RevisionNotFound,
			);
		});
	}

	#[test]
	fn verify_membership_succeeds_for_in_window_revision() {
		new_test_ext().execute_with(|| {
			seed_single_root(PEOPLE, 1, 42);
			push_revision(PEOPLE, 2, 99);

			let ca = Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(42), RING, 1, CTX, MSG)
				.unwrap();
			assert_eq!(ca.context, CTX);
		});
	}

	#[test]
	fn verify_membership_returns_revision_not_found_for_evicted() {
		new_test_ext().execute_with(|| {
			seed_single_root(PEOPLE, 1, 42);
			push_revision(PEOPLE, 2, 99);
			push_revision(PEOPLE, 3, 7);

			assert_noop!(
				Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(42), RING, 1, CTX, MSG),
				Error::<Test>::RevisionNotFound,
			);
		});
	}

	#[test]
	fn verify_memberships_in_ring_matches_specified_revision() {
		new_test_ext().execute_with(|| {
			seed_single_root(PEOPLE, 1, 42);
			push_revision(PEOPLE, 2, 99);

			// Valid batch against revision 1.
			let items = vec![RingMembershipProof {
				proof: proof_for(42),
				context: CTX.to_vec(),
				message: MSG.to_vec(),
			}];
			let out = Pallet::<Test>::verify_memberships_in_ring(&PEOPLE, RING, 1, &items).unwrap();
			assert_eq!(out.len(), 1);

			// Asking for revision 2 with a proof built against revision 1 fails.
			assert_noop!(
				Pallet::<Test>::verify_memberships_in_ring(&PEOPLE, RING, 2, &items),
				Error::<Test>::InvalidProof,
			);

			// Unknown revision returns RevisionNotFound.
			assert_noop!(
				Pallet::<Test>::verify_memberships_in_ring(&PEOPLE, RING, 99, &items),
				Error::<Test>::RevisionNotFound,
			);
		});
	}

	// Verifies batch verification returns one alias per proof and preserves input order.
	#[test]
	fn verify_memberships_in_ring_preserves_input_order() {
		new_test_ext().execute_with(|| {
			let members = [42, 99];
			seed_root(PEOPLE, 1, root_for(&members));

			let items = members
				.iter()
				.map(|&member| RingMembershipProof {
					proof: proof_for_with(member, &members, CTX, MSG),
					context: CTX.to_vec(),
					message: MSG.to_vec(),
				})
				.collect::<Vec<_>>();

			let out = Pallet::<Test>::verify_memberships_in_ring(&PEOPLE, RING, 1, &items).unwrap();
			assert_eq!(out.len(), 2);
			assert_eq!(out[0].alias, items[0].proof.alias());
			assert_eq!(out[1].alias, items[1].proof.alias());
			assert_eq!(out[0].context, CTX);
			assert_eq!(out[1].context, CTX);
		});
	}

	// Verifies a single invalid proof rejects the entire batch even after a valid item.
	#[test]
	fn verify_memberships_in_ring_fails_when_any_proof_is_invalid() {
		new_test_ext().execute_with(|| {
			seed_single_root(PEOPLE, 1, 42);

			// Second item's proof doesn't match the stored root.
			let items = vec![
				RingMembershipProof {
					proof: proof_for(42),
					context: CTX.to_vec(),
					message: MSG.to_vec(),
				},
				RingMembershipProof {
					proof: proof_for(99),
					context: CTX.to_vec(),
					message: MSG.to_vec(),
				},
			];

			assert_noop!(
				Pallet::<Test>::verify_memberships_in_ring(&PEOPLE, RING, 1, &items),
				Error::<Test>::InvalidProof,
			);
		});
	}

	#[test]
	fn ring_revision_returns_newest_in_window() {
		new_test_ext().execute_with(|| {
			assert_eq!(Pallet::<Test>::ring_revision(&PEOPLE, RING), None);

			seed_single_root(PEOPLE, 1, 42);
			assert_eq!(Pallet::<Test>::ring_revision(&PEOPLE, RING), Some(1));

			push_revision(PEOPLE, 2, 99);
			assert_eq!(Pallet::<Test>::ring_revision(&PEOPLE, RING), Some(2));
		});
	}

	#[test]
	fn is_revision_valid_reflects_window_contents() {
		new_test_ext().execute_with(|| {
			// No ring stored yet.
			assert!(!Pallet::<Test>::is_revision_valid(&PEOPLE, RING, 1));
			assert!(!Pallet::<Test>::is_revision_valid(&PEOPLE, RING, 2));
			assert!(!Pallet::<Test>::is_revision_valid(&PEOPLE, RING, 3));

			seed_single_root(PEOPLE, 1, 42);
			push_revision(PEOPLE, 2, 99);
			assert!(Pallet::<Test>::is_revision_valid(&PEOPLE, RING, 1));
			assert!(Pallet::<Test>::is_revision_valid(&PEOPLE, RING, 2));
			assert!(!Pallet::<Test>::is_revision_valid(&PEOPLE, RING, 3));

			// Once evicted, the old revision is no longer valid.
			push_revision(PEOPLE, 3, 7);
			assert!(!Pallet::<Test>::is_revision_valid(&PEOPLE, RING, 1));
			assert!(Pallet::<Test>::is_revision_valid(&PEOPLE, RING, 2));
			assert!(Pallet::<Test>::is_revision_valid(&PEOPLE, RING, 3));
		});
	}

	#[test]
	fn verify_membership_rejects_superseded_revision_past_retention() {
		new_test_ext().execute_with(|| {
			let start = 1_700_000_000;
			set_time_secs(start);
			seed_single_root(PEOPLE, 1, 42);
			// Revision 2 supersedes revision 1 at `start`.
			push_revision(PEOPLE, 2, 99);

			// Just before the retention deadline the superseded revision still verifies.
			set_time_secs(start + OldRootRetentionDuration::get() - 1);
			let ca = Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(42), RING, 1, CTX, MSG)
				.unwrap();
			assert_eq!(ca.context, CTX);

			// At the deadline it is rejected even though it is still in the window.
			set_time_secs(start + OldRootRetentionDuration::get());
			assert_eq!(ring_roots(PEOPLE, RING).unwrap().len(), 2);
			assert_noop!(
				Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(42), RING, 1, CTX, MSG),
				Error::<Test>::RevisionExpired,
			);

			// The newest revision keeps verifying regardless of age.
			let ca = Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(99), RING, 2, CTX, MSG)
				.unwrap();
			assert_eq!(ca.context, CTX);
		});
	}

	#[test]
	fn verify_memberships_in_ring_rejects_superseded_revision_past_retention() {
		new_test_ext().execute_with(|| {
			let start = 1_700_000_000;
			set_time_secs(start);
			seed_single_root(PEOPLE, 1, 42);
			push_revision(PEOPLE, 2, 99);

			let items = vec![RingMembershipProof {
				proof: proof_for(42),
				context: CTX.to_vec(),
				message: MSG.to_vec(),
			}];
			let out = Pallet::<Test>::verify_memberships_in_ring(&PEOPLE, RING, 1, &items).unwrap();
			assert_eq!(out.len(), 1);

			set_time_secs(start + OldRootRetentionDuration::get());
			assert_noop!(
				Pallet::<Test>::verify_memberships_in_ring(&PEOPLE, RING, 1, &items),
				Error::<Test>::RevisionExpired,
			);
		});
	}

	#[test]
	fn is_revision_valid_expires_superseded_revision() {
		new_test_ext().execute_with(|| {
			let start = 1_700_000_000;
			set_time_secs(start);
			seed_single_root(PEOPLE, 1, 42);
			push_revision(PEOPLE, 2, 99);

			set_time_secs(start + OldRootRetentionDuration::get() - 1);
			assert!(Pallet::<Test>::is_revision_valid(&PEOPLE, RING, 1));
			assert!(Pallet::<Test>::is_revision_valid(&PEOPLE, RING, 2));

			set_time_secs(start + OldRootRetentionDuration::get());
			assert!(!Pallet::<Test>::is_revision_valid(&PEOPLE, RING, 1));
			assert!(Pallet::<Test>::is_revision_valid(&PEOPLE, RING, 2));
		});
	}

	#[test]
	fn verify_membership_multi_context_matches_specified_revision() {
		new_test_ext().execute_with(|| {
			seed_single_root(PEOPLE, 1, 42);
			push_revision(PEOPLE, 2, 99);

			// Valid proof against revision 1.
			let out = Pallet::<Test>::verify_membership_multi_context(
				&PEOPLE,
				&proof_for(42),
				RING,
				1,
				&[CTX],
				MSG,
			)
			.unwrap();
			assert_eq!(out.len(), 1);

			// Asking for revision 2 with a proof built against revision 1 fails.
			assert_noop!(
				Pallet::<Test>::verify_membership_multi_context(
					&PEOPLE,
					&proof_for(42),
					RING,
					2,
					&[CTX],
					MSG
				),
				Error::<Test>::InvalidProof,
			);
		});
	}

	#[test]
	fn collection_not_found_when_no_exponent_stored() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(42), RING, 0, CTX, MSG),
				Error::<Test>::CollectionNotFound,
			);
		});
	}

	#[test]
	fn no_root_when_exponent_stored_but_ring_absent() {
		new_test_ext().execute_with(|| {
			RingCollectionExponents::<Test>::insert(PEOPLE, TEST_RING_EXPONENT);
			assert_noop!(
				Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(42), RING, 0, CTX, MSG),
				Error::<Test>::NoRoot,
			);
		});
	}
}

mod scan_cap_and_weights {
	use super::*;
	use crate::{pallet::Call, weights::WeightInfo};
	use frame_support::dispatch::GetDispatchInfo;

	#[test]
	fn charge_is_capped_as_next_ring_index_grows() {
		new_test_ext().execute_with(|| {
			let cap = MaxGapScanPerBatch::get();
			let batch_at_cap = mock_ring_root_updates_batch(2, 1000, [0], PEOPLE, cap);
			let batch_at_max = mock_ring_root_updates_batch(2, 1000, [0], PEOPLE, u32::MAX);

			// Declared weight is identical at the cap and far beyond it.
			let update_at_cap = Call::<Test>::process_ring_updates { batch: batch_at_cap.clone() }
				.get_dispatch_info()
				.call_weight;
			let update_at_max = Call::<Test>::process_ring_updates { batch: batch_at_max.clone() }
				.get_dispatch_info()
				.call_weight;
			assert_eq!(update_at_cap, update_at_max);

			let init_at_cap = Call::<Test>::initialize_ring_roots {
				ring_exponent: TEST_RING_EXPONENT,
				roots: batch_at_cap,
			}
			.get_dispatch_info()
			.call_weight;
			let init_at_max = Call::<Test>::initialize_ring_roots {
				ring_exponent: TEST_RING_EXPONENT,
				roots: batch_at_max,
			}
			.get_dispatch_info()
			.call_weight;
			assert_eq!(init_at_cap, init_at_max);
		});
	}

	#[test]
	fn process_ring_updates_refunds_to_scanned_count() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();
			// Scan already caught up to the frontier, so only one new index is examined.
			let mut state = make_collection_ring_state(0, 999, &[], &[]);
			state.next_scan_index = 999;
			RingCollectionStates::<Test>::insert(PEOPLE, state);

			let batch = mock_ring_root_updates_batch(2, 1000, [999], PEOPLE, 1000);
			let charged = Call::<Test>::process_ring_updates { batch: batch.clone() }
				.get_dispatch_info()
				.call_weight;

			// Subscriber processes a batch whose counter exceeds the scan cap.
			let post =
				MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch).unwrap();

			// Actual weight is priced on the single scanned index and undercuts the capped charge.
			let actual = post.actual_weight.unwrap();
			let expected = <() as WeightInfo>::process_ring_updates(1)
				.saturating_add(<() as WeightInfo>::detect_missing_in_range(1));
			assert_eq!(actual, expected);
			assert!(actual.all_lt(charged));
		});
	}

	#[test]
	fn charge_covers_a_lagging_cursor_under_a_small_batch_counter() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();
			// Frontier sits far ahead of the scan cursor after an earlier jump.
			RingCollectionStates::<Test>::insert(
				PEOPLE,
				make_collection_ring_state(1, 1000, &[], &[]),
			);

			// A later batch reports a counter far below the stored frontier.
			let batch = mock_ring_root_updates_batch(2, 1000, [], PEOPLE, 2);
			let charged = Call::<Test>::process_ring_updates { batch: batch.clone() }
				.get_dispatch_info()
				.call_weight;

			let post =
				MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch).unwrap();

			// The scan still walked a page from the cursor, and the charge covered it.
			assert_eq!(next_scan_index(PEOPLE), MaxGapScanPerBatch::get());
			assert!(post.actual_weight.unwrap().all_lte(charged));
		});
	}

	#[test]
	fn scan_stops_within_the_cap() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// Batch pushes the frontier far past the scan cap with no rings stored.
			let batch = mock_ring_root_updates_batch(2, 1000, [], PEOPLE, 10_000);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch));

			// Exactly one page is examined, and the cursor stops at its end.
			assert_eq!(
				RingCollectionStates::<Test>::get(PEOPLE).missing_indices.len(),
				MaxGapScanPerBatch::get() as usize,
			);
			assert_eq!(next_scan_index(PEOPLE), MaxGapScanPerBatch::get());
		});
	}

	#[test]
	fn scan_resumes_past_the_cap_on_later_batches() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();
			let cap = MaxGapScanPerBatch::get();

			// Frontier jumps several pages past the scan cap with no rings stored.
			let batch = mock_ring_root_updates_batch(2, 1000, [], PEOPLE, 3 * cap);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch));

			// First page records one gap per scanned index and parks the cursor at its end.
			assert_eq!(
				RingCollectionStates::<Test>::get(PEOPLE).missing_indices.len(),
				cap as usize,
			);
			assert_eq!(next_scan_index(PEOPLE), cap);

			// A later batch at the same frontier continues from the cursor.
			let batch = mock_ring_root_updates_batch(3, 2000, [], PEOPLE, 3 * cap);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch));

			// Gaps beyond the first page are detected instead of being skipped for good.
			let state = RingCollectionStates::<Test>::get(PEOPLE);
			assert_eq!(state.missing_indices.len(), 2 * cap as usize);
			assert!(state.missing_indices.contains_key(&cap));
			assert_eq!(state.next_scan_index, 2 * cap);
		});
	}

	#[test]
	fn capacity_break_leaves_the_cursor_in_place() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();
			// Missing tracking full with indices above the scan window
			let missing = (10_000..10_000 + MaxMissingRootsPerCollection::get())
				.map(|idx| (idx, 0))
				.collect::<Vec<_>>();
			RingCollectionStates::<Test>::insert(
				PEOPLE,
				make_collection_ring_state(0, 0, &missing, &[]),
			);

			let batch = mock_ring_root_updates_batch(2, 1000, [], PEOPLE, 100);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch));

			// Index 0 could not be recorded, so the next scan retries it.
			assert_eq!(next_scan_index(PEOPLE), 0);
			assert!(!is_missing(PEOPLE, 0));
		});
	}

	#[test]
	fn all_accounted_for_advances_the_cursor() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// Every index below the frontier arrives in the batch.
			let batch = mock_ring_root_updates_batch(2, 1000, 0..3, PEOPLE, 3);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch));

			// The scan is caught up without examining any index.
			assert_eq!(next_scan_index(PEOPLE), 3);
			assert!(RingCollectionStates::<Test>::get(PEOPLE).missing_indices.is_empty());
		});
	}

	#[test]
	fn frontier_covers_indices_above_the_batch_counter() {
		new_test_ext().execute_with(|| {
			setup_active_subscription();

			// Notifier delivers an index at or above the counter it reports.
			let batch = mock_ring_root_updates_batch(2, 1000, [5], PEOPLE, 1);
			assert_ok!(MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch));

			// Frontier covers the delivered index, so the gaps below it stay detectable.
			assert_eq!(RingCollectionStates::<Test>::get(PEOPLE).next_ring_index, 6);
			assert!(is_missing(PEOPLE, 0));
			assert!(!is_missing(PEOPLE, 5));
		});
	}

	#[test]
	fn initialize_refunds_clear_cost_when_not_reinitializing() {
		new_test_ext().execute_with(|| {
			// Ring 1 arrives and ring 0 does not, so the scan examines both indices.
			let batch = mock_ring_root_updates_batch(1, 1000, [1], PEOPLE, 2);
			let charged = Call::<Test>::initialize_ring_roots {
				ring_exponent: TEST_RING_EXPONENT,
				roots: batch.clone(),
			}
			.get_dispatch_info()
			.call_weight;

			// Fresh initialization takes the cheap branch without the clear.
			let post = MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch,
			)
			.unwrap();

			let actual = post.actual_weight.unwrap();
			let expected = <() as WeightInfo>::initialize_ring_roots(1)
				.saturating_add(<() as WeightInfo>::detect_missing_in_range(2));
			assert_eq!(actual, expected);
			assert!(actual.all_lt(charged));
		});
	}

	#[test]
	fn initialize_charges_clear_cost_when_reinitializing() {
		new_test_ext().execute_with(|| {
			let batch = mock_ring_root_updates_batch(1, 1000, [0], PEOPLE, 1);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));

			// Re-initialization with a higher sequence takes the clearing branch.
			let batch = mock_ring_root_updates_batch(2, 2000, [0], PEOPLE, 1);
			let post = MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch,
			)
			.unwrap();

			// The only ring is delivered, so the scan is caught up without examining an index.
			let expected = <() as WeightInfo>::initialize_ring_roots(1)
				.saturating_add(<() as WeightInfo>::detect_missing_in_range(0))
				.saturating_add(<() as WeightInfo>::clear_ring_data());
			assert_eq!(post.actual_weight.unwrap(), expected);
		});
	}
}

mod generation_and_purge {
	use super::*;
	use crate::{pallet::Call, weights::WeightInfo};
	use frame_support::{dispatch::GetDispatchInfo, traits::Authorize};
	use indiv_support::traits::MembershipProver;
	use sp_runtime::transaction_validity::{
		InvalidTransaction, TransactionSource, TransactionValidityError,
	};

	fn authorized_origin() -> RuntimeOrigin {
		RuntimeOrigin::from(frame_system::RawOrigin::Authorized)
	}

	fn insert_ring_entry(identifier: Identifier, ring_index: u32, generation: u32) {
		let mut roots = BoundedVec::new();
		roots
			.try_push(RingCommitmentRecord {
				root: mock_ring_root(ring_index as u64),
				revision: 1,
				source_time: 1000,
				source_sequence: 1,
			})
			.unwrap();
		RingRoots::<Test>::insert((generation, identifier, ring_index), roots);
	}

	#[test]
	fn reinit_hides_old_roots_immediately() {
		new_test_ext().execute_with(|| {
			let batch = mock_ring_root_updates_batch(1, 1000, [0], PEOPLE, 1);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));
			assert_eq!(Pallet::<Test>::ring_revision(&PEOPLE, 0), Some(1));

			// Notifier re-initializes with a higher sequence and a different ring.
			let batch = mock_ring_root_updates_batch(2, 2000, [5], PEOPLE, 6);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));

			// Old root is physically stored under the stale prefix but invisible to the prover.
			assert!(!has_ring_root(PEOPLE, 0));
			assert_eq!(RingRoots::<Test>::iter().count(), 2);
			assert_eq!(Pallet::<Test>::ring_revision(&PEOPLE, 0), None);
			assert!(!Pallet::<Test>::is_revision_valid(&PEOPLE, 0, 1));
			assert_eq!(Pallet::<Test>::ring_revision(&PEOPLE, 5), Some(1));
		});
	}

	#[test]
	fn built_over_stale_entry_resets_window() {
		new_test_ext().execute_with(|| {
			let batch = mock_ring_root_updates_batch(1, 1000, [0], PEOPLE, 1);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));
			assert_ok!(MembersSubscriber::terminate_subscription(RuntimeOrigin::root()));

			// Fresh initialization writes the same ring index over the stale entry.
			let mut updates: BoundedVec<RingRootUpdate<Test>, MaxUpdatesPerBatch> =
				BoundedVec::new();
			updates
				.try_push(RingRootUpdate {
					ring_index: 0,
					op: RingRootOp::Built { revision: 5, root: mock_ring_root(99) },
				})
				.unwrap();
			let batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 10,
				source_time: 2000,
				updates,
				next_ring_index: 1,
			};
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));

			// The window restarts instead of appending across generations.
			let roots = ring_roots(PEOPLE, 0).unwrap();
			assert_eq!(roots.len(), 1);
			assert_eq!(roots[0].revision, 5);
			assert_eq!(get_ring_count(PEOPLE), 1);
		});
	}

	#[test]
	fn deleted_op_on_stale_entry_does_not_underflow_ring_count() {
		new_test_ext().execute_with(|| {
			let batch = mock_ring_root_updates_batch(1, 1000, [0], PEOPLE, 1);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));
			assert_ok!(MembersSubscriber::terminate_subscription(RuntimeOrigin::root()));

			// Fresh initialization deletes a ring index that only exists as a stale entry.
			let mut updates: BoundedVec<RingRootUpdate<Test>, MaxUpdatesPerBatch> =
				BoundedVec::new();
			updates
				.try_push(RingRootUpdate { ring_index: 0, op: RingRootOp::Deleted })
				.unwrap();
			let batch = RingRootUpdatesBatch::<Test> {
				identifier: PEOPLE,
				sequence: 10,
				source_time: 2000,
				updates,
				next_ring_index: 1,
			};
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));

			// The delete finds nothing under the live prefix and ring_count stays at zero.
			assert!(!has_ring_root(PEOPLE, 0));
			assert_eq!(get_ring_count(PEOPLE), 0);

			// The stale entry is unreachable until the purge removes it.
			run_purge_to_completion();
			assert_eq!(RingRoots::<Test>::iter().count(), 0);
		});
	}

	#[test]
	fn detect_reflags_stale_entries_as_missing() {
		new_test_ext().execute_with(|| {
			let batch = mock_ring_root_updates_batch(1, 1000, [0, 1], PEOPLE, 2);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));
			assert_ok!(MembersSubscriber::terminate_subscription(RuntimeOrigin::root()));

			// Fresh initialization restores only ring 1 of the previous two.
			let batch = mock_ring_root_updates_batch(10, 2000, [1], PEOPLE, 2);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				batch
			));

			// The stale entry at index 0 counts as missing again.
			assert!(is_missing(PEOPLE, 0));
			assert!(!is_missing(PEOPLE, 1));
		});
	}

	#[test]
	fn purge_removes_only_stale_entries() {
		new_test_ext().execute_with(|| {
			insert_ring_entry(PEOPLE, 1, 0);
			insert_ring_entry(PEOPLE, 2, 0);
			insert_ring_entry(PEOPLE, 3, 1);
			CurrentGeneration::<Test>::put(1);
			queue_purge(0, 0);

			// A single page removes both stale entries and completes the purge.
			assert_ok!(MembersSubscriber::purge_stale_ring_roots(authorized_origin()));

			assert_eq!(RingRoots::<Test>::iter().count(), 1);
			assert!(has_ring_root(PEOPLE, 3));
			assert!(queued_purge().is_none());
		});
	}

	#[test]
	fn purge_pages_advance_until_done() {
		// `clear_prefix` counts backend deletions only, so entries must be committed out of
		// the overlay for the page limit to bite.
		let mut ext = new_test_ext();
		let page = PurgePageSize::get();
		let total = 2 * page + 50;

		// Two full pages and a remainder are stored under a stale generation.
		ext.execute_with(|| {
			for i in 0..total {
				insert_ring_entry(PEOPLE, i, 0);
			}
			CurrentGeneration::<Test>::put(1);
			queue_purge(0, 0);
		});
		ext.commit_all().expect("commit overlay to backend");

		// First page removes exactly one page and advances the page counter.
		ext.execute_with(|| {
			assert_ok!(MembersSubscriber::purge_stale_ring_roots(authorized_origin()));
			assert_eq!(queued_purge(), Some((0, 1)));
			assert_eq!(RingRoots::<Test>::iter().count(), (total - page) as usize);
		});
		ext.commit_all().expect("commit deletions to backend");

		// The next page resumes at the first surviving key instead of restarting.
		ext.execute_with(|| {
			assert_ok!(MembersSubscriber::purge_stale_ring_roots(authorized_origin()));
			assert_eq!(queued_purge(), Some((0, 2)));
			assert_eq!(RingRoots::<Test>::iter().count(), 50);
		});
		ext.commit_all().expect("commit deletions to backend");

		// The short final page empties the prefix and completes the purge.
		ext.execute_with(|| {
			assert_ok!(MembersSubscriber::purge_stale_ring_roots(authorized_origin()));
			assert_eq!(RingRoots::<Test>::iter().count(), 0);
			assert!(queued_purge().is_none());
		});
	}

	#[test]
	fn purge_advances_to_the_next_stale_generation() {
		new_test_ext().execute_with(|| {
			// Two clears in a row leave two stale generations behind.
			insert_ring_entry(PEOPLE, 0, 0);
			CurrentGeneration::<Test>::put(1);
			insert_ring_entry(PEOPLE, 1, 1);
			CurrentGeneration::<Test>::put(2);
			insert_ring_entry(PEOPLE, 2, 2);
			queue_purge(0, 0);

			// The first sweep finishes generation 0 and queues generation 1.
			assert_ok!(MembersSubscriber::purge_stale_ring_roots(authorized_origin()));
			assert_eq!(queued_purge(), Some((1, 0)));

			// Every stale generation is removed and the live one survives.
			run_purge_to_completion();
			assert_eq!(RingRoots::<Test>::iter().count(), 1);
			assert!(has_ring_root(PEOPLE, 2));
		});
	}

	#[test]
	fn purge_refunds_to_removed_count() {
		// The refund is priced on backend removals, so the entries must leave the overlay.
		let mut ext = new_test_ext();

		// Three stale entries, well under a full page.
		ext.execute_with(|| {
			insert_ring_entry(PEOPLE, 0, 0);
			insert_ring_entry(PEOPLE, 1, 0);
			insert_ring_entry(PEOPLE, 2, 0);
			CurrentGeneration::<Test>::put(1);
			queue_purge(0, 0);
		});
		ext.commit_all().expect("commit overlay to backend");

		ext.execute_with(|| {
			let charged = Call::<Test>::purge_stale_ring_roots {}.get_dispatch_info().call_weight;

			// A partial page refunds down to the number of entries removed.
			let post = MembersSubscriber::purge_stale_ring_roots(authorized_origin()).unwrap();
			let actual = post.actual_weight.unwrap();
			assert_eq!(actual, <() as WeightInfo>::purge_stale_ring_roots(3));
			assert!(actual.all_lt(charged));
		});
	}

	#[test]
	fn ocw_submits_purge_while_terminated() {
		new_test_ext().execute_with(|| {
			Subscription::<Test>::put(SubscriptionStatus::Terminated);
			CurrentGeneration::<Test>::put(1);
			queue_purge(0, 0);

			// Offchain worker runs on a terminated subscription.
			Pallet::<Test>::offchain_worker(1);

			assert_eq!(pending_ocw_tx_count(), 1);
		});
	}

	#[test]
	fn ocw_skips_purge_when_none_pending() {
		new_test_ext().execute_with(|| {
			Subscription::<Test>::put(SubscriptionStatus::Terminated);

			Pallet::<Test>::offchain_worker(1);

			assert_eq!(pending_ocw_tx_count(), 0);
		});
	}

	#[test]
	fn authorize_purge_rejects_external_source() {
		new_test_ext().execute_with(|| {
			CurrentGeneration::<Test>::put(1);
			queue_purge(0, 0);
			let call = Call::<Test>::purge_stale_ring_roots {};

			let result = call.authorize(TransactionSource::External).unwrap();

			assert_eq!(
				result.unwrap_err(),
				TransactionValidityError::from(InvalidTransaction::Call)
			);
		});
	}

	#[test]
	fn authorize_purge_rejects_when_nothing_to_purge() {
		new_test_ext().execute_with(|| {
			let call = Call::<Test>::purge_stale_ring_roots {};

			let result = call.authorize(TransactionSource::InBlock).unwrap();

			assert_eq!(
				result.unwrap_err(),
				TransactionValidityError::from(InvalidTransaction::Custom(0))
			);
		});
	}

	#[test]
	fn authorize_purge_tags_each_page() {
		new_test_ext().execute_with(|| {
			let call = Call::<Test>::purge_stale_ring_roots {};
			// Two clears happened, so generations 0 and 1 are both stale.
			CurrentGeneration::<Test>::put(2);

			// The first page of the first stale generation.
			queue_purge(0, 0);
			let first = call.authorize(TransactionSource::InBlock).unwrap().unwrap().0;

			// A later page of the same generation is a distinct pool entry.
			queue_purge(0, 1);
			let next_page = call.authorize(TransactionSource::InBlock).unwrap().unwrap().0;

			// So is the first page of the next stale generation.
			queue_purge(1, 0);
			let next_generation = call.authorize(TransactionSource::InBlock).unwrap().unwrap().0;

			assert_ne!(first.provides, next_page.provides);
			assert_ne!(first.provides, next_generation.provides);
		});
	}

	#[test]
	fn authorize_purge_rejects_the_live_generation() {
		new_test_ext().execute_with(|| {
			// A live entry sits in generation 0, and a purge is queued for that same
			// generation.
			insert_ring_entry(PEOPLE, 0, 0);
			queue_purge(0, 0);
			let call = Call::<Test>::purge_stale_ring_roots {};

			let result = call.authorize(TransactionSource::InBlock).unwrap();

			// The call never reaches dispatch, so the live entry cannot be cleared.
			assert_eq!(
				result.unwrap_err(),
				TransactionValidityError::from(InvalidTransaction::Custom(1))
			);
			assert!(RingRoots::<Test>::contains_key((0, PEOPLE, 0)));
			assert_eq!(queued_purge(), Some((0, 0)));
		});
	}
}

mod collection_bound {
	use super::*;
	use crate::{pallet::Call, weights::WeightInfo, Error};
	use frame_support::{
		dispatch::{DispatchResultWithPostInfo, GetDispatchInfo},
		traits::Get,
	};

	fn identifier(n: u8) -> Identifier {
		[n; 32]
	}

	fn collection_count() -> usize {
		RingCollectionExponents::<Test>::iter_keys().count()
	}

	fn init_collection(n: u8) -> DispatchResultWithPostInfo {
		MembersSubscriber::initialize_ring_roots(
			RuntimeOrigin::root(),
			TEST_RING_EXPONENT,
			mock_ring_root_updates_batch(1, 1000, [0], identifier(n), 1),
		)
	}

	#[test]
	fn rejects_a_collection_past_max_collections() {
		new_test_ext().execute_with(|| {
			// Notifier initializes collections up to the bound.
			for n in 0..MaxCollections::get() as u8 {
				assert_ok!(init_collection(n));
			}
			assert_eq!(collection_count(), MaxCollections::get() as usize);

			// One collection past the bound is refused.
			assert_noop!(
				init_collection(MaxCollections::get() as u8),
				Error::<Test>::TooManyCollections
			);
			assert_eq!(collection_count(), MaxCollections::get() as usize);
		});
	}

	#[test]
	fn re_initializing_a_known_collection_does_not_count_twice() {
		new_test_ext().execute_with(|| {
			assert_ok!(init_collection(0));

			// A later part of the same initialization repeats the identifier.
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				mock_ring_root_updates_batch(1, 1000, [1], identifier(0), 2)
			));

			assert_eq!(collection_count(), 1);
		});
	}

	#[test]
	fn clearing_ring_data_frees_the_bound_again() {
		new_test_ext().execute_with(|| {
			for n in 0..MaxCollections::get() as u8 {
				assert_ok!(init_collection(n));
			}

			// Notifier re-initializes with a higher sequence, which wipes every collection.
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				mock_ring_root_updates_batch(2, 2000, [0], identifier(200), 1)
			));

			// Only the re-initialized collection counts, so the bound has room again.
			assert_eq!(collection_count(), 1);
			assert_ok!(MembersSubscriber::initialize_ring_roots(
				RuntimeOrigin::root(),
				TEST_RING_EXPONENT,
				mock_ring_root_updates_batch(2, 2000, [0], identifier(201), 1)
			));
			assert_eq!(collection_count(), 2);
		});
	}

	#[test]
	fn updates_for_an_uninitialized_collection_are_refused() {
		new_test_ext().execute_with(|| {
			// Active subscription, but this collection never went through initialization.
			Subscription::<Test>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
			ProcessingState::<Test>::mutate(|s| s.last_processed_sequence = 1);
			let unknown = identifier(9);

			let batch = mock_ring_root_updates_batch(2, 2000, [0], unknown, 1);
			let charged = Call::<Test>::process_ring_updates { batch: batch.clone() }
				.get_dispatch_info()
				.call_weight;
			let post =
				MembersSubscriber::process_ring_updates(RuntimeOrigin::root(), batch).unwrap();

			// No state is created, so the collection cannot consume the bound.
			assert!(!RingCollectionStates::<Test>::contains_key(unknown));
			assert!(!has_ring_root(unknown, 0));
			assert_eq!(collection_count(), 0);

			// Weight refunds to the early return plus the exponent read.
			let actual = post.actual_weight.unwrap();
			let db: frame_support::weights::RuntimeDbWeight =
				<Test as frame_system::Config>::DbWeight::get();
			let expected =
				<() as WeightInfo>::process_ring_updates_stale_batch().saturating_add(db.reads(1));
			assert_eq!(actual, expected);
			assert!(actual.all_lt(charged));
		});
	}
}
