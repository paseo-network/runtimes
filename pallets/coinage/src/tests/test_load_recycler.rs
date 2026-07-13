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

use crate::{mock::*, *};
use codec::Encode;
use frame_support::{assert_err, assert_ok};
use sp_runtime::{transaction_validity::TransactionSource, DispatchError};
use verifiable::GenerateVerifiable;

/// Helper to build a load_recycler_with_coin extrinsic.
pub fn build_load_ext(
	signer: u64,
	member_key: MemberOf<Test>,
	proof_of_ownership: SignatureOf<Test>,
	as_coin: bool,
) -> Extrinsic {
	build_signed_as_coin_ext(
		signer,
		crate::Call::load_recycler_with_coin { member_key, proof_of_ownership },
		as_coin,
	)
}

#[test]
fn load_recycler_bad_origin_fail() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		// Construct transaction without AsCoin extension info (as_coin: false).
		// The extension will pass through as Signed origin.
		let ext = build_load_ext(signer, member, proof, false);

		// Validation passes
		assert_ok!(Executive::validate_transaction(
			TransactionSource::External,
			ext.clone(),
			Default::default(),
		));

		// Dispatch fails because the pallet expects Origin::Coin
		let res = Executive::apply_extrinsic(ext);
		assert_ok!(res.as_ref());
		assert_err!(res.unwrap(), DispatchError::BadOrigin);
	});
}

#[test]
fn load_recycler_no_coin_invalid() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		// Signer has no coin
		let ext = build_load_ext(signer, member, proof, true);
		assert_invalid(ext, CustomInvalidity::NoCoin);
	});
}

#[test]
fn load_recycler_member_key_used_invalid() {
	new_test_ext().execute_with(|| {
		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);

		// 1. First user loads successfully with `member`
		let user1 = 1;
		CoinsByOwner::<Test>::insert(user1, Coin { value: 0, age: 0 });
		let proof1 = CryptoOf::<Test>::sign(&secret, &user1.encode()).unwrap();
		let ext1 = build_load_ext(user1, member, proof1, true);
		assert_eq!(Executive::apply_extrinsic(ext1), Ok(Ok(())));

		// 2. Second user tries to load with the same `member`
		let user2 = 2;
		CoinsByOwner::<Test>::insert(user2, Coin { value: 0, age: 0 });
		// Proof matches user2 and secret, but member key is already in recycler system
		let proof2 = CryptoOf::<Test>::sign(&secret, &user2.encode()).unwrap();
		let ext2 = build_load_ext(user2, member, proof2, true);

		assert_invalid(ext2, CustomInvalidity::MemberKeyAlreadyUsed);
	});
}

#[test]
fn load_recycler_proof_invalid() {
	new_test_ext().execute_with(|| {
		let signer = 1;
		CoinsByOwner::<Test>::insert(signer, Coin { value: 0, age: 0 });

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);

		// Case 1: Proof signs wrong message (wrong signer ID)
		let wrong_signer = 2u64;
		let proof_wrong_msg = CryptoOf::<Test>::sign(&secret, &wrong_signer.encode()).unwrap();

		let ext1 = build_load_ext(signer, member, proof_wrong_msg, true);
		assert_invalid(ext1, CustomInvalidity::InvalidProofOfOwnership);

		// Case 2: Proof signed by a different secret (key mismatch)
		let wrong_secret = get_secret(2);
		let proof_wrong_key = CryptoOf::<Test>::sign(&wrong_secret, &signer.encode()).unwrap();

		let ext2 = build_load_ext(signer, member, proof_wrong_key, true);
		assert_invalid(ext2, CustomInvalidity::InvalidProofOfOwnership);
	});
}

#[test]
fn load_new_recycler_success() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let signer = 1;
		let value = 0;
		CoinsByOwner::<Test>::insert(signer, Coin { value, age: 0 });

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		let ext = build_load_ext(signer, member, proof, true);

		// Ensure no recycler collection exists yet for this value
		assert!(!RecyclerCollectionCreated::<Test>::contains_key(value));

		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));

		// Check recycler collection created
		assert!(RecyclerCollectionCreated::<Test>::contains_key(value));

		// Check member mapping
		assert!(RecyclersCoinToRecycler::<Test>::contains_key(member));

		// Coin removed from owner
		assert!(!CoinsByOwner::<Test>::contains_key(signer));
		System::assert_has_event(crate::Event::<Test>::RecyclerLoadedWithCoin { value }.into());
	});
}

#[test]
fn load_existing_recycler_success() {
	new_test_ext().execute_with(|| {
		let value = 0;

		// 1. Create recycler with first user
		let signer1 = 1;
		CoinsByOwner::<Test>::insert(signer1, Coin { value, age: 0 });
		let secret1 = get_secret(1);
		let member1 = CryptoOf::<Test>::member_from_secret(&secret1);
		let proof1 = CryptoOf::<Test>::sign(&secret1, &signer1.encode()).unwrap();

		let ext1 = build_load_ext(signer1, member1, proof1, true);
		assert_eq!(Executive::apply_extrinsic(ext1), Ok(Ok(())));

		// 2. Load with second user into EXISTING recycler
		let signer2 = 2;
		CoinsByOwner::<Test>::insert(signer2, Coin { value, age: 0 });
		let secret2 = get_secret(2);
		let member2 = CryptoOf::<Test>::member_from_secret(&secret2);
		let proof2 = CryptoOf::<Test>::sign(&secret2, &signer2.encode()).unwrap();

		let ext2 = build_load_ext(signer2, member2, proof2, true);
		assert_eq!(Executive::apply_extrinsic(ext2), Ok(Ok(())));

		// Check both members are mapped
		assert!(RecyclersCoinToRecycler::<Test>::contains_key(member1));
		assert!(RecyclersCoinToRecycler::<Test>::contains_key(member2));
	});
}

#[test]
fn load_recycler_full_trigger_new_success() {
	new_test_ext().execute_with(|| {
		let value = 0;
		// Ring capacity is 767 (RingExponent::R2e10). Fill it to trigger a new ring.
		let ring_capacity = R2E10_RING_CAPACITY;

		// Fill the recycler to its capacity
		for i in 0..ring_capacity {
			let signer = 1000 + i as u64;
			CoinsByOwner::<Test>::insert(signer, Coin { value, age: 0 });

			let secret = get_unique_secret();
			let member = CryptoOf::<Test>::member_from_secret(&secret);
			let proof = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

			let ext = build_load_ext(signer, member, proof, true);
			assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));
		}

		// Collection should exist
		assert!(RecyclerCollectionCreated::<Test>::contains_key(value));

		// Load one more coin -> should go into the next ring
		let signer_new = 2000;
		CoinsByOwner::<Test>::insert(signer_new, Coin { value, age: 0 });
		let secret_new = get_unique_secret();
		let member_new = CryptoOf::<Test>::member_from_secret(&secret_new);
		let proof_new = CryptoOf::<Test>::sign(&secret_new, &signer_new.encode()).unwrap();

		let ext_new = build_load_ext(signer_new, member_new, proof_new, true);
		assert_eq!(Executive::apply_extrinsic(ext_new), Ok(Ok(())));

		// Verify the new member is mapped
		assert!(RecyclersCoinToRecycler::<Test>::contains_key(member_new));
	});
}
