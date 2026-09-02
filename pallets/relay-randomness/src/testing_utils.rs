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

//! Relay randomness pallet testing utilities shared between tests and benchmarks.
//!
//! `cumulus_test_relay_sproof_builder` cannot be used here because it relies on
//! `sp_state_machine::prove_read`, which is only available with `std`, while the
//! benchmarks run inside the WASM runtime. This module builds the relay chain state
//! proof directly with `sp_trie`, which works in both contexts.

use alloc::vec::Vec;
use cumulus_pallet_parachain_system::RelayChainStateProof;
use cumulus_primitives_core::relay_chain;
use sp_runtime::traits::BlakeTwo256;
use sp_trie::{LayoutV1, Recorder, StorageProof, Trie, TrieDBBuilder, TrieDBMutBuilder, TrieMut};

type Layout = LayoutV1<BlakeTwo256>;

/// Build a relay chain state proof over `entries` proving the BABE randomness keys read
/// by the pallet, mirroring the witness produced by the collator.
///
/// Keys in `entries` that are not randomness keys still end up in the trie (and thus in
/// the state root), and randomness keys absent from `entries` are provably absent.
pub fn relay_state_proof(entries: &[(&[u8], Vec<u8>)]) -> RelayChainStateProof {
	relay_state_proof_for_keys(
		entries,
		&[
			relay_chain::well_known_keys::CURRENT_BLOCK_RANDOMNESS,
			relay_chain::well_known_keys::ONE_EPOCH_AGO_RANDOMNESS,
		],
	)
}

/// Build a relay chain state proof over `entries` proving only `proven_keys`.
///
/// A key present in `entries` but not in `proven_keys` may be unreadable from the proof,
/// which is how a witness missing required keys is simulated.
pub fn relay_state_proof_for_keys(
	entries: &[(&[u8], Vec<u8>)],
	proven_keys: &[&[u8]],
) -> RelayChainStateProof {
	let mut db = StorageProof::empty().into_memory_db::<BlakeTwo256>();
	let mut root = relay_chain::Hash::default();
	{
		let mut trie = TrieDBMutBuilder::<Layout>::new(&mut db, &mut root).build();
		for (key, value) in entries {
			trie.insert(key, value).expect("insert into in-memory trie cannot fail");
		}
	}

	// Record the nodes touched when reading the proven keys, exactly like the
	// collator-side `prove_read` does.
	let mut recorder = Recorder::<Layout>::new();
	{
		let trie = TrieDBBuilder::<Layout>::new(&db, &root).with_recorder(&mut recorder).build();
		for key in proven_keys {
			trie.get(key).expect("read from complete in-memory trie cannot fail");
		}
	}
	let nodes = recorder.drain().into_iter().map(|record| record.data).collect::<Vec<_>>();

	RelayChainStateProof::new(0.into(), root, StorageProof::new(nodes))
		.expect("proof is built from the root it is checked against")
}
