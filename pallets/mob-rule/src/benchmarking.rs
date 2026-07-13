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

//! Mob Rule pallet benchmarking.

extern crate alloc;

use super::*;

use crate::{
	testing_utils::{constants::PERSON_0_ALIAS, helpers},
	ActiveSince,
};
use alloc::vec;
use frame_benchmarking::v2::*;
use frame_support::{
	assert_ok,
	pallet_prelude::Get,
	traits::{
		fungible::{Mutate, MutateHold},
		OnPoll, UnfilteredDispatchable, UnixTime,
	},
	weights::WeightMeter,
};
use indiv_support::traits::{CountedMembers, Judgement, Truth};
// TODO: remove once mob-rule migrates to `#[pallet::authorize]` (deadline April 2027).
// See https://github.com/paritytech/polkadot-sdk/issues/2415.
#[allow(deprecated)]
use sp_runtime::traits::ValidateUnsigned;

pub trait BenchmarkHelper<T: Config> {
	/// Sets a valid time for benchmarks (moves away from genesis block).
	fn set_valid_time();
	/// Ensure the currency is correctly setup to mint and hold and transfer etc..
	fn setup_currency();
}

// --- Helpers

fn assert_last_event<T: Config>(generic_event: <T as frame_system::Config>::RuntimeEvent) {
	frame_system::Pallet::<T>::assert_last_event(generic_event.into());
}

#[benchmarks(
	where T: Config + core::marker::Send + core::marker::Sync,
)]
mod benches {
	use super::*;
	use frame_support::{dispatch::RawOrigin, traits::EnsureOriginWithArg};
	use frame_system::pallet_prelude::BlockNumberFor;

	// Worst case: Contempt vote that ripens the case.
	#[benchmark]
	fn vote() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::set_valid_time();
		T::BenchmarkHelper::setup_currency();

		let case_index = helpers::create_voting_case::<T>();

		// Force the ripening branch: pass the duration check, and make the result definitive.
		OpenCases::<T>::mutate(case_index, |maybe_case| {
			if let Some(case) = maybe_case {
				case.since = 0;
			}
		});
		T::EnsurePerson::set_active_count(1);

		let origin = T::EnsurePerson::try_successful_origin(&MOB_CONTEXT)
			.map_err(|_| BenchmarkError::Weightless)?;
		let voter_alias = T::EnsurePerson::ensure_origin(origin.clone(), &MOB_CONTEXT).unwrap();

		let opinion = Judgement::Contempt;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, case_index, opinion);

		assert!(Votes::<T>::contains_key(case_index, voter_alias));
		assert!(RipeCases::<T>::contains_key(case_index));
		assert!(!OpenCases::<T>::contains_key(case_index));
		assert_last_event::<T>(Event::Voted { case_index, voter: voter_alias, opinion }.into());

		Ok(())
	}

	// Includes the cost of `ValidateUnsigned::pre_dispatch`
	#[benchmark]
	fn close_case() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::set_valid_time();
		T::BenchmarkHelper::setup_currency();

		let case_index = helpers::create_ripe_case::<T>();

		assert!(RipeCases::<T>::contains_key(case_index));

		let call = Call::<T>::close_case { case_index };

		#[block]
		{
			#[allow(deprecated)]
			<Pallet<T> as ValidateUnsigned>::pre_dispatch(&call)
				.expect("pre-dispatch must succeed");
			call.dispatch_bypass_filter(RawOrigin::None.into())?;
		}

		assert!(!RipeCases::<T>::contains_key(case_index));
		assert!(DoneCases::<T>::contains_key(case_index));
		assert_last_event::<T>(
			Event::CaseClosed { case_index, verdict: Judgement::Truth(Truth::True) }.into(),
		);

		Ok(())
	}

	// Worst case: Contempt vote against a non-Contempt verdict triggers the penalty branch.
	// Includes `ValidateUnsigned::pre_dispatch`.
	#[benchmark]
	fn clean_vote() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::set_valid_time();
		T::BenchmarkHelper::setup_currency();

		let case_index = helpers::create_done_case::<T>(vec![PERSON_0_ALIAS], 0);
		// Disagree with the seeded verdict to hit the penalty branch.
		Votes::<T>::insert(case_index, PERSON_0_ALIAS, Judgement::Contempt);

		assert!(Votes::<T>::contains_key(case_index, PERSON_0_ALIAS));
		assert!(!VotingPenalties::<T>::contains_key(PERSON_0_ALIAS));

		let call = Call::<T>::clean_vote { case_index, voter: PERSON_0_ALIAS };

		#[block]
		{
			#[allow(deprecated)]
			<Pallet<T> as ValidateUnsigned>::pre_dispatch(&call)
				.expect("pre-dispatch must succeed");
			call.dispatch_bypass_filter(RawOrigin::None.into())?;
		}

		assert!(!Votes::<T>::contains_key(case_index, PERSON_0_ALIAS));
		assert!(VotingPenalties::<T>::contains_key(PERSON_0_ALIAS));
		assert_last_event::<T>(Event::VoteCleaned { case_index, voter: PERSON_0_ALIAS }.into());

		Ok(())
	}

	// Includes the cost of `ValidateUnsigned::pre_dispatch`
	#[benchmark]
	fn reap_case() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::set_valid_time();
		T::BenchmarkHelper::setup_currency();
		let now = T::Clock::now().as_secs();
		let max_claim_duration = T::MaxVoteClaimDuration::get();
		let old_time = now.saturating_sub(max_claim_duration + 3600); // 1 hour buffer
		let case_index = helpers::create_done_case::<T>(vec![], old_time);

		assert!(DoneCases::<T>::contains_key(case_index));

		let call = Call::<T>::reap_case { case_index };

		#[block]
		{
			#[allow(deprecated)]
			<Pallet<T> as ValidateUnsigned>::pre_dispatch(&call)
				.expect("pre-dispatch must succeed");
			call.dispatch_bypass_filter(RawOrigin::None.into())?;
		}

		assert!(!DoneCases::<T>::contains_key(case_index));
		assert_last_event::<T>(Event::CaseRemoved { case_index }.into());

		Ok(())
	}

	#[benchmark]
	fn intervene() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::set_valid_time();
		T::BenchmarkHelper::setup_currency();

		let case_index = helpers::create_voting_case::<T>();
		let verdict = Judgement::Truth(Truth::False);

		assert!(OpenCases::<T>::contains_key(case_index));

		#[extrinsic_call]
		_(RawOrigin::Root, case_index, verdict);

		assert!(!OpenCases::<T>::contains_key(case_index));
		assert!(DoneCases::<T>::contains_key(case_index));
		assert_last_event::<T>(Event::CaseIntervened { case_index, verdict }.into());

		Ok(())
	}

	// Worst case: Contempt vote against a non-Contempt verdict hits the penalty branch.
	#[benchmark]
	fn claim_vote() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::set_valid_time();
		T::BenchmarkHelper::setup_currency();

		let origin = T::EnsurePerson::try_successful_origin(&MOB_CONTEXT)
			.map_err(|_| BenchmarkError::Weightless)?;
		let voter_alias = T::EnsurePerson::ensure_origin(origin.clone(), &MOB_CONTEXT).unwrap();
		let case_index = helpers::create_done_case::<T>(vec![voter_alias], 0);
		// Disagree with the seeded verdict to hit the penalty branch.
		Votes::<T>::insert(case_index, voter_alias, Judgement::Contempt);

		assert!(!VotingPenalties::<T>::contains_key(voter_alias));

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, case_index);

		assert!(VotingPenalties::<T>::contains_key(voter_alias));
		assert_last_event::<T>(
			Event::VotesClaimed { voter: voter_alias, case_indices: alloc::vec![case_index] }
				.into(),
		);

		Ok(())
	}

	// Contempt vs non-Contempt verdict hits the penalty branch.
	#[benchmark]
	fn claim_votes(v: Linear<1, { T::MaxVotesClaimable::get() }>) -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::set_valid_time();
		T::BenchmarkHelper::setup_currency();

		let origin = T::EnsurePerson::try_successful_origin(&MOB_CONTEXT)
			.map_err(|_| BenchmarkError::Weightless)?;
		let voter_alias = T::EnsurePerson::ensure_origin(origin.clone(), &MOB_CONTEXT).unwrap();
		let mut case_indices = alloc::vec![];

		for _ in 0..v {
			let case_index = helpers::create_voting_case::<T>();
			Votes::<T>::insert(case_index, voter_alias, Judgement::Contempt);

			OpenCases::<T>::remove(case_index);
			let now = T::Clock::now().as_secs();
			let done_case = DoneCase {
				since: now.saturating_sub(T::VotesOpenForClaimsDuration::get() as u64 + 3600),
				verdict: Judgement::Truth(Truth::True),
			};
			DoneCases::<T>::insert(case_index, done_case);
			case_indices.push(case_index);
		}

		assert!(!VotingPenalties::<T>::contains_key(voter_alias));

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, case_indices.clone());

		assert!(VotingPenalties::<T>::contains_key(voter_alias));
		assert_last_event::<T>(Event::VotesClaimed { voter: voter_alias, case_indices }.into());

		Ok(())
	}

	#[benchmark]
	fn payout_rewards() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::set_valid_time();
		T::BenchmarkHelper::setup_currency();

		let origin = T::EnsurePerson::try_successful_origin(&MOB_CONTEXT)
			.map_err(|_| BenchmarkError::Weightless)?;
		let voter_alias = T::EnsurePerson::ensure_origin(origin.clone(), &MOB_CONTEXT).unwrap();
		let reward_amount: BalanceOf<T> = 10u32.into();

		let pot = Pallet::<T>::mob_rule_pot_id();
		let destination: T::AccountId = account("destination", 0, 0);

		helpers::fund_pot::<T>();
		let _ = T::Currency::mint_into(&pot, reward_amount);

		Credits::<T>::insert(
			voter_alias,
			MobCredit { voted: 1, cleaned: 1, correct: 1, credit: reward_amount },
		);
		T::Currency::hold(&HoldReason::Credit.into(), &pot, reward_amount)?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, destination.clone());

		assert_last_event::<T>(
			Event::RewardPayout { voter: voter_alias, destination, amount: reward_amount }.into(),
		);

		Ok(())
	}

	// A previous `PayoutDistribution` with leftover balance triggers the recycle
	// branch, and the schedule is dec'd to zero so it's removed from the Vec.
	#[benchmark]
	fn start_payout_round() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::setup_currency();

		AccumulatedPoints::<T>::put(100u64);

		let amount: BalanceOf<T> = 1000u32.into();
		let count = 1u32;
		let period = 0u32.into();

		helpers::fund_pot::<T>();

		assert_ok!(Pallet::<T>::schedule_payout_rounds(
			RawOrigin::Root.into(),
			amount,
			count,
			period
		));
		assert!(!RoundSchedules::<T>::get().is_empty());

		// Inject a previous distribution with non-zero `remaining_balance` so the recycle branch
		// fires. The hold placed by `schedule_payout_rounds` covers this amount.
		PayoutDistribution::<T>::put(CreditDistribution {
			round: 0,
			initial_balance: amount,
			remaining_balance: 1u32.into(),
			total_points: 50,
			start: 0u32.into(),
		});

		#[extrinsic_call]
		_(RawOrigin::Root);

		assert!(PayoutDistribution::<T>::exists());
		assert!(RoundSchedules::<T>::get().is_empty());

		Ok(())
	}

	#[benchmark]
	fn schedule_payout_rounds() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::setup_currency();

		let amount = 1000u32.into();
		let count = 5u32;
		let period = 100u32.into();

		helpers::fund_pot::<T>();

		#[extrinsic_call]
		_(RawOrigin::Root, amount, count, period);

		assert!(!RoundSchedules::<T>::get().is_empty());

		Ok(())
	}

	// `RoundSchedules` is full to `MaxPayoutRoundSchedules` and we remove index 0,
	// forcing a maximum-length `Vec::remove` shift.
	#[benchmark]
	fn remove_payout_schedule() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::setup_currency();

		let amount: BalanceOf<T> = 100u32.into();
		let count = 1u32;
		let period = 50u32.into();
		let max_schedules = T::MaxPayoutRoundSchedules::get();

		helpers::fund_pot::<T>();

		for _ in 0..max_schedules {
			assert_ok!(Pallet::<T>::schedule_payout_rounds(
				RawOrigin::Root.into(),
				amount,
				count,
				period,
			));
		}
		assert_eq!(RoundSchedules::<T>::get().len() as u32, max_schedules);

		#[extrinsic_call]
		_(RawOrigin::Root, 0u32);

		assert_eq!(RoundSchedules::<T>::get().len() as u32, max_schedules - 1);

		Ok(())
	}

	#[benchmark]
	fn claim_credit() -> Result<(), BenchmarkError> {
		let amount: BalanceOf<T> = 1000u32.into();

		T::BenchmarkHelper::set_valid_time();
		T::BenchmarkHelper::setup_currency();

		let origin = T::EnsurePerson::try_successful_origin(&MOB_CONTEXT)
			.map_err(|_| BenchmarkError::Weightless)?;
		let voter_alias = T::EnsurePerson::ensure_origin(origin.clone(), &MOB_CONTEXT).unwrap();

		let case_index = helpers::create_done_case::<T>(vec![voter_alias], 0);

		// To generate credits
		let _ = Pallet::<T>::claim_vote(origin.clone(), case_index);

		AccumulatedPoints::<T>::put(100u64);
		helpers::fund_pot::<T>();

		// Schedule and start payout rounds to create distribution
		let _ = Pallet::<T>::schedule_payout_rounds(
			RawOrigin::Root.into(),
			amount,
			1u32,
			100u32.into(),
		);
		let _ = Pallet::<T>::start_payout_round(RawOrigin::Root.into());

		// Ensure voting points exist for the current payout round and voter
		let distribution =
			PayoutDistribution::<T>::get().expect("Payout distribution should exist");

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin);

		// Verify credit was claimed (voting points should be removed)
		assert!(!VotingPoints::<T>::contains_key(distribution.round, voter_alias));

		Ok(())
	}

	#[benchmark]
	fn clean_points() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::setup_currency();

		PayoutDistribution::<T>::put(CreditDistribution {
			round: 1,
			initial_balance: 1000u32.into(),
			remaining_balance: 1000u32.into(),
			total_points: 100,
			start: 0u32.into(),
		});
		VotingPoints::<T>::insert(0u32, PERSON_0_ALIAS, 10u32);

		#[extrinsic_call]
		_(RawOrigin::Signed(account("acc", 0, 0)), 0u32, PERSON_0_ALIAS);

		assert_eq!(VotingPoints::<T>::get(0u32, PERSON_0_ALIAS), 0);

		Ok(())
	}

	// Includes the cost of `ValidateUnsigned::pre_dispatch`
	#[benchmark]
	fn force_ripen_case() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::setup_currency();
		T::BenchmarkHelper::set_valid_time();
		ActiveSince::<T>::put(0);

		let case_index = helpers::create_voting_case::<T>();

		// The case has to be old enough
		if let Some(mut case) = OpenCases::<T>::get(case_index) {
			let now = T::Clock::now().as_secs();
			let max_voting_duration = T::MaxVotingDuration::get() as u64;
			case.since = now.saturating_sub(max_voting_duration + 3600); // 1 hour buffer
			OpenCases::<T>::insert(case_index, case);
		}

		let call = Call::<T>::force_ripen_case { case_index };

		#[block]
		{
			#[allow(deprecated)]
			<Pallet<T> as ValidateUnsigned>::pre_dispatch(&call)
				.expect("pre-dispatch must succeed");
			call.dispatch_bypass_filter(RawOrigin::None.into())?;
		}

		assert!(RipeCases::<T>::contains_key(case_index));

		Ok(())
	}

	// sets up a case which meets all ripening conditions, so the call writes `RipeCases`
	#[benchmark]
	fn touch_case() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::set_valid_time();
		T::BenchmarkHelper::setup_currency();

		let case_index = helpers::create_voting_case::<T>();

		// Cast a vote to meet minimum turnout requirements
		let origin = T::EnsurePerson::try_successful_origin(&MOB_CONTEXT)
			.map_err(|_| BenchmarkError::Weightless)?;
		let _ = Pallet::<T>::vote(origin, case_index, Judgement::Truth(Truth::True))?;

		// Ensure the case meets the time requirements to become ripe
		if let Some(mut case) = OpenCases::<T>::get(case_index) {
			let now = T::Clock::now().as_secs();
			let min_case_duration = T::MinCaseDuration::get() as u64;
			case.since = now.saturating_sub(min_case_duration + 3600); // 1 hour buffer
			OpenCases::<T>::insert(case_index, case);
		}
		T::EnsurePerson::set_active_count(1);

		#[extrinsic_call]
		_(RawOrigin::Signed(account("acc", 0, 0)), case_index);

		assert!(RipeCases::<T>::contains_key(case_index));

		Ok(())
	}

	#[benchmark]
	fn clear_voting_penalty() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::setup_currency();
		let origin = T::EnsurePerson::try_successful_origin(&MOB_CONTEXT)
			.map_err(|_| BenchmarkError::Weightless)?;
		let voter_alias = T::EnsurePerson::ensure_origin(origin.clone(), &MOB_CONTEXT).unwrap();

		let penalty_start_block = BlockNumberFor::<T>::from(0u32);
		VotingPenalties::<T>::insert(voter_alias, penalty_start_block);

		// Advance block number to ensure the penalty has expired
		let current_block = frame_system::Pallet::<T>::block_number();
		let target_block = current_block + T::VotingPenaltyDuration::get() + 1u32.into();
		frame_system::Pallet::<T>::set_block_number(target_block);

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin);

		assert!(VotingPenalties::<T>::get(voter_alias).is_none());

		Ok(())
	}

	#[benchmark]
	fn on_poll_base() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::setup_currency();
		T::BenchmarkHelper::set_valid_time();

		let mut meter = WeightMeter::new();
		T::EnsurePerson::set_active_count(T::MinimumVoterThreshold::get());
		ActiveSince::<T>::put(T::Clock::now().as_secs());

		#[block]
		{
			pallet::Pallet::<T>::on_poll(0u32.into(), &mut meter);
		}

		assert_eq!(meter.consumed(), T::WeightInfo::on_poll_base());
		Ok(())
	}

	#[benchmark]
	fn set_active_since() -> Result<(), BenchmarkError> {
		ActiveSince::<T>::kill();

		#[block]
		{
			ActiveSince::<T>::put(0);
		}

		Ok(())
	}

	#[benchmark]
	fn kill_active_since() -> Result<(), BenchmarkError> {
		ActiveSince::<T>::put(0);

		#[block]
		{
			ActiveSince::<T>::kill();
		}

		Ok(())
	}

	// Implements a test for each benchmark. Execute with:
	// `cargo test -p indiv-pallet-mob-rule --features runtime-benchmarks`.
	impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
