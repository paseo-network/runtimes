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
	mock::*,
	pallet::{AccountToAlias, AliasToAccount, StaleSince},
	WeightInfo as _,
};
use frame_support::{assert_noop, assert_ok, dispatch::GetDispatchInfo};
use indiv_support::traits::{Alias, Context, ContextualAlias, Identifier};
use sp_runtime::DispatchError;

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
			AliasFee::set(&Some(PAID_FEE));
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
			AliasFee::set(&Some(PAID_FEE));
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
			AliasFee::set(&Some(PAID_FEE));
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
			AliasFee::set(&Some(PAID_FEE));
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
			AliasFee::set(&Some(PAID_FEE));
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
			AliasFee::set(&Some(PAID_FEE));
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
			AliasFee::set(&Some(PAID_FEE));
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
			AliasFee::set(&Some(PAID_FEE));

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
			AliasFee::set(&Some(PAID_FEE));
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
			AliasFee::set(&Some(0));
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
			AliasFee::set(&Some(PAID_FEE));
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
			AliasFee::set(&Some(PAID_FEE));
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

// ========== Stale-mapping sweep tests ==========

mod stale_alias_sweeps {
	use super::*;
	use crate::{AuthorizeInvalidity, StaleAliasAction};
	use frame_support::{
		pallet_prelude::{
			DispatchResult, InvalidTransaction, TransactionSource, TransactionValidityError,
		},
		traits::Hooks,
		BoundedVec,
	};
	use indiv_support::traits::PersonhoodLookup;

	fn seed_alias_at_rev_for(
		account: u64,
		alias: Alias,
		context: Context,
		revision: u32,
		ring: u32,
	) {
		let info = make_alias_info_for(PeopleCollection::get(), alias, context, revision, ring);
		AccountToAlias::<Test>::insert(account, &info);
		AliasToAccount::<Test>::insert(
			PeopleCollection::get(),
			ContextualAlias { alias, context },
			account,
		);
		frame_system::Pallet::<Test>::inc_sufficients(&account);
	}

	fn seed_alias_at_rev(alias: Alias, context: Context, revision: u32, ring: u32) {
		seed_alias_at_rev_for(ALICE, alias, context, revision, ring);
	}

	fn batch(accounts: &[u64]) -> BoundedVec<u64, MaxStaleAliasBatch> {
		BoundedVec::try_from(accounts.to_vec()).expect("test batch is within the bound")
	}

	/// The sweeps as the offchain worker dispatches them, on the authorized origin `authorize`
	/// admits them under.
	fn report(accounts: &[u64]) -> DispatchResult {
		AliasAccounts::report_stale_aliases(
			frame_system::RawOrigin::Authorized.into(),
			batch(accounts),
		)
	}

	fn retire(accounts: &[u64]) -> DispatchResult {
		AliasAccounts::retire_stale_aliases(
			frame_system::RawOrigin::Authorized.into(),
			batch(accounts),
		)
	}

	fn clear(accounts: &[u64]) -> DispatchResult {
		AliasAccounts::clear_stale_alias_reports(
			frame_system::RawOrigin::Authorized.into(),
			batch(accounts),
		)
	}

	/// What the transaction pool would make of a sweep over `accounts`.
	fn authorize(
		action: StaleAliasAction,
		accounts: &[u64],
	) -> Result<(), TransactionValidityError> {
		AliasAccounts::authorize_sweep(TransactionSource::Local, &batch(accounts), action)
			.map(|_| ())
	}

	fn invalidity(e: AuthorizeInvalidity) -> TransactionValidityError {
		InvalidTransaction::Custom(e as u8).into()
	}

	#[test]
	fn a_report_then_a_removal_clears_a_stale_mapping() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			seed_alias_at_rev(ALIAS_A, PEOPLE_CONTEXT, 1, 0);
			assert!(AccountToAlias::<Test>::get(ALICE).is_some());

			// Ring revision advances. The stored mapping now points to an outdated revision.
			set_mock_ring_revision(PeopleCollection::get(), 0, 2);

			// Advance time past the mapping retention.
			set_mock_time(MOCK_GENESIS_TIME + MappingRetention::get() + 1);

			// The report only stamps: the ring's newest root dates the mapping, so the retention
			// that has already elapsed counts.
			assert_ok!(authorize(StaleAliasAction::Report, &[ALICE]));
			assert_ok!(report(&[ALICE]));
			assert_eq!(StaleSince::<Test>::get(ALICE), Some(MOCK_GENESIS_TIME));
			assert!(AccountToAlias::<Test>::get(ALICE).is_some());
			assert!(System::events().iter().any(|record| record.event ==
				RuntimeEvent::AliasAccounts(crate::Event::StaleAliasReported {
					account: ALICE,
					collection: PeopleCollection::get(),
					alias: ALIAS_A,
					removable_at: MOCK_GENESIS_TIME + MappingRetention::get(),
				})));

			// The deadline that report named has passed, so the removal is admitted.
			assert_ok!(authorize(StaleAliasAction::Retire, &[ALICE]));
			assert_ok!(retire(&[ALICE]));

			assert!(AliasToAccount::<Test>::get(
				PeopleCollection::get(),
				ContextualAlias { alias: ALIAS_A, context: PEOPLE_CONTEXT }
			)
			.is_none());
			assert!(AccountToAlias::<Test>::get(ALICE).is_none());
			assert_eq!(StaleSince::<Test>::get(ALICE), None);
		});
	}

	/// One sweep carries a batch, so every mapping in it goes in one transaction.
	#[test]
	fn one_sweep_retires_every_mapping_in_its_batch() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			let holders = [ALICE, BOB, CHARLIE];
			for (i, holder) in holders.iter().enumerate() {
				seed_alias_at_rev_for(*holder, [i as u8 + 1; 32], PEOPLE_CONTEXT, 1, 0);
			}
			set_mock_ring_revision(PeopleCollection::get(), 0, 2);
			set_mock_time(MOCK_GENESIS_TIME + MappingRetention::get() + 1);

			let mut sorted = holders;
			sorted.sort();
			assert_ok!(report(&sorted));
			for holder in &sorted {
				assert_eq!(StaleSince::<Test>::get(holder), Some(MOCK_GENESIS_TIME));
			}

			assert_ok!(authorize(StaleAliasAction::Retire, &sorted));
			assert_ok!(retire(&sorted));
			for holder in &sorted {
				assert!(AccountToAlias::<Test>::get(holder).is_none());
				assert_eq!(StaleSince::<Test>::get(holder), None);
			}
		});
	}

	/// The hole a deleted ring used to open: nothing dates the mapping, so the report has to date
	/// it itself rather than letting the removal happen at once.
	#[test]
	fn a_removed_ring_keeps_the_full_retention() {
		new_test_ext().execute_with(|| {
			seed_alias_at_rev(ALIAS_A, PEOPLE_CONTEXT, 1, 0);
			remove_mock_ring_root(PeopleCollection::get(), 0);

			assert_ok!(report(&[ALICE]));
			assert_eq!(StaleSince::<Test>::get(ALICE), Some(MOCK_GENESIS_TIME));

			// A removal now would strand a consumer still resolving the mapping, so the pool
			// rejects the sweep that would carry it.
			assert_eq!(
				authorize(StaleAliasAction::Retire, &[ALICE]),
				Err(invalidity(AuthorizeInvalidity::WrongStaleState))
			);
			assert!(AccountToAlias::<Test>::get(ALICE).is_some());

			set_mock_time(MOCK_GENESIS_TIME + MappingRetention::get());
			assert_ok!(authorize(StaleAliasAction::Retire, &[ALICE]));
			assert_ok!(retire(&[ALICE]));
			assert!(AccountToAlias::<Test>::get(ALICE).is_none());
		});
	}

	/// Re-proving makes the mapping valid again, so the report against it must not survive: a later
	/// staleness starts a fresh retention.
	#[test]
	fn reproving_clears_the_report() {
		new_test_ext().execute_with(|| {
			AliasFee::set(&Some(100u64));
			setup_pgas_for(ALICE, 10_000);
			assert_ok!(AliasAccounts::set_alias_account(
				RuntimeOrigin::signed(ALICE),
				make_valid_proof(ALIAS_A),
				PeopleCollection::get(),
				0,
				1,
				PEOPLE_CONTEXT,
				MOCK_GENESIS_TIME,
			));

			set_mock_ring_revision(PeopleCollection::get(), 0, 2);
			assert_ok!(report(&[ALICE]));
			assert_eq!(StaleSince::<Test>::get(ALICE), Some(MOCK_GENESIS_TIME));

			assert_ok!(AliasAccounts::reprove_alias_account(
				RuntimeOrigin::signed(ALICE),
				make_valid_proof(ALIAS_A),
				0,
				2,
				MOCK_GENESIS_TIME,
			));
			assert_eq!(StaleSince::<Test>::get(ALICE), None);

			// Stale again under a newer root. The retention runs from that root, not from the
			// report the reprove voided.
			set_mock_time(MOCK_GENESIS_TIME + MappingRetention::get());
			set_mock_ring_revision(PeopleCollection::get(), 0, 3);
			assert_ok!(report(&[ALICE]));
			assert_eq!(
				authorize(StaleAliasAction::Retire, &[ALICE]),
				Err(invalidity(AuthorizeInvalidity::WrongStaleState))
			);
		});
	}

	/// Unsetting is the one path that leaves the account with no mapping at all, so a report it
	/// left behind would be an entry nothing ever collects.
	#[test]
	fn unsetting_clears_the_report() {
		new_test_ext().execute_with(|| {
			seed_alias_at_rev(ALIAS_A, PEOPLE_CONTEXT, 1, 0);
			set_mock_ring_revision(PeopleCollection::get(), 0, 2);

			assert_ok!(report(&[ALICE]));
			assert_eq!(StaleSince::<Test>::get(ALICE), Some(MOCK_GENESIS_TIME));

			assert_ok!(AliasAccounts::unset_alias_account(RuntimeOrigin::signed(ALICE)));
			assert_eq!(StaleSince::<Test>::get(ALICE), None);
		});
	}

	/// A swap hands the alias to a new account and strands the old one, whose report has to go with
	/// the mapping it was about.
	#[test]
	fn a_swap_clears_the_old_account_report() {
		new_test_ext().execute_with(|| {
			AliasFee::set(&Some(100u64));
			setup_pgas_for(ALICE, 10_000);
			setup_pgas_for(BOB, 10_000);
			assert_ok!(AliasAccounts::set_alias_account(
				RuntimeOrigin::signed(ALICE),
				make_valid_proof(ALIAS_A),
				PeopleCollection::get(),
				0,
				1,
				PEOPLE_CONTEXT,
				MOCK_GENESIS_TIME,
			));

			set_mock_ring_revision(PeopleCollection::get(), 0, 2);
			assert_ok!(report(&[ALICE]));
			assert_eq!(StaleSince::<Test>::get(ALICE), Some(MOCK_GENESIS_TIME));

			// BOB takes over the alias at the revision that is current again.
			assert_ok!(AliasAccounts::set_alias_account(
				RuntimeOrigin::signed(BOB),
				make_valid_proof(ALIAS_A),
				PeopleCollection::get(),
				0,
				2,
				PEOPLE_CONTEXT,
				MOCK_GENESIS_TIME,
			));
			assert!(AccountToAlias::<Test>::get(ALICE).is_none());
			assert_eq!(StaleSince::<Test>::get(ALICE), None);
			assert_eq!(StaleSince::<Test>::get(BOB), None);
		});
	}

	/// Each sweep is charged its own weight function, and each is charged per mapping in the batch.
	/// Sharing one would mean a batch of the heavy call paying for the cheap one.
	#[test]
	fn each_sweep_is_charged_its_own_weight() {
		new_test_ext().execute_with(|| {
			let accounts = batch(&[ALICE, BOB]);
			let charged = |call: crate::Call<Test>| call.get_dispatch_info().call_weight;

			let reporting =
				charged(crate::Call::report_stale_aliases { accounts: accounts.clone() });
			let retiring =
				charged(crate::Call::retire_stale_aliases { accounts: accounts.clone() });
			let clearing = charged(crate::Call::clear_stale_alias_reports { accounts });

			assert_eq!(reporting, MockWeightInfo::report_stale_aliases(2));
			assert_eq!(retiring, MockWeightInfo::retire_stale_aliases(2));
			assert_eq!(clearing, MockWeightInfo::clear_stale_alias_reports(2));
			assert!(reporting.all_lt(retiring));
			assert!(clearing.all_lt(reporting));

			// A longer batch is charged more, so the per-mapping term is really wired up.
			assert!(charged(crate::Call::retire_stale_aliases { accounts: batch(&[ALICE]) })
				.all_lt(retiring));
		});
	}

	/// A collection torn down and re-created under the same identifier restarts its revisions at
	/// zero, so a stored revision can be reissued and verify again. The report against it must go,
	/// otherwise the next staleness inherits a deadline that has long since passed.
	#[test]
	fn a_revision_that_verifies_again_clears_the_report() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			seed_alias_at_rev(ALIAS_A, PEOPLE_CONTEXT, 1, 0);
			set_mock_ring_revision(PeopleCollection::get(), 0, 2);
			set_mock_time(MOCK_GENESIS_TIME + MappingRetention::get() + 1);

			assert_ok!(report(&[ALICE]));
			assert!(StaleSince::<Test>::get(ALICE).is_some());

			// The collection comes back and rebuilds ring 0 at revision 1, the only record in a
			// fresh window, so the stored revision is newest again and never expires.
			let revived_at = MOCK_GENESIS_TIME + 10 * MappingRetention::get();
			set_mock_time(revived_at);
			set_mock_ring_revision(PeopleCollection::get(), 0, 1);

			assert_ok!(authorize(StaleAliasAction::ClearReport, &[ALICE]));
			assert_ok!(clear(&[ALICE]));
			assert_eq!(StaleSince::<Test>::get(ALICE), None);
			assert!(AccountToAlias::<Test>::get(ALICE).is_some());
			assert!(System::events().iter().any(|record| record.event ==
				RuntimeEvent::AliasAccounts(crate::Event::StaleAliasReportCleared {
					account: ALICE,
					collection: PeopleCollection::get(),
					alias: ALIAS_A,
				})));

			// Stale again under the revived collection. The retention runs from here, not from
			// the report the revival voided.
			set_mock_ring_revision(PeopleCollection::get(), 0, 3);
			set_mock_time(revived_at + MOCK_OLD_ROOT_RETENTION + 1);
			assert_ok!(report(&[ALICE]));
			assert_eq!(StaleSince::<Test>::get(ALICE), Some(revived_at));
			assert_eq!(
				authorize(StaleAliasAction::Retire, &[ALICE]),
				Err(invalidity(AuthorizeInvalidity::WrongStaleState))
			);
		});
	}

	/// The sweeps carry no checks of their own, so what keeps them honest is that a block runs
	/// `authorize` first. Applying the whole transaction is what proves that: the removal is
	/// refused while the mapping still resolves, and nothing is removed.
	#[test]
	fn a_retire_transaction_is_refused_while_the_mapping_still_resolves() {
		new_test_ext().execute_with(|| {
			seed_alias_at_rev(ALIAS_A, PEOPLE_CONTEXT, 1, 0);
			set_mock_ring_revision(PeopleCollection::get(), 0, 2);
			// Stale and stamped, but the retention has not run out.
			assert_ok!(report(&[ALICE]));

			let call = RuntimeCall::AliasAccounts(crate::Call::retire_stale_aliases {
				accounts: batch(&[ALICE]),
			});
			assert_eq!(
				apply_authorized(call).map(|dispatch| dispatch.map(|_| ()).map_err(|_| ())),
				Err(invalidity(AuthorizeInvalidity::WrongStaleState))
			);
			// The mapping is still there, so the call never ran.
			assert!(AccountToAlias::<Test>::get(ALICE).is_some());
			assert_eq!(StaleSince::<Test>::get(ALICE), Some(MOCK_GENESIS_TIME));
		});
	}

	/// The other half of the pipeline: past the retention the same transaction is admitted and the
	/// mapping goes. Without this the refusal above could pass for the wrong reason.
	#[test]
	fn a_retire_transaction_removes_a_mapping_past_its_retention() {
		new_test_ext().execute_with(|| {
			seed_alias_at_rev(ALIAS_A, PEOPLE_CONTEXT, 1, 0);
			set_mock_ring_revision(PeopleCollection::get(), 0, 2);
			assert_ok!(report(&[ALICE]));
			set_mock_time(MOCK_GENESIS_TIME + MappingRetention::get() + 1);

			let call = RuntimeCall::AliasAccounts(crate::Call::retire_stale_aliases {
				accounts: batch(&[ALICE]),
			});
			let dispatch = apply_authorized(call).expect("the sweep is admitted");
			assert_ok!(dispatch.map(|_| ()).map_err(|error| error.error));

			assert!(AccountToAlias::<Test>::get(ALICE).is_none());
			assert_eq!(StaleSince::<Test>::get(ALICE), None);
		});
	}

	/// A sweep only ever comes from this node's own offchain worker, so a gossiped one is refused
	/// before it reaches the state it would change.
	#[test]
	fn authorize_rejects_an_external_source() {
		new_test_ext().execute_with(|| {
			seed_alias_at_rev(ALIAS_A, PEOPLE_CONTEXT, 1, 0);
			set_mock_ring_revision(PeopleCollection::get(), 0, 2);

			assert_eq!(
				AliasAccounts::authorize_sweep(
					TransactionSource::External,
					&batch(&[ALICE]),
					StaleAliasAction::Report,
				)
				.map(|_| ()),
				Err(invalidity(AuthorizeInvalidity::TransactionNotLocal))
			);
			// The same sweep from this node is admitted, so the source is what refused it.
			assert_ok!(authorize(StaleAliasAction::Report, &[ALICE]));
		});
	}

	#[test]
	fn authorize_rejects_an_empty_batch() {
		new_test_ext().execute_with(|| {
			assert_eq!(
				authorize(StaleAliasAction::Report, &[]),
				Err(invalidity(AuthorizeInvalidity::EmptyBatch))
			);
		});
	}

	/// An unsorted batch is what a repeat hides in: the same account twice would be stamped twice,
	/// and the second pass would read state the first one changed.
	#[test]
	fn authorize_rejects_a_batch_that_is_not_ascending() {
		new_test_ext().execute_with(|| {
			seed_alias_at_rev_for(ALICE, ALIAS_A, PEOPLE_CONTEXT, 1, 0);
			seed_alias_at_rev_for(BOB, ALIAS_B, PEOPLE_CONTEXT, 1, 0);
			set_mock_ring_revision(PeopleCollection::get(), 0, 2);

			let (low, high) = if ALICE < BOB { (ALICE, BOB) } else { (BOB, ALICE) };
			assert_eq!(
				authorize(StaleAliasAction::Report, &[high, low]),
				Err(invalidity(AuthorizeInvalidity::UnsortedBatch))
			);
			assert_eq!(
				authorize(StaleAliasAction::Report, &[low, low]),
				Err(invalidity(AuthorizeInvalidity::UnsortedBatch))
			);
			assert_ok!(authorize(StaleAliasAction::Report, &[low, high]));
		});
	}

	/// Each sweep names the state it applies to, so a mapping in another state belongs to another
	/// call and is refused here.
	#[test]
	fn authorize_rejects_a_mapping_in_another_state() {
		new_test_ext().execute_with(|| {
			// A mapping that still verifies: nothing to report, nothing to clear.
			seed_alias_at_rev(ALIAS_A, PEOPLE_CONTEXT, 1, 0);
			assert_eq!(
				authorize(StaleAliasAction::Report, &[ALICE]),
				Err(invalidity(AuthorizeInvalidity::WrongStaleState))
			);
			assert_eq!(
				authorize(StaleAliasAction::ClearReport, &[ALICE]),
				Err(invalidity(AuthorizeInvalidity::WrongStaleState))
			);

			// An account with no mapping at all.
			assert_eq!(
				authorize(StaleAliasAction::Report, &[BOB]),
				Err(invalidity(AuthorizeInvalidity::WrongStaleState))
			);

			// Stale and unstamped: the report applies, the removal does not yet.
			set_mock_ring_revision(PeopleCollection::get(), 0, 2);
			assert_ok!(authorize(StaleAliasAction::Report, &[ALICE]));
			assert_eq!(
				authorize(StaleAliasAction::Retire, &[ALICE]),
				Err(invalidity(AuthorizeInvalidity::WrongStaleState))
			);
		});
	}

	/// One bad mapping voids the whole batch: the sweep is rebuilt on the next interval rather
	/// than applied in part.
	#[test]
	fn authorize_rejects_a_batch_with_one_mapping_in_another_state() {
		new_test_ext().execute_with(|| {
			seed_alias_at_rev_for(ALICE, ALIAS_A, PEOPLE_CONTEXT, 1, 0);
			seed_alias_at_rev_for(BOB, ALIAS_B, PEOPLE_CONTEXT, 1, 0);
			set_mock_ring_revision(PeopleCollection::get(), 0, 2);

			let (low, high) = if ALICE < BOB { (ALICE, BOB) } else { (BOB, ALICE) };
			assert_ok!(authorize(StaleAliasAction::Report, &[low, high]));

			// One of them is stamped already, so it belongs to no reporting sweep.
			StaleSince::<Test>::insert(high, MOCK_GENESIS_TIME);
			assert_eq!(
				authorize(StaleAliasAction::Report, &[low, high]),
				Err(invalidity(AuthorizeInvalidity::WrongStaleState))
			);
			assert_ok!(authorize(StaleAliasAction::Report, &[low]));
		});
	}

	/// The sweep is what makes the cleanup happen without anyone calling, so it has to sort the
	/// mappings into the call each of them needs.
	#[test]
	fn the_offchain_worker_submits_one_sweep_per_action() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			clear_pool();

			// Stale and unstamped, so it wants reporting.
			seed_alias_at_rev_for(ALICE, ALIAS_A, PEOPLE_CONTEXT, 1, 0);
			// Stale, stamped and past the retention, so it wants removing.
			seed_alias_at_rev_for(BOB, ALIAS_B, PEOPLE_CONTEXT, 1, 0);
			StaleSince::<Test>::insert(BOB, MOCK_GENESIS_TIME);
			// Valid but stamped, so its report wants clearing.
			seed_alias_at_rev_for(CHARLIE, [30u8; 32], PEOPLE_CONTEXT, 2, 0);
			StaleSince::<Test>::insert(CHARLIE, MOCK_GENESIS_TIME);

			set_mock_ring_revision(PeopleCollection::get(), 0, 2);
			set_mock_time(MOCK_GENESIS_TIME + MappingRetention::get() + 1);

			<AliasAccounts as Hooks<u64>>::offchain_worker(System::block_number());

			assert_eq!(
				submitted_calls(),
				vec![
					RuntimeCall::AliasAccounts(crate::Call::report_stale_aliases {
						accounts: batch(&[ALICE])
					}),
					RuntimeCall::AliasAccounts(crate::Call::retire_stale_aliases {
						accounts: batch(&[BOB])
					}),
					RuntimeCall::AliasAccounts(crate::Call::clear_stale_alias_reports {
						accounts: batch(&[CHARLIE])
					}),
				]
			);
		});
	}

	/// A chain with more stale mappings than one batch holds retires them over as many sweeps as
	/// it takes, rather than building a transaction no block accepts.
	#[test]
	fn the_offchain_worker_stops_at_the_batch_bound() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			clear_pool();

			let bound = MaxStaleAliasBatch::get();
			for i in 0..bound + 2 {
				seed_alias_at_rev_for(100 + i as u64, [i as u8 + 1; 32], PEOPLE_CONTEXT, 1, 0);
			}
			set_mock_ring_revision(PeopleCollection::get(), 0, 2);

			<AliasAccounts as Hooks<u64>>::offchain_worker(System::block_number());

			let calls = submitted_calls();
			assert_eq!(calls.len(), 1);
			let RuntimeCall::AliasAccounts(crate::Call::report_stale_aliases { accounts }) =
				&calls[0]
			else {
				panic!("the sweep reports the stale mappings")
			};
			assert_eq!(accounts.len() as u32, bound);
			// Ascending, which is what `authorize` holds a batch to.
			assert!(accounts.windows(2).all(|pair| pair[0] < pair[1]));
			assert_ok!(authorize(StaleAliasAction::Report, accounts));
		});
	}

	/// The sweep reads every mapping, so it runs on its interval rather than every block.
	#[test]
	fn the_offchain_worker_only_sweeps_on_its_interval() {
		new_test_ext().execute_with(|| {
			OffchainWorkerInterval::set(&5);
			seed_alias_at_rev(ALIAS_A, PEOPLE_CONTEXT, 1, 0);
			set_mock_ring_revision(PeopleCollection::get(), 0, 2);

			System::set_block_number(4);
			clear_pool();
			<AliasAccounts as Hooks<u64>>::offchain_worker(System::block_number());
			assert!(submitted_calls().is_empty());

			System::set_block_number(5);
			<AliasAccounts as Hooks<u64>>::offchain_worker(System::block_number());
			assert_eq!(submitted_calls().len(), 1);
		});
	}

	#[test]
	fn the_offchain_worker_submits_nothing_without_stale_mappings() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			clear_pool();
			seed_alias_at_rev(ALIAS_A, PEOPLE_CONTEXT, 1, 0);

			<AliasAccounts as Hooks<u64>>::offchain_worker(System::block_number());

			assert!(submitted_calls().is_empty());
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

			// Advance time past the mapping retention.
			set_mock_time(MOCK_GENESIS_TIME + MappingRetention::get() + 1);

			// Stored revision (5) != ring revision (3), so the alias is stale.
			assert_ok!(report(&[ALICE]));
			assert_ok!(authorize(StaleAliasAction::Retire, &[ALICE]));
			assert_ok!(retire(&[ALICE]));

			assert!(AccountToAlias::<Test>::get(ALICE).is_none());
		});
	}

	#[test]
	fn succeeds_when_old_revision_still_in_roots_but_retention_expired() {
		new_test_ext().execute_with(|| {
			// Alias created at revision 1 with source_time = MOCK_GENESIS_TIME.
			seed_alias_at_rev(ALIAS_A, PEOPLE_CONTEXT, 1, 0);

			// Ring revised to 2 at a later source_time, old revision 1 still in
			// BoundedVec.
			set_mock_time(MOCK_GENESIS_TIME + 5_000);
			push_mock_ring_revision(PeopleCollection::get(), 0, 2);

			// Revision 1 stays accepted by the member service until its retention has elapsed since
			// revision 2's source time. Past that the mapping has stopped counting as personhood
			// and is still stored, so a consumer that reads it without checking the revision
			// resolves it.
			set_mock_time(MOCK_GENESIS_TIME + 5_000 + MOCK_OLD_ROOT_RETENTION + 1);
			assert_eq!(AliasAccounts::personhood_info(&ALICE, &PEOPLE_CONTEXT).0, None);

			// The report dates the mapping by revision 2, the ring's newest root, and the removal
			// waits for the retention counted from there.
			assert_ok!(report(&[ALICE]));
			assert_eq!(StaleSince::<Test>::get(ALICE), Some(MOCK_GENESIS_TIME + 5_000));
			assert_eq!(
				authorize(StaleAliasAction::Retire, &[ALICE]),
				Err(invalidity(AuthorizeInvalidity::WrongStaleState))
			);

			set_mock_time(MOCK_GENESIS_TIME + 5_000 + MappingRetention::get() + 1);
			assert_ok!(authorize(StaleAliasAction::Retire, &[ALICE]));
			assert_ok!(retire(&[ALICE]));

			assert!(AccountToAlias::<Test>::get(ALICE).is_none());
		});
	}

	/// End-to-end coverage: create a mapping via the real `set_alias_account`
	/// extrinsic (burning the PGAS fee, writing both directions of storage),
	/// then remove the ring root and sweep it away. Distinct from the other
	/// tests in this module that seed storage directly.
	#[test]
	fn succeeds_when_ring_removed_after_real_setup() {
		new_test_ext().execute_with(|| {
			const CUSTOM_CONTEXT: Context = [42u8; 32];

			AliasFee::set(&Some(100u64));
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

			// Ring root removed entirely (e.g. target of a merge). Nothing dates the mapping, so
			// the report dates it by this call and the removal waits out the retention.
			remove_mock_ring_root(PeopleCollection::get(), 0);

			assert_ok!(report(&[ALICE]));
			set_mock_time(MOCK_GENESIS_TIME + MappingRetention::get());
			assert_ok!(retire(&[ALICE]));

			let ca = ContextualAlias { alias: ALIAS_A, context: CUSTOM_CONTEXT };
			assert!(AccountToAlias::<Test>::get(ALICE).is_none());
			assert!(AliasToAccount::<Test>::get(PeopleCollection::get(), ca).is_none());
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
	fn returns_some_for_non_latest_revision_within_retention() {
		new_test_ext().execute_with(|| {
			let info = make_alias_info(ALIAS_A, PEOPLE_CONTEXT);
			AccountToAlias::<Test>::insert(ALICE, &info);

			// New revision pushed but old one still in window and within retention.
			push_mock_ring_revision(PeopleCollection::get(), 0, 2);

			let (result, _) = AliasAccounts::personhood_info(&ALICE, &PEOPLE_CONTEXT);
			assert_eq!(result, Some((PeopleCollection::get(), ALIAS_A)));
		});
	}

	#[test]
	fn returns_none_for_non_latest_revision_past_retention() {
		new_test_ext().execute_with(|| {
			let info = make_alias_info(ALIAS_A, PEOPLE_CONTEXT);
			AccountToAlias::<Test>::insert(ALICE, &info);

			// New revision pushed and time advanced past the member service's retention.
			push_mock_ring_revision(PeopleCollection::get(), 0, 2);
			set_mock_time(MOCK_GENESIS_TIME + MOCK_OLD_ROOT_RETENTION + 1);

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
		AliasFee::set(&Some(PAID_FEE));
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
			AliasFee::set(&Some(100u64));
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

// ========== Integrity tests ==========

mod integrity {
	use super::*;
	use frame_support::traits::Hooks;

	/// Verify that the default mock configuration passes all integrity checks.
	#[test]
	fn passes_with_the_default_configuration() {
		new_test_ext().execute_with(|| {
			<crate::Pallet<Test> as Hooks<u64>>::integrity_test();
		});
	}

	/// A retention shorter than the member service's leaves no window, because the mapping would
	/// be retirable before it could first be reported.
	#[test]
	#[should_panic = "MappingRetention must exceed the member service's old root retention"]
	fn rejects_a_retention_below_the_member_service_retention() {
		new_test_ext().execute_with(|| {
			MappingRetention::set(&(MOCK_OLD_ROOT_RETENTION - 1));
			<crate::Pallet<Test> as Hooks<u64>>::integrity_test();
		});
	}

	/// Matching the member service exactly leaves a zero-length window, which the assertion also
	/// rejects.
	#[test]
	#[should_panic = "MappingRetention must exceed the member service's old root retention"]
	fn rejects_a_retention_equal_to_the_member_service_retention() {
		new_test_ext().execute_with(|| {
			MappingRetention::set(&MOCK_OLD_ROOT_RETENTION);
			<crate::Pallet<Test> as Hooks<u64>>::integrity_test();
		});
	}

	/// One second above the member service's retention is the boundary the assertion allows.
	#[test]
	fn accepts_a_retention_one_second_above_the_member_service_retention() {
		new_test_ext().execute_with(|| {
			MappingRetention::set(&(MOCK_OLD_ROOT_RETENTION + 1));
			<crate::Pallet<Test> as Hooks<u64>>::integrity_test();
		});
	}
}
