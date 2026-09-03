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

//! Proof-of-Personhood system.

#![cfg_attr(not(feature = "std"), no_std)]
#![recursion_limit = "128"]

extern crate alloc;
use alloc::vec::Vec;

use indiv_support::{
	traits::{
		Alias, Callback, Context, Judgement, JudgementContext, Statement, StatementOracle, Truth,
	},
	tx_priority,
	weight_budget::OcwWeightBudget,
};

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
pub mod runtime_api;
#[cfg(any(test, feature = "runtime-benchmarks"))]
mod testing_utils;
pub mod weights;

pub use pallet::*;
pub use weights::WeightInfo;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::{
		dispatch::{extract_actual_weight, GetDispatchInfo, PostDispatchInfo, RawOrigin},
		pallet_prelude::*,
		traits::{
			fungible::{Inspect, Mutate, MutateHold},
			tokens::{Precision, Preservation},
			EnsureOriginWithArg, UnixTime,
		},
		weights::WeightMeter,
		PalletId,
	};
	use frame_system::{
		offchain::{CreateAuthorizedTransaction, SubmitTransaction},
		pallet_prelude::*,
	};
	use indiv_support::traits::CountedMembers;
	use sp_arithmetic::traits::{SaturatedConversion, Saturating};
	use sp_runtime::{
		traits::{AccountIdConversion, Dispatchable, Zero},
		Perbill, Percent,
	};
	use xcm::v5::Location;

	pub const MOB_CONTEXT: Context = *b"pop:polkadot.network/mob-rule   ";
	const LOG_TARGET: &str = "runtime::mob-rule";

	pub(crate) type BalanceOf<T> =
		<<T as Config>::Currency as Inspect<<T as frame_system::Config>::AccountId>>::Balance;

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config:
		CreateAuthorizedTransaction<Call<Self>>
		+ frame_system::Config<RuntimeCall: Dispatchable<PostInfo = PostDispatchInfo>>
	{
		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;

		/// Currency used for reward payout.
		type Currency: Inspect<Self::AccountId>
			+ Mutate<Self::AccountId>
			+ MutateHold<Self::AccountId, Reason: From<HoldReason>>;

		/// The location of the used currency. It is informational only.
		#[pallet::constant]
		type CurrencyLocationInfo: Get<Location>;

		/// The source of time.
		type Clock: UnixTime;

		/// How to recognize a person.
		type EnsurePerson: EnsureOriginWithArg<OriginFor<Self>, Context, Success = Alias>
			+ CountedMembers;

		/// The number of seconds before a case times out and can be reaped.
		#[pallet::constant]
		type MaxVoteClaimDuration: Get<u64>;

		/// The number of seconds a case must stay open for voting before it can be closed.
		#[pallet::constant]
		type MinCaseDuration: Get<u32>;

		/// The maximum number of seconds for which a case could stay open for voting.
		#[pallet::constant]
		type MaxVotingDuration: Get<u32>;

		/// The minimum number of votes a case must receive before a verdict can be reached.
		#[pallet::constant]
		type MinTurnoutNominal: Get<u32>;

		/// The minimum number of votes as a percentage of the total voter count a case must receive
		/// before a verdict can be reached.
		#[pallet::constant]
		type MinTurnoutPercentage: Get<Percent>;

		/// The maximum number of payout round schedules that can be recorded.
		type MaxPayoutRoundSchedules: Get<u32>;

		/// The number of blocks that a penalty is applied for.
		#[pallet::constant]
		type VotingPenaltyDuration: Get<BlockNumberFor<Self>>;

		/// The origin which is able to intervene and force a case to a particular verdict.
		type InterventionOrigin: EnsureOrigin<Self::RuntimeOrigin>;

		/// Account Identifier from which the internal Pot is generated.
		type PotId: Get<PalletId>;

		/// The maximum number of votes claimable in a single extrinsic.
		#[pallet::constant]
		type MaxVotesClaimable: Get<u32>;

		/// The interval at which offchain worker runs.
		#[pallet::constant]
		type OffchainWorkInterval: Get<BlockNumberFor<Self>>;

		/// Maximum number of votes that will be cleaned during offchain worker run.
		#[pallet::constant]
		type CleanVotesBatchSize: Get<u32>;

		/// The time in seconds during which votes can be claimed on done cases.
		/// Measured from the time the case becomes 'Done'.
		#[pallet::constant]
		type VotesOpenForClaimsDuration: Get<u32>;

		/// The minimum number of active voters required to allow voting. When this threshold is not
		/// met, open cases cannot time out.
		#[pallet::constant]
		type MinimumVoterThreshold: Get<u32>;

		/// Benchmark helper for creating test data.
		#[cfg(feature = "runtime-benchmarks")]
		type BenchmarkHelper: crate::benchmarking::BenchmarkHelper<Self>;
	}

	/// A reason for the pallet placing a hold on funds.
	///
	/// The pot account of the pallet will be funded by the treasury periodically (e.g. once every 6
	/// months) but less frequently than payout rounds will being initiated (e.g. once every 2
	/// weeks). At any given point in time, we expect to have a lot more funds in the pot account
	/// than we need for a given round. Funds made available for voters during a payout round are
	/// held for this `Payout` reason. When a voter claims funds from there, they are not yet theirs
	/// and remain in the pot under the `Credit` hold until they trigger reward payout.
	#[pallet::composite_enum]
	pub enum HoldReason {
		/// The Pallet has reserved it for the current credit payout round.
		Payout,
		/// The Pallet has reserved it for storing users' credit until payout transfer is
		/// triggered.
		Credit,
	}

	#[derive(PartialEq, Eq, Clone, Default, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
	pub struct MobCredit<Balance> {
		/// Number of times a vote was cast.
		pub voted: u32,
		/// Number of times the vote was cleaned afterwards.
		pub cleaned: u32,
		/// Number of times the vote agreed with the aggregate verdict.
		pub correct: u32,
		/// Accumulated credit that can be paid out to an account.
		pub credit: Balance,
	}

	#[derive(PartialEq, Eq, Clone, Default, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
	pub struct VoteTally {
		/// Number of "aye" votes.
		pub aye: u32,
		/// Number of "nay" votes.
		pub nay: u32,

		/// Number of voters that believe the case is contemptuous.
		pub contempt: u32,
	}

	pub struct VoteCountResult {
		/// The verdict the current tally would yield.
		pub verdict: Judgement,
		/// The weight of "aye" votes.
		pub aye_weight: u32,
		/// The weight of "nay" votes.
		pub nay_weight: u32,
		/// The turnout at the moment when the votes were counted.
		pub turnout: Percent,
		/// Total number of votes cast.
		pub voter_count: u32,
		/// Flag that is set if the verdict cannot be flipped by remaining voters.
		pub definitive: bool,
	}

	impl VoteCountResult {
		/// Result of empty vote tally.
		fn empty() -> Self {
			Self {
				verdict: Judgement::Truth(Truth::False),
				aye_weight: 0,
				nay_weight: 0,
				turnout: Percent::from_percent(0),
				voter_count: 0,
				definitive: false,
			}
		}

		/// Create a new `VoteCountResult`.
		fn new(
			verdict: Judgement,
			aye_weight: u32,
			nay_weight: u32,
			turnout: Percent,
			voter_count: u32,
			definitive: bool,
		) -> Self {
			Self { verdict, aye_weight, nay_weight, turnout, voter_count, definitive }
		}

		/// Return whether the current vote is passing at a specific point in time in the case's
		/// lifetime. The required approval falls linearly with time from 100%. After the maximum
		/// duration of the case has passed, the case will pass if there is a simple majority of
		/// 50%.
		pub fn is_passing(&self, max_duration: u64, elapsed: u64) -> bool {
			let remaining = max_duration.saturating_sub(elapsed);
			let total_vote_weight: u64 = self.aye_weight.saturating_add(self.nay_weight).into();
			// The vote is passing if
			// approval > 1 - passing_threshold * ((max_duration - time_remaining) / max_duration)
			// where
			// approval = aye / total_vote_weight
			// passing_threshold = 50% or 1/2
			max_duration.saturating_mul(self.aye_weight.into()).saturating_mul(2) >
				max_duration.saturating_add(remaining).saturating_mul(total_vote_weight)
		}
	}

	impl VoteTally {
		pub fn count(&mut self, judgement: &Judgement) {
			use indiv_support::traits::{Judgement::*, Truth::*};
			match judgement {
				Truth(True) => self.aye.saturating_inc(),
				Truth(False) => self.nay.saturating_inc(),
				Contempt => self.contempt.saturating_inc(),
			}
		}

		pub fn discount(&mut self, judgement: Judgement) {
			use indiv_support::traits::{Judgement::*, Truth::*};
			match judgement {
				Truth(True) => self.aye.saturating_dec(),
				Truth(False) => self.nay.saturating_dec(),
				Contempt => self.contempt.saturating_dec(),
			}
		}
	}

	pub type Dissent = Percent;

	impl VoteTally {
		/// Count the votes and return the vote tally result.
		fn collapsed(&self, active_voters: u32) -> VoteCountResult {
			use indiv_support::traits::{Judgement::*, Truth::*};
			let vote_count = self.aye.saturating_add(self.nay).saturating_add(self.contempt);
			if vote_count == 0 {
				return VoteCountResult::empty();
			}

			// Contempt votes have the same weight as "nay" votes.
			let nay_weight = self.nay.saturating_add(self.contempt);

			let remaining_voters = active_voters.saturating_sub(vote_count);
			let mut definitive = remaining_voters == 0;
			let verdict = if self.aye > nay_weight {
				definitive |= self.aye.saturating_sub(nay_weight) > remaining_voters;
				Truth(True)
			} else {
				definitive |= nay_weight.saturating_sub(self.aye) > remaining_voters;
				// The case is contemptuous if the vote fails and there are more `contempt` votes
				// than `nay`s.
				if self.contempt >= self.nay {
					Contempt
				} else {
					Truth(False)
				}
			};
			VoteCountResult::new(
				verdict,
				self.aye,
				nay_weight,
				Percent::from_rational(vote_count, active_voters),
				vote_count,
				definitive,
			)
		}
	}

	pub type SecsSinceGenesis = u64;

	#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
	pub struct CaseDetails<RuntimeCall> {
		pub(crate) statement: Statement,
		pub(crate) context: JudgementContext,
		pub(crate) callback: Callback<(CaseIndex, JudgementContext, Judgement), RuntimeCall>,
	}

	#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
	pub struct OpenCase<RuntimeCall> {
		pub(crate) since: SecsSinceGenesis,
		pub(crate) details: CaseDetails<RuntimeCall>,
		tally: VoteTally,
	}

	#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
	pub struct RipeCase<RuntimeCall> {
		pub(crate) details: CaseDetails<RuntimeCall>,
		pub(crate) verdict: Judgement,
	}

	#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
	pub struct DoneCase {
		pub(crate) since: SecsSinceGenesis,
		pub(crate) verdict: Judgement,
	}

	/// The mob-rule extrinsic call that attempted to dispatch the callback.
	#[derive(
		PartialEq,
		Eq,
		Clone,
		Copy,
		Encode,
		Decode,
		Debug,
		TypeInfo,
		MaxEncodedLen,
		DecodeWithMemTracking,
	)]
	pub enum CallbackTrigger {
		/// The callback was triggered while executing the `close_case` extrinsic.
		CloseCase,
		/// The callback was triggered while executing the `intervene` extrinsic.
		Intervene,
	}

	pub type CaseIndex = u32;
	pub type RoundIndex = u32;
	pub type VoteCount = u64;

	/// Describes the current reward payout round parameters. The point/reward exchange rate will be
	/// determined by the pot administrator when they decide to start a new round, with a particular
	/// reward amount `initial_balance` which can be claimed against `total_points`.
	#[derive(PartialEq, Eq, Clone, Default, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
	pub struct CreditDistribution<Balance, BlockNumber> {
		/// The index of the payout round.
		pub round: RoundIndex,
		/// The funds that were used to fund this round.
		pub initial_balance: Balance,
		/// The remaining funds that are used to fund this round.
		pub remaining_balance: Balance,
		/// The total amount of points which can be claimed this round, accumulated by users before
		/// this current round was started.
		pub total_points: VoteCount,
		/// The block in which the current distribution started.
		pub start: BlockNumber,
	}

	/// Describes the current reward payout round parameters.
	#[derive(PartialEq, Eq, Clone, Default, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
	pub struct PayoutSchedule<Balance, BlockNumber> {
		/// The number of payout rounds remaining.
		pub remaining: RoundIndex,
		/// The funds used to fund each round.
		pub amount_per_round: Balance,
		/// The minimum number of blocks needed to pass between each round.
		pub period: BlockNumber,
	}

	/// The voting records of all aliases.
	#[pallet::storage]
	pub type Credits<T> =
		StorageMap<_, Blake2_128Concat, Alias, MobCredit<BalanceOf<T>>, ValueQuery>;

	/// The vote penalties of users and their starting moment. Voters with penalties cannot vote in
	/// contempt until the penalty expires and is removed.
	#[pallet::storage]
	pub type VotingPenalties<T> = StorageMap<_, Blake2_128Concat, Alias, BlockNumberFor<T>>;

	/// The accumulated points of all aliases, during each funding round. The points accumulated
	/// during round `N` can be redeemed for rewards only during round `N + 1`.
	#[pallet::storage]
	pub type VotingPoints<T> =
		StorageDoubleMap<_, Identity, RoundIndex, Blake2_128Concat, Alias, u32, ValueQuery>;

	/// All votes cast, segregated by case and voter.
	#[pallet::storage]
	pub type Votes<T> = StorageDoubleMap<
		_,
		Blake2_128Concat,
		CaseIndex,
		Blake2_128Concat,
		Alias,
		Judgement,
		OptionQuery,
	>;

	/// The number of cases, open or recently closed, stored in the pallet.
	#[pallet::storage]
	pub type CaseCount<T> = StorageValue<_, CaseIndex, ValueQuery>;

	/// The open cases recorded in the pallet. These cases can still be voted on.
	#[pallet::storage]
	pub type OpenCases<T> = StorageMap<
		_,
		Blake2_128Concat,
		CaseIndex,
		OpenCase<<T as frame_system::Config>::RuntimeCall>,
		OptionQuery,
	>;

	/// The ripe cases recorded in the pallet. The voting has concluded on these cases and a verdict
	/// has been reached, but they cannot be removed yet in order to allow people to claim their
	/// votes.
	#[pallet::storage]
	pub type RipeCases<T> = StorageMap<
		_,
		Blake2_128Concat,
		CaseIndex,
		RipeCase<<T as frame_system::Config>::RuntimeCall>,
		OptionQuery,
	>;

	/// The done cases recorded in the pallet. These cases are obsolete and can be removed from
	/// storage.
	#[pallet::storage]
	pub type DoneCases<T> = StorageMap<_, Blake2_128Concat, CaseIndex, DoneCase, OptionQuery>;

	/// The number of points accumulated through correct votes during the current payout round. This
	/// amount is going to determine the point/reward exchange rate of the next reward payout round.
	#[pallet::storage]
	pub type AccumulatedPoints<T> = StorageValue<_, VoteCount, ValueQuery>;

	/// Describes the parameters of the current payout round.
	#[pallet::storage]
	pub type PayoutDistribution<T> =
		StorageValue<_, CreditDistribution<BalanceOf<T>, BlockNumberFor<T>>, OptionQuery>;

	/// Describes the schedules of the payout rounds.
	#[pallet::storage]
	pub type RoundSchedules<T> = StorageValue<
		_,
		BoundedVec<
			PayoutSchedule<BalanceOf<T>, BlockNumberFor<T>>,
			<T as Config>::MaxPayoutRoundSchedules,
		>,
		ValueQuery,
	>;

	/// The timestamp, in seconds, of the moment enough active voters were detected and voting was
	/// enabled. This value will not be present if there are too few active voters.
	#[pallet::storage]
	pub type ActiveSince<T> = StorageValue<_, SecsSinceGenesis, OptionQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// A case has been created.
		CaseCreated {
			/// The case index that was created.
			case_index: CaseIndex,
		},
		/// A callback was triggered from mob-rule.
		Callback {
			/// The result of the callback.
			result: DispatchResult,
		},
		/// There was a codec error when trying to decode the callback call.
		CallbackError {
			/// The case whose callback failed to decode.
			case_index: CaseIndex,
			/// The mob-rule extrinsic call which attempted to trigger the callback.
			trigger: CallbackTrigger,
		},
		/// The case has been closed with the following result.
		CaseClosed {
			/// The case index that was closed.
			case_index: CaseIndex,
			/// The verdict of the closed case.
			verdict: Judgement,
		},
		/// A vote has been placed on a case.
		Voted {
			/// The case voted on.
			case_index: CaseIndex,
			/// The alias that voted.
			voter: Alias,
			/// The opinion of the voter.
			opinion: Judgement,
		},
		/// A vote has been cleaned.
		VoteCleaned {
			/// The case voted on.
			case_index: CaseIndex,
			/// The alias that voted.
			voter: Alias,
		},
		/// A case has been removed.
		CaseRemoved {
			/// The case voted on.
			case_index: CaseIndex,
		},
		/// A case has been intervened.
		CaseIntervened {
			/// The case that has been intervened.
			case_index: CaseIndex,
			/// The verdict applied to the case.
			verdict: Judgement,
		},
		/// Credits for votes have been claimed.
		VotesClaimed {
			/// The alias that voted.
			voter: Alias,
			/// The case that has been intervened.
			case_indices: Vec<CaseIndex>,
		},
		/// A reward that has been paid out.
		RewardPayout {
			/// The alias that earned a reward.
			voter: Alias,
			/// The destination account for the payout.
			destination: T::AccountId,
			/// The payout amount that was transferred.
			amount: BalanceOf<T>,
		},
		/// A new payout round has been started.
		PayoutRoundStarted {
			/// The index of the new payout round.
			round: RoundIndex,
			/// The initial balance allocated for the round.
			initial_balance: BalanceOf<T>,
			/// Total accumulated points claimable in this round.
			total_points: VoteCount,
		},
		/// Payout rounds have been scheduled.
		PayoutRoundsScheduled {
			/// The amount of funds per round.
			amount: BalanceOf<T>,
			/// The number of rounds scheduled.
			count: u32,
			/// The minimum period (in blocks) between rounds.
			period: BlockNumberFor<T>,
		},
		/// A payout schedule has been removed.
		PayoutScheduleRemoved {
			/// The index of the removed schedule.
			index: u32,
		},
		/// Credit has been claimed from the payout distribution.
		CreditClaimed {
			/// The alias of the voter who claimed.
			voter: Alias,
			/// The amount of credit claimed.
			amount: BalanceOf<T>,
		},
		/// Stale voting points have been cleaned.
		PointsCleaned {
			/// The round index from which points were removed.
			round: RoundIndex,
			/// The alias whose points were removed.
			voter: Alias,
		},
		/// A case has been touched (re-evaluated).
		CaseTouched {
			/// The case that was touched.
			case_index: CaseIndex,
			/// Whether the case was ripened as a result.
			ripened: bool,
		},
		/// A voting penalty has been cleared.
		VotingPenaltyCleared {
			/// The alias whose penalty was cleared.
			who: Alias,
		},
	}

	#[pallet::error]
	pub enum Error<T> {
		/// The case does not exist.
		NoSuchCase,
		/// The vote does not exist.
		NoSuchVote,
		/// The case is not open.
		NotOpen,
		/// The case is not ripe.
		NotRipe,
		/// The case is not yet done.
		NotDone,
		/// The decode of the call failed. Maybe there was a breaking runtime upgrade in between?
		CodecError,
		/// The call failed to dispatch. Maybe there was a breaking runtime upgrade in between?
		DispatchError,
		/// The case is too recent to be reaped.
		Recent,
		/// Not enough credit to payout the rewards.
		NoCredit,
		/// No mob credit distribution in place to reward voters.
		NoReward,
		/// No points to be converted to mob credit.
		NoPoints,
		/// Too many vote claims.
		TooManyClaims,
		/// No payout in progress.
		NoPayout,
		/// The point and/or credit arithmetic overflows.
		ArithmeticOverflow,
		/// Too many payout round schedules.
		TooManySchedules,
		/// No payout round schedule found.
		NoSchedule,
		/// No vote penalty found.
		NoPenalty,
		/// The vote penalty has not expired yet.
		Early,
		/// The vote cannot be cast due to a voting penalty in effect.
		UnderPenalty,
		/// The open case expiration is disabled due to insufficient active voters.
		CaseExpirationDisabled,
		/// The dispatched callback's weight exceeds the declared `max_callback_weight` upper
		/// bound.
		CallbackWeightTooLow,
	}

	/// Custom transaction-validity errors reported from `authorize` entry points.
	#[repr(u8)]
	pub enum CustomInvalidity {
		/// The `max_callback_weight` declared by a `close_case` transaction does not cover the
		/// actual weight of the case's callback.
		CallbackWeightTooLow = 55,
		/// The case is not ripe.
		CaseNotRipe = 56,
		/// The case is not done.
		CaseNotDone = 57,
		/// The case has not reached its configured time limit.
		CaseTooRecent = 58,
		/// The case is not open.
		CaseNotOpen = 59,
		/// Case expiration is disabled because there are too few active voters.
		CaseExpirationDisabled = 60,
	}

	impl From<CustomInvalidity> for TransactionValidityError {
		fn from(e: CustomInvalidity) -> Self {
			InvalidTransaction::Custom(e as u8).into()
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Feeless on success (determined only by top three lines).
		#[pallet::weight(T::WeightInfo::vote())]
		#[pallet::call_index(0)]
		pub fn vote(
			origin: OriginFor<T>,
			case_index: CaseIndex,
			opinion: Judgement,
		) -> DispatchResultWithPostInfo {
			let alias = T::EnsurePerson::ensure_origin(origin, &MOB_CONTEXT)?;
			ensure!(
				!matches!(opinion, Judgement::Contempt) ||
					!VotingPenalties::<T>::contains_key(alias),
				Error::<T>::UnderPenalty
			);
			let mut case = OpenCases::<T>::take(case_index).ok_or(Error::<T>::NotOpen)?;
			let mut credit = Credits::<T>::get(alias);
			let (_first_time, pays) = match Votes::<T>::get(case_index, alias) {
				Some(old_opinion) => {
					case.tally.discount(old_opinion);
					(false, Pays::Yes)
				},
				None => {
					credit.voted.saturating_inc();
					(true, Pays::No)
				},
			};
			case.tally.count(&opinion);
			Votes::<T>::insert(case_index, alias, opinion);

			let secs = T::Clock::now().as_secs().saturating_sub(case.since);

			let active_voters = T::EnsurePerson::active_count();
			let vote_count_result = case.tally.collapsed(active_voters);

			let minimum_turnout_reached = vote_count_result.turnout >=
				T::MinTurnoutPercentage::get() &&
				vote_count_result.voter_count >= T::MinTurnoutNominal::get();
			let minimum_duration_elapsed = secs > T::MinCaseDuration::get().into();

			if minimum_turnout_reached &&
				minimum_duration_elapsed &&
				(vote_count_result.definitive ||
					vote_count_result.is_passing(T::MaxVotingDuration::get().into(), secs))
			{
				let ripe_case =
					RipeCase { details: case.details, verdict: vote_count_result.verdict };
				RipeCases::<T>::insert(case_index, ripe_case);
			} else {
				OpenCases::<T>::insert(case_index, case);
			}

			Credits::<T>::insert(alias, credit);

			Self::deposit_event(Event::Voted { case_index, voter: alias, opinion });
			Ok(pays.into())
		}

		/// `max_callback_weight` is a caller-declared upper bound on the weight of the callback
		/// dispatched when the case is closed. It is included in this call's declared weight and
		/// checked against the actual callback weight on-chain, with the difference refunded. The
		/// call fails with `CallbackWeightTooLow` if the bound does not cover the callback.
		#[pallet::authorize(Pallet::<T>::authorize_close_case)]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_close_case())]
		#[pallet::weight(T::WeightInfo::close_case().saturating_add(*max_callback_weight))]
		#[pallet::call_index(1)]
		pub fn close_case(
			origin: OriginFor<T>,
			case_index: CaseIndex,
			max_callback_weight: Weight,
		) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;

			let case = Self::validate_close_case(case_index)?;
			RipeCases::<T>::remove(case_index);

			let callback_weight = Self::dispatch_callback(
				case_index,
				case.details.callback,
				case.details.context,
				case.verdict,
				CallbackTrigger::CloseCase,
				max_callback_weight,
			)?;

			let done_case = DoneCase { since: T::Clock::now().as_secs(), verdict: case.verdict };
			DoneCases::<T>::insert(case_index, done_case);

			Self::deposit_event(Event::CaseClosed { case_index, verdict: case.verdict });
			Ok(Some(T::WeightInfo::close_case().saturating_add(callback_weight)).into())
		}

		/// Origin must be authorized. The transaction is authorized in `AuthorizeCall`
		/// when the source is local (e.g. from the offchain worker). For external transactions, use
		/// `clean_vote_signed`.
		#[pallet::authorize(Pallet::<T>::authorize_clean_vote)]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_clean_vote())]
		#[pallet::weight(T::WeightInfo::clean_vote())]
		#[pallet::call_index(2)]
		pub fn clean_vote(
			origin: OriginFor<T>,
			case_index: CaseIndex,
			voter: Alias,
		) -> DispatchResult {
			ensure_authorized(origin)?;
			Self::clean_vote_inner(case_index, voter)?;
			Ok(())
		}

		#[pallet::authorize(Pallet::<T>::authorize_reap_case)]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_reap_case())]
		#[pallet::weight(T::WeightInfo::reap_case())]
		#[pallet::call_index(3)]
		pub fn reap_case(
			origin: OriginFor<T>,
			case_index: CaseIndex,
		) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;
			Self::validate_reap_case(case_index)?;
			DoneCases::<T>::remove(case_index);
			Self::deposit_event(Event::CaseRemoved { case_index });
			Ok(().into())
		}

		/// `max_callback_weight` is a caller-declared upper bound on the weight of the callback
		/// dispatched when the case is intervened. It is included in this call's declared weight
		/// and checked against the actual callback weight on-chain, with the difference refunded.
		/// The call fails with `CallbackWeightTooLow` if the bound does not cover the callback.
		#[pallet::weight(T::WeightInfo::intervene().saturating_add(*max_callback_weight))]
		#[pallet::call_index(4)]
		pub fn intervene(
			origin: OriginFor<T>,
			case_index: CaseIndex,
			verdict: Judgement,
			max_callback_weight: Weight,
		) -> DispatchResultWithPostInfo {
			T::InterventionOrigin::ensure_origin_or_root(origin)?;
			let case = OpenCases::<T>::take(case_index).ok_or(Error::<T>::NotOpen)?;

			let callback_weight = Self::dispatch_callback(
				case_index,
				case.details.callback,
				case.details.context,
				verdict,
				CallbackTrigger::Intervene,
				max_callback_weight,
			)?;

			let done_case = DoneCase { since: T::Clock::now().as_secs(), verdict };
			DoneCases::<T>::insert(case_index, done_case);

			Self::deposit_event(Event::CaseIntervened { case_index, verdict });
			Ok((Some(T::WeightInfo::intervene().saturating_add(callback_weight)), Pays::No).into())
		}

		/// A person claims the mob credit associated with a correct vote on a case.
		/// The case must be `Done`.
		#[pallet::weight(T::WeightInfo::claim_vote())]
		#[pallet::call_index(5)]
		pub fn claim_vote(
			origin: OriginFor<T>,
			case_index: CaseIndex,
		) -> DispatchResultWithPostInfo {
			let voter = T::EnsurePerson::ensure_origin(origin, &MOB_CONTEXT)?;
			Self::do_claim_votes(&voter, alloc::vec![case_index])?;
			Ok(Pays::No.into())
		}

		/// A person converts their claimed mob credit into a direct transfer.
		#[pallet::weight(T::WeightInfo::payout_rewards())]
		#[pallet::call_index(6)]
		pub fn payout_rewards(
			origin: OriginFor<T>,
			destination: T::AccountId,
		) -> DispatchResultWithPostInfo {
			let voter = T::EnsurePerson::ensure_origin(origin, &MOB_CONTEXT)?;

			let mut mob_credit = Credits::<T>::get(voter);
			if mob_credit.credit.is_zero() {
				return Err(Error::<T>::NoCredit.into());
			}
			let payout_amount = mob_credit.credit;
			mob_credit.credit = Zero::zero();

			let pot = Self::mob_rule_pot_id();
			let released = T::Currency::release(
				&HoldReason::Credit.into(),
				&pot,
				payout_amount,
				Precision::Exact,
			)?;
			debug_assert_eq!(payout_amount, released);

			let transferred =
				T::Currency::transfer(&pot, &destination, released, Preservation::Expendable)?;
			debug_assert_eq!(released, transferred);

			Credits::<T>::insert(voter, &mob_credit);
			Self::deposit_event(Event::RewardPayout { voter, destination, amount: transferred });
			Ok(Pays::No.into())
		}

		/// A person claims multiple mob credits associated a correct vote on a case. The
		/// case must be `Done`.
		#[pallet::weight(T::WeightInfo::claim_votes(case_indices.len() as u32))]
		#[pallet::call_index(7)]
		pub fn claim_votes(
			origin: OriginFor<T>,
			case_indices: Vec<CaseIndex>,
		) -> DispatchResultWithPostInfo {
			let voter = T::EnsurePerson::ensure_origin(origin, &MOB_CONTEXT)?;
			Self::do_claim_votes(&voter, case_indices)?;
			Ok(Pays::No.into())
		}

		#[pallet::weight(T::WeightInfo::start_payout_round())]
		#[pallet::call_index(8)]
		pub fn start_payout_round(origin: OriginFor<T>) -> DispatchResultWithPostInfo {
			T::InterventionOrigin::ensure_origin_or_root(origin)?;
			let points = AccumulatedPoints::<T>::get();
			ensure!(points > 0, Error::<T>::NoPoints);
			let now = frame_system::Pallet::<T>::block_number();
			let mut round_schedules = RoundSchedules::<T>::get();
			let current_schedule = round_schedules.get_mut(0).ok_or(Error::<T>::NoSchedule)?;
			let current_distribution = PayoutDistribution::<T>::get();

			ensure!(
				current_distribution.as_ref().is_none_or(|distribution| distribution
					.start
					.saturating_add(current_schedule.period) <=
					now),
				Error::<T>::Recent
			);
			current_schedule.remaining.saturating_dec();

			// Recycle leftover funds from last round.
			if let Some(remaining_balance) =
				current_distribution.as_ref().map(|d| d.remaining_balance)
			{
				let pot_account = Self::mob_rule_pot_id();
				T::Currency::release(
					&HoldReason::Payout.into(),
					&pot_account,
					remaining_balance,
					Precision::BestEffort,
				)?;
			}

			let round = current_distribution
				.as_ref()
				.map(|d| d.round.saturating_add(1))
				.unwrap_or_default();
			let initial_balance = current_schedule.amount_per_round;
			let next_payout_distribution = CreditDistribution {
				round,
				initial_balance,
				remaining_balance: initial_balance,
				total_points: points,
				start: now,
			};
			PayoutDistribution::<T>::put(next_payout_distribution);
			// Reset the points for the next round.
			AccumulatedPoints::<T>::put(0);
			// Remove the schedule if it's empty.
			if current_schedule.remaining == 0 {
				// This will be a removal followed by a shift of all the remaining elements, but we
				// expect this list to be short.
				round_schedules.remove(0);
			}
			// Update the stored schedules.
			RoundSchedules::<T>::put(round_schedules);
			Self::deposit_event(Event::PayoutRoundStarted {
				round,
				initial_balance,
				total_points: points,
			});
			Ok(Pays::No.into())
		}

		#[pallet::weight(T::WeightInfo::schedule_payout_rounds())]
		#[pallet::call_index(9)]
		pub fn schedule_payout_rounds(
			origin: OriginFor<T>,
			amount: BalanceOf<T>,
			count: u32,
			period: BlockNumberFor<T>,
		) -> DispatchResultWithPostInfo {
			T::InterventionOrigin::ensure_origin_or_root(origin)?;
			let pot_account = Self::mob_rule_pot_id();
			// Funds to be paid out are held during the scheduled payout rounds. Any unclaimed funds
			// from last round will be recycled.
			T::Currency::hold(
				&HoldReason::Payout.into(),
				&pot_account,
				amount.saturating_mul(count.into()),
			)?;
			RoundSchedules::<T>::try_mutate(|schedules| {
				schedules
					.try_push(PayoutSchedule { remaining: count, amount_per_round: amount, period })
					.map_err(|_| Error::<T>::TooManySchedules)
			})?;
			Self::deposit_event(Event::PayoutRoundsScheduled { amount, count, period });
			Ok(Pays::No.into())
		}

		#[pallet::weight(T::WeightInfo::remove_payout_schedule())]
		#[pallet::call_index(10)]
		pub fn remove_payout_schedule(
			origin: OriginFor<T>,
			index: u32,
		) -> DispatchResultWithPostInfo {
			T::InterventionOrigin::ensure_origin_or_root(origin)?;
			let pot_account = Self::mob_rule_pot_id();
			let mut round_schedules = RoundSchedules::<T>::get();
			ensure!((index as usize) < round_schedules.len(), Error::<T>::NoSchedule);
			let schedule = round_schedules.remove(index as usize);
			let amount_to_release =
				schedule.amount_per_round.saturating_mul(schedule.remaining.into());
			T::Currency::release(
				&HoldReason::Payout.into(),
				&pot_account,
				amount_to_release,
				Precision::BestEffort,
			)?;
			RoundSchedules::<T>::put(round_schedules);
			Self::deposit_event(Event::PayoutScheduleRemoved { index });
			Ok(Pays::No.into())
		}

		#[pallet::weight(T::WeightInfo::claim_credit())]
		#[pallet::call_index(11)]
		pub fn claim_credit(origin: OriginFor<T>) -> DispatchResultWithPostInfo {
			let voter = T::EnsurePerson::ensure_origin(origin, &MOB_CONTEXT)?;
			let mut credit_distribution =
				PayoutDistribution::<T>::get().ok_or(Error::<T>::NoReward)?;
			let mut mob_credit = Credits::<T>::get(voter);
			let points = VotingPoints::<T>::take(credit_distribution.round, voter);
			ensure!(points > 0, Error::<T>::NoPoints);
			// Safe because we ensure no round can start with 0 points to be claimed.
			// (and total points ≥ points > 0)
			// NOTE: `from_rational` always rounds down as documented.
			let claimable_portion =
				Perbill::from_rational(u64::from(points), credit_distribution.total_points);
			let claimable = claimable_portion.mul_floor(credit_distribution.initial_balance);
			let pot = Self::mob_rule_pot_id();
			let actual_released = T::Currency::release(
				&HoldReason::Payout.into(),
				&pot,
				claimable,
				Precision::BestEffort,
			)?;
			credit_distribution.remaining_balance =
				credit_distribution.remaining_balance.saturating_sub(actual_released);
			debug_assert_eq!(claimable, actual_released);
			T::Currency::hold(&HoldReason::Credit.into(), &pot, actual_released)?;
			mob_credit.credit = mob_credit
				.credit
				.checked_add(&actual_released)
				.ok_or(Error::<T>::ArithmeticOverflow)?;
			Credits::<T>::insert(voter, mob_credit);
			PayoutDistribution::<T>::put(credit_distribution);
			Self::deposit_event(Event::CreditClaimed { voter, amount: actual_released });

			Ok(Pays::No.into())
		}

		#[pallet::weight(T::WeightInfo::clean_points())]
		#[pallet::call_index(12)]
		pub fn clean_points(
			origin: OriginFor<T>,
			round: RoundIndex,
			voter: Alias,
		) -> DispatchResultWithPostInfo {
			// It doesn't really matter who calls this but we want people to pay if this call fails.
			// In the future this will be a free call through the `authorized` interface.
			let _ = ensure_signed(origin)?;
			ensure!(
				round < PayoutDistribution::<T>::get().ok_or(Error::<T>::NoPayout)?.round,
				Error::<T>::NoPayout
			);
			ensure!(VotingPoints::<T>::contains_key(round, voter), Error::<T>::NoPoints);
			// This whole call should be a task where we call
			// `VotingPoints::<T>::clear_prefix(round)` but if the map is big it requires multiple
			// calls with the output of one fed into the next one until all the votes in a round are
			// removed.
			VotingPoints::<T>::remove(round, voter);
			Self::deposit_event(Event::PointsCleaned { round, voter });
			Ok(Pays::No.into())
		}

		#[pallet::authorize(Pallet::<T>::authorize_force_ripen_case)]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_force_ripen_case())]
		#[pallet::weight(T::WeightInfo::force_ripen_case())]
		#[pallet::call_index(13)]
		pub fn force_ripen_case(
			origin: OriginFor<T>,
			case_index: CaseIndex,
		) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;

			let case = Self::validate_timeout_case(case_index)?;
			OpenCases::<T>::remove(case_index);

			let active_voters = T::EnsurePerson::active_count();
			let vote_count_result = case.tally.collapsed(active_voters);

			let ripe_case = RipeCase { details: case.details, verdict: vote_count_result.verdict };
			RipeCases::<T>::insert(case_index, ripe_case);

			Self::deposit_event(Event::CaseClosed {
				case_index,
				verdict: vote_count_result.verdict,
			});
			Ok(().into())
		}

		#[pallet::weight(T::WeightInfo::touch_case())]
		#[pallet::call_index(14)]
		pub fn touch_case(
			origin: OriginFor<T>,
			case_index: CaseIndex,
		) -> DispatchResultWithPostInfo {
			let _ = ensure_signed(origin)?;
			let case = OpenCases::<T>::take(case_index).ok_or(Error::<T>::NotOpen)?;
			let secs = T::Clock::now().as_secs().saturating_sub(case.since);

			let active_voters = T::EnsurePerson::active_count();
			let vote_count_result = case.tally.collapsed(active_voters);

			let minimum_turnout_reached = vote_count_result.turnout >=
				T::MinTurnoutPercentage::get() &&
				vote_count_result.voter_count >= T::MinTurnoutNominal::get();
			let minimum_duration_elapsed = secs > T::MinCaseDuration::get().into();

			if minimum_turnout_reached &&
				minimum_duration_elapsed &&
				(vote_count_result.definitive ||
					vote_count_result.is_passing(T::MaxVotingDuration::get().into(), secs))
			{
				let ripe_case =
					RipeCase { details: case.details, verdict: vote_count_result.verdict };
				RipeCases::<T>::insert(case_index, ripe_case);
				Self::deposit_event(Event::CaseTouched { case_index, ripened: true });
				Ok(Pays::No.into())
			} else {
				OpenCases::<T>::insert(case_index, case);
				Self::deposit_event(Event::CaseTouched { case_index, ripened: false });
				Ok(Pays::Yes.into())
			}
		}

		#[pallet::weight(T::WeightInfo::clear_voting_penalty())]
		#[pallet::call_index(15)]
		pub fn clear_voting_penalty(origin: OriginFor<T>) -> DispatchResultWithPostInfo {
			let alias = T::EnsurePerson::ensure_origin(origin, &MOB_CONTEXT)?;
			let start = VotingPenalties::<T>::take(alias).ok_or(Error::<T>::NoPenalty)?;
			let now = frame_system::Pallet::<T>::block_number();
			ensure!(
				start.saturating_add(T::VotingPenaltyDuration::get()) <= now,
				Error::<T>::Early
			);
			Self::deposit_event(Event::VotingPenaltyCleared { who: alias });

			Ok(Pays::No.into())
		}

		/// Origin must be signed.
		#[pallet::call_index(16)]
		#[pallet::weight(T::WeightInfo::clean_vote())]
		pub fn clean_vote_signed(
			origin: OriginFor<T>,
			case_index: CaseIndex,
			voter: Alias,
		) -> DispatchResultWithPostInfo {
			ensure_signed(origin)?;
			Self::clean_vote_inner(case_index, voter)?;
			Ok(Pays::No.into())
		}
	}

	#[pallet::extra_constants]
	impl<T: Config> Pallet<T> {
		/// Get a unique, inaccessible account ID from the `PotId`.
		pub fn mob_rule_pot_id() -> T::AccountId {
			T::PotId::get().into_account_truncating()
		}

		/// The context used for the proofs required to authenticate as a personal alias in mob
		/// rule.
		pub fn mob_rule_context() -> Context {
			MOB_CONTEXT
		}
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn offchain_worker(block_number: BlockNumberFor<T>) {
			let interval = T::OffchainWorkInterval::get();
			if interval == 0u32.into() || !(block_number % interval).is_zero() {
				return;
			}

			// Operation 1: closing cases - moving cases from "RipeCases" to "DoneCases".
			// Executes for all entries in "RipeCases" storage item.
			for (case_index, case) in RipeCases::<T>::iter() {
				if Self::validate_close_case(case_index).is_ok() {
					// The weight may increase between the current state and state of inclusion,
					// but it should be included soon and the offchain worker will re-run regularly
					// anyway.
					let max_callback_weight = Self::callback_weight(
						case_index,
						&case.details.callback,
						case.details.context,
						case.verdict,
					);
					let res = Self::submit_authorized_transaction(Call::close_case {
						case_index,
						max_callback_weight,
					});
					Self::log_offchain_worker_tx_submit_result(res, "close case");
				}
			}

			// Operation 2: cleaning votes -
			// removes entries from "Votes" given they are cast for "DoneCases"
			// and computes voter credit.
			let votes = Self::votes_to_clean();
			for (case_index, voters) in votes {
				for voter in voters {
					// No need to call validate_clean_case here since
					// the votes_to_clean does the filtering already
					let res =
						Self::submit_authorized_transaction(Call::clean_vote { case_index, voter });
					Self::log_offchain_worker_tx_submit_result(res, "clean case");
				}
			}

			// Operation 3: reaping cases - removing cases from "DoneCases" if configured timeout
			// has passed.
			// Executes for all entries in "DoneCases".
			for (case_index, _case) in DoneCases::<T>::iter() {
				if Self::validate_reap_case(case_index).is_ok() {
					let res = Self::submit_authorized_transaction(Call::reap_case { case_index });
					Self::log_offchain_worker_tx_submit_result(res, "reap case");
				}
			}

			// Operation 4: case timeout - moving cases from "OpenCases" to "RipeCases"
			// if configured timeout has passed.
			// Executes for all entries in "OpenCases".
			for (case_index, _case) in OpenCases::<T>::iter() {
				if Self::validate_timeout_case(case_index).is_ok() {
					let res =
						Self::submit_authorized_transaction(Call::force_ripen_case { case_index });
					Self::log_offchain_worker_tx_submit_result(res, "timeout case");
				}
			}
		}

		fn integrity_test() {
			assert!(
				T::VotesOpenForClaimsDuration::get() as u64 <= T::MaxVoteClaimDuration::get(),
				"the time when a case becomes reaped (`CaseTimeoutSecs`) should be longer than the \
				time given to voters to claim their votes (`VotesOpenForClaimsDuration`)",
			);

			let budget = OcwWeightBudget::from_normal_max::<T>();

			// `close_case` additionally charges `max_callback_weight`, a caller-declared upper
			// bound on the dispatched callback whose value depends on the runtime's registered
			// callbacks and so cannot be bounded here. It is checked against the block on its own:
			// a callback exceeding the budget would already fail to dispatch. This asserts the
			// pallet's own contribution leaves room for it.
			budget.assert_fits(
				"close_case",
				T::WeightInfo::close_case().saturating_add(T::WeightInfo::authorize_close_case()),
			);
			budget.assert_fits(
				"clean_vote",
				T::WeightInfo::clean_vote().saturating_add(T::WeightInfo::authorize_clean_vote()),
			);
			budget.assert_fits(
				"reap_case",
				T::WeightInfo::reap_case().saturating_add(T::WeightInfo::authorize_reap_case()),
			);
			budget.assert_fits(
				"force_ripen_case",
				T::WeightInfo::force_ripen_case()
					.saturating_add(T::WeightInfo::authorize_force_ripen_case()),
			);
		}

		fn on_poll(_: BlockNumberFor<T>, weight_meter: &mut WeightMeter) {
			// Use at most 50% of available weight to be cautious about weight
			// underestimates.
			let budget = weight_meter.remaining() / 2;
			let mut meter = WeightMeter::with_limit(budget);
			Self::do_on_poll(&mut meter);
			weight_meter.consume(meter.consumed());
		}
	}

	impl<T: Config> Pallet<T> {
		fn authorize_close_case(
			_source: TransactionSource,
			case_index: &CaseIndex,
			max_callback_weight: &Weight,
		) -> TransactionValidityWithRefund {
			let case = Self::validate_close_case(*case_index)
				.map_err(|_| CustomInvalidity::CaseNotRipe)?;

			let callback_weight = Self::callback_weight(
				*case_index,
				&case.details.callback,
				case.details.context,
				case.verdict,
			);
			ensure!(
				max_callback_weight.all_gte(callback_weight),
				CustomInvalidity::CallbackWeightTooLow
			);

			let validity = build_transaction_validity(
				"PersonhoodMobRuleCaseClosing",
				case_index,
				tx_priority::BACKGROUND_PROGRESS,
			)?;
			Ok((validity, Weight::zero()))
		}

		fn authorize_clean_vote(
			source: TransactionSource,
			case_index: &CaseIndex,
			voter: &Alias,
		) -> TransactionValidityWithRefund {
			// We only accept local transaction as the number of vote to clean can
			// be very large. In which case we don't want to spam the transaction pool
			// with free-to-craft external transactions, and force external transactions
			// to be signed.
			ensure!(
				matches!(source, TransactionSource::Local | TransactionSource::InBlock),
				InvalidTransaction::BadSigner
			);
			Self::validate_clean_vote(*case_index, *voter).map_err(|e| {
				TransactionValidityError::Invalid(match e {
					// Case is done but claims period not over yet
					Error::Recent => InvalidTransaction::Future,
					_ => InvalidTransaction::Stale,
				})
			})?;
			let validity = build_transaction_validity(
				"PersonhoodMobRuleVoteCleaning",
				(case_index, voter),
				tx_priority::CLEANUP,
			)?;
			Ok((validity, Weight::zero()))
		}

		fn authorize_reap_case(
			_source: TransactionSource,
			case_index: &CaseIndex,
		) -> TransactionValidityWithRefund {
			Self::validate_reap_case(*case_index).map_err(|error| match error {
				Error::NotDone => CustomInvalidity::CaseNotDone,
				Error::Recent => CustomInvalidity::CaseTooRecent,
				_ => {
					log::error!(target: LOG_TARGET, "Unexpected reap case validation error");
					CustomInvalidity::CaseNotDone
				},
			})?;
			let validity = build_transaction_validity(
				"PersonhoodMobRuleCaseReaping",
				case_index,
				tx_priority::CLEANUP,
			)?;
			Ok((validity, Weight::zero()))
		}

		fn authorize_force_ripen_case(
			_source: TransactionSource,
			case_index: &CaseIndex,
		) -> TransactionValidityWithRefund {
			Self::validate_timeout_case(*case_index).map_err(|error| match error {
				Error::NotOpen => CustomInvalidity::CaseNotOpen,
				Error::CaseExpirationDisabled => CustomInvalidity::CaseExpirationDisabled,
				Error::Recent => CustomInvalidity::CaseTooRecent,
				_ => {
					log::error!(target: LOG_TARGET, "Unexpected timeout case validation error");
					CustomInvalidity::CaseNotOpen
				},
			})?;
			let validity = build_transaction_validity(
				"PersonhoodMobRuleCaseTimeout",
				case_index,
				tx_priority::BACKGROUND_PROGRESS,
			)?;
			Ok((validity, Weight::zero()))
		}

		fn do_on_poll(weight_meter: &mut WeightMeter) {
			// Check if we have enough weight to perform the basic checks.
			if weight_meter.try_consume(T::WeightInfo::on_poll_base()).is_err() {
				return;
			}

			let maybe_active_since = ActiveSince::<T>::get();
			let voter_count = T::EnsurePerson::active_count();

			if maybe_active_since.is_none() && voter_count >= T::MinimumVoterThreshold::get() {
				// Enable voting if the voter threshold is met and we have enough weight.
				if weight_meter.try_consume(T::WeightInfo::set_active_since()).is_err() {
					return;
				}
				ActiveSince::<T>::put(T::Clock::now().as_secs());
			} else if maybe_active_since.is_some() && voter_count < T::MinimumVoterThreshold::get()
			{
				// Disable voting if the voter threshold is not met and we have enough weight.
				if weight_meter.try_consume(T::WeightInfo::kill_active_since()).is_err() {
					return;
				}
				ActiveSince::<T>::kill();
			}
		}

		/// Returns a list of cases where the user has a vote stored in `Votes`. If the `done_only`
		/// flag is set, only cases that are done and ready to be claimed will be returned. This
		/// function does not take the correctness of the vote into account.
		pub fn voted_on(voter: &Alias, done_only: bool) -> Vec<CaseIndex> {
			let mut cases = alloc::vec![];
			for case_index in Votes::<T>::iter_keys()
				.filter(|(_, alias)| alias == voter)
				.map(|(case, _)| case)
			{
				if done_only {
					if DoneCases::<T>::contains_key(case_index) {
						cases.push(case_index);
					}
				} else {
					cases.push(case_index);
				}
			}
			cases
		}

		// Do the process of claiming mob credits associated with a correct vote on a case.
		fn do_claim_votes(voter: &Alias, case_indices: Vec<CaseIndex>) -> DispatchResult {
			ensure!(
				case_indices.len() <= T::MaxVotesClaimable::get() as usize,
				Error::<T>::TooManyClaims
			);

			let mut credit = Credits::<T>::get(voter);

			// The round when this point will become claimable is the successor of the
			// payout round that is happening right now, or round 0 if there is no
			// payout round.
			let current_round = PayoutDistribution::<T>::get()
				.as_ref()
				.map(|d| d.round.saturating_add(1))
				.unwrap_or_default();
			let mut correct_votes = 0;
			for case_index in &case_indices {
				let vote = Votes::<T>::take(case_index, voter).ok_or(Error::<T>::NoSuchVote)?;
				let case = DoneCases::<T>::get(case_index).ok_or(Error::<T>::NotDone)?;
				if case.verdict.matches_intent(vote) ||
					(case.verdict == Judgement::Contempt &&
						vote == Judgement::Truth(Truth::False))
				{
					correct_votes.saturating_inc();
				} else if vote == Judgement::Contempt {
					let now = frame_system::Pallet::<T>::block_number();
					VotingPenalties::<T>::insert(voter, now);
				}
			}
			credit.cleaned = credit.cleaned.saturating_add(case_indices.len() as u32);
			credit.correct = credit.correct.saturating_add(correct_votes);
			VotingPoints::<T>::mutate(current_round, voter, |points| {
				*points = points.saturating_add(correct_votes);
			});
			AccumulatedPoints::<T>::mutate(|points| {
				*points = points.saturating_add(correct_votes.into())
			});
			Credits::<T>::insert(voter, &credit);
			Self::deposit_event(Event::VotesClaimed { voter: *voter, case_indices });
			Ok(())
		}

		fn validate_close_case(
			case_index: CaseIndex,
		) -> Result<RipeCase<<T as frame_system::Config>::RuntimeCall>, Error<T>> {
			RipeCases::<T>::get(case_index).ok_or(Error::<T>::NotRipe)
		}

		fn validate_clean_vote(case_index: CaseIndex, voter: Alias) -> Result<Judgement, Error<T>> {
			ensure!(
				!OpenCases::<T>::contains_key(case_index) &&
					!RipeCases::<T>::contains_key(case_index),
				Error::<T>::NotDone
			);
			if let Some(case) = DoneCases::<T>::get(case_index) {
				let votes_claims_duration: u64 = T::VotesOpenForClaimsDuration::get().into();
				let now = T::Clock::now().as_secs();
				ensure!(now > votes_claims_duration.saturating_add(case.since), Error::<T>::Recent);
			}
			Votes::<T>::get(case_index, voter).ok_or(Error::<T>::NoSuchVote)
		}

		fn validate_reap_case(case_index: CaseIndex) -> Result<DoneCase, Error<T>> {
			let case = DoneCases::<T>::get(case_index).ok_or(Error::<T>::NotDone)?;
			let now = T::Clock::now().as_secs();
			ensure!(
				now > case.since.saturating_add(T::MaxVoteClaimDuration::get()),
				Error::<T>::Recent
			);
			Ok(case)
		}

		fn validate_timeout_case(
			case_index: CaseIndex,
		) -> Result<OpenCase<<T as frame_system::Config>::RuntimeCall>, Error<T>> {
			let case = OpenCases::<T>::get(case_index).ok_or(Error::<T>::NotOpen)?;
			let active_since = ActiveSince::<T>::get().ok_or(Error::<T>::CaseExpirationDisabled)?;
			let secs = T::Clock::now()
				.as_secs()
				.saturating_sub(core::cmp::max(case.since, active_since));
			ensure!(secs >= T::MaxVotingDuration::get().into(), Error::<T>::Recent);
			Ok(case)
		}

		pub(crate) fn votes_to_clean() -> Vec<(CaseIndex, Vec<Alias>)> {
			use alloc::collections::BTreeMap;
			let mut cases: BTreeMap<CaseIndex, Vec<Alias>> = BTreeMap::new();
			let limit = T::CleanVotesBatchSize::get();
			let done_cases_len = DoneCases::<T>::iter().count();
			if done_cases_len == 0 {
				return cases.into_iter().collect();
			}
			let limit_per_case: usize =
				limit.saturating_div(done_cases_len as u32).saturated_into();
			let votes_claims_duration: u64 = T::VotesOpenForClaimsDuration::get().into();
			let now = T::Clock::now().as_secs();

			let mut cases_len = 0;
			for (case_index, voter, _vote) in Votes::<T>::iter() {
				if DoneCases::<T>::get(case_index)
					.is_some_and(|case| now > votes_claims_duration.saturating_add(case.since))
				{
					cases
						.entry(case_index)
						.and_modify(|voters| {
							if voters.len() < limit_per_case {
								voters.push(voter);
								cases_len += 1;
							}
						})
						.or_insert_with(|| {
							cases_len += 1;
							alloc::vec![voter]
						});
				}
				if cases_len >= limit {
					break;
				}
			}

			cases.into_iter().collect()
		}

		fn log_offchain_worker_tx_submit_result(res: Result<(), ()>, operation: &str) {
			match res {
				Ok(_) =>
					log::info!(target: LOG_TARGET, "offchain_worker - {operation} transaction submitted"),
				Err(e) =>
					log::error!(target: LOG_TARGET, "offchain_worker - failed to submit {operation} transaction: {e:?}"),
			}
		}

		fn submit_authorized_transaction(call: Call<T>) -> Result<(), ()> {
			let xt = T::create_authorized_transaction(call.into());
			SubmitTransaction::<T, Call<T>>::submit_transaction(xt)
		}

		fn clean_vote_inner(case_index: CaseIndex, voter: Alias) -> DispatchResult {
			let vote = Self::validate_clean_vote(case_index, voter)?;

			let maybe_verdict = DoneCases::<T>::get(case_index).map(|c| c.verdict);
			Votes::<T>::remove(case_index, voter);

			let mut credit = Credits::<T>::get(voter);
			credit.cleaned.saturating_inc();
			match maybe_verdict {
				Some(verdict)
					if verdict.matches_intent(vote) ||
						(verdict == Judgement::Contempt &&
							vote == Judgement::Truth(Truth::False)) =>
				{
					credit.correct.saturating_inc();
				},
				Some(_) if vote == Judgement::Contempt => {
					let now = frame_system::Pallet::<T>::block_number();
					VotingPenalties::<T>::insert(voter, now);
				},
				_ => {},
			}
			Credits::<T>::insert(voter, &credit);

			Self::deposit_event(Event::VoteCleaned { case_index, voter });

			Ok(())
		}

		/// Computes the declared (pre-dispatch) weight of a case's callback.
		///
		/// This is the value the offchain worker passes as `max_callback_weight` when submitting
		/// `close_case`, so the parent call's declared weight covers the dispatched callback.
		/// Returns zero when the callback fails to decode, matching `dispatch_callback` which
		/// dispatches nothing in that case.
		pub(crate) fn callback_weight(
			case_index: CaseIndex,
			callback: &Callback<
				(CaseIndex, JudgementContext, Judgement),
				<T as frame_system::Config>::RuntimeCall,
			>,
			context: JudgementContext,
			verdict: Judgement,
		) -> Weight {
			match callback.curry((case_index, context, verdict)) {
				Ok(call) => call.get_dispatch_info().call_weight,
				Err(_) => Weight::zero(),
			}
		}

		/// Dispatches the case callback and returns the weight it actually consumed.
		///
		/// `max_callback_weight` is the caller-declared upper bound already accounted for in the
		/// parent call's declared weight. The dispatched callback's pre-dispatch weight is checked
		/// against it (failing with `CallbackWeightTooLow` when the bound is insufficient) so the
		/// parent call can refund the difference between the declared bound and the returned
		/// weight. A callback that fails to decode dispatches nothing and consumes zero weight.
		fn dispatch_callback(
			case_index: CaseIndex,
			callback: Callback<
				(CaseIndex, JudgementContext, Judgement),
				<T as frame_system::Config>::RuntimeCall,
			>,
			context: JudgementContext,
			verdict: Judgement,
			trigger: CallbackTrigger,
			max_callback_weight: Weight,
		) -> Result<Weight, DispatchError> {
			match callback.curry((case_index, context, verdict)) {
				Err(error) => {
					log::warn!(
						target: LOG_TARGET,
						"callback decode failed: case_index={case_index}, trigger={trigger:?}, callback_pallet_index={}, callback_call_index={}, error={error}",
						callback.pallet_index(),
						callback.call_index(),
					);

					Self::deposit_event(Event::CallbackError { case_index, trigger });

					Ok(Weight::zero())
				},
				Ok(call) => {
					let dispatch_info = call.get_dispatch_info();
					ensure!(
						dispatch_info.call_weight.all_lte(max_callback_weight),
						Error::<T>::CallbackWeightTooLow
					);

					let result = call.dispatch(RawOrigin::Root.into());

					Self::deposit_event(Event::Callback {
						result: result.map(|_| ()).map_err(|e| e.error),
					});

					let actual_weight = extract_actual_weight(&result, &dispatch_info);
					Ok(actual_weight)
				},
			}
		}
	}

	fn build_transaction_validity(
		tag_prefix: &'static str,
		provides: impl Encode,
		priority: TransactionPriority,
	) -> TransactionValidity {
		ValidTransaction::with_tag_prefix(tag_prefix)
			.and_provides(provides)
			.propagate(true)
			.priority(priority)
			.build()
	}

	impl<T: Config> StatementOracle<<T as frame_system::Config>::RuntimeCall> for Pallet<T> {
		type Ticket = CaseIndex;

		fn judge_statement(
			statement: Statement,
			context: JudgementContext,
			callback: Callback<
				(Self::Ticket, JudgementContext, Judgement),
				<T as frame_system::Config>::RuntimeCall,
			>,
		) -> Result<Self::Ticket, DispatchError> {
			let ticket = CaseCount::<T>::get();
			CaseCount::<T>::mutate(|c| c.saturating_inc());
			let status = OpenCase {
				details: CaseDetails { statement, context, callback },
				since: T::Clock::now().as_secs(),
				tally: Default::default(),
			};
			OpenCases::<T>::insert(ticket, status);
			Self::deposit_event(Event::CaseCreated { case_index: ticket });
			Ok(ticket)
		}
	}
}
