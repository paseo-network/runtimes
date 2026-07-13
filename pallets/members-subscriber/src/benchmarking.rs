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

//! Members subscriber pallet benchmarking.

use super::*;
use crate::{
	pallet::{
		ProcessingState, RingCollectionExponents, RingCollectionStates, RingRoots, Subscription,
	},
	types::{
		Identifier, MembersOf, RingCollectionState, RingIndex, RingRootOp, RingRootUpdate,
		RingRootUpdatesBatch, SubscriptionStatus,
	},
};
use frame_benchmarking::{v2::*, BenchmarkError};
use frame_support::{
	pallet_prelude::BoundedBTreeMap,
	traits::{Authorize, EnsureOrigin, Get},
	BoundedVec,
};
use frame_system::RawOrigin as SystemOrigin;
use indiv_support::traits::RingExponent;
use sp_runtime::transaction_validity::TransactionSource;

const BENCH_IDENTIFIER: Identifier = [0u8; 32];

pub trait BenchmarkHelper<T: Config> {
	/// Initializes runtime state needed for benchmarks (e.g. timestamp, HRMP channels).
	fn init() {}
	/// Creates a mock ring root (`MembersOf<T>`).
	fn mock_ring_root(seed: u32) -> MembersOf<T>;
}

/// Fills in a ring root record `RingRoot` with rings.
fn fill_in_ring_roots<T: Config + BenchmarkHelper<T>>(
	identifier: Identifier,
	ring_index: RingIndex,
	base_seed: u32,
) {
	use crate::types::RingCommitmentRecord;

	let max_recent = T::MaxRecentRootsPerRing::get();
	let mut roots = BoundedVec::new();
	for j in 0..max_recent {
		roots
			.try_push(RingCommitmentRecord {
				root: T::mock_ring_root(base_seed.wrapping_add(j)),
				revision: j + 1,
				source_time: 1000,
				source_sequence: 1,
			})
			.expect("within MaxRecentRootsPerRing bound");
	}
	RingRoots::<T>::insert(identifier, ring_index, roots);
}

#[benchmarks(where T: BenchmarkHelper<T>)]
mod benches {
	use super::*;

	#[benchmark]
	fn initialize_ring_roots(
		n: Linear<1, { T::MaxUpdatesPerBatch::get() }>,
	) -> Result<(), BenchmarkError> {
		T::init();

		// Initializing from Terminated state (storage already cleared during termination)
		Subscription::<T>::put(SubscriptionStatus::Terminated);

		// Setting next_ring_index so that detect_missing_rings_in_batch has no delta
		// inside the extrinsic.
		// The detect_missing_rings_in_batch cost is accounted for separately.
		RingCollectionStates::<T>::insert(
			BENCH_IDENTIFIER,
			RingCollectionState { next_ring_index: n, ..Default::default() },
		);

		let mut updates = BoundedVec::new();
		for i in 0..n {
			let update = RingRootUpdate::<T> {
				ring_index: i as RingIndex,
				op: RingRootOp::Built { revision: 1, root: T::mock_ring_root(i) },
			};
			updates.try_push(update).expect("updates ok");
		}
		let batch = RingRootUpdatesBatch::<T> {
			identifier: BENCH_IDENTIFIER,
			sequence: 5,
			source_time: 2000,
			updates,
			next_ring_index: n,
		};

		let origin = T::EnsureNotifierOrigin::try_successful_origin()
			.map_err(|_| BenchmarkError::Stop("failed to construct notifier origin"))?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, RingExponent::R2e9, batch);

		assert_eq!(ProcessingState::<T>::get().last_processed_sequence, 5);
		assert_eq!(RingRoots::<T>::iter().count(), n as usize);

		Ok(())
	}

	#[benchmark]
	fn process_ring_updates(
		n: Linear<1, { T::MaxUpdatesPerBatch::get() }>,
	) -> Result<(), BenchmarkError> {
		T::init();

		Subscription::<T>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
		ProcessingState::<T>::mutate(|s| s.last_processed_sequence = 1);

		// Pre-populating RingRoots to full capacity
		for i in 0..n {
			fill_in_ring_roots::<T>(BENCH_IDENTIFIER, i as RingIndex, i);
		}

		// Missing indices for the collection
		let mut missing_rings = BoundedBTreeMap::<_, _, T::MaxMissingRootsPerCollection>::new();
		for i in 0..n {
			missing_rings.try_insert(i, 0u32).expect("within bounds");
		}
		RingCollectionStates::<T>::insert(
			BENCH_IDENTIFIER,
			RingCollectionState {
				ring_count: n,
				next_ring_index: n,
				missing_indices: missing_rings,
				..Default::default()
			},
		);

		// Updates for a single collection
		let mut updates = BoundedVec::new();
		for i in 0..n {
			let update = RingRootUpdate::<T> {
				ring_index: i as RingIndex,
				op: RingRootOp::Built { revision: 1, root: T::mock_ring_root(i) },
			};
			updates.try_push(update).expect("updates ok");
		}

		let batch = RingRootUpdatesBatch::<T> {
			identifier: BENCH_IDENTIFIER,
			sequence: 2,
			source_time: 2000,
			updates,
			next_ring_index: n,
		};

		let origin = T::EnsureNotifierOrigin::try_successful_origin()
			.map_err(|_| BenchmarkError::Stop("failed to construct notifier origin"))?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, batch);

		assert_eq!(ProcessingState::<T>::get().last_processed_sequence, 2);
		assert_eq!(RingRoots::<T>::iter().count(), n as usize);

		// missing_indices were cleared
		assert!(RingCollectionStates::<T>::get(BENCH_IDENTIFIER).missing_indices.is_empty());

		Ok(())
	}

	/// Stale-batch early-return path in `process_ring_updates`. Sequence is older
	/// than `last_processed_sequence`, so the extrinsic refunds back to just the
	/// two storage reads (`Subscription`, `ProcessingState`) without doing any
	/// processing work.
	#[benchmark]
	fn process_ring_updates_stale_batch() -> Result<(), BenchmarkError> {
		T::init();

		Subscription::<T>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
		ProcessingState::<T>::mutate(|s| s.last_processed_sequence = 5);

		let batch = RingRootUpdatesBatch::<T> {
			identifier: BENCH_IDENTIFIER,
			sequence: 1, // older than `last_processed_sequence` → stale
			source_time: 0,
			updates: BoundedVec::new(),
			next_ring_index: 0,
		};

		let origin = T::EnsureNotifierOrigin::try_successful_origin()
			.map_err(|_| BenchmarkError::Stop("failed to construct notifier origin"))?;

		#[extrinsic_call]
		process_ring_updates(origin as T::RuntimeOrigin, batch);

		// Nothing changed — early-return left state untouched.
		assert_eq!(ProcessingState::<T>::get().last_processed_sequence, 5);

		Ok(())
	}

	#[benchmark]
	fn terminate_subscription() -> Result<(), BenchmarkError> {
		T::init();

		Subscription::<T>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });

		// Worst-case: populate all storage that needs clearing across all collections
		let max_roots = T::MaxRingRootsPerCollection::get();
		let max_collections = T::MaxCollections::get();

		for c in 0..max_collections {
			let mut identifier = [0u8; 32];
			identifier[..4].copy_from_slice(&c.to_le_bytes());

			for i in 0..max_roots {
				fill_in_ring_roots::<T>(identifier, i as RingIndex, i);
			}

			let mut missing = BoundedBTreeMap::<_, _, T::MaxMissingRootsPerCollection>::new();
			missing.try_insert(0, 0u32).expect("within bounds");
			RingCollectionStates::<T>::insert(
				identifier,
				RingCollectionState {
					ring_count: max_roots,
					next_ring_index: max_roots,
					missing_indices: missing,
					..Default::default()
				},
			);
			RingCollectionExponents::<T>::insert(identifier, RingExponent::R2e9);
		}

		ProcessingState::<T>::mutate(|s| {
			s.last_processed_sequence = 1;
			s.last_batch_received_time = 1000;
			s.last_replay_request_time = 500;
		});

		#[extrinsic_call]
		_(SystemOrigin::Root);

		assert_eq!(Subscription::<T>::get(), SubscriptionStatus::Terminated);
		assert_eq!(RingRoots::<T>::iter().count(), 0);
		assert_eq!(RingCollectionStates::<T>::iter().count(), 0);
		assert_eq!(RingCollectionExponents::<T>::iter().count(), 0);
		assert_eq!(ProcessingState::<T>::get(), Default::default());

		Ok(())
	}

	/// Benchmark for the full `replay_missing_roots` extrinsic: BTreeSet conversion,
	/// retain filter, `process_collection_replay` helper, and merge-back into storage.
	/// Worst case: all provided indices match stored missing indices.
	#[benchmark]
	fn replay_missing_roots(
		n: Linear<1, { T::MaxMissingRootsPerCollection::get() }>,
	) -> Result<(), BenchmarkError> {
		T::init();

		Subscription::<T>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });

		let mut missing = BoundedBTreeMap::<_, _, T::MaxMissingRootsPerCollection>::new();
		for i in 0..n {
			missing.try_insert(i, 0u32).expect("within bounds");
		}
		RingCollectionStates::<T>::insert(
			BENCH_IDENTIFIER,
			RingCollectionState::<T::MaxMissingRootsPerCollection, T::MaxDeletedRingsPerCollection> {
				ring_count: 0,
				next_ring_index: n,
				missing_indices: missing,
				..Default::default()
			},
		);

		let indices: BoundedVec<_, T::MaxMissingRootsPerCollection> =
			(0..n).collect::<Vec<_>>().try_into().expect("within bounds");

		#[extrinsic_call]
		_(SystemOrigin::Authorized, BENCH_IDENTIFIER, indices);

		// XCM sent and replay timestamp updated
		assert!(ProcessingState::<T>::get().last_replay_request_time > 0);

		// Verifying first chunk was sent
		let first_chunk = n.min(T::MaxUpdatesPerBatch::get());
		frame_system::Pallet::<T>::assert_has_event(
			Event::<T>::ReplayRequestSent {
				identifier: BENCH_IDENTIFIER,
				indices_count: first_chunk,
			}
			.into(),
		);

		Ok(())
	}

	#[benchmark]
	fn authorize_replay_missing_roots(
		n: Linear<1, { T::MaxMissingRootsPerCollection::get() }>,
	) -> Result<(), BenchmarkError> {
		T::init();

		Subscription::<T>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });

		// Worst case for `any()`: only the last provided index (n-1) matches the missing_index
		let mut missing = BoundedBTreeMap::<_, _, T::MaxMissingRootsPerCollection>::new();
		// n-1 entries outside the provided range [0..n)
		for i in n..(2 * n - 1) {
			missing.try_insert(i, 0u32).expect("within bounds");
		}
		// Single matching entry at the last provided index
		missing.try_insert(n - 1, 0u32).expect("within bounds");
		RingCollectionStates::<T>::insert(
			BENCH_IDENTIFIER,
			RingCollectionState {
				ring_count: 0,
				next_ring_index: 2 * n,
				missing_indices: missing,
				..Default::default()
			},
		);

		let indices: BoundedVec<_, T::MaxMissingRootsPerCollection> =
			(0..n).collect::<Vec<_>>().try_into().expect("within bounds");

		let call = Call::<T>::replay_missing_roots { identifier: BENCH_IDENTIFIER, indices };

		#[block]
		{
			call.authorize(TransactionSource::InBlock).unwrap().unwrap();
		}

		Ok(())
	}

	#[benchmark]
	fn send_replay_request() -> Result<(), BenchmarkError> {
		T::init();

		// MaxUpdatesPerBatch indices per chunk
		let indices: Vec<RingIndex> = (0..T::MaxUpdatesPerBatch::get()).collect();

		#[block]
		{
			Pallet::<T>::send_replay_request(BENCH_IDENTIFIER, &indices)
				.expect("XCM send should succeed");
		}

		Ok(())
	}

	/// Benchmark for the `contains_key` loop in `detect_missing_rings_in_batch`.
	/// Measures cost of scanning a range of `n` indices.
	#[benchmark]
	fn detect_missing_in_range(
		n: Linear<1, { T::MaxUpdatesPerBatch::get() }>,
	) -> Result<(), BenchmarkError> {
		T::init();

		Subscription::<T>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
		ProcessingState::<T>::mutate(|s| s.last_processed_sequence = 1);

		// Pre-populate some ring roots so contains_key lookups are realistic
		for i in 0..n {
			fill_in_ring_roots::<T>(BENCH_IDENTIFIER, i as RingIndex, i);
		}

		RingCollectionStates::<T>::insert(
			BENCH_IDENTIFIER,
			RingCollectionState { ring_count: n, next_ring_index: n, ..Default::default() },
		);

		// Batch extends the range by n, triggering scan of n new indices
		let mut updates = BoundedVec::new();
		updates
			.try_push(RingRootUpdate::<T> {
				ring_index: n,
				op: RingRootOp::Built { revision: 1, root: T::mock_ring_root(n + 100) },
			})
			.expect("updates ok");

		let batch = RingRootUpdatesBatch::<T> {
			identifier: BENCH_IDENTIFIER,
			sequence: 2,
			source_time: 2000,
			updates,
			// Extends next_ring_index by n beyond current, forcing a scan of n indices
			next_ring_index: n.saturating_mul(2),
		};

		let old_next_ring_index = n;
		Pallet::<T>::store_ring_roots(&batch);

		#[block]
		{
			Pallet::<T>::detect_missing_rings_in_batch(&batch, old_next_ring_index);
		}

		// Indices n+1..2n-1 are missing (n-1 total)
		assert_eq!(
			RingCollectionStates::<T>::get(BENCH_IDENTIFIER).missing_indices.len(),
			(n - 1) as usize,
		);

		Ok(())
	}

	impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
