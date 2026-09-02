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

//! Tests for recovering a coin from an archived (deleted) recycler ring via
//! `unload_archived_recycler_into_external_asset`.

use crate::{
	extension::AsCoinage,
	mock::*,
	testing_utils::{to_bounded_proof, unloaded_root_and_non_inclusion_proof},
	*,
};
use codec::Encode;
use frame_support::{assert_noop, assert_ok};
use frame_system::AuthorizeCall;
use indiv_support::traits::{Alias, AppendOnlyMembers};
use sp_core::H256;
use sp_crypto_hashing::blake2_256;
use sp_runtime::{
	testing::UintAuthorityId,
	transaction_validity::{InvalidTransaction, TransactionSource, TransactionValidityError},
};

/// Outcome of setting up and cleaning a recycler ring so it is archived.
struct ArchivedSetup {
	value: Denomination,
	secrets: Vec<Secret>,
	ring_members: Vec<MemberOf<Test>>,
	recycler_root: MembersOf<Test>,
	/// The aliases unloaded before cleanup (the committed unloaded set).
	unloaded: Vec<Alias>,
	remaining: u32,
}

/// Build a recycler ring for denomination `0`, unload `num_unloaded` coins, then expire + clean it
/// so it is archived.
fn setup_archived_recycler(num_unloaded: usize) -> ArchivedSetup {
	setup_archived_recycler_with_value(0, num_unloaded)
}

/// Build a recycler ring for denomination `value`, unload `num_unloaded` coins, then expire + clean
/// it so it is archived. Captures the ring root and member list before cleanup removes the ring.
fn setup_archived_recycler_with_value(value: Denomination, num_unloaded: usize) -> ArchivedSetup {
	let ring_capacity = R2E10_RING_CAPACITY;

	let (secrets, _index, _revision) = setup_recycler(value, ring_capacity + 1, 0);
	for _ in 0..10 {
		Members::process_maintenance();
	}

	let identifier = Coinage::recycler_collection_identifier(TEST_INSTANCE_ID, value);
	let status = <Test as Config>::MemberService::ring_status(&identifier, 0).unwrap();
	assert_eq!(status.total, ring_capacity);
	let immutable_since = status.immutable_since.unwrap() as u32;
	let r0_revision = <Test as Config>::MemberService::ring_revision(&identifier, 0).unwrap();

	// Unload the first `num_unloaded` coins, marking them in `RecyclersUnloaded`.
	let unloaded: Vec<Alias> = secrets[..num_unloaded]
		.iter()
		.map(|s| {
			CryptoOf::<Test>::alias_in_context(
				s,
				crate::pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
			)
			.unwrap()
		})
		.collect();
	if num_unloaded > 0 {
		let call = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			aliases: unloaded.clone().try_into().unwrap(),
			value,
			index: 0,
			revision: r0_revision,
			to: 9999u64,
			max_fee: unload_token_fee_in_asset(),
		};
		let ext =
			build_unload_from_output_ext(call, value, 0, r0_revision, &secrets[..num_unloaded]);
		assert_eq!(Executive::apply_extrinsic(ext), Ok(Ok(())));
	}

	// Capture the ring-VRF root and members before cleanup removes the ring.
	let recycler_root = Coinage::recycler_ring_root(TEST_INSTANCE_ID, value, 0)
		.expect("ring 0 has a root before cleanup");
	let ring_members = Coinage::get_recycler_members(TEST_INSTANCE_ID, value, 0);

	// Expire the ring and trigger `clean_recycler` via the offchain worker, then a second block to
	// dust the removed ring's `RecyclersUnloaded` entries (keeps the accounting invariant clean).
	let expiration = get_u32::<<Test as crate::Config>::RecyclerExpirationTime>();
	advance_until_time(immutable_since + expiration);
	advance_block();
	advance_block();

	let remaining = ring_capacity - num_unloaded as u32;
	let archived =
		RecyclersArchives::<Test>::get((TEST_INSTANCE_ID, value, 0u32)).expect("archived");
	assert_eq!(archived.remaining, remaining);

	ArchivedSetup { value, secrets, ring_members, recycler_root, unloaded, remaining }
}

#[test]
fn recover_coin_from_archived_recycler_works() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let setup = setup_archived_recycler(5);
		let value = setup.value;

		// A not-yet-unloaded member (index 10) recovers their coin.
		let signer = ALICE;
		let to = 8888u64;
		fund_native(signer, 1_000_000);
		fund_native(FEE_DESTINATION, 1_000);

		let proven_msg = Coinage::unload_archived_proof_message(&signer);
		let (alias_proof, alias) =
			create_unload_proof(&setup.secrets[10], &setup.ring_members, &proven_msg);

		let (unloaded_root, proof_nodes) =
			unloaded_root_and_non_inclusion_proof(&setup.unloaded, &alias);

		let signer_native_before = Balances::free_balance(signer);
		let fee_dest_before = Balances::free_balance(FEE_DESTINATION);
		let to_asset_before = Assets::balance(TEST_ASSET_ID, to);
		let fee = Coinage::get_paid_unload_token_fee_in_native();

		assert_ok!(Coinage::unload_archived_recycler_into_external_asset(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			value,
			0,
			setup.recycler_root.clone(),
			unloaded_root,
			alias_proof,
			to_bounded_proof(proof_nodes),
			to,
			FeeCurrency::Native,
			native_max_fee_bound(),
		));

		// The full denomination was released to `to`.
		assert_eq!(Assets::balance(TEST_ASSET_ID, to) - to_asset_before, UNDERLYING_ASSET_UNIT);
		// The signer paid the unload-token price in native to the fee destination.
		assert_eq!(signer_native_before - Balances::free_balance(signer), fee);
		assert_eq!(Balances::free_balance(FEE_DESTINATION) - fee_dest_before, fee);
		// Nothing was destroyed.
		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(TEST_INSTANCE_ID), 0);

		// The archive's recoverable count decreased and its commitment was updated.
		let archived = RecyclersArchives::<Test>::get((TEST_INSTANCE_ID, value, 0u32)).unwrap();
		assert_eq!(archived.remaining, setup.remaining - 1);
		let expected_commitment = {
			let new_root = RecyclerManager::<Test>::unloaded_aliases_root(
				&[setup.unloaded.clone(), vec![alias]].concat(),
			)
			.unwrap();
			archive_commitment(new_root, &setup.recycler_root)
		};
		assert_eq!(archived.commitment, expected_commitment);

		check_accounting();

		System::assert_has_event(
			crate::Event::<Test>::ArchivedRecyclerUnloadedIntoExternalAsset {
				instance_id: TEST_INSTANCE_ID,
				who: signer,
				to,
				value,
				ring_index: 0,
				amount: UNDERLYING_ASSET_UNIT,
				fee_currency: FeeCurrency::Native,
				alias,
			}
			.into(),
		);
	});
}

// The unload-token fee is charged to the signer in the external asset (and the full denomination is
// released to `to`) when `FeeCurrency::ExternalAsset` is selected.
#[test]
fn recover_coin_charges_unload_token_fee_in_external_asset() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let setup = setup_archived_recycler(5);
		let value = setup.value;

		let signer = ALICE;
		let to = 8888u64;
		// Fund the signer with the external asset so it can pay the fee.
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), TEST_ASSET_ID, signer, 1_000));

		let proven_msg = Coinage::unload_archived_proof_message(&signer);
		let (alias_proof, alias) =
			create_unload_proof(&setup.secrets[10], &setup.ring_members, &proven_msg);
		let (unloaded_root, proof_nodes) =
			unloaded_root_and_non_inclusion_proof(&setup.unloaded, &alias);

		let fee = Coinage::get_paid_unload_token_fee_in_asset(TEST_INSTANCE_ID).unwrap();
		let signer_asset_before = Assets::balance(TEST_ASSET_ID, signer);
		let market_asset_before = Assets::balance(TEST_ASSET_ID, MOCK_MARKET);
		let to_asset_before = Assets::balance(TEST_ASSET_ID, to);

		assert_ok!(Coinage::unload_archived_recycler_into_external_asset(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			value,
			0,
			setup.recycler_root.clone(),
			unloaded_root,
			alias_proof,
			to_bounded_proof(proof_nodes),
			to,
			FeeCurrency::ExternalAsset,
			max_fee_bound(),
		));

		// The full denomination was released to `to`.
		assert_eq!(Assets::balance(TEST_ASSET_ID, to) - to_asset_before, UNDERLYING_ASSET_UNIT);
		// The signer's asset went to the market, which is what paying the fee costs it.
		assert_eq!(signer_asset_before - Assets::balance(TEST_ASSET_ID, signer), fee);
		assert_eq!(Assets::balance(TEST_ASSET_ID, MOCK_MARKET) - market_asset_before, fee);
		// Nothing was destroyed.
		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(TEST_INSTANCE_ID), 0);

		check_accounting();

		System::assert_has_event(
			crate::Event::<Test>::ArchivedRecyclerUnloadedIntoExternalAsset {
				instance_id: TEST_INSTANCE_ID,
				who: signer,
				to,
				value,
				ring_index: 0,
				amount: UNDERLYING_ASSET_UNIT,
				fee_currency: FeeCurrency::ExternalAsset,
				alias,
			}
			.into(),
		);
	});
}

// A fee that moved past `max_fee` after validation approved the call rejects it before the proofs
// are verified, and the caller is refunded down to what that exit cost rather than paying for the
// verification the call never did.
#[test]
fn recovery_with_fee_above_max_is_rejected_early_and_refunds_the_unused_weight() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let setup = setup_archived_recycler(5);
		let value = setup.value;

		let signer = ALICE;
		let to = 8888u64;
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), TEST_ASSET_ID, signer, 1_000));

		let proven_msg = Coinage::unload_archived_proof_message(&signer);
		let (alias_proof, alias) =
			create_unload_proof(&setup.secrets[10], &setup.ring_members, &proven_msg);
		let (unloaded_root, proof_nodes) =
			unloaded_root_and_non_inclusion_proof(&setup.unloaded, &alias);
		let max_fee = Coinage::get_paid_unload_token_fee_in_asset(TEST_INSTANCE_ID).unwrap() - 1;

		let call = crate::Call::<Test>::unload_archived_recycler_into_external_asset {
			instance_id: TEST_INSTANCE_ID,
			value,
			index: 0,
			recycler_root: setup.recycler_root.clone(),
			unloaded_root,
			alias_proof: alias_proof.clone(),
			non_inclusion_proof: to_bounded_proof(proof_nodes.clone()),
			to,
			fee_currency: FeeCurrency::ExternalAsset,
			max_fee,
		};
		let charged =
			frame_support::dispatch::GetDispatchInfo::get_dispatch_info(&call).call_weight;

		let err = Coinage::unload_archived_recycler_into_external_asset(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			value,
			0,
			setup.recycler_root.clone(),
			unloaded_root,
			alias_proof,
			to_bounded_proof(proof_nodes),
			to,
			FeeCurrency::ExternalAsset,
			max_fee,
		)
		.expect_err("the fee bound rejects the call");

		assert_eq!(err.error, Error::<Test>::FeeExceedsMaxFee.into());
		let refunded = err.post_info.actual_weight.expect("the early exit refunds");
		assert_eq!(
			refunded,
			<() as crate::WeightInfo>::unload_archived_recycler_into_external_asset_fee_fail()
		);
		// `ref_time` is as much as the recorded weights let this assert: the two-dimensional
		// invariant is `fee_fail_exit_costs_less_than_the_call_refunding_to_it`, which the call's
		// stale proof size currently blocks.
		assert!(
			refunded.ref_time() < charged.ref_time(),
			"the early exit must cost less than the charged worst case: {refunded:?} vs {charged:?}"
		);
		// Nothing was recovered: the archive still counts the coin as recoverable.
		let archived = RecyclersArchives::<Test>::get((TEST_INSTANCE_ID, value, 0u32)).unwrap();
		assert_eq!(archived.remaining, setup.remaining);
	});
}

// The best effort validation checks for `max_fee` during validation.
#[test]
fn recovery_with_fee_above_max_is_invalid() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let setup = setup_archived_recycler(5);
		let value = setup.value;

		let signer = ALICE;
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), TEST_ASSET_ID, signer, 1_000));

		let proven_msg = Coinage::unload_archived_proof_message(&signer);
		let (alias_proof, alias) =
			create_unload_proof(&setup.secrets[10], &setup.ring_members, &proven_msg);
		let (unloaded_root, proof_nodes) =
			unloaded_root_and_non_inclusion_proof(&setup.unloaded, &alias);
		let max_fee = Coinage::get_paid_unload_token_fee_in_asset(TEST_INSTANCE_ID).unwrap() - 1;

		let ext = unload_archived_ext_with_fee(
			signer,
			value,
			setup.recycler_root.clone(),
			unloaded_root,
			alias_proof,
			proof_nodes,
			FeeCurrency::ExternalAsset,
			max_fee,
		);
		assert_invalid(ext, CustomInvalidity::MaxFeeInsufficientForUnload);

		// Nothing was recovered: the archive still counts the coin as recoverable.
		let archived = RecyclersArchives::<Test>::get((TEST_INSTANCE_ID, value, 0u32)).unwrap();
		assert_eq!(archived.remaining, setup.remaining);
	});
}

// `max_fee` bounds the fee in whichever currency pays it: paying in native is not a conversion, but
// its price is not fixed either, since it follows `WeightToFee`. A bound below the native fee is
// rejected while validating, and, for a call that skipped validation, before any proof is verified.
#[test]
fn recovery_paying_in_native_respects_max_fee() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let setup = setup_archived_recycler(5);
		let value = setup.value;

		let signer = ALICE;
		let to = 8888u64;
		fund_native(signer, 1_000_000);
		fund_native(FEE_DESTINATION, 1_000);

		let proven_msg = Coinage::unload_archived_proof_message(&signer);
		let (alias_proof, alias) =
			create_unload_proof(&setup.secrets[10], &setup.ring_members, &proven_msg);
		let (unloaded_root, proof_nodes) =
			unloaded_root_and_non_inclusion_proof(&setup.unloaded, &alias);

		let fee_native = Coinage::get_paid_unload_token_fee_in_native();
		let signer_native_before = Balances::free_balance(signer);

		// One short of the fee: the transaction never reaches a block.
		let ext = unload_archived_ext_with_fee(
			signer,
			value,
			setup.recycler_root.clone(),
			unloaded_root,
			alias_proof.clone(),
			proof_nodes.clone(),
			FeeCurrency::Native,
			fee_native - 1,
		);
		assert_invalid(ext, CustomInvalidity::MaxFeeInsufficientForUnload);

		// The same bound in a dispatch that skipped validation exits early, refunded down to what
		// that exit cost, with nothing recovered.
		let err = Coinage::unload_archived_recycler_into_external_asset(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			value,
			0,
			setup.recycler_root.clone(),
			unloaded_root,
			alias_proof.clone(),
			to_bounded_proof(proof_nodes.clone()),
			to,
			FeeCurrency::Native,
			fee_native - 1,
		)
		.expect_err("the fee bound rejects the call");
		assert_eq!(err.error, Error::<Test>::FeeExceedsMaxFee.into());
		assert_eq!(
			err.post_info.actual_weight.expect("the early exit refunds"),
			<() as crate::WeightInfo>::unload_archived_recycler_into_external_asset_fee_fail()
		);
		assert_eq!(Balances::free_balance(signer), signer_native_before);
		let archived = RecyclersArchives::<Test>::get((TEST_INSTANCE_ID, value, 0u32)).unwrap();
		assert_eq!(archived.remaining, setup.remaining);

		// The fee itself is bound enough, and it is what the signer pays.
		assert_ok!(Coinage::unload_archived_recycler_into_external_asset(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			value,
			0,
			setup.recycler_root.clone(),
			unloaded_root,
			alias_proof,
			to_bounded_proof(proof_nodes),
			to,
			FeeCurrency::Native,
			fee_native
		));

		assert_eq!(signer_native_before - Balances::free_balance(signer), fee_native);
	});
}

// The fee is charged before the recovered value is released, so a destination that moves the
// market cannot push the conversion past the quote bounding it.
#[test]
fn recover_coin_charges_the_fee_before_releasing_the_value_to_the_market() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let setup = setup_archived_recycler(5);
		let value = setup.value;

		let signer = ALICE;
		// The market itself is the destination. A real market's pool account is derived
		// deterministically, so anything routing the output into it lands here.
		let to = MOCK_MARKET;
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), TEST_ASSET_ID, signer, 1_000_000));
		// With the market pricing the asset against its own reserve, releasing the value first
		// would make the fee cost more than the quote allows for.
		set_fee_conversion_reserve_pricing(true);

		let proven_msg = Coinage::unload_archived_proof_message(&signer);
		let (alias_proof, alias) =
			create_unload_proof(&setup.secrets[10], &setup.ring_members, &proven_msg);
		let (unloaded_root, proof_nodes) =
			unloaded_root_and_non_inclusion_proof(&setup.unloaded, &alias);

		let fee = Coinage::get_paid_unload_token_fee_in_asset(TEST_INSTANCE_ID).unwrap();
		let signer_asset_before = Assets::balance(TEST_ASSET_ID, signer);
		let market_asset_before = Assets::balance(TEST_ASSET_ID, MOCK_MARKET);

		assert_ok!(Coinage::unload_archived_recycler_into_external_asset(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			value,
			0,
			setup.recycler_root.clone(),
			unloaded_root,
			alias_proof,
			to_bounded_proof(proof_nodes),
			to,
			FeeCurrency::ExternalAsset,
			max_fee_bound()
		));

		// The signer paid exactly what was quoted before the call, and the market received that
		// fee on top of the recovered value.
		assert_eq!(signer_asset_before - Assets::balance(TEST_ASSET_ID, signer), fee);
		assert_eq!(
			Assets::balance(TEST_ASSET_ID, MOCK_MARKET) - market_asset_before,
			fee + UNDERLYING_ASSET_UNIT
		);
		check_accounting();
	});
}

// Recovery works for a different denomination: the released amount is scaled by the value's
// exponent and the archive is keyed by that value (not hardcoded to value 0).
#[test]
fn recover_coin_from_archived_recycler_with_different_value_works() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		// Denomination 3 (exponent) → 2^3 = 8x the base unit.
		let value: Denomination = 3;
		let setup = setup_archived_recycler_with_value(value, 5);
		assert_eq!(setup.value, value);

		let signer = ALICE;
		let to = 8888u64;
		fund_native(signer, 1_000_000);
		fund_native(FEE_DESTINATION, 1_000);

		let proven_msg = Coinage::unload_archived_proof_message(&signer);
		let (alias_proof, alias) =
			create_unload_proof(&setup.secrets[10], &setup.ring_members, &proven_msg);
		let (unloaded_root, proof_nodes) =
			unloaded_root_and_non_inclusion_proof(&setup.unloaded, &alias);

		let expected_amount =
			Coinage::denomination_to_asset_amount(UNDERLYING_ASSET_UNIT, value).unwrap();
		assert_eq!(expected_amount, UNDERLYING_ASSET_UNIT * 8);
		let to_asset_before = Assets::balance(TEST_ASSET_ID, to);

		assert_ok!(Coinage::unload_archived_recycler_into_external_asset(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			value,
			0,
			setup.recycler_root.clone(),
			unloaded_root,
			alias_proof,
			to_bounded_proof(proof_nodes),
			to,
			FeeCurrency::Native,
			native_max_fee_bound(),
		));

		// The value-scaled coin amount (not the base unit) was released to `to`.
		assert_eq!(Assets::balance(TEST_ASSET_ID, to) - to_asset_before, expected_amount);

		// The archive for this value was updated, and no value-0 archive was ever created.
		let archived = RecyclersArchives::<Test>::get((TEST_INSTANCE_ID, value, 0u32)).unwrap();
		assert_eq!(archived.remaining, setup.remaining - 1);
		let expected_commitment = {
			let new_root = RecyclerManager::<Test>::unloaded_aliases_root(
				&[setup.unloaded.clone(), vec![alias]].concat(),
			)
			.unwrap();
			archive_commitment(new_root, &setup.recycler_root)
		};
		assert_eq!(archived.commitment, expected_commitment);
		assert!(RecyclersArchives::<Test>::get((TEST_INSTANCE_ID, 0i8, 0u32)).is_none());

		assert_eq!(TotalValueOfDestroyedCoins::<Test>::get(TEST_INSTANCE_ID), 0);
		check_accounting();

		System::assert_has_event(
			crate::Event::<Test>::ArchivedRecyclerUnloadedIntoExternalAsset {
				instance_id: TEST_INSTANCE_ID,
				who: signer,
				to,
				value,
				ring_index: 0,
				amount: expected_amount,
				fee_currency: FeeCurrency::Native,
				alias,
			}
			.into(),
		);
	});
}

// Test for a recovery against a *large, dense* unloaded-aliases trie.
//
// The other recovery tests only commit to a handful of unloaded aliases, which yields a shallow
// trie where the alias's insert path happens to coincide with its lookup path. That can mask a real
// bug: on a large/dense trie the two paths diverge, so a minimal non-inclusion proof is not
// sufficient for the on-chain `delta_trie_root` re-insert (`IncompleteDatabase`), while a recorded
// insert proof is rejected by the strict `verify_trie_proof` (`ExtraneousHashReference`).
#[test]
fn recover_coin_from_archived_recycler_with_large_unloaded_set_works() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		// Reuse the ring/member setup only for a valid recovering member and ring root; we replace
		// the committed unloaded set below with a large one.
		let setup = setup_archived_recycler(0);
		let value = setup.value;

		let signer = ALICE;
		let to = 8888u64;
		fund_native(signer, 1_000_000);
		fund_native(FEE_DESTINATION, 1_000);

		let proven_msg = Coinage::unload_archived_proof_message(&signer);
		let (alias_proof, alias) =
			create_unload_proof(&setup.secrets[10], &setup.ring_members, &proven_msg);

		// A large, dense committed unloaded set (dummy aliases, distinct from the recovering
		// member's real alias) so the trie has real branching depth.
		let large_unloaded: Vec<Alias> = (0u32..2000)
			.map(|i| blake2_256(&(b"dummy-unloaded-alias", i).encode()))
			.collect();
		assert!(!large_unloaded.contains(&alias), "recovering alias must be absent from the set");

		// Re-archive the ring committing to the large unloaded set.
		let unloaded_root =
			RecyclerManager::<Test>::unloaded_aliases_root(&large_unloaded).unwrap();
		let commitment = archive_commitment(unloaded_root, &setup.recycler_root);
		let remaining = 5u32;
		RecyclersArchives::<Test>::insert(
			(TEST_INSTANCE_ID, value, 0u32),
			ArchivedRecycler { commitment, remaining },
		);

		// Proof generated the same way a real caller would (via the shared helper).
		let (proof_root, proof_nodes) =
			unloaded_root_and_non_inclusion_proof(&large_unloaded, &alias);
		assert_eq!(proof_root, unloaded_root);

		let to_asset_before = Assets::balance(TEST_ASSET_ID, to);

		assert_ok!(Coinage::unload_archived_recycler_into_external_asset(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			value,
			0,
			setup.recycler_root.clone(),
			unloaded_root,
			alias_proof,
			to_bounded_proof(proof_nodes),
			to,
			FeeCurrency::Native,
			native_max_fee_bound(),
		));

		// The coin was released and the commitment advanced to include the recovered alias.
		assert_eq!(Assets::balance(TEST_ASSET_ID, to) - to_asset_before, UNDERLYING_ASSET_UNIT);
		let archived = RecyclersArchives::<Test>::get((TEST_INSTANCE_ID, value, 0u32)).unwrap();
		assert_eq!(archived.remaining, remaining - 1);
		let expected_commitment = {
			let new_root = RecyclerManager::<Test>::unloaded_aliases_root(
				&[large_unloaded, vec![alias]].concat(),
			)
			.unwrap();
			archive_commitment(new_root, &setup.recycler_root)
		};
		assert_eq!(archived.commitment, expected_commitment);
	});
}

#[test]
fn double_recovery_is_rejected() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let setup = setup_archived_recycler(5);
		let value = setup.value;
		let signer = ALICE;
		let to = 8888u64;
		fund_native(signer, 1_000_000);
		fund_native(FEE_DESTINATION, 1_000);

		let proven_msg = Coinage::unload_archived_proof_message(&signer);
		let (alias_proof, alias) =
			create_unload_proof(&setup.secrets[10], &setup.ring_members, &proven_msg);
		let (unloaded_root, proof_nodes) =
			unloaded_root_and_non_inclusion_proof(&setup.unloaded, &alias);

		assert_ok!(Coinage::unload_archived_recycler_into_external_asset(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			value,
			0,
			setup.recycler_root.clone(),
			unloaded_root,
			alias_proof.clone(),
			to_bounded_proof(proof_nodes.clone()),
			to,
			FeeCurrency::Native,
			native_max_fee_bound(),
		));

		// Replaying against the now-stale `unloaded_root` fails the commitment check, because the
		// recovered alias is now part of the committed unloaded set.
		assert_noop!(
			Coinage::unload_archived_recycler_into_external_asset(
				RuntimeOrigin::signed(signer),
				TEST_INSTANCE_ID,
				value,
				0,
				setup.recycler_root.clone(),
				unloaded_root,
				alias_proof,
				to_bounded_proof(proof_nodes),
				to,
				FeeCurrency::Native,
				native_max_fee_bound(),
			),
			Error::<Test>::InvalidArchivedRoots,
		);
	});
}

#[test]
fn tampered_roots_are_rejected() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let setup = setup_archived_recycler(5);
		let value = setup.value;
		let signer = ALICE;
		let to = 8888u64;
		fund_native(signer, 1_000_000);

		let proven_msg = Coinage::unload_archived_proof_message(&signer);
		let (alias_proof, alias) =
			create_unload_proof(&setup.secrets[10], &setup.ring_members, &proven_msg);
		let (_unloaded_root, proof_nodes) =
			unloaded_root_and_non_inclusion_proof(&setup.unloaded, &alias);

		// A wrong unloaded_root breaks the commitment binding.
		assert_noop!(
			Coinage::unload_archived_recycler_into_external_asset(
				RuntimeOrigin::signed(signer),
				TEST_INSTANCE_ID,
				value,
				0,
				setup.recycler_root.clone(),
				H256::repeat_byte(0xAB),
				alias_proof,
				to_bounded_proof(proof_nodes),
				to,
				FeeCurrency::Native,
				native_max_fee_bound(),
			),
			Error::<Test>::InvalidArchivedRoots,
		);
	});
}

#[test]
fn unknown_archive_is_rejected() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let setup = setup_archived_recycler(5);
		let value = setup.value;
		let signer = ALICE;
		let to = 8888u64;
		fund_native(signer, 1_000_000);

		let proven_msg = Coinage::unload_archived_proof_message(&signer);
		let (alias_proof, alias) =
			create_unload_proof(&setup.secrets[10], &setup.ring_members, &proven_msg);
		let (unloaded_root, proof_nodes) =
			unloaded_root_and_non_inclusion_proof(&setup.unloaded, &alias);

		// Ring index 7 was never archived.
		assert_noop!(
			Coinage::unload_archived_recycler_into_external_asset(
				RuntimeOrigin::signed(signer),
				TEST_INSTANCE_ID,
				value,
				7,
				setup.recycler_root.clone(),
				unloaded_root,
				alias_proof,
				to_bounded_proof(proof_nodes),
				to,
				FeeCurrency::Native,
				native_max_fee_bound(),
			),
			Error::<Test>::ArchivedRecyclerNotFound,
		);
	});
}

#[test]
fn recovering_last_coin_removes_archive() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let setup = setup_archived_recycler(5);
		let value = setup.value;
		let signer = ALICE;
		let to = 8888u64;
		fund_native(signer, 1_000_000);
		fund_native(FEE_DESTINATION, 1_000);

		// Force the archive down to a single recoverable coin so this recovery drains it.
		RecyclersArchives::<Test>::mutate((TEST_INSTANCE_ID, value, 0u32), |maybe| {
			maybe.as_mut().unwrap().remaining = 1;
		});

		let proven_msg = Coinage::unload_archived_proof_message(&signer);
		let (alias_proof, alias) =
			create_unload_proof(&setup.secrets[10], &setup.ring_members, &proven_msg);
		let (unloaded_root, proof_nodes) =
			unloaded_root_and_non_inclusion_proof(&setup.unloaded, &alias);

		assert_ok!(Coinage::unload_archived_recycler_into_external_asset(
			RuntimeOrigin::signed(signer),
			TEST_INSTANCE_ID,
			value,
			0,
			setup.recycler_root.clone(),
			unloaded_root,
			alias_proof,
			to_bounded_proof(proof_nodes),
			to,
			FeeCurrency::Native,
			native_max_fee_bound(),
		));

		// The archive entry is removed once fully drained.
		assert!(RecyclersArchives::<Test>::get((TEST_INSTANCE_ID, value, 0u32)).is_none());
	});
}

// Recover several coins from the same archived recycler in sequence. Each recovery advances the
// committed unloaded set, so every call must supply a *different* unloaded root and a freshly
// generated non-inclusion proof against the previous recovery's updated commitment.
#[test]
fn multiple_recoveries_from_same_archive_works() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		// 5 coins were unloaded before the ring was archived; we now recover 4 more.
		let setup = setup_archived_recycler(5);
		let value = setup.value;
		let signer = ALICE;
		let to = 8888u64;
		fund_native(signer, 1_000_000);
		fund_native(FEE_DESTINATION, 1_000);

		// The committed unloaded set grows by one alias per recovery, so every recovery commits to
		// a different root and produces a distinct non-inclusion proof.
		let mut unloaded = setup.unloaded.clone();
		let recovering_members = [10usize, 11, 12, 13];

		for &member in recovering_members.iter() {
			let remaining_before = RecyclersArchives::<Test>::get((TEST_INSTANCE_ID, value, 0u32))
				.unwrap()
				.remaining;

			let proven_msg = Coinage::unload_archived_proof_message(&signer);
			let (alias_proof, alias) =
				create_unload_proof(&setup.secrets[member], &setup.ring_members, &proven_msg);

			// Roots/proof are computed against the current (growing) committed unloaded set.
			let (unloaded_root, proof_nodes) =
				unloaded_root_and_non_inclusion_proof(&unloaded, &alias);

			let to_asset_before = Assets::balance(TEST_ASSET_ID, to);

			assert_ok!(Coinage::unload_archived_recycler_into_external_asset(
				RuntimeOrigin::signed(signer),
				TEST_INSTANCE_ID,
				value,
				0,
				setup.recycler_root.clone(),
				unloaded_root,
				alias_proof,
				to_bounded_proof(proof_nodes),
				to,
				FeeCurrency::Native,
				native_max_fee_bound(),
			));

			// Each recovery released a full denomination to `to`.
			assert_eq!(Assets::balance(TEST_ASSET_ID, to) - to_asset_before, UNDERLYING_ASSET_UNIT);

			// The recovered alias joins the committed unloaded set for the next iteration.
			unloaded.push(alias);

			// The archive's count decreased and its commitment advanced to include the freshly
			// recovered alias.
			let archived = RecyclersArchives::<Test>::get((TEST_INSTANCE_ID, value, 0u32)).unwrap();
			assert_eq!(archived.remaining, remaining_before - 1);
			let expected_commitment = {
				let new_root = RecyclerManager::<Test>::unloaded_aliases_root(&unloaded).unwrap();
				archive_commitment(new_root, &setup.recycler_root)
			};
			assert_eq!(archived.commitment, expected_commitment);

			check_accounting();
		}
	});
}

#[test]
fn recovery_of_pre_archival_unloaded_alias_is_rejected() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let setup = setup_archived_recycler(5);
		let value = setup.value;
		let signer = ALICE;
		let to = 8888u64;
		fund_native(signer, 1_000_000);
		fund_native(FEE_DESTINATION, 1_000);

		// Member 0 already unloaded their coin before the ring was cleaned, so their alias is in
		// the committed unloaded set. They can still produce a valid ring membership proof and
		// they supply the *correct* current roots (passing the commitment check), so the trie
		// non-inclusion check is the only thing standing between them and a second payout.
		let proven_msg = Coinage::unload_archived_proof_message(&signer);
		let (alias_proof, alias) =
			create_unload_proof(&setup.secrets[0], &setup.ring_members, &proven_msg);
		assert!(setup.unloaded.contains(&alias), "alias must be in the committed unloaded set");

		// The proof helper records the insert path of `alias`, which for an already-included key
		// covers its lookup path: the on-chain lookup finds the alias and must reject.
		let (unloaded_root, proof_nodes) =
			unloaded_root_and_non_inclusion_proof(&setup.unloaded, &alias);

		assert_noop!(
			Coinage::unload_archived_recycler_into_external_asset(
				RuntimeOrigin::signed(signer),
				TEST_INSTANCE_ID,
				value,
				0,
				setup.recycler_root.clone(),
				unloaded_root,
				alias_proof,
				to_bounded_proof(proof_nodes),
				to,
				FeeCurrency::Native,
				native_max_fee_bound(),
			),
			Error::<Test>::AliasWasUnloadedOrInvalidProof,
		);
	});
}

/// Build a signed extrinsic for `unload_archived_recycler_into_external_asset` on ring index 0,
/// going through the full transaction validation pipeline (`AsCoinage` extension with `None`).
fn unload_archived_ext(
	signer: u64,
	value: Denomination,
	recycler_root: MembersOf<Test>,
	unloaded_root: H256,
	alias_proof: Proof,
	proof_nodes: Vec<Vec<u8>>,
) -> Extrinsic {
	unload_archived_ext_with_fee(
		signer,
		value,
		recycler_root,
		unloaded_root,
		alias_proof,
		proof_nodes,
		FeeCurrency::Native,
		native_max_fee_bound(),
	)
}

/// [`unload_archived_ext`] paying the fee in the given currency, bounded by `max_fee`.
#[allow(clippy::too_many_arguments)]
fn unload_archived_ext_with_fee(
	signer: u64,
	value: Denomination,
	recycler_root: MembersOf<Test>,
	unloaded_root: H256,
	alias_proof: Proof,
	proof_nodes: Vec<Vec<u8>>,
	fee_currency: FeeCurrency,
	max_fee: u64,
) -> Extrinsic {
	let call = crate::Call::<Test>::unload_archived_recycler_into_external_asset {
		instance_id: TEST_INSTANCE_ID,
		value,
		index: 0,
		recycler_root,
		unloaded_root,
		alias_proof,
		non_inclusion_proof: to_bounded_proof(proof_nodes),
		to: 8888u64,
		fee_currency,
		max_fee,
	};
	let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(None));
	Extrinsic::new_signed(RuntimeCall::Coinage(call), signer, UintAuthorityId(signer), extension)
}

// Two transactions competing to unload from the same archive state: the first is included, the
// second becomes stale at validation and is dropped from the pool without paying fees, instead of
// being included and failing.
#[test]
fn competing_unload_becomes_stale_instead_of_failing() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let setup = setup_archived_recycler(5);
		let value = setup.value;
		let signer = ALICE;
		fund_native(signer, 1_000_000);
		fund_native(FEE_DESTINATION, 1_000);

		let proven_msg = Coinage::unload_archived_proof_message(&signer);

		let (proof_a, alias_a) =
			create_unload_proof(&setup.secrets[10], &setup.ring_members, &proven_msg);
		let (root_a, nodes_a) = unloaded_root_and_non_inclusion_proof(&setup.unloaded, &alias_a);
		let tx_a = unload_archived_ext(
			signer,
			value,
			setup.recycler_root.clone(),
			root_a,
			proof_a,
			nodes_a,
		);

		let (proof_b, alias_b) =
			create_unload_proof(&setup.secrets[11], &setup.ring_members, &proven_msg);
		let (root_b, nodes_b) = unloaded_root_and_non_inclusion_proof(&setup.unloaded, &alias_b);
		let tx_b = unload_archived_ext(
			signer,
			value,
			setup.recycler_root.clone(),
			root_b,
			proof_b,
			nodes_b,
		);

		// Both transactions are valid against the current archive state.
		assert!(Executive::validate_transaction(
			TransactionSource::External,
			tx_a.clone(),
			Default::default()
		)
		.is_ok());
		assert!(Executive::validate_transaction(
			TransactionSource::External,
			tx_b.clone(),
			Default::default()
		)
		.is_ok());

		// The first competing transaction is included.
		assert_eq!(Executive::apply_extrinsic(tx_a), Ok(Ok(())));

		// The second's roots no longer match the updated commitment: it is stale, both in the pool
		// and at inclusion, so no fee is paid.
		assert_eq!(
			Executive::validate_transaction(
				TransactionSource::External,
				tx_b.clone(),
				Default::default()
			),
			Err(TransactionValidityError::Invalid(InvalidTransaction::Stale))
		);
		assert_eq!(
			Executive::apply_extrinsic(tx_b),
			Err(TransactionValidityError::Invalid(InvalidTransaction::Stale))
		);
	});
}

// A transaction built against the archive state anticipated after another pending unload is
// stale until that unload is included, then becomes valid.
#[test]
fn unload_built_against_anticipated_archive_state_becomes_valid() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let setup = setup_archived_recycler(5);
		let value = setup.value;
		let signer = ALICE;
		fund_native(signer, 1_000_000);
		fund_native(FEE_DESTINATION, 1_000);

		let proven_msg = Coinage::unload_archived_proof_message(&signer);

		let (proof_a, alias_a) =
			create_unload_proof(&setup.secrets[10], &setup.ring_members, &proven_msg);
		let (root_a, nodes_a) = unloaded_root_and_non_inclusion_proof(&setup.unloaded, &alias_a);
		let tx_a = unload_archived_ext(
			signer,
			value,
			setup.recycler_root.clone(),
			root_a,
			proof_a,
			nodes_a,
		);

		// A second transaction built against the archive state expected after `tx_a` is included:
		// its unloaded set contains `alias_a`.
		let (proof_b, alias_b) =
			create_unload_proof(&setup.secrets[11], &setup.ring_members, &proven_msg);
		let unloaded_after_a = [setup.unloaded.clone(), vec![alias_a]].concat();
		let (root_b, nodes_b) = unloaded_root_and_non_inclusion_proof(&unloaded_after_a, &alias_b);
		let tx_b = unload_archived_ext(
			signer,
			value,
			setup.recycler_root.clone(),
			root_b,
			proof_b,
			nodes_b,
		);

		// Until `tx_a` is included, `tx_b`'s roots do not match the stored commitment: it is
		// rejected as stale.
		assert_eq!(
			Executive::validate_transaction(
				TransactionSource::External,
				tx_b.clone(),
				Default::default()
			),
			Err(TransactionValidityError::Invalid(InvalidTransaction::Stale))
		);

		assert_eq!(Executive::apply_extrinsic(tx_a), Ok(Ok(())));

		// The archive reached the anticipated state, so `tx_b` is now valid and applies.
		assert!(Executive::validate_transaction(
			TransactionSource::External,
			tx_b.clone(),
			Default::default()
		)
		.is_ok());
		assert_eq!(Executive::apply_extrinsic(tx_b), Ok(Ok(())));
	});
}

// A transaction referencing a missing archive (drained and removed, or never created) is stale.
#[test]
fn unload_of_missing_archive_is_stale() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let setup = setup_archived_recycler(5);
		let value = setup.value;
		let signer = ALICE;
		fund_native(signer, 1_000_000);

		let proven_msg = Coinage::unload_archived_proof_message(&signer);
		let (alias_proof, alias) =
			create_unload_proof(&setup.secrets[10], &setup.ring_members, &proven_msg);
		let (unloaded_root, proof_nodes) =
			unloaded_root_and_non_inclusion_proof(&setup.unloaded, &alias);
		let ext = unload_archived_ext(
			signer,
			value,
			setup.recycler_root.clone(),
			unloaded_root,
			alias_proof,
			proof_nodes,
		);

		// Simulate the archive being drained and removed.
		RecyclersArchives::<Test>::remove((TEST_INSTANCE_ID, value, 0u32));

		assert_eq!(
			Executive::validate_transaction(
				TransactionSource::External,
				ext.clone(),
				Default::default()
			),
			Err(TransactionValidityError::Invalid(InvalidTransaction::Stale))
		);
		assert_eq!(
			Executive::apply_extrinsic(ext),
			Err(TransactionValidityError::Invalid(InvalidTransaction::Stale))
		);
	});
}

// The membership proof is bound to the signer; another account cannot replay it
// (anti-front-running), surfacing as an invalid alias proof.
#[test]
fn recovery_proof_bound_to_signer_rejects_other_signer() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let setup = setup_archived_recycler(5);
		let value = setup.value;
		let to = 8888u64;
		fund_native(BOB, 1_000_000);
		fund_native(FEE_DESTINATION, 1_000);

		// Proof bound to ALICE as the signer.
		let proven_msg = Coinage::unload_archived_proof_message(&ALICE);
		let (alias_proof, alias) =
			create_unload_proof(&setup.secrets[10], &setup.ring_members, &proven_msg);
		let (unloaded_root, proof_nodes) =
			unloaded_root_and_non_inclusion_proof(&setup.unloaded, &alias);

		// BOB replaying ALICE's proof recomputes a different `proven_msg`.
		assert_noop!(
			Coinage::unload_archived_recycler_into_external_asset(
				RuntimeOrigin::signed(BOB),
				TEST_INSTANCE_ID,
				value,
				0,
				setup.recycler_root.clone(),
				unloaded_root,
				alias_proof,
				to_bounded_proof(proof_nodes),
				to,
				FeeCurrency::Native,
				native_max_fee_bound(),
			),
			Error::<Test>::InvalidAliasProof
		);
	});
}
