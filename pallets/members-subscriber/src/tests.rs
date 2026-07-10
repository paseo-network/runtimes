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
		Event, ProcessingState, RingCollectionExponents, RingCollectionStates, RingRoots,
		Subscription,
	},
	types::{
		Identifier, RingCollectionState, RingRootOp, RingRootUpdate, RingRootUpdatesBatch,
		SubscriptionStatus,
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
	}
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

fn setup_active_subscription() {
	Subscription::<Test>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
	ProcessingState::<Test>::mutate(|s| s.last_processed_sequence = 1);
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
			assert!(RingRoots::<Test>::get(PEOPLE, 0).is_some());
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

			// Old ring root cleared, new one stored.
			assert!(RingRoots::<Test>::get(PEOPLE, 0).is_none());
			assert!(RingRoots::<Test>::get(PEOPLE, 5).is_some());
			assert_eq!(RingCollectionStates::<Test>::get(PEOPLE).ring_count, 1);

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
				let roots = RingRoots::<Test>::get(PEOPLE, i).expect("ring root should exist");
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

			// Old rings cleared, only new 3 exist
			assert_eq!(RingRoots::<Test>::iter().count(), 3);
			assert_eq!(get_ring_count(PEOPLE), 3);

			// Old rings (0-4) don't exist
			for i in 0..5u32 {
				assert!(RingRoots::<Test>::get(PEOPLE, i).is_none());
			}

			// New rings (10-12) exist
			for i in 10..13u32 {
				assert!(RingRoots::<Test>::get(PEOPLE, i).is_some());
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
			assert!(RingRoots::<Test>::contains_key(PEOPLE, 0));
			assert!(RingRoots::<Test>::contains_key(PEOPLE, 1));
			assert!(RingRoots::<Test>::contains_key(PEOPLE, 2));
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
			assert!(RingRoots::<Test>::contains_key(PEOPLE, 0));
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
				let roots = RingRoots::<Test>::get(PEOPLE, i).expect("ring root should exist");
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
				assert!(RingRoots::<Test>::contains_key(PEOPLE, i), "Ring {i} should exist");
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
			assert!(RingRoots::<Test>::get(PEOPLE, 1).is_none());
			// Rings 0 and 2 still exist
			assert!(RingRoots::<Test>::get(PEOPLE, 0).is_some());
			assert!(RingRoots::<Test>::get(PEOPLE, 2).is_some());
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

			// All ring data should be cleared
			assert_eq!(RingRoots::<Test>::iter().count(), 0);
			assert_eq!(RingCollectionStates::<Test>::iter().count(), 0);
			assert_eq!(RingCollectionExponents::<Test>::iter().count(), 0);
			assert_eq!(ProcessingState::<Test>::get(), Default::default());
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
			assert!(RingRoots::<Test>::get(PEOPLE, 1).is_some());
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
			let batch2 = mock_ring_root_updates_batch(1, 1000, [5], PEOPLE, 3);
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

			let roots = RingRoots::<Test>::get(PEOPLE, 0).expect("should exist");
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

			let roots = RingRoots::<Test>::get(PEOPLE, 0).unwrap();
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
			let roots = RingRoots::<Test>::get(PEOPLE, 0).unwrap();
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

			let roots = RingRoots::<Test>::get(PEOPLE, 0).unwrap();
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
			assert_eq!(RingRoots::<Test>::get(PEOPLE, 0).unwrap().len(), 2);

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

			assert!(RingRoots::<Test>::get(PEOPLE, 0).is_none());
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
			assert!(RingRoots::<Test>::get(PEOPLE, 0).is_none());

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

			let roots = RingRoots::<Test>::get(PEOPLE, 0).unwrap();
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
	use indiv_support::traits::{BatchProofItem, MembershipProver};

	const CTX: crate::types::Context = [9u8; 32];
	const MSG: &[u8] = b"msg";
	const RING: u32 = 0;

	/// Build a `TestProof` that validates against a ring root containing `seed`.
	fn proof_for(seed: u64) -> TestProof {
		TestProof {
			context: CTX.to_vec(),
			member: TestMemberKey(seed),
			members: vec![seed],
			message: MSG.to_vec(),
		}
	}

	/// Seed an active subscription with an exponent and a single root in the window.
	fn seed_single_root(identifier: Identifier, revision: u32, seed: u64) {
		Subscription::<Test>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
		RingCollectionExponents::<Test>::insert(identifier, TEST_RING_EXPONENT);
		let batch = RingRootUpdatesBatch::<Test> {
			identifier,
			sequence: revision as u64,
			source_time: 1000,
			updates: bounded_vec![RingRootUpdate {
				ring_index: RING,
				op: RingRootOp::Built { revision, root: mock_ring_root(seed) },
			}],
			next_ring_index: 1,
		};
		Pallet::<Test>::store_ring_roots(&batch);
	}

	/// Push one more revision onto an existing ring's sliding window.
	fn push_revision(identifier: Identifier, revision: u32, seed: u64) {
		let batch = RingRootUpdatesBatch::<Test> {
			identifier,
			sequence: revision as u64,
			source_time: 1000,
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

			let rca =
				Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(42), RING, CTX, MSG).unwrap();

			assert_eq!(rca.revision, 1);
			assert_eq!(rca.ring, RING);
			assert_eq!(rca.ca.context, CTX);
		});
	}

	#[test]
	fn verify_membership_accepts_older_root_still_in_window() {
		new_test_ext().execute_with(|| {
			// Window holds up to MaxRecentRootsPerRing = 2.
			seed_single_root(PEOPLE, 1, 42);
			push_revision(PEOPLE, 2, 99);

			// A proof built against revision 1 still validates and its revision is reported back.
			let rca =
				Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(42), RING, CTX, MSG).unwrap();
			assert_eq!(rca.revision, 1);

			// A proof built against the newest revision also validates.
			let rca =
				Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(99), RING, CTX, MSG).unwrap();
			assert_eq!(rca.revision, 2);
		});
	}

	#[test]
	fn verify_membership_rejects_proof_built_against_evicted_root() {
		new_test_ext().execute_with(|| {
			// Fill then overflow the window so revision 1 is evicted.
			seed_single_root(PEOPLE, 1, 42);
			push_revision(PEOPLE, 2, 99);
			push_revision(PEOPLE, 3, 7);

			// Sanity: only the two most recent revisions are retained.
			let roots = RingRoots::<Test>::get(PEOPLE, RING).unwrap();
			assert_eq!(roots.len(), 2);
			assert_eq!(roots[0].revision, 2);
			assert_eq!(roots[1].revision, 3);

			assert_noop!(
				Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(42), RING, CTX, MSG),
				Error::<Test>::InvalidProof,
			);
		});
	}

	#[test]
	fn verify_membership_at_rev_succeeds_for_in_window_revision() {
		new_test_ext().execute_with(|| {
			seed_single_root(PEOPLE, 1, 42);
			push_revision(PEOPLE, 2, 99);

			let ca = Pallet::<Test>::verify_membership_at_rev(
				&PEOPLE,
				&proof_for(42),
				RING,
				1,
				CTX,
				MSG,
			)
			.unwrap();
			assert_eq!(ca.context, CTX);
		});
	}

	#[test]
	fn verify_membership_at_rev_returns_revision_not_found_for_evicted() {
		new_test_ext().execute_with(|| {
			seed_single_root(PEOPLE, 1, 42);
			push_revision(PEOPLE, 2, 99);
			push_revision(PEOPLE, 3, 7);

			assert_noop!(
				Pallet::<Test>::verify_membership_at_rev(
					&PEOPLE,
					&proof_for(42),
					RING,
					1,
					CTX,
					MSG
				),
				Error::<Test>::RevisionNotFound,
			);
		});
	}

	#[test]
	fn verify_memberships_in_ring_preserves_order_and_returns_revision() {
		new_test_ext().execute_with(|| {
			seed_single_root(PEOPLE, 1, 42);

			let items = [b"m1", b"m2"]
				.iter()
				.map(|&msg| BatchProofItem {
					proof: TestProof {
						context: CTX.to_vec(),
						member: TestMemberKey(42),
						members: vec![42],
						message: msg.to_vec(),
					},
					context: CTX.to_vec(),
					message: msg.to_vec(),
				})
				.collect::<Vec<_>>();

			let out = Pallet::<Test>::verify_memberships_in_ring(&PEOPLE, RING, &items).unwrap();
			assert_eq!(out.len(), 2);
			assert!(out.iter().all(|rca| rca.revision == 1 && rca.ring == RING));
		});
	}

	#[test]
	fn verify_memberships_in_ring_fails_when_any_proof_is_invalid() {
		new_test_ext().execute_with(|| {
			seed_single_root(PEOPLE, 1, 42);

			// Second item's proof doesn't match the stored root.
			let items = vec![
				BatchProofItem {
					proof: proof_for(42),
					context: CTX.to_vec(),
					message: MSG.to_vec(),
				},
				BatchProofItem {
					proof: proof_for(99),
					context: CTX.to_vec(),
					message: MSG.to_vec(),
				},
			];

			assert_noop!(
				Pallet::<Test>::verify_memberships_in_ring(&PEOPLE, RING, &items),
				Error::<Test>::InvalidProof,
			);
		});
	}

	#[test]
	fn verify_memberships_in_ring_at_rev_matches_specified_revision() {
		new_test_ext().execute_with(|| {
			seed_single_root(PEOPLE, 1, 42);
			push_revision(PEOPLE, 2, 99);

			// Valid batch against revision 1.
			let items = vec![BatchProofItem {
				proof: proof_for(42),
				context: CTX.to_vec(),
				message: MSG.to_vec(),
			}];
			let out = Pallet::<Test>::verify_memberships_in_ring_at_rev(&PEOPLE, RING, 1, &items)
				.unwrap();
			assert_eq!(out.len(), 1);

			// Asking for revision 2 with a proof built against revision 1 fails.
			assert_noop!(
				Pallet::<Test>::verify_memberships_in_ring_at_rev(&PEOPLE, RING, 2, &items),
				Error::<Test>::InvalidProof,
			);

			// Unknown revision returns RevisionNotFound.
			assert_noop!(
				Pallet::<Test>::verify_memberships_in_ring_at_rev(&PEOPLE, RING, 99, &items),
				Error::<Test>::RevisionNotFound,
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
	fn collection_not_found_when_no_exponent_stored() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(42), RING, CTX, MSG),
				Error::<Test>::CollectionNotFound,
			);
		});
	}

	#[test]
	fn no_root_when_exponent_stored_but_ring_absent() {
		new_test_ext().execute_with(|| {
			RingCollectionExponents::<Test>::insert(PEOPLE, TEST_RING_EXPONENT);
			assert_noop!(
				Pallet::<Test>::verify_membership(&PEOPLE, &proof_for(42), RING, CTX, MSG),
				Error::<Test>::NoRoot,
			);
		});
	}
}
