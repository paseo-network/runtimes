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
use frame_support::weights::Weight;
use sp_core::{ed25519, Pair};
use sp_runtime::{
	traits::{DispatchInfoOf, ImplicationParts, TransactionExtension as _},
	transaction_validity::{InvalidTransaction, TransactionSource, TransactionValidityError},
};

type Ext = AuthorizeValueTransfer<Test, TestAuthorizationPubkey>;

fn any_call() -> RuntimeCall {
	RuntimeCall::System(frame_system::Call::remark { remark: vec![1, 2, 3] })
}

fn implication_for(call: &RuntimeCall) -> ImplicationParts<(u8, &RuntimeCall), (), ()> {
	ImplicationParts { base: (0u8, call), explicit: (), implicit: () }
}

fn signed_extension_for(call: &RuntimeCall) -> Ext {
	let (pair, _pubkey) = test_keypair();
	let implication = implication_for(call);
	let payload = payload_hash(&implication);
	AuthorizeValueTransfer(Some(pair.sign(&payload)), core::marker::PhantomData)
}

fn wrong_key_signature_for(call: &RuntimeCall) -> ed25519::Signature {
	let pair = ed25519::Pair::from_seed(&[0x24; 32]);
	let implication = implication_for(call);
	let payload = payload_hash(&implication);
	pair.sign(&payload)
}

fn validate(
	extension: &Ext,
	call: &RuntimeCall,
	implication: &impl sp_runtime::traits::Implication,
) -> Result<(sp_runtime::transaction_validity::ValidTransaction, bool), TransactionValidityError> {
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
		.map(|(validity, val, _origin)| (validity, val))
}

#[test]
fn no_signature_returns_val_false() {
	new_test_ext().execute_with(|| {
		let call = any_call();
		let implication = implication_for(&call);
		let extension = Ext::default();

		let (_, val) = validate(&extension, &call, &implication).expect("validate succeeds");
		assert!(!val);
	});
}

#[test]
fn valid_signature_returns_val_true() {
	new_test_ext().execute_with(|| {
		let call = any_call();
		let implication = implication_for(&call);
		let extension = signed_extension_for(&call);

		let (_, val) = validate(&extension, &call, &implication).expect("validate succeeds");
		assert!(val);
	});
}

#[test]
fn signature_signed_by_wrong_key_fails() {
	new_test_ext().execute_with(|| {
		let call = any_call();
		let implication = implication_for(&call);
		let extension: Ext =
			AuthorizeValueTransfer(Some(wrong_key_signature_for(&call)), core::marker::PhantomData);

		assert_eq!(
			validate(&extension, &call, &implication).map(|(v, _)| v),
			Err(TransactionValidityError::Invalid(InvalidTransaction::BadProof))
		);
	});
}

#[test]
fn signature_does_not_replay_across_different_inherited_implication() {
	new_test_ext().execute_with(|| {
		let call = any_call();
		let extension = signed_extension_for(&call);
		let different_implication =
			ImplicationParts { base: (1u8, &call), explicit: (), implicit: () };

		assert_eq!(
			validate(&extension, &call, &different_implication).map(|(v, _)| v),
			Err(TransactionValidityError::Invalid(InvalidTransaction::BadProof))
		);
	});
}

#[test]
fn weight_is_zero_without_signature() {
	new_test_ext().execute_with(|| {
		let extension = Ext::default();

		assert_eq!(extension.weight(&any_call()), Weight::zero());
	});
}

#[test]
fn weight_is_nonzero_with_signature() {
	new_test_ext().execute_with(|| {
		let call = any_call();
		let extension = signed_extension_for(&call);

		assert_ne!(extension.weight(&call), Weight::zero());
	});
}
