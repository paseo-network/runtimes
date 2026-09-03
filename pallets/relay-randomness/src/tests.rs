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

//! Tests for the relay-randomness pallet.

use crate::{
	mock::*, testing_utils::*, Randomness, RandomnessEntry, RandomnessValues, RelayBlockRandomness,
	RelayOneEpochAgoRandomness,
};
use codec::Encode;
use cumulus_pallet_parachain_system::OnSystemEvent;
use cumulus_primitives_core::relay_chain::well_known_keys::{
	CURRENT_BLOCK_RANDOMNESS, ONE_EPOCH_AGO_RANDOMNESS,
};
use indiv_support::traits::MomentRandomness;

/// Run the hook on a proof carrying the given randomness values, with the mock relay
/// parent number set to `relay_parent_number`.
fn process_proof(vrf: Option<[u8; 32]>, one_epoch_ago: [u8; 32], relay_parent_number: u32) {
	set_relay_parent_number(relay_parent_number);
	let proof = relay_state_proof(&[
		(CURRENT_BLOCK_RANDOMNESS, vrf.encode()),
		(ONE_EPOCH_AGO_RANDOMNESS, one_epoch_ago.encode()),
	]);
	<RelayRandomness as OnSystemEvent>::on_relay_state_proof(&proof);
}

#[test]
fn stores_randomness_values() {
	new_test_ext().execute_with(|| {
		process_proof(Some([1u8; 32]), [2u8; 32], 42);

		// The block VRF moment is the relay parent number minus the offset (2 in the
		// mock); the epoch moment subtracts one more block, as the value was fully
		// determined by the relay block preceding the one that first serves it.
		assert_eq!(
			Randomness::<Test>::get(),
			RandomnessValues {
				block: Some(RandomnessEntry { randomness: [1u8; 32], moment: 40 }),
				one_epoch_ago: Some(RandomnessEntry { randomness: [2u8; 32], moment: 39 })
			}
		);
		assert_eq!(RelayBlockRandomness::<Test>::randomness(), Some(([1u8; 32], 40)));
		assert_eq!(RelayOneEpochAgoRandomness::<Test>::randomness(), Some(([2u8; 32], 39)));
	});
}

#[test]
fn overwrites_entries_when_values_change() {
	new_test_ext().execute_with(|| {
		process_proof(Some([1u8; 32]), [2u8; 32], 42);
		process_proof(Some([4u8; 32]), [5u8; 32], 43);

		assert_eq!(RelayBlockRandomness::<Test>::randomness(), Some(([4u8; 32], 41)));
		assert_eq!(RelayOneEpochAgoRandomness::<Test>::randomness(), Some(([5u8; 32], 40)));
	});
}

#[test]
fn keeps_last_known_vrf_randomness_when_vrf_is_none() {
	new_test_ext().execute_with(|| {
		process_proof(Some([1u8; 32]), [2u8; 32], 42);
		// The relay parent at block 43 has no VRF output: the stored value is an
		// encoded `None`.
		process_proof(None, [2u8; 32], 43);

		// Carried over from relay parent 42.
		assert_eq!(RelayBlockRandomness::<Test>::randomness(), Some(([1u8; 32], 40)));
	});
}

#[test]
fn keeps_last_known_values_when_keys_are_absent() {
	new_test_ext().execute_with(|| {
		process_proof(Some([1u8; 32]), [2u8; 32], 42);

		// All randomness keys are provably absent from the relay chain state.
		set_relay_parent_number(43);
		let proof = relay_state_proof(&[(&b"unrelated key"[..], vec![0u8])]);
		<RelayRandomness as OnSystemEvent>::on_relay_state_proof(&proof);

		assert_eq!(RelayBlockRandomness::<Test>::randomness(), Some(([1u8; 32], 40)));
		assert_eq!(RelayOneEpochAgoRandomness::<Test>::randomness(), Some(([2u8; 32], 39)));
	});
}

#[test]
fn stores_nothing_when_randomness_never_available() {
	new_test_ext().execute_with(|| {
		// A relay chain state without any of the randomness keys, e.g. near genesis.
		let proof = relay_state_proof(&[(&b"unrelated key"[..], vec![0u8])]);
		<RelayRandomness as OnSystemEvent>::on_relay_state_proof(&proof);

		assert_eq!(Randomness::<Test>::get(), RandomnessValues::default());
		assert_eq!(RelayBlockRandomness::<Test>::randomness(), None);
		assert_eq!(RelayOneEpochAgoRandomness::<Test>::randomness(), None);
	});
}

#[test]
fn moments_only_advance_when_values_change() {
	new_test_ext().execute_with(|| {
		process_proof(Some([1u8; 32]), [2u8; 32], 42);
		// A para block whose relay parent carries the same values (e.g. same relay
		// parent, or unchanged within the epoch): every entry keeps its moment.
		process_proof(Some([1u8; 32]), [2u8; 32], 43);

		assert_eq!(RelayBlockRandomness::<Test>::randomness(), Some(([1u8; 32], 40)));
		assert_eq!(RelayOneEpochAgoRandomness::<Test>::randomness(), Some(([2u8; 32], 39)));

		// Distinct values advance the moments again.
		process_proof(Some([7u8; 32]), [8u8; 32], 44);
		assert_eq!(RelayBlockRandomness::<Test>::randomness(), Some(([7u8; 32], 42)));
		assert_eq!(RelayOneEpochAgoRandomness::<Test>::randomness(), Some(([8u8; 32], 41)));
	});
}

#[test]
fn moments_cover_the_lookahead() {
	new_test_ext().execute_with(|| {
		// The commitment moment is the current relay parent number, whether or not a
		// value was ever observed.
		set_relay_parent_number(42);
		assert_eq!(RelayBlockRandomness::<Test>::current_moment(), 42);
		assert_eq!(RelayOneEpochAgoRandomness::<Test>::current_moment(), 42);

		// Values first observed at relay parent 44 were already determinable while the
		// block with relay parent 42 was authored (offset 2 in the mock): their moments
		// do not exceed the commitment moment. The same holds for an epoch value first
		// served at relay parent 45, determined by relay block 44.
		process_proof(Some([1u8; 32]), [2u8; 32], 44);
		assert_eq!(RelayBlockRandomness::<Test>::randomness(), Some(([1u8; 32], 42)));
		process_proof(Some([7u8; 32]), [8u8; 32], 45);
		assert_eq!(RelayBlockRandomness::<Test>::randomness(), Some(([7u8; 32], 43)));
		assert_eq!(RelayOneEpochAgoRandomness::<Test>::randomness(), Some(([8u8; 32], 42)));

		// Values first observed one relay parent later were not: their moments exceed
		// it.
		process_proof(Some([9u8; 32]), [10u8; 32], 46);
		assert_eq!(RelayBlockRandomness::<Test>::randomness(), Some(([9u8; 32], 44)));
		assert_eq!(RelayOneEpochAgoRandomness::<Test>::randomness(), Some(([10u8; 32], 43)));
	});
}

#[test]
fn moments_saturate_at_zero_near_genesis() {
	new_test_ext().execute_with(|| {
		// The relay parent number is below the offset (2 in the mock).
		process_proof(Some([1u8; 32]), [2u8; 32], 1);

		assert_eq!(RelayBlockRandomness::<Test>::randomness(), Some(([1u8; 32], 0)));
		assert_eq!(RelayOneEpochAgoRandomness::<Test>::randomness(), Some(([2u8; 32], 0)));
	});
}

#[test]
#[should_panic(expected = "Invalid current block randomness")]
fn panics_when_current_block_randomness_is_undecodable() {
	new_test_ext().execute_with(|| {
		// `2` is not a valid `Option` discriminant.
		let proof = relay_state_proof(&[
			(CURRENT_BLOCK_RANDOMNESS, vec![2u8]),
			(ONE_EPOCH_AGO_RANDOMNESS, [2u8; 32].encode()),
		]);
		<RelayRandomness as OnSystemEvent>::on_relay_state_proof(&proof);
	});
}

#[test]
#[should_panic(expected = "Invalid one epoch ago randomness")]
fn panics_when_epoch_randomness_is_undecodable() {
	new_test_ext().execute_with(|| {
		// 31 bytes cannot decode into `[u8; 32]`.
		let proof = relay_state_proof(&[
			(CURRENT_BLOCK_RANDOMNESS, Some([1u8; 32]).encode()),
			(ONE_EPOCH_AGO_RANDOMNESS, vec![2u8; 31]),
		]);
		<RelayRandomness as OnSystemEvent>::on_relay_state_proof(&proof);
	});
}

#[test]
#[should_panic(expected = "Invalid current block randomness")]
fn panics_when_proof_does_not_contain_the_key() {
	new_test_ext().execute_with(|| {
		// The key exists in the relay chain state but the witness only proves the epoch
		// randomness key, so the current block randomness is unreadable.
		let proof = relay_state_proof_for_keys(
			&[
				(CURRENT_BLOCK_RANDOMNESS, Some([1u8; 32]).encode()),
				(ONE_EPOCH_AGO_RANDOMNESS, [2u8; 32].encode()),
			],
			&[ONE_EPOCH_AGO_RANDOMNESS],
		);
		<RelayRandomness as OnSystemEvent>::on_relay_state_proof(&proof);
	});
}
