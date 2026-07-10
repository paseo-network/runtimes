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

//! Tests for the alias-accounts pallet.

use crate::{
	extension::{AsRingAlias, AsRingAliasInfo, CustomValidity, Val},
	mock::*,
	origin::*,
	pallet::{AccountToAlias, AliasFee, AliasToAccount, Origin},
};
use frame_support::{assert_noop, assert_ok, pallet_prelude::TransactionSource, parameter_types};
use indiv_support::traits::{Alias, Context, ContextualAlias, Identifier};
use sp_runtime::{
	traits::{DispatchInfoOf, Implication, TransactionExtension, TxBaseImplication},
	transaction_validity::TransactionValidityError,
	DispatchError,
};

const ALICE: u64 = 1;
const BOB: u64 = 2;
const CHARLIE: u64 = 3;
const ALIAS_A: Alias = [10u8; 32];
const ALIAS_B: Alias = [20u8; 32];

fn make_valid_proof(alias: Alias) -> MockProof {
	MockProof { alias, valid: true }
}

fn make_invalid_proof() -> MockProof {
	MockProof { alias: [0u8; 32], valid: false }
}

// ========== set_alias_account tests ==========

mod set_alias_account {
	use frame_support::traits::UnixTime;

	use super::*;

	const CUSTOM_CONTEXT: Context = [42u8; 32];
	const PAID_FEE: u64 = 100;

	#[test]
	fn succeeds_with_custom_context() {
		new_test_ext().execute_with(|| {
			AliasFee::<Test>::put(PAID_FEE);
			setup_pgas_for(ALICE, 1_000);

			assert_ok!(AliasAccounts::set_alias_account(
				RuntimeOrigin::signed(ALICE),
				make_valid_proof(ALIAS_A),
				PeopleCollection::get(),
				0,
				1,
				CUSTOM_CONTEXT,
				MOCK_GENESIS_TIME
			));

			let info = AccountToAlias::<Test>::get(ALICE).unwrap();
			assert_eq!(info.ca.context, CUSTOM_CONTEXT);
			assert_eq!(info.ca.alias, ALIAS_A);
			assert_eq!(
				AliasToAccount::<Test>::get(
					PeopleCollection::get(),
					ContextualAlias { alias: ALIAS_A, context: CUSTOM_CONTEXT }
				),
				Some(ALICE)
			);
			// Fee was burned.
			assert_eq!(pgas_balance(ALICE), 1_000 - PAID_FEE);
		});
	}

	#[test]
	fn rejects_invalid_collection() {
		new_test_ext().execute_with(|| {
			AliasFee::<Test>::put(PAID_FEE);
			setup_pgas_for(ALICE, 1_000);

			assert_noop!(
				AliasAccounts::set_alias_account(
					RuntimeOrigin::signed(ALICE),
					make_valid_proof(ALIAS_A),
					INVALID_COLLECTION,
					0,
					1,
					CUSTOM_CONTEXT,
					MOCK_GENESIS_TIME,
				),
				crate::Error::<Test>::InvalidCollection
			);
		});
	}

	#[test]
	fn fails_when_fee_unset() {
		new_test_ext().execute_with(|| {
			setup_pgas_for(ALICE, 1_000);

			assert_noop!(
				AliasAccounts::set_alias_account(
					RuntimeOrigin::signed(ALICE),
					make_valid_proof(ALIAS_A),
					PeopleCollection::get(),
					0,
					1,
					CUSTOM_CONTEXT,
					MOCK_GENESIS_TIME,
				),
				crate::Error::<Test>::AliasFeeUnset
			);
		});
	}

	#[test]
	fn fails_with_invalid_proof() {
		new_test_ext().execute_with(|| {
			AliasFee::<Test>::put(PAID_FEE);
			setup_pgas_for(ALICE, 1_000);

			assert_noop!(
				AliasAccounts::set_alias_account(
					RuntimeOrigin::signed(ALICE),
					make_invalid_proof(),
					PeopleCollection::get(),
					0,
					1,
					CUSTOM_CONTEXT,
					MOCK_GENESIS_TIME,
				),
				crate::Error::<Test>::BadProof
			);
		});
	}

	#[test]
	fn fails_with_insufficient_pgas() {
		new_test_ext().execute_with(|| {
			AliasFee::<Test>::put(PAID_FEE);
			setup_pgas_for(ALICE, PAID_FEE - 1);

			assert!(AliasAccounts::set_alias_account(
				RuntimeOrigin::signed(ALICE),
				make_valid_proof(ALIAS_A),
				PeopleCollection::get(),
				0,
				1,
				CUSTOM_CONTEXT,
				MOCK_GENESIS_TIME,
			)
			.is_err());

			assert!(AccountToAlias::<Test>::get(ALICE).is_none());
			// No PGAS was burned — partial burn must not be possible.
			assert_eq!(pgas_balance(ALICE), PAID_FEE - 1);
		});
	}

	#[test]
	fn rejects_when_signer_already_holds_a_different_alias() {
		new_test_ext().execute_with(|| {
			AliasFee::<Test>::put(PAID_FEE);
			setup_pgas_for(ALICE, 10_000);

			// First mapping under custom context.
			assert_ok!(AliasAccounts::set_alias_account(
				RuntimeOrigin::signed(ALICE),
				make_valid_proof(ALIAS_A),
				PeopleCollection::get(),
				0,
				1,
				CUSTOM_CONTEXT,
				MOCK_GENESIS_TIME,
			));

			// Try to bind a *different* alias to ALICE → rejected.
			assert_noop!(
				AliasAccounts::set_alias_account(
					RuntimeOrigin::signed(ALICE),
					make_valid_proof(ALIAS_B),
					PeopleCollection::get(),
					0,
					1,
					CUSTOM_CONTEXT,
					MOCK_GENESIS_TIME,
				),
				crate::Error::<Test>::AccountInUse
			);
		});
	}

	#[test]
	fn rejects_replay_when_already_set() {
		new_test_ext().execute_with(|| {
			AliasFee::<Test>::put(PAID_FEE);
			setup_pgas_for(ALICE, 10_000);

			assert_ok!(AliasAccounts::set_alias_account(
				RuntimeOrigin::signed(ALICE),
				make_valid_proof(ALIAS_A),
				PeopleCollection::get(),
				0,
				1,
				CUSTOM_CONTEXT,
				MOCK_GENESIS_TIME,
			));

			assert_noop!(
				AliasAccounts::set_alias_account(
					RuntimeOrigin::signed(ALICE),
					make_valid_proof(ALIAS_A),
					PeopleCollection::get(),
					0,
					1,
					CUSTOM_CONTEXT,
					MOCK_GENESIS_TIME,
				),
				crate::Error::<Test>::AliasAccountAlreadySet
			);
		});
	}

	#[test]
	fn allows_account_swap_paid_again() {
		new_test_ext().execute_with(|| {
			AliasFee::<Test>::put(PAID_FEE);
			setup_pgas_for(ALICE, 10_000);
			setup_pgas_for(BOB, 10_000);

			assert_ok!(AliasAccounts::set_alias_account(
				RuntimeOrigin::signed(ALICE),
				make_valid_proof(ALIAS_A),
				PeopleCollection::get(),
				0,
				1,
				CUSTOM_CONTEXT,
				MOCK_GENESIS_TIME,
			));
			assert_eq!(pgas_balance(ALICE), 10_000 - PAID_FEE);

			// BOB takes over the same alias under the same context.
			assert_ok!(AliasAccounts::set_alias_account(
				RuntimeOrigin::signed(BOB),
				make_valid_proof(ALIAS_A),
				PeopleCollection::get(),
				0,
				1,
				CUSTOM_CONTEXT,
				MOCK_GENESIS_TIME,
			));
			assert_eq!(pgas_balance(BOB), 10_000 - PAID_FEE);
			// ALICE's PGAS balance is untouched by BOB's swap.
			assert_eq!(pgas_balance(ALICE), 10_000 - PAID_FEE);

			// Old ALICE mapping has been deleted
			assert!(AccountToAlias::<Test>::get(ALICE).is_none());

			// But new BOB mapping exists for ALICE Alias instead
			AccountToAlias::<Test>::get(BOB).expect("BOB should exist in mapping");
			assert_eq!(
				AliasToAccount::<Test>::get(
					PeopleCollection::get(),
					ContextualAlias { alias: ALIAS_A, context: CUSTOM_CONTEXT }
				),
				Some(BOB)
			);
		});
	}

	#[test]
	fn rejects_unsigned_origin() {
		new_test_ext().execute_with(|| {
			AliasFee::<Test>::put(PAID_FEE);

			assert_noop!(
				AliasAccounts::set_alias_account(
					RuntimeOrigin::none(),
					make_valid_proof(ALIAS_A),
					PeopleCollection::get(),
					0,
					1,
					CUSTOM_CONTEXT,
					MOCK_GENESIS_TIME,
				),
				DispatchError::BadOrigin
			);
		});
	}

	#[test]
	fn set_alias_in_new_revision_charges_fee() {
		new_test_ext().execute_with(|| {
			AliasFee::<Test>::put(PAID_FEE);
			setup_pgas_for(ALICE, 10_000);

			assert_ok!(AliasAccounts::set_alias_account(
				RuntimeOrigin::signed(ALICE),
				make_valid_proof(ALIAS_A),
				PeopleCollection::get(),
				0,
				1,
				CUSTOM_CONTEXT,
				MOCK_GENESIS_TIME,
			));
			assert_eq!(pgas_balance(ALICE), 10_000 - PAID_FEE);

			push_mock_ring_revision(PeopleCollection::get(), 0, 2);
			assert_ok!(AliasAccounts::set_alias_account(
				RuntimeOrigin::signed(ALICE),
				make_valid_proof(ALIAS_A),
				PeopleCollection::get(),
				0,
				2,
				CUSTOM_CONTEXT,
				MOCK_GENESIS_TIME,
			));
			// Fee charged again for the second registration.
			assert_eq!(pgas_balance(ALICE), 10_000 - 2 * PAID_FEE);

			let info = AccountToAlias::<Test>::get(ALICE)
				.expect("Alice alias should still exist in mapping");
			assert_eq!(info.revision, 2);
		});
	}

	#[test]
	fn succeeds_with_zero_fee() {
		new_test_ext().execute_with(|| {
			AliasFee::<Test>::put(0);
			// Deliberately do NOT fund ALICE with any PGAS.
			assert_eq!(pgas_balance(ALICE), 0);

			assert_ok!(AliasAccounts::set_alias_account(
				RuntimeOrigin::signed(ALICE),
				make_valid_proof(ALIAS_A),
				PeopleCollection::get(),
				0,
				1,
				CUSTOM_CONTEXT,
				MOCK_GENESIS_TIME,
			));

			assert!(AccountToAlias::<Test>::get(ALICE).is_some());
			assert_eq!(pgas_balance(ALICE), 0);
		});
	}

	#[test]
	fn fails_with_outdated_proof() {
		new_test_ext().execute_with(|| {
			AliasFee::<Test>::put(PAID_FEE);
			setup_pgas_for(ALICE, 1_000);

			let proof_valid_at = MOCK_GENESIS_TIME;
			set_mock_time(MOCK_GENESIS_TIME + ProofValidityWindow::get() + 1);
			let now = MockUnixTime::now().as_secs();

			assert!(now > proof_valid_at + ProofValidityWindow::get());

			assert_noop!(
				AliasAccounts::set_alias_account(
					RuntimeOrigin::signed(ALICE),
					make_valid_proof(ALIAS_A),
					PeopleCollection::get(),
					0,
					1,
					CUSTOM_CONTEXT,
					proof_valid_at,
				),
				crate::Error::<Test>::TimeOutOfRange
			);
		});
	}

	#[test]
	fn fails_with_future_proof() {
		new_test_ext().execute_with(|| {
			AliasFee::<Test>::put(PAID_FEE);
			setup_pgas_for(ALICE, 1_000);

			let proof_valid_at = MOCK_GENESIS_TIME + 10;
			let now = MockUnixTime::now().as_secs();
			assert!(now < proof_valid_at);

			assert_noop!(
				AliasAccounts::set_alias_account(
					RuntimeOrigin::signed(ALICE),
					make_valid_proof(ALIAS_A),
					PeopleCollection::get(),
					0,
					1,
					CUSTOM_CONTEXT,
					proof_valid_at,
				),
				crate::Error::<Test>::TimeOutOfRange
			);
		});
	}
}

// ========== unset_alias_account tests ==========

mod unset_alias_account {
	use super::*;

	fn seed_alias(account: u64, alias: Alias, context: Context) {
		let info = make_alias_info(alias, context);
		AccountToAlias::<Test>::insert(account, &info);
		AliasToAccount::<Test>::insert(
			PeopleCollection::get(),
			ContextualAlias { alias, context },
			account,
		);
		frame_system::Pallet::<Test>::inc_sufficients(&account);
	}

	#[test]
	fn succeeds_when_mapped() {
		new_test_ext().execute_with(|| {
			seed_alias(ALICE, ALIAS_A, PEOPLE_CONTEXT);

			assert_ok!(AliasAccounts::unset_alias_account(RuntimeOrigin::signed(ALICE)));

			assert!(AccountToAlias::<Test>::get(ALICE).is_none());
			assert!(AliasToAccount::<Test>::get(
				PeopleCollection::get(),
				ContextualAlias { alias: ALIAS_A, context: PEOPLE_CONTEXT }
			)
			.is_none());
		});
	}

	#[test]
	fn fails_when_not_mapped() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				AliasAccounts::unset_alias_account(RuntimeOrigin::signed(ALICE)),
				crate::Error::<Test>::InvalidAccount
			);
		});
	}

	#[test]
	fn fails_for_bad_origin() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				AliasAccounts::unset_alias_account(RuntimeOrigin::none()),
				DispatchError::BadOrigin
			);
		});
	}
}

// ========== clean_up_stale_alias tests ==========

mod clean_up_stale_alias {
	use super::*;

	fn seed_alias_at_rev(alias: Alias, context: Context, revision: u32, ring: u32) {
		let info = make_alias_info_for(PeopleCollection::get(), alias, context, revision, ring);
		AccountToAlias::<Test>::insert(ALICE, &info);
		AliasToAccount::<Test>::insert(
			PeopleCollection::get(),
			ContextualAlias { alias, context },
			ALICE,
		);
		frame_system::Pallet::<Test>::inc_sufficients(&ALICE);
	}

	#[test]
	fn succeeds_when_revision_stale_and_grace_elapsed() {
		new_test_ext().execute_with(|| {
			seed_alias_at_rev(ALIAS_A, PEOPLE_CONTEXT, 1, 0);
			assert!(AccountToAlias::<Test>::get(ALICE).is_some());

			// Ring revision advances. The stored mapping now points to an outdated revision.
			set_mock_ring_revision(PeopleCollection::get(), 0, 2);

			// Advance time past the cleanup grace period.
			set_mock_time(MOCK_GENESIS_TIME + CleanupGracePeriod::get() + 1);

			let ca = ContextualAlias { alias: ALIAS_A, context: PEOPLE_CONTEXT };
			assert_ok!(AliasAccounts::clean_up_stale_alias(
				RuntimeOrigin::signed(BOB),
				PeopleCollection::get(),
				ca,
			));

			assert!(AliasToAccount::<Test>::get(
				PeopleCollection::get(),
				ContextualAlias { alias: ALIAS_A, context: PEOPLE_CONTEXT }
			)
			.is_none());
			assert!(AccountToAlias::<Test>::get(ALICE).is_none());
		});
	}

	#[test]
	fn succeeds_when_ring_removed() {
		new_test_ext().execute_with(|| {
			seed_alias_at_rev(ALIAS_A, PEOPLE_CONTEXT, 1, 0);

			// Remove the ring root entirely
			remove_mock_ring_root(PeopleCollection::get(), 0);

			let ca = ContextualAlias { alias: ALIAS_A, context: PEOPLE_CONTEXT };
			assert_ok!(AliasAccounts::clean_up_stale_alias(
				RuntimeOrigin::signed(BOB),
				PeopleCollection::get(),
				ca,
			));

			assert!(AccountToAlias::<Test>::get(ALICE).is_none());
		});
	}

	#[test]
	fn fails_when_not_stale() {
		new_test_ext().execute_with(|| {
			seed_alias_at_rev(ALIAS_A, PEOPLE_CONTEXT, 1, 0);

			let ca = ContextualAlias { alias: ALIAS_A, context: PEOPLE_CONTEXT };
			assert_noop!(
				AliasAccounts::clean_up_stale_alias(
					RuntimeOrigin::signed(BOB),
					PeopleCollection::get(),
					ca,
				),
				crate::Error::<Test>::AliasNotStale
			);
		});
	}

	#[test]
	fn fails_when_no_mapping() {
		new_test_ext().execute_with(|| {
			let ca = ContextualAlias { alias: ALIAS_A, context: PEOPLE_CONTEXT };
			assert_noop!(
				AliasAccounts::clean_up_stale_alias(
					RuntimeOrigin::signed(BOB),
					PeopleCollection::get(),
					ca,
				),
				crate::Error::<Test>::InvalidAccount
			);
		});
	}

	#[test]
	fn fails_when_grace_period_not_elapsed() {
		new_test_ext().execute_with(|| {
			seed_alias_at_rev(ALIAS_A, PEOPLE_CONTEXT, 1, 0);

			// Ring revision advances
			set_mock_ring_revision(PeopleCollection::get(), 0, 2);

			// Don't advance time — still within grace period

			let ca = ContextualAlias { alias: ALIAS_A, context: PEOPLE_CONTEXT };
			assert_noop!(
				AliasAccounts::clean_up_stale_alias(
					RuntimeOrigin::signed(BOB),
					PeopleCollection::get(),
					ca,
				),
				crate::Error::<Test>::CleanupTooEarly
			);
		});
	}

	#[test]
	fn succeeds_when_stored_revision_is_newer() {
		new_test_ext().execute_with(|| {
			// Set up alias with revision 5.
			seed_alias_at_rev(ALIAS_A, PEOPLE_CONTEXT, 5, 0);

			// Set ring revision to 3 (lower than the stored 5), simulating a ring
			// index reuse after deletion (e.g. via merge) where the new ring starts
			// at a lower revision.
			set_mock_ring_revision(PeopleCollection::get(), 0, 3);

			// Advance time past grace period.
			set_mock_time(MOCK_GENESIS_TIME + CleanupGracePeriod::get() + 1);

			// Stored revision (5) != ring revision (3), so the alias is stale.
			let ca = ContextualAlias { alias: ALIAS_A, context: PEOPLE_CONTEXT };
			assert_ok!(AliasAccounts::clean_up_stale_alias(
				RuntimeOrigin::signed(BOB),
				PeopleCollection::get(),
				ca,
			));

			assert!(AccountToAlias::<Test>::get(ALICE).is_none());
		});
	}

	#[test]
	fn succeeds_when_old_revision_still_in_roots_but_grace_expired() {
		new_test_ext().execute_with(|| {
			// Alias created at revision 1 with source_time = MOCK_GENESIS_TIME.
			seed_alias_at_rev(ALIAS_A, PEOPLE_CONTEXT, 1, 0);

			// Ring revised to 2 at a later source_time, old revision 1 still in
			// BoundedVec.
			set_mock_time(MOCK_GENESIS_TIME + 5_000);
			push_mock_ring_revision(PeopleCollection::get(), 0, 2);

			// Past grace period for revision 1 but not revision 2 — cleanup too early.
			set_mock_time(MOCK_GENESIS_TIME + CleanupGracePeriod::get() + 1);
			let ca = ContextualAlias { alias: ALIAS_A, context: PEOPLE_CONTEXT };
			assert_noop!(
				AliasAccounts::clean_up_stale_alias(
					RuntimeOrigin::signed(BOB),
					PeopleCollection::get(),
					ca.clone(),
				),
				Error::<Test>::CleanupTooEarly
			);

			// Past grace for both revisions — cleanup succeeds.
			set_mock_time(MOCK_GENESIS_TIME + 5_000 + CleanupGracePeriod::get() + 1);
			assert_ok!(AliasAccounts::clean_up_stale_alias(
				RuntimeOrigin::signed(BOB),
				PeopleCollection::get(),
				ca,
			));

			assert!(AccountToAlias::<Test>::get(ALICE).is_none());
		});
	}

	/// End-to-end coverage: create a mapping via the real `set_alias_account`
	/// extrinsic (burning the PGAS fee, writing both directions of storage),
	/// then remove the ring root and clean it up. Distinct from the other
	/// tests in this module that seed storage directly.
	#[test]
	fn succeeds_when_ring_removed_after_real_setup() {
		new_test_ext().execute_with(|| {
			const CUSTOM_CONTEXT: Context = [42u8; 32];

			AliasFee::<Test>::put(100u64);
			setup_pgas_for(ALICE, 10_000);
			assert_ok!(AliasAccounts::set_alias_account(
				RuntimeOrigin::signed(ALICE),
				make_valid_proof(ALIAS_A),
				PeopleCollection::get(),
				0,
				1,
				CUSTOM_CONTEXT,
				MOCK_GENESIS_TIME,
			));
			assert!(AccountToAlias::<Test>::get(ALICE).is_some());
			assert_eq!(
				AliasToAccount::<Test>::get(
					PeopleCollection::get(),
					ContextualAlias { alias: ALIAS_A, context: CUSTOM_CONTEXT }
				),
				Some(ALICE)
			);

			// Ring root removed entirely (e.g. target of a merge) — cleanup is
			// allowed immediately.
			remove_mock_ring_root(PeopleCollection::get(), 0);

			let ca = ContextualAlias { alias: ALIAS_A, context: CUSTOM_CONTEXT };
			assert_ok!(AliasAccounts::clean_up_stale_alias(
				RuntimeOrigin::signed(BOB),
				PeopleCollection::get(),
				ca.clone(),
			));

			assert!(AccountToAlias::<Test>::get(ALICE).is_none());
			assert!(AliasToAccount::<Test>::get(PeopleCollection::get(), ca).is_none());
		});
	}
}

// ========== EnsureOrigin tests ==========

mod origin_guards {
	use super::*;
	use frame_support::traits::EnsureOrigin;

	parameter_types! {
		pub const TestCollection: Identifier = PeopleCollection::get();
		pub const TestContext: Context = PEOPLE_CONTEXT;
	}

	#[test]
	fn ensure_ring_alias_of_works() {
		new_test_ext().execute_with(|| {
			let info = make_alias_info(ALIAS_A, PEOPLE_CONTEXT);
			let origin: RuntimeOrigin = Origin::RingAlias(info).into();

			let alias = EnsureRingAliasOf::<Test, TestCollection>::try_origin(origin)
				.expect("should succeed");
			assert_eq!(alias, ALIAS_A);
		});
	}

	#[test]
	fn ensure_ring_alias_of_rejects_wrong_collection() {
		new_test_ext().execute_with(|| {
			let info = make_alias_info_for(INVALID_COLLECTION, ALIAS_A, PEOPLE_CONTEXT, 1, 0);
			let origin: RuntimeOrigin = Origin::RingAlias(info).into();

			let result = EnsureRingAliasOf::<Test, TestCollection>::try_origin(origin);
			assert!(result.is_err());
		});
	}

	#[test]
	fn ensure_ring_alias_in_context_works() {
		new_test_ext().execute_with(|| {
			let info = make_alias_info(ALIAS_A, PEOPLE_CONTEXT);
			let origin: RuntimeOrigin = Origin::RingAlias(info).into();

			let alias =
				EnsureRingAliasInContext::<Test, TestCollection, TestContext>::try_origin(origin)
					.expect("should succeed");
			assert_eq!(alias, ALIAS_A);
		});
	}

	#[test]
	fn ensure_ring_alias_in_context_rejects_wrong_context() {
		new_test_ext().execute_with(|| {
			let info = make_alias_info(ALIAS_A, INVALID_CONTEXT);
			let origin: RuntimeOrigin = Origin::RingAlias(info).into();

			let result =
				EnsureRingAliasInContext::<Test, TestCollection, TestContext>::try_origin(origin);
			assert!(result.is_err());
		});
	}

	#[test]
	fn ensure_ring_alias_returns_full_info() {
		new_test_ext().execute_with(|| {
			let info = make_alias_info(ALIAS_A, PEOPLE_CONTEXT);
			let origin: RuntimeOrigin = Origin::RingAlias(info.clone()).into();

			let result_info = EnsureRingAlias::<Test>::try_origin(origin).expect("should succeed");
			assert_eq!(result_info, info);
		});
	}
}

// ========== Transaction extension tests ==========

mod transaction_extension {
	use super::*;

	fn make_dispatch_info() -> DispatchInfoOf<RuntimeCall> {
		DispatchInfoOf::<RuntimeCall> {
			call_weight: frame_support::weights::Weight::zero(),
			extension_weight: frame_support::weights::Weight::zero(),
			class: frame_support::dispatch::DispatchClass::Normal,
			pays_fee: frame_support::dispatch::Pays::Yes,
		}
	}

	fn validate_extension(
		ext: &AsRingAlias<Test>,
		origin: RuntimeOrigin,
		call: &RuntimeCall,
		implication: &impl Implication,
	) -> Result<
		(sp_runtime::transaction_validity::ValidTransaction, Val<Test>, RuntimeOrigin),
		sp_runtime::transaction_validity::TransactionValidityError,
	> {
		ext.validate(
			origin,
			call,
			&make_dispatch_info(),
			0,
			(),
			implication,
			TransactionSource::External,
		)
	}

	fn passthrough_call() -> RuntimeCall {
		RuntimeCall::AliasAccounts(crate::Call::unset_alias_account {})
	}

	#[test]
	fn passthrough_when_not_using_extension() {
		new_test_ext().execute_with(|| {
			let ext = AsRingAlias::<Test>::none();
			let call = passthrough_call();
			let origin = RuntimeOrigin::none();
			let implication = TxBaseImplication(());

			let (_, val, _) =
				validate_extension(&ext, origin, &call, &implication).expect("should succeed");
			assert!(matches!(val, Val::NotUsing));
		});
	}

	// ---- WithAccount tests ----

	#[test]
	fn with_account_succeeds() {
		new_test_ext().execute_with(|| {
			// Set up alias mapping and ensure account exists
			let info = make_alias_info(ALIAS_A, PEOPLE_CONTEXT);
			AccountToAlias::<Test>::insert(ALICE, &info);
			frame_system::Pallet::<Test>::inc_sufficients(&ALICE);

			let ext = AsRingAlias::<Test>::new(Some(AsRingAliasInfo::WithAccount(0)));
			let call = RuntimeCall::AliasAccounts(crate::Call::unset_alias_account {});
			let origin = RuntimeOrigin::signed(ALICE);
			let implication = TxBaseImplication(());

			let (_, val, _) =
				validate_extension(&ext, origin, &call, &implication).expect("should succeed");
			assert!(matches!(val, Val::UsingAccount(..)));
		});
	}

	#[test]
	fn with_account_rejects_none_origin() {
		new_test_ext().execute_with(|| {
			let ext = AsRingAlias::<Test>::new(Some(AsRingAliasInfo::WithAccount(0)));
			let call = RuntimeCall::AliasAccounts(crate::Call::unset_alias_account {});
			let origin = RuntimeOrigin::none();
			let implication = TxBaseImplication(());

			let result = validate_extension(&ext, origin, &call, &implication);
			assert_eq!(
				result.unwrap_err(),
				TransactionValidityError::from(CustomValidity::OriginNotSigned)
			);
		});
	}

	#[test]
	fn with_account_rejects_without_mapping() {
		new_test_ext().execute_with(|| {
			let ext = AsRingAlias::<Test>::new(Some(AsRingAliasInfo::WithAccount(0)));
			let call = RuntimeCall::AliasAccounts(crate::Call::unset_alias_account {});
			let origin = RuntimeOrigin::signed(ALICE);
			let implication = TxBaseImplication(());

			let result = validate_extension(&ext, origin, &call, &implication);
			assert_eq!(
				result.unwrap_err(),
				TransactionValidityError::from(CustomValidity::NoAliasMapping)
			);
		});
	}

	#[test]
	fn with_account_rejects_stale_revision() {
		new_test_ext().execute_with(|| {
			// Set up alias mapping with revision 1
			let info = make_alias_info(ALIAS_A, PEOPLE_CONTEXT);
			AccountToAlias::<Test>::insert(ALICE, &info);
			frame_system::Pallet::<Test>::inc_sufficients(&ALICE);
			// But ring revision is now 2
			set_mock_ring_revision(PeopleCollection::get(), 0, 2);

			let ext = AsRingAlias::<Test>::new(Some(AsRingAliasInfo::WithAccount(0)));
			let call = RuntimeCall::AliasAccounts(crate::Call::unset_alias_account {});
			let origin = RuntimeOrigin::signed(ALICE);
			let implication = TxBaseImplication(());

			let result = validate_extension(&ext, origin, &call, &implication);
			assert_eq!(
				result.unwrap_err(),
				TransactionValidityError::from(CustomValidity::StaleRevision)
			);
		});
	}

	#[test]
	fn with_account_rejects_expired_grace_period_for_non_latest() {
		new_test_ext().execute_with(|| {
			// Alias mapping with revision 1, root source_time = MOCK_GENESIS_TIME.
			let info = make_alias_info(ALIAS_A, PEOPLE_CONTEXT);
			AccountToAlias::<Test>::insert(ALICE, &info);
			frame_system::Pallet::<Test>::inc_sufficients(&ALICE);

			// Pushing a newer revision so revision 1 is no longer the latest.
			push_mock_ring_revision_at(PeopleCollection::get(), 0, 2, MOCK_GENESIS_TIME + 1_000);

			// Advancing time past grace period for revision 1.
			set_mock_time(MOCK_GENESIS_TIME + CleanupGracePeriod::get() + 1);

			let ext = AsRingAlias::<Test>::new(Some(AsRingAliasInfo::WithAccount(0)));
			let call = RuntimeCall::AliasAccounts(crate::Call::unset_alias_account {});
			let origin = RuntimeOrigin::signed(ALICE);
			let implication = TxBaseImplication(());

			let result = validate_extension(&ext, origin, &call, &implication);
			assert_eq!(
				result.unwrap_err(),
				TransactionValidityError::from(CustomValidity::StaleRevision)
			);
		});
	}

	#[test]
	fn with_account_accepts_latest_revision_beyond_grace_period() {
		new_test_ext().execute_with(|| {
			// Alias mapping with revision 1 (the only and latest root).
			let info = make_alias_info(ALIAS_A, PEOPLE_CONTEXT);
			AccountToAlias::<Test>::insert(ALICE, &info);
			frame_system::Pallet::<Test>::inc_sufficients(&ALICE);

			// Advancing time past CleanupGracePeriod.
			set_mock_time(MOCK_GENESIS_TIME + CleanupGracePeriod::get() + 1);

			let ext = AsRingAlias::<Test>::new(Some(AsRingAliasInfo::WithAccount(0)));
			let call = RuntimeCall::AliasAccounts(crate::Call::unset_alias_account {});
			let origin = RuntimeOrigin::signed(ALICE);
			let implication = TxBaseImplication(());

			let (_, val, _) =
				validate_extension(&ext, origin, &call, &implication).expect("should succeed");
			assert!(matches!(val, Val::UsingAccount(..)));
		});
	}
}

// ========== Personhood lookup tests ==========

mod personhood_lookup {
	use super::*;
	use indiv_support::traits::PersonhoodLookup;

	#[test]
	fn returns_none_for_unregistered_account() {
		new_test_ext().execute_with(|| {
			assert!(AliasAccounts::personhood_info(&ALICE, &PEOPLE_CONTEXT).0.is_none());
		});
	}

	#[test]
	fn returns_some_for_registered_account_with_matching_context() {
		new_test_ext().execute_with(|| {
			let info = make_alias_info(ALIAS_A, PEOPLE_CONTEXT);
			AccountToAlias::<Test>::insert(ALICE, &info);

			let (result, _) = AliasAccounts::personhood_info(&ALICE, &PEOPLE_CONTEXT);
			assert_eq!(result, Some((PeopleCollection::get(), ALIAS_A)));
		});
	}

	#[test]
	fn returns_none_for_wrong_context() {
		new_test_ext().execute_with(|| {
			let info = make_alias_info(ALIAS_A, PEOPLE_CONTEXT);
			AccountToAlias::<Test>::insert(ALICE, &info);

			// Same account, different context — not found
			assert!(AliasAccounts::personhood_info(&ALICE, &PEOPLE_LITE_CONTEXT).0.is_none());
			assert!(AliasAccounts::personhood_info(&ALICE, &INVALID_CONTEXT).0.is_none());
		});
	}

	#[test]
	fn returns_some_for_non_latest_revision_within_grace() {
		new_test_ext().execute_with(|| {
			let info = make_alias_info(ALIAS_A, PEOPLE_CONTEXT);
			AccountToAlias::<Test>::insert(ALICE, &info);

			// New revision pushed but old one still in window and within grace.
			push_mock_ring_revision(PeopleCollection::get(), 0, 2);

			let (result, _) = AliasAccounts::personhood_info(&ALICE, &PEOPLE_CONTEXT);
			assert_eq!(result, Some((PeopleCollection::get(), ALIAS_A)));
		});
	}

	#[test]
	fn returns_none_for_non_latest_revision_past_grace() {
		new_test_ext().execute_with(|| {
			let info = make_alias_info(ALIAS_A, PEOPLE_CONTEXT);
			AccountToAlias::<Test>::insert(ALICE, &info);

			// New revision pushed and time advanced past the grace period.
			push_mock_ring_revision(PeopleCollection::get(), 0, 2);
			set_mock_time(MOCK_GENESIS_TIME + CleanupGracePeriod::get() + 1);

			let (result, _) = AliasAccounts::personhood_info(&ALICE, &PEOPLE_CONTEXT);
			assert!(result.is_none());
		});
	}

	#[test]
	fn returns_none_for_revision_evicted_from_window() {
		new_test_ext().execute_with(|| {
			let info = make_alias_info(ALIAS_A, PEOPLE_CONTEXT);
			AccountToAlias::<Test>::insert(ALICE, &info);

			// Replacing all roots with a new revision — old revision no longer in window.
			set_mock_ring_revision(PeopleCollection::get(), 0, 2);

			let (result, _) = AliasAccounts::personhood_info(&ALICE, &PEOPLE_CONTEXT);
			assert!(result.is_none());
		});
	}

	#[test]
	fn returns_none_for_removed_ring() {
		new_test_ext().execute_with(|| {
			let info = make_alias_info(ALIAS_A, PEOPLE_CONTEXT);
			AccountToAlias::<Test>::insert(ALICE, &info);

			remove_mock_ring_root(PeopleCollection::get(), 0);

			let (result, _) = AliasAccounts::personhood_info(&ALICE, &PEOPLE_CONTEXT);
			assert!(result.is_none());
		});
	}

	#[test]
	fn same_alias_different_contexts_are_independent() {
		new_test_ext().execute_with(|| {
			// Same alias value registered under PEOPLE_CONTEXT for ALICE
			let info_people = make_alias_info(ALIAS_A, PEOPLE_CONTEXT);
			AccountToAlias::<Test>::insert(ALICE, &info_people);

			// Same alias value registered under PEOPLE_LITE_CONTEXT for BOB
			set_mock_ring_revision(PeopleLiteCollection::get(), 0, 1);
			let info_lite = make_alias_info_for(
				PeopleLiteCollection::get(),
				ALIAS_A,
				PEOPLE_LITE_CONTEXT,
				1,
				0,
			);
			AccountToAlias::<Test>::insert(BOB, &info_lite);

			// ALICE is only found under PEOPLE_CONTEXT
			let (alice_people, _) = AliasAccounts::personhood_info(&ALICE, &PEOPLE_CONTEXT);
			assert_eq!(alice_people, Some((PeopleCollection::get(), ALIAS_A)));
			assert!(AliasAccounts::personhood_info(&ALICE, &PEOPLE_LITE_CONTEXT).0.is_none());

			// BOB is only found under PEOPLE_LITE_CONTEXT
			let (bob_lite, _) = AliasAccounts::personhood_info(&BOB, &PEOPLE_LITE_CONTEXT);
			assert_eq!(bob_lite, Some((PeopleLiteCollection::get(), ALIAS_A)));
			assert!(AliasAccounts::personhood_info(&BOB, &PEOPLE_CONTEXT).0.is_none());
		});
	}

	#[test]
	fn different_accounts_same_context_are_independent() {
		new_test_ext().execute_with(|| {
			// ALICE and BOB both registered under PEOPLE_CONTEXT with different aliases
			let info_alice = make_alias_info(ALIAS_A, PEOPLE_CONTEXT);
			AccountToAlias::<Test>::insert(ALICE, &info_alice);

			let info_bob = make_alias_info(ALIAS_B, PEOPLE_CONTEXT);
			AccountToAlias::<Test>::insert(BOB, &info_bob);

			// Both found, independently
			assert_eq!(
				AliasAccounts::personhood_info(&ALICE, &PEOPLE_CONTEXT).0,
				Some((PeopleCollection::get(), ALIAS_A))
			);
			assert_eq!(
				AliasAccounts::personhood_info(&BOB, &PEOPLE_CONTEXT).0,
				Some((PeopleCollection::get(), ALIAS_B))
			);

			// Unregistered account returns None
			assert!(AliasAccounts::personhood_info(&CHARLIE, &PEOPLE_CONTEXT).0.is_none());
		});
	}
}

// ========== Personhood lookup by proof tests ==========

mod personhood_lookup_by_proof {
	use super::*;
	use indiv_support::traits::{PersonhoodLookup, PersonhoodProofRequest};

	const RING: u32 = 0;
	const REVISION: u32 = 1;
	const MSG: &[u8] = b"test-msg";

	fn request(
		identifier: Identifier,
		proof: MockProof,
		alias: Alias,
		context: Context,
		revision: u32,
	) -> PersonhoodProofRequest<'static, MockProof> {
		PersonhoodProofRequest {
			identifier,
			proof,
			alias,
			ring_index: RING,
			context,
			revision,
			message: MSG,
		}
	}

	#[test]
	fn succeeds_for_valid_proof_against_people_ring() {
		new_test_ext().execute_with(|| {
			seed_collection_exponent(PeopleCollection::get());

			let (matched, _) = AliasAccounts::personhood_info_by_proof(request(
				PeopleCollection::get(),
				make_valid_proof(ALIAS_A),
				ALIAS_A,
				PEOPLE_CONTEXT,
				REVISION,
			));

			assert!(matched);
		});
	}

	#[test]
	fn succeeds_for_valid_proof_against_lite_ring() {
		new_test_ext().execute_with(|| {
			set_mock_ring_revision(PeopleLiteCollection::get(), RING, REVISION);
			seed_collection_exponent(PeopleLiteCollection::get());

			let (matched, _) = AliasAccounts::personhood_info_by_proof(request(
				PeopleLiteCollection::get(),
				make_valid_proof(ALIAS_A),
				ALIAS_A,
				PEOPLE_LITE_CONTEXT,
				REVISION,
			));

			assert!(matched);
		});
	}

	#[test]
	fn returns_false_when_proof_belongs_to_other_collection() {
		new_test_ext().execute_with(|| {
			// Only the Lite ring exists; remove the People ring set up by `new_test_ext`.
			remove_mock_ring_root(PeopleCollection::get(), RING);
			set_mock_ring_revision(PeopleLiteCollection::get(), RING, REVISION);
			seed_collection_exponent(PeopleLiteCollection::get());

			let (matched, _) = AliasAccounts::personhood_info_by_proof(request(
				PeopleCollection::get(),
				make_valid_proof(ALIAS_A),
				ALIAS_A,
				PEOPLE_CONTEXT,
				REVISION,
			));

			assert!(!matched);
		});
	}

	#[test]
	fn returns_false_for_invalid_proof() {
		new_test_ext().execute_with(|| {
			seed_collection_exponent(PeopleCollection::get());

			let (matched, _) = AliasAccounts::personhood_info_by_proof(request(
				PeopleCollection::get(),
				make_invalid_proof(),
				ALIAS_A,
				PEOPLE_CONTEXT,
				REVISION,
			));

			assert!(!matched);
		});
	}

	#[test]
	fn returns_false_when_derived_alias_does_not_match_claim() {
		new_test_ext().execute_with(|| {
			seed_collection_exponent(PeopleCollection::get());

			let (matched, _) = AliasAccounts::personhood_info_by_proof(request(
				PeopleCollection::get(),
				make_valid_proof(ALIAS_A),
				ALIAS_B,
				PEOPLE_CONTEXT,
				REVISION,
			));

			assert!(!matched);
		});
	}

	#[test]
	fn returns_false_when_collection_has_no_ring_root() {
		new_test_ext().execute_with(|| {
			remove_mock_ring_root(PeopleCollection::get(), RING);
			seed_collection_exponent(PeopleCollection::get());

			let (matched, _) = AliasAccounts::personhood_info_by_proof(request(
				PeopleCollection::get(),
				make_valid_proof(ALIAS_A),
				ALIAS_A,
				PEOPLE_CONTEXT,
				REVISION,
			));

			assert!(!matched);
		});
	}

	#[test]
	fn returns_false_when_revision_not_in_roots() {
		new_test_ext().execute_with(|| {
			seed_collection_exponent(PeopleCollection::get());

			let (matched, _) = AliasAccounts::personhood_info_by_proof(request(
				PeopleCollection::get(),
				make_valid_proof(ALIAS_A),
				ALIAS_A,
				PEOPLE_CONTEXT,
				REVISION + 5,
			));

			assert!(!matched);
		});
	}
}
// ========== reprove_alias_account tests ==========

mod reprove_alias_account {
	use super::*;
	use frame_support::traits::UnixTime;

	const CUSTOM_CONTEXT: Context = [42u8; 32];
	const PAID_FEE: u64 = 100;

	fn setup_paid_alias() {
		AliasFee::<Test>::put(PAID_FEE);
		setup_pgas_for(ALICE, 10_000);
		assert_ok!(AliasAccounts::set_alias_account(
			RuntimeOrigin::signed(ALICE),
			make_valid_proof(ALIAS_A),
			PeopleCollection::get(),
			0,
			1,
			CUSTOM_CONTEXT,
			MOCK_GENESIS_TIME,
		));
	}

	#[test]
	fn succeeds_and_is_free() {
		new_test_ext().execute_with(|| {
			setup_paid_alias();
			let pgas_after_setup = pgas_balance(ALICE);

			// Advance the ring revision and re-prove.
			push_mock_ring_revision(PeopleCollection::get(), 0, 2);

			assert_ok!(AliasAccounts::reprove_alias_account(
				RuntimeOrigin::signed(ALICE),
				make_valid_proof(ALIAS_A),
				0,
				2,
				MOCK_GENESIS_TIME,
			));

			let info = AccountToAlias::<Test>::get(ALICE).unwrap();
			assert_eq!(info.revision, 2);
			// No PGAS burned.
			assert_eq!(pgas_balance(ALICE), pgas_after_setup);
		});
	}

	#[test]
	fn rejects_when_no_mapping() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				AliasAccounts::reprove_alias_account(
					RuntimeOrigin::signed(ALICE),
					make_valid_proof(ALIAS_A),
					0,
					1,
					MOCK_GENESIS_TIME,
				),
				crate::Error::<Test>::InvalidAccount
			);
		});
	}

	#[test]
	fn rejects_alias_mismatch() {
		new_test_ext().execute_with(|| {
			setup_paid_alias();
			push_mock_ring_revision(PeopleCollection::get(), 0, 2);

			// Proof produces a different alias.
			assert_noop!(
				AliasAccounts::reprove_alias_account(
					RuntimeOrigin::signed(ALICE),
					make_valid_proof(ALIAS_B),
					0,
					2,
					MOCK_GENESIS_TIME,
				),
				crate::Error::<Test>::ReproveMismatch
			);
		});
	}

	#[test]
	fn rejects_no_op_at_same_revision() {
		new_test_ext().execute_with(|| {
			setup_paid_alias();

			// Same revision, same ring as currently stored — nothing to update.
			assert_noop!(
				AliasAccounts::reprove_alias_account(
					RuntimeOrigin::signed(ALICE),
					make_valid_proof(ALIAS_A),
					0,
					1,
					MOCK_GENESIS_TIME,
				),
				crate::Error::<Test>::AliasAccountAlreadySet
			);
		});
	}

	#[test]
	fn rejects_revision_regression_on_same_ring() {
		new_test_ext().execute_with(|| {
			// Set up the paid mapping at revision 5.
			AliasFee::<Test>::put(100u64);
			setup_pgas_for(ALICE, 10_000);
			set_mock_ring_revision(PeopleCollection::get(), 0, 5);
			assert_ok!(AliasAccounts::set_alias_account(
				RuntimeOrigin::signed(ALICE),
				make_valid_proof(ALIAS_A),
				PeopleCollection::get(),
				0,
				5,
				CUSTOM_CONTEXT,
				MOCK_GENESIS_TIME,
			));

			// Push an older revision back into the roots window so the proof
			// validates against revision 3, then try to re-prove against it on
			// the same ring — must be rejected as a regression.
			push_mock_ring_revision(PeopleCollection::get(), 0, 3);

			assert_noop!(
				AliasAccounts::reprove_alias_account(
					RuntimeOrigin::signed(ALICE),
					make_valid_proof(ALIAS_A),
					0,
					3,
					MOCK_GENESIS_TIME,
				),
				crate::Error::<Test>::AliasAccountAlreadySet
			);
		});
	}

	#[test]
	fn fails_with_outdated_proof() {
		new_test_ext().execute_with(|| {
			let proof_valid_at = MOCK_GENESIS_TIME;
			set_mock_time(MOCK_GENESIS_TIME + ProofValidityWindow::get() + 1);
			let now = MockUnixTime::now().as_secs();

			assert!(now > proof_valid_at + ProofValidityWindow::get());

			assert_noop!(
				AliasAccounts::reprove_alias_account(
					RuntimeOrigin::signed(ALICE),
					make_valid_proof(ALIAS_A),
					0,
					1,
					proof_valid_at,
				),
				crate::Error::<Test>::TimeOutOfRange
			);
		});
	}

	#[test]
	fn fails_with_future_proof() {
		new_test_ext().execute_with(|| {
			let proof_valid_at = MOCK_GENESIS_TIME + 10;
			let now = MockUnixTime::now().as_secs();
			assert!(now < proof_valid_at);

			assert_noop!(
				AliasAccounts::reprove_alias_account(
					RuntimeOrigin::signed(ALICE),
					make_valid_proof(ALIAS_A),
					0,
					1,
					proof_valid_at,
				),
				crate::Error::<Test>::TimeOutOfRange
			);
		});
	}
}

// ========== set_alias_fee tests ==========

mod set_alias_fee {
	use super::*;

	#[test]
	fn manager_origin_can_set_fee() {
		new_test_ext().execute_with(|| {
			assert_ok!(AliasAccounts::set_alias_fee(RuntimeOrigin::root(), 250));
			assert_eq!(AliasFee::<Test>::get(), Some(250));

			// Override.
			assert_ok!(AliasAccounts::set_alias_fee(RuntimeOrigin::root(), 0));
			assert_eq!(AliasFee::<Test>::get(), Some(0));
		});
	}

	#[test]
	fn non_manager_origin_rejected() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				AliasAccounts::set_alias_fee(RuntimeOrigin::signed(ALICE), 250),
				DispatchError::BadOrigin
			);
			assert_noop!(
				AliasAccounts::set_alias_fee(RuntimeOrigin::none(), 250),
				DispatchError::BadOrigin
			);
		});
	}
}
