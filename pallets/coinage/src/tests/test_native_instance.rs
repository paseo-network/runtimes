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

//! Tests for an instance whose coins wrap the native token.
//!
//! [`Config::Fungibles`] covers the native token alongside the chain's assets, so nothing
//! distinguishes the native token as an underlying asset: an instance wraps it like any other,
//! and the same currency can back the coins and collateralize a sponsored instance's loads.

use crate::{mock::*, *};
use codec::Encode;
use frame_support::{
	assert_noop, assert_ok,
	traits::{
		fungibles::{Inspect, InspectHold},
		Currency,
	},
};
use sp_crypto_hashing::blake2_256;
use sp_runtime::bounded_vec;

/// The load deposit price used across these tests.
const PRICE: u64 = 10;

/// The buffer `create_sufficient_instance` expects on the pallet account for the native token.
fn existential_deposit() -> u64 {
	<Balances as Currency<u64>>::minimum_balance()
}

/// The native balance of `who`, free and held together.
fn native_balance(who: u64) -> u64 {
	<NativeAndAssets as Inspect<_>>::total_balance(NATIVE_DEPOSIT_ID, &who)
}

/// The native balance of `who` held as the backing of loaded coins.
fn native_wrapped(who: u64) -> u64 {
	<NativeAndAssets as InspectHold<_>>::balance_on_hold(
		NATIVE_DEPOSIT_ID,
		&HoldReason::Wrapped.into(),
		&who,
	)
}

/// Create the sufficient instance wrapping the native token, buffer included.
fn create_native_instance() -> InstanceId {
	fund_native(Coinage::pallet_account(), existential_deposit());
	let instance_id = NextInstanceId::<Test>::get();
	assert_ok!(Coinage::create_sufficient_instance(
		RuntimeOrigin::root(),
		NATIVE_DEPOSIT_ID,
		UNDERLYING_ASSET_UNIT
	));
	instance_id
}

/// Unload `secret`'s key of the native instance to `to`.
fn unload_to(instance_id: InstanceId, secret: &Secret, index: RingIndex, revision: u32, to: u64) {
	let ring_members = Coinage::get_recycler_members(instance_id, 0, index);
	let proven_msg = [1u8; 32];
	let (proof, alias) = create_unload_proof(secret, &ring_members, &proven_msg);
	assert_ok!(Coinage::unload_recycler_into_external_asset(
		Origin::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: UnloadFee::Prepaid
		}
		.into(),
		instance_id,
		bounded_vec![alias],
		0,
		index,
		revision,
		to,
		0,
	));
}

/// Unload `secret`'s key of the native instance to `to`, paying the fee out of the output.
fn unload_to_paying_from_output(
	instance_id: InstanceId,
	secret: &Secret,
	index: RingIndex,
	revision: u32,
	to: u64,
	max_fee: u64,
) -> DispatchResultWithPostInfo {
	let ring_members = Coinage::get_recycler_members(instance_id, 0, index);
	let proven_msg = [1u8; 32];
	let (proof, alias) = create_unload_proof(secret, &ring_members, &proven_msg);
	// Normally done by the extension's `prepare`.
	RecyclerManager::<Test>::mark_alias_unloaded(instance_id, 0, index, alias);
	Coinage::unload_recycler_into_external_asset(
		Origin::UnloadToken {
			alias_proofs: bounded_vec![proof],
			proven_msg,
			fee: UnloadFee::FromOutput { fee_recycler_value: 0, fee_recycler_index: index },
		}
		.into(),
		instance_id,
		bounded_vec![alias],
		0,
		index,
		revision,
		to,
		max_fee,
	)
}

#[test]
fn native_instance_creation_needs_the_existential_deposit_buffer() {
	new_test_ext_no_instance().execute_with(|| {
		System::set_block_number(1);
		let pallet_account = Coinage::pallet_account();

		// The native token needs no touch, but the buffer against the pallet account being
		// dusted is required of it like of any asset.
		assert_noop!(
			Coinage::create_sufficient_instance(
				RuntimeOrigin::root(),
				NATIVE_DEPOSIT_ID,
				UNDERLYING_ASSET_UNIT
			),
			Error::<Test>::PalletAccountBelowMinimumBalance
		);

		let instance_id = create_native_instance();

		let record = Instances::<Test>::get(instance_id).expect("instance was created");
		assert_eq!(record.asset_id, NATIVE_DEPOSIT_ID);
		assert_eq!(record.asset_unit, UNDERLYING_ASSET_UNIT);
		assert_eq!(record.mode, InstanceMode::Sufficient);
		assert_eq!(Coinage::get_instance_ids(NATIVE_DEPOSIT_ID), vec![instance_id]);
		System::assert_has_event(
			crate::Event::<Test>::InstanceCreated {
				instance_id,
				asset_id: NATIVE_DEPOSIT_ID,
				asset_unit: UNDERLYING_ASSET_UNIT,
				mode: InstanceMode::Sufficient,
			}
			.into(),
		);

		// The call issued no native: the pallet account still holds only the buffer.
		assert_eq!(native_balance(pallet_account), existential_deposit());
	});
}

#[test]
fn native_instance_loads_and_unloads_the_native_token() {
	new_test_ext_no_instance().execute_with(|| {
		let instance_id = create_native_instance();
		let pallet_account = Coinage::pallet_account();

		// Loading two keys moves the coins' backing to the pallet account, held as wrapped.
		let (secrets, index, revision) =
			setup_recycler_for(instance_id, NATIVE_DEPOSIT_ID, 0, 2, 0);
		assert_eq!(native_wrapped(pallet_account), 2 * UNDERLYING_ASSET_UNIT);
		assert_eq!(
			native_balance(pallet_account),
			existential_deposit() + 2 * UNDERLYING_ASSET_UNIT
		);
		let issuance_before = Balances::total_issuance();

		// Unloading pays the destination in native out of that hold.
		let to = 9_401;
		unload_to(instance_id, &secrets[0], index, revision, to);
		assert_eq!(native_balance(to), UNDERLYING_ASSET_UNIT);
		assert_eq!(native_wrapped(pallet_account), UNDERLYING_ASSET_UNIT);

		// Unwrapping is a transfer out of the hold: no native was issued or burnt.
		assert_eq!(Balances::total_issuance(), issuance_before);
	});
}

#[test]
fn native_instance_prices_its_fee_without_the_market() {
	new_test_ext_no_instance().execute_with(|| {
		System::set_block_number(1);
		let instance_id = create_native_instance();
		let pallet_account = Coinage::pallet_account();
		let (secrets, index, revision) =
			setup_recycler_for(instance_id, NATIVE_DEPOSIT_ID, 0, 2, 0);

		// The coins already wrap the native currency, so the fee is its own quote: no pool exists
		// for a pair of one asset, and the market is never asked for one.
		let fee = Coinage::get_paid_unload_token_fee_in_native();
		assert!(!fee.is_zero());
		assert_eq!(Coinage::get_paid_unload_token_fee_in_asset(instance_id), Some(fee));

		// Paying it moves the fee straight to the fee destination, out of the output.
		let to = 9_501;
		let fee_destination_before = native_balance(FEE_DESTINATION);
		let issuance_before = Balances::total_issuance();
		assert_ok!(unload_to_paying_from_output(
			instance_id,
			&secrets[1],
			index,
			revision,
			to,
			fee
		));
		assert_eq!(native_balance(to), UNDERLYING_ASSET_UNIT - fee);
		assert_eq!(native_balance(FEE_DESTINATION), fee_destination_before + fee);
		assert_eq!(native_wrapped(pallet_account), UNDERLYING_ASSET_UNIT);
		// The fee was moved, not minted: the market would have had to mint the native side.
		assert_eq!(Balances::total_issuance(), issuance_before);
	});
}

/// Build the input and proof of a non-anonymous unload of `secret`'s key, whose proven message
/// binds the instance, the inputs, the destination and the signer.
fn non_anonymous_input(
	instance_id: InstanceId,
	secret: &Secret,
	index: RingIndex,
	revision: u32,
	to: u64,
	signer: u64,
) -> (
	UnloadRecyclerInput<<Test as Config>::MaxConsolidation>,
	BoundedVec<Proof, <Test as Config>::MaxConsolidation>,
) {
	type RInput = UnloadRecyclerInput<<Test as Config>::MaxConsolidation>;
	let members = Coinage::get_recycler_members(instance_id, 0, index);

	// The proven message covers the aliases, so it takes two passes: one to derive the alias from a
	// placeholder message, one to prove against the message that carries it.
	let placeholder: Vec<RInput> =
		vec![UnloadRecyclerInput { value: 0, index, revision, aliases: bounded_vec![[0u8; 32]] }];
	let proven_msg = blake2_256(&(instance_id, &placeholder, &to, &signer).encode());
	let (_, alias) = create_unload_proof(secret, &members, &proven_msg);

	let inputs: Vec<RInput> =
		vec![UnloadRecyclerInput { value: 0, index, revision, aliases: bounded_vec![alias] }];
	let proven_msg = blake2_256(&(instance_id, &inputs, &to, &signer).encode());
	let (proof, alias) = create_unload_proof(secret, &members, &proven_msg);

	let input = UnloadRecyclerInput { value: 0, index, revision, aliases: bounded_vec![alias] };
	(input, bounded_vec![proof])
}

/// A native instance has no market to convert through, so a signer paying with
/// [`FeeCurrency::ExternalAsset`] pays the native fee itself. `max_fee` still bounds it: it is the
/// parameter that call bounds its fee with, whatever the instance wraps.
#[test]
fn native_instance_signer_pays_the_native_fee_for_the_external_asset_currency() {
	new_test_ext_no_instance().execute_with(|| {
		System::set_block_number(1);
		let instance_id = create_native_instance();
		let (secrets, index, revision) =
			setup_recycler_for(instance_id, NATIVE_DEPOSIT_ID, 0, 2, 0);

		let signer = ALICE;
		let to = 9_601;
		let fee = Coinage::get_paid_unload_token_fee_in_native();
		// The signer keeps its account alive across the fee, so fund it above the deposit.
		fund_native(signer, existential_deposit() + fee * 4);

		// A bound below the fee rejects the call, even though no conversion takes place.
		let (input, proofs) =
			non_anonymous_input(instance_id, &secrets[0], index, revision, to, signer);
		let err = Coinage::unload_recycler_into_external_asset_non_anonymous(
			RuntimeOrigin::signed(signer),
			instance_id,
			input.clone(),
			proofs.clone(),
			to,
			FeeCurrency::ExternalAsset,
			fee - 1,
		)
		.expect_err("the bound rejects the call");
		assert_eq!(err.error, Error::<Test>::FeeExceedsMaxFee.into());

		// With a bound that covers it, the fee moves in native from the signer to the destination
		// and the unloaded value reaches `to` untouched.
		let signer_before = native_balance(signer);
		let fee_destination_before = native_balance(FEE_DESTINATION);
		let issuance_before = Balances::total_issuance();
		assert_ok!(Coinage::unload_recycler_into_external_asset_non_anonymous(
			RuntimeOrigin::signed(signer),
			instance_id,
			input,
			proofs,
			to,
			FeeCurrency::ExternalAsset,
			fee
		));

		assert_eq!(signer_before - native_balance(signer), fee);
		assert_eq!(native_balance(FEE_DESTINATION) - fee_destination_before, fee);
		assert_eq!(native_balance(to), UNDERLYING_ASSET_UNIT);
		// No market minted the native side of a conversion that never happened.
		assert_eq!(Balances::total_issuance(), issuance_before);
	});
}

#[test]
fn native_instance_can_be_sponsored_and_collateralized_in_native() {
	new_test_ext_no_instance().execute_with(|| {
		// A permissionless creation, wrapping the native token and paying for it in native.
		fund_native(SPONSOR, 1_000_000);
		let pallet_account = Coinage::pallet_account();
		let sponsor_before = native_balance(SPONSOR);
		let instance_id = NextInstanceId::<Test>::get();
		assert_ok!(Coinage::create_sponsored_instance(
			RuntimeOrigin::signed(SPONSOR),
			NATIVE_DEPOSIT_ID,
			UNDERLYING_ASSET_UNIT,
			None
		));

		// The pallet account's buffer came from the creator, who also carries the creation
		// deposit, both in the currency the coins wrap.
		assert_eq!(native_balance(pallet_account), existential_deposit());
		assert_eq!(
			<NativeAndAssets as InspectHold<_>>::balance_on_hold(
				NATIVE_DEPOSIT_ID,
				&HoldReason::InstanceCreationDeposit.into(),
				&SPONSOR,
			),
			InstanceCreationDepositAmount::get()
		);
		// Only the buffer left the creator: the creation deposit is held on their own account.
		assert_eq!(native_balance(SPONSOR), sponsor_before - existential_deposit());

		// The pot collateralizes loads in native too: the same currency backs the coins on the
		// pallet account and the load deposits on the pot, under distinct hold reasons.
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 100);
		let (_secrets, _index, _revision) =
			setup_recycler_for(instance_id, NATIVE_DEPOSIT_ID, 0, 1, 0);
		assert_eq!(native_wrapped(pallet_account), UNDERLYING_ASSET_UNIT);
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), PRICE);
		check_load_deposit_invariant(instance_id, 1);
	});
}
