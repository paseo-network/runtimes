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
use crate::mock::*;

use indiv_pallet_nft_claims::CollectionMinter;
use pallet_revive::{
	precompiles::{
		alloy::sol_types::{Revert, SolCall, SolError, SolInterface},
		AddressMapper,
	},
	sp_runtime::Weight,
	ExecConfig, TransactionLimits,
};
use sp_runtime::AccountId32;

/// Call the minter precompile with `input` and return the raw execution result.
fn call_precompile(caller: &AccountId32, input: Vec<u8>) -> pallet_revive::ExecReturnValue {
	pallet_revive::Pallet::<Test>::bare_call(
		RuntimeOrigin::signed(caller.clone()),
		minter_address(),
		0u32.into(),
		TransactionLimits::WeightAndDeposit { weight_limit: Weight::MAX, deposit_limit: u64::MAX },
		input,
		&ExecConfig::new_substrate_tx(),
	)
	.result
	.expect("precompile call should execute")
}

/// Call the minter precompile with `input`, attaching `value`, and return the raw execution
/// result.
fn call_with_value(
	caller: &AccountId32,
	input: Vec<u8>,
	value: u64,
) -> pallet_revive::ExecReturnValue {
	pallet_revive::Pallet::<Test>::bare_call(
		RuntimeOrigin::signed(caller.clone()),
		minter_address(),
		value.into(),
		TransactionLimits::WeightAndDeposit { weight_limit: Weight::MAX, deposit_limit: u64::MAX },
		input,
		&ExecConfig::new_substrate_tx(),
	)
	.result
	.expect("precompile call should execute")
}

/// The account the precompile address maps to, where stray value would land.
fn precompile_account() -> AccountId32 {
	<Test as pallet_revive::Config>::AddressMapper::to_fallback_account_id(&minter_address())
}

/// Call the minter precompile with `input`, expecting success, and return the output data.
fn call_ok(caller: &AccountId32, input: Vec<u8>) -> Vec<u8> {
	let output = call_precompile(caller, input);
	assert!(!output.did_revert(), "expected success, got revert: {output:?}");
	output.data
}

/// Call the minter precompile with `input`, expecting a revert whose reason contains `reason`.
fn call_reverted_with(caller: &AccountId32, input: Vec<u8>, reason: &str) {
	let output = call_precompile(caller, input);
	assert!(output.did_revert(), "expected revert, got success: {output:?}");
	let decoded = Revert::abi_decode(&output.data).expect("revert data decodes as Error(string)");
	assert!(
		decoded.reason.contains(reason),
		"revert reason {:?} does not contain {reason:?}",
		decoded.reason
	);
}

fn setup_collection(owner: &AccountId32) -> CollectionId {
	map_account(owner);
	pallet_scarcity::Pallet::<Test>::do_create_collection(owner.clone()).unwrap()
}

/// The registration `collectionMinter` reports for `collection`.
fn read_minter(collection: CollectionId) -> INftClaimsMinter::collectionMinterReturn {
	let reader = id_to_account(99);
	map_account(&reader);
	let data = call_ok(&reader, INftClaimsMinter::collectionMinterCall { collection }.abi_encode());
	INftClaimsMinter::collectionMinterCall::abi_decode_returns(&data).unwrap()
}

#[test]
fn set_random_minter_round_trips() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);

		call_ok(&alice, INftClaimsMinter::setRandomMinterCall { collection }.abi_encode());

		assert_eq!(
			CollectionMinters::<Test>::get(collection),
			Some(CollectionMinter { owner: alice.clone(), selection: ItemSelection::Random })
		);
		System::assert_has_event(RuntimeEvent::NftClaims(
			indiv_pallet_nft_claims::Event::CollectionMinterSet {
				collection,
				selection: Some(ItemSelection::Random),
			},
		));

		let minter = read_minter(collection);
		assert_eq!(minter.kind, KIND_RANDOM);
		assert_eq!(minter.minter, Address::ZERO);
		assert_eq!(minter.owner, address_of::<Test>(&alice));
	});
}

#[test]
fn set_contract_minter_round_trips() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let contract = H160([0xCC; 20]);

		call_ok(
			&alice,
			INftClaimsMinter::setContractMinterCall { collection, minter: contract.0.into() }
				.abi_encode(),
		);

		assert_eq!(
			CollectionMinters::<Test>::get(collection),
			Some(CollectionMinter {
				owner: alice.clone(),
				selection: ItemSelection::Contract(contract),
			})
		);

		let minter = read_minter(collection);
		assert_eq!(minter.kind, KIND_CONTRACT);
		assert_eq!(minter.minter.into_array(), contract.0);
		assert_eq!(minter.owner, address_of::<Test>(&alice));
	});
}

#[test]
fn clear_minter_round_trips() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);

		// A contract selection, so the cleared read has a non-zero minter to lose: cleared from
		// a random one it reports the zero address either way, and the assertion below would
		// only repeat what `collection_minter_answers_none_when_unregistered` already pins.
		let contract = H160([0xCC; 20]);
		call_ok(
			&alice,
			INftClaimsMinter::setContractMinterCall { collection, minter: contract.0.into() }
				.abi_encode(),
		);
		let before = read_minter(collection);
		assert_eq!(before.kind, KIND_CONTRACT);
		assert_eq!(before.minter.into_array(), contract.0);

		call_ok(&alice, INftClaimsMinter::clearMinterCall { collection }.abi_encode());

		assert_eq!(CollectionMinters::<Test>::get(collection), None);
		System::assert_has_event(RuntimeEvent::NftClaims(
			indiv_pallet_nft_claims::Event::CollectionMinterSet { collection, selection: None },
		));

		let minter = read_minter(collection);
		assert_eq!(minter.kind, KIND_NONE);
		assert_eq!(minter.minter, Address::ZERO);
		assert_eq!(minter.owner, Address::ZERO);
	});
}

#[test]
fn non_owner_cannot_register_or_withdraw() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let bob = id_to_account(2);
		let collection = setup_collection(&alice);
		map_account(&bob);

		for input in [
			INftClaimsMinter::setRandomMinterCall { collection }.abi_encode(),
			INftClaimsMinter::setContractMinterCall {
				collection,
				minter: H160([0xCC; 20]).0.into(),
			}
			.abi_encode(),
			INftClaimsMinter::clearMinterCall { collection }.abi_encode(),
		] {
			call_reverted_with(&bob, input, "caller is not the collection owner");
		}
		assert_eq!(CollectionMinters::<Test>::get(collection), None);
	});
}

#[test]
fn unknown_collection_reverts() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		map_account(&alice);

		call_reverted_with(
			&alice,
			INftClaimsMinter::setRandomMinterCall { collection: 7 }.abi_encode(),
			"unknown collection",
		);
	});
}

#[test]
fn rejected_contract_selection_reverts() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		MinterContractValid::set(&false);

		call_reverted_with(
			&alice,
			INftClaimsMinter::setContractMinterCall {
				collection,
				minter: H160([0xCC; 20]).0.into(),
			}
			.abi_encode(),
			"no contract code at the minter address",
		);
		assert_eq!(CollectionMinters::<Test>::get(collection), None);

		// The selector only checks contract selections, so random registration still works.
		call_ok(&alice, INftClaimsMinter::setRandomMinterCall { collection }.abi_encode());
	});
}

#[test]
fn collection_minter_answers_none_when_unregistered() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);

		// A collection that was never registered and an unknown one answer alike.
		for queried in [collection, 999] {
			let minter = read_minter(queried);
			assert_eq!(minter.kind, KIND_NONE);
			assert_eq!(minter.minter, Address::ZERO);
			assert_eq!(minter.owner, Address::ZERO);
		}
	});
}

/// Assert that calling the precompile with `input` and value attached is rejected and costs
/// nothing.
fn assert_rejects_value(caller: &AccountId32, input: Vec<u8>, method: &str) {
	let before = Balances::free_balance(caller);
	let output = call_with_value(caller, input, 1_000);
	assert!(output.did_revert(), "{method}: expected revert, got success: {output:?}");
	let decoded = Revert::abi_decode(&output.data).expect("revert data decodes as Error(string)");
	assert!(
		decoded.reason.contains("this precompile does not accept value"),
		"{method}: revert reason {:?}",
		decoded.reason
	);
	// The frame unwinds the transfer with the rest of its state changes.
	assert_eq!(Balances::free_balance(caller), before, "{method}: caller was charged");
	assert_eq!(
		Balances::free_balance(precompile_account()),
		0,
		"{method}: value stranded at the precompile"
	);
}

/// No function of the interface is payable, so every one of them must reject attached value.
///
/// The cases below are checked against the generated selector set, so a method added to
/// `INftClaimsMinter.sol` fails this test until it is covered here. Arguments are the ones
/// that would otherwise succeed, which is what makes each case prove the rejection wins over
/// the real path rather than over some other revert.
#[test]
fn every_method_rejects_attached_value() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let contract = H160([0xCC; 20]);

		let calls = alloc::vec![
			INftClaimsMinterCalls::setRandomMinter(INftClaimsMinter::setRandomMinterCall {
				collection,
			}),
			INftClaimsMinterCalls::setContractMinter(INftClaimsMinter::setContractMinterCall {
				collection,
				minter: contract.0.into(),
			}),
			INftClaimsMinterCalls::clearMinter(INftClaimsMinter::clearMinterCall { collection }),
			INftClaimsMinterCalls::collectionMinter(INftClaimsMinter::collectionMinterCall {
				collection,
			}),
		];

		// Exhaustiveness: every generated selector has a case above.
		let covered = calls.iter().map(|call| call.selector()).collect::<Vec<_>>();
		for selector in INftClaimsMinterCalls::selectors() {
			assert!(covered.contains(&selector), "no case for selector {selector:?}");
		}
		assert_eq!(covered.len(), INftClaimsMinterCalls::COUNT);

		for call in &calls {
			let method = alloc::format!("selector {:02x?}", call.selector());
			assert_rejects_value(&alice, call.abi_encode(), &method);
		}

		// None of the mutators above took effect.
		assert_eq!(CollectionMinters::<Test>::get(collection), None);
	});
}

/// Every pallet error variant the precompile can reach must map to a catchable revert.
///
/// The mapping in `revert_nft_claims` is a runtime list, so the compiler cannot flag a
/// variant added to `pallet-nft-claims` later. This test walks the variants from the error
/// type's own metadata and fails on any reachable one that starts trapping instead of
/// reverting.
#[test]
fn mapped_nft_claims_errors_are_exhaustive() {
	// The ABI covers only `set_collection_minter`; the claim and tree-delivery errors cannot
	// surface through it.
	const UNREACHABLE: [&str; 7] = [
		"UnknownAwardBlock",
		"LeafIndexOutOfBounds",
		"AlreadyClaimed",
		"InvalidProof",
		"CollectionNotRegistered",
		"CollectionOwnerChanged",
		"NoItems",
	];

	let pallet_index = match DispatchError::from(NftClaimsError::<Test>::NotCollectionOwner) {
		DispatchError::Module(module) => module.index,
		other => panic!("pallet errors are module errors, got {other:?}"),
	};
	let variants = match <NftClaimsError<Test> as scale_info::TypeInfo>::type_info().type_def {
		scale_info::TypeDef::Variant(def) => def.variants,
		other => panic!("pallet errors are a variant type, got {other:?}"),
	};
	assert!(!variants.is_empty(), "error metadata carries no variants");

	for variant in &variants {
		let error = DispatchError::Module(sp_runtime::ModuleError {
			index: pallet_index,
			error: [variant.index, 0, 0, 0],
			message: None,
		});
		let reverts = matches!(revert_nft_claims::<Test>(error), Error::Revert(_));
		let reachable = !UNREACHABLE.contains(&variant.name);
		assert_eq!(
			reverts,
			reachable,
			"{}: reverts={reverts}, but it is {} through this precompile. Map it in \
			 `revert_nft_claims`, or add it to UNREACHABLE if the ABI cannot reach it.",
			variant.name,
			if reachable { "reachable" } else { "unreachable" }
		);
	}

	for name in UNREACHABLE {
		assert!(
			variants.iter().any(|variant| variant.name == name),
			"UNREACHABLE lists {name}, which no longer exists in the pallet"
		);
	}
}

/// Frame-flag guards, driven through `pallet_revive::precompiles::run`, which executes a
/// precompile inside a frame with controlled read-only and delegate-call flags.
///
/// `pallet-revive` exports that harness only under `runtime-benchmarks`, and enabling the
/// feature unconditionally would grow the benchmark-only methods of the FRAME traits without
/// enabling the feature on the pallets that implement them, breaking any workspace-wide
/// build. The gate keeps it to feature-enabled runs, which is where CI exercises it.
#[cfg(feature = "runtime-benchmarks")]
mod guards {
	use super::*;
	use pallet_revive::precompiles::run::{
		precompile as run_precompile, CallSetup, VmBinaryModule,
	};

	fn read_call() -> INftClaimsMinterCalls {
		INftClaimsMinterCalls::collectionMinter(INftClaimsMinter::collectionMinterCall {
			collection: 0,
		})
	}

	fn mutating_call() -> INftClaimsMinterCalls {
		INftClaimsMinterCalls::setRandomMinter(INftClaimsMinter::setRandomMinterCall {
			collection: 0,
		})
	}

	fn assert_denied_with(result: Result<Vec<u8>, Error>, expected: pallet_revive::Error<Test>) {
		let expected: DispatchError = expected.into();
		match result {
			Err(Error::Error(e)) => assert_eq!(e.error, expected),
			other => panic!("expected {expected:?}, got {other:?}"),
		}
	}

	#[test]
	fn delegate_call_is_denied() {
		new_test_ext().execute_with(|| {
			let mut setup = CallSetup::<Test>::new(VmBinaryModule::dummy());
			setup.set_delegate_call(true);
			let (mut ext, _) = setup.ext();

			for input in [read_call(), mutating_call()] {
				let result = run_precompile::<NftClaimsMinter<Test, MINTER_INDEX>, _>(
					&mut ext,
					&minter_address().0,
					&input,
				);
				assert_denied_with(result, pallet_revive::Error::<Test>::PrecompileDelegateDenied);
			}
		});
	}

	#[test]
	fn read_only_frame_denies_mutations_and_serves_reads() {
		new_test_ext().execute_with(|| {
			let mut setup = CallSetup::<Test>::new(VmBinaryModule::dummy());
			setup.set_read_only(true);
			let (mut ext, _) = setup.ext();

			let mutation = run_precompile::<NftClaimsMinter<Test, MINTER_INDEX>, _>(
				&mut ext,
				&minter_address().0,
				&mutating_call(),
			);
			assert_denied_with(mutation, pallet_revive::Error::<Test>::StateChangeDenied);

			// Views keep answering inside a STATICCALL frame.
			let read = run_precompile::<NftClaimsMinter<Test, MINTER_INDEX>, _>(
				&mut ext,
				&minter_address().0,
				&read_call(),
			)
			.expect("reads must succeed in a read-only frame");
			let minter = INftClaimsMinter::collectionMinterCall::abi_decode_returns(&read).unwrap();
			assert_eq!(minter.kind, KIND_NONE);
		});
	}
}
