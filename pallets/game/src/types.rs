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
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use frame_support::{pallet_prelude::Get, BoundedVec};
use indiv_pallet_airdrop::{types::AirdropPrize, RegistrationEntry};
use indiv_pallet_score::AccountOrPerson;
use indiv_support::traits::{Alias, RevisionIndex, RingIndex};
use scale_info::TypeInfo;
use sp_core::sr25519::vrf::VrfSignature;

/// The index of a player in a round in a game.
///
/// A player have one index per round, this index determines the group the players play in.
pub type PlayerIndex = u32;

/// The index of a round in a game.
pub type RoundIndex = u8;

/// The report made by one player about another player.
#[derive(
	Encode,
	Decode,
	MaxEncodedLen,
	TypeInfo,
	Clone,
	Copy,
	Debug,
	Eq,
	PartialEq,
	DecodeWithMemTracking,
)]
pub enum Report {
	/// Reported as a real person.
	Person,
	/// Reported as not a real person.
	NotPerson,
}

/// The full report of a player about other players in a game. This includes the report about all
/// the other players the player played with, in all the rounds, order by
/// `(round index, player index)`.
// Optimization: This can be a bitvec
pub type FullReport<T> =
	BoundedVec<BoundedVec<Report, <T as Config>::MaxGroupSize>, <T as Config>::MaxRounds>;

/// The information of a player.
///
/// The player may hold a deposit and a sufficient ref, both must be handled when the player is
/// removed.
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Debug, DecodeWithMemTracking)]
pub struct Player<PlayDeposit> {
	/// The index of the first game this player was eligible to play.
	pub first_game: u32,
	/// The player has registered for the current game.
	pub registered: bool,
	/// The player sent a report for the current game.
	pub sent_report: bool,
	/// When the player's attendance was determined early during the reporting phase, the
	/// result of the early `set_attendance` call is cached here so the player-process phase
	/// can reuse it without calling the score pallet again. Reset to `None` at the end of the
	/// player-process phase so the next game starts fresh.
	pub early_attendance_enactment: Option<EarlyAttendanceEnactment>,
	/// The number of positive report the player received.
	pub yes_person: u8,
	/// The number of negative report the player received.
	pub no_not_person: u8,
	/// The maximum total vote weight this player can receive across all rounds, computed
	/// during the shuffle phase from the actual group composition using each co-player's
	/// snapshotted [`Self::vote_weight`].
	pub expected_max_vote_weight: u16,
	/// The vote weight this player casts when reporting, snapshotted during the shuffle
	/// phase from the score pallet's personhood status. Using a frozen value keeps the
	/// received tallies consistent with [`Self::expected_max_vote_weight`] even if the
	/// player's personhood changes mid-game (e.g. via early attendance enactment).
	pub vote_weight: u8,
	/// The credibility, the reason the player can play.
	/// This is also used to know if the player is recognized or not.
	pub credibility: PlayerCredibility<PlayDeposit>,
}

/// The cached outcome of an early `set_attendance` call.
///
/// Stored in [`Player::early_attendance_enactment`] so the player-process phase can finish
/// the player's state transitions.
#[derive(
	Encode,
	Decode,
	MaxEncodedLen,
	TypeInfo,
	Debug,
	Clone,
	Copy,
	PartialEq,
	Eq,
	DecodeWithMemTracking,
)]
pub struct EarlyAttendanceEnactment {
	/// The attendance applied to the player during the early processing.
	pub attendance: bool,
	/// Whether the player should be kept in `Players` or archived after the player-process
	/// phase finishes their state transitions.
	pub disposition: PlayerDisposition,
}

/// What to do with the player record once the player-process phase runs.
///
/// Computed when attendance is applied, from the attendance outcome together with the score
/// pallet's response (externally-recognized status and post-attendance score).
#[derive(
	Encode,
	Decode,
	MaxEncodedLen,
	TypeInfo,
	Debug,
	Clone,
	Copy,
	PartialEq,
	Eq,
	DecodeWithMemTracking,
)]
pub enum PlayerDisposition {
	/// Keep the player in `Players`.
	Keep,
	/// Move the player to `ArchivedPlayers` as an account that can later be kicked.
	ArchiveKickable,
	/// Move the player to `ArchivedPlayers` as an externally-recognized person.
	ArchiveUnkickable,
}

/// The outcome of the attendance check for a player.
///
/// The attendance can be determined from partial results during the reporting phase: as soon as
/// all possible remaining votes cannot change the outcome, the attendance becomes final.
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttendanceStatus {
	/// The player attendance is definitely successful: even if every yet-to-be-received vote
	/// counted as `NotPerson`, the player would still attend.
	Attended,
	/// The player attendance is definitely unsuccessful: even if every yet-to-be-received vote
	/// counted as `Person`, the player would still not attend.
	NotAttended,
	/// The attendance cannot yet be decided: some future votes could still flip the outcome.
	Pending,
}

/// Per-event airdrop registration data supplied by the player at game sign-up.
///
/// If the player is recognized (pallet-score `Recognition` is `Recognized` or
/// `ExternallyRecognized`) then they must provide the alias variant, otherwise they must provide
/// the account variant.
#[derive(Encode, Decode, TypeInfo, Debug, Clone, PartialEq, Eq)]
#[scale_info(skip_type_params(P))]
pub enum AirdropVrf<P> {
	/// VRF for non-recognized players.
	///
	/// When used with `indiv-pallet-airdrop`, this is the sr25519 VRF signature from the
	/// player's acount key (meaning only account id from sr25519 keys are supported) for the
	/// transcript built by `indiv_pallet_airdrop::vrf::transcript_for_event`. So the transcipt
	/// has the data "domain" with the value `VRF_TRANSCRIPT_LABEL ++ event_id` (`event_id` is
	/// `b"pop:game:airdrop:           " ++ game_index.to_be_bytes()`) and the data
	/// "signer" with the value the sr25519 public key.
	///
	/// This is used by `Config::Airdrop::participate_with_account`.
	Account(VrfSignature),
	/// VRF for recognized players.
	///
	/// When used with `indiv-pallet-airdrop`, this is the proof of membership in the
	/// collection `PEOPLE_IDENTIFIER` of the participant. It verifies the proof in the collection
	/// `PEOPLE_IDENTIFIER` at ring `ring_index` and revision `revision` for the context
	/// `blake2_256(AIRDROP_CONTEXT_BASE ++ event_id)` (`event_id` is
	/// `b"pop:game:airdrop:           " ++ game_index.to_be_bytes()`) for the message
	/// the scale encoded registration entry of the participant (so for alias-based player `0u8 ++
	/// alias`, for account-based player `1u8 ++ account_id`).
	///
	/// This is used by `Config::Airdrop::participate_with_alias` and
	/// `Config::Airdrop::participate_with_account`.
	Alias { proof: P, ring_index: RingIndex, revision: RevisionIndex },
}

// `VrfSignature` doesn't derive `DecodeWithMemTracking`, but it decodes from fixed-size arrays
// without heap allocation, so we can implement `DecodeWithMemTracking` for `AirdropVrf`.
impl<P: DecodeWithMemTracking> DecodeWithMemTracking for AirdropVrf<P> {}

/// The sign-up variants for the call `sign_up_inner`.
pub(crate) enum SignUpArgs<AccountId, Signature, Proof> {
	/// The player is an alias.
	Alias {
		who: Alias,
		identifier_key: CommunicationIdentifier,
		statement_account: AccountId,
		statement_account_signature: Signature,
		airdrop: Option<AirdropVrf<Proof>>,
	},
	/// The player is an account.
	Account {
		who: AccountId,
		identifier_key: CommunicationIdentifier,
		new_invited: bool,
		airdrop: Option<AirdropVrf<Proof>>,
	},
}

/// The credibility of a player.
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Debug, DecodeWithMemTracking)]
pub enum PlayerCredibility<PlayDeposit> {
	/// Is playing using the invitation.
	Invited,
	/// Is recognized or **has been** recognized.
	Recognized,
	/// Is playing using a deposit.
	Deposit(PlayDeposit),
}

/// The information for a player that has been archived, when its score is zero.
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Debug)]
pub enum ArchivedPlayer<BlockNumber> {
	Kickable { first_game: u32, archived_since: BlockNumber },
	Unkickable { first_game: u32 },
}

impl<BlockNumber> ArchivedPlayer<BlockNumber> {
	/// Return the index of the first game of this archived player.
	pub fn first_game(&self) -> u32 {
		match self {
			Self::Kickable { first_game, .. } | Self::Unkickable { first_game } => *first_game,
		}
	}
}

/// The information of a game.
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Debug, DecodeWithMemTracking)]
pub struct GameInfo<AccountId: Into<sp_statement_store::AccountId>> {
	/// The index of the game, used as an identifier.
	pub index: u32,
	/// The end of the registration period in seconds since Unix epoch.
	pub registration_ends: u32,
	/// The shuffle deadline in seconds since Unix epoch.
	/// If the deadline is missed, the game is cancelled.
	pub shuffle_deadline: u32,
	/// The start of the game in seconds since Unix epoch.
	pub game_date: u32,
	/// The end of the reporting period in seconds since Unix epoch.
	pub report_ends: u32,
	/// The current state of the game.
	pub state: GameState<AccountId>,
	/// The maximum number of players in a group.
	pub max_group_size: u32,
	/// The number of rounds in this game.
	pub rounds: u8,
	/// The number of registered players whose attendance has not yet been determined.
	///
	/// Initialized to the registered-player count by `shuffle_step_insert` and
	/// decremented in `apply_attendance` each time a player's attendance is
	/// settled. Settlement happens *early* when an incoming `report` makes a
	/// named player's tally decisive (via `try_early_attendance_enactment`),
	/// or *late* when `player_process_step1` resolves the rest after `report_ends`.
	/// When the counter reaches zero before `report_ends`, `process_reporting`
	/// leaves the reporting phase early instead of waiting for the deadline.
	pub pending_attendance: u32,
	/// Whether an airdrop event was successfully scheduled for this game. When `false`,
	/// registration for airdrop at sign-up is skipped and no cancel is dispatched on
	/// game cancellation.
	pub airdrop_scheduled: bool,
}

/// The state of a game.
///
/// State transitions:
///
/// ```text
///   Registration ──► Shuffle ──► Reporting ──► PlayerProcess ──► (game killed)
///         │             │
///         └─────────────┴────────────► Cancelling ──► (game killed)
/// ```
///
/// - [`Self::Registration`] → [`Self::Shuffle`] when the registration deadline passes and the
///   player count is acceptable for grouping.
/// - [`Self::Registration`] → [`Self::Cancelling`] when the deadline passes but the player count is
///   not acceptable.
/// - [`Self::Shuffle`] → [`Self::Reporting`] when all shuffle sub-steps complete and the
///   attendance-report session is successfully started.
/// - [`Self::Shuffle`] → [`Self::Cancelling`] when starting the attendance-report session fails.
/// - [`Self::Reporting`] → [`Self::PlayerProcess`] when the reporting deadline passes, or earlier
///   if every player's attendance has already been settled.
/// - [`Self::PlayerProcess`] → (game killed) after all three sub-steps complete (see
///   [`PlayerProcessStep`] for the internal transitions).
/// - [`Self::Cancelling`] → (game killed) after every player has been reset.
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Debug, PartialEq)]
pub enum GameState<AccountId: Into<sp_statement_store::AccountId>> {
	/// The game is in registration period. Players can sign-up.
	Registration { next_player_index: u32 },
	/// The game finished registration and is shuffling the players.
	Shuffle { step: ShuffleStep<AccountId> },
	/// The game is in the reporting period. Players can report each other.
	Reporting { player_count: u32 },
	/// The game finished the reporting period and is processing the result.
	/// See [`PlayerProcessStep`] for the internal sub-steps.
	PlayerProcess { step: PlayerProcessStep<AccountId> },
	/// The game is cancelled and cleaning itself.
	Cancelling { last_iteration: Option<AccountOrPerson<AccountId>> },
}

/// The steps for the player-process phase.
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Debug, PartialEq)]
pub enum PlayerProcessStep<AccountId: Into<sp_statement_store::AccountId>> {
	/// Iterate every player to settle attendance and burn deposits.
	Step1ProcessPlayers { last_iteration: Option<AccountOrPerson<AccountId>>, player_count: u32 },
	/// Drain the per-game index <-> player maps (`IndexToPlayer` and
	/// `PlayerToIndex`) in bounded chunks, then kill the game.
	Step2ClearIndices,
}

/// The steps for the shuffle phase
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Debug, PartialEq)]
pub enum ShuffleStep<AccountId: Into<sp_statement_store::AccountId>> {
	/// Insertion of players into storage.
	Step1Insert { last_iteration: Option<AccountOrPerson<AccountId>> },
	/// Retrieval of player from storage to get their order.
	/// First we index recognized players, then not recognized players.
	Step2Retrieve { next_player_index: u32, recognized_finished: bool },
	/// Iterate over each registered player and compute their `expected_max_vote_weight`
	/// from the actual group composition produced by [`Self::Step2Retrieve`].
	Step3ComputeWeights { last_iteration: Option<AccountOrPerson<AccountId>>, player_count: u32 },
	/// All players have been indexed. We now try to start the attendance report session.
	Step4AwaitSession { player_count: u32 },
}

/// A setting for a distribution of players in groups.
pub struct GroupsSetting {
	/// The maximum number of players in a group.
	pub max_per_group: u32,
	/// The total number of players.
	pub player_count: u32,
}

impl GroupsSetting {
	/// Calculate if the number of players is acceptable to make groups where
	/// all groups are at least `max_per_group - 1` players.
	pub fn acceptable_player_count<T: Config>(&self) -> bool {
		match self.max_per_group {
			0 => true,
			1 => true,
			2 => self.player_count >= 1,
			_ => {
				#[cfg(feature = "testnet")]
				if T::TESTNET {
					return self.player_count >= 1;
				}

				// Special condition for one group.
				if self.player_count == self.max_per_group ||
					self.player_count + 1 == self.max_per_group
				{
					return true;
				}

				let threshold = (self.max_per_group - 1).saturating_mul(self.max_per_group - 2);
				self.player_count >= threshold
			},
		}
	}

	/// Calculate the number of groups, it is the minimum number of groups that can contain all the
	/// players.
	pub fn number_of_group(&self) -> u32 {
		let div = self.player_count.checked_div(self.max_per_group).unwrap_or(self.player_count);
		let remainder = self.player_count.checked_rem(self.max_per_group).unwrap_or(0);
		if remainder > 0 {
			div.saturating_add(1)
		} else {
			div
		}
	}

	/// Calculate the group of a player, given its position.
	pub fn group_index_from_player_index(&self, shuffled_position: u32) -> u32 {
		shuffled_position.checked_rem(self.number_of_group()).unwrap_or(0)
	}

	/// Calculate the members of a group, given its index.
	pub fn group_members(&self, group_index: u32) -> impl Iterator<Item = u32> {
		let number_of_group = self.number_of_group();
		let max_per_group = self.max_per_group;
		let number_of_player = self.player_count;
		(0..max_per_group)
			.map(move |i| group_index.saturating_add(i.saturating_mul(number_of_group)))
			.filter(move |&x| x < number_of_player)
	}
}

/// Defines the schedule of a game.
#[derive(
	Encode, Decode, MaxEncodedLen, TypeInfo, Debug, Clone, PartialEq, DecodeWithMemTracking, Default,
)]
pub struct GameSchedule<AssetId, Balance> {
	/// In seconds, since Unix epoch. Defines when the game should actually start.
	pub game_play_time: u32,
	/// The number of rounds in the scheduled game.
	pub rounds: u8,
	/// The maximum number of players in one group.
	///
	/// Groups are ensured to be at least `max_group_size - 1` players, so this value must be at
	/// least 3 in order to ensure that groups contains at least 2 players.
	pub max_group_size: u32,
	/// Prize spec for the per-game airdrop event. `None` skips airdrop scheduling for this game.
	pub airdrop_prize: Option<AirdropPrize<AssetId, Balance>>,
}

/// `GameSchedule` for the runtime.
pub type GameScheduleOf<T> =
	crate::types::GameSchedule<<T as Config>::AirdropAssetId, <T as Config>::AirdropAssetBalance>;

/// A trait for calculating various game phase timestamps based on a game play start time.
/// All returned times in seconds since Unix epoch.
pub trait GameTimes<T: Config> {
	/// Calculates the time at which game should start.
	fn game_play_time(&self) -> u32;

	/// Calculates the latest time at which registration phase of a game should start, if passed
	/// then the game should be skipped. The calculation assumes a maximum time that the
	/// registration phase should take.
	fn registration_start(&self) -> u32 {
		let phases = configured_phases::<T>();
		self.game_play_time()
			.saturating_sub(phases.shuffle)
			.saturating_sub(phases.post_shuffle_margin)
			.saturating_sub(phases.registration)
	}

	/// Calculates the time at which registration phase of a game should end. The calculation
	/// assumes a maximum time that the shuffle phase should take, which is the phase between
	/// registration and reporting.
	fn registration_end(&self) -> u32 {
		let phases = configured_phases::<T>();
		self.game_play_time()
			.saturating_sub(phases.shuffle)
			.saturating_sub(phases.post_shuffle_margin)
	}

	/// The time at which if the shuffle phase is not finished, the game will be cancelled.
	fn shuffle_deadline(&self) -> u32 {
		let phases = configured_phases::<T>();
		self.game_play_time().saturating_sub(phases.post_shuffle_margin)
	}

	/// Calculates the time at which reporting phase of a game should end. The calculation assumes a
	/// maximum time that the reporting phase should take.
	fn reporting_end(&self) -> u32 {
		let phases = configured_phases::<T>();
		self.game_play_time().saturating_add(phases.reporting)
	}

	/// Calculates the time at which player process phase of a game should end, which effectively
	/// means the end of the game. The calculation assumes a maximum time that the player process
	/// phase should take.
	fn player_process_end(&self) -> u32 {
		let phases = configured_phases::<T>();
		self.game_play_time()
			.saturating_add(phases.reporting)
			.saturating_add(phases.player_process)
	}
}

fn configured_phases<T: Config>() -> PhaseDurationValues {
	StoredPhaseDurations::<T>::get().unwrap_or_else(T::DefaultPhaseDurations::get)
}

impl<T: Config, AssetId, Balance> GameTimes<T> for GameSchedule<AssetId, Balance> {
	fn game_play_time(&self) -> u32 {
		self.game_play_time
	}
}

impl<T: Config, AccountId: Into<sp_statement_store::AccountId>> GameTimes<T>
	for GameInfo<AccountId>
{
	fn game_play_time(&self) -> u32 {
		self.game_date
	}
}

/// Contains the duration values for each game phase in seconds.
#[derive(
	Encode, Decode, MaxEncodedLen, TypeInfo, Debug, Clone, PartialEq, DecodeWithMemTracking,
)]
pub struct PhaseDurationValues {
	/// Registration phase minimum duration in seconds
	pub registration: u32,
	/// Shuffle phase duration in seconds
	pub shuffle: u32,
	/// Minimum time between shuffle and game play time in seconds
	pub post_shuffle_margin: u32,
	/// Reporting phase duration after game play time in seconds
	///
	/// The reporting phase will end after this duration after game play time.
	/// During this time, players will play the game and submit their reports.
	/// So the reporting duration must take into account the time needed for players to play the
	/// game and submit their reports.
	pub reporting: u32,
	/// Player process phase duration in seconds
	pub player_process: u32,
	/// Airdrop claim-window duration in seconds.
	///
	/// The scheduled airdrop's `end_time` is set to
	/// `game_play_time + reporting + airdrop_claim_window`.
	pub airdrop_claim_window: u32,
}

/// A step result for a process in multiple step.
#[must_use]
#[derive(Debug, PartialEq)]
pub enum StepResult {
	/// The step needs to be continued.
	Continue,
	/// The step is finished.
	Finished,
}

/// A type alias for the NFT type used in the game.
pub type Nft = [u8; 32];

/// The reason why a statement is invalid for indiv-pallet-game.
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Debug, Clone, PartialEq)]
pub enum InvalidStatementReason {
	/// The game is not in the reporting phase, statements are valid only during the
	/// reporting phase of the game.
	NoReportingPhase,
	/// The statement is not signed, only signed statements are valid for usage in game.
	StatementIsNotSigned,
	/// The signature of the statement failed to verify.
	InvalidSignature,
	/// The topic 0 of the statement is invalid, it must be the topic for the current game.
	InvalidTopic0 { expected: Option<[u8; 32]>, actual: Option<[u8; 32]> },
	/// The signer of the statement is not a player.
	AccountIsNotAPlayer,
	/// The player is not registered for the current game.
	PlayerIsNotRegistered,
}

/// Helper to convert from AccountOrPerson to RegistrationEntry.
pub fn into_registration_entry<AccountId>(
	account_or_person: AccountOrPerson<AccountId>,
) -> RegistrationEntry<AccountId> {
	match account_or_person {
		AccountOrPerson::Account(account_id) => RegistrationEntry::Account { account_id },
		AccountOrPerson::Person(alias) => RegistrationEntry::Alias { alias },
	}
}
