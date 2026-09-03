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
use frame_support::{
	assert_err, assert_ok,
	dispatch::GetDispatchInfo,
	traits::fungibles::{Inspect, MutateHold},
	BoundedVec,
};
use frame_system::AuthorizeCall;
use indiv_support::traits::{Alias, MembershipProver};
use sp_runtime::{bounded_vec, testing::UintAuthorityId, DispatchError};
use verifiable::GenerateVerifiable;

/// Mints assets to a temporary user and transfers them to the pallet account on hold.
/// This simulates the state after `load_recycler_with_external_asset`.
fn fund_pallet(amount: u64) {
	let temp_user = 9999;
	let asset_id = TEST_ASSET_ID;
	assert_ok!(Assets::mint(RuntimeOrigin::signed(1), asset_id, temp_user, amount));
	assert_ok!(AssetsWithHolder::transfer_and_hold(
		asset_id,
		&HoldReason::Wrapped.into(),
		&temp_user,
		&Coinage::pallet_account(),
		amount,
		frame_support::traits::tokens::Precision::Exact,
		frame_support::traits::tokens::Preservation::Expendable,
		frame_support::traits::tokens::Fortitude::Polite,
	));
}

/// Helper to build the unload extrinsic using AsUnloadTokenPeople extension.
fn build_unload_ext(
	call: RuntimeCall,
	period: u32,
	counter: u32,
	recycler_secrets: &[Secret],
	value: Denomination,
	index: u32,
	bad_proof: bool,
	people_alias_override: Option<Alias>,
) -> Extrinsic {
	// 1. Calculate Implication
	let inherited_implication = ((0u8, &call), (), ());
	let proven_msg = sp_io::hashing::blake2_256(&inherited_implication.encode());

	// 2. Generate Alias Proofs
	let mut alias_proofs_vec = Vec::new();
	let ring_members = Coinage::get_recycler_members(TEST_INSTANCE_ID, value, index);

	for secret in recycler_secrets {
		let member = CryptoOf::<Test>::member_from_secret(secret);

		let proof = if bad_proof {
			// Create a proof from a member NOT in the ring
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
	let people_alias = people_alias_override.unwrap_or([0u8; 32]);
	let people_proof = MembershipProof {
		context: context.to_vec(),
		msg: intent_msg.to_vec(),
		alias: people_alias,
	};

	let info =
		AsCoinageInfo::AsUnloadTokenPeople { proof: people_proof, period, counter, alias_proofs };

	let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(info)));
	Extrinsic::new_transaction(call, extension)
}

#[test]
fn wrong_origin_fail() {
	new_test_ext().execute_with(|| {
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: BoundedVec::new(),
			value: 0,
			index: 0,
			revision: 0,
			to: 1,
			max_fee: 0,
		});

		// Standard signed extension (no info -> pass-through -> Signed origin)
		let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(None));
		let uxt = Extrinsic::new_signed(call, 1, UintAuthorityId(1), extension);

		// Should fail because pallet expects Origin::UnloadToken
		assert_err!(Executive::apply_extrinsic(uxt).unwrap(), DispatchError::BadOrigin);
	});
}

#[test]
fn origin_alias_proofs_and_call_aliases_mismatch_fail() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);

		// Use a random alias that doesn't match the proof
		let wrong_alias = CryptoOf::<Test>::alias_in_context(
			&get_secret(99),
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![wrong_alias],
			value: 0,
			index,
			revision,
			to: 1,
			max_fee: 0,
		});

		// The extension will have the valid proof for the secret in setup_recycler,
		// but the call has wrong_alias. This triggers mismatch.
		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, false, None);

		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), Error::<Test>::ProofAndAliasMismatch);
	});
}

#[test]
fn wrong_instance_rejected_in_validation() {
	new_test_ext().execute_with(|| {
		// Build a valid recycler ring first, then pass a wrong instance id. Recyclers are
		// addressed by an identifier derived from the instance id and denomination, so a wrong
		// instance addresses a recycler that does not exist. The validator must reject the
		// unload before `prepare` consumes the free unload token, otherwise the user
		// permanently loses an allowance entry while dispatch fails.
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = secrets[0].clone();

		let alias =
			CryptoOf::<Test>::alias_in_context(&secret, UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID + 1,
			aliases: bounded_vec![alias],
			value: 0,
			index,
			revision,
			to: 1,
			max_fee: 0,
		});
		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, false, None);

		assert_invalid(ext, CustomInvalidity::InvalidRecyclerRevision);

		// Storage that `prepare` would have written is untouched.
		assert!(!crate::ConsumedFreeUnloadTokens::<Test>::contains_key(0u32, alias));
	});
}

#[test]
fn origin_alias_proofs_are_wrong_fail() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = secrets[0].clone();

		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value: 0,
			index,
			revision,
			to: 1,
			max_fee: 0,
		});

		// Build with bad_proof = true
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

		// Provide wrong revision in call
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value: 0,
			index,
			revision: revision + 1,
			to: 1,
			max_fee: 0,
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

		// Provide non-existent index
		let wrong_index = index + 1;
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value: 0,
			index: wrong_index,
			revision,
			to: 1,
			max_fee: 0,
		});

		let ext = build_unload_ext(call, 0, 0, &secrets, 0, wrong_index, true, None);
		// Note: This fails validation at the validate_recycler_revision check, which returns
		// InvalidRecyclerRevision if recycler is missing.
		assert_invalid(ext, CustomInvalidity::InvalidRecyclerRevision);
	});
}

#[test]
fn validate_alias_proof_rejects_removed_ring() {
	new_test_ext().execute_with(|| {
		// Build a one-member recycler so we can remove the ring after creating a valid proof.
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let identifier = Coinage::recycler_collection_identifier(TEST_INSTANCE_ID, 0);
		let ring_members = Coinage::get_recycler_members(TEST_INSTANCE_ID, 0, index);
		let proven_msg = [42u8; 32];
		let (proof, alias) = create_unload_proof(&secrets[0], &ring_members, &proven_msg);

		// Sanity check: the proof is valid while the recycler ring still exists.
		let validated_alias = RecyclerManager::<Test>::validate_alias_proof(
			TEST_INSTANCE_ID,
			0,
			index,
			revision,
			&proof,
			&proven_msg,
		)
		.expect("proof should be valid before ring removal");
		assert_eq!(validated_alias, alias);

		// Remove the recycler ring.
		indiv_pallet_members::CurrentRingIndex::<Test>::insert(identifier, index.saturating_add(1));
		assert_ok!(<Test as Config>::MemberService::remove_ring(&identifier, index));

		// Members rejects the proof because removed rings are instantly expired.
		<Members as MembershipProver>::verify_membership(
			&identifier,
			&proof,
			index,
			revision,
			UNLOADING_RECYCLER_CONTEXT,
			&proven_msg,
		)
		.expect_err("members should reject proofs from removed rings");

		// Coinage must also reject the same proof because a removed recycler is no longer
		// spendable.
		assert!(
			matches!(
				RecyclerManager::<Test>::validate_alias_proof(
					TEST_INSTANCE_ID,
					0,
					index,
					revision,
					&proof,
					&proven_msg,
				),
				Err(ValidateAliasProofError::InvalidRevision)
			),
			"coinage must reject removed recycler rings"
		);
	})
}

#[test]
fn unload_ext_invalid_when_ring_removed_even_if_old_root_retained() {
	new_test_ext().execute_with(|| {
		// Build a live recycler and capture a valid unload extrinsic before removing its ring.
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = secrets[0].clone();
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];
		let identifier = Coinage::recycler_collection_identifier(TEST_INSTANCE_ID, 0);

		// Use the old revision in the call so only the removed-ring policy decides validity.
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value: 0,
			index,
			revision,
			to: 1,
			max_fee: 0,
		});
		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, false, None);

		// Remove the ring but leave its retained old root available in members.
		indiv_pallet_members::CurrentRingIndex::<Test>::insert(identifier, index.saturating_add(1));
		assert_ok!(<Test as Config>::MemberService::remove_ring(&identifier, index));

		// The unload path must fail on coinage's spendability check, not on proof generation.
		assert_invalid(ext, CustomInvalidity::InvalidRecyclerRevision);
	})
}

#[test]
fn recycler_already_unloaded_fail() {
	new_test_ext().execute_with(|| {
		setup_asset();
		// Value 0 = 1000 units. Fund enough for one unload.
		fund_pallet(2000);

		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = secrets[0].clone();
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];

		// 1. Success unload
		let call1 = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: aliases.clone(),
			value: 0,
			index,
			revision,
			to: 1,
			max_fee: 0,
		});
		let ext1 = build_unload_ext(call1, 0, 0, &secrets, 0, index, false, Some([1; 32]));
		assert_eq!(Executive::apply_extrinsic(ext1), Ok(Ok(())));

		// 2. Retry unloading the same alias
		let call2 = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value: 0,
			index,
			revision,
			to: 2,
			max_fee: 0,
		});
		// Use a different People alias to bypass the UnloadToken double-spend check,
		// so we specifically hit RecyclerAlreadyUnloaded check.
		let ext2 = build_unload_ext(call2, 0, 0, &secrets, 0, index, false, Some([2; 32]));

		let res = Executive::apply_extrinsic(ext2);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), Error::<Test>::RecyclerAlreadyUnloaded);
	});
}

#[test]
fn success_with_one_alias() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_asset();
		// Value 0 = 1000 units.
		fund_pallet(1000);

		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = secrets[0].clone();
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];

		let dest = 1;
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: aliases.clone(),
			value: 0,
			index,
			revision,
			to: dest,
			max_fee: 0,
		});

		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, false, None);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Verify destination has the asset on hold
		// Note: The pallet transfers ON HOLD to the user.
		let balance_held = AssetsWithHolder::total_balance(TEST_ASSET_ID, &dest);
		assert_eq!(balance_held, 1000);

		// Verify recycler marked unloaded
		assert!(matches!(
			RecyclerAliasStates::<Test>::get((TEST_INSTANCE_ID, 0, index, aliases[0])),
			Some(AliasState::Unloaded),
		));
		System::assert_has_event(
			crate::Event::<Test>::RecyclerUnloadedIntoExternalAsset {
				instance_id: TEST_INSTANCE_ID,
				to: dest,
				value: 0,
				input_count: 1,
				amount: 1000,
			}
			.into(),
		);
	});
}

#[test]
fn prepaid_ignores_nonzero_max_fee() {
	// A prepaid unload token takes no fee out of the unloaded asset, so `max_fee` bounds nothing:
	// any value passes validation and dispatch, and the whole unloaded value goes to `to`.
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_asset();
		fund_pallet(1000);

		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];

		let dest = 1;
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value: 0,
			index,
			revision,
			to: dest,
			max_fee: 999,
		});

		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, false, None);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Nothing was withheld for a fee.
		assert_eq!(AssetsWithHolder::total_balance(TEST_ASSET_ID, &dest), 1000);
	});
}

// Prepaid external-asset unload refunds down to the `Prepaid` benchmarked weight via
// `PostDispatchInfo`, never above the charged worst case `max(prepaid, from_output)`.
#[test]
fn success_with_one_alias_refunds_prepaid() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_asset();
		fund_pallet(1000);
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let alias =
			CryptoOf::<Test>::alias_in_context(&secrets[0], UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();
		let aliases: BoundedVec<Alias, _> = bounded_vec![alias];

		// Build the alias proof for the Prepaid origin (all aliases are validated in the call).
		let msg_hash = [0u8; 32];
		let ring_members = Coinage::get_recycler_members(TEST_INSTANCE_ID, 0, index);
		let member = CryptoOf::<Test>::member_from_secret(&secrets[0]);
		let commitment =
			CryptoOf::<Test>::open(recycler_ring_size(), &member, ring_members.into_iter())
				.unwrap();
		let (proof, _) = CryptoOf::<Test>::create(
			commitment,
			&secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
			&msg_hash,
		)
		.unwrap();

		let post = Coinage::unload_recycler_into_external_asset(
			pallet::Origin::<Test>::UnloadToken {
				alias_proofs: bounded_vec![proof],
				proven_msg: msg_hash,
				fee: UnloadFee::Prepaid,
			}
			.into(),
			TEST_INSTANCE_ID,
			aliases,
			0,
			index,
			revision,
			1,
			0,
		)
		.expect("unload should succeed");

		assert_eq!(
			post.actual_weight,
			Some(
				Coinage::unload_recycler_into_external_asset_prepaid_weight(1)
					.saturating_add(<Test as Config>::WeightInfo::read_instance())
			),
		);
		assert!(post.actual_weight.unwrap().all_lte(
			Coinage::unload_recycler_into_external_asset_max_weight(1)
				.saturating_add(<Test as Config>::WeightInfo::read_instance())
		));
	});
}

#[test]
fn success_with_multiple_aliases() {
	new_test_ext().execute_with(|| {
		setup_asset();
		// Value 0 = 1000 units. 3 aliases = 3000 units.
		fund_pallet(3000);

		let (secrets, index, revision) = setup_recycler(0, 3, 0);

		let mut aliases = Vec::new();
		for s in &secrets {
			aliases.push(
				CryptoOf::<Test>::alias_in_context(s, UNLOADING_RECYCLER_CONTEXT.as_ref()).unwrap(),
			);
		}

		let dest = 1;
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: aliases.clone().try_into().unwrap(),
			value: 0,
			index,
			revision,
			to: dest,
			max_fee: 0,
		});

		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, false, None);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Verify destination has the asset on hold
		let balance_held = AssetsWithHolder::total_balance(TEST_ASSET_ID, &dest);
		assert_eq!(balance_held, 3000);

		// Verify recycler marked unloaded
		for alias in aliases {
			assert!(matches!(
				RecyclerAliasStates::<Test>::get((TEST_INSTANCE_ID, 0, index, alias)),
				Some(AliasState::Unloaded),
			));
		}
	});
}

#[test]
fn success_with_previous_revision() {
	new_test_ext().execute_with(|| {
		setup_asset();
		fund_pallet(1000);

		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = secrets[0].clone();
		let aliases: BoundedVec<Alias, _> = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];

		// Capture the ring members BEFORE adding more coins
		let ring_members_v1 = Coinage::get_recycler_members(TEST_INSTANCE_ID, 0, index);

		// Step 2: Add more coins and trigger ring rebuild
		let new_secret = get_secret(100);
		let new_member = CryptoOf::<Test>::member_from_secret(&new_secret);
		assert_ok!(RecyclerManager::<Test>::load(TEST_INSTANCE_ID, 0, new_member));
		Members::process_maintenance();

		// Verify revision has increased
		let identifier = Coinage::recycler_collection_identifier(TEST_INSTANCE_ID, 0);
		let new_rev = <Test as Config>::MemberService::ring_revision(&identifier, index).unwrap();
		assert!(new_rev > revision, "revision should have increased after rebuild");

		// Step 3: Use proof generated against the OLD revision (previous_root)
		let dest = 1;
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: aliases.clone(),
			value: 0,
			index,
			revision, // Using the OLD revision
			to: dest,
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
		let people_proof = MembershipProof {
			context: context.to_vec(),
			msg: intent_msg.to_vec(),
			alias: [0u8; 32],
		};

		let info = AsCoinageInfo::AsUnloadTokenPeople {
			proof: people_proof,
			period: 0,
			counter: 0,
			alias_proofs,
		};

		let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(info)));
		let ext = Extrinsic::new_transaction(call, extension);

		// Should succeed because the previous revision's root is still valid
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Verify destination has the asset
		let balance_held = AssetsWithHolder::total_balance(TEST_ASSET_ID, &dest);
		assert_eq!(balance_held, 1000);

		// Verify recycler marked unloaded
		assert!(matches!(
			RecyclerAliasStates::<Test>::get((TEST_INSTANCE_ID, 0, index, aliases[0])),
			Some(AliasState::Unloaded),
		));
	});
}

/// Sponsored-instance settle flow of `unload_recycler_into_external_asset` through the given
/// prepaid unload-token extension flavor: unloading releases the key's deposit, and after a
/// switch to sufficient the remaining sponsored-loaded key still unloads while settling
/// nothing.
fn sponsored_unload_settles(make_variant: impl FnOnce() -> UnloadTokenVariant) {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let variant = make_variant();
		let (instance_id, secrets, index, revision) = setup_sponsored_recycler(10, 100, 2, 0);
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 20);

		// Unloading one key releases its deposit to the pot's free balance.
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id,
			aliases: bounded_vec![recycler_alias(&secrets[0])],
			value: 0,
			index,
			revision,
			to: 9_201,
			max_fee: 0,
		};
		let free_before = pot_free(instance_id, NATIVE_DEPOSIT_ID);
		let ext = variant.build_ext(instance_id, call, &secrets[0..1], 0, index, revision, 0);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 10);
		assert_eq!(pot_free(instance_id, NATIVE_DEPOSIT_ID), free_before + 10);
		check_load_deposit_invariant(instance_id, 1);

		// Switching to sufficient releases the remaining deposit; the other key, loaded while
		// the instance was sponsored, still unloads and settles nothing.
		assert_ok!(Coinage::make_instance_sufficient(RuntimeOrigin::root(), instance_id));
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 0);
		let free_after_switch = pot_free(instance_id, NATIVE_DEPOSIT_ID);
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id,
			aliases: bounded_vec![recycler_alias(&secrets[1])],
			value: 0,
			index,
			revision,
			to: 9_202,
			max_fee: 0,
		};
		let ext = variant.build_ext(instance_id, call, &secrets[1..2], 0, index, revision, 1);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));
		assert_eq!(AssetsWithHolder::total_balance(SPONSORED_ASSET_ID, &9_202), 1000);
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 0);
		assert_eq!(pot_free(instance_id, NATIVE_DEPOSIT_ID), free_after_switch);
	});
}

#[test]
fn sponsored_unload_settles_the_load_deposit_people_token() {
	sponsored_unload_settles(|| UnloadTokenVariant::People);
}

#[test]
fn sponsored_unload_settles_the_load_deposit_lite_people_token() {
	sponsored_unload_settles(|| UnloadTokenVariant::LitePeople);
}

#[test]
fn sponsored_unload_settles_the_load_deposit_paid_token() {
	sponsored_unload_settles(|| paid_unload_token_variant(2));
}

#[test]
fn sponsored_ring_lifecycle_settles_oldest_first_and_at_archival() {
	/// The load deposit price used across this test.
	const PRICE: u64 = 10;

	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let instance_id = setup_sponsored_instance();
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 20_000);

		let value: Denomination = 0;
		let ring_capacity = R2E10_RING_CAPACITY;
		let (secrets, index, _revision) =
			setup_recycler_for(instance_id, SPONSORED_ASSET_ID, value, ring_capacity + 1, 0);
		for _ in 0..10 {
			Members::process_maintenance();
		}
		// The extra maintenance rounds onboarded more members, so re-fetch the revision the
		// proofs must be built against.
		let revision = <Test as Config>::MemberService::ring_revision(
			&Coinage::recycler_collection_identifier(instance_id, value),
			index,
		)
		.expect("ring exists");
		let loaded = ring_capacity + 1;
		check_load_deposit_invariant(instance_id, loaded);
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), u64::from(loaded) * PRICE);

		let ring_members = Coinage::get_recycler_members(instance_id, value, index);
		let identifier = Coinage::recycler_collection_identifier(instance_id, value);
		let immutable_since = <Test as Config>::MemberService::ring_status(&identifier, index)
			.unwrap()
			.immutable_since
			.unwrap() as u32;

		// Unload one key: the deposit is released in full to the pot.
		let unload_one = |secret: &Secret, dest: u64| {
			let proven_msg = [1u8; 32];
			let (proof, alias) = create_unload_proof(secret, &ring_members, &proven_msg);
			assert_ok!(Coinage::unload_recycler_into_external_asset(
				Origin::UnloadToken {
					alias_proofs: bounded_vec![proof],
					proven_msg,
					fee: UnloadFee::Prepaid
				}
				.into(),
				instance_id,
				bounded_vec![alias],
				value,
				index,
				revision,
				dest,
				0,
			));
			alias
		};

		let free_before = pot_free(instance_id, NATIVE_DEPOSIT_ID);
		let alias_0 = unload_one(&secrets[0], 8_881);
		assert_eq!(pot_free(instance_id, NATIVE_DEPOSIT_ID), free_before + PRICE);
		check_load_deposit_invariant(instance_id, loaded - 1);
		System::assert_has_event(
			crate::Event::<Test>::LoadDepositsReleased {
				instance_id,
				currency: NATIVE_DEPOSIT_ID,
				amount: PRICE,
				count: 1,
			}
			.into(),
		);

		// A price change plus a load rotates the current tier into the old slot.
		set_load_deposit(NATIVE_DEPOSIT_ID, 12);
		setup_recycler_for(instance_id, SPONSORED_ASSET_ID, value, 1, 50);
		assert_eq!(
			old_tier(instance_id),
			Some(DepositTier { asset_id: NATIVE_DEPOSIT_ID, price: PRICE, count: loaded - 1 })
		);
		assert_eq!(
			current_tier(instance_id),
			Some(DepositTier { asset_id: NATIVE_DEPOSIT_ID, price: 12, count: 1 })
		);

		// Settlement drains the old tier, whatever key the unload nominally stood for.
		let alias_1 = unload_one(&secrets[1], 8_882);
		assert_eq!(
			old_tier(instance_id),
			Some(DepositTier { asset_id: NATIVE_DEPOSIT_ID, price: PRICE, count: loaded - 2 })
		);
		assert_eq!(current_tier(instance_id).map(|tier| tier.count), Some(1));
		check_load_deposit_invariant(instance_id, loaded - 1);

		// Ring cleanup settles the deposits of every key it archives, oldest tier first: the
		// archived coins no longer occupy a ring, so the pot stops collateralizing them. Only the
		// two keys sitting in the still-live rings keep a deposit.
		let recycler_root =
			Coinage::recycler_ring_root(instance_id, value, index).expect("ring has a root");
		let free_before = pot_free(instance_id, NATIVE_DEPOSIT_ID);
		let expiration = get_u32::<<Test as crate::Config>::RecyclerExpirationTime>();
		advance_until_time(immutable_since + expiration);
		advance_block();
		advance_block();
		let archived = RecyclersArchives::<Test>::get((instance_id, value, index))
			.expect("cleaned ring with remaining coins is archived");
		let archived_count = ring_capacity - 2;
		assert_eq!(archived.remaining, archived_count);
		assert_eq!(
			old_tier(instance_id),
			Some(DepositTier { asset_id: NATIVE_DEPOSIT_ID, price: PRICE, count: 1 })
		);
		assert_eq!(
			current_tier(instance_id),
			Some(DepositTier { asset_id: NATIVE_DEPOSIT_ID, price: 12, count: 1 })
		);
		check_load_deposit_invariant(instance_id, 2);
		assert_eq!(
			pot_free(instance_id, NATIVE_DEPOSIT_ID),
			free_before + u64::from(archived_count) * PRICE
		);
		System::assert_has_event(
			crate::Event::<Test>::LoadDepositsReleased {
				instance_id,
				currency: NATIVE_DEPOSIT_ID,
				amount: u64::from(archived_count) * PRICE,
				count: archived_count,
			}
			.into(),
		);

		// Recovering from the archive settles nothing: the deposit went back at cleanup.
		let signer = ALICE;
		fund_native(signer, 1_000_000);
		fund_native(FEE_DESTINATION, 1_000);
		let ledger_before = (current_tier(instance_id), old_tier(instance_id));
		let held_before = pot_held(instance_id, NATIVE_DEPOSIT_ID);
		let free_before = pot_free(instance_id, NATIVE_DEPOSIT_ID);
		let proven_msg = Coinage::unload_archived_proof_message(&signer);
		let (alias_proof, alias) = create_unload_proof(&secrets[10], &ring_members, &proven_msg);
		let unloaded = vec![alias_0, alias_1];
		let (unloaded_root, proof_nodes) =
			testing_utils::unloaded_root_and_non_inclusion_proof(&unloaded, &alias);
		let declared = crate::Call::<Test>::unload_archived_recycler_into_external_asset {
			instance_id,
			value,
			index,
			recycler_root: recycler_root.clone(),
			unloaded_root,
			alias_proof: alias_proof.clone(),
			non_inclusion_proof: testing_utils::to_bounded_proof(proof_nodes.clone()),
			to: 8_883,
			fee_currency: FeeCurrency::Native,
			max_fee: native_max_fee_bound(),
		}
		.get_dispatch_info()
		.call_weight;
		assert_ok!(Coinage::unload_archived_recycler_into_external_asset(
			RuntimeOrigin::signed(signer),
			instance_id,
			value,
			index,
			recycler_root,
			unloaded_root,
			alias_proof,
			testing_utils::to_bounded_proof(proof_nodes),
			8_883,
			FeeCurrency::Native,
			native_max_fee_bound()
		));

		// A recovery carries no settlement surcharge, and leaves the pot exactly as it was.
		assert_eq!(
			declared,
			<Test as Config>::WeightInfo::unload_archived_recycler_into_external_asset()
		);
		assert_eq!((current_tier(instance_id), old_tier(instance_id)), ledger_before);
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), held_before);
		assert_eq!(pot_free(instance_id, NATIVE_DEPOSIT_ID), free_before);
		check_load_deposit_invariant(instance_id, 2);

		// Nothing stays held for the keys that were never recovered, however much time passes.
		advance_until_time(immutable_since + 100 * expiration);
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), PRICE + 12);
	});
}
