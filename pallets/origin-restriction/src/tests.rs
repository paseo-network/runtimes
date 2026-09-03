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
use frame_support::{assert_noop, assert_ok};
use sp_runtime::{testing::UintAuthorityId, transaction_validity::InvalidTransaction};

/// Test that a non-restricted origin (`NON_RESTRICTED_ORIGIN`) is never tracked, i.e., no usage.
#[test]
fn non_restricted_origin_is_not_charged() {
	new_test_ext().execute_with(|| {
		advance_by(1);

		assert_ok!(exec_signed_tx(NON_RESTRICTED_ORIGIN, MockPalletCall::do_something {}));

		assert!(
			Usages::<Test>::iter().next().is_none(),
			"Non-restricted origin should have no tracked usage."
		);
	});
}

/// Test that restricted origins (`RESTRICTED_ORIGIN_1`, `RESTRICTED_ORIGIN_2`) have their usage
/// tracked, refunded on Pays::No, and also can exceed the limit one time if the call is
/// whitelisted.
#[test]
fn restricted_origin_works() {
	new_test_ext().execute_with(|| {
		// length of the extrinsic.
		let len = {
			let tx_ext = (RestrictOrigin::<Test>::new(true),);
			let tx = UncheckedExtrinsic::new_signed(
				MockPalletCall::do_something {}.into(),
				RESTRICTED_ORIGIN_1,
				UintAuthorityId(RESTRICTED_ORIGIN_1),
				tx_ext,
			);
			tx.encoded_size() as u64
		};

		let mut previous_used = 0;

		assert_eq!(ALLOWANCE_RECOVERY_PER_BLOCK, 5);
		assert_eq!(CALL_WEIGHT, 15);
		assert_eq!(MAX_ALLOWANCE, 124);

		// Move beyond block 0 for events
		advance_by(1);

		// Normal call => usage increases
		assert_ok!(exec_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something {}));
		let usage = Usages::<Test>::get(RuntimeRestrictedEntity::A).unwrap();
		assert_eq!(usage.used, previous_used + CALL_WEIGHT + len);
		assert_eq!(usage.at_block, RELAY_BLOCK_GENESIS + 1);

		// A call with `Pays::No` => usage is refunded
		previous_used = Usages::<Test>::get(RuntimeRestrictedEntity::A).unwrap().used;
		assert_ok!(exec_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something_refunded {}));
		let usage_after = Usages::<Test>::get(RuntimeRestrictedEntity::A).unwrap();
		assert_eq!(usage_after.used, previous_used);

		// Again a normal call => usage increases
		assert_ok!(exec_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something {}));
		let usage = Usages::<Test>::get(RuntimeRestrictedEntity::A).unwrap();
		assert_eq!(usage.used, previous_used + CALL_WEIGHT + len);

		// A call with `Pays::yes` => actual weight is less than estimated
		previous_used = Usages::<Test>::get(RuntimeRestrictedEntity::A).unwrap().used;
		assert_ok!(exec_signed_tx(
			RESTRICTED_ORIGIN_1,
			MockPalletCall::do_something_with_less_actual_weight {}
		));
		let usage = Usages::<Test>::get(RuntimeRestrictedEntity::A).unwrap();
		assert_eq!(usage.used, previous_used + CALL_WEIGHT_LESS + len);

		// Now we have reached the limit
		// Normal calls that push usage above the max should fail.
		assert_noop!(
			exec_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something {}),
			InvalidTransaction::Payment
		);

		// Advance a few blocks to partially recover usage
		advance_by(1);
		// Still not enough to do another normal call if we haven't recovered enough.
		assert_noop!(
			exec_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something {}),
			InvalidTransaction::Payment
		);

		// Advance one more block => total 5 blocks.
		advance_by(1);
		previous_used = Usages::<Test>::get(RuntimeRestrictedEntity::A).unwrap().used;
		assert_ok!(exec_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something {}));
		let current_usage = Usages::<Test>::get(RuntimeRestrictedEntity::A).unwrap();
		let recovered_amount = 2 * ALLOWANCE_RECOVERY_PER_BLOCK;

		// Usage = (previous_used - recovered_amount) + (CALL_WEIGHT + len).
		assert_eq!(current_usage.used, previous_used + CALL_WEIGHT + len - recovered_amount);
		assert_eq!(current_usage.at_block, RELAY_BLOCK_GENESIS + 3);
	});
}

/// Storage mutation happens in `prepare`, not `validate`. Running `validate` on its own must
/// leave `Usages` untouched; the usage is only recorded once the full pipeline runs `prepare`.
#[test]
fn usage_is_mutated_in_prepare_not_validate() {
	new_test_ext().execute_with(|| {
		advance_by(1);

		// Running only `validate` for a restricted origin must not write any usage.
		assert_ok!(validate_only_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something {}));
		assert!(
			Usages::<Test>::get(RuntimeRestrictedEntity::A).is_none(),
			"`validate` must not mutate storage; the write belongs in `prepare`."
		);

		// The full pipeline runs `prepare`, which records the usage.
		assert_ok!(exec_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something {}));
		assert!(
			Usages::<Test>::get(RuntimeRestrictedEntity::A).is_some(),
			"`prepare` must record the usage."
		);
	});
}

#[test]
fn one_time_excess_works_and_works_only_one_time() {
	new_test_ext().execute_with(|| {
		advance_by(1);

		// Given usage is 0, RESTRICTED_ORIGIN_1 can exceed once.
		assert_ok!(exec_signed_tx(
			RESTRICTED_ORIGIN_1,
			MockPalletCall::do_something_allowed_excess {}
		));
		let current_usage = Usages::<Test>::get(RuntimeRestrictedEntity::A).unwrap();
		assert!(current_usage.used > MAX_ALLOWANCE);

		// Now that usage has exceeded the max, even the "allowed excess" call should fail.
		assert_noop!(
			exec_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something_allowed_excess {}),
			InvalidTransaction::Payment
		);
	});
}

#[test]
fn one_time_excess_is_origin_specific() {
	new_test_ext().execute_with(|| {
		advance_by(1);

		// try the "allowed_excess" call from RESTRICTED_ORIGIN_2:
		// It should *fail*, because RESTRICTED_ORIGIN_2 is not in `OperationAllowedOneTimeExcess`.
		assert_noop!(
			exec_signed_tx(RESTRICTED_ORIGIN_2, MockPalletCall::do_something_allowed_excess {}),
			InvalidTransaction::Payment
		);

		// Demonstrate that RESTRICTED_ORIGIN_1 *can* exceed (for completeness).
		assert_ok!(exec_signed_tx(
			RESTRICTED_ORIGIN_1,
			MockPalletCall::do_something_allowed_excess {}
		));
		let current_usage = Usages::<Test>::get(RuntimeRestrictedEntity::A).unwrap();
		assert!(current_usage.used > MAX_ALLOWANCE);
	});
}

#[test]
fn one_time_excess_requires_usage_zero() {
	new_test_ext().execute_with(|| {
		advance_by(1);

		// We use a bit of the allowance.
		assert_ok!(exec_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something {}));
		let usage = Usages::<Test>::get(RuntimeRestrictedEntity::A).unwrap();
		assert!(usage.used < MAX_ALLOWANCE);
		assert!(usage.used > 0);

		// Now that usage is non-zero, we can call exceeding operations.
		assert_noop!(
			exec_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something_allowed_excess {}),
			InvalidTransaction::Payment
		);
	});
}

#[test]
fn clean_usage_works() {
	new_test_ext().execute_with(|| {
		// Move beyond block 0 for clarity in block numbering.
		advance_by(1);

		// 1) Attempt to clean usage with no recorded usage => should fail with NoUsage.
		assert_noop!(
			OriginsRestriction::clean_usage(
				frame_system::RawOrigin::Root.into(),
				RuntimeRestrictedEntity::A
			),
			Error::<Test>::NoUsage
		);

		// Create some usage for RESTRICTED_ORIGIN_1 (which maps to RuntimeRestrictedEntity::A).
		assert_ok!(exec_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something {}));
		let usage = Usages::<Test>::get(RuntimeRestrictedEntity::A).expect("Usage must be present");
		assert!(usage.used > 0, "Usage should have increased after the call");

		// 2) Try cleaning while usage is non-zero => should fail with NotZero.
		assert_noop!(
			OriginsRestriction::clean_usage(
				frame_system::RawOrigin::Root.into(),
				RuntimeRestrictedEntity::A
			),
			Error::<Test>::NotZero
		);

		// Figure out how many blocks to advance so that usage recovers fully back to zero.
		// The usage recovers ALLOWANCE_RECOVERY_PER_BLOCK every block, so compute the needed
		// blocks.
		let used_amount = usage.used;
		let blocks_needed = used_amount.div_ceil(ALLOWANCE_RECOVERY_PER_BLOCK); // Ceiling division

		advance_by(blocks_needed);

		// 3) Now that enough blocks have passed, usage should be zero => clean_usage should
		//    succeed.
		assert_ok!(OriginsRestriction::clean_usage(
			frame_system::RawOrigin::Root.into(),
			RuntimeRestrictedEntity::A
		));

		// We expect the storage to be removed and the UsageCleaned event to be emitted.
		assert!(Usages::<Test>::get(RuntimeRestrictedEntity::A).is_none());
		System::assert_last_event(RuntimeEvent::OriginsRestriction(Event::UsageCleaned {
			entity: RuntimeRestrictedEntity::A,
		}));

		// 4) Calling again when there is no usage => fail with NoUsage.
		assert_noop!(
			OriginsRestriction::clean_usage(
				frame_system::RawOrigin::Root.into(),
				RuntimeRestrictedEntity::A
			),
			Error::<Test>::NoUsage
		);
	});
}

#[test]
fn restrict_origin_extension_disabled_behavior() {
	new_test_ext().execute_with(|| {
		// Move to a non-zero block number for clarity.
		advance_by(1);

		// 1) Attempt from restricted origin => Expect InvalidTransaction::Call
		// because the pallet explicitly forbids restricted origins if the extension is off.
		assert_noop!(
			exec_signed_tx_disabled(RESTRICTED_ORIGIN_1, MockPalletCall::do_something {}),
			sp_runtime::transaction_validity::InvalidTransaction::Call
		);

		// 2) Attempt from non-restricted origin => Should succeed and also
		// should not track any usage since usage is only tracked for restricted origins.
		assert_ok!(exec_signed_tx_disabled(NON_RESTRICTED_ORIGIN, MockPalletCall::do_something {}));
		assert!(
			Usages::<Test>::iter().next().is_none(),
			"Extension is disabled, so no usage should be tracked for non-restricted origins."
		);
	});
}

/// Usage recovers with the relay chain block number. Parachain blocks alone recover nothing.
#[test]
fn usage_recovers_with_relay_chain_blocks() {
	new_test_ext().execute_with(|| {
		advance_by(1);

		assert_ok!(exec_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something {}));
		let used = Usages::<Test>::get(RuntimeRestrictedEntity::A).unwrap().used;
		let relay_blocks_to_recover = used.div_ceil(ALLOWANCE_RECOVERY_PER_BLOCK);

		// Parachain blocks on their own leave the usage untouched.
		advance_para_by(relay_blocks_to_recover * 10);
		assert_noop!(
			OriginsRestriction::clean_usage(
				frame_system::RawOrigin::Root.into(),
				RuntimeRestrictedEntity::A
			),
			Error::<Test>::NotZero
		);

		// The same count of relay chain blocks recovers the whole usage.
		advance_relay_by(relay_blocks_to_recover);
		assert_ok!(OriginsRestriction::clean_usage(
			frame_system::RawOrigin::Root.into(),
			RuntimeRestrictedEntity::A
		));
	});
}

/// The usage is stamped with the relay chain block number, not the parachain one.
#[test]
fn usage_is_stamped_with_the_relay_chain_block_number() {
	new_test_ext().execute_with(|| {
		advance_by(1);
		advance_relay_by(7);

		assert_ok!(exec_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something {}));

		let usage = Usages::<Test>::get(RuntimeRestrictedEntity::A).unwrap();
		assert_eq!(usage.at_block, RELAY_BLOCK_GENESIS + 8);
	});
}
