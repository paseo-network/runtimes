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

#![allow(clippy::unit_arg)] // TODO: Perhaps fix the need for this.

// To simplify tests AccountId == PersonalId and account_id == personal_id for any given account_id

use super::{mock::*, *};
use crate::extension::AsProofOfInkParticipantInfo;
use frame_support::{assert_noop, assert_ok, dispatch::Pays};
use indiv_support::traits::Truth;
use sp_runtime::{
	bounded_vec,
	testing::TestSignature,
	traits::{BadOrigin, Get},
	transaction_validity::InvalidTransaction,
	BoundedVec, DispatchError,
};
use verifiable::mock::Mock;

#[test]
fn basic_initialize_works() {
	TestExt::new().execute_with(|| {});
}

#[test]
fn denied_payment_tx_ext_works() {
	TestExt::new().execute_with(|| {
		assert_noop!(
			exec_tx(DENIED_PAYMENT_ACCOUNT, 0, PoICall::reroll {}, None),
			InvalidTransaction::Payment
		);
	});
}

#[test]
fn advance_to_works() {
	TestExt::new().execute_with(|| {
		assert_eq!(System::block_number(), 0);
		advance_to(10);
		assert_eq!(System::block_number(), 10);
		advance_to(11);
		assert_eq!(System::block_number(), 11);
	});
}

#[test]
fn advance_by_works() {
	TestExt::new().execute_with(|| {
		assert_eq!(System::block_number(), 0);
		advance_by(10);
		assert_eq!(System::block_number(), 10);
		advance_by(11);
		assert_eq!(System::block_number(), 21);
	});
}

#[test]
fn mock_people_works() {
	TestExt::new().execute_with(|| {
		let id = MockNextId::<Test>::get();
		MockPeople::renew_id_reservation(id).unwrap_err();
		assert_eq!(MockPeople::reserve_new_id(), id);
		MockPeople::renew_id_reservation(id).unwrap_err();
		assert_eq!(MockPeople::reserve_new_id(), id + 1);
		assert_ok!(MockPeople::cancel_id_reservation(id));
		assert_eq!(MockPeople::reserve_new_id(), id + 2);
		assert_ok!(MockPeople::renew_id_reservation(id));
	});
}

#[test]
fn reusing_id_works() {
	TestExt::new().execute_with(|| {
		const PERSON: AccountId = 0;
		const CANDIDATE: AccountId = 1;
		const RESERVER: AccountId = 2;

		assert_ok!(mock_person(PERSON, None));
		assert_ok!(mock_candidate(CANDIDATE, None, None, None, false));
		assert!(People::<Test>::contains_key(PERSON));
		assert!(!People::<Test>::contains_key(CANDIDATE));
		assert!(MockReserved::<Test>::contains_key(CANDIDATE));
		assert!(!People::<Test>::contains_key(RESERVER));
		assert!(!MockReserved::<Test>::contains_key(RESERVER));

		// Ensure ID of existing person or candidate cannot be claimed
		assert_noop!(
			MockPeople::renew_id_reservation(CANDIDATE),
			DispatchError::Other("Invalid id reservation")
		);
		assert_noop!(
			MockPeople::renew_id_reservation(PERSON),
			DispatchError::Other("Invalid id reservation")
		);

		// Ensure an unreserved ID can be reserved
		assert_ok!(MockPeople::cancel_id_reservation(CANDIDATE));
		assert_ok!(MockPeople::renew_id_reservation(CANDIDATE));
		assert!(MockReserved::<Test>::contains_key(CANDIDATE));
	});
}

#[test]
fn mocks_work() {
	TestExt::new().execute_with(|| {
		assert_ok!(mock_designs());
		for i in 0..3 {
			assert_ok!(mock_person(i, None));
			assert_eq!(People::<Test>::try_get(i).unwrap().design, None);
		}

		// Mock a candidate who has applied with a deposit
		assert_ok!(mock_candidate(5, None, None, None, false));
		assert!(matches!(
			Candidates::<Test>::get(5).unwrap(),
			Candidate::Applied { cred: Credibility::Deposit(_), .. }
		));

		// Mock a candidate who has applied with a referral
		assert_ok!(mock_candidate(6, Some(0), None, None, false));
		assert!(matches!(
			Candidates::<Test>::get(6).unwrap(),
			Candidate::Applied { cred: Credibility::Referred(_), .. }
		));
		assert_eq!(People::<Test>::get(0).unwrap().active_referrals.to_vec(), vec![6]);

		// Mock a candidate who has committed to a design
		assert_ok!(mock_candidate(
			7,
			None,
			Some((InkChoice::DesignedElective(0, 0), Allocation::Full)),
			None,
			false
		));
		assert!(matches!(
			Candidates::<Test>::get(7).unwrap(),
			Candidate::Selected { judging, allocation, .. }
				if judging.is_none() && allocation == Allocation::Full
		));

		// Mock a candidate in the Selected state who is in the process of being judged
		assert_ok!(mock_candidate(
			8,
			None,
			Some((InkChoice::DesignedElective(0, 1), Allocation::Full)),
			Some(Default::default()),
			false
		));
		assert!(matches!(
			Candidates::<Test>::get(8).unwrap(),
			Candidate::Selected { judging, allocation, .. }
				if judging.is_some() && allocation == Allocation::Full
		));

		// Mock a candidate in the Selected state who has been successfully judged
		assert_ok!(mock_candidate(
			9,
			None,
			Some((InkChoice::DesignedElective(0, 2), Allocation::Full)),
			None,
			true
		));
		assert!(matches!(
			Candidates::<Test>::get(9).unwrap(),
			Candidate::Proven { was_referred: false, .. }
		));
	});
}

#[test]
fn bake_design_works() {
	TestExt::new().execute_with(|| {
		const PARENT: AccountId = 0;
		const SECONDARY: AccountId = 1;
		const NO_DESIGN: AccountId = 3;
		const NON_PERSON: AccountId = 4;
		const NOT_PROCEDURAL: AccountId = 5;
		const CANDIDATE: AccountId = 9;

		assert_ok!(mock_designs());
		let entropy = get_entropy(CANDIDATE);
		let next_id = MockNextId::<Test>::get();

		// A lot of matches in here since Error doesn't implement PartialEq
		// Designs from correct families work
		assert!(matches!(
			PoI::bake_design(InkChoice::DesignedElective(0, 0), entropy, CANDIDATE, next_id),
			Ok(InkSpec::DesignedElective(0, 0))
		));
		assert!(matches!(
			PoI::bake_design(InkChoice::Procedural(1, 0), entropy, CANDIDATE, next_id),
			Ok(InkSpec::Procedural(1, _))
		));
		assert!(matches!(
			PoI::bake_design(InkChoice::ProceduralAccount(2), entropy, CANDIDATE, next_id),
			Ok(InkSpec::ProceduralAccount(2, _))
		));
		assert!(matches!(
			PoI::bake_design(InkChoice::ProceduralPersonal(3), entropy, CANDIDATE, next_id),
			Ok(InkSpec::ProceduralPersonal(3, id))
			if id == next_id
		));

		// Bad family, family 4 does not exist
		assert!(matches!(
			PoI::bake_design(InkChoice::DesignedElective(4, 0), entropy, CANDIDATE, next_id),
			Err(Error::<Test>::BadFamily)
		));
		assert!(matches!(
			PoI::bake_design(InkChoice::Procedural(4, 0), entropy, CANDIDATE, next_id),
			Err(Error::<Test>::BadFamily)
		));
		assert!(matches!(
			PoI::bake_design(InkChoice::ProceduralAccount(4), entropy, CANDIDATE, next_id),
			Err(Error::<Test>::BadFamily)
		));
		assert!(matches!(
			PoI::bake_design(InkChoice::ProceduralPersonal(4), entropy, CANDIDATE, next_id),
			Err(Error::<Test>::BadFamily)
		));

		// Incorrect family for each branch
		assert!(matches!(
			PoI::bake_design(InkChoice::DesignedElective(1, 0), entropy, CANDIDATE, next_id),
			Err(Error::<Test>::WrongFamily)
		));
		assert!(matches!(
			PoI::bake_design(InkChoice::Procedural(2, 0), entropy, CANDIDATE, next_id),
			Err(Error::<Test>::WrongFamily)
		));
		assert!(matches!(
			PoI::bake_design(InkChoice::ProceduralAccount(3), entropy, CANDIDATE, next_id),
			Err(Error::<Test>::WrongFamily)
		));
		assert!(matches!(
			PoI::bake_design(InkChoice::ProceduralPersonal(0), entropy, CANDIDATE, next_id),
			Err(Error::<Test>::WrongFamily)
		));

		// Procedural and design are within range - range and count are both 10
		assert!(matches!(
			PoI::bake_design(InkChoice::DesignedElective(0, 1000), entropy, CANDIDATE, next_id),
			Err(Error::<Test>::IndexTooBig)
		));
		assert!(matches!(
			PoI::bake_design(InkChoice::Procedural(1, 10), entropy, CANDIDATE, next_id),
			Err(Error::<Test>::IndexTooBig)
		));

		// ProceduralDerivative
		const PARENT_SEED: ProceduralSeed = [PARENT as u8; 4];
		const SECONDARY_SEED: ProceduralSeed = [SECONDARY as u8; 4];
		assert_ok!(mock_person(PARENT, Some(InkSpec::Procedural(1, PARENT_SEED))));
		assert_ok!(mock_person(SECONDARY, Some(InkSpec::Procedural(1, SECONDARY_SEED))));
		assert_ok!(mock_person(NOT_PROCEDURAL, Some(InkSpec::DesignedElective(0, 0))));
		assert_ok!(mock_person(NO_DESIGN, None));

		// Works with one or two valid parents
		assert!(matches!(
			PoI::bake_design(
				InkChoice::ProceduralDerivative(PARENT, None),
				entropy,
				CANDIDATE,
				next_id
			),
			Ok(InkSpec::Procedural(1, _))
		));
		assert_ok!(PoI::bake_design(
			InkChoice::ProceduralDerivative(PARENT, Some(SECONDARY)),
			entropy,
			CANDIDATE,
			next_id
		));

		// Parent must be a valid person with a design which is Procedural
		assert!(matches!(
			PoI::bake_design(
				InkChoice::ProceduralDerivative(NO_DESIGN, None),
				entropy,
				CANDIDATE,
				next_id
			),
			Err(Error::<Test>::BadParent)
		));
		assert!(matches!(
			PoI::bake_design(
				InkChoice::ProceduralDerivative(NON_PERSON, None),
				entropy,
				CANDIDATE,
				next_id
			),
			Err(Error::<Test>::BadParent)
		));
		assert!(matches!(
			PoI::bake_design(
				InkChoice::ProceduralDerivative(NOT_PROCEDURAL, None),
				entropy,
				CANDIDATE,
				next_id
			),
			Err(Error::<Test>::DesignInvalid)
		));

		// Secondary must be a valid person with a design which is Procedural
		assert!(matches!(
			PoI::bake_design(
				InkChoice::ProceduralDerivative(PARENT, Some(NO_DESIGN)),
				entropy,
				CANDIDATE,
				next_id
			),
			Err(Error::<Test>::BadParent)
		));
		assert!(matches!(
			PoI::bake_design(
				InkChoice::ProceduralDerivative(PARENT, Some(NON_PERSON)),
				entropy,
				CANDIDATE,
				next_id
			),
			Err(Error::<Test>::BadParent)
		));
		assert!(matches!(
			PoI::bake_design(
				InkChoice::ProceduralDerivative(PARENT, Some(NOT_PROCEDURAL)),
				entropy,
				CANDIDATE,
				next_id
			),
			Err(Error::<Test>::DesignInvalid)
		));
	});
}

#[test]
fn apply_works() {
	TestExt::new().execute_with(|| {
		const CANDIDATE: AccountId = 9;

		advance_by(1);
		// Check that a signed origin can apply
		assert_ok!(PoI::apply(RuntimeOrigin::signed(CANDIDATE)));
		System::assert_last_event(Event::CandidateApplied { account_id: CANDIDATE }.into());
		assert!(matches!(
			Candidates::<Test>::get(CANDIDATE),
			Some(Candidate::Applied { cred, .. } )
			if matches!(
				cred,
				Credibility::Deposit(_)
			)
		));

		// Cannot apply twice
		advance_by(1);
		assert_noop!(PoI::apply(RuntimeOrigin::signed(CANDIDATE)), Error::<Test>::InProgress);

		// Applications are limited to signed origins
		assert_noop!(PoI::apply(RuntimeOrigin::none()), BadOrigin);
		assert_noop!(PoI::apply(RuntimeOrigin::root()), BadOrigin);
	});
}

#[test]
fn submit_evidence_works() {
	TestExt::new().execute_with(|| {
		const REFERRER: AccountId = 0;
		const CANDIDATE: AccountId = 9;
		const RETRY_CANDIDATE: AccountId = 8;

		let evidence = mock_evidence();

		advance_by(1);
		// Must be signed origin
		assert_noop!(PoI::submit_evidence(RuntimeOrigin::root(), evidence), BadOrigin);
		assert_noop!(PoI::submit_evidence(RuntimeOrigin::none(), evidence), BadOrigin);

		// Candidate must have applied
		assert_noop!(
			PoI::submit_evidence(RuntimeOrigin::signed(CANDIDATE), evidence),
			Error::<Test>::NotApplied
		);

		// Candidate must have committed
		assert_ok!(mock_candidate(CANDIDATE, None, None, None, false));
		advance_by(1);
		assert_noop!(
			PoI::submit_evidence(RuntimeOrigin::signed(CANDIDATE), evidence),
			Error::<Test>::NotSelected
		);

		// Ensure committed candidate can start a judgement
		assert_ok!(mock_designs());
		assert_ok!(mock_person(REFERRER, None));
		assert_ok!(mock_candidate(
			CANDIDATE,
			None,
			Some((InkChoice::DesignedElective(0, 0), Allocation::InitDone)),
			None,
			false
		));
		advance_by(1);
		let initial_alloc_count = AllocationCount::<Test>::get();
		assert_ok!(
			PoI::submit_evidence(RuntimeOrigin::signed(CANDIDATE), evidence),
			Pays::No.into()
		);
		System::assert_last_event(Event::JudgementRequested { account_id: CANDIDATE }.into());
		assert!(matches!(
			Candidates::<Test>::get(CANDIDATE).unwrap(),
			Candidate::Selected { allocation, judging, .. }
				if allocation == Allocation::InitDone && judging.is_some()
		));
		assert_eq!(AllocationCount::<Test>::get(), initial_alloc_count);

		// Check that submit_evidence returns the correct weight for a retrying candidate
		let Ok(Candidate::Selected {
			cred,
			since,
			design,
			allocation,
			reserved,
			entropy,
			judging,
			..
		}) = mock_candidate(
			RETRY_CANDIDATE,
			None,
			Some((InkChoice::DesignedElective(0, 1), Allocation::InitDone)),
			None,
			false,
		)
		else {
			unreachable!("Candidate created in this state.");
		};
		Candidates::<Test>::insert(
			RETRY_CANDIDATE,
			Candidate::Selected {
				since,
				cred,
				reserved,
				entropy,
				design,
				allocation,
				judging,
				failed: 1,
			},
		);

		advance_by(1);
		let initial_alloc_count = AllocationCount::<Test>::get();
		assert_ok!(PoI::submit_evidence(RuntimeOrigin::signed(RETRY_CANDIDATE), evidence));
		System::assert_last_event(Event::JudgementRequested { account_id: RETRY_CANDIDATE }.into());
		assert!(matches!(
			Candidates::<Test>::get(RETRY_CANDIDATE).unwrap(),
			Candidate::Selected { allocation, judging, .. }
				if allocation == Allocation::InitDone && judging.is_some()
		));
		assert_eq!(AllocationCount::<Test>::get(), initial_alloc_count);

		// Test probable acceptable path (needs better judge_statement mock) - TODO

		// Candidate can't call submit_evidence when they've already started
		advance_by(1);
		assert_noop!(
			PoI::submit_evidence(RuntimeOrigin::signed(CANDIDATE), evidence),
			Error::<Test>::AlreadyStarted
		);
	});
}

#[test]
fn judged_happy_path_works() {
	use Judgement::*;
	TestExt::new().execute_with(|| {
		const PERSON: AccountId = 0;
		const NON_PERSON: AccountId = AccountId::MAX;
		const CANDIDATE: AccountId = 9;
		const UNPREPARED_CANDIDATE: AccountId = 8;

		advance_by(1);
		assert_ok!(mock_designs());
		assert_ok!(mock_person(PERSON, None));

		let (ticket, context) = prepare_for_judgement(CANDIDATE);
		assert_ok!(mock_candidate(
			CANDIDATE,
			None,
			Some((InkChoice::DesignedElective(0, 0), Allocation::Full)),
			Some(ticket),
			false
		));
		let judgement = Truth(True);

		// Only root can call judged
		let disallowed_origins = vec![
			RuntimeOrigin::signed(CANDIDATE),
			RuntimeOrigin::signed(PERSON),
			RuntimeOrigin::signed(NON_PERSON),
			RuntimeOrigin::none(),
		];
		for origin in disallowed_origins {
			assert_noop!(PoI::judged(origin, ticket, context.clone(), judgement), BadOrigin);
		}

		// Ensure ticket matches candidate's ticket
		let wrong_ticket: OracleTicketOf<Test> = [1u8; 32];
		assert_noop!(
			PoI::judged(RuntimeOrigin::root(), wrong_ticket, context.clone(), judgement),
			Error::<Test>::UnexpectedJudgement
		);

		// Candidate needs to have called submit_evidence for the callback to be allowed
		let (ticket, context) = prepare_for_judgement(UNPREPARED_CANDIDATE);
		assert_ok!(mock_candidate(
			UNPREPARED_CANDIDATE,
			None,
			Some((InkChoice::DesignedElective(0, 1), Allocation::Full)),
			None,
			false
		));
		assert_noop!(
			PoI::judged(RuntimeOrigin::root(), ticket, context.clone(), judgement),
			Error::<Test>::BadContext
		);

		let test_cases: Vec<JudgedTest> = vec![
			// JudgedTest::new(judgement, allocation, referred, should_succeed, soft_fail,
			// bannable) Applied candidate contemptuous
			JudgedTest::new(Contempt, Allocation::Full, false, false, false, true),
			// Referred candidate contemptuous
			JudgedTest::new(Contempt, Allocation::Full, true, false, false, true),
			// Applied candidate false
			JudgedTest::new(Truth(False), Allocation::Full, false, false, false, false),
			// Referred candidate false
			JudgedTest::new(Truth(False), Allocation::Full, true, false, false, false),
			// Applied candidate truthful, initial allocation
			JudgedTest::new(Truth(True), Allocation::Initial, false, false, true, false),
			// Applied candidate truthful
			JudgedTest::new(Truth(True), Allocation::Full, false, true, false, false),
			// Referred candidate truthful
			JudgedTest::new(Truth(True), Allocation::Full, true, true, false, false),
		];
		for (
			i,
			JudgedTest { judgement, allocation, referred, should_succeed, soft_fail, bannable },
		) in test_cases.iter().enumerate()
		{
			let referrer = if *referred {
				let referrer_id = (i + 10) as AccountId;
				assert_ok!(mock_person(referrer_id, None));
				Some(referrer_id)
			} else {
				None
			};

			let was_referred = referred;

			let candidate = (i + 100) as AccountId;
			let (ticket, context) = prepare_for_judgement(candidate);

			let Ok(Candidate::Selected { design, reserved, allocation, .. }) = mock_candidate(
				candidate,
				referrer,
				Some((
					InkChoice::DesignedElective(0, (i + 3).try_into().unwrap()),
					allocation.clone(),
				)),
				Some(ticket),
				false,
			) else {
				unreachable!("Candidate created in this state.")
			};
			if !*soft_fail {
				Candidates::<Test>::mutate(candidate, |info| {
					if let Some(Candidate::Selected { failed, .. }) = info.as_mut() {
						*failed = 1;
					}
				});
			}

			advance_by(1);
			let alloc_count = AllocationCount::<Test>::get();
			assert_ok!(
				PoI::judged(RuntimeOrigin::root(), ticket, context.clone(), *judgement),
				PostDispatchInfo {
					actual_weight: Some(<Test as Config>::WeightInfo::judged(0)),
					pays_fee: Pays::No
				}
			);
			System::assert_last_event(
				Event::JudgementProvided { account_id: candidate, judgement: *judgement }.into(),
			);
			let mut active: BoundedVec<_, <Test as crate::Config>::MaxActiveReferrals> =
				bounded_vec![];
			let mut good = 0;
			let mut bad = 0;

			// Contrived if statement to fit test cases
			if allocation == Allocation::Initial {
				// Special case where Probable can be OK
				active = bounded_vec![candidate];
				// Candidate has no failures and can now request full allocation (InitDone)
				assert!(matches!(
					Candidates::<Test>::get(candidate),
					Some(Candidate::Selected{
						failed,
						allocation: updated_allocation,
						..
					})
						if failed == 0
							&& updated_allocation == Allocation::InitDone
				));
			} else if *should_succeed {
				// All good, allocation removed as proof has been provided
				good = 1;
				assert_eq!(AllocationCount::<Test>::get(), alloc_count - 1);
				assert!(matches!(
					Candidates::<Test>::get(candidate),
					Some(Candidate::Proven {
						design: actual_design,
						reserved: actual_reserved,
						was_referred: actual_was_referred,
						was_invited: actual_was_invited
					})
						if actual_design == design
							&& actual_reserved == reserved
							&& actual_was_referred == *was_referred
							&& !actual_was_invited
				));
			} else if *bannable {
				// Candidate and allocation are removed, bad referral if referred
				bad = 1;
				assert_eq!(AllocationCount::<Test>::get(), alloc_count - 1);
				assert!(!Candidates::<Test>::contains_key(candidate));
			} else if *soft_fail {
				// Soft fail, allocation remains and candidate remains active
				active = bounded_vec![candidate];
				assert_eq!(AllocationCount::<Test>::get(), alloc_count);
				assert!(matches!(
					Candidates::<Test>::get(candidate),
					Some(Candidate::Selected{failed, ..})
					if failed == 1
				));
			} else {
				bad = 1;
				assert_eq!(AllocationCount::<Test>::get(), alloc_count - 1);
				assert!(!Candidates::<Test>::contains_key(candidate));
			}

			match referrer {
				Some(referrer) => {
					assert!(matches!(
						People::<Test>::get(referrer).unwrap(),
						Person {
							active_referrals,
							successful_referrals,
							referrals,
							bad_referrals,
							banned,
							..
						}
							if active_referrals == active
								&& successful_referrals == good
								&& bad_referrals == bad
								&& referrals == active.len() as u32 + good + bad
								&& banned == *bannable
					));
				},
				None => {
					if *bannable {
						// Check ticket was burnt (needs better consideration mock) - TODO
					} else {
						// Check ticket was dropped (needs better consideration mock) - TODO
					}
				},
			}
		}
	});
}

#[test]
fn bad_referral_limit_works() {
	TestExt::new().execute_with(|| {
		const PERSON: AccountId = 0;
		const CANDIDATE: AccountId = 9;
		const REFERRED_CANDIDATE: AccountId = 8;

		advance_by(1);
		assert_ok!(mock_designs());
		assert_ok!(mock_person(PERSON, None));

		let (regular_ticket, regular_context) = prepare_for_judgement(CANDIDATE);
		let (referred_ticket, referred_context) = prepare_for_judgement(REFERRED_CANDIDATE);
		assert_ok!(mock_candidate(
			CANDIDATE,
			None,
			Some((InkChoice::DesignedElective(0, 0), Allocation::Full)),
			Some(regular_ticket),
			false
		));
		Candidates::<Test>::mutate(CANDIDATE, |maybe_state| {
			let Some(Candidate::Selected { failed, .. }) = maybe_state.as_mut() else {
				unreachable!("Candidate created in this state.")
			};
			*failed = 1;
		});
		assert_ok!(mock_candidate(
			REFERRED_CANDIDATE,
			Some(PERSON),
			Some((InkChoice::DesignedElective(0, 1), Allocation::Full)),
			Some(referred_ticket),
			false
		));
		Candidates::<Test>::mutate(REFERRED_CANDIDATE, |maybe_state| {
			let Some(Candidate::Selected { failed, .. }) = maybe_state.as_mut() else {
				unreachable!("Candidate created in this state.")
			};
			*failed = 1;
		});

		// Cannot say
		let judgement = Judgement::Truth(False);
		advance_by(1);
		let initial_alloc_count = AllocationCount::<Test>::get();
		assert_ok!(
			PoI::judged(RuntimeOrigin::root(), regular_ticket, regular_context.clone(), judgement),
			PostDispatchInfo {
				actual_weight: Some(<Test as Config>::WeightInfo::judged(0)),
				pays_fee: Pays::No
			}
		);
		System::assert_last_event(
			Event::JudgementProvided { account_id: CANDIDATE, judgement }.into(),
		);
		assert_eq!(AllocationCount::<Test>::get(), initial_alloc_count - 1);
		assert_eq!(People::<Test>::get(PERSON).unwrap().bad_referrals, 0);

		let alloc_count = AllocationCount::<Test>::get();
		assert_ok!(
			PoI::judged(
				RuntimeOrigin::root(),
				referred_ticket,
				referred_context.clone(),
				judgement
			),
			PostDispatchInfo {
				actual_weight: Some(<Test as Config>::WeightInfo::judged(0)),
				pays_fee: Pays::No
			}
		);
		System::assert_last_event(
			Event::JudgementProvided { account_id: REFERRED_CANDIDATE, judgement }.into(),
		);
		assert_eq!(AllocationCount::<Test>::get(), alloc_count - 1);
		assert_eq!(People::<Test>::get(PERSON).unwrap().bad_referrals, 1);
	});
}

#[test]
fn register_works() {
	TestExt::new().execute_with(|| {
		const CANDIDATE: AccountId = 9;
		const REWARD: u64 = 1;
		let (pk, sk) = mock_key(1234);
		let proof = {
			let mut m = b"pop register using".to_vec();
			m.extend_from_slice(&CANDIDATE.encode()[..]);
			Mock::sign(&sk, &m[..]).unwrap()
		};
		let wrong_proof = {
			let mut m = b"pop register using".to_vec();
			m.extend_from_slice(&pk.encode()[..]);
			Mock::sign(&sk, &m[..]).unwrap()
		};

		advance_by(1);
		assert_ok!(append_reimbursement_values(4, 1, 10));
		// Must be signed origin
		assert_noop!(
			PoI::register_non_referred(RuntimeOrigin::root(), pk, REWARD, proof),
			BadOrigin
		);
		assert_noop!(
			PoI::register_non_referred(RuntimeOrigin::none(), pk, REWARD, proof),
			BadOrigin
		);

		assert_ok!(mock_designs());
		let design = InkChoice::DesignedElective(0, 0);
		assert_ok!(mock_candidate(CANDIDATE, None, Some((design, Allocation::Full)), None, true));
		advance_by(1);
		let Candidate::Proven { reserved, design, was_referred: false, was_invited: false } =
			Candidates::<Test>::get(CANDIDATE).unwrap()
		else {
			unreachable!("Candidate was created in this state.")
		};

		// Wrong call
		assert_noop!(
			PoI::register_referred(RuntimeOrigin::signed(CANDIDATE), pk, REWARD, proof),
			Error::<Test>::NotReferredCandidate
		);
		// Wrong proof
		assert_noop!(
			PoI::register_non_referred(RuntimeOrigin::signed(CANDIDATE), pk, REWARD, wrong_proof),
			Error::<Test>::InvalidProofOfOwnership
		);
		// Correct call
		System::reset_events();
		assert_ok!(
			PoI::register_non_referred(RuntimeOrigin::signed(CANDIDATE), pk, REWARD, proof,)
		);
		System::assert_has_event(
			Event::PersonRegistered { account_id: CANDIDATE, personal_id: reserved }.into(),
		);
		assert!(!Candidates::<Test>::contains_key(CANDIDATE));
		assert!(MockReserved::<Test>::get(reserved).is_none());
		assert!(matches!(
			People::<Test>::try_get(reserved).unwrap(),
			Person { design: actual_design, .. }
			if actual_design == Some(design)
		));
		assert_reward_registered(vec![REWARD]);
		assert_reward_registered(vec![REWARD]);
	});
}

#[test]
fn register_with_referrer_works() {
	TestExt::new().execute_with(|| {
		const CANDIDATE: AccountId = 9;
		const REWARD: u64 = 1;
		const REFERRER: AccountId = 4;
		const REFERRER_REWARD: u64 = 2;
		let (pk, sk) = mock_key(1234);
		let proof = {
			let mut m = b"pop register using".to_vec();
			m.extend_from_slice(&CANDIDATE.encode()[..]);
			Mock::sign(&sk, &m[..]).unwrap()
		};
		let wrong_proof = {
			let mut m = b"pop register using".to_vec();
			m.extend_from_slice(&REFERRER.encode()[..]);
			Mock::sign(&sk, &m[..]).unwrap()
		};

		advance_by(1);
		// Must be signed origin
		assert_noop!(PoI::register_referred(RuntimeOrigin::root(), pk, REWARD, proof), BadOrigin);
		assert_noop!(PoI::register_referred(RuntimeOrigin::none(), pk, REWARD, proof), BadOrigin);

		assert_ok!(mock_designs());
		assert_ok!(append_reimbursement_values(4, 1, 10));
		assert_ok!(mock_person(REFERRER, None));
		let design = InkChoice::DesignedElective(0, 0);
		assert_ok!(mock_candidate(
			CANDIDATE,
			Some(REFERRER),
			Some((design, Allocation::Full)),
			None,
			true
		));
		advance_by(1);
		let Candidate::Proven { reserved, design, was_referred: true, was_invited: false } =
			Candidates::<Test>::get(CANDIDATE).unwrap()
		else {
			unreachable!("Candidate was created in this state.")
		};

		assert_ok!(PoI::register_successful_referral_reward(
			RuntimeOrigin::signed(REFERRER),
			REFERRER_REWARD,
		));
		System::assert_has_event(Event::ReferralVoucherRegistered { referrer: REFERRER }.into());

		// Wrong call
		assert_noop!(
			PoI::register_non_referred(RuntimeOrigin::signed(CANDIDATE), pk, REWARD, proof),
			Error::<Test>::ReferredCandidate
		);
		// Wrong proof
		assert_noop!(
			PoI::register_referred(RuntimeOrigin::signed(CANDIDATE), pk, REWARD, wrong_proof),
			Error::<Test>::InvalidProofOfOwnership
		);
		// Correct call
		System::reset_events();
		assert_ok!(PoI::register_referred(RuntimeOrigin::signed(CANDIDATE), pk, REWARD, proof,));
		System::assert_has_event(
			Event::PersonRegistered { account_id: CANDIDATE, personal_id: reserved }.into(),
		);
		assert!(!Candidates::<Test>::contains_key(CANDIDATE));
		assert!(matches!(
			People::<Test>::try_get(reserved).unwrap(),
			Person { design: actual_design, .. }
			if actual_design == Some(design)
		));
		assert_reward_registered(vec![REFERRER_REWARD]);
		assert_reward_registered(vec![REWARD]);
	});
}

#[test]
fn reroll_works() {
	TestExt::new().reroll_timeout(10).execute_with(|| {
		const CANDIDATE: AccountId = 9;

		advance_by(1);
		// Must be signed origin
		assert_noop!(PoI::reroll(RuntimeOrigin::root()), BadOrigin);
		assert_noop!(PoI::reroll(RuntimeOrigin::none()), BadOrigin);

		// Cannot reroll a non-existent candidate
		assert!(Candidates::<Test>::get(CANDIDATE).is_none());
		assert_noop!(PoI::reroll(RuntimeOrigin::signed(CANDIDATE)), Error::<Test>::NotApplied);

		assert_ok!(mock_candidate(CANDIDATE, None, None, None, false));
		advance_by(1);
		// Get original struct values before reroll
		let Candidate::Applied {
			cred: original_cred,
			entropy: original_entropy,
			entropy_since: original_entropy_since,
		} = Candidates::<Test>::get(CANDIDATE).unwrap()
		else {
			unreachable!("Candidate was created in this state.")
		};

		// Ensure that a candidate can reroll after the timeout
		advance_by(10); // To reroll timeout
		assert_ok!(PoI::reroll(RuntimeOrigin::signed(CANDIDATE)));
		System::assert_last_event(Event::Rerolled { account_id: CANDIDATE }.into());
		assert!(matches!(
			Candidates::<Test>::get(CANDIDATE),
			Some(Candidate::Applied { cred, entropy, entropy_since })
				if cred == original_cred
					&& entropy == original_entropy
					&& entropy_since == original_entropy_since + 10
		));
	});
}

#[test]
fn commit_works() {
	TestExt::new().maximum(3).fasttrack_count(1).execute_with(|| {
		const RESERVED: AccountId = 0;
		const CANDIDATE: AccountId = 9;
		const INITIAL_CANDIDATE: AccountId = 8;
		const ID_CANDIDATE: AccountId = 7;
		const OVERALLOCATED_CANDIDATE: AccountId = 6;
		const DUPLICATED_DESIGN_CANDIDATE: AccountId = 5;

		advance_by(1);
		assert_ok!(mock_designs());
		let design = InkChoice::DesignedElective(0, 0);

		// Committing is limited to signed origins
		assert_noop!(PoI::commit(RuntimeOrigin::none(), design.clone(), None), BadOrigin);
		assert_noop!(PoI::commit(RuntimeOrigin::root(), design.clone(), None), BadOrigin);

		// Non-existent candidates cannot commit
		assert_noop!(
			PoI::commit(RuntimeOrigin::signed(CANDIDATE), design.clone(), None),
			Error::<Test>::NotApplied
		);

		// Existing candidate can commit full while fast_tracks exist
		assert_ok!(mock_candidate(CANDIDATE, None, None, None, false));
		let reserved_id = MockNextId::<Test>::get();
		advance_by(1);
		let alloc_count = AllocationCount::<Test>::get();
		assert_ok!(PoI::commit(
			RuntimeOrigin::signed(CANDIDATE),
			InkChoice::DesignedElective(0, 0),
			None
		));
		System::assert_last_event(
			Event::DesignCommitted { account_id: CANDIDATE, reserved_id }.into(),
		);
		assert!(AllocationCount::<Test>::get() == alloc_count + 1);
		assert!(matches!(
			Candidates::<Test>::get(CANDIDATE),
			Some(Candidate::Selected { allocation, .. })
			if allocation == Allocation::Full
		));
		// Assert that correct storage has been allocated when it's implemented - TODO
		// e.g.:
		// assert_eq!(DataStore::<Test>::check_storage(CANDIDATE),
		// 	(config.full_alloc_len, config.full_alloc_count)
		// );

		// Existing candidate can commit initial now that the fast_tracks are used up
		assert_ok!(mock_candidate(INITIAL_CANDIDATE, None, None, None, false));
		let reserved_id = MockNextId::<Test>::get();
		advance_by(1);
		let alloc_count = AllocationCount::<Test>::get();
		assert_ok!(PoI::commit(
			RuntimeOrigin::signed(INITIAL_CANDIDATE),
			InkChoice::DesignedElective(0, 1),
			None
		));
		System::assert_last_event(
			Event::DesignCommitted { account_id: INITIAL_CANDIDATE, reserved_id }.into(),
		);
		assert!(AllocationCount::<Test>::get() == alloc_count + 1);
		assert!(matches!(
			Candidates::<Test>::get(INITIAL_CANDIDATE),
			Some(Candidate::Selected { allocation, .. })
			if allocation == Allocation::Initial
		));
		// Assert that correct storage has been allocated when it's implemented - TODO
		// e.g.:
		// assert_eq!(DataStore::<Test>::check_storage(INITIAL_CANDIDATE),
		// 	(config.init_alloc_len, config.init_alloc_count)
		// );

		// Existing candidate cannot commit with invalid ID
		assert_ok!(mock_candidate(ID_CANDIDATE, None, None, None, false));
		let reserved_id = MockNextId::<Test>::get() + 1; // will be invalid
		advance_by(1);
		assert_noop!(
			PoI::commit(
				RuntimeOrigin::signed(ID_CANDIDATE),
				InkChoice::DesignedElective(0, 0),
				Some(reserved_id)
			),
			DispatchError::Other("Invalid id reservation")
		);

		// Existing candidate cannot commit with reserved ID
		MockReserved::<Test>::insert(RESERVED, ());
		advance_by(1);
		assert_noop!(
			PoI::commit(
				RuntimeOrigin::signed(ID_CANDIDATE),
				InkChoice::DesignedElective(0, 2),
				Some(RESERVED)
			),
			DispatchError::Other("Invalid id reservation")
		);

		// Existing candidate can commit full while fast_tracks exist
		assert_ok!(mock_candidate(DUPLICATED_DESIGN_CANDIDATE, None, None, None, false));
		advance_by(1);
		assert_noop!(
			PoI::commit(
				RuntimeOrigin::signed(DUPLICATED_DESIGN_CANDIDATE),
				InkChoice::DesignedElective(0, 0),
				None
			),
			Error::<Test>::DesignTaken
		);

		// Existing candidate can commit with specific ID
		let reserved_id = MockNextId::<Test>::get();
		advance_by(1);
		let alloc_count = AllocationCount::<Test>::get();
		assert_ok!(PoI::commit(
			RuntimeOrigin::signed(ID_CANDIDATE),
			InkChoice::DesignedElective(0, 2),
			None
		));
		System::assert_last_event(
			Event::DesignCommitted { account_id: ID_CANDIDATE, reserved_id }.into(),
		);
		assert!(AllocationCount::<Test>::get() == alloc_count + 1);
		assert!(matches!(
			Candidates::<Test>::get(ID_CANDIDATE),
			Some(Candidate::Selected { allocation, .. })
			if allocation == Allocation::Initial
		));
		// Assert that correct storage has been allocated when it's implemented - TODO
		// e.g.:
		// assert_eq!(DataStore::<Test>::check_storage(CANDIDATE),
		// 	(config.full_alloc_len, config.full_alloc_count)
		// );

		// Ensure the max allocation config is respected
		assert_ok!(mock_candidate(OVERALLOCATED_CANDIDATE, None, None, None, false));
		advance_by(1);
		assert_noop!(
			PoI::commit(
				RuntimeOrigin::signed(OVERALLOCATED_CANDIDATE),
				InkChoice::DesignedElective(0, 3),
				None
			),
			Error::<Test>::Busy
		);
	});
}

#[test]
fn allocate_full_works() {
	TestExt::new().execute_with(|| {
		const CANDIDATE: AccountId = 9;
		const INIT_CANDIDATE: AccountId = 8;

		assert_ok!(mock_designs());
		advance_by(1);
		// Allocations are limited to signed origins
		assert_noop!(PoI::allocate_full(RuntimeOrigin::none()), BadOrigin);
		assert_noop!(PoI::allocate_full(RuntimeOrigin::root()), BadOrigin);

		// Candidate must have applied
		assert_noop!(
			PoI::allocate_full(RuntimeOrigin::signed(CANDIDATE)),
			Error::<Test>::NotApplied
		);

		// Candidate must have committed
		assert_ok!(mock_candidate(CANDIDATE, None, None, None, false));
		assert_noop!(
			PoI::allocate_full(RuntimeOrigin::signed(CANDIDATE)),
			Error::<Test>::BadContext
		);

		// Ensure candidate still in Initial state cannot allocate full
		assert_ok!(mock_candidate(
			INIT_CANDIDATE,
			None,
			Some((InkChoice::DesignedElective(0, 0), Allocation::Initial)),
			None,
			false
		));
		assert_noop!(
			PoI::allocate_full(RuntimeOrigin::signed(INIT_CANDIDATE)),
			Error::<Test>::Improbable
		);

		// Ensure a candidate who has committed can allocate full
		assert_ok!(mock_candidate(
			CANDIDATE,
			None,
			Some((InkChoice::DesignedElective(0, 1), Allocation::InitDone)),
			None,
			false
		));

		advance_by(1);
		let alloc_count = AllocationCount::<Test>::get();
		assert_ok!(PoI::allocate_full(RuntimeOrigin::signed(CANDIDATE)));
		System::assert_last_event(Event::FullyAllocated { account_id: CANDIDATE }.into());
		assert!(matches!(
			Candidates::<Test>::get(CANDIDATE),
			Some(Candidate::Selected { allocation, .. })
			if allocation == Allocation::Full
		));
		assert!(alloc_count == AllocationCount::<Test>::get());
	});
}

#[test]
fn timeout_works() {
	TestExt::new().timeout(10).execute_with(|| {
		const CANDIDATE: AccountId = 10;
		const APPLIED_CANDIDATE: AccountId = 9;
		const REFERRED_CANDIDATE: AccountId = 8;
		const REFERRER: AccountId = 0;

		assert_ok!(mock_designs());
		advance_by(1);
		// Timeouts are limited to signed origins
		assert_noop!(PoI::timeout(RuntimeOrigin::none(), CANDIDATE), BadOrigin);
		assert_noop!(PoI::timeout(RuntimeOrigin::root(), CANDIDATE), BadOrigin);

		// Cannot timeout a non-existent candidate
		assert_noop!(PoI::timeout(RuntimeOrigin::signed(CANDIDATE), CANDIDATE), Error::<Test>::NotApplied);

		// Not possible to flakeout a candidate who has not yet committed
		assert_ok!(mock_candidate(APPLIED_CANDIDATE, None, None, None, false));
		advance_by(11); // Past timeout
		assert_noop!(
			PoI::timeout(RuntimeOrigin::signed(APPLIED_CANDIDATE), APPLIED_CANDIDATE),
			Error::<Test>::BadContext
		);

		// Ensure applied candidate can be timed out
		assert_ok!(mock_designs());
		assert_ok!(mock_candidate(
			CANDIDATE,
			None,
			Some((InkChoice::DesignedElective(0, 0), Allocation::Full)),
			None,
			false
		));
		assert_noop!(PoI::timeout(RuntimeOrigin::signed(CANDIDATE), CANDIDATE), Error::<Test>::TooEarly);
		advance_by(11); // Past timeout
		assert_ok!(PoI::timeout(RuntimeOrigin::signed(CANDIDATE), CANDIDATE));
		System::assert_last_event(Event::TimedOut { account_id: CANDIDATE }.into());
		assert_eq!(Candidates::<Test>::get(CANDIDATE), None);

		// Check ticket was burnt (needs consideration mock) - TODO

		// Ensure referred applied candidate can be timed out
		advance_by(1);
		assert_ok!(mock_person(REFERRER, None));
		assert_ok!(mock_candidate(
			REFERRED_CANDIDATE,
			Some(REFERRER),
			Some((InkChoice::DesignedElective(0, 0), Allocation::Full)),
			None,
			false
		));
		assert_noop!(
			PoI::timeout(RuntimeOrigin::signed(REFERRED_CANDIDATE), REFERRED_CANDIDATE),
			Error::<Test>::TooEarly
		);
		advance_by(11); // Past timeout
		assert_ok!(PoI::timeout(RuntimeOrigin::signed(REFERRED_CANDIDATE), REFERRED_CANDIDATE));
		System::assert_last_event(Event::TimedOut { account_id: REFERRED_CANDIDATE }.into());
		assert_eq!(Candidates::<Test>::get(REFERRED_CANDIDATE), None);
		assert!(matches!(
			People::<Test>::get(REFERRER).unwrap(), Person { active_referrals, successful_referrals, bad_referrals, referrals, .. }
			if active_referrals.is_empty() && successful_referrals == 0 && bad_referrals == 1 && referrals == 1
		));
	});
}

#[test]
fn flakeout_works() {
	TestExt::new().execute_with(|| {
		const REFERRER: AccountId = 0;
		const CANDIDATE: AccountId = 9;
		const COMMITTED_CANDIDATE: AccountId = 8;
		const REFERRED_CANDIDATE: AccountId = 7;

		assert_ok!(mock_designs());
		advance_by(1);
		// Flakeouts are limited to signed origins
		assert_noop!(PoI::flakeout(RuntimeOrigin::none()), BadOrigin);
		assert_noop!(PoI::flakeout(RuntimeOrigin::root()), BadOrigin);

		// Not possible to flakeout a non-existent candidate
		assert_noop!(PoI::flakeout(RuntimeOrigin::signed(CANDIDATE)), Error::<Test>::NotApplied);

		// Not possible to flakeout a candidate who has already committed
		assert_ok!(mock_candidate(
			COMMITTED_CANDIDATE,
			None,
			Some((InkChoice::DesignedElective(0, 0), Allocation::Full)),
			None,
			false
		));
		assert_noop!(
			PoI::flakeout(RuntimeOrigin::signed(COMMITTED_CANDIDATE)),
			Error::<Test>::BadContext
		);

		// Path for a candidate who applies
		assert_ok!(mock_candidate(CANDIDATE, None, None, None, false));
		advance_by(1);
		assert_ok!(PoI::flakeout(RuntimeOrigin::signed(CANDIDATE)));
		System::assert_last_event(Event::FlakedOut { account_id: CANDIDATE }.into());
		assert_eq!(Candidates::<Test>::get(CANDIDATE), None);
		// Check ticket was dropped (needs consideration mock) - TODO

		// Path for a referred candidate
		advance_by(1);
		assert_ok!(mock_person(REFERRER, None));
		assert_ok!(mock_candidate(REFERRED_CANDIDATE, Some(REFERRER), None, None, false));
		advance_by(1);
		assert_ok!(PoI::flakeout(RuntimeOrigin::signed(REFERRED_CANDIDATE)));
		System::assert_last_event(Event::FlakedOut { account_id: REFERRED_CANDIDATE }.into());
		assert_eq!(Candidates::<Test>::get(REFERRED_CANDIDATE), None);
		assert!(matches!(
			People::<Test>::get(REFERRER).unwrap(), Person { active_referrals, successful_referrals, bad_referrals, referrals, .. }
			if active_referrals.is_empty() && successful_referrals == 0 && bad_referrals == 0 && referrals == 1
		));
	});
}

#[test]
fn set_referral_ticket_doesnt_work_at_referral_limit() {
	TestExt::new().execute_with(|| {
		const REFERRER: AccountId = 0;
		const CANDIDATE_BASE: AccountId = 1337;
		const REWARD: u64 = 1;
		assert_ok!(append_reimbursement_values(4, 1, 10));
		assert_ok!(mock_person(REFERRER, Some(InkSpec::DesignedElective(0, 0))));

		let referral_process = |index: u64, expect_success: bool| {
			let candidate: AccountId = CANDIDATE_BASE + index;
			let ticket: AccountId = 999 + index;
			let signature = TestSignature(ticket, candidate.encode());

			advance_by(1);

			if expect_success {
				assert_ok!(PoI::set_referral_ticket(RuntimeOrigin::signed(REFERRER), ticket,));
				assert_eq!(
					ReferralTickets::<Test>::get(REFERRER).unwrap(),
					vec![ReferralTicket { ticket }]
				);

				let call = PoICall::apply_with_signature {
					referrer: REFERRER,
					signature: signature.clone(),
					ticket,
				};
				assert_ok!(exec_tx(
					candidate,
					0,
					call,
					Some(AsProofOfInkParticipantInfo::AsApplyWithSig(0))
				));
				assert!(ReferralTickets::<Test>::get(REFERRER).is_none());
			} else {
				assert_noop!(
					PoI::set_referral_ticket(RuntimeOrigin::signed(REFERRER), ticket),
					Error::<Test>::NoMoreReferrals
				);
			}
		};

		// Referring users works up until the limit...
		let max_referrals: u32 = <Test as crate::Config>::MaxActiveReferrals::get();
		for index in 0..max_referrals as u64 {
			referral_process(index, true);
		}

		// ...but any more will fail.
		referral_process(max_referrals as u64, false);

		// Now we will show that a candidate can be accepted, and the user can refer again.

		/* -- Start Candidate Acceptance -- */
		let candidate = CANDIDATE_BASE;
		assert!(People::<Test>::get(REFERRER).unwrap().active_referrals.contains(&candidate));
		assert_ok!(mock_designs());
		let evidence = mock_evidence();
		let judgement = Judgement::Truth(True);
		assert_ok!(
			PoI::commit(RuntimeOrigin::signed(candidate), InkChoice::DesignedElective(0, 0), None),
			Pays::No.into()
		);
		assert_ok!(
			PoI::submit_evidence(RuntimeOrigin::signed(candidate), evidence),
			Pays::No.into()
		);
		// Get the oracle ticket coming from `judging`.
		let status = Candidates::<Test>::get(candidate).unwrap();
		let Candidate::Selected { judging, .. } = status else {
			panic!("unexpected candidate status")
		};
		let context = candidate.encode().try_into().unwrap();

		let initial_referral_count =
			People::<Test>::get(REFERRER).unwrap().active_referrals.len() as u32;
		assert_ok!(
			PoI::judged(RuntimeOrigin::root(), judging.unwrap(), context, judgement),
			PostDispatchInfo {
				actual_weight: Some(<Test as Config>::WeightInfo::judged(
					initial_referral_count.saturating_sub(1)
				)),
				pays_fee: Pays::No
			}
		);
		assert_ok!(PoI::register_successful_referral_reward(
			RuntimeOrigin::signed(REFERRER),
			REWARD
		));
		// Final checks everything went as expected.
		assert!(!People::<Test>::get(REFERRER).unwrap().active_referrals.contains(&candidate));
		/* -- End Candidate Acceptance -- */

		// Now one more passes!
		referral_process(max_referrals as u64, true);
	})
}

#[test]
fn set_referral_ticket_respects_global_max_limit() {
	TestExt::new().execute_with(|| {
		const REFERRER: AccountId = 0;
		assert_ok!(mock_person(REFERRER, Some(InkSpec::DesignedElective(0, 0))));
		let max_active_referrals = <Test as Config>::MaxActiveReferrals::get();

		People::<Test>::mutate(REFERRER, |maybe_record| {
			let record = maybe_record.as_mut().unwrap();
			record.allowed_referral_tickets = max_active_referrals;
			for index in 0u64..(max_active_referrals / 2).into() {
				let ticket: AccountId = 999 + index;
				record.active_referrals.try_push(ticket).unwrap();
			}
			if max_active_referrals % 2 == 1 {
				let ticket: AccountId = 999u64 + (max_active_referrals as u64 / 2);
				record.active_referrals.try_push(ticket).unwrap();
			}
		});
		let mut tickets = vec![];
		for index in 0u64..(max_active_referrals / 2).into() {
			let ticket: AccountId = 9999 + index;
			tickets.push(ReferralTicket { ticket });
		}
		let tickets: BoundedVec<ReferralTicket<AccountId>, <Test as Config>::MaxActiveReferrals> =
			tickets.try_into().unwrap();
		ReferralTickets::<Test>::insert(REFERRER, tickets);

		let ticket: AccountId = 99999;
		assert_noop!(
			PoI::set_referral_ticket(RuntimeOrigin::signed(REFERRER), ticket),
			Error::<Test>::NoMoreReferrals
		);
	})
}

#[test]
fn active_referrals_are_retained_correctly() {
	TestExt::new().execute_with(|| {
		const REFERRER: AccountId = 0;
		const BASE_CANDIDATE: AccountId = 1337;
		assert_ok!(mock_person(REFERRER, Some(InkSpec::DesignedElective(0, 0))));

		let referral_process = |index: u64| {
			let candidate: AccountId = BASE_CANDIDATE + index;
			let ticket: u64 = 999 + index;
			let signature = TestSignature(ticket, candidate.encode());

			advance_by(1);
			assert_ok!(PoI::set_referral_ticket(RuntimeOrigin::signed(REFERRER), ticket));

			let call = PoICall::apply_with_signature {
				referrer: REFERRER,
				signature: signature.clone(),
				ticket,
			};
			assert_ok!(exec_tx(
				candidate,
				0,
				call,
				Some(AsProofOfInkParticipantInfo::AsApplyWithSig(0))
			));
		};

		// Referring users works up until the limit...
		let max_referrals: u32 = <Test as crate::Config>::MaxActiveReferrals::get();
		for index in 0..max_referrals as u64 {
			referral_process(index);
		}

		// referral end.
		let referral_ends =
			[PoI::referral_ok, PoI::referral_flaked, PoI::referral_bad, PoI::referral_contemptuous];
		// referral success.
		let referral_outcomes = [true, false, false, false];
		// For the logic below to work, we need `max_referrals` to be greater than or equal the
		// number of referral end paths.
		assert!(max_referrals >= referral_ends.len() as u32);

		// We should be able to see all possible referral end paths result in removing the candidate
		// from the `active_referrals` list.
		for (index, (ref_function, success)) in
			referral_ends.iter().zip(referral_outcomes.iter()).enumerate()
		{
			let candidate = BASE_CANDIDATE + index as u64;
			assert!(People::<Test>::get(REFERRER).unwrap().active_referrals.contains(&candidate));
			let pending_rewards = People::<Test>::get(REFERRER).unwrap().pending_referral_rewards;
			ref_function(REFERRER, &candidate);
			assert!(!People::<Test>::get(REFERRER).unwrap().active_referrals.contains(&candidate));
			assert_eq!(
				People::<Test>::get(REFERRER).unwrap().pending_referral_rewards,
				pending_rewards + *success as u32
			);
		}
	})
}

#[test]
fn referral_ticket_works() {
	TestExt::new().execute_with(|| {
		const REFERRER: AccountId = 0;
		const BANNED_REFERRER: AccountId = 1;
		const NON_PERSON: AccountId = AccountId::MAX;

		let first_ticket: u64 = 999;

		advance_by(1);
		// Must be signed origin
		assert_noop!(PoI::set_referral_ticket(RuntimeOrigin::root(), first_ticket), BadOrigin);
		assert_noop!(PoI::set_referral_ticket(RuntimeOrigin::none(), first_ticket), BadOrigin);

		// Non-person cannot set a referral ticket
		assert!(!People::<Test>::contains_key(NON_PERSON));
		assert_noop!(
			PoI::set_referral_ticket(RuntimeOrigin::signed(NON_PERSON), first_ticket,),
			BadOrigin
		);

		// Banned person cannot set a referral ticket
		assert_ok!(mock_person(BANNED_REFERRER, None));
		People::<Test>::mutate_extant(BANNED_REFERRER, |person| {
			person.banned = true;
		});
		assert_noop!(
			PoI::set_referral_ticket(RuntimeOrigin::signed(BANNED_REFERRER), first_ticket,),
			Error::<Test>::Banned
		);

		// Person can refer a candidate
		assert_ok!(mock_person(REFERRER, Some(InkSpec::DesignedElective(0, 0))));
		assert!(People::<Test>::contains_key(REFERRER));
		advance_by(1);
		assert_ok!(PoI::set_referral_ticket(RuntimeOrigin::signed(REFERRER), first_ticket,));
		System::assert_last_event(
			Event::TicketReferred { referrer: REFERRER, ticket: first_ticket }.into(),
		);
		assert_eq!(
			ReferralTickets::<Test>::get(REFERRER).unwrap(),
			vec![ReferralTicket { ticket: first_ticket }],
		);
		assert!(matches!(
			People::<Test>::get(REFERRER).unwrap(),
			Person { active_referrals, bad_referrals, successful_referrals, referrals, .. }
			if active_referrals.is_empty() && bad_referrals == 0 && successful_referrals == 0 && referrals == 0
		));

		let second_ticket: u64 = 777;

		// Setting the referral ticket again fails due to tickets' limit reached
		assert_noop!(
			PoI::set_referral_ticket(RuntimeOrigin::signed(REFERRER), second_ticket,),
			Error::<Test>::NoMoreReferrals
		);
		assert_eq!(
			ReferralTickets::<Test>::get(REFERRER).unwrap(),
			vec![ReferralTicket { ticket: first_ticket }],
		);
		assert!(matches!(
			People::<Test>::get(REFERRER).unwrap(),
			Person { active_referrals, bad_referrals, successful_referrals, referrals, .. }
			if active_referrals.is_empty() && bad_referrals == 0 && successful_referrals == 0 && referrals == 0
		));

		// Cancel the ticket we just set
		assert_ok!(PoI::cancel_referral_ticket(RuntimeOrigin::signed(REFERRER), first_ticket));
		System::assert_last_event(
			Event::TicketCancelled { referrer: REFERRER, ticket: first_ticket }.into(),
		);
		assert!(ReferralTickets::<Test>::contains_key(REFERRER));
		assert_eq!(ReferralTickets::<Test>::get(REFERRER).unwrap().len(), 0);

		// Can't cancel a non-existent ticket
		assert_noop!(
			PoI::cancel_referral_ticket(RuntimeOrigin::signed(REFERRER), first_ticket),
			Error::<Test>::NoTicket
		);
		assert!(ReferralTickets::<Test>::contains_key(REFERRER));
		assert_eq!(ReferralTickets::<Test>::get(REFERRER).unwrap().len(), 0);

		// Banned persons can't cancel a ticket
		let tickets = BoundedVec::<
			ReferralTicket<<Test as Config>::Ticket>,
			<Test as Config>::MaxActiveReferrals,
		>::try_from(vec![ReferralTicket { ticket: first_ticket }])
		.unwrap();
		ReferralTickets::<Test>::insert(BANNED_REFERRER, tickets);
		assert_noop!(
			PoI::cancel_referral_ticket(RuntimeOrigin::signed(BANNED_REFERRER), first_ticket),
			Error::<Test>::Banned
		);
	});
}

#[test]
fn referral_ticket_with_value_change() {
	TestExt::new().execute_with(|| {
		const CANDIDATE_1: AccountId = 6;
		const CANDIDATE_2: AccountId = 7;
		const CANDIDATE_3: AccountId = 8;
		const CANDIDATE_4: AccountId = 9;
		const CANDIDATE_5: AccountId = 10;
		const REFERRER_1: AccountId = 0;
		const REFERRER_2: AccountId = 1;
		const REFERRER_3: AccountId = 2;
		const REFERRER_4: AccountId = 3;
		const REFERRER_5: AccountId = 4;
		const REWARD_C1: u64 = 1;
		const REWARD_R1: u64 = 2;
		const REWARD_C2: u64 = 3;
		const REWARD_R2: u64 = 4;
		const REWARD_C3: u64 = 5;
		const REWARD_R3: u64 = 6;
		const REWARD_C4: u64 = 7;
		const REWARD_R4: u64 = 8;
		const REWARD_C5: u64 = 9;
		const REWARD_R5: u64 = 10;
		let (pk, sk) = mock_key(1234);
		let proof_1 = {
			let mut m = b"pop register using".to_vec();
			m.extend_from_slice(&CANDIDATE_1.encode()[..]);
			Mock::sign(&sk, &m[..]).unwrap()
		};
		let proof_2 = {
			let mut m = b"pop register using".to_vec();
			m.extend_from_slice(&CANDIDATE_2.encode()[..]);
			Mock::sign(&sk, &m[..]).unwrap()
		};
		let proof_3 = {
			let mut m = b"pop register using".to_vec();
			m.extend_from_slice(&CANDIDATE_3.encode()[..]);
			Mock::sign(&sk, &m[..]).unwrap()
		};
		let proof_4 = {
			let mut m = b"pop register using".to_vec();
			m.extend_from_slice(&CANDIDATE_4.encode()[..]);
			Mock::sign(&sk, &m[..]).unwrap()
		};
		let proof_5 = {
			let mut m = b"pop register using".to_vec();
			m.extend_from_slice(&CANDIDATE_5.encode()[..]);
			Mock::sign(&sk, &m[..]).unwrap()
		};

		advance_by(1);

		assert_ok!(mock_designs());
		let design_1 = InkChoice::DesignedElective(0, 6);
		let design_2 = InkChoice::DesignedElective(0, 7);
		let design_3 = InkChoice::DesignedElective(0, 8);
		let design_4 = InkChoice::DesignedElective(0, 9);
		let design_5 = InkChoice::DesignedElective(0, 10);
		assert_ok!(mock_person(REFERRER_1, Some(InkSpec::DesignedElective(0, 0))));
		assert_ok!(mock_person(REFERRER_2, Some(InkSpec::DesignedElective(0, 1))));
		assert_ok!(mock_person(REFERRER_3, Some(InkSpec::DesignedElective(0, 2))));
		assert_ok!(mock_person(REFERRER_4, Some(InkSpec::DesignedElective(0, 3))));
		assert_ok!(mock_person(REFERRER_5, Some(InkSpec::DesignedElective(0, 4))));
		assert_ok!(mock_candidate(
			CANDIDATE_1,
			Some(REFERRER_1),
			Some((design_1, Allocation::Full)),
			None,
			true
		));
		assert_ok!(mock_candidate(
			CANDIDATE_2,
			Some(REFERRER_2),
			Some((design_2, Allocation::Full)),
			None,
			true
		));
		assert_ok!(mock_candidate(
			CANDIDATE_3,
			Some(REFERRER_3),
			Some((design_3, Allocation::Full)),
			None,
			true
		));
		assert_ok!(mock_candidate(
			CANDIDATE_4,
			Some(REFERRER_4),
			Some((design_4, Allocation::Full)),
			None,
			true
		));
		assert_ok!(mock_candidate(
			CANDIDATE_5,
			Some(REFERRER_5),
			Some((design_5, Allocation::Full)),
			None,
			true
		));
		advance_by(1);
		let Candidate::Proven { was_referred: true, was_invited: false, .. } =
			Candidates::<Test>::get(CANDIDATE_1).unwrap()
		else {
			unreachable!("Candidate was created in this state.")
		};
		let Candidate::Proven { was_referred: true, was_invited: false, .. } =
			Candidates::<Test>::get(CANDIDATE_2).unwrap()
		else {
			unreachable!("Candidate was created in this state.")
		};
		let Candidate::Proven { was_referred: true, was_invited: false, .. } =
			Candidates::<Test>::get(CANDIDATE_3).unwrap()
		else {
			unreachable!("Candidate was created in this state.")
		};
		let Candidate::Proven { was_referred: true, was_invited: false, .. } =
			Candidates::<Test>::get(CANDIDATE_4).unwrap()
		else {
			unreachable!("Candidate was created in this state.")
		};
		let Candidate::Proven { was_referred: true, was_invited: false, .. } =
			Candidates::<Test>::get(CANDIDATE_5).unwrap()
		else {
			unreachable!("Candidate was created in this state.")
		};

		// No values
		assert_ok!(PoI::register_referred(
			RuntimeOrigin::signed(CANDIDATE_1),
			pk,
			REWARD_C1,
			proof_1
		));
		assert_reward_registered(Vec::new());
		assert_ok!(PoI::register_successful_referral_reward(
			RuntimeOrigin::signed(REFERRER_1),
			REWARD_R1
		));
		assert_reward_registered(Vec::new());

		// Add reimbursement values
		assert_ok!(append_reimbursement_values(4, 2, 2));

		// Register the second rewards
		assert_ok!(PoI::register_referred(
			RuntimeOrigin::signed(CANDIDATE_2),
			pk,
			REWARD_C2,
			proof_2
		));
		assert_reward_registered(vec![REWARD_C2]);
		assert_reward_value(REWARD_C2, 4);
		assert_ok!(PoI::register_successful_referral_reward(
			RuntimeOrigin::signed(REFERRER_2),
			REWARD_R2
		));
		assert_reward_registered(vec![REWARD_R2]);
		assert_reward_value(REWARD_R2, 2);

		// Rewards have been consumed
		assert_eq!(ReferredReimbursementValues::<Test>::get().unwrap()[0], (4, 1));
		assert_eq!(ReferrerReimbursementValues::<Test>::get().unwrap()[0], (2, 1));

		// Add another reward value
		assert_ok!(append_reimbursement_values(2, 1, 5));
		// It should be before the old one in the list
		assert_eq!(&ReferredReimbursementValues::<Test>::get().unwrap()[..], &[(2, 5), (4, 1)][..]);
		assert_eq!(&ReferrerReimbursementValues::<Test>::get().unwrap()[..], &[(1, 5), (2, 1)][..]);

		// Register the third rewards
		assert_ok!(PoI::register_referred(
			RuntimeOrigin::signed(CANDIDATE_3),
			pk,
			REWARD_C3,
			proof_3
		));
		assert_reward_registered(vec![REWARD_C2, REWARD_C3]);
		assert_reward_value(REWARD_C3, 4);
		assert_ok!(PoI::register_successful_referral_reward(
			RuntimeOrigin::signed(REFERRER_3),
			REWARD_R3
		));
		assert_reward_registered(vec![REWARD_R2, REWARD_R3]);
		assert_reward_value(REWARD_R3, 2);
		// Initial values have been entirely consumed
		assert_eq!(ReferredReimbursementValues::<Test>::get().unwrap()[0], (2, 5));
		assert_eq!(ReferrerReimbursementValues::<Test>::get().unwrap()[0], (1, 5));
		// Register the fourth wave of rewards, the new values apply immediately.
		assert_ok!(PoI::register_referred(
			RuntimeOrigin::signed(CANDIDATE_4),
			pk,
			REWARD_C4,
			proof_4
		));
		assert_reward_registered(vec![REWARD_C2, REWARD_C3, REWARD_C4]);
		assert_reward_value(REWARD_C4, 2);
		assert_ok!(PoI::register_successful_referral_reward(
			RuntimeOrigin::signed(REFERRER_4),
			REWARD_R4
		));
		assert_reward_registered(vec![REWARD_R2, REWARD_R3, REWARD_R4]);
		assert_reward_value(REWARD_R4, 1);
		// New values have been consumed
		assert_eq!(ReferredReimbursementValues::<Test>::get().unwrap()[0], (2, 4));
		assert_eq!(ReferrerReimbursementValues::<Test>::get().unwrap()[0], (1, 4));

		// Register the fifth wave of rewards
		assert_ok!(PoI::register_referred(
			RuntimeOrigin::signed(CANDIDATE_5),
			pk,
			REWARD_C5,
			proof_5
		));
		assert_reward_registered(vec![REWARD_C2, REWARD_C3, REWARD_C4, REWARD_C5]);
		assert_reward_value(REWARD_C5, 2);
		assert_ok!(PoI::register_successful_referral_reward(
			RuntimeOrigin::signed(REFERRER_5),
			REWARD_R5
		));
		assert_reward_registered(vec![REWARD_R2, REWARD_R3, REWARD_R4, REWARD_R5]);
		assert_reward_value(REWARD_R5, 1);
		// New values have been consumed
		assert_eq!(ReferredReimbursementValues::<Test>::get().unwrap()[0], (2, 3));
		assert_eq!(ReferrerReimbursementValues::<Test>::get().unwrap()[0], (1, 3));
	});
}

#[test]
fn apply_with_signature_works() {
	TestExt::new().execute_with(|| {
		const REFERRER: AccountId = 0;
		const BANNED_REFERRER: AccountId = 9;
		const CANDIDATE: AccountId = 10;
		const EXISTING_CANDIDATE: AccountId = 11;

		let ticket: u64 = 999;
		let signature = TestSignature(ticket, CANDIDATE.encode());

		advance_by(1);
		// Ensure a candidate cannot apply without a signature from a person
		let call = PoICall::apply_with_signature {
			referrer: REFERRER,
			signature: signature.clone(),
			ticket,
		};
		assert_noop!(
			exec_tx(CANDIDATE, 0, call, Some(AsProofOfInkParticipantInfo::AsApplyWithSig(0))),
			InvalidTransaction::BadProof
		);

		// Applying with a signature is limited to signed origins
		assert_noop!(
			PoI::apply_with_signature(RuntimeOrigin::none(), REFERRER, signature.clone(), ticket),
			BadOrigin
		);
		assert_noop!(
			PoI::apply_with_signature(RuntimeOrigin::root(), REFERRER, signature.clone(), ticket),
			BadOrigin
		);
		assert_noop!(
			PoI::apply_with_signature(
				RuntimeOrigin::signed(CANDIDATE),
				REFERRER,
				signature.clone(),
				ticket,
			),
			BadOrigin
		);

		// Mock person
		assert_ok!(mock_person(BANNED_REFERRER, Some(InkSpec::DesignedElective(0, 10))));
		People::<Test>::mutate_extant(BANNED_REFERRER, |person| {
			person.banned = true;
		});

		// Ensure a candidate cannot apply with a signature from a banned referrer
		let call = PoICall::apply_with_signature {
			referrer: BANNED_REFERRER,
			signature: signature.clone(),
			ticket,
		};
		assert_noop!(
			exec_tx(CANDIDATE, 0, call, Some(AsProofOfInkParticipantInfo::AsApplyWithSig(0))),
			InvalidTransaction::BadProof
		);

		// Mock person
		assert_ok!(mock_person(REFERRER, Some(InkSpec::DesignedElective(0, 0))));
		assert_ok!(PoI::set_referral_ticket(RuntimeOrigin::signed(REFERRER), ticket));

		// Ensure a candidate cannot apply without a valid signature
		let invalid_signature = TestSignature(17, CANDIDATE.encode());
		let call = PoICall::apply_with_signature {
			referrer: REFERRER,
			signature: invalid_signature.clone(),
			ticket,
		};
		assert_noop!(
			exec_tx(CANDIDATE, 0, call, Some(AsProofOfInkParticipantInfo::AsApplyWithSig(0))),
			InvalidTransaction::BadProof
		);

		// Ensure an existing candidate cannot reapply with the ticket
		assert_ok!(mock_candidate(EXISTING_CANDIDATE, None, None, None, false));
		advance_by(1);
		let existing_candidate_signature = TestSignature(ticket, EXISTING_CANDIDATE.encode());
		let call = PoICall::apply_with_signature {
			referrer: REFERRER,
			signature: existing_candidate_signature.clone(),
			ticket,
		};
		assert_noop!(
			exec_tx(
				EXISTING_CANDIDATE,
				0,
				call,
				Some(AsProofOfInkParticipantInfo::AsApplyWithSig(0))
			),
			InvalidTransaction::BadSigner
		);

		// Ensure nonce is checked
		advance_by(1);
		let call = PoICall::apply_with_signature {
			referrer: REFERRER,
			signature: signature.clone(),
			ticket,
		};
		// this is not no-op, but rejected during validation so reverted.
		assert_eq!(
			exec_tx(CANDIDATE, 1, call, Some(AsProofOfInkParticipantInfo::AsApplyWithSig(1)))
				.unwrap_err(),
			InvalidTransaction::Future.into()
		);

		// Ensure a candidate can apply with the ticket
		advance_by(1);
		let call = PoICall::apply_with_signature {
			referrer: REFERRER,
			signature: signature.clone(),
			ticket,
		};
		assert_ok!(exec_tx(
			CANDIDATE,
			0,
			call,
			Some(AsProofOfInkParticipantInfo::AsApplyWithSig(0))
		));
		System::assert_last_event(
			Event::TicketApplied { account_id: CANDIDATE, referrer: REFERRER }.into(),
		);
		assert!(matches!(
			Candidates::<Test>::get(CANDIDATE), Some(Candidate::Applied { cred, .. })
			if cred == Credibility::Referred(REFERRER)
		));
	});
}

mod referral_tickets {
	use super::*;

	#[test]
	fn fails_when_one_candidate_attempts_to_use_two_different_tickets() {
		TestExt::new().execute_with(|| {
			let referrer: AccountId = 1;
			let candidate: AccountId = 10;
			let ticket_1: u64 = 1234;
			let ticket_2: u64 = 1235;
			let signature_ticket_1 =
				sp_runtime::testing::TestSignature(ticket_1, candidate.encode());
			let signature_ticket_2 =
				sp_runtime::testing::TestSignature(ticket_2, candidate.encode());

			assert_ok!(mock_designs());
			assert_ok!(mock_person(referrer, Some(InkSpec::DesignedElective(0, 0))));

			// Referrer sets 1st ticket
			assert_ok!(PoI::set_referral_ticket(RuntimeOrigin::signed(referrer), ticket_1));
			assert_eq!(
				ReferralTickets::<Test>::get(referrer).map(|rt| rt[0].ticket),
				Some(ticket_1)
			);

			// Artificially increase referrers allowance of referral tickets
			People::<Test>::mutate(referrer, |p| {
				p.as_mut().unwrap().allowed_referral_tickets.saturating_inc();
			});

			// Referrer sets 2nd ticket
			assert_ok!(PoI::set_referral_ticket(RuntimeOrigin::signed(referrer), ticket_2));
			assert_eq!(
				ReferralTickets::<Test>::get(referrer).map(|rt| rt[1].ticket),
				Some(ticket_2)
			);

			// Candidate uses the 1st ticket
			let apply_call = Call::apply_with_signature {
				referrer,
				signature: signature_ticket_1,
				ticket: ticket_1,
			};

			assert_ok!(exec_tx(
				candidate,
				0,
				apply_call,
				Some(AsProofOfInkParticipantInfo::AsApplyWithSig(0))
			));

			// The same candidate uses the 2nd ticket
			let apply_call = Call::apply_with_signature {
				referrer,
				signature: signature_ticket_2,
				ticket: ticket_2,
			};

			// The call fails
			assert_noop!(
				exec_tx(
					candidate,
					1,
					apply_call,
					Some(AsProofOfInkParticipantInfo::AsApplyWithSig(0))
				),
				InvalidTransaction::BadSigner
			);
		});
	}

	#[test]
	fn reusing_a_ticket_fails() {
		TestExt::new().execute_with(|| {
			let referrer: AccountId = 1;
			let candidate_1: AccountId = 10;
			let candidate_2: AccountId = 11;
			let ticket: u64 = 1234;
			let signature_ticket_candidate_1 =
				sp_runtime::testing::TestSignature(ticket, candidate_1.encode());
			let signature_ticket_candidate_2 =
				sp_runtime::testing::TestSignature(ticket, candidate_2.encode());

			assert_ok!(mock_designs());
			assert_ok!(mock_person(referrer, Some(InkSpec::DesignedElective(0, 0))));

			// Referrer sets the ticket
			assert_ok!(PoI::set_referral_ticket(RuntimeOrigin::signed(referrer), ticket));
			assert_eq!(ReferralTickets::<Test>::get(referrer).map(|rt| rt[0].ticket), Some(ticket));

			// Candidate 1 uses the ticket
			let apply_call = Call::apply_with_signature {
				referrer,
				signature: signature_ticket_candidate_1,
				ticket,
			};

			assert_ok!(exec_tx(
				candidate_1,
				0,
				apply_call,
				Some(AsProofOfInkParticipantInfo::AsApplyWithSig(0))
			));

			// Candidate 2 uses the same ticket
			let apply_call = Call::apply_with_signature {
				referrer,
				signature: signature_ticket_candidate_2,
				ticket,
			};

			// The call fails
			assert_noop!(
				exec_tx(
					candidate_2,
					1,
					apply_call,
					Some(AsProofOfInkParticipantInfo::AsApplyWithSig(0))
				),
				InvalidTransaction::BadProof
			);
		});
	}

	#[test]
	fn cannot_set_new_ticket_when_referrer_has_unregistered_rewards() {
		use crate::Event;
		use frame_support::{assert_noop, assert_ok};

		let referrer: AccountId = 1;
		let candidate_10: AccountId = 10;
		let candidate_11: AccountId = 11;
		let new_ticket: u64 = 1234;

		TestExt::new().execute_with(|| {
			advance_by(1);

			// Make sure the referrer is recognized and has an actual design.
			// `mock_person` and `mock_designs` are from your sample code.
			assert_ok!(mock_designs());
			assert_ok!(append_reimbursement_values(4, 1, 10));
			assert_ok!(mock_person(referrer, Some(InkSpec::DesignedElective(0, 0))));

			// Referrer sets a referral ticket:
			// This should succeed (the referrer has zero pending rewards).
			assert_ok!(PoI::set_referral_ticket(RuntimeOrigin::signed(referrer), new_ticket));
			assert_eq!(
				ReferralTickets::<Test>::get(referrer).map(|rt| rt[0].ticket),
				Some(new_ticket)
			);

			// Candidate uses the referral ticket and is proven:
			assert_ok!(mock_candidate(candidate_10, Some(referrer), None, None, true));
			assert_ok!(mock_candidate(candidate_11, Some(referrer), None, None, true));

			// Verify the referrer now has 2 pending reward
			assert_eq!(People::<Test>::get(referrer).unwrap().pending_referral_rewards, 2);

			// Try to set another referral ticket -> fails
			let second_ticket: u64 = 5555;
			assert_noop!(
				PoI::set_referral_ticket(RuntimeOrigin::signed(referrer), second_ticket),
				Error::<Test>::RewardToRegister
			);

			// Referrer calls `register_successful_referral_reward`
			let dummy_ref_reward_key = 99;
			assert_ok!(PoI::register_successful_referral_reward(
				RuntimeOrigin::signed(referrer),
				dummy_ref_reward_key,
			));

			// Verify the referrer now has 1 pending reward
			assert_eq!(People::<Test>::get(referrer).unwrap().pending_referral_rewards, 1);

			// Try to set another referral ticket -> fails
			let second_ticket: u64 = 5555;
			assert_noop!(
				PoI::set_referral_ticket(RuntimeOrigin::signed(referrer), second_ticket),
				Error::<Test>::RewardToRegister
			);

			// Referrer calls `register_successful_referral_reward`
			let dummy_ref_reward_key = 100;
			assert_ok!(PoI::register_successful_referral_reward(
				RuntimeOrigin::signed(referrer),
				dummy_ref_reward_key,
			));

			// Now the pending reward count is zero, so set_referral_ticket works again
			let updated_person_info = People::<Test>::get(referrer).unwrap();
			assert_eq!(updated_person_info.pending_referral_rewards, 0);

			assert_ok!(PoI::set_referral_ticket(RuntimeOrigin::signed(referrer), second_ticket));

			// Check ticket is correctly inserted
			let new_tickets =
				ReferralTickets::<Test>::get(referrer).expect("Ref ticket must be inserted");
			assert_eq!(new_tickets[0].ticket, new_ticket);
			assert_eq!(new_tickets[1].ticket, second_ticket);
			frame_system::Pallet::<Test>::assert_last_event(
				Event::TicketReferred { referrer, ticket: second_ticket }.into(),
			);
		});
	}

	#[test]
	fn ticket_cancellation() {
		TestExt::new().execute_with(|| {
			let referrer: AccountId = 1;
			let ticket: u64 = 1234;

			assert_ok!(mock_designs());
			assert_ok!(mock_person(referrer, Some(InkSpec::DesignedElective(0, 0))));

			// Referrer sets the ticket
			assert_ok!(PoI::set_referral_ticket(RuntimeOrigin::signed(referrer), ticket));
			assert_eq!(ReferralTickets::<Test>::get(referrer).map(|rt| rt[0].ticket), Some(ticket));

			// Referrer cancels the ticket
			assert_ok!(PoI::cancel_referral_ticket(RuntimeOrigin::signed(referrer), ticket));
			assert_eq!(ReferralTickets::<Test>::get(referrer).unwrap().len(), 0);

			// Referrer cancels the same ticket
			assert_noop!(
				PoI::cancel_referral_ticket(RuntimeOrigin::signed(referrer), ticket),
				Error::<Test>::NoTicket
			);
		});
	}
}

#[test]
fn referred_candidate_full_flow_works() {
	// We'll do the entire flow for a referred candidate:
	// 1) Setup a referrer with a referral ticket
	// 2) Candidate applies via `apply_with_signature`
	// 3) Candidate calls `commit` (referred origin)
	// 4) Candidate does a `reroll` (referred origin) (optional, but demonstrates usage)
	// 5) Candidate `submit_evidence` (referred origin)
	// 6) Oracle (Root) calls `judged` => candidate proven
	// 7) Candidate calls `register_referred` (referred origin)
	//
	// For every call possible we will use the `ReferredCandidate` origin in order to assert
	// it is passing.
	TestExt::new().execute_with(|| {
		// ------------------------------------------------------------
		// Setup the environment
		// ------------------------------------------------------------
		const REFERRER: u64 = 1;
		const CANDIDATE: u64 = DENIED_PAYMENT_ACCOUNT;
		const REFERRAL_TICKET: u64 = 777;
		let referral_signature = sp_runtime::testing::TestSignature(
			REFERRAL_TICKET,
			CANDIDATE.encode(), // The payload that must match in apply_with_signature
		);

		advance_by(1);
		assert_ok!(mock_designs());
		assert_ok!(append_reimbursement_values(4, 1, 10));
		assert_ok!(mock_person(REFERRER, Some(InkSpec::DesignedElective(0, 0))));

		// Referrer sets a referral ticket.
		// This simply needs a regular Signed origin.
		assert_ok!(PoI::set_referral_ticket(RuntimeOrigin::signed(REFERRER), REFERRAL_TICKET));
		System::assert_has_event(
			Event::TicketReferred { referrer: REFERRER, ticket: REFERRAL_TICKET }.into(),
		);

		// ------------------------------------------------------------
		// Step 1) Candidate applies with signature
		// ------------------------------------------------------------
		let apply_call = Call::apply_with_signature {
			referrer: REFERRER,
			signature: referral_signature,
			ticket: REFERRAL_TICKET,
		};
		//
		// We pass `(false, true)` so that
		//  - `ProofOfInkAsReferred` is disabled (false),
		//  - `ProofOfInkApplyWithSig` is enabled (true).
		//
		// This ensures the `apply_with_signature` signed-extension checks pass,
		// giving us the origin `AuthorizedApplyWithSig(CANDIDATE)`.
		assert_ok!(exec_tx(
			CANDIDATE,
			0,
			apply_call,
			Some(AsProofOfInkParticipantInfo::AsApplyWithSig(0))
		));

		// We now expect the candidate to be in `Candidate::Applied { cred: Referred(...) }`.
		assert!(
			matches!(
				Candidates::<Test>::get(CANDIDATE),
				Some(Candidate::Applied { cred: Credibility::Referred(r), .. }) if r == REFERRER
			),
			"apply_with_signature did not set candidate to referred Applied status."
		);
		System::assert_has_event(
			Event::TicketApplied { account_id: CANDIDATE, referrer: REFERRER }.into(),
		);

		// ------------------------------------------------------------
		// Step 2) Candidate commits a design
		// ------------------------------------------------------------
		let commit_call =
			Call::commit { choice: InkChoice::DesignedElective(0, 1), require_id: None };
		//
		// This extrinsic uses `ensure_signed_or_referred`.
		// So we pass `(true, false)` to enable the **ReferredCandidate** extension.
		assert_ok!(exec_tx(
			CANDIDATE,
			1,
			commit_call,
			Some(AsProofOfInkParticipantInfo::AsReferred(1))
		));

		assert!(
			matches!(
				Candidates::<Test>::get(CANDIDATE),
				Some(Candidate::Selected { cred: Credibility::Referred(_), .. })
			),
			"commit did not move candidate into Selected status."
		);

		// ------------------------------------------------------------
		// Step 3) (optional) Candidate does `reroll` as a referred origin
		// ------------------------------------------------------------
		// We can only reroll if enough blocks have passed:
		let config = Configuration::<Test>::get();
		System::set_block_number(System::block_number() + config.reroll_timeout + 1);

		let reroll_call = Call::reroll {};
		assert_ok!(exec_tx(
			CANDIDATE,
			2,
			reroll_call,
			Some(AsProofOfInkParticipantInfo::AsReferred(2))
		));

		// ------------------------------------------------------------
		// Step 4) Candidate `submit_evidence` as referred origin
		// ------------------------------------------------------------
		let evidence_hash: EvidenceHash = Default::default();
		let submit_evidence_call = Call::submit_evidence { evidence: evidence_hash };
		assert_ok!(exec_tx(
			CANDIDATE,
			3,
			submit_evidence_call,
			Some(AsProofOfInkParticipantInfo::AsReferred(3))
		));

		// The candidate is now in a `Candidate::Selected { judging: Some(...), ... }`.

		// ------------------------------------------------------------
		// Step 5) Oracle (Root) provides judgement
		// ------------------------------------------------------------
		// `judged` must be called by Root. Let's get the ticket from the candidate's status:
		let status = Candidates::<Test>::get(CANDIDATE).expect("candidate must exist");
		let Candidate::Selected { judging: Some(judging_ticket), .. } = status else {
			panic!("Candidate not in Selected state with a `judging` ticket")
		};
		let context = CANDIDATE.encode().try_into().unwrap();
		let root = RuntimeOrigin::root();
		let judgement = Judgement::Truth(Truth::True);
		assert_ok!(PoI::judged(root, judging_ticket, context, judgement));
		// candidate is now in `Candidate::Proven { was_referred: true, ... }`.

		// ------------------------------------------------------------
		// Step 6) Candidate calls `register_referred`
		// ------------------------------------------------------------
		// The call signature is: `register_referred(origin, key, destination)`.
		// We need to pass a mock key for the personal ID that was reserved:
		let proven = Candidates::<Test>::get(CANDIDATE).unwrap();
		let personal_id = match proven {
			Candidate::Proven { reserved, .. } => reserved,
			_ => panic!("Candidate not proven after judgement"),
		};

		// Just produce a mock key for that ID.
		let destination = 123456u64;
		let (pk, sk) = mock_key(1234);
		let proof_of_ownership = {
			let mut m = b"pop register using".to_vec();
			m.extend_from_slice(&CANDIDATE.encode());
			Mock::sign(&sk, &m[..]).unwrap()
		};
		let register_referred_call =
			Call::register_referred { key: pk, destination, proof_of_ownership };

		assert_ok!(exec_tx(
			CANDIDATE,
			4,
			register_referred_call,
			Some(AsProofOfInkParticipantInfo::AsReferred(4))
		));

		// Candidate is now removed from `Candidates`, and recognized as a full Person with ID.
		assert!(!Candidates::<Test>::contains_key(CANDIDATE));
		assert!(matches!(
			People::<Test>::get(personal_id).unwrap().design,
			Some(InkSpec::DesignedElective(0, 1))
		));
		System::assert_has_event(
			Event::PersonRegistered { account_id: CANDIDATE, personal_id }.into(),
		);

		// The referrer is now allowed to refer two people (the allowance got incremented)
		assert_eq!(People::<Test>::get(REFERRER).unwrap().allowed_referral_tickets, 2);
	});
}

/// Describes the lifecycle of candidate from invitation to personhood recognition.
#[test]
fn invited_candidate_full_flow() {
	TestExt::new().execute_with(|| {
		// ------------------------------------------------------------
		// Setup the environment
		// ------------------------------------------------------------
		const INVITER: u64 = 1;
		const CANDIDATE: u64 = 2;
		const INVITATION_TICKET: u64 = 777;
		let invitation_signature = sp_runtime::testing::TestSignature(
			INVITATION_TICKET,
			CANDIDATE.encode(), // The payload that must match in apply_with_invitation
		);

		advance_by(1);
		assert_ok!(mock_designs());
		assert_ok!(mock_person(INVITER, Some(InkSpec::DesignedElective(0, 0))));
		assert_ok!(append_reimbursement_values(4, 1, 10));

		// ------------------------------------------------------------
		// 1. Inviter receives one invitation to give away
		// ------------------------------------------------------------
		assert_ok!(PoI::grant_invites(RuntimeOrigin::root(), INVITER, 1));
		System::assert_has_event(Event::InvitesGranted { account: INVITER, count: 1 }.into());
		assert_ok!(PoI::set_invite_ticket(RuntimeOrigin::signed(INVITER), INVITATION_TICKET));
		System::assert_has_event(
			Event::InviteTicketSet { inviter: INVITER, ticket: INVITATION_TICKET }.into(),
		);

		// ------------------------------------------------------------
		// 2. Candidate applies with invitation
		// ------------------------------------------------------------
		let apply_call = Call::apply_with_invitation {
			inviter: INVITER,
			signature: invitation_signature,
			ticket: INVITATION_TICKET,
		};

		// We pass `(false, false, true)` so that
		//  - `ProofOfInkAsReferred` is disabled (false),
		//  - `ProofOfInkApplyWithSig` is disabled (false).
		//  - `ProofOfInkAsInvited` is enabled (true).
		//
		// This ensures the `apply_with_invitation` signed-extension checks pass,
		// giving us the origin `InvitedCandidate(CANDIDATE)`.
		assert_ok!(exec_tx(
			CANDIDATE,
			0,
			apply_call,
			Some(AsProofOfInkParticipantInfo::AsInvited(0))
		));

		// We now expect the candidate to be in `Candidate::Applied { cred: Invited(...) }`.
		assert!(
			matches!(
				Candidates::<Test>::get(CANDIDATE),
				Some(Candidate::Applied { cred: Credibility::Invited(t), .. }) if t == INVITATION_TICKET
			),
			"apply_with_invitation did not set candidate to Invited credibility or ticket is incorrect"
		);
		System::assert_has_event(
			Event::InvitedCandidateApplied { who: CANDIDATE, inviter: INVITER }.into(),
		);

		// ------------------------------------------------------------
		// 3. Candidate commits to a design
		// ------------------------------------------------------------
		let commit_call =
			Call::commit { choice: InkChoice::DesignedElective(0, 1), require_id: None };

		// This extrinsic uses `ensure_credible`.
		// So we pass `(Some(AsProofOfInkParticipantInfo::AsInvited(0)))` to enable the
		// **InvitedCandidate** extension.
		assert_ok!(exec_tx(
			CANDIDATE,
			1,
			commit_call,
			Some(AsProofOfInkParticipantInfo::AsInvited(1))
		));

		assert!(
			matches!(
				Candidates::<Test>::get(CANDIDATE),
				Some(Candidate::Selected { cred: Credibility::Invited(_), .. })
			),
			"commit did not move candidate into Selected status."
		);

		// ------------------------------------------------------------
		// 4. Candidate `submit_evidence` as invited origin
		// ------------------------------------------------------------
		let evidence_hash: EvidenceHash = Default::default();
		let submit_evidence_call = Call::submit_evidence { evidence: evidence_hash };
		assert_ok!(exec_tx(
			CANDIDATE,
			2,
			submit_evidence_call,
			Some(AsProofOfInkParticipantInfo::AsInvited(2))
		));

		// The candidate is now in a `Candidate::Selected { judging: Some(...), ... }`.
		let status = Candidates::<Test>::get(CANDIDATE).expect("candidate must exist");
		let Candidate::Selected { judging: Some(judging_ticket), .. } = status else {
			panic!("Candidate not in Selected state with a `judging` ticket")
		};

		// ------------------------------------------------------------
		// 5. Oracle (Root) provides judgement
		// ------------------------------------------------------------
		let context = CANDIDATE.encode().try_into().unwrap();
		let judgement = Judgement::Truth(Truth::True);
		assert_ok!(PoI::judged(RuntimeOrigin::root(), judging_ticket, context, judgement));

		// candidate is now in `Candidate::Proven { was_referred: true, ... }`.
		let proven = Candidates::<Test>::get(CANDIDATE).unwrap();
		let personal_id = match proven {
			Candidate::Proven { reserved, .. } => reserved,
			_ => panic!("Candidate not proven after judgement"),
		};

		// ------------------------------------------------------------
		// 6. Candidate calls `register_non_referred`
		// ------------------------------------------------------------

		let destination = 123456u64;
		let (pk, sk) = mock_key(1234);
		let message = {
			let mut m = b"pop register using".to_vec();
			m.extend_from_slice(&CANDIDATE.encode());
			m
		};
		let proof_of_ownership = Mock::sign(&sk, &message[..]).unwrap();
		let register_non_referred_call =
			Call::register_non_referred { key: pk, destination, proof_of_ownership };

		assert_ok!(exec_tx(
			CANDIDATE,
			3,
			register_non_referred_call,
			Some(AsProofOfInkParticipantInfo::AsInvited(3))
		)
		.unwrap());

		// Candidate is now removed from `Candidates`, and recognized as a full Person with ID.
		assert!(!Candidates::<Test>::contains_key(CANDIDATE));
		assert!(matches!(
			People::<Test>::get(personal_id).unwrap().design,
			Some(InkSpec::DesignedElective(0, 1))
		));
	});
}

/// Validates that calls with AsInvited transaction extension made by not-invited candidate will
/// fail.
#[test]
fn non_invited_candidate_using_invited_extension() {
	TestExt::new().execute_with(|| {
		// Setup
		const CANDIDATE: u64 = 1;
		let (pk, sk) = mock_key(1234);
		let proof_of_ownership = {
			let mut m = b"pop register using".to_vec();
			m.extend_from_slice(&CANDIDATE.encode());
			Mock::sign(&sk, &m[..]).unwrap()
		};
		advance_by(1);
		System::inc_sufficients(&CANDIDATE);
		assert_ok!(mock_designs());

		// Candidate applies as signed origin
		let apply_call = Call::apply {};

		assert_ok!(exec_tx(CANDIDATE, 0, apply_call, None,));

		assert!(matches!(
			Candidates::<Test>::get(CANDIDATE),
			Some(Candidate::Applied { cred: Credibility::Deposit(_), .. })
		));
		System::assert_has_event(Event::CandidateApplied { account_id: CANDIDATE }.into());

		// Candidate commits to a design
		let commit_call =
			Call::commit { choice: InkChoice::DesignedElective(0, 1), require_id: None };

		// Candidate tries to call "commit" with AsInvited extension, but the attempt fails
		assert_noop!(
			exec_tx(
				CANDIDATE,
				1,
				commit_call.clone(),
				Some(AsProofOfInkParticipantInfo::AsInvited(1))
			),
			InvalidTransaction::BadSigner
		);

		// Same call as signed origin succeeds
		assert_ok!(exec_tx(CANDIDATE, 1, commit_call, None));

		// and the candidate is now in "Selected" state
		assert!(matches!(
			Candidates::<Test>::get(CANDIDATE),
			Some(Candidate::Selected { cred: Credibility::Deposit(_), .. })
		));

		// Candidate submits evidence
		let evidence_hash: EvidenceHash = Default::default();
		let submit_evidence_call = Call::submit_evidence { evidence: evidence_hash };

		// Candidate tries to call "submit_evidence" with AsInvited extension, but the attempt fails
		assert_noop!(
			exec_tx(
				CANDIDATE,
				2,
				submit_evidence_call.clone(),
				Some(AsProofOfInkParticipantInfo::AsInvited(2))
			),
			InvalidTransaction::BadSigner
		);

		// Same call as signed origin succeeds
		assert_ok!(exec_tx(CANDIDATE, 2, submit_evidence_call, None,));

		// and the candidate is now in "Selected" state with some "judging"
		let status = Candidates::<Test>::get(CANDIDATE).expect("candidate must exist");
		let Candidate::Selected { judging: Some(judging_ticket), .. } = status else {
			panic!("Candidate not in Selected state with a `judging` ticket")
		};

		// Oracle (Root) provides judgement
		let context = CANDIDATE.encode().try_into().unwrap();
		let judgement = Judgement::Truth(Truth::True);
		assert_ok!(PoI::judged(RuntimeOrigin::root(), judging_ticket, context, judgement));
		assert!(matches!(
			Candidates::<Test>::get(CANDIDATE),
			Some(Candidate::Proven { was_invited: false, was_referred: false, .. })
		));
		System::assert_has_event(
			Event::JudgementProvided { account_id: CANDIDATE, judgement }.into(),
		);

		// Candidate calls `register_non_referred`
		let destination = 123456u64;
		let register_non_referred_call =
			Call::register_non_referred { key: pk, destination, proof_of_ownership };

		// Candidate tries to call "register_non_referred" with AsInvited extension, but the attempt
		// fails
		assert_noop!(
			exec_tx(
				CANDIDATE,
				3,
				register_non_referred_call.clone(),
				Some(AsProofOfInkParticipantInfo::AsInvited(3))
			),
			InvalidTransaction::BadSigner
		);

		// Same call as signed origin succeeds
		assert_ok!(exec_tx(CANDIDATE, 3, register_non_referred_call, None,));
	});
}

#[test]
fn granted_invites_can_be_removed() {
	TestExt::new().execute_with(|| {
		const INVITER: u64 = 1;
		const INVITATION_TICKET: u64 = 777;
		advance_by(1);

		// Inviter receives two invitations to give away
		assert_ok!(PoI::grant_invites(RuntimeOrigin::root(), INVITER, 2));
		assert_eq!(AvailableInvites::<Test>::get(INVITER), 2);
		System::assert_has_event(Event::InvitesGranted { account: INVITER, count: 2 }.into());

		// Inviter uses one invitation
		assert_ok!(PoI::set_invite_ticket(RuntimeOrigin::signed(INVITER), INVITATION_TICKET));
		assert_eq!(AvailableInvites::<Test>::get(INVITER), 1);
		assert!(PendingInvites::<Test>::get(INVITER, INVITATION_TICKET).is_some());
		System::assert_has_event(
			Event::InviteTicketSet { inviter: INVITER, ticket: INVITATION_TICKET }.into(),
		);

		// The invites previously granted to the Inviter are cancelled
		assert_ok!(PoI::remove_available_and_pending_invites(RuntimeOrigin::root(), INVITER, 100));
		System::assert_has_event(Event::AllInvitesRemoved { inviter: INVITER }.into());

		// Both unused and used invites should be removed
		assert!(PendingInvites::<Test>::get(INVITER, INVITATION_TICKET).is_none());
		assert_eq!(AvailableInvites::<Test>::get(INVITER), 0);
	});
}

#[test]
fn invite_tickets_can_be_cancelled() {
	TestExt::new().execute_with(|| {
		const INVITER: u64 = 1;
		const INVITATION_TICKET_1: u64 = 777;
		const INVITATION_TICKET_2: u64 = 778;
		advance_by(1);

		// Inviter receives three invitations to give away
		assert_ok!(PoI::grant_invites(RuntimeOrigin::root(), INVITER, 3));
		assert_eq!(AvailableInvites::<Test>::get(INVITER), 3);
		System::assert_has_event(Event::InvitesGranted { account: INVITER, count: 3 }.into());

		// Inviter uses two invitations
		assert_ok!(PoI::set_invite_ticket(RuntimeOrigin::signed(INVITER), INVITATION_TICKET_1));
		System::assert_has_event(
			Event::InviteTicketSet { inviter: INVITER, ticket: INVITATION_TICKET_1 }.into(),
		);
		assert_ok!(PoI::set_invite_ticket(RuntimeOrigin::signed(INVITER), INVITATION_TICKET_2));
		System::assert_has_event(
			Event::InviteTicketSet { inviter: INVITER, ticket: INVITATION_TICKET_2 }.into(),
		);
		assert_eq!(AvailableInvites::<Test>::get(INVITER), 1);
		assert!(PendingInvites::<Test>::get(INVITER, INVITATION_TICKET_1).is_some());
		assert!(PendingInvites::<Test>::get(INVITER, INVITATION_TICKET_2).is_some());

		// --- Inviter cancels 1st invitation ticket
		assert_ok!(PoI::cancel_invite_ticket(RuntimeOrigin::signed(INVITER), INVITATION_TICKET_1));
		System::assert_has_event(
			Event::InviteTicketCancelled { inviter: INVITER, ticket: INVITATION_TICKET_1 }.into(),
		);

		// The ticket is removed
		assert!(PendingInvites::<Test>::get(INVITER, INVITATION_TICKET_1).is_none());

		// The other ticket stays available
		assert!(PendingInvites::<Test>::get(INVITER, INVITATION_TICKET_2).is_some());

		// Removed ticket comes back as available
		assert_eq!(AvailableInvites::<Test>::get(INVITER), 2);

		// --- Inviter cancels 2nd invitation ticket
		assert_ok!(PoI::cancel_invite_ticket(RuntimeOrigin::signed(INVITER), INVITATION_TICKET_2));
		System::assert_has_event(
			Event::InviteTicketCancelled { inviter: INVITER, ticket: INVITATION_TICKET_2 }.into(),
		);

		// The ticket is removed
		assert!(PendingInvites::<Test>::get(INVITER, INVITATION_TICKET_2).is_none());

		// Removed ticket comes back as available
		assert_eq!(AvailableInvites::<Test>::get(INVITER), 3);
	});
}

#[test]
fn designed_elective_committed_map_is_updated_by_judgement() {
	use indiv_support::traits::Truth::{False, True};

	TestExt::new().execute_with(|| {
		advance_by(1);
		assert_ok!(mock_designs());

		// 1. Contempt - the design entry must be removed
		const CONTEMPT_CAND: u64 = 100;
		let (ticket, ctx) = prepare_for_judgement(CONTEMPT_CAND);
		assert_ok!(mock_candidate(
			CONTEMPT_CAND,
			None,
			Some((InkChoice::DesignedElective(0, 0), Allocation::Full)),
			Some(ticket),
			false
		));
		assert_eq!(CommittedDesigns::<Test>::get(0, 0), Some(DesignStatus::Reserved));

		assert_ok!(PoI::judged(RuntimeOrigin::root(), ticket, ctx, Judgement::Contempt));
		System::assert_has_event(
			Event::JudgementProvided { account_id: CONTEMPT_CAND, judgement: Judgement::Contempt }
				.into(),
		);
		assert_eq!(
			CommittedDesigns::<Test>::get(0, 0),
			None,
			"reserved entry must be cleared on contempt"
		);

		// 2. Truth‑False + max‑retries reached ‒ the design entry must be removed
		const HARD_FAIL_CAND: u64 = 101;
		let (ticket, ctx) = prepare_for_judgement(HARD_FAIL_CAND);
		assert_ok!(mock_candidate(
			HARD_FAIL_CAND,
			None,
			Some((InkChoice::DesignedElective(0, 1), Allocation::Full)),
			Some(ticket),
			false
		));
		// Make the next failure exceed MaxRetryAttempts
		Candidates::<Test>::mutate(HARD_FAIL_CAND, |info| {
			if let Some(Candidate::Selected { failed, .. }) = info.as_mut() {
				*failed = <<Test as Config>::MaxRetryAttempts as Get<u32>>::get();
			}
		});
		assert_eq!(CommittedDesigns::<Test>::get(0, 1), Some(DesignStatus::Reserved));

		assert_ok!(PoI::judged(RuntimeOrigin::root(), ticket, ctx, Judgement::Truth(False)));
		System::assert_has_event(
			Event::JudgementProvided {
				account_id: HARD_FAIL_CAND,
				judgement: Judgement::Truth(False),
			}
			.into(),
		);
		assert_eq!(
			CommittedDesigns::<Test>::get(0, 1),
			None,
			"reserved entry must be cleared after hard failure"
		);

		// 3. Truth-True (success) - the design entry must become Committed
		const SUCCESS_CAND: u64 = 102;
		let (ticket, ctx) = prepare_for_judgement(SUCCESS_CAND);
		assert_ok!(mock_candidate(
			SUCCESS_CAND,
			None,
			Some((InkChoice::DesignedElective(0, 2), Allocation::Full)),
			Some(ticket),
			false
		));
		assert_eq!(CommittedDesigns::<Test>::get(0, 2), Some(DesignStatus::Reserved));

		assert_ok!(PoI::judged(RuntimeOrigin::root(), ticket, ctx, Judgement::Truth(True)));
		assert_eq!(
			CommittedDesigns::<Test>::get(0, 2),
			Some(DesignStatus::Committed),
			"entry must transition to Committed on success"
		);
	});
}

#[test]
fn set_configuration_emits_event() {
	TestExt::new().execute_with(|| {
		advance_by(1);
		let config = new_config();
		assert_ok!(PoI::set_configuration(RuntimeOrigin::root(), config.clone()));
		System::assert_has_event(Event::ConfigurationSet { config }.into());
	});
}

#[test]
fn retry_granted_emits_event() {
	TestExt::new().execute_with(|| {
		advance_by(1);
		assert_ok!(mock_designs());

		const CANDIDATE: u64 = 100;
		let (ticket, ctx) = prepare_for_judgement(CANDIDATE);
		// Create candidate with failed = 0 (default).
		assert_ok!(mock_candidate(
			CANDIDATE,
			None,
			Some((InkChoice::DesignedElective(0, 0), Allocation::Full)),
			Some(ticket),
			false
		));

		// Judge with Truth(False). `failed` increments from 0 to 1. Since 1 is not greater
		// than MaxRetryAttempts (also 1), a retry is granted instead of removing the candidate.
		assert_ok!(PoI::judged(RuntimeOrigin::root(), ticket, ctx, Judgement::Truth(False)));
		System::assert_has_event(Event::RetryGranted { account_id: CANDIDATE, failures: 1 }.into());

		assert!(matches!(
			Candidates::<Test>::get(CANDIDATE),
			Some(Candidate::Selected { failed: 1, .. })
		));
	});
}

#[test]
fn remove_available_and_pending_invites_emits_event() {
	TestExt::new().execute_with(|| {
		advance_by(1);
		const INVITER: u64 = 1;
		const INVITATION_TICKET_1: u64 = 100;
		const INVITATION_TICKET_2: u64 = 101;

		assert_ok!(mock_person(INVITER, None));
		assert_ok!(PoI::grant_invites(RuntimeOrigin::root(), INVITER, 2));
		assert_ok!(PoI::set_invite_ticket(RuntimeOrigin::signed(INVITER), INVITATION_TICKET_1));
		assert_ok!(PoI::set_invite_ticket(RuntimeOrigin::signed(INVITER), INVITATION_TICKET_2));

		assert_ok!(PoI::remove_available_and_pending_invites(RuntimeOrigin::root(), INVITER, 10));
		// Note: The in-memory test backend always clears all entries on `clear_prefix`
		// regardless of the limit, so `SomeInvitesRemoved` cannot be triggered in unit
		// tests. Only `AllInvitesRemoved` is emitted here. The `SomeInvitesRemoved` path
		// would require a real trie-backed state that supports cursor-based iteration.
		System::assert_has_event(Event::AllInvitesRemoved { inviter: INVITER }.into());
		assert_eq!(AvailableInvites::<Test>::get(INVITER), 0);
		assert!(PendingInvites::<Test>::get(INVITER, INVITATION_TICKET_1).is_none());
		assert!(PendingInvites::<Test>::get(INVITER, INVITATION_TICKET_2).is_none());
	});
}
