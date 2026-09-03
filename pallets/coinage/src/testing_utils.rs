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

//! Coinage pallet testing utilities shared between tests and benchmarks.

use crate::{MAX_TRIE_NODE_LEN, MAX_TRIE_PROOF_NODES};
use alloc::vec::Vec;
use frame_support::{traits::ConstU32, BoundedVec};
use indiv_support::traits::Alias;
use sp_core::H256;
use sp_runtime::traits::BlakeTwo256;
use sp_trie::{LayoutV1, Recorder, StorageProof, TrieDBMutBuilder, TrieMut};

type Layout = LayoutV1<BlakeTwo256>;

/// The bounded non-inclusion proof type taken by
/// [`crate::Call::unload_archived_recycler_into_external_asset`].
pub type BoundedTrieProof =
	BoundedVec<BoundedVec<u8, ConstU32<MAX_TRIE_NODE_LEN>>, ConstU32<MAX_TRIE_PROOF_NODES>>;

/// Build the unloaded-aliases trie over `unloaded` and a recovery proof for `target` against its
/// root.
///
/// Records the nodes touched by inserting `target`, rather than a read/non-inclusion proof. The
/// on-chain recovery both verifies non-inclusion *and* re-inserts the alias via
/// `delta_trie_root`; a plain lookup proof can omit nodes the insert needs, which surfaces as
/// `IncompleteDatabase` for larger/denser tries. The recorded insert proof is a superset of the
/// non-inclusion path and is sufficient for both steps.
pub fn unloaded_root_and_non_inclusion_proof(
	unloaded: &[Alias],
	target: &Alias,
) -> (H256, Vec<Vec<u8>>) {
	let mut db = StorageProof::empty().into_memory_db::<BlakeTwo256>();
	let mut root = H256::default();
	{
		let mut trie = TrieDBMutBuilder::<Layout>::new(&mut db, &mut root).build();
		for alias in unloaded {
			trie.insert(&alias[..], &[]).expect("insert");
		}
	}
	let mut recorder = Recorder::<Layout>::new();
	{
		let mut insert_root = root;
		let mut trie = TrieDBMutBuilder::<Layout>::from_existing(&mut db, &mut insert_root)
			.with_recorder(&mut recorder)
			.build();
		trie.insert(&target[..], &[]).expect("insert");
	}
	let proof = recorder.drain().into_iter().map(|record| record.data).collect();
	(root, proof)
}

/// Convert raw proof nodes into the bounded form taken by
/// [`crate::Call::unload_archived_recycler_into_external_asset`].
pub fn to_bounded_proof(proof: Vec<Vec<u8>>) -> BoundedTrieProof {
	proof
		.into_iter()
		.map(|n| BoundedVec::try_from(n).expect("trie node fits MAX_TRIE_NODE_LEN"))
		.collect::<Vec<_>>()
		.try_into()
		.expect("proof fits MAX_TRIE_PROOF_NODES")
}
