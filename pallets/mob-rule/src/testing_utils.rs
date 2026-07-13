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

//! Mob Rule pallet testing utilities.

#[cfg(any(test, feature = "runtime-benchmarks"))]
pub mod helpers {
	use crate::{testing_utils::constants::DEFAULT_POT_AMOUNT, *};
	use frame_support::{assert_ok, traits::fungible::Mutate};
	use indiv_support::traits::{InkSpec, Judgement, Statement, Truth};

	/// Creates a voting case for testing purposes.
	pub fn create_voting_case<T: Config>() -> CaseIndex {
		let judge_statement_result = Pallet::<T>::judge_statement(
			Statement::ProofOfInk {
				design: InkSpec::DesignedElective(0, 0),
				evidence: [0; 32],
				probable_acceptable: true,
			},
			Default::default(),
			Callback::from_parts(0, 0),
		);
		assert_ok!(judge_statement_result);
		judge_statement_result.unwrap()
	}

	/// Funds the mob rule pot with a specified amount.
	pub fn fund_pot<T: Config>() {
		let pot = Pallet::<T>::mob_rule_pot_id();
		let amount: BalanceOf<T> = DEFAULT_POT_AMOUNT.into();
		let _ = T::Currency::mint_into(&pot, amount);
	}

	/// Creates a ripe case by creating a voting case and moving it to ripe state.
	pub fn create_ripe_case<T: Config>() -> CaseIndex {
		let case_index = create_voting_case::<T>();

		let open_case = OpenCases::<T>::take(case_index).expect("Case should exist");
		let ripe_case =
			RipeCase { details: open_case.details, verdict: Judgement::Truth(Truth::True) };
		RipeCases::<T>::insert(case_index, ripe_case);

		case_index
	}

	/// Creates a done case with specified voters and timing.
	///
	/// # Parameters
	/// - `voters`: List of voter aliases to add votes for (can be empty)
	/// - `done_since`: Timestamp when the case was completed
	///
	/// # Returns
	/// - `CaseIndex`: The case index
	pub fn create_done_case<T: Config>(voters: Vec<Alias>, done_since: u64) -> CaseIndex {
		let case_index = create_voting_case::<T>();

		for &voter in &voters {
			Votes::<T>::insert(case_index, voter, Judgement::Truth(Truth::True));
		}

		OpenCases::<T>::remove(case_index);

		let done_case = DoneCase { since: done_since, verdict: Judgement::Truth(Truth::True) };
		DoneCases::<T>::insert(case_index, done_case);

		case_index
	}
}

#[cfg(any(test, feature = "runtime-benchmarks"))]
pub mod constants {
	use indiv_support::traits::Alias;

	/// Two days in milliseconds
	#[cfg(test)]
	pub const TWO_DAYS_MS: u64 = 2 * 24 * 60 * 60 * 1000;

	/// Two weeks in milliseconds
	#[cfg(test)]
	pub const TWO_WEEKS_MS: u64 = 14 * 24 * 60 * 60 * 1000;

	/// One hour in milliseconds
	#[cfg(test)]
	pub const ONE_HOUR_MS: u64 = 60 * 60 * 1000;

	/// Default pot funding amount
	pub const DEFAULT_POT_AMOUNT: u32 = 100_000_000;

	/// Person 0 alias
	pub const PERSON_0_ALIAS: Alias = [0u8; 32];
}
