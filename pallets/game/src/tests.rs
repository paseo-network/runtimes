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
	dispatch::Pays,
	traits::{Get, Hooks, OffchainWorker},
	BoundedVec,
};
use indiv_pallet_people::PEOPLE_MEMBER_IDENTIFIER;
use indiv_support::traits::{
	AddOnlyPeopleTrait, AppendOnlyMembers, RingExponent, RingMode, RingPosition,
};
use sp_core::{crypto::VrfSecret, ed25519, sr25519, Pair};
use sp_runtime::{testing::TestSignature, transaction_validity::InvalidTransaction, AccountId32};
use sp_statement_store::Statement;
use std::{slice, time::Duration};
use verifiable::{mock::Mock, GenerateVerifiable};

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

		// What Alice's attendance earns her in NFT claim credits is asserted in
		// `indiv-pallet-nft-credits`, which owns them.
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
						airdrops: None
					},
				));
			} else {
				assert_ok!(exec_signed_tx(
					INVITED,
					Call::sign_up_with_account {
						identifier_key: DEFAULT_IDENTIFIER_KEY,
						airdrops: None
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
		let post_info = Game::set_invite_ticket(RuntimeOrigin::signed(INVITER), ticket1)
			.expect("inviter has invites left");
		// Distributing an invite is free.
		assert_eq!(post_info.pays_fee, Pays::No);
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
		let error = Game::set_invite_ticket(RuntimeOrigin::signed(ALICE), 123)
			.expect_err("caller has no invites");
		assert_eq!(error.error, Error::<Test>::NoInvites.into());
		assert_eq!(error.post_info.pays_fee, Pays::Yes);
	});
}

#[test]
fn set_invite_ticket_fail_when_already_invited() {
	new_test_ext().execute_with(|| {
		assert_ok!(Game::grant_invites(RuntimeOrigin::root(), ALICE, 2));
		assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(ALICE), 123));
		let error = Game::set_invite_ticket(RuntimeOrigin::signed(ALICE), 123)
			.expect_err("ticket is already pending");
		assert_eq!(error.error, Error::<Test>::AlreadyInvited.into());
		assert_eq!(error.post_info.pays_fee, Pays::Yes);
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
						airdrops: None
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
						airdrops: None
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
						airdrops: None
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
						airdrops: None
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
						airdrops: None
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
				Call::sign_up_with_invite {
					identifier_key: DEFAULT_IDENTIFIER_KEY,
					airdrops: None
				},
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

// `GameIdx` must start at 0 and grow by 1.
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
		mock_statement_store().add_stmt(keep_stmt.clone());
		mock_statement_store().add_stmt(rm_stmt_1.clone());
		mock_statement_store().add_stmt(rm_stmt_2.clone());

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

		assert_eq!(remaining_statements(), vec![keep_stmt]);
	});

	// when an alias player gets archived.
	new_test_ext().execute_with(|| {
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
		mock_statement_store().add_stmt(keep_stmt.clone());
		mock_statement_store().add_stmt(rm_stmt_1.clone());
		mock_statement_store().add_stmt(rm_stmt_2.clone());

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
		assert_eq!(remaining_statements(), vec![keep_stmt]);
	});

	// when an account-based player gets archived.
	new_test_ext().execute_with(|| {
		MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs(1));
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
		mock_statement_store().add_stmt(keep_stmt.clone());
		mock_statement_store().add_stmt(rm_stmt_1.clone());
		mock_statement_store().add_stmt(rm_stmt_2.clone());

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
		assert_eq!(remaining_statements(), vec![keep_stmt]);
	});
}

/// Read the statements currently held in the mock store.
fn remaining_statements() -> Vec<Statement> {
	mock_statement_store().statements()
}

/// Build a statement signed by `pair`, so `remove_by(pair.public())` matches it.
fn statement_signed_by(pair: &ed25519::Pair) -> Statement {
	let mut s = Statement::new();
	s.sign_ed25519_private(pair);
	s
}

// The offchain worker walks the block's events and, for every `StmtUsageRemoved { who }`, removes
// exactly the statements signed by `who` from the statement store, leaving all others untouched.
#[test]
fn offchain_worker_removes_statements_for_each_emitted_who() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);

		let victim_a = ed25519::Pair::generate_with_phrase(None).0;
		let victim_b = ed25519::Pair::generate_with_phrase(None).0;
		let keeper = ed25519::Pair::generate_with_phrase(None).0;

		let keep_stmt = statement_signed_by(&keeper);
		mock_statement_store().add_stmt(statement_signed_by(&victim_a));
		mock_statement_store().add_stmt(statement_signed_by(&victim_b));
		mock_statement_store().add_stmt(keep_stmt.clone());

		// One event per removed account; the worker must honour every one of them.
		Game::deposit_event(Event::StmtUsageRemoved { who: victim_a.public().0 });
		Game::deposit_event(Event::StmtUsageRemoved { who: victim_b.public().0 });

		AllPalletsWithSystem::offchain_worker(System::block_number());

		// Both victims' statements are gone; the unrelated keeper statement survives.
		assert_eq!(remaining_statements(), vec![keep_stmt]);
	});
}

// The offchain worker must not remove anything when the block carries no `StmtUsageRemoved` event,
// even if other game events are present.
#[test]
fn offchain_worker_keeps_statements_without_removal_event() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);

		let signer = ed25519::Pair::generate_with_phrase(None).0;
		let stmt = statement_signed_by(&signer);
		mock_statement_store().add_stmt(stmt.clone());

		// An unrelated event is present, but no `StmtUsageRemoved`, so nothing is removed.
		Game::deposit_event(Event::GameCancelled { game_index: 0 });

		AllPalletsWithSystem::offchain_worker(System::block_number());

		assert_eq!(remaining_statements(), vec![stmt]);
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
				airdrops: test_airdrops(1),
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
			assert!(matches!(
				crate::Game::<Test>::get().unwrap().state,
				GameState::Cancelling { .. }
			));

			// Game should be removed
			advance_process();
			assert!(crate::Game::<Test>::get().is_none());

			// Game history entry is removed when the game is cancelled.
			assert!(GameHistory::<Test>::get(game_index).is_none());

			// Cancellation routed the airdrop event into the airdrop pallet's clean-up pipeline
			// (or dropped it outright if still `Scheduled`).
			let event_id = Game::airdrop_event_id(game_index, 0);
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
fn offboard_suspends_recognized_player() {
	new_test_ext().execute_with(|| {
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

		// Reach personhood and register, so ALICE is `Recognized`.
		let mut safety = 0;
		while !Score::reached_personhood(&alice) {
			run_game_scenario(schedule.clone(), slice::from_ref(&alice), |_p| {
				Some(attend_report.clone()) // attend
			});
			schedule.game_play_time += 10; // ensure times don't overlap
			safety += 1;
			assert!(safety < 200, "took too many games to reach personhood");
		}
		let (key, sk) = mock_key(1234);
		let proof = {
			let mut m = b"pop register using".to_vec();
			m.extend_from_slice(&ALICE.encode()[..]);
			Mock::sign(&sk, &m[..]).unwrap()
		};
		assert_ok!(Score::register(RuntimeOrigin::signed(ALICE), Some((key, proof))));

		// Offboard while no game is ongoing.
		assert_ok!(Game::offboard(RuntimeOrigin::signed(ALICE)));

		assert!(!indiv_pallet_score::Participants::<Test>::contains_key(&alice));
		// The offboard suspended the personhood: the member key is suspended in the people
		// ring.
		assert_eq!(
			indiv_pallet_members::Members::<Test>::get(PEOPLE_MEMBER_IDENTIFIER, key),
			Some(RingPosition::Suspended)
		);
		// The mutation session opened by the offboard is closed again.
		assert!(
			indiv_pallet_members::RingsState::<Test>::get(PEOPLE_MEMBER_IDENTIFIER).append_only()
		);
	});
}

#[test]
fn kickout_suspends_recognized_player() {
	new_test_ext().execute_with(|| {
		// Create the people collection first
		assert_ok!(Members::create_collection(
			0,
			PEOPLE_MEMBER_IDENTIFIER,
			1,
			RingMode::Flexible,
			RingExponent::R2e9,
			None,
		));

		let player = AccountOrPerson::Account(ALICE);

		// An archived kickable player whose recognition is `Recognized`, backed by a real
		// person.
		assert_ok!(indiv_pallet_score::Pallet::<Test>::onboard_for_recognition(&ALICE));
		let id = People::reserve_new_id();
		let (key, _sk) = mock_key(1234);
		assert_ok!(People::recognize_personhood(id, Some(key)));
		indiv_pallet_score::Participants::<Test>::mutate(&player, |p| {
			p.as_mut().unwrap().recognition = indiv_pallet_score::Recognition::Recognized(id);
		});
		ArchivedPlayers::<Test>::insert(
			&player,
			ArchivedPlayer::Kickable { archived_since: 0, first_game: 0 },
		);

		let kickout_time: u64 = <Test as Config>::NonPlayingKickoutTime::get();
		System::set_block_number(kickout_time + 1);

		assert_ok!(Game::kickout(RuntimeOrigin::signed(EVE), ALICE));

		assert!(!indiv_pallet_score::Participants::<Test>::contains_key(&player));
		// The kickout suspended the personhood: the member key is suspended in the people
		// ring.
		assert_eq!(
			indiv_pallet_members::Members::<Test>::get(PEOPLE_MEMBER_IDENTIFIER, key),
			Some(RingPosition::Suspended)
		);
		// The mutation session opened by the kickout is closed again.
		assert!(
			indiv_pallet_members::RingsState::<Test>::get(PEOPLE_MEMBER_IDENTIFIER).append_only()
		);
	});
}

#[test]
fn offboard_fails_when_suspension_fails() {
	new_test_ext().execute_with(|| {
		let alice = AccountOrPerson::Account(ALICE);

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 25,
			rounds: 2,
			max_group_size: 2,
			..Default::default()
		};
		run_game_scenario(schedule, slice::from_ref(&alice), |_player| {
			Some(vec![BoundedVec::default(), BoundedVec::default()].try_into().unwrap())
		});

		// Make ALICE `Recognized` with a personal id that does not belong to any person, so
		// the suspension on offboard fails.
		indiv_pallet_score::Participants::<Test>::mutate(&alice, |p| {
			p.as_mut().unwrap().recognition = indiv_pallet_score::Recognition::Recognized(404);
		});

		let err = Game::offboard(RuntimeOrigin::signed(ALICE)).unwrap_err();
		assert_eq!(err.error, indiv_pallet_people::Error::<Test>::NotPerson.into());
	});
}

#[test]
fn kickout_fails_when_suspension_fails() {
	new_test_ext().execute_with(|| {
		let player = AccountOrPerson::Account(ALICE);

		// An archived kickable player who is `Recognized` with a personal id that does not
		// belong to any person, so the suspension on kickout fails.
		assert_ok!(indiv_pallet_score::Pallet::<Test>::onboard_for_recognition(&ALICE));
		indiv_pallet_score::Participants::<Test>::mutate(&player, |p| {
			p.as_mut().unwrap().recognition = indiv_pallet_score::Recognition::Recognized(404);
		});
		ArchivedPlayers::<Test>::insert(
			&player,
			ArchivedPlayer::Kickable { archived_since: 0, first_game: 0 },
		);

		let kickout_time: u64 = <Test as Config>::NonPlayingKickoutTime::get();
		System::set_block_number(kickout_time + 1);

		let err = Game::kickout(RuntimeOrigin::signed(EVE), ALICE).unwrap_err();
		assert_eq!(err.error, indiv_pallet_people::Error::<Test>::NotPerson.into());
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
		assert!(matches!(crate::Game::<Test>::get().unwrap().state, GameState::Cancelling { .. }));

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
						airdrops: None
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
			airdrops_scheduled: 0,
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

/// End-to-end equivalence guard for the arithmetic `shuffle_step_compute_weights`, exercising
/// *multiple* groups and rounds with a mix of recognized (person) and not-recognized (candidate)
/// players. For every player we recompute the expected weight the "slow" way the optimization
/// replaced - reading each actual co-player's snapshotted `vote_weight` via `IndexToPlayer` - and
/// assert it matches the value the optimization derived from index arithmetic alone. A divergence
/// would mean the recognized/not-recognized index band (or `recognized_count`) is wrong.
#[test]
fn expected_max_vote_weight_matches_per_member_oracle_multi_group() {
	use crate::types::GroupsSetting;

	new_test_ext().execute_with(|| {
		PeopleVoteWeight::set(&3u8);
		CandidateVoteWeight::set(&2u8);

		const RECOGNIZED: u64 = 4;
		const CANDIDATES: u64 = 4;
		let max_group_size = 3u32;
		let rounds = 2u8;
		let player_count = (RECOGNIZED + CANDIDATES) as u32;

		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 20,
			rounds,
			max_group_size,
			..Default::default()
		};
		assert_ok!(Game::new_game(&schedule));

		// Recognized players: externally recognized aliases (snapshot = PeopleVoteWeight).
		for i in 0..RECOGNIZED {
			let alias = id_to_alias(i);
			let stmt_acc = id_to_account(1000 + i);
			assert_ok!(Game::sign_up_with_alias(
				runtime_origin_for_alias(&alias),
				DEFAULT_IDENTIFIER_KEY,
				stmt_acc.clone(),
				AccountAuthority(stmt_acc),
				None,
			));
		}
		// Candidates: account players (snapshot = CandidateVoteWeight).
		for i in 0..CANDIDATES {
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(id_to_account(i)),
				DEFAULT_IDENTIFIER_KEY,
				None,
			));
		}

		advance_to_reporting_phase(&schedule);

		// The scenario must span more than one group, otherwise group membership is trivial.
		let groups = GroupsSetting { max_per_group: max_group_size, player_count };
		assert!(groups.number_of_group() >= 2, "test must span multiple groups");

		// Oracle: recompute every player's expected weight from the actual co-players, exactly as
		// the pre-optimization code did, and compare to the stored arithmetic result.
		for (player_id, round_indices) in PlayerToIndex::<Test>::iter() {
			let mut oracle: u32 = 0;
			for round in 0..rounds {
				let player_idx = round_indices[usize::from(round)];
				let group_index = groups.group_index_from_player_index(player_idx);
				for member_idx in groups.group_members(group_index) {
					if member_idx == player_idx {
						continue;
					}
					let member_id = IndexToPlayer::<Test>::get((round, member_idx))
						.expect("index maps to a player");
					let member = Players::<Test>::get(&member_id).expect("player exists");
					oracle = oracle.saturating_add(member.vote_weight as u32);
				}
			}
			let oracle_u16: u16 = oracle.try_into().unwrap_or(u16::MAX);
			let stored = Players::<Test>::get(&player_id).unwrap().expected_max_vote_weight;
			assert_eq!(
				stored, oracle_u16,
				"expected_max_vote_weight mismatch for {player_id:?}: \
				 arithmetic={stored} vs per-member oracle={oracle_u16}",
			);
		}
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
		// A 4-player, single-round game. ALICE reports `Person` on her 3 co-players, so the
		// submitted report carries `e = 3` co-player entries. Pre-dispatch, `report` is
		// charged for the worst case of `e + 1 = 4` early enactments (every co-player plus
		// the reporter). But after ALICE's lone report nobody's attendance is decidable: each
		// co-player has a single `Person` vote with votes still outstanding, and ALICE has
		// received no votes at all. Zero enactments fire, so the whole `n` (enactment) axis is
		// refunded, while the `e` axis (known exactly up front) is charged in full.
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
		let full_report = build_report_with_opinion(&alice, |_| Report::Person);
		let e = full_report.iter().map(|round| round.len() as u32).sum::<u32>();
		assert_eq!(e, 3, "ALICE has 3 co-players in a single full group of 4");

		// Pre-condition: no player has been early-enacted yet.
		assert_eq!(
			Players::<Test>::iter()
				.filter(|(_, p)| p.early_attendance_enactment.is_some())
				.count(),
			0,
		);

		let upfront_weight = crate::Call::<Test>::report { full_report: full_report.clone() }
			.get_dispatch_info()
			.call_weight;
		// Mirror the `report` weight annotation's pre-dispatch bound: `report(e, e + 1)`.
		assert_eq!(upfront_weight, <MockWeightInfo as WeightInfo>::report(e, e + 1));
		assert_eq!(upfront_weight, Weight::from_parts((e + 1) as u64, e as u64));

		let post_info =
			Game::report(RuntimeOrigin::signed(ALICE), full_report).expect("report should succeed");
		assert_eq!(post_info.pays_fee, Pays::No);

		// ALICE's lone report leaves every attendance undecided, so nothing is enacted.
		let actual_enacted = Players::<Test>::iter()
			.filter(|(_, p)| p.early_attendance_enactment.is_some())
			.count() as u32;
		assert_eq!(actual_enacted, 0, "no attendance is decidable after a single report");

		// Post-dispatch actual weight: the `n` axis drops to the real enactment count, the
		// `e` axis is unchanged.
		let actual_weight = post_info.actual_weight.expect("post-dispatch weight must be set");
		assert_eq!(actual_weight, <MockWeightInfo as WeightInfo>::report(e, actual_enacted));
		assert_eq!(actual_weight, Weight::from_parts(0, e as u64));

		// The enactment axis is fully refunded; the co-player-entry axis is not.
		let refund = upfront_weight.saturating_sub(actual_weight);
		assert_eq!(refund, Weight::from_parts((e + 1) as u64, 0));
	});
}

#[test]
fn max_received_votes_counts_one_vote_per_co_member_per_round() {
	new_test_ext().execute_with(|| {
		let group_size: u32 = <Test as Config>::MaxGroupSize::get();
		let rounds: u32 = <Test as Config>::MaxRounds::get();

		// Independently reconstruct the model the bound encodes: each round a player is
		// voted on by every co-member of their group, all but themselves.
		let expected: u32 = (0..rounds).map(|_| group_size - 1).sum();
		assert_eq!(Game::max_received_votes(), expected);

		// Guard against the earlier `MaxGroupSize * MaxRounds - 1` formula, which drops
		// only one self-vote for the whole game instead of one per round and so over-counts
		// by `MaxRounds - 1`.
		assert!(rounds > 1, "mock must run multi-round games for this guard to bite");
		assert_ne!(Game::max_received_votes(), group_size * rounds - 1);

		// The bound is exactly what `integrity_test` multiplies against the vote weight to
		// keep the worst-case `u8` `yes_person`/`no_not_person` tally from overflowing.
		let max_vote_weight = PeopleVoteWeight::get().max(CandidateVoteWeight::get());
		assert!(
			Game::max_received_votes() as u64 * max_vote_weight as u64 <= u8::MAX as u64,
			"worst-case vote tally must fit in u8",
		);
	});
}

// `offboard` charges `max(account, person)` up front and refunds the branch actually
// taken. An archived account offboards via the cheaper `offboard_account` path.
#[test]
fn offboard_refunds_account_branch() {
	use frame_support::dispatch::GetDispatchInfo;

	new_test_ext().execute_with(|| {
		let alice = AccountOrPerson::Account(ALICE);
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 25,
			rounds: 2,
			max_group_size: 2,
			..Default::default()
		};
		run_game_scenario(schedule, std::slice::from_ref(&alice), |_player| None);
		assert!(ArchivedPlayers::<Test>::contains_key(&alice), "player should be archived");

		let charged = crate::Call::<Test>::offboard {}.get_dispatch_info().call_weight;
		assert_eq!(
			charged,
			<MockWeightInfo as WeightInfo>::offboard_account()
				.max(<MockWeightInfo as WeightInfo>::offboard_person())
		);

		let post = Game::offboard(RuntimeOrigin::signed(ALICE)).expect("offboard should succeed");
		let actual = post.actual_weight.expect("post-dispatch weight must be set");
		assert_eq!(actual, <MockWeightInfo as WeightInfo>::offboard_account());
		assert!(
			actual.all_lt(charged),
			"account path {actual:?} must be refunded below the worst-case charge {charged:?}",
		);
	});
}

mod cancel_game {
	use super::*;
	use sp_runtime::DispatchError;

	#[test]
	fn manager_origin_transitions_running_game_to_cancelling() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: Default::default(),
			};
			assert_ok!(Game::new_game(&schedule));
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(ALICE),
				DEFAULT_IDENTIFIER_KEY,
				None,
			));
			let game = crate::Game::<Test>::get().expect("game exists");
			assert!(matches!(game.state, GameState::Registration { .. }));

			assert_ok!(Game::cancel_game(RuntimeOrigin::root()));

			// Game stays in storage with state flipped to `Cancelling`; the
			// per-player cleanup runs on subsequent blocks via on_poll.
			let game = crate::Game::<Test>::get().expect("game still in storage");
			assert!(matches!(game.state, GameState::Cancelling { .. }));
			let player = AccountOrPerson::<AccountId32>::Account(ALICE);
			let info = crate::Players::<Test>::get(&player).expect("player record retained");
			assert!(info.registered, "cleanup happens later, not during the extrinsic");
			// `GameCancelled` is emitted up-front, when the game is flipped to `Cancelling`.
			System::assert_has_event(
				crate::Event::<Test>::GameCancelled { game_index: game.index }.into(),
			);
		});
	}

	#[test]
	fn process_cancelling_drains_residue_and_kills_game() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(1),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = crate::GameIndex::<Test>::get();
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(ALICE),
				DEFAULT_IDENTIFIER_KEY,
				None,
			));
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(BOB),
				DEFAULT_IDENTIFIER_KEY,
				None,
			));

			assert_ok!(Game::cancel_game(RuntimeOrigin::root()));

			// `advance_process` runs on_poll to completion (multiple iterations
			// if needed), driving `process_cancelling` until Game is killed.
			advance_process();

			assert!(crate::Game::<Test>::get().is_none(), "cleanup must remove Game");
			assert!(
				crate::GameHistory::<Test>::get(game_index).is_none(),
				"cleanup must remove the GameHistory entry"
			);
			let alice = AccountOrPerson::<AccountId32>::Account(ALICE);
			let bob = AccountOrPerson::<AccountId32>::Account(BOB);
			assert!(!crate::Players::<Test>::get(&alice).unwrap().registered);
			assert!(!crate::Players::<Test>::get(&bob).unwrap().registered);
			System::assert_has_event(crate::Event::<Test>::GameCancelled { game_index }.into());
		});
	}

	#[test]
	fn cancel_blocks_auto_start_of_next_scheduled_game_until_cleanup_done() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			let current = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: Default::default(),
			};
			assert_ok!(Game::new_game(&current));
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(ALICE),
				DEFAULT_IDENTIFIER_KEY,
				None,
			));
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(BOB),
				DEFAULT_IDENTIFIER_KEY,
				None,
			));

			let durations = <Test as Config>::DefaultPhaseDurations::get();
			let next = GameSchedule::<u32, u128> {
				game_play_time: GameTimes::<Test>::player_process_end(&current) +
					durations.registration +
					durations.shuffle +
					durations.post_shuffle_margin,
				rounds: 2,
				max_group_size: 3,
				airdrops: Default::default(),
			};
			assert_ok!(Game::schedule_games(RuntimeOrigin::root(), vec![next.clone()]));
			assert_eq!(GameSchedules::<Test>::get().len(), 1);

			assert_ok!(Game::cancel_game(RuntimeOrigin::root()));

			// Right after cancellation, the current game is still present in
			// storage and the successor is still queued.
			let game = crate::Game::<Test>::get().expect("cancelling game still in storage");
			assert!(matches!(game.state, GameState::Cancelling { .. }));
			assert_eq!(
				game.game_date, current.game_play_time,
				"queued successor must not start yet"
			);
			assert_eq!(GameSchedules::<Test>::get().len(), 1, "successor must remain queued");

			// With full weight, on_poll can finish the cancellation cleanup and
			// remove the current game.
			advance_process();
			assert!(crate::Game::<Test>::get().is_none(), "cleanup must remove current game");
			assert_eq!(GameSchedules::<Test>::get().len(), 1, "successor should still be queued");

			// Once the current game is fully gone, the next on_poll may start the
			// queued successor.
			advance_process();
			let game = crate::Game::<Test>::get().expect("queued successor should start");
			assert_eq!(game.game_date, next.game_play_time);
			assert_eq!(GameSchedules::<Test>::get().len(), 0);
		});
	}

	#[test]
	fn signed_origin_is_rejected() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(1),
			};
			assert_ok!(Game::new_game(&schedule));

			assert_noop!(Game::cancel_game(RuntimeOrigin::signed(ALICE)), DispatchError::BadOrigin,);

			// Game state untouched.
			let game = crate::Game::<Test>::get().expect("game still in storage");
			assert!(matches!(game.state, GameState::Registration { .. }));
		});
	}

	#[test]
	fn errors_when_no_game_exists() {
		new_test_ext().execute_with(|| {
			assert!(crate::Game::<Test>::get().is_none());

			assert_noop!(Game::cancel_game(RuntimeOrigin::root()), crate::Error::<Test>::NoGame,);
		});
	}

	fn force_game_state(state: GameState<AccountId32>) {
		crate::Game::<Test>::mutate(|maybe_game| {
			let game = maybe_game.as_mut().expect("game in storage");
			game.state = state;
		});
	}

	fn setup_one_player_game() {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 2,
			max_group_size: 3,
			airdrops: Default::default(),
		};
		assert_ok!(Game::new_game(&schedule));
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(ALICE),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));
	}

	#[test]
	fn cancel_when_game_is_in_shuffle() {
		new_test_ext().execute_with(|| {
			setup_one_player_game();
			force_game_state(GameState::Shuffle {
				step: ShuffleStep::Step1Insert { last_iteration: None },
			});

			assert_ok!(Game::cancel_game(RuntimeOrigin::root()));

			let game = crate::Game::<Test>::get().expect("game still in storage");
			assert!(matches!(game.state, GameState::Cancelling { .. }));
		});
	}

	#[test]
	fn process_cancelling_drains_residual_shuffle_entries() {
		new_test_ext().execute_with(|| {
			setup_one_player_game();
			let game = crate::Game::<Test>::get().expect("game exists");
			let rounds = game.rounds;

			// Seed `ShuffleRecognized` / `ShuffleNotRecognized` to simulate
			// cancellation landing mid-shuffle.
			for round in 0..rounds {
				let hash = sp_io::hashing::blake2_256(&[round, 1]);
				let aop = AccountOrPerson::<AccountId32>::Account(ALICE);
				crate::ShuffleRecognized::<Test>::insert(round, hash, &aop);
				let hash = sp_io::hashing::blake2_256(&[round, 2]);
				crate::ShuffleNotRecognized::<Test>::insert(round, hash, &aop);
			}

			force_game_state(GameState::Shuffle {
				step: ShuffleStep::Step1Insert { last_iteration: None },
			});
			assert_ok!(Game::cancel_game(RuntimeOrigin::root()));

			advance_process();

			assert!(crate::Game::<Test>::get().is_none(), "game should be killed");
			for round in 0..rounds {
				assert_eq!(
					crate::ShuffleRecognized::<Test>::iter_prefix(round).count(),
					0,
					"recognized entries for round {round} must be drained",
				);
				assert_eq!(
					crate::ShuffleNotRecognized::<Test>::iter_prefix(round).count(),
					0,
					"not-recognized entries for round {round} must be drained",
				);
			}
		});
	}

	#[test]
	fn rejects_when_game_is_in_reporting() {
		new_test_ext().execute_with(|| {
			setup_one_player_game();
			force_game_state(GameState::Reporting { player_count: 1 });

			assert_noop!(
				Game::cancel_game(RuntimeOrigin::root()),
				crate::Error::<Test>::InvalidGameState,
			);

			assert!(crate::Game::<Test>::get().is_some());
		});
	}

	#[test]
	fn rejects_when_game_is_in_player_process() {
		new_test_ext().execute_with(|| {
			setup_one_player_game();
			force_game_state(GameState::PlayerProcess {
				step: PlayerProcessStep::Step1ProcessPlayers {
					last_iteration: None,
					player_count: 1,
				},
			});

			assert_noop!(
				Game::cancel_game(RuntimeOrigin::root()),
				crate::Error::<Test>::InvalidGameState,
			);

			assert!(crate::Game::<Test>::get().is_some());
		});
	}

	#[test]
	fn rejects_when_game_is_already_cancelling() {
		new_test_ext().execute_with(|| {
			setup_one_player_game();
			force_game_state(GameState::Cancelling {
				step: CancellingStep::Step2DrainPlayers { last_iteration: None },
			});

			// Re-issuing cancel_game on an already-Cancelling game is a no-op
			// at best and confusing at worst; reject it.
			assert_noop!(
				Game::cancel_game(RuntimeOrigin::root()),
				crate::Error::<Test>::InvalidGameState,
			);

			let game = crate::Game::<Test>::get().expect("game still in storage");
			assert!(matches!(game.state, GameState::Cancelling { .. }));
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
			airdrops_scheduled: 0,
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
			put_game_in_state(GameState::Cancelling {
				step: CancellingStep::Step2DrainPlayers { last_iteration: None },
			});
			assert_noop!(
				Game::set_game_phases(RuntimeOrigin::root(), distinct_phases()),
				Error::<Test>::InvalidGameState,
			);
		});
	}
}

mod airdrop {
	use super::*;
	use crate::{extension::CustomError, Game as GameStorage};
	use codec::{Decode, Encode, MaxEncodedLen};
	use frame_support::{
		pallet_prelude::Weight,
		traits::fungibles::{Inspect, Mutate, MutateHold},
	};
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

	/// The account variant with `n` dummy entries; the VRF signatures do not verify.
	fn account_proofs(
		n: u32,
	) -> crate::AirdropVrfs<<verifiable::mock::Mock as verifiable::GenerateVerifiable>::Proof> {
		crate::AirdropVrfs::Account(
			(0..n)
				.map(|_| dummy_vrf())
				.collect::<Vec<_>>()
				.try_into()
				.expect("n is bounded by MAX_GAME_AIRDROPS"),
		)
	}

	/// The alias variant with `n` default entries; the mock member service accepts any proof.
	fn alias_proofs(
		n: u32,
	) -> crate::AirdropVrfs<<verifiable::mock::Mock as verifiable::GenerateVerifiable>::Proof> {
		crate::AirdropVrfs::Alias {
			proofs: (0..n)
				.map(|_| Default::default())
				.collect::<Vec<_>>()
				.try_into()
				.expect("n is bounded by MAX_GAME_AIRDROPS"),
			ring_index: 0,
			revision: 0,
		}
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

	/// Flip the airdrop events at indices `0..n` of `game_index` into `Status::Registering` so
	/// participation calls land.
	fn open_airdrop_events(game_index: GameIdx, n: u8) {
		for airdrop_index in 0..n {
			set_event_status(
				Game::airdrop_event_id(game_index, airdrop_index),
				Status::Registering { total_participants: 0 },
			);
		}
	}

	/// Register ALICE's sr25519 keypair and build a VRF signature bound to the event id of
	/// `game_index`'s airdrop at `airdrop_index`. Returns the public key and the signature.
	fn alice_event_vrf(
		game_index: GameIdx,
		airdrop_index: u8,
	) -> (sp_core::sr25519::Public, VrfSignature) {
		use sp_core::{crypto::VrfSecret, sr25519, Pair as _};
		let pair = sr25519::Pair::from_seed(b"alice_vrf_seed_____padding______");
		register_account_pubkey(ALICE, pair.public());
		let event_id = Game::airdrop_event_id(game_index, airdrop_index);
		let signature = pair.vrf_sign(
			&indiv_pallet_airdrop::vrf::transcript_for_event(&event_id, &pair.public())
				.into_sign_data(),
		);
		(pair.public(), signature)
	}

	/// The account variant with one entry per airdrop index `0..n`, each bound to its own event
	/// id.
	fn alice_airdrop_vrfs(
		game_index: GameIdx,
		n: u8,
	) -> crate::AirdropVrfs<<verifiable::mock::Mock as verifiable::GenerateVerifiable>::Proof> {
		crate::AirdropVrfs::Account(
			(0..n)
				.map(|airdrop_index| {
					let (_public, signature) = alice_event_vrf(game_index, airdrop_index);
					signature
				})
				.collect::<Vec<_>>()
				.try_into()
				.expect("n is bounded by MAX_GAME_AIRDROPS"),
		)
	}

	/// The slot ALICE's VRF resolves to for `game_index`'s airdrop at `airdrop_index`.
	fn alice_event_slot(game_index: GameIdx, airdrop_index: u8) -> BigEndianU256 {
		let (public, signature) = alice_event_vrf(game_index, airdrop_index);
		let event_id = Game::airdrop_event_id(game_index, airdrop_index);
		let entropy =
			indiv_pallet_airdrop::vrf::verify_and_extract_entropy(&public, &event_id, &signature)
				.expect("signature verifies");
		BigEndianU256::from(entropy)
	}

	/// Move the airdrop event at index 0 for `game_index` directly into `Status::Claiming` and
	/// stage a winning entry for `registrant` so a `claim_airdrop` call lands.
	fn stage_claim_for(game_index: GameIdx, registrant: RegistrationEntry<AccountId32>) {
		stage_claim_for_index(game_index, 0, registrant);
	}

	/// Move the airdrop event at `airdrop_index` for `game_index` directly into
	/// `Status::Claiming` and stage a winning entry for `registrant` so a `claim_airdrop` call
	/// lands.
	fn stage_claim_for_index(
		game_index: GameIdx,
		airdrop_index: u8,
		registrant: RegistrationEntry<AccountId32>,
	) {
		let event_id = Game::airdrop_event_id(game_index, airdrop_index);
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
				airdrops: test_airdrops(1),
			};
			let now = <Test as crate::Config>::UnixTime::now().as_secs();
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			let event_id = Game::airdrop_event_id(game_index, 0);
			let event = AirdropEvents::<Test>::get(event_id).expect("event scheduled");
			assert_eq!(event.info.registration_starts, now);
			assert_eq!(event.info.draw_time, GameTimes::<Test>::game_play_time(&schedule) as u64);
			// end_time = draw_time + the schedule entry's `claim_window`.
			let game = GameStorage::<Test>::get().expect("game exists");
			assert_ne!(game.airdrops_scheduled, 0);
			assert_eq!(event.info.end_time, 10 + TEST_AIRDROP_CLAIM_WINDOW);
			// The schedule call carried the schedule's airdrop prize.
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
				airdrops: test_airdrops(1),
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
			let event = AirdropEvents::<Test>::get(Game::airdrop_event_id(game_index, 0))
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
				airdrops: test_airdrops(1),
			};
			assert_ok!(Game::new_game(&schedule));
			assert!(crate::Game::<Test>::exists());
			let game = GameStorage::<Test>::get().expect("game exists");
			// Scheduling was attempted but failed, so the event isn't in airdrop storage and
			// `airdrops_scheduled` is empty.
			assert_eq!(game.airdrops_scheduled, 0);
			assert!(AirdropEvents::<Test>::get(Game::airdrop_event_id(game.index, 0)).is_none());
		});
	}

	#[test]
	fn sign_up_with_empty_proof_when_schedule_failed_succeeds() {
		// If `new_game` fails to schedule the airdrop, `airdrops_scheduled` is 0. The check is
		// count-based, so a `Some` carrying zero VRFs matches and the sign-up succeeds (no
		// participate dispatch); the player is recorded in `Players` as usual.
		new_test_ext().execute_with(|| {
			break_airdrop_schedule();
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(1),
			};
			assert_ok!(Game::new_game(&schedule));
			let event_id = Game::airdrop_event_id(GameIndex::<Test>::get(), 0);
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(ALICE),
				DEFAULT_IDENTIFIER_KEY,
				Some(account_proofs(0)),
			));
			assert!(Players::<Test>::get(AccountOrPerson::Account(ALICE)).is_some());
			// No participate happened: nothing in `Registrations` for this event.
			assert!(AirdropRegistrations::<Test>::iter_prefix(event_id).next().is_none());
		});
	}

	#[test]
	fn sign_up_with_proof_when_schedule_failed_is_rejected() {
		// When scheduling failed (`airdrops_scheduled` is 0) a sign-up carrying airdrop VRFs is
		// rejected: the supplied VRF count does not match the scheduled count.
		new_test_ext().execute_with(|| {
			break_airdrop_schedule();
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(1),
			};
			assert_ok!(Game::new_game(&schedule));
			assert_noop!(
				Game::sign_up_with_account(
					RuntimeOrigin::signed(ALICE),
					DEFAULT_IDENTIFIER_KEY,
					Some(account_proofs(1)),
				),
				Error::<Test>::InvalidAirdropVrfCount
			);
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
				airdrops: test_airdrops(1),
			};
			assert_ok!(Game::new_game(&schedule));
			let event_id = Game::airdrop_event_id(GameIndex::<Test>::get(), 0);

			// `participate_with_account` only accepts events in `Status::Registering`. The
			// game scheduled the event in `Scheduled`; flip it so the call lands.
			set_event_status(event_id, Status::Registering { total_participants: 0 });

			// Build a real sr25519 VRF signature for ALICE, bound to the event's own id.
			// Register the keypair's pubkey for ALICE so `AccountIdToPublic` resolves to the
			// pair we sign with.
			let pair = sr25519::Pair::from_seed(b"alice_vrf_seed_____padding______");
			register_account_pubkey(ALICE, pair.public());
			let signature = pair.vrf_sign(
				&indiv_pallet_airdrop::vrf::transcript_for_event(&event_id, &pair.public())
					.into_sign_data(),
			);

			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(ALICE),
				DEFAULT_IDENTIFIER_KEY,
				Some(crate::AirdropVrfs::Account(vec![signature.clone()].try_into().unwrap())),
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
				airdrops: test_airdrops(1),
			};
			assert_ok!(Game::new_game(&schedule));
			let event_id = Game::airdrop_event_id(GameIndex::<Test>::get(), 0);
			set_event_status(event_id, Status::Registering { total_participants: 0 });

			let alias: Alias = [0xAA; 32];
			let stmt_account = id_to_account(0xA11A5);
			assert_ok!(Game::sign_up_with_alias(
				runtime_origin_for_alias(&alias),
				DEFAULT_IDENTIFIER_KEY,
				stmt_account.clone(),
				AccountAuthority(stmt_account),
				Some(alias_proofs(1)),
			));

			// `MockAirdropMemberService::verify_membership` returns the blake2 of the
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
				airdrops: test_airdrops(1),
			};
			assert_ok!(Game::new_game(&schedule));
			assert_noop!(
				Game::sign_up_with_account(
					RuntimeOrigin::signed(ALICE),
					DEFAULT_IDENTIFIER_KEY,
					Some(alias_proofs(1)),
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
				airdrops: test_airdrops(1),
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
					Some(account_proofs(1)),
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
				airdrops: test_airdrops(1),
			};
			assert_ok!(Game::new_game(&schedule));
			let event_id = Game::airdrop_event_id(GameIndex::<Test>::get(), 0);
			register_account_pubkey(ALICE, sp_core::sr25519::Public::from([1u8; 32]));
			assert_noop!(
				Game::sign_up_with_account(
					RuntimeOrigin::signed(ALICE),
					DEFAULT_IDENTIFIER_KEY,
					Some(account_proofs(1)),
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
				airdrops: test_airdrops(1),
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
						airdrops: Some(account_proofs(1)),
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
				airdrops: test_airdrops(1),
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
						airdrops: Some(alias_proofs(1)),
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
		use sp_runtime::{
			traits::{Applyable, Checkable},
			transaction_validity::TransactionSource,
		};

		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(2),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			let event_id = Game::airdrop_event_id(game_index, 0);
			// `participate_with_account` only accepts events in `Status::Registering`; the game
			// scheduled the events in `Scheduled`, flip them so the inner calls land successfully.
			open_airdrop_events(game_index, 2);

			let ticket = 44u64;
			let invite_sig = TestSignature(ticket, ALICE.encode());
			assert_ok!(Game::grant_invites(RuntimeOrigin::root(), BOB, 1));
			assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(BOB), ticket));

			let nonce = frame_system::Account::<Test>::get(&ALICE).nonce;
			let x = mock::Extrinsic::new_signed(
				Call::sign_up_with_invite {
					identifier_key: DEFAULT_IDENTIFIER_KEY,
					airdrops: Some(alice_airdrop_vrfs(game_index, 2)),
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

			// `participate_with_account` would insert a `Registrations` entry into each of the
			// game's events on success; the inner rollback must have reverted all of them.
			assert!(AirdropRegistrations::<Test>::iter_prefix(event_id).next().is_none());
			assert!(AirdropRegistrations::<Test>::iter_prefix(Game::airdrop_event_id(
				game_index, 1
			))
			.next()
			.is_none());
		});
	}

	#[test]
	fn claim_rejects_when_never_attended() {
		new_test_ext().execute_with(|| {
			force_participant(AccountOrPerson::Account(ALICE), Recognition::Recognized(0), None);
			assert_noop!(
				Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 1, 0, id_to_account(99)),
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
				Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 4, 0, id_to_account(99)),
				Error::<Test>::NotEligibleForAirdrop,
			);

			// Next game.
			stage_claim_for(6, RegistrationEntry::Account { account_id: ALICE });
			assert_noop!(
				Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 6, 0, id_to_account(99)),
				Error::<Test>::NotEligibleForAirdrop,
			);

			// Sanity: claiming game 5 — the actual `last_attended_game` — succeeds.
			stage_claim_for(5, RegistrationEntry::Account { account_id: ALICE });
			assert_ok!(Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 5, 0, id_to_account(99)));
		});
	}

	#[test]
	fn claim_rejects_when_not_recognized() {
		new_test_ext().execute_with(|| {
			// Player attended game 1 but never reached recognition or personhood
			force_participant(AccountOrPerson::Account(ALICE), Recognition::NotRecognized, Some(1));
			assert_noop!(
				Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 1, 0, id_to_account(99)),
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
				airdrops: test_airdrops(1),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			let event_id = Game::airdrop_event_id(game_index, 0);
			set_event_status(event_id, Status::Registering { total_participants: 0 });

			// Sign up with the Account VRF (player is `NotRecognized` at signup), bound to the
			// event's own id.
			let pair = sr25519::Pair::from_seed(b"alice_vrf_seed_____padding______");
			register_account_pubkey(ALICE, pair.public());
			let signature = pair.vrf_sign(
				&indiv_pallet_airdrop::vrf::transcript_for_event(&event_id, &pair.public())
					.into_sign_data(),
			);
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(ALICE),
				DEFAULT_IDENTIFIER_KEY,
				Some(crate::AirdropVrfs::Account(vec![signature].try_into().unwrap())),
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
				0,
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
				airdrops: test_airdrops(1),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			let event_id = Game::airdrop_event_id(game_index, 0);
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
				Some(alias_proofs(1)),
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
				0,
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
			assert_ok!(Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 1, 0, id_to_account(99),));
			assert!(!AirdropWinners::<Test>::contains_key(
				Game::airdrop_event_id(1, 0),
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
			assert_ok!(Game::claim_airdrop(
				RuntimeOrigin::signed(ALICE),
				7,
				0,
				beneficiary.clone(),
			));
			// Account-registrant entry consumed.
			assert!(!AirdropWinners::<Test>::contains_key(
				Game::airdrop_event_id(7, 0),
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
				0,
				beneficiary.clone(),
			));
			// Alias-registrant entry consumed.
			assert!(!AirdropWinners::<Test>::contains_key(
				Game::airdrop_event_id(9, 0),
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
			let event_id = Game::airdrop_event_id(last_game_index, 0);
			stage_claim_for(last_game_index, RegistrationEntry::Account { account_id: ALICE });
			assert_ok!(Game::claim_airdrop(
				RuntimeOrigin::signed(ALICE),
				last_game_index,
				0,
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
				airdrops: test_airdrops(1),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			let event_id = Game::airdrop_event_id(game_index, 0);

			// Open airdrop registration and register ALICE as a participant while the
			// game is still in its Registration phase.
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
				Some(crate::AirdropVrfs::Account(vec![signature].try_into().unwrap())),
			));

			// The game is still in Registration and the airdrop has one participant.
			assert!(matches!(
				GameStorage::<Test>::get().expect("game remains running").state,
				GameState::Registration { .. },
			));
			assert!(matches!(
				AirdropEvents::<Test>::get(event_id).expect("event registering").status,
				Status::Registering { total_participants: 1 },
			));

			// Make ALICE eligible to claim, so the rejection below is specifically due to
			// the cancelled (no-longer-claiming) airdrop rather than claim ineligibility.
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
			// The draw waits for randomness produced after the close; advance the mock
			// source before capturing the entropy.
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
					0,
					id_to_account(98),
				))
			}));

			// Cancelling the game cancels its airdrop.
			assert_ok!(Game::cancel_game(RuntimeOrigin::root()));

			// The event was already `Claiming` with one winner drawn, so cancellation moves it to
			// `ClearingRegistrations` preserving the drawn winner count.
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
				Game::claim_airdrop(RuntimeOrigin::signed(ALICE), game_index, 0, id_to_account(99)),
				indiv_pallet_airdrop::Error::<Test>::NotClaiming,
			);
		});
	}

	#[test]
	fn cancel_game_cancels_all_scheduled_airdrops() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(3),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			// The airdrop events are scheduled but not yet started.
			for airdrop_index in 0..3 {
				let event_id = Game::airdrop_event_id(game_index, airdrop_index);
				assert!(matches!(
					AirdropEvents::<Test>::get(event_id).expect("event scheduled").status,
					Status::Scheduled,
				));
			}
			assert_ok!(Game::cancel_game(RuntimeOrigin::root()));
			// Cancelling not-yet-started events drops every one of them and releases their prize
			// allocations.
			for airdrop_index in 0..3 {
				let event_id = Game::airdrop_event_id(game_index, airdrop_index);
				assert!(AirdropEvents::<Test>::get(event_id).is_none());
			}
		});
	}

	#[test]
	fn new_game_schedules_multiple_airdrops_with_per_index_timing() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(3),
			};
			let now = <Test as crate::Config>::UnixTime::now().as_secs();
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			let game = GameStorage::<Test>::get().expect("game exists");
			assert_eq!(game.airdrops_scheduled, 3);

			for airdrop_index in 0..3u8 {
				let event_id = Game::airdrop_event_id(game_index, airdrop_index);
				let event = AirdropEvents::<Test>::get(event_id).expect("event scheduled");
				// Registration opens immediately, the draw happens at its per-index offset and
				// the claim window runs from the draw.
				let draw_time = 10 + airdrop_index as u64 * 86_400;
				assert_eq!(event.info.registration_starts, now);
				assert_eq!(event.info.draw_time, draw_time);
				assert_eq!(event.info.end_time, draw_time + TEST_AIRDROP_CLAIM_WINDOW);
				assert_eq!(event.info.prize, test_airdrop_prize());
				System::assert_has_event(
					crate::Event::<Test>::AirdropScheduled { game_index, airdrop_index, event_id }
						.into(),
				);
			}
		});
	}

	#[test]
	fn new_game_with_empty_airdrops_schedules_no_event() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: Default::default(),
			};
			assert_ok!(Game::new_game(&schedule));
			let game = GameStorage::<Test>::get().expect("game exists");
			assert_eq!(game.airdrops_scheduled, 0);
			assert_eq!(indiv_pallet_airdrop::Events::<Test>::iter().count(), 0);
			// Neither success nor failure events: no scheduling was attempted.
			for record in System::events() {
				assert!(!matches!(
					record.event,
					RuntimeEvent::Game(
						crate::Event::<Test>::AirdropScheduled { .. } |
							crate::Event::<Test>::AirdropScheduleFailed { .. }
					),
				));
			}
		});
	}

	/// Occupy the event id of airdrop index 1 of the upcoming game so its scheduling fails
	/// with `DuplicateEventId`, and return that foreign event's id.
	fn occupy_next_game_airdrop_index_1() -> indiv_pallet_airdrop::types::EventId {
		let next_game_index = GameIndex::<Test>::get() + 1;
		let occupied = Game::airdrop_event_id(next_game_index, 1);
		AirdropEvents::<Test>::insert(
			occupied,
			ActiveEvent {
				id: occupied,
				info: EventInfo {
					prize: test_airdrop_prize(),
					registration_starts: 0,
					draw_time: 1,
					end_time: 2,
				},
				status: Status::Scheduled,
				source: None,
			},
		);
		occupied
	}

	#[test]
	fn schedule_failure_stops_at_first_failing_airdrop() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			let occupied = occupy_next_game_airdrop_index_1();

			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(3),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();

			// Scheduling stops at the failing index: only the prefix (index 0) is scheduled,
			// index 2 is not attempted.
			let game = GameStorage::<Test>::get().expect("game exists");
			assert_eq!(game.airdrops_scheduled, 1);
			assert!(AirdropEvents::<Test>::get(Game::airdrop_event_id(game_index, 2)).is_none());
			System::assert_has_event(
				crate::Event::<Test>::AirdropScheduleFailed {
					game_index,
					airdrop_index: 1,
					error: indiv_pallet_airdrop::Error::<Test>::DuplicateEventId.into(),
				}
				.into(),
			);

			// A sign-up registers into exactly the scheduled prefix; the foreign event occupying
			// index 1's id is never touched.
			open_airdrop_events(game_index, 1);
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(ALICE),
				DEFAULT_IDENTIFIER_KEY,
				Some(alice_airdrop_vrfs(game_index, 1)),
			));
			let slot = alice_event_slot(game_index, 0);
			assert!(AirdropRegistrations::<Test>::contains_key(
				Game::airdrop_event_id(game_index, 0),
				slot,
			));
			assert!(AirdropRegistrations::<Test>::iter_prefix(occupied).next().is_none());
		});
	}

	#[test]
	fn cancellation_only_touches_scheduled_airdrops() {
		new_test_ext().execute_with(|| {
			// Same partial-failure setup: index 1's id is occupied by a foreign event, so only
			// index 0 is scheduled.
			let occupied = occupy_next_game_airdrop_index_1();
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(3),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();

			assert_ok!(Game::cancel_game(RuntimeOrigin::root()));

			// The scheduled event (index 0) is cancelled and dropped; the foreign event
			// occupying index 1's id must not be cancelled with it.
			assert!(AirdropEvents::<Test>::get(Game::airdrop_event_id(game_index, 0)).is_none());
			assert!(matches!(
				AirdropEvents::<Test>::get(occupied).expect("foreign event untouched").status,
				Status::Scheduled,
			));
		});
	}

	#[test]
	fn shuffle_deadline_cancellation_cancels_all_airdrops() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(2),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();

			// Force the game into the shuffle phase and move time past the shuffle deadline.
			let deadline = GameStorage::<Test>::get().expect("game exists").shuffle_deadline;
			GameStorage::<Test>::mutate(|maybe_game| {
				maybe_game.as_mut().expect("game exists").state =
					GameState::Shuffle { step: ShuffleStep::Step1Insert { last_iteration: None } };
			});
			MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs(deadline as u64 + 1));
			advance_process_with_on_poll_only();

			assert!(matches!(
				GameStorage::<Test>::get().expect("game cancelling").state,
				GameState::Cancelling { .. },
			));
			for airdrop_index in 0..2 {
				let event_id = Game::airdrop_event_id(game_index, airdrop_index);
				assert!(AirdropEvents::<Test>::get(event_id).is_none());
			}
		});
	}

	#[test]
	fn sign_up_registers_into_every_scheduled_event() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(3),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			open_airdrop_events(game_index, 3);

			// Account path: one VRF per event, each landing at its own per-event slot.
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(ALICE),
				DEFAULT_IDENTIFIER_KEY,
				Some(alice_airdrop_vrfs(game_index, 3)),
			));

			// Alias path: one proof per event; the mock member service derives the slot from
			// the encoded registration entry, so it is the same in every event.
			let alias: Alias = [0xAA; 32];
			let stmt_account = id_to_account(0xA11A5);
			assert_ok!(Game::sign_up_with_alias(
				runtime_origin_for_alias(&alias),
				DEFAULT_IDENTIFIER_KEY,
				stmt_account.clone(),
				AccountAuthority(stmt_account),
				Some(alias_proofs(3)),
			));
			let alias_entry = RegistrationEntry::<AccountId32>::Alias { alias };
			let alias_slot = BigEndianU256::from(sp_io::hashing::blake2_256(&alias_entry.encode()));

			for airdrop_index in 0..3 {
				let event_id = Game::airdrop_event_id(game_index, airdrop_index);
				assert_eq!(
					AirdropRegistrations::<Test>::get(
						event_id,
						alice_event_slot(game_index, airdrop_index),
					),
					Some(RegistrationEntry::Account { account_id: ALICE }),
				);
				assert_eq!(
					AirdropRegistrations::<Test>::get(event_id, alias_slot),
					Some(alias_entry.clone()),
				);
			}
		});
	}

	#[test]
	fn sign_up_fails_when_one_scheduled_event_not_accepting() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(3),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			// Open only indices 0 and 2; index 1 stays `Scheduled` and rejects registrations.
			set_event_status(
				Game::airdrop_event_id(game_index, 0),
				Status::Registering { total_participants: 0 },
			);
			set_event_status(
				Game::airdrop_event_id(game_index, 2),
				Status::Registering { total_participants: 0 },
			);

			// The whole sign-up fails and leaves no trace, including the registration the
			// airdrop pallet already wrote into event 0 before failing on event 1.
			assert_noop!(
				Game::sign_up_with_account(
					RuntimeOrigin::signed(ALICE),
					DEFAULT_IDENTIFIER_KEY,
					Some(alice_airdrop_vrfs(game_index, 3)),
				),
				indiv_pallet_airdrop::Error::<Test>::NotAcceptingRegistrations,
			);
			assert!(Players::<Test>::get(AccountOrPerson::Account(ALICE)).is_none());
		});
	}

	#[test]
	fn sign_up_rejects_vrf_bound_to_another_event() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(2),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			open_airdrop_events(game_index, 2);

			// Entries are bound positionally: swapping the two per-event VRFs must be rejected
			// when the first entry (bound to event 1) is verified against event 0.
			let (_public, signature_0) = alice_event_vrf(game_index, 0);
			let (_public, signature_1) = alice_event_vrf(game_index, 1);
			let swapped =
				crate::AirdropVrfs::Account(vec![signature_1, signature_0].try_into().unwrap());
			assert_noop!(
				Game::sign_up_with_account(
					RuntimeOrigin::signed(ALICE),
					DEFAULT_IDENTIFIER_KEY,
					Some(swapped),
				),
				indiv_pallet_airdrop::Error::<Test>::InvalidVrfProof,
			);
		});
	}

	#[test]
	fn sign_up_rejects_vrf_count_mismatch() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(2),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			open_airdrop_events(game_index, 2);

			// One entry for two scheduled events: rejected before any verification.
			assert_noop!(
				Game::sign_up_with_account(
					RuntimeOrigin::signed(ALICE),
					DEFAULT_IDENTIFIER_KEY,
					Some(alice_airdrop_vrfs(game_index, 1)),
				),
				Error::<Test>::InvalidAirdropVrfCount,
			);
		});
	}

	#[test]
	fn sign_up_rejects_vrf_from_previous_game() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(1),
			};
			// Game 1: ALICE prepares a VRF for its airdrop event but never signs up; the game
			// is cancelled and cleaned up.
			assert_ok!(Game::new_game(&schedule));
			let first_game_index = GameIndex::<Test>::get();
			let (_public, stale_signature) = alice_event_vrf(first_game_index, 0);
			assert_ok!(Game::cancel_game(RuntimeOrigin::root()));
			advance_process();
			assert!(GameStorage::<Test>::get().is_none());

			// Game 2: the event id embeds the game index, so game 1's VRF is a replay and must
			// be rejected.
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			assert_ne!(game_index, first_game_index);
			open_airdrop_events(game_index, 1);
			assert_noop!(
				Game::sign_up_with_account(
					RuntimeOrigin::signed(ALICE),
					DEFAULT_IDENTIFIER_KEY,
					Some(crate::AirdropVrfs::Account(vec![stale_signature].try_into().unwrap())),
				),
				indiv_pallet_airdrop::Error::<Test>::InvalidVrfProof,
			);

			// A VRF bound to the current game's event goes through.
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(ALICE),
				DEFAULT_IDENTIFIER_KEY,
				Some(alice_airdrop_vrfs(game_index, 1)),
			));
		});
	}

	#[test]
	fn claim_airdrop_selects_event_by_index() {
		new_test_ext().execute_with(|| {
			let alice_entry = RegistrationEntry::<AccountId32>::Account { account_id: ALICE };
			force_participant(AccountOrPerson::Account(ALICE), Recognition::Recognized(0), Some(5));

			// Index 0: claimable with a winning entry for ALICE.
			stage_claim_for_index(5, 0, alice_entry.clone());
			// Index 1: claimable, but ALICE is not among the winners.
			stage_claim_for_index(5, 1, alice_entry.clone());
			AirdropWinners::<Test>::remove(Game::airdrop_event_id(5, 1), &alice_entry);
			// Index 2: drawn later, still registering — claims are not open yet.
			stage_claim_for_index(5, 2, alice_entry.clone());
			set_event_status(
				Game::airdrop_event_id(5, 2),
				Status::Registering { total_participants: 1 },
			);

			// Index 3 was never scheduled.
			assert_noop!(
				Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 5, 3, id_to_account(99)),
				indiv_pallet_airdrop::Error::<Test>::UnknownEvent,
			);
			// Winning the day-0 draw grants nothing for day 1.
			assert_noop!(
				Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 5, 1, id_to_account(99)),
				indiv_pallet_airdrop::Error::<Test>::NoSuchWinner,
			);
			// Day 2 has not drawn yet.
			assert_noop!(
				Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 5, 2, id_to_account(99)),
				indiv_pallet_airdrop::Error::<Test>::NotClaiming,
			);
			// The index ALICE actually won pays out.
			assert_ok!(Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 5, 0, id_to_account(99)));
		});
	}

	#[test]
	fn claim_each_won_airdrop_separately() {
		new_test_ext().execute_with(|| {
			let alice_entry = RegistrationEntry::<AccountId32>::Account { account_id: ALICE };
			force_participant(AccountOrPerson::Account(ALICE), Recognition::Recognized(0), Some(5));
			stage_claim_for_index(5, 0, alice_entry.clone());
			stage_claim_for_index(5, 1, alice_entry.clone());

			let beneficiary = id_to_account(123);
			let prize_amount = test_airdrop_prize().asset_amount;
			assert_ok!(Game::claim_airdrop(
				RuntimeOrigin::signed(ALICE),
				5,
				0,
				beneficiary.clone()
			));
			assert_ok!(Game::claim_airdrop(
				RuntimeOrigin::signed(ALICE),
				5,
				1,
				beneficiary.clone()
			));
			assert_eq!(
				<Assets as Inspect<AccountId32>>::balance(TEST_AIRDROP_ASSET_ID, &beneficiary),
				2 * prize_amount,
			);
			// Both winning entries are consumed; a second claim on either index fails.
			assert_noop!(
				Game::claim_airdrop(RuntimeOrigin::signed(ALICE), 5, 0, beneficiary),
				indiv_pallet_airdrop::Error::<Test>::NoSuchWinner,
			);
		});
	}

	#[test]
	fn extension_validate_accepts_empty_airdrop_when_none_scheduled() {
		new_test_ext().execute_with(|| {
			break_airdrop_schedule();
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(1),
			};
			assert_ok!(Game::new_game(&schedule));
			assert_eq!(GameStorage::<Test>::get().expect("game exists").airdrops_scheduled, 0);

			let ticket = 45u64;
			let signature = TestSignature(ticket, ALICE.encode());
			assert_ok!(Game::grant_invites(RuntimeOrigin::root(), BOB, 1));
			assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(BOB), ticket));
			let nonce = frame_system::Account::<Test>::get(&ALICE).nonce;
			let tx_ext = GameAsInvitedData { nonce, inviter: BOB, ticket, signature };

			// With no scheduled event the VRF count must be zero; a `Some` carrying zero VRFs is
			// accepted and the sign-up lands without any registration.
			assert_ok!(exec_invited_tx(
				ALICE,
				tx_ext,
				Call::sign_up_with_invite {
					identifier_key: DEFAULT_IDENTIFIER_KEY,
					airdrops: Some(account_proofs(0)),
				},
			));
			assert!(Players::<Test>::get(AccountOrPerson::Account(ALICE)).is_some());
			assert_eq!(indiv_pallet_airdrop::Registrations::<Test>::iter().count(), 0);
		});
	}

	#[test]
	fn extension_validate_rejects_airdrop_when_none_scheduled() {
		new_test_ext().execute_with(|| {
			break_airdrop_schedule();
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(1),
			};
			assert_ok!(Game::new_game(&schedule));
			assert_eq!(GameStorage::<Test>::get().expect("game exists").airdrops_scheduled, 0);

			let ticket = 45u64;
			let signature = TestSignature(ticket, ALICE.encode());
			assert_ok!(Game::grant_invites(RuntimeOrigin::root(), BOB, 1));
			assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(BOB), ticket));
			let nonce = frame_system::Account::<Test>::get(&ALICE).nonce;
			let tx_ext = GameAsInvitedData { nonce, inviter: BOB, ticket, signature };

			// Supplying a VRF for a game that scheduled no event is rejected at validation: the
			// VRF count (1) does not match the scheduled count (0).
			assert_noop!(
				exec_invited_tx(
					ALICE,
					tx_ext,
					Call::sign_up_with_invite {
						identifier_key: DEFAULT_IDENTIFIER_KEY,
						airdrops: Some(account_proofs(1)),
					},
				),
				TransactionExecutionError::Validity(
					InvalidTransaction::Custom(CustomError::InvalidAirdropVrfCount as u8).into(),
				),
			);
		});
	}

	#[test]
	fn extension_validate_accepts_no_airdrop_with_scheduled_events() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(2),
			};
			assert_ok!(Game::new_game(&schedule));

			let ticket = 46u64;
			let signature = TestSignature(ticket, ALICE.encode());
			assert_ok!(Game::grant_invites(RuntimeOrigin::root(), BOB, 1));
			assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(BOB), ticket));
			let nonce = frame_system::Account::<Test>::get(&ALICE).nonce;
			let tx_ext = GameAsInvitedData { nonce, inviter: BOB, ticket, signature };

			assert_ok!(exec_invited_tx(
				ALICE,
				tx_ext,
				Call::sign_up_with_invite {
					identifier_key: DEFAULT_IDENTIFIER_KEY,
					airdrops: None
				},
			));
			assert!(Players::<Test>::get(AccountOrPerson::Account(ALICE)).is_some());
			assert_eq!(indiv_pallet_airdrop::Registrations::<Test>::iter().count(), 0);
		});
	}

	#[test]
	fn extension_validate_rejects_when_one_event_not_accepting() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(2),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			// Only the first event accepts registrations; the second stays `Scheduled`.
			open_airdrop_events(game_index, 1);

			let vrfs = alice_airdrop_vrfs(game_index, 2);
			let ticket = 47u64;
			let invite_sig = TestSignature(ticket, ALICE.encode());
			assert_ok!(Game::grant_invites(RuntimeOrigin::root(), BOB, 1));
			assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(BOB), ticket));
			let nonce = frame_system::Account::<Test>::get(&ALICE).nonce;
			let tx_ext = GameAsInvitedData { nonce, inviter: BOB, ticket, signature: invite_sig };

			// A valid VRF is not enough: the registration must be acceptable by every scheduled
			// event for the transaction to validate.
			assert_noop!(
				exec_invited_tx(
					ALICE,
					tx_ext,
					Call::sign_up_with_invite {
						identifier_key: DEFAULT_IDENTIFIER_KEY,
						airdrops: Some(vrfs),
					},
				),
				TransactionExecutionError::Validity(
					InvalidTransaction::Custom(CustomError::InvalidAirdropRegistration as u8)
						.into(),
				),
			);
		});
	}

	#[test]
	fn extension_validate_rejects_vrf_count_mismatch() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(2),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			open_airdrop_events(game_index, 2);

			let ticket = 48u64;
			let invite_sig = TestSignature(ticket, ALICE.encode());
			assert_ok!(Game::grant_invites(RuntimeOrigin::root(), BOB, 1));
			assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(BOB), ticket));
			let nonce = frame_system::Account::<Test>::get(&ALICE).nonce;
			let tx_ext = GameAsInvitedData { nonce, inviter: BOB, ticket, signature: invite_sig };

			// One valid VRF for two scheduled events: rejected with the dedicated custom error
			// before any verification runs.
			assert_noop!(
				exec_invited_tx(
					ALICE,
					tx_ext,
					Call::sign_up_with_invite {
						identifier_key: DEFAULT_IDENTIFIER_KEY,
						airdrops: Some(alice_airdrop_vrfs(game_index, 1)),
					},
				),
				TransactionExecutionError::Validity(
					InvalidTransaction::Custom(CustomError::InvalidAirdropVrfCount as u8).into(),
				),
			);
		});
	}

	#[test]
	fn invited_sign_up_registers_into_every_scheduled_event() {
		new_test_ext().execute_with(|| {
			let schedule = GameSchedule::<u32, u128> {
				game_play_time: 10,
				rounds: 2,
				max_group_size: 3,
				airdrops: test_airdrops(2),
			};
			assert_ok!(Game::new_game(&schedule));
			let game_index = GameIndex::<Test>::get();
			open_airdrop_events(game_index, 2);

			let ticket = 49u64;
			let invite_sig = TestSignature(ticket, ALICE.encode());
			assert_ok!(Game::grant_invites(RuntimeOrigin::root(), BOB, 1));
			assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(BOB), ticket));
			let nonce = frame_system::Account::<Test>::get(&ALICE).nonce;
			let tx_ext = GameAsInvitedData { nonce, inviter: BOB, ticket, signature: invite_sig };

			// The full invited transaction (validate then dispatch) lands and registers the
			// player into every scheduled event.
			assert_ok!(exec_invited_tx(
				ALICE,
				tx_ext,
				Call::sign_up_with_invite {
					identifier_key: DEFAULT_IDENTIFIER_KEY,
					airdrops: Some(alice_airdrop_vrfs(game_index, 2)),
				},
			));
			assert!(Players::<Test>::get(AccountOrPerson::Account(ALICE)).is_some());
			for airdrop_index in 0..2 {
				assert_eq!(
					AirdropRegistrations::<Test>::get(
						Game::airdrop_event_id(game_index, airdrop_index),
						alice_event_slot(game_index, airdrop_index),
					),
					Some(RegistrationEntry::Account { account_id: ALICE }),
				);
			}
		});
	}

	// The sign-up calls charge their exact weight up front from the call arguments: the
	// supplied variant picks the branch (account entries only verify for new players, alias
	// entries only for recognized ones) and the entry count scales it. No airdrops means the
	// branch is unknown, so the worst case of both at zero entries is charged.
	#[test]
	fn sign_up_calls_charge_by_airdrop_variant_and_count() {
		use frame_support::dispatch::GetDispatchInfo;

		let charged = |call: crate::Call<Test>| call.get_dispatch_info().call_weight;

		assert_eq!(
			charged(Call::sign_up_with_account {
				identifier_key: DEFAULT_IDENTIFIER_KEY,
				airdrops: Some(account_proofs(2)),
			}),
			<MockWeightInfo as WeightInfo>::sign_up_with_account_new(2),
		);
		assert_eq!(
			charged(Call::sign_up_with_account {
				identifier_key: DEFAULT_IDENTIFIER_KEY,
				airdrops: Some(alias_proofs(3)),
			}),
			<MockWeightInfo as WeightInfo>::sign_up_with_account_recognized(3),
		);
		assert_eq!(
			charged(Call::sign_up_with_account {
				identifier_key: DEFAULT_IDENTIFIER_KEY,
				airdrops: None,
			}),
			<MockWeightInfo as WeightInfo>::sign_up_with_account_new(0)
				.max(<MockWeightInfo as WeightInfo>::sign_up_with_account_recognized(0)),
		);

		assert_eq!(
			charged(Call::sign_up_with_invite {
				identifier_key: DEFAULT_IDENTIFIER_KEY,
				airdrops: Some(account_proofs(2)),
			}),
			<MockWeightInfo as WeightInfo>::sign_up_with_invite(2),
		);
		assert_eq!(
			charged(Call::sign_up_with_invite {
				identifier_key: DEFAULT_IDENTIFIER_KEY,
				airdrops: None,
			}),
			<MockWeightInfo as WeightInfo>::sign_up_with_invite(0),
		);

		let stmt_account = id_to_account(0xA11A5);
		assert_eq!(
			charged(Call::sign_up_with_alias {
				identifier_key: DEFAULT_IDENTIFIER_KEY,
				statement_account: stmt_account.clone(),
				sig: AccountAuthority(stmt_account),
				airdrops: Some(alias_proofs(2)),
			}),
			<MockWeightInfo as WeightInfo>::sign_up_with_alias(2),
		);
	}

	// `GameAsInvited` charges its validation weight from the call's airdrop entry count; a call
	// that is not `sign_up_with_invite` fails validation before any airdrop work and an unused
	// extension is free.
	#[test]
	fn extension_weight_charges_by_call_airdrop_count() {
		use sp_runtime::traits::TransactionExtension as _;

		let ticket = 1u64;
		let ext = crate::GameAsInvited::<Test>::new(Some(GameAsInvitedData {
			nonce: 0,
			inviter: BOB,
			ticket,
			signature: TestSignature(ticket, ALICE.encode()),
		}));

		let signup: RuntimeCall = Call::sign_up_with_invite {
			identifier_key: DEFAULT_IDENTIFIER_KEY,
			airdrops: Some(account_proofs(2)),
		}
		.into();
		assert_eq!(ext.weight(&signup), <MockWeightInfo as WeightInfo>::as_invited_tx_ext(2));

		let no_airdrops: RuntimeCall =
			Call::sign_up_with_invite { identifier_key: DEFAULT_IDENTIFIER_KEY, airdrops: None }
				.into();
		assert_eq!(ext.weight(&no_airdrops), <MockWeightInfo as WeightInfo>::as_invited_tx_ext(0));

		let other: RuntimeCall = Call::cancel_game {}.into();
		assert_eq!(ext.weight(&other), <MockWeightInfo as WeightInfo>::as_invited_tx_ext(0));

		let unused = crate::GameAsInvited::<Test>::new(None);
		assert_eq!(unused.weight(&signup), Weight::zero());
	}

	#[test]
	fn airdrop_id_derivation_matches_layout() {
		// The event id layout: 27-byte base, the airdrop index and the game index BE encoded.
		let event_id = Game::airdrop_event_id(7, 3);
		let mut expected = [0u8; 32];
		expected[0..27].copy_from_slice(b"pop:game:airdrop:          ");
		expected[27] = 3;
		expected[28..32].copy_from_slice(&7u32.to_be_bytes());
		assert_eq!(event_id, expected);

		// Sibling events differ only in the airdrop index byte; other games differ in the game
		// index bytes.
		assert_eq!(Game::airdrop_event_id(7, 4)[27], 4);
		assert_eq!(Game::airdrop_event_id(7, 4)[28..32], event_id[28..32]);
		assert_ne!(Game::airdrop_event_id(8, 3)[28..32], event_id[28..32]);
	}
}

/// Directly drives `shuffle_step_retrieve` in a tight loop within one block. Covers three
/// invariants the cache-resume optimization must preserve:
///   1. every seeded player from both `ShuffleRecognized` and `ShuffleNotRecognized` is consumed
///      exactly once across all rounds;
///   2. the recognized → not-recognized phase transition does not skip any not-recognized entries
///      (regression guard for the per-phase cache reset);
///   3. `PlayerToIndex` / `IndexToPlayer` end up consistent: each player's per-round index points
///      back at that player, and indices are assigned densely from 0.
#[test]
fn shuffle_step_retrieve_resumes_and_drains_both_phases() {
	new_test_ext().execute_with(|| {
		const ROUNDS: u8 = 3;
		const RECOGNIZED: u32 = 7;
		const NOT_RECOGNIZED: u32 = 5;
		let total = RECOGNIZED + NOT_RECOGNIZED;

		// Seed both maps for every round. Hashes are arbitrary but unique per
		// (round, player) so iteration order is well-defined under `Identity`.
		let mut expected: Vec<AccountOrPerson<AccountId32>> = Vec::new();
		for i in 0..RECOGNIZED {
			let player = AccountOrPerson::Account(id_to_account(i as u64));
			expected.push(player.clone());
			for round in 0..ROUNDS {
				let mut hash = [0u8; 32];
				hash[0] = round;
				hash[1..5].copy_from_slice(&i.to_be_bytes());
				crate::ShuffleRecognized::<Test>::insert(round, hash, &player);
			}
		}
		for i in 0..NOT_RECOGNIZED {
			let player = AccountOrPerson::Person(id_to_alias((RECOGNIZED + i) as u64));
			expected.push(player.clone());
			for round in 0..ROUNDS {
				let mut hash = [0xFFu8; 32];
				hash[0] = round;
				hash[1..5].copy_from_slice(&i.to_be_bytes());
				crate::ShuffleNotRecognized::<Test>::insert(round, hash, &player);
			}
		}

		// Drive the function to completion in this single block.
		let mut next_player_index: u32 = 0;
		let mut phase = ShuffleRetrievePhase::Recognized;
		let mut resume_cursors: Vec<Option<[u8; 32]>> = vec![None; usize::from(ROUNDS)];
		let mut transition_seen = false;
		loop {
			let was_recognized = matches!(phase, ShuffleRetrievePhase::Recognized);
			let res = crate::Pallet::<Test>::shuffle_step_retrieve(
				&mut next_player_index,
				&mut phase,
				&mut resume_cursors,
				ROUNDS,
			);
			if was_recognized && matches!(phase, ShuffleRetrievePhase::NotRecognized { .. }) {
				transition_seen = true;
				// On transition the cache must be reset so the not-recognized phase starts
				// from the prefix root rather than after a stale recognized key.
				assert!(
					resume_cursors.iter().all(|k| k.is_none()),
					"cache must be reset on recognized → not-recognized transition",
				);
			}
			if res == crate::StepResult::Finished {
				break;
			}
		}

		assert!(transition_seen, "test must exercise the phase transition");

		// `recognized_count` must capture exactly the number of recognized players. This is the
		// boundary `shuffle_step_compute_weights` relies on to derive vote weights arithmetically.
		let ShuffleRetrievePhase::NotRecognized { recognized_count } = phase else {
			panic!("retrieve must finish in the not-recognized phase");
		};
		assert_eq!(
			recognized_count, RECOGNIZED,
			"recognized_count must equal the number of recognized players",
		);

		// Index band invariant: in every round, indices `[0, recognized_count)` map to recognized
		// players and `[recognized_count, total)` to not-recognized ones. The first `RECOGNIZED`
		// entries of `expected` are the recognized players (seeded into `ShuffleRecognized`).
		let recognized_players = &expected[..RECOGNIZED as usize];
		for round in 0..ROUNDS {
			for idx in 0..total {
				let player = IndexToPlayer::<Test>::get((round, idx)).unwrap();
				assert_eq!(
					recognized_players.contains(&player),
					idx < recognized_count,
					"round {round} idx {idx}: recognized/not-recognized band mismatch",
				);
			}
		}

		// Both source maps must be fully drained.
		for round in 0..ROUNDS {
			assert_eq!(
				crate::ShuffleRecognized::<Test>::iter_prefix(round).count(),
				0,
				"ShuffleRecognized round {round} not fully drained",
			);
			assert_eq!(
				crate::ShuffleNotRecognized::<Test>::iter_prefix(round).count(),
				0,
				"ShuffleNotRecognized round {round} not fully drained",
			);
		}

		// Every seeded player must have an entry in `PlayerToIndex` with the right shape.
		for player in &expected {
			let indices =
				PlayerToIndex::<Test>::get(player).expect("seeded player must have indices");
			assert_eq!(indices.len(), usize::from(ROUNDS));
		}

		// `IndexToPlayer` must contain exactly `total` entries per round and round-trip
		// through `PlayerToIndex`.
		for round in 0..ROUNDS {
			for idx in 0..total {
				let player = IndexToPlayer::<Test>::get((round, idx)).unwrap_or_else(|| {
					panic!("missing IndexToPlayer entry for round {round} idx {idx}")
				});
				let player_indices =
					PlayerToIndex::<Test>::get(&player).expect("player must have indices");
				assert_eq!(
					player_indices[usize::from(round)],
					idx,
					"IndexToPlayer/PlayerToIndex disagree for round {round}",
				);
			}
			assert_eq!(
				IndexToPlayer::<Test>::iter().filter(|((r, _), _)| *r == round).count(),
				total as usize,
				"unexpected entry count in IndexToPlayer for round {round}",
			);
		}
	});
}

/// Verify that the default mock configuration passes all integrity checks,
/// including the block-fit assertion for the `report` extrinsic.
#[test]
fn integrity_test_passes() {
	new_test_ext().execute_with(|| {
		<crate::Pallet<Test> as Hooks<u64>>::integrity_test();
	});
}

/// Negative coverage for every reachable failure branch of `GameAsInvited::validate`.
///
/// Each test builds an otherwise-valid invited sign-up and perturbs a single precondition so the
/// extension rejects the transaction with the matching `CustomError`. The airdrop branches
/// (`InvalidAirdropVrfVariant`, `InvalidAirdropRegistration`) are covered in `mod airdrop`, and
/// `UnexpectedInvalidity` is a defensive path in the transactional layer that the mock cannot
/// trigger.
mod invited_validation {
	use super::*;
	use crate::extension::CustomError;
	use frame_support::dispatch::GetDispatchInfo;
	use sp_runtime::{
		traits::{TransactionExtension, TxBaseImplication},
		transaction_validity::{TransactionSource, TransactionValidityError},
	};

	const TICKET: u64 = 44;

	/// A schedule whose game sits in its registration phase at genesis time (`now == 0`), with
	/// `registration_ends == 8`. No airdrop, so the airdrop validation path is skipped.
	fn registration_schedule() -> GameSchedule<u32, u128> {
		GameSchedule::<u32, u128> {
			game_play_time: 10,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		}
	}

	/// Create a game in its registration phase.
	fn start_registration_game() {
		assert_ok!(Game::new_game(&registration_schedule()));
	}

	/// Grant BOB an invite and register a ticket, so `PendingInvites(BOB, TICKET)` exists.
	fn create_pending_invite() {
		assert_ok!(Game::grant_invites(RuntimeOrigin::root(), BOB, 1));
		assert_ok!(Game::set_invite_ticket(RuntimeOrigin::signed(BOB), TICKET));
	}

	/// The invited-sign-up call ALICE submits.
	fn sign_up_call() -> Call<Test> {
		Call::sign_up_with_invite { identifier_key: DEFAULT_IDENTIFIER_KEY, airdrops: None }
	}

	/// Extension data with a nonce and ticket signature that are valid for ALICE.
	fn valid_data() -> GameAsInvitedData<Test> {
		GameAsInvitedData {
			nonce: frame_system::Account::<Test>::get(&ALICE).nonce,
			inviter: BOB,
			ticket: TICKET,
			signature: TestSignature(TICKET, ALICE.encode()),
		}
	}

	fn assert_rejected(result: Result<(), TransactionExecutionError>, error: CustomError) {
		assert_noop!(
			result,
			TransactionExecutionError::Validity(InvalidTransaction::Custom(error as u8).into())
		);
	}

	// Baseline: the fully valid setup that every negative test perturbs must itself succeed, so
	// each rejection below is attributable to the single precondition it removes.
	#[test]
	fn valid_invited_sign_up_succeeds() {
		new_test_ext().execute_with(|| {
			start_registration_game();
			create_pending_invite();
			assert_ok!(exec_invited_tx(ALICE, valid_data(), sign_up_call()));
			assert!(Players::<Test>::contains_key(AccountOrPerson::Account(ALICE)));
		});
	}

	#[test]
	fn origin_not_signed_rejected() {
		new_test_ext().execute_with(|| {
			start_registration_game();
			create_pending_invite();

			// `exec_invited_tx` always signs, so drive `validate` directly with a `None` origin.
			let call: RuntimeCall = sign_up_call().into();
			let info = call.get_dispatch_info();
			let ext = crate::GameAsInvited::<Test>::new(Some(valid_data()));
			let result = ext.validate(
				frame_system::RawOrigin::None.into(),
				&call,
				&info,
				0,
				(),
				&TxBaseImplication(()),
				TransactionSource::External,
			);
			let expected: TransactionValidityError =
				InvalidTransaction::Custom(CustomError::OriginNotSigned as u8).into();
			assert_eq!(result.err(), Some(expected));
		});
	}

	#[test]
	fn call_not_sign_up_with_invite_rejected() {
		new_test_ext().execute_with(|| {
			start_registration_game();
			create_pending_invite();

			// A signed origin with the extension, but a call that is not `sign_up_with_invite`.
			assert_rejected(
				exec_invited_tx(ALICE, valid_data(), Call::set_invite_ticket { ticket: 1 }),
				CustomError::CallNotSignUpWithInvite,
			);
		});
	}

	#[test]
	fn not_new_player_rejected() {
		new_test_ext().execute_with(|| {
			start_registration_game();
			create_pending_invite();

			// ALICE joins as an account player first, so she is no longer a new player.
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(ALICE),
				DEFAULT_IDENTIFIER_KEY,
				None
			));
			assert!(Players::<Test>::contains_key(AccountOrPerson::Account(ALICE)));

			assert_rejected(
				exec_invited_tx(ALICE, valid_data(), sign_up_call()),
				CustomError::NotNewPlayer,
			);
		});
	}

	#[test]
	fn no_game_rejected() {
		new_test_ext().execute_with(|| {
			// No `new_game`, so `Game::get()` is `None`.
			assert_rejected(
				exec_invited_tx(ALICE, valid_data(), sign_up_call()),
				CustomError::NoGame,
			);
		});
	}

	#[test]
	fn game_registration_ended_rejected() {
		new_test_ext().execute_with(|| {
			start_registration_game();
			create_pending_invite();

			// Advance time to the registration deadline.
			let registration_ends = GameTimes::<Test>::registration_end(&registration_schedule());
			MOCK_UNIX_TIME
				.with(|t| *t.borrow_mut() = Duration::from_secs(registration_ends as u64));

			assert_rejected(
				exec_invited_tx(ALICE, valid_data(), sign_up_call()),
				CustomError::GameRegistrationEnded,
			);
		});
	}

	#[test]
	fn game_not_in_registration_rejected() {
		new_test_ext().execute_with(|| {
			start_registration_game();
			create_pending_invite();

			// Force the game out of its registration state while keeping `now < registration_ends`
			// (the only way to reach this branch, since the state machine would otherwise change
			// state only once the deadline has already passed).
			crate::Game::<Test>::mutate(|game| {
				game.as_mut().expect("game exists").state =
					GameState::Reporting { player_count: 0 };
			});

			assert_rejected(
				exec_invited_tx(ALICE, valid_data(), sign_up_call()),
				CustomError::GameNotInRegistration,
			);
		});
	}

	#[test]
	fn account_already_used_for_stmt_account_rejected() {
		new_test_ext().execute_with(|| {
			start_registration_game();
			create_pending_invite();

			// Mark ALICE as an existing statement account.
			crate::StmtAccountToAlias::<Test>::insert(ALICE, [1u8; 32]);

			assert_rejected(
				exec_invited_tx(ALICE, valid_data(), sign_up_call()),
				CustomError::AccountAlreadyUsedForStmtAccount,
			);
		});
	}

	#[test]
	fn cannot_onboard_already_onboarded_rejected() {
		new_test_ext().execute_with(|| {
			start_registration_game();
			create_pending_invite();

			// ALICE is already a score participant, so onboarding for recognition would fail.
			assert_ok!(indiv_pallet_score::Pallet::<Test>::onboard_for_recognition(&ALICE));

			assert_rejected(
				exec_invited_tx(ALICE, valid_data(), sign_up_call()),
				CustomError::CannotOnboardAlreadyOnboarded,
			);
		});
	}

	#[test]
	fn no_pending_invite_rejected() {
		new_test_ext().execute_with(|| {
			start_registration_game();
			// No `create_pending_invite`, so `PendingInvites(BOB, TICKET)` is absent.
			assert_rejected(
				exec_invited_tx(ALICE, valid_data(), sign_up_call()),
				CustomError::NoPendingInvite,
			);
		});
	}

	#[test]
	fn invalid_signature_rejected() {
		new_test_ext().execute_with(|| {
			start_registration_game();
			create_pending_invite();

			// A ticket signature over the wrong message (BOB instead of the signer ALICE).
			let data = GameAsInvitedData {
				signature: TestSignature(TICKET, BOB.encode()),
				..valid_data()
			};

			assert_rejected(
				exec_invited_tx(ALICE, data, sign_up_call()),
				CustomError::InvalidSignature,
			);
		});
	}

	#[test]
	fn stale_nonce_rejected() {
		new_test_ext().execute_with(|| {
			start_registration_game();
			create_pending_invite();

			// `valid_data` captures ALICE's current nonce (0); bumping the account nonce past it
			// makes the provided nonce stale. This is a raw `InvalidTransaction::Stale`, not a
			// `CustomError`, since the nonce check delegates to `CheckNonce`.
			let data = valid_data();
			frame_system::Account::<Test>::mutate(&ALICE, |account| account.nonce = 1);

			assert_noop!(
				exec_invited_tx(ALICE, data, sign_up_call()),
				TransactionExecutionError::Validity(InvalidTransaction::Stale.into())
			);
		});
	}
}

/// A lite person invites themselves, i.e. the account they bound to their alias in the score
/// context, to play the game without any deposit.
mod sign_up_with_account_lite_invite {
	use super::*;
	use deposit::DepositStorage;
	use frame_support::dispatch::{DispatchResultWithPostInfo, Pays};
	use sp_runtime::DispatchError;
	use sp_statement_store::{get_allowance, StatementAllowance};

	/// The lite person's alias in the score context.
	const LITE_ALIAS: Alias = [7u8; 32];
	/// The account the lite person plays with, e.g. a product key.
	const PRODUCT_KEY: AccountId32 = AccountId32::new(*b"product_key_____________________");

	fn game_schedule(game_play_time: u32) -> GameSchedule<u32, u128> {
		GameSchedule::<u32, u128> {
			game_play_time,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		}
	}

	/// Start a game and return its index. Its registration phase is open, as
	/// `sign_up_with_account_lite_invite` requires.
	fn start_game(game_play_time: u32) -> u32 {
		assert_ok!(Game::new_game(&game_schedule(game_play_time)));
		crate::Game::<Test>::get().expect("a game is ongoing").index
	}

	/// A lite invite of `account` by the lite person of `LITE_ALIAS`.
	fn lite_invite(account: AccountId32) -> DispatchResultWithPostInfo {
		Game::sign_up_with_account_lite_invite(
			runtime_origin_for_lite_alias(&LITE_ALIAS),
			account,
			DEFAULT_IDENTIFIER_KEY,
			None,
		)
	}

	/// A lite person signs their designated account up for free: it is registered for the ongoing
	/// game with an invited credibility, no deposit is taken, the statement allowance is granted
	/// once, and the account goes on to attend the game.
	#[test]
	fn lite_person_signs_up_without_deposit_and_plays() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			let player = AccountOrPerson::Account(PRODUCT_KEY);

			assert_eq!(get_allowance(PRODUCT_KEY), StatementAllowance::default());

			run_game_scenario_with_phase(
				game_schedule(10),
				|| {
					let game_index = crate::Game::<Test>::get().expect("a game is ongoing").index;
					let post_info =
						lite_invite(PRODUCT_KEY).expect("the lite alias can lite invite");

					assert_eq!(post_info.pays_fee, Pays::No);
					assert_eq!(LiteInvites::<Test>::get(LITE_ALIAS), Some(PRODUCT_KEY));
					System::assert_has_event(
						Event::<Test>::LiteInvited { player: PRODUCT_KEY }.into(),
					);
					let player_info =
						Players::<Test>::get(&player).expect("the account is a player");
					assert!(player_info.registered);
					assert!(matches!(player_info.credibility, PlayerCredibility::Invited));
					assert_eq!(player_info.first_game, game_index);
					assert!(indiv_pallet_score::Participants::<Test>::contains_key(&player));
					assert_eq!(get_allowance(PRODUCT_KEY), PlayerStatementLimit::get());
					assert!(DepositStorage::<Test>::get().active.is_empty());
				},
				|| {
					assert_ok!(Game::report(
						RuntimeOrigin::signed(PRODUCT_KEY),
						vec![BoundedVec::new()].try_into().unwrap(),
					));
				},
			);

			assert!(DepositStorage::<Test>::get().active.is_empty());
			assert_eq!(get_allowance(PRODUCT_KEY), PlayerStatementLimit::get());
			assert!(!ArchivedPlayers::<Test>::contains_key(&player));
			let score = indiv_pallet_score::Participants::<Test>::get(&player)
				.expect("the account is a participant")
				.score;
			assert!(score > 0, "the account attended the game it played for free");
		});
	}

	/// The call needs a game whose registration is still open, as any sign-up does: with no game,
	/// past the registration deadline, or past the registration phase, nothing is created.
	#[test]
	fn lite_invite_needs_an_open_registration_phase() {
		new_test_ext().execute_with(|| {
			// No game at all.
			let error = lite_invite(PRODUCT_KEY).expect_err("no game is ongoing");
			assert_eq!(error.error, Error::<Test>::NoGame.into());
			assert_eq!(error.post_info.pays_fee, Pays::Yes);

			// A game whose registration deadline has passed. BOB plays it so it is not cancelled
			// for lack of players.
			let schedule = game_schedule(10);
			assert_ok!(Game::new_game(&schedule));
			assert_ok!(Game::sign_up_with_account(
				RuntimeOrigin::signed(BOB),
				DEFAULT_IDENTIFIER_KEY,
				None,
			));
			let registration_ends = GameTimes::<Test>::registration_end(&schedule);
			MOCK_UNIX_TIME
				.with(|t| *t.borrow_mut() = Duration::from_secs((registration_ends + 1) as u64));
			assert_noop!(lite_invite(PRODUCT_KEY), Error::<Test>::NoRegistration);

			// A game past its registration phase.
			advance_process(); // registration to shuffle
			advance_process(); // shuffle to report
			assert_noop!(lite_invite(PRODUCT_KEY), Error::<Test>::NoRegistration);
			assert!(!Players::<Test>::contains_key(AccountOrPerson::Account(PRODUCT_KEY)));
			assert!(LiteInvites::<Test>::iter().next().is_none());
		});
	}

	/// An archived account is signed up again for free, as a fresh invited player of the new game,
	/// and its statement allowance is granted again.
	#[test]
	fn archived_account_is_signed_up_again() {
		new_test_ext().execute_with(|| {
			let player = AccountOrPerson::Account(PRODUCT_KEY);
			let mut first_game = 0;

			// The account plays its first game but doesn't report, so it is absent, its score
			// stays zero and it gets archived.
			run_game_scenario_with_phase(
				game_schedule(10),
				|| {
					assert_ok!(lite_invite(PRODUCT_KEY));
					first_game = Players::<Test>::get(&player).expect("player").first_game;
				},
				|| {},
			);
			assert!(ArchivedPlayers::<Test>::contains_key(&player));
			assert!(!Players::<Test>::contains_key(&player));
			assert_eq!(get_allowance(PRODUCT_KEY), StatementAllowance::default());

			// The lite person signs the same account up for the next game, which becomes the game
			// it is eligible from, as for any account signing up with an invite.
			let next_game = start_game(30);
			assert_ne!(next_game, first_game);
			assert_ok!(lite_invite(PRODUCT_KEY));
			assert!(!ArchivedPlayers::<Test>::contains_key(&player));
			let player_info = Players::<Test>::get(&player).expect("the account is a player again");
			assert!(player_info.registered);
			assert!(matches!(player_info.credibility, PlayerCredibility::Invited));
			assert_eq!(player_info.first_game, next_game);
			assert_eq!(get_allowance(PRODUCT_KEY), PlayerStatementLimit::get());
			assert!(DepositStorage::<Test>::get().active.is_empty());
		});
	}

	/// A live player must use `sign_up_with_account`: a lite invite while already playing is
	/// rejected, and pays a fee.
	#[test]
	fn lite_invite_of_a_playing_account_fails() {
		new_test_ext().execute_with(|| {
			start_game(10);
			assert_ok!(lite_invite(PRODUCT_KEY));

			let error = lite_invite(PRODUCT_KEY).expect_err("the account is already a player");
			assert_eq!(error.error, Error::<Test>::UseInviteButAlreadyPlaying.into());
			assert_eq!(error.post_info.pays_fee, Pays::Yes);
		});
	}

	/// A lite person invites one account ever: another account bound to the same alias is rejected,
	/// even once the first one stopped playing.
	#[test]
	fn another_account_is_never_invitable() {
		new_test_ext().execute_with(|| {
			let first_player = AccountOrPerson::Account(PRODUCT_KEY);
			start_game(10);
			assert_ok!(lite_invite(PRODUCT_KEY));

			// The lite person names another account, but their alias is already recorded with the
			// first one.
			let error = lite_invite(BOB).expect_err("another account was invited");
			assert_eq!(error.error, Error::<Test>::AnotherAccountInvited.into());
			assert_eq!(error.post_info.pays_fee, Pays::Yes);

			// The first account leaves the game, which is only possible once it is not registered
			// for a game, so the game is cancelled first for lack of other players.
			MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs(100));
			for _ in 0..6 {
				advance_process();
			}
			assert_ok!(Game::offboard(RuntimeOrigin::signed(PRODUCT_KEY)));
			assert!(!Players::<Test>::contains_key(&first_player));

			// The new account is still not invitable, and the recorded one is unchanged.
			start_game(200);
			assert_noop!(lite_invite(BOB), Error::<Test>::AnotherAccountInvited);
			assert_eq!(LiteInvites::<Test>::get(LITE_ALIAS), Some(PRODUCT_KEY));
		});
	}

	/// An account already used as the statement account of a person cannot be signed up, as
	/// statement accounts and players must not overlap.
	#[test]
	fn statement_account_cannot_be_invited() {
		new_test_ext().execute_with(|| {
			StmtAccountToAlias::<Test>::insert(PRODUCT_KEY, [1u8; 32]);
			start_game(10);

			assert_noop!(lite_invite(PRODUCT_KEY), Error::<Test>::StatementAccountAlreadyInUse);
		});
	}

	/// Only a lite alias in the score context can lite invite: a signed origin, a person's alias
	/// and a lite alias in another context are all rejected.
	#[test]
	fn only_a_lite_alias_in_the_score_context_can_lite_invite() {
		new_test_ext().execute_with(|| {
			start_game(10);
			let invite = |origin| {
				Game::sign_up_with_account_lite_invite(
					origin,
					PRODUCT_KEY,
					DEFAULT_IDENTIFIER_KEY,
					None,
				)
			};

			// A signed origin is not a lite alias.
			assert_noop!(invite(RuntimeOrigin::signed(PRODUCT_KEY)), DispatchError::BadOrigin);

			// A personal alias, i.e. a full person, is not a lite alias either. They sign up with
			// `sign_up_with_alias`.
			assert_noop!(invite(runtime_origin_for_alias(&LITE_ALIAS)), DispatchError::BadOrigin);

			// A lite alias in another context than the score context is not accepted.
			assert_noop!(
				invite(runtime_origin_for_lite_alias_in_context(
					&LITE_ALIAS,
					&lite_people_auth_context()
				)),
				DispatchError::BadOrigin
			);

			assert!(LiteInvites::<Test>::iter().next().is_none());
			assert!(Players::<Test>::iter().next().is_none());
		});
	}
}
