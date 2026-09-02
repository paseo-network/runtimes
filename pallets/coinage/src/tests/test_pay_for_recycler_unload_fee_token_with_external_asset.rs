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

use crate::{extension::AsCoinage, mock::*, *};
use codec::Encode;
use frame_support::{assert_err, assert_noop, assert_ok, BoundedVec};
use frame_system::AuthorizeCall;
use indiv_support::traits::AppendOnlyMembers;
use sp_runtime::{
	bounded_vec, testing::UintAuthorityId, transaction_validity::TransactionSource, DispatchError,
	TokenError,
};
use verifiable::GenerateVerifiable;

fn build_ext(
	signer: u64,
	member_key: MemberOf<Test>,
	proof_of_ownership: SignatureOf<Test>,
) -> Extrinsic {
	let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(None));
	Extrinsic::new_signed(
		crate::Call::pay_for_recycler_unload_fee_token_with_external_asset {
			instance_id: TEST_INSTANCE_ID,
			member_key,
			proof_of_ownership,
			max_fee: unload_token_fee_in_asset(),
		}
		.into(),
		signer,
		UintAuthorityId(signer),
		extension,
	)
}

fn fund_external_asset(who: u64, amount: u64) {
	let asset_id = TEST_ASSET_ID;
	assert_ok!(Assets::mint(RuntimeOrigin::signed(1), asset_id, who, amount));
}

#[test]
fn wrong_origin_fail() {
	new_test_ext().execute_with(|| {
		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &1u64.encode()).unwrap();

		assert_noop!(
			Coinage::pay_for_recycler_unload_fee_token_with_external_asset(
				RuntimeOrigin::none(),
				TEST_INSTANCE_ID,
				member,
				proof,
				unload_token_fee_in_asset()
			),
			DispatchError::BadOrigin
		);
	});
}

#[test]
fn member_key_already_used_fail() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let signer1 = 1;
		let signer2 = 2;
		fund_external_asset(signer1, 1000);
		fund_external_asset(signer2, 1000);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);

		let proof1 = CryptoOf::<Test>::sign(&secret, &signer1.encode()).unwrap();
		let ext1 = build_ext(signer1, member, proof1);
		assert_eq!(Executive::apply_extrinsic(ext1), Ok(Ok(())));

		let proof2 = CryptoOf::<Test>::sign(&secret, &signer2.encode()).unwrap();
		let ext2 = build_ext(signer2, member, proof2);

		let res = Executive::apply_extrinsic(ext2);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), Error::<Test>::MemberKeyAlreadyUsed);
	});
}

#[test]
fn wrong_proof_of_ownership_fail() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let signer = 1;
		fund_external_asset(signer, 1000);
		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);

		let proof = CryptoOf::<Test>::sign(&secret, &999u64.encode()).unwrap();

		let ext = build_ext(signer, member, proof);

		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), Error::<Test>::InvalidProofOfOwnership);
	});
}

/// The conversion of the caller's asset is bounded by `max_fee`. This call is dispatched without
/// an extension check on the bound, so the bound is enforced in the dispatch itself.
#[test]
fn max_fee_below_the_converted_fee_fails() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_asset();
		let signer = 1;
		fund_external_asset(signer, 1000);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		assert_noop!(
			Coinage::pay_for_recycler_unload_fee_token_with_external_asset(
				RuntimeOrigin::signed(signer),
				TEST_INSTANCE_ID,
				member,
				proof,
				unload_token_fee_in_asset() - 1
			),
			Error::<Test>::FeeExceedsMaxFee
		);
	});
}

/// Paying with the asset needs a conversion into the native fee. Where none is available, the call
/// says so instead of charging something it could not price, and the caller can still buy the token
/// with the native currency.
#[test]
fn fee_conversion_unavailable_fails() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_asset();
		let signer = 1;
		fund_external_asset(signer, 1000);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		// No quote is available from the very first one on.
		set_fee_conversion_unavailable_at(Some(0));

		assert_noop!(
			Coinage::pay_for_recycler_unload_fee_token_with_external_asset(
				RuntimeOrigin::signed(signer),
				TEST_INSTANCE_ID,
				member,
				proof,
				1_000
			),
			Error::<Test>::CannotConvertAssetToNative
		);
		assert!(!PaidUnloadTokenMembers::<Test>::contains_key(member));

		set_fee_conversion_unavailable_at(None);
	});
}

/// The conversion keeps the payer's asset account alive, so the fee cannot be paid out of the
/// asset's minimum balance: a signer holding exactly the fee has to top up before they can pay.
#[test]
fn fee_cannot_take_the_signer_below_the_asset_minimum_balance() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_asset();
		let signer = 1;
		let fee = Coinage::quote_paid_unload_token_fee_in_asset(TEST_INSTANCE_ID).unwrap();
		let min_balance = Assets::minimum_balance(TEST_ASSET_ID);
		fund_external_asset(signer, fee);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		assert_noop!(
			Coinage::pay_for_recycler_unload_fee_token_with_external_asset(
				RuntimeOrigin::signed(signer),
				TEST_INSTANCE_ID,
				member,
				proof,
				fee
			),
			TokenError::NotExpendable
		);

		// With the minimum balance on top of the fee, the account survives the payment.
		fund_external_asset(signer, min_balance);
		assert_ok!(Coinage::pay_for_recycler_unload_fee_token_with_external_asset(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			member,
			proof,
			fee
		));
		assert_eq!(Assets::balance(TEST_ASSET_ID, signer), min_balance);
	});
}

#[test]
fn success_accounting() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_asset();
		let signer = 1;
		let initial_bal = 1000;
		fund_external_asset(signer, initial_bal);

		let fee = Coinage::quote_paid_unload_token_fee_in_asset(TEST_INSTANCE_ID).unwrap();
		let fee_dest = get_u64::<<Test as Config>::FeeDestination>();
		let asset_id = TEST_ASSET_ID;
		let market_initial = Assets::balance(asset_id, MOCK_MARKET);
		let dest_native_initial = Balances::free_balance(fee_dest);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		let ext = build_ext(signer, member, proof);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		assert_eq!(Assets::balance(asset_id, signer), initial_bal - fee);
		// The signer's asset went to the market, and the fee destination was paid in native.
		assert_eq!(Assets::balance(asset_id, MOCK_MARKET), market_initial + fee);
		assert_eq!(
			Balances::free_balance(fee_dest) - dest_native_initial,
			Coinage::get_paid_unload_token_fee_in_native()
		);
		System::assert_has_event(
			crate::Event::<Test>::PaidUnloadTokenRegisteredWithExternalAsset {
				instance_id: TEST_INSTANCE_ID,
				who: signer,
				fee,
			}
			.into(),
		);
	});
}

#[test]
fn success_token_is_usable_in_unload_call() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let signer = 1;
		fund_external_asset(signer, 1000);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		// 1. Pay
		let ext = build_ext(signer, member, proof);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		let period: u32 = 0;
		let index: u32 = 0;
		let val = 0;

		// 2. Build Ring
		Members::process_maintenance();

		// 3. Setup Recycler
		let (recycler_secrets, r_idx, r_rev) = setup_recycler(val, 1, 0);

		// 4. Unload
		let dest = 2;
		let aliases = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&recycler_secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap()];
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value: val,
			index: r_idx,
			revision: r_rev,
			to: dest,
			max_fee: 0,
		});

		let inherited = ((0u8, &call), (), ());
		let proven_msg = sp_io::hashing::blake2_256(&inherited.encode());

		let r_com = CryptoOf::<Test>::open(
			recycler_ring_size(),
			&CryptoOf::<Test>::member_from_secret(&recycler_secrets[0]),
			Coinage::get_recycler_members(TEST_INSTANCE_ID, val, r_idx).into_iter(),
		)
		.unwrap();
		let (r_proof, _) = CryptoOf::<Test>::create(
			r_com,
			&recycler_secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
			&proven_msg,
		)
		.unwrap();

		let alias_proofs = BoundedVec::try_from(vec![r_proof]).unwrap();

		let intent_msg =
			sp_io::hashing::blake2_256(&[alias_proofs.encode(), inherited.encode()].concat());

		let mut ctx = [0u8; 32];
		ctx[..28].copy_from_slice(PAID_UNLOAD_TOKEN_CONTEXT_BASE.as_ref());
		ctx[28..32].copy_from_slice(&period.to_le_bytes());

		let members = Coinage::get_paid_token_ring_members(period, index);
		let com =
			CryptoOf::<Test>::open(paid_token_ring_size(), &member, members.into_iter()).unwrap();
		let (p_proof, _) = CryptoOf::<Test>::create(com, &secret, &ctx, &intent_msg).unwrap();

		let id = Coinage::paid_token_collection_identifier(period);
		let revision = <Test as Config>::MemberService::ring_revision(&id, index).unwrap();
		let info = crate::extension::AsCoinageInfo::AsUnloadTokenPaid {
			proof: p_proof,
			period,
			paid_token_ring_index: index,
			paid_token_ring_revision: revision,
			alias_proofs,
		};

		let uxt = Extrinsic::new_signed(
			call,
			0,
			UintAuthorityId(0),
			(AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(info))),
		);

		assert_ok!(Executive::validate_transaction(
			TransactionSource::External,
			uxt,
			Default::default()
		));
	});
}

#[test]
fn success_new_ring_creation() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let ring_size = R2E10_RING_CAPACITY;

		for i in 0..ring_size {
			let signer = 1000 + i as u64;
			fund_external_asset(signer, 1000);
			let secret = get_unique_secret();
			let member = CryptoOf::<Test>::member_from_secret(&secret);
			let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();
			let ext = build_ext(signer, member, proof);
			assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));
		}

		// Ensure all 767 members are processed
		for _ in 0..10 {
			Members::process_maintenance();
		}

		// Verify ring 0 is full
		let id = Coinage::paid_token_collection_identifier(0);
		let status0 = <Test as Config>::MemberService::ring_status(&id, 0).unwrap();
		assert_eq!(status0.total, ring_size);

		let signer = 2000;
		fund_external_asset(signer, 1000);
		let secret = get_unique_secret();
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();
		let ext = build_ext(signer, member, proof);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		assert!(PaidUnloadTokenMembers::<Test>::contains_key(member));
	});
}

#[test]
fn success_with_previous_revision() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let signer = 1;
		fund_external_asset(signer, 1000);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		// 1. Pay for token
		let ext = build_ext(signer, member, proof);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		let period = 0;
		let index: u32 = 0;
		let val = 0;

		// 2. Build Ring
		let paid_id = Coinage::paid_token_collection_identifier(period);
		indiv_pallet_members::OnboardingSize::<Test>::insert(paid_id, 1u32);
		Members::process_maintenance();

		// Capture the ring members BEFORE adding more members
		let members_v1 = Coinage::get_paid_token_ring_members(period, index);
		let rev_after_first_build = Coinage::get_paid_token_ring_revision(period, index).unwrap();

		// 3. Add another member and build again (previous_root is set)
		let signer2 = 2;
		fund_external_asset(signer2, 1000);
		let secret2 = get_secret(2);
		let member2 = CryptoOf::<Test>::member_from_secret(&secret2);
		let proof2 = CryptoOf::<Test>::sign(&secret2, &signer2.encode()).unwrap();
		let ext2 = build_ext(signer2, member2, proof2);
		assert_eq!(Executive::apply_extrinsic(ext2), Ok(Ok(())));

		Members::process_maintenance();

		// Verify revision incremented
		let rev_after_second_build = Coinage::get_paid_token_ring_revision(period, index).unwrap();
		assert!(rev_after_second_build > rev_after_first_build);

		// 4. Setup Recycler
		let (recycler_secrets, r_idx, r_rev) = setup_recycler(val, 1, 0);

		// 5. Create unload call using OLD revision
		let dest = 3;
		let aliases = bounded_vec![CryptoOf::<Test>::alias_in_context(
			&recycler_secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.unwrap()];
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases,
			value: val,
			index: r_idx,
			revision: r_rev,
			to: dest,
			max_fee: 0,
		});

		let inherited = ((0u8, &call), (), ());
		let proven_msg = sp_io::hashing::blake2_256(&inherited.encode());

		let r_com = CryptoOf::<Test>::open(
			recycler_ring_size(),
			&CryptoOf::<Test>::member_from_secret(&recycler_secrets[0]),
			Coinage::get_recycler_members(TEST_INSTANCE_ID, val, r_idx).into_iter(),
		)
		.unwrap();
		let (r_proof, _) = CryptoOf::<Test>::create(
			r_com,
			&recycler_secrets[0],
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
			&proven_msg,
		)
		.unwrap();

		let alias_proofs = BoundedVec::try_from(vec![r_proof]).unwrap();

		let intent_msg =
			sp_io::hashing::blake2_256(&[alias_proofs.encode(), inherited.encode()].concat());

		let mut ctx = [0u8; 32];
		ctx[..28].copy_from_slice(PAID_UNLOAD_TOKEN_CONTEXT_BASE.as_ref());
		ctx[28..32].copy_from_slice(&period.to_le_bytes());

		// Generate proof using OLD ring members (v1)
		let com = CryptoOf::<Test>::open(paid_token_ring_size(), &member, members_v1.into_iter())
			.unwrap();
		let (p_proof, _) = CryptoOf::<Test>::create(com, &secret, &ctx, &intent_msg).unwrap();

		// Use OLD revision - should succeed because previous_root is valid
		let info = crate::extension::AsCoinageInfo::AsUnloadTokenPaid {
			proof: p_proof,
			period,
			paid_token_ring_index: index,
			paid_token_ring_revision: rev_after_first_build,
			alias_proofs,
		};

		let uxt = Extrinsic::new_signed(
			call,
			0,
			UintAuthorityId(0),
			(AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(info))),
		);

		// Should succeed because the previous revision's root is still valid
		assert_ok!(Executive::validate_transaction(
			TransactionSource::External,
			uxt,
			Default::default()
		));
	});
}
