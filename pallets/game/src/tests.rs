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
	mock::{deposit::MockConsideration, Game, *},
	*,
};
use codec::Encode;
use frame_support::{
	assert_noop, assert_ok,
	traits::{Get, OffchainWorker},
	BoundedVec,
};
use indiv_pallet_people::PEOPLE_MEMBER_IDENTIFIER;
use indiv_support::traits::{AppendOnlyMembers, RingExponent, RingMode};
use sp_core::{crypto::VrfSecret, ed25519, sr25519, Pair};
use sp_runtime::{testing::TestSignature, transaction_validity::InvalidTransaction, AccountId32};
use sp_statement_store::Statement;
use std::{slice, time::Duration};
use verifiable::{mock::Mock, GenerateVerifiable};

// TODO: test that offchain worker executes correctly.
// TODO: test that each validate unsigned fails when invalid.

const ALICE: AccountId32 = AccountId32::new(*b"10______________________________");
const BOB: AccountId32 = AccountId32::new(*b"20______________________________");
const CHARLIE: AccountId32 = AccountId32::new(*b"30______________________________");
const DAVE: AccountId32 = AccountId32::new(*b"40______________________________");
const EVE: AccountId32 = AccountId32::new(*b"50______________________________");

// Test one game with votes, groups and reports including
// - a player that doesn't send a report
// - a player that is not a person
#[test]
fn basic_game() {
	new_test_ext().execute_with(|| {
		let players = [
			AccountOrPerson::Account(ALICE),
			AccountOrPerson::Account(BOB),
			AccountOrPerson::Account(CHARLIE),
			AccountOrPerson::Account(DAVE),
			AccountOrPerson::Account(EVE),
		];

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 2,
			max_group_size: 3,
			..Default::default()
		};

		run_game_scenario(schedule, &players, |player| {
			let number_of_group: u32 = 2;
			let max_per_group: u32 = 3;
			let rounds: usize = 2;

			let mut full_report = Vec::new();
			for round in 0..rounds {
				let player_shuffled_position = PlayerToIndex::<Test>::get(player).unwrap()[round];
				let player_group_index = player_shuffled_position % number_of_group;
				let other_players = (0..max_per_group)
					.map(|x| player_group_index + x * number_of_group)
					.filter(|&x| x < players.len() as u32)
					.filter(|&x| x != player_shuffled_position);

				let partial_report = other_players
					.map(|x| {
						if x == PlayerToIndex::<Test>::get(AccountOrPerson::Account(BOB)).unwrap()
							[round]
						{
							Report::NotPerson
						} else {
							Report::Person
						}
					})
					.collect::<Vec<_>>();
				full_report.push(partial_report.try_into().unwrap());
			}

			// Charlie doesn't send a report
			if player.account().unwrap() == &CHARLIE {
				None
			} else {
				Some(full_report.try_into().unwrap())
			}
		});

		// Game history was recorded for the first game (index starts at 1).
		assert_eq!(GameHistory::<Test>::get(1), Some(10));

		// Bob is not a person
		let score =
			indiv_pallet_score::Participants::<Test>::get(AccountOrPerson::Account(BOB)).unwrap();
		assert_eq!(score.score, 0);
		assert_eq!(score.streak.absence(), 1);

		// Charlie didn't send a report
		let score =
			indiv_pallet_score::Participants::<Test>::get(AccountOrPerson::Account(CHARLIE))
				.unwrap();
		assert_eq!(score.score, 0);
		assert_eq!(score.streak.absence(), 1);

		// Attendance history: BOB and CHARLIE did not attend -> empty history.
		assert!(PlayerAttendanceHistory::<Test>::get(AccountOrPerson::Account(BOB)).is_empty());
		assert!(PlayerAttendanceHistory::<Test>::get(AccountOrPerson::Account(CHARLIE)).is_empty());

		// Alice ends up "Attended" so she now receives one NFT per group co-member across
		// all rounds (including Charlie, who skipped reporting). With 5 players and
		// max_group_size = 3 → number_of_group = 2, Alice (index 0 in round 0) is in a
		// group of 3 in one round (2 co-members) and a group of 2 in the other round
		// (1 co-member): 3 NFTs total.
		assert!(Nfts::<Test>::iter().count() > 0);
		assert_eq!(Nfts::<Test>::iter_prefix(&players[0]).count(), 3);
	});
}

#[test]
fn attended_player_gets_nfts_from_non_reporting_co_members() {
	// Regression test for the "mint all unminted NFTs on attendance" feature.
	//
	// Setup: a single 3-player group, single round. Alice is the only one who
	// reports (she calls everyone a `Person`). Bob and Charlie never report.
	//
	// Under the old contract, Alice — despite being attended — would receive no
	// NFTs at all because nobody reported `Person` on her. Under the new contract,
	// the moment Alice's attendance is enacted, the pallet must backfill an NFT for
	// every group co-member (Bob and Charlie) ⇒ exactly 2 NFTs for Alice.
	new_test_ext().execute_with(|| {
		let alice = AccountOrPerson::Account(ALICE);
		let bob = AccountOrPerson::Account(BOB);
		let charlie = AccountOrPerson::Account(CHARLIE);
		let players = [alice.clone(), bob.clone(), charlie.clone()];

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 3,
			..Default::default()
		};

		run_game_scenario(schedule, &players, |player| {
			// Only Alice reports; Bob and Charlie are silent (mimicking
			// early-enacted losers that skip the reporting call).
			if player.account() != Some(&ALICE) {
				return None;
			}

			// Alice's group in round 0 is the entire player set; she reports
			// every co-member as `Person`.
			let alice_index = PlayerToIndex::<Test>::get(&alice).unwrap()[0];
			let group_size: u32 = 3;
			let number_of_group: u32 = 1;
			let partial_report = (0..group_size)
				.map(|x| (alice_index % number_of_group) + x * number_of_group)
				.filter(|&x| x < players.len() as u32)
				.filter(|&x| x != alice_index)
				.map(|_| Report::Person)
				.collect::<Vec<_>>();
			Some(vec![partial_report.try_into().unwrap()].try_into().unwrap())
		});

		// Alice attended (she sent her report and her tally is 0 yes / 0 no, which
		// passes the `yes.saturating_sub(1) >= no` rule).
		let alice_score = indiv_pallet_score::Participants::<Test>::get(&alice).unwrap();
		assert!(alice_score.streak.absence() == 0, "alice should have attended");

		// Bob and Charlie did not attend (no report sent).
		let bob_score = indiv_pallet_score::Participants::<Test>::get(&bob).unwrap();
		let charlie_score = indiv_pallet_score::Participants::<Test>::get(&charlie).unwrap();
		assert_eq!(bob_score.streak.absence(), 1);
		assert_eq!(charlie_score.streak.absence(), 1);

		// The new contract: Alice has 2 NFTs — one keyed by (game, round, Bob,
		// Alice), one by (game, round, Charlie, Alice) — even though neither Bob
		// nor Charlie ever submitted a report.
		assert_eq!(Nfts::<Test>::iter_prefix(&alice).count(), 2);
		assert!(Nfts::<Test>::contains_key(&alice, Game::compute_nft(1, 0, &bob, &alice)));
		assert!(Nfts::<Test>::contains_key(&alice, Game::compute_nft(1, 0, &charlie, &alice)));
	});
}

#[test]
fn game_with_zero_player() {
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 2,
			max_group_size: 3,
			..Default::default()
		};
		run_game_scenario(schedule, &[], |_| unreachable!());
		assert!(crate::Game::<Test>::get().is_none());
	});
}

#[test]
fn starting_game_after_registration_starts() {
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 25,
			rounds: 2,
			max_group_size: 2,
			..Default::default()
		};

		let registration_start = GameTimes::<Test>::registration_start(&schedule);
		MOCK_UNIX_TIME
			.with(|v| *v.borrow_mut() = Duration::from_secs(registration_start as u64 + 1));

		// fails - can't start a game after registration has started
		assert_noop!(Game::new_game(&schedule), Error::<Test>::OutdatedGameSetup);
	});
}

// Multi-game integration test: runs game1 through all phases with hooks, then simulates
// a delay causing time to pass game2's registration_start. Starting game2 should fail.
#[test]
fn outdated_game_schedule_multi_game() {
	new_test_ext().execute_with(|| {
		let players = [AccountOrPerson::Account(ALICE), AccountOrPerson::Account(BOB)];

		// 1. Two games are scheduled with minimum valid spacing (no overlap at scheduling time).
		let durations = <Test as Config>::DefaultPhaseDurations::get();

		let game_1 = GameSchedule::<u32, u128> {
			game_play_time: 100,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};

		let game_1_player_process_end = GameTimes::<Test>::player_process_end(&game_1);
		let minimal_next_game_play_time = game_1_player_process_end +
			durations.registration +
			durations.shuffle +
			durations.post_shuffle_margin;

		let game_2 = GameSchedule::<u32, u128> {
			game_play_time: minimal_next_game_play_time,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};

		assert_ok!(Game::schedule_games(
			RuntimeOrigin::root(),
			vec![game_1.clone(), game_2.clone()]
		));
		assert_eq!(GameSchedules::<Test>::get().len(), 2);

		// Start the first game.
		advance_process();
		assert!(crate::Game::<Test>::get().is_some());
		assert_eq!(GameSchedules::<Test>::get().len(), 1);

		// 2. The first game runs through all phases with hooks.
		let game_1_registration_ends = GameTimes::<Test>::registration_end(&game_1);
		let game_1_game_play_time = GameTimes::<Test>::game_play_time(&game_1);
		let game_1_report_ends = GameTimes::<Test>::reporting_end(&game_1);

		// Sign up players.
		for p in &players {
			match p {
				AccountOrPerson::Account(acc) => {
					assert_ok!(Game::sign_up_with_account(
						RuntimeOrigin::signed(acc.clone()),
						DEFAULT_IDENTIFIER_KEY,
						None,
					));
				},
				AccountOrPerson::Person(alias) => {
					let account = AccountId32::new(*alias);
					assert_ok!(Game::sign_up_with_alias(
						runtime_origin_for_alias(alias),
						DEFAULT_IDENTIFIER_KEY,
						account.clone(),
						AccountAuthority(account),
						None,
					));
				},
			}
		}

		// End registration, shuffle.
		MOCK_UNIX_TIME
			.with(|v| *v.borrow_mut() = Duration::from_secs(game_1_registration_ends as u64 + 1));
		advance_process(); // registration -> shuffle
		advance_process(); // shuffle -> report

		// Report phase: each player submits a report using the hook pattern.
		MOCK_UNIX_TIME
			.with(|v| *v.borrow_mut() = Duration::from_secs(game_1_game_play_time as u64));

		let report_generator =
			|_player: &AccountOrPerson<AccountId32>| -> Option<FullReport<Test>> {
				Some(vec![vec![Report::Person].try_into().unwrap()].try_into().unwrap())
			};

		for p in &players {
			if let AccountOrPerson::Account(acc) = p {
				if let Some(full_report) = report_generator(p) {
					assert_ok!(Game::report(RuntimeOrigin::signed(acc.clone()), full_report));
				}
			}
		}

		// Player process phase.
		MOCK_UNIX_TIME
			.with(|v| *v.borrow_mut() = Duration::from_secs(game_1_report_ends as u64 + 1));
		advance_process(); // report -> player_process
		advance_process(); // step1 -> step2
		advance_process(); // step2 -> done

		// First game is done, second game still in schedule.
		assert!(crate::Game::<Test>::get().is_none());
		assert_eq!(GameSchedules::<Test>::get().len(), 1);

		// 3. Time is advanced past the second game's registration_start (simulating delay).
		let game_2_registration_start = GameTimes::<Test>::registration_start(&game_2);
		MOCK_UNIX_TIME
			.with(|v| *v.borrow_mut() = Duration::from_secs(game_2_registration_start as u64 + 1));

		// 4. Attempting to start the second game fails with OutdatedGameSetup.
		advance_process(); // on_poll tries to start the game but it is already too late -> skipped.
		assert_eq!(GameSchedules::<Test>::get().len(), 0);
		assert!(crate::Game::<Test>::get().is_none());
	});
}

// Test 2 successive games:
// - a player that signs up for the first game but not for the second game
#[test]
fn not_registered_in_game_is_equal_absent() {
	new_test_ext().execute_with(|| {
		let players = [AccountOrPerson::Account(ALICE), AccountOrPerson::Account(BOB)];

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 2,
			max_group_size: 2,
			..Default::default()
		};

		run_game_scenario(schedule, &players, |_player| {
			Some(
				vec![
					vec![Report::Person].try_into().unwrap(),
					vec![Report::Person].try_into().unwrap(),
				]
				.try_into()
				.unwrap(),
			)
		});

		// Game history for first game and attendance history for both players.
		assert_eq!(GameHistory::<Test>::get(1), Some(10));
		assert_eq!(PlayerAttendanceHistory::<Test>::get(AccountOrPerson::Account(ALICE)), vec![1]);
		assert_eq!(PlayerAttendanceHistory::<Test>::get(AccountOrPerson::Account(BOB)), vec![1]);

		for player in players {
			let score = indiv_pallet_score::Participants::<Test>::get(player).unwrap();
			assert_eq!(score.score, 1);
			assert_eq!(score.streak.attendance(), 1);
		}

		let players = [AccountOrPerson::Account(ALICE)];

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 25,
			rounds: 2,
			max_group_size: 2,
			..Default::default()
		};

		run_game_scenario(schedule, &players, |_player| {
			Some(vec![BoundedVec::default(), BoundedVec::default()].try_into().unwrap())
		});

		// Second game's history recorded and ALICE attendance history includes both games.
		assert_eq!(GameHistory::<Test>::get(2), Some(25));
		assert_eq!(
			PlayerAttendanceHistory::<Test>::get(AccountOrPerson::Account(ALICE)),
			vec![1, 2]
		);

		for player in players {
			let score = indiv_pallet_score::Participants::<Test>::get(player).unwrap();
			assert_eq!(score.score, 3);
			assert_eq!(score.streak.attendance(), 2);
		}

		// Bob didn't register
		let score =
			indiv_pallet_score::Participants::<Test>::get(AccountOrPerson::Account(BOB)).unwrap();
		assert_eq!(score.score, 0);
		assert_eq!(score.streak.absence(), 1);
	});
}

#[test]
fn offboard_archived_player() {
	new_test_ext().execute_with(|| {
		// ------------------------------------------------------
		// 1) Create a new game with exactly ONE player, who ends up being archived at the end
		//    (score == 0).
		// ------------------------------------------------------

		// We'll use a single player named ALICE
		let alice = AccountOrPerson::Account(ALICE);

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 25,
			rounds: 2,
			max_group_size: 2,
			..Default::default()
		};

		run_game_scenario(
			schedule,
			slice::from_ref(&alice),
			|_player| None, // ALICE doesn't report at all
		);

		// At this point, ALICE is archived, because she ends up with zero score:
		assert!(ArchivedPlayers::<Test>::contains_key(&alice), "Player should have been archived");

		// ------------------------------------------------------
		// 2) Offboard the archived player
		// ------------------------------------------------------

		// We call offboard using ALICE's signature origin
		assert_ok!(Game::offboard(RuntimeOrigin::signed(ALICE)));

		// The Offboarded event is emitted.
		System::assert_has_event(Event::<Test>::Offboarded { who: alice.clone() }.into());

		// offboard() removes them from ArchivedPlayers and from indiv_pallet_score as well
		assert!(
			!ArchivedPlayers::<Test>::contains_key(&alice),
			"Player should no longer be archived after offboard"
		);

		// Also confirm that the player is no longer in the Score participants map
		assert_eq!(
			indiv_pallet_score::Participants::<Test>::get(&alice),
			None,
			"Player must be removed from the Score participants too"
		);
	});
}

#[test]
fn offboard_live_player() {
	use sp_statement_store::{get_allowance, StatementAllowance};

	new_test_ext().execute_with(|| {
		// ------------------------------------------------------
		// 1) Create a new game with ALICE, who remains "live" (i.e. not archived).
		// ------------------------------------------------------

		let alice = AccountOrPerson::Account(ALICE);
		let expected_allowance: StatementAllowance = PlayerStatementLimit::get();
		let zero_allowance = StatementAllowance::default();

		// Initial allowance should be zero.
		assert_eq!(get_allowance(ALICE), zero_allowance);

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 25,
			rounds: 2,
			max_group_size: 2,
			..Default::default()
		};

		run_game_scenario(schedule, slice::from_ref(&alice), |_player| {
			Some(vec![BoundedVec::default(), BoundedVec::default()].try_into().unwrap())
		});

		// ALICE is a "live" participant => was never archived
		assert!(!ArchivedPlayers::<Test>::contains_key(&alice), "ALICE should not be archived");
		// She should have a nonzero score
		let score =
			indiv_pallet_score::Participants::<Test>::get(&alice).expect("Score must exist");
		assert!(score.score > 0, "ALICE must have a positive score, thus not archived");
		// Allowance should be set for the live player.
		assert_eq!(
			get_allowance(ALICE),
			expected_allowance,
			"ALICE should have allowance while being a live player"
		);

		// ------------------------------------------------------
		// 2) Offboard the "live" (non-archived) player
		// ------------------------------------------------------

		// We call offboard
		assert_ok!(Game::offboard(RuntimeOrigin::signed(ALICE)));

		// The Offboarded event is emitted.
		System::assert_has_event(Event::<Test>::Offboarded { who: alice.clone() }.into());

		// Confirm that it removed ALICE from Players or ArchivedPlayers
		assert!(!Players::<Test>::contains_key(&alice), "Player storage must be gone");
		assert!(
			!ArchivedPlayers::<Test>::contains_key(&alice),
			"ArchivedPlayers entry must also be gone"
		);
		// Confirm removed from Score
		assert_eq!(
			indiv_pallet_score::Participants::<Test>::get(&alice),
			None,
			"Must be removed from Score participants"
		);
		// Allowance must be cleared after offboarding.
		assert_eq!(
			get_allowance(ALICE),
			zero_allowance,
			"ALICE allowance should be 0 after offboarding as a live player"
		);
	});
}

#[test]
fn archived_player_can_sign_up_again() {
	new_test_ext().execute_with(|| {
		// ------------------------------------------------------
		// 1) Create a game with ALICE, who is archived by having zero score after that game
		//    finishes.
		// ------------------------------------------------------

		let alice = AccountOrPerson::Account(ALICE);

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 2,
			max_group_size: 1,
			..Default::default()
		};

		run_game_scenario(
			schedule,
			slice::from_ref(&alice),
			|_player| None, // ALICE doesn't report at all
		);

		// She should now be archived
		assert!(ArchivedPlayers::<Test>::contains_key(&alice));

		// ------------------------------------------------------
		// 2) Start a brand new game, ALICE tries to sign up again. This triggers the
		//    "ArchivedPlayers::<T>::take(...)" logic that reintroduces her in `Players`.
		// ------------------------------------------------------

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 25,
			rounds: 2,
			max_group_size: 1,
			..Default::default()
		};

		run_game_scenario(schedule, slice::from_ref(&alice), |_player| {
			Some(vec![BoundedVec::default(), BoundedVec::default()].try_into().unwrap())
		});

		// Check that this removed ALICE from Archived
		assert!(
			!ArchivedPlayers::<Test>::contains_key(&alice),
			"Archived player must be cleared once they sign up again"
		);
		assert!(Players::<Test>::contains_key(&alice), "Player should now reappear in Players");
	});
}

#[test]
fn cannot_offboard_during_game() {
	new_test_ext().execute_with(|| {
		// ------------------------------------------------------
		// 1) Create a game with ALICE
		// ------------------------------------------------------

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 2,
			max_group_size: 1,
			..Default::default()
		};

		assert_ok!(Game::new_game(&schedule));

		// Sign up
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(ALICE),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));

		// ------------------------------------------------------
		// 2) try offboarding
		// ------------------------------------------------------
		assert_noop!(
			Game::offboard(RuntimeOrigin::signed(ALICE)),
			Error::<Test>::CannotOffboardWhileRegisteredForGame,
		);
	});
}

#[test]
fn cannot_register_twice() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		// 1) Create a new game
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 25,
			rounds: 2,
			max_group_size: 1,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));

		// 2) First sign_up should succeed
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(ALICE),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));

		// The SignedUp event is emitted.
		System::assert_has_event(
			Event::<Test>::SignedUp { who: AccountOrPerson::Account(ALICE) }.into(),
		);

		// 3) Second sign_up should fail with `AlreadyRegistered`
		assert_noop!(
			Game::sign_up_with_account(RuntimeOrigin::signed(ALICE), DEFAULT_IDENTIFIER_KEY, None),
			Error::<Test>::AlreadyRegistered
		);
	});
}

#[test]
fn cannot_report_twice() {
	new_test_ext().execute_with(|| {
		// Create a new game.
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 20,
			rounds: 1,
			max_group_size: 1,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));

		// Sign up player.
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(ALICE),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));

		// Fast-forward time beyond registration_ends, so we can transition.
		let registration_ends = GameTimes::<Test>::registration_end(&schedule);
		MOCK_UNIX_TIME
			.with(|v| *v.borrow_mut() = Duration::from_secs(registration_ends as u64 + 1));

		advance_process(); // Transition to shuffle.
		advance_process(); // Shuffle to report.

		// Now the game is in `Reporting` phase.

		// Player sees exactly 0 other player.
		let partial_report: BoundedVec<Report, _> = BoundedVec::default();
		let full_report: BoundedVec<_, _> = vec![partial_report]
			.try_into()
			.expect("Failed to convert Vec<BoundedVec<Report>> into FullReport<T>");

		// First call to report is OK.
		assert_ok!(Game::report(RuntimeOrigin::signed(ALICE), full_report.clone()));

		// The ReportSubmitted event is emitted.
		System::assert_has_event(
			Event::<Test>::ReportSubmitted {
				who: AccountOrPerson::Account(ALICE),
				game_index: GameIndex::<Test>::get(),
			}
			.into(),
		);

		// Second call to report should fail with ReportAlreadySent.
		assert_noop!(
			Game::report(RuntimeOrigin::signed(ALICE), full_report),
			Error::<Test>::ReportAlreadySent
		);
	});
}

mod games_scheduling {
	use super::*;
	use frame_support::pallet_prelude::Get;
	use sp_runtime::Weight;

	#[test]
	fn schedule_games_validates_game_params() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			// No ongoing game
			assert!(crate::Game::<Test>::get().is_none());

			// No scheduled games
			assert_eq!(GameSchedules::<Test>::get().len(), 0);

			// Attempt to schedule a game with invalid number of rounds fails
			assert_noop!(
				Game::schedule_games(
					RuntimeOrigin::root(),
					vec![GameSchedule::<u32, u128> {
						game_play_time: 100,
						rounds: 0, // rounds must be > 0
						max_group_size: 2,
						..Default::default()
					}]
				),
				Error::<Test>::InvalidGameSetup
			);

			// Attempt to schedule a game with invalid group size fails
			assert_noop!(
				Game::schedule_games(
					RuntimeOrigin::root(),
					vec![GameSchedule::<u32, u128> {
						game_play_time: 100,
						rounds: 2,
						max_group_size: 0, // max_group_size must be > 0
						..Default::default()
					}]
				),
				Error::<Test>::InvalidGameSetup
			);

			// Success given valid parameters
			assert_ok!(Game::schedule_games(
				RuntimeOrigin::root(),
				vec![GameSchedule::<u32, u128> {
					game_play_time: 100,
					rounds: 2,
					max_group_size: 2,
					..Default::default()
				}]
			));

			// The GamesScheduled event is emitted.
			System::assert_has_event(Event::<Test>::GamesScheduled { count: 1 }.into());

			assert_eq!(GameSchedules::<Test>::get().len(), 1);
		});
	}

	#[test]
	fn schedule_games_checks_if_max_games_scheduled_limit_is_respected() {
		new_test_ext().execute_with(|| {
			// let offset: u32 = <Test as Config>::GamePhasesOffset::get();

			// No ongoing game
			assert!(crate::Game::<Test>::get().is_none());

			// No scheduled games
			assert_eq!(GameSchedules::<Test>::get().len(), 0);

			// Attempt to schedule more than the limit fails
			let max_games: usize = <Test as Config>::MaxGameSchedules::get();
			let too_many_games = (0..max_games + 1)
				.map(|i| GameSchedule::<u32, u128> {
					game_play_time: i as u32 * 100,
					rounds: 2,
					max_group_size: 2,
					..Default::default()
				})
				.collect::<Vec<_>>();

			assert_noop!(
				Game::schedule_games(RuntimeOrigin::root(), too_many_games),
				Error::<Test>::TooManyGameSchedules
			);

			// Attempt to schedule less than the limit succeeds
			let games_within_limit = (0..max_games - 1)
				.map(|i| GameSchedule::<u32, u128> {
					game_play_time: i as u32 * 100,
					rounds: 2,
					max_group_size: 2,
					..Default::default()
				})
				.collect::<Vec<_>>();

			assert_ok!(Game::schedule_games(RuntimeOrigin::root(), games_within_limit.clone()));

			assert_eq!(GameSchedules::<Test>::get().len(), max_games - 1);

			// Attempt to schedule more games that would cross the limit fails
			assert_noop!(
				Game::schedule_games(
					RuntimeOrigin::root(),
					vec![
						GameSchedule::<u32, u128> {
							game_play_time: 100,
							rounds: 2,
							max_group_size: 2,
							..Default::default()
						},
						GameSchedule::<u32, u128> {
							game_play_time: 200,
							rounds: 2,
							max_group_size: 2,
							..Default::default()
						}
					]
				),
				Error::<Test>::TooManyGameSchedules
			);

			// Attempt to schedule more games, that would reach the limit succeeds
			assert_ok!(Game::schedule_games(
				RuntimeOrigin::root(),
				vec![GameSchedule::<u32, u128> {
					game_play_time: max_games as u32 * 100,
					rounds: 2,
					max_group_size: 2,
					..Default::default()
				}]
			));

			assert_eq!(GameSchedules::<Test>::get().len(), max_games);
		});
	}

	#[test]
	fn schedule_games_checks_chronological_order_of_provided_schedules() {
		new_test_ext().execute_with(|| {
			// No ongoing game
			assert!(crate::Game::<Test>::get().is_none());

			// No scheduled games
			assert_eq!(GameSchedules::<Test>::get().len(), 0);

			// Attempt to schedule games listed in non-chronological order fails
			assert_noop!(
				Game::schedule_games(
					RuntimeOrigin::root(),
					vec![
						GameSchedule::<u32, u128> {
							game_play_time: 200,
							rounds: 2,
							max_group_size: 2,
							..Default::default()
						},
						GameSchedule::<u32, u128> {
							game_play_time: 100, // Earlier than the previous game
							rounds: 2,
							max_group_size: 2,
							..Default::default()
						}
					]
				),
				Error::<Test>::InvalidGameSetup
			);

			// Attempt to schedule games listed in chronological order succeeds
			assert_ok!(Game::schedule_games(
				RuntimeOrigin::root(),
				vec![
					GameSchedule::<u32, u128> {
						game_play_time: 100,
						rounds: 2,
						max_group_size: 2,
						..Default::default()
					},
					GameSchedule::<u32, u128> {
						game_play_time: 200,
						rounds: 2,
						max_group_size: 2,
						..Default::default()
					}
				]
			));

			// One ongoing game exists - just before the previously scheduled games
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 20,
				rounds: 2,
				max_group_size: 2,
				..Default::default()
			};
			assert_ok!(Game::new_game(&schedule));

			// The above scenario once again - games times correct and past last scheduled game
			assert_ok!(Game::schedule_games(
				RuntimeOrigin::root(),
				vec![
					GameSchedule::<u32, u128> {
						game_play_time: 300,
						rounds: 2,
						max_group_size: 2,
						..Default::default()
					},
					GameSchedule::<u32, u128> {
						game_play_time: 400,
						rounds: 2,
						max_group_size: 2,
						..Default::default()
					}
				]
			));
		});
	}

	#[test]
	fn schedule_games_checks_for_time_overlaps_between_games() {
		new_test_ext().execute_with(|| {
			// No ongoing game
			assert!(crate::Game::<Test>::get().is_none());

			// No scheduled games
			assert_eq!(GameSchedules::<Test>::get().len(), 0);

			// Attempt to schedule two games that overlap fails
			let game1 = GameSchedule::<u32, u128> {
				game_play_time: 100,
				rounds: 2,
				max_group_size: 2,
				..Default::default()
			};

			let durations = <Test as Config>::DefaultPhaseDurations::get();
			let game1_player_process_end = GameTimes::<Test>::player_process_end(&game1);
			let minimal_next_game_play_time = game1_player_process_end +
				durations.registration +
				durations.shuffle +
				durations.post_shuffle_margin;
			let game2 = GameSchedule::<u32, u128> {
				// game2 reporting would start at game1 estimated end, leaving no time for
				// registration and shuffle
				game_play_time: game1_player_process_end,
				rounds: 2,
				max_group_size: 2,
				..Default::default()
			};

			assert_noop!(
				Game::schedule_games(RuntimeOrigin::root(), vec![game1.clone(), game2]),
				Error::<Test>::InvalidGameSetup
			);

			let game2_bis = GameSchedule::<u32, u128> {
				// Sill off by one second
				game_play_time: minimal_next_game_play_time - 1,
				rounds: 2,
				max_group_size: 2,
				..Default::default()
			};

			assert_noop!(
				Game::schedule_games(RuntimeOrigin::root(), vec![game1.clone(), game2_bis]),
				Error::<Test>::InvalidGameSetup
			);

			// Succeeds otherwise
			let game2_no_overlap = GameSchedule::<u32, u128> {
				game_play_time: minimal_next_game_play_time,
				rounds: 2,
				max_group_size: 2,
				..Default::default()
			};

			assert_ok!(Game::schedule_games(RuntimeOrigin::root(), vec![game1, game2_no_overlap]));

			assert_eq!(GameSchedules::<Test>::get().len(), 2);
		});
	}

	#[test]
	fn games_to_schedule_must_be_after_currently_scheduled_ones() {
		new_test_ext().execute_with(|| {
			// No ongoing game
			assert!(crate::Game::<Test>::get().is_none());

			// Several scheduled games
			let existing_games = vec![
				GameSchedule::<u32, u128> {
					game_play_time: 100,
					rounds: 2,
					max_group_size: 2,
					..Default::default()
				},
				GameSchedule::<u32, u128> {
					game_play_time: 200,
					rounds: 2,
					max_group_size: 2,
					..Default::default()
				},
				GameSchedule::<u32, u128> {
					game_play_time: 300,
					rounds: 2,
					max_group_size: 2,
					..Default::default()
				},
			];

			assert_ok!(Game::schedule_games(RuntimeOrigin::root(), existing_games.clone()));

			// Attempt to schedule a game before the scheduled ones fails
			assert_noop!(
				Game::schedule_games(
					RuntimeOrigin::root(),
					vec![GameSchedule::<u32, u128> {
						game_play_time: 50, // Before first game
						rounds: 2,
						max_group_size: 2,
						..Default::default()
					}]
				),
				Error::<Test>::InvalidGameSetup
			);

			// Attempt to schedule a game in between the scheduled ones fails
			assert_noop!(
				Game::schedule_games(
					RuntimeOrigin::root(),
					vec![GameSchedule::<u32, u128> {
						game_play_time: 150, // Between first and second games
						rounds: 2,
						max_group_size: 2,
						..Default::default()
					}]
				),
				Error::<Test>::InvalidGameSetup
			);

			// Attempt to schedule a games after the scheduled ones succeeds
			assert_ok!(Game::schedule_games(
				RuntimeOrigin::root(),
				vec![GameSchedule::<u32, u128> {
					game_play_time: 400, // After all existing games
					rounds: 2,
					max_group_size: 2,
					..Default::default()
				}]
			));

			assert_eq!(GameSchedules::<Test>::get().len(), 4);
		});
	}

	#[test]
	fn remove_scheduled_game_fails_given_not_existing_game() {
		new_test_ext().execute_with(|| {
			// Two scheduled games
			assert_ok!(Game::schedule_games(
				RuntimeOrigin::root(),
				vec![
					GameSchedule::<u32, u128> {
						game_play_time: 100,
						rounds: 2,
						max_group_size: 2,
						..Default::default()
					},
					GameSchedule::<u32, u128> {
						game_play_time: 200,
						rounds: 2,
						max_group_size: 2,
						..Default::default()
					}
				]
			));

			// Attempt to remove a game that doesn't exist in the schedule fails
			assert_noop!(
				Game::remove_scheduled_game(RuntimeOrigin::root(), 300),
				Error::<Test>::NoSuchGameScheduled
			);

			// The schedule remains unchanged
			assert_eq!(GameSchedules::<Test>::get().len(), 2);
		});
	}

	#[test]
	fn remove_scheduled_game_succeeds_for_games_regardless_their_order_in_schedule() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			// Several scheduled games
			assert_ok!(Game::schedule_games(
				RuntimeOrigin::root(),
				vec![
					GameSchedule::<u32, u128> {
						game_play_time: 100,
						rounds: 2,
						max_group_size: 2,
						..Default::default()
					},
					GameSchedule::<u32, u128> {
						game_play_time: 200,
						rounds: 2,
						max_group_size: 2,
						..Default::default()
					},
					GameSchedule::<u32, u128> {
						game_play_time: 300,
						rounds: 2,
						max_group_size: 2,
						..Default::default()
					},
					GameSchedule::<u32, u128> {
						game_play_time: 400,
						rounds: 2,
						max_group_size: 2,
						..Default::default()
					},
					GameSchedule::<u32, u128> {
						game_play_time: 500,
						rounds: 2,
						max_group_size: 2,
						..Default::default()
					}
				]
			));
			assert_eq!(GameSchedules::<Test>::get().len(), 5);

			// Attempt to remove the 2nd game succeeds
			assert_ok!(Game::remove_scheduled_game(RuntimeOrigin::root(), 200));
			System::assert_has_event(
				Event::<Test>::ScheduledGameRemoved { game_play_time: 200 }.into(),
			);
			assert_eq!(GameSchedules::<Test>::get().len(), 4);

			// Attempt to remove the last game succeeds
			assert_ok!(Game::remove_scheduled_game(RuntimeOrigin::root(), 500));
			System::assert_has_event(
				Event::<Test>::ScheduledGameRemoved { game_play_time: 500 }.into(),
			);
			assert_eq!(GameSchedules::<Test>::get().len(), 3);

			// Attempt to remove the first game succeeds
			assert_ok!(Game::remove_scheduled_game(RuntimeOrigin::root(), 100));
			System::assert_has_event(
				Event::<Test>::ScheduledGameRemoved { game_play_time: 100 }.into(),
			);
			assert_eq!(GameSchedules::<Test>::get().len(), 2);
		});
	}

	/// Describes the full execution flow of games scheduling, including execution of scheduled
	/// games.
	/// Initial state: no ongoing game, no scheduled games.
	/// The flow goes as follows:
	/// 1. A few games are scheduled: ManagerOrigin calls schedule_games.
	/// 2. Enough time passes for the first scheduled game to become the ongoing game (through
	///    on_poll trigger).
	/// 3. The first game passes through all of its phases and is no longer the ongoing game
	///    (removed from the storage item).
	/// 4. The second game from the schedule is scheduled as next game.
	/// 5. The process repeats till there's no more scheduled games.
	#[test]
	fn games_scheduling_flow() {
		new_test_ext().execute_with(|| {
			let players = [AccountOrPerson::Account(ALICE), AccountOrPerson::Account(BOB)];

			// 1. A few games are scheduled
			let schedules = vec![
				GameSchedule::<u32, u128> {
					game_play_time: 100,
					rounds: 1,
					max_group_size: 2,
					..Default::default()
				},
				GameSchedule::<u32, u128> {
					game_play_time: 200,
					rounds: 1,
					max_group_size: 2,
					..Default::default()
				},
				GameSchedule::<u32, u128> {
					game_play_time: 300,
					rounds: 1,
					max_group_size: 2,
					..Default::default()
				},
			];
			assert_ok!(Game::schedule_games(RuntimeOrigin::root(), schedules.clone()));
			assert_eq!(GameSchedules::<Test>::get().len(), 3);

			// 2. Enough time passes for the first scheduled game to become the ongoing game. No
			// ongoing game, so the first one from the schedule will become one.
			advance_process();
			assert!(crate::Game::<Test>::get().is_some());
			assert_eq!(GameSchedules::<Test>::get().len(), 2);

			// 3. The first game passes through all of its phases and is no longer the ongoing game.
			let game_1 = schedules[0].clone();
			run_basic_game_scenario_with_hooks(game_1, &players);

			// 4. The second game from the schedule is scheduled as next game.
			let game_2 = &schedules[1];
			assert_eq!(GameSchedules::<Test>::get().len(), 1);
			assert!(crate::Game::<Test>::get().is_some());
			assert_eq!(crate::Game::<Test>::get().unwrap().game_date, game_2.game_play_time);

			// 5. The process repeats till there's no more scheduled games.

			// The second game is executed
			run_basic_game_scenario_with_hooks(game_2.clone(), &players);

			// The last game becomes the ongoing game
			let game_3 = &schedules[2];
			assert_eq!(GameSchedules::<Test>::get().len(), 0);
			assert!(crate::Game::<Test>::get().is_some());
			assert_eq!(crate::Game::<Test>::get().unwrap().game_date, game_3.game_play_time);

			// The last game is executed
			run_basic_game_scenario_with_hooks(game_3.clone(), &players);
			assert!(crate::Game::<Test>::get().is_none());
		});
	}

	/// Verifies what happens in a case when there are more players to process than what can be
	/// achieved in one block for shuffle and player process phases
	#[test]
	fn many_players_to_process() {
		new_test_ext().execute_with(|| {
			// Generate 1000 players
			let mut players = vec![];
			for i in 0..1000 {
				let player = if i % 2 == 0 {
					AccountOrPerson::Account(id_to_account(i))
				} else {
					AccountOrPerson::Person(id_to_alias(i))
				};
				players.push(player);
			}

			// Some weight: big enough to process some shuffle and player process, and small enough
			// to not be able to process 1000 players in one block. We double because the hooks halve
			// the available weight
			let weights = Weight::from_parts(2 * (15 + 10 * 10), 2 * (15 + 10 * 10));

			// Two games are scheduled
			let schedules = vec![
				GameSchedule::<u32, u128> { game_play_time: 100, rounds: 1, max_group_size: 10, ..Default::default() },
				GameSchedule::<u32, u128> { game_play_time: 200, rounds: 1, max_group_size: 10, ..Default::default() },
			];
			assert_ok!(Game::schedule_games(RuntimeOrigin::root(), schedules.clone()));
			assert_eq!(GameSchedules::<Test>::get().len(), 2);

			// The first game registration starts
			advance_process();
			assert!(crate::Game::<Test>::get().is_some());
			assert_eq!(GameSchedules::<Test>::get().len(), 1);
			assert!(
				crate::Game::<Test>::get().unwrap().state ==
					GameState::Registration { next_player_index: 0 }
			);

			// --- Below the game rolls through all of its phases

			let game = &schedules[0];
			// Sign up
			for p in &players {
				match p {
					AccountOrPerson::Account(account_id) => {
						assert_ok!(Game::sign_up_with_account(
							RuntimeOrigin::signed(account_id.clone()),
							DEFAULT_IDENTIFIER_KEY,
							None,
						));
					},
					AccountOrPerson::Person(alias) => {
						let stmt_account = AccountId32::new(*alias);
						assert_ok!(Game::sign_up_with_alias(
							runtime_origin_for_alias(alias),
							DEFAULT_IDENTIFIER_KEY,
							stmt_account.clone(),
							AccountAuthority(stmt_account),
							None,
						));
					},
				}
			}

			// Move time, end registration, shuffle
			MOCK_UNIX_TIME.with(|v| {
				*v.borrow_mut() = Duration::from_secs(
					<GameSchedule<u32, u128> as GameTimes<Test>>::registration_end(game) as u64 + 1,
				)
			});
			advance_process_with_weights(weights, Weight::zero()); // register to shuffle
			assert_eq!(
				crate::Game::<Test>::get().unwrap().state,
				GameState::Shuffle { step: ShuffleStep::Step1Insert { last_iteration: None } }
			);

			advance_process_with_weights(weights, weights); // some incomplete shuffle
			assert!(
				matches!(
					crate::Game::<Test>::get().unwrap().state,
					GameState::Shuffle {
						step: ShuffleStep::Step1Insert { last_iteration: Some(_) }
					},
				),
				"Phase must still be in shuffle, we are testing for multi block shuffle",
			);

			let mut stop_at_step2 = false;

			// Finish the shuffle
			while matches!(crate::Game::<Test>::get().unwrap().state, GameState::Shuffle { .. }) {
				advance_process_with_weights(weights, weights); // continue the shuffle
				stop_at_step2 |= matches!(
					crate::Game::<Test>::get().unwrap().state,
					GameState::Shuffle { step: ShuffleStep::Step2Retrieve { .. }}
				);
			}

			assert!(stop_at_step2, "To ensure correct flow if step 2 take multiple steps, we must check that it stopped at least once in step 2");

			// Now state is in report
			assert_eq!(
				crate::Game::<Test>::get().unwrap().state,
				GameState::Reporting { player_count: 1000 }
			);

			// Move time to game day
			MOCK_UNIX_TIME.with(|v| {
				*v.borrow_mut() =
					Duration::from_secs(GameTimes::<Test>::game_play_time(game) as u64)
			});

			let mut report = vec![];
			for _ in 0..9 {
				report.push(Report::Person);
			}

			// Each player sends a report for all other players
			for p in players {
				if let AccountOrPerson::Account(account_id) = p {
					assert_ok!(Game::report(
						RuntimeOrigin::signed(account_id),
						vec![report.clone().try_into().unwrap()].try_into().unwrap()
					));
				}
			}

			// Move time to after report_ends
			MOCK_UNIX_TIME.with(|v| {
				*v.borrow_mut() = Duration::from_secs(
					<GameSchedule<u32, u128> as GameTimes<Test>>::reporting_end(game) as u64 + 1,
				)
			});
			advance_process_with_weights(weights, Weight::zero()); // report to player process
			assert!(matches!(
				crate::Game::<Test>::get().unwrap().state,
				GameState::PlayerProcess {
					step: PlayerProcessStep::Step1ProcessPlayers { last_iteration: None, .. },
				}
			));

			advance_process_with_weights(weights, weights); // some incomplete player process
			assert!(
				matches!(
					crate::Game::<Test>::get().unwrap().state,
					GameState::PlayerProcess {
						step: PlayerProcessStep::Step1ProcessPlayers {
							last_iteration: Some(_),
							..
						},
					}
				),
				"Phase must still be in processing, we are testing for multi block processing",
			);

			// finish the player process, then the clearing-indices and ending phases
			while crate::Game::<Test>::get().is_some() {
				// We use zero for idle weight in order not to trigger the next game in on_idle.
				advance_process_with_weights(weights, Weight::zero());
			}

			// Now the game is finished
			assert!(crate::Game::<Test>::get().is_none());

			advance_process_with_weights(weights, weights); // finished game to next game

			// New game has started
			assert_eq!(
				crate::Game::<Test>::get().unwrap().state,
				GameState::Registration { next_player_index: 0 }
			);
		});
	}
}

#[test]
fn kickout_scenarios() {
	new_test_ext().execute_with(|| {
		let kickout_time: u64 = <Test as Config>::NonPlayingKickoutTime::get();
		use frame_system::Pallet as SystemPallet;

		let alice_key = AccountOrPerson::Account(ALICE);

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};

		run_game_scenario(schedule, &[AccountOrPerson::Account(ALICE)], |_player| None);

		assert!(
			ArchivedPlayers::<Test>::contains_key(&alice_key),
			"alice_account must be in ArchivedPlayers"
		);

		// Check `kickout` fails if not enough blocks have passed (Error::Early)
		let archived_since = match ArchivedPlayers::<Test>::get(&alice_key).unwrap() {
			ArchivedPlayer::Kickable { archived_since, .. } => archived_since,
			ArchivedPlayer::Unkickable { .. } => panic!("alice must be kickable"),
		};
		SystemPallet::<Test>::set_block_number(archived_since + kickout_time);
		assert_noop!(Game::kickout(RuntimeOrigin::signed(EVE), ALICE), Error::<Test>::Early);

		// Succeeds if enough blocks have passed
		SystemPallet::<Test>::set_block_number(archived_since + kickout_time + 1);
		assert_ok!(Game::kickout(RuntimeOrigin::signed(EVE), ALICE));

		// The KickedOut event is emitted.
		System::assert_has_event(Event::<Test>::KickedOut { player: alice_key.clone() }.into());

		// Confirm the archived record is removed, and the score participant is removed
		assert!(
			!ArchivedPlayers::<Test>::contains_key(&alice_key),
			"kickout must remove from ArchivedPlayers"
		);
		assert_eq!(
			indiv_pallet_score::Participants::<Test>::get(&alice_key),
			None,
			"kickout also removes from indiv_pallet_score"
		);
	});
}

#[test]
fn person_cant_be_kicked() {
	new_test_ext().execute_with(|| {
		use crate::ArchivedPlayer;

		// We use a Person.
		let unkickable_player = AccountOrPerson::Person([1u8; 32]);

		// Now run a game scenario in which this externally recognized participant
		// fails to report (=> archived as Unkickable).
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		run_game_scenario(
			schedule,
			std::slice::from_ref(&unkickable_player),
			/* no reporting => absent */
			|_p| None,
		);

		// Confirm that the participant is now archived as `Unkickable`.
		let archived = ArchivedPlayers::<Test>::get(&unkickable_player).expect("Must be archived");

		assert!(matches!(archived, ArchivedPlayer::Unkickable { .. }));
	});
}

// `offboard` must not erase `PlayerAttendanceHistory`.
#[test]
fn offboard_retains_attendance_history() {
	new_test_ext().execute_with(|| {
		let alice = AccountOrPerson::Account(ALICE);
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 25,
			rounds: 2,
			max_group_size: 2,
			..Default::default()
		};

		// ALICE never reports → archived for inactivity, satisfies offboard's
		// `archived.is_some()` precondition.
		run_game_scenario(schedule, slice::from_ref(&alice), |_player| None);
		assert!(ArchivedPlayers::<Test>::contains_key(&alice));

		// Inject an attendance from an earlier game so we have something to test the
		// retention against (ALICE didn't report this game, so her natural attendance
		// is empty).
		let earlier_game = 99u32;
		Game::note_attendance(earlier_game, &alice);
		assert!(PlayerAttendanceHistory::<Test>::get(&alice).contains(&earlier_game));

		assert_ok!(Game::offboard(RuntimeOrigin::signed(ALICE)));

		assert!(
			PlayerAttendanceHistory::<Test>::get(&alice).contains(&earlier_game),
			"offboard must not erase PlayerAttendanceHistory",
		);
	});
}

// Same invariant for `kickout`. See `offboard_retains_attendance_history`.
#[test]
fn kickout_retains_attendance_history() {
	new_test_ext().execute_with(|| {
		use frame_system::Pallet as SystemPallet;
		let kickout_time: u64 = <Test as Config>::NonPlayingKickoutTime::get();
		let alice = AccountOrPerson::Account(ALICE);
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};

		// ALICE doesn't report → archived as kickable.
		run_game_scenario(schedule, slice::from_ref(&alice), |_player| None);
		assert!(ArchivedPlayers::<Test>::contains_key(&alice));

		let earlier_game = 99u32;
		Game::note_attendance(earlier_game, &alice);

		let since = SystemPallet::<Test>::block_number();
		SystemPallet::<Test>::set_block_number(since + kickout_time + 1);
		assert_ok!(Game::kickout(RuntimeOrigin::signed(EVE), ALICE));

		assert!(
			PlayerAttendanceHistory::<Test>::get(&alice).contains(&earlier_game),
			"kickout must not erase PlayerAttendanceHistory",
		);
	});
}

// Both Person- and Account-attendees count toward `GameParticipantCount`.
#[test]
fn note_attendance_counts_both_account_and_person_attendees() {
	new_test_ext().execute_with(|| {
		let game_index = 7u32;
		let alice = AccountOrPerson::Account(ALICE);
		let alias: Alias = [42u8; 32];
		let bob_person = AccountOrPerson::Person(alias);

		Game::note_attendance(game_index, &alice);
		assert!(PlayerAttendanceHistory::<Test>::get(&alice).contains(&game_index));
		assert_eq!(GameParticipantCount::<Test>::get(game_index), 1);

		Game::note_attendance(game_index, &bob_person);
		assert!(PlayerAttendanceHistory::<Test>::get(&bob_person).contains(&game_index));
		assert_eq!(GameParticipantCount::<Test>::get(game_index), 2);
	});
}

#[test]
fn manager_can_set_play_deposit() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		assert_eq!(PlayDepositAmount::<Test>::get(), 2);
		assert_ok!(Game::set_play_deposit(RuntimeOrigin::root(), 42));
		assert_eq!(PlayDepositAmount::<Test>::get(), 42);
		System::assert_last_event(Event::PlayDepositSet { amount: 42 }.into());
	});
}

#[test]
fn non_manager_cannot_set_play_deposit() {
	new_test_ext().execute_with(|| {
		assert_noop!(
			Game::set_play_deposit(RuntimeOrigin::signed(ALICE), 42),
			sp_runtime::DispatchError::BadOrigin
		);
	});
}

#[test]
fn play_deposit_change_only_applies_to_future_signups() {
	new_test_ext().execute_with(|| {
		use deposit::DepositStorage;

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));

		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(ALICE),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));
		assert_eq!(
			DepositStorage::<Test>::get().active.into_inner(),
			vec![(ALICE, MockConsideration(0))]
		);

		assert_ok!(Game::set_play_deposit(RuntimeOrigin::root(), 99));
		assert_eq!(PlayDepositAmount::<Test>::get(), 99);
		assert_eq!(
			DepositStorage::<Test>::get().active.into_inner(),
			vec![(ALICE, MockConsideration(0))]
		);

		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(BOB),
			DEFAULT_IDENTIFIER_KEY,
			None
		));
		assert_eq!(
			DepositStorage::<Test>::get().active.into_inner(),
			vec![(ALICE, MockConsideration(0)), (BOB, MockConsideration(1))]
		);
	});
}

#[test]
fn set_play_deposit_rejects_zero() {
	new_test_ext().execute_with(|| {
		let original = PlayDepositAmount::<Test>::get();
		assert_noop!(
			Game::set_play_deposit(RuntimeOrigin::root(), 0),
			Error::<Test>::InvalidPlayDeposit
		);
		assert_eq!(PlayDepositAmount::<Test>::get(), original);
	});
}

#[test]
fn deposit_required_if_no_invite_and_account() {
	new_test_ext().execute_with(|| {
		use deposit::DepositStorage;

		// Initially, there should be no deposits at all.
		let before = DepositStorage::<Test>::get();
		assert_eq!(before.active.len(), 0);
		assert_eq!(before.burned.len(), 0);
		assert_eq!(before.dropped.len(), 0);

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};

		// Create a game.
		assert_ok!(Game::new_game(&schedule));

		// An Account (ID=1) signs up with NO invite => deposit must be created.
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(ALICE),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));

		let after = DepositStorage::<Test>::get();
		assert_eq!(after.active.len(), 1, "Exactly one deposit must now be active");
		assert_eq!(after.active[0].0, ALICE, "The deposit belongs to account=1");
		assert_eq!(after.burned.len(), 0);
		assert_eq!(after.dropped.len(), 0);
	});
}

#[test]
fn deposit_not_required_for_person() {
	new_test_ext().execute_with(|| {
		use deposit::DepositStorage;

		let before = DepositStorage::<Test>::get();
		assert_eq!(before.active.len(), 0);
		assert_eq!(before.burned.len(), 0);
		assert_eq!(before.dropped.len(), 0);

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};

		// Create a game.
		assert_ok!(Game::new_game(&schedule));

		// A Person signs up => no deposit required.
		let alias_person = [9u8; 32];
		let alias_origin = runtime_origin_for_alias(&alias_person);
		let stmt_account = AccountId32::new(alias_person);
		assert_ok!(Game::sign_up_with_alias(
			alias_origin.clone(),
			DEFAULT_IDENTIFIER_KEY,
			stmt_account.clone(),
			AccountAuthority(stmt_account),
			None,
		));

		let after_person = DepositStorage::<Test>::get();
		assert_eq!(after_person.active.len(), 0, "No deposit created for a Person");
		assert_eq!(after_person.burned.len(), 0);
		assert_eq!(after_person.dropped.len(), 0);
	});
}

#[test]
fn deposit_not_required_for_person_reached_personhood() {
	new_test_ext().execute_with(|| {
		use deposit::DepositStorage;

		let alice = AccountOrPerson::Account(ALICE);

		let mut schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};

		run_game_scenario(schedule.clone(), slice::from_ref(&alice), |_p| {
			Some(vec![Default::default()].try_into().unwrap())
		});

		schedule.game_play_time += 10;
		while !Score::reached_personhood(&alice) {
			let deposits = DepositStorage::<Test>::get();
			assert_eq!(deposits.active.len(), 1);
			assert_eq!(deposits.burned.len(), 0);
			assert_eq!(deposits.dropped.len(), 0);

			run_game_scenario(schedule.clone(), slice::from_ref(&alice), |_p| {
				Some(vec![Default::default()].try_into().unwrap())
			});
			schedule.game_play_time += 10;
		}

		let deposits = DepositStorage::<Test>::get();
		assert_eq!(deposits.active.len(), 0);
		assert_eq!(deposits.burned.len(), 0);
		assert_eq!(deposits.dropped.len(), 1);

		// Create a game.
		assert_ok!(Game::new_game(&schedule));

		// Signs up => no deposit required.
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(ALICE),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));

		let after_person = DepositStorage::<Test>::get();
		assert_eq!(after_person.active.len(), 0);
		assert_eq!(after_person.burned.len(), 0);
		assert_eq!(after_person.dropped.len(), 1);
	});
}

#[test]
fn deposit_slashed_if_not_attending_first_game_otherwise_kept_active() {
	new_test_ext().execute_with(|| {
		use deposit::DepositStorage;

		let alice = AccountOrPerson::Account(ALICE);
		let bob = AccountOrPerson::Account(BOB);

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};

		run_game_scenario(schedule, &[alice.clone(), bob.clone()], |p| {
			if *p == alice {
				Some(vec![vec![Report::NotPerson].try_into().unwrap()].try_into().unwrap())
			} else {
				None
			}
		});

		// The "None" above ensures the player never calls `report`, thus is absent.
		// Because it's an account with no invite, a deposit is created at sign_up,
		// then is BURNED at the end. Let's confirm:
		let final_deposit = DepositStorage::<Test>::get();

		assert_eq!(
			final_deposit.burned.into_inner(),
			vec![MockConsideration(1)],
			"bob deposit is burned"
		);
		assert_eq!(
			final_deposit.active.into_inner(),
			vec![(ALICE, MockConsideration(0))],
			"alice deposit is active"
		);
		assert_eq!(final_deposit.dropped.len(), 0, "No deposits should be dropped");
	});
}

#[test]
fn deposit_is_given_back_when_offboarding() {
	new_test_ext().execute_with(|| {
		use deposit::DepositStorage;

		// Initially, there should be no deposits
		let initial = DepositStorage::<Test>::get();
		assert_eq!(initial.active.len(), 0);
		assert_eq!(initial.burned.len(), 0);
		assert_eq!(initial.dropped.len(), 0);

		let alice = AccountOrPerson::Account(ALICE);

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};

		// Create/run one short game.
		run_game_scenario(schedule, slice::from_ref(&alice), |_p| {
			Some(vec![BoundedVec::default()].try_into().unwrap())
		});

		// Alice is not recognized, deposit is active
		let participant = indiv_pallet_score::Participants::<Test>::get(&alice).unwrap();
		assert_eq!(participant.score, 1);
		assert!(!Score::reached_personhood(&alice));
		let mid = DepositStorage::<Test>::get();
		assert_eq!(mid.active.len(), 1, "Should see 1 active deposit for Alice");
		assert_eq!(mid.burned.len(), 0);
		assert_eq!(mid.dropped.len(), 0);

		// Offboard now that no game is active.
		assert_ok!(Game::offboard(RuntimeOrigin::signed(ALICE)));

		// Confirm the deposit is returned — i.e. now found in `dropped`.
		let final_ds = DepositStorage::<Test>::get();
		assert_eq!(final_ds.active.len(), 0);
		assert_eq!(final_ds.burned.len(), 0);
		assert_eq!(final_ds.dropped.len(), 1);
	});
}

// Test the flow for the deposit for a non-invited player
// - signs up and pay deposit
// - attends
// - attends
// - is absent
// - is absent -> score 0 -> deposit slashed
#[test]
fn player_without_invite_deposit_flow() {
	new_test_ext().execute_with(|| {
		use deposit::DepositStorage;

		let alice = AccountOrPerson::Account(ALICE);

		// ─────────────────────────────────────────────────────────────────────────────
		//  Game #1: The participant attends => deposit remains active => score is 1
		// ─────────────────────────────────────────────────────────────────────────────
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		run_game_scenario(schedule, slice::from_ref(&alice), |_p| {
			// Return Some(empty reports) => "attend" for round #0
			let partial_report: BoundedVec<Report, _> = BoundedVec::default();
			let full_report: FullReport<Test> = vec![partial_report].try_into().unwrap();
			Some(full_report)
		});

		assert!(indiv_pallet_score::Participants::<Test>::get(&alice).unwrap().score == 1);
		let deposits1 = DepositStorage::<Test>::get();
		assert_eq!(deposits1.active.len(), 1);
		assert_eq!(deposits1.burned.len(), 0);
		assert_eq!(deposits1.dropped.len(), 0);

		// ─────────────────────────────────────────────────────────────────────────────
		//  Game #2: The participant attends => deposit remains active => score is 3
		// ─────────────────────────────────────────────────────────────────────────────
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 20,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		run_game_scenario(schedule, slice::from_ref(&alice), |_p| {
			// Return Some(empty reports) => "attend" for round #0
			let partial_report: BoundedVec<Report, _> = BoundedVec::default();
			let full_report: FullReport<Test> = vec![partial_report].try_into().unwrap();
			Some(full_report)
		});

		assert_eq!(indiv_pallet_score::Participants::<Test>::get(&alice).unwrap().score, 3);
		let deposits1 = DepositStorage::<Test>::get();
		assert_eq!(deposits1.active.len(), 1);
		assert_eq!(deposits1.burned.len(), 0);
		assert_eq!(deposits1.dropped.len(), 0);

		// ─────────────────────────────────────────────────────────────────────────────
		//  Game #3: The participant is absent => deposit remains active => score is 2
		// ─────────────────────────────────────────────────────────────────────────────
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 60,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		run_game_scenario(schedule, slice::from_ref(&alice), |_p| {
			// Return None => means "no report" => absent
			None
		});

		assert_eq!(indiv_pallet_score::Participants::<Test>::get(&alice).unwrap().score, 2);
		let deposits1 = DepositStorage::<Test>::get();
		assert_eq!(deposits1.active.len(), 1);
		assert_eq!(deposits1.burned.len(), 0);
		assert_eq!(deposits1.dropped.len(), 0);

		// ─────────────────────────────────────────────────────────────────────────────
		//  Game #4: The participant is absent => score is 0 => deposit slashed
		// ─────────────────────────────────────────────────────────────────────────────
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 90,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		run_game_scenario(schedule, slice::from_ref(&alice), |_p| {
			// Return None => means "no report" => absent
			None
		});

		assert_eq!(indiv_pallet_score::Participants::<Test>::get(&alice).unwrap().score, 0);
		let deposits2 = DepositStorage::<Test>::get();
		assert_eq!(deposits2.active.len(), 0);
		assert_eq!(deposits2.burned.len(), 1);
		assert_eq!(deposits2.dropped.len(), 0);
	});
}

// tests:
// * invites doesn't require deposit in the first participated game
// * even in another later game
// * but if go back to zero, and play again then requires deposit or invite
#[test]
fn invited_player_deposit_scenarios() {
	new_test_ext().execute_with(|| {
		use deposit::DepositStorage;

		const INVITER: AccountId32 = ALICE;
		const INVITED: AccountId32 = AccountId32::new(*b"101_____________________________");
		let ticket = 456_u64;
		let signature = TestSignature(ticket, INVITED.encode());
		let ticket_2 = 457_u64;
		let signature_2 = TestSignature(ticket_2, INVITED.encode());

		assert_eq!(DepositStorage::<Test>::get().active.len(), 0);
		assert_eq!(DepositStorage::<Test>::get().burned.len(), 0);
		assert_eq!(DepositStorage::<Test>::get().dropped.len(), 0);

		// Helper: run the game flow from new_game() -> sign_up() -> shuffle -> report -> process
		fn run_game_flow(
			game_date: u32,
			use_invite: Option<(AccountId32, u64, TestSignature)>,
			do_report: bool,
		) {
			use std::time::Duration;

			let schedule = GameSchedule::<u32, u128> {
				game_play_time: game_date,
				rounds: 1,
				max_group_size: 2,
				..Default::default()
			};

			let registration_ends = GameTimes::<Test>::registration_end(&schedule);
			let game_play_time = GameTimes::<Test>::game_play_time(&schedule);
			let report_ends = GameTimes::<Test>::reporting_end(&schedule);

			// 1) Create game
			assert_ok!(Game::new_game(&schedule));

			// 2) sign_up
			if let Some((INVITER, ticket, signature)) = use_invite {
				let nonce = frame_system::Account::<Test>::get(&INVITED).nonce;
				assert_ok!(exec_invited_tx(
					INVITED,
					crate::GameAsInvitedData { nonce, inviter: INVITER, ticket, signature },
					Call::sign_up_with_invite {
						identifier_key: DEFAULT_IDENTIFIER_KEY,
						airdrop: None
					},
				));
			} else {
				assert_ok!(exec_signed_tx(
					INVITED,
					Call::sign_up_with_account {
						identifier_key: DEFAULT_IDENTIFIER_KEY,
						airdrop: None
					},
				));
			}

			// 3) Fast-forward time beyond registration_ends => triggers shuffle
			MOCK_UNIX_TIME
				.with(|m| *m.borrow_mut() = Duration::from_secs((registration_ends + 1) as u64));
			advance_process(); // register to shuffle
			advance_process(); // shuffle to report

			// 4) Move to the game day, optionally do report if do_report == true
			MOCK_UNIX_TIME.with(|m| *m.borrow_mut() = Duration::from_secs(game_play_time as u64));
			if do_report {
				let full_report: FullReport<Test> = vec![Default::default()].try_into().unwrap();
				assert_ok!(Game::report(RuntimeOrigin::signed(INVITED), full_report));
			}

			// 5) Move time beyond report_ends => transition => process
			MOCK_UNIX_TIME
				.with(|m| *m.borrow_mut() = Duration::from_secs((report_ends + 1) as u64));
			advance_process(); // report to player process
			advance_process(); // step1 -> step2
			advance_process(); // step2 -> step3
			advance_process(); // step3 -> done
		}

		// ------------------------------------------------------
		// 1) GIVE out invites from root -> (inviter)
		// ------------------------------------------------------
		assert_ok!(Game::grant_invites(RuntimeOrigin::root(), INVITER, 2));

		// `inviter` sets a ticket for `invited_account`
		assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(INVITER), ticket));
		assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(INVITER), ticket_2));

		// ------------------------------------------------------
		// Game #1: The invited user signs up with an invite => no deposit => attends
		// ------------------------------------------------------
		run_game_flow(
			/* game_date= */ 10,
			Some((INVITER, ticket, signature.clone())),
			/* do_report= */ true,
		);
		assert_eq!(DepositStorage::<Test>::get().active.len(), 0);
		assert_eq!(DepositStorage::<Test>::get().burned.len(), 0);
		assert_eq!(DepositStorage::<Test>::get().dropped.len(), 0);

		// ------------------------------------------------------
		// Game #2: No invite passed, but the user is still invited => no deposit
		// ------------------------------------------------------
		run_game_flow(
			/* game_date= */ 20, /* use_invite= */ None, /* do_report= */ true,
		);
		assert_eq!(DepositStorage::<Test>::get().active.len(), 0);
		assert_eq!(DepositStorage::<Test>::get().burned.len(), 0);
		assert_eq!(DepositStorage::<Test>::get().dropped.len(), 0);

		// ------------------------------------------------------
		// Game #3: The user signs up (still no deposit), does NOT report => absent
		// ------------------------------------------------------
		run_game_flow(
			/* game_date= */ 30, /* use_invite= */ None, /* do_report= */ false,
		);
		assert_eq!(DepositStorage::<Test>::get().active.len(), 0);
		assert_eq!(DepositStorage::<Test>::get().burned.len(), 0);
		assert_eq!(DepositStorage::<Test>::get().dropped.len(), 0);

		// ------------------------------------------------------
		// Game #4: The user signs up (still no deposit), does NOT report => absent
		// => score 0 => archived
		// ------------------------------------------------------
		run_game_flow(
			/* game_date= */ 40, /* use_invite= */ None, /* do_report= */ false,
		);
		assert!(ArchivedPlayers::<Test>::get(AccountOrPerson::Account(INVITED)).is_some());
		assert_eq!(DepositStorage::<Test>::get().active.len(), 0);
		assert_eq!(DepositStorage::<Test>::get().burned.len(), 0);
		assert_eq!(DepositStorage::<Test>::get().dropped.len(), 0);

		// ------------------------------------------------------
		// Game #5: The user signs up with a new invite (still no deposit), no report => archived
		// ------------------------------------------------------
		run_game_flow(
			/* game_date= */ 50,
			/* use_invite= */ Some((INVITER, ticket_2, signature_2.clone())),
			/* do_report= */ false,
		);
		assert_eq!(DepositStorage::<Test>::get().active.len(), 0);
		assert_eq!(DepositStorage::<Test>::get().burned.len(), 0);
		assert_eq!(DepositStorage::<Test>::get().dropped.len(), 0);

		// ------------------------------------------------------
		// Game #6: The user signs up, now it requires a deposit.
		// ------------------------------------------------------
		run_game_flow(
			/* game_date= */ 60, /* use_invite= */ None, /* do_report= */ true,
		);
		assert_eq!(DepositStorage::<Test>::get().active.len(), 1);
		assert_eq!(DepositStorage::<Test>::get().burned.len(), 0);
		assert_eq!(DepositStorage::<Test>::get().dropped.len(), 0);
	});
}

#[test]
fn invites_flow_scenario() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		use crate::AvailableInvites;

		let root_origin = RuntimeOrigin::root();
		const INVITER: AccountId32 = ALICE;
		let ticket1 = 101u64;
		let ticket2 = 202u64;
		let ticket3 = 303u64;
		let ticket_not_exists = 999u64;

		// Initially, no invites are set:
		assert_eq!(AvailableInvites::<Test>::get(INVITER), 0);

		// ------------------------------------------------------------------
		// (1) Root calls `grant_invites(INVITER, 2)`.
		// ------------------------------------------------------------------
		assert_ok!(Game::grant_invites(root_origin.clone(), INVITER, 2));
		// The InvitesGranted event is emitted.
		System::assert_has_event(
			Event::<Test>::InvitesGranted { account: INVITER, count: 2 }.into(),
		);
		// Check storage
		assert_eq!(AvailableInvites::<Test>::get(INVITER), 2);
		assert_eq!(PendingInvites::<Test>::iter_keys().count(), 0);

		// ------------------------------------------------------------------
		// (2) Inviter calls `set_invite_ticket(ticket1)` => should succeed.
		// ------------------------------------------------------------------
		assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(INVITER), ticket1));
		// The InviteTicketSet event is emitted.
		System::assert_has_event(Event::<Test>::InviteTicketSet { inviter: INVITER }.into());
		assert_eq!(AvailableInvites::<Test>::get(INVITER), 1);
		assert!(PendingInvites::<Test>::contains_key(INVITER, ticket1));

		// ------------------------------------------------------------------
		// (3) Inviter calls `set_invite_ticket(ticket2)` => should succeed.
		// ------------------------------------------------------------------
		assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(INVITER), ticket2));
		// The InviteTicketSet event is emitted.
		System::assert_has_event(Event::<Test>::InviteTicketSet { inviter: INVITER }.into());
		assert_eq!(AvailableInvites::<Test>::get(INVITER), 0);
		assert!(PendingInvites::<Test>::contains_key(INVITER, ticket1));
		assert!(PendingInvites::<Test>::contains_key(INVITER, ticket2));

		// ------------------------------------------------------------------
		// (4) Trying `set_invite_ticket(ticket3)` again => fails with `NoInvites`
		//     because we have no more available invites.
		// ------------------------------------------------------------------
		assert_noop!(
			Game::set_invite_ticket(RuntimeOrigin::signed(INVITER), ticket3),
			Error::<Test>::NoInvites
		);
		// Confirm storage hasn't changed
		assert_eq!(AvailableInvites::<Test>::get(INVITER), 0);
		assert!(PendingInvites::<Test>::contains_key(INVITER, ticket1));
		assert!(PendingInvites::<Test>::contains_key(INVITER, ticket2));

		// ------------------------------------------------------------------
		// (5) Cancel a ticket that does *not* exist => fails with `NoTicket`.
		// ------------------------------------------------------------------
		assert_noop!(
			Game::cancel_invite_ticket(RuntimeOrigin::signed(INVITER), ticket_not_exists),
			Error::<Test>::NoTicket
		);

		// ------------------------------------------------------------------
		// (6) Inviter calls `cancel_invite_ticket(ticket1)` => success.
		//     This should remove `ticket1` from `pending` and increment `available`.
		// ------------------------------------------------------------------
		assert_ok!(Game::cancel_invite_ticket(RuntimeOrigin::signed(INVITER), ticket1));
		// The InviteTicketCancelled event is emitted.
		System::assert_has_event(Event::<Test>::InviteTicketCancelled { inviter: INVITER }.into());
		assert_eq!(AvailableInvites::<Test>::get(INVITER), 1);
		assert!(!PendingInvites::<Test>::contains_key(INVITER, ticket1));
		assert!(PendingInvites::<Test>::contains_key(INVITER, ticket2));

		// (Optional) We could re-invite using `ticket1` again, but let's skip.

		// ------------------------------------------------------------------
		// (7) Root calls `remove_available_and_pending_invites(INVITER)`.
		//     This zeroes out everything and removes the key from inviters storage.
		// ------------------------------------------------------------------
		assert_ok!(Game::remove_available_and_pending_invites(root_origin.clone(), INVITER, 100));
		assert_eq!(AvailableInvites::<Test>::get(INVITER), 0);
		assert!(!PendingInvites::<Test>::contains_key(INVITER, ticket1));
		assert!(!PendingInvites::<Test>::contains_key(INVITER, ticket2));
	});
}

#[test]
fn set_invite_ticket_fail_when_no_inviter_record() {
	new_test_ext().execute_with(|| {
		// Caller tries to set an invite ticket but has no invites at all => `NoInvites`.
		assert_noop!(
			Game::set_invite_ticket(RuntimeOrigin::signed(ALICE), 123),
			Error::<Test>::NoInvites
		);
	});
}

#[test]
fn test_invited_player_never_pay_fees() {
	new_test_ext().execute_with(|| {
		// Suppose we have an "inviter" with invites
		const INVITER: AccountId32 = ALICE;
		let ticket = 12345u64;
		let signature = TestSignature(ticket, NOT_FUNDED_ACCOUNT.encode());

		// Give 1 invite
		assert_ok!(Game::grant_invites(RuntimeOrigin::root(), INVITER, 1));
		assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(INVITER), ticket));

		// Run a first game
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		run_game_scenario_with_phase(
			schedule,
			/* sign up */
			|| {
				assert_ok!(exec_invited_tx(
					NOT_FUNDED_ACCOUNT,
					GameAsInvitedData {
						nonce: 0,
						inviter: INVITER,
						ticket,
						signature: signature.clone()
					},
					Call::sign_up_with_invite {
						identifier_key: DEFAULT_IDENTIFIER_KEY,
						airdrop: None
					}
				));
			},
			/* report_ */
			|| {
				let empty_full_report = vec![vec![].try_into().unwrap()].try_into().unwrap();
				assert_ok!(exec_participant_tx(
					NOT_FUNDED_ACCOUNT,
					1, // participant nonce
					Call::report { full_report: empty_full_report }
				));
			},
		);

		// Run a second game
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 40,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		run_game_scenario_with_phase(
			schedule,
			/* sign up */
			|| {
				assert_ok!(exec_participant_tx(
					NOT_FUNDED_ACCOUNT,
					2, // participant nonce
					Call::sign_up_with_account {
						identifier_key: DEFAULT_IDENTIFIER_KEY,
						airdrop: None
					}
				));
			},
			/* report_ */
			|| {
				let empty_full_report = vec![vec![].try_into().unwrap()].try_into().unwrap();
				assert_ok!(exec_participant_tx(
					NOT_FUNDED_ACCOUNT,
					3, // participant nonce
					Call::report { full_report: empty_full_report }
				));
			},
		);

		// At the end, we might offboard that user, if we want:
		assert_ok!(exec_participant_tx(NOT_FUNDED_ACCOUNT, 4, Call::offboard {}));
	});
}

#[test]
fn test_non_invited_player_scenario_dont_pay_fees() {
	new_test_ext().execute_with(|| {
		frame_system::Pallet::<Test>::inc_sufficients(&NOT_FUNDED_ACCOUNT);

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		run_game_scenario_with_phase(
			schedule,
			|| {
				// We don't use a transaction extension, because we want to be able to pay.
				assert_ok!(Game::sign_up_with_account(
					RuntimeOrigin::signed(NOT_FUNDED_ACCOUNT),
					DEFAULT_IDENTIFIER_KEY,
					None,
				));
			},
			|| {
				let empty_full_report = vec![vec![].try_into().unwrap()].try_into().unwrap();
				assert_ok!(exec_participant_tx(
					NOT_FUNDED_ACCOUNT,
					0, // nonce
					Call::report { full_report: empty_full_report }
				));
			},
		);

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 40,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		run_game_scenario_with_phase(
			schedule,
			|| {
				assert_ok!(exec_participant_tx(
					NOT_FUNDED_ACCOUNT,
					1, // nonce
					Call::sign_up_with_account {
						identifier_key: DEFAULT_IDENTIFIER_KEY,
						airdrop: None
					}
				));
			},
			|| {
				let empty_full_report = vec![vec![].try_into().unwrap()].try_into().unwrap();
				assert_ok!(exec_participant_tx(
					NOT_FUNDED_ACCOUNT,
					2, // nonce
					Call::report { full_report: empty_full_report }
				));
			},
		);

		// Offboard after the scenario
		assert_ok!(exec_participant_tx(
			NOT_FUNDED_ACCOUNT,
			3, // nonce
			Call::offboard {}
		));
	});
}

// Test that the sufficient reference is correctly removed when offboarded
#[test]
fn sufficient_scenario_1_offboarded() {
	new_test_ext().execute_with(|| {
		// Suppose we have an "inviter" with invites
		const INVITER: AccountId32 = ALICE;
		let ticket = 12345u64;
		let signature = TestSignature(ticket, NOT_FUNDED_ACCOUNT.encode());

		// Give 1 invite
		assert_ok!(Game::grant_invites(RuntimeOrigin::root(), INVITER, 1));
		assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(INVITER), ticket));

		// Run a first game
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		run_game_scenario_with_phase(
			schedule,
			/* sign up */
			|| {
				assert_ok!(exec_invited_tx(
					NOT_FUNDED_ACCOUNT,
					GameAsInvitedData {
						nonce: 0,
						inviter: INVITER,
						ticket,
						signature: signature.clone()
					},
					Call::sign_up_with_invite {
						identifier_key: DEFAULT_IDENTIFIER_KEY,
						airdrop: None
					}
				));
			},
			/* report_ */
			|| {
				let empty_full_report = vec![vec![].try_into().unwrap()].try_into().unwrap();
				assert_ok!(exec_participant_tx(
					NOT_FUNDED_ACCOUNT,
					1, // participant nonce
					Call::report { full_report: empty_full_report }
				));
			},
		);

		// Account is sufficient
		assert_eq!(frame_system::Account::<Test>::get(NOT_FUNDED_ACCOUNT).sufficients, 1,);

		// At the end, we might offboard that user, if we want:
		assert_ok!(exec_participant_tx(NOT_FUNDED_ACCOUNT, 2, Call::offboard {}));

		// Account is no longer sufficient
		assert_eq!(frame_system::Account::<Test>::get(NOT_FUNDED_ACCOUNT).sufficients, 0,);
	});
}

// Test that the sufficient reference is correctly removed after kickout
#[test]
fn sufficient_scenario_2_archived() {
	new_test_ext().execute_with(|| {
		// Suppose we have an "inviter" with invites
		const INVITER: AccountId32 = ALICE;
		let ticket = 12345u64;
		let signature = TestSignature(ticket, NOT_FUNDED_ACCOUNT.encode());

		// Give 1 invite
		assert_ok!(Game::grant_invites(RuntimeOrigin::root(), INVITER, 1));
		assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(INVITER), ticket));

		// Run a first game
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		run_game_scenario_with_phase(
			schedule,
			/* sign up */
			|| {
				assert_ok!(exec_invited_tx(
					NOT_FUNDED_ACCOUNT,
					GameAsInvitedData {
						nonce: 0,
						inviter: INVITER,
						ticket,
						signature: signature.clone()
					},
					Call::sign_up_with_invite {
						identifier_key: DEFAULT_IDENTIFIER_KEY,
						airdrop: None
					}
				));

				// Account is sufficient
				assert_eq!(frame_system::Account::<Test>::get(NOT_FUNDED_ACCOUNT).sufficients, 1);
			},
			/* report_ */
			|| {
				// Account is sufficient
				assert_eq!(frame_system::Account::<Test>::get(NOT_FUNDED_ACCOUNT).sufficients, 1);

				// No report
			},
		);

		// Account is no longer sufficient
		assert_eq!(frame_system::Account::<Test>::get(NOT_FUNDED_ACCOUNT).sufficients, 1);

		for _ in 0u32..<Test as crate::Config>::NonPlayingKickoutTime::get() {
			advance_process()
		}

		assert_ok!(Game::kickout(RuntimeOrigin::signed(ALICE), NOT_FUNDED_ACCOUNT));

		// Account is no longer sufficient
		assert_eq!(frame_system::Account::<Test>::get(NOT_FUNDED_ACCOUNT).sufficients, 0);
	});
}

#[test]
fn use_invite_but_already_playing() {
	new_test_ext().execute_with(|| {
		// Alice play a game.
		let alice = AccountOrPerson::Account(ALICE);
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		run_game_scenario(schedule, &[alice], |_player| {
			Some(vec![BoundedVec::new()].try_into().unwrap())
		});

		// Then Alice tries to sign up again with an invite.
		const INVITER: AccountId32 = ALICE;
		let ticket = 456_u64;
		let signature = TestSignature(ticket, ALICE.encode());
		PendingInvites::<Test>::insert(INVITER, ticket, ());
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 25,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));
		assert_noop!(
			exec_invited_tx(
				ALICE,
				crate::GameAsInvitedData { nonce: 0, inviter: INVITER, ticket, signature },
				Call::sign_up_with_invite { identifier_key: DEFAULT_IDENTIFIER_KEY, airdrop: None },
			),
			InvalidTransaction::Custom(142),
		);
	});
}

// Sign-up must fail when there is conflict betwen existing statement accounts or player accounts
#[test]
fn sign_up_fails_if_statement_account_already_used() {
	// Case 1: An account tries to sign-up, but that AccountId is already used as the
	// *statement account* of a person.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));

		// a person registers first, binding `BOB` as its statement account
		let alias_1 = [1u8; 32];
		assert_ok!(Game::sign_up_with_alias(
			runtime_origin_for_alias(&alias_1),
			DEFAULT_IDENTIFIER_KEY,
			BOB, // statement account
			AccountAuthority(BOB),
			None,
		));

		// Now the same AccountId (`BOB`) tries to sign-up as an account player -> must fail
		assert_noop!(
			Game::sign_up_with_account(RuntimeOrigin::signed(BOB), DEFAULT_IDENTIFIER_KEY, None),
			Error::<Test>::StatementAccountAlreadyInUse
		);
	});

	// Case 2: A second person tries to reuse a statement account that another person has already
	// bound.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));

		// First person binds `BOB`
		let alias_1 = [1u8; 32];
		assert_ok!(Game::sign_up_with_alias(
			runtime_origin_for_alias(&alias_1),
			DEFAULT_IDENTIFIER_KEY,
			BOB,
			AccountAuthority(BOB),
			None,
		));

		// Second person, different alias, tries the *same* statement account -> must fail
		let alias_2 = [2u8; 32];
		assert_noop!(
			Game::sign_up_with_alias(
				runtime_origin_for_alias(&alias_2),
				DEFAULT_IDENTIFIER_KEY,
				BOB,
				AccountAuthority(BOB),
				None,
			),
			Error::<Test>::StatementAccountAlreadyInUse
		);
	});

	// Case 3: A person tries to sign-up with a statement account that is already
	// an ordinary account-player.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));

		// Account-based player (`BOB`) signs up first
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(BOB),
			DEFAULT_IDENTIFIER_KEY,
			None
		));

		// Person now tries to use `BOB` as their statement account -> must fail
		let alias_1 = [3u8; 32];
		assert_noop!(
			Game::sign_up_with_alias(
				runtime_origin_for_alias(&alias_1),
				DEFAULT_IDENTIFIER_KEY,
				BOB,
				AccountAuthority(BOB),
				None,
			),
			Error::<Test>::StatementAccountAlreadyInUse
		);
	});
}

// `GameIndex` must start at 0 and grow by 1.
#[test]
fn game_index_is_incremented_for_each_game() {
	new_test_ext().execute_with(|| {
		assert_eq!(GameIndex::<Test>::get(), 0);

		run_game_scenario(
			GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 1,
				max_group_size: 2,
				..Default::default()
			},
			&[AccountOrPerson::Account(ALICE)],
			|_player| {
				let empty_full_report: FullReport<Test> =
					vec![BoundedVec::default()].try_into().unwrap();
				Some(empty_full_report)
			},
		);

		assert_eq!(GameIndex::<Test>::get(), 1);

		run_game_scenario(
			GameSchedule::<u32, u128> {
				game_play_time: 20,
				rounds: 1,
				max_group_size: 2,
				..Default::default()
			},
			&[AccountOrPerson::Account(ALICE)],
			|_player| {
				let empty_full_report: FullReport<Test> =
					vec![BoundedVec::default()].try_into().unwrap();
				Some(empty_full_report)
			},
		);

		assert_eq!(GameIndex::<Test>::get(), 2);
	});
}

// statements for a statement account are cleared by offchain worker
// * when an alias switches to a new statement account.
// * when an alias player gets archived.
// * when an account-based player gets archived.
#[test]
fn statements_cleared() {
	// when an alias switches to a new statement account.
	new_test_ext().execute_with(|| {
		MOCK_STATEMENT_STORE.clear();
		advance_process();

		let alias: [u8; 32] = [7; 32];
		let old_stmt = ed25519::Pair::generate_with_phrase(None).0;
		let new_stmt = ed25519::Pair::generate_with_phrase(None).0;

		// Game 1: bind alias -> old_stmt and complete the game
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 20,
			rounds: 1,
			max_group_size: 1,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));
		assert_ok!(Game::sign_up_with_alias(
			runtime_origin_for_alias(&alias),
			DEFAULT_IDENTIFIER_KEY,
			old_stmt.public().into(),
			AccountAuthority(old_stmt.public().into()),
			None,
		));

		// Add statements signed by old_stmt and some by others
		let keep_stmt = {
			let mut s = Statement::new();
			s.sign_ed25519_private(&new_stmt);
			s
		};
		let rm_stmt_1 = {
			let mut s = Statement::new();
			s.sign_ed25519_private(&old_stmt);
			s
		};
		let rm_stmt_2 = {
			let mut s = Statement::new();
			s.sign_ed25519_private(&old_stmt);
			s
		};
		MOCK_STATEMENT_STORE.add_stmt(keep_stmt.clone());
		MOCK_STATEMENT_STORE.add_stmt(rm_stmt_1.clone());
		MOCK_STATEMENT_STORE.add_stmt(rm_stmt_2.clone());

		// Drive Game 1 to completion
		let registration_ends = GameTimes::<Test>::registration_end(&schedule);
		crate::mock::MOCK_UNIX_TIME
			.with(|t| *t.borrow_mut() = Duration::from_secs(registration_ends as u64 + 1));
		advance_process_with_on_poll_only(); // registration -> shuffle
		let game_play_time = GameTimes::<Test>::game_play_time(&schedule);
		crate::mock::MOCK_UNIX_TIME
			.with(|t| *t.borrow_mut() = Duration::from_secs(game_play_time as u64 + 1));
		advance_process(); // shuffle -> reporting
		let report_ends = GameTimes::<Test>::reporting_end(&schedule);
		crate::mock::MOCK_UNIX_TIME
			.with(|t| *t.borrow_mut() = Duration::from_secs(report_ends as u64 + 1));
		advance_process(); // reporting -> player process
		advance_process(); // step1 -> step2
		advance_process(); // step2 -> step3
		advance_process(); // step3 -> done
		advance_process(); // done -> next game

		// Game 2: rebind alias to new_stmt (emits StmtUsageRemoved(old_stmt))
		let schedule2 = GameSchedule::<u32, u128> {
			game_play_time: 40,
			rounds: 1,
			max_group_size: 1,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule2));
		assert_ok!(Game::sign_up_with_alias(
			runtime_origin_for_alias(&alias),
			DEFAULT_IDENTIFIER_KEY,
			new_stmt.public().into(),
			AccountAuthority(new_stmt.public().into()),
			None,
		));

		// Offchain worker should call remove_by(old_stmt)
		AllPalletsWithSystem::offchain_worker(System::block_number());

		let left = MOCK_STATEMENT_STORE
			.0
			.lock()
			.unwrap()
			.iter()
			.map(|(_, s)| s.clone())
			.collect::<Vec<_>>();
		assert_eq!(left, vec![keep_stmt]);
	});

	// when an alias player gets archived.
	new_test_ext().execute_with(|| {
		MOCK_STATEMENT_STORE.clear();
		MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs(1));
		// Ensure we start on a live block
		advance_process();

		// Alias and its initially bound statement account
		let alias: [u8; 32] = [11; 32];
		let stmt_pair = ed25519::Pair::generate_with_phrase(None).0; // will be removed
		let outsider_pair = ed25519::Pair::generate_with_phrase(None).0; // will be kept

		// Create a short game (1 round, single-member groups to keep it simple)
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 20,
			rounds: 1,
			max_group_size: 1,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));

		// Bind alias -> stmt_pair (so any statements signed by stmt_pair belong to this alias)
		assert_ok!(Game::sign_up_with_alias(
			runtime_origin_for_alias(&alias),
			DEFAULT_IDENTIFIER_KEY,
			stmt_pair.public().into(),
			AccountAuthority(stmt_pair.public().into()),
			None,
		));

		// Add three statements into the mock store:
		// - two signed by `stmt_pair` (must be removed)
		// - one signed by an outsider (must remain)
		let keep_stmt = {
			let mut s = Statement::new();
			s.sign_ed25519_private(&outsider_pair);
			s
		};
		let rm_stmt_1 = {
			let mut s = Statement::new();
			s.sign_ed25519_private(&stmt_pair);
			s
		};
		let rm_stmt_2 = {
			let mut s = Statement::new();
			s.sign_ed25519_private(&stmt_pair);
			s
		};
		MOCK_STATEMENT_STORE.add_stmt(keep_stmt.clone());
		MOCK_STATEMENT_STORE.add_stmt(rm_stmt_1.clone());
		MOCK_STATEMENT_STORE.add_stmt(rm_stmt_2.clone());

		// Fast-forward to after registration -> Reporting
		let registration_ends = GameTimes::<Test>::registration_end(&schedule);
		MOCK_UNIX_TIME
			.with(|t| *t.borrow_mut() = Duration::from_secs(registration_ends as u64 + 1));
		advance_process_with_on_poll_only(); // registration -> shuffle
		advance_process(); // shuffle -> reporting

		// Do NOT report for the alias (absent) -> they are externally recognised, so archiving
		// triggers Move time past report_ends and run processing.
		let report_ends = GameTimes::<Test>::reporting_end(&schedule);
		MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs(report_ends as u64 + 1));

		advance_process(); // reporting -> player process
		advance_process(); // player process (archives alias & emits StmtUsageRemoved)

		// Run the offchain worker in the SAME block that just processed players.
		AllPalletsWithSystem::offchain_worker(System::block_number());

		// Only the outsider-signed statement must remain.
		let left = MOCK_STATEMENT_STORE
			.0
			.lock()
			.unwrap()
			.iter()
			.map(|(_, s)| s.clone())
			.collect::<Vec<_>>();
		assert_eq!(left, vec![keep_stmt]);
	});

	// when an account-based player gets archived.
	new_test_ext().execute_with(|| {
		MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs(1));
		MOCK_STATEMENT_STORE.clear();
		// Ensure we start on a live block
		advance_process();

		// Pick an ed25519 key and use its PUBLIC as the AccountId32 so that
		// the `StmtUsageRemoved { who }` matches the ed25519 signer bytes.
		let acc_pair = ed25519::Pair::generate_with_phrase(None).0; // will be removed
		let account: AccountId32 = acc_pair.public().into();
		let outsider_pair = ed25519::Pair::generate_with_phrase(None).0; // will be kept

		// Create a short game (1 round, single-member groups to keep it simple)
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 20,
			rounds: 1,
			max_group_size: 1,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));

		// Sign up as a normal account player (no invite)
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(account.clone()),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));

		// Insert statements: two signed by the account's ed25519 key (to be removed),
		// one by an outsider (to be kept).
		let keep_stmt = {
			let mut s = Statement::new();
			s.sign_ed25519_private(&outsider_pair);
			s
		};
		let rm_stmt_1 = {
			let mut s = Statement::new();
			s.sign_ed25519_private(&acc_pair);
			s
		};
		let rm_stmt_2 = {
			let mut s = Statement::new();
			s.sign_ed25519_private(&acc_pair);
			s
		};
		MOCK_STATEMENT_STORE.add_stmt(keep_stmt.clone());
		MOCK_STATEMENT_STORE.add_stmt(rm_stmt_1.clone());
		MOCK_STATEMENT_STORE.add_stmt(rm_stmt_2.clone());

		// Fast-forward to Reporting
		let registration_ends = GameTimes::<Test>::registration_end(&schedule);
		MOCK_UNIX_TIME
			.with(|t| *t.borrow_mut() = Duration::from_secs(registration_ends as u64 + 1));
		advance_process_with_on_poll_only(); // registration -> shuffle
		advance_process(); // shuffle -> reporting

		// Do NOT report for the account (absent) so it gets archived (score stays 0).
		let report_ends = GameTimes::<Test>::reporting_end(&schedule);
		MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs(report_ends as u64 + 1));

		advance_process(); // reporting -> player process
		advance_process(); // player process (archives account & emits StmtUsageRemoved)

		// Run the offchain worker in the SAME block that processed players.
		AllPalletsWithSystem::offchain_worker(System::block_number());

		// Only the outsider-signed statement must remain.
		let left = MOCK_STATEMENT_STORE
			.0
			.lock()
			.unwrap()
			.iter()
			.map(|(_, s)| s.clone())
			.collect::<Vec<_>>();
		assert_eq!(left, vec![keep_stmt]);
	});
}

// Verifies that GroupsSetting::acceptable_player_count()
// returns the expected value for a variety of edge-cases.
#[test]
fn acceptable_player_count_cases() {
	use crate::types::GroupsSetting;

	assert!(GroupsSetting { max_per_group: 0, player_count: 0 }.acceptable_player_count::<Test>());
	assert!(GroupsSetting { max_per_group: 0, player_count: 1 }.acceptable_player_count::<Test>());
	assert!(GroupsSetting { max_per_group: 0, player_count: 10 }.acceptable_player_count::<Test>());

	assert!(GroupsSetting { max_per_group: 1, player_count: 0 }.acceptable_player_count::<Test>());
	assert!(GroupsSetting { max_per_group: 1, player_count: 1 }.acceptable_player_count::<Test>());
	assert!(GroupsSetting { max_per_group: 1, player_count: 10 }.acceptable_player_count::<Test>());

	assert!(!GroupsSetting { max_per_group: 2, player_count: 0 }.acceptable_player_count::<Test>());
	assert!(GroupsSetting { max_per_group: 2, player_count: 1 }.acceptable_player_count::<Test>());
	assert!(GroupsSetting { max_per_group: 2, player_count: 10 }.acceptable_player_count::<Test>());

	assert!(!GroupsSetting { max_per_group: 3, player_count: 1 }.acceptable_player_count::<Test>());
	assert!(GroupsSetting { max_per_group: 3, player_count: 2 }.acceptable_player_count::<Test>());
	assert!(GroupsSetting { max_per_group: 3, player_count: 3 }.acceptable_player_count::<Test>());
	assert!(GroupsSetting { max_per_group: 3, player_count: 4 }.acceptable_player_count::<Test>());
	assert!(GroupsSetting { max_per_group: 3, player_count: 4 }.acceptable_player_count::<Test>());

	assert!(!GroupsSetting { max_per_group: 4, player_count: 2 }.acceptable_player_count::<Test>());
	assert!(GroupsSetting { max_per_group: 4, player_count: 3 }.acceptable_player_count::<Test>());
	assert!(GroupsSetting { max_per_group: 4, player_count: 4 }.acceptable_player_count::<Test>());
	assert!(!GroupsSetting { max_per_group: 4, player_count: 5 }.acceptable_player_count::<Test>());
	assert!(GroupsSetting { max_per_group: 4, player_count: 6 }.acceptable_player_count::<Test>());
	assert!(GroupsSetting { max_per_group: 4, player_count: 7 }.acceptable_player_count::<Test>());

	assert!(!GroupsSetting { max_per_group: 5, player_count: 2 }.acceptable_player_count::<Test>());
	assert!(!GroupsSetting { max_per_group: 5, player_count: 3 }.acceptable_player_count::<Test>());
	assert!(GroupsSetting { max_per_group: 5, player_count: 4 }.acceptable_player_count::<Test>());
	assert!(GroupsSetting { max_per_group: 5, player_count: 5 }.acceptable_player_count::<Test>());
	for i in 6..12 {
		assert!(
			!GroupsSetting { max_per_group: 5, player_count: i }.acceptable_player_count::<Test>()
		);
	}
	assert!(GroupsSetting { max_per_group: 5, player_count: 12 }.acceptable_player_count::<Test>());
}

mod game_cancellation {
	use super::*;

	#[test]
	fn game_cancelled_due_to_player_count_too_low() {
		new_test_ext().execute_with(|| {
			// The game is initialised with an airdrop scheduled, so the cancellation should
			// also cancel the airdrop event via `T::Airdrop::cancel`.
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 4,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();

			// Game history is written when the game starts.
			assert_eq!(GameHistory::<Test>::get(game_index), Some(schedule.game_play_time));

			// Alice signs up for the game
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(ALICE),
				DEFAULT_IDENTIFIER_KEY,
				None,
			));

			// Person player (alias + statement account)
			let alias: Alias = [42u8; 32];
			let stmt_account = id_to_account(999);
			assert_ok!(Game::sign_up_with_alias(
				runtime_origin_for_alias(&alias),
				DEFAULT_IDENTIFIER_KEY,
				stmt_account.clone(),
				AccountAuthority(stmt_account.clone()),
				None,
			));

			// Mappings must exist now.
			assert_eq!(StmtAccountToAlias::<Test>::iter().count(), 1);
			assert_eq!(AliasToStmtAccount::<Test>::iter().count(), 1);

			// Game should be in registration state
			assert!(crate::Game::<Test>::get().is_some());
			assert_eq!(
				crate::Game::<Test>::get().unwrap().state,
				GameState::Registration { next_player_index: 2 }
			);

			// Time for the registration elapses
			let registration_ends = GameTimes::<Test>::registration_end(&schedule);
			crate::mock::MOCK_UNIX_TIME
				.with(|t| *t.borrow_mut() = Duration::from_secs((registration_ends + 1) as u64));

			// Game should be in cancelling state
			advance_process_with_on_poll_only();
			assert!(crate::Game::<Test>::get().is_some());
			assert_eq!(
				crate::Game::<Test>::get().unwrap().state,
				GameState::Cancelling { last_iteration: None }
			);

			// Game should be removed
			advance_process();
			assert!(crate::Game::<Test>::get().is_none());

			// Game history entry is removed when the game is cancelled.
			assert!(GameHistory::<Test>::get(game_index).is_none());

			// Cancellation routed the airdrop event into the airdrop pallet's clean-up pipeline
			// (or dropped it outright if still `Scheduled`).
			let event_id = Game::airdrop_event_id(game_index);
			let event = indiv_pallet_airdrop::Events::<Test>::get(event_id);
			assert!(
				event.is_none() ||
					matches!(
						event.expect("checked").status,
						indiv_pallet_airdrop::types::Status::ClearingRegistrations { .. } |
							indiv_pallet_airdrop::types::Status::ClearingWinners { .. } |
							indiv_pallet_airdrop::types::Status::Finalizing { .. },
					),
			);

			// Participant score should be reset
			let score =
				indiv_pallet_score::Participants::<Test>::get(AccountOrPerson::Account(ALICE))
					.unwrap();
			assert_eq!(score.score, 0);
			assert_eq!(score.streak.absence(), 0);

			assert!(PlayerToIndex::<Test>::get(AccountOrPerson::Account(ALICE)).is_none());

			// Statement account mapping is no longer cleared on cancellation.
			assert_eq!(StmtAccountToAlias::<Test>::iter().count(), 1);
			assert_eq!(AliasToStmtAccount::<Test>::iter().count(), 1);
		});
	}
}

#[test]
fn offboard_allowed_when_game_in_registration_and_player_not_registered() {
	new_test_ext().execute_with(|| {
		// 1. Run a finished game so BOB ends up in `Players` with `registered = false`.
		let finished = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		run_game_scenario(finished, &[AccountOrPerson::Account(BOB)], |_| {
			Some(vec![BoundedVec::<Report, _>::default()].try_into().unwrap())
		});

		// Sanity-check: BOB is stored but not registered.
		let bob_key = AccountOrPerson::Account(BOB);
		assert!(
			Players::<Test>::get(&bob_key).is_some_and(|p| !p.registered),
			"BOB must exist in Players with registered = false"
		);

		// 2. Start a *new* game - still in Registration. ALICE signs up, BOB does *not*.
		let registering = GameSchedule::<u32, u128> {
			game_play_time: 50,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		assert_ok!(Game::new_game(&registering));
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(ALICE),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));

		// 2.a Off-boarding ALICE must fail (already registered).
		assert_noop!(
			Game::offboard(RuntimeOrigin::signed(ALICE)),
			Error::<Test>::CannotOffboardWhileRegisteredForGame,
		);

		// 2.b Off-boarding BOB must succeed (not registered in this game).
		assert_ok!(Game::offboard(RuntimeOrigin::signed(BOB)));
		assert!(
			!Players::<Test>::contains_key(&bob_key),
			"BOB should be removed from Players after successful offboard"
		);
		assert_eq!(
			indiv_pallet_score::Participants::<Test>::get(&bob_key),
			None,
			"BOB must also be removed from Score participants"
		);
	});
}

// Test that `PeopleVoteWeight` and `CandidateVoteWeight` are applied correctly
#[test]
fn candidate_and_people_vote_weights_applied() {
	new_test_ext().execute_with(|| {
		// Set value different from 1.
		PeopleVoteWeight::set(&3u8);
		CandidateVoteWeight::set(&2u8);

		// Make sure the mocked runtime is working as expected.
		assert_eq!(<<Test as Config>::PeopleVoteWeight as Get<u8>>::get(), 3u8);
		assert_eq!(<<Test as Config>::CandidateVoteWeight as Get<u8>>::get(), 2u8);

		// Players
		let alias_person: Alias = [10u8; 32]; // recognised "person"
		let alias_origin = runtime_origin_for_alias(&alias_person);
		let stmt_acc_person = id_to_account(10); // statement account for the person
		let candidate_account = id_to_account(20); // ordinary account (candidate)
		let target_account = id_to_account(30); // who will be judged

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 3, // put everyone in the same group
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));

		// Person signs-up (instantly recognised)
		assert_ok!(Game::sign_up_with_alias(
			alias_origin.clone(),
			DEFAULT_IDENTIFIER_KEY,
			stmt_acc_person.clone(),
			AccountAuthority(stmt_acc_person.clone()),
			None,
		));

		// Candidate & target sign-up normally
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(candidate_account.clone()),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(target_account.clone()),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));

		// Fast-forward to **Reporting** phase
		let reg_end = GameTimes::<Test>::registration_end(&schedule);
		MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs((reg_end + 1) as u64));

		advance_process(); // registration -> shuffle
		advance_process(); // shuffle -> reporting

		// Helper that builds a 1-round FullReport with the desired opinion on `target`.
		let build_report = |reporter: &AccountOrPerson<AccountId32>, target_is_person: bool| {
			let reporter_indices = PlayerToIndex::<Test>::get(reporter).unwrap();
			let reporter_idx = reporter_indices[0];

			let game = crate::Game::<Test>::get().unwrap();
			let player_count = match game.state {
				GameState::Reporting { player_count } => player_count,
				_ => unreachable!("must be Reporting"),
			};

			let groups = GroupsSetting { max_per_group: game.max_group_size, player_count };
			let group_idx = groups.group_index_from_player_index(reporter_idx);

			// Build the partial report in the deterministic `(round, player_index)` order.
			let mut partial: Vec<Report> = Vec::new();
			for member_idx in groups.group_members(group_idx) {
				if member_idx == reporter_idx {
					continue
				}
				let member_player = IndexToPlayer::<Test>::get((0, member_idx)).unwrap();
				let about_target =
					member_player == AccountOrPerson::Account(target_account.clone());
				partial.push(if about_target {
					if target_is_person {
						Report::Person
					} else {
						Report::NotPerson
					}
				} else {
					// Opinion on the "other" member doesn't matter for this test.
					Report::Person
				});
			}

			vec![partial.try_into().unwrap()].try_into().unwrap()
		};

		// Recognised person votes **Person** for the target -> +3
		assert_ok!(Game::report(
			alias_origin.clone(),
			build_report(&AccountOrPerson::Person(alias_person), /* target_is_person */ true)
		));

		// Candidate votes **NotPerson** for the target -> +2
		assert_ok!(Game::report(
			RuntimeOrigin::signed(candidate_account.clone()),
			build_report(
				&AccountOrPerson::Account(candidate_account),
				/* target_is_person */ false
			)
		));

		// Inspect the accumulated tallies
		let info = Players::<Test>::get(AccountOrPerson::Account(target_account))
			.expect("target must exist");
		assert_eq!(info.yes_person, 3, "`yes_person` must equal PeopleVoteWeight (3)");
		assert_eq!(info.no_not_person, 2, "`no_not_person` must equal CandidateVoteWeight (2)");
	});
}

// The three recognised persons must be distributed one-per-group.
// Setup: 3 persons + 6 candidates, group-size 3, rounds 2.
#[test]
fn recognised_people_are_evenly_distributed() {
	new_test_ext().execute_with(|| {
		use crate::types::GroupsSetting;
		use std::collections::HashSet;

		// 1. Create the game (2 rounds, groups of 3)
		let sched = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 2,
			max_group_size: 3,
			..Default::default()
		};
		assert_ok!(Game::new_game(&sched));

		// 2. Sign-up: three recognised persons ...
		let aliases = [[1u8; 32], [2u8; 32], [3u8; 32]];
		for (i, alias) in aliases.iter().enumerate() {
			let stmt_acc = id_to_account(1_000 + i as u64);
			assert_ok!(Game::sign_up_with_alias(
				runtime_origin_for_alias(alias),
				DEFAULT_IDENTIFIER_KEY,
				stmt_acc.clone(),
				AccountAuthority(stmt_acc),
				None,
			));
		}

		// ... plus six ordinary candidate accounts.
		for i in 0..6 {
			let acc = id_to_account(2_000 + i);
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(acc),
				DEFAULT_IDENTIFIER_KEY,
				None,
			));
		}

		// 3. Fast-forward to *after* registration so that the shuffle phase completes.
		let reg_end = GameTimes::<Test>::registration_end(&sched);
		MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs((reg_end + 1) as u64));

		// Advance blocks until the game reaches Reporting.
		loop {
			advance_process(); // run on-poll / on-idle once
			if let Some(g) = crate::Game::<Test>::get() {
				if matches!(g.state, GameState::Reporting { .. }) {
					break
				}
			} else {
				panic!("game vanished before Reporting phase");
			}
		}

		// 4. Build the GroupsSetting helper
		let (player_cnt, max_per_group) = {
			let game = crate::Game::<Test>::get().unwrap();
			let player_count = match game.state {
				GameState::Reporting { player_count } => player_count,
				_ => unreachable!("must be Reporting"),
			};
			(player_count, game.max_group_size)
		};
		let groups = GroupsSetting { max_per_group, player_count: player_cnt };
		assert_eq!(groups.number_of_group(), 3, "should be exactly 3 groups for 9 players");

		// 5. For each round ensure the 3 aliases occupy three distinct group indices {0,1,2}.
		for round in 0..sched.rounds {
			let mut grp_set: HashSet<u32> = HashSet::new();

			for alias in aliases {
				let indices = PlayerToIndex::<Test>::get(AccountOrPerson::Person(alias))
					.expect("alias must have indices");
				let idx = indices[round as usize];
				grp_set.insert(groups.group_index_from_player_index(idx));
			}

			assert_eq!(
				grp_set,
				HashSet::from([0, 1, 2]),
				"round {round}: recognised players not spread one-per-group"
			);
		}
	});
}

mod one_group_scenario {
	use super::*;

	fn build_good_report(size: u32) -> FullReport<Test> {
		use crate::Report;
		use frame_support::BoundedVec;

		let per_round = size as usize;
		let partial_vec = vec![Report::Person; per_round];
		let partial_bv: BoundedVec<Report, <Test as Config>::MaxGroupSize> =
			partial_vec.try_into().expect("fits into max group size");

		// Exactly 3 identical rounds
		let full = vec![partial_bv.clone(), partial_bv.clone(), partial_bv.clone()];
		full.try_into().expect("3 rounds <= MaxRounds")
	}

	// max_group_size = 6 and player_count = 5
	#[test]
	fn group6_players5_everybody_attends() {
		new_test_ext().execute_with(|| {
			let players: Vec<AccountOrPerson<AccountId32>> =
				(0u64..5).map(|i| AccountOrPerson::Account(id_to_account(i))).collect();

			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 3,
				max_group_size: 6,
				..Default::default()
			};

			run_game_scenario(schedule, &players, |_| Some(build_good_report(4)));

			// Everyone must have a strictly-positive score afterwards
			for p in &players {
				let score = indiv_pallet_score::Participants::<Test>::get(p)
					.expect("participant must exist")
					.score;
				assert!(score > 0, "everybody should be considered 'attended'");
			}
		});
	}

	// max_group_size = 6 and player_count = 6
	#[test]
	fn group6_players6_everybody_attends() {
		new_test_ext().execute_with(|| {
			let players: Vec<AccountOrPerson<AccountId32>> =
				(0..6).map(|i| AccountOrPerson::Account(id_to_account(i))).collect();

			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 20,
				rounds: 3,
				max_group_size: 6,
				..Default::default()
			};

			run_game_scenario(schedule, &players, |_| Some(build_good_report(5)));

			for p in &players {
				let score = indiv_pallet_score::Participants::<Test>::get(p)
					.expect("participant must exist")
					.score;
				assert!(score > 0, "everybody should be considered 'attended'");
			}
		});
	}

	#[test]
	fn groups_5_6_scenario() {
		use crate::types::GroupsSetting;

		let gs = GroupsSetting { max_per_group: 6, player_count: 5 };
		assert_eq!(gs.number_of_group(), 1);

		for idx in 0..5 {
			assert_eq!(gs.group_index_from_player_index(idx), 0);
		}

		assert_eq!(gs.group_members(0).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
	}

	#[test]
	fn groups_6_6_scenario() {
		use crate::types::GroupsSetting;

		let gs = GroupsSetting { max_per_group: 6, player_count: 6 };
		assert_eq!(gs.number_of_group(), 1);

		for idx in 0..6 {
			assert_eq!(gs.group_index_from_player_index(idx), 0);
		}

		assert_eq!(gs.group_members(0).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4, 5]);
	}
}

#[test]
fn nft_spec() {
	let calculated = Game::compute_nft(
		32,
		5,
		&AccountOrPerson::Account([1u8; 32].into()),
		&AccountOrPerson::Person([2u8; 32]),
	);
	let expected = (b"polkadot-pop-game", 32u32, 5u8, 0u8, [1u8; 32], 1u8, [2u8; 32])
		.using_encoded(sp_io::hashing::blake2_256);

	assert_eq!(calculated, expected);
	assert_eq!(
		calculated,
		[
			135, 205, 206, 159, 168, 238, 124, 124, 43, 173, 199, 120, 3, 56, 148, 117, 67, 126,
			78, 190, 126, 15, 187, 177, 224, 186, 115, 113, 73, 121, 224, 196
		]
	);

	let calculated = Game::compute_nft(
		35,
		9,
		&AccountOrPerson::Person([3u8; 32]),
		&AccountOrPerson::Account([4u8; 32].into()),
	);
	let expected = (b"polkadot-pop-game", 35u32, 9u8, 1u8, [3u8; 32], 0u8, [4u8; 32])
		.using_encoded(sp_io::hashing::blake2_256);

	assert_eq!(calculated, expected);
	assert_eq!(
		calculated,
		[
			86, 240, 179, 9, 73, 32, 219, 236, 202, 127, 104, 185, 169, 196, 74, 74, 168, 221, 30,
			78, 35, 75, 128, 151, 175, 250, 203, 174, 199, 71, 243, 194
		]
	);
}

#[test]
fn reach_lose_and_regain_personhood() {
	new_test_ext().execute_with(|| {
		use frame_support::BoundedVec;

		// Create the people collection first
		assert_ok!(Members::create_collection(
			0,
			PEOPLE_MEMBER_IDENTIFIER,
			1,
			RingMode::Flexible,
			RingExponent::R2e9,
			None,
		));

		let alice = AccountOrPerson::Account(ALICE);

		let mut schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 1,
			..Default::default()
		};
		let attend_report: FullReport<Test> = vec![BoundedVec::default()].try_into().unwrap();

		// 1) Reach personhood: run games where ALICE attends until personhood is true.
		let mut safety = 0;
		while !Score::reached_personhood(&alice) {
			run_game_scenario(schedule.clone(), slice::from_ref(&alice), |_p| {
				Some(attend_report.clone()) // attend
			});
			schedule.game_play_time += 10; // ensure times don't overlap
			safety += 1;
			assert!(safety < 200, "took too many games to reach personhood");
		}
		assert!(Score::reached_personhood(&alice), "must have reached personhood");

		let (key_for_person, sk) = mock_key(1234);
		let proof = {
			let mut m = b"pop register using".to_vec();
			m.extend_from_slice(&ALICE.encode()[..]);
			Mock::sign(&sk, &m[..]).unwrap()
		};
		assert_ok!(Score::register(RuntimeOrigin::signed(ALICE), Some((key_for_person, proof))));

		// 2) Lose personhood: run games where ALICE signs up but does NOT report (absent) until
		//    personhood becomes false.
		let mut safety2 = 0;
		while Score::reached_personhood(&alice) {
			run_game_scenario(schedule.clone(), slice::from_ref(&alice), |_p| {
				None // absent (no report)
			});
			schedule.game_play_time += 10;
			safety2 += 1;
			assert!(safety2 < 200, "took too many games to lose personhood");
		}
		assert!(!Score::reached_personhood(&alice), "personhood should have been lost");

		// 3) Regain personhood: attend again until personhood becomes true.
		let mut safety3 = 0;
		while !Score::reached_personhood(&alice) {
			run_game_scenario(schedule.clone(), slice::from_ref(&alice), |_p| {
				Some(attend_report.clone()) // attend
			});
			schedule.game_play_time += 10;
			safety3 += 1;
			assert!(safety3 < 200, "took too many games to regain personhood");
		}
		assert!(Score::reached_personhood(&alice), "must have regained personhood");
		assert_ok!(Score::register(RuntimeOrigin::signed(ALICE), None));
	});
}

#[test]
fn game_cancelled_due_to_shuffle_deadline_missed() {
	new_test_ext().execute_with(|| {
		// Set up a game that *would* be acceptable to shuffle (2 players; max_group_size = 3).
		// With MaxGroupSize = 3, GroupsSetting::acceptable_player_count() is true for 2 players.
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 100,
			rounds: 2,
			max_group_size: 3,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));

		// Register one account player and one person (alias + statement account).
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(ALICE),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));
		let alias: Alias = [77u8; 32];
		let stmt_account = id_to_account(777);
		assert_ok!(Game::sign_up_with_alias(
			runtime_origin_for_alias(&alias),
			DEFAULT_IDENTIFIER_KEY,
			stmt_account.clone(),
			AccountAuthority(stmt_account),
			None,
		));

		// Sanity: we are in registration with two registered players.
		assert!(crate::Game::<Test>::get().is_some());
		assert_eq!(
			crate::Game::<Test>::get().unwrap().state,
			GameState::Registration { next_player_index: 2 }
		);

		// Move time to exactly registration_end + 1 (which equals shuffle_deadline).
		let reg_end = GameTimes::<Test>::registration_end(&schedule);
		let shuffle_deadline = GameTimes::<Test>::shuffle_deadline(&schedule);
		MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs((reg_end + 1) as u64));

		// One on_poll: Registration -> Shuffle (no shuffle work yet).
		advance_process_with_on_poll_only();
		assert!(crate::Game::<Test>::get().is_some());
		assert_eq!(
			crate::Game::<Test>::get().unwrap().state,
			GameState::Shuffle { step: ShuffleStep::Step1Insert { last_iteration: None } }
		);

		// Push time beyond shuffle_deadline so shuffles() cancels immediately.
		MOCK_UNIX_TIME
			.with(|t| *t.borrow_mut() = Duration::from_secs((shuffle_deadline + 1) as u64));
		advance_process_with_on_poll_only();
		assert!(crate::Game::<Test>::get().is_some());
		assert_eq!(
			crate::Game::<Test>::get().unwrap().state,
			GameState::Cancelling { last_iteration: None }
		);

		// Let the Cancelling phase do its cleanup in the next block.
		advance_process();
		assert!(crate::Game::<Test>::get().is_none());

		// Participant (ALICE) remains in score with zero score and no absence recorded.
		let score =
			indiv_pallet_score::Participants::<Test>::get(AccountOrPerson::Account(ALICE)).unwrap();
		assert_eq!(score.score, 0);
		assert_eq!(score.streak.absence(), 0);

		// No PlayerToIndex mappings should remain.
		assert!(PlayerToIndex::<Test>::get(AccountOrPerson::Account(ALICE)).is_none());

		// The corresponding GameHistory entry was removed.
		let current_index = GameIndex::<Test>::get(); // == 1 in a fresh test ext
		assert_eq!(GameHistory::<Test>::get(current_index), None);
	});
}

#[test]
fn credibility_flow_deposit_to_recognized_to_archived() {
	new_test_ext().execute_with(|| {
		use deposit::DepositStorage;

		// 1 active member yields a personhood threshold of 1.
		indiv_pallet_members::ActiveMembers::<Test>::insert(PEOPLE_MEMBER_IDENTIFIER, 1);

		let subject_acc = ALICE;
		let subject = AccountOrPerson::Account(subject_acc);

		// ─────────────────────────────────────────────────────────────────────────────
		// Game #1 (deposit -> recognized): a single attendance flips personhood,
		// which triggers Deposit -> Recognized and drops the deposit.
		// ─────────────────────────────────────────────────────────────────────────────
		let g1 = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 1,
			..Default::default()
		};

		run_game_scenario(g1, slice::from_ref(&subject), |_p| {
			// Empty report in a 1‑member group => "attended"
			Some(
				vec![BoundedVec::<Report, <Test as Config>::MaxGroupSize>::default()]
					.try_into()
					.unwrap(),
			)
		});

		// SCORE says the account reached personhood…
		assert!(Score::reached_personhood(&subject));

		// …and the pallet updated credibility Deposit -> Recognized.
		let player = Players::<Test>::get(&subject).expect("player should still be stored");
		match player.credibility {
			PlayerCredibility::Recognized => {},
			other => panic!("expected Recognized after reaching personhood, got {other:?}"),
		}

		// Deposit must be DROPPED (not active, not burned).
		let ds = DepositStorage::<Test>::get();
		assert_eq!(ds.active.len(), 0, "deposit should not remain active");
		assert_eq!(ds.burned.len(), 0, "deposit should not be burned on recognition");
		assert_eq!(ds.dropped.len(), 1, "deposit should be dropped when recognized");

		// ─────────────────────────────────────────────────────────────────────────────
		// Games #2+ (recognized -> archived): be absent until SCORE goes to 0.
		// Since deposit was dropped already, archive must NOT burn anything.
		// ─────────────────────────────────────────────────────────────────────────────
		let mut when = 20u32;
		while let Some(part) = indiv_pallet_score::Participants::<Test>::get(&subject) {
			if part.score == 0 {
				break;
			}
			let g = GameSchedule::<u32, u128> {
				game_play_time: when,
				rounds: 1,
				max_group_size: 1,
				..Default::default()
			};
			run_game_scenario(g, slice::from_ref(&subject), |_p| None); // absent
			when += 10;
		}

		// Now archived and removed from Players.
		let archived = ArchivedPlayers::<Test>::get(&subject).expect("must be archived");
		assert!(
			matches!(archived, ArchivedPlayer::Kickable { .. }),
			"internally recognized should be Kickable"
		);
		assert!(!Players::<Test>::contains_key(&subject));

		// No *new* burns must have happened (deposit was already dropped).
		let ds2 = DepositStorage::<Test>::get();
		assert_eq!(ds2.burned.len(), 0, "no burn on archive after deposit was dropped");
	});
}

#[test]
fn credibility_flow_invited_to_recognized_in_score_to_archived() {
	new_test_ext().execute_with(|| {
		use deposit::DepositStorage;

		// 1 active member yields a personhood threshold of 1.
		indiv_pallet_members::ActiveMembers::<Test>::insert(PEOPLE_MEMBER_IDENTIFIER, 1);

		// INVITER has invites; INVITED will use one.
		const INVITER: AccountId32 = ALICE;
		let invited = AccountId32::new(*b"ivtd____________________________");
		let invited_key = AccountOrPerson::Account(invited.clone());
		let ticket: u64 = 4242;
		let signature = TestSignature(ticket, invited.encode());

		// Root grants + inviter registers the ticket.
		assert_ok!(Game::grant_invites(RuntimeOrigin::root(), INVITER, 1));
		assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(INVITER), ticket));

		// ─────────────────────────────────────────────────────────────────────────────
		// Game #1 (invited -> recognized in SCORE): a single attendance flips personhood.
		// NOTE: PlayerCredibility remains Invited in the current implementation.
		// ─────────────────────────────────────────────────────────────────────────────
		let g1 = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 1,
			..Default::default()
		};

		run_game_scenario_with_phase(
			g1,
			|| {
				let nonce = frame_system::Account::<Test>::get(&invited).nonce;
				assert_ok!(exec_invited_tx(
					invited.clone(),
					GameAsInvitedData {
						nonce,
						inviter: INVITER,
						ticket,
						signature: signature.clone()
					},
					Call::sign_up_with_invite {
						identifier_key: DEFAULT_IDENTIFIER_KEY,
						airdrop: None
					}
				));
			},
			|| {
				let empty = vec![BoundedVec::<Report, <Test as Config>::MaxGroupSize>::default()]
					.try_into()
					.unwrap();
				assert_ok!(Game::report(RuntimeOrigin::signed(invited.clone()), empty));
			},
		);

		// SCORE-level recognition happened…
		assert!(Score::reached_personhood(&invited_key));

		// …but pallet credibility (by design today) stays Invited.
		let cred_now = Players::<Test>::get(&invited_key)
			.expect("player should be present")
			.credibility;
		assert!(matches!(cred_now, PlayerCredibility::Invited));

		// Invited players never create deposits.
		let ds = DepositStorage::<Test>::get();
		assert_eq!(ds.active.len(), 0);
		assert_eq!(ds.burned.len(), 0);
		assert_eq!(ds.dropped.len(), 0);

		// ─────────────────────────────────────────────────────────────────────────────
		// Games #2+ (to archive): sign up without an invite (no deposit is required
		// while not archived), then be absent until SCORE hits 0.
		// ─────────────────────────────────────────────────────────────────────────────
		let mut when = 20u32;
		while let Some(part) = indiv_pallet_score::Participants::<Test>::get(&invited_key) {
			if part.score == 0 {
				break;
			}
			let g = GameSchedule::<u32, u128> {
				game_play_time: when,
				rounds: 1,
				max_group_size: 1,
				..Default::default()
			};
			run_game_scenario_with_phase(
				g,
				|| {
					// Not archived yet -> sign_up_with_account is free of deposit in this pallet.
					assert_ok!(Game::sign_up_with_account(
						RuntimeOrigin::signed(invited.clone()),
						DEFAULT_IDENTIFIER_KEY,
						None,
					));
				},
				|| {
					// No report => absent
				},
			);
			when += 10;
		}

		// Archived as Kickable (not externally recognized).
		let archived = ArchivedPlayers::<Test>::get(&invited_key).expect("must be archived");
		assert!(matches!(archived, ArchivedPlayer::Kickable { .. }));
		assert!(!Players::<Test>::contains_key(&invited_key));

		// Deposits remain untouched for invited flow.
		let ds2 = DepositStorage::<Test>::get();
		assert_eq!(ds2.active.len(), 0);
		assert_eq!(ds2.burned.len(), 0);
		assert_eq!(ds2.dropped.len(), 0);
	});
}

#[test]
fn vote_weight_tracks_current_recognition_and_drops_when_lost() {
	new_test_ext().execute_with(|| {
		// Make the two weights obviously different.
		PeopleVoteWeight::set(&5u8);
		CandidateVoteWeight::set(&2u8);

		// Reporter is an ordinary account. We'll reuse it through all scenarios.
		let reporter_acc = ALICE;
		let reporter = AccountOrPerson::Account(reporter_acc.clone());

		// Three distinct targets—one per scenario (to avoid state coupling).
		let target1 = id_to_account(2_001);
		let target2 = id_to_account(2_002);
		let target3 = id_to_account(2_003);

		// Small helper: start a 1-round, 2-per-group game, have `reporter_acc` report
		// "Person" on `target_acc`, and assert that the `yes_person` tally equals `expected_w`.
		let assert_vote_weight = |game_time: u32,
		                          reporter_acc: AccountId32,
		                          target_acc: AccountId32,
		                          expected_w: u8| {
			use core::time::Duration;

			let schedule = GameSchedule::<u32, u128> {
				game_play_time: game_time,
				rounds: 1,
				max_group_size: 2, // reporter and target in the same group
				..Default::default()
			};

			// New game + sign-ups.
			assert_ok!(Game::new_game(&schedule));
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(reporter_acc.clone()),
				DEFAULT_IDENTIFIER_KEY,
				None,
			));
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(target_acc.clone()),
				DEFAULT_IDENTIFIER_KEY,
				None,
			));

			// Fast-forward to Reporting.
			let reg_end = GameTimes::<Test>::registration_end(&schedule);
			MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs((reg_end + 1) as u64));
			advance_process(); // registration -> shuffle
			advance_process(); // shuffle -> reporting

			// Reporter submits a single opinion (there's exactly one peer in the group).
			let partial: BoundedVec<Report, <Test as Config>::MaxGroupSize> =
				vec![Report::Person].try_into().unwrap();
			let full: FullReport<Test> = vec![partial].try_into().unwrap();
			assert_ok!(Game::report(RuntimeOrigin::signed(reporter_acc.clone()), full));

			// Check immediate tallies (before PlayerProcess resets them).
			let info = Players::<Test>::get(AccountOrPerson::Account(target_acc.clone()))
				.expect("target must be registered");
			assert_eq!(
				info.yes_person, expected_w,
				"reporter should contribute exactly the expected vote weight"
			);

			// Finish the game so we can start another one.
			let rep_end = GameTimes::<Test>::reporting_end(&schedule);
			MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs((rep_end + 1) as u64));
			advance_process(); // reporting -> player process
			advance_process(); // step1 -> step2
			advance_process(); // step2 -> step3
			advance_process(); // step3 -> done
		};

		// ─────────────────────────────────────────────────────────────────────────────
		// Scenario 1: NOT recognized => candidate weight.
		// ─────────────────────────────────────────────────────────────────────────────
		assert!(
			!Score::reached_personhood(&reporter),
			"fresh reporter must not be recognized at start"
		);
		assert_vote_weight(
			/* game_time */ 10,
			reporter_acc.clone(),
			target1,
			<Test as Config>::CandidateVoteWeight::get(),
		);

		// ─────────────────────────────────────────────────────────────────────────────
		// Build the reporter UP to recognition (personhood).
		// We just run small one-player games where the reporter attends (empty report).
		// ─────────────────────────────────────────────────────────────────────────────
		let mut t = 100;
		let mut sched = GameSchedule::<u32, u128> {
			game_play_time: t,
			rounds: 1,
			max_group_size: 1, // single-player "group" => empty report is valid attendance
			..Default::default()
		};
		let empty_full_report: FullReport<Test> =
			vec![BoundedVec::<Report, <Test as Config>::MaxGroupSize>::default()]
				.try_into()
				.unwrap();

		// One "attend" to initialize, then loop until personhood is reached.
		run_game_scenario(sched.clone(), slice::from_ref(&reporter), |_p| {
			Some(empty_full_report.clone())
		});
		t += 10;
		while !Score::reached_personhood(&reporter) {
			sched.game_play_time = t;
			run_game_scenario(sched.clone(), slice::from_ref(&reporter), |_p| {
				Some(empty_full_report.clone())
			});
			t += 10;
		}
		assert!(Score::reached_personhood(&reporter), "reporter should be recognized now");

		// ─────────────────────────────────────────────────────────────────────────────
		// Scenario 2: recognized => people weight.
		// ─────────────────────────────────────────────────────────────────────────────
		assert_vote_weight(
			/* game_time */ 1_000,
			reporter_acc.clone(),
			target2,
			<Test as Config>::PeopleVoteWeight::get(),
		);

		// ─────────────────────────────────────────────────────────────────────────────
		// Register the reporter as a person so that absence triggers suspension
		// (reached_personhood only resets via the grace-period suspension path,
		// which requires Recognized status).
		// ─────────────────────────────────────────────────────────────────────────────
		{
			assert_ok!(Members::create_collection(
				0,
				PEOPLE_MEMBER_IDENTIFIER,
				1,
				RingMode::Flexible,
				RingExponent::R2e9,
				None,
			));
			let (key, sk) = mock_key(42);
			let proof = {
				let mut msg = b"pop register using".to_vec();
				msg.extend_from_slice(&reporter_acc.encode()[..]);
				Mock::sign(&sk, &msg[..]).unwrap()
			};
			assert_ok!(Score::register(
				RuntimeOrigin::signed(reporter_acc.clone()),
				Some((key, proof))
			));
		}

		// ─────────────────────────────────────────────────────────────────────────────
		// Push the reporter BELOW recognition via suspension.
		// With 100k active members the absence grace ratio is (1, 6), so two
		// consecutive absences are needed to exceed the allowance and suspend.
		// ─────────────────────────────────────────────────────────────────────────────
		let down1 = GameSchedule::<u32, u128> {
			game_play_time: 1_100,
			rounds: 1,
			max_group_size: 1,
			..Default::default()
		};
		run_game_scenario(down1, slice::from_ref(&reporter), |_p| None); // 1st absence
		let down2 = GameSchedule::<u32, u128> {
			game_play_time: 1_110,
			rounds: 1,
			max_group_size: 1,
			..Default::default()
		};
		run_game_scenario(down2, slice::from_ref(&reporter), |_p| None); // 2nd absence → suspended
		assert!(
			!Score::reached_personhood(&reporter),
			"reporter should have lost personhood after suspension"
		);
		let part = indiv_pallet_score::Participants::<Test>::get(&reporter).unwrap();
		assert!(part.score > 0);

		// ─────────────────────────────────────────────────────────────────────────────
		// Scenario 3: once recognition is lost (but not archived) => candidate weight.
		// ─────────────────────────────────────────────────────────────────────────────
		assert_vote_weight(
			/* game_time */ 2_000,
			reporter_acc,
			target3,
			<Test as Config>::CandidateVoteWeight::get(),
		);
	});
}

/// Test that statement allowance is correctly managed for account-based players:
/// - Allowance increases when a player signs up for a game
/// - Allowance decreases when a player gets archived
/// - Allowance is NOT decreased between consecutive games if the player is still active
#[test]
fn statement_allowance_for_account_players() {
	use sp_statement_store::{get_allowance, StatementAllowance};

	new_test_ext().execute_with(|| {
		let player1 = AccountOrPerson::Account(ALICE);
		let player2 = AccountOrPerson::Account(BOB);
		let expected_allowance: StatementAllowance = PlayerStatementLimit::get();
		let zero_allowance = StatementAllowance::default();

		// Initial allowance should be zero for both players
		assert_eq!(get_allowance(ALICE), zero_allowance);
		assert_eq!(get_allowance(BOB), zero_allowance);

		// ─────────────────────────────────────────────────────────────────────────────
		// Game 1: Both players sign up
		// ─────────────────────────────────────────────────────────────────────────────
		let schedule1 = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};

		assert_ok!(Game::new_game(&schedule1));

		// Player 1 (ALICE) signs up - allowance should increase
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(ALICE),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));
		assert_eq!(get_allowance(ALICE), expected_allowance);

		// Player 2 (BOB) signs up - allowance should increase
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(BOB),
			DEFAULT_IDENTIFIER_KEY,
			None
		));
		assert_eq!(get_allowance(BOB), expected_allowance);

		// Move to reporting phase
		let registration_ends = GameTimes::<Test>::registration_end(&schedule1);
		MOCK_UNIX_TIME
			.with(|t| *t.borrow_mut() = Duration::from_secs((registration_ends + 1) as u64));
		advance_process(); // registration -> shuffle
		advance_process(); // shuffle -> reporting

		// ALICE does NOT report (will be absent)
		// BOB reports (will attend)
		let partial: BoundedVec<Report, <Test as Config>::MaxGroupSize> =
			vec![Report::Person].try_into().unwrap();
		let full: FullReport<Test> = vec![partial].try_into().unwrap();
		assert_ok!(Game::report(RuntimeOrigin::signed(BOB), full));

		// Finish game 1
		let report_ends = GameTimes::<Test>::reporting_end(&schedule1);
		MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs((report_ends + 1) as u64));
		advance_process(); // reporting -> player process
		advance_process(); // step1 -> step2
		advance_process(); // step2 -> step3
		advance_process(); // step3 -> done

		// After game 1:
		// - ALICE was absent (no report), should have score 0 and be archived
		// - BOB attended, should still be active
		assert!(
			ArchivedPlayers::<Test>::contains_key(&player1),
			"ALICE should be archived after being absent"
		);
		assert!(
			!ArchivedPlayers::<Test>::contains_key(&player2),
			"BOB should NOT be archived after attending"
		);

		// ALICE's allowance should be cleared (archived player has allowance removed)
		assert_eq!(
			get_allowance(ALICE),
			zero_allowance,
			"ALICE allowance should be 0 after being archived"
		);

		// BOB's allowance should still be set (still active player)
		assert_eq!(
			get_allowance(BOB),
			expected_allowance,
			"BOB allowance should remain after game 1"
		);

		// ─────────────────────────────────────────────────────────────────────────────
		// Game 2: BOB participates again (allowance should NOT change between games)
		// ─────────────────────────────────────────────────────────────────────────────
		let schedule2 = GameSchedule::<u32, u128> {
			game_play_time: 25,
			rounds: 1,
			max_group_size: 1,
			..Default::default()
		};

		assert_ok!(Game::new_game(&schedule2));

		// BOB signs up for game 2 - allowance should NOT increase (already has allowance)
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(BOB),
			DEFAULT_IDENTIFIER_KEY,
			None
		));
		assert_eq!(
			get_allowance(BOB),
			expected_allowance,
			"BOB allowance should not double after signing up for game 2"
		);

		// Move to reporting phase
		let registration_ends2 = GameTimes::<Test>::registration_end(&schedule2);
		MOCK_UNIX_TIME
			.with(|t| *t.borrow_mut() = Duration::from_secs((registration_ends2 + 1) as u64));
		advance_process(); // registration -> shuffle
		advance_process(); // shuffle -> reporting

		// BOB does NOT report this time (will be absent and eventually archived)
		// Finish game 2
		let report_ends2 = GameTimes::<Test>::reporting_end(&schedule2);
		MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs((report_ends2 + 1) as u64));
		advance_process(); // reporting -> player process
		advance_process(); // step1 -> step2
		advance_process(); // step2 -> step3
		advance_process(); // step3 -> done

		// ─────────────────────────────────────────────────────────────────────────────
		// Continue with more games until BOB is archived (score reaches 0)
		// ─────────────────────────────────────────────────────────────────────────────
		let mut game_time = 40u32;
		while !ArchivedPlayers::<Test>::contains_key(&player2) {
			// Check allowance is still set while player is active
			assert_eq!(
				get_allowance(BOB),
				expected_allowance,
				"BOB allowance should remain while not archived"
			);

			let schedule = GameSchedule::<u32, u128> {
				game_play_time: game_time,
				rounds: 1,
				max_group_size: 1,
				..Default::default()
			};

			run_game_scenario(schedule, slice::from_ref(&player2), |_| None); // absent
			game_time += 15;
		}

		// After BOB is archived, allowance should be cleared
		assert!(ArchivedPlayers::<Test>::contains_key(&player2), "BOB should now be archived");
		assert_eq!(
			get_allowance(BOB),
			zero_allowance,
			"BOB allowance should be 0 after being archived"
		);
	});
}

/// Test that statement allowance is correctly managed for alias-based players:
/// - Allowance increases on first sign-up with a statement account
/// - Allowance transfers when the alias switches statement accounts
/// - Allowance decreases when the alias is archived
#[test]
fn statement_allowance_for_alias_players() {
	use sp_statement_store::{get_allowance, StatementAllowance};

	new_test_ext().execute_with(|| {
		let alias = id_to_alias(101);
		let player = AccountOrPerson::Person(alias);
		let stmt_account_1 = id_to_account(201);
		let stmt_account_2 = id_to_account(202);
		let expected_allowance: StatementAllowance = PlayerStatementLimit::get();
		let zero_allowance = StatementAllowance::default();
		let empty_report: FullReport<Test> =
			vec![BoundedVec::<Report, <Test as Config>::MaxGroupSize>::default()]
				.try_into()
				.unwrap();

		let advance_to_reporting = |schedule: &GameSchedule<u32, u128>| {
			let registration_ends = GameTimes::<Test>::registration_end(schedule);
			MOCK_UNIX_TIME.with(|t| {
				*t.borrow_mut() = Duration::from_secs((registration_ends + 1) as u64);
			});
			advance_process(); // registration -> shuffle
			advance_process(); // shuffle -> reporting
		};

		let finish_game = |schedule: &GameSchedule<u32, u128>| {
			let report_ends = GameTimes::<Test>::reporting_end(schedule);
			MOCK_UNIX_TIME
				.with(|t| *t.borrow_mut() = Duration::from_secs((report_ends + 1) as u64));
			advance_process(); // reporting -> player process
			advance_process(); // step1 -> step2
			advance_process(); // step2 -> step3
			advance_process(); // step3 -> done
		};

		// Initial allowance should be zero for both statement accounts.
		assert_eq!(get_allowance(&stmt_account_1), zero_allowance);
		assert_eq!(get_allowance(&stmt_account_2), zero_allowance);

		// ─────────────────────────────────────────────────────────────────────────────
		// Game 1: alias signs up with statement account 1, allowance increases.
		// ─────────────────────────────────────────────────────────────────────────────
		let schedule1 = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 1,
			..Default::default()
		};

		assert_ok!(Game::new_game(&schedule1));
		assert_ok!(Game::sign_up_with_alias(
			runtime_origin_for_alias(&alias),
			DEFAULT_IDENTIFIER_KEY,
			stmt_account_1.clone(),
			AccountAuthority(stmt_account_1.clone()),
			None,
		));
		assert_eq!(get_allowance(&stmt_account_1), expected_allowance);
		assert_eq!(get_allowance(&stmt_account_2), zero_allowance);

		advance_to_reporting(&schedule1);
		assert_ok!(Game::report(runtime_origin_for_alias(&alias), empty_report.clone()));
		finish_game(&schedule1);

		assert!(!ArchivedPlayers::<Test>::contains_key(&player));
		assert_eq!(get_allowance(&stmt_account_1), expected_allowance);

		// ─────────────────────────────────────────────────────────────────────────────
		// Game 2: alias signs up again with the same statement account, allowance unchanged.
		// ─────────────────────────────────────────────────────────────────────────────
		let schedule2 = GameSchedule::<u32, u128> {
			game_play_time: 25,
			rounds: 1,
			max_group_size: 1,
			..Default::default()
		};

		assert_ok!(Game::new_game(&schedule2));
		assert_ok!(Game::sign_up_with_alias(
			runtime_origin_for_alias(&alias),
			DEFAULT_IDENTIFIER_KEY,
			stmt_account_1.clone(),
			AccountAuthority(stmt_account_1.clone()),
			None,
		));
		assert_eq!(get_allowance(&stmt_account_1), expected_allowance);

		advance_to_reporting(&schedule2);
		assert_ok!(Game::report(runtime_origin_for_alias(&alias), empty_report.clone()));
		finish_game(&schedule2);

		assert!(!ArchivedPlayers::<Test>::contains_key(&player));
		assert_eq!(get_allowance(&stmt_account_1), expected_allowance);

		// ─────────────────────────────────────────────────────────────────────────────
		// Game 3: alias switches statement account, allowance transfers.
		// ─────────────────────────────────────────────────────────────────────────────
		let schedule3 = GameSchedule::<u32, u128> {
			game_play_time: 40,
			rounds: 1,
			max_group_size: 1,
			..Default::default()
		};

		assert_ok!(Game::new_game(&schedule3));
		assert_ok!(Game::sign_up_with_alias(
			runtime_origin_for_alias(&alias),
			DEFAULT_IDENTIFIER_KEY,
			stmt_account_2.clone(),
			AccountAuthority(stmt_account_2.clone()),
			None,
		));
		assert_eq!(get_allowance(&stmt_account_1), zero_allowance);
		assert_eq!(get_allowance(&stmt_account_2), expected_allowance);

		advance_to_reporting(&schedule3);
		assert_ok!(Game::report(runtime_origin_for_alias(&alias), empty_report.clone()));
		finish_game(&schedule3);

		assert!(!ArchivedPlayers::<Test>::contains_key(&player));
		assert_eq!(get_allowance(&stmt_account_2), expected_allowance);

		// ─────────────────────────────────────────────────────────────────────────────
		// Game 4: alias is absent, gets archived, allowance decreases.
		// ─────────────────────────────────────────────────────────────────────────────
		let schedule4 = GameSchedule::<u32, u128> {
			game_play_time: 55,
			rounds: 1,
			max_group_size: 1,
			..Default::default()
		};

		assert_ok!(Game::new_game(&schedule4));
		assert_ok!(Game::sign_up_with_alias(
			runtime_origin_for_alias(&alias),
			DEFAULT_IDENTIFIER_KEY,
			stmt_account_2.clone(),
			AccountAuthority(stmt_account_2.clone()),
			None,
		));
		assert_eq!(get_allowance(&stmt_account_2), expected_allowance);

		advance_to_reporting(&schedule4);
		// No report => absent.
		finish_game(&schedule4);

		assert!(ArchivedPlayers::<Test>::contains_key(&player));
		assert_eq!(get_allowance(&stmt_account_2), zero_allowance);
	});
}

// ─────────────────────────────────────────────────────────────────────────────
// Early attendance enactment, pending_attendance tracking, early reporting-
// phase transition, and vote_weight snapshotting.
// ─────────────────────────────────────────────────────────────────────────────

/// Build a `FullReport` for `reporter` where the opinion on each co-player in
/// each round is chosen by `opinion`.
fn build_report_with_opinion(
	reporter: &AccountOrPerson<AccountId32>,
	opinion: impl Fn(&AccountOrPerson<AccountId32>) -> Report,
) -> FullReport<Test> {
	let reporter_indices = PlayerToIndex::<Test>::get(reporter).expect("reporter has indices");
	let game = crate::Game::<Test>::get().expect("game exists");
	let player_count = match game.state {
		GameState::Reporting { player_count } => player_count,
		_ => panic!("game must be in Reporting state"),
	};
	let groups = GroupsSetting { max_per_group: game.max_group_size, player_count };
	let mut full = Vec::new();
	for round in 0..game.rounds {
		let reporter_idx = reporter_indices[round as usize];
		let group_idx = groups.group_index_from_player_index(reporter_idx);
		let partial: Vec<Report> = groups
			.group_members(group_idx)
			.filter(|&idx| idx != reporter_idx)
			.map(|idx| {
				let member = IndexToPlayer::<Test>::get((round, idx)).expect("member exists");
				opinion(&member)
			})
			.collect();
		full.push(partial.try_into().expect("fits MaxGroupSize"));
	}
	full.try_into().expect("fits MaxRounds")
}

/// Advance through registration and the full shuffle phase, stopping in Reporting.
fn advance_to_reporting_phase(schedule: &GameSchedule<u32, u128>) {
	let reg_end = GameTimes::<Test>::registration_end(schedule);
	MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs((reg_end + 1) as u64));
	advance_process(); // registration -> shuffle
	advance_process(); // shuffle -> reporting (steps 1-4)
	assert!(matches!(crate::Game::<Test>::get().unwrap().state, GameState::Reporting { .. }));
}

#[test]
fn early_enactment_attended_when_group_saturates_yes() {
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 20,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(ALICE),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(BOB),
			DEFAULT_IDENTIFIER_KEY,
			None
		));
		advance_to_reporting_phase(&schedule);

		// Each player has exactly one co-player in the single round.
		let report: FullReport<Test> =
			vec![vec![Report::Person].try_into().unwrap()].try_into().unwrap();
		assert_ok!(Game::report(RuntimeOrigin::signed(ALICE), report.clone()));
		assert_ok!(Game::report(RuntimeOrigin::signed(BOB), report));

		for acc in [ALICE, BOB] {
			let info = Players::<Test>::get(AccountOrPerson::Account(acc.clone())).unwrap();
			assert!(
				matches!(
					info.early_attendance_enactment,
					Some(EarlyAttendanceEnactment { attendance: true, .. })
				),
				"{acc:?} should be early-enacted as attended"
			);
		}
		assert_eq!(crate::Game::<Test>::get().unwrap().pending_attendance, 0);
	});
}

#[test]
fn early_enactment_not_attended_without_own_report() {
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 20,
			rounds: 1,
			max_group_size: 3,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));
		for acc in [ALICE, BOB, CHARLIE] {
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(acc),
				DEFAULT_IDENTIFIER_KEY,
				None,
			));
		}
		advance_to_reporting_phase(&schedule);

		let alice_key = AccountOrPerson::Account(ALICE);

		// BOB and CHARLIE both report ALICE as NotPerson; their opinion on the
		// third co-player is Person (we don't want to accidentally early-enact them).
		let opinion = |target: &AccountOrPerson<AccountId32>| {
			if target == &alice_key {
				Report::NotPerson
			} else {
				Report::Person
			}
		};
		assert_ok!(Game::report(
			RuntimeOrigin::signed(BOB),
			build_report_with_opinion(&AccountOrPerson::Account(BOB), opinion),
		));
		assert_ok!(Game::report(
			RuntimeOrigin::signed(CHARLIE),
			build_report_with_opinion(&AccountOrPerson::Account(CHARLIE), opinion),
		));

		let alice_info = Players::<Test>::get(&alice_key).unwrap();
		assert!(!alice_info.sent_report, "ALICE never called report in this test");
		assert!(matches!(
			alice_info.early_attendance_enactment,
			Some(EarlyAttendanceEnactment { attendance: false, .. })
		));
	});
}

#[test]
fn nft_value_records_mint_time() {
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 100,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(ALICE),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(BOB),
			DEFAULT_IDENTIFIER_KEY,
			None
		));
		advance_to_reporting_phase(&schedule);

		// Pin a deterministic timestamp inside the reporting window so we can assert
		// the value recorded in `Nfts`.
		let report_open = schedule.game_play_time;
		MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs(report_open as u64));

		let report: FullReport<Test> =
			vec![vec![Report::Person].try_into().unwrap()].try_into().unwrap();
		assert_ok!(Game::report(RuntimeOrigin::signed(ALICE), report.clone()));
		assert_ok!(Game::report(RuntimeOrigin::signed(BOB), report));

		let alice = AccountOrPerson::Account(ALICE);
		let bob = AccountOrPerson::Account(BOB);
		let nft_for_alice = Game::compute_nft(1, 0, &bob, &alice);
		assert_eq!(Nfts::<Test>::get(&alice, nft_for_alice), Some(report_open));
	});
}

#[test]
fn notperson_nft_promoted_when_attendance_decided() {
	// Four-player group, single round. BOB votes NotPerson on ALICE; CHARLIE/DAVE
	// vote Person on her. ALICE ends up Attended (2 yes, 1 no, no remaining) — the
	// staged NotPerson NFT from BOB must be promoted from `NftCandidates` into
	// `Nfts` the moment ALICE's attendance is decided.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 50,
			rounds: 1,
			max_group_size: 4,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));
		for acc in [ALICE, BOB, CHARLIE, DAVE] {
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(acc),
				DEFAULT_IDENTIFIER_KEY,
				None,
			));
		}
		advance_to_reporting_phase(&schedule);

		let alice = AccountOrPerson::Account(ALICE);
		let bob = AccountOrPerson::Account(BOB);
		let charlie = AccountOrPerson::Account(CHARLIE);
		let dave = AccountOrPerson::Account(DAVE);

		assert_ok!(Game::report(
			RuntimeOrigin::signed(ALICE),
			build_report_with_opinion(&alice, |_| Report::Person),
		));
		assert_ok!(Game::report(
			RuntimeOrigin::signed(BOB),
			build_report_with_opinion(&bob, |target| if target == &alice {
				Report::NotPerson
			} else {
				Report::Person
			},),
		));
		assert_ok!(Game::report(
			RuntimeOrigin::signed(CHARLIE),
			build_report_with_opinion(&charlie, |_| Report::Person),
		));

		// After ALICE/BOB/CHARLIE: ALICE has yes=1 (CHARLIE), no=1 (BOB), remaining=1.
		// Still Pending — the staged NotPerson NFT lives in `NftCandidates`, not yet
		// in `Nfts`.
		let nft_no_from_bob = Game::compute_nft(1, 0, &bob, &alice);
		assert_eq!(Players::<Test>::get(&alice).unwrap().early_attendance_enactment, None);
		assert!(NftCandidates::<Test>::contains_key(&alice, nft_no_from_bob));
		assert!(!Nfts::<Test>::contains_key(&alice, nft_no_from_bob));

		// DAVE's Person vote tips ALICE to Attended (2 yes, 1 no, no remaining).
		// Early-enactment runs and promotes ALICE's staged NotPerson NFT into
		// `Nfts`.
		assert_ok!(Game::report(
			RuntimeOrigin::signed(DAVE),
			build_report_with_opinion(&dave, |_| Report::Person),
		));
		assert!(matches!(
			Players::<Test>::get(&alice).unwrap().early_attendance_enactment,
			Some(EarlyAttendanceEnactment { attendance: true, .. })
		));
		assert!(Nfts::<Test>::contains_key(&alice, nft_no_from_bob));
		assert!(!NftCandidates::<Test>::contains_key(&alice, nft_no_from_bob));
	});
}

#[test]
fn notperson_nft_discarded_when_attestee_does_not_attend() {
	// BOB and CHARLIE both vote NotPerson on ALICE; ALICE never reports — she ends
	// up early-enacted as not attended, so the staged NotPerson NFTs against her are
	// dropped rather than minted.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 50,
			rounds: 1,
			max_group_size: 3,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));
		for acc in [ALICE, BOB, CHARLIE] {
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(acc),
				DEFAULT_IDENTIFIER_KEY,
				None,
			));
		}
		advance_to_reporting_phase(&schedule);

		let alice = AccountOrPerson::Account(ALICE);
		let bob = AccountOrPerson::Account(BOB);
		let charlie = AccountOrPerson::Account(CHARLIE);

		let opinion = |target: &AccountOrPerson<AccountId32>| {
			if target == &alice {
				Report::NotPerson
			} else {
				Report::Person
			}
		};
		assert_ok!(Game::report(
			RuntimeOrigin::signed(BOB),
			build_report_with_opinion(&bob, opinion),
		));
		assert_ok!(Game::report(
			RuntimeOrigin::signed(CHARLIE),
			build_report_with_opinion(&charlie, opinion),
		));

		assert!(matches!(
			Players::<Test>::get(&alice).unwrap().early_attendance_enactment,
			Some(EarlyAttendanceEnactment { attendance: false, .. })
		));
		// Both NotPerson NFTs are gone — neither staged nor minted.
		assert_eq!(NftCandidates::<Test>::iter_prefix(&alice).count(), 0);
		assert_eq!(Nfts::<Test>::iter_prefix(&alice).count(), 0);
	});
}

#[test]
fn notperson_nft_routed_directly_when_attestee_already_attended() {
	// Four-player group, single round. ALICE/BOB/CHARLIE all report all-Person —
	// at that point ALICE is yes=2, no=0, remaining=1 → Attended early. DAVE then
	// reports NotPerson on ALICE: because ALICE is already enacted, the NotPerson
	// NFT skips the candidate stage and lands directly in `Nfts`.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 50,
			rounds: 1,
			max_group_size: 4,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));
		for acc in [ALICE, BOB, CHARLIE, DAVE] {
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(acc),
				DEFAULT_IDENTIFIER_KEY,
				None,
			));
		}
		advance_to_reporting_phase(&schedule);

		let alice = AccountOrPerson::Account(ALICE);
		let bob = AccountOrPerson::Account(BOB);
		let charlie = AccountOrPerson::Account(CHARLIE);
		let dave = AccountOrPerson::Account(DAVE);

		assert_ok!(Game::report(
			RuntimeOrigin::signed(ALICE),
			build_report_with_opinion(&alice, |_| Report::Person),
		));
		assert_ok!(Game::report(
			RuntimeOrigin::signed(BOB),
			build_report_with_opinion(&bob, |_| Report::Person),
		));
		assert_ok!(Game::report(
			RuntimeOrigin::signed(CHARLIE),
			build_report_with_opinion(&charlie, |_| Report::Person),
		));
		assert!(matches!(
			Players::<Test>::get(&alice).unwrap().early_attendance_enactment,
			Some(EarlyAttendanceEnactment { attendance: true, .. })
		));

		assert_ok!(Game::report(
			RuntimeOrigin::signed(DAVE),
			build_report_with_opinion(&dave, |target| if target == &alice {
				Report::NotPerson
			} else {
				Report::Person
			},),
		));
		let nft_no_from_dave = Game::compute_nft(1, 0, &dave, &alice);
		assert!(Nfts::<Test>::contains_key(&alice, nft_no_from_dave));
		assert!(!NftCandidates::<Test>::contains_key(&alice, nft_no_from_dave));
	});
}

#[test]
fn early_enactment_pending_with_partial_votes() {
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 20,
			rounds: 1,
			max_group_size: 4,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));
		for acc in [ALICE, BOB, CHARLIE, DAVE] {
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(acc),
				DEFAULT_IDENTIFIER_KEY,
				None,
			));
		}
		advance_to_reporting_phase(&schedule);

		// Only BOB reports; ALICE has received 1 of 3 possible votes.
		assert_ok!(Game::report(
			RuntimeOrigin::signed(BOB),
			build_report_with_opinion(&AccountOrPerson::Account(BOB), |_| Report::Person),
		));

		let alice_info = Players::<Test>::get(AccountOrPerson::Account(ALICE)).unwrap();
		assert_eq!(alice_info.yes_person, 1);
		assert_eq!(alice_info.early_attendance_enactment, None);
	});
}

#[test]
fn pending_attendance_counts_registered_only() {
	new_test_ext().execute_with(|| {
		// Game 1: three players attend so they remain in `Players` with `registered = false`.
		let schedule1 = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 3,
			..Default::default()
		};
		run_game_scenario(
			schedule1,
			&[
				AccountOrPerson::Account(ALICE),
				AccountOrPerson::Account(BOB),
				AccountOrPerson::Account(CHARLIE),
			],
			|_| {
				Some(
					vec![vec![Report::Person, Report::Person].try_into().unwrap()]
						.try_into()
						.unwrap(),
				)
			},
		);
		for acc in [ALICE, BOB, CHARLIE] {
			let p = Players::<Test>::get(AccountOrPerson::Account(acc))
				.expect("player still in Players after game 1");
			assert!(!p.registered);
		}

		// Game 2: only DAVE and EVE sign up. The three non-registered players linger
		// in `Players` but must not contribute to `pending_attendance`.
		let schedule2 = GameSchedule::<u32, u128> {
			game_play_time: 30,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule2));
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(DAVE),
			DEFAULT_IDENTIFIER_KEY,
			None
		));
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(EVE),
			DEFAULT_IDENTIFIER_KEY,
			None
		));
		advance_to_reporting_phase(&schedule2);

		assert_eq!(crate::Game::<Test>::get().unwrap().pending_attendance, 2);
	});
}

#[test]
fn reporting_ends_early_when_all_attendance_determined() {
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 20,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(ALICE),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(BOB),
			DEFAULT_IDENTIFIER_KEY,
			None
		));
		advance_to_reporting_phase(&schedule);

		let report_ends = GameTimes::<Test>::reporting_end(&schedule);

		// Both players report the other Person; both become early-enacted Attended.
		let report: FullReport<Test> =
			vec![vec![Report::Person].try_into().unwrap()].try_into().unwrap();
		assert_ok!(Game::report(RuntimeOrigin::signed(ALICE), report.clone()));
		assert_ok!(Game::report(RuntimeOrigin::signed(BOB), report));

		let game = crate::Game::<Test>::get().unwrap();
		assert_eq!(game.pending_attendance, 0);
		assert!(
			matches!(game.state, GameState::Reporting { .. }),
			"report calls don't flip state on their own"
		);

		// Time must remain strictly below `report_ends` so that the state
		// transition below can only come from the `pending_attendance == 0` branch.
		let time_before = MOCK_UNIX_TIME.with(|t| *t.borrow()).as_secs();
		assert!(time_before < report_ends as u64);

		// Run to completion without ever advancing time past `report_ends`.
		while crate::Game::<Test>::get().is_some() {
			advance_process();
		}
		let time_after = MOCK_UNIX_TIME.with(|t| *t.borrow()).as_secs();
		assert!(
			time_after < report_ends as u64,
			"the full game must finish before report_ends — proves the early-exit branch fired",
		);

		// Both players attended in the new attendance history.
		let idx = GameIndex::<Test>::get();
		for acc in [ALICE, BOB] {
			assert_eq!(
				PlayerAttendanceHistory::<Test>::get(AccountOrPerson::Account(acc)).into_inner(),
				vec![idx],
			);
		}
	});
}

#[test]
fn player_process_step2_uses_marginal_weight_for_follow_up_chunks() {
	new_test_ext().execute_with(|| {
		// Matches `mock::Test`'s `Config::MaxRounds = ConstUint<10>`.
		let rounds = 10u8;
		for i in 0..(PLAYER_PROCESS_STEP2_CHUNK + 1) {
			let mut alias = [0u8; 32];
			let i_bytes = i.to_le_bytes();
			alias[..i_bytes.len()].copy_from_slice(&i_bytes);
			let account_or_person = AccountOrPerson::Person(alias);

			let indices =
				BoundedVec::try_from(vec![i; rounds as usize]).expect("rounds within bound");
			PlayerToIndex::<Test>::insert(&account_or_person, indices);
			IndexToPlayer::<Test>::insert((0u8, i), &account_or_person);
		}

		crate::Game::<Test>::put(GameInfo {
			index: 0,
			registration_ends: 0,
			shuffle_deadline: 0,
			game_date: 0,
			report_ends: 0,
			state: GameState::PlayerProcess { step: PlayerProcessStep::Step2ClearIndices },
			max_group_size: <Test as Config>::MaxGroupSize::get(),
			rounds,
			pending_attendance: 0,
			airdrop_scheduled: false,
		});

		let first_chunk_weight = <MockWeightInfo as WeightInfo>::player_process_step2();
		let marginal_chunk_weight =
			<MockWeightInfo as WeightInfo>::player_process_step2_inner_loop();
		let mut meter =
			WeightMeter::with_limit(first_chunk_weight.saturating_add(marginal_chunk_weight));

		Game::player_process_step2(&mut meter);

		assert!(crate::Game::<Test>::get().is_none());
		assert_eq!(IndexToPlayer::<Test>::iter().count(), 0);
		assert_eq!(PlayerToIndex::<Test>::iter().count(), 0);
	});
}

#[test]
fn vote_weight_snapshotted_during_shuffle() {
	new_test_ext().execute_with(|| {
		PeopleVoteWeight::set(&3u8);
		CandidateVoteWeight::set(&2u8);

		let alias: Alias = [10u8; 32];
		let stmt_acc = id_to_account(10);
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 20,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));
		assert_ok!(Game::sign_up_with_alias(
			runtime_origin_for_alias(&alias),
			DEFAULT_IDENTIFIER_KEY,
			stmt_acc.clone(),
			AccountAuthority(stmt_acc),
			None,
		));
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(ALICE),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));
		advance_to_reporting_phase(&schedule);

		let alias_info = Players::<Test>::get(AccountOrPerson::Person(alias)).unwrap();
		let candidate_info = Players::<Test>::get(AccountOrPerson::Account(ALICE)).unwrap();

		assert_eq!(alias_info.vote_weight, 3, "alias is externally recognized");
		assert_eq!(candidate_info.vote_weight, 2, "account is a candidate");
	});
}

#[test]
fn expected_max_vote_weight_is_sum_over_coplayers() {
	new_test_ext().execute_with(|| {
		PeopleVoteWeight::set(&3u8);
		CandidateVoteWeight::set(&2u8);

		let alias: Alias = [10u8; 32];
		let stmt_acc = id_to_account(10);
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 20,
			rounds: 2,
			max_group_size: 3,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));
		assert_ok!(Game::sign_up_with_alias(
			runtime_origin_for_alias(&alias),
			DEFAULT_IDENTIFIER_KEY,
			stmt_acc.clone(),
			AccountAuthority(stmt_acc),
			None,
		));
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(ALICE),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(BOB),
			DEFAULT_IDENTIFIER_KEY,
			None
		));
		advance_to_reporting_phase(&schedule);

		// 3 players in one group, 2 rounds. Every player's co-players are always
		// the other two.
		//   alias (vw=3): co-players (2 + 2) per round, 2 rounds => 8
		//   ALICE (vw=2): co-players (3 + 2) per round, 2 rounds => 10
		//   BOB   (vw=2): co-players (3 + 2) per round, 2 rounds => 10
		let alias_info = Players::<Test>::get(AccountOrPerson::Person(alias)).unwrap();
		let alice_info = Players::<Test>::get(AccountOrPerson::Account(ALICE)).unwrap();
		let bob_info = Players::<Test>::get(AccountOrPerson::Account(BOB)).unwrap();

		assert_eq!(alias_info.expected_max_vote_weight, (2 + 2) * 2);
		assert_eq!(alice_info.expected_max_vote_weight, (3 + 2) * 2);
		assert_eq!(bob_info.expected_max_vote_weight, (3 + 2) * 2);
	});
}

#[test]
fn reporter_vote_weight_read_from_stored_snapshot() {
	new_test_ext().execute_with(|| {
		PeopleVoteWeight::set(&5u8);
		CandidateVoteWeight::set(&2u8);

		let alias: Alias = [77u8; 32];
		let stmt_acc = id_to_account(77);
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 20,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));
		// Externally recognized reporter.
		assert_ok!(Game::sign_up_with_alias(
			runtime_origin_for_alias(&alias),
			DEFAULT_IDENTIFIER_KEY,
			stmt_acc.clone(),
			AccountAuthority(stmt_acc),
			None,
		));
		// Account-based target.
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(ALICE),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));
		advance_to_reporting_phase(&schedule);

		let alias_key = AccountOrPerson::Person(alias);
		// Sanity: alias is live-recognized and its snapshot is PeopleVoteWeight.
		assert!(Score::reached_personhood(&alias_key));
		assert_eq!(Players::<Test>::get(&alias_key).unwrap().vote_weight, 5);

		// Overwrite the snapshot to diverge from live recognition. Also lift the
		// target's `expected_max_vote_weight` so the received weight isn't clipped
		// by the early-enactment bound.
		Players::<Test>::mutate(&alias_key, |p| {
			p.as_mut().unwrap().vote_weight = 2;
		});
		let alice_key = AccountOrPerson::Account(ALICE);
		Players::<Test>::mutate(&alice_key, |p| {
			p.as_mut().unwrap().expected_max_vote_weight = u16::MAX;
		});

		// Alias reports ALICE as Person. If the reporter weight comes from the
		// snapshot, ALICE.yes_person == 2. If it came from live `reached_personhood`,
		// it would be 5.
		let report: FullReport<Test> =
			vec![vec![Report::Person].try_into().unwrap()].try_into().unwrap();
		assert_ok!(Game::report(runtime_origin_for_alias(&alias), report));

		let alice_info = Players::<Test>::get(&alice_key).unwrap();
		assert_eq!(
			alice_info.yes_person, 2,
			"reporter_vote_weight must come from the stored snapshot",
		);
	});
}

// With realistic parameters (8 players, max_group_size=8, rounds=6,
// PeopleVoteWeight=7) the per-player raw sum is `7 * 7 * 6 = 294`, which
// exceeds u8 range. The field is stored as `u16`, so the value must be
// recorded exactly without truncation.
#[test]
fn expected_max_vote_weight_does_not_truncate() {
	new_test_ext().execute_with(|| {
		PeopleVoteWeight::set(&7u8);
		CandidateVoteWeight::set(&1u8);

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 20,
			rounds: 6,
			max_group_size: 8,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));

		let mut aliases: Vec<Alias> = vec![];
		for i in 0u64..8 {
			let alias: Alias = [(20 + i) as u8; 32];
			let stmt_acc = id_to_account(20 + i);
			assert_ok!(Game::sign_up_with_alias(
				runtime_origin_for_alias(&alias),
				DEFAULT_IDENTIFIER_KEY,
				stmt_acc.clone(),
				AccountAuthority(stmt_acc),
				None,
			));
			aliases.push(alias);
		}
		advance_to_reporting_phase(&schedule);

		// Each player has 7 co-players per round, weight 7, over 6 rounds.
		let expected_raw_sum: u32 = 7 * 7 * 6;
		for alias in &aliases {
			let info = Players::<Test>::get(AccountOrPerson::Person(*alias)).unwrap();
			assert_eq!(info.expected_max_vote_weight as u32, expected_raw_sum);
		}
	});
}

#[test]
fn report_post_dispatch_refunds_unused_weight() {
	use frame_support::dispatch::{GetDispatchInfo, Pays};
	use sp_runtime::Weight;

	new_test_ext().execute_with(|| {
		// Lone-reporter game. With `rounds = 1, max_group_size = 1` the reporter
		// has no co-players, so `reported_players` is empty and the only enactment
		// candidate is the reporter themselves. This forces a large gap between the
		// pre-dispatch overcharge (worst case = `max_enactments()`) and the
		// post-dispatch actual cost (1 enactment), and we can observe the refund.
		let alias = id_to_alias(1001);
		let stmt_account = id_to_account(2001);
		let empty_report: FullReport<Test> =
			vec![BoundedVec::<Report, <Test as Config>::MaxGroupSize>::default()]
				.try_into()
				.unwrap();

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 1,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));
		assert_ok!(Game::sign_up_with_alias(
			runtime_origin_for_alias(&alias),
			DEFAULT_IDENTIFIER_KEY,
			stmt_account.clone(),
			AccountAuthority(stmt_account.clone()),
			None,
		));

		// Advance to reporting phase.
		let reg_end = GameTimes::<Test>::registration_end(&schedule);
		MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs((reg_end + 1) as u64));
		advance_process(); // registration -> shuffle
		advance_process(); // shuffle -> reporting

		// Pre-condition: no player has been early-enacted yet.
		assert_eq!(
			Players::<Test>::iter()
				.filter(|(_, p)| p.early_attendance_enactment.is_some())
				.count(),
			0,
		);

		let upfront_weight = crate::Call::<Test>::report { full_report: empty_report.clone() }
			.get_dispatch_info()
			.call_weight;
		let max_p = Pallet::<Test>::max_enactments();
		assert_eq!(upfront_weight, Weight::from_parts(max_p as u64, max_p as u64));
		assert_eq!(upfront_weight, <MockWeightInfo as WeightInfo>::report(max_p));

		let post_info = Game::report(runtime_origin_for_alias(&alias), empty_report)
			.expect("report should succeed");

		assert_eq!(post_info.pays_fee, Pays::No);

		// Count the actual full enactments by inspecting `Players` storage.
		// 1-player game the reporter's `determine_attendance` saturates to
		// `Attended` (remaining vote weight is 0), so exactly one enactment fires
		let actual_enacted = Players::<Test>::iter()
			.filter(|(_, p)| p.early_attendance_enactment.is_some())
			.count() as u32;
		assert_eq!(actual_enacted, 1, "1-player game must produce exactly 1 enactment");

		// Post-dispatch actual weight: `MockWeightInfo::report(1) = (1, 1)`.
		let actual_weight = post_info.actual_weight.expect("post-dispatch weight must be set");
		assert_eq!(actual_weight, <MockWeightInfo as WeightInfo>::report(actual_enacted));
		assert_eq!(actual_weight, Weight::from_parts(1, 1));

		assert!(
			actual_weight.all_lt(upfront_weight),
			"actual weight {actual_weight:?} must be strictly less than upfront {upfront_weight:?}",
		);
		let refund = upfront_weight.saturating_sub(actual_weight);
		assert_eq!(refund, Weight::from_parts((max_p - 1) as u64, (max_p - 1) as u64));
	});
}

mod kill_current_game {
	use super::*;
	use sp_runtime::DispatchError;

	#[test]
	fn manager_origin_kills_running_game() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = crate::GameIndex::<Test>::get();
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(ALICE),
				DEFAULT_IDENTIFIER_KEY,
				None,
			));
			assert!(crate::Game::<Test>::get().is_some());

			assert_ok!(Game::kill_current_game(RuntimeOrigin::root()));

			assert!(crate::Game::<Test>::get().is_none());
			// The signed-up player should no longer be marked as registered.
			let player = AccountOrPerson::<AccountId32>::Account(ALICE);
			let info = crate::Players::<Test>::get(&player).expect("player record retained");
			assert!(!info.registered);
			// `GameKilled` event is emitted with the index of the killed game.
			System::assert_last_event(
				crate::Event::<Test>::GameKilled { index: game_index }.into(),
			);
		});
	}

	#[test]
	fn signed_origin_is_rejected() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			assert_ok!(Game::new_game(&schedule));

			assert_noop!(
				Game::kill_current_game(RuntimeOrigin::signed(ALICE)),
				DispatchError::BadOrigin,
			);

			// The game must still be in storage.
			assert!(crate::Game::<Test>::get().is_some());
		});
	}

	#[test]
	fn errors_when_no_game_exists() {
		new_test_ext().execute_with(|| {
			assert!(crate::Game::<Test>::get().is_none());

			// Nothing to kill — but the call still succeeds (no-op cleanup).
			assert_ok!(Game::kill_current_game(RuntimeOrigin::root()));
		});
	}
}

mod set_game_phases {
	use super::*;
	use crate::{Game as GameStorage, PhaseDurationValues};
	use sp_runtime::DispatchError;

	fn distinct_phases() -> PhaseDurationValues {
		PhaseDurationValues {
			registration: 7,
			shuffle: 11,
			post_shuffle_margin: 13,
			reporting: 17,
			player_process: 19,
			airdrop_claim_window: 23,
		}
	}

	fn put_game_in_state(state: GameState<AccountId32>) {
		GameStorage::<Test>::put(GameInfo {
			index: 1,
			registration_ends: 0,
			shuffle_deadline: 0,
			game_date: 0,
			report_ends: 0,
			state,
			max_group_size: 3,
			rounds: 1,
			pending_attendance: 0,
			airdrop_scheduled: false,
		});
	}

	#[test]
	fn manager_origin_sets_phases_and_emits_event() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			assert!(StoredPhaseDurations::<Test>::get().is_none());

			let phases = distinct_phases();
			assert_ok!(Game::set_game_phases(RuntimeOrigin::root(), phases.clone()));

			assert_eq!(StoredPhaseDurations::<Test>::get(), Some(phases.clone()));
			System::assert_last_event(Event::<Test>::GamePhasesSet { phases }.into());
		});
	}

	#[test]
	fn signed_origin_is_rejected() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				Game::set_game_phases(RuntimeOrigin::signed(ALICE), distinct_phases()),
				DispatchError::BadOrigin,
			);
			assert!(StoredPhaseDurations::<Test>::get().is_none());
		});
	}

	#[test]
	fn override_propagates_to_game_scheduling() {
		new_test_ext().execute_with(|| {
			// Pick durations that are clearly distinguishable from the
			// chain-default `GamePhaseDurations` so the assertion below
			// would fail if `configured_phases()` ignored the override.
			let phases = PhaseDurationValues {
				registration: 1,
				shuffle: 1,
				post_shuffle_margin: 1,
				reporting: 1,
				player_process: 1,
				airdrop_claim_window: 1,
			};
			assert_ok!(Game::set_game_phases(RuntimeOrigin::root(), phases));

			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 1_000,
				rounds: 2,
				max_group_size: 3,
				..Default::default()
			};
			assert_ok!(Game::new_game(&schedule));

			let game = GameStorage::<Test>::get().expect("game exists");
			// With our overrides:
			//   registration_ends = game_play_time - shuffle - post_shuffle_margin = 998
			//   report_ends       = game_play_time + reporting                     = 1001
			assert_eq!(game.registration_ends, 998);
			assert_eq!(game.report_ends, 1_001);
		});
	}

	#[test]
	fn registration_phase_allows_override() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			put_game_in_state(GameState::Registration { next_player_index: 0 });

			let phases = distinct_phases();
			assert_ok!(Game::set_game_phases(RuntimeOrigin::root(), phases.clone()));
			assert_eq!(StoredPhaseDurations::<Test>::get(), Some(phases.clone()));
			System::assert_last_event(Event::<Test>::GamePhasesSet { phases }.into());
		});
	}

	#[test]
	fn shuffle_phase_rejects_override() {
		new_test_ext().execute_with(|| {
			put_game_in_state(GameState::Shuffle {
				step: ShuffleStep::Step1Insert { last_iteration: None },
			});
			assert_noop!(
				Game::set_game_phases(RuntimeOrigin::root(), distinct_phases()),
				Error::<Test>::InvalidGameState,
			);
		});
	}

	#[test]
	fn reporting_phase_rejects_override() {
		new_test_ext().execute_with(|| {
			put_game_in_state(GameState::Reporting { player_count: 0 });
			assert_noop!(
				Game::set_game_phases(RuntimeOrigin::root(), distinct_phases()),
				Error::<Test>::InvalidGameState,
			);
		});
	}

	#[test]
	fn player_process_phase_rejects_override() {
		new_test_ext().execute_with(|| {
			put_game_in_state(GameState::PlayerProcess {
				step: PlayerProcessStep::Step2ClearIndices,
			});
			assert_noop!(
				Game::set_game_phases(RuntimeOrigin::root(), distinct_phases()),
				Error::<Test>::InvalidGameState,
			);
		});
	}

	#[test]
	fn cancelling_phase_rejects_override() {
		new_test_ext().execute_with(|| {
			put_game_in_state(GameState::Cancelling { last_iteration: None });
			assert_noop!(
				Game::set_game_phases(RuntimeOrigin::root(), distinct_phases()),
				Error::<Test>::InvalidGameState,
			);
		});
	}
}

mod airdrop {
	use super::*;
	use crate::{extension::CustomError, AirdropVrf, Game as GameStorage};
	use codec::{Decode, Encode, MaxEncodedLen};
	use frame_support::traits::fungibles::{Inspect, Mutate, MutateHold};
	use indiv_pallet_airdrop::{
		types::{ActiveEvent, EventInfo, RegistrationEntry, Status},
		BigEndianU256, Events as AirdropEvents, Registrations as AirdropRegistrations,
		SupportedAssets as AirdropSupportedAssets, Winners as AirdropWinners,
	};
	use indiv_pallet_score::{AttendanceHistory, Participant, Recognition, Streak};
	use mock::AssetsWithHolder;
	use sp_core::sr25519::vrf::VrfSignature;

	fn dummy_vrf() -> VrfSignature {
		let bytes = vec![0u8; VrfSignature::max_encoded_len()];
		VrfSignature::decode(&mut &bytes[..]).expect("zero bytes decode to a VrfSignature")
	}

	fn account_proof(
	) -> AirdropVrf<<verifiable::mock::Mock as verifiable::GenerateVerifiable>::Proof> {
		AirdropVrf::Account(dummy_vrf())
	}

	fn alias_proof() -> AirdropVrf<<verifiable::mock::Mock as verifiable::GenerateVerifiable>::Proof>
	{
		AirdropVrf::Alias { proof: Default::default(), ring_index: 0, revision: 0 }
	}

	fn force_recognition(player: AccountOrPerson<AccountId32>, recognition: Recognition) {
		force_participant(player, recognition, None);
	}

	fn force_participant(
		player: AccountOrPerson<AccountId32>,
		recognition: Recognition,
		last_attended_game: Option<u32>,
	) {
		let reached =
			matches!(recognition, Recognition::Recognized(_) | Recognition::ExternallyRecognized,);
		indiv_pallet_score::Participants::<Test>::insert(
			player,
			Participant {
				score: 0,
				streak: Streak::Attended(0),
				attendance_history: AttendanceHistory::default(),
				credit: 0,
				cashed_out: false,
				reached_personhood: reached,
				has_ever_reached_personhood: false,
				recognition,
				last_attended_game,
			},
		);
	}

	/// Disable the airdrop's prize asset so the next `T::Airdrop::schedule` call
	/// returns an error. The game pallet should swallow this error and still
	/// create the game.
	fn break_airdrop_schedule() {
		AirdropSupportedAssets::<Test>::remove(TEST_AIRDROP_ASSET_ID);
	}

	/// Mutate the status of an existing airdrop event in place.
	fn set_event_status(event_id: indiv_pallet_airdrop::types::EventId, status: Status) {
		AirdropEvents::<Test>::mutate(event_id, |maybe_event| {
			let event = maybe_event.as_mut().expect("event exists");
			event.status = status;
		});
	}

	/// Move the airdrop event for `game_index` directly into `Status::Claiming` and
	/// stage a winning entry for `registrant` so a `claim_airdrop` call lands.
	fn stage_claim_for(game_index: u32, registrant: RegistrationEntry<AccountId32>) {
		let event_id = Game::airdrop_event_id(game_index);
		let info = EventInfo {
			prize: test_airdrop_prize(),
			registration_starts: 0,
			draw_time: 0,
			end_time: u64::MAX,
		};
		AirdropEvents::<Test>::insert(
			event_id,
			ActiveEvent {
				id: event_id,
				info,
				status: Status::Claiming {
					total_participants: 1,
					effective_winners: 1,
					claimed: 0,
				},
				source: None,
			},
		);
		AirdropWinners::<Test>::insert(event_id, &registrant, BigEndianU256::from([0u8; 32]));
		// Hold the prize amount on the pot so `do_claim`'s `release` succeeds.
		let pot = indiv_pallet_airdrop::Pallet::<Test>::airdrop_pot_id();
		let amount = test_airdrop_prize().asset_amount;
		<AssetsWithHolder as Mutate<AccountId32>>::mint_into(TEST_AIRDROP_ASSET_ID, &pot, amount)
			.expect("mint prize into pot");
		<AssetsWithHolder as MutateHold<AccountId32>>::hold(
			TEST_AIRDROP_ASSET_ID,
			&indiv_pallet_airdrop::HoldReason::Airdrop.into(),
			&pot,
			amount,
		)
		.expect("hold prize on pot");
	}

	#[test]
	fn new_game_schedules_airdrop_with_correct_timing_and_event_id() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			let now = <Test as crate::Config>::UnixTime::now().as_secs();
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			let event_id = Game::airdrop_event_id(game_index);
			let event = AirdropEvents::<Test>::get(event_id).expect("event scheduled");
			assert_eq!(event.info.registration_starts, now);
			assert_eq!(event.info.draw_time, GameTimes::<Test>::game_play_time(&schedule) as u64);
			// end_time = game_play_time + reporting + the chain-configured `airdrop_claim_window`.
			let game = GameStorage::<Test>::get().expect("game exists");
			assert!(game.airdrop_scheduled);
			assert_eq!(event.info.end_time, 10 + 2 + TEST_AIRDROP_CLAIM_WINDOW);
			// The schedule call carried the schedule's airdrop_prize.
			assert_eq!(event.info.prize, test_airdrop_prize());
		});
	}

	#[test]
	fn airdrop_registration_opens_at_now_not_game_registration_start() {
		// The airdrop registration must open immediately when the game registration starts.
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 100,
				rounds: 2,
				max_group_size: 3,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			// `now` is set strictly before the game's registration phase starts, so the two
			// timestamps are distinct and the test can tell them apart.
			let now = 10;
			MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs(now));
			// This is actually the latest time the game can start.
			let game_registration_start = GameTimes::<Test>::registration_start(&schedule);
			assert!(now < game_registration_start as u64);

			// Queue the game and let the automation start it: `on_poll`/`on_idle` picks up the
			// schedule and calls `new_game` itself.
			assert_ok!(Game::schedule_games(RuntimeOrigin::root(), vec![schedule.clone()]));
			advance_process();
			assert!(crate::Game::<Test>::get().is_some(), "automation started the game");
			assert!(GameSchedules::<Test>::get().is_empty(), "schedule consumed by automation");

			let game_index = GameIndex::<Test>::get();
			let event = AirdropEvents::<Test>::get(Game::airdrop_event_id(game_index))
				.expect("event scheduled");
			// Airdrop registration opens at `now`.
			assert_eq!(event.info.registration_starts, now);
			// The draw still happens at game play time.
			assert_eq!(event.info.draw_time, schedule.game_play_time as u64);
		});
	}

	#[test]
	fn schedule_failure_does_not_fail_new_game() {
		new_test_ext().execute_with(|| {
			break_airdrop_schedule();
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			assert_ok!(Game::new_game(&schedule));
			assert!(crate::Game::<Test>::exists());
			let game = GameStorage::<Test>::get().expect("game exists");
			// Scheduling was attempted but failed, so the event isn't in airdrop storage and
			// `airdrop_scheduled` is `false`.
			assert!(!game.airdrop_scheduled);
			assert!(AirdropEvents::<Test>::get(Game::airdrop_event_id(game.index)).is_none());
		});
	}

	#[test]
	fn sign_up_with_proof_when_schedule_failed_succeeds_and_skips_participate() {
		// If `new_game` fails to schedule the airdrop, sign-up with an airdrop proof must
		// still succeed — the proof is silently ignored (no participate dispatch), and the
		// player is recorded in `Players` as usual.
		new_test_ext().execute_with(|| {
			break_airdrop_schedule();
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			assert_ok!(Game::new_game(&schedule));
			let event_id = Game::airdrop_event_id(GameIndex::<Test>::get());
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(ALICE),
				DEFAULT_IDENTIFIER_KEY,
				Some(account_proof()),
			));
			assert!(Players::<Test>::get(AccountOrPerson::Account(ALICE)).is_some());
			// No participate happened: nothing in `Registrations` for this event.
			assert!(AirdropRegistrations::<Test>::iter_prefix(event_id).next().is_none());
		});
	}

	#[test]
	fn sign_up_with_account_proof_dispatches_participate_with_account() {
		use sp_core::{crypto::VrfSecret, sr25519, Pair as _};

		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			assert_ok!(Game::new_game(&schedule));
			let event_id = Game::airdrop_event_id(GameIndex::<Test>::get());

			// `participate_with_account` only accepts events in `Status::Registering`. The
			// game scheduled the event in `Scheduled`; flip it so the call lands.
			set_event_status(event_id, Status::Registering { total_participants: 0 });

			// Build a real sr25519 VRF signature for ALICE. Register the keypair's pubkey
			// for ALICE so `AccountIdToPublic` resolves to the pair we sign with.
			let pair = sr25519::Pair::from_seed(b"alice_vrf_seed_____padding______");
			register_account_pubkey(ALICE, pair.public());
			let signature = pair.vrf_sign(
				&indiv_pallet_airdrop::vrf::transcript_for_event(&event_id, &pair.public())
					.into_sign_data(),
			);

			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(ALICE),
				DEFAULT_IDENTIFIER_KEY,
				Some(AirdropVrf::Account(signature.clone())),
			));

			// The participate dispatch landed: a `Registrations` slot was inserted for ALICE
			// under the entropy that `do_participate_with_account` derived from the VRF.
			let entropy = indiv_pallet_airdrop::vrf::verify_and_extract_entropy(
				&pair.public(),
				&event_id,
				&signature,
			)
			.expect("signature verifies");
			let slot = indiv_pallet_airdrop::BigEndianU256::from(entropy);
			assert_eq!(
				AirdropRegistrations::<Test>::get(event_id, slot),
				Some(indiv_pallet_airdrop::types::RegistrationEntry::Account { account_id: ALICE }),
			);
		});
	}

	#[test]
	fn sign_up_with_alias_proof_dispatches_participate_with_alias() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			assert_ok!(Game::new_game(&schedule));
			let event_id = Game::airdrop_event_id(GameIndex::<Test>::get());
			set_event_status(event_id, Status::Registering { total_participants: 0 });

			let alias: Alias = [0xAA; 32];
			let stmt_account = id_to_account(0xA11A5);
			assert_ok!(Game::sign_up_with_alias(
				runtime_origin_for_alias(&alias),
				DEFAULT_IDENTIFIER_KEY,
				stmt_account.clone(),
				AccountAuthority(stmt_account),
				Some(alias_proof()),
			));

			// `MockAirdropMemberService::verify_membership_at_rev` returns the blake2 of the
			// encoded participant_origin as the alias; look up that slot.
			let entry = RegistrationEntry::<AccountId32>::Alias { alias };
			let slot_alias: indiv_support::traits::Alias =
				sp_io::hashing::blake2_256(&entry.encode());
			let slot = BigEndianU256::from(slot_alias);
			assert_eq!(AirdropRegistrations::<Test>::get(event_id, slot), Some(entry));
		});
	}

	#[test]
	fn sign_up_rejects_alias_variant_for_non_recognized_player() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			assert_ok!(Game::new_game(&schedule));
			assert_noop!(
				Game::sign_up_with_account(
					RuntimeOrigin::signed(ALICE),
					DEFAULT_IDENTIFIER_KEY,
					Some(alias_proof()),
				),
				Error::<Test>::InvalidAirdropVrfVariantForRecognition,
			);
		});
	}

	#[test]
	fn sign_up_rejects_account_variant_for_recognized_player() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			assert_ok!(Game::new_game(&schedule));
			// Pre-recognized account player: in `Participants` as `Recognized(_)` and in
			// `ArchivedPlayers` so the sign-up's onboard step is skipped.
			force_recognition(AccountOrPerson::Account(ALICE), Recognition::Recognized(0));
			ArchivedPlayers::<Test>::insert(
				AccountOrPerson::Account(ALICE),
				ArchivedPlayer::Unkickable { first_game: 0 },
			);
			assert_noop!(
				Game::sign_up_with_account(
					RuntimeOrigin::signed(ALICE),
					DEFAULT_IDENTIFIER_KEY,
					Some(account_proof()),
				),
				Error::<Test>::InvalidAirdropVrfVariantForRecognition,
			);
		});
	}

	#[test]
	fn sign_up_participate_failure_rolls_back() {
		// If the airdrop pallet rejects the participation, the whole sign-up must roll
		// back — no `Players` row should land. ALICE's sr25519 public key is registered so
		// `do_participate_with_account` reaches `verify_and_extract_entropy`, which then
		// rejects the dummy VRF signature with `InvalidVrfProof`.
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			assert_ok!(Game::new_game(&schedule));
			let event_id = Game::airdrop_event_id(GameIndex::<Test>::get());
			register_account_pubkey(ALICE, sp_core::sr25519::Public::from([1u8; 32]));
			assert_noop!(
				Game::sign_up_with_account(
					RuntimeOrigin::signed(ALICE),
					DEFAULT_IDENTIFIER_KEY,
					Some(account_proof()),
				),
				indiv_pallet_airdrop::Error::<Test>::InvalidVrfProof,
			);
			assert!(Players::<Test>::get(AccountOrPerson::Account(ALICE)).is_none());
			// And the rejected proof left no registration behind in the airdrop pallet.
			assert!(AirdropRegistrations::<Test>::iter_prefix(event_id).next().is_none());
		});
	}

	/// `GameAsInvited::validate` runs `validate_register_for_airdrop`, which wraps the airdrop
	/// participate call in `with_transaction(Rollback(_))`.
	/// This test verifies it does indeed rollback.
	///
	/// Case: InvalidAirdropRegistration
	#[test]
	fn extension_validate_airdrop_failure_does_not_change_storage() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			assert_ok!(Game::new_game(&schedule));

			// ALICE's sr25519 pubkey is registered so `participate_with_account` reaches the
			// VRF check and rejects the dummy signature with `InvalidVrfProof` — surfaced by
			// the extension as `InvalidAirdropRegistration`.
			register_account_pubkey(ALICE, sp_core::sr25519::Public::from([1u8; 32]));

			let ticket = 42u64;
			let signature = TestSignature(ticket, ALICE.encode());
			assert_ok!(Game::grant_invites(RuntimeOrigin::root(), BOB, 1));
			assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(BOB), ticket));

			let nonce = frame_system::Account::<Test>::get(&ALICE).nonce;
			let tx_ext = GameAsInvitedData { nonce, inviter: BOB, ticket, signature };

			assert_noop!(
				exec_invited_tx(
					ALICE,
					tx_ext,
					Call::sign_up_with_invite {
						identifier_key: DEFAULT_IDENTIFIER_KEY,
						airdrop: Some(account_proof()),
					},
				),
				TransactionExecutionError::Validity(
					InvalidTransaction::Custom(CustomError::InvalidAirdropRegistration as u8)
						.into(),
				),
			);
		});
	}

	/// `GameAsInvited::validate` runs `validate_register_for_airdrop`, which wraps the airdrop
	/// participate call in `with_transaction(Rollback(_))`.
	/// This test verifies it does indeed rollback.
	///
	/// Case: InvalidAirdropVrfVariant
	#[test]
	fn extension_validate_wrong_vrf_variant_does_not_change_storage() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			assert_ok!(Game::new_game(&schedule));

			let ticket = 43u64;
			let signature = TestSignature(ticket, ALICE.encode());
			assert_ok!(Game::grant_invites(RuntimeOrigin::root(), BOB, 1));
			assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(BOB), ticket));

			let nonce = frame_system::Account::<Test>::get(&ALICE).nonce;
			let tx_ext = GameAsInvitedData { nonce, inviter: BOB, ticket, signature };

			// Alias proof for a non-recognized account ⇒ `InvalidAirdropVrfVariant`.
			assert_noop!(
				exec_invited_tx(
					ALICE,
					tx_ext,
					Call::sign_up_with_invite {
						identifier_key: DEFAULT_IDENTIFIER_KEY,
						airdrop: Some(alias_proof()),
					},
				),
				TransactionExecutionError::Validity(
					InvalidTransaction::Custom(CustomError::InvalidAirdropVrfVariant as u8).into(),
				),
			);
		});
	}

	/// `GameAsInvited::validate` runs `validate_register_for_airdrop`, which wraps the airdrop
	/// participate call in `with_transaction(Rollback(_))`.
	/// This test verifies it does indeed rollback.
	///
	/// Case: Successful
	#[test]
	fn extension_validate_success_rolls_back_airdrop_registration() {
		use frame_support::dispatch::GetDispatchInfo;
		use sp_core::{crypto::VrfSecret, sr25519, Pair as _};
		use sp_runtime::{
			traits::{Applyable, Checkable},
			transaction_validity::TransactionSource,
		};

		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			assert_ok!(Game::new_game(&schedule));
			let event_id = Game::airdrop_event_id(GameIndex::<Test>::get());
			// `participate_with_account` only accepts events in `Status::Registering`; the game
			// scheduled the event in `Scheduled`, flip it so the inner call lands successfully.
			set_event_status(event_id, Status::Registering { total_participants: 0 });

			// Real sr25519 keypair for ALICE so `participate_with_account` verifies the VRF.
			let pair = sr25519::Pair::from_seed(b"alice_vrf_seed_____padding______");
			register_account_pubkey(ALICE, pair.public());
			let signature = pair.vrf_sign(
				&indiv_pallet_airdrop::vrf::transcript_for_event(&event_id, &pair.public())
					.into_sign_data(),
			);

			let ticket = 44u64;
			let invite_sig = TestSignature(ticket, ALICE.encode());
			assert_ok!(Game::grant_invites(RuntimeOrigin::root(), BOB, 1));
			assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(BOB), ticket));

			let nonce = frame_system::Account::<Test>::get(&ALICE).nonce;
			let x = mock::Extrinsic::new_signed(
				Call::sign_up_with_invite {
					identifier_key: DEFAULT_IDENTIFIER_KEY,
					airdrop: Some(AirdropVrf::Account(signature.clone())),
				}
				.into(),
				ALICE,
				AccountAuthority(ALICE),
				(
					crate::GameAsInvited::<Test>::new(Some(GameAsInvitedData {
						nonce,
						inviter: BOB,
						ticket,
						signature: invite_sig,
					})),
					indiv_pallet_score::ScoreAsParticipant::<Test>::new(None),
					mock::DenyNotFundedAccount,
				),
			);
			let info = x.get_dispatch_info();
			let len = x.encoded_size();
			let checked =
				Checkable::check(x, &frame_system::ChainContext::<Test>::default()).unwrap();

			// Run validate.
			assert_ok!(checked.validate::<Test>(TransactionSource::External, &info, len));

			// `participate_with_account` would insert a `Registrations` entry on success; the
			// inner rollback must have reverted it.
			assert!(AirdropRegistrations::<Test>::iter_prefix(event_id).next().is_none());
		});
	}

	#[test]
	fn claim_rejects_when_never_attended() {
		new_test_ext().execute_with(|| {
			force_participant(AccountOrPerson::Account(ALICE), Recognition::Recognized(0), None);
			assert_noop!(
				Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 1, id_to_account(99)),
				Error::<Test>::NotEligibleForAirdrop,
			);
		});
	}

	#[test]
	fn claim_rejects_when_game_index_does_not_match_last_attended() {
		// Eligibility is `last_attended_game == Some(game_index)`. ALICE attended game 5;
		// claims for the previous (game 4) and the next (game 6) must both be rejected.
		new_test_ext().execute_with(|| {
			force_participant(AccountOrPerson::Account(ALICE), Recognition::Recognized(0), Some(5));

			// Previous game.
			stage_claim_for(4, RegistrationEntry::Account { account_id: ALICE });
			assert_noop!(
				Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 4, id_to_account(99)),
				Error::<Test>::NotEligibleForAirdrop,
			);

			// Next game.
			stage_claim_for(6, RegistrationEntry::Account { account_id: ALICE });
			assert_noop!(
				Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 6, id_to_account(99)),
				Error::<Test>::NotEligibleForAirdrop,
			);

			// Sanity: claiming game 5 — the actual `last_attended_game` — succeeds.
			stage_claim_for(5, RegistrationEntry::Account { account_id: ALICE });
			assert_ok!(Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 5, id_to_account(99)));
		});
	}

	#[test]
	fn claim_rejects_when_not_recognized() {
		new_test_ext().execute_with(|| {
			// Player attended game 1 but never reached recognition or personhood
			force_participant(AccountOrPerson::Account(ALICE), Recognition::NotRecognized, Some(1));
			assert_noop!(
				Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 1, id_to_account(99)),
				Error::<Test>::NotEligibleForAirdrop,
			);
		});
	}

	#[test]
	fn claim_succeeds_after_attended_account_vrf_signup() {
		// Account-based player who was NOT recognized at game start: they entered the
		// airdrop draw using the Account VRF variant of `sign_up_with_account`. They later
		// attended (so `last_attended_game == Some(game_index)`) and registered for personhood
		// (recognition flipped to `Recognized`); from this point they're eligible to claim.
		use sp_core::{crypto::VrfSecret, sr25519, Pair as _};

		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			let event_id = Game::airdrop_event_id(game_index);
			set_event_status(event_id, Status::Registering { total_participants: 0 });

			// Sign up with the Account VRF (player is `NotRecognized` at signup).
			let pair = sr25519::Pair::from_seed(b"alice_vrf_seed_____padding______");
			register_account_pubkey(ALICE, pair.public());
			let signature = pair.vrf_sign(
				&indiv_pallet_airdrop::vrf::transcript_for_event(&event_id, &pair.public())
					.into_sign_data(),
			);
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(ALICE),
				DEFAULT_IDENTIFIER_KEY,
				Some(AirdropVrf::Account(signature)),
			));

			// Player attended the game, then later acquired recognition via
			// `score::register`. We model both directly on the participant record.
			let who = AccountOrPerson::Account(ALICE);
			assert_ok!(indiv_pallet_score::Pallet::<Test>::set_attendance(&who, true, game_index));
			indiv_pallet_score::Participants::<Test>::mutate(&who, |maybe| {
				maybe.as_mut().unwrap().recognition = Recognition::Recognized(0);
			});

			// Stage the event as `Claiming` with ALICE as the sole winner, then claim.
			stage_claim_for(game_index, RegistrationEntry::Account { account_id: ALICE });
			assert_ok!(Game::claim_airdrop(
				RuntimeOrigin::signed(ALICE),
				game_index,
				id_to_account(99),
			));
			assert!(!AirdropWinners::<Test>::contains_key(
				event_id,
				RegistrationEntry::<AccountId32>::Account { account_id: ALICE },
			));
		});
	}

	#[test]
	fn claim_succeeds_after_attended_alias_vrf_signup() {
		// Account-based player who WAS already recognized at game start: they entered the
		// airdrop draw using the Alias VRF variant of `sign_up_with_account`. They attended
		// (so `last_attended_game == Some(game_index)`) and their recognition stayed `Recognized`.
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			let event_id = Game::airdrop_event_id(game_index);
			set_event_status(event_id, Status::Registering { total_participants: 0 });

			// Pre-recognized account player: in `Participants` as `Recognized(_)` and in
			// `ArchivedPlayers` so the sign-up's onboard step is skipped.
			force_participant(AccountOrPerson::Account(ALICE), Recognition::Recognized(0), None);
			ArchivedPlayers::<Test>::insert(
				AccountOrPerson::Account(ALICE),
				ArchivedPlayer::Unkickable { first_game: 0 },
			);

			// Sign up with the Alias VRF (only valid because the player is recognized).
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(ALICE),
				DEFAULT_IDENTIFIER_KEY,
				Some(alias_proof()),
			));

			// Player attended the game; recognition remains `Recognized`.
			assert_ok!(indiv_pallet_score::Pallet::<Test>::set_attendance(
				&AccountOrPerson::Account(ALICE),
				true,
				game_index,
			));

			stage_claim_for(game_index, RegistrationEntry::Account { account_id: ALICE });
			assert_ok!(Game::claim_airdrop(
				RuntimeOrigin::signed(ALICE),
				game_index,
				id_to_account(99),
			));
			assert!(!AirdropWinners::<Test>::contains_key(
				event_id,
				RegistrationEntry::<AccountId32>::Account { account_id: ALICE },
			));
		});
	}

	#[test]
	fn claim_succeeds_when_last_attended_externally_recognized() {
		new_test_ext().execute_with(|| {
			force_participant(
				AccountOrPerson::Account(ALICE),
				Recognition::ExternallyRecognized,
				Some(1),
			);
			stage_claim_for(1, RegistrationEntry::Account { account_id: ALICE });
			assert_ok!(Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 1, id_to_account(99),));
			assert!(!AirdropWinners::<Test>::contains_key(
				Game::airdrop_event_id(1),
				RegistrationEntry::<AccountId32>::Account { account_id: ALICE },
			));
		});
	}

	#[test]
	fn claim_forwards_account_registrant() {
		new_test_ext().execute_with(|| {
			force_participant(AccountOrPerson::Account(ALICE), Recognition::Recognized(0), Some(7));
			stage_claim_for(7, RegistrationEntry::Account { account_id: ALICE });
			let beneficiary = id_to_account(123);
			let prize_amount = test_airdrop_prize().asset_amount;
			assert_ok!(Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 7, beneficiary.clone(),));
			// Account-registrant entry consumed.
			assert!(!AirdropWinners::<Test>::contains_key(
				Game::airdrop_event_id(7),
				RegistrationEntry::<AccountId32>::Account { account_id: ALICE },
			));
			// Prize was paid to the beneficiary.
			assert_eq!(
				<Assets as Inspect<AccountId32>>::balance(TEST_AIRDROP_ASSET_ID, &beneficiary),
				prize_amount,
			);
		});
	}

	#[test]
	fn claim_person_origin_forwards_alias_registrant() {
		new_test_ext().execute_with(|| {
			let alias: Alias = [0xCC; 32];
			force_participant(
				AccountOrPerson::Person(alias),
				Recognition::ExternallyRecognized,
				Some(9),
			);
			stage_claim_for(9, RegistrationEntry::Alias { alias });
			let beneficiary = id_to_account(123);
			let prize_amount = test_airdrop_prize().asset_amount;
			assert_ok!(Game::claim_airdrop(
				runtime_origin_for_alias(&alias),
				9,
				beneficiary.clone(),
			));
			// Alias-registrant entry consumed.
			assert!(!AirdropWinners::<Test>::contains_key(
				Game::airdrop_event_id(9),
				RegistrationEntry::<AccountId32>::Alias { alias },
			));
			assert_eq!(
				<Assets as Inspect<AccountId32>>::balance(TEST_AIRDROP_ASSET_ID, &beneficiary),
				prize_amount,
			);
		});
	}

	#[test]
	fn claim_succeeds_when_reached_personhood_without_register() {
		// Full game-flow test: ALICE plays games until she reaches personhood (her score
		// climbs over the personhood threshold). She never calls `score::register`, so
		// her `recognition` stays `NotRecognized` even though `reached_personhood` is
		// true. She must still be allowed to claim — the eligibility check is
		// `(recognized() || reached_personhood) && last_attended_game == Some(game_index)`.
		new_test_ext().execute_with(|| {
			let alice = AccountOrPerson::Account(ALICE);

			let mut schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 1,
				max_group_size: 2,
				..Default::default()
			};
			run_game_scenario(schedule.clone(), slice::from_ref(&alice), |_| {
				Some(vec![Default::default()].try_into().unwrap())
			});
			schedule.game_play_time += 10;
			while !Score::reached_personhood(&alice) {
				run_game_scenario(schedule.clone(), slice::from_ref(&alice), |_| {
					Some(vec![Default::default()].try_into().unwrap())
				});
				schedule.game_play_time += 10;
			}

			// ALICE reached personhood through play, attended her last game, but never
			// registered with the score pallet — so she is still `NotRecognized`.
			let last_game_index = GameIndex::<Test>::get();
			let p = indiv_pallet_score::Participants::<Test>::get(&alice).unwrap();
			assert!(p.reached_personhood);
			assert_eq!(p.last_attended_game, Some(last_game_index));
			assert!(matches!(p.recognition, Recognition::NotRecognized));

			// Stage an airdrop event in `Claiming` with ALICE as the pre-registered
			// winner, then claim — without ever having called `score::register`. The claim
			// must target the same game index ALICE last attended.
			let event_id = Game::airdrop_event_id(last_game_index);
			stage_claim_for(last_game_index, RegistrationEntry::Account { account_id: ALICE });
			assert_ok!(Game::claim_airdrop(
				RuntimeOrigin::signed(ALICE),
				last_game_index,
				id_to_account(99),
			));
			assert!(!AirdropWinners::<Test>::contains_key(
				event_id,
				RegistrationEntry::<AccountId32>::Account { account_id: ALICE },
			));
		});
	}

	#[test]
	fn claim_rejected_for_cancelled_game() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 2,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			let event_id = Game::airdrop_event_id(game_index);

			MOCK_UNIX_TIME.with(|t| {
				*t.borrow_mut() =
					Duration::from_secs(GameTimes::<Test>::registration_start(&schedule) as u64)
			});
			assert_ok!(indiv_pallet_airdrop::Pallet::<Test>::start_registration_authorized(
				frame_system::RawOrigin::Authorized.into(),
				event_id,
				0,
			));

			let pair = sr25519::Pair::from_seed(b"alice_vrf_seed_____padding______");
			register_account_pubkey(ALICE, pair.public());
			let signature = pair.vrf_sign(
				&indiv_pallet_airdrop::vrf::transcript_for_event(&event_id, &pair.public())
					.into_sign_data(),
			);
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(ALICE),
				DEFAULT_IDENTIFIER_KEY,
				Some(AirdropVrf::Account(signature)),
			));

			MOCK_UNIX_TIME.with(|t| {
				*t.borrow_mut() =
					Duration::from_secs(GameTimes::<Test>::registration_end(&schedule) as u64)
			});
			advance_process(); // registration -> shuffle
			advance_process(); // shuffle -> reporting
			assert!(matches!(
				GameStorage::<Test>::get().expect("game remains running").state,
				GameState::Reporting { .. },
			));

			// Make ALICE eligible to claim without finishing the current game.
			let alice = AccountOrPerson::Account(ALICE);
			indiv_pallet_score::PersonhoodThreshold::<Test>::put(1);
			assert_ok!(indiv_pallet_score::Pallet::<Test>::set_attendance(
				&alice, true, game_index
			));

			MOCK_UNIX_TIME.with(|t| {
				*t.borrow_mut() =
					Duration::from_secs(GameTimes::<Test>::game_play_time(&schedule) as u64)
			});
			assert_ok!(indiv_pallet_airdrop::Pallet::<Test>::close_registration_authorized(
				frame_system::RawOrigin::Authorized.into(),
				event_id,
				0,
			));
			// v0.3.1's airdrop inserts an `AwaitingEntropy` step between closing registration
			// and drawing: the draw only proceeds on randomness whose moment is strictly past
			// the moment registration closed at. Advance the mock source, then capture.
			advance_airdrop_randomness();
			assert_ok!(indiv_pallet_airdrop::Pallet::<Test>::capture_entropy_authorized(
				frame_system::RawOrigin::Authorized.into(),
				event_id,
				0,
			));
			assert_ok!(indiv_pallet_airdrop::Pallet::<Test>::draw_winners_authorized(
				frame_system::RawOrigin::Authorized.into(),
				event_id,
				0,
			));
			assert_ok!(indiv_pallet_airdrop::Pallet::<Test>::close_drawing_authorized(
				frame_system::RawOrigin::Authorized.into(),
				event_id,
				0,
			));
			assert!(matches!(
				AirdropEvents::<Test>::get(event_id).expect("event is claimable").status,
				Status::Claiming { total_participants: 1, effective_winners: 1, claimed: 0 },
			));
			assert!(AirdropWinners::<Test>::contains_key(
				event_id,
				RegistrationEntry::<AccountId32>::Account { account_id: ALICE },
			));
			assert_ok!(frame_support::storage::with_transaction(|| {
				frame_support::storage::TransactionOutcome::Rollback(Game::claim_airdrop(
					RuntimeOrigin::signed(ALICE),
					game_index,
					id_to_account(98),
				))
			}));

			assert_ok!(Game::kill_current_game(RuntimeOrigin::root()));
			assert!(matches!(
				AirdropEvents::<Test>::get(event_id)
					.expect("event is clearing after cancel")
					.status,
				Status::ClearingRegistrations {
					total_participants: 1,
					effective_winners: 1,
					claimed: 0,
					cleaned_registrations: 0,
				},
			));
			assert_noop!(
				Game::claim_airdrop(RuntimeOrigin::signed(ALICE), game_index, id_to_account(99)),
				indiv_pallet_airdrop::Error::<Test>::NotClaiming,
			);
		});
	}

	#[test]
	fn kill_current_game_cancels_airdrop() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrop_prize: Some(test_airdrop_prize()),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			let event_id = Game::airdrop_event_id(game_index);
			// The airdrop event is scheduled but not yet started.
			assert!(matches!(
				indiv_pallet_airdrop::Events::<Test>::get(event_id)
					.expect("event scheduled")
					.status,
				indiv_pallet_airdrop::types::Status::Scheduled,
			));
			assert_ok!(Game::kill_current_game(RuntimeOrigin::root()));
			// Cancelling a not-yet-started event drops it and releases the full prize allocation.
			assert!(indiv_pallet_airdrop::Events::<Test>::get(event_id).is_none());
		});
	}
}
