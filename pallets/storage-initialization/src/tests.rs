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
	mock::*, pallet::Event, InitializeIndividualityPallets, OnPollState, OnPollStatus, Pallet,
	XcmTransferInitiatedAt,
};
use frame_support::{migrations::SteppedMigration, traits::Hooks, weights::WeightMeter};

#[test]
fn successful_migration_part() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		// Create the people collection in the members pallet before running the migration
		use frame_support::assert_ok;
		assert_ok!(indiv_pallet_people::Pallet::<Test>::create_people_collection(
			frame_system::Origin::<Test>::Authorized.into()
		));

		// Make sure there are no people, nor keys and that the onboarding queue is empty
		assert_eq!(indiv_pallet_people::People::<Test>::iter().count(), 0);
		assert_eq!(indiv_pallet_people::Keys::<Test>::iter().count(), 0);

		let identifier = indiv_pallet_people::PEOPLE_MEMBER_IDENTIFIER;
		let (head, _) = indiv_pallet_members::QueuePageIndices::<Test>::get(identifier);
		assert_eq!(indiv_pallet_members::OnboardingQueue::<Test>::get(identifier, head).len(), 0);

		// Make sure design families and configuration are empty
		assert_eq!(indiv_pallet_proof_of_ink::DesignFamilies::<Test>::iter().count(), 0);
		assert_eq!(indiv_pallet_proof_of_ink::Configuration::<Test>::get(), Default::default());

		// Start the migration
		let mut weight_meter = WeightMeter::new();
		let mut cursor = None;
		while let Some(new_cursor) =
			InitializeIndividualityPallets::<Test>::step(cursor, &mut weight_meter).unwrap()
		{
			cursor = Some(new_cursor);
		}

		// Note: Chunks are now managed by chunks-manager pallet and are added via
		// the add_chunks extrinsic (not during migration). Chunk page hashes can be
		// initialized in genesis config, but the actual chunks require separate submission.

		// Check if the initial set of recognised people was added to the people pallet
		assert_ne!(indiv_pallet_people::People::<Test>::iter().count(), 0);
		assert_ne!(indiv_pallet_people::Keys::<Test>::iter().count(), 0);

		// Check the onboarding queue - its head should be moved forward
		let (head, _) = indiv_pallet_members::QueuePageIndices::<Test>::get(identifier);
		assert_ne!(indiv_pallet_members::OnboardingQueue::<Test>::get(identifier, head).len(), 0);

		// Check design families and configuration
		assert_ne!(indiv_pallet_proof_of_ink::DesignFamilies::<Test>::iter().count(), 0);
		assert_ne!(indiv_pallet_proof_of_ink::Configuration::<Test>::get(), Default::default());

		// Check game schedules
		assert_ne!(indiv_pallet_game::GameSchedules::<Test>::get().len(), 0);

		// Check that invites were distributed
		let invite_recipient = InviteRecipient::get();
		assert_eq!(
			indiv_pallet_proof_of_ink::AvailableInvites::<Test>::get(&invite_recipient),
			100
		);
		assert_eq!(indiv_pallet_game::AvailableInvites::<Test>::get(&invite_recipient), 100);

		// Check that on_poll has been initiated by the migration
		use crate::{OnPollState, OnPollStatus};
		assert_eq!(OnPollStatus::<Test>::get(), OnPollState::CreatingAsset);

		// Check migration events were emitted
		System::assert_has_event(Event::<Test>::MigrationPeopleRecognized.into());
		System::assert_has_event(Event::<Test>::MigrationOnboardingSizeSet.into());
		System::assert_has_event(Event::<Test>::MigrationProofOfInkInitialized.into());
		System::assert_has_event(Event::<Test>::MigrationGamesScheduled.into());
		System::assert_has_event(Event::<Test>::MigrationInvitesGranted.into());
		System::assert_has_event(Event::<Test>::MigrationReimbursementValuesSet.into());
		System::assert_has_event(Event::<Test>::MigrationAttestationAllowancesSet.into());
		System::assert_has_event(Event::<Test>::MigrationCompleted.into());
	});
}

#[test]
fn on_poll_state_changes() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		// To simulate migration finishing and starting the on_poll part
		OnPollStatus::<Test>::set(OnPollState::CreatingAsset);

		// on_poll triggered for the 1st time - asset creation completes, moves to XcmFundsTransfer
		let mut weight_meter = WeightMeter::new();
		Pallet::<Test>::on_poll(1u32.into(), &mut weight_meter);
		assert_eq!(OnPollStatus::<Test>::get(), OnPollState::XcmFundsTransfer);
		System::assert_has_event(Event::<Test>::AssetCreated.into());

		// on_poll triggered for the 2nd time - XCM transfer
		let mut weight_meter = WeightMeter::new();
		Pallet::<Test>::on_poll(2u32.into(), &mut weight_meter);
		assert_eq!(OnPollStatus::<Test>::get(), OnPollState::VerifyingFunds);
		System::assert_has_event(Event::<Test>::XcmFundsTransferSent.into());

		crate::simulate_xcm_transfer_success::<Test>().unwrap();

		// on_poll triggered for the 3rd time - verifying funds
		let mut weight_meter = WeightMeter::new();
		Pallet::<Test>::on_poll(3u32.into(), &mut weight_meter);
		assert_eq!(OnPollStatus::<Test>::get(), OnPollState::FundingPots);
		System::assert_has_event(Event::<Test>::FundsVerified.into());

		// on_poll triggered for the 4th time - pot funding
		let mut weight_meter = WeightMeter::new();
		Pallet::<Test>::on_poll(4u32.into(), &mut weight_meter);
		assert_eq!(
			OnPollStatus::<Test>::get(),
			OnPollState::SettingPeopleLiteAttestationAllowances
		);
		System::assert_has_event(Event::<Test>::PotsFunded.into());

		// on_poll triggered for the 5th time - setting People Lite attestation allowances
		let mut weight_meter = WeightMeter::new();
		Pallet::<Test>::on_poll(5u32.into(), &mut weight_meter);
		assert_eq!(OnPollStatus::<Test>::get(), OnPollState::SchedulingMobRulePayouts);
		System::assert_has_event(Event::<Test>::PeopleLiteAttestationAllowancesSet.into());

		// on_poll triggered for the 6th time - scheduling mob rule payouts
		let mut weight_meter = WeightMeter::new();
		Pallet::<Test>::on_poll(6u32.into(), &mut weight_meter);
		assert_eq!(OnPollStatus::<Test>::get(), OnPollState::SchedulingScorePayouts);
		System::assert_has_event(Event::<Test>::MobRulePayoutsScheduled.into());

		// on_poll triggered for the 6th time - scheduling score payouts; transitions to
		// the terminal `Done` state and emits init-complete.
		let mut weight_meter = WeightMeter::new();
		Pallet::<Test>::on_poll(6u32.into(), &mut weight_meter);
		assert_eq!(OnPollStatus::<Test>::get(), OnPollState::Done);
		assert!(!indiv_pallet_mob_rule::RoundSchedules::<Test>::get().is_empty());
		assert!(!indiv_pallet_score::RoundSchedules::<Test>::get().is_empty());
		System::assert_has_event(Event::<Test>::ScorePayoutsScheduled.into());
		System::assert_has_event(Event::<Test>::OnPollInitializationCompleted.into());

		// Further ticks in `Done` are no-ops.
		let mut weight_meter = WeightMeter::new();
		Pallet::<Test>::on_poll(7u32.into(), &mut weight_meter);
		assert_eq!(OnPollStatus::<Test>::get(), OnPollState::Done);
	});
}

#[test]
fn on_poll_awaits_migration_finish() {
	new_test_ext().execute_with(|| {
		assert_eq!(OnPollStatus::<Test>::get(), OnPollState::Inactive);

		let mut weight_meter = WeightMeter::new();
		Pallet::<Test>::on_poll(1u32.into(), &mut weight_meter);

		assert_eq!(OnPollStatus::<Test>::get(), OnPollState::Inactive);
	});
}

#[test]
fn on_poll_retries_xcm_send() {
	new_test_ext().execute_with(|| {
		// Moving straight to the XCM transfer
		OnPollStatus::<Test>::set(OnPollState::XcmFundsTransfer);

		let start_block = 1u32;
		System::set_block_number(start_block.into());

		// on_poll trigerred
		let mut weight_meter = WeightMeter::new();
		Pallet::<Test>::on_poll(start_block.into(), &mut weight_meter);
		assert_eq!(OnPollStatus::<Test>::get(), OnPollState::VerifyingFunds);

		// XCM sent
		let initiated_at = XcmTransferInitiatedAt::<Test>::get();
		assert_eq!(initiated_at, Some(start_block.into()));

		// Timeout period
		let timeout = <Test as crate::Config>::XcmTimeout::get();
		let timeout_block: u32 = start_block + (timeout as u32) + 1u32;
		System::set_block_number(timeout_block.into());

		// Should timeout and reset to XcmFundsTransfer
		let mut weight_meter = WeightMeter::new();
		Pallet::<Test>::on_poll(timeout_block.into(), &mut weight_meter);
		assert_eq!(OnPollStatus::<Test>::get(), OnPollState::XcmFundsTransfer);
		System::assert_has_event(Event::<Test>::XcmFundsTransferTimedOut.into());
	});
}

#[test]
fn on_poll_sets_people_lite_attestation_allowances() {
	use hex_literal::hex;
	use sp_runtime::AccountId32;

	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		OnPollStatus::<Test>::set(OnPollState::SettingPeopleLiteAttestationAllowances);

		let acc1 = AccountId32::from(hex!(
			"0000000000000000000000000000000000000000000000000000000000000001"
		));
		let acc2 = AccountId32::from(hex!(
			"0000000000000000000000000000000000000000000000000000000000000002"
		));
		let acc3 = AccountId32::from(hex!(
			"0000000000000000000000000000000000000000000000000000000000000003"
		));

		// Pre-condition: no allowance set yet.
		assert_eq!(indiv_pallet_people_lite::AttestationAllowance::<Test>::get(&acc1), 0);
		assert_eq!(indiv_pallet_people_lite::AttestationAllowance::<Test>::get(&acc2), 0);
		assert_eq!(indiv_pallet_people_lite::AttestationAllowance::<Test>::get(&acc3), 0);

		let mut weight_meter = WeightMeter::new();
		Pallet::<Test>::on_poll(1u32.into(), &mut weight_meter);

		// Transitioned to the next state and emitted the completion event.
		assert_eq!(OnPollStatus::<Test>::get(), OnPollState::SchedulingMobRulePayouts);
		System::assert_has_event(Event::<Test>::PeopleLiteAttestationAllowancesSet.into());

		// Seed allowances were applied.
		assert_eq!(indiv_pallet_people_lite::AttestationAllowance::<Test>::get(&acc1), 100);
		assert_eq!(indiv_pallet_people_lite::AttestationAllowance::<Test>::get(&acc2), 50);
		assert_eq!(indiv_pallet_people_lite::AttestationAllowance::<Test>::get(&acc3), 25);
	});
}
