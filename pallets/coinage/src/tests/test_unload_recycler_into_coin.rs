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
	*,
};
use codec::Encode;
use frame_support::{assert_err, assert_ok, BoundedVec};
use frame_system::AuthorizeCall;
use indiv_support::traits::Alias;
use sp_runtime::{bounded_vec, testing::UintAuthorityId, DispatchError};
use verifiable::GenerateVerifiable;

/// Helper to build the unload extrinsic using AsUnloadTokenPeople extension.
/// `people_alias_override` allows simulating different users (UnloadToken identities).
fn build_unload_ext(
	call: RuntimeCall,
	period: u32,
	counter: u32,
	recycler_secrets: &[Secret],
	value: CoinValue,
	index: u32,
	bad_proof: bool,
	people_alias_override: Option<Alias>,
) -> Extrinsic {
	// 1. Calculate Implication
	// The transaction extension pipeline is (AuthorizeCall, AsCoinage).
	// AuthorizeCall has Val=() and Implicit=().
	// The implication is ((BaseImplication), Val_AuthorizeCall, Implicit_AuthorizeCall).
	let inherited_implication = ((0u8, &call), (), ());
	let proven_msg = sp_io::hashing::blake2_256(&inherited_implication.encode());

	// 2. Generate Alias Proofs (must be created before the people proof)
	let mut alias_proofs_vec = Vec::new();
	let ring_members = Coinage::get_recycler_members(value, index);

	for secret in recycler_secrets {
		let member = CryptoOf::<Test>::member_from_secret(secret);

		let proof = if bad_proof {
			// Just create a bad proof from an entirely different ring.
			let another_secret = CryptoOf::<Test>::new_secret([199u8; 32]);
			let another_member = CryptoOf::<Test>::member_from_secret(&another_secret);
			let commitment = CryptoOf::<Test>::open(
				recycler_ring_size(),
				&another_member,
				vec![another_member].into_iter(),
			)
			.unwrap();
			CryptoOf::<Test>::create(
				commitment,
				&another_secret,
				UNLOADING_RECYCLER_CONTEXT.as_ref(),
				&proven_msg,
			)
			.unwrap()
			.0
		} else {
			let commitment = CryptoOf::<Test>::open(
				recycler_ring_size(),
				&member,
				ring_members.clone().into_iter(),
			)
			.unwrap();
			CryptoOf::<Test>::create(
				commitment,
				secret,
				UNLOADING_RECYCLER_CONTEXT.as_ref(),
				&proven_msg,
			)
			.unwrap()
			.0
		};

		alias_proofs_vec.push(proof);
	}

	let alias_proofs = BoundedVec::try_from(alias_proofs_vec).expect("Too many proofs");

	// 3. Generate People Proof (Mock)
	let intent_msg = sp_io::hashing::blake2_256(
		&[alias_proofs.encode(), inherited_implication.encode()].concat(),
	);
	let context = crate::pallet::free_unload_token_context(period, counter);
	// Use explicit alias or default. Different alias = different user.
	let people_alias = people_alias_override.unwrap_or([0u8; 32]);
	let people_proof =
		PeopleProof { context: context.to_vec(), msg: intent_msg.to_vec(), alias: people_alias };

	let info =
		AsCoinageInfo::AsUnloadTokenPeople { proof: people_proof, period, counter, alias_proofs };

	let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(info)));
	Extrinsic::new_signed(call, 0, UintAuthorityId(0), extension)
}

#[test]
fn bad_origin_fail() {
	new_test_ext().execute_with(|| {
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			aliases: BoundedVec::new(),
			value: 0,
			index: 0,
			revision: 0,
			to: 1,
		});

		// Standard signed extension (no info -> Signed origin)
		let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(None));
		let uxt = Extrinsic::new_signed(call, 1, UintAuthorityId(1), extension);

		assert_err!(Executive::apply_extrinsic(uxt).unwrap(), DispatchError::BadOrigin);
	});
}

#[test]
fn invalid_alias_proof_fail() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = secrets[0].clone();

		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			aliases,
			value: 0,
			index,
			revision,
			to: 1,
		});

		// Build the extension with bad_proof = true
		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, true, None);

		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), Error::<Test>::InvalidAliasProof);
	});
}

#[test]
fn outdated_revision_invalid() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = secrets[0].clone();
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];

		// Use wrong revision
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			aliases,
			value: 0,
			index,
			revision: revision + 1,
			to: 1,
		});

		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, false, None);
		assert_invalid(ext, CustomInvalidity::InvalidRecyclerRevision);
	});
}

#[test]
fn recycler_not_exist_invalid() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = secrets[0].clone();
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];

		// Use wrong index
		let wrong_index = index + 1;
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			aliases,
			value: 0,
			index: wrong_index,
			revision,
			to: 1,
		});

		let ext = build_unload_ext(call, 0, 0, &secrets, 0, wrong_index, true, None);
		assert_invalid(ext, CustomInvalidity::InvalidRecyclerRevision);
	});
}

#[test]
fn alias_already_used_fail() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = secrets[0].clone();
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];

		// First unload success. User 1.
		let call1 = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			aliases: aliases.clone(),
			value: 0,
			index,
			revision,
			to: 1,
		});
		let ext1 = build_unload_ext(call1.clone(), 0, 0, &secrets, 0, index, false, Some([1; 32]));
		assert_eq!(Executive::apply_extrinsic(ext1), Ok(Ok(())));

		// The second unload fails (RecyclerAlreadyUnloaded).
		// Must use a different People alias (User 2) to pass the UnloadTokenConsumed check in
		// validation.
		let call2 = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			aliases: aliases.clone(),
			value: 0,
			index,
			revision,
			to: 2,
		});
		let ext2 = build_unload_ext(call2, 0, 0, &secrets, 0, index, false, Some([2; 32]));

		let res = Executive::apply_extrinsic(ext2);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), Error::<Test>::RecyclerAlreadyUnloaded);
	});
}

#[test]
fn duplicate_alias_in_single_tx_fails() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 2, 0);
		let repeated_secret = secrets[0].clone();
		let repeated_alias = CryptoOf::<Test>::alias_in_context(
			&repeated_secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			aliases: bounded_vec![repeated_alias, repeated_alias],
			value: 0,
			index,
			revision,
			to: 1,
		});

		// Use the same proof source twice so both proofs are individually valid
		// but map to the same alias within one transaction.
		let ext = build_unload_ext(
			call,
			0,
			0,
			&[repeated_secret.clone(), repeated_secret],
			0,
			index,
			false,
			None,
		);

		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), Error::<Test>::RecyclerAlreadyUnloaded);
	});
}

#[test]
fn proof_alias_mismatch_fail() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		// Pass wrong alias in call (e.g. from random secret)
		let wrong_alias = CryptoOf::<Test>::alias_in_context(
			&get_secret(99),
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			aliases: bounded_vec![wrong_alias],
			value: 0,
			index,
			revision,
			to: 1,
		});

		// Proof is derived from `secrets[0]` which corresponds to the correct member,
		// but does not match `wrong_alias`.
		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, false, None);

		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), Error::<Test>::ProofAndAliasMismatch);
	});
}

#[test]
fn dest_already_used_invalid() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = secrets[0].clone();
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];

		let dest = 1;
		// Destination already has coin
		CoinsByOwner::<Test>::insert(dest, Coin { value: 0, age: 0 });

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			aliases,
			value: 0,
			index,
			revision,
			to: dest,
		});

		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, false, None);
		assert_invalid(ext, CustomInvalidity::AddressAlreadyHasCoin);
	});
}

#[test]
fn result_too_big_fail() {
	new_test_ext().execute_with(|| {
		let max_exp = <Test as Config>::MaximumExponent::get();
		let (secrets, index, revision) = setup_recycler(max_exp, 2, 0);

		let mut aliases = Vec::new();
		for s in &secrets {
			aliases.push(
				CryptoOf::<Test>::alias_in_context(s, UNLOADING_RECYCLER_CONTEXT.as_ref()).unwrap(),
			);
		}

		// Consolidate 2 coins of MaxExponent -> MaxExponent + 1 (Too big)
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			aliases: aliases.try_into().unwrap(),
			value: max_exp,
			index,
			revision,
			to: 1,
		});

		let ext = build_unload_ext(call, 0, 0, &secrets, max_exp, index, false, None);
		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), Error::<Test>::ConsolidationTooBig);
	});
}

#[test]
fn not_power_of_two_fail() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 3, 0);

		let mut aliases = Vec::new();
		for s in &secrets {
			aliases.push(
				CryptoOf::<Test>::alias_in_context(s, UNLOADING_RECYCLER_CONTEXT.as_ref()).unwrap(),
			);
		}

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			aliases: aliases.try_into().unwrap(),
			value: 0,
			index,
			revision,
			to: 1,
		});

		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, false, None);
		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), Error::<Test>::InvalidConsolidation);
	});
}

#[test]
fn empty_aliases_fail() {
	new_test_ext().execute_with(|| {
		// Setup a recycler so it exists
		let (_secrets, index, revision) = setup_recycler(0, 1, 0);

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			aliases: BoundedVec::new(),
			value: 0,
			index,
			revision,
			to: 1,
		});

		// Build extension with no alias proofs (empty)
		let ext = build_unload_ext(call, 0, 0, &[], 0, index, false, None);
		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), Error::<Test>::EmptyInputs);
	});
}

#[test]
fn success_one() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = secrets[0].clone();
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];

		let dest = 1;
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			aliases,
			value: 0,
			index,
			revision,
			to: dest,
		});

		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, false, None);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		let coin = CoinsByOwner::<Test>::get(dest).unwrap();
		assert_eq!(coin, Coin { value: 0, age: 0 });

		// Verify alias was recorded as unloaded
		let alias =
			CryptoOf::<Test>::alias_in_context(&secret, UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();
		assert!(RecyclersUnloaded::<Test>::contains_key((0i8, index, alias)));
		System::assert_has_event(
			crate::Event::<Test>::RecyclerUnloadedIntoCoin {
				to: dest,
				input_value: 0,
				output_value: 0,
				input_count: 1,
			}
			.into(),
		);
	});
}

#[test]
fn success_many() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 4, 0);

		let mut aliases = Vec::new();
		for s in &secrets {
			aliases.push(
				CryptoOf::<Test>::alias_in_context(s, UNLOADING_RECYCLER_CONTEXT.as_ref()).unwrap(),
			);
		}

		let dest = 1;
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			aliases: aliases.try_into().unwrap(),
			value: 0,
			index,
			revision,
			to: dest,
		});

		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, false, None);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		let coin = CoinsByOwner::<Test>::get(dest).unwrap();
		// 4 inputs of value 0 -> value 0 + log2(4) = 2
		assert_eq!(coin, Coin { value: 2, age: 0 });

		// Verify all aliases were recorded as unloaded
		for s in &secrets {
			let alias =
				CryptoOf::<Test>::alias_in_context(s, UNLOADING_RECYCLER_CONTEXT.as_ref()).unwrap();
			assert!(RecyclersUnloaded::<Test>::contains_key((0i8, index, alias)));
		}
	});
}

#[test]
fn success_max_consolidation() {
	new_test_ext().execute_with(|| {
		let count = MAX_CONSOLIDATION;

		// Use minimum exponent so result stays within bounds
		let base_value = get_i8::<MinimumExponent>();
		let result_value = base_value + count.ilog2() as i8;

		let (secrets, index, revision) = setup_recycler(base_value, count, 0);

		let mut aliases = Vec::new();
		for s in &secrets {
			aliases.push(
				CryptoOf::<Test>::alias_in_context(s, UNLOADING_RECYCLER_CONTEXT.as_ref()).unwrap(),
			);
		}

		let dest = 1;
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			aliases: aliases.try_into().unwrap(),
			value: base_value,
			index,
			revision,
			to: dest,
		});

		let ext = build_unload_ext(call, 0, 0, &secrets, base_value, index, false, None);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		let coin = CoinsByOwner::<Test>::get(dest).unwrap();
		assert_eq!(coin, Coin { value: result_value, age: 0 });

		// Verify all aliases were recorded as unloaded
		for s in &secrets {
			let alias =
				CryptoOf::<Test>::alias_in_context(s, UNLOADING_RECYCLER_CONTEXT.as_ref()).unwrap();
			assert!(RecyclersUnloaded::<Test>::contains_key((base_value, index, alias)));
		}
	});
}

#[test]
fn success_with_previous_revision() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);

		// Step 1: Setup recycler with initial coin and build revision 0.
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = secrets[0].clone();
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];

		// Capture the original ring members before rotating the root.
		let ring_members_v1 = Coinage::get_recycler_members(0, index);

		// Step 2: Add another recycler member through the pallet API and rebuild.
		let extra_user = 20_000u64;
		let asset_id = TEST_ASSET_ID;
		let amount = Coinage::coin_value_to_asset_amount(0).unwrap();
		let new_secret = get_secret(100);
		let new_member = CryptoOf::<Test>::member_from_secret(&new_secret);
		let new_member_proof = CryptoOf::<Test>::sign(&new_secret, &extra_user.encode()).unwrap();

		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), asset_id, extra_user, amount));
		assert_ok!(Coinage::load_recycler_with_external_asset(
			RuntimeOrigin::signed(extra_user),
			crate::pallet::CodecPreservation::Expendable,
			0,
			new_member,
			new_member_proof
		));
		Members::process_maintenance();

		let identifier = Coinage::recycler_collection_identifier(0);
		let new_revision =
			<Test as Config>::MemberService::ring_revision(&identifier, index).unwrap();
		assert!(new_revision > revision, "revision should increase after rebuild");

		// Step 3: Use proof generated against the old revision.
		let dest = 1;
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			aliases: aliases.clone(),
			value: 0,
			index,
			revision,
			to: dest,
		});

		let inherited_implication = ((0u8, &call), (), ());
		let proven_msg = sp_io::hashing::blake2_256(&inherited_implication.encode());

		// Create alias proofs first (they sign proven_msg)
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let commitment =
			CryptoOf::<Test>::open(recycler_ring_size(), &member, ring_members_v1.into_iter())
				.unwrap();
		let (proof, _) = CryptoOf::<Test>::create(
			commitment,
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
			&proven_msg,
		)
		.unwrap();

		let alias_proofs = BoundedVec::truncate_from(vec![proof]);

		// Compute intent_msg from alias_proofs + inherited_implication
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
		let ext = Extrinsic::new_transaction(call, extension);

		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		let coin = CoinsByOwner::<Test>::get(dest).unwrap();
		assert_eq!(coin, Coin { value: 0, age: 0 });
		assert!(RecyclersUnloaded::<Test>::contains_key((0i8, index, aliases[0])));
	});
}

/// Test unloading from a full recycler (767 members, from RingExponent::R2e10).
///
/// This verifies that recyclers filled to capacity work correctly for unloading.
/// When loading fills a recycler, the head index advances - this test ensures
/// the recycler is built and indexed correctly in that scenario.
#[test]
fn success_unload_from_full_recycler() {
	new_test_ext().execute_with(|| {
		// Setup a full recycler (767 members, i.e. ring capacity from RingExponent::R2e10)
		let (secrets, index, revision) = setup_recycler(0, R2E10_RING_CAPACITY, 0);

		// Verify we got the first recycler (index 0)
		assert_eq!(index, 0);

		// Unload a single member
		let secret = secrets[0].clone();
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];

		let dest = 1;
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			aliases,
			value: 0,
			index,
			revision,
			to: dest,
		});

		let ext =
			build_unload_ext(call, 0, 0, std::slice::from_ref(&secret), 0, index, false, None);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Verify unload succeeded
		let coin = CoinsByOwner::<Test>::get(dest).unwrap();
		assert_eq!(coin, Coin { value: 0, age: 0 });

		// Verify alias was recorded as unloaded
		let alias =
			CryptoOf::<Test>::alias_in_context(&secret, UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();
		assert!(RecyclersUnloaded::<Test>::contains_key((0i8, index, alias)));
	});
}
