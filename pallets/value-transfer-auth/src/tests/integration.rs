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
	extension::{payload_hash, AuthorizeValueTransfer},
	mock::{new_test_ext, test_keypair, RuntimeCall, RuntimeOrigin, Test, TestAuthorizationPubkey},
};
use codec::Encode;
use sp_core::Pair;
use sp_runtime::{
	generic::Era,
	traits::{DispatchInfoOf, ImplicationParts, TransactionExtension as _},
	transaction_validity::{InvalidTransaction, TransactionSource, TransactionValidityError},
};

type Ext = AuthorizeValueTransfer<Test, TestAuthorizationPubkey>;

fn value_call() -> RuntimeCall {
	RuntimeCall::System(frame_system::Call::remark_with_event { remark: vec![1, 2, 3] })
}

fn validate<I: sp_runtime::traits::Implication>(
	extension: &Ext,
	call: &RuntimeCall,
	implication: &I,
) -> Result<sp_runtime::transaction_validity::ValidTransaction, TransactionValidityError> {
	let info = DispatchInfoOf::<RuntimeCall>::default();
	extension
		.validate(
			RuntimeOrigin::none(),
			call,
			&info,
			call.encode().len(),
			(),
			implication,
			TransactionSource::External,
		)
		.map(|(validity, _val, _origin)| validity)
}

fn sign_over<I: sp_runtime::traits::Implication>(implication: &I) -> Ext {
	let (pair, _) = test_keypair();
	let hash = payload_hash(implication);
	AuthorizeValueTransfer(Some(pair.sign(&hash)), core::marker::PhantomData)
}

#[test]
fn signature_binds_check_nonce_implicit() {
	new_test_ext().execute_with(|| {
		let call = value_call();
		let impl_nonce_5 = ImplicationParts { base: (0u8, &call), explicit: (), implicit: 5u64 };
		let extension = sign_over(&impl_nonce_5);

		assert!(validate(&extension, &call, &impl_nonce_5).is_ok());

		let impl_nonce_6 = ImplicationParts { base: (0u8, &call), explicit: (), implicit: 6u64 };
		assert_eq!(
			validate(&extension, &call, &impl_nonce_6),
			Err(TransactionValidityError::Invalid(InvalidTransaction::BadProof))
		);
	});
}

#[test]
fn signature_binds_check_genesis_implicit() {
	new_test_ext().execute_with(|| {
		let call = value_call();
		let genesis_a = [0xAAu8; 32];
		let genesis_b = [0xBBu8; 32];

		let impl_genesis_a =
			ImplicationParts { base: (0u8, &call), explicit: (), implicit: genesis_a };
		let extension = sign_over(&impl_genesis_a);

		assert!(validate(&extension, &call, &impl_genesis_a).is_ok());

		let impl_genesis_b =
			ImplicationParts { base: (0u8, &call), explicit: (), implicit: genesis_b };
		assert_eq!(
			validate(&extension, &call, &impl_genesis_b),
			Err(TransactionValidityError::Invalid(InvalidTransaction::BadProof))
		);
	});
}

#[test]
fn signature_binds_check_era_implicit() {
	new_test_ext().execute_with(|| {
		let call = value_call();
		let era_a = (Era::mortal(64, 1u64), [0u8; 32]);
		let era_b = (Era::mortal(64, 2u64), [0u8; 32]);

		let impl_era_a = ImplicationParts { base: (0u8, &call), explicit: (), implicit: era_a };
		let extension = sign_over(&impl_era_a);

		assert!(validate(&extension, &call, &impl_era_a).is_ok());

		let impl_era_b = ImplicationParts { base: (0u8, &call), explicit: (), implicit: era_b };
		assert_eq!(
			validate(&extension, &call, &impl_era_b),
			Err(TransactionValidityError::Invalid(InvalidTransaction::BadProof))
		);
	});
}
