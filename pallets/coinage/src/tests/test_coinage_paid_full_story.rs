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

//! Full story test for coinage paid flow.

use crate::{
	extension::{AsCoinage, AsCoinageInfo},
	mock::*,
	*,
};
use codec::Encode;
use frame_support::{assert_ok, traits::UnixTime, BoundedVec};
use frame_system::AuthorizeCall;
use indiv_support::traits::Alias;
use sp_runtime::{bounded_vec, testing::UintAuthorityId};
use verifiable::GenerateVerifiable;

/// Helper to execute a signed transaction with the default extension pipeline.
fn exec_signed(who: u64, call: RuntimeCall) {
	let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(None));
	let uxt = Extrinsic::new_signed(call, who, UintAuthorityId(who), extension);
	Executive::apply_extrinsic(uxt)
		.expect("Extrinsic valid")
		.expect("Execution successful");
}

/// Helper to execute a transaction as a coin (using AsCoin extension).
fn exec_as_coin(who_account: u64, call: RuntimeCall) {
	let extension =
		(AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(AsCoinageInfo::AsCoin)));
	// Use the coin account as the signer. AsCoinage will verify ownership
	// via the signature against the coin ID.
	let uxt = Extrinsic::new_signed(call, who_account, UintAuthorityId(who_account), extension);
	Executive::apply_extrinsic(uxt)
		.expect("Extrinsic valid")
		.expect("Execution successful");
}

#[test]
fn coinage_paid_full_story() {
	// This test simulates a complete user journey in the coinage system with paid tokens.
	//
	// 1. Alice funds her account and loads a recycler (onboarding).
	// 2. Alice pays the fee to register a paid unload token member key.
	// 3. System builds the rings.
	// 4. Alice unloads her recycler using the token into a fresh coin.
	// 5. Alice splits the fresh coin.
	// 6. Alice recycles the split coins and consolidates them back into one using a new paid token.
	// 7. Alice offboards by unloading the consolidated coin back to public assets.
	new_test_ext().execute_with(|| {
		setup_asset();
		advance_block();

		let alice = 1u64;
		let asset_id = TEST_ASSET_ID;

		// Values
		let coin_value_initial: i8 = 1; // 2 units
		let coin_value_split: i8 = 0; // 1 unit
		let asset_unit = get_u64::<<Test as Config>::UnderlyingAssetUnit>();

		// Initial asset amount: 2 units * 1000 = 2000
		let asset_amount_initial = asset_unit << (coin_value_initial as u32);

		let fee_amount = Coinage::paid_unload_token_fee_in_asset().ok().unwrap();
		let fund_amount = asset_amount_initial + fee_amount * 3 + 1000;

		// Fund Alice
		assert_ok!(Assets::mint(RuntimeOrigin::signed(alice), asset_id, alice, fund_amount));

		// Action 1: Initiate Onboarding (Load Recycler)
		let alice_recycler_secret_0 = get_secret(42);
		let alice_recycler_member_0 =
			CryptoOf::<Test>::member_from_secret(&alice_recycler_secret_0);
		let proof_of_ownership =
			CryptoOf::<Test>::sign(&alice_recycler_secret_0, &alice.encode()).unwrap();

		let load_call = RuntimeCall::Coinage(crate::Call::load_recycler_with_external_asset {
			preservation: CodecPreservation::Expendable,
			value: coin_value_initial,
			member_key: alice_recycler_member_0,
			proof_of_ownership,
		});

		exec_signed(alice, load_call);

		let r_val = RecyclersCoinToRecycler::<Test>::get(alice_recycler_member_0).unwrap();
		assert_eq!(r_val, coin_value_initial);
		let r_idx_0 = 0u32; // first ring after loading

		// Action 2: Pay for Unload Token
		let alice_payment_secret_1 = get_secret(101);
		let alice_payment_member_1 = CryptoOf::<Test>::member_from_secret(&alice_payment_secret_1);
		let payment_proof_1 =
			CryptoOf::<Test>::sign(&alice_payment_secret_1, &alice.encode()).unwrap();

		let pay_fee_call = RuntimeCall::Coinage(
			crate::Call::pay_for_recycler_unload_fee_token_with_external_asset {
				member_key: alice_payment_member_1,
				proof_of_ownership: payment_proof_1,
			},
		);

		exec_signed(alice, pay_fee_call);

		let now_secs = MockTime::now().as_secs() as u32;
		let period_duration = get_u32::<<Test as Config>::PaidUnloadTokenTimePeriod>();
		let period = now_secs / period_duration;
		let payment_ring_index = 0;

		// Action 3: Trigger ring builds via pallet-members
		Members::process_maintenance();

		let identifier = Coinage::paid_token_collection_identifier(period);
		let payment_revision =
			<Test as Config>::MemberService::ring_revision(&identifier, payment_ring_index as u32)
				.unwrap();

		// Action 4: Unload into Coin (using Paid Token)
		let fresh_alice_coin_0 = 100u64;
		let aliases_vec: BoundedVec<Alias, <Test as Config>::MaxConsolidation> =
			bounded_vec![CryptoOf::<Test>::alias_in_context(
				&alice_recycler_secret_0,
				UNLOADING_RECYCLER_CONTEXT.as_ref(),
			)
			.unwrap()];

		let recycler_identifier_0 = Coinage::recycler_collection_identifier(coin_value_initial);
		let r_rev_0 =
			<Test as Config>::MemberService::ring_revision(&recycler_identifier_0, r_idx_0)
				.unwrap();

		let unload_call = crate::Call::unload_recycler_into_coin {
			aliases: aliases_vec,
			value: coin_value_initial,
			index: r_idx_0,
			revision: r_rev_0,
			to: fresh_alice_coin_0,
		};

		let uxt = build_unload_paid_ext(
			unload_call,
			&alice_payment_secret_1,
			payment_ring_index,
			payment_revision,
			period,
			&[alice_recycler_secret_0],
			coin_value_initial,
			r_idx_0,
		);

		Executive::apply_extrinsic(uxt)
			.expect("Extrinsic valid")
			.expect("Execution successful");

		let coin0 = CoinsByOwner::<Test>::get(fresh_alice_coin_0).unwrap();
		assert_eq!(coin0.value, coin_value_initial);

		// Action 5: Split ($2 -> $1 + $1)
		let fresh_alice_coin_1 = 101u64;
		let fresh_alice_coin_2 = 102u64;

		let split_call = RuntimeCall::Coinage(crate::Call::split {
			split_into: bounded_vec![(
				coin_value_split,
				bounded_vec![fresh_alice_coin_1, fresh_alice_coin_2],
			)],
		});
		exec_as_coin(fresh_alice_coin_0, split_call);

		// Action 6: Recycle & Consolidate
		let current_coin_1 = fresh_alice_coin_1;
		let current_coin_2 = fresh_alice_coin_2;

		// Load Coins
		let alice_recycler_secret_1 = get_secret(51);
		let alice_recycler_member_1 =
			CryptoOf::<Test>::member_from_secret(&alice_recycler_secret_1);
		let proof_1 =
			CryptoOf::<Test>::sign(&alice_recycler_secret_1, &current_coin_1.encode()).unwrap();

		let load_call_1 = RuntimeCall::Coinage(crate::Call::load_recycler_with_coin {
			member_key: alice_recycler_member_1,
			proof_of_ownership: proof_1,
		});
		exec_as_coin(current_coin_1, load_call_1);

		let alice_recycler_secret_2 = get_secret(52);
		let alice_recycler_member_2 =
			CryptoOf::<Test>::member_from_secret(&alice_recycler_secret_2);
		let proof_2 =
			CryptoOf::<Test>::sign(&alice_recycler_secret_2, &current_coin_2.encode()).unwrap();

		let load_call_2 = RuntimeCall::Coinage(crate::Call::load_recycler_with_coin {
			member_key: alice_recycler_member_2,
			proof_of_ownership: proof_2,
		});
		exec_as_coin(current_coin_2, load_call_2);

		// Pay Fee for Consolidation
		let alice_payment_secret_2 = get_secret(102);
		let alice_payment_member_2 = CryptoOf::<Test>::member_from_secret(&alice_payment_secret_2);
		let payment_proof_2 =
			CryptoOf::<Test>::sign(&alice_payment_secret_2, &alice.encode()).unwrap();

		// Capture period for Action 6
		let now_secs_6 = MockTime::now().as_secs() as u32;
		let period_6 = now_secs_6 / period_duration;

		let pay_fee_call_2 = RuntimeCall::Coinage(
			crate::Call::pay_for_recycler_unload_fee_token_with_external_asset {
				member_key: alice_payment_member_2,
				proof_of_ownership: payment_proof_2,
			},
		);
		exec_signed(alice, pay_fee_call_2);

		// Trigger ring builds via pallet-members
		Members::process_maintenance();

		let identifier_6 = Coinage::paid_token_collection_identifier(period_6);
		let payment_revision_6 = <Test as Config>::MemberService::ring_revision(
			&identifier_6,
			payment_ring_index as u32,
		)
		.unwrap();

		// Unload (Consolidate)
		let val_cons = RecyclersCoinToRecycler::<Test>::get(alice_recycler_member_1).unwrap();
		let identifier_cons = Coinage::recycler_collection_identifier(val_cons);
		let idx_cons = 0u32; // members are in the first ring
		let rev_cons =
			<Test as Config>::MemberService::ring_revision(&identifier_cons, idx_cons).unwrap();
		let fresh_consolidated = 300u64;
		let recycler_secrets = vec![alice_recycler_secret_1, alice_recycler_secret_2];
		let aliases_vec: BoundedVec<Alias, <Test as Config>::MaxConsolidation> = recycler_secrets
			.iter()
			.map(|s| {
				CryptoOf::<Test>::alias_in_context(s, UNLOADING_RECYCLER_CONTEXT.as_ref()).unwrap()
			})
			.collect::<Vec<_>>()
			.try_into()
			.expect("should fit in MaxConsolidation");

		let unload_call = crate::Call::unload_recycler_into_coin {
			aliases: aliases_vec,
			value: val_cons,
			index: idx_cons,
			revision: rev_cons,
			to: fresh_consolidated,
		};

		let uxt = build_unload_paid_ext(
			unload_call,
			&alice_payment_secret_2,
			payment_ring_index,
			payment_revision_6,
			period_6,
			&recycler_secrets,
			val_cons,
			idx_cons,
		);
		Executive::apply_extrinsic(uxt)
			.expect("Extrinsic valid")
			.expect("Execution successful");

		let consolidated_coin = CoinsByOwner::<Test>::get(fresh_consolidated).unwrap();
		assert_eq!(consolidated_coin.value, coin_value_initial);

		// Action 7: Offboard
		let current_offboard = fresh_consolidated;

		let alice_recycler_secret_off = get_secret(60);
		let alice_recycler_member_off =
			CryptoOf::<Test>::member_from_secret(&alice_recycler_secret_off);
		let proof_off =
			CryptoOf::<Test>::sign(&alice_recycler_secret_off, &current_offboard.encode()).unwrap();

		let load_call_off = RuntimeCall::Coinage(crate::Call::load_recycler_with_coin {
			member_key: alice_recycler_member_off,
			proof_of_ownership: proof_off,
		});
		exec_as_coin(current_offboard, load_call_off);

		let alice_payment_secret_3 = get_secret(103);
		let alice_payment_member_3 = CryptoOf::<Test>::member_from_secret(&alice_payment_secret_3);
		let payment_proof_3 =
			CryptoOf::<Test>::sign(&alice_payment_secret_3, &alice.encode()).unwrap();

		// Capture period for Action 7
		let now_secs_7 = MockTime::now().as_secs() as u32;
		let period_7 = now_secs_7 / period_duration;

		let pay_fee_call_3 = RuntimeCall::Coinage(
			crate::Call::pay_for_recycler_unload_fee_token_with_external_asset {
				member_key: alice_payment_member_3,
				proof_of_ownership: payment_proof_3,
			},
		);
		exec_signed(alice, pay_fee_call_3);

		// Trigger ring builds via pallet-members
		Members::process_maintenance();

		let identifier_7 = Coinage::paid_token_collection_identifier(period_7);
		let payment_revision_7 = <Test as Config>::MemberService::ring_revision(
			&identifier_7,
			payment_ring_index as u32,
		)
		.unwrap();

		let val_off = RecyclersCoinToRecycler::<Test>::get(alice_recycler_member_off).unwrap();
		let identifier_off = Coinage::recycler_collection_identifier(val_off);
		let idx_off = 0u32; // members are in the first ring
		let rev_off =
			<Test as Config>::MemberService::ring_revision(&identifier_off, idx_off).unwrap();
		let aliases_vec: BoundedVec<Alias, <Test as Config>::MaxConsolidation> =
			bounded_vec![CryptoOf::<Test>::alias_in_context(
				&alice_recycler_secret_off,
				UNLOADING_RECYCLER_CONTEXT.as_ref(),
			)
			.unwrap()];

		let unload_call = crate::Call::unload_recycler_into_external_asset {
			aliases: aliases_vec,
			value: val_off,
			index: idx_off,
			revision: rev_off,
			to: alice,
		};

		let uxt = build_unload_paid_ext(
			unload_call,
			&alice_payment_secret_3,
			payment_ring_index,
			payment_revision_7,
			period_7,
			&[alice_recycler_secret_off],
			val_off,
			idx_off,
		);

		let balance_before = Assets::balance(asset_id, alice);
		Executive::apply_extrinsic(uxt)
			.expect("Extrinsic valid")
			.expect("Execution successful");
		let balance_after = Assets::balance(asset_id, alice);

		assert_eq!(balance_after, balance_before + asset_amount_initial);
	});
}
