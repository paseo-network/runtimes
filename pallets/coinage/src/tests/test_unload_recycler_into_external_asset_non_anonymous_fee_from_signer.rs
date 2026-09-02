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
	pallet::{Error, RecyclerAliasStates},
	*,
};
use codec::Encode;
use frame_support::{
	assert_noop, assert_ok,
	dispatch::{DispatchErrorWithPostInfo, Pays, PostDispatchInfo},
	traits::Currency,
};
use sp_crypto_hashing::blake2_256;
use sp_runtime::{bounded_vec, TokenError};
use verifiable::GenerateVerifiable;

#[test]
fn wrong_instance_rejected_in_validation() {
	new_test_ext().execute_with(|| {
		// Set up a recycler ring, then pass a wrong instance id, which addresses a recycler
		// that does not exist. The validator must reject the tx, otherwise the signer pays
		// the inclusion fee for a dispatch that fails.
		let dest = CHARLIE;
		let signer = ALICE;
		let (input, proofs, _) = setup_single_unload(0, dest, signer, 0);

		let call = crate::Call::<Test>::unload_recycler_into_external_asset_non_anonymous {
			instance_id: TEST_INSTANCE_ID + 1,
			input,
			alias_proofs: proofs,
			to: dest,
			fee_currency: FeeCurrency::Native,
			max_fee: native_max_fee_bound(),
		};
		let ext = build_signed_ext(signer, call);

		assert_invalid(ext, CustomInvalidity::InvalidRecyclerRevision);
	});
}

/// Setup data for a single recycler unload test.
/// Returns (input, proofs, secrets) ready for calling the pallet.
fn setup_single_unload(
	value: Denomination,
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
	let proven_msg = blake2_256(&(TEST_INSTANCE_ID, &inputs, &dest, &signer).encode());
	let (_, alias) = create_unload_proof(&secrets[0], &members, &proven_msg);

	// Rebuild input with actual alias
	let input: RInput =
		UnloadRecyclerInput { value, index, revision, aliases: bounded_vec![alias] };
	let inputs = vec![input.clone()];

	// Recalculate with actual alias
	let proven_msg = blake2_256(&(TEST_INSTANCE_ID, &inputs, &dest, &signer).encode());
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
	BoundedVec<
		UnloadRecyclerInput<<Test as Config>::MaxConsolidation>,
		<Test as Config>::MaxConsolidation,
	>,
	BoundedVec<Proof, <Test as Config>::MaxConsolidation>,
	Vec<Secret>,
	Vec<Secret>,
) {
	let value1: Denomination = 0; // $1
	let value2: Denomination = 1; // $2

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
	let proven_msg = blake2_256(&(TEST_INSTANCE_ID, &inputs, &dest, &signer).encode());
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
	let proven_msg = blake2_256(&(TEST_INSTANCE_ID, &inputs, &dest, &signer).encode());
	let (proof1, alias1) = create_unload_proof(&secrets1[0], &members1, &proven_msg);
	let (proof2, alias2) = create_unload_proof(&secrets2[0], &members2, &proven_msg);

	let final_inputs = bounded_vec![
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

/// The error a non-anonymous unload returns when the signer cannot pay the fee: the fee is charged
/// before any proof is verified, so the call refunds down to its `fee_fail` weight.
fn fee_fail_error(error: TokenError) -> DispatchErrorWithPostInfo {
	let actual_weight =
		<() as crate::WeightInfo>::unload_recyclers_into_external_asset_non_anonymous_fee_fail();
	DispatchErrorWithPostInfo {
		post_info: PostDispatchInfo { actual_weight: Some(actual_weight), pays_fee: Pays::Yes },
		error: error.into(),
	}
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
		let value: Denomination = 0; // $1 coin

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
			TEST_INSTANCE_ID,
			input.clone(),
			proofs,
			dest,
			FeeCurrency::Native,
			native_max_fee_bound(),
		));

		// Check fee was charged in native
		let fee = MockPaidUnloadTokenFeeOverride::get().unwrap();
		let alice_native_after = Balances::free_balance(ALICE);
		let fee_dest_native_after = Balances::free_balance(FEE_DESTINATION);
		assert_eq!(alice_native_before - alice_native_after, fee);
		assert_eq!(fee_dest_native_after - fee_dest_native_before, fee);

		// Check external asset was transferred (value=0 means 1 * UNDERLYING_ASSET_UNIT)
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				TEST_ASSET_ID,
				&dest,
			);
		assert_eq!(
			charlie_external_asset_after - charlie_external_asset_before,
			UNDERLYING_ASSET_UNIT
		);

		// Check alias was marked as unloaded
		assert!(matches!(
			RecyclerAliasStates::<Test>::get((
				TEST_INSTANCE_ID,
				value,
				input.index,
				input.aliases[0]
			)),
			Some(AliasState::Unloaded),
		));
		System::assert_has_event(
			crate::Event::<Test>::RecyclersUnloadedIntoExternalAssetNonAnonymous {
				instance_id: TEST_INSTANCE_ID,
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
		let value: Denomination = 0; // $1 coin

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
			instance_id: TEST_INSTANCE_ID,
			input: input.clone(),
			alias_proofs: proofs,
			to: dest,
			fee_currency: FeeCurrency::Native,
			max_fee: native_max_fee_bound(),
		};
		let ext = build_signed_ext(signer, call);

		// Apply the extrinsic
		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "Extrinsic should succeed: {result:?}");

		// Check fee was charged in native
		let fee = MockPaidUnloadTokenFeeOverride::get().unwrap();
		let alice_native_after = Balances::free_balance(ALICE);
		let fee_dest_native_after = Balances::free_balance(FEE_DESTINATION);
		assert_eq!(alice_native_before - alice_native_after, fee);
		assert_eq!(fee_dest_native_after - fee_dest_native_before, fee);

		// Check external asset was transferred (value=0 means 1 * UNDERLYING_ASSET_UNIT)
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				TEST_ASSET_ID,
				&dest,
			);
		assert_eq!(
			charlie_external_asset_after - charlie_external_asset_before,
			UNDERLYING_ASSET_UNIT
		);

		// Check alias was marked as unloaded
		assert!(matches!(
			RecyclerAliasStates::<Test>::get((
				TEST_INSTANCE_ID,
				value,
				input.index,
				input.aliases[0]
			)),
			Some(AliasState::Unloaded),
		));
	});
}

#[test]
fn with_external_asset_fee_works_call() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let dest = CHARLIE;
		let signer = ALICE;
		let value: Denomination = 0; // $1 coin

		let (input, proofs, _) = setup_single_unload(value, dest, signer, 0);

		let asset_id = TEST_ASSET_ID;
		let alice_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id, &signer,
			);
		let market_asset_before = <AssetsWithHolder as frame_support::traits::fungibles::Inspect<
			_,
		>>::balance(asset_id, &MOCK_MARKET);
		let fee_dest_native_before = Balances::free_balance(FEE_DESTINATION);

		// Call the non-anonymous unload with external asset fee
		assert_ok!(Coinage::unload_recycler_into_external_asset_non_anonymous(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			input,
			proofs,
			dest,
			FeeCurrency::ExternalAsset,
			max_fee_bound(),
		));

		// Check fee was charged in external asset
		let fee = MockPaidUnloadTokenFeeOverride::get().unwrap();
		let alice_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id, &signer,
			);
		let market_asset_after = <AssetsWithHolder as frame_support::traits::fungibles::Inspect<
			_,
		>>::balance(asset_id, &MOCK_MARKET);
		assert_eq!(alice_external_asset_before - alice_external_asset_after, fee);
		assert_eq!(market_asset_after - market_asset_before, fee);
		// The fee itself reaches the destination in native, not in the asset.
		assert_eq!(
			Balances::free_balance(FEE_DESTINATION) - fee_dest_native_before,
			Coinage::get_paid_unload_token_fee_in_native()
		);
	});
}

/// The conversion takes the fee out of the signer's asset account and keeps that account alive, so
/// the fee cannot be paid out of the asset's minimum balance.
#[test]
fn external_asset_fee_cannot_take_the_signer_below_the_minimum_balance_call() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let dest = CHARLIE;
		let signer = BOB;
		let (input, proofs, _) = setup_single_unload(0, dest, signer, 0);

		// Leave the signer holding exactly the fee, one minimum balance short of what paying it
		// and staying alive needs.
		let asset_id = TEST_ASSET_ID;
		let min_balance = Assets::minimum_balance(asset_id);
		let fee = unload_token_fee_in_asset();
		let excess = Assets::balance(asset_id, signer) - fee;
		assert_ok!(Assets::burn(RuntimeOrigin::signed(ALICE), asset_id, signer, excess));

		assert_noop!(
			Coinage::unload_recycler_into_external_asset_non_anonymous(
				RuntimeOrigin::signed(signer),
				TEST_INSTANCE_ID,
				input.clone(),
				proofs.clone(),
				dest,
				FeeCurrency::ExternalAsset,
				max_fee_bound()
			),
			fee_fail_error(TokenError::NotExpendable)
		);

		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), asset_id, signer, min_balance));
		assert_ok!(Coinage::unload_recycler_into_external_asset_non_anonymous(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			input,
			proofs,
			dest,
			FeeCurrency::ExternalAsset,
			max_fee_bound()
		));
		assert_eq!(Assets::balance(asset_id, signer), min_balance);
	});
}

/// The fee is paid by converting the signer's asset, and `max_fee` bounds what the conversion may
/// take. A bound below what it costs makes the transaction invalid, so it never reaches a block.
///
/// (This is best effort validation)
#[test]
fn external_asset_fee_above_max_fee_is_invalid() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let dest = CHARLIE;
		let signer = ALICE;
		let (input, proofs, _) = setup_single_unload(0, dest, signer, 0);

		let call = crate::Call::<Test>::unload_recycler_into_external_asset_non_anonymous {
			instance_id: TEST_INSTANCE_ID,
			input,
			alias_proofs: proofs,
			to: dest,
			fee_currency: FeeCurrency::ExternalAsset,
			max_fee: unload_token_fee_in_asset() - 1,
		};

		assert_invalid(
			build_signed_ext(signer, call),
			CustomInvalidity::MaxFeeInsufficientForUnload,
		);
	});
}

/// The fee bound is checked before any proof is verified, and a call rejected on it refunds down
/// to what that early exit cost. A conversion that moved after validation approved the call is not
/// the caller's doing, so they should not pay for the verification the call never did.
#[test]
fn fee_above_max_is_rejected_early_and_refunds_the_unused_weight() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let dest = CHARLIE;
		let signer = ALICE;
		let (input, proofs, _) = setup_single_unload(0, dest, signer, 0);

		let call = crate::Call::<Test>::unload_recyclers_into_external_asset_non_anonymous {
			instance_id: TEST_INSTANCE_ID,
			inputs: bounded_vec![input.clone()],
			alias_proofs: proofs.clone(),
			to: dest,
			fee_currency: FeeCurrency::ExternalAsset,
			max_fee: unload_token_fee_in_asset() - 1,
		};
		let charged =
			frame_support::dispatch::GetDispatchInfo::get_dispatch_info(&call).call_weight;

		let err = Coinage::unload_recyclers_into_external_asset_non_anonymous(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			bounded_vec![input],
			proofs,
			dest,
			FeeCurrency::ExternalAsset,
			unload_token_fee_in_asset() - 1,
		)
		.expect_err("the fee bound rejects the call");

		assert_eq!(err.error, Error::<Test>::FeeExceedsMaxFee.into());
		let refunded = err.post_info.actual_weight.expect("the early exit refunds");
		assert_eq!(
			refunded,
			<() as crate::WeightInfo>::unload_recyclers_into_external_asset_non_anonymous_fee_fail(
			)
		);
		assert!(
			refunded.all_lt(charged),
			"the early exit must cost less than the charged worst case: {refunded:?} vs {charged:?}"
		);
	});
}

/// The single-recycler call delegates to the batch one, so it takes the same early exit and
/// refunds down to it, below its own charged worst case.
#[test]
fn single_recycler_fee_above_max_is_rejected_early_and_refunds_the_unused_weight() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let dest = CHARLIE;
		let signer = ALICE;
		let (input, proofs, _) = setup_single_unload(0, dest, signer, 0);
		let max_fee = unload_token_fee_in_asset() - 1;

		let call = crate::Call::<Test>::unload_recycler_into_external_asset_non_anonymous {
			instance_id: TEST_INSTANCE_ID,
			input: input.clone(),
			alias_proofs: proofs.clone(),
			to: dest,
			fee_currency: FeeCurrency::ExternalAsset,
			max_fee,
		};
		let charged =
			frame_support::dispatch::GetDispatchInfo::get_dispatch_info(&call).call_weight;

		let err = Coinage::unload_recycler_into_external_asset_non_anonymous(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			input,
			proofs,
			dest,
			FeeCurrency::ExternalAsset,
			max_fee,
		)
		.expect_err("the fee bound rejects the call");

		assert_eq!(err.error, Error::<Test>::FeeExceedsMaxFee.into());
		let refunded = err.post_info.actual_weight.expect("the early exit refunds");
		assert_eq!(
			refunded,
			<() as crate::WeightInfo>::unload_recyclers_into_external_asset_non_anonymous_fee_fail(
			)
		);
		assert!(
			refunded.all_lt(charged),
			"the early exit must cost less than the charged worst case: {refunded:?} vs {charged:?}"
		);
	});
}

/// One fee is charged per recycler, so a batch of two costs two of them. The conversion is quoted
/// for the total, and the destination is paid that total in native.
#[test]
fn external_asset_fee_is_charged_once_per_recycler_call() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let dest = CHARLIE;
		let signer = ALICE;
		let (inputs, proofs, _, _) = setup_multi_unload(dest, signer);
		assert_eq!(inputs.len(), 2, "the batch must hold two recyclers for the fee count to show");

		let one_fee = unload_token_fee_in_asset();
		let signer_asset_before = Assets::balance(TEST_ASSET_ID, signer);
		let market_asset_before = Assets::balance(TEST_ASSET_ID, MOCK_MARKET);
		let fee_dest_native_before = Balances::free_balance(FEE_DESTINATION);

		assert_ok!(Coinage::unload_recyclers_into_external_asset_non_anonymous(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			inputs,
			proofs,
			dest,
			FeeCurrency::ExternalAsset,
			max_fee_bound()
		));

		// Two recyclers, two fees: the signer's asset paid for both of them, and both reached the
		// destination in native.
		assert_eq!(signer_asset_before - Assets::balance(TEST_ASSET_ID, signer), one_fee * 2);
		assert_eq!(Assets::balance(TEST_ASSET_ID, MOCK_MARKET) - market_asset_before, one_fee * 2);
		assert_eq!(
			Balances::free_balance(FEE_DESTINATION) - fee_dest_native_before,
			Coinage::get_paid_unload_token_fee_in_native() * 2
		);
	});
}

/// `max_fee` bounds the whole conversion rather than a single fee, so a batch of two recyclers
/// needs a bound covering both. Validation rejects a bound that covers only one, and the dispatch
/// enforces the same bound for a call that never went through validation.
#[test]
fn external_asset_max_fee_must_cover_every_recycler_fee() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let dest = CHARLIE;
		let signer = ALICE;
		let (inputs, proofs, _, _) = setup_multi_unload(dest, signer);
		// Enough for one of the two fees.
		let max_fee = unload_token_fee_in_asset();

		let call = crate::Call::<Test>::unload_recyclers_into_external_asset_non_anonymous {
			instance_id: TEST_INSTANCE_ID,
			inputs: inputs.clone(),
			alias_proofs: proofs.clone(),
			to: dest,
			fee_currency: FeeCurrency::ExternalAsset,
			max_fee,
		};
		assert_invalid(
			build_signed_ext(signer, call),
			CustomInvalidity::MaxFeeInsufficientForUnload,
		);

		let err = Coinage::unload_recyclers_into_external_asset_non_anonymous(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			inputs,
			proofs,
			dest,
			FeeCurrency::ExternalAsset,
			max_fee,
		)
		.expect_err("a bound covering one of the two fees rejects the call");
		assert_eq!(err.error, Error::<Test>::FeeExceedsMaxFee.into());
	});
}

/// The same bound applies to a batch paying in native: `max_fee` is one bound on the whole call, in
/// whichever currency pays it, so it has to cover one native fee per recycler.
#[test]
fn native_max_fee_must_cover_every_recycler_fee() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let dest = CHARLIE;
		let signer = ALICE;
		let (inputs, proofs, _, _) = setup_multi_unload(dest, signer);
		let fee_native = Coinage::get_paid_unload_token_fee_in_native();

		// A bound covering one of the two fees.
		let call = crate::Call::<Test>::unload_recyclers_into_external_asset_non_anonymous {
			instance_id: TEST_INSTANCE_ID,
			inputs: inputs.clone(),
			alias_proofs: proofs.clone(),
			to: dest,
			fee_currency: FeeCurrency::Native,
			max_fee: fee_native,
		};
		assert_invalid(
			build_signed_ext(signer, call),
			CustomInvalidity::MaxFeeInsufficientForUnload,
		);

		let native_before = Balances::free_balance(signer);
		let err = Coinage::unload_recyclers_into_external_asset_non_anonymous(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			inputs.clone(),
			proofs.clone(),
			dest,
			FeeCurrency::Native,
			fee_native,
		)
		.expect_err("a bound covering one of the two fees rejects the call");
		assert_eq!(err.error, Error::<Test>::FeeExceedsMaxFee.into());
		// The call exits before charging anything.
		assert_eq!(Balances::free_balance(signer), native_before);

		// A bound covering both fees pays them.
		assert_ok!(Coinage::unload_recyclers_into_external_asset_non_anonymous(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			inputs,
			proofs,
			dest,
			FeeCurrency::Native,
			fee_native * 2
		));
		assert_eq!(native_before - Balances::free_balance(signer), fee_native * 2);
	});
}

/// A batch with no inputs can never succeed, and pricing a fee for zero unloads would quote a
/// zero conversion. The caller is told the inputs are empty rather than that the asset cannot pay
/// fees.
#[test]
fn empty_inputs_is_invalid_as_empty_inputs() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let call = crate::Call::<Test>::unload_recyclers_into_external_asset_non_anonymous {
			instance_id: TEST_INSTANCE_ID,
			inputs: bounded_vec![],
			alias_proofs: bounded_vec![],
			to: CHARLIE,
			fee_currency: FeeCurrency::ExternalAsset,
			max_fee: max_fee_bound(),
		};

		assert_invalid(build_signed_ext(ALICE, call), CustomInvalidity::EmptyInputs);
	});
}

/// The same reasoning as [`empty_inputs_is_invalid_as_empty_inputs`], for the dispatch: the call is
/// reachable without the extension (nested in a batch), so the emptiness check has to come before
/// the fee bound there too.
#[test]
fn empty_inputs_fails_as_empty_inputs_call() {
	new_test_ext().execute_with(|| {
		setup_balances();

		assert_noop!(
			Coinage::unload_recyclers_into_external_asset_non_anonymous(
				RuntimeOrigin::signed(ALICE),
				TEST_INSTANCE_ID,
				bounded_vec![],
				bounded_vec![],
				CHARLIE,
				FeeCurrency::ExternalAsset,
				max_fee_bound()
			),
			Error::<Test>::EmptyInputs
		);
	});
}

#[test]
fn with_external_asset_fee_works_extrinsic() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let dest = CHARLIE;
		let signer = ALICE;
		let value: Denomination = 0; // $1 coin

		let (input, proofs, _) = setup_single_unload(value, dest, signer, 0);

		let asset_id = TEST_ASSET_ID;
		let alice_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id, &signer,
			);
		let market_asset_before = <AssetsWithHolder as frame_support::traits::fungibles::Inspect<
			_,
		>>::balance(asset_id, &MOCK_MARKET);

		// Build the call and extrinsic
		let call = crate::Call::<Test>::unload_recycler_into_external_asset_non_anonymous {
			instance_id: TEST_INSTANCE_ID,
			input,
			alias_proofs: proofs,
			to: dest,
			fee_currency: FeeCurrency::ExternalAsset,
			max_fee: unload_token_fee_in_asset(),
		};
		let ext = build_signed_ext(signer, call);

		// Apply the extrinsic
		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "Extrinsic should succeed: {result:?}");

		// Check fee was charged in external asset
		let fee = MockPaidUnloadTokenFeeOverride::get().unwrap();
		let alice_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id, &signer,
			);
		let market_asset_after = <AssetsWithHolder as frame_support::traits::fungibles::Inspect<
			_,
		>>::balance(asset_id, &MOCK_MARKET);
		assert_eq!(alice_external_asset_before - alice_external_asset_after, fee);
		assert_eq!(market_asset_after - market_asset_before, fee);
	});
}

// ============================================================================
// Single recycler tests - failure cases
// ============================================================================

#[test]
fn fails_with_invalid_proof_call() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: Denomination = 0;

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
		let wrong_proven_msg =
			blake2_256(&(TEST_INSTANCE_ID, &inputs, &dest, &wrong_signer).encode());
		let (_, alias) = create_unload_proof(&secrets[0], &members, &wrong_proven_msg);

		let input: RInput =
			UnloadRecyclerInput { value, index, revision, aliases: bounded_vec![alias] };
		let inputs = vec![input.clone()];
		let wrong_proven_msg =
			blake2_256(&(TEST_INSTANCE_ID, &inputs, &dest, &wrong_signer).encode());
		let (proof, alias) = create_unload_proof(&secrets[0], &members, &wrong_proven_msg);
		let input: RInput =
			UnloadRecyclerInput { value, index, revision, aliases: bounded_vec![alias] };

		// Should fail because proof was signed with different signer
		assert_noop!(
			Coinage::unload_recycler_into_external_asset_non_anonymous(
				RuntimeOrigin::signed(signer),
				TEST_INSTANCE_ID,
				input,
				bounded_vec![proof],
				dest,
				FeeCurrency::Native,
				native_max_fee_bound(),
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
		let value: Denomination = 0;

		let (input, proofs, _) = setup_single_unload(value, dest, signer, 0);

		// Set ALICE's native balance to less than the fee AFTER setup
		Balances::make_free_balance_be(&ALICE, 1);

		// Should fail due to insufficient balance for fee
		assert_noop!(
			Coinage::unload_recycler_into_external_asset_non_anonymous(
				RuntimeOrigin::signed(signer),
				TEST_INSTANCE_ID,
				input,
				proofs,
				dest,
				FeeCurrency::Native,
				native_max_fee_bound(),
			),
			fee_fail_error(TokenError::FundsUnavailable)
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
			TEST_INSTANCE_ID,
			inputs,
			proofs,
			dest,
			FeeCurrency::Native,
			native_max_fee_bound(),
		));

		// Check total external asset transferred: $1 + $2 = $3 = 3 * UNDERLYING_ASSET_UNIT
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id, &dest,
			);
		assert_eq!(
			charlie_external_asset_after - charlie_external_asset_before,
			3 * UNDERLYING_ASSET_UNIT
		);
		System::assert_has_event(
			crate::Event::<Test>::RecyclersUnloadedIntoExternalAssetNonAnonymous {
				instance_id: TEST_INSTANCE_ID,
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
			instance_id: TEST_INSTANCE_ID,
			inputs,
			alias_proofs: proofs,
			to: dest,
			fee_currency: FeeCurrency::Native,
			max_fee: native_max_fee_bound(),
		};
		let ext = build_signed_ext(signer, call);

		// Apply the extrinsic
		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "Extrinsic should succeed: {result:?}");

		// Check total external asset transferred: $1 + $2 = $3 = 3 * UNDERLYING_ASSET_UNIT
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				asset_id, &dest,
			);
		assert_eq!(
			charlie_external_asset_after - charlie_external_asset_before,
			3 * UNDERLYING_ASSET_UNIT
		);
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

		let value1: Denomination = 0; // $1
		let value2: Denomination = 1; // $2

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
		let proven_msg =
			blake2_256(&(TEST_INSTANCE_ID, &placeholder_inputs, &dest, &signer).encode());
		let (_, alias1) = create_unload_proof(&secrets1[0], &members1, &proven_msg);

		// For second proof, use WRONG signer to make it invalid
		let wrong_proven_msg =
			blake2_256(&(TEST_INSTANCE_ID, &placeholder_inputs, &dest, &wrong_signer).encode());
		let (_, alias2) = create_unload_proof(&secrets2[0], &members2, &wrong_proven_msg);

		// Build final inputs with actual aliases
		let final_inputs: BoundedVec<RInput, <Test as Config>::MaxConsolidation> = bounded_vec![
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
		let proven_msg = blake2_256(&(TEST_INSTANCE_ID, &final_inputs, &dest, &signer).encode());
		let (proof1, _) = create_unload_proof(&secrets1[0], &members1, &proven_msg);

		// Second proof is still created with wrong message (wrong signer)
		let wrong_proven_msg =
			blake2_256(&(TEST_INSTANCE_ID, &final_inputs, &dest, &wrong_signer).encode());
		let (proof2, _) = create_unload_proof(&secrets2[0], &members2, &wrong_proven_msg);

		// Should fail because proof2 was signed with wrong signer
		assert_noop!(
			Coinage::unload_recyclers_into_external_asset_non_anonymous(
				RuntimeOrigin::signed(signer),
				TEST_INSTANCE_ID,
				final_inputs,
				bounded_vec![proof1, proof2],
				dest,
				FeeCurrency::Native,
				native_max_fee_bound(),
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
fn setup_rotated_recycler(
) -> (Vec<Secret>, Vec<mock::Member>, Denomination, RingIndex, RevisionIndex) {
	let value: Denomination = 0;

	// Setup initial recycler
	let (secrets, index, revision) = setup_recycler(value, 2, 0);
	let members_v1: Vec<_> = secrets.iter().map(CryptoOf::<Test>::member_from_secret).collect();

	// Add more coins and trigger ring rebuild via pallet-members
	let new_secret = get_secret(100);
	let new_member = CryptoOf::<Test>::member_from_secret(&new_secret);
	assert_ok!(RecyclerManager::<Test>::load(TEST_INSTANCE_ID, value, new_member));
	Members::process_maintenance();

	// Verify revision has increased
	let identifier = Coinage::recycler_collection_identifier(TEST_INSTANCE_ID, value);
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
		let proven_msg = blake2_256(&(TEST_INSTANCE_ID, &inputs, &dest, &signer).encode());
		let (_, alias) = create_unload_proof(&secrets[0], &members_v1, &proven_msg);

		let input: RInput = UnloadRecyclerInput {
			value,
			index,
			revision: old_revision,
			aliases: bounded_vec![alias],
		};
		let inputs = vec![input.clone()];
		let proven_msg = blake2_256(&(TEST_INSTANCE_ID, &inputs, &dest, &signer).encode());
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
			TEST_INSTANCE_ID,
			input.clone(),
			bounded_vec![proof],
			dest,
			FeeCurrency::Native,
			native_max_fee_bound(),
		));

		// Check fee was charged
		let fee = MockPaidUnloadTokenFeeOverride::get().unwrap();
		let alice_native_after = Balances::free_balance(ALICE);
		let fee_dest_native_after = Balances::free_balance(FEE_DESTINATION);
		assert_eq!(alice_native_before - alice_native_after, fee);
		assert_eq!(fee_dest_native_after - fee_dest_native_before, fee);

		// Check external asset was transferred (value=0 means 1 * UNDERLYING_ASSET_UNIT)
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				TEST_ASSET_ID,
				&dest,
			);
		assert_eq!(
			charlie_external_asset_after - charlie_external_asset_before,
			UNDERLYING_ASSET_UNIT
		);

		// Check alias was marked as unloaded
		assert!(matches!(
			RecyclerAliasStates::<Test>::get((TEST_INSTANCE_ID, value, index, input.aliases[0])),
			Some(AliasState::Unloaded),
		));
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
		let proven_msg = blake2_256(&(TEST_INSTANCE_ID, &inputs, &dest, &signer).encode());
		let (_, alias) = create_unload_proof(&secrets[0], &members_v1, &proven_msg);

		let input: RInput = UnloadRecyclerInput {
			value,
			index,
			revision: old_revision,
			aliases: bounded_vec![alias],
		};
		let inputs = vec![input.clone()];
		let proven_msg = blake2_256(&(TEST_INSTANCE_ID, &inputs, &dest, &signer).encode());
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
			instance_id: TEST_INSTANCE_ID,
			input: input.clone(),
			alias_proofs: bounded_vec![proof],
			to: dest,
			fee_currency: FeeCurrency::Native,
			max_fee: native_max_fee_bound(),
		};
		let ext = build_signed_ext(signer, call);

		// Apply the extrinsic
		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "Extrinsic should succeed: {result:?}");

		// Check fee was charged
		let fee = MockPaidUnloadTokenFeeOverride::get().unwrap();
		let alice_native_after = Balances::free_balance(ALICE);
		let fee_dest_native_after = Balances::free_balance(FEE_DESTINATION);
		assert_eq!(alice_native_before - alice_native_after, fee);
		assert_eq!(fee_dest_native_after - fee_dest_native_before, fee);

		// Check external asset was transferred (value=0 means 1 * UNDERLYING_ASSET_UNIT)
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				TEST_ASSET_ID,
				&dest,
			);
		assert_eq!(
			charlie_external_asset_after - charlie_external_asset_before,
			UNDERLYING_ASSET_UNIT
		);

		// Check alias was marked as unloaded
		assert!(matches!(
			RecyclerAliasStates::<Test>::get((TEST_INSTANCE_ID, value, index, input.aliases[0])),
			Some(AliasState::Unloaded),
		));
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

		let value: Denomination = 0;
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
		let proven_msg = blake2_256(&(TEST_INSTANCE_ID, &input, &dest, &signer).encode());
		let (proof, _) = create_unload_proof(&secrets[0], &members, &proven_msg);

		// Build the call
		let call = crate::Call::<Test>::unload_recycler_into_external_asset_non_anonymous {
			instance_id: TEST_INSTANCE_ID,
			input,
			alias_proofs: bounded_vec![proof],
			to: dest,
			fee_currency: FeeCurrency::Native,
			max_fee: native_max_fee_bound(),
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

#[test]
fn sponsored_non_anonymous_unload_settles_the_load_deposit() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let (instance_id, secrets, index, _revision) = setup_sponsored_recycler(10, 100, 2, 0);
		let signer = ALICE;
		fund_native(signer, 10_000);
		fund_native(FEE_DESTINATION, 100);
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 20);

		// Unloading one key releases its deposit to the pot's free balance.
		let (input, proofs) =
			build_non_anonymous_unload(instance_id, &secrets[0..1], 0, index, signer, 9_501);
		let call = crate::Call::<Test>::unload_recycler_into_external_asset_non_anonymous {
			instance_id,
			input,
			alias_proofs: proofs,
			to: 9_501,
			fee_currency: FeeCurrency::Native,
			max_fee: native_max_fee_bound(),
		};
		let free_before = pot_free(instance_id, NATIVE_DEPOSIT_ID);
		assert_eq!(Executive::apply_extrinsic(build_signed_ext(signer, call)), Ok(Ok(())));
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 10);
		assert_eq!(pot_free(instance_id, NATIVE_DEPOSIT_ID), free_before + 10);
		check_load_deposit_invariant(instance_id, 1);

		// Switching to sufficient releases the remaining deposit; the other key, loaded while
		// the instance was sponsored, still unloads and settles nothing.
		assert_ok!(Coinage::make_instance_sufficient(RuntimeOrigin::root(), instance_id));
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 0);
		let free_after_switch = pot_free(instance_id, NATIVE_DEPOSIT_ID);
		let (input, proofs) =
			build_non_anonymous_unload(instance_id, &secrets[1..2], 0, index, signer, 9_502);
		let call = crate::Call::<Test>::unload_recycler_into_external_asset_non_anonymous {
			instance_id,
			input,
			alias_proofs: proofs,
			to: 9_502,
			fee_currency: FeeCurrency::Native,
			max_fee: native_max_fee_bound(),
		};
		assert_eq!(Executive::apply_extrinsic(build_signed_ext(signer, call)), Ok(Ok(())));
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 0);
		assert_eq!(pot_free(instance_id, NATIVE_DEPOSIT_ID), free_after_switch);
	});
}

#[test]
fn sponsored_non_anonymous_multi_unload_settles_the_load_deposit() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let (instance_id, secrets, index, _revision) = setup_sponsored_recycler(10, 100, 2, 0);
		let signer = ALICE;
		fund_native(signer, 10_000);
		fund_native(FEE_DESTINATION, 100);
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 20);

		// Unloading both keys in one input releases one deposit per alias.
		let (input, proofs) =
			build_non_anonymous_unload(instance_id, &secrets[0..2], 0, index, signer, 9_503);
		let call = crate::Call::<Test>::unload_recyclers_into_external_asset_non_anonymous {
			instance_id,
			inputs: bounded_vec![input],
			alias_proofs: proofs,
			to: 9_503,
			fee_currency: FeeCurrency::Native,
			max_fee: native_max_fee_bound(),
		};
		let free_before = pot_free(instance_id, NATIVE_DEPOSIT_ID);
		assert_eq!(Executive::apply_extrinsic(build_signed_ext(signer, call)), Ok(Ok(())));
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 0);
		assert_eq!(pot_free(instance_id, NATIVE_DEPOSIT_ID), free_before + 20);
		check_load_deposit_invariant(instance_id, 0);
	});
}

// ============================================================================
// `inputs` length bound and the proof-count pre-check
// ============================================================================

/// The encoded call, with `inputs` replaced by `count` copies of one input.
///
/// Built from the encoding, not from the call type, because the point is what the decoder accepts:
/// `inputs` is a `BoundedVec`, so the wire format carries a length the call type cannot express.
fn encoded_multi_unload_with_inputs(count: u32) -> Vec<u8> {
	let (input, proofs, _) = setup_single_unload(0, CHARLIE, ALICE, 0);
	let inputs = alloc::vec![input; count as usize];

	let mut encoded = alloc::vec![12u8];
	encoded.extend(TEST_INSTANCE_ID.encode());
	encoded.extend(inputs.encode());
	encoded.extend(proofs.encode());
	encoded.extend(CHARLIE.encode());
	encoded.extend(FeeCurrency::Native.encode());
	encoded.extend(native_max_fee_bound().encode());
	encoded
}

/// `inputs` is bounded by `MaxConsolidation`, so a longer list never reaches dispatch: it is
/// rejected while the extrinsic is decoded, before the call is weighed. Without the bound a
/// `pallet_utility` batch can nest the call and bypass the [`AsCoinage`] extension, whose weight is
/// the only thing that prices `inputs`.
#[test]
fn more_inputs_than_max_consolidation_does_not_decode() {
	new_test_ext().execute_with(|| {
		let max = <<Test as Config>::MaxConsolidation as Get<u32>>::get();

		let over = encoded_multi_unload_with_inputs(max + 1);
		assert!(crate::Call::<Test>::decode(&mut &over[..]).is_err());

		// The bound itself is what rejects it: one fewer input decodes.
		let at_bound = encoded_multi_unload_with_inputs(max);
		assert!(crate::Call::<Test>::decode(&mut &at_bound[..]).is_ok());
	});
}

/// One proof per alias is required before the fee is charged, the amounts are summed and `inputs`
/// is hashed. The declared weight is a function of `alias_proofs.len()`, while the work is one
/// recycler unload per input and one proof verification per alias, so a short `alias_proofs` buys
/// work the weight does not cover.
///
/// The `max_fee` covers one unload and the call names two, so the fee bound is what rejects the
/// call if the proof count is checked later.
#[test]
fn proof_count_is_checked_before_the_fee_bound_call() {
	new_test_ext().execute_with(|| {
		setup_balances();
		let signer = ALICE;
		let (inputs, proofs, _, _) = setup_multi_unload(CHARLIE, signer);
		let one_proof: BoundedVec<Proof, <Test as Config>::MaxConsolidation> =
			bounded_vec![proofs[0].clone()];

		assert_noop!(
			Coinage::unload_recyclers_into_external_asset_non_anonymous(
				RuntimeOrigin::signed(signer),
				TEST_INSTANCE_ID,
				inputs,
				one_proof,
				CHARLIE,
				FeeCurrency::Native,
				Coinage::get_paid_unload_token_fee_in_native()
			),
			Error::<Test>::ProofAndAliasMismatch
		);
	});
}

/// An input with no aliases consumes no proof, so padding `inputs` with such entries keeps the
/// proof count matching while adding a recycler unload per entry. They are rejected on the same
/// pass as the proof count, which keeps `inputs.len()` at or below `alias_proofs.len()`.
///
/// The `max_fee` covers one unload and the padded call names two, so the fee bound is what rejects
/// the call if the aliases are checked later.
#[test]
fn alias_less_input_is_rejected_before_the_fee_bound_call() {
	new_test_ext().execute_with(|| {
		setup_balances();
		let signer = ALICE;
		let (input, proofs, _) = setup_single_unload(0, CHARLIE, signer, 0);
		let alias_less = UnloadRecyclerInput {
			value: input.value,
			index: input.index,
			revision: input.revision,
			aliases: bounded_vec![],
		};

		assert_noop!(
			Coinage::unload_recyclers_into_external_asset_non_anonymous(
				RuntimeOrigin::signed(signer),
				TEST_INSTANCE_ID,
				bounded_vec![alias_less, input],
				proofs,
				CHARLIE,
				FeeCurrency::Native,
				Coinage::get_paid_unload_token_fee_in_native()
			),
			Error::<Test>::EmptyInputs
		);
	});
}
