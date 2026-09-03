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

//! Staking pallet benchmarking.

#![allow(unused)]
#![allow(clippy::needless_range_loop)]

extern crate alloc;
use super::*;
use crate::Pallet as PoI;
use alloc::vec::Vec;
use frame_benchmarking::{
	account, impl_benchmark_test_suite, v2::*, whitelisted_caller, BenchmarkError,
};
use frame_support::{
	assert_ok,
	pallet_prelude::ConstU32,
	traits::{fungible::Mutate, Consideration, ConstU16, EnsureOrigin, Get},
};
use frame_system::RawOrigin as SystemOrigin;
use indiv_support::traits::{Judgement, JudgementContext};
#[cfg(feature = "std")]
use sp_runtime::testing::{TestSignature, UintAuthorityId};
use sp_runtime::{traits::DispatchTransaction, BoundedVec};

const BENCHMARKING_UPPER_LIMIT: u32 = 100_000;

const SEED: u32 = 0;

const PEOPLE_COUNT: usize = 1000;
const CANDIDATE_COUNT: usize = 1000;

const DESIGNED_FAMILY_START_INDEX: FamilyIndex = 0;
const PROCEDURAL_ACCOUNT_FAMILY_START_INDEX: FamilyIndex = 10;
const PROCEDURAL_PERSONAL_FAMILY_START_INDEX: FamilyIndex = 20;
const PROCEDURAL_FAMILY_START_INDEX: FamilyIndex = 30;
const FAMILIES_PER_KIND: FamilyIndex = 10;

/// Benchmark Helper
pub trait BenchmarkHelper<T: Config> {
	fn create_tickets(seed: u64) -> BoundedVec<ReferralTicket<T::Ticket>, T::MaxActiveReferrals>;
	fn create_ticket(seed: u64) -> (T::TicketPublic, T::Ticket);
	fn sign(seed: u64, msg: &[u8]) -> T::TicketSignature;
	fn build_person_origin(personal_id: PersonalId) -> T::RuntimeOrigin;
	fn setup_currency();
}

fn assert_last_event<T: Config>(generic_event: <T as frame_system::Config>::RuntimeEvent) {
	frame_system::Pallet::<T>::assert_last_event(generic_event.into());
}

fn account_id_to_slice<T: Config>(account_id: T::AccountId) -> [u8; 32] {
	let mut ret = [0u8; 32];
	account_id.using_encoded(|bytes| {
		let len = bytes.len().min(32);
		ret[..len].copy_from_slice(&bytes[..len]);
	});
	ret
}

fn generate_ink_spec<T: Config>(account_id: T::AccountId, personal_id: PersonalId) -> InkSpec {
	let family_index: FamilyIndex = personal_id as u16 % (FAMILIES_PER_KIND * 4);
	match family_index / FAMILIES_PER_KIND {
		0 => InkSpec::DesignedElective(family_index, personal_id as u16),
		1 => InkSpec::ProceduralAccount(family_index, account_id_to_slice::<T>(account_id)),
		2 => InkSpec::ProceduralPersonal(family_index, personal_id),
		3 => InkSpec::Procedural(
			family_index,
			account_id_to_slice::<T>(account_id)[..4].try_into().unwrap(),
		),
		_ => unreachable!(),
	}
}

fn register_families<T: Config>() {
	for i in 0..FAMILIES_PER_KIND {
		<DesignFamilies<T>>::insert(
			DESIGNED_FAMILY_START_INDEX + i,
			Family { kind: FamilyKind::Designed { count: 10000 }, id: [0u8; 32] },
		);
		<DesignFamilies<T>>::insert(
			PROCEDURAL_ACCOUNT_FAMILY_START_INDEX + i,
			Family { kind: FamilyKind::ProceduralAccount, id: [0u8; 32] },
		);
		<DesignFamilies<T>>::insert(
			PROCEDURAL_PERSONAL_FAMILY_START_INDEX + i,
			Family { kind: FamilyKind::ProceduralPersonal, id: [0u8; 32] },
		);
		<DesignFamilies<T>>::insert(
			PROCEDURAL_FAMILY_START_INDEX + i,
			Family { kind: FamilyKind::Procedural { range: 10 }, id: [0u8; 32] },
		);
	}
}

fn register_people<T: Config>(count: u32) -> Vec<(T::AccountId, PersonalId)> {
	register_people_with_referrals::<T>(count, T::MaxActiveReferrals::get() - 1)
}

fn register_people_with_referrals<T: Config>(
	count: u32,
	referrals: u32,
) -> Vec<(T::AccountId, PersonalId)> {
	// Ensure the people collection is initialized before registering people.
	T::People::initialize_people_collection();

	(0..count)
		.map(|c| {
			let who: T::AccountId = account("person", c, SEED);
			let personal_id = T::People::reserve_new_id();
			let (key, _secret) = T::People::mock_key(personal_id);
			assert_ok!(T::People::recognize_personhood(personal_id, Some(key)));
			let design = generate_ink_spec::<T>(who.clone(), personal_id);
			let referrals: Vec<T::AccountId> =
				(0..referrals).map(|c| account("init_referrals", c, SEED)).collect::<Vec<_>>();
			<People<T>>::insert(
				personal_id,
				Person {
					design: Some(design.clone()),
					active_referrals: referrals.try_into().unwrap(),
					allowed_referral_tickets: 1,
					bad_referrals: 0,
					successful_referrals: 0,
					referrals: 0,
					derivatives: 0,
					banned: false,
					pending_referral_rewards: 0,
				},
			);
			if let InkSpec::DesignedElective(family_id, design_index) = design {
				<CommittedDesigns<T>>::insert(family_id, design_index, DesignStatus::Committed);
			}

			(who, personal_id)
		})
		.collect()
}

fn register_candidates<T: Config>(count: u32) -> Vec<T::AccountId> {
	let candidates: Vec<T::AccountId> =
		(0..count).map(|c| account("candidate", c, SEED)).collect::<Vec<_>>();

	for who in candidates.iter() {
		let footprint = Pallet::<T>::apply_footprint();
		T::Deposit::ensure_successful(who, footprint);
		let Ok(deposit) = T::Deposit::new(who, footprint) else {
			unreachable!("we called `ensure_successful` above");
		};
		let status: CandidateOf<T> = Candidate::Applied {
			cred: Credibility::Deposit(deposit),
			entropy: (b"poi/apply", who).using_encoded(|s| T::Randomness::random(s).0),
			entropy_since: frame_system::Pallet::<T>::block_number(),
		};
		<Candidates<T>>::insert(who, status);
	}

	candidates
}

fn register_reimbursement_values<T: Config>(
	referred_value: BalanceOf<T>,
	referrer_value: BalanceOf<T>,
) {
	let mut values: BoundedVec<(BalanceOf<T>, u32), T::MaxReimbursementValues> = Default::default();
	values.try_push((referred_value, 1)).expect("should have space for 1 value");
	values.try_push((referred_value, 1)).expect("should have space for 2 values");
	ReferredReimbursementValues::<T>::put(values);

	let mut values: BoundedVec<(BalanceOf<T>, u32), T::MaxReimbursementValues> = Default::default();
	values.try_push((referrer_value, 1)).expect("should have space for 1 value");
	values.try_push((referrer_value, 1)).expect("should have space for 2 values");
	ReferrerReimbursementValues::<T>::put(values);
}

fn reimbursement_value<T: Config>() -> BalanceOf<T> {
	T::Currency::minimum_balance().max(1u32.into())
}

#[benchmarks(
	where T: Config + core::marker::Send + core::marker::Sync,
	T::RuntimeCall: Dispatchable<Info = DispatchInfo, PostInfo = PostDispatchInfo> + From<Call<T>>,
	<T::RuntimeCall as Dispatchable>::RuntimeOrigin: AsSystemOriginSigner<T::AccountId> + AsTransactionAuthorizedOrigin + Clone,
)]
mod benches {
	use super::*;
	use crate::extension::{AsProofOfInkParticipant, AsProofOfInkParticipantInfo};
	use frame_support::dispatch::{DispatchInfo, PostDispatchInfo};
	use sp_runtime::traits::{AsSystemOriginSigner, AsTransactionAuthorizedOrigin, Dispatchable};

	#[benchmark]
	fn apply() -> Result<(), BenchmarkError> {
		register_families::<T>();
		register_people::<T>(PEOPLE_COUNT as u32);
		register_candidates::<T>(CANDIDATE_COUNT as u32);

		let caller: T::AccountId = account("apply", 0, SEED);
		let _ = frame_system::Pallet::<T>::inc_providers(&caller);
		T::Deposit::ensure_successful(&caller, Pallet::<T>::apply_footprint());

		#[extrinsic_call]
		_(SystemOrigin::Signed(caller.clone()));

		// assert event
		assert_last_event::<T>(Event::CandidateApplied { account_id: caller }.into());
		Ok(())
	}

	#[benchmark]
	fn submit_evidence(d: Linear<0, 3>, a: Linear<0, 1>) -> Result<(), BenchmarkError> {
		register_families::<T>();
		register_people::<T>(PEOPLE_COUNT as u32);
		let candidates = register_candidates::<T>(CANDIDATE_COUNT as u32);

		let caller: T::AccountId = whitelisted_caller();
		let referrer: PersonalId = 5;
		let reserved: PersonalId = T::People::reserve_new_id();

		let entropy = (b"poi/apply", caller.clone()).using_encoded(|s| T::Randomness::random(s).0);
		let allocation = if a > 0 { Allocation::Full } else { Allocation::Initial };
		let choice = match d {
			0 => InkChoice::DesignedElective(0, 9999),
			1 => InkChoice::ProceduralAccount(10),
			2 => InkChoice::ProceduralPersonal(20),
			3 => InkChoice::Procedural(30, 0),
			_ => unreachable!("there are only 4 ink specs so far"),
		};
		let design =
			PoI::<T>::bake_design(choice.clone(), entropy, caller.clone(), reserved).unwrap();

		let status = Candidate::Selected {
			since: frame_system::Pallet::<T>::block_number(),
			cred: Credibility::Referred(referrer),
			reserved,
			entropy,
			design,
			allocation,
			judging: None,
			failed: 0,
		};
		frame_system::Pallet::<T>::inc_sufficients(&caller);
		<Candidates<T>>::insert(&caller, status);
		<People<T>>::mutate(referrer, |person| {
			person.as_mut().unwrap().active_referrals.try_push(caller.clone()).unwrap()
		});

		let evidence_hash: EvidenceHash = Default::default();

		#[extrinsic_call]
		_(SystemOrigin::Signed(caller.clone()), evidence_hash);

		assert!(
			matches!(Candidates::<T>::get(caller.clone()).unwrap(), Candidate::Selected { judging, .. } if judging.is_some())
		);
		assert_last_event::<T>(Event::JudgementRequested { account_id: caller }.into());
		Ok(())
	}

	#[benchmark]
	fn judged(r: Linear<0, { T::MaxActiveReferrals::get() - 1 }>) -> Result<(), BenchmarkError> {
		register_families::<T>();
		register_people_with_referrals::<T>(PEOPLE_COUNT as u32, r);
		let candidates = register_candidates::<T>(CANDIDATE_COUNT as u32);

		let caller: T::AccountId = whitelisted_caller();
		let referrer: PersonalId = 5;
		let reserved: PersonalId = T::People::reserve_new_id();

		let entropy = (b"poi/apply", caller.clone()).using_encoded(|s| T::Randomness::random(s).0);
		let allocation = Allocation::Full;
		let cred = Credibility::Referred(referrer);
		let design = PoI::<T>::bake_design(
			InkChoice::DesignedElective(0, 9999),
			entropy,
			caller.clone(),
			reserved,
		)
		.unwrap();
		let judgement = Judgement::Truth(True);
		let ticket: OracleTicketOf<T> = Default::default();
		let context: JudgementContext = caller.encode().try_into().unwrap();
		let expected_context_id = T::AccountId::decode(&mut &context[..]).unwrap();
		assert_eq!(caller, expected_context_id);
		let status = Candidate::Selected {
			since: frame_system::Pallet::<T>::block_number(),
			cred,
			reserved,
			entropy,
			design: design.clone(),
			allocation,
			judging: Some(ticket.clone()),
			failed: 0,
		};
		frame_system::Pallet::<T>::inc_sufficients(&caller);
		<Candidates<T>>::insert(&caller, status);
		<People<T>>::mutate(referrer, |person| {
			person.as_mut().unwrap().active_referrals.try_push(caller.clone()).unwrap()
		});
		<AllocationCount<T>>::put(1);

		#[extrinsic_call]
		_(SystemOrigin::Root, ticket, context, judgement);

		assert_eq!(<AllocationCount<T>>::get(), 0);
		let expected_status =
			Candidate::Proven { design, reserved, was_referred: true, was_invited: false };
		assert_eq!(<Candidates<T>>::get(&caller).unwrap(), expected_status);
		assert_last_event::<T>(Event::JudgementProvided { account_id: caller, judgement }.into());
		Ok(())
	}

	#[benchmark]
	fn register_referred() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::setup_currency();
		let referred_value = reimbursement_value::<T>();
		let pot = Pallet::<T>::proof_of_ink_pot_id();
		assert_ok!(T::Currency::mint_into(&pot, referred_value));

		register_families::<T>();
		register_reimbursement_values::<T>(referred_value, referred_value);
		register_people::<T>(PEOPLE_COUNT as u32);
		let candidates = register_candidates::<T>(CANDIDATE_COUNT as u32);

		let caller: T::AccountId = whitelisted_caller();
		let reserved: PersonalId = T::People::reserve_new_id();
		assert!(!People::<T>::contains_key(reserved));

		let design = InkSpec::DesignedElective(0, 9999);
		let status = Candidate::Proven {
			design: design.clone(),
			reserved,
			was_referred: true,
			was_invited: false,
		};
		<Candidates<T>>::insert(&caller, status);
		frame_system::Pallet::<T>::inc_sufficients(&caller);

		let sk = T::Crypto::new_secret([12; 32]);
		let pk = T::Crypto::member_from_secret(&sk);
		let proof_of_ownership = {
			let mut msg = PROOF_OF_OWNERSHIP_PREFIX.to_vec();
			msg.extend_from_slice(&caller.encode());
			T::Crypto::sign(&sk, &msg[..]).unwrap()
		};
		let destination: T::AccountId = account("destination", 0, SEED);

		#[extrinsic_call]
		_(SystemOrigin::Signed(caller.clone()), pk, destination, proof_of_ownership);

		assert!(!Candidates::<T>::contains_key(&caller));
		assert!(matches!(
			People::<T>::get(reserved),
			Some(Person { design: Some(actual_design), .. }) if design == actual_design
		));
		frame_system::Pallet::<T>::assert_has_event(
			Event::PersonRegistered { account_id: caller, personal_id: reserved }.into(),
		);
		Ok(())
	}

	#[benchmark]
	fn register_non_referred() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::setup_currency();
		let referred_value = reimbursement_value::<T>();
		let referrer_value = reimbursement_value::<T>();
		let pot = Pallet::<T>::proof_of_ink_pot_id();
		assert_ok!(T::Currency::mint_into(&pot, referred_value));
		assert_ok!(T::Currency::mint_into(&pot, referrer_value));

		register_families::<T>();
		register_reimbursement_values::<T>(referred_value, referrer_value);
		register_people::<T>(PEOPLE_COUNT as u32);
		let candidates = register_candidates::<T>(CANDIDATE_COUNT as u32);

		let caller: T::AccountId = whitelisted_caller();
		let reserved: PersonalId = T::People::reserve_new_id();
		assert!(!People::<T>::contains_key(reserved));

		let design = InkSpec::DesignedElective(0, 9999);
		let status = Candidate::Proven {
			design: design.clone(),
			reserved,
			was_referred: false,
			was_invited: false,
		};
		<Candidates<T>>::insert(&caller, status);

		let sk = T::Crypto::new_secret([12; 32]);
		let pk = T::Crypto::member_from_secret(&sk);
		let proof_of_ownership = {
			let mut msg = PROOF_OF_OWNERSHIP_PREFIX.to_vec();
			msg.extend_from_slice(&caller.encode());
			T::Crypto::sign(&sk, &msg[..]).unwrap()
		};
		let destination: T::AccountId = account("destination", 1, SEED);

		#[extrinsic_call]
		_(SystemOrigin::Signed(caller.clone()), pk, destination, proof_of_ownership);

		assert!(!Candidates::<T>::contains_key(&caller));
		assert!(matches!(
			People::<T>::get(reserved),
			Some(Person { design: Some(actual_design), .. }) if design == actual_design
		));
		assert_last_event::<T>(
			Event::PersonRegistered { account_id: caller, personal_id: reserved }.into(),
		);
		Ok(())
	}

	#[benchmark]
	fn reroll() -> Result<(), BenchmarkError> {
		register_families::<T>();
		register_people::<T>(PEOPLE_COUNT as u32);
		let candidates = register_candidates::<T>(CANDIDATE_COUNT as u32);

		let caller = candidates.get(999).cloned().unwrap();
		whitelist!(caller);

		let initial_block_number = frame_system::Pallet::<T>::block_number();
		frame_system::Pallet::<T>::set_block_number(
			initial_block_number +
				<Configuration<T>>::get().reroll_timeout +
				<Configuration<T>>::get().reroll_timeout,
		);
		let expected_entropy_since =
			initial_block_number + <Configuration<T>>::get().reroll_timeout;

		#[extrinsic_call]
		_(SystemOrigin::Signed(caller.clone()));

		// assert event
		let actual_status = <Candidates<T>>::get(&caller).unwrap();
		assert!(matches!(
			actual_status,
			Candidate::Applied {
				entropy_since: actual_entropy_since,
				..
			} if actual_entropy_since == expected_entropy_since
		));
		assert_last_event::<T>(Event::Rerolled { account_id: caller }.into());
		Ok(())
	}

	#[benchmark]
	fn commit(c: Linear<0, 4>, a: Linear<0, 1>) -> Result<(), BenchmarkError> {
		register_families::<T>();
		register_people::<T>(PEOPLE_COUNT as u32);
		let candidates = register_candidates::<T>(CANDIDATE_COUNT as u32);

		let caller: T::AccountId = whitelisted_caller();
		let referrer: PersonalId = 5;

		let entropy = (b"poi/apply", caller.clone()).using_encoded(|s| T::Randomness::random(s).0);
		let status = Candidate::Applied {
			cred: Credibility::Referred(referrer),
			entropy,
			entropy_since: frame_system::Pallet::<T>::block_number(),
		};
		frame_system::Pallet::<T>::inc_sufficients(&caller);
		<Candidates<T>>::insert(&caller, status);
		<People<T>>::mutate(referrer, |person| {
			person.as_mut().unwrap().active_referrals.try_push(caller.clone()).unwrap()
		});

		let expected_id: PersonalId = T::People::reserve_new_id() + 1;
		let config = Configuration::<T>::get();
		let (allocation, alloc_count) = if a > 0 {
			(Allocation::Full, 0)
		} else {
			(Allocation::Initial, config.fasttrack_count + 1)
		};
		AllocationCount::<T>::put(alloc_count);

		let choice = match c {
			0 => InkChoice::DesignedElective(0, 9999),
			1 => InkChoice::ProceduralAccount(10),
			2 => InkChoice::ProceduralPersonal(20),
			3 => InkChoice::Procedural(30, 0),
			4 => InkChoice::ProceduralDerivative(30, Some(31)),
			_ => unreachable!("there are only 5 ink choices so far"),
		};

		let design =
			PoI::<T>::bake_design(choice.clone(), entropy, caller.clone(), expected_id).unwrap();

		#[extrinsic_call]
		_(SystemOrigin::Signed(caller.clone()), choice, None);

		assert_eq!(AllocationCount::<T>::get(), alloc_count + 1);
		let expected_status = Candidate::Selected {
			since: frame_system::Pallet::<T>::block_number(),
			cred: Credibility::Referred(referrer),
			reserved: expected_id,
			entropy,
			design,
			allocation,
			judging: None,
			failed: 0,
		};
		assert_eq!(Candidates::<T>::get(caller.clone()).unwrap(), expected_status);
		assert_last_event::<T>(
			Event::DesignCommitted { account_id: caller, reserved_id: expected_id }.into(),
		);
		Ok(())
	}

	#[benchmark]
	fn allocate_full() -> Result<(), BenchmarkError> {
		register_families::<T>();
		register_people::<T>(PEOPLE_COUNT as u32);
		let candidates = register_candidates::<T>(CANDIDATE_COUNT as u32);

		let caller: T::AccountId = whitelisted_caller();
		let referrer: PersonalId = 5;
		let reserved: PersonalId = T::People::reserve_new_id();

		let entropy = (b"poi/apply", caller.clone()).using_encoded(|s| T::Randomness::random(s).0);
		let status = Candidate::Selected {
			since: frame_system::Pallet::<T>::block_number(),
			cred: Credibility::Referred(referrer),
			reserved,
			entropy,
			design: InkSpec::DesignedElective(0, 9999),
			allocation: Allocation::InitDone,
			judging: None,
			failed: 0,
		};
		frame_system::Pallet::<T>::inc_sufficients(&caller);
		<Candidates<T>>::insert(&caller, status);
		<People<T>>::mutate(referrer, |person| {
			person.as_mut().unwrap().active_referrals.try_push(caller.clone()).unwrap()
		});

		#[extrinsic_call]
		_(SystemOrigin::Signed(caller.clone()));

		assert!(matches!(
			<Candidates<T>>::get(&caller).unwrap(),
			Candidate::Selected { allocation: Allocation::Full, .. }
		));
		assert_last_event::<T>(Event::FullyAllocated { account_id: caller }.into());
		Ok(())
	}

	#[benchmark]
	fn timeout() -> Result<(), BenchmarkError> {
		register_families::<T>();
		register_people::<T>(PEOPLE_COUNT as u32);
		register_candidates::<T>(CANDIDATE_COUNT as u32);

		let caller: T::AccountId = whitelisted_caller();
		let referrer: PersonalId = 5;
		let reserved: PersonalId = T::People::reserve_new_id();

		let status = Candidate::Selected {
			since: frame_system::Pallet::<T>::block_number(),
			cred: Credibility::Referred(referrer),
			reserved,
			entropy: (b"poi/apply", caller.clone()).using_encoded(|s| T::Randomness::random(s).0),
			design: InkSpec::DesignedElective(0, 9999),
			allocation: Allocation::Initial,
			judging: None,
			failed: 0,
		};
		frame_system::Pallet::<T>::inc_sufficients(&caller);
		<Candidates<T>>::insert(&caller, status);
		<People<T>>::mutate(referrer, |person| {
			person.as_mut().unwrap().active_referrals.try_push(caller.clone()).unwrap()
		});
		AllocationCount::<T>::put(1);

		let config = Configuration::<T>::get();
		let mut expired = frame_system::Pallet::<T>::block_number() + config.timeout;
		expired.saturating_inc();
		frame_system::Pallet::<T>::set_block_number(expired);

		assert!(<People<T>>::get(referrer).unwrap().active_referrals.is_full());

		#[extrinsic_call]
		_(SystemOrigin::Signed(caller.clone()), caller.clone());

		// assert event
		assert!(!<Candidates<T>>::contains_key(&caller));
		assert!(!<People<T>>::get(referrer).unwrap().active_referrals.is_full());
		assert_eq!(AllocationCount::<T>::get(), 0);
		assert_last_event::<T>(Event::TimedOut { account_id: caller }.into());
		Ok(())
	}

	#[benchmark]
	fn flakeout(r: Linear<0, { T::MaxActiveReferrals::get() - 1 }>) -> Result<(), BenchmarkError> {
		register_families::<T>();
		register_people_with_referrals::<T>(PEOPLE_COUNT as u32, r);
		register_candidates::<T>(CANDIDATE_COUNT as u32);

		let caller: T::AccountId = whitelisted_caller();
		let referrer: PersonalId = 5;

		let status = Candidate::Applied {
			cred: Credibility::Referred(referrer),
			entropy: (b"poi/apply", caller.clone()).using_encoded(|s| T::Randomness::random(s).0),
			entropy_since: frame_system::Pallet::<T>::block_number(),
		};
		frame_system::Pallet::<T>::inc_sufficients(&caller);
		<Candidates<T>>::insert(&caller, status);
		<People<T>>::mutate(referrer, |person| {
			person.as_mut().unwrap().active_referrals.try_push(caller.clone()).unwrap()
		});

		assert!(<People<T>>::get(referrer).unwrap().active_referrals.len() as u32 == r + 1);

		#[extrinsic_call]
		_(SystemOrigin::Signed(caller.clone()));

		// assert event
		assert!(!<Candidates<T>>::contains_key(&caller));
		assert!(<People<T>>::get(referrer).unwrap().active_referrals.len() as u32 == r);
		assert_last_event::<T>(Event::FlakedOut { account_id: caller }.into());
		Ok(())
	}

	#[benchmark]
	fn apply_with_signature() -> Result<(), BenchmarkError> {
		register_families::<T>();
		register_people::<T>(PEOPLE_COUNT as u32);
		register_candidates::<T>(CANDIDATE_COUNT as u32);

		let caller: T::AccountId = whitelisted_caller();
		let referrer: PersonalId = 0u64;
		let referrals = <People<T>>::get(referrer).unwrap().referrals;

		let tickets = T::BenchmarkHelper::create_tickets(referrer);
		let ticket = tickets[0].clone();
		<ReferralTickets<T>>::insert(referrer, tickets);

		let msg = caller.encode();
		let signature = T::BenchmarkHelper::sign(referrer, &msg[..]);

		#[extrinsic_call]
		_(Origin::AuthorizedApplyWithSig(caller.clone()), referrer, signature, ticket.ticket);

		// assert event
		let actual_status = <Candidates<T>>::get(&caller).unwrap();
		assert!(matches!(
			actual_status,
			Candidate::Applied {
				cred: Credibility::Referred(actual_referrer),
				..
			} if actual_referrer == referrer
		));
		assert_last_event::<T>(Event::TicketApplied { account_id: caller, referrer }.into());
		Ok(())
	}

	#[benchmark]
	fn add_design_family() -> Result<(), BenchmarkError> {
		register_families::<T>();

		let index = PROCEDURAL_FAMILY_START_INDEX + FAMILIES_PER_KIND;
		let kind = FamilyKind::Designed { count: 10000 };
		let id = [0u8; 32];

		let origin =
			T::ManagerOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, index, kind.clone(), id);

		// assert event
		assert_eq!(<DesignFamilies<T>>::get(index), Some(Family { kind: kind.clone(), id }));
		assert_last_event::<T>(Event::FamilyAdded { index, kind, id }.into());
		Ok(())
	}

	#[benchmark]
	fn set_referral_ticket() -> Result<(), BenchmarkError> {
		register_families::<T>();
		let people = register_people::<T>(PEOPLE_COUNT as u32);
		register_candidates::<T>(CANDIDATE_COUNT as u32);

		let referrer: PersonalId = people.first().cloned().unwrap().1;

		let (_ticket_public, ticket) = T::BenchmarkHelper::create_ticket(referrer);
		let origin = T::BenchmarkHelper::build_person_origin(referrer);

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, ticket.clone());

		// assert event
		let actual_ticket = <ReferralTickets<T>>::get(referrer).unwrap();
		assert_eq!(actual_ticket[0], ReferralTicket { ticket: ticket.clone() });
		assert_last_event::<T>(Event::TicketReferred { referrer, ticket }.into());
		Ok(())
	}

	#[benchmark]
	fn cancel_referral_ticket() -> Result<(), BenchmarkError> {
		register_families::<T>();
		let people = register_people::<T>(PEOPLE_COUNT as u32);
		register_candidates::<T>(CANDIDATE_COUNT as u32);

		let referrer: PersonalId = people.first().cloned().unwrap().1;

		let tickets = T::BenchmarkHelper::create_tickets(referrer);
		let ticket = tickets[0].clone();
		<ReferralTickets<T>>::insert(referrer, tickets);
		let origin = T::BenchmarkHelper::build_person_origin(referrer);

		let ticket_value = ticket.ticket.clone();

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, ticket.ticket);

		assert!(<ReferralTickets<T>>::contains_key(referrer));
		assert_eq!(ReferralTickets::<T>::get(referrer).unwrap().len(), 0);

		assert_last_event::<T>(Event::TicketCancelled { referrer, ticket: ticket_value }.into());
		Ok(())
	}

	#[benchmark]
	fn as_invited_tx_ext() -> Result<(), BenchmarkError> {
		register_families::<T>();
		register_people::<T>(PEOPLE_COUNT as u32);
		register_candidates::<T>(CANDIDATE_COUNT as u32);

		let inviter_account_id: T::AccountId = account("some inviter", 0, 0);
		let ticket_seed = 3;
		let (_ticket_public, ticket) = T::BenchmarkHelper::create_ticket(ticket_seed);
		PendingInvites::<T>::insert(inviter_account_id.clone(), ticket.clone(), ());

		let caller: T::AccountId = whitelisted_caller();
		let msg = caller.encode();
		let signature = T::BenchmarkHelper::sign(ticket_seed, &msg[..]);

		let tx_ext = AsProofOfInkParticipant::<T>::new(Some(
			AsProofOfInkParticipantInfo::AsInvited(0u32.into()),
		));
		let origin = SystemOrigin::Signed(caller);
		let call: T::RuntimeCall =
			Call::apply_with_invitation { inviter: inviter_account_id, ticket, signature }.into();
		let len = call.encode().len();

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call, &Default::default(), len, 0, |_| {
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	#[benchmark]
	fn register_successful_referral_reward() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::setup_currency();
		let referrer_value = reimbursement_value::<T>();
		let pot = Pallet::<T>::proof_of_ink_pot_id();
		assert_ok!(T::Currency::mint_into(&pot, referrer_value));

		register_families::<T>();
		register_reimbursement_values::<T>(referrer_value, referrer_value);

		let people = register_people::<T>(1);
		let referrer_pid: PersonalId = people.first().cloned().unwrap().1;

		// Manually give that person one pending referral reward.
		People::<T>::mutate(referrer_pid, |maybe_person| {
			let p = maybe_person.as_mut().expect("person must exist");
			p.pending_referral_rewards = 1;
		});

		let destination: T::AccountId = account("destination", 0, SEED);
		let origin = T::BenchmarkHelper::build_person_origin(referrer_pid);

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, destination);

		// The referrer should have no more pending rewards.
		assert_eq!(
			People::<T>::get(referrer_pid).expect("person exists").pending_referral_rewards,
			0
		);

		Ok(())
	}

	#[benchmark]
	fn grant_invites() -> Result<(), BenchmarkError> {
		let receiver: T::AccountId = account("receiver", 0, 0);
		let count: u32 = 5;
		let origin = T::InvitationsOrigin::try_successful_origin().unwrap();

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, receiver.clone(), count);

		// The receiver of invites successfully has them assigned to him
		let available_invites = AvailableInvites::<T>::get(&receiver);
		assert_eq!(available_invites, count, "Receiver should have the given number of invites");

		Ok(())
	}

	#[benchmark]
	fn remove_available_and_pending_invites(
		n: Linear<1, BENCHMARKING_UPPER_LIMIT>,
	) -> Result<(), BenchmarkError> {
		// The concerned account has a few available and pending invites
		let account_with_invites: T::AccountId = account("acc", 0, 0);
		AvailableInvites::<T>::insert(&account_with_invites, 10);
		for i in 0..n {
			let (_, ticket) = T::BenchmarkHelper::create_ticket(i as u64);
			PendingInvites::<T>::insert(&account_with_invites, ticket, ());
		}
		let origin = T::InvitationsOrigin::try_successful_origin().unwrap();

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, account_with_invites.clone(), n);

		// The concerned account no more has any available nor pending invites
		assert!(
			!AvailableInvites::<T>::contains_key(&account_with_invites),
			"Available invites should be removed"
		);

		assert_eq!(
			PendingInvites::<T>::iter_prefix(&account_with_invites).count(),
			0,
			"All pending invites should be removed"
		);

		Ok(())
	}

	#[benchmark]
	fn set_invite_ticket() -> Result<(), BenchmarkError> {
		// The caller is a signed origin with a few available tickets
		let caller: T::AccountId = whitelisted_caller();

		let initial_invites: u32 = 5;
		AvailableInvites::<T>::insert(&caller, initial_invites);

		let (_, ticket) = T::BenchmarkHelper::create_ticket(1);

		#[extrinsic_call]
		_(SystemOrigin::Signed(caller.clone()), ticket.clone());

		// The ticket is added to pending invites of the caller
		assert!(
			PendingInvites::<T>::contains_key(&caller, &ticket),
			"Ticket should be added to pending invites"
		);

		// The caller has less available invites now
		let remaining_invites = AvailableInvites::<T>::get(&caller);
		assert_eq!(
			remaining_invites,
			initial_invites - 1,
			"Available invites should be decremented"
		);

		Ok(())
	}

	#[benchmark]
	fn cancel_invite_ticket() -> Result<(), BenchmarkError> {
		// The caller is a signed origin with one pending invite
		let caller: T::AccountId = whitelisted_caller();
		let (_, ticket) = T::BenchmarkHelper::create_ticket(1);
		PendingInvites::<T>::insert(caller.clone(), ticket.clone(), ());

		#[extrinsic_call]
		_(SystemOrigin::Signed(caller.clone()), ticket.clone());

		// The ticket is removed from pending tickets of the caller
		assert!(PendingInvites::<T>::get(&caller, &ticket).is_none());

		// The number of available tickets of the caller is increased
		assert_eq!(AvailableInvites::<T>::get(&caller), 1);

		Ok(())
	}

	#[benchmark]
	fn set_configuration() -> Result<(), BenchmarkError> {
		let new_cfg = ConfigRecord {
			reroll_timeout: 42u32.into(),
			fasttrack_count: 7,
			maximum: 2_000,
			full_alloc_len: 128 * 1024,
			full_alloc_count: 8,
			init_alloc_len: 8 * 1024,
			init_alloc_count: 4,
			timeout: 84u32.into(),
		};

		let origin =
			T::ManagerOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, new_cfg.clone());

		assert_eq!(Configuration::<T>::get(), new_cfg);

		Ok(())
	}

	#[benchmark]
	fn apply_with_invitation() -> Result<(), BenchmarkError> {
		register_families::<T>();
		let people = register_people::<T>(PEOPLE_COUNT as u32);
		register_candidates::<T>(CANDIDATE_COUNT as u32);

		let inviter_account_id: T::AccountId = account("some inviter", 0, 0);
		let ticket_seed = 3;
		let (_ticket_public, ticket) = T::BenchmarkHelper::create_ticket(ticket_seed);
		PendingInvites::<T>::insert(inviter_account_id.clone(), ticket.clone(), ());

		let caller: T::AccountId = whitelisted_caller();
		let msg = caller.encode();
		let signature = T::BenchmarkHelper::sign(ticket_seed, &msg[..]);

		frame_system::Pallet::<T>::inc_providers(&caller);

		assert!(!Candidates::<T>::contains_key(&caller));

		#[extrinsic_call]
		_(Origin::InvitedCandidate(caller.clone()), inviter_account_id, ticket, signature);

		assert!(Candidates::<T>::contains_key(&caller));

		Ok(())
	}

	#[benchmark]
	fn as_apply_with_sig_tx_ext() -> Result<(), BenchmarkError> {
		register_families::<T>();

		let people = register_people::<T>(1);
		let referrer_pid: PersonalId = people.first().cloned().unwrap().1;

		let tickets = T::BenchmarkHelper::create_tickets(referrer_pid as u64);
		let ticket = tickets[0].clone();
		ReferralTickets::<T>::insert(referrer_pid, tickets);

		let caller: T::AccountId = whitelisted_caller();
		let msg = caller.encode();
		let signature = T::BenchmarkHelper::sign(referrer_pid as u64, &msg[..]);

		let tx_ext = AsProofOfInkParticipant::<T>::new(Some(
			AsProofOfInkParticipantInfo::AsApplyWithSig(0u32.into()),
		));

		let call: T::RuntimeCall =
			Call::apply_with_signature { referrer: referrer_pid, signature, ticket: ticket.ticket }
				.into();

		let origin = SystemOrigin::Signed(caller.clone());
		let len = call.encode().len();

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call, &Default::default(), len, 0, |_| {
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	#[benchmark]
	fn as_referred_tx_ext() -> Result<(), BenchmarkError> {
		register_families::<T>();
		register_people::<T>(PEOPLE_COUNT as u32);
		register_candidates::<T>(CANDIDATE_COUNT as u32);

		let caller: T::AccountId = whitelisted_caller();
		let referrer: PersonalId = 0u64;
		let referrals = <People<T>>::get(referrer).unwrap().referrals;

		let tickets = T::BenchmarkHelper::create_tickets(referrer);
		let ticket = tickets[0].clone();
		<ReferralTickets<T>>::insert(referrer, tickets);

		let msg = caller.encode();
		let signature = T::BenchmarkHelper::sign(referrer, &msg[..]);
		Pallet::<T>::apply_with_signature(
			Origin::AuthorizedApplyWithSig(caller.clone()).into(),
			referrer,
			signature,
			ticket.ticket,
		)
		.unwrap();

		let tx_ext = AsProofOfInkParticipant::<T>::new(Some(
			AsProofOfInkParticipantInfo::AsReferred(0u32.into()),
		));
		let call: T::RuntimeCall = Call::flakeout {}.into();
		let origin = SystemOrigin::Signed(caller.clone());
		let len = call.encode().len();

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call, &Default::default(), len, 0, |_| {
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	#[benchmark]
	fn set_reimbursement_values(
		c: Linear<1, { T::MaxReimbursementValues::get() }>,
	) -> Result<(), BenchmarkError> {
		let mut values = Vec::new();
		for i in 0..c {
			values.push((1u32.into(), 1));
		}
		let values: BoundedVec<(BalanceOf<T>, u32), _> =
			values.try_into().expect("values must be able to fit in bounded vec");

		let origin =
			T::ManagerOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;

		#[block]
		{
			assert_ok!(Pallet::<T>::set_reimbursement_values(
				origin,
				values.clone(),
				values.clone()
			));
		}

		assert_eq!(ReferrerReimbursementValues::<T>::get(), Some(values.clone()));
		assert_eq!(ReferredReimbursementValues::<T>::get(), Some(values));

		Ok(())
	}

	// Implements a test for each benchmark. Execute with:
	// `cargo test -p indiv-pallet-proof-of-ink --features runtime-benchmarks`.
	impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
