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

//! Proof-of-Personhood system: Game Pallet
//!
//! This pallet provides a mechanism for organizing repeated "games" in which players verify each
//! other’s personhood. A typical flow for each game looks like this:
//!
//! 1. **Scheduling**: A privileged origin calls `schedule_games` with settings for multiple games,
//!    including their `game_play_time`.
//!
//! 2. **New Game**: if the previous game has finished result processing and there is a game
//!    scheduled then `on_poll`/`on_idle` will start the new game in the registration phase. The new
//!    game is stored in [`Game`].
//!
//! 3. **Registration**: Players call [`Pallet::sign_up_with_account`],
//!    [`Pallet::sign_up_with_invite`] or [`Pallet::sign_up_with_alias`] to be part of the game. A
//!    player who fails to register before the deadline is considered absent.
//!
//! 4. **Shuffle**:
//!    - After registration ends, `on_poll`/`on_idle` automatically triggers the transition to
//!      shuffle phase.
//!    - Then the `on_poll`/`on_idle` will operate the shuffle until all player have been shuffled.
//!    - Once the shuffling is complete, the pallet transitions to [`GameState::Reporting`].
//!
//! 5. **Reporting**:
//!    - Until `report_ends`, each player can call `report` to submit their observations about the
//!      group members they played with across all rounds. Each observation is a [`Report`] (either
//!      `Person` or `NotPerson`).
//!    - A player who fails to report will be considered absent in this game.
//!
//! 6. **Result Processing**:
//!    - After `report_ends`, `on_poll`/`on_idle` automatically triggers the transition to the phase
//!      [`GameState::PlayerProcess`].
//!    - Then `on_poll`/`on_idle` process all the results and update players final attendance status
//!      based on the aggregate of received reports. If a player is considered a person (i.e.,
//!      sufficiently many `Person` reports vs. `NotPerson`), they are marked as “attended” in
//!      [`indiv_pallet_score`]. Otherwise, they are marked absent.
//!    - At the end of processing, the game concludes and is removed from storage.
//!
//! # player index, groups, rounds and report.
//!
//! After the shuffle phase, each player obtained a unique index for each round. This index is used
//! to determine the group the player will be in for each round.
//! Given the maximum group size and the player count in the game info. The number of group is
//! calculated as `number_of_group = ⌈player_count/max_group_size⌉` (`⌈a/b⌉` is div_ceil).
//! The group number for a player is calculated as `group_index = player_index % number_of_group`,
//! the members of a group is calculated as:
//! ```math
//! given: f(i) = group_index + i x number_of_group
//! { f(i) ∣ i ∈ {0,1,…,max_group_size−1} and f(i) < number_of_player }
//! or equivalently:
//! {group_index+i⋅number_of_group∣i∈{0,1,…,max_per_group−1}}∩{0,1,…,number_of_player−1}
//! ```
//! After participating in the game in the given group in each round, the player will report on
//! other players in the group. The report is a list of round reports, one for each round. Each
//! round report is a list of reports for all other players in the group, ordered by player index.
//!
//! Players have different vote weights and are differently distributed depending on whether they
//! are recognized as a unique individual in pallet-score. Throughout this pallet, we use
//! "recognized players"/"people" for players that are recognized as unique individuals in
//! pallet-score, and "not-recognized players"/"candidates" for players that are not currently
//! recognized as unique individuals in pallet-score.
//! During the shuffle phase, the recognized players are assigned the first player indices, and the
//! not-recognized players are assigned the last player indices. Given the calculation of groups,
//! this ensures that recognized players are evenly distributed across groups.
//! Additionally, the vote weight of recognized players is [`Config::PeopleVoteWeight`] and the vote
//! weight of not-recognized players is [`Config::CandidateVoteWeight`].
//!
//! Note that this status is different from the player's participant info field: `recognition` in
//! pallet-score, which is the recognition status in the `People` registry. The player is considered
//! recognized in pallet-score when the field `reached_personhood` is true.
//!
//! # Attendance
//!
//! A participant is considered attending if:
//! * they signed up for the game
//! * they sent a report
//! * strictly more than half of the voter reported them as a person, or the total number of report
//!   is 0.
//!
//! Otherwise the participant is considered absent.
//!
//! # Deposit/Credibility/Invitation/Archival
//!
//! New players can sign up for a game by proving an initial credibility.
//! Either they are recognized by another DIM, and they use their alias to register to the games,
//! they are considered credible. Or they use an account and need to prove their credibility by
//! using an invite ticket, an invitation from their lite personhood, or by paying a deposit.
//! We differentiate between players playing using an account, and players already recognized by
//! another DIM and playing using their alias.
//!
//! For account-based players: if after a game the player's score reach 0 then they are archived,
//! their credibility is lost, in case of a deposit, the deposit is burned.
//! When archived, to sign up for a game, they need to pay a new deposit.
//! After [`Config::NonPlayingKickoutTime`] blocks, the archived account-based players can be kicked
//! out and completely removed from indiv-pallet-game and indiv-pallet-score. If they come back,
//! they are considered new players.
//!
//! For alias-based players: if absent for one game, then they are archived, they can always sign-up
//! again for free. Their credibility being given by their alias.
//! Archived alias-based players are never kicked out.
//!
//! # Invite process
//!
//! 1. The configured origin [`Config::InviteIssuer`] give some invites to some accounts with the
//!    call `grant_invites`.
//! 2. Those accounts distribute the invites to other accounts using `set_invite_ticket`, a ticket
//!    is an account id, and the private key is revealed only to the invited person, as part of the
//!    invitation.
//! 3. The invited player sign-up with the invite by providing a proof. Given, the invited player
//!    sign-up with account A, and the ticket is account B, the proof is the signature by B of the
//!    message "A". Invited player pays no fees, no deposit, as long as its score is not zero. If
//!    its score goes to zero then the invite status is revoked, and player should pay the deposit
//!    or find a new invite to play the game.
//!
//! # Lite invite process
//!
//! A lite invite is an invitation from a lite person. Lite personhood is credible enough to play,
//! so a lite person needs no invite ticket from an inviter, they invite an account of their own
//! with the call `sign_up_with_account_lite_invite`:
//!
//! 1. The lite person binds the account they want to play with to their alias in
//!    [`indiv_pallet_score::Pallet::score_context`], in indiv-pallet-people-lite.
//! 2. That account signs up with `sign_up_with_account_lite_invite`, as a player with an invited
//!    credibility and without paying any deposit.
//! 3. The account signs up for later games with `sign_up_with_account`, for free. If its score
//!    reaches zero it is archived, as any account-based player, and
//!    `sign_up_with_account_lite_invite` signs it up again.
//!
//! A lite person invites one account, ever: the first account they invite is the only one they can
//! play with.
//!
//! # Statement store usage
//!
//! All players in the game are given some statement store usage allowance. The player's allowance
//! is cleared and their statements removed when archived or offboarded.
//!
//! # NFT claim credits
//!
//! A player earns one credit per successful report of one player on another, in one round of one
//! game. A credit is not an NFT: it is the entitlement to mint one, and the minting happens on
//! Asset Hub.
//!
//! This pallet only triggers the awards, through [`Config::NftClaimCredits`]. Everything a credit
//! becomes afterwards belongs to `indiv-pallet-nft-credits`, which holds the awards, commits them
//! to one Merkle root per block, delivers those roots to the minting chain and serves the
//! inclusion proofs a claimant needs. Its module documentation covers the whole path.
//!
//! # Calls
//!
//! Player calls:
//!
//! - `sign_up_with_invite`: sign up for the game using an account and an invite, the account-based
//!   player must be new or archived, otherwise they must use `sign_up_with_account` for free.
//! - `sign_up_with_account`: sign up for the game using an account, this is free if the player is
//!   not new nor archived, otherwise they must pay a deposit.
//! - `sign_up_with_alias`: sign up for the game using an alias.
//! - `sign_up_with_account_lite_invite`: sign up for the game without deposit as a lite person,
//!   using the account bound to the lite person's alias in the score context. The account-based
//!   player must be new or archived, otherwise they must use `sign_up_with_account` for free.
//! - `report`: Each participant’s self-contained reporting of whether their peers are persons.
//! - `offboard`: Offboard from the game.
//!
//! Manager origin calls: (configured origin [`Config::ManagerOrigin`])
//!
//! - `schedule_games`: schedule future games.
//! - `set_play_deposit`: set the signup deposit amount for account-based players.
//!
//! Invite issuer calls: (configured origin [`Config::InviteIssuer`])
//!
//! - `grant_invites`: grant some number of invites to an account.
//! - `remove_available_and_pending_invites`: clear all invites for a given account.
//!
//! Inviter calls: (accounts granted some invites to distribute)
//! - `set_invite_ticket`: register a new ticket (an invite) for a future participant.
//! - `cancel_invite_ticket`: cancel a previously set invite ticket.
//!
//! Other calls:
//!
//! - `kickout`: Kick out a player that is not playing. Persons are not kickable.
//!
//! # Code debt
//!
//! An "alias" in this pallet often refers implicitly to the alias of a person, not the alias of a
//! lite person (for unfortunate historical reasons).
//!
//! `sign_up_with_account` covers both a live player, for free, and a new or archived player, from
//! whom it takes a deposit. The two cases should better be told apart instead:
//! `sign_up_with_account_lite_invite`, `sign_up_with_account_invitation` and
//! `sign_up_with_account_pay_deposit` for a new or archived player, and
//! `sign_up_with_account_active_player` for an active player.
//!
//! Invitation and deposit could potentially be removed and it could be refactored to support only
//! lite people.

#![cfg_attr(not(feature = "std"), no_std)]
#![recursion_limit = "128"]

extern crate alloc;

// The mock runtime in `mock_runtime.rs` is shared with `indiv-pallet-nft-credits`, which includes
// the same file, so it names this pallet's items by the path that crate sees them under.
#[cfg(test)]
extern crate self as indiv_pallet_game;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
mod extension;
#[cfg(test)]
mod mock;
pub mod runtime_api;
#[cfg(test)]
mod tests;
mod types;
pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
pub use benchmarking::BenchmarkHelper;
pub use extension::{GameAsInvited, GameAsInvitedData};
pub use pallet::*;
pub use types::*;
pub use weights::WeightInfo;

use crate::extension::CustomError;
use alloc::{vec, vec::Vec};
use core::time::Duration;
use frame_support::{
	dispatch::PostDispatchInfo,
	sp_runtime::Saturating,
	storage::{with_transaction, TransactionOutcome},
	traits::{
		fungible::Inspect, Consideration, Defensive, EnsureOriginWithArg, IsSubType, OriginTrait,
		UnixTime,
	},
	weights::WeightMeter,
};
use frame_system::offchain::CreateAuthorizedTransaction;
use indiv_pallet_airdrop::types::{
	Airdrop, EventId as AirdropEventId, EventInfo as AirdropEventInfo,
	RegistrationEntry as AirdropRegistrationEntry,
};
use indiv_pallet_score::AccountOrPerson;
use indiv_support::{
	credit_trees::AwardCredits,
	traits::{Alias, CommunicationIdentifier, Context},
	weight_budget::OcwWeightBudget,
};
use sp_runtime::traits::{IdentifyAccount, Verify, Zero};
use sp_statement_store::{
	decrease_allowance_by, increase_allowance_by, runtime_api::statement_store, StatementAllowance,
};

/// An upper bound on a number of operation
const OP_UPPER_BOUND: u32 = 100_000;

/// Chunk size for clearing the per-game index <-> player maps in `player_process_step2`.
pub const PLAYER_PROCESS_STEP2_CHUNK: u32 = 100;

/// Chunk size for clearing the shuffle maps in `process_cancelling`.
const CANCELLING_SHUFFLE_CHUNK: u32 = 100;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	/// `EventInfo` for the airdrop API.
	pub type AirdropEventInfoOf<T> =
		AirdropEventInfo<<T as Config>::AirdropAssetId, <T as Config>::AirdropAssetBalance>;

	/// Type alias to access the type `Proof` in the trait `Airdrop` in the runtime.
	pub type AirdropProofOf<T> = <<T as Config>::Airdrop as Airdrop<
		<T as frame_system::Config>::AccountId,
		<T as Config>::AirdropAssetId,
		<T as Config>::AirdropAssetBalance,
	>>::Proof;

	/// Type alias to access the type of `Ticket`.
	pub(crate) type TicketOf<T> =
		<<<T as Config>::TicketSignature as Verify>::Signer as IdentifyAccount>::AccountId;

	/// Native chain balance, used for the play deposit held on the `Balances` pallet.
	pub type NativeBalanceOf<T> =
		<<T as Config>::NativeFungible as Inspect<<T as frame_system::Config>::AccountId>>::Balance;

	/// Every `GAME_PROCESS_SKIPPED_BLOCK` blocks, the game process in on_poll/on_idle is skipped.
	/// This is a defense mechanism if the game process is wrongly weighted.
	pub const GAME_PROCESS_SKIPPED_BLOCK: u32 = 8;

	pub(crate) const LOG_TARGET: &str = "runtime::indiv-pallet-game";

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config:
		frame_system::Config<
			RuntimeCall: IsSubType<Call<Self>> + IsSubType<indiv_pallet_score::Call<Self>>,
			RuntimeOrigin: From<Origin<Self>> + OriginTrait<PalletsOrigin: TryInto<Origin<Self>>>,
			AccountId: From<sp_statement_store::AccountId> + Into<sp_statement_store::AccountId>,
			RuntimeEvent: TryInto<Event<Self>>,
		> + indiv_pallet_score::Config
		+ CreateAuthorizedTransaction<Call<Self>>
	{
		#[cfg(feature = "testnet")]
		const TESTNET: bool = false;

		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;

		/// The provider for getting unix time.
		type UnixTime: UnixTime;

		/// The maximum number of rounds in a game.
		///
		/// Note: the actual number of rounds is configured per game.
		#[pallet::constant]
		type MaxRounds: Get<u32>;

		/// The maximum number of players in a group.
		///
		/// Note: the actual number of players in a group is configured per game.
		#[pallet::constant]
		type MaxGroupSize: Get<u32>;

		/// The minimum number of players in a group.
		///
		/// When scheduling a game the value for `max_group_size` must be at least this minimum + 1.
		///
		/// In order to have no single-player groups, this value must be at least 2.
		///
		/// Note: the actual minimum number of players in a group is configured per game, it is
		/// `max_group_size - 1`.
		#[pallet::constant]
		type MinGroupSize: Get<u32>;

		/// The origin that can schedule and remove the game.
		type ManagerOrigin: EnsureOrigin<Self::RuntimeOrigin>;

		/// The origin that can issue game invites.
		type InviteIssuer: EnsureOrigin<Self::RuntimeOrigin>;

		/// Origin certifying a lite person by an alias in a context, and yielding that alias.
		///
		/// [`Pallet::sign_up_with_account_lite_invite`] uses it with the context
		/// [`indiv_pallet_score::Pallet::score_context`]: lite personhood is credibility enough to play,
		/// and the alias identifies the lite person without linking to their account.
		type EnsureLiteAlias: EnsureOriginWithArg<Self::RuntimeOrigin, Context, Success = Alias>;

		/// The time after which a player that is not playing can be kicked out.
		///
		/// This is only for account players, not persons.
		#[pallet::constant]
		type NonPlayingKickoutTime: Get<BlockNumberFor<Self>>;

		/// Native chain fungible used to hold the play deposit. Only its [`Inspect::Balance`]
		/// associated type is consumed by this pallet; the actual hold is taken by
		/// [`Config::PlayDeposit`].
		type NativeFungible: Inspect<Self::AccountId>;

		/// The deposit required to play a single game, parameterised by the configured
		/// [`PlayDepositAmount`] balance.
		type PlayDeposit: Consideration<Self::AccountId, NativeBalanceOf<Self>>;
		/// The default play deposit (native balance) used when [`PlayDepositAmount`] has never
		/// been explicitly configured.
		#[pallet::constant]
		type DefaultPlayDeposit: Get<NativeBalanceOf<Self>>;

		/// The default durations of each game phase, in seconds. When
		/// `StoredPhaseDurations` is `Some`, those values take precedence
		/// over this default; the override is set at runtime via
		/// [`Pallet::set_game_phases`].
		#[pallet::constant]
		type DefaultPhaseDurations: Get<PhaseDurationValues>;

		/// The Maximum number of game schedules the pallet can store.
		#[pallet::constant]
		type MaxGameSchedules: Get<u32>;

		/// The maximum number of past games for which player attendance is stored. Any attendance
		/// entries older than this will be imminently purged from storage.
		#[pallet::constant]
		type MaxAttendanceHistoryDepth: Get<u32>;

		/// The pallet that owns the NFT claim credits a game awards.
		///
		/// Playing is what earns a credit, so the game triggers the awards, but nothing about a
		/// credit afterwards is the game's: `indiv-pallet-nft-credits` holds them, commits them to
		/// per-block Merkle roots and delivers those to the chain that mints against them. A
		/// runtime that plays games without minting anything sets this to `()`.
		type NftClaimCredits: AwardCredits<Self::AccountId>;

		/// Signature used to verify tickets.
		type TicketSignature: Verify<Signer: IdentifyAccount<AccountId: Parameter + MaxEncodedLen + Send + Sync>>
			+ Parameter
			+ Send
			+ Sync;

		/// The limit for the statement store usage for each player.
		type PlayerStatementLimit: Get<StatementAllowance>;

		/// Signature for accounts.
		type AccountSignature: Verify<Signer: IdentifyAccount<AccountId = Self::AccountId>>
			+ Parameter
			+ Send
			+ Sync;

		/// The weight for the vote of recognized people.
		type PeopleVoteWeight: Get<u8>;

		/// The weight for the vote of candidates not yet recognized.
		type CandidateVoteWeight: Get<u8>;

		/// Asset id used by [`Self::Airdrop`].
		type AirdropAssetId: Parameter + MaxEncodedLen + Default;

		/// Asset balance used by [`Self::Airdrop`].
		type AirdropAssetBalance: Parameter + MaxEncodedLen + Copy + Default + From<u32>;

		/// Utility to airdrop players that become or stay recognized after each game.
		type Airdrop: Airdrop<
			Self::AccountId,
			Self::AirdropAssetId,
			Self::AirdropAssetBalance,
			EventInfo = AirdropEventInfoOf<Self>,
			Proof: Parameter + codec::DecodeWithMemTracking,
		>;

		/// Account that funds the per-game airdrop prize allocation
		/// (`max_winners × asset_amount` is transferred per scheduled game).
		#[pallet::constant]
		type AirdropSource: Get<Self::AccountId>;

		#[cfg(feature = "runtime-benchmarks")]
		type BenchmarkHelper: BenchmarkHelper<
			Self::AccountSignature,
			Self::TicketSignature,
			TicketOf<Self>,
			Self::AccountId,
			Self::AirdropAssetId,
		>;
	}

	#[pallet::extra_constants]
	impl<T: Config> Pallet<T> {
		/// The base message of the proof of ownership of an account by an alias.
		///
		/// The full message is this base concatenated to the alias and then hashed with
		/// `blake2_256` (blake2 with 256 bit output).
		pub fn proof_of_ownership_msg_base() -> [u8; 32] {
			*b"pop:game:stmt_account_for_alias:"
		}

		/// Maximum number of full early-attendance enactments a single `report` call can
		/// trigger: every unique co-player across all rounds, plus the reporter.
		///
		/// Used as the pre-dispatch overcharge bound on `report`.
		pub fn max_enactments() -> u32 {
			Self::max_attestations(T::MaxRounds::get(), T::MaxGroupSize::get()).saturating_add(1)
		}

		/// [`Self::max_attestations`] over any game the runtime allows: the most votes a single
		/// player can receive, and equally the most co-players they can report on.
		pub(crate) fn max_received_votes() -> u32 {
			Self::max_attestations(T::MaxRounds::get(), T::MaxGroupSize::get())
		}

		/// The base string for the airdrop event id derivation. The actual event id is this base
		/// concatenated with the airdrop index and the game index BE encoded (see
		/// [`Pallet::airdrop_event_id`]).
		pub fn airdrop_event_id_base() -> [u8; 27] {
			*b"pop:game:airdrop:          "
		}
	}

	#[pallet::origin]
	#[derive(
		CloneNoBound,
		PartialEqNoBound,
		EqNoBound,
		DebugNoBound,
		Encode,
		Decode,
		MaxEncodedLen,
		TypeInfo,
		DecodeWithMemTracking,
	)]
	#[scale_info(skip_type_params(T))]
	pub enum Origin<T: Config> {
		/// An invited origin.
		///
		/// This allows to dispatch the call `sign_up_with_invite` using an invite and avoid paying
		/// the deposit and transaction fees.
		/// This origin can be enabled from the transaction extension [`GameAsInvited`].
		Invited(T::AccountId),
	}

	/// Phase-duration override set by [`Config::ManagerOrigin`] via
	/// `set_game_phases`. `None` means the chain falls back to
	/// `T::DefaultPhaseDurations`.
	#[pallet::storage]
	pub(crate) type StoredPhaseDurations<T: Config> = StorageValue<_, PhaseDurationValues>;

	/// The configured native balance held as deposit when an account player signs up.
	///
	/// This value is only used for new deposit creations; existing active deposits
	/// retain the amount they were created with.
	#[pallet::storage]
	pub type PlayDepositAmount<T: Config> =
		StorageValue<_, NativeBalanceOf<T>, ValueQuery, T::DefaultPlayDeposit>;

	/// All the player with zero score but still onboarded in indiv_pallet_score.
	#[pallet::storage]
	pub type ArchivedPlayers<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		AccountOrPerson<T::AccountId>,
		ArchivedPlayer<BlockNumberFor<T>>,
	>;

	/// All the player in the game.
	///
	/// Account-based player accounts in this storage and statement accounts in
	/// [`StmtAccountToAlias`] must not overlap.
	#[pallet::storage]
	pub type Players<T: Config> =
		StorageMap<_, Blake2_128Concat, AccountOrPerson<T::AccountId>, Player<T::PlayDeposit>>;

	/// The current game index.
	#[pallet::storage]
	pub type GameIndex<T: Config> = StorageValue<_, GameIdx, ValueQuery>;

	/// The information for the next game or ongoing game.
	#[pallet::storage]
	pub type Game<T: Config> = StorageValue<_, GameInfo<T::AccountId>>;

	/// The mapping from past games, identified by their game index, to the start timestamp, in
	/// seconds since the UNIX epoch.
	#[pallet::storage]
	pub(crate) type GameHistory<T: Config> = StorageMap<_, Twox64Concat, GameIdx, u32>;

	/// Entries of previously attended games of each player. Retained on `offboard`
	/// and `kickout`. Bounded by `MaxAttendanceHistoryDepth` per player.
	#[pallet::storage]
	pub(crate) type PlayerAttendanceHistory<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		AccountOrPerson<T::AccountId>,
		BoundedVec<GameIdx, T::MaxAttendanceHistoryDepth>,
		ValueQuery,
	>;

	/// Count of confirmed attendees per game index. Incremented by
	/// [`Pallet::note_attendance`] for every attendance entry, regardless of
	/// whether the player is recorded as `AccountOrPerson::Person` or
	/// `AccountOrPerson::Account`. Exposed for off-chain consumers.
	#[pallet::storage]
	pub type GameParticipantCount<T: Config> =
		StorageMap<_, Twox64Concat, GameIdx, u32, ValueQuery>;

	/// The mapping from round index and player index to player.
	#[pallet::storage]
	pub type IndexToPlayer<T: Config> =
		StorageMap<_, Twox64Concat, (RoundIndex, PlayerIndex), AccountOrPerson<T::AccountId>>;

	/// The mapping from player to their indices in each round.
	#[pallet::storage]
	pub type PlayerToIndex<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		AccountOrPerson<T::AccountId>,
		BoundedVec<PlayerIndex, T::MaxRounds>,
	>;

	/// Storage used to compute the shuffle order of recognized people.
	#[pallet::storage]
	pub(crate) type ShuffleRecognized<T: Config> = StorageDoubleMap<
		_,
		Twox64Concat,
		RoundIndex,
		Identity,
		ShufflePositionKey,
		AccountOrPerson<T::AccountId>,
	>;

	/// Storage used to compute the shuffle order of not recognized people, i.e. candidates.
	#[pallet::storage]
	pub(crate) type ShuffleNotRecognized<T: Config> = StorageDoubleMap<
		_,
		Twox64Concat,
		RoundIndex,
		Identity,
		ShufflePositionKey,
		AccountOrPerson<T::AccountId>,
	>;

	#[pallet::storage]
	pub type GameSchedules<T: Config> =
		StorageValue<_, BoundedVec<GameScheduleOf<T>, T::MaxGameSchedules>, ValueQuery>;

	/// Number of invites available to distribute for an account.
	#[pallet::storage]
	pub type AvailableInvites<T: Config> =
		StorageMap<_, Blake2_128Concat, T::AccountId, u32, ValueQuery>;

	/// Pending invites for each inviter.
	///
	/// If the key `(inviter, ticket)` exists then the ticket is currently a pending invite, from
	/// the inviter.
	#[pallet::storage]
	pub(crate) type PendingInvites<T: Config> =
		StorageDoubleMap<_, Blake2_128Concat, T::AccountId, Blake2_128Concat, TicketOf<T>, ()>;

	/// The account each lite person designated to play with, keyed by the lite person's alias in
	/// [`indiv_pallet_score::Pallet::score_context`].
	#[pallet::storage]
	pub type LiteInvites<T: Config> = StorageMap<_, Blake2_128Concat, Alias, T::AccountId>;

	/// Mapping from alias to the account id to use for interacting with the statement store.
	///
	/// Account-based player simply use their account and are not part of this mapping.
	/// Reverse mapping is available in storage [`StmtAccountToAlias`].
	///
	/// This is removed when the alias-based player gets archived.
	/// This is updated when the alias-based player signs up with another statement account.
	#[pallet::storage]
	pub(crate) type AliasToStmtAccount<T: Config> =
		StorageMap<_, Blake2_128Concat, Alias, T::AccountId>;

	/// Mapping from the account id to use for interacting with statement store to the alias.
	///
	/// Account-based player simply use their account and are not part of this mapping.
	/// Reverse mapping is available in storage [`AliasToStmtAccount`].
	/// This is removed when the alias-based player gets archived.
	/// This is updated when the alias-based player signs up with another statement account.
	///
	/// Statement accounts in this storage and account-based player accounts in [`Players`] must
	/// not overlap.
	#[pallet::storage]
	pub(crate) type StmtAccountToAlias<T: Config> =
		StorageMap<_, Blake2_128Concat, T::AccountId, Alias>;

	/// The communication identifiers used by players to establish an encrypted P2P connection in
	/// order to play the game. The account under which the communication identifier is registered
	/// should be the same account used to interact with the statement store.
	#[pallet::storage]
	pub(crate) type CommunicationIdentifiers<T: Config> =
		StorageMap<_, Blake2_128Concat, T::AccountId, CommunicationIdentifier>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// A new game is starting.
		NewGame { registration_ends: u32, game_date: u32, report_ends: u32 },
		/// The game and its post-process has ended.
		GameEnded { index: GameIdx },
		/// The game phase durations were overridden by [`Config::ManagerOrigin`].
		GamePhasesSet { phases: PhaseDurationValues },
		/// A player signed up for the game.
		SignedUp { who: AccountOrPerson<T::AccountId> },
		/// A player submitted their report.
		ReportSubmitted { who: AccountOrPerson<T::AccountId>, game_index: GameIdx },
		/// A player offboarded from the game.
		Offboarded { who: AccountOrPerson<T::AccountId> },
		/// An archived player was kicked out.
		KickedOut { player: AccountOrPerson<T::AccountId> },
		/// Invites were granted to an account.
		InvitesGranted { account: T::AccountId, count: u32 },
		/// An invite ticket was set.
		InviteTicketSet { inviter: T::AccountId },
		/// An invite ticket was cancelled.
		InviteTicketCancelled { inviter: T::AccountId },
		/// A lite person invited the account `player` to play on their behalf, which is now a
		/// player with an invited credibility.
		LiteInvited { player: T::AccountId },
		/// Games were scheduled.
		GamesScheduled { count: u32 },
		/// A scheduled game was removed.
		ScheduledGameRemoved { game_play_time: u32 },
		/// Statement store usage removed for the account.
		StmtUsageRemoved { who: [u8; 32] },
		/// All invites have been removed for the inviter.
		AllInvitesRemoved { inviter: T::AccountId },
		/// Some invites have been removed for the inviter, some are remaining.
		SomeInvitesRemoved { inviter: T::AccountId },
		/// The configured play deposit was updated.
		PlayDepositSet { amount: NativeBalanceOf<T> },
		/// An airdrop event was scheduled for the current game.
		AirdropScheduled { game_index: GameIdx, airdrop_index: u8, event_id: AirdropEventId },
		/// An airdrop event for the current game failed to schedule.
		AirdropScheduleFailed { game_index: GameIdx, airdrop_index: u8, error: DispatchError },
		/// Game `game_index` was cancelled.
		GameCancelled { game_index: GameIdx },
	}

	#[pallet::error]
	pub enum Error<T> {
		/// Game ongoing.
		GameOngoing,
		/// No registration phase ongoing.
		NoRegistration,
		/// The setup is outdated.
		OutdatedGameSetup,
		/// Invalid setup.
		InvalidGameSetup,
		/// Invalid report.
		InvalidReport,
		/// No game ongoing.
		NoGame,
		/// No report phase ongoing.
		NoReporting,
		/// Not registered.
		NotRegistered,
		/// Player already registered.
		AlreadyRegistered,
		/// Report already sent.
		ReportAlreadySent,
		/// Operation is not valid yet.
		Early,
		/// The operation expect a player account.
		NotKickablePlayer,
		/// No archived player found.
		NoArchivedPlayer,
		/// No ticket found.
		NoTicket,
		/// No invite available.
		NoInvites,
		/// Invite is already set.
		AlreadyInvited,
		/// Not an account based player, expected an account based player.
		NotAccountPlayer,
		/// The player can't use an invite if already playing.
		UseInviteButAlreadyPlaying,
		/// The lite person already invited another account, and invites only one account ever.
		AnotherAccountInvited,
		/// The number of existing schedules and new schedules exceeds the configured limit.
		TooManyGameSchedules,
		/// The game that was supposed to be removed was not found in scheduled games.
		NoSuchGameScheduled,
		/// The statement account signature is invalid.
		InvalidStatementAccountSignature,
		/// The statement account is already in used by another player.
		StatementAccountAlreadyInUse,
		/// Internal error invalid state.
		InternalErrorInvalidState,
		/// The operation cannot be performed in the current game state.
		InvalidGameState,
		/// No player found.
		NoPlayer,
		/// The player cannot offboard while registered for a game.
		CannotOffboardWhileRegisteredForGame,
		/// Invalid state
		InvalidState,
		/// `set_play_deposit`: the supplied amount must be non-zero.
		InvalidPlayDeposit,
		InvalidAirdropVrfVariantForAccount,
		InvalidAirdropVrfVariantForRecognition,
		/// The number of supplied airdrop VRF entries does not match the number of scheduled
		/// airdrop events.
		InvalidAirdropVrfCount,
		/// `claim_airdrop`: the claimant is not recognized in pallet-score, or their most recent
		/// attended game does not match the `game_index` of the airdrop.
		NotEligibleForAirdrop,
	}

	/// A reason for this pallet placing a hold on funds.
	#[pallet::composite_enum]
	pub enum HoldReason {
		/// Native balance held as the signup deposit for account-based players.
		PlayDeposit,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn integrity_test() {
			let max_votes = Self::max_received_votes();
			assert!(
				max_votes as u64 * T::PeopleVoteWeight::get() as u64 <= u8::MAX as u64,
				"max_votes * people_vote_weight must fit in u8"
			);
			assert!(
				max_votes as u64 * T::CandidateVoteWeight::get() as u64 <= u8::MAX as u64,
				"max_votes * candidate_vote_weight must fit in u8"
			);

			// A `report` call's worst case must fit `Normal.max_extrinsic`. Its `Linear`
			// weight is extrapolated to the production maximum here: `e_max` co-player
			// entries across all rounds, with every one plus the reporter early-enacted
			// (`max_enactments() == e_max + 1`). If a runtime bumps `MaxRounds` or
			// `MaxGroupSize` past what the block can hold, this fails at `construct_runtime!`
			// time rather than as silently-dropped `report` transactions in production.
			//
			// `e_max` is the worst-case *valid* report length. The pre-dispatch weight reads
			// `full_report.len()` before validation, so a maximally-filled invalid report can
			// reach `MaxRounds * MaxGroupSize` entries (one extra per round); that still fits
			// with wide margin and only ever fails validation, so bounding the valid case
			// here suffices.
			let e_max = Self::max_enactments().saturating_sub(1);
			let report_worst_case =
				<T as Config>::WeightInfo::report(e_max, Self::max_enactments());
			OcwWeightBudget::from_normal_max::<T>().assert_fits("report", report_worst_case);

			// A sign-up worst case (registering into the maximum number of airdrop events) must
			// fit `Normal.max_extrinsic`, otherwise sign-ups against a fully airdropped game are
			// unsubmittable.
			let max_airdrops: u32 = MAX_GAME_AIRDROPS.into();
			let sign_up_worst_case = <T as Config>::WeightInfo::sign_up_with_alias(max_airdrops)
				.max(<T as Config>::WeightInfo::sign_up_with_account_new(max_airdrops))
				.max(<T as Config>::WeightInfo::sign_up_with_account_recognized(max_airdrops))
				.max(<T as Config>::WeightInfo::sign_up_with_account_lite_invite(max_airdrops))
				.max(
					<T as Config>::WeightInfo::sign_up_with_invite(max_airdrops)
						.saturating_add(<T as Config>::WeightInfo::as_invited_tx_ext(max_airdrops)),
				);
			OcwWeightBudget::from_normal_max::<T>().assert_fits("sign_up", sign_up_worst_case);
		}

		fn on_idle(n: BlockNumberFor<T>, remaining_weight: Weight) -> Weight {
			// Use at most 50% of available weight to be cautious about weight
			// underestimates.
			let budget = remaining_weight / 2;
			let mut meter = WeightMeter::with_limit(budget);
			Self::do_on_idle(n, &mut meter);
			meter.consumed()
		}

		fn on_poll(n: BlockNumberFor<T>, weight_meter: &mut WeightMeter) {
			let budget = weight_meter.remaining() / 2;
			let mut meter = WeightMeter::with_limit(budget);
			Self::do_on_poll(n, &mut meter);
			weight_meter.consume(meter.consumed());
		}

		fn offchain_worker(_block_number: BlockNumberFor<T>) {
			// Remove the statements for a specific account id when the event is emitted.
			for event in frame_system::Pallet::<T>::read_events_no_consensus() {
				if let Ok(Event::<T>::StmtUsageRemoved { who }) = event.event.try_into() {
					statement_store::remove_by(who);
				}
			}
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Sign up for the game using an account and an invite.
		///
		/// This is for new players or archived players, other players should use
		/// [`Pallet::sign_up_with_account`] for free.
		///
		/// A game must be ongoing and in its registration phase.
		///
		/// `airdrops` optionally enters the player into the airdrop draws scheduled for this
		/// game. Pass `None` to skip it; otherwise it holds exactly one VRF per scheduled
		/// airdrop event (`airdrops_scheduled` in the [`Game`] storage), in airdrop-index order.
		/// Each entry is bound to its own event id (see [`Pallet::airdrop_event_id`]), seeds the
		/// player's draw slot for that event and proves their identity path: the alias variant if
		/// the player is recognized (pallet-score `recognition` is `Recognized` or
		/// `ExternallyRecognized`), otherwise the account variant. The sign-up fails entirely if
		/// any event does not accept its registration or the number of supplied VRFs does not
		/// match `airdrops_scheduled`. See the documentation of [`AirdropVrfs`] for more details.
		///
		/// The origin must be a signed by an account and use the `GameAsInvited` extension.
		#[pallet::call_index(0)]
		#[pallet::weight(<T as Config>::WeightInfo::sign_up_with_invite(
			airdrops.as_ref().map_or(0, AirdropVrfs::count) as u32
		))]
		pub fn sign_up_with_invite(
			origin: OriginFor<T>,
			identifier_key: CommunicationIdentifier,
			airdrops: Option<AirdropVrfs<AirdropProofOf<T>>>,
		) -> DispatchResultWithPostInfo {
			let who = match origin.clone().into_caller().try_into() {
				Ok(Origin::Invited(account)) => account,
				_ => return Err(DispatchError::BadOrigin.into()),
			};

			Self::sign_up_inner(SignUpArgs::Account {
				who,
				identifier_key,
				new_invited: true,
				airdrops,
			})?;

			Ok(Pays::No.into())
		}

		/// Sign up for the game using an account.
		///
		/// If the player is new or archived, then a deposit will be taken from the signer.
		/// Otherwise the call is free.
		///
		/// A game must be ongoing and in its registration phase.
		///
		/// `airdrops` optionally enters the player into the airdrop draws scheduled for this
		/// game. Pass `None` to skip it; otherwise it holds exactly one VRF per scheduled
		/// airdrop event (`airdrops_scheduled` in the [`Game`] storage), in airdrop-index order.
		/// Each entry is bound to its own event id (see [`Pallet::airdrop_event_id`]), seeds the
		/// player's draw slot for that event and proves their identity path: the alias variant if
		/// the player is recognized (pallet-score `recognition` is `Recognized` or
		/// `ExternallyRecognized`), otherwise the account variant. The sign-up fails entirely if
		/// any event does not accept its registration or the number of supplied VRFs does not
		/// match `airdrops_scheduled`. See the documentation of [`AirdropVrfs`] for more details.
		///
		/// The origin must be signed by an account, or, be signed by an account and use
		/// `ScoreAsParticipant` extension.
		#[pallet::call_index(1)]
		#[pallet::weight(match airdrops {
			Some(AirdropVrfs::Account(vrfs)) =>
				<T as Config>::WeightInfo::sign_up_with_account_new(vrfs.len() as u32),
			Some(AirdropVrfs::Alias { proofs, .. }) =>
				<T as Config>::WeightInfo::sign_up_with_account_recognized(proofs.len() as u32),
			None => <T as Config>::WeightInfo::sign_up_with_account_new(0)
				.max(<T as Config>::WeightInfo::sign_up_with_account_recognized(0)),
		})]
		pub fn sign_up_with_account(
			origin: OriginFor<T>,
			identifier_key: CommunicationIdentifier,
			airdrops: Option<AirdropVrfs<AirdropProofOf<T>>>,
		) -> DispatchResultWithPostInfo {
			let who = indiv_pallet_score::Pallet::<T>::ensure_signed_or_participant(origin)?;

			Self::sign_up_inner(SignUpArgs::Account {
				who,
				identifier_key,
				new_invited: false,
				airdrops,
			})?;

			Ok(Pays::No.into())
		}

		/// Sign up for the game.
		///
		/// If a player is already recognized by another DIM, they can sign using their alias and
		/// don't need any deposit or invite to prove their initial credibility.
		/// On top of this their score is never going below personhood threshold and the player will
		/// never get archived.
		///
		/// A game must be ongoing and in its registration phase.
		///
		/// The origin must be a personal alias.
		///
		/// Parameters:
		/// - `statement_account`: the account id to use to interact with the statement store during
		///   the game.
		/// - `sig`: the proof of ownership of the account `statement_account` by an alias, it is
		///   the signature of the message `"pop:game:stmt_account_for_alias:"` concatenated to the
		///   alias, and then hashed with `blake2_256` (blake2 256bit output). The base of the
		///   message can be found in the constant: `proof_of_ownership_msg_base`.
		/// - `airdrops`: optionally enters the player into the airdrop draws scheduled for this
		///   game. Pass `None` to skip it; otherwise it holds exactly one VRF per scheduled airdrop
		///   event (`airdrops_scheduled` in the `Game` storage), in airdrop-index order. Each entry
		///   is bound to its own event id (see `Pallet::airdrop_event_id`) and seeds the player's
		///   draw slot for that event; the alias variant must be used for alias-based players given
		///   they are recognized in pallet-score participant information. The sign-up fails
		///   entirely if any event does not accept its registration or the number of supplied VRFs
		///   does not match `airdrops_scheduled`. See the documentation of `AirdropVrfs` for more
		///   details.
		#[pallet::call_index(2)]
		#[pallet::weight(<T as Config>::WeightInfo::sign_up_with_alias(
			airdrops.as_ref().map_or(0, AirdropVrfs::count) as u32
		))]
		pub fn sign_up_with_alias(
			origin: OriginFor<T>,
			identifier_key: CommunicationIdentifier,
			statement_account: T::AccountId,
			sig: T::AccountSignature,
			airdrops: Option<AirdropVrfs<AirdropProofOf<T>>>,
		) -> DispatchResultWithPostInfo {
			let who = indiv_pallet_score::Pallet::<T>::ensure_person(origin)?;

			Self::sign_up_inner(SignUpArgs::Alias {
				who,
				identifier_key,
				statement_account,
				statement_account_signature: sig,
				airdrops,
			})?;

			Ok(Pays::No.into())
		}

		/// Sign up for the game using an account and a lite invite, the invitation of a lite
		/// person.
		///
		/// Lite personhood is credible enough to play, so a lite person can invite an account of
		/// their own. It is registered as an account-based player with an invited credibility.
		///
		/// A game must be ongoing and in its registration phase.
		///
		/// `airdrops` optionally enters the player into the airdrop draws scheduled for this
		/// game. Pass `None` to skip it; otherwise it holds exactly one VRF per scheduled
		/// airdrop event (`airdrops_scheduled` in the [`Game`] storage), in airdrop-index order.
		/// Each entry is bound to its own event id (see [`Pallet::airdrop_event_id`]), seeds the
		/// player's draw slot for that event and proves their identity path: the alias variant if
		/// the player is recognized (pallet-score `recognition` is `Recognized` or
		/// `ExternallyRecognized`), otherwise the account variant. The sign-up fails entirely if
		/// any event does not accept its registration or the number of supplied VRFs does not
		/// match `airdrops_scheduled`. See the documentation of [`AirdropVrfs`] for more details.
		///
		/// The origin must be a lite alias in [`indiv_pallet_score::Pallet::score_context`], see
		/// [`Config::EnsureLiteAlias`]. The lite person names the account playing on their behalf
		/// with `account`, so the playing account is not the lite person's own account and stays
		/// unlinkable from it.
		///
		/// A lite person invites one account, ever: the first account they sign up this way is the
		/// only one they can play with, even after it stopped playing.
		///
		/// This must be used by lite-people on their first signup or after they have been
		/// archived. Further signups while the player is active must use
		/// [`Pallet::sign_up_with_account`] instead.
		#[pallet::call_index(21)]
		#[pallet::weight(<T as Config>::WeightInfo::sign_up_with_account_lite_invite(
			airdrops.as_ref().map_or(0, AirdropVrfs::count) as u32
		))]
		pub fn sign_up_with_account_lite_invite(
			origin: OriginFor<T>,
			account: T::AccountId,
			identifier_key: CommunicationIdentifier,
			airdrops: Option<AirdropVrfs<AirdropProofOf<T>>>,
		) -> DispatchResultWithPostInfo {
			let alias = T::EnsureLiteAlias::ensure_origin(
				origin,
				&indiv_pallet_score::Pallet::<T>::score_context(),
			)?;

			if let Some(invited) = LiteInvites::<T>::get(alias) {
				ensure!(invited == account, Error::<T>::AnotherAccountInvited);
			}

			Self::sign_up_inner(SignUpArgs::Account {
				who: account.clone(),
				identifier_key,
				new_invited: true,
				airdrops,
			})?;

			LiteInvites::<T>::insert(alias, &account);

			Self::deposit_event(Event::<T>::LiteInvited { player: account });

			Ok(Pays::No.into())
		}

		/// After the game, send the full report.
		///
		/// The game must be ongoing and in its reporting phase.
		///
		/// The origin must be an alias, or signed by an account, or signed by an account and use
		/// `ScoreAsParticipant` extension.
		///
		/// `full_report` holds one entry per round (exactly `game.rounds` of them), and each
		/// round lists one `Report` per co-player in the reporter's group for that round, in
		/// group order and excluding the reporter. A round whose length does not match the
		/// reporter's real group membership is rejected with `Error::InvalidReport`. Each
		/// `Report::Person` vote awards an attendance NFT claim credit to the attestee immediately;
		/// `Report::NotPerson` awards nothing.
		///
		/// After the votes from the report are counted, the reporter and each of the reported
		/// players whose attendance can now be determined are processed early. This lets the
		/// game skip the player-process phase entirely when every player has been processed by
		/// the end of reporting.
		///
		/// On success the call pays no fee (`Pays::No`). Its weight scales with the number of
		/// co-player entries across all rounds, not the round count, so partial groups are not
		/// overcharged. The pre-dispatch charge assumes every co-player entry plus the reporter
		/// is early-enacted and is refunded down to the actual enactment count post-dispatch.
		#[pallet::call_index(3)]
		#[pallet::weight({
			// `e` is the exact number of co-player entries in the submitted report (its
			// flattened length). It equals the reporter's real group membership across all
			// rounds, so it is not overcharged for partial groups (unlike scaling by round
			// count, which assumes every group is full). All of `report`'s per-item PoV and
			// compute lives in the per-co-player inner loop, so `e` captures 100% of the
			// scaling cost. Every co-player entry, plus the reporter, may be early-enacted,
			// so the pre-dispatch enactment bound is `e + 1`.
			let e = full_report.iter().map(|round| round.len() as u32).sum::<u32>();
			let n = e.saturating_add(1);
			<T as Config>::WeightInfo::report(e, n)
		})]
		pub fn report(
			origin: OriginFor<T>,
			full_report: FullReport<T>,
		) -> DispatchResultWithPostInfo {
			let who =
				indiv_pallet_score::Pallet::<T>::ensure_signed_or_participant_or_person(origin)?;

			let game = Game::<T>::get().ok_or(Error::<T>::NoGame)?;
			let game_index = game.index;
			let now = T::UnixTime::now();
			ensure!(now < Duration::from_secs(game.report_ends as u64), Error::<T>::NoReporting);
			let award_time = now.as_secs() as u32;
			let group_size = game.max_group_size;
			let GameState::Reporting { player_count } = game.state else {
				return Err(Error::<T>::NoReporting.into());
			};

			// Use the vote weight snapshotted during the shuffle phase rather than a dynamic
			// `reached_personhood` lookup: if the reporter was early-enacted as `Attended`
			// mid-reporting and crossed the personhood threshold, their live weight would
			// exceed the `expected_max_vote_weight` cached for other players, breaking the
			// bound used in `determine_attendance`.
			let reporter_vote_weight =
				Players::<T>::mutate::<_, Result<_, Error<T>>, _>(&who, |player_info| {
					let player_info = player_info.as_mut().ok_or(Error::<T>::NotRegistered)?;
					ensure!(!player_info.sent_report, Error::<T>::ReportAlreadySent);
					player_info.sent_report = true;
					Ok(player_info.vote_weight)
				})?;

			ensure!(full_report.len() == game.rounds as usize, Error::<T>::InvalidReport);

			// Number of co-player entries across all rounds. The loop below validates that each
			// round's report length matches the reporter's real group membership, so this
			// flattened length is the exact per-item cost driver charged as `e` in the weight.
			let co_player_entries = full_report.iter().map(|round| round.len() as u32).sum::<u32>();

			let reporter_indices =
				PlayerToIndex::<T>::get(&who).ok_or(Error::<T>::NotRegistered)?;

			// Collect all players that received a new vote during this report so we can
			// check their attendance once, after the loop updates their tallies. A linear
			// `contains` scan is deliberate: `v` is bounded by `MaxRounds * (MaxGroupSize - 1)`
			// (15 in production), where a flat cache-local scan beats a `BTreeSet` (measured
			// ~2x faster at that size), the quadratic term is negligible in-memory work.
			let mut reported_players: Vec<AccountOrPerson<T::AccountId>> = Vec::new();

			for round in 0..game.rounds {
				let mut report_iter =
					full_report.get(round as usize).ok_or(Error::<T>::InvalidReport)?.iter();

				let &reporter_index = reporter_indices
					.get(round as usize)
					.defensive_proof(
						"indiv-pallet-game: player should have an index for each round",
					)
					.ok_or(Error::<T>::InternalErrorInvalidState)?;

				let groups_setting = GroupsSetting { max_per_group: group_size, player_count };

				let group_index = groups_setting.group_index_from_player_index(reporter_index);
				// The reporter's place in their group. They are the attester of every credit
				// this round awards, so this is the slot all of them take, and the one the
				// attendance backfill reads off the group for this same player.
				let attester_position = groups_setting
					.group_members(group_index)
					.position(|index| index == reporter_index)
					.defensive_proof("indiv-pallet-game: reporter is a member of their own group")
					.ok_or(Error::<T>::InternalErrorInvalidState)?
					as AttesterPosition;
				let other_people_in_group =
					groups_setting.group_members(group_index).filter(|&i| i != reporter_index);

				let mut log_other_people_in_group = vec![];
				for reported_index in other_people_in_group {
					log_other_people_in_group.push(reported_index);
					let report = report_iter.next().ok_or(Error::<T>::InvalidReport)?;
					let reported_player = IndexToPlayer::<T>::get((round, reported_index))
						.defensive_proof("indiv-pallet-game: index should map to a player")
						.ok_or(Error::<T>::InternalErrorInvalidState)?;
					Players::<T>::mutate(&reported_player, |reported_player_info| {
						let reported_player_info = reported_player_info
							.as_mut()
							.defensive_proof("indiv-pallet-game: player should exist")
							.ok_or(Error::<T>::InternalErrorInvalidState)?;
						match report {
							Report::Person =>
								reported_player_info.yes_person = reported_player_info
									.yes_person
									.saturating_add(reporter_vote_weight),
							Report::NotPerson =>
								reported_player_info.no_not_person = reported_player_info
									.no_not_person
									.saturating_add(reporter_vote_weight),
						}
						Result::<_, Error<T>>::Ok(())
					})?;

					// A `Person` vote awards immediately: the attestee earns this credit
					// regardless of their final attendance. A `NotPerson` vote awards
					// nothing at this point. If the attestee ends up attending, the credit
					// is backfilled by `award_attendance_credits` when their attendance is
					// finalized.
					if *report == Report::Person {
						T::NftClaimCredits::award_report_credit(
							game_index,
							round,
							&who,
							&reported_player,
							attester_position,
							award_time,
						);
					}

					if !reported_players.contains(&reported_player) {
						reported_players.push(reported_player);
					}
				}

				ensure!(report_iter.next().is_none(), Error::<T>::InvalidReport);

				log::trace!(
					target: LOG_TARGET,
					"Report: reporter {reporter_index}, round {round}, reported \
					{log_other_people_in_group:?}",
				)
			}

			Self::deposit_event(Event::<T>::ReportSubmitted { who: who.clone(), game_index });

			let mut enacted_count: u32 = 0;
			for reported_player in reported_players {
				if Self::try_early_attendance_enactment(&reported_player) {
					enacted_count = enacted_count.saturating_add(1);
				}
			}

			// And the reporter themselves, whose `sent_report` is now `true`.
			if Self::try_early_attendance_enactment(&who) {
				enacted_count = enacted_count.saturating_add(1);
			}

			Ok(PostDispatchInfo {
				actual_weight: Some(<T as Config>::WeightInfo::report(
					co_player_entries,
					enacted_count,
				)),
				pays_fee: Pays::No,
			})
		}

		/// Offboard a player from the game.
		///
		/// The origin must be an alias, or signed by an account, or signed by an account and use
		/// `ScoreAsParticipant` extension.
		///
		/// There must be no game or the existing game must be in registration phase and the player
		/// must have not signed up for the game.
		///
		/// A player whose pallet-score recognition is `Recognized` has their personhood suspended,
		/// the same as an absence-triggered suspension. Offboarding is permanent. Playing as a
		/// person again after a suspension and then re-recognition requires onboarding under a new
		/// personal id with a new key.
		#[pallet::call_index(4)]
		#[pallet::weight(
			<T as Config>::WeightInfo::offboard_account()
				.max(<T as Config>::WeightInfo::offboard_person())
		)]
		pub fn offboard(origin: OriginFor<T>) -> DispatchResultWithPostInfo {
			let who =
				indiv_pallet_score::Pallet::<T>::ensure_signed_or_participant_or_person(origin)?;

			let archived = ArchivedPlayers::<T>::take(&who);
			let player = Players::<T>::take(&who);
			let player_is_offboarding = player.is_some();
			// `PlayerAttendanceHistory` is retained on offboard so an identity that
			// attended a game keeps its recorded attendance.
			let game = Game::<T>::get();

			let game_is_none_or_registration =
				game.is_none_or(|g| matches!(g.state, GameState::Registration { .. }));
			ensure!(game_is_none_or_registration, Error::<T>::InvalidGameState);

			let player_is_none_or_not_registered = player.as_ref().is_none_or(|p| !p.registered);
			ensure!(
				player_is_none_or_not_registered,
				Error::<T>::CannotOffboardWhileRegisteredForGame
			);

			ensure!(archived.is_some() || player.is_some(), Error::<T>::NoPlayer);

			if let Some(player) = player {
				if let PlayerCredibility::Deposit(play_deposit) = player.credibility {
					if let AccountOrPerson::Account(account) = &who {
						play_deposit.drop(account)?;
					} else {
						defensive!("person player should not have deposit");
					}
				}
			}

			let stmt_account = match &who {
				AccountOrPerson::Person(alias) => AliasToStmtAccount::<T>::take(alias)
					.defensive_proof("pallet-game: alias player must have a statement account")
					.inspect(|stmt_account| StmtAccountToAlias::<T>::remove(stmt_account)),
				AccountOrPerson::Account(account) => Some(account.clone()),
			};

			if let Some(stmt_account) = stmt_account {
				if player_is_offboarding {
					decrease_allowance_by(
						stmt_account.clone().into(),
						T::PlayerStatementLimit::get(),
					);
				}
				Self::deposit_event(Event::<T>::StmtUsageRemoved { who: stmt_account.into() });
			}

			indiv_pallet_score::Pallet::<T>::offboard(&who)?;

			let actual_weight = match &who {
				AccountOrPerson::Account(_) => <T as Config>::WeightInfo::offboard_account(),
				AccountOrPerson::Person(_) => <T as Config>::WeightInfo::offboard_person(),
			};

			Self::deposit_event(Event::<T>::Offboarded { who });

			Ok(Some(actual_weight).into())
		}

		/// Kickout a kickable player that is not playing after `NonPlayingKickoutTime`.
		///
		/// The origin must be signed by an account.
		///
		/// A player whose pallet-score recognition is `Recognized` has their personhood suspended,
		/// the same as an absence-triggered suspension. The kickout is permanent. Playing as a
		/// person again after a suspension and then re-recognition requires onboarding under a new
		/// personal id with a new key.
		///
		/// - `player`: The player to kickout. It must be archived and kickable with
		///   `archived_since` older than `NonPlayingKickoutTime`.
		// This call could be made unsigned if needed
		#[pallet::call_index(5)]
		#[pallet::weight(<T as Config>::WeightInfo::kickout())]
		pub fn kickout(origin: OriginFor<T>, player: T::AccountId) -> DispatchResultWithPostInfo {
			ensure_signed(origin)?;

			let player = AccountOrPerson::Account(player);

			let archived =
				ArchivedPlayers::<T>::take(&player).ok_or(Error::<T>::NoArchivedPlayer)?;
			// See `offboard`: `PlayerAttendanceHistory` is retained.

			let ArchivedPlayer::Kickable { archived_since, .. } = archived else {
				return Err(Error::<T>::NotKickablePlayer.into());
			};
			let now = frame_system::Pallet::<T>::block_number();

			ensure!(
				now > archived_since.saturating_add(T::NonPlayingKickoutTime::get()),
				Error::<T>::Early
			);

			indiv_pallet_score::Pallet::<T>::offboard(&player)?;

			Self::deposit_event(Event::<T>::KickedOut { player });

			Ok(Pays::No.into())
		}

		/// Grant some invites to an account so they can distribute them.
		///
		/// The origin must be `InviteIssuer`.
		///
		/// - `account`: The account to grant invites to.
		/// - `count`: The number of invites to grant.
		#[pallet::call_index(6)]
		#[pallet::weight(<T as Config>::WeightInfo::grant_invites())]
		pub fn grant_invites(
			origin: OriginFor<T>,
			account: T::AccountId,
			count: u32,
		) -> DispatchResult {
			T::InviteIssuer::ensure_origin(origin)?;

			let mut available = AvailableInvites::<T>::get(&account);

			available = available.saturating_add(count);

			AvailableInvites::<T>::insert(&account, available);

			Self::deposit_event(Event::<T>::InvitesGranted { account, count });

			Ok(())
		}

		/// Clear all invites given to an account.
		///
		/// The origin must be `InviteIssuer`.
		///
		/// - `account`: The account to remove all invites from.
		/// - `limit`: The maximum number of pending invites to remove.
		#[pallet::call_index(7)]
		#[pallet::weight(<T as Config>::WeightInfo::remove_available_and_pending_invites(*limit))]
		pub fn remove_available_and_pending_invites(
			origin: OriginFor<T>,
			account: T::AccountId,
			limit: u32,
		) -> DispatchResult {
			T::InviteIssuer::ensure_origin(origin)?;

			AvailableInvites::<T>::remove(&account);
			let res = PendingInvites::<T>::clear_prefix(&account, limit, None);
			if res.maybe_cursor.is_some() {
				Self::deposit_event(Event::SomeInvitesRemoved { inviter: account });
			} else {
				Self::deposit_event(Event::AllInvitesRemoved { inviter: account });
			}

			Ok(())
		}

		/// Invite an account.
		///
		/// The origin must be signed by an account and have some invites left. The call is free on
		/// success.
		///
		/// - `ticket`: The invite ticket to set.
		#[pallet::call_index(8)]
		#[pallet::weight(<T as Config>::WeightInfo::set_invite_ticket())]
		pub fn set_invite_ticket(
			origin: OriginFor<T>,
			ticket: TicketOf<T>,
		) -> DispatchResultWithPostInfo {
			let inviter = ensure_signed(origin)?;

			let mut available = AvailableInvites::<T>::get(&inviter);
			available = available.checked_sub(1).ok_or(Error::<T>::NoInvites)?;

			ensure!(
				!PendingInvites::<T>::contains_key(&inviter, &ticket),
				Error::<T>::AlreadyInvited
			);

			PendingInvites::<T>::insert(&inviter, &ticket, ());
			if available > 0 {
				AvailableInvites::<T>::insert(&inviter, available);
			} else {
				AvailableInvites::<T>::remove(&inviter);
			}

			Self::deposit_event(Event::<T>::InviteTicketSet { inviter });

			Ok(Pays::No.into())
		}

		/// Cancel an invite.
		///
		/// The origin must be signed by the account that owns the ticket to cancel.
		///
		/// - `ticket`: The invite ticket to cancel.
		#[pallet::call_index(9)]
		#[pallet::weight(<T as Config>::WeightInfo::cancel_invite_ticket())]
		pub fn cancel_invite_ticket(origin: OriginFor<T>, ticket: TicketOf<T>) -> DispatchResult {
			let inviter = ensure_signed(origin)?;

			PendingInvites::<T>::take(&inviter, &ticket).ok_or(Error::<T>::NoTicket)?;

			let mut available = AvailableInvites::<T>::get(&inviter);
			available = available.saturating_add(1);
			AvailableInvites::<T>::insert(&inviter, available);

			Self::deposit_event(Event::<T>::InviteTicketCancelled { inviter });

			Ok(())
		}

		/// Schedules new games according to provided schedules.
		/// Schedules must be in chronological order, and after the ongoing game (if there is any).
		#[pallet::call_index(10)]
		#[pallet::weight(<T as Config>::WeightInfo::schedule_games(games_schedules.len() as u32))]
		pub fn schedule_games(
			origin: OriginFor<T>,
			games_schedules: Vec<GameScheduleOf<T>>,
		) -> DispatchResult {
			<T as Config>::ManagerOrigin::ensure_origin_or_root(origin)?;

			ensure!(
				GameSchedules::<T>::get().len().saturating_add(games_schedules.len()) as u32 <=
					T::MaxGameSchedules::get(),
				Error::<T>::TooManyGameSchedules
			);

			// To validate that the newly scheduled games will take place after the currently
			// planned ones.
			// Using the now value of time as default allows us to easily validate
			// that the schedules are not set in the past.
			let mut last_game_end_time = GameSchedules::<T>::get().last().map_or(
				Game::<T>::get().map_or(T::UnixTime::now(), |game| {
					Duration::from_secs(GameTimes::<T>::player_process_end(&game) as u64)
				}),
				|schedule| {
					Duration::from_secs(
						crate::types::GameTimes::<T>::player_process_end(schedule) as u64
					)
				},
			);

			for schedule in &games_schedules {
				// Checks that games do not overlap in time and that schedules were provided in
				// chronological order.

				ensure!(
					last_game_end_time <=
						Duration::from_secs(GameTimes::<T>::registration_start(schedule) as u64),
					Error::<T>::InvalidGameSetup
				);

				ensure!(schedule.rounds > 0, Error::<T>::InvalidGameSetup);
				ensure!(
					u32::from(schedule.rounds) <= T::MaxRounds::get(),
					Error::<T>::InvalidGameSetup
				);

				ensure!(
					// Note: a game is cancelled if groups can't be filled with at least
					// `max_group_size - 1`. So `>` is used here, ensuring the groups are at least
					// the required minimum size.
					schedule.max_group_size > T::MinGroupSize::get(),
					Error::<T>::InvalidGameSetup
				);
				ensure!(
					schedule.max_group_size <= T::MaxGroupSize::get(),
					Error::<T>::InvalidGameSetup
				);

				last_game_end_time =
					Duration::from_secs(GameTimes::<T>::player_process_end(schedule) as u64);
			}

			let count = games_schedules.len() as u32;
			GameSchedules::<T>::try_mutate(|current_schedules| {
				games_schedules.into_iter().try_for_each(|schedule| {
					current_schedules
						.try_push(schedule)
						.map_err(|_| Error::<T>::TooManyGameSchedules)
				})
			})?;

			Self::deposit_event(Event::<T>::GamesScheduled { count });

			Ok(())
		}

		#[pallet::call_index(11)]
		#[pallet::weight(<T as Config>::WeightInfo::remove_scheduled_game())]
		pub fn remove_scheduled_game(origin: OriginFor<T>, game_play_time: u32) -> DispatchResult {
			<T as Config>::ManagerOrigin::ensure_origin_or_root(origin)?;

			// Look for a game with the same reporting_start_time in the scheduled games and remove
			// it if it exists.
			if let Ok(index) = GameSchedules::<T>::get()
				.binary_search_by_key(&game_play_time, |schedule| schedule.game_play_time)
			{
				GameSchedules::<T>::mutate(|schedules| {
					schedules.remove(index);
				});
			} else {
				return Err(Error::<T>::NoSuchGameScheduled.into());
			}

			Self::deposit_event(Event::<T>::ScheduledGameRemoved { game_play_time });

			Ok(())
		}

		/// Update the configured play deposit amount for future account signups.
		#[pallet::call_index(16)]
		#[pallet::weight(<T as Config>::WeightInfo::set_play_deposit())]
		pub fn set_play_deposit(
			origin: OriginFor<T>,
			amount: NativeBalanceOf<T>,
		) -> DispatchResult {
			<T as Config>::ManagerOrigin::ensure_origin_or_root(origin)?;
			ensure!(!amount.is_zero(), Error::<T>::InvalidPlayDeposit);
			PlayDepositAmount::<T>::put(amount);
			Self::deposit_event(Event::<T>::PlayDepositSet { amount });
			Ok(())
		}

		/// Claim a prize from the airdrop event at index `airdrop_index` scheduled for
		/// `game_index`.
		///
		/// A game schedules one airdrop event per entry in its schedule's `airdrops` (see
		/// [`GameSchedule`]); `airdrop_index` selects which of them to claim from. A player who
		/// won several of the game's airdrop events claims each of them separately.
		///
		/// Eligibility requires 2 conditions on the claimant to be recognized and have attended the
		/// game. In more details:
		/// * to be either recognized in pallet-score (`recognition.is_recognized()`) or to have
		///   reached the personhood score (`reached_personhood`),
		/// * AND for `game_index` to match the participant's `last_attended_game` — i.e. the most
		///   recent game the claimant actually attended must be exactly the game the airdrop is
		///   tied to. Subsequent game attendance overrides this information so the claim must be
		///   made before attending another game. In particular an airdrop event drawn late relative
		///   to the game play time must be claimed before the claimant attends the next game.
		///
		/// Claims against a cancelled game are rejected.
		// If an airdrop is scheduled with a closing window later than the next game, the actual
		// airdrop claim window will be closed when the next game is played.
		//
		// We may want to relax the claimability to just: being recognized in pallet-score after
		// the game the airdrop is tied to. Effectively changing the condition
		// `last_attended_game == Some(game_index)` to `last_attended_game >= Some(game_index)`.
		#[pallet::call_index(17)]
		#[pallet::weight(<T as Config>::WeightInfo::claim_airdrop())]
		pub fn claim_airdrop(
			origin: OriginFor<T>,
			game_index: GameIdx,
			airdrop_index: u8,
			beneficiary: T::AccountId,
		) -> DispatchResultWithPostInfo {
			let claimant =
				indiv_pallet_score::Pallet::<T>::ensure_signed_or_participant_or_person(origin)?;

			let event_id = Self::airdrop_event_id(game_index, airdrop_index);
			let eligible = indiv_pallet_score::Participants::<T>::get(&claimant).is_some_and(|p| {
				(p.recognition.is_recognized() || p.reached_personhood) &&
					p.last_attended_game == Some(game_index)
			});
			ensure!(eligible, Error::<T>::NotEligibleForAirdrop);

			let registrant = match claimant {
				AccountOrPerson::Person(alias) => AirdropRegistrationEntry::Alias { alias },
				AccountOrPerson::Account(account_id) =>
					AirdropRegistrationEntry::Account { account_id },
			};

			T::Airdrop::claim(event_id, registrant, beneficiary)?;

			Ok(Pays::No.into())
		}

		/// Force start the shuffle before its normal start time.
		///
		/// This action can only be performed by the root origin and is only meant for testing.
		#[pallet::call_index(101)]
		#[pallet::weight(Weight::zero())]
		#[cfg(feature = "testnet")]
		pub fn testnet_force_start_shuffle(origin: OriginFor<T>) -> DispatchResult {
			if !T::TESTNET {
				return Err(Error::<T>::InvalidState.into());
			}

			ensure_root(origin)?;
			let mut game = Game::<T>::get().ok_or(Error::<T>::NoGame)?;
			let GameState::<T::AccountId>::Registration { .. } = game.state else {
				return Err(Error::<T>::NoRegistration.into());
			};
			let now = T::UnixTime::now();
			game.registration_ends = now.as_secs().try_into().unwrap_or(u32::MAX);
			game.state =
				GameState::Shuffle { step: ShuffleStep::Step1Insert { last_iteration: None } };
			Game::<T>::put(game);
			Ok(())
		}

		/// Force end a game's reporting phase before its normal end time.
		///
		/// This action can only be performed by the root origin and is only meant for testing.
		#[pallet::call_index(102)]
		#[pallet::weight(Weight::zero())]
		#[cfg(feature = "testnet")]
		pub fn testnet_force_end_reporting(origin: OriginFor<T>) -> DispatchResult {
			if !T::TESTNET {
				return Err(Error::<T>::InvalidState.into());
			}

			ensure_root(origin)?;
			let mut game = Game::<T>::get().ok_or(Error::<T>::NoGame)?;
			let GameState::<T::AccountId>::Reporting { player_count } = game.state else {
				return Err(Error::<T>::NoReporting.into());
			};
			let now = T::UnixTime::now();
			game.report_ends = now.as_secs().try_into().unwrap_or(u32::MAX);
			game.shuffle_deadline = core::cmp::min(game.shuffle_deadline, game.report_ends);
			game.game_date = core::cmp::min(game.game_date, game.report_ends);
			game.state = GameState::PlayerProcess {
				step: PlayerProcessStep::Step1ProcessPlayers { last_iteration: None, player_count },
			};
			Game::<T>::put(game);
			Ok(())
		}

		/// Override the game phase durations.
		///
		/// Restricted to [`Config::ManagerOrigin`] (or root). Until reset, all future
		/// game schedules use these phases instead of [`Config::DefaultPhaseDurations`].
		/// To revert, the manager re-issues the call with the desired explicit
		/// values — there is no separate clear extrinsic.
		///
		/// Only callable while no game exists or the current game is still in its
		/// Registration phase; otherwise fails with [`Error::InvalidGameState`]. This
		/// prevents changing phase durations once players have committed to a game
		/// whose timing is already locked in.
		#[pallet::call_index(14)]
		#[pallet::weight(<T as Config>::WeightInfo::set_game_phases())]
		pub fn set_game_phases(
			origin: OriginFor<T>,
			phases: PhaseDurationValues,
		) -> DispatchResult {
			<T as Config>::ManagerOrigin::ensure_origin_or_root(origin)?;
			// No game or Registration only: in both, no player is committed to the
			// timings we're about to change. The no-game case is intentional.
			if let Some(game) = Game::<T>::get() {
				ensure!(
					matches!(game.state, GameState::Registration { .. }),
					Error::<T>::InvalidGameState,
				);
			}
			StoredPhaseDurations::<T>::put(phases.clone());
			Self::deposit_event(Event::<T>::GamePhasesSet { phases });
			Ok(())
		}

		/// Cancel the current game while it is still in the Registration or Shuffle phase.
		///
		/// Restricted to [`Config::ManagerOrigin`] (or root).
		#[pallet::call_index(15)]
		#[pallet::weight(<T as Config>::WeightInfo::cancel_game())]
		pub fn cancel_game(origin: OriginFor<T>) -> DispatchResult {
			<T as Config>::ManagerOrigin::ensure_origin_or_root(origin)?;

			let mut game = Game::<T>::get().ok_or(Error::<T>::NoGame)?;
			ensure!(
				matches!(game.state, GameState::Registration { .. } | GameState::Shuffle { .. }),
				Error::<T>::InvalidGameState
			);

			Self::on_game_cancelled(&game);
			game.state = GameState::Cancelling { step: CancellingStep::Step1DrainShuffle };
			Game::<T>::put(game);

			Ok(())
		}
	}

	impl<T: Config> Pallet<T> {
		fn do_on_idle(n: BlockNumberFor<T>, weight_meter: &mut WeightMeter) {
			if weight_meter.try_consume(<T as Config>::WeightInfo::get_game()).is_err() {
				return;
			}
			if let Some(game) = Game::<T>::get() {
				Self::process_game(weight_meter, n, game);
			}
		}
		fn do_on_poll(n: BlockNumberFor<T>, weight_meter: &mut WeightMeter) {
			if weight_meter.try_consume(<T as Config>::WeightInfo::get_game()).is_err() {
				return;
			}
			let game = Game::<T>::get();
			match game {
				// If there's no ongoing game at a time, take the first scheduled game and make it
				// the next one.
				None => {
					if weight_meter
						.try_consume(<T as Config>::WeightInfo::get_game_schedules(
							T::MaxGameSchedules::get(),
						))
						.is_err()
					{
						return;
					}
					let mut schedules = <GameSchedules<T>>::get();
					if !schedules.is_empty() {
						let schedule_airdrops =
							schedules.first().map_or(0, |schedule| schedule.airdrops.len() as u32);
						let weight = <T as Config>::WeightInfo::new_game(schedule_airdrops)
							.saturating_add(<T as Config>::WeightInfo::put_game_schedules());

						if weight_meter.try_consume(weight).is_err() {
							return;
						}

						let schedule = schedules.remove(0);
						let _ = Self::new_game(&schedule).inspect_err(|e| {
							log::error!(
								target: LOG_TARGET,
								"Failed to start new game from schedule {e:?}",
							);
						});
						// Regardless the outcome of new_game call, the next scheduled game is
						// removed. If new_game call fails, regardless of the reason, that
						// game must be removed from scheduled ones as the call will never
						// succeed. The reason for failure may be that the previous game took
						// longer to finish than expected.
						GameSchedules::<T>::put(schedules);
					}
				},
				Some(game) => Self::process_game(weight_meter, n, game),
			}
		}

		/// Create a new game.
		///
		/// Parameters:
		/// - `schedule`: The schedule of the game.
		pub(crate) fn new_game(schedule: &GameScheduleOf<T>) -> DispatchResult {
			ensure!(!Game::<T>::exists(), Error::<T>::GameOngoing);

			let now = T::UnixTime::now();

			// The game must be set up before its own registration phase starts.
			let registration_starts = GameTimes::<T>::registration_start(schedule);
			let registration_ends = GameTimes::<T>::registration_end(schedule);
			let game_play_time = GameTimes::<T>::game_play_time(schedule);
			let report_ends = GameTimes::<T>::reporting_end(schedule);

			ensure!(
				now < Duration::from_secs(registration_starts as u64),
				Error::<T>::OutdatedGameSetup
			);
			ensure!(registration_ends < game_play_time, Error::<T>::InvalidGameSetup);
			ensure!(game_play_time < report_ends, Error::<T>::InvalidGameSetup);
			ensure!(schedule.rounds > 0, Error::<T>::InvalidGameSetup);
			ensure!(
				u32::from(schedule.rounds) <= T::MaxRounds::get(),
				Error::<T>::InvalidGameSetup
			);
			ensure!(
				schedule.max_group_size >= T::MinGroupSize::get(),
				Error::<T>::InvalidGameSetup
			);
			ensure!(
				schedule.max_group_size <= T::MaxGroupSize::get(),
				Error::<T>::InvalidGameSetup
			);

			let index = GameIndex::<T>::mutate(|index| {
				*index = index.saturating_add(1);
				*index
			});
			let airdrops_scheduled = Self::try_schedule_airdrops(
				index,
				// The airdrop registrations open immediately, same as the game.
				now.as_secs(),
				game_play_time as u64,
				schedule,
			);
			Game::<T>::put(GameInfo {
				index,
				registration_ends,
				game_date: game_play_time,
				shuffle_deadline: GameTimes::<T>::shuffle_deadline(schedule),
				report_ends,
				state: GameState::Registration { next_player_index: 0 },
				max_group_size: schedule.max_group_size,
				rounds: schedule.rounds,
				pending_attendance: 0,
				airdrops_scheduled,
			});
			GameHistory::<T>::insert(index, game_play_time);

			log::trace!(
				target: LOG_TARGET,
				"New game, index: {index}"
			);

			Self::deposit_event(Event::NewGame {
				registration_ends,
				game_date: game_play_time,
				report_ends,
			});

			Ok(())
		}

		/// Deterministic `EventId` for the airdrop event at `airdrop_index` associated with
		/// `game_index`.
		///
		/// The event id is the base concatenated with the airdrop index and the game index BE
		/// encoded.
		///
		/// To discover the event ids of the current game: the game index and the number of
		/// scheduled airdrops (`airdrops_scheduled`, the events carry indices
		/// `0..airdrops_scheduled`) are available in the [`Game`] storage, and each successfully
		/// scheduled event also emits [`Event::AirdropScheduled`] with its `airdrop_index` and
		/// `event_id`.
		pub fn airdrop_event_id(game_index: GameIdx, airdrop_index: u8) -> AirdropEventId {
			let mut event_id = [0u8; 32];
			event_id[0..27].copy_from_slice(Self::airdrop_event_id_base().as_ref());
			event_id[27] = airdrop_index;
			event_id[28..32].copy_from_slice(&game_index.to_be_bytes());
			event_id
		}

		/// Best-effort schedule of the airdrop events for this game, one per entry in the
		/// schedule's `airdrops`. Returns the number of scheduled events; the events carry the
		/// airdrop indices `0..count`.
		///
		/// Every event opens registration at `registration_starts`, draws its winners at
		/// `game_play_time + draw_offset` and closes claims its own `claim_window` after its
		/// draw time. Claims only become possible once attendance is recorded (during the
		/// reporting phase at the earliest), so schedules must pick `draw_offset` and
		/// `claim_window` such that the window extends past the reporting phase.
		///
		/// Scheduling is best-effort and stops at the first failure: only the prefix of the
		/// schedule's airdrops up to (excluding) the failing one is scheduled.
		fn try_schedule_airdrops(
			game_index: GameIdx,
			registration_starts: u64,
			game_play_time: u64,
			schedule: &GameScheduleOf<T>,
		) -> u8 {
			let mut scheduled = 0u8;
			for (airdrop_index, airdrop) in schedule.airdrops.iter().enumerate() {
				// MAX_GAME_AIRDROPS is less than 256, so this cast is safe.
				let airdrop_index = airdrop_index as u8;
				let event_id = Self::airdrop_event_id(game_index, airdrop_index);
				let draw_time = game_play_time.saturating_add(airdrop.draw_offset as u64);
				let end_time = draw_time.saturating_add(airdrop.claim_window as u64);
				let info = AirdropEventInfo {
					prize: airdrop.prize.clone(),
					registration_starts,
					draw_time,
					end_time,
				};
				let res = frame_support::storage::with_storage_layer(|| {
					T::Airdrop::schedule(T::AirdropSource::get(), event_id, info)
				});
				match res {
					Ok(()) => {
						scheduled.saturating_inc();
						Self::deposit_event(Event::<T>::AirdropScheduled {
							game_index,
							airdrop_index,
							event_id,
						});
					},
					Err(error) => {
						log::warn!(
							target: LOG_TARGET,
							"airdrop {airdrop_index} schedule failed for game {game_index}: \
							{error:?}",
						);
						Self::deposit_event(Event::<T>::AirdropScheduleFailed {
							game_index,
							airdrop_index,
							error,
						});
						break;
					},
				}
			}
			scheduled
		}

		/// Called from every transition to `GameState::Cancelling`.
		/// * cancel the game's airdrop events (those that were scheduled).
		/// * deposit event.
		pub(crate) fn on_game_cancelled(game: &GameInfo<T::AccountId>) {
			for airdrop_index in 0..game.airdrops_scheduled {
				let event_id = Self::airdrop_event_id(game.index, airdrop_index);
				T::Airdrop::cancel(event_id);
			}
			Self::deposit_event(Event::<T>::GameCancelled { game_index: game.index });
		}

		/// **Warning**: Storage must be rollbacked on error.
		fn sign_up_inner(
			args: SignUpArgs<T::AccountId, T::AccountSignature, AirdropProofOf<T>>,
		) -> DispatchResult {
			// Some pre-condition here are duplicated in validation checks in `GameAsInvited`
			// transaction extension. This is to prevent consuming the invite when it would fail.
			// If further checks are needed, they might also need to be duplicated in the
			// transaction exstension `GameAsInvited`.
			// Signing up with an invite must never fail after the transaction extension validation,
			// except, potentially, extreme conditions.
			//
			// TODO(paritytech/individuality#230): refactor to avoid duplicated checks and enforce
			// the success of `sign_up_with_invite`.

			// Check the game state.
			let mut game = Game::<T>::get().ok_or(Error::<T>::NoGame)?;
			ensure!(
				T::UnixTime::now() < Duration::from_secs(game.registration_ends as u64),
				Error::<T>::NoRegistration
			);
			let GameState::Registration { next_player_index } = &mut game.state else {
				return Err(Error::<T>::NoRegistration.into());
			};

			let (who, new_invited, airdrops) = match args {
				SignUpArgs::Alias {
					who,
					identifier_key,
					statement_account,
					statement_account_signature,
					airdrops,
				} => {
					// We require the proof of ownership to prevent person from blocking other
					// player with account they don't own.
					let msg = (Self::proof_of_ownership_msg_base(), &who)
						.using_encoded(sp_io::hashing::blake2_256);
					ensure!(
						statement_account_signature.verify(&msg[..], &statement_account),
						Error::<T>::InvalidStatementAccountSignature,
					);

					let prev_stmt_account = AliasToStmtAccount::<T>::get(who);
					if prev_stmt_account != Some(statement_account.clone()) {
						if let Some(prev_stmt_account) = prev_stmt_account {
							StmtAccountToAlias::<T>::remove(&prev_stmt_account);
							decrease_allowance_by(
								prev_stmt_account.clone().into(),
								T::PlayerStatementLimit::get(),
							);
							Self::deposit_event(Event::<T>::StmtUsageRemoved {
								who: prev_stmt_account.into(),
							});
						}

						// We prevent any overlap between statement accounts and player.
						ensure!(
							!StmtAccountToAlias::<T>::contains_key(&statement_account) &&
								!Players::<T>::contains_key(AccountOrPerson::Account(
									statement_account.clone()
								)) && !ArchivedPlayers::<T>::contains_key(AccountOrPerson::Account(
								statement_account.clone()
							)),
							Error::<T>::StatementAccountAlreadyInUse,
						);

						StmtAccountToAlias::<T>::insert(&statement_account, who);
						AliasToStmtAccount::<T>::insert(who, &statement_account);
						increase_allowance_by(
							statement_account.clone().into(),
							T::PlayerStatementLimit::get(),
						);
					}
					CommunicationIdentifiers::<T>::insert(&statement_account, identifier_key);

					(AccountOrPerson::Person(who), false, airdrops)
				},
				SignUpArgs::Account { who, identifier_key, new_invited, airdrops } => {
					// We prevent any overlap between statement accounts and player.
					ensure!(
						!StmtAccountToAlias::<T>::contains_key(&who),
						Error::<T>::StatementAccountAlreadyInUse,
					);
					CommunicationIdentifiers::<T>::insert(&who, identifier_key);

					(AccountOrPerson::Account(who.clone()), new_invited, airdrops)
				},
			};

			let maybe_player = Players::<T>::get(&who);
			let already_playing = maybe_player.is_some();

			// Ensure the player is not playing if using an invite.
			ensure!(!new_invited || !already_playing, Error::<T>::UseInviteButAlreadyPlaying);

			// Ensure the player isn't registered yet for the game
			ensure!(
				maybe_player.as_ref().is_none_or(|player| !player.registered),
				Error::<T>::AlreadyRegistered
			);

			let maybe_archived = ArchivedPlayers::<T>::take(&who);

			// Onboard the player in indiv_pallet_score.
			if maybe_archived.is_none() && !already_playing {
				match &who {
					AccountOrPerson::Account(account) => {
						indiv_pallet_score::Pallet::<T>::onboard_for_recognition(account)?;
					},
					AccountOrPerson::Person(person) => {
						indiv_pallet_score::Pallet::<T>::onboard_externally_recognized(person)?;
					},
				}
			}

			// Play credibility
			let (first_game, credibility) = if let Some(player) = maybe_player {
				(player.first_game, player.credibility)
			} else if new_invited {
				(game.index, PlayerCredibility::Invited)
			} else {
				let credibility = match &who {
					AccountOrPerson::Account(account) => PlayerCredibility::Deposit(
						T::PlayDeposit::new(account, PlayDepositAmount::<T>::get())?,
					),
					AccountOrPerson::Person(_) => PlayerCredibility::Recognized,
				};
				let first_game =
					maybe_archived.map(|player| player.first_game()).unwrap_or(game.index);
				(first_game, credibility)
			};

			Players::<T>::insert(
				&who,
				Player {
					first_game,
					registered: true,
					sent_report: false,
					early_attendance_enactment: None,
					yes_person: 0,
					no_not_person: 0,
					expected_max_vote_weight: 0,
					vote_weight: 0,
					credibility,
				},
			);

			Self::register_for_airdrop(airdrops, &who, game.airdrops_scheduled, game.index)?;

			// An account-identified player gets allowance when signed up
			if !already_playing {
				if let Some(account) = who.account() {
					increase_allowance_by(account.clone().into(), T::PlayerStatementLimit::get());
				}
			}

			next_player_index.saturating_inc();
			Game::<T>::put(game);

			Self::deposit_event(Event::<T>::SignedUp { who: who.clone() });

			log::trace!(
				target: LOG_TARGET,
				"Player {who:?} registered for game",
			);

			Ok(())
		}

		// This function is mirrored `validate_register_for_airdrop`, changes must be kept in sync.
		//
		// Registers the player into every scheduled airdrop event, one entry per event in
		// airdrop-index order, return error on the first failure.
		fn register_for_airdrop(
			airdrops: Option<AirdropVrfs<AirdropProofOf<T>>>,
			who: &AccountOrPerson<T::AccountId>,
			airdrops_scheduled: u8,
			game_index: GameIdx,
		) -> DispatchResult {
			let Some(airdrops) = airdrops else {
				return Ok(());
			};
			ensure!(
				airdrops.count() == usize::from(airdrops_scheduled),
				Error::<T>::InvalidAirdropVrfCount
			);

			let recognized = indiv_pallet_score::Participants::<T>::get(who)
				.defensive_proof("pallet-game: registering account must be a participant")
				.is_some_and(|p| p.recognition.is_recognized());
			match airdrops {
				AirdropVrfs::Account(vrfs) => {
					ensure!(!recognized, Error::<T>::InvalidAirdropVrfVariantForRecognition);
					let AccountOrPerson::Account(acct) = &who else {
						return Err(Error::<T>::InvalidAirdropVrfVariantForAccount.into());
					};
					for (airdrop_index, sig) in vrfs.into_iter().enumerate() {
						let event_id = Self::airdrop_event_id(game_index, airdrop_index as u8);
						T::Airdrop::participate_with_account(acct.clone(), event_id, sig)?;
					}
				},
				AirdropVrfs::Alias { proofs, ring_index, revision } => {
					ensure!(recognized, Error::<T>::InvalidAirdropVrfVariantForRecognition);
					let participant_origin = into_registration_entry(who.clone());
					for (airdrop_index, proof) in proofs.into_iter().enumerate() {
						let event_id = Self::airdrop_event_id(game_index, airdrop_index as u8);
						T::Airdrop::participate_with_alias(
							event_id,
							participant_origin.clone(),
							proof,
							ring_index,
							revision,
						)?;
					}
				},
			}
			Ok(())
		}

		/// Validation-only counterpart of [`Self::register_for_airdrop`] used by the
		/// `GameAsInvited` transaction extension.
		// TODO(paritytech/individuality#230): ideally change the onboarding flow to first onboard
		// and then register for the game so the check for onboarding only consists of checking
		// the invitation. Or otherwise we may not want to check the validity of the VRF and maybe
		// not even check the validity of the complete call, just let the invitation do 5 calls
		// until being consumed, the invited being responsible for doing valid calls.
		pub(crate) fn validate_register_for_airdrop(
			airdrops: &Option<AirdropVrfs<AirdropProofOf<T>>>,
			who: &AccountOrPerson<T::AccountId>,
			game_index: GameIdx,
			airdrops_scheduled: u8,
		) -> Result<(), CustomError> {
			use CustomError::*;
			use TransactionOutcome::*;

			let Some(airdrops) = airdrops.as_ref() else {
				return Ok(());
			};
			if airdrops.count() != usize::from(airdrops_scheduled) {
				return Err(InvalidAirdropVrfCount);
			}

			let recognized = indiv_pallet_score::Participants::<T>::get(who)
				// When None, it will be onboarded as NotRecognized.
				.is_some_and(|p| p.recognition.is_recognized());

			let variant_ok = match airdrops {
				AirdropVrfs::Alias { .. } => recognized,
				AirdropVrfs::Account(_) => !recognized,
			};
			if !variant_ok {
				return Err(InvalidAirdropVrfVariant);
			}

			let res = match airdrops {
				AirdropVrfs::Account(vrfs) => {
					let AccountOrPerson::Account(acct) = who else {
						return Err(InvalidAirdropVrfVariant);
					};
					with_transaction(|| {
						Rollback(Ok(vrfs.iter().enumerate().try_for_each(
							|(airdrop_index, sig)| {
								let event_id =
									Self::airdrop_event_id(game_index, airdrop_index as u8);
								T::Airdrop::participate_with_account(
									acct.clone(),
									event_id,
									sig.clone(),
								)
							},
						)))
					})
				},
				AirdropVrfs::Alias { proofs, ring_index, revision } => {
					let participant_origin = into_registration_entry(who.clone());
					with_transaction(|| {
						Rollback(Ok(proofs.iter().enumerate().try_for_each(
							|(airdrop_index, proof)| {
								let event_id =
									Self::airdrop_event_id(game_index, airdrop_index as u8);
								T::Airdrop::participate_with_alias(
									event_id,
									participant_origin.clone(),
									proof.clone(),
									*ring_index,
									*revision,
								)
							},
						)))
					})
				},
			};

			res
				// The transactional layer should succeed.
				.map_err(|err: DispatchError| {
					log::debug!(
						target: LOG_TARGET,
						"Airdrop registration failed at transactional layer: {err:?}",
					);
					UnexpectedInvalidity
				})?
				// If the inner call failed, it means the airdrop registration is invalid.
				.map_err(|err| {
					log::debug!(
						target: LOG_TARGET,
						"Airdrop registration failed for account {who:?}: {err:?}",
					);
					InvalidAirdropRegistration
				})?;

			Ok(())
		}

		/// Writes into `ShuffleRecognized` and `ShuffleNotRecognized`.
		/// Update the `last_player_key` to the inserted player.
		/// Update the `pending_attendance` for the game when a registered player is inserted.
		pub(crate) fn shuffle_step_insert(
			last_player_key: &mut Option<AccountOrPerson<T::AccountId>>,
			pending_attendance: &mut u32,
			rounds: u8,
			parent_hash: &T::Hash,
		) -> StepResult {
			let next_player = last_player_key
				.clone()
				.map(Players::<T>::iter_from_key)
				.unwrap_or_else(Players::<T>::iter)
				.next();

			let Some((player_id, player_info)) = next_player else {
				// No more players to shuffle.
				return StepResult::Finished;
			};

			// Ideally we should iterate on registered players only, or at least refund the weight
			// when skipping this part.
			if player_info.registered {
				*pending_attendance = pending_attendance.saturating_add(1);

				let player_recognized_as_person =
					indiv_pallet_score::Pallet::<T>::reached_personhood(&player_id);

				// Snapshot the player's vote weight so it stays consistent across the
				// reporting phase even if their personhood changes mid-game (e.g. via
				// early attendance enactment).
				let vote_weight = if player_recognized_as_person {
					T::PeopleVoteWeight::get()
				} else {
					T::CandidateVoteWeight::get()
				};
				Players::<T>::mutate(&player_id, |maybe_player| {
					if let Some(player) = maybe_player.as_mut() {
						player.vote_weight = vote_weight;
					} else {
						defensive!("indiv-pallet-game: player should exist");
					}
				});

				for round in 0..rounds {
					let hash = sp_io::hashing::blake2_256(
						(&parent_hash, &player_id, round).encode().as_ref(),
					);

					if player_recognized_as_person {
						ShuffleRecognized::<T>::insert(round, hash, &player_id);
					} else {
						ShuffleNotRecognized::<T>::insert(round, hash, &player_id);
					}

					log::trace!(target: LOG_TARGET, "Shuffle: 1 player inserted in round {round}");
				}
			}

			*last_player_key = Some(player_id.clone());

			StepResult::Continue
		}

		/// Reads from and writes to `PlayerToIndex`, `IndexToPlayer`, `ShuffleRecognized` and
		/// `ShuffleNotRecognized`.
		/// Update the `next_player_index` to the next index if one is processed.
		/// Update the `phase` from [`ShuffleRetrievePhase::Recognized`] to
		/// [`ShuffleRetrievePhase::NotRecognized`] when the recognized population is exhausted.
		/// Update the `resume_cursors` to the last drained key for each round, to optimize the
		/// retrieval of the next player.
		///
		/// `resume_cursors` **MUST** be passed with one entry per round (all initially `None`).
		// `resume_cursors` is a pure optimization cache: it lets each step resume draining right
		// after the previous key instead of iterating from the beginning of the trie prefix, which
		// would require skipping over all the keys already removed (and still buffered in the
		// overlay) this block.
		pub(crate) fn shuffle_step_retrieve(
			next_player_index: &mut PlayerIndex,
			phase: &mut ShuffleRetrievePhase,
			resume_cursors: &mut [Option<ShufflePositionKey>],
			rounds: u8,
		) -> StepResult {
			for round in 0..rounds {
				let last_key = resume_cursors.get(usize::from(round)).copied().flatten();

				// Resuming after the last key drained.
				let player_iter = match (&*phase, last_key) {
					(ShuffleRetrievePhase::NotRecognized { .. }, Some(key)) =>
						ShuffleNotRecognized::<T>::iter_prefix_from(
							round,
							ShuffleNotRecognized::<T>::hashed_key_for(round, key),
						),
					(ShuffleRetrievePhase::NotRecognized { .. }, None) =>
						ShuffleNotRecognized::<T>::iter_prefix(round),
					(ShuffleRetrievePhase::Recognized, Some(key)) =>
						ShuffleRecognized::<T>::iter_prefix_from(
							round,
							ShuffleRecognized::<T>::hashed_key_for(round, key),
						),
					(ShuffleRetrievePhase::Recognized, None) =>
						ShuffleRecognized::<T>::iter_prefix(round),
				};

				// Drain the next entry.
				let next_player = player_iter.drain().next();

				let Some((hash, player_id)) = next_player else {
					// One iteration is done, reset the cache.
					resume_cursors.iter_mut().for_each(|key| *key = None);
					match phase {
						// Not-recognized phase drained. All is done.
						ShuffleRetrievePhase::NotRecognized { .. } => return StepResult::Finished,
						// Recognized phase drained. Transition to the not-recognized phase.
						// `next_player_index` is exactly the number of recognized players.
						ShuffleRetrievePhase::Recognized => {
							*phase = ShuffleRetrievePhase::NotRecognized {
								recognized_count: *next_player_index,
							};
							return StepResult::Continue;
						},
					}
				};

				// Cache the drained key to resume after it on the next step.
				if let Some(cached) = resume_cursors.get_mut(usize::from(round)) {
					*cached = Some(hash);
				} else {
					defensive!("indiv-pallet-game: resume_cursors should have one entry per round");
				}

				let mut indices = PlayerToIndex::<T>::get(&player_id).unwrap_or_else(|| {
					let mut indices = BoundedVec::default();
					indices.bounded_resize(usize::from(rounds), 0);
					indices
				});
				if let Some(index) = indices.get_mut(usize::from(round)).defensive_proof(
					"indiv-pallet-game: indices are initialized with length = rounds",
				) {
					*index = *next_player_index;
				}
				PlayerToIndex::<T>::insert(&player_id, &indices);
				IndexToPlayer::<T>::insert((round, *next_player_index), &player_id);

				log::trace!(target: LOG_TARGET, "Shuffle: player {player_id:?}, indices: {indices:?}");
			}

			*next_player_index = next_player_index.saturating_add(1);

			StepResult::Continue
		}

		/// Compute the exact `expected_max_vote_weight` for one registered player by summing
		/// the personhood-derived vote weights of all their co-players across every round.
		pub(crate) fn shuffle_step_compute_weights(
			last_iteration: &mut Option<AccountOrPerson<T::AccountId>>,
			rounds: u8,
			player_count: u32,
			recognized_count: u32,
			max_per_group: u32,
		) -> StepResult {
			let mut remaining_players = match last_iteration.clone() {
				None => PlayerToIndex::<T>::iter(),
				Some(last) => PlayerToIndex::<T>::iter_from_key(last),
			};

			let Some((player_id, round_indices)) = remaining_players.next() else {
				return StepResult::Finished;
			};

			let groups_setting = GroupsSetting { max_per_group, player_count };
			let people_vote_weight = u32::from(T::PeopleVoteWeight::get());
			let candidate_vote_weight = u32::from(T::CandidateVoteWeight::get());
			let mut total_weight: u32 = 0;

			for round in 0..rounds {
				let Some(&player_idx) = round_indices
					.get(usize::from(round))
					.defensive_proof("indiv-pallet-game: round_indices have one entry per round")
				else {
					continue;
				};

				let group_index = groups_setting.group_index_from_player_index(player_idx);
				for member_idx in groups_setting.group_members(group_index) {
					// Skip because a player does not vote on themselves at report time
					if member_idx == player_idx {
						continue;
					}
					// Recognized players occupy indices `[0, recognized_count-1]` in every round,
					// so we can determine the vote weight of a member by their index alone.
					let member_weight = if member_idx < recognized_count {
						people_vote_weight
					} else {
						candidate_vote_weight
					};
					total_weight = total_weight.saturating_add(member_weight);
				}
			}

			let expected_max_vote_weight: u16 = total_weight.try_into().unwrap_or(u16::MAX);

			Players::<T>::mutate(&player_id, |maybe_player| {
				if let Some(player) = maybe_player.as_mut() {
					player.expected_max_vote_weight = expected_max_vote_weight;
				} else {
					defensive!("indiv-pallet-game: player should exist");
				}
			});

			log::trace!(
				target: LOG_TARGET,
				"Shuffle: expected weight for {player_id:?} is {expected_max_vote_weight}",
			);

			*last_iteration = Some(player_id);

			StepResult::Continue
		}

		/// Shuffle the players for the game given the available weight.
		/// Arguments:
		/// - `weight_meter`: The available weight.
		/// - `game`: The current game; caller must guarantee its state is `Shuffle`.
		pub(crate) fn shuffles(weight_meter: &mut WeightMeter, mut game: GameInfo<T::AccountId>) {
			let base_weight = <T as Config>::WeightInfo::shuffles_base().saturating_add(
				<T as Config>::WeightInfo::on_game_cancelled(game.airdrops_scheduled.into()),
			);
			if weight_meter.try_consume(base_weight).is_err() {
				return;
			}

			let GameState::Shuffle { ref mut step } = game.state else {
				defensive!("game state is not shuffle");
				return;
			};

			let now = T::UnixTime::now();
			if now > Duration::from_secs(game.shuffle_deadline.into()) {
				Self::on_game_cancelled(&game);
				game.state = GameState::Cancelling { step: CancellingStep::Step1DrainShuffle };
				Game::<T>::put(game);
				return;
			}

			let parent_hash = frame_system::Pallet::<T>::parent_hash();

			// Per-round cache of the last drained key, used by `shuffle_step_retrieve` to resume
			// iteration. Only meaningful within this single `shuffles` invocation.
			let mut resume_cursors = vec![None; usize::from(game.rounds)];

			for _ in 0..OP_UPPER_BOUND {
				match step {
					ShuffleStep::Step1Insert { ref mut last_iteration } => {
						if weight_meter
							.try_consume(<T as Config>::WeightInfo::shuffle_step_insert(
								game.rounds.into(),
							))
							.is_err()
						{
							break;
						}

						let step_result = Self::shuffle_step_insert(
							last_iteration,
							&mut game.pending_attendance,
							game.rounds,
							&parent_hash,
						);

						match step_result {
							StepResult::Finished => {
								*step = ShuffleStep::Step2Retrieve {
									next_player_index: 0,
									phase: ShuffleRetrievePhase::Recognized,
								};
							},
							StepResult::Continue => {},
						}
					},
					ShuffleStep::Step2Retrieve { ref mut next_player_index, ref mut phase } => {
						if weight_meter
							.try_consume(<T as Config>::WeightInfo::shuffle_step_retrieve(
								game.rounds.into(),
							))
							.is_err()
						{
							break;
						}

						let step_result = Self::shuffle_step_retrieve(
							next_player_index,
							phase,
							&mut resume_cursors,
							game.rounds,
						);

						match step_result {
							StepResult::Finished => {
								// Retrieve reports `Finished` after the not-recognized phase,
								// so `recognized_count` is available.
								let recognized_count = match phase {
									ShuffleRetrievePhase::NotRecognized { recognized_count } =>
										*recognized_count,
									ShuffleRetrievePhase::Recognized => {
										defensive!(
											"indiv-pallet-game: retrieve finished before the \
											recognized phase completed"
										);
										*next_player_index
									},
								};
								*step = ShuffleStep::Step3ComputeWeights {
									last_iteration: None,
									player_count: *next_player_index,
									recognized_count,
								};
							},
							StepResult::Continue => {},
						}
					},
					ShuffleStep::Step3ComputeWeights {
						ref mut last_iteration,
						player_count,
						recognized_count,
					} => {
						if weight_meter
							.try_consume(<T as Config>::WeightInfo::shuffle_step_compute_weights(
								// Per-call cost scales with the inner iterations
								// `rounds * (group_size - 1)`; charge `rounds * max_group_size` as
								// a tight upper bound (a single component avoids the n*r
								// interaction that a separable `f(n, r)` weight cannot
								// model).
								u32::from(game.rounds).saturating_mul(game.max_group_size),
							))
							.is_err()
						{
							break;
						}

						let step_result = Self::shuffle_step_compute_weights(
							last_iteration,
							game.rounds,
							*player_count,
							*recognized_count,
							game.max_group_size,
						);

						match step_result {
							StepResult::Finished => {
								*step =
									ShuffleStep::Step4AwaitSession { player_count: *player_count };
							},
							StepResult::Continue => {},
						}
					},
					ShuffleStep::Step4AwaitSession { player_count } => {
						if weight_meter
							.try_consume(<T as Config>::WeightInfo::shuffle_step_start_session())
							.is_err()
						{
							break;
						}

						if !indiv_pallet_score::Pallet::<T>::can_start_attendance_report_session() {
							break;
						}
						let Ok(()) =
							indiv_pallet_score::Pallet::<T>::start_attendance_report_session()
						else {
							defensive!(
								"indiv-pallet-game: could not start attendance report \
								session, but was validated beforehand"
							);
							break;
						};
						game.state = GameState::Reporting { player_count: *player_count };
						log::trace!(target: LOG_TARGET, "Game is now in reporting phase");
						break;
					},
				}
			}

			Game::<T>::put(game);
		}

		/// Process the reporting phase: end the reporting when the conditions are met.
		pub(crate) fn process_reporting(weight_meter: &mut WeightMeter) {
			if weight_meter
				.try_consume(<T as Config>::WeightInfo::process_reporting())
				.is_err()
			{
				return;
			}

			let Some(mut game) = Game::<T>::get() else {
				defensive!("game should exist while processing reporting");
				return;
			};

			let GameState::Reporting { player_count } = game.state else {
				defensive!("game state is not reporting phase");
				return;
			};

			// Transition to player process either when the reporting window has closed
			// or as soon as every registered player's attendance has already been
			// enacted early (nothing left to wait for).
			let now = T::UnixTime::now();
			let reporting_ended = now >= Duration::from_secs(game.report_ends.into());
			let nothing_pending = game.pending_attendance == 0;
			if !reporting_ended && !nothing_pending {
				return;
			}

			game.state = GameState::PlayerProcess {
				step: PlayerProcessStep::Step1ProcessPlayers { last_iteration: None, player_count },
			};
			Game::<T>::put(game);
		}

		/// Attempt to enact the attendance of a player early.
		///
		/// This is a no-op if the player does not exist, has already been early-enacted,
		/// or if their attendance is still pending. Reads and writes `Game` storage
		/// directly.
		///
		/// Returns `true` if the full enactment path ran (attendance was applied and the
		/// result cached on the player), `false` for every early-bail case. Claim credit
		/// materialization is *not* done here: it is deferred to
		/// `process_player_attendance_outcome` so `report`'s weight stays linear.
		pub(crate) fn try_early_attendance_enactment(
			player_id: &AccountOrPerson<T::AccountId>,
		) -> bool {
			let Some(player) = Players::<T>::get(player_id) else { return false };
			if player.early_attendance_enactment.is_some() {
				return false;
			}
			let Some(mut game) = Game::<T>::get() else {
				defensive!("indiv-pallet-game: game should exist");
				return false;
			};
			let attendance = match Self::determine_attendance(&player) {
				AttendanceStatus::Attended => true,
				AttendanceStatus::NotAttended => false,
				AttendanceStatus::Pending => return false,
			};

			let GameState::Reporting { .. } = game.state else {
				defensive!("indiv-pallet-game: early enactment outside reporting phase");
				return false;
			};

			let result = Self::apply_attendance(
				player_id,
				attendance,
				player.registered,
				game.index,
				&mut game.pending_attendance,
			);

			Game::<T>::put(game);

			// Cache the result on the player so `player_process_step1` can reuse it instead
			// of calling the score pallet a second time.
			Players::<T>::mutate(player_id, |maybe_player| {
				if let Some(player) = maybe_player.as_mut() {
					player.early_attendance_enactment = Some(result);
				} else {
					defensive!("indiv-pallet-game: player should exist");
				}
			});

			log::trace!(
				target: LOG_TARGET,
				"Player {player_id:?} early-enacted, attendance: {attendance}",
			);

			true
		}

		/// Apply the attendance of `player_id` to `indiv_pallet_score`, record the
		/// attendance if any, update `game_pending_attendance` for registered players,
		/// and return the [`EarlyAttendanceEnactment`] the caller needs to continue
		/// processing.
		pub(crate) fn apply_attendance(
			player_id: &AccountOrPerson<T::AccountId>,
			attendance: bool,
			registered: bool,
			game_index: GameIdx,
			game_pending_attendance: &mut u32,
		) -> EarlyAttendanceEnactment {
			let res =
				indiv_pallet_score::Pallet::<T>::set_attendance(player_id, attendance, game_index);
			defensive_assert!(res.is_ok(), "player is registered in pallet score");
			if attendance {
				Self::note_attendance(game_index, player_id);
			}

			let (externally_recognized, zero_score) = if let Ok(participant) = res {
				(participant.recognition.is_externally_recognized(), participant.score == 0)
			} else {
				defensive!("player is registered in pallet score");
				(false, false)
			};

			// Only registered players contribute to `game_pending_attendance`:
			// non-registered holdovers were never counted at shuffle time.
			if registered {
				*game_pending_attendance = game_pending_attendance.saturating_sub(1);
			}

			let disposition = if externally_recognized && (zero_score || !attendance) {
				// Externally-recognized players cannot be kicked because their personhood
				// was established elsewhere.
				PlayerDisposition::ArchiveUnkickable
			} else if zero_score {
				PlayerDisposition::ArchiveKickable
			} else {
				PlayerDisposition::Keep
			};

			EarlyAttendanceEnactment { attendance, disposition }
		}

		/// Determine the attendance status of a player given a partial vote state.
		pub(crate) fn determine_attendance(player: &Player<T::PlayDeposit>) -> AttendanceStatus {
			if !player.registered {
				return AttendanceStatus::NotAttended;
			}

			let expected_max_weight = player.expected_max_vote_weight as u32;

			let yes = player.yes_person as u32;
			let no = player.no_not_person as u32;
			let received = yes.saturating_add(no);
			let remaining = expected_max_weight.saturating_sub(received);

			// Best case for the player (all remaining votes are `Person`): if the
			// attendance vote still fails, the player cannot attend regardless of future
			// reports.
			let best_case_yes = yes.saturating_add(remaining);
			if best_case_yes.saturating_sub(1) < no {
				return AttendanceStatus::NotAttended;
			}

			// Worst case for the player (all remaining votes are `NotPerson`): if the
			// attendance vote still passes and the player already sent their report, they
			// are definitely attending.
			if player.sent_report {
				let worst_case_no = no.saturating_add(remaining);
				if yes.saturating_sub(1) >= worst_case_no {
					return AttendanceStatus::Attended;
				}
			}

			AttendanceStatus::Pending
		}

		/// Process one player's attendance outcome and advance the storage iterator.
		///
		/// Side effects:
		/// - May call into `indiv_pallet_score::Pallet::set_attendance`.
		/// - Routes attendance NFT claim credits, archives or resets player state, and may drop a
		///   play deposit.
		/// - Updates `last_iteration` and returns the next item from `iterator`.
		/// - Adds the number of NFT claim credits recorded for this player to `credits_awarded`, so
		///   the caller can debit the block capacity it reserved.
		pub(crate) fn process_player_attendance_outcome(
			game_index: GameIdx,
			rounds: u8,
			max_group_size: u32,
			pending_attendance: &mut u32,
			player_count: u32,
			last_iteration: &mut Option<AccountOrPerson<T::AccountId>>,
			next_player: (AccountOrPerson<T::AccountId>, Player<T::PlayDeposit>),
			iterator: &mut impl Iterator<Item = (AccountOrPerson<T::AccountId>, Player<T::PlayDeposit>)>,
			award_time: u32,
			credits_awarded: &mut u32,
		) -> Option<(AccountOrPerson<T::AccountId>, Player<T::PlayDeposit>)> {
			let (player_id, player) = next_player;

			let EarlyAttendanceEnactment { attendance, disposition } =
				if let Some(cached) = player.early_attendance_enactment {
					cached
				} else {
					let half_plus_one = player.yes_person.saturating_sub(1) >= player.no_not_person;
					let attendance = player.registered && player.sent_report && half_plus_one;

					Self::apply_attendance(
						&player_id,
						attendance,
						player.registered,
						game_index,
						pending_attendance,
					)
				};

			// `report` defers NFT claim credit materialization to here so its weight stays linear
			// in `(e, n)` rather than `O(e · n)`. An attendee earns one credit per group
			// co-member in every round played; a non-attendee earns none, and unearned
			// credits were never awarded, so there is nothing to undo.
			if attendance {
				*credits_awarded =
					credits_awarded.saturating_add(T::NftClaimCredits::award_attendance_credits(
						game_index,
						rounds,
						max_group_size,
						player_count,
						&player_id,
						award_time,
					));
			}

			let archived_player = match disposition {
				PlayerDisposition::ArchiveKickable => Some(ArchivedPlayer::Kickable {
					first_game: player.first_game,
					archived_since: frame_system::Pallet::<T>::block_number(),
				}),
				PlayerDisposition::ArchiveUnkickable =>
					Some(ArchivedPlayer::Unkickable { first_game: player.first_game }),
				PlayerDisposition::Keep => None,
			};

			if let Some(archived_player) = archived_player {
				if let PlayerCredibility::Deposit(deposit) = player.credibility {
					if let AccountOrPerson::Account(account) = &player_id {
						deposit.burn(account);
					} else {
						defensive!("person player should not have deposit");
					}
				}

				let stmt_account = match &player_id {
					AccountOrPerson::Person(alias) => AliasToStmtAccount::<T>::take(alias)
						.defensive_proof(
							"indiv-pallet-game: alias player must have a statement account",
						)
						.inspect(|stmt_account| StmtAccountToAlias::<T>::remove(stmt_account)),
					AccountOrPerson::Account(account) => Some(account.clone()),
				};

				if let Some(stmt_account) = stmt_account {
					decrease_allowance_by(
						stmt_account.clone().into(),
						T::PlayerStatementLimit::get(),
					);
					Self::deposit_event(Event::<T>::StmtUsageRemoved { who: stmt_account.into() });
				}

				Players::<T>::remove(&player_id);
				ArchivedPlayers::<T>::insert(&player_id, archived_player);
			} else {
				let credibility = match player.credibility {
					PlayerCredibility::Deposit(deposit)
						if indiv_pallet_score::Pallet::<T>::reached_personhood(&player_id) =>
					{
						match &player_id {
							AccountOrPerson::Account(account) => {
								let _ = deposit.drop(account).defensive_proof(
									"indiv-pallet-game: deposit drop should not fail",
								);
							},
							AccountOrPerson::Person(_) => {
								defensive!("person player should not have deposit");
							},
						}
						PlayerCredibility::Recognized
					},
					credibility => credibility,
				};

				Players::<T>::insert(
					&player_id,
					Player {
						first_game: player.first_game,
						registered: false,
						sent_report: false,
						early_attendance_enactment: None,
						yes_person: 0,
						no_not_person: 0,
						expected_max_vote_weight: 0,
						vote_weight: 0,
						credibility,
					},
				);
			}

			log::trace!(
				target: LOG_TARGET,
				"Player {player_id:?} processed, attendance: {attendance}",
			);

			*last_iteration = Some(player_id);
			iterator.next()
		}

		pub(crate) fn player_process_step1(weight_meter: &mut WeightMeter) {
			if weight_meter
				.try_consume(<T as Config>::WeightInfo::player_process_step1())
				.is_err()
			{
				return;
			}

			let Some(mut game) = Game::<T>::get() else {
				defensive!("game should exist while processing players");
				return;
			};

			let GameState::PlayerProcess {
				step:
					PlayerProcessStep::Step1ProcessPlayers { ref mut last_iteration, player_count },
			} = game.state
			else {
				defensive!("game state is not process player");
				return;
			};

			let mut iterator = match last_iteration.clone() {
				None => Players::<T>::iter(),
				Some(last) => Players::<T>::iter_from_key(last),
			};

			let per_player_weight =
				<T as Config>::WeightInfo::player_process_step1_inner_loop(game.rounds as u32);
			let award_time = T::UnixTime::now().as_secs() as u32;
			// A player's backfill cannot be split across blocks, so a player is only processed
			// while its worst case still fits what the block can award and no credit ever has to
			// be dropped. The remaining players are processed in a later block, which starts with
			// its own awards. Only the credits really awarded are debited, so players that award
			// nothing (non-attendees) or less than their worst case (credits already awarded
			// during reporting) do not shorten the block.
			let max_credits_per_player =
				Self::max_attestations(game.rounds as u32, game.max_group_size);
			let mut credit_capacity = T::NftClaimCredits::remaining_capacity();
			let mut next_player = iterator.next();

			for _ in 0..OP_UPPER_BOUND {
				let Some(next_player_to_process) = next_player.take() else { break };
				if credit_capacity < max_credits_per_player {
					Game::<T>::put(game);
					return;
				}
				if weight_meter.try_consume(per_player_weight).is_err() {
					Game::<T>::put(game);
					return;
				}

				let mut credits_awarded = 0;
				next_player = Self::process_player_attendance_outcome(
					game.index,
					game.rounds,
					game.max_group_size,
					&mut game.pending_attendance,
					player_count,
					last_iteration,
					next_player_to_process,
					&mut iterator,
					award_time,
					&mut credits_awarded,
				);
				credit_capacity = credit_capacity.saturating_sub(credits_awarded);
			}

			if next_player.is_none() {
				let _ = indiv_pallet_score::Pallet::<T>::end_attendance_report_session()
					.defensive_proof(
						"indiv-pallet-game: session was started before the process players phase",
					);

				game.state =
					GameState::PlayerProcess { step: PlayerProcessStep::Step2ClearIndices };
			}
			Game::<T>::put(game);
		}

		/// Clear one bounded chunk from each per-game index <-> player map, and from the game's
		/// [`AwardedNftClaimCredits`].
		///
		/// The awarded-credit slots go with the maps, being positions in groups only those maps
		/// resolve. Step 1 has finished the attendance backfill by now, so no award can name
		/// this game again and the slots have no reader left.
		pub(crate) fn player_process_step2_inner_loop(
			game_index: GameIdx,
			cursor1: &mut Option<Vec<u8>>,
			cursor2: &mut Option<Vec<u8>>,
			cursor3: &mut Option<Vec<u8>>,
			done1: &mut bool,
			done2: &mut bool,
			done3: &mut bool,
		) {
			if !*done1 {
				let r = IndexToPlayer::<T>::clear(PLAYER_PROCESS_STEP2_CHUNK, cursor1.as_deref());
				*cursor1 = r.maybe_cursor;
				*done1 = cursor1.is_none();
			}
			if !*done2 {
				let r = PlayerToIndex::<T>::clear(PLAYER_PROCESS_STEP2_CHUNK, cursor2.as_deref());
				*cursor2 = r.maybe_cursor;
				*done2 = cursor2.is_none();
			}
			if !*done3 {
				*cursor3 = T::NftClaimCredits::clear_game_credits(
					game_index,
					PLAYER_PROCESS_STEP2_CHUNK,
					cursor3.as_deref(),
				);
				*done3 = cursor3.is_none();
			}
		}

		pub(crate) fn player_process_step2(weight_meter: &mut WeightMeter) {
			if weight_meter
				.try_consume(<T as Config>::WeightInfo::player_process_step2())
				.is_err()
			{
				return;
			}

			let Some(game) = Game::<T>::get() else {
				defensive!("indiv-pallet-game: game should exist while processing players");
				return;
			};

			let GameState::PlayerProcess { step: PlayerProcessStep::Step2ClearIndices } =
				game.state
			else {
				defensive!("indiv-pallet-game: game state is not process player step 2");
				return;
			};

			let mut cursor1: Option<Vec<u8>> = None;
			let mut cursor2: Option<Vec<u8>> = None;
			let mut cursor3: Option<Vec<u8>> = None;
			let mut done1 = false;
			let mut done2 = false;
			let mut done3 = false;
			for _ in 0..OP_UPPER_BOUND {
				if weight_meter
					.try_consume(<T as Config>::WeightInfo::player_process_step2_inner_loop())
					.is_err()
				{
					return;
				}

				Self::player_process_step2_inner_loop(
					game.index,
					&mut cursor1,
					&mut cursor2,
					&mut cursor3,
					&mut done1,
					&mut done2,
					&mut done3,
				);

				if done1 && done2 && done3 {
					break;
				}
			}

			// If still not done continue later.
			if !(done1 && done2 && done3) {
				defensive!(
					"indiv-pallet-game: player_process_step2 exhausted OP_UPPER_BOUND without draining; possible cursor bug or miscalibrated weight meter"
				);
				return;
			}

			Self::deposit_event(Event::<T>::GameEnded { index: game.index });
			Game::<T>::kill();
		}

		/// Process the cancelling of the game, one single step, return true if finished.
		pub(crate) fn process_cancelling_step_player(
			game_index: GameIdx,
			last_iteration: &mut Option<AccountOrPerson<T::AccountId>>,
			rounds: u8,
		) -> bool {
			let mut iterator = match last_iteration {
				None => Players::<T>::iter(),
				Some(last) => Players::<T>::iter_from_key(last),
			};

			if let Some((player_id, player)) = iterator.next() {
				// Reset the player info.
				Players::<T>::insert(
					&player_id,
					Player {
						first_game: player.first_game,
						registered: false,
						sent_report: false,
						early_attendance_enactment: None,
						yes_person: 0,
						no_not_person: 0,
						expected_max_vote_weight: 0,
						vote_weight: 0,
						credibility: player.credibility,
					},
				);

				// The credit mask names group positions, so it is only readable through the
				// index maps and goes with them.
				T::NftClaimCredits::forget_player_credits(game_index, &player_id);

				// Clean index <-> player mapping.
				if let Some(indices) = PlayerToIndex::<T>::take(&player_id) {
					for round in 0..rounds {
						if let Some(index) = indices.get(round as usize).defensive_proof(
							"indiv-pallet-game: indices are consistent with rounds",
						) {
							IndexToPlayer::<T>::remove((round, index));
						}
					}
				}

				*last_iteration = Some(player_id);
				false
			} else {
				true
			}
		}

		/// Process the cancelling of the game.
		pub(crate) fn process_cancelling(weight_meter: &mut WeightMeter) {
			if weight_meter
				.try_consume(<T as Config>::WeightInfo::process_cancelling())
				.is_err()
			{
				return;
			}

			let Some(mut game) = Game::<T>::get() else {
				defensive!("game should exist in order to be cancelled");
				return;
			};
			let GameState::Cancelling { ref mut step } = game.state else {
				defensive!("game state is not cancelling");
				return;
			};

			let rounds = game.rounds;
			let game_index = game.index;
			let mut finished = false;
			let mut shuffle_cursor1: Option<Vec<u8>> = None;
			let mut shuffle_cursor2: Option<Vec<u8>> = None;
			let mut shuffle_done1 = false;
			let mut shuffle_done2 = false;
			for _ in 0..OP_UPPER_BOUND {
				match step {
					CancellingStep::Step1DrainShuffle => {
						if weight_meter
							.try_consume(
								<T as Config>::WeightInfo::process_cancelling_step_shuffle(),
							)
							.is_err()
						{
							break;
						}
						Self::process_cancelling_step_shuffle(
							&mut shuffle_cursor1,
							&mut shuffle_cursor2,
							&mut shuffle_done1,
							&mut shuffle_done2,
						);
						// Once both shuffle maps are drained, move on to draining players.
						if shuffle_done1 && shuffle_done2 {
							*step = CancellingStep::Step2DrainPlayers { last_iteration: None };
						}
					},
					CancellingStep::Step2DrainPlayers { last_iteration } => {
						if weight_meter
							.try_consume(<T as Config>::WeightInfo::process_cancelling_step_player(
								rounds as u32,
							))
							.is_err()
						{
							break;
						}
						finished = Self::process_cancelling_step_player(
							game_index,
							last_iteration,
							rounds,
						);
						if finished {
							break;
						}
					},
				}
			}

			if finished {
				Game::<T>::kill();
				GameHistory::<T>::remove(game.index);
			} else {
				Game::<T>::put(game);
			}
		}

		/// Clear one bounded chunk from each shuffle map ([`ShuffleRecognized`]
		/// and [`ShuffleNotRecognized`]). The caller checks `done1 && done2`
		/// after this call to detect overall completion.
		pub(crate) fn process_cancelling_step_shuffle(
			cursor1: &mut Option<Vec<u8>>,
			cursor2: &mut Option<Vec<u8>>,
			done1: &mut bool,
			done2: &mut bool,
		) {
			if !*done1 {
				let r = ShuffleRecognized::<T>::clear(CANCELLING_SHUFFLE_CHUNK, cursor1.as_deref());
				*cursor1 = r.maybe_cursor;
				*done1 = cursor1.is_none();
			}
			if !*done2 {
				let r =
					ShuffleNotRecognized::<T>::clear(CANCELLING_SHUFFLE_CHUNK, cursor2.as_deref());
				*cursor2 = r.maybe_cursor;
				*done2 = cursor2.is_none();
			}
		}

		/// Process the game.
		pub(crate) fn process_game(
			weight_meter: &mut WeightMeter,
			n: BlockNumberFor<T>,
			mut game: GameInfo<T::AccountId>,
		) {
			// We skip some block as a defense mechanism if the game process is wrongly weighted.
			//
			// NOTE: This only defends against faulty weight if the block number can increase when
			// block is overweight, which is the case if we use the relay chain block number.
			// Using the parachain block number here makes this skipping useless: if the STF is
			// stuck then the block number doesn't increase.
			if n % GAME_PROCESS_SKIPPED_BLOCK.into() == 0u32.into() {
				return;
			}

			match game.state {
				GameState::Registration { next_player_index } => {
					if weight_meter.try_consume(<T as Config>::WeightInfo::unix_time()).is_err() {
						return;
					}
					let now = T::UnixTime::now();
					if now >= Duration::from_secs(game.registration_ends.into()) {
						let weight = <T as Config>::WeightInfo::put_game().saturating_add(
							<T as Config>::WeightInfo::on_game_cancelled(
								game.airdrops_scheduled.into(),
							),
						);
						if weight_meter.try_consume(weight).is_err() {
							return;
						}

						let player_count = next_player_index;

						let group_setting =
							GroupsSetting { max_per_group: game.max_group_size, player_count };

						if group_setting.acceptable_player_count::<T>() {
							game.state = GameState::Shuffle {
								step: ShuffleStep::Step1Insert { last_iteration: None },
							};
							log::trace!(
								target: LOG_TARGET,
								"Game shuffle started, player count: {player_count:?}",
							);
						} else {
							Self::on_game_cancelled(&game);
							game.state =
								GameState::Cancelling { step: CancellingStep::Step1DrainShuffle };
							log::trace!(
								target: LOG_TARGET,
								"Game cancelled due to unacceptable player count. player count: \
								{player_count:?}, max group size: {:?}",
								game.max_group_size,
							);
						}
						Game::<T>::put(game);
					}
				},
				GameState::Shuffle { .. } => {
					Self::shuffles(weight_meter, game);
				},
				GameState::Reporting { .. } => {
					Self::process_reporting(weight_meter);
				},
				GameState::PlayerProcess { step } => match step {
					PlayerProcessStep::Step1ProcessPlayers { .. } => {
						Self::player_process_step1(weight_meter);
					},
					PlayerProcessStep::Step2ClearIndices => {
						Self::player_process_step2(weight_meter);
					},
				},
				GameState::Cancelling { .. } => {
					Self::process_cancelling(weight_meter);
				},
			}
		}

		/// The number of attestations one player is involved in over a game with these
		/// settings: one per co-member of their group (they never report on themselves), in
		/// every round played.
		///
		/// The same count in every direction, which is why it has one definition: the votes a
		/// player can receive, the co-players a single `report` covers and the NFT claim
		/// credits an attendee can earn. Pass a game's own `rounds`/`max_group_size` for that
		/// game's bound, or the configured maxima for the worst case over any game
		/// ([`Self::max_received_votes`]).
		pub(crate) fn max_attestations(rounds: u32, max_group_size: u32) -> u32 {
			rounds.saturating_mul(max_group_size.saturating_sub(1))
		}

		/// Add a game to a player's attendance historical record.
		pub fn note_attendance(game_index: GameIdx, player: &AccountOrPerson<T::AccountId>) {
			let mut attended_games = PlayerAttendanceHistory::<T>::get(player);
			if attended_games.try_push(game_index).is_err() {
				attended_games.remove(0);
				let _ = attended_games
					.try_push(game_index)
					.defensive_proof("attended game list must hold one more game after pop");
			}
			PlayerAttendanceHistory::<T>::insert(player, attended_games);
			GameParticipantCount::<T>::mutate(game_index, |count| {
				*count = count.saturating_add(1);
			});
		}
	}
}
