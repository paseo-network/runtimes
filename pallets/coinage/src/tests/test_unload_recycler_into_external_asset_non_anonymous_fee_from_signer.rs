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

//! Tests for `unload_recycler_into_external_asset_non_anonymous` and
//! `unload_recyclers_into_external_asset_non_anonymous` extrinsics.
//!
//! Test naming convention:
//! - `*_call` - tests the pallet call directly via `RuntimeOrigin::signed()`
//! - `*_extrinsic` - tests the full flow via `Executive::apply_extrinsic()`

use crate::{
	mock,
	mock::*,
	pallet::{Error, RecyclersUnloaded},
	*,
};
use codec::Encode;
use frame_support::{assert_noop, assert_ok, traits::Currency};
use sp_io::hashing::blake2_256;
use sp_runtime::{bounded_vec, DispatchError};
use verifiable::GenerateVerifiable;

#[test]
fn asset_id_not_set_native_fee_rejected_in_validation() {
	new_test_ext().execute_with(|| {
		// Set up a recycler ring + fund accounts, then clear `UnderlyingAssetId` storage so
		// the validator must reject the tx before dispatch hits `transfer_external_asset`.
		// With `FeeCurrency::Native` the validator previously skipped the asset-id check —
		// dispatch would fail and the signer would still pay the inclusion fee.
		setup_balances();
		let dest = CHARLIE;
		let signer = ALICE;
		let (input, proofs, _) = setup_single_unload(0, dest, signer, 0);
		crate::UnderlyingAssetId::<Test>::kill();

		let call = crate::Call::<Test>::unload_recycler_into_external_asset_non_anonymous {
			input,
			alias_proofs: proofs,
			to: dest,
			fee_currency: FeeCurrency::Native,
		};
		let ext = build_signed_ext(signer, call);

		assert_invalid(ext, CustomInvalidity::AssetIdNotSet);
	});
}

/// Setup data for a single recycler unload test.
/// Returns (input, proofs, secrets) ready for calling the pallet.
fn setup_single_unload(
	value: CoinValue,
	dest: u64,
	signer: u64,
	seed_offset: u8,
) -> (
	UnloadRecyclerInput<<Test as Config>::MaxConsolidation>,
	BoundedVec<Proof, <Test as Config>::MaxConsolidation>,
	Vec<Secret>,
) {
	let (secrets, index, revision) = setup_recycler(value, 2, seed_offset);
	let members: Vec<_> = secrets.iter().map(CryptoOf::<Test>::member_from_secret).collect();

	type RInput = UnloadRecyclerInput<<Test as Config>::MaxConsolidation>;

	// Build input with placeholder alias first
	let input: RInput =
		UnloadRecyclerInput { value, index, revision, aliases: bounded_vec![[0u8; 32]] };
	let inputs = vec![input];

	// Compute proven_msg (unified format)
	let proven_msg = blake2_256(&(&inputs, &dest, &signer).encode());
	let (_, alias) = create_unload_proof(&secrets[0], &members, &proven_msg);

	// Rebuild input with actual alias
	let input: RInput =
		UnloadRecyclerInput { value, index, revision, aliases: bounded_vec![alias] };
	let inputs = vec![input.clone()];

	// Recalculate with actual alias
	let proven_msg = blake2_256(&(&inputs, &dest, &signer).encode());
	let (proof, alias) = create_unload_proof(&secrets[0], &members, &proven_msg);
	let input: RInput =
		UnloadRecyclerInput { value, index, revision, aliases: bounded_vec![alias] };

	(input, bounded_vec![proof], secrets)
}

/// Setup data for a multi-recycler unload test.
/// Returns (inputs, proofs, secrets1, secrets2).
fn setup_multi_unload(
	dest: u64,
	signer: u64,
) -> (
	Vec<UnloadRecyclerInput<<Test as Config>::MaxConsolidation>>,
	BoundedVec<Proof, <Test as Config>::MaxConsolidation>,
	Vec<Secret>,
	Vec<Secret>,
) {
	let value1: CoinValue = 0; // $1
	let value2: CoinValue = 1; // $2

	let (secrets1, index1, revision1) = setup_recycler(value1, 2, 0);
	let members1: Vec<_> = secrets1.iter().map(CryptoOf::<Test>::member_from_secret).collect();

	let (secrets2, index2, revision2) = setup_recycler(value2, 2, 10);
	let members2: Vec<_> = secrets2.iter().map(CryptoOf::<Test>::member_from_secret).collect();

	type RInput = UnloadRecyclerInput<<Test as Config>::MaxConsolidation>;

	// Build placeholder inputs
	let inputs: Vec<RInput> = vec![
		UnloadRecyclerInput {
			value: value1,
			index: index1,
			revision: revision1,
			aliases: bounded_vec![[0u8; 32]],
		},
		UnloadRecyclerInput {
			value: value2,
			index: index2,
			revision: revision2,
			aliases: bounded_vec![[0u8; 32]],
		},
	];

	// Compute proven_msg
	let proven_msg = blake2_256(&(&inputs, &dest, &signer).encode());
	let (_, alias1) = create_unload_proof(&secrets1[0], &members1, &proven_msg);
	let (_, alias2) = create_unload_proof(&secrets2[0], &members2, &proven_msg);

	// Rebuild with actual aliases
	let inputs: Vec<RInput> = vec![
		UnloadRecyclerInput {
			value: value1,
			index: index1,
			revision: revision1,
			aliases: bounded_vec![alias1],
		},
		UnloadRecyclerInput {
			value: value2,
			index: index2,
			revision: revision2,
			aliases: bounded_vec![alias2],
		},
	];

	// Recalculate proven_msg
	let proven_msg = blake2_256(&(&inputs, &dest, &signer).encode());
	let (proof1, alias1) = create_unload_proof(&secrets1[0], &members1, &proven_msg);
	let (proof2, alias2) = create_unload_proof(&secrets2[0], &members2, &proven_msg);

	let final_inputs = vec![
		UnloadRecyclerInput {
			value: value1,
			index: index1,
			revision: revision1,
			aliases: bounded_vec![alias1],
		},
		UnloadRecyclerInput {
			value: value2,
			index: index2,
			revision: revision2,
			aliases: bounded_vec![alias2],
		},
	];

	(final_inputs, bounded_vec![proof1, proof2], secrets1, secrets2)
}

// ============================================================================
// Single recycler tests - direct call
// ============================================================================

#[test]
fn with_native_fee_works_call() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let dest = CHARLIE;
		let signer = ALICE;
		let value: CoinValue = 0; // $1 coin

		let (input, proofs, _) = setup_single_unload(value, dest, signer, 0);

		let alice_native_before = Balances::free_balance(ALICE);
		let fee_dest_native_before = Balances::free_balance(FEE_DESTINATION);
		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				TEST_ASSET_ID,
				&dest,
			);

		// Call the non-anonymous unload
		assert_ok!(Coinage::unload_recycler_into_external_asset_non_anonymous(
			RuntimeOrigin::signed(signer),
			input.clone(),
			proofs,
			dest,
			FeeCurrency::Native,
		));

		// Check fee was charged in native (PaidUnloadTokenFee=2, conversion multiplies by 2, so
		// native fee = 4)
		let alice_native_after = Balances::free_balance(ALICE);
		let fee_dest_native_after = Balances::free_balance(FEE_DESTINATION);
		assert_eq!(alice_native_before - alice_native_after, 4);
		assert_eq!(fee_dest_native_after - fee_dest_native_before, 4);

		// Check external asset was transferred (value=0 means 1 * UNDERLYING_ASSET_UNIT = 1000)
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				TEST_ASSET_ID,
				&dest,
			);
		assert_eq!(charlie_external_asset_after - charlie_external_asset_before, 1000);

		// Check alias was marked as unloaded
		assert!(RecyclersUnloaded::<Test>::contains_key((value, input.index, input.aliases[0])));
		System::assert_has_event(
			crate::Event::<Test>::RecyclersUnloadedIntoExternalAssetNonAnonymous {
				who: signer,
				to: dest,
				input_count: 1,
				amount: 1000,
				fee_currency: FeeCurrency::Native,
			}
			.into(),
		);
	});
}

#[test]
fn with_native_fee_works_extrinsic() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let dest = CHARLIE;
		let signer = ALICE;
		let value: CoinValue = 0; // $1 coin

		let (input, proofs, _) = setup_single_unload(value, dest, signer, 0);

		let alice_native_before = Balances::free_balance(ALICE);
		let fee_dest_native_before = Balances::free_balance(FEE_DESTINATION);
		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				TEST_ASSET_ID,
				&dest,
			);

		// Build the call and extrinsic
		let call = crate::Call::<Test>::unload_recycler_into_external_asset_non_anonymous {
			input: input.clone(),
			alias_proofs: proofs,
			to: dest,
			fee_currency: FeeCurrency::Native,
		};
		let ext = build_signed_ext(signer, call);

		// Apply the extrinsic
		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "Extrinsic should succeed: {result:?}");

		// Check fee was charged in native (PaidUnloadTokenFee=2, conversion multiplies by 2, so
		// native fee = 4)
		let alice_native_after = Balances::free_balance(ALICE);
		let fee_dest_native_after = Balances::free_balance(FEE_DESTINATION);
		assert_eq!(alice_native_before - alice_native_after, 4);
		assert_eq!(fee_dest_native_after - fee_dest_native_before, 4);

		// Check external asset was transferred (value=0 means 1 * UNDERLYING_ASSET_UNIT = 1000)
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				TEST_ASSET_ID,
				&dest,
			);
		assert_eq!(charlie_external_asset_after - charlie_external_asset_before, 1000);

		// Check alias was marked as unloaded
		assert!(RecyclersUnloaded::<Test>::contains_key((value, input.index, input.aliases[0])));
	});
}

#[test]
fn with_external_asset_fee_works_call() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let dest = CHARLIE;
		let signer = ALICE;
		let value: CoinValue = 0; // $1 coin

		let (input, proofs, _) = setup_single_unload(value, dest, signer, 0);

		let asset_id = TEST_ASSET_ID;
		let alice_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id, &signer,
			);
		let fee_dest_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id,
				&FEE_DESTINATION,
			);

		// Call the non-anonymous unload with external asset fee
		assert_ok!(Coinage::unload_recycler_into_external_asset_non_anonymous(
			RuntimeOrigin::signed(signer),
			input,
			proofs,
			dest,
			FeeCurrency::ExternalAsset,
		));

		// Check fee was charged in external asset (PaidUnloadTokenFee=2)
		let alice_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id, &signer,
			);
		let fee_dest_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id,
				&FEE_DESTINATION,
			);
		assert_eq!(alice_external_asset_before - alice_external_asset_after, 2);
		assert_eq!(fee_dest_external_asset_after - fee_dest_external_asset_before, 2);
	});
}

#[test]
fn with_external_asset_fee_works_extrinsic() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let dest = CHARLIE;
		let signer = ALICE;
		let value: CoinValue = 0; // $1 coin

		let (input, proofs, _) = setup_single_unload(value, dest, signer, 0);

		let asset_id = TEST_ASSET_ID;
		let alice_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id, &signer,
			);
		let fee_dest_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id,
				&FEE_DESTINATION,
			);

		// Build the call and extrinsic
		let call = crate::Call::<Test>::unload_recycler_into_external_asset_non_anonymous {
			input,
			alias_proofs: proofs,
			to: dest,
			fee_currency: FeeCurrency::ExternalAsset,
		};
		let ext = build_signed_ext(signer, call);

		// Apply the extrinsic
		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "Extrinsic should succeed: {result:?}");

		// Check fee was charged in external asset (PaidUnloadTokenFee=2)
		let alice_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id, &signer,
			);
		let fee_dest_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id,
				&FEE_DESTINATION,
			);
		assert_eq!(alice_external_asset_before - alice_external_asset_after, 2);
		assert_eq!(fee_dest_external_asset_after - fee_dest_external_asset_before, 2);
	});
}

// ============================================================================
// Single recycler tests - failure cases
// ============================================================================

#[test]
fn fails_with_invalid_proof_call() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: CoinValue = 0;

		// Setup recycler
		let (secrets, index, revision) = setup_recycler(value, 2, 0);
		let members: Vec<_> = secrets.iter().map(CryptoOf::<Test>::member_from_secret).collect();

		let dest = CHARLIE;
		let signer = ALICE;

		type RInput = UnloadRecyclerInput<<Test as Config>::MaxConsolidation>;

		// Create proof with wrong message (different signer)
		let wrong_signer = BOB;
		let input: RInput =
			UnloadRecyclerInput { value, index, revision, aliases: bounded_vec![[0u8; 32]] };
		let inputs = vec![input];
		let wrong_proven_msg = blake2_256(&(&inputs, &dest, &wrong_signer).encode());
		let (_, alias) = create_unload_proof(&secrets[0], &members, &wrong_proven_msg);

		let input: RInput =
			UnloadRecyclerInput { value, index, revision, aliases: bounded_vec![alias] };
		let inputs = vec![input.clone()];
		let wrong_proven_msg = blake2_256(&(&inputs, &dest, &wrong_signer).encode());
		let (proof, alias) = create_unload_proof(&secrets[0], &members, &wrong_proven_msg);
		let input: RInput =
			UnloadRecyclerInput { value, index, revision, aliases: bounded_vec![alias] };

		// Should fail because proof was signed with different signer
		assert_noop!(
			Coinage::unload_recycler_into_external_asset_non_anonymous(
				RuntimeOrigin::signed(signer),
				input,
				bounded_vec![proof],
				dest,
				FeeCurrency::Native,
			),
			Error::<Test>::InvalidAliasProof
		);
	});
}

#[test]
fn fails_with_insufficient_native_fee_balance_call() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let dest = CHARLIE;
		let signer = ALICE;
		let value: CoinValue = 0;

		let (input, proofs, _) = setup_single_unload(value, dest, signer, 0);

		// Set ALICE's native balance to less than the fee AFTER setup
		Balances::make_free_balance_be(&ALICE, 1); // Less than fee of 4

		// Should fail due to insufficient balance for fee
		assert_noop!(
			Coinage::unload_recycler_into_external_asset_non_anonymous(
				RuntimeOrigin::signed(signer),
				input,
				proofs,
				dest,
				FeeCurrency::Native,
			),
			DispatchError::Token(sp_runtime::TokenError::FundsUnavailable)
		);
	});
}

// ============================================================================
// Multiple recyclers tests - direct call
// ============================================================================

#[test]
fn multiple_recyclers_works_call() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let dest = CHARLIE;
		let signer = ALICE;

		let (inputs, proofs, _, _) = setup_multi_unload(dest, signer);

		let asset_id = TEST_ASSET_ID;
		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id, &dest,
			);

		// Call the non-anonymous multi-recycler unload
		assert_ok!(Coinage::unload_recyclers_into_external_asset_non_anonymous(
			RuntimeOrigin::signed(signer),
			inputs,
			proofs,
			dest,
			FeeCurrency::Native,
		));

		// Check total external asset transferred: $1 + $2 = $3 = 3000 units
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id, &dest,
			);
		assert_eq!(charlie_external_asset_after - charlie_external_asset_before, 3000);
		System::assert_has_event(
			crate::Event::<Test>::RecyclersUnloadedIntoExternalAssetNonAnonymous {
				who: signer,
				to: dest,
				input_count: 2,
				amount: 3000,
				fee_currency: FeeCurrency::Native,
			}
			.into(),
		);
	});
}

#[test]
fn multiple_recyclers_works_extrinsic() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let dest = CHARLIE;
		let signer = ALICE;

		let (inputs, proofs, _, _) = setup_multi_unload(dest, signer);

		let asset_id = TEST_ASSET_ID;
		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id, &dest,
			);

		// Build the call and extrinsic
		let call = crate::Call::<Test>::unload_recyclers_into_external_asset_non_anonymous {
			inputs,
			alias_proofs: proofs,
			to: dest,
			fee_currency: FeeCurrency::Native,
		};
		let ext = build_signed_ext(signer, call);

		// Apply the extrinsic
		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "Extrinsic should succeed: {result:?}");

		// Check total external asset transferred: $1 + $2 = $3 = 3000 units
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id, &dest,
			);
		assert_eq!(charlie_external_asset_after - charlie_external_asset_before, 3000);
	});
}

// ============================================================================
// Multiple recyclers tests - failure cases
// ============================================================================

#[test]
fn multiple_recyclers_with_mixed_valid_invalid_proofs_fails_call() {
	// Test that when one proof is valid and another is invalid, the whole operation fails
	new_test_ext().execute_with(|| {
		setup_balances();

		let value1: CoinValue = 0; // $1
		let value2: CoinValue = 1; // $2

		// Setup first recycler
		let (secrets1, index1, revision1) = setup_recycler(value1, 2, 0);
		let members1: Vec<_> = secrets1.iter().map(CryptoOf::<Test>::member_from_secret).collect();

		// Setup second recycler
		let (secrets2, index2, revision2) = setup_recycler(value2, 2, 10);
		let members2: Vec<_> = secrets2.iter().map(CryptoOf::<Test>::member_from_secret).collect();

		let dest = CHARLIE;
		let signer = ALICE;
		let wrong_signer = BOB; // For creating invalid proof

		type RInput = UnloadRecyclerInput<<Test as Config>::MaxConsolidation>;

		// Build inputs
		let placeholder_inputs: Vec<RInput> = vec![
			UnloadRecyclerInput {
				value: value1,
				index: index1,
				revision: revision1,
				aliases: bounded_vec![[0u8; 32]],
			},
			UnloadRecyclerInput {
				value: value2,
				index: index2,
				revision: revision2,
				aliases: bounded_vec![[0u8; 32]],
			},
		];

		// Get aliases first
		let proven_msg = blake2_256(&(&placeholder_inputs, &dest, &signer).encode());
		let (_, alias1) = create_unload_proof(&secrets1[0], &members1, &proven_msg);

		// For second proof, use WRONG signer to make it invalid
		let wrong_proven_msg = blake2_256(&(&placeholder_inputs, &dest, &wrong_signer).encode());
		let (_, alias2) = create_unload_proof(&secrets2[0], &members2, &wrong_proven_msg);

		// Build final inputs with actual aliases
		let final_inputs = vec![
			UnloadRecyclerInput {
				value: value1,
				index: index1,
				revision: revision1,
				aliases: bounded_vec![alias1],
			},
			UnloadRecyclerInput {
				value: value2,
				index: index2,
				revision: revision2,
				aliases: bounded_vec![alias2],
			},
		];

		// Recalculate proven_msg with actual aliases
		let proven_msg = blake2_256(&(&final_inputs, &dest, &signer).encode());
		let (proof1, _) = create_unload_proof(&secrets1[0], &members1, &proven_msg);

		// Second proof is still created with wrong message (wrong signer)
		let wrong_proven_msg = blake2_256(&(&final_inputs, &dest, &wrong_signer).encode());
		let (proof2, _) = create_unload_proof(&secrets2[0], &members2, &wrong_proven_msg);

		// Should fail because proof2 was signed with wrong signer
		assert_noop!(
			Coinage::unload_recyclers_into_external_asset_non_anonymous(
				RuntimeOrigin::signed(signer),
				final_inputs,
				bounded_vec![proof1, proof2],
				dest,
				FeeCurrency::Native,
			),
			Error::<Test>::InvalidAliasProof
		);
	});
}

// ============================================================================
// Previous revision tests
// ============================================================================

/// Setup a recycler with rotated revision (has previous_root set).
/// Returns (secrets_v1, members_v1, value, index, old_revision) where the proofs
/// should be created against members_v1 with old_revision.
fn setup_rotated_recycler() -> (Vec<Secret>, Vec<mock::Member>, CoinValue, RingIndex, RevisionIndex)
{
	let value: CoinValue = 0;

	// Setup initial recycler
	let (secrets, index, revision) = setup_recycler(value, 2, 0);
	let members_v1: Vec<_> = secrets.iter().map(CryptoOf::<Test>::member_from_secret).collect();

	// Add more coins and trigger ring rebuild via pallet-members
	let new_secret = get_secret(100);
	let new_member = CryptoOf::<Test>::member_from_secret(&new_secret);
	assert_ok!(RecyclerManager::<Test>::load(value, new_member));
	Members::process_maintenance();

	// Verify revision has increased
	let identifier = Coinage::recycler_collection_identifier(value);
	let new_rev = <Test as Config>::MemberService::ring_revision(&identifier, index).unwrap();
	assert!(new_rev > revision, "revision should have increased after rebuild");

	(secrets, members_v1, value, index, revision)
}

#[test]
fn success_with_previous_revision_call() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let (secrets, members_v1, value, index, old_revision) = setup_rotated_recycler();
		let dest = CHARLIE;
		let signer = ALICE;

		type RInput = UnloadRecyclerInput<<Test as Config>::MaxConsolidation>;

		// Build input with placeholder alias, using OLD revision
		let input: RInput = UnloadRecyclerInput {
			value,
			index,
			revision: old_revision,
			aliases: bounded_vec![[0u8; 32]],
		};
		let inputs = vec![input];

		// Compute proven_msg and create proof using OLD ring members
		let proven_msg = blake2_256(&(&inputs, &dest, &signer).encode());
		let (_, alias) = create_unload_proof(&secrets[0], &members_v1, &proven_msg);

		let input: RInput = UnloadRecyclerInput {
			value,
			index,
			revision: old_revision,
			aliases: bounded_vec![alias],
		};
		let inputs = vec![input.clone()];
		let proven_msg = blake2_256(&(&inputs, &dest, &signer).encode());
		let (proof, alias) = create_unload_proof(&secrets[0], &members_v1, &proven_msg);
		let input: RInput = UnloadRecyclerInput {
			value,
			index,
			revision: old_revision,
			aliases: bounded_vec![alias],
		};

		let alice_native_before = Balances::free_balance(ALICE);
		let fee_dest_native_before = Balances::free_balance(FEE_DESTINATION);
		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				TEST_ASSET_ID,
				&dest,
			);

		// Call with OLD revision - should succeed
		assert_ok!(Coinage::unload_recycler_into_external_asset_non_anonymous(
			RuntimeOrigin::signed(signer),
			input.clone(),
			bounded_vec![proof],
			dest,
			FeeCurrency::Native,
		));

		// Check fee was charged (native fee = 4)
		let alice_native_after = Balances::free_balance(ALICE);
		let fee_dest_native_after = Balances::free_balance(FEE_DESTINATION);
		assert_eq!(alice_native_before - alice_native_after, 4);
		assert_eq!(fee_dest_native_after - fee_dest_native_before, 4);

		// Check external asset was transferred (value=0 means 1000 units)
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				TEST_ASSET_ID,
				&dest,
			);
		assert_eq!(charlie_external_asset_after - charlie_external_asset_before, 1000);

		// Check alias was marked as unloaded
		assert!(RecyclersUnloaded::<Test>::contains_key((value, index, input.aliases[0])));
	});
}

#[test]
fn success_with_previous_revision_extrinsic() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let (secrets, members_v1, value, index, old_revision) = setup_rotated_recycler();
		let dest = CHARLIE;
		let signer = ALICE;

		type RInput = UnloadRecyclerInput<<Test as Config>::MaxConsolidation>;

		// Build input with placeholder alias, using OLD revision
		let input: RInput = UnloadRecyclerInput {
			value,
			index,
			revision: old_revision,
			aliases: bounded_vec![[0u8; 32]],
		};
		let inputs = vec![input];

		// Compute proven_msg and create proof using OLD ring members
		let proven_msg = blake2_256(&(&inputs, &dest, &signer).encode());
		let (_, alias) = create_unload_proof(&secrets[0], &members_v1, &proven_msg);

		let input: RInput = UnloadRecyclerInput {
			value,
			index,
			revision: old_revision,
			aliases: bounded_vec![alias],
		};
		let inputs = vec![input.clone()];
		let proven_msg = blake2_256(&(&inputs, &dest, &signer).encode());
		let (proof, alias) = create_unload_proof(&secrets[0], &members_v1, &proven_msg);
		let input: RInput = UnloadRecyclerInput {
			value,
			index,
			revision: old_revision,
			aliases: bounded_vec![alias],
		};

		let alice_native_before = Balances::free_balance(ALICE);
		let fee_dest_native_before = Balances::free_balance(FEE_DESTINATION);
		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				TEST_ASSET_ID,
				&dest,
			);

		// Build the call and extrinsic
		let call = crate::Call::<Test>::unload_recycler_into_external_asset_non_anonymous {
			input: input.clone(),
			alias_proofs: bounded_vec![proof],
			to: dest,
			fee_currency: FeeCurrency::Native,
		};
		let ext = build_signed_ext(signer, call);

		// Apply the extrinsic
		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "Extrinsic should succeed: {result:?}");

		// Check fee was charged (native fee = 4)
		let alice_native_after = Balances::free_balance(ALICE);
		let fee_dest_native_after = Balances::free_balance(FEE_DESTINATION);
		assert_eq!(alice_native_before - alice_native_after, 4);
		assert_eq!(fee_dest_native_after - fee_dest_native_before, 4);

		// Check external asset was transferred (value=0 means 1000 units)
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				TEST_ASSET_ID,
				&dest,
			);
		assert_eq!(charlie_external_asset_after - charlie_external_asset_before, 1000);

		// Check alias was marked as unloaded
		assert!(RecyclersUnloaded::<Test>::contains_key((value, index, input.aliases[0])));
	});
}

// ============================================================================
// Extension validation tests
// ============================================================================

#[test]
fn invalid_revision_fails_in_extension() {
	// Test that invalid revision is caught in the extension (before fee payment).
	// This protects users from paying fees when their proofs are outdated due to
	// ring revision changes.
	use crate::{extension::AsCoinage, mock::assert_invalid, pallet::CustomInvalidity};
	use frame_system::AuthorizeCall;
	use sp_runtime::testing::UintAuthorityId;

	new_test_ext().execute_with(|| {
		setup_balances();

		let value: CoinValue = 0;
		let (secrets, index, revision) = setup_recycler(value, 2, 0);
		let members: Vec<_> = secrets.iter().map(CryptoOf::<Test>::member_from_secret).collect();

		let dest = CHARLIE;
		let signer = ALICE;

		// Get alias
		let alias = CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();

		// Build input with WRONG revision
		let wrong_revision = revision + 1;
		let input = UnloadRecyclerInput {
			value,
			index,
			revision: wrong_revision,
			aliases: bounded_vec![alias],
		};

		// Create proof (doesn't matter if it's valid, we're testing revision check)
		let proven_msg = blake2_256(&(&input, &dest, &signer).encode());
		let (proof, _) = create_unload_proof(&secrets[0], &members, &proven_msg);

		// Build the call
		let call = crate::Call::<Test>::unload_recycler_into_external_asset_non_anonymous {
			input,
			alias_proofs: bounded_vec![proof],
			to: dest,
			fee_currency: FeeCurrency::Native,
		};

		// Build signed extrinsic with None extension (normal signed call)
		let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(None));
		let ext = Extrinsic::new_signed(
			RuntimeCall::Coinage(call),
			signer,
			UintAuthorityId(signer),
			extension,
		);

		// The extension should reject this with InvalidRecyclerRevision
		// BEFORE fee payment happens
		assert_invalid(ext, CustomInvalidity::InvalidRecyclerRevision);
	});
}
