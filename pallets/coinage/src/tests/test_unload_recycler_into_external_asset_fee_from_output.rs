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
use frame_support::{assert_err_ignore_postinfo, assert_ok, traits::fungibles::InspectHold};
use sp_crypto_hashing::blake2_256;
use sp_runtime::bounded_vec;
use verifiable::GenerateVerifiable;

/// Setup data for a single recycler unload with fee from output.
/// Returns (aliases, proof, secrets, index, revision).
fn setup_single_unload_from_output(
	value: Denomination,
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

		let value: Denomination = 0; // $1 coin
		let dest = CHARLIE;

		let (aliases, proof, _, index, revision) = setup_single_unload_from_output(value, dest, 0);
		let alias = aliases[0];
		let proven_msg = blake2_256(&(value, index, revision, &aliases, &dest).encode());

		let market_asset_before = <AssetsWithHolder as frame_support::traits::fungibles::Inspect<
			_,
		>>::balance(10, &MOCK_MARKET);
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
		RecyclerManager::<Test>::mark_alias_unloaded(TEST_INSTANCE_ID, value, index, alias);

		// Call the unload
		let post = Coinage::unload_recycler_into_external_asset(
			RuntimeOrigin::from(pallet_origin),
			TEST_INSTANCE_ID,
			aliases.clone(),
			value,
			index,
			revision,
			dest,
			unload_token_fee_in_asset(),
		)
		.expect("unload should succeed");

		// The `#[pallet::weight]` charges the worst case `max(prepaid, from_output)`; this
		// `FromOutput` call refunds down to the `FromOutput` benchmarked weight plus the instance
		// read via `PostDispatchInfo`, which never exceeds the charged worst case.
		let n = aliases.len();
		assert_eq!(
			post.actual_weight,
			Some(
				Coinage::unload_recycler_into_external_asset_from_output_weight(n)
					.saturating_add(<Test as Config>::WeightInfo::read_instance())
			),
		);
		assert!(post.actual_weight.unwrap().all_lte(
			Coinage::unload_recycler_into_external_asset_max_weight(n)
				.saturating_add(<Test as Config>::WeightInfo::read_instance())
		));

		// Check fee was transferred to fee destination
		let fee = MockPaidUnloadTokenFeeOverride::get().unwrap();
		let market_asset_after = <AssetsWithHolder as frame_support::traits::fungibles::Inspect<
			_,
		>>::balance(10, &MOCK_MARKET);
		assert_eq!(market_asset_after - market_asset_before, fee);

		// Check external asset was transferred (value=0 means 1 * UNDERLYING_ASSET_UNIT, minus
		// the fee)
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		assert_eq!(
			charlie_external_asset_after - charlie_external_asset_before,
			UNDERLYING_ASSET_UNIT - fee
		);
		System::assert_has_event(
			crate::Event::<Test>::RecyclerUnloadedIntoExternalAsset {
				instance_id: TEST_INSTANCE_ID,
				to: dest,
				value,
				input_count: 1,
				amount: UNDERLYING_ASSET_UNIT - fee,
			}
			.into(),
		);
	});
}

/// The fee is taken out of the unloaded value, so a quote above that value can never be paid.
/// Validation rejects the transaction, rather than letting the dispatch fail with
/// `InsufficientUnloadForFee` and lock the caller's first alias.
#[test]
fn unloaded_value_below_the_fee_is_invalid_extrinsic() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let value: Denomination = 0;
		let dest = CHARLIE;
		let (secrets, index, revision) = setup_recycler(value, 2, 0);

		// The market now asks more of the asset than one coin of this denomination is worth, as a
		// price move against the caller does.
		MockPaidUnloadTokenFeeOverride::set(&Some(UNDERLYING_ASSET_UNIT + 1));
		assert!(unload_token_fee_in_asset() > UNDERLYING_ASSET_UNIT);

		let aliases = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref()
		)
		.unwrap()];
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value,
			index,
			revision,
			to: dest,
			// The caller's own bound is ample: what the output can back is the binding one.
			max_fee: unload_token_fee_in_asset() * 2,
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[..1]);

		assert_invalid(ext, CustomInvalidity::UnloadedValueBelowFee);
	});
}

/// The fee is taken out of everything the call unloads, not out of a single coin: two aliases back
/// twice the value, so a fee worth more than one coin is still covered. A check that looked at one
/// coin's worth would reject this transaction.
#[test]
fn several_aliases_back_a_fee_above_one_coin() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let value: Denomination = 0;
		let dest = CHARLIE;
		let (secrets, index, revision) = setup_recycler(value, 2, 0);

		// Above one coin, below the two being unloaded.
		let fee = UNDERLYING_ASSET_UNIT + UNDERLYING_ASSET_UNIT / 2;
		MockPaidUnloadTokenFeeOverride::set(&Some(fee));

		let aliases = bounded_vec![
			CryptoOf::<Test>::alias_in_context(&secrets[0], UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap(),
			CryptoOf::<Test>::alias_in_context(&secrets[1], UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap()
		];
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..2]);

		let dest_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		let market_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&MOCK_MARKET,
			);

		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// The two coins paid the fee together and `to` received what was left of them.
		let dest_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		let market_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&MOCK_MARKET,
			);
		assert_eq!(market_after - market_before, fee);
		assert_eq!(dest_after - dest_before, 2 * UNDERLYING_ASSET_UNIT - fee);
	});
}

/// The counterpart: a fee above everything the call unloads cannot be paid out of it, however many
/// aliases back it.
#[test]
fn fee_above_every_alias_unloaded_is_invalid() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let value: Denomination = 0;
		let dest = CHARLIE;
		let (secrets, index, revision) = setup_recycler(value, 2, 0);

		MockPaidUnloadTokenFeeOverride::set(&Some(2 * UNDERLYING_ASSET_UNIT + 1));

		let aliases = bounded_vec![
			CryptoOf::<Test>::alias_in_context(&secrets[0], UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap(),
			CryptoOf::<Test>::alias_in_context(&secrets[1], UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap()
		];
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value,
			index,
			revision,
			to: dest,
			// The caller's own bound is ample: what the output can back is the binding one.
			max_fee: unload_token_fee_in_asset() * 2,
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..2]);

		assert_invalid(ext, CustomInvalidity::UnloadedValueBelowFee);
	});
}

/// The caller's own bound is checked while validating too, so a `max_fee` below the quote is
/// rejected before the transaction is included and the fee alias stays usable.
#[test]
fn max_fee_below_the_quote_is_invalid_extrinsic() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let value: Denomination = 0;
		let dest = CHARLIE;
		let (secrets, index, revision) = setup_recycler(value, 2, 0);
		let alias =
			CryptoOf::<Test>::alias_in_context(&secrets[0], UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();

		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![alias],
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset() - 1,
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);

		assert_invalid(ext, CustomInvalidity::MaxFeeInsufficientForUnload);
		// The alias was never marked, so the caller can retry with a bound that covers the quote.
		assert_ne!(
			RecyclerAliasStates::<Test>::get((TEST_INSTANCE_ID, value, index, alias)),
			Some(AliasState::Unloaded),
		);
	});
}

/// A market that takes less of the asset than it quoted leaves the pallet account holding asset
/// that the unload had already set aside for the fee. That surplus must go back on hold, or the
/// pallet account keeps free balance no coin accounts for, and the unloader must be credited the
/// part the fee did not consume.
///
/// This should never happen in practice but we cover it.
#[test]
fn fee_conversion_taking_less_than_quoted_re_holds_the_difference_call() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let value: Denomination = 0; // $1 coin
		let dest = CHARLIE;

		let (aliases, proof, _, index, revision) = setup_single_unload_from_output(value, dest, 0);
		let alias = aliases[0];
		let proven_msg = blake2_256(&(value, index, revision, &aliases, &dest).encode());

		let pallet_account = Coinage::pallet_account();
		let quoted = unload_token_fee_in_asset();
		let discount = 1;
		let spent = quoted - discount;
		let free_before = AssetsWithHolder::balance(TEST_ASSET_ID, &pallet_account);
		let held_before = AssetsWithHolder::balance_on_hold(
			TEST_ASSET_ID,
			&HoldReason::Wrapped.into(),
			&pallet_account,
		);
		let dest_before = AssetsWithHolder::balance(TEST_ASSET_ID, &dest);

		let pallet_origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: pallet::UnloadFee::FromOutput {
				fee_recycler_value: value,
				fee_recycler_index: index,
			},
		};
		RecyclerManager::<Test>::mark_alias_unloaded(TEST_INSTANCE_ID, value, index, alias);

		set_fee_conversion_swap_discount(discount);
		assert_ok!(Coinage::unload_recycler_into_external_asset(
			RuntimeOrigin::from(pallet_origin),
			TEST_INSTANCE_ID,
			aliases,
			value,
			index,
			revision,
			dest,
			quoted,
		));
		set_fee_conversion_swap_discount(0);

		// Only what the swap took left the pallet, and the unloader got the rest of the output.
		assert_eq!(AssetsWithHolder::balance(TEST_ASSET_ID, &MOCK_MARKET), spent);
		assert_eq!(
			AssetsWithHolder::balance(TEST_ASSET_ID, &dest) - dest_before,
			UNDERLYING_ASSET_UNIT - spent
		);
		// The pallet account gained no free balance: the unspent asset went back on hold, and the
		// hold is down by exactly the whole unloaded coin.
		assert_eq!(AssetsWithHolder::balance(TEST_ASSET_ID, &pallet_account), free_before);
		assert_eq!(
			AssetsWithHolder::balance_on_hold(
				TEST_ASSET_ID,
				&HoldReason::Wrapped.into(),
				&pallet_account
			),
			held_before - UNDERLYING_ASSET_UNIT
		);
	});
}

#[test]
fn unload_recycler_into_external_asset_with_fee_from_output_works_extrinsic() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: Denomination = 0; // $1 coin
		let dest = CHARLIE;

		let (aliases, _, secrets, index, revision) =
			setup_single_unload_from_output(value, dest, 0);

		let market_asset_before = <AssetsWithHolder as frame_support::traits::fungibles::Inspect<
			_,
		>>::balance(10, &MOCK_MARKET);
		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);

		// Build call and extrinsic
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);

		// Apply the extrinsic
		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "Extrinsic should succeed: {result:?}");

		// Check fee was transferred to fee destination
		let fee = MockPaidUnloadTokenFeeOverride::get().unwrap();
		let market_asset_after = <AssetsWithHolder as frame_support::traits::fungibles::Inspect<
			_,
		>>::balance(10, &MOCK_MARKET);
		assert_eq!(market_asset_after - market_asset_before, fee);

		// Check external asset was transferred (value=0 means 1 * UNDERLYING_ASSET_UNIT, minus
		// the fee)
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		assert_eq!(
			charlie_external_asset_after - charlie_external_asset_before,
			UNDERLYING_ASSET_UNIT - fee
		);
	});
}

#[test]
fn fee_from_output_fails_when_first_input_recycler_mismatches_fee_recycler_call() {
	new_test_ext().execute_with(|| {
		setup_balances();

		// Setup TWO recyclers with different values
		let cheap_value: Denomination = 0; // $1 coin (fee recycler in extension)
		let expensive_value: Denomination = 2; // $4 coin (what attacker tries to claim)

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
		RecyclerManager::<Test>::mark_alias_unloaded(
			TEST_INSTANCE_ID,
			cheap_value,
			cheap_index,
			cheap_alias,
		);

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
			TEST_INSTANCE_ID,
			bounded_vec![cheap_alias], // Using cheap alias
			expensive_value,           // But claiming expensive value!
			expensive_index,
			expensive_revision,
			dest,
			unload_token_fee_in_asset(),
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
		let cheap_value: Denomination = 0; // $1 - fee recycler in extension
		let expensive_value: Denomination = 1; // $2 - what attacker tries to claim

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
		RecyclerManager::<Test>::mark_alias_unloaded(
			TEST_INSTANCE_ID,
			cheap_value,
			cheap_index,
			cheap_alias,
		);

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
			TEST_INSTANCE_ID,
			bounded_vec![cheap_alias], // Using cheap alias
			expensive_value,           // But claiming expensive value!
			exp_index,
			exp_revision,
			dest,
			unload_token_fee_in_asset(),
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
	// This test verifies that minimum denomination works correctly with fee deduction.
	// With current mock, smallest coin = 250, so we get transfer = 250 - fee.
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: Denomination = -2; // Smallest coin ($0.25 = 250 units)
		let dest = CHARLIE;

		let (aliases, proof, _, index, revision) = setup_single_unload_from_output(value, dest, 0);
		let alias = aliases[0];
		let proven_msg = blake2_256(&(value, index, revision, &aliases, &dest).encode());

		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		let market_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&MOCK_MARKET,
			);

		let pallet_origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: pallet::UnloadFee::FromOutput {
				fee_recycler_value: value,
				fee_recycler_index: index,
			},
		};

		RecyclerManager::<Test>::mark_alias_unloaded(TEST_INSTANCE_ID, value, index, alias);

		assert_ok!(Coinage::unload_recycler_into_external_asset(
			RuntimeOrigin::from(pallet_origin),
			TEST_INSTANCE_ID,
			aliases,
			value,
			index,
			revision,
			dest,
			unload_token_fee_in_asset(),
		));

		// Coin = 250, transfer = coin - fee
		let fee = MockPaidUnloadTokenFeeOverride::get().unwrap();
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		let market_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&MOCK_MARKET,
			);

		assert_eq!(charlie_external_asset_after - charlie_external_asset_before, 250 - fee);
		assert_eq!(market_after - market_before, fee);
	});
}

#[test]
fn fee_from_output_with_minimum_coin_works_extrinsic() {
	new_test_ext().execute_with(|| {
		setup_balances();
		let value: Denomination = MinimumExponentForOutputUnloadFee::get();
		let dest = CHARLIE;

		let (aliases, _, secrets, index, revision) =
			setup_single_unload_from_output(value, dest, 0);

		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		let market_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&MOCK_MARKET,
			);

		// Build call and extrinsic
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);

		// Apply the extrinsic
		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "Extrinsic should succeed: {result:?}");

		// Coin = 1 * UNDERLYING_ASSET_UNIT, transfer = coin - fee
		let fee = MockPaidUnloadTokenFeeOverride::get().unwrap();
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		let market_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&MOCK_MARKET,
			);

		assert_eq!(
			charlie_external_asset_after - charlie_external_asset_before,
			UNDERLYING_ASSET_UNIT - fee
		);
		assert_eq!(market_after - market_before, fee);
	});
}

#[test]
fn fee_equals_denomination_results_in_zero_transfer_call() {
	// Test that when fee equals denomination, the transfer succeeds with zero going to destination.
	new_test_ext().execute_with(|| {
		setup_balances();

		// Set fee equal to minimum denomination (250 units)
		MockPaidUnloadTokenFeeOverride::set(&Some(250));

		let value: Denomination = -2; // Smallest coin ($0.25 = 250 units)
		let dest = CHARLIE;

		let (aliases, proof, _, index, revision) = setup_single_unload_from_output(value, dest, 0);
		let alias = aliases[0];
		let proven_msg = blake2_256(&(value, index, revision, &aliases, &dest).encode());

		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		let market_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&MOCK_MARKET,
			);

		let pallet_origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: pallet::UnloadFee::FromOutput {
				fee_recycler_value: value,
				fee_recycler_index: index,
			},
		};

		RecyclerManager::<Test>::mark_alias_unloaded(TEST_INSTANCE_ID, value, index, alias);

		assert_ok!(Coinage::unload_recycler_into_external_asset(
			RuntimeOrigin::from(pallet_origin),
			TEST_INSTANCE_ID,
			aliases,
			value,
			index,
			revision,
			dest,
			unload_token_fee_in_asset(),
		));

		// Fee = 250, coin = 250, transfer = 0
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		let market_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&MOCK_MARKET,
			);

		assert_eq!(charlie_external_asset_after - charlie_external_asset_before, 0); // 250 - 250 = 0
		assert_eq!(market_after - market_before, 250);
	});
}

#[test]
fn fee_exceeds_denomination_fails_call() {
	// Test that when fee exceeds denomination, the operation fails with InsufficientUnloadForFee.
	new_test_ext().execute_with(|| {
		setup_balances();

		// Set fee higher than minimum denomination (250 units)
		MockPaidUnloadTokenFeeOverride::set(&Some(300));

		let value: Denomination = -2; // Smallest coin ($0.25 = 250 units)
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

		RecyclerManager::<Test>::mark_alias_unloaded(TEST_INSTANCE_ID, value, index, alias);

		// Should fail because fee (300) > denomination (250)
		let result = Coinage::unload_recycler_into_external_asset(
			RuntimeOrigin::from(pallet_origin),
			TEST_INSTANCE_ID,
			aliases,
			value,
			index,
			revision,
			dest,
			unload_token_fee_in_asset(),
		);

		assert_err_ignore_postinfo!(result, Error::<Test>::InsufficientUnloadForFee);
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

		let value: Denomination = 0;
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
			TEST_INSTANCE_ID,
			aliases.clone(),
			value,
			index,
			revision,
			dest,
			0,
		));

		// Second unload with same alias - should fail with RecyclerAlreadyUnloaded
		let pallet_origin2 = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: pallet::UnloadFee::Prepaid,
		};

		let result = Coinage::unload_recycler_into_external_asset(
			RuntimeOrigin::from(pallet_origin2),
			TEST_INSTANCE_ID,
			bounded_vec![alias],
			value,
			index,
			revision,
			dest,
			0,
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
) -> (Vec<Secret>, Vec<Member>, Denomination, RingIndex, RevisionIndex) {
	let value: Denomination = 0;

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

		let market_asset_before = <AssetsWithHolder as frame_support::traits::fungibles::Inspect<
			_,
		>>::balance(10, &MOCK_MARKET);
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
		RecyclerManager::<Test>::mark_alias_unloaded(TEST_INSTANCE_ID, value, index, alias);

		// Call with OLD revision - should succeed because previous_root is valid
		assert_ok!(Coinage::unload_recycler_into_external_asset(
			RuntimeOrigin::from(pallet_origin),
			TEST_INSTANCE_ID,
			aliases.clone(),
			value,
			index,
			old_revision,
			dest,
			unload_token_fee_in_asset(),
		));

		// Check fee was transferred to fee destination
		let fee = MockPaidUnloadTokenFeeOverride::get().unwrap();
		let market_asset_after = <AssetsWithHolder as frame_support::traits::fungibles::Inspect<
			_,
		>>::balance(10, &MOCK_MARKET);
		assert_eq!(market_asset_after - market_asset_before, fee);

		// Check external asset was transferred (value=0 means 1 * UNDERLYING_ASSET_UNIT, minus
		// the fee)
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		assert_eq!(
			charlie_external_asset_after - charlie_external_asset_before,
			UNDERLYING_ASSET_UNIT - fee
		);
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
			instance_id: TEST_INSTANCE_ID,
			aliases: aliases.clone(),
			value,
			index,
			revision: old_revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
		};
		let runtime_call: RuntimeCall = call.into();

		// Compute the inherited_implication for signing
		let inherited_implication = ((0u8, &runtime_call), (), ());

		// Single alias: no other proofs, so intent_msg = blake2_256([] ++ inherited_implication)
		let other_proofs = Vec::<Proof>::new();
		let retry_counter = 0u8;
		let intent_msg = (&other_proofs, retry_counter, &inherited_implication)
			.using_encoded(sp_io::hashing::blake2_256);

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
			instance_id: TEST_INSTANCE_ID,
			aliases: aliases.clone(),
			value,
			index,
			revision: old_revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
		};
		let runtime_call: RuntimeCall = call.into();
		let inherited_implication = ((0u8, &runtime_call), (), ());

		// Recompute intent_msg with the actual alias in the call
		let other_proofs = Vec::<Proof>::new();
		let intent_msg = (&other_proofs, retry_counter, &inherited_implication)
			.using_encoded(sp_io::hashing::blake2_256);

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

		let market_asset_before = <AssetsWithHolder as frame_support::traits::fungibles::Inspect<
			_,
		>>::balance(10, &MOCK_MARKET);
		let charlie_external_asset_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);

		// Build the extension with the proof using OLD revision
		let info = Some(AsCoinageInfo::AsUnloadTokenFromOutput {
			fee_recycler_value: value,
			fee_recycler_index: index,
			fee_recycler_revision: old_revision,
			retry_counter,
			alias_proofs: bounded_vec![proof],
		});
		let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(info));
		let ext = Extrinsic::new_transaction(runtime_call, extension);

		// Apply the extrinsic
		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "Extrinsic should succeed: {result:?}");

		// Check fee was transferred to fee destination
		let fee = MockPaidUnloadTokenFeeOverride::get().unwrap();
		let market_asset_after = <AssetsWithHolder as frame_support::traits::fungibles::Inspect<
			_,
		>>::balance(10, &MOCK_MARKET);
		assert_eq!(market_asset_after - market_asset_before, fee);

		// Check external asset was transferred (value=0 means 1 * UNDERLYING_ASSET_UNIT, minus
		// the fee)
		let charlie_external_asset_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		assert_eq!(
			charlie_external_asset_after - charlie_external_asset_before,
			UNDERLYING_ASSET_UNIT - fee
		);
	});
}

/// The counterpart for the fee-from-output path: the conversion becoming unavailable between
/// `validate` and the dispatch means something in the runtime traded on its price in between, which
/// the `max_fee` check in validation assumes cannot happen. The dispatch fails, and the fee alias
/// reserved in `prepare` is locked for a retry rather than destroyed.
#[test]
fn fee_conversion_failure_during_dispatch_locks_the_fee_alias() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: Denomination = 0;
		let dest = CHARLIE;
		let (secrets, index, revision) = setup_recycler(value, 1, 0);
		let alias = CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();

		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![alias],
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);

		// The quotes taken while validating succeed; the one the dispatch takes does not.
		set_fee_conversion_unavailable_at(Some(1));

		let result = Executive::apply_extrinsic(ext);
		assert_eq!(result, Ok(Err(Error::<Test>::CannotConvertAssetToNative.into())));

		// Nothing was unloaded, and the fee alias is locked for a retry instead.
		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(TEST_INSTANCE_ID), 0);
		assert!(
			super::get_recycler_alias_lock_until(value, index, alias).is_some(),
			"failed dispatch should lock the fee alias for retry"
		);

		// Reset
		set_fee_conversion_unavailable_at(None);
	});
}

/// The other way the price can move: the quote is still available, it has just grown past what the
/// caller allowed. Validation found it within `max_fee` against this same state, so the dispatch
/// fails and, as above, the fee alias is locked for a retry.
#[test]
fn quote_above_max_fee_during_dispatch_locks_the_fee_alias() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: Denomination = 0;
		let dest = CHARLIE;
		let (secrets, index, revision) = setup_recycler(value, 1, 0);
		let alias = CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();

		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![alias],
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);

		// Quote 0 (validate) prices the fee within `max_fee`, quote 1 (dispatch) above it.
		set_fee_conversion_quote_surcharge_at(Some((1, 1)));

		let result = Executive::apply_extrinsic(ext);
		assert_eq!(result, Ok(Err(Error::<Test>::FeeExceedsMaxFee.into())));

		// Nothing was unloaded, and the fee alias is locked for a retry instead.
		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(TEST_INSTANCE_ID), 0);
		assert!(
			super::get_recycler_alias_lock_until(value, index, alias).is_some(),
			"failed dispatch should lock the fee alias for retry"
		);

		// Reset
		set_fee_conversion_quote_surcharge_at(None);
	});
}

#[test]
fn failed_output_token_extrinsic_locks_fee_alias_instead_of_destroying_it() {
	// When an output-token extrinsic fails dispatch, the first alias reserved in prepare should
	// be restored into a temporary failed-dispatch lock instead of being treated as destroyed.
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: Denomination = 0; // $1 coin = 1000 underlying units
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

		// Pre-mark alias1 as unavailable so the call gets past extension validation, then fails
		// during dispatch when it tries to consume the second alias.
		RecyclerManager::<Test>::mark_alias_unloaded(TEST_INSTANCE_ID, value, index, alias1);

		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(TEST_INSTANCE_ID), 0);

		// Build extrinsic with 2 aliases: alias0 (fee, validated in extension) + alias1 (fails)
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![alias0, alias1],
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..2]);

		// Apply extrinsic: extension validate passes (alias0 not yet unloaded),
		// prepare reserves alias0, dispatch fails (alias1 already unloaded),
		// post_dispatch should lock alias0 for retry.
		let result = Executive::apply_extrinsic(ext);
		// Dispatch error: Ok(Err(..))
		assert!(matches!(result, Ok(Err(_))), "Dispatch should fail: {result:?}");

		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(TEST_INSTANCE_ID), 0);
		// The fee alias was only reserved for this attempt, so it must not remain in the unloaded
		// set after the dispatch error.
		assert!(!matches!(
			RecyclerAliasStates::<Test>::get((TEST_INSTANCE_ID, value, index, alias0)),
			Some(AliasState::Unloaded),
		));
		// The externally visible recovery behavior is a temporary retry lock.
		assert!(
			super::get_recycler_alias_lock_until(value, index, alias0).is_some(),
			"failed dispatch should lock the fee alias for retry"
		);
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

		let value: Denomination = 0; // $1 coin

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
			instance_id: TEST_INSTANCE_ID,
			aliases: aliases_abc,
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
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
			instance_id: TEST_INSTANCE_ID,
			aliases: aliases_cde,
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
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

		let value: Denomination = 0;
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
			instance_id: TEST_INSTANCE_ID,
			aliases: vec![alias_b].try_into().unwrap(),
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
		};
		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);

		assert_invalid(ext, CustomInvalidity::FirstCallAliasMismatch);
	});
}

#[test]
fn failed_output_token_extrinsic_needs_fresh_retry_counter_after_lock_expiry() {
	new_test_ext().execute_with(|| {
		setup_balances();

		let value: Denomination = 0;
		let dest = CHARLIE;
		let (secrets, index, revision) = setup_recycler(value, 2, 0);
		let alias = CryptoOf::<Test>::alias_in_context(
			&secrets[0],
			crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap();

		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![alias],
			value,
			index,
			revision,
			to: dest,
			max_fee: unload_token_fee_in_asset(),
		};
		let ext =
			build_unload_from_output_ext(call.clone(), value, index, revision, &secrets[0..1]);

		// The swap takes more of the asset than the quote validation accepted allowed for, which
		// only the dispatch can find out.
		set_fee_conversion_swap_surcharge(1);
		let result = Executive::apply_extrinsic(ext.clone());
		assert!(matches!(result, Ok(Err(_))), "dispatch should fail: {result:?}");

		let lock_until = super::get_recycler_alias_lock_until(value, index, alias)
			.expect("failed dispatch should lock the alias");
		advance_until_time(lock_until as u32);
		assert_eq!(super::get_recycler_alias_lock_until(value, index, alias), None);

		assert_invalid(ext, CustomInvalidity::AliasTemporarilyLocked);

		let refreshed_ext =
			build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);
		assert_ok!(Executive::validate_transaction(
			TransactionSource::External,
			refreshed_ext,
			Default::default()
		));
	});
}

#[test]
fn fee_from_output_multiple_aliases_skips_only_first_call() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let value: Denomination = 0; // $1 coin
		let dest = CHARLIE;

		// One input consolidating two aliases from the same recycler ring. Under FromOutput the
		// extension pre-marks only the first alias, so the dispatch must verify the second via the
		// `&current_proofs[1..]` slice path.
		let (secrets, index, revision) = setup_recycler(value, 2, 0);
		let members: Vec<_> = secrets.iter().map(CryptoOf::<Test>::member_from_secret).collect();
		let aliases: Vec<Alias> = secrets
			.iter()
			.map(|s| {
				CryptoOf::<Test>::alias_in_context(s, UNLOADING_RECYCLER_CONTEXT.as_ref()).unwrap()
			})
			.collect();
		let proven_msg = blake2_256(&(value, index, revision, &aliases, &dest).encode());
		let proofs: Vec<Proof> = secrets
			.iter()
			.map(|s| create_unload_proof(s, &members, &proven_msg).0)
			.collect();
		let bounded_aliases: BoundedVec<Alias, <Test as Config>::MaxConsolidation> =
			aliases.clone().try_into().unwrap();

		let market_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&MOCK_MARKET,
			);
		let charlie_before =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);

		let pallet_origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: proofs.try_into().unwrap(),
			proven_msg,
			fee: pallet::UnloadFee::FromOutput {
				fee_recycler_value: value,
				fee_recycler_index: index,
			},
		};

		// The extension pre-marks only the first alias.
		RecyclerManager::<Test>::mark_alias_unloaded(TEST_INSTANCE_ID, value, index, aliases[0]);

		let post = Coinage::unload_recycler_into_external_asset(
			RuntimeOrigin::from(pallet_origin),
			TEST_INSTANCE_ID,
			bounded_aliases.clone(),
			value,
			index,
			revision,
			dest,
			unload_token_fee_in_asset(),
		)
		.expect("multi-alias from-output unload should succeed");

		let n = bounded_aliases.len();
		assert_eq!(
			post.actual_weight,
			Some(
				Coinage::unload_recycler_into_external_asset_from_output_weight(n)
					.saturating_add(<Test as Config>::WeightInfo::read_instance())
			),
		);
		assert!(post.actual_weight.unwrap().all_lte(
			Coinage::unload_recycler_into_external_asset_max_weight(n)
				.saturating_add(<Test as Config>::WeightInfo::read_instance())
		));

		// Both aliases end up unloaded: the first from the pre-mark, the second from dispatch.
		for alias in &aliases {
			assert_eq!(
				RecyclerAliasStates::<Test>::get((TEST_INSTANCE_ID, value, index, *alias)),
				Some(AliasState::Unloaded),
			);
		}

		// Two $1 coins = 2 * UNDERLYING_ASSET_UNIT, transfer = coins - fee
		let fee = MockPaidUnloadTokenFeeOverride::get().unwrap();
		let market_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(
				10,
				&MOCK_MARKET,
			);
		assert_eq!(market_after - market_before, fee);
		let charlie_after =
			<AssetsWithHolder as frame_support::traits::fungibles::Inspect<_>>::balance(10, &dest);
		assert_eq!(charlie_after - charlie_before, 2 * UNDERLYING_ASSET_UNIT - fee);
	});
}

#[test]
fn sponsored_from_output_unload_settles_the_load_deposit() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let (instance_id, secrets, index, revision) = setup_sponsored_recycler(10, 100, 2, 0);
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 20);

		// Unloading one key releases its deposit to the pot's free balance; the unload fee
		// deducted from the output does not touch the deposit accounting.
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id,
			aliases: bounded_vec![recycler_alias(&secrets[0])],
			value: 0,
			index,
			revision,
			to: 9_301,
			max_fee: unload_token_fee_in_asset(),
		};
		let free_before = pot_free(instance_id, NATIVE_DEPOSIT_ID);
		let ext =
			build_unload_from_output_ext_for(instance_id, call, 0, index, revision, &secrets[0..1]);
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
			to: 9_302,
			max_fee: unload_token_fee_in_asset(),
		};
		let ext =
			build_unload_from_output_ext_for(instance_id, call, 0, index, revision, &secrets[1..2]);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 0);
		assert_eq!(pot_free(instance_id, NATIVE_DEPOSIT_ID), free_after_switch);
	});
}
