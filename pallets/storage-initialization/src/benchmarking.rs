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
use frame_benchmarking::{v2::*, BenchmarkError};
use frame_support::weights::WeightMeter;
use frame_system::pallet_prelude::BlockNumberFor;

pub trait BenchmarkHelper {
	fn set_unix_time(d: core::time::Duration);
}

#[benchmarks(
	where
		T::AccountId: From<sp_runtime::AccountId32>,
)]
mod benches {
	use super::*;

	#[benchmark]
	fn on_poll_status_read() -> Result<(), BenchmarkError> {
		OnPollStatus::<T>::set(OnPollState::Inactive);

		#[block]
		{
			let _ = OnPollStatus::<T>::get();
		}

		Ok(())
	}

	#[benchmark]
	fn init_pallet_proof_of_ink() -> Result<(), BenchmarkError> {
		let mut weight_meter = WeightMeter::new();

		// Set the cursor at the desired step case
		let cursor = Some(MigrationState::InitializingPalletProofOfInk);

		// Run only one step, thus only pallet proof of ink should be initialized
		#[block]
		{
			InitializeIndividualityPallets::<T>::step(cursor, &mut weight_meter).unwrap();
		}

		assert_ne!(indiv_pallet_proof_of_ink::DesignFamilies::<T>::iter().count(), 0);
		assert_ne!(indiv_pallet_proof_of_ink::Configuration::<T>::get(), Default::default());

		Ok(())
	}

	#[benchmark]
	fn on_poll_create_asset() -> Result<(), BenchmarkError> {
		use frame_support::traits::Hooks;

		OnPollStatus::<T>::set(OnPollState::CreatingAsset);

		let mut weight_meter = WeightMeter::new();

		#[block]
		{
			Pallet::<T>::on_poll(1u32.into(), &mut weight_meter);
		}

		assert_eq!(OnPollStatus::<T>::get(), OnPollState::XcmFundsTransfer);

		Ok(())
	}

	#[benchmark]
	fn on_poll_xcm_transfer() -> Result<(), BenchmarkError> {
		use frame_support::traits::Hooks;

		OnPollStatus::<T>::set(OnPollState::XcmFundsTransfer);

		let mut weight_meter = WeightMeter::new();

		#[block]
		{
			Pallet::<T>::on_poll(1u32.into(), &mut weight_meter);
		}

		// TODO: tighten back to `assert_eq!(..., OnPollState::VerifyingFunds)` once
		// `runtimes/next-people-paseo/src/genesis_config_presets.rs:33` is bumped from
		// `const SAFE_XCM_VERSION: u32 = 4` to `xcm::prelude::XCM_VERSION`.
		let final_state = OnPollStatus::<T>::get();
		assert!(
			final_state == OnPollState::VerifyingFunds ||
				final_state == OnPollState::XcmFundsTransfer
		);

		Ok(())
	}

	#[benchmark]
	fn on_poll_verify_funds() -> Result<(), BenchmarkError> {
		use frame_support::traits::Hooks;

		OnPollStatus::<T>::set(OnPollState::VerifyingFunds);
		XcmTransferInitiatedAt::<T>::put(BlockNumberFor::<T>::from(0u32));

		let (asset_id, owner, is_sufficient, min_balance) = get_transfer_asset_configuration::<T>();
		<T as Config>::Assets::create(asset_id, owner, is_sufficient, min_balance)?;

		// Advance the block number past `XcmTimeout` so the timeout check fires.
		let past_timeout =
			BlockNumberFor::<T>::from(T::XcmTimeout::get()) + BlockNumberFor::<T>::from(1u32);
		frame_system::Pallet::<T>::set_block_number(past_timeout);

		let mut weight_meter = WeightMeter::new();

		#[block]
		{
			Pallet::<T>::on_poll(1u32.into(), &mut weight_meter);
		}

		assert_eq!(OnPollStatus::<T>::get(), OnPollState::XcmFundsTransfer);
		assert!(!XcmTransferInitiatedAt::<T>::exists());

		Ok(())
	}

	#[benchmark]
	fn fund_pots() -> Result<(), BenchmarkError> {
		use frame_support::traits::Hooks;

		OnPollStatus::<T>::set(OnPollState::FundingPots);

		let (asset_id, owner, is_sufficient, min_balance) = get_transfer_asset_configuration::<T>();
		<T as Config>::Assets::create(asset_id.clone(), owner, is_sufficient, min_balance)?;

		simulate_xcm_transfer_success::<T>()?;

		let mut weight_meter = WeightMeter::new();

		#[block]
		{
			Pallet::<T>::on_poll(1u32.into(), &mut weight_meter);
		}

		// The MockAssets always succeeds, so the funding operation completes successfully.
		assert_eq!(OnPollStatus::<T>::get(), OnPollState::SettingPeopleLiteAttestationAllowances);

		Ok(())
	}

	#[benchmark]
	fn on_poll_set_people_lite_attestation_allowances() -> Result<(), BenchmarkError> {
		use frame_support::traits::Hooks;

		OnPollStatus::<T>::set(OnPollState::SettingPeopleLiteAttestationAllowances);

		let mut weight_meter = WeightMeter::new();

		#[block]
		{
			Pallet::<T>::on_poll(1u32.into(), &mut weight_meter);
		}

		assert_eq!(OnPollStatus::<T>::get(), OnPollState::SchedulingMobRulePayouts);

		Ok(())
	}

	#[benchmark]
	fn on_poll_schedule_mob_rule_payouts() -> Result<(), BenchmarkError> {
		use frame_support::traits::Hooks;

		OnPollStatus::<T>::set(OnPollState::SchedulingMobRulePayouts);

		let mut weight_meter = WeightMeter::new();

		#[block]
		{
			Pallet::<T>::on_poll(1u32.into(), &mut weight_meter);
		}

		assert_eq!(OnPollStatus::<T>::get(), OnPollState::SchedulingScorePayouts);
		assert_eq!(indiv_pallet_mob_rule::RoundSchedules::<T>::get().len(), 1);

		Ok(())
	}

	#[benchmark]
	fn on_poll_schedule_score_payouts() -> Result<(), BenchmarkError> {
		use frame_support::traits::Hooks;

		OnPollStatus::<T>::set(OnPollState::SchedulingScorePayouts);

		let mut weight_meter = WeightMeter::new();

		#[block]
		{
			Pallet::<T>::on_poll(1u32.into(), &mut weight_meter);
		}

		assert_eq!(OnPollStatus::<T>::get(), OnPollState::Done);
		let new_schedules = indiv_pallet_score::RoundSchedules::<T>::get();
		assert_eq!(new_schedules.len(), 1);

		Ok(())
	}

	#[benchmark]
	fn set_proof_of_ink_reimbursement_values() -> Result<(), BenchmarkError> {
		let mut weight_meter = WeightMeter::new();
		let (expected_referred, expected_referrer) = get_initial_reimbursement_values::<T>();

		let cursor = Some(MigrationState::SettingProofOfInkReimbursementValues);

		#[block]
		{
			InitializeIndividualityPallets::<T>::step(cursor, &mut weight_meter).unwrap();
		}

		assert_eq!(
			indiv_pallet_proof_of_ink::ReferredReimbursementValues::<T>::get(),
			Some(expected_referred),
		);
		assert_eq!(
			indiv_pallet_proof_of_ink::ReferrerReimbursementValues::<T>::get(),
			Some(expected_referrer),
		);

		Ok(())
	}

	#[benchmark]
	fn set_people_lite_attestation_allowances(n: Linear<1, 100>) -> Result<(), BenchmarkError> {
		use frame_benchmarking::account;

		// Setup: create n accounts
		let accounts: sp_std::vec::Vec<(T::AccountId, u32)> = (0..n)
			.map(|i| {
				let account = account("attestation_recipient", i, 0);
				(account, 100u32)
			})
			.collect();

		#[block]
		{
			for (account, count) in accounts {
				indiv_pallet_people_lite::Pallet::<T>::increase_attestation_allowance(
					frame_system::RawOrigin::Root.into(),
					account,
					count,
				)
				.unwrap();
			}
		}

		Ok(())
	}

	#[benchmark]
	fn grant_invites_migration() -> Result<(), BenchmarkError> {
		const INVITES_COUNT: u32 = 100;

		let mut weight_meter = WeightMeter::new();
		let recipient = <T as Config>::InvitesRecipient::get();
		let cursor = Some(MigrationState::GrantingInvites);

		#[block]
		{
			InitializeIndividualityPallets::<T>::step(cursor, &mut weight_meter).unwrap();
		}

		assert_eq!(
			indiv_pallet_proof_of_ink::AvailableInvites::<T>::get(&recipient),
			INVITES_COUNT,
		);
		assert_eq!(indiv_pallet_game::AvailableInvites::<T>::get(&recipient), INVITES_COUNT,);
		frame_system::Pallet::<T>::assert_has_event(Event::<T>::MigrationInvitesGranted.into());

		Ok(())
	}

	impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
