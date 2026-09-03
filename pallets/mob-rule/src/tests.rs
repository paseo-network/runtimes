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

pub type AccountId = <Test as frame_system::Config>::AccountId;
use crate::{
	mock::{MobRule, TestExt, *},
	testing_utils::{constants, helpers},
	*,
};
use frame_support::{
	assert_ok, dispatch::GetDispatchInfo, pallet_prelude::Authorize, traits::UnixTime,
	weights::Weight,
};
use sp_core::Get;
use std::time::Duration;

pub const VOTER_VALID: AccountId = 1;
pub const VOTER_INVALID: AccountId = 6;

const VOTER_VALID_2: AccountId = 2;
const VOTER_VALID_3: AccountId = 3;
const VOTER_VALID_4: AccountId = 3;

const MOCK_ACCOUNT_ID1: AccountId = 11;

fn authorized_origin() -> RuntimeOrigin {
	frame_system::RawOrigin::Authorized.into()
}

/// Computes the exact weight of a ripe case's callback, i.e. the `max_callback_weight` value
/// that `authorize_close_case` accepts for `close_case`.
fn ripe_case_callback_weight(case_index: CaseIndex) -> Weight {
	let case = RipeCases::<Test>::get(case_index).unwrap();
	MobRule::callback_weight(case_index, &case.details.callback, case.details.context, case.verdict)
}

mod voting {
	use super::*;
	use frame_support::{assert_noop, dispatch::Pays};
	use indiv_support::traits::Truth;
	use sp_runtime::DispatchError::BadOrigin;

	#[test]
	/// For more information, check
	/// [`EnsureAliasLowerThan5`](EnsureAliasLowerThan5) implementation in `mock.rs`
	/// which is used as the personhood guard for the MobRule pallet.
	fn voting_access_gated_by_ensure_person_guard() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			let case_index = helpers::create_voting_case::<Test>();

			// The voter with no rights votes
			assert_noop!(
				MobRule::vote(
					RuntimeOrigin::signed(VOTER_INVALID),
					case_index,
					Judgement::Truth(Truth::True),
				),
				BadOrigin
			);

			// The voter with valid rights votes
			let alias = EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID));
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID),
				case_index,
				Judgement::Truth(Truth::True),
			));
			System::assert_has_event(
				Event::Voted { case_index, voter: alias, opinion: Judgement::Truth(Truth::True) }
					.into(),
			);
		});
	}

	#[test]
	fn fails_if_case_not_exists() {
		TestExt::new().execute_with(|| {
			assert_noop!(
				MobRule::vote(RuntimeOrigin::signed(VOTER_VALID), 0, Judgement::Truth(Truth::True),),
				Error::<Test>::NotOpen,
			);
		});
	}

	#[test]
	fn voter_votes_twice() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			let case_index = helpers::create_voting_case::<Test>();
			let alias = EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID));

			// First vote cast
			let voting_result = MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID),
				case_index,
				Judgement::Truth(Truth::True),
			);
			assert_ok!(voting_result);
			assert_eq!(voting_result.unwrap().pays_fee, Pays::No);
			System::assert_has_event(
				Event::Voted { case_index, voter: alias, opinion: Judgement::Truth(Truth::True) }
					.into(),
			);

			// Second vote cast - this time with different judgement
			let voting_result = MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID),
				case_index,
				Judgement::Truth(Truth::False),
			);
			assert_ok!(voting_result);
			// This time the voter will have to pay
			assert_eq!(voting_result.unwrap().pays_fee, Pays::Yes);
			System::assert_has_event(
				Event::Voted { case_index, voter: alias, opinion: Judgement::Truth(Truth::False) }
					.into(),
			);

			// The stored vote of the voter must be the last one
			assert_eq!(
				Votes::<Test>::get(case_index, alias).unwrap(),
				Judgement::Truth(Truth::False)
			);
		});
	}

	#[test]
	fn voter_votes_contempt_under_penalty() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			let case_index = helpers::create_voting_case::<Test>();

			let alias = EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID));
			VotingPenalties::<Test>::insert(alias, 0);

			// First vote cast with `Contempt`, but the voter has a penalty.
			assert_noop!(
				MobRule::vote(RuntimeOrigin::signed(VOTER_VALID), case_index, Judgement::Contempt,),
				Error::<Test>::UnderPenalty
			);

			// Second vote cast - this time with different judgement.
			let voting_result = MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID),
				case_index,
				Judgement::Truth(Truth::False),
			);
			assert_ok!(voting_result);
			assert_eq!(voting_result.unwrap().pays_fee, Pays::No);
			System::assert_has_event(
				Event::Voted { case_index, voter: alias, opinion: Judgement::Truth(Truth::False) }
					.into(),
			);
		});
	}
}

mod case_lifecycle {
	use super::*;
	use core::time::Duration;
	use frame_support::{assert_noop, dispatch::Pays};
	use indiv_support::traits::Truth;

	#[test]
	fn case_becomes_ripe_after_timeout() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			EnsureAliasLowerThan5::set_voter_count(1);
			// GIVEN an open case exists
			let case_index = helpers::create_voting_case::<Test>();

			// AND two days have passed since it's opening
			mock::Now::set(Duration::from_millis(constants::TWO_DAYS_MS));

			// WHEN a voter votes in the case
			let alias = EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID));
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID),
				case_index,
				Judgement::Truth(Truth::True),
			));
			System::assert_has_event(
				Event::Voted { case_index, voter: alias, opinion: Judgement::Truth(Truth::True) }
					.into(),
			);

			// THEN the case is moved from open to ripe cases
			assert!(OpenCases::<Test>::get(case_index).is_none());
			assert!(RipeCases::<Test>::get(case_index).is_some());
		});
	}

	#[test]
	fn case_timeout_timeout_with_too_few_votes() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			EnsureAliasLowerThan5::set_voter_count(2);
			ActiveSince::<Test>::put(<Test as Config>::Clock::now().as_secs());
			// GIVEN an open case exists
			let case_index = helpers::create_voting_case::<Test>();

			// WHEN a voter votes in the case
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID),
				case_index,
				Judgement::Truth(Truth::True),
			));

			assert_noop!(
				MobRule::force_ripen_case(authorized_origin(), case_index),
				Error::<Test>::Recent
			);

			// AND almost two weeks have passed since it's opening
			mock::Now::set(Duration::from_millis(constants::TWO_WEEKS_MS - 1));

			// Still early.
			assert_noop!(
				MobRule::force_ripen_case(authorized_origin(), case_index),
				Error::<Test>::Recent
			);

			// AND the full period has passed now.
			mock::Now::set(Duration::from_millis(constants::TWO_WEEKS_MS));

			assert_ok!(MobRule::force_ripen_case(authorized_origin(), case_index),);

			// THEN the case is moved from open to ripe cases
			assert!(OpenCases::<Test>::get(case_index).is_none());
			assert!(RipeCases::<Test>::get(case_index).is_some());
			System::assert_has_event(
				Event::CaseClosed { case_index, verdict: Judgement::Truth(Truth::True) }.into(),
			);
		});
	}

	#[test]
	fn case_timeout_timeout_with_inactivity_period() {
		TestExt::new().execute_with(|| {
			// This test will pass through time periods iteratively, one week each step. It will
			// create a new case every week, while the normal maximum duration for a case is 2
			// weeks.
			//
			// The voting status will be simulated as follows:
			// - week 0 starts with voting disabled
			// - week 1 enables voting again
			// - week 3 disables voting
			// - week 4 enables voting again
			//
			// Below is a time chart of the open cases, with T being the time dimension and the
			// cases below, indexed by their number. The periods of activity are marked between `()`
			// on the time axis. On the cases graph, `=` represents regular time elapsed while `-`
			// represents time elapsed due to inactivity.
			//
			// T --(-----)---(-----
			// 0 ======---
			// 1    ======
			// 2       ======------
			// 3          ======---
			// 4             ======

			EnsureAliasLowerThan5::set_voter_count(3);
			ActiveSince::<Test>::kill();
			let case_1 = helpers::create_voting_case::<Test>();
			assert_noop!(
				MobRule::force_ripen_case(authorized_origin(), case_1),
				Error::<Test>::CaseExpirationDisabled
			);

			mock::Now::set(Duration::from_millis(constants::TWO_WEEKS_MS / 2));
			let case_2 = helpers::create_voting_case::<Test>();
			ActiveSince::<Test>::put(<Test as Config>::Clock::now().as_secs());
			assert_noop!(
				MobRule::force_ripen_case(authorized_origin(), case_1),
				Error::<Test>::Recent
			);
			assert_noop!(
				MobRule::force_ripen_case(authorized_origin(), case_2),
				Error::<Test>::Recent
			);

			mock::Now::set(Duration::from_millis(constants::TWO_WEEKS_MS));
			let case_3 = helpers::create_voting_case::<Test>();
			assert_noop!(
				MobRule::force_ripen_case(authorized_origin(), case_1),
				Error::<Test>::Recent
			);
			assert_noop!(
				MobRule::force_ripen_case(authorized_origin(), case_2),
				Error::<Test>::Recent
			);
			assert_noop!(
				MobRule::force_ripen_case(authorized_origin(), case_3),
				Error::<Test>::Recent
			);

			// Pass enough time for case 1 and case 2 to finish, but kill the storage value only
			// after they are processed because otherwise the new inactivity period would not allow
			// them to be reaped.
			mock::Now::set(Duration::from_millis(constants::TWO_WEEKS_MS * 3 / 2));
			let case_4 = helpers::create_voting_case::<Test>();
			assert_ok!(MobRule::force_ripen_case(authorized_origin(), case_1));
			assert_ok!(MobRule::force_ripen_case(authorized_origin(), case_2));

			ActiveSince::<Test>::kill();
			assert_noop!(
				MobRule::force_ripen_case(authorized_origin(), case_3),
				Error::<Test>::CaseExpirationDisabled
			);
			assert_noop!(
				MobRule::force_ripen_case(authorized_origin(), case_4),
				Error::<Test>::CaseExpirationDisabled
			);

			mock::Now::set(Duration::from_millis(constants::TWO_WEEKS_MS * 2));
			ActiveSince::<Test>::put(<Test as Config>::Clock::now().as_secs());
			let case_5 = helpers::create_voting_case::<Test>();
			assert_noop!(
				MobRule::force_ripen_case(authorized_origin(), case_3),
				Error::<Test>::Recent
			);
			assert_noop!(
				MobRule::force_ripen_case(authorized_origin(), case_4),
				Error::<Test>::Recent
			);
			assert_noop!(
				MobRule::force_ripen_case(authorized_origin(), case_5),
				Error::<Test>::Recent
			);

			mock::Now::set(Duration::from_millis(constants::TWO_WEEKS_MS * 5 / 2));
			assert_noop!(
				MobRule::force_ripen_case(authorized_origin(), case_3),
				Error::<Test>::Recent
			);
			assert_noop!(
				MobRule::force_ripen_case(authorized_origin(), case_4),
				Error::<Test>::Recent
			);
			assert_noop!(
				MobRule::force_ripen_case(authorized_origin(), case_5),
				Error::<Test>::Recent
			);

			mock::Now::set(Duration::from_millis(constants::TWO_WEEKS_MS * 3));
			assert_ok!(MobRule::force_ripen_case(authorized_origin(), case_3));
			assert_ok!(MobRule::force_ripen_case(authorized_origin(), case_4));
			assert_ok!(MobRule::force_ripen_case(authorized_origin(), case_5));
		});
	}

	#[test]
	fn case_touch_works() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			EnsureAliasLowerThan5::set_voter_count(5);
			// GIVEN an open case exists
			let case_index = helpers::create_voting_case::<Test>();

			// WHEN 2 out of 5 voters vote in the case, approval is 50%
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID),
				case_index,
				Judgement::Truth(Truth::True),
			));
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID_2),
				case_index,
				Judgement::Truth(Truth::False),
			));

			assert!(OpenCases::<Test>::get(case_index).is_some());
			assert!(RipeCases::<Test>::get(case_index).is_none());

			// AND two days have passed since it's opening
			mock::Now::set(Duration::from_millis(constants::TWO_DAYS_MS));

			// 3rd vote coming in, approval is 66.6%
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID_3),
				case_index,
				Judgement::Truth(Truth::True),
			));
			// The case is still too fresh.
			assert!(OpenCases::<Test>::get(case_index).is_some());
			assert!(RipeCases::<Test>::get(case_index).is_none());

			// We're right on the edge of the required time to pass in order to allow the vote to go
			// through with 2/3 of the vote.
			mock::Now::set(Duration::from_millis(constants::TWO_WEEKS_MS * 2 / 3));
			// We update the case.
			assert_ok!(MobRule::touch_case(RuntimeOrigin::signed(VOTER_VALID), case_index),);
			// But it still doesn't become ripe.
			assert!(OpenCases::<Test>::get(case_index).is_some());
			assert!(RipeCases::<Test>::get(case_index).is_none());
			// AND the event is emitted with ripened: false
			System::assert_has_event(Event::CaseTouched { case_index, ripened: false }.into());

			// AND we're past the full period now.
			mock::Now::set(Duration::from_millis(
				constants::TWO_WEEKS_MS * 2 / 3 + constants::ONE_HOUR_MS,
			));
			// We update the case.
			assert_ok!(MobRule::touch_case(RuntimeOrigin::signed(VOTER_VALID), case_index),);
			// THEN the case is moved from open to ripe cases
			assert!(OpenCases::<Test>::get(case_index).is_none());
			assert!(RipeCases::<Test>::get(case_index).is_some());
			// AND the event is emitted with ripened: true
			System::assert_has_event(Event::CaseTouched { case_index, ripened: true }.into());
		});
	}

	#[test]
	fn open_case_cannot_be_closed() {
		TestExt::new().execute_with(|| {
			// GIVEN an open case exists
			let case_index = helpers::create_voting_case::<Test>();

			// WHEN close_case is called on it,
			// THEN it fails with relevant error
			assert_noop!(
				MobRule::close_case(authorized_origin(), case_index, Weight::MAX),
				Error::<Test>::NotRipe
			);
		});
	}

	#[test]
	fn ripe_case_can_be_closed() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			// GIVEN one ripe case exists
			let case_index = helpers::create_ripe_case::<Test>();

			// WHEN close_case is called on it
			assert!(MobRule::close_case(authorized_origin(), case_index, Weight::MAX).is_ok());

			// THEN it is removed from RipeCases and inserted to DoneCases
			assert!(OpenCases::<Test>::get(case_index).is_none());
			assert!(RipeCases::<Test>::get(case_index).is_none());
			assert!(DoneCases::<Test>::get(case_index).is_some());
			System::assert_has_event(
				Event::CaseClosed { case_index, verdict: Judgement::Truth(Truth::True) }.into(),
			);
		});
	}

	#[test]
	fn close_case_emits_structured_callback_error_for_invalid_callback() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			let case_index = helpers::create_ripe_case::<Test>();

			RipeCases::<Test>::mutate(case_index, |case| {
				case.as_mut().unwrap().details.callback = Callback::from_parts(u8::MAX, u8::MAX);
			});

			assert_ok!(MobRule::close_case(authorized_origin(), case_index, Weight::MAX));

			System::assert_has_event(
				Event::CallbackError { case_index, trigger: CallbackTrigger::CloseCase }.into(),
			);
		});
	}

	#[test]
	fn close_case_refunds_unused_callback_weight() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			let case_index = helpers::create_ripe_case::<Test>();
			let case = RipeCases::<Test>::get(case_index).unwrap();
			let callback_weight = case
				.details
				.callback
				.curry((case_index, case.details.context, case.verdict))
				.unwrap()
				.get_dispatch_info()
				.call_weight;

			// Declare a bound generously above the callback's actual weight so a refund is due.
			let max_callback_weight =
				callback_weight.saturating_add(Weight::from_parts(1_000, 1_000));
			let declared = Call::<Test>::close_case { case_index, max_callback_weight }
				.get_dispatch_info()
				.call_weight;

			let post = MobRule::close_case(authorized_origin(), case_index, max_callback_weight)
				.expect("ripe case can be closed");

			let expected =
				<Test as Config>::WeightInfo::close_case().saturating_add(callback_weight);
			assert_eq!(post.actual_weight, Some(expected));
			assert!(post.actual_weight.unwrap().all_lt(declared));
		});
	}

	#[test]
	fn close_case_fails_when_callback_weight_bound_too_low() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			let case_index = helpers::create_ripe_case::<Test>();
			let case = RipeCases::<Test>::get(case_index).unwrap();
			let callback_weight = case
				.details
				.callback
				.curry((case_index, case.details.context, case.verdict))
				.unwrap()
				.get_dispatch_info()
				.call_weight;

			// A bound below the callback weight must be rejected and the case left untouched.
			let too_low = callback_weight.saturating_sub(Weight::from_parts(1, 0));
			assert_noop!(
				MobRule::close_case(authorized_origin(), case_index, too_low),
				Error::<Test>::CallbackWeightTooLow
			);
			assert!(RipeCases::<Test>::contains_key(case_index));
		});
	}

	#[test]
	fn only_done_case_can_be_removed() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			// GIVEN one done case exists that has timed out
			let done_case_index = helpers::create_done_case::<Test>(vec![], 0);

			// AND timeout for it has passed
			mock::Now::set(Duration::from_millis(constants::TWO_DAYS_MS));

			// WHEN reap_case is called on it
			assert!(MobRule::reap_case(authorized_origin(), done_case_index).is_ok());

			// Then it is removed from DoneCases
			assert!(DoneCases::<Test>::get(done_case_index).is_none());
			System::assert_has_event(Event::CaseRemoved { case_index: done_case_index }.into());

			// GIVEN one ripe case exists
			let ripe_case_index = helpers::create_ripe_case::<Test>();

			// WHEN reap_case is called on it
			let reap_case_call_result = MobRule::reap_case(authorized_origin(), ripe_case_index);

			// THEN the call fails
			assert_noop!(reap_case_call_result, Error::<Test>::NotDone);

			// GIVEN one open case exists
			let open_case_index = helpers::create_voting_case::<Test>();

			// WHEN reap_case is called on it
			let reap_case_call_result = MobRule::reap_case(authorized_origin(), open_case_index);

			// THEN the call fails
			assert_noop!(reap_case_call_result, Error::<Test>::NotDone);
		});
	}

	#[test]
	fn reap_case_attempt_does_not_remove_cases_too_early() {
		TestExt::new().execute_with(|| {
			// GIVEN one done case exists that has NOT timed out
			let done_case_index = helpers::create_done_case::<Test>(
				vec![],
				<Test as crate::Config>::Clock::now().as_secs(),
			);

			// WHEN reap_case is called on it
			assert_noop!(
				MobRule::reap_case(authorized_origin(), done_case_index),
				Error::<Test>::Recent
			);

			// THEN it is NOT removed from DoneCases
			assert!(DoneCases::<Test>::get(done_case_index).is_some());
		});
	}

	#[test]
	fn intervene_closes_an_open_case() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			// GIVEN one open case exists
			let open_case_index = helpers::create_voting_case::<Test>();

			// WHEN intervene is called on it
			assert!(MobRule::intervene(
				RuntimeOrigin::root(),
				open_case_index,
				Judgement::Truth(Truth::True),
				Weight::MAX
			)
			.is_ok());

			// THEN it is removed from OpenCases and inserted to DoneCases
			assert!(OpenCases::<Test>::get(open_case_index).is_none());
			assert!(DoneCases::<Test>::get(open_case_index).is_some());
			System::assert_has_event(
				Event::CaseIntervened {
					case_index: open_case_index,
					verdict: Judgement::Truth(Truth::True),
				}
				.into(),
			);
		});
	}

	#[test]
	fn intervene_emits_structured_callback_error_for_invalid_callback() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			let case_index = helpers::create_voting_case::<Test>();

			OpenCases::<Test>::mutate(case_index, |case| {
				case.as_mut().unwrap().details.callback = Callback::from_parts(u8::MAX, u8::MAX);
			});

			assert_ok!(MobRule::intervene(
				RuntimeOrigin::root(),
				case_index,
				Judgement::Truth(Truth::True),
				Weight::MAX,
			));

			System::assert_has_event(
				Event::CallbackError { case_index, trigger: CallbackTrigger::Intervene }.into(),
			);
		});
	}

	#[test]
	fn intervene_refunds_unused_callback_weight() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			let case_index = helpers::create_voting_case::<Test>();
			let verdict = Judgement::Truth(Truth::True);
			let case = OpenCases::<Test>::get(case_index).unwrap();
			let callback_weight = case
				.details
				.callback
				.curry((case_index, case.details.context, verdict))
				.unwrap()
				.get_dispatch_info()
				.call_weight;

			// Declare a bound generously above the callback's actual weight so a refund is due.
			let max_callback_weight =
				callback_weight.saturating_add(Weight::from_parts(1_000, 1_000));
			let declared = Call::<Test>::intervene { case_index, verdict, max_callback_weight }
				.get_dispatch_info()
				.call_weight;

			let post =
				MobRule::intervene(RuntimeOrigin::root(), case_index, verdict, max_callback_weight)
					.expect("open case can be intervened");

			let expected =
				<Test as Config>::WeightInfo::intervene().saturating_add(callback_weight);
			assert_eq!(post.actual_weight, Some(expected));
			assert_eq!(post.pays_fee, Pays::No);
			assert!(post.actual_weight.unwrap().all_lt(declared));
		});
	}

	#[test]
	fn voter_gets_penalty() {
		TestExt::new().execute_with(|| {
			EnsureAliasLowerThan5::set_voter_count(3);
			advance_to(10);
			let case_index = helpers::create_voting_case::<Test>();

			let alias = EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID));

			// First vote cast with `Contempt`, the others vote `False`.
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID),
				case_index,
				Judgement::Contempt,
			));
			System::assert_has_event(
				Event::Voted { case_index, voter: alias, opinion: Judgement::Contempt }.into(),
			);
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID_2),
				case_index,
				Judgement::Truth(Truth::False),
			));
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID_3),
				case_index,
				Judgement::Truth(Truth::False),
			));

			// AND two days have passed since it's opening
			mock::Now::set(Duration::from_millis(constants::TWO_DAYS_MS));

			// Update the case.
			assert_ok!(MobRule::touch_case(RuntimeOrigin::signed(VOTER_VALID), case_index));
			// The case is moved from open to ripe cases
			assert!(OpenCases::<Test>::get(case_index).is_none());
			assert!(matches!(
				RipeCases::<Test>::get(case_index),
				Some(RipeCase { verdict: Judgement::Truth(Truth::False), .. })
			));
			// No penalty yet and we can't clear it.
			assert_eq!(VotingPenalties::<Test>::get(alias), None);
			assert_noop!(
				MobRule::clear_voting_penalty(RuntimeOrigin::signed(VOTER_VALID)),
				Error::<Test>::NoPenalty
			);

			// Close the case and register the penalty when cleaning the bad vote.
			advance_to(15);
			assert!(MobRule::close_case(authorized_origin(), case_index, Weight::MAX).is_ok());
			assert!(OpenCases::<Test>::get(case_index).is_none());
			assert!(RipeCases::<Test>::get(case_index).is_none());
			assert!(DoneCases::<Test>::get(case_index).is_some());
			System::assert_has_event(
				Event::CaseClosed { case_index, verdict: Judgement::Truth(Truth::False) }.into(),
			);

			// Vote is no longer open for claim - the claim period has passed
			mock::Now::set(Duration::from_millis(
				constants::TWO_DAYS_MS + constants::ONE_HOUR_MS * 2,
			));
			assert_ok!(MobRule::clean_vote(authorized_origin(), case_index, alias));
			System::assert_has_event(Event::VoteCleaned { case_index, voter: alias }.into());

			assert_eq!(VotingPenalties::<Test>::get(alias), Some(15));

			// Still early
			advance_to(24);
			assert_noop!(
				MobRule::clear_voting_penalty(RuntimeOrigin::signed(VOTER_VALID)),
				Error::<Test>::Early
			);

			// Move past config value of `VotingPenaltyDuration`.`
			advance_to(25);
			assert_ok!(MobRule::clear_voting_penalty(RuntimeOrigin::signed(VOTER_VALID)));
			assert!(!VotingPenalties::<Test>::contains_key(alias));
			// AND the event is emitted
			System::assert_has_event(Event::VotingPenaltyCleared { who: alias }.into());
		});
	}

	#[test]
	fn definitive_verdict_closes_case() {
		TestExt::new().execute_with(|| {
			EnsureAliasLowerThan5::set_voter_count(5);
			advance_to(10);
			let nay_case_index = helpers::create_voting_case::<Test>();
			let contempt_case_index = helpers::create_voting_case::<Test>();
			let mixed_false_case_index = helpers::create_voting_case::<Test>();
			let mixed_contempt_case_index = helpers::create_voting_case::<Test>();

			// AND two days have passed since it's opening
			mock::Now::set(Duration::from_millis(constants::TWO_DAYS_MS));

			// Case which reaches an early "nay"
			{
				assert_ok!(MobRule::vote(
					RuntimeOrigin::signed(VOTER_VALID),
					nay_case_index,
					Judgement::Truth(Truth::False),
				));
				assert_ok!(MobRule::vote(
					RuntimeOrigin::signed(VOTER_VALID_2),
					nay_case_index,
					Judgement::Truth(Truth::False),
				));
				// One for dissent.
				assert_ok!(MobRule::vote(
					RuntimeOrigin::signed(VOTER_VALID_3),
					nay_case_index,
					Judgement::Truth(Truth::True),
				));
			}

			// Case which reaches an early "contempt"
			{
				assert_ok!(MobRule::vote(
					RuntimeOrigin::signed(VOTER_VALID),
					contempt_case_index,
					Judgement::Contempt,
				));
				assert_ok!(MobRule::vote(
					RuntimeOrigin::signed(VOTER_VALID_2),
					contempt_case_index,
					Judgement::Contempt,
				));
				// One for dissent.
				assert_ok!(MobRule::vote(
					RuntimeOrigin::signed(VOTER_VALID_3),
					contempt_case_index,
					Judgement::Truth(Truth::True),
				));
			}

			// Case which reaches an early "nay", but mixed
			{
				assert_ok!(MobRule::vote(
					RuntimeOrigin::signed(VOTER_VALID),
					mixed_false_case_index,
					Judgement::Contempt,
				));
				assert_ok!(MobRule::vote(
					RuntimeOrigin::signed(VOTER_VALID_2),
					mixed_false_case_index,
					Judgement::Truth(Truth::False),
				));
				// One for dissent.
				assert_ok!(MobRule::vote(
					RuntimeOrigin::signed(VOTER_VALID_3),
					mixed_false_case_index,
					Judgement::Truth(Truth::True),
				));
			}

			// Case which reaches an early "contempt", but mixed
			{
				assert_ok!(MobRule::vote(
					RuntimeOrigin::signed(VOTER_VALID),
					mixed_contempt_case_index,
					Judgement::Contempt,
				));
				assert_ok!(MobRule::vote(
					RuntimeOrigin::signed(VOTER_VALID_2),
					mixed_contempt_case_index,
					Judgement::Truth(Truth::False),
				));
				// One for dissent.
				assert_ok!(MobRule::vote(
					RuntimeOrigin::signed(VOTER_VALID_3),
					mixed_contempt_case_index,
					Judgement::Truth(Truth::True),
				));
			}

			// Despite the approval, the cases are still open because
			assert!(OpenCases::<Test>::get(nay_case_index).is_some());
			assert!(OpenCases::<Test>::get(contempt_case_index).is_some());
			assert!(OpenCases::<Test>::get(mixed_false_case_index).is_some());
			assert!(OpenCases::<Test>::get(mixed_contempt_case_index).is_some());

			{
				assert_ok!(MobRule::vote(
					RuntimeOrigin::signed(VOTER_VALID_4),
					nay_case_index,
					Judgement::Truth(Truth::False),
				));
				assert_ok!(MobRule::vote(
					RuntimeOrigin::signed(VOTER_VALID_4),
					contempt_case_index,
					Judgement::Contempt,
				));
				assert_ok!(MobRule::vote(
					RuntimeOrigin::signed(VOTER_VALID_4),
					mixed_false_case_index,
					Judgement::Truth(Truth::False),
				));
				assert_ok!(MobRule::vote(
					RuntimeOrigin::signed(VOTER_VALID_4),
					mixed_contempt_case_index,
					Judgement::Contempt,
				));
			}

			// The cases are moved from open to ripe because the last vote pushed the result over
			// the point where it could have been overturned.
			assert!(OpenCases::<Test>::get(nay_case_index).is_none());
			assert!(OpenCases::<Test>::get(contempt_case_index).is_none());
			assert!(OpenCases::<Test>::get(mixed_false_case_index).is_none());
			assert!(OpenCases::<Test>::get(mixed_contempt_case_index).is_none());
			assert!(matches!(
				RipeCases::<Test>::get(nay_case_index),
				Some(RipeCase { verdict: Judgement::Truth(Truth::False), .. })
			));
			assert!(matches!(
				RipeCases::<Test>::get(contempt_case_index),
				Some(RipeCase { verdict: Judgement::Contempt, .. })
			));
			assert!(matches!(
				RipeCases::<Test>::get(mixed_false_case_index),
				Some(RipeCase { verdict: Judgement::Truth(Truth::False), .. })
			));
			assert!(matches!(
				RipeCases::<Test>::get(mixed_contempt_case_index),
				Some(RipeCase { verdict: Judgement::Contempt, .. })
			));
		});
	}
}

/// Step-by-step process to obtain rewards accumulated as a result of voting on a case.
/// Some steps are callable by root.
///
/// 1. Voters cast their votes - `fn vote`
///    - Voters participate in the voting process by casting their votes on a case.
///
/// 2. Voters claim the credits associated with their votes - `fn claim_vote` / `fn claim_votes`
///    - After voting, voters can claim the credits they earned from their votes.
///
/// 3. Payout round schedule creation - `fn schedule_payout_rounds`
///    - The root schedules payout round, which results in the insertion of a new round schedule int
///      `RoundSchedules`.
///
/// 4. Payout round start according to the schedule - `fn start_payout_round`
///    - The root starts a payout round, which creates a new payout distribution in
///      `PayoutDistribution` and resets the accumulated points.
///
/// 5. Voters claim their credits - `fn claim_credit`
///    - Voters claim their credits, which results in their `MobCredit.credit` being topped up and
///      inserted into `Credits`, and the `PayoutDistribution.remaining_balance` being reduced by
///      the claimed amount.
///
/// 6. Voters trigger rewards payout - `fn payout_rewards`
///    - Voters trigger rewards payout, which transfers the voter's full accrued credit from the pot
///      to a destination account.
mod rewards {
	use super::*;
	use frame_support::{
		assert_noop,
		traits::fungible::{InspectHold, Mutate, MutateHold},
	};
	use indiv_support::traits::Truth;
	use sp_runtime::{
		DispatchError::Token,
		TokenError::{BelowMinimum, FundsUnavailable},
	};
	use std::time::Duration;

	#[test]
	fn insufficient_pot_amount_causes_schedule_payout_failure() {
		TestExt::new().execute_with(|| {
			// GIVEN the pot amount is too small
			let _ = mock::Balances::mint_into(&MobRule::mob_rule_pot_id(), 10u64);

			// WHEN schedule_payout_rounds is called
			// THEN it fails with relevant error
			assert_noop!(
				MobRule::schedule_payout_rounds(RuntimeOrigin::root(), 100, 1, 10),
				Token(FundsUnavailable),
			);
		});
	}

	#[test]
	fn schedule_payout_stores_correct_schedule() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			// GIVEN the pot amount is sufficient
			helpers::fund_pot::<Test>();

			// WHEN schedule_payout_rounds is called
			// THEN it succeeds
			assert_ok!(MobRule::schedule_payout_rounds(RuntimeOrigin::root(), 10, 1, 1));

			// AND balance is put on hold
			assert_eq!(
				Balances::balance_on_hold(&HoldReason::Payout.into(), &MobRule::mob_rule_pot_id()),
				10
			);
			// AND new schedule is created
			assert_eq!(RoundSchedules::<Test>::get().len(), 1);
			// AND the event is emitted
			System::assert_has_event(
				Event::PayoutRoundsScheduled { amount: 10, count: 1, period: 1 }.into(),
			);
		});
	}

	#[test]
	fn payout_schedule_can_be_removed() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			// GIVEN a schedule exists
			helpers::fund_pot::<Test>();
			assert_ok!(MobRule::schedule_payout_rounds(RuntimeOrigin::root(), 10, 1, 1));
			assert_eq!(RoundSchedules::<Test>::get().len(), 1);

			// AND payout schedule creation triggered balance hold
			assert_eq!(
				Balances::balance_on_hold(&HoldReason::Payout.into(), &MobRule::mob_rule_pot_id()),
				10
			);

			// WHEN remove_payout_schedule is called
			// THEN it succeeds
			assert_ok!(MobRule::remove_payout_schedule(RuntimeOrigin::root(), 0));

			// AND removes the schedule from the storage
			assert_eq!(RoundSchedules::<Test>::get().len(), 0);

			// AND payout schedule removal triggered balance release
			assert_eq!(
				Balances::balance_on_hold(&HoldReason::Payout.into(), &MobRule::mob_rule_pot_id()),
				0
			);
			// AND the event is emitted
			System::assert_has_event(Event::PayoutScheduleRemoved { index: 0 }.into());
		});
	}

	#[test]
	fn start_payout_called_but_nobody_claimed_their_credits() {
		TestExt::new().execute_with(|| {
			// GIVEN the pot amount is sufficient
			helpers::fund_pot::<Test>();

			// AND payout round is scheduled
			assert_ok!(MobRule::schedule_payout_rounds(RuntimeOrigin::root(), 10, 1, 1));

			// WHEN start payout is called
			// THEN it fails due to no points claimed
			assert_noop!(
				MobRule::start_payout_round(RuntimeOrigin::root()),
				Error::<Test>::NoPoints
			);
		});
	}

	#[test]
	fn start_payout_called_but_no_schedule_exists() {
		TestExt::new().execute_with(|| {
			AccumulatedPoints::<Test>::set(10);

			assert_noop!(
				MobRule::start_payout_round(RuntimeOrigin::root()),
				Error::<Test>::NoSchedule
			);
		});
	}

	#[test]
	fn start_payout_prepares_new_credit_distribution() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			// GIVEN the pot amount is sufficient
			helpers::fund_pot::<Test>();

			// AND payout round is scheduled
			assert_ok!(MobRule::schedule_payout_rounds(RuntimeOrigin::root(), 10, 1, 1));

			AccumulatedPoints::<Test>::set(10);

			// WHEN
			assert_ok!(MobRule::start_payout_round(RuntimeOrigin::root()));

			// Schedule is removed
			assert!(RoundSchedules::<Test>::get().is_empty());
			// Reset of accumulated points
			assert_eq!(AccumulatedPoints::<Test>::get(), 0);
			// New payout distribution
			assert!(PayoutDistribution::<Test>::get().is_some());
			// AND the event is emitted
			System::assert_has_event(
				Event::PayoutRoundStarted { round: 0, initial_balance: 10, total_points: 10 }
					.into(),
			);
		});
	}

	#[test]
	fn clean_points_emits_event() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			let voter = EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID));
			// Set up a past round and some stale points
			PayoutDistribution::<Test>::put(CreditDistribution {
				round: 1,
				initial_balance: 10,
				remaining_balance: 10,
				total_points: 10,
				start: 0,
			});
			VotingPoints::<Test>::insert(0, voter, 5u32);

			assert_ok!(MobRule::clean_points(RuntimeOrigin::signed(VOTER_VALID), 0, voter));

			System::assert_has_event(Event::PointsCleaned { round: 0, voter }.into());
		});
	}

	#[test]
	fn voter_tries_to_claim_credit_but_no_payout_distribution_exists() {
		TestExt::new().execute_with(|| {
			assert_noop!(
				MobRule::claim_credit(RuntimeOrigin::signed(VOTER_VALID)),
				Error::<Test>::NoReward
			);
		});
	}

	#[test]
	fn voter_tries_to_claim_credit_but_claim_votes_was_not_called_prior() {
		TestExt::new().execute_with(|| {
			let credit_distribution = CreditDistribution {
				round: 1,
				initial_balance: 1000,
				remaining_balance: 1000,
				total_points: 500,
				start: 0,
			};
			PayoutDistribution::<Test>::put(credit_distribution);

			assert_noop!(
				MobRule::claim_credit(RuntimeOrigin::signed(VOTER_VALID)),
				Error::<Test>::NoPoints
			);
		});
	}

	/// Shows the whole flow of voting, slightly simplified:
	/// - one voter, one round, all rewards claimed.
	#[test]
	fn from_vote_to_reward() {
		TestExt::new().execute_with(|| {
			advance_to(1);
			EnsureAliasLowerThan5::set_voter_count(1);
			// An open case exists
			let case_index = helpers::create_voting_case::<Test>();

			let alias = EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID));

			// Voter casts their vote in the case
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID),
				case_index,
				Judgement::Truth(Truth::True),
			));
			System::assert_has_event(
				Event::Voted { case_index, voter: alias, opinion: Judgement::Truth(Truth::True) }
					.into(),
			);

			// The voter's mob credit is modified as a result of his vote
			assert_eq!(
				Credits::<Test>::get(alias),
				MobCredit::<u64> { voted: 1, cleaned: 0, correct: 0, credit: 0 }
			);

			// Voter tries to claim the credit for his vote but the case is not 'Done' yet.
			assert_noop!(
				MobRule::claim_vote(RuntimeOrigin::signed(VOTER_VALID), case_index),
				Error::<Test>::NotDone
			);

			// "Touch" the case after the timeout so the case becomes 'Ripe'
			// AND two days have passed since it's opening
			mock::Now::set(Duration::from_millis(constants::TWO_DAYS_MS));
			assert_ok!(MobRule::touch_case(RuntimeOrigin::signed(VOTER_VALID), case_index,));
			assert!(OpenCases::<Test>::get(case_index).is_none());
			assert!(RipeCases::<Test>::get(case_index).is_some());

			// The case becomes closed
			assert!(MobRule::close_case(authorized_origin(), case_index, Weight::MAX).is_ok());
			assert!(OpenCases::<Test>::get(case_index).is_none());
			assert!(RipeCases::<Test>::get(case_index).is_none());
			assert!(DoneCases::<Test>::get(case_index).is_some());

			// Voter claims the credit again. This time successfully
			assert_ok!(MobRule::claim_vote(RuntimeOrigin::signed(VOTER_VALID), case_index));
			System::assert_has_event(
				Event::VotesClaimed { voter: alias, case_indices: vec![case_index] }.into(),
			);

			// Voter's mob credit changes as an effect of the previous call
			assert_eq!(
				Credits::<Test>::get(alias),
				MobCredit::<u64> { voted: 1, cleaned: 1, correct: 1, credit: 0 }
			);
			assert_eq!(VotingPoints::<Test>::get(0, alias), 1);
			assert_eq!(AccumulatedPoints::<Test>::get(), 1);

			// In the meantime pot and payout distribution are prepared
			helpers::fund_pot::<Test>();
			assert_ok!(MobRule::schedule_payout_rounds(RuntimeOrigin::root(), 10, 1, 1));
			assert_ok!(MobRule::start_payout_round(RuntimeOrigin::root()));

			// Since the claim_vote call was done, the voter can now claim credit for the vote
			assert_ok!(MobRule::claim_credit(RuntimeOrigin::signed(VOTER_VALID)));

			// Voter's mob credit got modified
			assert_eq!(
				Credits::<Test>::get(alias),
				MobCredit::<u64> { voted: 1, cleaned: 1, correct: 1, credit: 10 }
			);
			// AND the event is emitted
			System::assert_has_event(Event::CreditClaimed { voter: alias, amount: 10 }.into());
			// Payout distribution changed
			assert_eq!(
				PayoutDistribution::<Test>::get().unwrap(),
				CreditDistribution {
					round: 0,
					initial_balance: 10,
					remaining_balance: 0,
					total_points: 1,
					start: 1,
				}
			);

			// Voter triggers payout of his rewards
			let payout_destination_voter_1 = 11;
			assert_ok!(MobRule::payout_rewards(
				RuntimeOrigin::signed(VOTER_VALID),
				payout_destination_voter_1
			));
			System::assert_has_event(
				Event::RewardPayout {
					voter: alias,
					destination: payout_destination_voter_1,
					amount: 10,
				}
				.into(),
			);

			// Voter's mob credit got modified again
			assert_eq!(
				Credits::<Test>::get(alias),
				MobCredit::<u64> { voted: 1, cleaned: 1, correct: 1, credit: 0 }
			);

			// Full credit has been paid out in one transfer.
			assert_eq!(
				Balances::balance_on_hold(&HoldReason::Credit.into(), &MobRule::mob_rule_pot_id()),
				0
			);
			// Destination account receives the transferred reward.
			assert_eq!(Balances::free_balance(payout_destination_voter_1), 10);
		});
	}

	#[test]
	fn payout_rewards_release_failure_preserves_credit() {
		TestExt::new().execute_with(|| {
			let alias = EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID));
			Credits::<Test>::insert(
				alias,
				MobCredit::<u64> { voted: 0, cleaned: 0, correct: 0, credit: 10 },
			);
			let destination = MOCK_ACCOUNT_ID1;

			assert_noop!(
				MobRule::payout_rewards(RuntimeOrigin::signed(VOTER_VALID), destination),
				Token(FundsUnavailable)
			);

			assert_eq!(Credits::<Test>::get(alias).credit, 10);
			assert_eq!(
				Balances::balance_on_hold(&HoldReason::Credit.into(), &MobRule::mob_rule_pot_id()),
				0
			);
			assert_eq!(Balances::free_balance(destination), 0);
		});
	}

	#[test]
	fn payout_rewards_transfer_failure_preserves_credit_and_hold() {
		TestExt::new().execute_with(|| {
			let alias = EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID));
			let pot = MobRule::mob_rule_pot_id();
			helpers::fund_pot::<Test>();
			assert_ok!(Balances::hold(&HoldReason::Credit.into(), &pot, 1));
			Credits::<Test>::insert(
				alias,
				MobCredit::<u64> { voted: 0, cleaned: 0, correct: 0, credit: 1 },
			);

			// Below existential deposit in mock runtime => transfer fails after release attempt.
			let destination = 42;
			assert_noop!(
				MobRule::payout_rewards(RuntimeOrigin::signed(VOTER_VALID), destination),
				Token(BelowMinimum)
			);

			assert_eq!(Credits::<Test>::get(alias).credit, 1);
			assert_eq!(Balances::balance_on_hold(&HoldReason::Credit.into(), &pot), 1);
			assert_eq!(Balances::free_balance(destination), 0);
		});
	}

	/// Shows how the system should behave in a more real-life scenario:
	/// - multiple voters vote on a case,
	/// - rewards are distributed in multiple rounds,
	/// - not all voters claim the rewards,
	/// - there's still one simplification though: all voters vote with the same judgement.
	///
	/// ## Note on time limit to claim the rewards:
	/// The time to claim one's rewards is limited because there is no general value of a vote
	/// but rather this value depends on the total number of votes cast in a round.
	/// Thus allowing voters to claim their rewards whenever they want would result in voters
	/// aiming to claim in rounds with less total claim in order to maximize their reward.
	#[test]
	fn multiple_payout_rounds_and_multiple_voters() {
		TestExt::new().execute_with(|| {
			EnsureAliasLowerThan5::set_voter_count(3);
			let alias_voter_valid: Alias =
				EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID));
			let alias_voter_valid_2: Alias =
				EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID_2));
			let alias_voter_valid_3: Alias =
				EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID_3));

			// An open case exists
			let case_index = helpers::create_voting_case::<Test>();

			// Voters casts their votes in the case
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID),
				case_index,
				Judgement::Truth(Truth::True),
			));
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID_2),
				case_index,
				Judgement::Truth(Truth::True),
			));

			// Voter 3 votes after the timeout so the case becomes 'Ripe'
			// Voter 3 votes after the timeout so the case becomes 'Ripe'
			Now::set(Duration::from_millis(constants::TWO_DAYS_MS));
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID_3),
				case_index,
				Judgement::Truth(Truth::True),
			));
			assert!(OpenCases::<Test>::get(case_index).is_none());
			assert!(RipeCases::<Test>::get(case_index).is_some());

			// The case becomes closed
			assert!(MobRule::close_case(authorized_origin(), case_index, Weight::MAX).is_ok());
			assert!(OpenCases::<Test>::get(case_index).is_none());
			assert!(RipeCases::<Test>::get(case_index).is_none());
			assert!(DoneCases::<Test>::get(case_index).is_some());

			// Voters 1 and 2 claim the credit
			assert_ok!(MobRule::claim_vote(RuntimeOrigin::signed(VOTER_VALID), case_index));
			assert_ok!(MobRule::claim_vote(RuntimeOrigin::signed(VOTER_VALID_2), case_index));

			// Their mob credit changes
			assert_eq!(
				Credits::<Test>::get(alias_voter_valid),
				MobCredit::<u64> { voted: 1, cleaned: 1, correct: 1, credit: 0 }
			);
			assert_eq!(VotingPoints::<Test>::get(0, alias_voter_valid), 1);
			assert_eq!(
				Credits::<Test>::get(alias_voter_valid_2),
				MobCredit::<u64> { voted: 1, cleaned: 1, correct: 1, credit: 0 }
			);
			assert_eq!(VotingPoints::<Test>::get(0, alias_voter_valid_2), 1);

			// but that of voter 3 stays the same
			assert_eq!(
				Credits::<Test>::get(alias_voter_valid_3),
				MobCredit::<u64> { voted: 1, cleaned: 0, correct: 0, credit: 0 }
			);
			assert_eq!(VotingPoints::<Test>::get(0, alias_voter_valid_3), 0);

			// Accumulated points by all the voters change as-well
			assert_eq!(AccumulatedPoints::<Test>::get(), 2);

			// Pot and payout distribution preparation
			helpers::fund_pot::<Test>();
			// 20 tokens distributed in each of the 2 rounds (so 40 tokens in total) with a period
			// of 1 block
			assert_ok!(MobRule::schedule_payout_rounds(RuntimeOrigin::root(), 20, 2, 1));

			// --- ROUND 1

			// 1st round starts
			assert_ok!(MobRule::start_payout_round(RuntimeOrigin::root()));

			// Initial payout distribution
			assert_eq!(
				PayoutDistribution::<Test>::get().unwrap(),
				CreditDistribution {
					round: 0,
					initial_balance: 20,
					remaining_balance: 20,
					// 2 points because 2 voters called claim_vote
					total_points: 2,
					start: 0,
				}
			);

			// Only voter 1 claims the credit
			assert_ok!(MobRule::claim_credit(RuntimeOrigin::signed(VOTER_VALID)));

			// So only voter's 1 mob credit changes
			assert_eq!(
				Credits::<Test>::get(alias_voter_valid),
				MobCredit::<u64> { voted: 1, cleaned: 1, correct: 1, credit: 10 }
			);
			assert_eq!(
				Credits::<Test>::get(alias_voter_valid_2),
				MobCredit::<u64> { voted: 1, cleaned: 1, correct: 1, credit: 0 }
			);

			// claim_credit call causes the required funds to be held on the pot account
			assert_eq!(
				Balances::balance_on_hold(&HoldReason::Credit.into(), &MobRule::mob_rule_pot_id()),
				10
			);

			// Payout distribution changed
			assert_eq!(
				PayoutDistribution::<Test>::get().unwrap(),
				CreditDistribution {
					round: 0,
					initial_balance: 20,
					remaining_balance: 10,
					total_points: 2,
					start: 0,
				}
			);

			// Voter 1 triggers payout of their rewards
			let payout_destination_voter_1 = 11;
			assert_ok!(MobRule::payout_rewards(
				RuntimeOrigin::signed(VOTER_VALID),
				payout_destination_voter_1
			));

			// Voter's mob credit got modified again
			assert_eq!(
				Credits::<Test>::get(alias_voter_valid),
				MobCredit::<u64> { voted: 1, cleaned: 1, correct: 1, credit: 0 }
			);

			// Destination account receives the transferred reward.
			assert_eq!(Balances::free_balance(payout_destination_voter_1), 10);

			// --- ROUND 2

			// 2nd round start triggered but nobody claimed credit
			assert_noop!(
				MobRule::start_payout_round(RuntimeOrigin::root()),
				Error::<Test>::NoPoints
			);

			// Voter 3 claims the credit
			assert_ok!(MobRule::claim_vote(RuntimeOrigin::signed(VOTER_VALID_3), case_index));
			// Voters 1 and 2 cannot claim the credit for the second time
			assert_noop!(
				MobRule::claim_vote(RuntimeOrigin::signed(VOTER_VALID), case_index),
				Error::<Test>::NoSuchVote
			);
			assert_noop!(
				MobRule::claim_vote(RuntimeOrigin::signed(VOTER_VALID_2), case_index),
				Error::<Test>::NoSuchVote
			);

			// 2nd round starts
			advance_to(2);
			assert_ok!(MobRule::start_payout_round(RuntimeOrigin::root()));

			// No funds should be held in pot in the beginning of new round
			assert_eq!(
				Balances::balance_on_hold(&HoldReason::Credit.into(), &MobRule::mob_rule_pot_id()),
				0
			);

			// Payout distribution for the 2nd round
			assert_eq!(
				PayoutDistribution::<Test>::get().unwrap(),
				CreditDistribution {
					round: 1,
					initial_balance: 20,
					remaining_balance: 20,
					// Only points of voter 3 are claimable this round.
					// Points of voter 2 were lost during the previous round since he did not
					// finalise claiming.
					total_points: 1,
					start: 2,
				}
			);

			// Voter 3 claims the credit
			assert_ok!(MobRule::claim_credit(RuntimeOrigin::signed(VOTER_VALID_3)));

			// So his mob credit changes. All the credit of the round goes to him as he's the only
			// one claiming.
			assert_eq!(
				Credits::<Test>::get(alias_voter_valid_3),
				MobCredit::<u64> { voted: 1, cleaned: 1, correct: 1, credit: 20 }
			);

			// Payout distribution changes
			assert_eq!(
				PayoutDistribution::<Test>::get().unwrap(),
				CreditDistribution {
					round: 1,
					initial_balance: 20,
					remaining_balance: 0,
					total_points: 1,
					start: 2,
				}
			);

			// Necessary funds are held in pot account
			assert_eq!(
				Balances::balance_on_hold(&HoldReason::Credit.into(), &MobRule::mob_rule_pot_id()),
				20
			);

			// Voter 3 triggers payout of his rewards.
			let destination_1_voter_3: AccountId = 13;
			assert_ok!(MobRule::payout_rewards(
				RuntimeOrigin::signed(VOTER_VALID_3),
				destination_1_voter_3
			));

			// Voter's mob credit got fully paid out in one transfer.
			assert_eq!(
				Credits::<Test>::get(alias_voter_valid_3),
				MobCredit::<u64> { voted: 1, cleaned: 1, correct: 1, credit: 0 }
			);

			let destination_2_voter_3: AccountId = 23;
			assert_noop!(
				MobRule::payout_rewards(
					RuntimeOrigin::signed(VOTER_VALID_3),
					destination_2_voter_3
				),
				Error::<Test>::NoCredit
			);

			// Destination accounts receive the transferred rewards.
			assert_eq!(Balances::free_balance(destination_1_voter_3), 20);
			assert_eq!(Balances::free_balance(destination_2_voter_3), 0);

			// All the funds in the Pot are released now
			assert_eq!(
				Balances::balance_on_hold(&HoldReason::Credit.into(), &MobRule::mob_rule_pot_id()),
				0
			);
		});
	}

	/// Aims to show how the system will behave in case of:
	/// - 2 persons voting on case A,
	/// - one of them voting also on case B.
	///
	/// To show that the person who voted more times should receive more credit for their efforts.
	#[test]
	fn multiple_cases_multiple_voters() {
		TestExt::new().execute_with(|| {
			EnsureAliasLowerThan5::set_voter_count(3);
			let alias_voter_valid: Alias =
				EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID));
			let alias_voter_valid_2: Alias =
				EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID_2));

			// Two open cases exist
			let case_a = helpers::create_voting_case::<Test>();
			let case_b = helpers::create_voting_case::<Test>();

			// Voter 1 votes in case A
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID),
				case_a,
				Judgement::Truth(Truth::True),
			));

			// Voter 2 votes in both cases after the timeout (to make them 'Ripe')
			// Voter 3 votes after the timeout so the case becomes 'Ripe'
			Now::set(Duration::from_millis(constants::TWO_DAYS_MS));
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID_2),
				case_a,
				Judgement::Truth(Truth::True),
			));
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID_2),
				case_b,
				Judgement::Truth(Truth::True),
			));

			// Both cases become closed
			assert!(MobRule::close_case(authorized_origin(), case_a, Weight::MAX).is_ok());
			assert!(MobRule::close_case(authorized_origin(), case_b, Weight::MAX).is_ok());

			// Both voters claim the credit for case A
			assert_ok!(MobRule::claim_vote(RuntimeOrigin::signed(VOTER_VALID), case_a));
			assert_ok!(MobRule::claim_vote(RuntimeOrigin::signed(VOTER_VALID_2), case_a));

			// Voter 2 claims the credit for case B as-well
			assert_ok!(MobRule::claim_vote(RuntimeOrigin::signed(VOTER_VALID_2), case_b));

			// Voter 1 tries to do it too, but since he didn't vote in case B - no credit for him
			assert_noop!(
				MobRule::claim_vote(RuntimeOrigin::signed(VOTER_VALID), case_b),
				Error::<Test>::NoSuchVote
			);

			// Their mob credit changes
			assert_eq!(
				Credits::<Test>::get(alias_voter_valid),
				MobCredit::<u64> { voted: 1, cleaned: 1, correct: 1, credit: 0 }
			);
			assert_eq!(VotingPoints::<Test>::get(0, alias_voter_valid), 1);
			assert_eq!(
				Credits::<Test>::get(alias_voter_valid_2),
				MobCredit::<u64> { voted: 2, cleaned: 2, correct: 2, credit: 0 }
			);
			assert_eq!(VotingPoints::<Test>::get(0, alias_voter_valid_2), 2);

			// Accumulated points by all the voters change as-well
			assert_eq!(AccumulatedPoints::<Test>::get(), 3);

			// Pot and payout distribution preparation
			helpers::fund_pot::<Test>();
			// 33 tokens distributed in one round with a period of 1 block
			assert_ok!(MobRule::schedule_payout_rounds(RuntimeOrigin::root(), 33, 1, 1));

			// round 1 starts
			assert_ok!(MobRule::start_payout_round(RuntimeOrigin::root()));

			// Initial payout distribution
			assert_eq!(
				PayoutDistribution::<Test>::get().unwrap(),
				CreditDistribution {
					round: 0,
					initial_balance: 33,
					remaining_balance: 33,
					total_points: 3,
					start: 0,
				}
			);

			// Both voters claim the credit
			assert_ok!(MobRule::claim_credit(RuntimeOrigin::signed(VOTER_VALID)));
			assert_ok!(MobRule::claim_credit(RuntimeOrigin::signed(VOTER_VALID_2)));

			// claim_credit call causes the required funds to be held on the pot account
			assert_eq!(
				Balances::balance_on_hold(&HoldReason::Credit.into(), &MobRule::mob_rule_pot_id()),
				31 // -2 due to rounding for each claim
			);

			// Their mob credit changes again
			assert_eq!(
				Credits::<Test>::get(alias_voter_valid),
				MobCredit::<u64> { voted: 1, cleaned: 1, correct: 1, credit: 10 }
			);
			assert_eq!(
				Credits::<Test>::get(alias_voter_valid_2),
				MobCredit::<u64> { voted: 2, cleaned: 2, correct: 2, credit: 21 }
			);

			// Payout distribution changed
			assert_eq!(
				PayoutDistribution::<Test>::get().unwrap(),
				CreditDistribution {
					round: 0,
					initial_balance: 33,
					remaining_balance: 2,
					total_points: 3,
					start: 0,
				}
			);

			// Voters trigger rewards payout.
			let destination_1_voter_1: AccountId = 11;
			assert_ok!(MobRule::payout_rewards(
				RuntimeOrigin::signed(VOTER_VALID),
				destination_1_voter_1
			));
			// Voter 2 receives the full available credit (21) on the first payout call.
			// The second payout call fails with `NoCredit` because nothing remains.
			let destination_1_voter_2: AccountId = 12;
			assert_ok!(MobRule::payout_rewards(
				RuntimeOrigin::signed(VOTER_VALID_2),
				destination_1_voter_2
			));
			let destination_2_voter_2: AccountId = 22;
			assert_noop!(
				MobRule::payout_rewards(
					RuntimeOrigin::signed(VOTER_VALID_2),
					destination_2_voter_2
				),
				Error::<Test>::NoCredit
			);

			// Payouts consume both voters' accrued mob credit.
			assert_eq!(
				Credits::<Test>::get(alias_voter_valid),
				MobCredit::<u64> { voted: 1, cleaned: 1, correct: 1, credit: 0 }
			);

			assert_eq!(
				Credits::<Test>::get(alias_voter_valid_2),
				MobCredit::<u64> { voted: 2, cleaned: 2, correct: 2, credit: 0 }
			);

			// Destination accounts reflect the successful payouts.
			assert_eq!(Balances::free_balance(destination_1_voter_1), 10);
			assert_eq!(Balances::free_balance(destination_1_voter_2), 21);
			assert_eq!(Balances::free_balance(destination_2_voter_2), 0);

			// All held funds in the pot are released now.
			assert_eq!(
				Balances::balance_on_hold(&HoldReason::Credit.into(), &MobRule::mob_rule_pot_id()),
				0
			);
		});
	}
}

mod offchain_worker {
	use super::*;
	use codec::Decode;
	use frame_support::{assert_noop, assert_ok, pallet_prelude::Get, traits::OffchainWorker};
	use indiv_support::traits::Truth::True;
	use sp_core::offchain::{
		testing::{TestOffchainExt, TestTransactionPoolExt},
		OffchainDbExt, OffchainWorkerExt, TransactionPoolExt,
	};
	use sp_runtime::transaction_validity::{
		TransactionSource, TransactionValidityError, UnknownTransaction,
	};
	use std::time::Duration;

	#[test]
	fn submits_transaction_only_at_interval_ticks() {
		let mut ext = new_test_ext();
		let (offchain, _state) = TestOffchainExt::new();
		let (pool, state) = TestTransactionPoolExt::new();
		ext.register_extension(OffchainDbExt::new(offchain.clone()));
		ext.register_extension(OffchainWorkerExt::new(offchain));
		ext.register_extension(TransactionPoolExt::new(pool));

		ext.execute_with(|| {
			// One ripe case exists
			let case_index = helpers::create_ripe_case::<Test>();

			// Offchain worker should not submit the transaction
			// if the block numbers are in between the interval so
			// starting from block number 1 to T::OffchainWorkInterval - 1
			let interval: u64 = <Test as Config>::OffchainWorkInterval::get();
			let mut block = 1;
			while block < interval {
				System::set_block_number(block);
				MobRule::offchain_worker(block);
				assert_eq!(state.read().transactions.len(), 0);
				block += 1;
			}

			// At T::OffchainWorkInterval the offchain worker should submit the transaction
			System::set_block_number(block);
			MobRule::offchain_worker(block);
			assert_eq!(state.read().transactions.len(), 1);

			// and the transaction should be close_case call
			let transaction = state.write().transactions.pop().unwrap();
			let ex: Extrinsic = Decode::decode(&mut &*transaction).unwrap();
			let closed_case_index = match &ex.function {
				crate::mock::RuntimeCall::MobRule(crate::Call::close_case {
					case_index, ..
				}) => *case_index,
				e => panic!("Unexpected call: {e:?}"),
			};
			assert_eq!(closed_case_index, case_index);
			assert_ok!(exec_tx(ex, TransactionSource::Local));
			assert!(!RipeCases::<Test>::contains_key(case_index));
			assert!(DoneCases::<Test>::contains_key(case_index));
		});
	}

	#[test]
	fn offchain_worker_closes_ripe_case_automatically() {
		let mut ext = new_test_ext();
		let (offchain, _state) = TestOffchainExt::new();
		let (pool, state) = TestTransactionPoolExt::new();
		ext.register_extension(OffchainDbExt::new(offchain.clone()));
		ext.register_extension(OffchainWorkerExt::new(offchain));
		ext.register_extension(TransactionPoolExt::new(pool));

		ext.execute_with(|| {
			System::set_block_number(1);
			// A ripe case exists.
			let case_index = helpers::create_ripe_case::<Test>();
			assert!(RipeCases::<Test>::contains_key(case_index));

			// Drive the chain: the offchain worker runs every block and its submitted
			// transactions are applied automatically. It closes the case at the first interval
			// tick without any manual extrinsic.
			let interval: u64 = <Test as Config>::OffchainWorkInterval::get();
			let mut take_submitted = || core::mem::take(&mut state.write().transactions);
			run_offchain_to_block(interval + 1, &mut take_submitted);

			assert!(!RipeCases::<Test>::contains_key(case_index));
			assert!(DoneCases::<Test>::contains_key(case_index));
			System::assert_has_event(
				Event::CaseClosed { case_index, verdict: Judgement::Truth(True) }.into(),
			);
		});
	}

	#[test]
	fn offchain_worker_ripens_times_out_and_reaps_case_automatically() {
		let mut ext = new_test_ext();
		let (offchain, _state) = TestOffchainExt::new();
		let (pool, state) = TestTransactionPoolExt::new();
		ext.register_extension(OffchainDbExt::new(offchain.clone()));
		ext.register_extension(OffchainWorkerExt::new(offchain));
		ext.register_extension(TransactionPoolExt::new(pool));

		ext.execute_with(|| {
			System::set_block_number(1);
			EnsureAliasLowerThan5::set_voter_count(1);
			ActiveSince::<Test>::put(<Test as Config>::Clock::now().as_secs());

			// An open case that nobody resolves.
			let case_index = helpers::create_voting_case::<Test>();
			assert!(OpenCases::<Test>::contains_key(case_index));

			let mut take_submitted = || core::mem::take(&mut state.write().transactions);

			// Once the voting duration elapses, the offchain worker times the case out, moving
			// it from open to ripe automatically.
			mock::Now::set(Duration::from_millis(constants::TWO_WEEKS_MS));
			let interval: u64 = <Test as Config>::OffchainWorkInterval::get();
			run_offchain_to_block(interval + 1, &mut take_submitted);
			assert!(!OpenCases::<Test>::contains_key(case_index));
			assert!(RipeCases::<Test>::contains_key(case_index));

			// The next interval the offchain worker closes the now-ripe case.
			run_offchain_to_block(2 * interval + 1, &mut take_submitted);
			assert!(!RipeCases::<Test>::contains_key(case_index));
			assert!(DoneCases::<Test>::contains_key(case_index));

			// After the claims period the offchain worker reaps the done case.
			mock::Now::set(Duration::from_millis(constants::TWO_WEEKS_MS * 2));
			run_offchain_to_block(3 * interval + 1, &mut take_submitted);
			assert!(!DoneCases::<Test>::contains_key(case_index));
			System::assert_has_event(Event::CaseRemoved { case_index }.into());
		});
	}

	#[test]
	fn rejects_bare_close_case_extrinsic_without_authorize_extension() {
		TestExt::new().execute_with(|| {
			let case_index = helpers::create_ripe_case::<Test>();

			assert_noop!(
				exec_tx(
					Extrinsic::new_bare(
						Call::close_case { case_index, max_callback_weight: Weight::MAX }.into()
					),
					TransactionSource::Local,
				),
				TransactionExecutionError::Validity(TransactionValidityError::Unknown(
					UnknownTransaction::NoUnsignedValidator
				)),
			);
		});
	}

	#[test]
	fn validates_case_close_before_submitting_transaction() {
		let mut ext = new_test_ext();
		let (offchain, _state) = TestOffchainExt::new();
		let (pool, state) = TestTransactionPoolExt::new();
		ext.register_extension(OffchainDbExt::new(offchain.clone()));
		ext.register_extension(OffchainWorkerExt::new(offchain));
		ext.register_extension(TransactionPoolExt::new(pool));

		ext.execute_with(|| {
			// One open case exists
			helpers::create_voting_case::<Test>();

			// Offchain worker executes
			let interval: u64 = <Test as Config>::OffchainWorkInterval::get();
			System::set_block_number(interval);
			MobRule::offchain_worker(interval);

			// No transactions submitted
			assert_eq!(state.read().transactions.len(), 0);

			// One ripe case exists
			let case_index = helpers::create_ripe_case::<Test>();

			// Offchain worker executes again
			MobRule::offchain_worker(interval);

			// 1 transaction submitted
			assert_eq!(state.read().transactions.len(), 1);

			// and the transaction should be close_case call
			let transaction = state.write().transactions.pop().unwrap();
			let ex: Extrinsic = Decode::decode(&mut &*transaction).unwrap();
			let closed_case_index = match &ex.function {
				crate::mock::RuntimeCall::MobRule(crate::Call::close_case {
					case_index, ..
				}) => *case_index,
				e => panic!("Unexpected call: {e:?}"),
			};
			assert_eq!(closed_case_index, case_index);
			assert_ok!(exec_tx(ex, TransactionSource::Local));
			assert!(!RipeCases::<Test>::contains_key(case_index));
			assert!(DoneCases::<Test>::contains_key(case_index));
		});
	}

	#[test]
	fn validates_reap_case_before_submitting_transaction() {
		let mut ext = new_test_ext();
		let (offchain, _state) = TestOffchainExt::new();
		let (pool, state) = TestTransactionPoolExt::new();
		ext.register_extension(OffchainDbExt::new(offchain.clone()));
		ext.register_extension(OffchainWorkerExt::new(offchain));
		ext.register_extension(TransactionPoolExt::new(pool));

		ext.execute_with(|| {
			// One open case exists
			helpers::create_voting_case::<Test>();

			// Offchain worker executes
			let interval: u64 = <Test as Config>::OffchainWorkInterval::get();
			System::set_block_number(interval);
			MobRule::offchain_worker(interval);

			// No transactions submitted
			assert_eq!(state.read().transactions.len(), 0);

			// One done case exists that has timed out
			let done_case_index = helpers::create_done_case::<Test>(vec![], 0);

			// AND timeout for it has passed
			mock::Now::set(Duration::from_millis(constants::TWO_DAYS_MS));

			// Offchain worker executes again
			MobRule::offchain_worker(interval);

			// 1 transaction submitted
			assert_eq!(state.read().transactions.len(), 1);

			// and the transaction should be reap_case call
			let transaction = state.write().transactions.pop().unwrap();
			let ex: Extrinsic = Decode::decode(&mut &*transaction).unwrap();
			let reap_case_index = match &ex.function {
				crate::mock::RuntimeCall::MobRule(crate::Call::reap_case {
					case_index, ..
				}) => *case_index,
				e => panic!("Unexpected call: {e:?}"),
			};
			assert_eq!(done_case_index, reap_case_index);
			assert_ok!(exec_tx(ex, TransactionSource::Local));
			assert!(!DoneCases::<Test>::contains_key(done_case_index));
		});
	}

	#[test]
	fn validates_timeout_case_before_submitting_transaction() {
		let mut ext = new_test_ext();
		let (offchain, _state) = TestOffchainExt::new();
		let (pool, state) = TestTransactionPoolExt::new();
		ext.register_extension(OffchainDbExt::new(offchain.clone()));
		ext.register_extension(OffchainWorkerExt::new(offchain));
		ext.register_extension(TransactionPoolExt::new(pool));

		ext.execute_with(|| {
			// Enable voting at moment 0
			ActiveSince::<Test>::put(0);
			// One open case exists
			let case_index = helpers::create_voting_case::<Test>();

			// Offchain worker executes
			let interval: u64 = <Test as Config>::OffchainWorkInterval::get();
			System::set_block_number(interval);
			MobRule::offchain_worker(interval);

			// No transactions submitted
			assert_eq!(state.read().transactions.len(), 0);

			// The full period has passed now for the case
			// AND two weeks have passed since it's opening
			mock::Now::set(Duration::from_millis(constants::TWO_WEEKS_MS));

			// Offchain worker executes again
			MobRule::offchain_worker(interval);

			// 1 transaction submitted
			assert_eq!(state.read().transactions.len(), 1);

			// and the transaction should be case_timeout call
			let transaction = state.write().transactions.pop().unwrap();
			let ex: Extrinsic = Decode::decode(&mut &*transaction).unwrap();
			let timeout_case_index = match &ex.function {
				crate::mock::RuntimeCall::MobRule(crate::Call::force_ripen_case {
					case_index,
					..
				}) => *case_index,
				e => panic!("Unexpected call: {e:?}"),
			};
			assert_eq!(timeout_case_index, case_index);
			assert_ok!(exec_tx(ex, TransactionSource::Local));
			assert!(RipeCases::<Test>::contains_key(case_index));
		});
	}

	#[test]
	fn cleans_only_votes_in_done_cases() {
		let mut ext = new_test_ext();
		let (offchain, _state) = TestOffchainExt::new();
		let (pool, state) = TestTransactionPoolExt::new();
		ext.register_extension(OffchainDbExt::new(offchain.clone()));
		ext.register_extension(OffchainWorkerExt::new(offchain));
		ext.register_extension(TransactionPoolExt::new(pool));

		ext.execute_with(|| {
			// Two open cases exist
			let case_index_1 = helpers::create_voting_case::<Test>();
			helpers::create_voting_case::<Test>();
			// And the voter votes in the first one
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID),
				case_index_1,
				Judgement::Truth(True),
			));

			// Offchain worker executes
			let interval: u64 = <Test as Config>::OffchainWorkInterval::get();
			System::set_block_number(interval);
			MobRule::offchain_worker(interval);

			// No transactions submitted
			assert_eq!(state.read().transactions.len(), 0);

			// Case 1 becomes ripe
			// AND two days have passed since it's opening
			mock::Now::set(Duration::from_millis(constants::TWO_DAYS_MS));
			assert_ok!(MobRule::touch_case(RuntimeOrigin::signed(VOTER_VALID), case_index_1));
			// Case 1 becomes closed
			assert!(MobRule::close_case(authorized_origin(), case_index_1, Weight::MAX).is_ok());

			// Current time after the votes claims duration
			let claims_duration: u32 =
				<mock::Test as pallet::Config>::VotesOpenForClaimsDuration::get();
			let claims_duration_in_milis = claims_duration as u64 * 1000;
			let cased_closed_at_in_milis =
				DoneCases::<Test>::get(case_index_1).unwrap().since * 1000;
			mock::Now::set(Duration::from_millis(
				cased_closed_at_in_milis + claims_duration_in_milis * 2,
			));

			// Offchain worker executes again
			MobRule::offchain_worker(interval);

			// 1 transaction submitted
			assert_eq!(state.read().transactions.len(), 1);

			// and the transaction should be clean_vote call
			let transaction = state.write().transactions.pop().unwrap();
			let ex: Extrinsic = Decode::decode(&mut &*transaction).unwrap();
			let (clean_vote_case_index, clean_vote_voter) = match &ex.function {
				crate::mock::RuntimeCall::MobRule(crate::Call::clean_vote {
					case_index,
					voter,
					..
				}) => (*case_index, *voter),
				e => panic!("Unexpected call: {e:?}"),
			};
			assert_eq!(clean_vote_case_index, case_index_1);

			let voter = EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID));
			assert_eq!(clean_vote_voter, voter);
			assert_ok!(exec_tx(ex, TransactionSource::Local));
			assert!(!Votes::<Test>::contains_key(case_index_1, voter));
		});
	}

	mod votes_to_clean {
		use super::*;

		#[test]
		fn no_done_cases() {
			TestExt::new().execute_with(|| {
				// No done cases exist,
				// so the votes_to_clean function should return an empty vector
				assert!(MobRule::votes_to_clean().is_empty());
			});
		}

		#[test]
		fn one_done_case() {
			TestExt::new().execute_with(|| {
				// One done case exists with a vote
				let voter = constants::PERSON_0_ALIAS;
				let case_index = helpers::create_done_case::<Test>(vec![voter], 0);

				// Current time after the votes claims duration
				let claims_duration: u32 =
					<mock::Test as pallet::Config>::VotesOpenForClaimsDuration::get();
				let claims_duration_in_milis = claims_duration as u64 * 1000;
				mock::Now::set(Duration::from_millis(claims_duration_in_milis * 2));

				// Getting the votes to clean
				let votes = MobRule::votes_to_clean();

				// Should return one case with one voter
				assert_eq!(votes.len(), 1);
				assert_eq!(votes[0].0, case_index);
				assert_eq!(votes[0].1.len(), 1);
				assert_eq!(votes[0].1[0], voter);
			});
		}

		#[test]
		fn multiple_done_cases_with_multiple_votes_not_exceeding_limits() {
			TestExt::new().execute_with(|| {
				// 2 open and 2 done cases exist
				helpers::create_voting_case::<Test>();
				helpers::create_voting_case::<Test>();

				// Create done cases with votes
				let voter1 = constants::PERSON_0_ALIAS;
				let done_case_1 = helpers::create_done_case::<Test>(vec![voter1], 0);
				let done_case_2 = helpers::create_done_case::<Test>(vec![voter1], 0);

				// Add additional votes
				let voter2 = EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID_2));
				Votes::<Test>::insert(done_case_1, voter2, Judgement::Truth(True));
				Votes::<Test>::insert(done_case_2, voter1, Judgement::Truth(True));

				// Current time after the votes claims duration
				let claims_duration: u32 =
					<mock::Test as pallet::Config>::VotesOpenForClaimsDuration::get();
				let claims_duration_in_milis = claims_duration as u64 * 1000;
				mock::Now::set(Duration::from_millis(claims_duration_in_milis * 2));

				// Getting the votes to clean
				let votes = MobRule::votes_to_clean();

				// Should return two cases with their respective voters
				assert_eq!(votes.len(), 2);

				// First case should have two voters
				let votes_1 = votes.iter().find(|v| v.0 == done_case_1).unwrap();
				assert_eq!(votes_1.1.len(), 2);
				assert!(votes_1.1.contains(&voter1));
				assert!(votes_1.1.contains(&voter2));

				// Second case should have one voter
				let votes_2 = votes.iter().find(|v| v.0 == done_case_2).unwrap();
				assert_eq!(votes_2.1.len(), 1);
				assert!(votes_2.1.contains(&voter1));
			});
		}

		#[test]
		fn respects_votes_claims_duration() {
			TestExt::new().execute_with(|| {
				// One done case exists
				let done_case = helpers::create_done_case::<Test>(vec![], 0);

				// with a vote on it
				let voter1 = EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID));
				Votes::<Test>::insert(done_case, voter1, Judgement::Truth(True));

				// Current time is within the votes claims duration

				// Getting the votes to clean - should return empty vec as claims duration hasn't
				// passed
				let votes = MobRule::votes_to_clean();
				assert!(votes.is_empty());

				// Set current time to be after the votes claims duration
				let claims_duration: u32 =
					<mock::Test as pallet::Config>::VotesOpenForClaimsDuration::get();
				let claims_duration_in_milis = claims_duration as u64 * 1000;
				mock::Now::set(Duration::from_millis(claims_duration_in_milis * 2));

				// Getting the votes to clean - should now return the votes
				let votes = MobRule::votes_to_clean();
				assert_eq!(votes.len(), 1);
			});
		}

		#[test]
		fn respects_limits_on_per_case_and_total() {
			TestExt::new().execute_with(|| {
				// 3 done cases exist
				let done_case_1 = helpers::create_done_case::<Test>(vec![], 0);
				let done_case_2 = helpers::create_done_case::<Test>(vec![], 0);
				let done_case_3 = helpers::create_done_case::<Test>(vec![], 0);

				// with number of votes exceeding per-case limit
				let voter1 = EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID));
				let voter2 = EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID_2));
				let voter3 = EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID_3));
				let voter4 = EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID_4));

				Votes::<Test>::insert(done_case_1, voter1, Judgement::Truth(True));
				Votes::<Test>::insert(done_case_1, voter2, Judgement::Truth(True));
				Votes::<Test>::insert(done_case_1, voter3, Judgement::Truth(True));
				Votes::<Test>::insert(done_case_1, voter4, Judgement::Truth(True));

				Votes::<Test>::insert(done_case_2, voter1, Judgement::Truth(True));
				Votes::<Test>::insert(done_case_2, voter2, Judgement::Truth(True));
				Votes::<Test>::insert(done_case_2, voter3, Judgement::Truth(True));
				Votes::<Test>::insert(done_case_2, voter4, Judgement::Truth(True));

				Votes::<Test>::insert(done_case_3, voter1, Judgement::Truth(True));
				Votes::<Test>::insert(done_case_3, voter2, Judgement::Truth(True));
				Votes::<Test>::insert(done_case_3, voter3, Judgement::Truth(True));
				Votes::<Test>::insert(done_case_3, voter4, Judgement::Truth(True));

				// Current time after the votes claims duration
				let claims_duration: u32 =
					<mock::Test as pallet::Config>::VotesOpenForClaimsDuration::get();
				let claims_duration_in_milis = claims_duration as u64 * 1000;
				mock::Now::set(Duration::from_millis(claims_duration_in_milis * 2));

				// Getting the votes to clean
				let votes = MobRule::votes_to_clean();

				// Should return all the three cases but with limited voters
				assert_eq!(votes.len(), 3);

				let limit: u32 = <mock::Test as pallet::Config>::CleanVotesBatchSize::get();
				let per_case_limit: usize = limit as usize / 3;

				// First case should have votes limited by per-case limit
				let votes_1 = votes.iter().find(|v| v.0 == done_case_1).unwrap();
				assert_eq!(votes_1.1.len(), per_case_limit);

				// Second case should also respect the per-case limit
				let votes_2 = votes.iter().find(|v| v.0 == done_case_2).unwrap();
				assert_eq!(votes_2.1.len(), per_case_limit);

				// Same for the third one
				let votes_3 = votes.iter().find(|v| v.0 == done_case_3).unwrap();
				assert_eq!(votes_3.1.len(), per_case_limit);

				// Total number of votes should not exceed the total limit
				let total_votes: usize = votes.iter().map(|(_, voters)| voters.len()).sum();
				assert!(total_votes <= limit as usize);
			});
		}
	}
}

mod authorize {
	use super::*;
	use indiv_support::traits::Truth::True;
	use sp_runtime::transaction_validity::TransactionSource;
	use std::time::Duration;

	#[test]
	fn works_for_close_case() {
		TestExt::new().execute_with(|| {
			// Set-up to make the check validating close_case need pass
			let case_index = helpers::create_ripe_case::<Test>();

			// close_case call should succeed
			let max_callback_weight = ripe_case_callback_weight(case_index);
			let valid_call = Call::<Test>::close_case { case_index, max_callback_weight };
			assert!(valid_call.authorize(TransactionSource::Local).is_some());
			assert_ok!(valid_call.authorize(TransactionSource::Local).unwrap(),);
		});
	}

	#[test]
	fn close_case_authorize_discards_case_that_is_not_ripe() {
		TestExt::new().execute_with(|| {
			let case_index = helpers::create_voting_case::<Test>();
			let call = Call::<Test>::close_case { case_index, max_callback_weight: Weight::MAX };

			assert_eq!(
				call.authorize(TransactionSource::Local),
				Some(Err(CustomInvalidity::CaseNotRipe.into()))
			);
		});
	}

	#[test]
	fn close_case_authorize_rejects_too_low_callback_weight() {
		TestExt::new().execute_with(|| {
			let case_index = helpers::create_ripe_case::<Test>();
			let callback_weight = ripe_case_callback_weight(case_index);

			// A bound that does not cover the actual callback weight is rejected by `authorize`,
			// so the transaction never enters a block.
			let too_low = Call::<Test>::close_case {
				case_index,
				max_callback_weight: callback_weight.saturating_sub(Weight::from_parts(1, 0)),
			};
			assert_eq!(
				too_low.authorize(TransactionSource::Local),
				Some(Err(CustomInvalidity::CallbackWeightTooLow.into()))
			);

			// The exact callback weight, and any larger upper bound, are accepted.
			let exact =
				Call::<Test>::close_case { case_index, max_callback_weight: callback_weight };
			assert_ok!(exact.authorize(TransactionSource::Local).unwrap());
			let over = Call::<Test>::close_case {
				case_index,
				max_callback_weight: callback_weight.saturating_add(Weight::from_parts(1, 1)),
			};
			assert_ok!(over.authorize(TransactionSource::Local).unwrap());
		});
	}

	#[test]
	fn works_for_clean_vote() {
		TestExt::new().execute_with(|| {
			// Set-up to make the check validating clean_vote need pass
			let case_index = helpers::create_voting_case::<Test>();
			assert_ok!(MobRule::vote(
				RuntimeOrigin::signed(VOTER_VALID),
				case_index,
				Judgement::Truth(True),
			));
			// AND two days have passed since it's opening
			mock::Now::set(Duration::from_millis(constants::TWO_DAYS_MS));
			assert_ok!(MobRule::touch_case(RuntimeOrigin::signed(VOTER_VALID), case_index));
			assert!(MobRule::close_case(authorized_origin(), case_index, Weight::MAX).is_ok());

			// Vote is no longer open for claim - the claim period has passed
			mock::Now::set(Duration::from_millis(
				constants::TWO_DAYS_MS + constants::ONE_HOUR_MS * 2,
			));

			// clean_vote call should succeed
			let voter = EnsureAliasLowerThan5::get_alias(RuntimeOrigin::signed(VOTER_VALID));
			let valid_call = Call::<Test>::clean_vote { case_index, voter };
			assert!(valid_call.authorize(TransactionSource::Local).is_some());
			assert_ok!(valid_call.authorize(TransactionSource::Local).unwrap(),);
		});
	}

	#[test]
	fn works_for_reap_case() {
		TestExt::new().execute_with(|| {
			// Set-up to make the check validating reap_case need pass
			let case_index = helpers::create_done_case::<Test>(vec![], 0);

			// AND timeout for it has passed
			mock::Now::set(Duration::from_millis(constants::TWO_DAYS_MS));

			// reap_case call should succeed
			let valid_call = Call::<Test>::reap_case { case_index };
			assert!(valid_call.authorize(TransactionSource::Local).is_some());
			assert_ok!(valid_call.authorize(TransactionSource::Local).unwrap(),);
		});
	}

	#[test]
	fn reap_case_authorize_discards_case_before_claim_window_ends() {
		TestExt::new().execute_with(|| {
			let case_index = helpers::create_done_case::<Test>(vec![], 0);
			let call = Call::<Test>::reap_case { case_index };

			assert_eq!(
				call.authorize(TransactionSource::Local),
				Some(Err(CustomInvalidity::CaseTooRecent.into()))
			);
		});
	}

	#[test]
	fn reap_case_authorize_discards_case_that_is_not_done() {
		TestExt::new().execute_with(|| {
			let call = Call::<Test>::reap_case { case_index: 0 };

			assert_eq!(
				call.authorize(TransactionSource::Local),
				Some(Err(CustomInvalidity::CaseNotDone.into()))
			);
		});
	}

	#[test]
	fn works_for_case_timeout() {
		TestExt::new().execute_with(|| {
			// Enable voting at moment 0
			ActiveSince::<Test>::put(0);
			// Set-up to make the check validating case_timeout need pass
			let case_index = helpers::create_voting_case::<Test>();
			// AND two weeks have passed since it's opening
			mock::Now::set(Duration::from_millis(constants::TWO_WEEKS_MS));

			// clean_vote call should succeed
			let valid_call = Call::<Test>::force_ripen_case { case_index };
			assert!(valid_call.authorize(TransactionSource::Local).is_some());
			assert_ok!(valid_call.authorize(TransactionSource::Local).unwrap(),);
		});
	}

	#[test]
	fn force_ripen_case_authorize_discards_case_before_voting_timeout() {
		TestExt::new().execute_with(|| {
			ActiveSince::<Test>::put(0);
			let case_index = helpers::create_voting_case::<Test>();
			let call = Call::<Test>::force_ripen_case { case_index };

			assert_eq!(
				call.authorize(TransactionSource::Local),
				Some(Err(CustomInvalidity::CaseTooRecent.into()))
			);
		});
	}

	#[test]
	fn force_ripen_case_authorize_discards_case_that_is_not_open() {
		TestExt::new().execute_with(|| {
			let call = Call::<Test>::force_ripen_case { case_index: 0 };

			assert_eq!(
				call.authorize(TransactionSource::Local),
				Some(Err(CustomInvalidity::CaseNotOpen.into()))
			);
		});
	}

	#[test]
	fn force_ripen_case_authorize_discards_when_expiration_is_disabled() {
		TestExt::new().execute_with(|| {
			let case_index = helpers::create_voting_case::<Test>();
			let call = Call::<Test>::force_ripen_case { case_index };

			assert_eq!(
				call.authorize(TransactionSource::Local),
				Some(Err(CustomInvalidity::CaseExpirationDisabled.into()))
			);
		});
	}

	#[test]
	fn fails_for_other_calls() {
		TestExt::new().execute_with(|| {
			let vote_call = Call::<Test>::vote { case_index: 0, opinion: Judgement::Contempt };
			assert_eq!(vote_call.authorize(TransactionSource::Local), None);

			let intervene_call = Call::<Test>::intervene {
				case_index: 0,
				verdict: Judgement::Contempt,
				max_callback_weight: Weight::MAX,
			};
			assert_eq!(intervene_call.authorize(TransactionSource::Local), None);

			let claim_vote_call = Call::<Test>::claim_vote { case_index: 0 };
			assert_eq!(claim_vote_call.authorize(TransactionSource::Local), None);
		});
	}
}

mod hooks {
	use super::*;
	use frame_support::{
		traits::{Get, OnPoll},
		weights::WeightMeter,
	};
	use sp_runtime::Weight;
	use std::time::Duration;

	use crate::{ActiveSince, WeightInfo};

	/// The default mock configuration must pass every integrity check, including the block-fit
	/// assertions for the offchain-worker-submitted authorized calls.
	#[test]
	fn integrity_test_passes() {
		TestExt::new().execute_with(|| {
			<crate::Pallet<Test> as frame_support::traits::Hooks<u64>>::integrity_test();
		});
	}

	#[test]
	fn on_poll_works() {
		TestExt::new().execute_with(|| {
			let threshold = <Test as Config>::MinimumVoterThreshold::get();
			let base_weight = <<Test as Config>::WeightInfo as WeightInfo>::on_poll_base();
			let set_weight = <<Test as Config>::WeightInfo as WeightInfo>::set_active_since();
			let kill_weight = <<Test as Config>::WeightInfo as WeightInfo>::kill_active_since();
			EnsureAliasLowerThan5::set_voter_count(0);
			ActiveSince::<Test>::kill();

			mock::Now::set(Duration::from_millis(constants::TWO_WEEKS_MS));
			let mut meter = WeightMeter::new();
			MobRule::on_poll(0, &mut meter);
			assert!(!ActiveSince::<Test>::exists());

			EnsureAliasLowerThan5::set_voter_count(threshold);
			let mut meter = WeightMeter::with_limit(base_weight.saturating_div(2));
			// Not enough weight to do the checks.
			MobRule::on_poll(0, &mut meter);
			assert!(!ActiveSince::<Test>::exists());
			assert_eq!(meter.consumed(), Weight::zero());

			// `on_poll` halves the meter's remaining weight before dispatching to
			// `do_on_poll`, so each outer limit below is `2x` the inner budget under test.
			let mut meter = WeightMeter::with_limit(base_weight.saturating_mul(2));
			// Not enough weight to set the value.
			MobRule::on_poll(0, &mut meter);
			assert!(!ActiveSince::<Test>::exists());
			assert_eq!(meter.consumed(), base_weight);

			let mut meter =
				WeightMeter::with_limit(base_weight.saturating_add(set_weight).saturating_mul(2));
			// Value is set.
			MobRule::on_poll(0, &mut meter);
			let activation_time = <Test as Config>::Clock::now().as_secs();
			assert_eq!(ActiveSince::<Test>::get().unwrap(), activation_time);
			assert_eq!(meter.consumed(), base_weight.saturating_add(set_weight));

			mock::Now::set(Duration::from_millis(constants::TWO_WEEKS_MS * 2));

			let mut meter = WeightMeter::with_limit(base_weight.saturating_mul(2));
			// Value stays the same.
			MobRule::on_poll(0, &mut meter);
			assert_eq!(ActiveSince::<Test>::get().unwrap(), activation_time);
			assert_eq!(meter.consumed(), base_weight);

			EnsureAliasLowerThan5::set_voter_count(threshold - 1);

			let mut meter = WeightMeter::with_limit(base_weight.saturating_div(2));
			// Not enough weight to do the checks.
			MobRule::on_poll(0, &mut meter);
			assert_eq!(ActiveSince::<Test>::get().unwrap(), activation_time);
			assert_eq!(meter.consumed(), Weight::zero());

			let mut meter = WeightMeter::with_limit(base_weight.saturating_mul(2));
			// Not enough weight to kill the value.
			MobRule::on_poll(0, &mut meter);
			assert_eq!(ActiveSince::<Test>::get().unwrap(), activation_time);
			assert_eq!(meter.consumed(), base_weight);

			let mut meter =
				WeightMeter::with_limit(base_weight.saturating_add(kill_weight).saturating_mul(2));
			// Value is killed.
			MobRule::on_poll(0, &mut meter);
			assert!(!ActiveSince::<Test>::exists());
			assert_eq!(meter.consumed(), base_weight.saturating_add(kill_weight));

			EnsureAliasLowerThan5::set_voter_count(threshold * 2);
			let mut meter =
				WeightMeter::with_limit(base_weight.saturating_add(set_weight).saturating_mul(2));
			// Value is set.
			MobRule::on_poll(0, &mut meter);
			let activation_time = <Test as Config>::Clock::now().as_secs();
			assert_eq!(ActiveSince::<Test>::get().unwrap(), activation_time);
			assert_eq!(meter.consumed(), base_weight.saturating_add(set_weight));
		})
	}
}

#[test]
fn invalid_if_not_local() {
	TestExt::new().execute_with(|| {
		use sp_runtime::transaction_validity::{InvalidTransaction, TransactionSource};

		// clean_vote must be invalid if not local
		let call = Call::<Test>::clean_vote { case_index: 0, voter: constants::PERSON_0_ALIAS };
		assert_eq!(
			call.authorize(TransactionSource::External),
			Some(Err(InvalidTransaction::BadSigner.into()))
		);
	});
}

#[test]
fn clean_vote_signed_works() {
	TestExt::new().execute_with(|| {
		advance_to(1);
		// GIVEN a done case with a recorded vote
		let voter = constants::PERSON_0_ALIAS;
		let case_index = helpers::create_done_case::<Test>(vec![voter], 0);

		// AND the claim window has elapsed
		let claims_duration_secs: u32 =
			<mock::Test as pallet::Config>::VotesOpenForClaimsDuration::get();
		let claims_duration_ms = claims_duration_secs as u64 * 1000;
		mock::Now::set(Duration::from_millis(claims_duration_ms * 2));

		// WHEN the signed variant is called
		assert_ok!(MobRule::clean_vote_signed(
			RuntimeOrigin::signed(VOTER_VALID),
			case_index,
			voter
		));

		// THEN the vote is removed and credit updated
		assert!(!Votes::<Test>::contains_key(case_index, voter));
		let credit = Credits::<Test>::get(voter);
		assert_eq!(credit.cleaned, 1);
		assert_eq!(credit.correct, 1);
		System::assert_has_event(Event::VoteCleaned { case_index, voter }.into());
	});
}
