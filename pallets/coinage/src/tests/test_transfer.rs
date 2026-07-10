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

use crate::{mock::*, pallet::CustomInvalidity, *};
use frame_support::{assert_err, assert_ok};
use sp_runtime::{transaction_validity::TransactionSource, DispatchError};

/// Helper to build a transfer extrinsic.
pub fn build_transfer_ext(signer: u64, to: u64, as_coin: bool) -> Extrinsic {
	build_signed_as_coin_ext(signer, crate::Call::transfer { to }, as_coin)
}

#[test]
fn transfer_bad_origin_fail() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		let dest = 2;
		// Construct transaction without AsCoin extension info.
		// The extension will validate as "NotUsing" and set origin to Signed(signer).
		let ext = build_transfer_ext(signer, dest, false);

		// Validation passes because the extension allows pass-through.
		assert_ok!(Executive::validate_transaction(
			TransactionSource::External,
			ext.clone(),
			Default::default(),
		));

		// Dispatch fails because the pallet expects Origin::Coin.
		// This represents the "fail" scenario (valid transaction, failed execution).
		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref()); // Ensure validity
		assert_err!(res.unwrap(), DispatchError::BadOrigin);
	});
}

#[test]
fn transfer_coin_not_exist_invalid() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		let dest = 2;
		// Signer has no coin in storage.
		let ext = build_transfer_ext(signer, dest, true);
		assert_invalid(ext, CustomInvalidity::NoCoin);
	});
}

#[test]
fn transfer_dest_has_coin_invalid() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		let dest = 2;
		// Signer has a coin.
		CoinsByOwner::<Test>::insert(signer, Coin { value: 1, age: 0 });
		// Destination already has a coin.
		CoinsByOwner::<Test>::insert(dest, Coin { value: 2, age: 5 });

		let ext = build_transfer_ext(signer, dest, true);
		assert_invalid(ext, CustomInvalidity::AddressAlreadyHasCoin);
	});
}

#[test]
fn transfer_coin_max_age_invalid() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		let dest = 2;
		let max_age = get_u16::<<Test as Config>::MaximumAge>();
		// Insert coin with max age.
		CoinsByOwner::<Test>::insert(signer, Coin { value: 1, age: max_age });

		let ext = build_transfer_ext(signer, dest, true);
		assert_invalid(ext, CustomInvalidity::CoinTooOld);
	});
}

#[test]
fn transfer_valid_success() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let signer = 1;
		let dest = 2;
		let initial_age = 10;
		let value = 1;
		// Insert a valid coin for signer.
		CoinsByOwner::<Test>::insert(signer, Coin { value, age: initial_age });

		let ext = build_transfer_ext(signer, dest, true);

		// Execute transaction
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Check storage updates
		assert!(!CoinsByOwner::<Test>::contains_key(signer), "Signer coin should be removed");
		let dest_coin = CoinsByOwner::<Test>::get(dest).expect("Dest should have received coin");
		assert_eq!(
			dest_coin,
			Coin { value, age: initial_age + 1 },
			"Coin should have same value and age + 1"
		);
		System::assert_has_event(
			crate::Event::<Test>::CoinTransferred { to: dest, value, new_age: initial_age + 1 }
				.into(),
		);
	});
}
