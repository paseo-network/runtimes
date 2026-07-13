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
	value: CoinValue,
	index: u32,
	bad_proof: bool,
	people_alias_override: Option<Alias>,
) -> Extrinsic {
	// 1. Calculate Implication
	let inherited_implication = ((0u8, &call), (), ());
	let proven_msg = sp_io::hashing::blake2_256(&inherited_implication.encode());

	// 2. Generate Alias Proofs
	let mut alias_proofs_vec = Vec::new();
	let ring_members = Coinage::get_recycler_members(value, index);

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
	let people_proof =
		PeopleProof { context: context.to_vec(), msg: intent_msg.to_vec(), alias: people_alias };

	let info =
		AsCoinageInfo::AsUnloadTokenPeople { proof: people_proof, period, counter, alias_proofs };

	let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(info)));
	Extrinsic::new_transaction(call, extension)
}

#[test]
fn wrong_origin_fail() {
	new_test_ext().execute_with(|| {
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			aliases: BoundedVec::new(),
			value: 0,
			index: 0,
			revision: 0,
			to: 1,
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
			aliases: bounded_vec![wrong_alias],
			value: 0,
			index,
			revision,
			to: 1,
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
fn asset_id_not_set_rejected_in_validation() {
	new_test_ext().execute_with(|| {
		// Build a valid recycler ring first (this needs `UnderlyingAssetId` set), then
		// clear the storage. The validator must reject the unload before `prepare`
		// consumes the free unload token — otherwise the user permanently loses an
		// allowance entry while dispatch fails inside `transfer_external_asset`.
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = secrets[0].clone();
		crate::UnderlyingAssetId::<Test>::kill();

		let alias =
			CryptoOf::<Test>::alias_in_context(&secret, UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();

		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			aliases: bounded_vec![alias],
			value: 0,
			index,
			revision,
			to: 1,
		});
		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, false, None);

		assert_invalid(ext, CustomInvalidity::AssetIdNotSet);

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
			aliases,
			value: 0,
			index,
			revision,
			to: 1,
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

		// Provide non-existent index
		let wrong_index = index + 1;
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			aliases,
			value: 0,
			index: wrong_index,
			revision,
			to: 1,
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
		let identifier = Coinage::recycler_collection_identifier(0);
		let ring_members = Coinage::get_recycler_members(0, index);
		let proven_msg = [42u8; 32];
		let (proof, alias) = create_unload_proof(&secrets[0], &ring_members, &proven_msg);

		// Sanity check: the proof is valid while the recycler ring still exists.
		let validated_alias =
			RecyclerManager::<Test>::validate_alias_proof(0, index, revision, &proof, &proven_msg)
				.expect("proof should be valid before ring removal");
		assert_eq!(validated_alias, alias);

		// Remove the recycler ring.
		indiv_pallet_members::CurrentRingIndex::<Test>::insert(identifier, index.saturating_add(1));
		assert_ok!(<Test as Config>::MemberService::remove_ring(&identifier, index));

		// Members rejects the proof because removed rings are instantly expired.
		<Members as MembershipProver>::verify_membership_at_rev(
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
		let identifier = Coinage::recycler_collection_identifier(0);

		// Use the old revision in the call so only the removed-ring policy decides validity.
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			aliases,
			value: 0,
			index,
			revision,
			to: 1,
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
			aliases: aliases.clone(),
			value: 0,
			index,
			revision,
			to: 1,
		});
		let ext1 = build_unload_ext(call1, 0, 0, &secrets, 0, index, false, Some([1; 32]));
		assert_eq!(Executive::apply_extrinsic(ext1), Ok(Ok(())));

		// 2. Retry unloading the same alias
		let call2 = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			aliases,
			value: 0,
			index,
			revision,
			to: 2,
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
			aliases: aliases.clone(),
			value: 0,
			index,
			revision,
			to: dest,
		});

		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, false, None);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Verify destination has the asset on hold
		// Note: The pallet transfers ON HOLD to the user.
		let balance_held = AssetsWithHolder::total_balance(TEST_ASSET_ID, &dest);
		assert_eq!(balance_held, 1000);

		// Verify recycler marked unloaded
		assert!(RecyclersUnloaded::<Test>::contains_key((0, index, aliases[0])));
		System::assert_has_event(
			crate::Event::<Test>::RecyclerUnloadedIntoExternalAsset {
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
			aliases: aliases.clone().try_into().unwrap(),
			value: 0,
			index,
			revision,
			to: dest,
		});

		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, false, None);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Verify destination has the asset on hold
		let balance_held = AssetsWithHolder::total_balance(TEST_ASSET_ID, &dest);
		assert_eq!(balance_held, 3000);

		// Verify recycler marked unloaded
		for alias in aliases {
			assert!(RecyclersUnloaded::<Test>::contains_key((0, index, alias)));
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
		let ring_members_v1 = Coinage::get_recycler_members(0, index);

		// Step 2: Add more coins and trigger ring rebuild
		let new_secret = get_secret(100);
		let new_member = CryptoOf::<Test>::member_from_secret(&new_secret);
		assert_ok!(RecyclerManager::<Test>::load(0, new_member));
		Members::process_maintenance();

		// Verify revision has increased
		let identifier = Coinage::recycler_collection_identifier(0);
		let new_rev = <Test as Config>::MemberService::ring_revision(&identifier, index).unwrap();
		assert!(new_rev > revision, "revision should have increased after rebuild");

		// Step 3: Use proof generated against the OLD revision (previous_root)
		let dest = 1;
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			aliases: aliases.clone(),
			value: 0,
			index,
			revision, // Using the OLD revision
			to: dest,
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
		let ext = Extrinsic::new_transaction(call, extension);

		// Should succeed because the previous revision's root is still valid
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Verify destination has the asset
		let balance_held = AssetsWithHolder::total_balance(TEST_ASSET_ID, &dest);
		assert_eq!(balance_held, 1000);

		// Verify recycler marked unloaded
		assert!(RecyclersUnloaded::<Test>::contains_key((0, index, aliases[0])));
	});
}
