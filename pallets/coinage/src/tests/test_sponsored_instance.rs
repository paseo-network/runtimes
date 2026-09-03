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

//! Tests for sponsored instances: creation, pot funding and withdrawal, and the load-deposit
//! ledger (charge, rotation, settlement, collapse).

use crate::{mock::*, pallet::*, *};
use codec::Encode;
use frame_support::{
	assert_noop, assert_ok,
	dispatch::GetDispatchInfo,
	traits::{
		fungibles::{Inspect, InspectHold},
		Consideration, UnfilteredDispatchable,
	},
	BoundedVec,
};
use sp_runtime::{bounded_vec, TokenError};
use verifiable::GenerateVerifiable;

/// The load deposit price used across these tests.
const PRICE: u64 = 10;

// ============================================================================
// `create_sponsored_instance`
// ============================================================================

#[test]
fn create_sponsored_instance_works() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		setup_asset();
		create_asset(SPONSORED_ASSET_ID);
		fund_native(SPONSOR, 1_000_000);
		assert_ok!(Assets::mint(
			RuntimeOrigin::signed(ALICE),
			SPONSORED_ASSET_ID,
			SPONSOR,
			1_000_000
		));
		create_asset(EXTRA_ASSET_ID_BASE);
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), EXTRA_ASSET_ID_BASE, SPONSOR, 1_000));

		let supply_before = Assets::total_supply(SPONSORED_ASSET_ID);
		let sponsor_asset_before = Assets::balance(SPONSORED_ASSET_ID, SPONSOR);
		let sponsor_native_before = Balances::free_balance(SPONSOR);
		let instance_id = crate::NextInstanceId::<Test>::get();

		assert_ok!(Coinage::create_sponsored_instance(
			RuntimeOrigin::signed(SPONSOR),
			SPONSORED_ASSET_ID,
			UNDERLYING_ASSET_UNIT,
			Some((EXTRA_ASSET_ID_BASE, 500))
		));

		let record = Instances::<Test>::get(instance_id).expect("instance was created");
		assert_eq!(record.mode, InstanceMode::Sponsored);
		assert_eq!(record.asset_id, SPONSORED_ASSET_ID);
		assert_eq!(record.asset_unit, UNDERLYING_ASSET_UNIT);
		System::assert_has_event(
			crate::Event::<Test>::InstanceCreated {
				instance_id,
				asset_id: SPONSORED_ASSET_ID,
				asset_unit: UNDERLYING_ASSET_UNIT,
				mode: InstanceMode::Sponsored,
			}
			.into(),
		);

		// The creation deposit is held on the creator in native, and its ticket is kept in the
		// record alongside the account it is attributable to.
		assert_eq!(
			<NativeAndAssets as InspectHold<_>>::balance_on_hold(
				NATIVE_DEPOSIT_ID,
				&HoldReason::InstanceCreationDeposit.into(),
				&SPONSOR,
			),
			InstanceCreationDepositAmount::get()
		);
		// The ticket is the one a fresh creation deposit for this instance's footprint produces.
		let other = 4_244;
		fund_native(other, 1_000);
		assert_eq!(
			record.creator,
			Some((
				SPONSOR,
				<Test as Config>::InstanceCreationDeposit::new(
					&other,
					Coinage::instance_creation_footprint()
				)
				.expect("the account is funded")
			))
		);

		// The pallet account's minimum balance came from the creator, nothing was minted.
		let min_balance = Assets::minimum_balance(SPONSORED_ASSET_ID);
		assert_eq!(Assets::balance(SPONSORED_ASSET_ID, Coinage::pallet_account()), min_balance);
		assert_eq!(
			Assets::balance(SPONSORED_ASSET_ID, SPONSOR),
			sponsor_asset_before - min_balance
		);
		assert_eq!(Assets::total_supply(SPONSORED_ASSET_ID), supply_before);

		// Creation transfers the creator no native: the pot's account for a currency is created
		// by the funding that first needs it, and the creation deposit stays held on the
		// creator's own account.
		let pot = Coinage::pot_account(instance_id);
		assert_eq!(Balances::free_balance(pot), 0);
		assert_eq!(
			sponsor_native_before - Balances::free_balance(SPONSOR),
			InstanceCreationDepositAmount::get()
		);

		// The initial funding is recorded as the creator's contribution.
		assert_eq!(PotContributions::<Test>::get((instance_id, SPONSOR, EXTRA_ASSET_ID_BASE)), 500);
		assert_eq!(Assets::balance(EXTRA_ASSET_ID_BASE, pot), 500);
	});
}

#[test]
fn create_sponsored_instance_negative_cases() {
	new_test_ext().execute_with(|| {
		let instance_id = setup_sponsored_instance();

		// `SponsorOrigin` is `EnsureSigned` in the mock: a root creation is rejected.
		assert_noop!(
			Coinage::create_sponsored_instance(
				RuntimeOrigin::root(),
				SPONSORED_ASSET_ID,
				UNDERLYING_ASSET_UNIT,
				None
			),
			sp_runtime::DispatchError::BadOrigin
		);

		// Unknown asset and invalid units. The pallet account's buffer is transferred before the
		// creation validates the asset, so an unknown one fails inside the fungible and which
		// error comes out is not part of the call's contract.
		assert!(Coinage::create_sponsored_instance(
			RuntimeOrigin::signed(SPONSOR),
			12_345,
			UNDERLYING_ASSET_UNIT,
			None
		)
		.is_err());
		// Funded in the asset, so the pallet account's buffer is covered and the unit is what the
		// call rejects.
		create_asset(12);
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), 12, SPONSOR, 1_000));
		assert_noop!(
			Coinage::create_sponsored_instance(RuntimeOrigin::signed(SPONSOR), 12, 0, None),
			Error::<Test>::InvalidAssetUnit
		);
		// `MinimumExponent` is -2, so a unit not divisible by 4 truncates.
		assert_noop!(
			Coinage::create_sponsored_instance(RuntimeOrigin::signed(SPONSOR), 12, 1_001, None),
			Error::<Test>::InvalidAssetUnit
		);

		let _ = instance_id;
	});
}

#[test]
fn permissionless_creation_can_be_switched_off() {
	new_test_ext().execute_with(|| {
		setup_asset();
		create_asset(SPONSORED_ASSET_ID);
		fund_native(SPONSOR, 1_000_000);
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), SPONSORED_ASSET_ID, SPONSOR, 1_000));

		EnablePermissionless::set(&false);
		assert_noop!(
			Coinage::create_sponsored_instance(
				RuntimeOrigin::signed(SPONSOR),
				SPONSORED_ASSET_ID,
				UNDERLYING_ASSET_UNIT,
				None
			),
			Error::<Test>::SponsoredInstancesDisabled
		);

		// Governance can still hand an instance the sponsored economics itself.
		assert_ok!(Coinage::make_instance_sponsored(RuntimeOrigin::root(), TEST_INSTANCE_ID));

		EnablePermissionless::set(&true);
		assert_ok!(Coinage::create_sponsored_instance(
			RuntimeOrigin::signed(SPONSOR),
			SPONSORED_ASSET_ID,
			UNDERLYING_ASSET_UNIT,
			None
		));
	});
}

#[test]
fn create_sponsored_instance_without_the_creation_deposit_fails() {
	new_test_ext().execute_with(|| {
		setup_asset();
		create_asset(SPONSORED_ASSET_ID);
		// A creator with nothing at all fails on the pallet account's buffer, which the call
		// transfers before it takes the creation deposit. The failure comes from the fungible, so
		// the error is not part of the call's contract.
		let broke = 4_242;
		let next_before = crate::NextInstanceId::<Test>::get();
		assert!(Coinage::create_sponsored_instance(
			RuntimeOrigin::signed(broke),
			SPONSORED_ASSET_ID,
			UNDERLYING_ASSET_UNIT,
			None
		)
		.is_err());

		// With the buffer covered but no native for the deposit, the creation deposit is what
		// fails.
		assert_ok!(Assets::mint(
			RuntimeOrigin::signed(ALICE),
			SPONSORED_ASSET_ID,
			broke,
			Assets::minimum_balance(SPONSORED_ASSET_ID)
		));
		assert_noop!(
			Coinage::create_sponsored_instance(
				RuntimeOrigin::signed(broke),
				SPONSORED_ASSET_ID,
				UNDERLYING_ASSET_UNIT,
				None
			),
			TokenError::FundsUnavailable
		);

		// Nothing survives: no instance and no recycler collection for it.
		assert_eq!(crate::NextInstanceId::<Test>::get(), next_before);
		assert!(Instances::<Test>::get(next_before).is_none());
		assert!(!RecyclerCollectionCreated::<Test>::contains_key(next_before, 0));
	});
}

#[test]
fn broke_creator_fails_atomically() {
	new_test_ext().execute_with(|| {
		// Only the privileged instance exists, so collection creation succeeds and the
		// failure comes from the creation deposit hold.
		setup_asset();
		create_asset(12);
		let broke = 77u64;
		fund_native(broke, 10);
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), 12, broke, 1_000));
		let next_before = crate::NextInstanceId::<Test>::get();
		let call = crate::Call::<Test>::create_sponsored_instance {
			asset_id: 12,
			asset_unit: UNDERLYING_ASSET_UNIT,
			initial_funding: None,
		};
		assert_noop!(
			call.dispatch_bypass_filter(RuntimeOrigin::signed(broke)),
			TokenError::FundsUnavailable
		);
		assert_eq!(crate::NextInstanceId::<Test>::get(), next_before);
		assert!(crate::AssetToInstance::<Test>::iter_key_prefix(12).next().is_none());
		assert!(Instances::<Test>::get(next_before).is_none());
	});
}

#[test]
fn failed_initial_funding_reverts_the_whole_creation() {
	new_test_ext().execute_with(|| {
		setup_asset();
		create_asset(SPONSORED_ASSET_ID);
		fund_native(SPONSOR, 1_000_000);
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), SPONSORED_ASSET_ID, SPONSOR, 1_000));
		// A currency whose minimum balance the bundled funding does not reach, so the last step
		// of the creation fails.
		let high_min_asset = 4_243;
		assert_ok!(Assets::force_create(RuntimeOrigin::root(), high_min_asset, ALICE, true, 10));
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), high_min_asset, SPONSOR, 100));

		let next_before = crate::NextInstanceId::<Test>::get();
		let call = crate::Call::<Test>::create_sponsored_instance {
			asset_id: SPONSORED_ASSET_ID,
			asset_unit: UNDERLYING_ASSET_UNIT,
			initial_funding: Some((high_min_asset, 9)),
		};
		assert_noop!(
			call.dispatch_bypass_filter(RuntimeOrigin::signed(SPONSOR)),
			Error::<Test>::FundingBelowMinimumBalance
		);

		// Nothing survives: no instance, no creation deposit hold, no contribution record.
		assert_eq!(crate::NextInstanceId::<Test>::get(), next_before);
		assert!(Instances::<Test>::get(next_before).is_none());
		assert!(crate::AssetToInstance::<Test>::iter_key_prefix(SPONSORED_ASSET_ID)
			.next()
			.is_none());
		assert_eq!(
			<NativeAndAssets as InspectHold<_>>::balance_on_hold(
				NATIVE_DEPOSIT_ID,
				&HoldReason::InstanceCreationDeposit.into(),
				&SPONSOR,
			),
			0
		);
		assert!(!PotContributions::<Test>::contains_key((next_before, SPONSOR, high_min_asset)));
	});
}

#[test]
fn create_sponsored_instance_touches_the_pallet_account_for_a_non_sufficient_asset() {
	new_test_ext().execute_with(|| {
		setup_asset();
		// A non-sufficient asset: the pallet account cannot receive it until it is touched,
		// which the call does itself at the creator's expense.
		let asset_id = 4_321;
		assert_ok!(Assets::force_create(RuntimeOrigin::root(), asset_id, ALICE, false, 1));
		// A non-sufficient asset needs its holders to exist already, the creator included.
		fund_native(SPONSOR, 1_000_000);
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), asset_id, SPONSOR, 1_000));

		let pallet_account = Coinage::pallet_account();
		assert!(!pallet_assets::Account::<Test>::contains_key(asset_id, pallet_account));

		assert_ok!(Coinage::create_sponsored_instance(
			RuntimeOrigin::signed(SPONSOR),
			asset_id,
			UNDERLYING_ASSET_UNIT,
			None
		));

		// The account was touched and given the asset's minimum balance, transferred from the
		// creator: the manual preparation `create_sufficient_instance` expects of governance is not
		// needed.
		assert!(pallet_assets::Account::<Test>::contains_key(asset_id, pallet_account));
		let min_balance = Assets::minimum_balance(asset_id);
		assert_eq!(Assets::balance(asset_id, pallet_account), min_balance);
		assert_eq!(Assets::balance(asset_id, SPONSOR), 1_000 - min_balance);
	});
}

#[test]
fn create_sponsored_instance_leaves_a_provisioned_pallet_account_alone() {
	new_test_ext().execute_with(|| {
		// The privileged instance's setup already gave the pallet account the buffer for
		// `TEST_ASSET_ID`.
		setup_asset();
		fund_native(SPONSOR, 1_000_000);
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), TEST_ASSET_ID, SPONSOR, 1_000));
		let pallet_account = Coinage::pallet_account();
		let pallet_before = Assets::balance(TEST_ASSET_ID, pallet_account);
		assert!(pallet_before >= Assets::minimum_balance(TEST_ASSET_ID));

		assert_ok!(Coinage::create_sponsored_instance(
			RuntimeOrigin::signed(SPONSOR),
			TEST_ASSET_ID,
			UNDERLYING_ASSET_UNIT,
			None
		));

		// Already at or above the minimum balance: nothing is taken from the creator.
		assert_eq!(Assets::balance(TEST_ASSET_ID, pallet_account), pallet_before);
		assert_eq!(Assets::balance(TEST_ASSET_ID, SPONSOR), 1_000);
	});
}

#[test]
fn an_asset_can_be_wrapped_at_several_units() {
	new_test_ext().execute_with(|| {
		// The privileged instance already wraps `TEST_ASSET_ID` at `UNDERLYING_ASSET_UNIT`, and
		// governance's choice of unit does not stop anyone from wrapping it at another one.
		let sponsored = setup_sponsored_instance();
		fund_native(SPONSOR, 1_000_000);
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), TEST_ASSET_ID, SPONSOR, 1_000_000));

		let next = crate::NextInstanceId::<Test>::get();
		assert_ok!(Coinage::create_sponsored_instance(
			RuntimeOrigin::signed(SPONSOR),
			TEST_ASSET_ID,
			UNDERLYING_ASSET_UNIT * 4,
			None
		));

		let record = Instances::<Test>::get(next).expect("instance was created");
		assert_eq!(record.asset_id, TEST_ASSET_ID);
		assert_eq!(record.asset_unit, UNDERLYING_ASSET_UNIT * 4);
		assert_eq!(record.mode, InstanceMode::Sponsored);

		// Both instances are listed under the asset, each with its own unit.
		let mut instances = Coinage::get_instance_ids(TEST_ASSET_ID);
		instances.sort();
		assert_eq!(instances, vec![TEST_INSTANCE_ID, next]);

		// Each instance keeps its own pot and ledger.
		assert_ne!(Coinage::pot_account(sponsored), Coinage::pot_account(next));
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);
		fund_pot(next, NATIVE_DEPOSIT_ID, 100);
		assert_ok!(try_load_with_unit(next, TEST_ASSET_ID, UNDERLYING_ASSET_UNIT * 4, 0));
		check_load_deposit_invariant(next, 1);
		assert!(current_tier(sponsored).is_none());
	});
}

#[test]
fn instance_count_is_not_capped_by_the_members_collection_bound() {
	new_test_ext().execute_with(|| {
		// Each instance creates 10 recycler collections in the mock and the members pallet
		// bounds collections per owner at 20. Four instances (40 collections) only fit
		// because every instance is its own recycler collection owner.
		let first = setup_sponsored_instance();
		fund_native(SPONSOR, 10_000_000);
		for asset_id in [13u32, 14] {
			create_asset(asset_id);
			assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), asset_id, SPONSOR, 1_000_000));
			assert_ok!(Coinage::create_sponsored_instance(
				RuntimeOrigin::signed(SPONSOR),
				asset_id,
				UNDERLYING_ASSET_UNIT,
				None
			));
		}
		assert_eq!(crate::NextInstanceId::<Test>::get(), first + 3);
	});
}

#[test]
fn instance_creation_footprint_scales_with_the_denomination_range() {
	new_test_ext_no_instance().execute_with(|| {
		// The mock wraps `[-2, 7]`, so ten denominations and ten recycler collections, each
		// estimated at 4 storage entries and 300 bytes, on top of the two registry entries.
		let registry_bytes = InstanceRecord::<Test>::max_encoded_len() as u64 +
			<FungiblesAssetIdOf<Test> as MaxEncodedLen>::max_encoded_len() as u64;
		let footprint = Coinage::instance_creation_footprint();
		assert_eq!(footprint.count, 10 * 4 + 2);
		assert_eq!(footprint.size, 10 * 300 + registry_bytes);

		// One denomination fewer is one collection fewer.
		MaximumExponent::set(&6);
		let smaller = Coinage::instance_creation_footprint();
		assert_eq!(smaller.count, footprint.count - 4);
		assert_eq!(smaller.size, footprint.size - 300);
	});
}

// ============================================================================
// `fund_pot`
// ============================================================================

#[test]
fn fund_pot_and_withdraw_round_trip() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let instance_id = setup_sponsored_instance();
		let pot = Coinage::pot_account(instance_id);

		create_asset(EXTRA_ASSET_ID_BASE);
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), EXTRA_ASSET_ID_BASE, SPONSOR, 1_000));
		let sponsor_before = Assets::balance(EXTRA_ASSET_ID_BASE, SPONSOR);

		assert_ok!(Coinage::fund_pot(
			RuntimeOrigin::signed(SPONSOR),
			instance_id,
			EXTRA_ASSET_ID_BASE,
			600
		));
		assert_eq!(PotContributions::<Test>::get((instance_id, SPONSOR, EXTRA_ASSET_ID_BASE)), 600);
		assert_eq!(Assets::balance(EXTRA_ASSET_ID_BASE, pot), 600);
		System::assert_has_event(
			crate::Event::<Test>::PotFunded {
				instance_id,
				funder: SPONSOR,
				currency: EXTRA_ASSET_ID_BASE,
				amount: 600,
			}
			.into(),
		);

		// Nothing is held, so everything but the pot's minimum balance is withdrawable: the
		// account it was funded into must survive the withdrawal.
		let min_balance = Assets::minimum_balance(EXTRA_ASSET_ID_BASE);
		assert_noop!(
			Coinage::withdraw_pot_funds(
				RuntimeOrigin::signed(SPONSOR),
				instance_id,
				EXTRA_ASSET_ID_BASE,
				600
			),
			TokenError::NotExpendable
		);
		assert_ok!(Coinage::withdraw_pot_funds(
			RuntimeOrigin::signed(SPONSOR),
			instance_id,
			EXTRA_ASSET_ID_BASE,
			600 - min_balance
		));
		assert_eq!(
			PotContributions::<Test>::get((instance_id, SPONSOR, EXTRA_ASSET_ID_BASE)),
			min_balance
		);
		assert_eq!(Assets::balance(EXTRA_ASSET_ID_BASE, SPONSOR), sponsor_before - min_balance);
		assert_eq!(Assets::balance(EXTRA_ASSET_ID_BASE, pot), min_balance);
	});
}

#[test]
fn fund_pot_negative_cases() {
	new_test_ext().execute_with(|| {
		let instance_id = setup_sponsored_instance();
		fund_native(SPONSOR, 1_000);

		assert_noop!(
			Coinage::fund_pot(RuntimeOrigin::signed(SPONSOR), instance_id, NATIVE_DEPOSIT_ID, 0),
			Error::<Test>::ZeroAmount
		);
		assert_noop!(
			Coinage::withdraw_pot_funds(
				RuntimeOrigin::signed(SPONSOR),
				instance_id,
				NATIVE_DEPOSIT_ID,
				0
			),
			Error::<Test>::ZeroAmount
		);

		// A currency that does not exist fails on the transfer itself; no record is created.
		assert!(Coinage::fund_pot(RuntimeOrigin::signed(SPONSOR), instance_id, 4_242, 100).is_err());
		assert!(!PotContributions::<Test>::contains_key((instance_id, SPONSOR, 4_242)));

		// A funding below the currency's minimum balance could be dusted by the transfer right
		// away, so it is refused whether or not the pot already has an account for it.
		let high_min_asset = 4_243;
		assert_ok!(Assets::force_create(RuntimeOrigin::root(), high_min_asset, ALICE, true, 10));
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), high_min_asset, SPONSOR, 100));
		assert_noop!(
			Coinage::fund_pot(RuntimeOrigin::signed(SPONSOR), instance_id, high_min_asset, 9),
			Error::<Test>::FundingBelowMinimumBalance
		);
		assert_ok!(Coinage::fund_pot(
			RuntimeOrigin::signed(SPONSOR),
			instance_id,
			high_min_asset,
			10
		));
		assert_noop!(
			Coinage::fund_pot(RuntimeOrigin::signed(SPONSOR), instance_id, high_min_asset, 9),
			Error::<Test>::FundingBelowMinimumBalance
		);

		// A privileged instance has no pot.
		assert_noop!(
			Coinage::fund_pot(
				RuntimeOrigin::signed(SPONSOR),
				TEST_INSTANCE_ID,
				NATIVE_DEPOSIT_ID,
				100
			),
			Error::<Test>::InstanceNotSponsored
		);
	});
}

#[test]
fn fund_pot_creates_the_pot_account_for_the_currency() {
	new_test_ext().execute_with(|| {
		// An instance switched to sponsored starts with a pot that has no account anywhere, so
		// a funding would have nowhere to land.
		setup_asset();
		assert_ok!(Coinage::make_instance_sponsored(RuntimeOrigin::root(), TEST_INSTANCE_ID));
		let pot = Coinage::pot_account(TEST_INSTANCE_ID);
		assert_eq!(Balances::free_balance(pot), 0);
		assert!(!pallet_assets::Account::<Test>::contains_key(EXTRA_ASSET_ID_BASE, pot));

		create_asset(EXTRA_ASSET_ID_BASE);
		fund_native(SPONSOR, 1_000);
		assert_ok!(Assets::mint(RuntimeOrigin::signed(ALICE), EXTRA_ASSET_ID_BASE, SPONSOR, 1_000));

		// The funder pays whatever touching the account costs, and it is not recorded as a
		// contribution.
		assert_ok!(Coinage::fund_pot(
			RuntimeOrigin::signed(SPONSOR),
			TEST_INSTANCE_ID,
			EXTRA_ASSET_ID_BASE,
			500
		));
		assert!(pallet_assets::Account::<Test>::contains_key(EXTRA_ASSET_ID_BASE, pot));
		assert_eq!(Assets::balance(EXTRA_ASSET_ID_BASE, pot), 500);
		assert_eq!(
			PotContributions::<Test>::get((TEST_INSTANCE_ID, SPONSOR, EXTRA_ASSET_ID_BASE)),
			500
		);
		assert!(!PotContributions::<Test>::contains_key((
			TEST_INSTANCE_ID,
			SPONSOR,
			NATIVE_DEPOSIT_ID
		)));

		// A second funding finds the account already there.
		assert_ok!(Coinage::fund_pot(
			RuntimeOrigin::signed(SPONSOR),
			TEST_INSTANCE_ID,
			EXTRA_ASSET_ID_BASE,
			100
		));
		assert_eq!(Assets::balance(EXTRA_ASSET_ID_BASE, pot), 600);

		// And the native side of the union works the same way.
		assert_ok!(Coinage::fund_pot(
			RuntimeOrigin::signed(SPONSOR),
			TEST_INSTANCE_ID,
			NATIVE_DEPOSIT_ID,
			200
		));
		assert_eq!(Balances::free_balance(pot), 200);
	});
}

#[test]
fn prefunding_a_non_current_currency_works() {
	new_test_ext().execute_with(|| {
		let instance_id = setup_sponsored_instance();
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);

		// Funding a currency that is not the current deposit currency succeeds, creates the
		// pot's asset account and is withdrawable like any other contribution.
		fund_pot(instance_id, EXTRA_ASSET_ID_BASE, 400);
		let pot = Coinage::pot_account(instance_id);
		assert_eq!(Assets::balance(EXTRA_ASSET_ID_BASE, pot), 400);
		assert_ok!(Coinage::withdraw_pot_funds(
			RuntimeOrigin::signed(SPONSOR),
			instance_id,
			EXTRA_ASSET_ID_BASE,
			400 - Assets::minimum_balance(EXTRA_ASSET_ID_BASE)
		));
	});
}

// ============================================================================
// `withdraw_pot_funds`
// ============================================================================

#[test]
fn withdrawing_the_whole_contribution_removes_the_record() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let instance_id = setup_sponsored_instance();
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 100);
		fund_native(BOB, 1_000);
		assert_ok!(Coinage::fund_pot(
			RuntimeOrigin::signed(BOB),
			instance_id,
			NATIVE_DEPOSIT_ID,
			100
		));

		// Bob's funds keep the pot's account alive, so the sponsor's whole contribution is
		// withdrawable and the record is removed rather than kept at zero.
		assert_ok!(Coinage::withdraw_pot_funds(
			RuntimeOrigin::signed(SPONSOR),
			instance_id,
			NATIVE_DEPOSIT_ID,
			100
		));
		assert!(!PotContributions::<Test>::contains_key((instance_id, SPONSOR, NATIVE_DEPOSIT_ID)));
		assert!(Coinage::get_pot_contributions(instance_id, SPONSOR).is_empty());
		System::assert_has_event(
			crate::Event::<Test>::PotFundsWithdrawn {
				instance_id,
				funder: SPONSOR,
				currency: NATIVE_DEPOSIT_ID,
				amount: 100,
			}
			.into(),
		);

		// A further withdrawal has no record to draw on.
		assert_noop!(
			Coinage::withdraw_pot_funds(
				RuntimeOrigin::signed(SPONSOR),
				instance_id,
				NATIVE_DEPOSIT_ID,
				1
			),
			Error::<Test>::WithdrawExceedsContribution
		);
	});
}

#[test]
fn withdrawal_is_capped_by_the_record() {
	new_test_ext().execute_with(|| {
		let instance_id = setup_sponsored_instance();
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 500);

		// More than the contribution, and an account that never funded.
		assert_noop!(
			Coinage::withdraw_pot_funds(
				RuntimeOrigin::signed(SPONSOR),
				instance_id,
				NATIVE_DEPOSIT_ID,
				501
			),
			Error::<Test>::WithdrawExceedsContribution
		);
		assert_noop!(
			Coinage::withdraw_pot_funds(
				RuntimeOrigin::signed(BOB),
				instance_id,
				NATIVE_DEPOSIT_ID,
				1
			),
			Error::<Test>::WithdrawExceedsContribution
		);

		// A donation via plain transfer creates no record and is not withdrawable by anyone.
		let pot = Coinage::pot_account(instance_id);
		fund_native(BOB, 1_000);
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(BOB), pot, 300));
		assert_noop!(
			Coinage::withdraw_pot_funds(
				RuntimeOrigin::signed(BOB),
				instance_id,
				NATIVE_DEPOSIT_ID,
				300
			),
			Error::<Test>::WithdrawExceedsContribution
		);
	});
}

#[test]
fn withdrawal_is_capped_by_what_is_free() {
	new_test_ext().execute_with(|| {
		let instance_id = setup_sponsored_instance();
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 3 * PRICE);

		// Two of the three deposits' worth gets held.
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 0));
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 1));

		// The full contribution is not withdrawable: holds are not reducible balance.
		assert_noop!(
			Coinage::withdraw_pot_funds(
				RuntimeOrigin::signed(SPONSOR),
				instance_id,
				NATIVE_DEPOSIT_ID,
				3 * PRICE
			),
			TokenError::FundsUnavailable
		);
		// The free part is, less what keeps the pot's account alive.
		let existential_deposit =
			<Balances as frame_support::traits::Currency<u64>>::minimum_balance();
		assert_ok!(Coinage::withdraw_pot_funds(
			RuntimeOrigin::signed(SPONSOR),
			instance_id,
			NATIVE_DEPOSIT_ID,
			PRICE - existential_deposit
		));
		check_load_deposit_invariant(instance_id, 2);
	});
}

#[test]
fn two_funders_share_one_free_slice() {
	new_test_ext().execute_with(|| {
		let instance_id = setup_sponsored_instance();
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, PRICE);
		fund_native(BOB, 1_000);
		assert_ok!(Coinage::fund_pot(
			RuntimeOrigin::signed(BOB),
			instance_id,
			NATIVE_DEPOSIT_ID,
			PRICE
		));

		// One deposit's worth gets held; one stays free.
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 0));

		// First-come-first-served on the free slice, each capped at their own record.
		let existential_deposit =
			<Balances as frame_support::traits::Currency<u64>>::minimum_balance();
		assert_ok!(Coinage::withdraw_pot_funds(
			RuntimeOrigin::signed(SPONSOR),
			instance_id,
			NATIVE_DEPOSIT_ID,
			PRICE - existential_deposit
		));
		assert_noop!(
			Coinage::withdraw_pot_funds(
				RuntimeOrigin::signed(BOB),
				instance_id,
				NATIVE_DEPOSIT_ID,
				PRICE - existential_deposit
			),
			TokenError::FundsUnavailable
		);
	});
}

#[test]
fn pot_views_report_what_a_withdrawal_can_actually_move() {
	new_test_ext().execute_with(|| {
		let instance_id = setup_sponsored_instance();
		let prefunded = EXTRA_ASSET_ID_BASE + 1;
		create_asset(EXTRA_ASSET_ID_BASE);
		create_asset(prefunded);
		set_load_deposit(EXTRA_ASSET_ID_BASE, PRICE);
		fund_pot(instance_id, EXTRA_ASSET_ID_BASE, 100);
		fund_pot(instance_id, prefunded, 500);
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 100);
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 0));

		let pot = Coinage::pot_account(instance_id);
		let min_balance = Assets::minimum_balance(prefunded);
		assert!(min_balance > 0, "the asset needs a minimum balance for this test to bite");
		let contributions = Coinage::get_pot_contributions(instance_id, SPONSOR);
		let reported = |currency: u32| {
			contributions
				.iter()
				.find(|(c, _, _)| *c == currency)
				.map(|(_, contribution, withdrawable)| (*contribution, *withdrawable))
				.expect("the contribution is recorded")
		};

		// Nothing is held in the prefunded currency, so all of it is withdrawable but the
		// minimum balance keeping the pot's account for it alive.
		assert_eq!(reported(prefunded), (500, 500 - min_balance));
		assert_ok!(Coinage::withdraw_pot_funds(
			RuntimeOrigin::signed(SPONSOR),
			instance_id,
			prefunded,
			500 - min_balance
		));
		assert_eq!(Assets::balance(prefunded, pot), min_balance);

		// The deposit currency additionally has a hold on it, which the view also subtracts.
		let free_in_deposit_currency = 100 - PRICE - min_balance;
		assert_eq!(reported(EXTRA_ASSET_ID_BASE), (100, free_in_deposit_currency));
		let status = Coinage::get_pot_status(instance_id).expect("sponsored instance");
		assert_eq!(status.free_in_deposit_asset, free_in_deposit_currency);
		assert_ok!(Coinage::withdraw_pot_funds(
			RuntimeOrigin::signed(SPONSOR),
			instance_id,
			EXTRA_ASSET_ID_BASE,
			free_in_deposit_currency
		));

		// Native works the same way: the pot's account survives at its existential deposit.
		let existential_deposit =
			<Balances as frame_support::traits::Currency<u64>>::minimum_balance();
		assert_eq!(reported(NATIVE_DEPOSIT_ID), (100, 100 - existential_deposit));
		assert_eq!(Balances::free_balance(pot), 100);
		assert_ok!(Coinage::withdraw_pot_funds(
			RuntimeOrigin::signed(SPONSOR),
			instance_id,
			NATIVE_DEPOSIT_ID,
			100 - existential_deposit
		));
		assert_eq!(Balances::free_balance(pot), existential_deposit);
	});
}

#[test]
fn pot_status_reports_the_invalidity_the_next_load_would_hit() {
	new_test_ext().execute_with(|| {
		let instance_id = setup_sponsored_instance();
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);
		let blocked =
			|| Coinage::get_pot_status(instance_id).expect("sponsored instance").loads_blocked;

		// An unfunded pot blocks the next load, and the view hands back the very invalidity the
		// pool reports, at the same custom code.
		assert_eq!(blocked(), Some(CustomInvalidity::PotCannotCoverLoadDeposit));
		assert_eq!(
			CustomInvalidity::PotCannotCoverLoadDeposit.encode(),
			vec![CustomInvalidity::PotCannotCoverLoadDeposit as u8]
		);

		// Funded, nothing blocks it.
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 1_000);
		assert_eq!(blocked(), None);

		// A price change with the old tier already taken blocks it for the other reason, which
		// no funding fixes.
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 0));
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE + 1);
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 1));
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE + 2);
		assert_eq!(blocked(), Some(CustomInvalidity::LoadDepositOldTierOccupied));
	});
}

// ============================================================================
// `collapse_load_deposits`
// ============================================================================

#[test]
fn collapse_refunds_tops_up_and_migrates_currencies() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let instance_id = setup_sponsored_instance();
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 1_000);

		// Two tiers: one key at 10 and one at 20.
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 0));
		set_load_deposit(NATIVE_DEPOSIT_ID, 20);
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 1));
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 30);

		// Collapse down to 5: a pure refund needing no pot funds.
		set_load_deposit(NATIVE_DEPOSIT_ID, 5);
		assert_ok!(Coinage::collapse_load_deposits(RuntimeOrigin::signed(BOB), instance_id));
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 10);
		check_load_deposit_invariant(instance_id, 2);
		System::assert_has_event(
			crate::Event::<Test>::LoadDepositsCollapsed {
				instance_id,
				currency: NATIVE_DEPOSIT_ID,
				price: 5,
				count: 2,
			}
			.into(),
		);

		// Collapsing an already-collapsed ledger errors.
		assert_noop!(
			Coinage::collapse_load_deposits(RuntimeOrigin::signed(BOB), instance_id),
			Error::<Test>::NothingToCollapse
		);

		// Collapse up to 15: the shortfall is taken from the pot's free balance.
		set_load_deposit(NATIVE_DEPOSIT_ID, 15);
		assert_ok!(Coinage::collapse_load_deposits(RuntimeOrigin::signed(BOB), instance_id));
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 30);
		check_load_deposit_invariant(instance_id, 2);

		// Currency switch: old-currency holds are released in full, the new requirement is
		// taken fresh, no conversion anywhere.
		create_asset(EXTRA_ASSET_ID_BASE);
		set_load_deposit(EXTRA_ASSET_ID_BASE, 7);
		// A rotating load is invalid until the pot holds the new currency.
		assert!(matches!(
			Pallet::<Test>::ensure_can_charge_load_deposit(instance_id, 1),
			Err(CustomInvalidity::PotCannotCoverLoadDeposit)
		));
		// A collapse without new-currency funds fails with the ledger untouched.
		assert_noop!(
			Coinage::collapse_load_deposits(RuntimeOrigin::signed(BOB), instance_id),
			Error::<Test>::PotCannotCoverLoadDeposit
		);
		fund_pot(instance_id, EXTRA_ASSET_ID_BASE, 100);
		let native_free_before = pot_free(instance_id, NATIVE_DEPOSIT_ID);
		assert_ok!(Coinage::collapse_load_deposits(RuntimeOrigin::signed(BOB), instance_id));
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 0);
		assert_eq!(pot_held(instance_id, EXTRA_ASSET_ID_BASE), 14);
		assert_eq!(pot_free(instance_id, NATIVE_DEPOSIT_ID), native_free_before + 30);
		check_load_deposit_invariant(instance_id, 2);

		// The released old-currency balance is reclaimable up to the funder's record, less what
		// keeps the pot's account alive.
		let existential_deposit =
			<Balances as frame_support::traits::Currency<u64>>::minimum_balance();
		assert_ok!(Coinage::withdraw_pot_funds(
			RuntimeOrigin::signed(SPONSOR),
			instance_id,
			NATIVE_DEPOSIT_ID,
			1_000 - existential_deposit
		));
	});
}

#[test]
fn collapse_merges_the_tiers_even_at_an_unchanged_price() {
	new_test_ext().execute_with(|| {
		let instance_id = setup_sponsored_instance();
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 1_000);
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 0));
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE + 1);
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 1));
		assert!(old_tier(instance_id).is_some());
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), PRICE + PRICE + 1);

		// The current tier already matches the deposit, but the occupied old slot still makes
		// the ledger collapsible: both tiers merge at the current price.
		assert_ok!(Coinage::collapse_load_deposits(RuntimeOrigin::signed(BOB), instance_id));
		assert!(old_tier(instance_id).is_none());
		assert_eq!(
			current_tier(instance_id),
			Some(DepositTier { asset_id: NATIVE_DEPOSIT_ID, price: PRICE + 1, count: 2 })
		);
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 2 * (PRICE + 1));
		check_load_deposit_invariant(instance_id, 2);
	});
}

#[test]
fn collapse_reprices_only_the_keys_still_loaded() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		// Three keys at `PRICE`, then one more after a re-price, so both tiers are live.
		let value: Denomination = 0;
		let (instance_id, secrets, index, revision) = setup_sponsored_recycler(PRICE, 1_000, 3, 0);
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE + 5);
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 0));
		check_load_deposit_invariant(instance_id, 4);

		// Settlement is oldest first, so unloading two keys drains the old tier down to one.
		let ring_members = Coinage::get_recycler_members(instance_id, value, index);
		let unload_one = |secret: &Secret, dest: u64| {
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
				value,
				index,
				revision,
				dest,
				0,
			));
		};
		unload_one(&secrets[0], 9_301);
		unload_one(&secrets[1], 9_302);
		assert_eq!(
			old_tier(instance_id),
			Some(DepositTier { asset_id: NATIVE_DEPOSIT_ID, price: PRICE, count: 1 })
		);
		check_load_deposit_invariant(instance_id, 2);

		// The collapse re-prices the two keys still loaded, not the four ever loaded.
		set_load_deposit(NATIVE_DEPOSIT_ID, 2 * PRICE);
		assert_ok!(Coinage::collapse_load_deposits(RuntimeOrigin::signed(BOB), instance_id));
		assert_eq!(
			current_tier(instance_id),
			Some(DepositTier { asset_id: NATIVE_DEPOSIT_ID, price: 2 * PRICE, count: 2 })
		);
		assert!(old_tier(instance_id).is_none());
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 4 * PRICE);
		check_load_deposit_invariant(instance_id, 2);
		System::assert_has_event(
			crate::Event::<Test>::LoadDepositsCollapsed {
				instance_id,
				currency: NATIVE_DEPOSIT_ID,
				price: 2 * PRICE,
				count: 2,
			}
			.into(),
		);
	});
}

#[test]
fn collapse_up_is_capped_by_the_released_collateral_plus_the_free_balance() {
	new_test_ext().execute_with(|| {
		let instance_id = setup_sponsored_instance();
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 100);
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 0));

		// Everything the pot could put behind that one key: the collateral the collapse releases
		// first, plus the free balance it can still part with.
		let available =
			pot_held(instance_id, NATIVE_DEPOSIT_ID) + pot_free(instance_id, NATIVE_DEPOSIT_ID);
		assert!(
			available > PRICE,
			"the top-up must draw on the free balance for this test to bite"
		);

		// One unit more than that fails, with the ledger and the hold untouched.
		set_load_deposit(NATIVE_DEPOSIT_ID, available + 1);
		assert_noop!(
			Coinage::collapse_load_deposits(RuntimeOrigin::signed(BOB), instance_id),
			Error::<Test>::PotCannotCoverLoadDeposit
		);
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), PRICE);
		assert_eq!(
			current_tier(instance_id),
			Some(DepositTier { asset_id: NATIVE_DEPOSIT_ID, price: PRICE, count: 1 })
		);
		check_load_deposit_invariant(instance_id, 1);

		// Exactly that much goes through: the released collateral counts towards the new
		// requirement, so the pot is not asked to cover the whole re-priced deposit on its own.
		set_load_deposit(NATIVE_DEPOSIT_ID, available);
		assert_ok!(Coinage::collapse_load_deposits(RuntimeOrigin::signed(BOB), instance_id));
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), available);
		assert_eq!(pot_free(instance_id, NATIVE_DEPOSIT_ID), 0);
		check_load_deposit_invariant(instance_id, 1);
	});
}

#[test]
fn collapse_negative_cases() {
	new_test_ext().execute_with(|| {
		let instance_id = setup_sponsored_instance();

		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);
		assert_noop!(
			Coinage::collapse_load_deposits(RuntimeOrigin::signed(BOB), TEST_INSTANCE_ID),
			Error::<Test>::InstanceNotSponsored
		);
		// An instance that never loaded has no ledger.
		assert_noop!(
			Coinage::collapse_load_deposits(RuntimeOrigin::signed(BOB), instance_id),
			Error::<Test>::NothingToCollapse
		);

		// The call is permissionless but still signed: neither root nor an unsigned origin can
		// make it, whatever the ledger holds.
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 100);
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 0));
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE + 1);
		assert_noop!(
			Coinage::collapse_load_deposits(RuntimeOrigin::root(), instance_id),
			sp_runtime::DispatchError::BadOrigin
		);
		assert_noop!(
			Coinage::collapse_load_deposits(RuntimeOrigin::none(), instance_id),
			sp_runtime::DispatchError::BadOrigin
		);
		// Any signed account can, sponsor or not.
		assert_ok!(Coinage::collapse_load_deposits(RuntimeOrigin::signed(BOB), instance_id));
	});
}

// ============================================================================
// `make_instance_sufficient` and `make_instance_sponsored`
// ============================================================================

#[test]
fn make_instance_sufficient_releases_everything() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let instance_id = setup_sponsored_instance();
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 100);
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 0));

		// A second tier in another currency, so the release walks two currencies.
		create_asset(EXTRA_ASSET_ID_BASE);
		set_load_deposit(EXTRA_ASSET_ID_BASE, 7);
		fund_pot(instance_id, EXTRA_ASSET_ID_BASE, 100);
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 1));
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), PRICE);
		assert_eq!(pot_held(instance_id, EXTRA_ASSET_ID_BASE), 7);

		assert_noop!(
			Coinage::make_instance_sufficient(RuntimeOrigin::signed(ALICE), instance_id),
			sp_runtime::DispatchError::BadOrigin
		);
		assert_noop!(
			Coinage::make_instance_sufficient(RuntimeOrigin::root(), TEST_INSTANCE_ID),
			Error::<Test>::InstanceNotSponsored
		);

		assert_ok!(Coinage::make_instance_sufficient(RuntimeOrigin::root(), instance_id));
		assert_eq!(
			Instances::<Test>::get(instance_id).expect("instance exists").mode,
			InstanceMode::Sufficient
		);
		System::assert_has_event(
			crate::Event::<Test>::InstanceModeSet { instance_id, mode: InstanceMode::Sufficient }
				.into(),
		);

		// Every hold was released and the ledger removed.
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 0);
		assert_eq!(pot_held(instance_id, EXTRA_ASSET_ID_BASE), 0);
		assert!(current_tier(instance_id).is_none());
		assert!(old_tier(instance_id).is_none());

		// Funders reclaim their contributions, the collateral having become free, down to what
		// keeps each of the pot's accounts alive.
		let existential_deposit =
			<Balances as frame_support::traits::Currency<u64>>::minimum_balance();
		assert_ok!(Coinage::withdraw_pot_funds(
			RuntimeOrigin::signed(SPONSOR),
			instance_id,
			NATIVE_DEPOSIT_ID,
			100 - existential_deposit
		));
		assert_ok!(Coinage::withdraw_pot_funds(
			RuntimeOrigin::signed(SPONSOR),
			instance_id,
			EXTRA_ASSET_ID_BASE,
			100 - Assets::minimum_balance(EXTRA_ASSET_ID_BASE)
		));

		// Loads take no deposit from here on, whatever `LoadDeposit` says.
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 2));
		assert_eq!(pot_held(instance_id, EXTRA_ASSET_ID_BASE), 0);
		assert!(current_tier(instance_id).is_none());
	});
}

#[test]
fn make_instance_sponsored_restarts_the_count() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		// Two keys loaded while the instance is privileged: no deposits behind them.
		let value: Denomination = 0;
		let (secrets, index, revision) = setup_recycler(value, 2, 0);
		let instance_id = TEST_INSTANCE_ID;

		assert_noop!(
			Coinage::make_instance_sponsored(RuntimeOrigin::signed(ALICE), instance_id),
			sp_runtime::DispatchError::BadOrigin
		);
		assert_ok!(Coinage::make_instance_sponsored(RuntimeOrigin::root(), instance_id));
		assert_eq!(
			Instances::<Test>::get(instance_id).expect("instance exists").mode,
			InstanceMode::Sponsored
		);
		assert_noop!(
			Coinage::make_instance_sponsored(RuntimeOrigin::root(), instance_id),
			Error::<Test>::InstanceAlreadySponsored
		);

		// The ledger restarted from zero: loads are invalid until governance sets the deposit
		// and the pot is funded, and pre-existing keys are not collateralized.
		assert!(current_tier(instance_id).is_none());
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);
		assert!(matches!(
			Pallet::<Test>::ensure_can_charge_load_deposit(instance_id, 1),
			Err(CustomInvalidity::PotCannotCoverLoadDeposit)
		));
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 100);

		// One key loaded after the switch is collateralized.
		assert_ok!(try_load(instance_id, TEST_ASSET_ID, 0));
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), PRICE);
		check_load_deposit_invariant(instance_id, 1);

		// Settlement has no attribution: the first pre-switch key's unload drains the unit
		// backing the post-switch key.
		let ring_members = Coinage::get_recycler_members(instance_id, value, index);
		let unload_one = |secret: &Secret, dest: u64| {
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
				value,
				index,
				revision,
				dest,
				0,
			));
		};
		let free_before = pot_free(instance_id, NATIVE_DEPOSIT_ID);
		unload_one(&secrets[0], 8_891);
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 0);
		assert_eq!(pot_free(instance_id, NATIVE_DEPOSIT_ID), free_before + PRICE);

		// The second pre-switch key's unload finds a drained ledger and releases nothing,
		// without failing the exit.
		unload_one(&secrets[1], 8_892);
		assert_eq!(pot_held(instance_id, NATIVE_DEPOSIT_ID), 0);
		assert_eq!(pot_free(instance_id, NATIVE_DEPOSIT_ID), free_before + PRICE);
	});
}

#[test]
fn mode_round_trip_returns_the_creation_deposit() {
	new_test_ext().execute_with(|| {
		let instance_id = setup_sponsored_instance();
		let held = || {
			<NativeAndAssets as InspectHold<_>>::balance_on_hold(
				NATIVE_DEPOSIT_ID,
				&HoldReason::InstanceCreationDeposit.into(),
				&SPONSOR,
			)
		};
		let free = || Balances::free_balance(SPONSOR);
		assert_eq!(held(), InstanceCreationDepositAmount::get());
		let free_before = free();
		assert!(Instances::<Test>::get(instance_id).expect("instance exists").creator.is_some());

		// A never-loaded ledger releases nothing but the switch still lands, and the creation
		// deposit goes back to the creator: a sufficient instance's permanent footprint is
		// carried by the chain.
		assert_ok!(Coinage::make_instance_sufficient(RuntimeOrigin::root(), instance_id));
		let record = Instances::<Test>::get(instance_id).expect("instance exists");
		assert_eq!(record.mode, InstanceMode::Sufficient);
		assert_eq!(record.creator, None);
		assert_eq!(held(), 0);
		assert_eq!(free(), free_before + InstanceCreationDepositAmount::get());

		// Sponsored again takes no new deposit, so the instance carries no creator from here on.
		assert_ok!(Coinage::make_instance_sponsored(RuntimeOrigin::root(), instance_id));
		let record = Instances::<Test>::get(instance_id).expect("instance exists");
		assert_eq!(record.mode, InstanceMode::Sponsored);
		assert_eq!(record.creator, None);
		assert_eq!(held(), 0);
		assert_eq!(free(), free_before + InstanceCreationDepositAmount::get());

		// A second switch to sufficient has no ticket to drop and still lands.
		assert_ok!(Coinage::make_instance_sufficient(RuntimeOrigin::root(), instance_id));
		assert_eq!(
			Instances::<Test>::get(instance_id).expect("instance exists").mode,
			InstanceMode::Sufficient
		);
		assert_eq!(held(), 0);
	});
}

// ============================================================================
// Weight accounting
//
// A load or unload call's declared weight prices the load-deposit surcharge off the instance's
// mode: a sponsored instance carries the charge or settlement up front, a privileged one never
// does, and no `PostDispatchInfo` refund is involved. The exception is
// `load_recycler_with_coin`, whose instance comes from the coin origin rather than the call
// arguments, so it declares the worst case and refunds. The mock's `WeightInfo` gives the charge
// and the settlement non-zero values, so these assertions compare two different weights rather
// than zero with zero.
// ============================================================================

/// A `load_recycler_with_external_asset` call, for its pre-dispatch weight. The declared weight
/// does not depend on the arguments, so a throwaway key is fine.
fn load_call(instance_id: InstanceId) -> crate::Call<Test> {
	let secret = get_unique_secret();
	crate::Call::<Test>::load_recycler_with_external_asset {
		instance_id,
		preservation: CodecPreservation::Expendable,
		value: 0,
		member_key: CryptoOf::<Test>::member_from_secret(&secret),
		proof_of_ownership: CryptoOf::<Test>::sign(&secret, &ALICE.encode()).unwrap(),
	}
}

/// Load two keys into `instance_id` and return the input and proof unloading the first one
/// non-anonymously, bound to `signer` and `to`.
fn setup_non_anonymous_unload(
	instance_id: InstanceId,
	asset_id: u32,
	signer: u64,
	to: u64,
	seed_offset: u8,
) -> (
	UnloadRecyclerInput<<Test as Config>::MaxConsolidation>,
	BoundedVec<Proof, <Test as Config>::MaxConsolidation>,
) {
	let value: Denomination = 0;
	let (secrets, index, _revision) =
		setup_recycler_for(instance_id, asset_id, value, 2, seed_offset);
	build_non_anonymous_unload(instance_id, &secrets[0..1], value, index, signer, to)
}

#[test]
fn only_a_sponsored_load_declares_the_deposit_surcharge() {
	new_test_ext().execute_with(|| {
		let instance_id = setup_sponsored_instance();
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 10_000);

		let base = <Test as Config>::WeightInfo::load_recycler_with_external_asset();
		let charge = <Test as Config>::WeightInfo::charge_load_deposit();
		assert!(!charge.is_zero(), "the surcharge must be non-zero for this test to bite");

		// The declared weight prices the charge off the instance's mode: only a sponsored load
		// carries the surcharge; a privileged load pays only for the instance read that decides
		// it.
		let read = <Test as Config>::WeightInfo::read_instance();
		assert_ne!(charge, read, "distinct weights, otherwise the assertions are tautological");
		let declared_sponsored = load_call(instance_id).get_dispatch_info().call_weight;
		assert_eq!(declared_sponsored, base.saturating_add(charge));
		let declared_privileged = load_call(TEST_INSTANCE_ID).get_dispatch_info().call_weight;
		assert_eq!(declared_privileged, base.saturating_add(read));

		// Dispatch refunds nothing: the declared weight is already exact, whether or not the
		// sponsored load rotates.
		let post = try_load(instance_id, SPONSORED_ASSET_ID, 0).unwrap();
		assert_eq!(post.actual_weight, None);
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE + 1);
		let post = try_load(instance_id, SPONSORED_ASSET_ID, 1).unwrap();
		assert_eq!(post.actual_weight, None);
		assert!(old_tier(instance_id).is_some(), "the second load rotated");
		let post = try_load(TEST_INSTANCE_ID, TEST_ASSET_ID, 100).unwrap();
		assert_eq!(post.actual_weight, None);
	});
}

#[test]
fn only_a_privileged_unload_refunds_the_settlement_surcharge() {
	new_test_ext().execute_with(|| {
		let instance_id = setup_sponsored_instance();
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 10_000);

		let value: Denomination = 0;
		let (secrets, index, revision) =
			setup_recycler_for(instance_id, SPONSORED_ASSET_ID, value, 2, 0);
		let members = Coinage::get_recycler_members(instance_id, value, index);

		// A price change plus a load fills the old slot, so the settlement walks both tiers.
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE + 1);
		setup_recycler_for(instance_id, SPONSORED_ASSET_ID, value, 1, 50);
		assert!(old_tier(instance_id).is_some());

		let proven_msg = [7u8; 32];
		let (proof, alias) = create_unload_proof(&secrets[0], &members, &proven_msg);
		let declared = crate::Call::<Test>::unload_recycler_into_external_asset {
			instance_id,
			aliases: bounded_vec![alias],
			value,
			index,
			revision,
			to: 8_701,
			max_fee: 0,
		}
		.get_dispatch_info()
		.call_weight;

		let post = Coinage::unload_recycler_into_external_asset(
			Origin::UnloadToken {
				alias_proofs: bounded_vec![proof],
				proven_msg,
				fee: UnloadFee::Prepaid,
			}
			.into(),
			instance_id,
			bounded_vec![alias],
			value,
			index,
			revision,
			8_701,
			0,
		)
		.unwrap();

		// A sponsored unload settles, so it pays the surcharge its declared weight carries; what
		// it still refunds here is the unused `FromOutput` fee branch.
		let expected = Pallet::<Test>::unload_recycler_into_external_asset_prepaid_weight(1)
			.saturating_add(<Test as Config>::WeightInfo::settle_load_deposits());
		assert_eq!(post.actual_weight, Some(expected));
		assert!(post.actual_weight.unwrap().all_lte(declared));

		// The same unload on a privileged instance settles nothing, paying only for the instance
		// read that decides it.
		let (secrets, index, revision) = setup_recycler(value, 2, 60);
		let members = Coinage::get_recycler_members(TEST_INSTANCE_ID, value, index);
		let proven_msg = [8u8; 32];
		let (proof, alias) = create_unload_proof(&secrets[0], &members, &proven_msg);
		let post = Coinage::unload_recycler_into_external_asset(
			Origin::UnloadToken {
				alias_proofs: bounded_vec![proof],
				proven_msg,
				fee: UnloadFee::Prepaid,
			}
			.into(),
			TEST_INSTANCE_ID,
			bounded_vec![alias],
			value,
			index,
			revision,
			8_702,
			0,
		)
		.unwrap();
		assert_eq!(
			post.actual_weight,
			Some(
				Pallet::<Test>::unload_recycler_into_external_asset_prepaid_weight(1)
					.saturating_add(<Test as Config>::WeightInfo::read_instance())
			)
		);
		assert!(post.actual_weight.unwrap().all_lt(declared));
	});
}

#[test]
fn non_anonymous_unloads_declare_the_deposit_settlement() {
	new_test_ext().execute_with(|| {
		let instance_id = setup_sponsored_instance();
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 10_000);
		setup_balances();
		fund_native(FEE_DESTINATION, 1_000);

		let signer = ALICE;
		let to = CHARLIE;
		let settle = <Test as Config>::WeightInfo::settle_load_deposits();
		assert!(!settle.is_zero(), "the surcharge must be non-zero for this test to bite");

		// The singular call declares its own weight plus the settlement surcharge, and its
		// dispatch (delegating to the plural body) refunds nothing.
		let (input, proofs) =
			setup_non_anonymous_unload(instance_id, SPONSORED_ASSET_ID, signer, to, 70);
		let declared = crate::Call::<Test>::unload_recycler_into_external_asset_non_anonymous {
			instance_id,
			input: input.clone(),
			alias_proofs: proofs.clone(),
			to,
			fee_currency: FeeCurrency::Native,
			max_fee: native_max_fee_bound(),
		}
		.get_dispatch_info()
		.call_weight;
		assert_eq!(
			declared,
			Pallet::<Test>::unload_recycler_into_external_asset_non_anonymous_weight(1)
				.saturating_add(settle)
		);
		let post = Coinage::unload_recycler_into_external_asset_non_anonymous(
			RuntimeOrigin::signed(signer),
			instance_id,
			input,
			proofs,
			to,
			FeeCurrency::Native,
			native_max_fee_bound(),
		)
		.unwrap();
		assert_eq!(post.actual_weight, None);

		// The plural call declares against the plural benchmark the same way.
		let (input, proofs) =
			setup_non_anonymous_unload(instance_id, SPONSORED_ASSET_ID, signer, to, 71);
		let declared = crate::Call::<Test>::unload_recyclers_into_external_asset_non_anonymous {
			instance_id,
			inputs: bounded_vec![input.clone()],
			alias_proofs: proofs.clone(),
			to,
			fee_currency: FeeCurrency::Native,
			max_fee: native_max_fee_bound(),
		}
		.get_dispatch_info()
		.call_weight;
		assert_eq!(
			declared,
			Pallet::<Test>::unload_recyclers_into_external_asset_non_anonymous_weight(1)
				.saturating_add(settle)
		);
		let post = Coinage::unload_recyclers_into_external_asset_non_anonymous(
			RuntimeOrigin::signed(signer),
			instance_id,
			bounded_vec![input],
			proofs,
			to,
			FeeCurrency::Native,
			native_max_fee_bound(),
		)
		.unwrap();
		assert_eq!(post.actual_weight, None);
	});
}

// ============================================================================
// Load-deposit ledger and pot invariants
// ============================================================================

#[test]
fn charge_validation_skips_sufficient_instances_and_zero_key_loads() {
	new_test_ext().execute_with(|| {
		let instance_id = setup_sponsored_instance();
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);

		// A sufficient instance needs no pot, whatever the deposit says.
		assert!(Pallet::<Test>::ensure_can_charge_load_deposit(TEST_INSTANCE_ID, u32::MAX).is_ok());
		// Zero keys hold nothing, so the unfunded pot is irrelevant.
		assert!(Pallet::<Test>::ensure_can_charge_load_deposit(instance_id, 0).is_ok());
		// One key on the same unfunded pot is what fails.
		assert!(matches!(
			Pallet::<Test>::ensure_can_charge_load_deposit(instance_id, 1),
			Err(CustomInvalidity::PotCannotCoverLoadDeposit)
		));
	});
}

#[test]
fn lazy_rotation_records_only_loaded_at_prices() {
	new_test_ext().execute_with(|| {
		let instance_id = setup_sponsored_instance();
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE);
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 1_000);

		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 0));

		// Two changes with no load in between produce one tier, not two: no tier at 12 is
		// ever recorded.
		set_load_deposit(NATIVE_DEPOSIT_ID, 12);
		set_load_deposit(NATIVE_DEPOSIT_ID, 15);
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 1));

		assert_eq!(
			old_tier(instance_id),
			Some(DepositTier { asset_id: NATIVE_DEPOSIT_ID, price: PRICE, count: 1 })
		);
		assert_eq!(
			current_tier(instance_id),
			Some(DepositTier { asset_id: NATIVE_DEPOSIT_ID, price: 15, count: 1 })
		);
		check_load_deposit_invariant(instance_id, 2);
	});
}

#[test]
fn occupied_old_tier_refuses_rotating_loads_until_collapse() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let instance_id = setup_sponsored_instance();
		fund_pot(instance_id, NATIVE_DEPOSIT_ID, 10_000);

		// Two prices with a load each: the first tier is rotated into the old slot, filling it.
		for (i, price) in [PRICE, PRICE + 1].into_iter().enumerate() {
			set_load_deposit(NATIVE_DEPOSIT_ID, price);
			assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, i as u64));
		}
		assert_eq!(
			old_tier(instance_id),
			Some(DepositTier { asset_id: NATIVE_DEPOSIT_ID, price: PRICE, count: 1 })
		);
		check_load_deposit_invariant(instance_id, 2);

		// The next change makes any rotating load invalid, with the ledger untouched.
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE + 2);
		assert!(matches!(
			Pallet::<Test>::ensure_can_charge_load_deposit(instance_id, 1),
			Err(CustomInvalidity::LoadDepositOldTierOccupied)
		));
		let err = try_load(instance_id, SPONSORED_ASSET_ID, 10).unwrap_err();
		assert_eq!(err.error, Error::<Test>::LoadDepositOldTierOccupied.into());
		check_load_deposit_invariant(instance_id, 2);

		// A load at the unchanged price still succeeds: no rotation, so the occupied slot is
		// irrelevant.
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE + 1);
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 11));
		check_load_deposit_invariant(instance_id, 3);

		// Collapse reduces the whole ledger to one tier and loads resume at the new price.
		set_load_deposit(NATIVE_DEPOSIT_ID, PRICE + 2);
		assert_ok!(Coinage::collapse_load_deposits(RuntimeOrigin::signed(BOB), instance_id));
		assert!(old_tier(instance_id).is_none());
		assert_eq!(
			current_tier(instance_id),
			Some(DepositTier { asset_id: NATIVE_DEPOSIT_ID, price: PRICE + 2, count: 3 })
		);
		check_load_deposit_invariant(instance_id, 3);
		assert_ok!(try_load(instance_id, SPONSORED_ASSET_ID, 12));
		check_load_deposit_invariant(instance_id, 4);
	});
}

#[test]
fn pot_and_mode_calls_reject_a_missing_instance() {
	new_test_ext().execute_with(|| {
		let missing = 9_999;
		fund_native(SPONSOR, 1_000);

		assert_noop!(
			Coinage::fund_pot(RuntimeOrigin::signed(SPONSOR), missing, NATIVE_DEPOSIT_ID, 100),
			Error::<Test>::InstanceNotFound
		);
		assert_noop!(
			Coinage::collapse_load_deposits(RuntimeOrigin::signed(SPONSOR), missing),
			Error::<Test>::InstanceNotFound
		);
		assert_noop!(
			Coinage::make_instance_sufficient(RuntimeOrigin::root(), missing),
			Error::<Test>::InstanceNotFound
		);
		assert_noop!(
			Coinage::make_instance_sponsored(RuntimeOrigin::root(), missing),
			Error::<Test>::InstanceNotFound
		);

		// Withdrawal needs no instance, only a record, so a missing instance reads as an absent
		// contribution.
		assert_noop!(
			Coinage::withdraw_pot_funds(
				RuntimeOrigin::signed(SPONSOR),
				missing,
				NATIVE_DEPOSIT_ID,
				1
			),
			Error::<Test>::WithdrawExceedsContribution
		);

		// The validity-side check reports the missing instance to the pool.
		assert!(matches!(
			Pallet::<Test>::ensure_can_charge_load_deposit(missing, 1),
			Err(CustomInvalidity::InstanceNotFound)
		));
	});
}

// ============================================================================
// Extension variant x pallet call coverage for the load-deposit functions
//
// Every transaction extension variant and pallet call combination that touches
// `ensure_can_charge_load_deposit` (validation), `charge_load_deposit` or
// `settle_load_deposits` (dispatch) are tested in their own respective files.
//
// Charge side (invalid while the deposit cannot be collateralized, charged once it can):
// - `None` x `load_recycler_with_external_asset`: test_load_recycler_with_external_asset.rs,
//   `broke_pot_blocks_sponsored_loads_in_validation_and_dispatch` and
//   `sponsored_load_charges_the_pot_never_the_user`.
// - `AsCoin` x `load_recycler_with_coin`: test_load_recycler.rs,
//   `sponsored_load_with_coin_requires_and_charges_the_load_deposit`.
// - `InfallibleUnpaidSigned` x `load_recycler_with_external_asset_unpaid`:
//   test_infallible_unpaid_ext.rs, `sponsored_unpaid_load_requires_and_charges_the_load_deposit`.
// - `InfallibleUnpaidSigned` x `load_recycler_with_external_asset_unpaid_batch`:
//   test_infallible_unpaid_ext.rs, `sponsored_unpaid_batch_load_charges_the_deposit_per_item`.
// - `AsUnloadToken{People,LitePeople,Paid,FromOutput}` x
//   `unload_recycler_into_external_asset_and_loaded_coins` (charge and settle in one call):
//   test_unload_recycler_into_external_asset_and_loaded_coins.rs,
//   `sponsored_mixed_output_*_deposit_flow`.
//
// Settle side (deposit released on unload, or at the cleanup of the key's expired recycler when it
// was never unloaded; a switch to sufficient releases the rest and later unloads of
// sponsored-loaded keys settle nothing without failing):
// - `AsUnloadToken{People,LitePeople,Paid}` x `unload_recycler_into_coin`:
//   test_unload_recycler_into_coin.rs, `sponsored_unload_into_coin_settles_the_load_deposit_*`.
// - `AsUnloadToken{People,LitePeople,Paid}` x `unload_recycler_into_external_asset`:
//   test_unload_recycler_into_external_asset.rs, `sponsored_unload_settles_the_load_deposit_*`.
// - `AsUnloadTokenFromOutput` x `unload_recycler_into_external_asset`:
//   test_unload_recycler_into_external_asset_fee_from_output.rs,
//   `sponsored_from_output_unload_settles_the_load_deposit`.
// - `AsUnloadToken{People,LitePeople,Paid,FromOutput}` x `unload_recycler_into_coins`:
//   test_unload_recycler_into_coins.rs, `sponsored_unload_into_coins_settles_the_load_deposit_*`.
// - `None` x `unload_recycler_into_external_asset_non_anonymous` and
//   `unload_recyclers_into_external_asset_non_anonymous`:
//   test_unload_recycler_into_external_asset_non_anonymous_fee_from_signer.rs,
//   `sponsored_non_anonymous_unload_settles_the_load_deposit` and
//   `sponsored_non_anonymous_multi_unload_settles_the_load_deposit`.
// - `None` x `clean_recycler` (the archived keys' deposits, settled on cleanup, and the recovery
//   through `unload_archived_recycler_into_external_asset` settling nothing on top):
//   test_unload_recycler_into_external_asset.rs,
//   `sponsored_ring_lifecycle_settles_oldest_first_and_at_archival`.
// ============================================================================
