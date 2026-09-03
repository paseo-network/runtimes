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

//! Tests for unload_recycler_into_external_asset with fee paid from output.
//!
//! Test naming convention:
//! - `*_call` - tests the pallet call directly by manually creating pallet origin
//! - `*_extrinsic` - tests the full flow via `Executive::apply_extrinsic()`

use crate::{
	mock::{
		assert_invalid, get_secret, setup_balances, setup_recycler, Executive, Member, CHARLIE, *,
	},
	pallet::{self, CustomInvalidity, Error},
	*,
};
use codec::Encode;
use frame_support::assert_ok;
use sp_io::hashing::blake2_256;
use sp_runtime::bounded_vec;
use verifiable::GenerateVerifiable;

/// Setup data for a single recycler unload with fee from output.
/// Returns (aliases, proof, secrets, index, revision).
fn setup_single_unload_from_output(
	value: CoinValue,
	dest: u64,
	seed_offset: u8,
) -> (
	BoundedVec<Alias, <Test as Config>::MaxConsolidation>,
	Proof,
	Vec<Secret>,
	RingIndex,
	RevisionIndex,
) {
	let (secrets, index, revision) = setup_recycler(value, 2, seed_offset);
	let members: Vec<_> = secrets.iter().map(CryptoOf::<Test>::member_from_secret).collect();

	// Create proven message with placeholder alias
	let aliases = vec![[0u8; 32]];
	let proven_msg = blake2_256(&(value, index, revision, &aliases, &dest).encode());
	let (_, alias) = create_unload_proof(&secrets[0], &members, &proven_msg);

	// Recalculate with actual alias
	let aliases = vec![alias];
	let proven_msg = blake2_256(&(value, index, revision, &aliases, &dest).encode());
	let (proof, alias) = create_unload_proof(&secrets[0], &members, &proven_msg);
	let aliases = bounded_vec![alias];

	(aliases, proof, secrets, index, revision)
}

// ============================================================================
// Single recycler tests - direct call
// ============================================================================

#[test]
fn unload_recycler_into_external_asset_with_fee_from_output_works_call() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let value: CoinValue = 0; // $1 coin
		let dest = CHARLIE;

		let (aliases, proof, _, index, revision) = setup_single_unload_from_output(value, dest, 0);
		let alias = aliases[0];
		let proven_msg = blake2_256(&(value, index, revision, &aliases, &dest).encode());

		let fee_dest_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&FEE_DESTINATION,
			);
		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);

		// Create the UnloadToken origin with fee from output
		let pallet_origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: pallet::UnloadFee::FromOutput {
				fee_recycler_value: value,
				fee_recycler_index: index,
			},
		};

		// Mark first alias as unloaded (normally done in extension prepare)
		RecyclerManager::<Test>::mark_alias_unloaded(value, index, alias);

		// Call the unload
		assert_ok!(Coinage::unload_recycler_into_external_asset(
			RuntimeOrigin::from(pallet_origin),
			aliases.clone(),
			value,
			index,
			revision,
			dest,
		));

		// Check fee was transferred to fee destination (PaidUnloadTokenFee=2)
		let fee_dest_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&FEE_DESTINATION,
			);
		assert_eq!(fee_dest_external_asset_after - fee_dest_external_asset_before, 2);

		// Check external asset was transferred (value=0 means 1 * UNDERLYING_ASSET_UNIT = 1000,
		// minus fee of 2)
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		assert_eq!(charlie_external_asset_after - charlie_external_asset_before, 998);
		System::assert_has_event(
			crate::Event::<Test>::RecyclerUnloadedIntoExternalAsset {
				to: dest,
				value,
				input_count: 1,
				amount: 998,
			}
			.into(),
		);
	});
}

#[test]
fn unload_recycler_into_external_asset_with_fee_from_output_works_extrinsic() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: CoinValue = 0; // $1 coin
		let dest = CHARLIE;

		let (aliases, _, secrets, index, revision) =
			setup_single_unload_from_output(value, dest, 0);

		let fee_dest_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&FEE_DESTINATION,
			);
		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);

		// Build call and extrinsic
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			aliases,
			value,
			index,
			revision,
			to: dest,
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);

		// Apply the extrinsic
		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "Extrinsic should succeed: {result:?}");

		// Check fee was transferred to fee destination (PaidUnloadTokenFee=2)
		let fee_dest_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&FEE_DESTINATION,
			);
		assert_eq!(fee_dest_external_asset_after - fee_dest_external_asset_before, 2);

		// Check external asset was transferred (value=0 means 1 * UNDERLYING_ASSET_UNIT = 1000,
		// minus fee of 2)
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		assert_eq!(charlie_external_asset_after - charlie_external_asset_before, 998);
	});
}

#[test]
fn fee_from_output_fails_when_first_input_recycler_mismatches_fee_recycler_call() {
	new_test_ext().execute_with(|| {
		setup_balances();

		// Setup TWO recyclers with different values
		let cheap_value: CoinValue = 0; // $1 coin (fee recycler in extension)
		let expensive_value: CoinValue = 2; // $4 coin (what attacker tries to claim)

		// Setup cheap recycler - this is what extension validates against
		let (cheap_secrets, cheap_index, _cheap_revision) = setup_recycler(cheap_value, 2, 0);
		let cheap_members: Vec<_> =
			cheap_secrets.iter().map(CryptoOf::<Test>::member_from_secret).collect();

		// Setup expensive recycler - this is what attacker tries to claim from
		let (expensive_secrets, expensive_index, expensive_revision) =
			setup_recycler(expensive_value, 2, 10);
		let _expensive_members: Vec<_> =
			expensive_secrets.iter().map(CryptoOf::<Test>::member_from_secret).collect();

		let dest = CHARLIE;

		// Attacker creates proven_msg for the EXPENSIVE recycler call
		// (this is what the call will use)
		let fake_aliases = vec![[0u8; 32]]; // placeholder
		let proven_msg_for_expensive = blake2_256(
			&(expensive_value, expensive_index, expensive_revision, &fake_aliases, &dest).encode(),
		);

		// But validates proof against CHEAP recycler in extension
		let (_, cheap_alias) =
			create_unload_proof(&cheap_secrets[0], &cheap_members, &proven_msg_for_expensive);

		// Recalculate with actual alias
		let aliases = vec![cheap_alias];
		let proven_msg = blake2_256(
			&(expensive_value, expensive_index, expensive_revision, &aliases, &dest).encode(),
		);
		let (cheap_proof, cheap_alias) =
			create_unload_proof(&cheap_secrets[0], &cheap_members, &proven_msg);

		// Simulate extension: mark cheap alias as unloaded in CHEAP recycler
		RecyclerManager::<Test>::mark_alias_unloaded(cheap_value, cheap_index, cheap_alias);

		// Create origin with cheap alias (from extension validation)
		// Extension validated against CHEAP recycler, so that's what's in the origin
		let pallet_origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![cheap_proof],
			proven_msg,
			fee: pallet::UnloadFee::FromOutput {
				fee_recycler_value: cheap_value, // Extension validated cheap recycler
				fee_recycler_index: cheap_index,
			},
		};

		// ATTACK: Call unload with EXPENSIVE recycler parameters but cheap alias
		// If vulnerable, this would succeed and attacker gets $4 instead of $1
		let result = Coinage::unload_recycler_into_external_asset(
			RuntimeOrigin::from(pallet_origin),
			bounded_vec![cheap_alias], // Using cheap alias
			expensive_value,           // But claiming expensive value!
			expensive_index,
			expensive_revision,
			dest,
		);

		assert!(
			result.is_err(),
			"SECURITY VULNERABILITY: Call should fail when input recycler mismatches fee
			recycler"
		);
	});
}

#[test]
fn fee_from_output_fails_when_first_input_recycler_mismatches_call() {
	new_test_ext().execute_with(|| {
		setup_balances();

		// Setup TWO recyclers: cheap (fee recycler) and expensive (what call claims)
		let cheap_value: CoinValue = 0; // $1 - fee recycler in extension
		let expensive_value: CoinValue = 1; // $2 - what attacker tries to claim

		let (cheap_secrets, cheap_index, _cheap_revision) = setup_recycler(cheap_value, 2, 0);
		let cheap_members: Vec<_> =
			cheap_secrets.iter().map(CryptoOf::<Test>::member_from_secret).collect();

		let (_exp_secrets, exp_index, exp_revision) = setup_recycler(expensive_value, 2, 10);

		let dest = CHARLIE;

		// Attacker creates proven_msg for the EXPENSIVE recycler call
		// (this is what the call will use)
		let fake_aliases = vec![[0u8; 32]]; // placeholder
		let proven_msg_for_expensive =
			blake2_256(&(expensive_value, exp_index, exp_revision, &fake_aliases, &dest).encode());

		// But validates proof against CHEAP recycler in extension
		let (_, cheap_alias) =
			create_unload_proof(&cheap_secrets[0], &cheap_members, &proven_msg_for_expensive);

		// Recalculate with actual alias
		let aliases = vec![cheap_alias];
		let proven_msg =
			blake2_256(&(expensive_value, exp_index, exp_revision, &aliases, &dest).encode());
		let (cheap_proof, cheap_alias) =
			create_unload_proof(&cheap_secrets[0], &cheap_members, &proven_msg);

		// Simulate extension: mark cheap alias as unloaded in CHEAP recycler
		RecyclerManager::<Test>::mark_alias_unloaded(cheap_value, cheap_index, cheap_alias);

		// Create origin with cheap alias (from extension validation)
		// Extension validated against CHEAP recycler, so that's what's in the origin
		let pallet_origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![cheap_proof],
			proven_msg,
			fee: pallet::UnloadFee::FromOutput {
				fee_recycler_value: cheap_value, // Extension validated cheap recycler
				fee_recycler_index: cheap_index,
			},
		};

		// ATTACK: Call unload with EXPENSIVE recycler parameters but cheap alias
		// If vulnerable, this would succeed and attacker gets $2 instead of $1
		let result = Coinage::unload_recycler_into_external_asset(
			RuntimeOrigin::from(pallet_origin),
			bounded_vec![cheap_alias], // Using cheap alias
			expensive_value,           // But claiming expensive value!
			exp_index,
			exp_revision,
			dest,
		);

		// This SHOULD fail because the call's recycler (expensive) doesn't match
		// the fee recycler (cheap) that was validated in the extension
		assert!(
			result.is_err(),
			"SECURITY VULNERABILITY: Call should fail when input recycler mismatches fee recycler"
		);
	});
}

#[test]
fn fee_from_output_with_minimum_coin_works_call() {
	// This test verifies that minimum coin value works correctly with fee deduction.
	// With current mock, smallest coin = 250, fee = 2, so we get transfer = 248.
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: CoinValue = -2; // Smallest coin ($0.25 = 250 units)
		let dest = CHARLIE;

		let (aliases, proof, _, index, revision) = setup_single_unload_from_output(value, dest, 0);
		let alias = aliases[0];
		let proven_msg = blake2_256(&(value, index, revision, &aliases, &dest).encode());

		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		let fee_dest_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&FEE_DESTINATION,
			);

		let pallet_origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: pallet::UnloadFee::FromOutput {
				fee_recycler_value: value,
				fee_recycler_index: index,
			},
		};

		RecyclerManager::<Test>::mark_alias_unloaded(value, index, alias);

		assert_ok!(Coinage::unload_recycler_into_external_asset(
			RuntimeOrigin::from(pallet_origin),
			aliases,
			value,
			index,
			revision,
			dest,
		));

		// Fee = 2, coin = 250, transfer = 248
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		let fee_dest_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&FEE_DESTINATION,
			);

		assert_eq!(charlie_external_asset_after - charlie_external_asset_before, 248); // 250 - 2 = 248
		assert_eq!(fee_dest_after - fee_dest_before, 2);
	});
}

#[test]
fn fee_from_output_with_minimum_coin_works_extrinsic() {
	new_test_ext().execute_with(|| {
		setup_balances();
		let value: CoinValue = MinimumExponentForOutputUnloadFee::get();
		let dest = CHARLIE;

		let (aliases, _, secrets, index, revision) =
			setup_single_unload_from_output(value, dest, 0);

		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		let fee_dest_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&FEE_DESTINATION,
			);

		// Build call and extrinsic
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			aliases,
			value,
			index,
			revision,
			to: dest,
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);

		// Apply the extrinsic
		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "Extrinsic should succeed: {result:?}");

		// Fee = 2, coin = 1000, transfer = 998
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		let fee_dest_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&FEE_DESTINATION,
			);

		assert_eq!(charlie_external_asset_after - charlie_external_asset_before, 998); // 1000 - 2 = 998
		assert_eq!(fee_dest_after - fee_dest_before, 2);
	});
}

#[test]
fn fee_equals_coin_value_results_in_zero_transfer_call() {
	// Test that when fee equals coin value, the transfer succeeds with zero going to destination.
	new_test_ext().execute_with(|| {
		setup_balances();

		// Set fee equal to minimum coin value (250 units)
		MockPaidUnloadTokenFeeOverride::set(&Some(250));

		let value: CoinValue = -2; // Smallest coin ($0.25 = 250 units)
		let dest = CHARLIE;

		let (aliases, proof, _, index, revision) = setup_single_unload_from_output(value, dest, 0);
		let alias = aliases[0];
		let proven_msg = blake2_256(&(value, index, revision, &aliases, &dest).encode());

		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		let fee_dest_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&FEE_DESTINATION,
			);

		let pallet_origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: pallet::UnloadFee::FromOutput {
				fee_recycler_value: value,
				fee_recycler_index: index,
			},
		};

		RecyclerManager::<Test>::mark_alias_unloaded(value, index, alias);

		assert_ok!(Coinage::unload_recycler_into_external_asset(
			RuntimeOrigin::from(pallet_origin),
			aliases,
			value,
			index,
			revision,
			dest,
		));

		// Fee = 250, coin = 250, transfer = 0
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		let fee_dest_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&FEE_DESTINATION,
			);

		assert_eq!(charlie_external_asset_after - charlie_external_asset_before, 0); // 250 - 250 = 0
		assert_eq!(fee_dest_after - fee_dest_before, 250);
	});
}

#[test]
fn fee_exceeds_coin_value_fails_call() {
	// Test that when fee exceeds coin value, the operation fails with InsufficientUnloadForFee.
	new_test_ext().execute_with(|| {
		setup_balances();

		// Set fee higher than minimum coin value (250 units)
		MockPaidUnloadTokenFeeOverride::set(&Some(300));

		let value: CoinValue = -2; // Smallest coin ($0.25 = 250 units)
		let dest = CHARLIE;

		let (aliases, proof, _, index, revision) = setup_single_unload_from_output(value, dest, 0);
		let alias = aliases[0];
		let proven_msg = blake2_256(&(value, index, revision, &aliases, &dest).encode());

		let pallet_origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: pallet::UnloadFee::FromOutput {
				fee_recycler_value: value,
				fee_recycler_index: index,
			},
		};

		RecyclerManager::<Test>::mark_alias_unloaded(value, index, alias);

		// Should fail because fee (300) > coin value (250)
		let result = Coinage::unload_recycler_into_external_asset(
			RuntimeOrigin::from(pallet_origin),
			aliases,
			value,
			index,
			revision,
			dest,
		);

		assert_eq!(result, Err(Error::<Test>::InsufficientUnloadForFee.into()));
	});
}

// ============================================================================
// Double spend tests
// ============================================================================

#[test]
fn concurrent_unload_same_alias_fails_call() {
	// Test that attempting to unload the same alias twice fails.
	// Uses Prepaid fee mode to ensure RecyclerManager::unload is called for both attempts.
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: CoinValue = 0;
		let dest = CHARLIE;

		let (aliases, proof, _, index, revision) = setup_single_unload_from_output(value, dest, 0);
		let alias = aliases[0];
		let proven_msg = blake2_256(&(value, index, revision, &aliases, &dest).encode());

		// First unload with Prepaid - should succeed
		let pallet_origin1 = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof.clone()],
			proven_msg,
			fee: pallet::UnloadFee::Prepaid,
		};

		assert_ok!(Coinage::unload_recycler_into_external_asset(
			RuntimeOrigin::from(pallet_origin1),
			aliases.clone(),
			value,
			index,
			revision,
			dest,
		));

		// Second unload with same alias - should fail with RecyclerAlreadyUnloaded
		let pallet_origin2 = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: pallet::UnloadFee::Prepaid,
		};

		let result = Coinage::unload_recycler_into_external_asset(
			RuntimeOrigin::from(pallet_origin2),
			bounded_vec![alias],
			value,
			index,
			revision,
			dest,
		);

		assert!(
			result.is_err(),
			"Second unload with same alias should fail with RecyclerAlreadyUnloaded"
		);
	});
}

// ============================================================================
// Previous revision tests
// ============================================================================

/// Setup a recycler with rotated revision for fee_from_output tests.
/// Returns (secrets_v1, members_v1, value, index, old_revision).
fn setup_rotated_recycler_from_output(
) -> (Vec<Secret>, Vec<Member>, CoinValue, RingIndex, RevisionIndex) {
	let value: CoinValue = 0;

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

		let (secrets, members_v1, value, index, old_revision) =
			setup_rotated_recycler_from_output();
		let dest = CHARLIE;

		// Build placeholder alias for proven_msg calculation
		let aliases: BoundedVec<_, <Test as crate::Config>::MaxConsolidation> =
			bounded_vec![[0u8; 32]];
		let proven_msg = blake2_256(&(value, index, old_revision, &aliases, &dest).encode());

		// Create proof using the OLD ring members
		let (_, alias) = create_unload_proof(&secrets[0], &members_v1, &proven_msg);

		let aliases: BoundedVec<_, <Test as crate::Config>::MaxConsolidation> = bounded_vec![alias];
		let proven_msg = blake2_256(&(value, index, old_revision, &aliases, &dest).encode());
		let (proof, alias) = create_unload_proof(&secrets[0], &members_v1, &proven_msg);
		let aliases: BoundedVec<_, <Test as crate::Config>::MaxConsolidation> = bounded_vec![alias];

		let fee_dest_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&FEE_DESTINATION,
			);
		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);

		// Create the UnloadToken origin with fee from output, using OLD revision
		let pallet_origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: pallet::UnloadFee::FromOutput {
				fee_recycler_value: value,
				fee_recycler_index: index,
			},
		};

		// Mark first alias as unloaded (normally done in extension prepare)
		RecyclerManager::<Test>::mark_alias_unloaded(value, index, alias);

		// Call with OLD revision - should succeed because previous_root is valid
		assert_ok!(Coinage::unload_recycler_into_external_asset(
			RuntimeOrigin::from(pallet_origin),
			aliases.clone(),
			value,
			index,
			old_revision,
			dest,
		));

		// Check fee was transferred to fee destination (PaidUnloadTokenFee=2)
		let fee_dest_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&FEE_DESTINATION,
			);
		assert_eq!(fee_dest_external_asset_after - fee_dest_external_asset_before, 2);

		// Check external asset was transferred (value=0 means 1000 units, minus fee of 2)
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		assert_eq!(charlie_external_asset_after - charlie_external_asset_before, 998);
	});
}

#[test]
fn success_with_previous_revision_extrinsic() {
	use crate::extension::{AsCoinage, AsCoinageInfo};
	use frame_system::AuthorizeCall;

	new_test_ext().execute_with(|| {
		setup_balances();

		let (secrets, members_v1, value, index, old_revision) =
			setup_rotated_recycler_from_output();
		let dest = CHARLIE;

		// Build placeholder alias for proven_msg calculation
		let aliases: BoundedVec<_, <Test as crate::Config>::MaxConsolidation> =
			bounded_vec![[0u8; 32]];

		// Build call using OLD revision (with placeholder alias first)
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			aliases: aliases.clone(),
			value,
			index,
			revision: old_revision,
			to: dest,
		};
		let runtime_call: RuntimeCall = call.into();

		// Compute the inherited_implication for signing
		let inherited_implication = ((0u8, &runtime_call), (), ());

		// Single alias: no other proofs, so intent_msg = blake2_256([] ++ inherited_implication)
		let other_proofs = Vec::<Proof>::new();
		let intent_msg = sp_io::hashing::blake2_256(
			&[other_proofs.encode(), inherited_implication.encode()].concat(),
		);

		// Create proof using the OLD ring members
		let member = CryptoOf::<Test>::member_from_secret(&secrets[0]);
		let commitment =
			CryptoOf::<Test>::open(recycler_ring_size(), &member, members_v1.clone().into_iter())
				.expect("should open");
		let (_, alias) = CryptoOf::<Test>::create(
			commitment,
			&secrets[0],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
			&intent_msg,
		)
		.expect("should create proof");

		// Now rebuild with the real alias
		let aliases: BoundedVec<_, <Test as crate::Config>::MaxConsolidation> = bounded_vec![alias];
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			aliases: aliases.clone(),
			value,
			index,
			revision: old_revision,
			to: dest,
		};
		let runtime_call: RuntimeCall = call.into();
		let inherited_implication = ((0u8, &runtime_call), (), ());

		// Recompute intent_msg with the actual alias in the call
		let other_proofs = Vec::<Proof>::new();
		let intent_msg = sp_io::hashing::blake2_256(
			&[other_proofs.encode(), inherited_implication.encode()].concat(),
		);

		// Create final proof using the OLD ring members
		let commitment =
			CryptoOf::<Test>::open(recycler_ring_size(), &member, members_v1.into_iter())
				.expect("should open");
		let (proof, _) = CryptoOf::<Test>::create(
			commitment,
			&secrets[0],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
			&intent_msg,
		)
		.expect("should create proof");

		let fee_dest_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&FEE_DESTINATION,
			);
		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);

		// Build the extension with the proof using OLD revision
		let info = Some(AsCoinageInfo::AsUnloadTokenFromOutput {
			fee_recycler_value: value,
			fee_recycler_index: index,
			fee_recycler_revision: old_revision,
			alias_proofs: bounded_vec![proof],
		});
		let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(info));
		let ext = Extrinsic::new_transaction(runtime_call, extension);

		// Apply the extrinsic
		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "Extrinsic should succeed: {result:?}");

		// Check fee was transferred to fee destination (PaidUnloadTokenFee=2)
		let fee_dest_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&FEE_DESTINATION,
			);
		assert_eq!(fee_dest_external_asset_after - fee_dest_external_asset_before, 2);

		// Check external asset was transferred (value=0 means 1000 units, minus fee of 2)
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		assert_eq!(charlie_external_asset_after - charlie_external_asset_before, 998);
	});
}

#[test]
fn failed_output_token_extrinsic_tracks_destroyed_value() {
	// When an output-token extrinsic fails dispatch, the first alias (consumed in prepare as
	// spam penalty) should have its value tracked in TotalValueOfDestroyedCoins.
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: CoinValue = 0; // $1 coin = 1000 underlying units
		let dest = CHARLIE;

		// Setup recycler with 3 members (need >2 aliases to cause dispatch failure)
		let (secrets, index, revision) = setup_recycler(value, 3, 0);

		// Compute aliases deterministically
		let alias0 = CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let alias1 = CryptoOf::<Test>::alias_in_context(
			&secrets[1],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();

		// Pre-mark alias1 as unloaded so dispatch will fail with RecyclerAlreadyUnloaded
		RecyclerManager::<Test>::mark_alias_unloaded(value, index, alias1);

		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(), 0);

		// Build extrinsic with 2 aliases: alias0 (fee, validated in extension) + alias1 (fails)
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			aliases: bounded_vec![alias0, alias1],
			value,
			index,
			revision,
			to: dest,
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..2]);

		// Apply extrinsic: extension validate passes (alias0 not yet unloaded),
		// prepare marks alias0 as unloaded, dispatch fails (alias1 already unloaded),
		// post_dispatch should track destroyed value.
		let result = Executive::apply_extrinsic(ext);
		// Dispatch error: Ok(Err(..))
		assert!(matches!(result, Ok(Err(_))), "Dispatch should fail: {result:?}");

		// fee_recycler_value=0 → 1000 underlying units should be tracked as destroyed
		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(), 1000);
	});
}

// ============================================================================
// Extension double-spend tests
// ============================================================================

#[test]
fn fee_from_output_first_alias_double_spend_fails_extrinsic() {
	// Test that when paying with output, the first alias validated in the extension
	// is checked against double spend.
	// Scenario:
	// - First unload: aliases A, B, C (success, all marked as unloaded)
	// - Second unload: aliases C, D, E where C is first (should fail in extension with
	//   RecyclerAlreadyUnloaded because C was already unloaded)
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: CoinValue = 0; // $1 coin

		// Setup recycler with 5 members for aliases A, B, C, D, E
		let (secrets, index, revision) = setup_recycler(value, 5, 0);

		let dest = CHARLIE;

		// Get aliases (deterministic based on secret and context)
		let alias_a = CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let alias_b = CryptoOf::<Test>::alias_in_context(
			&secrets[1],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let alias_c = CryptoOf::<Test>::alias_in_context(
			&secrets[2],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let alias_d = CryptoOf::<Test>::alias_in_context(
			&secrets[3],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();
		let alias_e = CryptoOf::<Test>::alias_in_context(
			&secrets[4],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();

		// First unload: aliases A, B, C
		let aliases_abc = bounded_vec![alias_a, alias_b, alias_c];

		// Build call for first unload
		let call1 = crate::Call::<Test>::unload_recycler_into_external_asset {
			aliases: aliases_abc,
			value,
			index,
			revision,
			to: dest,
		};

		// Build full extrinsic with AsUnloadTokenFromOutput extension
		let ext1 = build_unload_from_output_ext(call1, value, index, revision, &secrets[0..3]);

		// Apply the first extrinsic - should succeed
		let result1 = Executive::apply_extrinsic(ext1);
		assert!(result1.is_ok(), "First unload should succeed: {result1:?}");

		// Second unload: aliases C, D, E where C is first (already unloaded)
		let aliases_cde = bounded_vec![alias_c, alias_d, alias_e];

		// Build call for second unload
		let call2 = crate::Call::<Test>::unload_recycler_into_external_asset {
			aliases: aliases_cde,
			value,
			index,
			revision,
			to: dest,
		};

		// Build extrinsic with C as first alias (already unloaded)
		let ext2 = build_unload_from_output_ext(call2, value, index, revision, &secrets[2..5]);

		// The extension should reject this because alias C (from secrets[2]) was already
		// unloaded in the first transaction. The extension's validate_alias_proof checks
		// RecyclersUnloaded storage and returns RecyclerAlreadyUnloaded.
		assert_invalid(ext2, CustomInvalidity::RecyclerAlreadyUnloaded);
	});
}

// Regression test: the first alias in the call must match the alias derived from the
// first proof validated in the extension.
#[test]
fn fee_from_output_mismatched_first_alias_in_call_fails_extrinsic() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: CoinValue = 0;
		let dest = CHARLIE;

		let (secrets, index, revision) = setup_recycler(value, 2, 0);

		// alias_b is derived from secrets[1], NOT from secrets[0]
		let alias_b = CryptoOf::<Test>::alias_in_context(
			&secrets[1],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();

		// Build a call with alias_b as the first alias, but use secrets[0] for the proof.
		// The proof will derive alias_a (from secrets[0]), which differs from alias_b.
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			aliases: vec![alias_b].try_into().unwrap(),
			value,
			index,
			revision,
			to: dest,
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);

		assert_invalid(ext, CustomInvalidity::FirstCallAliasMismatch);
	});
}
