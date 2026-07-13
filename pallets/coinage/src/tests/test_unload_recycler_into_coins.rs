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

use crate::{
	extension::{AsCoinage, AsCoinageInfo},
	mock::*,
	pallet::CustomInvalidity,
	*,
};
use codec::Encode;
use frame_support::{assert_err, assert_ok, traits::fungibles::Inspect, BoundedVec};
use frame_system::AuthorizeCall;
use indiv_support::traits::Alias;
use sp_runtime::{bounded_vec, testing::UintAuthorityId, transaction_validity::TransactionSource};
use verifiable::GenerateVerifiable;

fn bounded_split(
	split_into: Vec<(CoinValue, Vec<u64>)>,
) -> BoundedVec<
	(CoinValue, BoundedVec<u64, <Test as Config>::MaxSplitOutputs>),
	<Test as Config>::MaxSplitOutputs,
> {
	split_into
		.into_iter()
		.map(|(v, d)| (v, d.try_into().unwrap()))
		.collect::<Vec<_>>()
		.try_into()
		.unwrap()
}

fn build_ext(
	call: RuntimeCall,
	period: u32,
	counter: u32,
	recycler_secrets: &[Secret],
	value: CoinValue,
	index: u32,
	people_alias_override: Option<Alias>,
) -> Extrinsic {
	let inherited_implication = ((0u8, &call), (), ());
	let proven_msg = sp_io::hashing::blake2_256(&inherited_implication.encode());

	let mut alias_proofs_vec = Vec::new();
	let ring_members = Coinage::get_recycler_members(value, index);

	for secret in recycler_secrets {
		let member = CryptoOf::<Test>::member_from_secret(secret);
		let commitment =
			CryptoOf::<Test>::open(recycler_ring_size(), &member, ring_members.clone().into_iter())
				.unwrap();
		let (proof, _) = CryptoOf::<Test>::create(
			commitment,
			secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
			&proven_msg,
		)
		.unwrap();
		alias_proofs_vec.push(proof);
	}

	let alias_proofs = BoundedVec::try_from(alias_proofs_vec).expect("Too many proofs");

	let intent_msg = sp_io::hashing::blake2_256(
		&[alias_proofs.encode(), inherited_implication.encode()].concat(),
	);

	let context = crate::pallet::free_unload_token_context(period, counter);
	let people_alias = people_alias_override.unwrap_or([0u8; 32]);
	let people_proof =
		PeopleProof { context: context.to_vec(), msg: intent_msg.to_vec(), alias: people_alias };

	let info =
		AsCoinageInfo::AsUnloadTokenPeople { proof: people_proof, period, counter, alias_proofs };

	let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(info)));
	Extrinsic::new_signed(call, 0, UintAuthorityId(0), extension)
}

#[test]
fn success_unload_and_split() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = secrets[0].clone();
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];

		let dest1 = 1;
		let dest2 = 2;

		// Recycler value 0. Split into two coins of value -1.
		// In mock, MinExponent is -2.
		// value 0 = 1000 units (in terms of min exponent units, i.e., 2^(0 - (-2)) = 4 units).
		// value -1 = 500 units (i.e., 2^(-1 - (-2)) = 2 units).
		// 2 * 2 = 4. Matches.

		let split_into = bounded_split(vec![(-1, vec![dest1, dest2])]);

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coins {
			aliases: aliases.clone(),
			value: 0,
			index,
			revision,
			split_into,
			max_fee: 0,
		});

		let ext = build_ext(call, 0, 0, &secrets, 0, index, None);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		let coin1 = CoinsByOwner::<Test>::get(dest1).unwrap();
		assert_eq!(coin1, Coin { value: -1, age: 1 });
		let coin2 = CoinsByOwner::<Test>::get(dest2).unwrap();
		assert_eq!(coin2, Coin { value: -1, age: 1 });

		assert!(RecyclersUnloaded::<Test>::contains_key((0, index, aliases[0])));
		System::assert_has_event(
			crate::Event::<Test>::RecyclerUnloadedIntoCoins { output_count: 2 }.into(),
		);
	});
}

#[test]
fn split_invalid_sum_fail() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap()];

		let dest1 = 1;
		// Unload 0 -> Split into one -1. The sum doesn't match.
		// value 0 = 4 units, value -1 = 2 units. 2 != 4.
		let split_into = bounded_split(vec![(-1, vec![dest1])]);

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coins {
			aliases,
			value: 0,
			index,
			revision,
			split_into,
			max_fee: 0,
		});

		let ext = build_ext(call, 0, 0, &secrets, 0, index, None);
		// Validation passes (split params not checked in extension)
		assert_ok!(Executive::validate_transaction(
			TransactionSource::External,
			ext.clone(),
			Default::default()
		));
		// Dispatch fails
		assert_err!(Executive::apply_extrinsic(ext).unwrap(), Error::<Test>::InvalidSplit);
	});
}

#[test]
fn dest_has_coin_invalid() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap()];

		let dest1 = 1;
		CoinsByOwner::<Test>::insert(dest1, Coin { value: 0, age: 0 });

		let split_into = bounded_split(vec![(0, vec![dest1])]); // 0 -> 0 (sum ok)

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coins {
			aliases,
			value: 0,
			index,
			revision,
			split_into,
			max_fee: 0,
		});

		let ext = build_ext(call, 0, 0, &secrets, 0, index, None);
		// Checked in validation
		assert_invalid(ext, CustomInvalidity::AddressAlreadyHasCoin);
	});
}

#[test]
fn split_into_multiple_values() {
	new_test_ext().execute_with(|| {
		// Setup recycler with value 1 (8 units in min exponent terms: 2^(1 - (-2)) = 8)
		let (secrets, index, revision) = setup_recycler(1, 1, 0);
		let secret = secrets[0].clone();
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];

		let dest1 = 1;
		let dest2 = 2;
		let dest3 = 3;

		// Split value 1 (8 units) into:
		// - 2 coins of value -1 (2 units each = 4 units)
		// - 1 coin of value 0 (4 units)
		// Total: 4 + 4 = 8 units. Matches.
		let split_into = bounded_split(vec![(-1, vec![dest1, dest2]), (0, vec![dest3])]);

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coins {
			aliases,
			value: 1,
			index,
			revision,
			split_into,
			max_fee: 0,
		});

		let ext = build_ext(call, 0, 0, &secrets, 1, index, None);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		assert_eq!(CoinsByOwner::<Test>::get(dest1).unwrap(), Coin { value: -1, age: 1 });
		assert_eq!(CoinsByOwner::<Test>::get(dest2).unwrap(), Coin { value: -1, age: 1 });
		assert_eq!(CoinsByOwner::<Test>::get(dest3).unwrap(), Coin { value: 0, age: 1 });
	});
}

#[test]
fn consolidate_and_split() {
	new_test_ext().execute_with(|| {
		// Setup recycler with 2 coins of value 0 (they will be consolidated to value 1)
		let (secrets, index, revision) = setup_recycler(0, 2, 0);
		let aliases: Vec<Alias> = secrets
			.iter()
			.map(|s| {
				CryptoOf::<Test>::alias_in_context(s, UNLOADING_RECYCLER_CONTEXT.as_ref()).unwrap()
			})
			.collect();

		let dest1 = 1;
		let dest2 = 2;
		let dest3 = 3;
		let dest4 = 4;

		// Consolidate 2 coins of value 0 -> value 1 (8 units)
		// Split into 4 coins of value -1 (2 units each = 8 units)
		let split_into = bounded_split(vec![(-1, vec![dest1, dest2, dest3, dest4])]);

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coins {
			aliases: aliases.clone().try_into().unwrap(),
			value: 0,
			index,
			revision,
			split_into,
			max_fee: 0,
		});

		let ext = build_ext(call, 0, 0, &secrets, 0, index, None);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		assert_eq!(CoinsByOwner::<Test>::get(dest1).unwrap(), Coin { value: -1, age: 1 });
		assert_eq!(CoinsByOwner::<Test>::get(dest2).unwrap(), Coin { value: -1, age: 1 });
		assert_eq!(CoinsByOwner::<Test>::get(dest3).unwrap(), Coin { value: -1, age: 1 });
		assert_eq!(CoinsByOwner::<Test>::get(dest4).unwrap(), Coin { value: -1, age: 1 });

		for alias in &aliases {
			assert!(RecyclersUnloaded::<Test>::contains_key((0, index, *alias)));
		}
	});
}

#[test]
fn empty_split_invalid() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap()];

		// Empty destination list for a value
		let split_into = bounded_split(vec![(0, vec![])]);

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coins {
			aliases,
			value: 0,
			index,
			revision,
			split_into,
			max_fee: 0,
		});

		let ext = build_ext(call, 0, 0, &secrets, 0, index, None);
		// Validation catches empty splits early.
		assert_invalid(ext, CustomInvalidity::EmptySplit);
	});
}

#[test]
fn empty_split_fail() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap()];

		// Empty destination list for a value
		let split_into = bounded_split(vec![(0, vec![])]);

		// Build alias proofs for the origin
		let msg_hash = [0u8; 32];
		let ring_members = Coinage::get_recycler_members(0, index);
		let mut alias_proofs_vec = Vec::new();
		for secret in &secrets {
			let member = CryptoOf::<Test>::member_from_secret(secret);
			let commitment = CryptoOf::<Test>::open(
				recycler_ring_size(),
				&member,
				ring_members.clone().into_iter(),
			)
			.unwrap();
			let (proof, _) = CryptoOf::<Test>::create(
				commitment,
				secret,
				UNLOADING_RECYCLER_CONTEXT.as_ref(),
				&msg_hash,
			)
			.unwrap();
			alias_proofs_vec.push(proof);
		}
		let alias_proofs = BoundedVec::try_from(alias_proofs_vec).unwrap();

		// Call the pallet directly and check the error
		assert_err!(
			Coinage::unload_recycler_into_coins(
				Origin::UnloadToken { alias_proofs, proven_msg: msg_hash, fee: UnloadFee::Prepaid }
					.into(),
				aliases,
				0,
				index,
				revision,
				split_into,
				0,
			),
			Error::<Test>::InvalidSplit
		);
	});
}

/// Verify that many empty destination arrays are rejected early in validation,
/// preventing CPU DoS attacks that bypass the max_split_outputs limit.
#[test]
fn many_empty_splits_rejected_early() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap()];

		// Create one non-empty entry followed by one empty entry.
		// Without the fix, empty entries would bypass max_split_outputs since len() == 0.
		let split_into = bounded_split(vec![(0, vec![1]), (0, vec![])]);

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coins {
			aliases,
			value: 0,
			index,
			revision,
			split_into,
			max_fee: 0,
		});

		let ext = build_ext(call, 0, 0, &secrets, 0, index, None);
		// Validation should reject the empty split immediately.
		assert_invalid(ext, CustomInvalidity::EmptySplit);
	});
}

#[test]
fn duplicate_destinations_fail() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap()];

		let dest1 = 1;

		// Same destination used twice
		let split_into = bounded_split(vec![(-1, vec![dest1, dest1])]);

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coins {
			aliases,
			value: 0,
			index,
			revision,
			split_into,
			max_fee: 0,
		});

		let ext = build_ext(call, 0, 0, &secrets, 0, index, None);
		// Validation passes, dispatch fails (mapped to InvalidSplit)
		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), Error::<Test>::InvalidSplit);
	});
}

#[test]
fn split_not_sorted_fail() {
	new_test_ext().execute_with(|| {
		// Setup recycler with value 1 (8 units)
		let (secrets, index, revision) = setup_recycler(1, 1, 0);
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap()];

		let dest1 = 1;
		let dest2 = 2;

		// Split values not in ascending order (0 before -1)
		let split_into = bounded_split(vec![(0, vec![dest1]), (-1, vec![dest2, dest2])]);

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coins {
			aliases,
			value: 1,
			index,
			revision,
			split_into,
			max_fee: 0,
		});

		let ext = build_ext(call, 0, 0, &secrets, 1, index, None);
		// Validation passes, dispatch fails (mapped to InvalidSplit)
		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), Error::<Test>::InvalidSplit);
	});
}

#[test]
fn invalid_revision_invalid() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap()];

		let split_into = bounded_split(vec![(0, vec![1])]);

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coins {
			aliases,
			value: 0,
			index,
			revision: revision + 1, // Wrong revision
			split_into,
			max_fee: 0,
		});

		let ext = build_ext(call, 0, 0, &secrets, 0, index, None);
		// Validation check (revision IS checked in validation)
		assert_invalid(ext, CustomInvalidity::InvalidRecyclerRevision);
	});
}

#[test]
fn success_unload_and_split_non_power_of_two() {
	new_test_ext().execute_with(|| {
		// Setup recycler with 3 loaded coins (non power of two)
		let (secrets, index, revision) = setup_recycler(0, 3, 0);

		let mut aliases = Vec::new();
		for secret in &secrets {
			aliases.push(
				CryptoOf::<Test>::alias_in_context(secret, UNLOADING_RECYCLER_CONTEXT.as_ref())
					.unwrap(),
			);
		}

		let dest1 = 1;
		let dest2 = 2;

		// Unload 3 coins of value 0.
		// MinExponent = -2.
		// Value 0 -> Offset 2 -> Unit = 4.
		// Total Input = 3 * 4 = 12 units.
		//
		// Split into:
		// 1 coin of value 1 (8 units)
		// 1 coin of value 0 (4 units)
		// Total Output = 8 + 4 = 12 units. Matches.
		let split_into = bounded_split(vec![(0, vec![dest2]), (1, vec![dest1])]);

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coins {
			aliases: aliases.clone().try_into().unwrap(),
			value: 0,
			index,
			revision,
			split_into,
			max_fee: 0,
		});

		let ext = build_ext(call, 0, 0, &secrets, 0, index, None);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		let coin1 = CoinsByOwner::<Test>::get(dest1).unwrap();
		assert_eq!(coin1, Coin { value: 1, age: 1 });
		let coin2 = CoinsByOwner::<Test>::get(dest2).unwrap();
		assert_eq!(coin2, Coin { value: 0, age: 1 });

		for alias in &aliases {
			assert!(RecyclersUnloaded::<Test>::contains_key((0, index, *alias)));
		}
	});
}

#[test]
fn empty_inputs_fail() {
	new_test_ext().execute_with(|| {
		let (_secrets, index, revision) = setup_recycler(0, 1, 0);
		let aliases: BoundedVec<Alias, _> = BoundedVec::new();
		let split_into = bounded_split(vec![]);

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coins {
			aliases,
			value: 0,
			index,
			revision,
			split_into,
			max_fee: 0,
		});

		let ext = build_ext(call, 0, 0, &[], 0, index, None);

		// Empty inputs IS checked in dispatch, not validation
		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), Error::<Test>::EmptyInputs);
	});
}

/// Verify that validation stops early when destination count exceeds MaxSplitOutputs,
/// preventing unbounded storage reads (bounded by `MaxSplitOutputs^2`).
#[test]
fn too_many_outputs_stops_early() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap()];

		let max_split_outputs = get_u32::<<Test as Config>::MaxSplitOutputs>();

		// Create max + 2 destinations. The destination at index max + 1 already has a coin.
		// If validation was unbounded, it would check all destinations and return
		// AddressAlreadyHasCoin. With the fix, it should return TooManySplits first because
		// the count check happens before the storage reads for destinations beyond the limit.

		// Number of destinations to generate.
		// Example: if `max_split_outputs = 3`, then `num_dests = 5`.
		let num_dests = (max_split_outputs + 2) as usize;

		// Generate destination IDs starting at 100.
		// Example: if `num_dests = 5`, this produces `vec![100, 101, 102, 103, 104]`.
		let dests: Vec<u64> = (100u64..).take(num_dests).collect();
		let dest_with_coin = *dests.last().unwrap();
		CoinsByOwner::<Test>::insert(dest_with_coin, Coin { value: 0, age: 0 });

		let split_into = bounded_split(vec![
			(-2, dests[0..max_split_outputs as usize].to_vec()),
			(-1, dests[max_split_outputs as usize..].to_vec()),
		]);

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coins {
			aliases: aliases.try_into().unwrap(),
			value: 0,
			index,
			revision,
			split_into,
			max_fee: 0,
		});

		let ext = build_ext(call, 0, 0, &secrets, 0, index, None);

		// Should get TooManySplits, NOT AddressAlreadyHasCoin, proving validation stopped early.
		assert_invalid(ext, CustomInvalidity::TooManySplits);
	});
}

#[test]
fn success_with_previous_revision() {
	new_test_ext().execute_with(|| {
		// Step 1: Setup recycler with initial coins and build
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = secrets[0].clone();
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];

		// Capture the ring members BEFORE adding more coins
		let ring_members_v1 = Coinage::get_recycler_members(0, index);

		// Step 2: Add more coins and build again (this rotates the revision)
		let new_secret = get_secret(100);
		let new_member = CryptoOf::<Test>::member_from_secret(&new_secret);
		assert_ok!(RecyclerManager::<Test>::load(0, new_member));
		Members::process_maintenance();

		// Step 3: Use proof generated against the OLD revision (previous_root)
		let dest1 = 1;
		let dest2 = 2;
		// Split 1 coin of value 0 (4 units) into 2 coins of value -1 (2 units each)
		let split_into = bounded_split(vec![(-1, vec![dest1, dest2])]);

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coins {
			aliases: aliases.clone(),
			value: 0,
			index,
			revision, // Using the OLD revision
			split_into,
			max_fee: 0,
		});

		// Build extension with proof against the OLD ring members
		let inherited_implication = ((0u8, &call), (), ());
		let proven_msg = sp_io::hashing::blake2_256(&inherited_implication.encode());

		// Generate proof using the OLD ring members (v1)
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let commitment = CryptoOf::<Test>::open(
			recycler_ring_size(),
			&member,
			ring_members_v1.clone().into_iter(),
		)
		.unwrap();
		let (proof, _) = CryptoOf::<Test>::create(
			commitment,
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
			&proven_msg,
		)
		.unwrap();

		let alias_proofs = BoundedVec::try_from(vec![proof]).expect("Too many proofs");

		let intent_msg = sp_io::hashing::blake2_256(
			&[alias_proofs.encode(), inherited_implication.encode()].concat(),
		);

		let context = crate::pallet::free_unload_token_context(0, 0);
		let people_proof =
			PeopleProof { context: context.to_vec(), msg: intent_msg.to_vec(), alias: [0u8; 32] };

		let info = AsCoinageInfo::AsUnloadTokenPeople {
			proof: people_proof,
			period: 0,
			counter: 0,
			alias_proofs,
		};

		let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(info)));
		let ext = Extrinsic::new_signed(call, 0, UintAuthorityId(0), extension);

		// Should succeed because the previous revision's root is still valid
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		let coin1 = CoinsByOwner::<Test>::get(dest1).unwrap();
		assert_eq!(coin1, Coin { value: -1, age: 1 });
		let coin2 = CoinsByOwner::<Test>::get(dest2).unwrap();
		assert_eq!(coin2, Coin { value: -1, age: 1 });

		// Verify recycler state
		assert_eq!(RecyclersUnloaded::<Test>::iter_prefix((0i8, index)).count(), 1);
	});
}

// ============================================================================
// FromOutput fee mode tests
// ============================================================================

#[test]
fn from_output_success_with_max_fee_above_unload_token_fee() {
	// Test successful unload with FromOutput fee mode where max_fee covers the network fee
	// and has remainder that gets burned.
	//
	// Setup:
	// - MinimumExponent = -2, UNDERLYING_ASSET_UNIT = 1000
	// - coin_value_to_asset_amount(-2) = 1000 >> 2 = 250 asset units per min unit
	// - MockPaidUnloadTokenFeeOverride = 2 (fee is 2 asset units)
	// - max_fee = 250 asset units
	// - 250 > 2, so fee is covered and 248 is burned
	//
	// Using recycler value 0:
	// - 1 coin of value 0 = 4 units (2^(0 - (-2)) = 4)
	// - max_fee = 250 asset units = 1 unit
	// - split_into must account for 4 - 1 = 3 units
	// - 3 units = 1 coin of value -1 (2 units) + 1 coin of value -2 (1 unit)
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let value: CoinValue = 0;
		let (secrets, index, revision) = setup_recycler(value, 1, 0);
		let alias =
			CryptoOf::<Test>::alias_in_context(&secrets[0], UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();
		let aliases: BoundedVec<Alias, _> = bounded_vec![alias];

		let dest1 = 100;
		let dest2 = 101;

		// Input: 1 coin of value 0 = 4 units
		// max_fee: 250 asset units = 1 unit
		// Output: 3 units = 1 coin value -1 (2 units) + 1 coin value -2 (1 unit)
		let split_into = bounded_split(vec![(-2, vec![dest2]), (-1, vec![dest1])]);

		let call = crate::Call::<Test>::unload_recycler_into_coins {
			aliases: aliases.clone(),
			value,
			index,
			revision,
			split_into,
			max_fee: 250,
		};

		let fee_dest_before = AssetsWithHolder::balance(10, &FEE_DESTINATION);
		let destroyed_before = TotalValueOfDestroyedCoins::<Test>::get();

		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets);
		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "Extrinsic should succeed: {result:?}");
		assert!(result.unwrap().is_ok(), "Dispatch should succeed");

		// Check coins were created
		assert_eq!(CoinsByOwner::<Test>::get(dest1).unwrap(), Coin { value: -1, age: 1 });
		assert_eq!(CoinsByOwner::<Test>::get(dest2).unwrap(), Coin { value: -2, age: 1 });

		// Check fee was transferred (2 asset units)
		let fee_dest_after = AssetsWithHolder::balance(10, &FEE_DESTINATION);
		assert_eq!(fee_dest_after - fee_dest_before, 2);

		// Check remainder was burned (250 - 2 = 248 asset units)
		let destroyed_after = TotalValueOfDestroyedCoins::<Test>::get();
		assert_eq!(destroyed_after - destroyed_before, 248);

		// Check alias was marked as unloaded
		assert!(RecyclersUnloaded::<Test>::contains_key((value, index, alias)));

		// Check event
		System::assert_has_event(
			crate::Event::<Test>::RecyclerUnloadedIntoCoins { output_count: 2 }.into(),
		);
	});
}

#[test]
fn from_output_invalid_when_max_fee_below_unload_token_fee() {
	// Test that the transaction is INVALID (not failing) when max_fee is insufficient
	// to cover the network fee.
	//
	// We override the fee to be 300 asset units.
	// With max_fee = 250 asset units, 250 < 300, so validation fails.
	new_test_ext().execute_with(|| {
		setup_balances();

		// Set fee higher than max_fee can cover
		MockPaidUnloadTokenFeeOverride::set(&Some(300));

		let value: CoinValue = 0;
		let (secrets, index, revision) = setup_recycler(value, 1, 0);
		let alias =
			CryptoOf::<Test>::alias_in_context(&secrets[0], UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();
		let aliases: BoundedVec<Alias, _> = bounded_vec![alias];

		let dest1 = 100;

		// Input: 4 units, max_fee: 250 asset units = 1 unit, output: 3 units
		let split_into = bounded_split(vec![(-2, vec![dest1]), (-1, vec![dest1 + 1])]);

		let call = crate::Call::<Test>::unload_recycler_into_coins {
			aliases,
			value,
			index,
			revision,
			split_into,
			max_fee: 250, // 250 < 300 (fee)
		};

		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets);

		// Transaction should be invalid, not fail during dispatch
		assert_invalid(ext, CustomInvalidity::MaxFeeInsufficientForUnload);
	});
}

#[test]
fn from_output_fail_split_plus_max_fee_more_than_input() {
	// Test that dispatch fails when split_into + max_fee > total input units.
	//
	// Input: 1 coin of value 0 = 4 units
	// max_fee: 250 asset units = 1 unit
	// Expected output: 3 units
	// Actual split_into: 4 units (too much)
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: CoinValue = 0;
		let (secrets, index, revision) = setup_recycler(value, 1, 0);
		let alias =
			CryptoOf::<Test>::alias_in_context(&secrets[0], UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();
		let aliases: BoundedVec<Alias, _> = bounded_vec![alias];

		let dest1 = 100;

		// Input: 4 units, max_fee: 250 asset units = 1 unit
		// Expected available for split: 3 units
		// Actual split_into: 1 coin of value 0 = 4 units (more than 3)
		let split_into = bounded_split(vec![(0, vec![dest1])]);

		let call = crate::Call::<Test>::unload_recycler_into_coins {
			aliases,
			value,
			index,
			revision,
			split_into,
			max_fee: 250,
		};

		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets);

		// Validation passes, dispatch fails with InvalidSplit
		assert_ok!(Executive::validate_transaction(
			TransactionSource::External,
			ext.clone(),
			Default::default()
		));

		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "apply_extrinsic should succeed");
		assert_err!(result.unwrap(), Error::<Test>::InvalidSplit);
	});
}

#[test]
fn from_output_fail_split_plus_max_fee_less_than_input() {
	// Test that dispatch fails when split_into + max_fee < total input units.
	//
	// Input: 1 coin of value 0 = 4 units
	// max_fee: 250 asset units = 1 unit
	// Expected output: 3 units
	// Actual split_into: 2 units (too little)
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: CoinValue = 0;
		let (secrets, index, revision) = setup_recycler(value, 1, 0);
		let alias =
			CryptoOf::<Test>::alias_in_context(&secrets[0], UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();
		let aliases: BoundedVec<Alias, _> = bounded_vec![alias];

		let dest1 = 100;

		// Input: 4 units, max_fee: 250 asset units = 1 unit
		// Expected available for split: 3 units
		// Actual split_into: 1 coin of value -1 = 2 units (less than 3)
		let split_into = bounded_split(vec![(-1, vec![dest1])]);

		let call = crate::Call::<Test>::unload_recycler_into_coins {
			aliases,
			value,
			index,
			revision,
			split_into,
			max_fee: 250,
		};

		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets);

		// Validation passes, dispatch fails with InvalidSplit
		assert_ok!(Executive::validate_transaction(
			TransactionSource::External,
			ext.clone(),
			Default::default()
		));

		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "apply_extrinsic should succeed");
		assert_err!(result.unwrap(), Error::<Test>::InvalidSplit);
	});
}

#[test]
fn prepaid_with_nonzero_max_fee_invalid() {
	// Test that using Prepaid fee mode with a non-zero max_fee is rejected at validation
	// with MaxFeeNotAllowedForPrepaid.
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap()];

		let dest1 = 1;
		let dest2 = 2;

		// Valid split: value 0 (4 units) -> 2 coins of value -1 (2 units each)
		// This would succeed with max_fee: 0, but we use max_fee: 250 to trigger the error.
		let split_into = bounded_split(vec![(-1, vec![dest1, dest2])]);

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coins {
			aliases,
			value: 0,
			index,
			revision,
			split_into,
			max_fee: 250, // Non-zero max_fee with Prepaid mode should fail
		});

		// build_ext uses Prepaid fee mode (via AsUnloadTokenPeople)
		let ext = build_ext(call, 0, 0, &secrets, 0, index, None);

		// Transaction should be invalid at validation
		assert_invalid(ext, CustomInvalidity::MaxFeeNotAllowedForPrepaid);
	});
}

#[test]
fn prepaid_with_nonzero_max_fee_dispatch_fail() {
	// Test that direct dispatch with Prepaid fee mode and non-zero max_fee fails with
	// MaxFeeNotAllowedForPrepaid error.
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let aliases: BoundedVec<Alias, _> = secrets
			.iter()
			.map(|s| {
				CryptoOf::<Test>::alias_in_context(s, UNLOADING_RECYCLER_CONTEXT.as_ref()).unwrap()
			})
			.collect::<Vec<_>>()
			.try_into()
			.unwrap();

		let dest1 = 1;
		let dest2 = 2;

		// Valid split: value 0 (4 units) -> 2 coins of value -1 (2 units each)
		let split_into = bounded_split(vec![(-1, vec![dest1, dest2])]);

		// Build alias proofs for direct dispatch
		let msg_hash = [0u8; 32];
		let ring_members = Coinage::get_recycler_members(0, index);
		let mut alias_proofs_vec = Vec::new();
		for secret in &secrets {
			let member = CryptoOf::<Test>::member_from_secret(secret);
			let commitment = CryptoOf::<Test>::open(
				recycler_ring_size(),
				&member,
				ring_members.clone().into_iter(),
			)
			.unwrap();
			let (proof, _) = CryptoOf::<Test>::create(
				commitment,
				secret,
				UNLOADING_RECYCLER_CONTEXT.as_ref(),
				&msg_hash,
			)
			.unwrap();
			alias_proofs_vec.push(proof);
		}
		let alias_proofs = BoundedVec::try_from(alias_proofs_vec).expect("Too many proofs");

		// Direct dispatch with Prepaid fee mode and non-zero max_fee
		let result = Coinage::unload_recycler_into_coins(
			pallet::Origin::<Test>::UnloadToken {
				alias_proofs,
				proven_msg: msg_hash,
				fee: UnloadFee::Prepaid,
			}
			.into(),
			aliases,
			0,
			index,
			revision,
			split_into,
			250, // Non-zero max_fee with Prepaid mode should fail
		);

		assert_err!(result, Error::<Test>::MaxFeeNotAllowedForPrepaid);
	});
}

#[test]
fn from_output_fail_max_fee_not_multiple_of_min_coin() {
	// Test that max_fee must be an exact multiple of the minimum coin amount.
	// If not, dispatch fails with InvalidMaxFee (prevents silent truncation).
	//
	// Setup:
	// - MinimumExponent = -2, UNDERLYING_ASSET_UNIT = 1000
	// - coin_value_to_asset_amount(-2) = 1000 >> 2 = 250 asset units per min unit
	// - max_fee = 251 (not a multiple of 250, remainder = 1)
	//
	// Without the check, 251 / 250 would silently truncate to 1 unit,
	// breaking the accounting invariant.
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: CoinValue = 0;
		let (secrets, index, revision) = setup_recycler(value, 1, 0);
		let alias =
			CryptoOf::<Test>::alias_in_context(&secrets[0], UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();
		let aliases: BoundedVec<Alias, _> = bounded_vec![alias];

		let dest1 = 100;
		let dest2 = 101;

		// Input: 1 coin of value 0 = 4 units
		// If max_fee were 250 (1 unit), output would be 3 units
		// But we use 251 which is NOT a multiple of 250
		let split_into = bounded_split(vec![(-2, vec![dest2]), (-1, vec![dest1])]);

		let call = crate::Call::<Test>::unload_recycler_into_coins {
			aliases,
			value,
			index,
			revision,
			split_into,
			max_fee: 251, // 251 % 250 = 1, not a multiple
		};

		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets);

		// Validation passes (remainder check is in dispatch)
		assert_ok!(Executive::validate_transaction(
			TransactionSource::External,
			ext.clone(),
			Default::default()
		));

		// Dispatch fails with InvalidMaxFee due to remainder
		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "apply_extrinsic should succeed");
		assert_err!(result.unwrap(), Error::<Test>::InvalidMaxFee);
	});
}
