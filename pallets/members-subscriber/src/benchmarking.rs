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
		CurrentGeneration, ProcessingState, QueuedRingPurge, RingCollectionExponents,
		RingCollectionStates, RingRoots, Subscription,
	},
	types::{
		Identifier, MembersOf, RingCollectionState, RingIndex, RingPurgeProgress, RingRootOp,
		RingRootUpdate, RingRootUpdatesBatch, SubscriptionStatus,
	},
};
use alloc::collections::BTreeMap;
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
	Pallet::<T>::set_current_ring_roots(&identifier, ring_index, roots);
}

/// Collection state with both bounded sets at capacity.
fn worst_case_collection_state<T: Config>(
) -> RingCollectionState<T::MaxMissingRootsPerCollection, T::MaxDeletedRingsPerCollection> {
	let missing = T::MaxMissingRootsPerCollection::get();
	let deleted = T::MaxDeletedRingsPerCollection::get();
	let next_ring_index = missing.saturating_add(deleted);

	RingCollectionState {
		next_ring_index,
		missing_indices: (0..missing)
			.map(|i| (i, 0u32))
			.collect::<BTreeMap<_, _>>()
			.try_into()
			.expect("at capacity"),
		// Deleted indices sit above the missing ones, so no index is in both sets
		deleted_indices: (missing..next_ring_index)
			.collect::<BTreeSet<_>>()
			.try_into()
			.expect("at capacity"),
		..Default::default()
	}
}

/// Distinct collection identifier.
fn bench_identifier(c: u32) -> Identifier {
	let mut identifier = [0u8; 32];
	identifier[..4].copy_from_slice(&c.to_le_bytes());
	identifier
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

		// Worst case for the collection bound: the batch adds the last collection that fits,
		// so the call walks every existing exponent before accepting it.
		for c in 1..T::MaxCollections::get() {
			RingCollectionExponents::<T>::insert(bench_identifier(c), RingExponent::R2e9);
		}

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
		RingCollectionExponents::<T>::insert(BENCH_IDENTIFIER, RingExponent::R2e9);

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

		// Worst-case: all collections carry state to clear inline. Ring roots are
		// marked as stale via the generation bump, so their count does not matter.
		let max_collections = T::MaxCollections::get();

		for c in 0..max_collections {
			let identifier = bench_identifier(c);

			for i in 0..T::MaxUpdatesPerBatch::get() {
				fill_in_ring_roots::<T>(identifier, i as RingIndex, i);
			}

			RingCollectionStates::<T>::insert(
				identifier,
				RingCollectionState {
					ring_count: T::MaxUpdatesPerBatch::get(),
					..worst_case_collection_state::<T>()
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
		// Ring roots remain physically stored; they are marked as stale though
		assert!(RingRoots::<T>::iter().count() > 0);
		assert_eq!(CurrentGeneration::<T>::get(), 1);
		assert!(QueuedRingPurge::<T>::get().is_some());
		assert_eq!(RingCollectionStates::<T>::iter().count(), 0);
		assert_eq!(RingCollectionExponents::<T>::iter().count(), 0);
		assert_eq!(ProcessingState::<T>::get(), Default::default());

		Ok(())
	}

	#[benchmark]
	fn clear_ring_data() -> Result<(), BenchmarkError> {
		T::init();

		// Worst-case: all collections carry state to clear inline
		for c in 0..T::MaxCollections::get() {
			let identifier = bench_identifier(c);
			RingCollectionStates::<T>::insert(
				identifier,
				RingCollectionState { ring_count: 1, ..worst_case_collection_state::<T>() },
			);
			RingCollectionExponents::<T>::insert(identifier, RingExponent::R2e9);
		}
		ProcessingState::<T>::mutate(|s| s.last_processed_sequence = 1);

		#[block]
		{
			Pallet::<T>::clear_all_ring_data();
		}

		assert_eq!(CurrentGeneration::<T>::get(), 1);
		assert!(QueuedRingPurge::<T>::get().is_some());
		assert_eq!(RingCollectionStates::<T>::iter().count(), 0);
		assert_eq!(RingCollectionExponents::<T>::iter().count(), 0);
		assert_eq!(ProcessingState::<T>::get(), Default::default());

		Ok(())
	}

	#[benchmark]
	fn purge_stale_ring_roots(
		n: Linear<1, { T::PurgePageSize::get() }>,
	) -> Result<(), BenchmarkError> {
		T::init();

		// Worst-case: every visited entry is stale and removed
		for i in 0..n {
			fill_in_ring_roots::<T>(BENCH_IDENTIFIER, i as RingIndex, i);
		}
		CurrentGeneration::<T>::put(2);
		QueuedRingPurge::<T>::put(RingPurgeProgress { generation: 0, page: 0 });

		#[extrinsic_call]
		_(SystemOrigin::Authorized);

		assert_eq!(RingRoots::<T>::iter().count(), 0);
		assert_eq!(QueuedRingPurge::<T>::get(), Some(RingPurgeProgress { generation: 1, page: 0 }));

		Ok(())
	}

	#[benchmark]
	fn authorize_purge_stale_ring_roots() -> Result<(), BenchmarkError> {
		T::init();

		// Generation 0 is stale only once the live one has moved past it, which is what
		// `authorize` checks before it accepts the call.
		CurrentGeneration::<T>::put(1);
		QueuedRingPurge::<T>::put(RingPurgeProgress { generation: 0, page: 0 });

		let call = Call::<T>::purge_stale_ring_roots {};

		#[block]
		{
			call.authorize(TransactionSource::InBlock).unwrap().unwrap();
		}

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

	/// Benchmark for the scan loop in `detect_missing_rings_in_batch`.
	/// Measures cost of scanning a range of `n` indices, swept over the full per-batch
	/// scan cap since the extrinsics charge up to `MaxGapScanPerBatch` indices.
	#[benchmark]
	fn detect_missing_in_range(
		n: Linear<1, { T::MaxGapScanPerBatch::get() }>,
	) -> Result<(), BenchmarkError> {
		T::init();

		Subscription::<T>::put(SubscriptionStatus::Active { initialized_at_sequence: 1 });
		ProcessingState::<T>::mutate(|s| s.last_processed_sequence = 1);

		// Every index is a gap.
		RingCollectionStates::<T>::insert(
			BENCH_IDENTIFIER,
			RingCollectionState { ring_count: 0, next_ring_index: n, ..Default::default() },
		);

		// The batch only carries the collection identifier for the scan; the scanned range
		// comes from the stored state's frontier and scan cursor.
		let batch = RingRootUpdatesBatch::<T> {
			identifier: BENCH_IDENTIFIER,
			sequence: 2,
			source_time: 2000,
			updates: BoundedVec::new(),
			next_ring_index: n,
		};

		#[block]
		{
			Pallet::<T>::detect_missing_rings_in_batch(&batch);
		}

		assert_eq!(
			RingCollectionStates::<T>::get(BENCH_IDENTIFIER).missing_indices.len(),
			n as usize
		);

		Ok(())
	}

	impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
