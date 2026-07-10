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

//! Tests for the `InfallibleUnpaidSigned` transaction extension variant.

use crate::{extension::*, mock::*, *};
use codec::Encode;
use frame_support::assert_ok;
use frame_system::AuthorizeCall;
use sp_runtime::{
	testing::UintAuthorityId,
	transaction_validity::{InvalidTransaction, TransactionValidityError},
};
use verifiable::GenerateVerifiable;
use CodecPreservation::{Expendable, Protect};

fn fund_asset(who: u64, amount: u64) {
	assert_ok!(Assets::mint(RuntimeOrigin::signed(1), TEST_ASSET_ID, who, amount));
}

fn asset_balance(who: u64) -> u64 {
	Assets::balance(TEST_ASSET_ID, who)
}

/// Build an infallible-unpaid extrinsic for `load_recycler_with_external_asset`.
fn build_infallible_unpaid_ext(
	signer: u64,
	nonce: u32,
	preservation: CodecPreservation,
	value: CoinValue,
	member_key: MemberOf<Test>,
	proof_of_ownership: SignatureOf<Test>,
) -> Extrinsic {
	let call = crate::Call::load_recycler_with_external_asset_unpaid {
		preservation,
		value,
		member_key,
		proof_of_ownership,
	};
	let info = Some(AsCoinageInfo::InfallibleUnpaidSigned { nonce });
	let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(info));
	Extrinsic::new_signed(call.into(), signer, UintAuthorityId(signer), extension)
}

// ==================== Validation tests ====================

#[test]
fn infallible_unpaid_invalid_call() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let user = 1;
		fund_asset(user, 10_000);

		// Build an infallible_unpaid extrinsic but for a `transfer` call instead of
		// `load_recycler_with_external_asset`.
		let call = crate::Call::<Test>::transfer { to: 2 };
		let info = Some(AsCoinageInfo::InfallibleUnpaidSigned { nonce: 0 });
		let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(info));
		let ext = Extrinsic::new_signed(call.into(), user, UintAuthorityId(user), extension);

		assert_invalid(ext, CustomInvalidity::InvalidCall);
	});
}

#[test]
fn infallible_unpaid_asset_id_not_set_rejected_in_validation() {
	new_test_ext().execute_with(|| {
		setup_asset();
		Coinage::do_initialize().unwrap();
		let user = 42;
		fund_asset(user, 10_000);
		crate::UnderlyingAssetId::<Test>::kill();

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

		let ext = build_infallible_unpaid_ext(user, 0, Protect, 0, member, proof);
		assert_invalid(ext, CustomInvalidity::AssetIdNotSet);
	});
}

#[test]
fn infallible_unpaid_insufficient_balance_zero() {
	new_test_ext().execute_with(|| {
		setup_asset();
		Coinage::do_initialize().unwrap();
		let user = 42;
		// User has zero asset balance.

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

		let ext = build_infallible_unpaid_ext(user, 0, Protect, 0, member, proof);
		assert_invalid(ext, CustomInvalidity::InfallibleUnpaidSignedInsufficientBalance);
	});
}

#[test]
fn infallible_unpaid_insufficient_balance_just_below() {
	new_test_ext().execute_with(|| {
		setup_asset();
		Coinage::do_initialize().unwrap();
		let user = 42;
		let amount = Coinage::coin_value_to_asset_amount(0i8).unwrap();
		assert!(amount > 0, "amount must be non-zero for this test to be meaningful");

		// Fund user with exactly amount - 1 (not enough).
		fund_asset(user, amount - 1);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

		let ext = build_infallible_unpaid_ext(user, 0, Protect, 0, member, proof);
		assert_invalid(ext, CustomInvalidity::InfallibleUnpaidSignedInsufficientBalance);
	});
}

#[test]
fn infallible_unpaid_sufficient_balance() {
	new_test_ext().execute_with(|| {
		setup_asset();
		Coinage::do_initialize().unwrap();
		let user = 42;
		let amount = Coinage::coin_value_to_asset_amount(0i8).unwrap();

		// Fund with extra to cover the existential deposit (Protect preservation).
		fund_asset(user, amount + 1);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

		let ext = build_infallible_unpaid_ext(user, 0, Protect, 0, member, proof);
		// Validation should pass.
		let res = Executive::validate_transaction(
			sp_runtime::transaction_validity::TransactionSource::External,
			ext,
			Default::default(),
		);
		assert!(res.is_ok(), "validation should pass when balance >= amount: {res:?}");
	});
}

#[test]
fn infallible_unpaid_wrong_nonce_stale() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let user = 1;
		fund_asset(user, 10_000);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

		// First call with nonce 0 succeeds, incrementing nonce to 1.
		let ext = build_infallible_unpaid_ext(user, 0, Protect, 0, member, proof);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));
		assert_eq!(frame_system::Pallet::<Test>::account_nonce(user), 1);

		// Second call with stale nonce 0 should fail.
		let secret2 = get_secret(2);
		let member2 = CryptoOf::<Test>::member_from_secret(&secret2);
		let proof2 = CryptoOf::<Test>::sign(&secret2, &user.encode()).unwrap();
		let ext2 = build_infallible_unpaid_ext(user, 0, Protect, 0, member2, proof2);
		assert_eq!(
			Executive::validate_transaction(
				sp_runtime::transaction_validity::TransactionSource::External,
				ext2,
				Default::default(),
			),
			Err(TransactionValidityError::Invalid(InvalidTransaction::Stale))
		);
	});
}

#[test]
fn infallible_unpaid_future_nonce_has_requires() {
	new_test_ext().execute_with(|| {
		setup_asset();
		Coinage::do_initialize().unwrap();
		let user = 1;
		fund_asset(user, 10_000);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

		// Nonce 1 when current is 0 validates but produces a `requires` dependency,
		// so it cannot be included in a block until nonce 0 is consumed.
		let ext = build_infallible_unpaid_ext(user, 1, Protect, 0, member, proof);
		let res = Executive::validate_transaction(
			sp_runtime::transaction_validity::TransactionSource::External,
			ext,
			Default::default(),
		);
		let valid = res.unwrap();
		assert!(!valid.requires.is_empty(), "future nonce should produce requires dependency");
	});
}

#[test]
fn infallible_unpaid_recycler_collection_not_created() {
	new_test_ext().execute_with(|| {
		setup_asset();
		// Intentionally skip `Coinage::do_initialize()` so no recycler collection exists.
		let user = 1;
		fund_asset(user, 10_000);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

		let ext = build_infallible_unpaid_ext(user, 0, Protect, 0, member, proof);
		assert_eq!(
			Executive::validate_transaction(
				sp_runtime::transaction_validity::TransactionSource::External,
				ext,
				Default::default(),
			),
			Err(TransactionValidityError::Invalid(InvalidTransaction::Custom(
				CustomInvalidity::RecyclerCollectionNotCreated as u8
			)))
		);
	});
}

// ==================== Success path tests ====================

#[test]
fn infallible_unpaid_load_recycler_success_protect() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_asset();
		let user = 1;
		let value = 0;
		let asset_amount = Coinage::coin_value_to_asset_amount(value).unwrap();
		// Fund with extra to cover the existential deposit (Protect preservation).
		fund_asset(user, asset_amount + 1);

		let balance_before = asset_balance(user);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

		let ext = build_infallible_unpaid_ext(user, 0, Protect, value, member, proof);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Recycler loaded.
		assert!(RecyclersCoinToRecycler::<Test>::contains_key(member));

		// Only the asset transfer was deducted, no fee.
		let balance_after = asset_balance(user);
		assert_eq!(balance_before - balance_after, asset_amount);

		// Nonce incremented.
		assert_eq!(frame_system::Pallet::<Test>::account_nonce(user), 1);

		// Event emitted.
		System::assert_has_event(
			crate::Event::<Test>::RecyclerLoadedWithExternalAsset {
				who: user,
				value,
				amount: asset_amount,
			}
			.into(),
		);
	});
}

#[test]
fn infallible_unpaid_load_recycler_success_expendable() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_asset();
		let user = 1;
		let value = 0;
		let asset_amount = Coinage::coin_value_to_asset_amount(value).unwrap();
		// Fund with exactly the asset amount (Expendable allows draining the account).
		fund_asset(user, asset_amount);

		let balance_before = asset_balance(user);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &user.encode()).unwrap();

		let ext = build_infallible_unpaid_ext(user, 0, Expendable, value, member, proof);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Recycler loaded.
		assert!(RecyclersCoinToRecycler::<Test>::contains_key(member));

		// Only the asset transfer was deducted, no fee.
		let balance_after = asset_balance(user);
		assert_eq!(balance_before - balance_after, asset_amount);

		// Event emitted.
		System::assert_has_event(
			crate::Event::<Test>::RecyclerLoadedWithExternalAsset {
				who: user,
				value,
				amount: asset_amount,
			}
			.into(),
		);
	});
}

#[test]
fn infallible_unpaid_nonce_increments_across_calls() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let user = 1;
		fund_asset(user, 10_000);

		// First call with nonce 0.
		let secret1 = get_secret(1);
		let member1 = CryptoOf::<Test>::member_from_secret(&secret1);
		let proof1 = CryptoOf::<Test>::sign(&secret1, &user.encode()).unwrap();
		let ext1 = build_infallible_unpaid_ext(user, 0, Protect, 0, member1, proof1);
		assert_eq!(Executive::apply_extrinsic(ext1), Ok(Ok(())));
		assert_eq!(frame_system::Pallet::<Test>::account_nonce(user), 1);

		// Second call with nonce 1.
		let secret2 = get_secret(2);
		let member2 = CryptoOf::<Test>::member_from_secret(&secret2);
		let proof2 = CryptoOf::<Test>::sign(&secret2, &user.encode()).unwrap();
		let ext2 = build_infallible_unpaid_ext(user, 1, Protect, 0, member2, proof2);
		assert_eq!(Executive::apply_extrinsic(ext2), Ok(Ok(())));
		assert_eq!(frame_system::Pallet::<Test>::account_nonce(user), 2);
	});
}
