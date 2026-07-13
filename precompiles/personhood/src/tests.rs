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

use super::*;
use crate::{mock::*, DEFAULT_CONTEXT_ALIAS};

use alloy::sol_types::SolCall;
use indiv_support::traits::{Alias, Context, PEOPLE_IDENTIFIER, PEOPLE_LITE_IDENTIFIER};
use pallet_revive::{precompiles::AddressMapper, ExecConfig, TransactionLimits};
use sp_runtime::Weight;

fn test_context() -> Context {
	let mut ctx = DEFAULT_CONTEXT_ALIAS;
	ctx[..5].copy_from_slice(b"dotns");
	ctx
}

fn call_precompile(
	caller: u64,
	target_account: &sp_runtime::AccountId32,
	context: &Context,
) -> IPersonhood::PersonhoodInfo {
	let caller_account = id_to_account(caller);
	map_account::<Test>(&caller_account);

	let target_address = <Test as pallet_revive::Config>::AddressMapper::to_address(target_account);

	let input = IPersonhood::personhoodStatusCall {
		account: target_address.0.into(),
		context: (*context).into(),
	}
	.abi_encode();

	let data = pallet_revive::Pallet::<Test>::bare_call(
		RuntimeOrigin::signed(caller_account),
		PRECOMPILE_ADDR,
		0u32.into(),
		TransactionLimits::WeightAndDeposit { weight_limit: Weight::MAX, deposit_limit: u64::MAX },
		input,
		&ExecConfig::new_substrate_tx(),
	)
	.result
	.expect("precompile call should succeed")
	.data;

	IPersonhood::personhoodStatusCall::abi_decode_returns(&data).unwrap()
}

#[test]
fn returns_none_for_unknown_account() {
	new_test_ext().execute_with(|| {
		let info = call_precompile(1, &id_to_account(99), &test_context());
		assert_eq!(info.status as u8, NO_STATUS);
		assert_eq!(info.contextAlias.0, DEFAULT_CONTEXT_ALIAS);
	});
}

#[test]
fn returns_lite_with_alias() {
	new_test_ext().execute_with(|| {
		let target = id_to_account(10);
		map_account::<Test>(&target);
		set_personhood(&target, &test_context(), *PEOPLE_LITE_IDENTIFIER, ALICE_ALIAS);

		let info = call_precompile(1, &target, &test_context());
		assert_eq!(info.status as u8, LITE_STATUS);
		assert_eq!(info.contextAlias.0, ALICE_ALIAS);
	});
}

#[test]
fn returns_full_with_alias() {
	new_test_ext().execute_with(|| {
		let target = id_to_account(20);
		map_account::<Test>(&target);
		set_personhood(&target, &test_context(), *PEOPLE_IDENTIFIER, BOB_ALIAS);

		let info = call_precompile(1, &target, &test_context());
		assert_eq!(info.status as u8, FULL_STATUS);
		assert_eq!(info.contextAlias.0, BOB_ALIAS);
	});
}

#[test]
fn wrong_context_returns_none() {
	new_test_ext().execute_with(|| {
		let target = id_to_account(4);
		map_account::<Test>(&target);
		let other_context = [0xFFu8; 32];
		set_personhood(&target, &other_context, *PEOPLE_IDENTIFIER, DAVE_ALIAS);

		let info = call_precompile(1, &target, &test_context());
		assert_eq!(info.status as u8, NO_STATUS);
	});
}

#[test]
fn unknown_collection_returns_none() {
	new_test_ext().execute_with(|| {
		let target = id_to_account(5);
		map_account::<Test>(&target);
		let unknown_collection = [0xABu8; 32];
		set_personhood(&target, &test_context(), unknown_collection, EVE_ALIAS);

		let info = call_precompile(1, &target, &test_context());
		assert_eq!(info.status as u8, NO_STATUS);
		assert_eq!(info.contextAlias.0, DEFAULT_CONTEXT_ALIAS);
	});
}

fn call_proof_precompile(
	caller: u64,
	expected_status: u8,
	expected_alias: Alias,
	context: &Context,
	proof: Vec<u8>,
) -> pallet_revive::ContractResult<pallet_revive::ExecReturnValue, u64> {
	let caller_account = id_to_account(caller);
	map_account::<Test>(&caller_account);

	let input = IPersonhood::personhoodInfoByProofCall {
		request: IPersonhood::ProofVerificationRequest {
			expectedStatus: expected_status,
			proof: proof.into(),
			expectedAlias: expected_alias.into(),
			ringIndex: 0,
			context: (*context).into(),
			revision: 1,
			message: Vec::new().into(),
		},
	}
	.abi_encode();

	pallet_revive::Pallet::<Test>::bare_call(
		RuntimeOrigin::signed(caller_account),
		PRECOMPILE_ADDR,
		0u32.into(),
		TransactionLimits::WeightAndDeposit { weight_limit: Weight::MAX, deposit_limit: u64::MAX },
		input,
		&ExecConfig::new_substrate_tx(),
	)
}

fn decode_proof_status(
	result: pallet_revive::ContractResult<pallet_revive::ExecReturnValue, u64>,
) -> bool {
	let data = result.result.expect("precompile call should succeed").data;
	IPersonhood::personhoodInfoByProofCall::abi_decode_returns(&data).unwrap()
}

#[test]
fn proof_returns_full_for_people_match() {
	new_test_ext().execute_with(|| {
		set_proof_result(ALICE_ALIAS, test_context(), *PEOPLE_IDENTIFIER);

		let ok = decode_proof_status(call_proof_precompile(
			1,
			FULL_STATUS,
			ALICE_ALIAS,
			&test_context(),
			Vec::new(),
		));
		assert!(ok);
	});
}

#[test]
fn proof_returns_lite_for_people_lite_match() {
	new_test_ext().execute_with(|| {
		set_proof_result(BOB_ALIAS, test_context(), *PEOPLE_LITE_IDENTIFIER);

		let ok = decode_proof_status(call_proof_precompile(
			1,
			LITE_STATUS,
			BOB_ALIAS,
			&test_context(),
			Vec::new(),
		));
		assert!(ok);
	});
}

#[test]
fn proof_returns_none_for_unknown_alias() {
	new_test_ext().execute_with(|| {
		let ok = decode_proof_status(call_proof_precompile(
			1,
			FULL_STATUS,
			CHARLIE_ALIAS,
			&test_context(),
			Vec::new(),
		));
		assert!(!ok);
	});
}

#[test]
fn proof_returns_none_for_proof_belonging_to_other_collection() {
	new_test_ext().execute_with(|| {
		// Proof was issued under People-Lite but caller asks for Full.
		set_proof_result(DAVE_ALIAS, test_context(), *PEOPLE_LITE_IDENTIFIER);

		let ok = decode_proof_status(call_proof_precompile(
			1,
			FULL_STATUS,
			DAVE_ALIAS,
			&test_context(),
			Vec::new(),
		));
		assert!(!ok);
	});
}

#[test]
fn proof_returns_none_for_unsupported_status() {
	new_test_ext().execute_with(|| {
		set_proof_result(ALICE_ALIAS, test_context(), *PEOPLE_IDENTIFIER);

		let ok = decode_proof_status(call_proof_precompile(
			1,
			99,
			ALICE_ALIAS,
			&test_context(),
			Vec::new(),
		));
		assert!(!ok);
	});
}

#[test]
fn proof_unsupported_status_refunds_gas() {
	new_test_ext().execute_with(|| {
		set_proof_result(ALICE_ALIAS, test_context(), *PEOPLE_IDENTIFIER);

		let result_match = call_proof_precompile(
			1,
			FULL_STATUS,
			ALICE_ALIAS,
			&test_context(),
			Vec::new(),
		);
		let result_unsupported = call_proof_precompile(
			2,
			99,
			ALICE_ALIAS,
			&test_context(),
			Vec::new(),
		);

		let weight_match = result_match.weight_consumed;
		let weight_unsupported = result_unsupported.weight_consumed;
		assert!(decode_proof_status(result_match));
		assert!(!decode_proof_status(result_unsupported));

		assert!(
			weight_unsupported.ref_time() < weight_match.ref_time(),
			"unsupported-status path ({weight_unsupported:?}) should refund vs matched path ({weight_match:?})",
		);
	});
}

#[test]
fn proof_oversized_returns_none_and_refunds_gas() {
	new_test_ext().execute_with(|| {
		set_proof_result(ALICE_ALIAS, test_context(), *PEOPLE_IDENTIFIER);

		let result_match =
			call_proof_precompile(1, FULL_STATUS, ALICE_ALIAS, &test_context(), Vec::new());
		let result_oversized = call_proof_precompile(
			2,
			FULL_STATUS,
			ALICE_ALIAS,
			&test_context(),
			alloc::vec![0xAA; 1],
		);

		let weight_match = result_match.weight_consumed;
		let weight_oversized = result_oversized.weight_consumed;
		assert!(decode_proof_status(result_match));
		assert!(!decode_proof_status(result_oversized));

		assert!(
			weight_oversized.ref_time() < weight_match.ref_time(),
			"oversized-proof path ({weight_oversized:?}) should refund vs matched path ({weight_match:?})",
		);
	});
}
