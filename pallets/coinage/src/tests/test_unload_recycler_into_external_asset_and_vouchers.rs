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
	mock::*,
	pallet::{self, Error},
	*,
};
use codec::Encode;
use frame_support::{assert_err, assert_ok, traits::fungibles::InspectHold as _, BoundedVec};
use frame_system::AuthorizeCall;
use indiv_support::traits::Alias;
use sp_core::blake2_256;
use sp_runtime::{bounded_vec, testing::UintAuthorityId};
use verifiable::GenerateVerifiable;

fn build_unload_ext(
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
	let info = crate::extension::AsCoinageInfo::AsUnloadTokenPeople {
		proof: people_proof,
		period,
		counter,
		alias_proofs,
	};

	let extension =
		(AuthorizeCall::<Test>::new(), crate::extension::AsCoinage::<Test>::new(Some(info)));
	Extrinsic::new_signed(call, 0, UintAuthorityId(0), extension)
}

fn create_alias_and_proof(
	secret: &Secret,
	value: CoinValue,
	index: RingIndex,
	proven_msg: &[u8; 32],
) -> (Alias, ProofOf<Test>) {
	let ring_members = Coinage::get_recycler_members(value, index);
	let member = CryptoOf::<Test>::member_from_secret(secret);
	let commitment =
		CryptoOf::<Test>::open(recycler_ring_size(), &member, ring_members.into_iter()).unwrap();
	let (proof, alias) = CryptoOf::<Test>::create(
		commitment,
		secret,
		UNLOADING_RECYCLER_CONTEXT.as_ref(),
		proven_msg,
	)
	.unwrap();

	(alias, proof)
}

#[test]
fn mixed_output_prepaid_success() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_asset();
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = &secrets[0];
		let proven_msg = [11u8; 32];
		let (alias, proof) = create_alias_and_proof(secret, 0, index, &proven_msg);
		let origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: UnloadFee::Prepaid,
		};

		let voucher_secret = CryptoOf::<Test>::new_secret([200u8; 32]);
		let voucher_member = CryptoOf::<Test>::member_from_secret(&voucher_secret);
		let pallet_acc = Coinage::pallet_account();

		assert_ok!(Coinage::unload_recycler_into_external_asset_and_vouchers(
			RuntimeOrigin::from(origin),
			bounded_vec![alias],
			0,
			index,
			revision,
			42,
			500,
			bounded_vec![(-1, voucher_member)],
		));

		assert!(RecyclersUnloaded::<Test>::contains_key((0, index, alias)));
		assert_eq!(RecyclersCoinToRecycler::<Test>::get(voucher_member), Some(-1));
		assert_eq!(AssetsWithHolder::total_balance(TEST_ASSET_ID, &42,), 500);
		assert_eq!(AssetsWithHolder::total_balance_on_hold(TEST_ASSET_ID, &pallet_acc,), 500);
		System::assert_has_event(
			crate::Event::<Test>::RecyclerUnloadedIntoExternalAssetAndVouchers {
				to: 42,
				value: 0,
				input_count: 1,
				external_asset_amount: 500,
				voucher_count: 1,
			}
			.into(),
		);
	});
}

#[test]
fn mixed_output_empty_aliases_rejected() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = &secrets[0];
		let proven_msg = [12u8; 32];
		let (_alias, proof) = create_alias_and_proof(secret, 0, index, &proven_msg);
		let origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: UnloadFee::Prepaid,
		};

		assert_err!(
			Coinage::unload_recycler_into_external_asset_and_vouchers(
				RuntimeOrigin::from(origin),
				bounded_vec![],
				0,
				index,
				revision,
				42,
				0,
				bounded_vec![(
					-2,
					CryptoOf::<Test>::member_from_secret(&CryptoOf::<Test>::new_secret([12u8; 32]))
				)],
			),
			Error::<Test>::EmptyInputs
		);
	});
}

#[test]
fn mixed_output_empty_vouchers_rejected() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = &secrets[0];
		let proven_msg = [13u8; 32];
		let (alias, proof) = create_alias_and_proof(secret, 0, index, &proven_msg);
		let origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: UnloadFee::Prepaid,
		};

		assert_err!(
			Coinage::unload_recycler_into_external_asset_and_vouchers(
				RuntimeOrigin::from(origin),
				bounded_vec![alias],
				0,
				index,
				revision,
				42,
				1000,
				bounded_vec![],
			),
			Error::<Test>::InvalidSplit
		);
	});
}

#[test]
fn mixed_output_from_output_success() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let value: CoinValue = 0;
		let dest = CHARLIE;
		let (secrets, index, revision) = setup_recycler(value, 2, 0);
		let secret = &secrets[0];
		let fee_amount = Coinage::get_paid_unload_token_fee_in_asset().unwrap();
		let external_asset_amount = 500u64;
		let proven_msg = blake2_256(&(value, index, revision, &dest).encode());
		let (alias, proof) = create_alias_and_proof(secret, value, index, &proven_msg);

		RecyclerManager::<Test>::mark_alias_unloaded(value, index, alias);

		let origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: pallet::UnloadFee::FromOutput {
				fee_recycler_value: value,
				fee_recycler_index: index,
			},
		};

		let voucher_secret = CryptoOf::<Test>::new_secret([201u8; 32]);
		let voucher_member = CryptoOf::<Test>::member_from_secret(&voucher_secret);
		let fee_dest_before = AssetsWithHolder::total_balance(TEST_ASSET_ID, &FEE_DESTINATION);
		let dest_before = AssetsWithHolder::total_balance(TEST_ASSET_ID, &dest);
		let pallet_acc = Coinage::pallet_account();
		let pallet_hold_before =
			AssetsWithHolder::total_balance_on_hold(TEST_ASSET_ID, &pallet_acc);

		assert_ok!(Coinage::unload_recycler_into_external_asset_and_vouchers(
			RuntimeOrigin::from(origin),
			bounded_vec![alias],
			value,
			index,
			revision,
			dest,
			external_asset_amount,
			bounded_vec![(-1, voucher_member)],
		));

		assert!(RecyclersUnloaded::<Test>::contains_key((value, index, alias)));
		assert_eq!(RecyclersCoinToRecycler::<Test>::get(voucher_member), Some(-1));
		assert_eq!(
			AssetsWithHolder::total_balance(TEST_ASSET_ID, &FEE_DESTINATION,) - fee_dest_before,
			fee_amount
		);
		assert_eq!(
			AssetsWithHolder::total_balance(TEST_ASSET_ID, &dest,) - dest_before,
			external_asset_amount - fee_amount
		);
		assert_eq!(
			pallet_hold_before -
				AssetsWithHolder::total_balance_on_hold(TEST_ASSET_ID, &pallet_acc,),
			external_asset_amount
		);
		System::assert_has_event(
			crate::Event::<Test>::RecyclerUnloadedIntoExternalAssetAndVouchers {
				to: dest,
				value,
				input_count: 1,
				external_asset_amount: external_asset_amount - fee_amount,
				voucher_count: 1,
			}
			.into(),
		);
	});
}

#[test]
fn mixed_output_voucher_only_success() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_asset();
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = &secrets[0];
		let proven_msg = [14u8; 32];
		let (alias, proof) = create_alias_and_proof(secret, 0, index, &proven_msg);
		let origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: UnloadFee::Prepaid,
		};

		let voucher_secret = CryptoOf::<Test>::new_secret([14u8; 32]);
		let voucher_member = CryptoOf::<Test>::member_from_secret(&voucher_secret);

		assert_ok!(Coinage::unload_recycler_into_external_asset_and_vouchers(
			RuntimeOrigin::from(origin),
			bounded_vec![alias],
			0,
			index,
			revision,
			42,
			0,
			bounded_vec![(0, voucher_member)],
		));

		assert_eq!(RecyclersCoinToRecycler::<Test>::get(voucher_member), Some(0));
		assert_eq!(AssetsWithHolder::total_balance(TEST_ASSET_ID, &42,), 0);
		System::assert_has_event(
			crate::Event::<Test>::RecyclerUnloadedIntoExternalAssetAndVouchers {
				to: 42,
				value: 0,
				input_count: 1,
				external_asset_amount: 0,
				voucher_count: 1,
			}
			.into(),
		);
	});
}

#[test]
fn mixed_output_exact_balance_mismatch_rejected() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = &secrets[0];
		let proven_msg = [22u8; 32];
		let (alias, proof) = create_alias_and_proof(secret, 0, index, &proven_msg);
		let origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: UnloadFee::Prepaid,
		};

		let voucher_secret = CryptoOf::<Test>::new_secret([202u8; 32]);
		let voucher_member = CryptoOf::<Test>::member_from_secret(&voucher_secret);

		assert_err!(
			Coinage::unload_recycler_into_external_asset_and_vouchers(
				RuntimeOrigin::from(origin),
				bounded_vec![alias],
				0,
				index,
				revision,
				42,
				400,
				bounded_vec![(-1, voucher_member)],
			),
			Error::<Test>::InvalidSplit
		);
	});
}

#[test]
fn mixed_output_duplicate_voucher_member_key_rejected() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(1, 1, 0);
		let secret = &secrets[0];
		let proven_msg = [33u8; 32];
		let (alias, proof) = create_alias_and_proof(secret, 1, index, &proven_msg);
		let origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: UnloadFee::Prepaid,
		};

		let voucher_secret = CryptoOf::<Test>::new_secret([203u8; 32]);
		let voucher_member = CryptoOf::<Test>::member_from_secret(&voucher_secret);

		assert_err!(
			Coinage::unload_recycler_into_external_asset_and_vouchers(
				RuntimeOrigin::from(origin),
				bounded_vec![alias],
				1,
				index,
				revision,
				42,
				4000,
				bounded_vec![(0, voucher_member), (0, voucher_member)],
			),
			Error::<Test>::MemberKeyAlreadyUsed
		);
	});
}

#[test]
fn mixed_output_used_voucher_member_key_rejected() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = &secrets[0];
		let proven_msg = [34u8; 32];
		let (alias, proof) = create_alias_and_proof(secret, 0, index, &proven_msg);
		let origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: UnloadFee::Prepaid,
		};

		let voucher_secret = CryptoOf::<Test>::new_secret([204u8; 32]);
		let voucher_member = CryptoOf::<Test>::member_from_secret(&voucher_secret);
		assert_ok!(RecyclerManager::<Test>::load_batch_grouped(&[(1, voucher_member)]));

		assert_err!(
			Coinage::unload_recycler_into_external_asset_and_vouchers(
				RuntimeOrigin::from(origin),
				bounded_vec![alias],
				0,
				index,
				revision,
				42,
				500,
				bounded_vec![(-1, voucher_member)],
			),
			Error::<Test>::MemberKeyAlreadyUsed
		);
	});
}

#[test]
fn mixed_output_invalid_voucher_value_rejected() {
	new_test_ext().execute_with(|| {
		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = &secrets[0];
		let proven_msg = [44u8; 32];
		let (alias, proof) = create_alias_and_proof(secret, 0, index, &proven_msg);
		let origin = pallet::Origin::<Test>::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: UnloadFee::Prepaid,
		};

		let voucher_secret = CryptoOf::<Test>::new_secret([204u8; 32]);
		let voucher_member = CryptoOf::<Test>::member_from_secret(&voucher_secret);
		let invalid_value = <Test as Config>::MaximumExponent::get() + 1;

		assert_err!(
			Coinage::unload_recycler_into_external_asset_and_vouchers(
				RuntimeOrigin::from(origin),
				bounded_vec![alias],
				0,
				index,
				revision,
				42,
				0,
				bounded_vec![(invalid_value, voucher_member)],
			),
			Error::<Test>::CoinValueOutOfBound
		);
	});
}

#[test]
fn mixed_output_invalid_split_fails_in_extension_before_consuming_free_token() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_asset();

		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = &secrets[0];
		let alias: Alias =
			CryptoOf::<Test>::alias_in_context(secret, UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();
		let voucher_secret = CryptoOf::<Test>::new_secret([207u8; 32]);
		let voucher_member = CryptoOf::<Test>::member_from_secret(&voucher_secret);
		let people_alias = [77u8; 32];

		let call =
			RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset_and_vouchers {
				aliases: bounded_vec![alias],
				value: 0,
				index,
				revision,
				to: 42,
				external_asset_amount: 400,
				new_vouchers: bounded_vec![(-1, voucher_member)],
			});

		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, Some(people_alias));
		assert_invalid(ext, CustomInvalidity::InvalidSplit);
		assert!(!Coinage::is_free_token_alias_consumed(0, people_alias));
		assert!(!RecyclersUnloaded::<Test>::contains_key((0, index, alias)));
	});
}

#[test]
fn mixed_output_invalid_split_fails_in_extension_before_premarking_alias() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let value: CoinValue = 0;
		let dest = CHARLIE;
		let (secrets, index, revision) = setup_recycler(value, 1, 0);
		let alias =
			CryptoOf::<Test>::alias_in_context(&secrets[0], UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();
		let voucher_secret = CryptoOf::<Test>::new_secret([208u8; 32]);
		let voucher_member = CryptoOf::<Test>::member_from_secret(&voucher_secret);

		let call = crate::Call::unload_recycler_into_external_asset_and_vouchers {
			aliases: bounded_vec![alias],
			value,
			index,
			revision,
			to: dest,
			external_asset_amount: 501,
			new_vouchers: bounded_vec![(-1, voucher_member)],
		};

		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);
		assert_invalid(ext, CustomInvalidity::InvalidSplit);
		assert!(!RecyclersUnloaded::<Test>::contains_key((value, index, alias)));
	});
}

#[test]
fn mixed_output_from_output_rejects_asset_portion_below_fee() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_balances();

		let value: CoinValue = 0;
		let dest = CHARLIE;
		let (secrets, index, revision) = setup_recycler(value, 1, 0);
		let alias =
			CryptoOf::<Test>::alias_in_context(&secrets[0], UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();
		let voucher_secret = CryptoOf::<Test>::new_secret([211u8; 32]);
		let voucher_member = CryptoOf::<Test>::member_from_secret(&voucher_secret);

		let call = crate::Call::unload_recycler_into_external_asset_and_vouchers {
			aliases: bounded_vec![alias],
			value,
			index,
			revision,
			to: dest,
			external_asset_amount: 0,
			new_vouchers: bounded_vec![(0, voucher_member)],
		};

		let ext = build_unload_from_output_ext(call, value, index, revision, &secrets[0..1]);
		assert_invalid(ext, CustomInvalidity::InvalidSplit);
		assert!(!RecyclersUnloaded::<Test>::contains_key((value, index, alias)));
	});
}

#[test]
fn mixed_output_multi_value_vouchers_work_via_unload_token_extension() {
	new_test_ext().execute_with(|| {
		// This is the one end-to-end test that still proves grouped voucher
		// reminting across multiple values through the unload-token extension.
		System::set_block_number(1);
		setup_asset();

		let (secrets, index, revision) = setup_recycler(0, 1, 0);
		let secret = &secrets[0];
		let alias: Alias =
			CryptoOf::<Test>::alias_in_context(secret, UNLOADING_RECYCLER_CONTEXT.as_ref())
				.unwrap();
		let voucher_secret_1 = CryptoOf::<Test>::new_secret([209u8; 32]);
		let voucher_secret_2 = CryptoOf::<Test>::new_secret([210u8; 32]);
		let voucher_member_1 = CryptoOf::<Test>::member_from_secret(&voucher_secret_1);
		let voucher_member_2 = CryptoOf::<Test>::member_from_secret(&voucher_secret_2);

		let call =
			RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset_and_vouchers {
				aliases: bounded_vec![alias],
				value: 0,
				index,
				revision,
				to: 42,
				external_asset_amount: 250,
				new_vouchers: bounded_vec![(-2, voucher_member_1), (-1, voucher_member_2)],
			});

		let ext = build_unload_ext(call, 0, 0, &secrets, 0, index, None);
		let result = Executive::apply_extrinsic(ext);
		assert!(result.is_ok(), "Extrinsic should succeed: {result:?}");
		assert_eq!(RecyclersCoinToRecycler::<Test>::get(voucher_member_1), Some(-2));
		assert_eq!(RecyclersCoinToRecycler::<Test>::get(voucher_member_2), Some(-1));
		assert_eq!(AssetsWithHolder::total_balance(TEST_ASSET_ID, &42,), 250);
	});
}
