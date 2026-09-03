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

//! Tests for the NFT claim credits: awarding, the per-block Merkle trees, the proofs a claimant
//! builds and the delivery of the trees to the claims chain.
//!
//! The mock runs the game as well, since that is what awards a credit, so these are the tests of
//! the pair: what a report and an attendance backfill leave behind, and what happens to it
//! afterwards.

use crate::{mock::*, *};
use frame_support::{
	assert_noop, assert_ok,
	traits::{Hooks, OffchainWorker},
	BoundedVec,
};
use indiv_pallet_game::{
	EarlyAttendanceEnactment, FullReport, Game as GameStore, GameSchedule, GameState, GameTimes,
	GroupsSetting, IndexToPlayer, PlayerProcessStep, PlayerToIndex, Players, Report,
};
use indiv_pallet_score::AccountOrPerson;
use sp_core::{bounded_vec, H256};
use sp_runtime::AccountId32;
use std::time::Duration;

const ALICE: AccountId32 = AccountId32::new(*b"10______________________________");
const BOB: AccountId32 = AccountId32::new(*b"20______________________________");
const CHARLIE: AccountId32 = AccountId32::new(*b"30______________________________");
const DAVE: AccountId32 = AccountId32::new(*b"40______________________________");
const EVE: AccountId32 = AccountId32::new(*b"50______________________________");

fn build_report_with_opinion(
	reporter: &AccountOrPerson<AccountId32>,
	opinion: impl Fn(&AccountOrPerson<AccountId32>) -> Report,
) -> FullReport<Test> {
	let reporter_indices = PlayerToIndex::<Test>::get(reporter).expect("reporter has indices");
	let game = GameStore::<Test>::get().expect("game exists");
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

/// Schedule `schedule`, sign `accounts` up for it and advance to the reporting phase.
fn start_game_in_reporting_phase(schedule: &GameSchedule<u32, u128>, accounts: &[AccountId32]) {
	start_scheduled_game(schedule);
	for acc in accounts {
		assert_ok!(Game::sign_up_with_account(
			RuntimeOrigin::signed(acc.clone()),
			DEFAULT_IDENTIFIER_KEY,
			None,
		));
	}
	advance_to_reporting_phase(schedule);
}

/// Advance a registered game to its reporting phase.
fn advance_to_reporting_phase(schedule: &GameSchedule<u32, u128>) {
	let reg_end = GameTimes::<Test>::registration_end(schedule);
	MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs((reg_end + 1) as u64));
	advance_process(); // registration -> shuffle
	advance_process(); // shuffle -> reporting (steps 1-4)
	assert!(matches!(GameStore::<Test>::get().unwrap().state, GameState::Reporting { .. }));
}

/// Take a scheduled game up to its registration phase, which is where a player can sign up.
///
/// The game is scheduled and then reached through `on_poll`, the way a live chain does it, rather
/// than started directly: `new_game` is the game pallet's own.
fn start_scheduled_game(schedule: &GameSchedule<u32, u128>) {
	assert_ok!(Game::schedule_games(RuntimeOrigin::root(), vec![schedule.clone()]));
	// The clock stays where it is: `new_game` sets a game up *before* its registration starts, and
	// `on_poll` takes the first schedule as soon as there is no game. It skips every
	// `GAME_PROCESS_SKIPPED_BLOCK`th block, so give it a few.
	for _ in 0..4 {
		if matches!(
			GameStore::<Test>::get().map(|game| game.state),
			Some(GameState::Registration { .. })
		) {
			return;
		}
		advance_process();
	}
	panic!("the scheduled game did not open registration");
}

/// The total number of leaves committed across all blocks' trees.
fn committed_leaf_count() -> u32 {
	NftClaimCreditRoots::<Test>::iter().map(|(_, tree)| tree.leaf_count).sum()
}

/// The credits awarded to `claimant` so far, from the `NftClaimCreditAwarded` events of every
/// block the test has run, which outlive the game's own credit state.
fn awarded_credits(claimant: &AccountOrPerson<AccountId32>) -> Vec<NftClaimCredit> {
	recorded_events()
		.into_iter()
		.filter_map(|event| match event {
			RuntimeEvent::NftCredits(Event::<Test>::NftClaimCreditAwarded {
				claimant: awarded_to,
				credit,
				..
			}) if awarded_to == *claimant => Some(credit),
			_ => None,
		})
		.collect::<Vec<_>>()
}

/// The total number of credits awarded so far, over every claimant.
fn awarded_credit_count() -> usize {
	recorded_events()
		.into_iter()
		.filter(|event| {
			matches!(event, RuntimeEvent::NftCredits(Event::<Test>::NftClaimCreditAwarded { .. }))
		})
		.count()
}

#[test]
fn nft_claim_credit_spec() {
	let calculated = NftCredits::compute_nft_claim_credit(
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

	let calculated = NftCredits::compute_nft_claim_credit(
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
fn credit_block_index_keys_the_root_the_credit_lands_in() {
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 100,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		start_scheduled_game(&schedule);
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

		// Pin a deterministic timestamp inside the reporting window, so that the root the
		// credit's block keys can be told apart from the block number itself.
		let report_open = schedule.game_play_time;
		MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs(report_open as u64));

		let award_block = System::block_number();
		let report: FullReport<Test> =
			vec![vec![Report::Person].try_into().unwrap()].try_into().unwrap();
		assert_ok!(Game::report(RuntimeOrigin::signed(ALICE), report.clone()));
		assert_ok!(Game::report(RuntimeOrigin::signed(BOB), report));

		let alice = AccountOrPerson::Account(ALICE);
		let bob = AccountOrPerson::Account(BOB);
		let credit_for_alice = NftCredits::compute_nft_claim_credit(1, 0, &bob, &alice);
		assert!(awarded_credits(&alice).contains(&credit_for_alice));

		// The indexed block is what a wallet redeems with: it keys the root committing to the
		// credit's leaf, which carries the game index and the award timestamp.
		assert_eq!(NftClaimCreditBlocks::<Test>::get(&alice).to_vec(), vec![award_block]);
		advance_process();
		let credit_root = NftClaimCreditRoots::<Test>::get(award_block).expect("the block awarded");
		assert_eq!(credit_root.game_index, 1);
		assert_eq!(credit_root.timestamp, report_open);
	});
}

#[test]
fn notperson_credit_backfilled_when_attendee_attends() {
	// Four-player group, single round. BOB votes NotPerson on ALICE; CHARLIE/DAVE
	// vote Person on her. ALICE ends up Attended (2 yes, 1 no, no remaining). A
	// `NotPerson` vote awards nothing during reporting, but once ALICE is finalised
	// as attended `award_attendance_credits` backfills one credit per group co-member —
	// BOB's included — as awarded credits.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 50,
			rounds: 1,
			max_group_size: 4,
			..Default::default()
		};
		start_scheduled_game(&schedule);
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

		// After ALICE/BOB/CHARLIE: ALICE has yes=1 (CHARLIE), no=1 (BOB), remaining=1
		// → still Pending. BOB's NotPerson vote awarded nothing.
		let credit_from_bob = NftCredits::compute_nft_claim_credit(1, 0, &bob, &alice);
		assert_eq!(Players::<Test>::get(&alice).unwrap().early_attendance_enactment, None);
		assert!(!awarded_credits(&alice).contains(&credit_from_bob));

		// DAVE's Person vote tips ALICE to Attended (2 yes, 1 no, no remaining), so she
		// is early-enacted — but credit materialisation is deferred to player processing,
		// so BOB's credit is still not awarded.
		assert_ok!(Game::report(
			RuntimeOrigin::signed(DAVE),
			build_report_with_opinion(&dave, |_| Report::Person),
		));
		assert!(matches!(
			Players::<Test>::get(&alice).unwrap().early_attendance_enactment,
			Some(EarlyAttendanceEnactment { attendance: true, .. })
		));
		assert!(!awarded_credits(&alice).contains(&credit_from_bob));

		MOCK_UNIX_TIME.with(|v| {
			*v.borrow_mut() =
				Duration::from_secs(GameTimes::<Test>::reporting_end(&schedule) as u64 + 1)
		});
		advance_process(); // report -> player_process step1
		advance_process(); // run step1 (backfills ALICE's co-member credits)

		// ALICE attended, so she earns a credit from every co-member regardless of how
		// they voted — BOB's NotPerson credit is now awarded too.
		assert!(awarded_credits(&alice).contains(&credit_from_bob));
	});
}

#[test]
fn notperson_credit_absent_when_attestee_does_not_attend() {
	// BOB and CHARLIE both vote NotPerson on ALICE; ALICE never reports and is
	// enacted as not attended. `NotPerson` votes never award during reporting, and a
	// non-attendee gets no attendance backfill, so ALICE ends up with no credits.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 50,
			rounds: 1,
			max_group_size: 3,
			..Default::default()
		};
		start_scheduled_game(&schedule);
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
		// NotPerson votes award nothing during reporting.
		assert!(awarded_credits(&alice).is_empty());

		MOCK_UNIX_TIME.with(|v| {
			*v.borrow_mut() =
				Duration::from_secs(GameTimes::<Test>::reporting_end(&schedule) as u64 + 1)
		});
		advance_process(); // report -> player_process step1
		advance_process(); // run step1 (no backfill for a non-attendee)

		// ALICE did not attend, so no credits are ever awarded to her.
		assert!(awarded_credits(&alice).is_empty());
	});
}

#[test]
fn notperson_credit_backfilled_when_attendee_already_attended() {
	// Four-player group, single round. ALICE/BOB/CHARLIE all report all-Person — at
	// that point ALICE is yes=2, no=0, remaining=1 → Attended early. DAVE then
	// reports NotPerson on ALICE: the vote awards nothing, but because ALICE attended
	// she earns a credit from every co-member (DAVE included), backfilled by
	// `award_attendance_credits` when she is processed.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 50,
			rounds: 1,
			max_group_size: 4,
			..Default::default()
		};
		start_scheduled_game(&schedule);
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

		// DAVE reports NotPerson on the already-attended ALICE. The vote awards nothing,
		// and materialisation is deferred, so DAVE's credit is not present yet.
		assert_ok!(Game::report(
			RuntimeOrigin::signed(DAVE),
			build_report_with_opinion(&dave, |target| if target == &alice {
				Report::NotPerson
			} else {
				Report::Person
			},),
		));
		let credit_from_dave = NftCredits::compute_nft_claim_credit(1, 0, &dave, &alice);
		assert!(!awarded_credits(&alice).contains(&credit_from_dave));

		MOCK_UNIX_TIME.with(|v| {
			*v.borrow_mut() =
				Duration::from_secs(GameTimes::<Test>::reporting_end(&schedule) as u64 + 1)
		});
		advance_process(); // report -> player_process step1
		advance_process(); // run step1 (backfills ALICE's co-member credits)

		// ALICE attended, so the credit from DAVE is backfilled from group membership.
		assert!(awarded_credits(&alice).contains(&credit_from_dave));
	});
}

/// The leaf a credit awarded to `claimant` by `attester` in round 0 of game 1 contributes.
fn credit_leaf(
	attester: &AccountOrPerson<AccountId32>,
	claimant: &AccountOrPerson<AccountId32>,
) -> NftClaimCreditLeaf {
	let credit = NftCredits::compute_nft_claim_credit(1, 0, attester, claimant);
	NftCredits::compute_nft_claim_credit_leaf(claimant, &credit)
}

#[test]
fn awarded_credits_are_committed_to_the_awarding_block_root() {
	// Two players reporting `Person` on each other award one credit each. Both leaves are
	// buffered in the block the reports land in and committed as that block's tree by the
	// next block's `on_initialize`.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 100,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		start_game_in_reporting_phase(&schedule, &[ALICE, BOB]);

		let report_open = schedule.game_play_time;
		MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs(report_open as u64));

		let alice = AccountOrPerson::Account(ALICE);
		let bob = AccountOrPerson::Account(BOB);
		let report: FullReport<Test> =
			vec![vec![Report::Person].try_into().unwrap()].try_into().unwrap();

		let award_block = System::block_number();
		assert_ok!(Game::report(RuntimeOrigin::signed(ALICE), report.clone()));
		assert_ok!(Game::report(RuntimeOrigin::signed(BOB), report));

		// ALICE reported first, so BOB's credit is the first leaf.
		let leaves = vec![credit_leaf(&alice, &bob), credit_leaf(&bob, &alice)];
		assert_eq!(
			NftClaimCreditAwards::<Test>::get(award_block)
				.iter()
				.map(|award| NftCredits::compute_nft_claim_credit_leaf(
					&award.claimant,
					&award.credit
				))
				.collect::<Vec<_>>(),
			leaves,
		);
		assert_eq!(
			PendingNftClaimCreditRootInfo::<Test>::get(),
			Some(NftClaimCreditRootInfo { game_index: 1, timestamp: report_open })
		);
		// Nothing is committed until the block is over.
		assert_eq!(NftClaimCreditRoots::<Test>::iter().count(), 0);

		advance_process();

		let expected = NftClaimCreditTree {
			game_index: 1,
			root: binary_merkle_tree::merkle_root::<BlakeTwo256, _>(leaves).into(),
			leaf_count: 2,
			timestamp: report_open,
		};
		assert_eq!(NftClaimCreditRoots::<Test>::get(award_block), Some(expected));
		assert_eq!(PendingNftClaimCreditRootInfo::<Test>::get(), None);
		// The awards stay behind the root, so the block's claims are provable from state.
		assert_eq!(NftClaimCreditAwards::<Test>::decode_len(award_block), Some(2));
		assert!(System::events().iter().any(|record| record.event ==
			RuntimeEvent::NftCredits(Event::<Test>::NftClaimCreditRootRecorded {
				block: award_block,
				credit_root: expected,
			})));
	});
}

#[test]
fn a_built_credit_tree_is_queued_for_delivery() {
	// Only a block that really has a tree owes a delivery: an empty block commits nothing, so it
	// must not take a queue slot or a delivery sequence number.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 100,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		start_game_in_reporting_phase(&schedule, &[ALICE, BOB]);

		MOCK_UNIX_TIME
			.with(|t| *t.borrow_mut() = Duration::from_secs(schedule.game_play_time as u64));
		let report: FullReport<Test> =
			vec![vec![Report::Person].try_into().unwrap()].try_into().unwrap();

		let award_block = System::block_number();
		assert_ok!(Game::report(RuntimeOrigin::signed(ALICE), report));
		assert!(
			CreditTreeDeliveryQueue::<Test>::get().is_empty(),
			"nothing is queued before the block is over",
		);

		advance_process();
		assert_eq!(CreditTreeDeliveryQueue::<Test>::get().to_vec(), vec![(0, award_block)]);
		assert_eq!(NextCreditTreeSequence::<Test>::get(), 1);

		// The next block awards no credit, so it builds no tree and queues nothing.
		advance_process();
		assert_eq!(CreditTreeDeliveryQueue::<Test>::get().to_vec(), vec![(0, award_block)]);
		assert_eq!(NextCreditTreeSequence::<Test>::get(), 1);
	});
}

/// The block's leaf set, rebuilt the way an off-chain client has to: from the
/// `NftClaimCreditAwarded` events alone, ordered by the leaf index they carry.
fn leaves_from_events() -> Vec<NftClaimCreditLeaf> {
	let mut leaves = System::events()
		.iter()
		.filter_map(|record| match &record.event {
			RuntimeEvent::NftCredits(Event::<Test>::NftClaimCreditAwarded {
				claimant,
				credit,
				leaf_index,
			}) => Some((*leaf_index, NftCredits::compute_nft_claim_credit_leaf(claimant, credit))),
			_ => None,
		})
		.collect::<Vec<_>>();
	leaves.sort_by_key(|(leaf_index, _)| *leaf_index);
	assert_eq!(
		leaves.iter().map(|(leaf_index, _)| *leaf_index).collect::<Vec<_>>(),
		(0..leaves.len() as u32).collect::<Vec<_>>(),
		"the awarded credits must cover every leaf index of the block exactly once",
	);
	leaves.into_iter().map(|(_, leaf)| leaf).collect::<Vec<_>>()
}

#[test]
fn awarded_credit_events_yield_verifiable_inclusion_proofs() {
	// The proof a claimant presents on Asset Hub is built off chain from the block's
	// `NftClaimCreditAwarded` events, without replaying the block. Rebuild the leaf set from
	// them and check every leaf's inclusion proof verifies against the root the chain
	// committed for that block.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 100,
			rounds: 1,
			max_group_size: 4,
			..Default::default()
		};
		start_game_in_reporting_phase(&schedule, &[ALICE, BOB, CHARLIE, DAVE]);
		MOCK_UNIX_TIME
			.with(|t| *t.borrow_mut() = Duration::from_secs(schedule.game_play_time as u64));

		let award_block = System::block_number();
		for acc in [ALICE, BOB, CHARLIE] {
			let who = AccountOrPerson::Account(acc.clone());
			assert_ok!(Game::report(
				RuntimeOrigin::signed(acc),
				build_report_with_opinion(&who, |_| Report::Person),
			));
		}

		let leaves = leaves_from_events();
		assert_eq!(
			leaves,
			NftClaimCreditAwards::<Test>::get(award_block)
				.iter()
				.map(|award| NftCredits::compute_nft_claim_credit_leaf(
					&award.claimant,
					&award.credit
				))
				.collect::<Vec<_>>(),
		);

		advance_process();
		let credit_root =
			NftClaimCreditRoots::<Test>::get(award_block).expect("the block awarded credits");
		assert_eq!(credit_root.leaf_count, leaves.len() as u32);

		for leaf_index in 0..credit_root.leaf_count {
			let proof =
				binary_merkle_tree::merkle_proof::<BlakeTwo256, _, _>(leaves.clone(), leaf_index);
			assert_eq!(CreditProofNode::from(proof.root), credit_root.root);
			assert!(binary_merkle_tree::verify_proof::<BlakeTwo256, _, _>(
				&credit_root.root.into(),
				proof.proof,
				proof.number_of_leaves,
				proof.leaf_index,
				&proof.leaf
			));
		}
	});
}

/// The block's awards, rebuilt the way an off-chain client has to once a block's awards have been
/// pruned: from the `NftClaimCreditAwarded` events alone, ordered by the leaf index they carry.
/// This is what `NftCredits::nft_claim_credit_proof_from_awards` takes.
fn awards_from_events() -> Vec<NftClaimCreditAward<AccountId32>> {
	let mut awards = System::events()
		.iter()
		.filter_map(|record| match &record.event {
			RuntimeEvent::NftCredits(Event::<Test>::NftClaimCreditAwarded {
				claimant,
				credit,
				leaf_index,
			}) => Some((
				*leaf_index,
				NftClaimCreditAward { claimant: claimant.clone(), credit: *credit },
			)),
			_ => None,
		})
		.collect::<Vec<_>>();
	awards.sort_by_key(|(leaf_index, _)| *leaf_index);
	awards.into_iter().map(|(_, award)| award).collect::<Vec<_>>()
}

/// Play one block of reporting in a group of four and return the award block with the awards it
/// recorded, in leaf order, once the block's root is recorded.
fn award_credits_in_one_block() -> (BlockNumberFor<Test>, Vec<NftClaimCreditAward<AccountId32>>) {
	let schedule = GameSchedule::<u32, u128> {
		game_play_time: 100,
		rounds: 1,
		max_group_size: 4,
		..Default::default()
	};
	start_game_in_reporting_phase(&schedule, &[ALICE, BOB, CHARLIE, DAVE]);
	MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs(schedule.game_play_time as u64));

	let award_block = System::block_number();
	for acc in [ALICE, BOB, CHARLIE] {
		let who = AccountOrPerson::Account(acc.clone());
		assert_ok!(Game::report(
			RuntimeOrigin::signed(acc),
			build_report_with_opinion(&who, |_| Report::Person),
		));
	}
	let awards = awards_from_events();
	advance_process();
	assert_eq!(NftClaimCreditAwards::<Test>::get(award_block).to_vec(), awards);
	(award_block, awards)
}

/// Assert that `proof` is the inclusion proof of `award` at `leaf_index` against `credit_root`,
/// the way Asset Hub verifies it.
fn assert_credit_proof(
	proof: &NftClaimCreditProof,
	credit_root: &NftClaimCreditTree,
	award: &NftClaimCreditAward<AccountId32>,
	leaf_index: u32,
) {
	assert_eq!(proof.credit, award.credit);
	assert_eq!(proof.leaf_index, leaf_index);
	// The root, the leaf count and the leaf are the verifier's own: it holds the tree and it
	// authenticated the claimant, so nothing here comes out of `proof`.
	let leaf = NftCredits::compute_nft_claim_credit_leaf(&award.claimant, &award.credit);
	assert!(binary_merkle_tree::verify_proof::<BlakeTwo256, _, _>(
		&credit_root.root.into(),
		proof.proof.iter().copied().map(H256::from).collect::<Vec<_>>(),
		credit_root.leaf_count,
		proof.leaf_index,
		&leaf
	));
}

#[test]
fn credit_proofs_api_serves_a_claimant_from_state_alone() {
	// The intended off-chain path: a claimant asks for one award block and gets back a verifiable
	// proof per credit they hold there, without reading a single event.
	new_test_ext().execute_with(|| {
		let (award_block, awards) = award_credits_in_one_block();
		let credit_root =
			NftClaimCreditRoots::<Test>::get(award_block).expect("the block awarded credits");

		for claimant in [ALICE, BOB, CHARLIE, DAVE].map(AccountOrPerson::Account) {
			let expected = awards
				.iter()
				.enumerate()
				.filter(|(_, award)| award.claimant == claimant)
				.collect::<Vec<_>>();
			let proofs = NftCredits::nft_claim_credit_proofs(award_block, &claimant)
				.expect("the block's awards are retained");
			assert_eq!(proofs.len(), expected.len(), "one proof per credit of {claimant:?}");
			for (proof, (leaf_index, award)) in proofs.iter().zip(expected) {
				assert_credit_proof(proof, &credit_root, award, leaf_index as u32);
			}
		}
	});
}

#[test]
fn credit_proofs_api_returns_nothing_for_a_claimant_the_block_awarded_nothing() {
	new_test_ext().execute_with(|| {
		let (award_block, _) = award_credits_in_one_block();
		assert_eq!(
			NftCredits::nft_claim_credit_proofs(award_block, &AccountOrPerson::Account(EVE)),
			Ok(vec![])
		);
	});
}

#[test]
fn credit_proofs_api_rejects_a_block_that_awarded_nothing() {
	new_test_ext().execute_with(|| {
		let (award_block, _) = award_credits_in_one_block();
		assert_eq!(
			NftCredits::nft_claim_credit_proofs(award_block + 1, &AccountOrPerson::Account(ALICE)),
			Err(NftClaimCreditProofError::UnknownAwardBlock)
		);
	});
}

#[test]
fn pruned_award_block_keeps_its_root_and_falls_back_to_the_events() {
	// A block dropping out of the retained window must cost no claimant their mint: the root
	// stays, and the awards rebuilt from the block's events still yield the same proof.
	new_test_ext().execute_with(|| {
		MaxRetainedAwardBlocks::set(&1);
		let (award_block, awards) = award_credits_in_one_block();
		let credit_root =
			NftClaimCreditRoots::<Test>::get(award_block).expect("the block awarded credits");
		let claimant = awards[0].claimant.clone();

		// A second award block pushes the first out of the one-entry window.
		let report = build_report_with_opinion(&AccountOrPerson::Account(DAVE), |_| Report::Person);
		assert_ok!(Game::report(RuntimeOrigin::signed(DAVE), report));
		let second_block = System::block_number();
		advance_process();

		assert!(!NftClaimCreditAwards::<Test>::contains_key(award_block));
		assert!(NftClaimCreditAwards::<Test>::contains_key(second_block));
		assert_eq!(
			NftClaimCreditAwardBlocks::<Test>::get().to_vec(),
			vec![second_block],
			"the ring holds only the newest award block",
		);
		// The root outlives the awards, so the credits are still mintable.
		assert_eq!(NftClaimCreditRoots::<Test>::get(award_block), Some(credit_root));
		assert_eq!(
			NftCredits::nft_claim_credit_proofs(award_block, &claimant),
			Err(NftClaimCreditProofError::AwardsPruned)
		);

		let proof = NftCredits::nft_claim_credit_proof_from_awards(award_block, awards.clone(), 0)
			.expect("the block's awards rehash to its root");
		assert_credit_proof(&proof, &credit_root, &awards[0], 0);
	});
}

#[test]
fn credit_proof_from_awards_api_rejects_awards_that_do_not_match_the_recorded_root() {
	// Every way a caller can get the awards wrong is caught here rather than on Asset Hub: a
	// block that awarded nothing, an incomplete list, a leaf outside the tree, and a list in the
	// wrong order, which is the one an events reader can hit while holding every award.
	new_test_ext().execute_with(|| {
		let (award_block, awards) = award_credits_in_one_block();
		let leaf_count = awards.len() as u32;
		assert!(leaf_count > 1, "the reordering case needs at least two awards");

		assert_eq!(
			NftCredits::nft_claim_credit_proof_from_awards(award_block + 1, awards.clone(), 0),
			Err(NftClaimCreditProofError::UnknownAwardBlock)
		);
		assert_eq!(
			NftCredits::nft_claim_credit_proof_from_awards(award_block, awards[..1].to_vec(), 0),
			Err(NftClaimCreditProofError::LeafCountMismatch { expected: leaf_count })
		);
		assert_eq!(
			NftCredits::nft_claim_credit_proof_from_awards(award_block, awards.clone(), leaf_count),
			Err(NftClaimCreditProofError::LeafIndexOutOfBounds)
		);

		let mut reordered = awards.clone();
		reordered.swap(0, 1);
		assert_eq!(
			NftCredits::nft_claim_credit_proof_from_awards(award_block, reordered, 0),
			Err(NftClaimCreditProofError::RootMismatch)
		);
	});
}

/// `count` distinct leaves, standing in for a block's leaf set.
fn leaves(count: u32) -> Vec<NftClaimCreditLeaf> {
	(0..count)
		.map(|index| NftClaimCreditLeaf(sp_io::hashing::blake2_256(&index.encode())))
		.collect::<Vec<_>>()
}

#[test]
fn credit_tree_proofs_match_the_merkle_crate_at_every_index() {
	// The pallet holds its own copy of the tree layout Asset Hub's `verify_proof` rehashes along,
	// so it must give the root and siblings the crate gives, for leaf counts that promote an odd
	// node and those that do not.
	for count in (1..=17).chain([31, 32, 33, 64, 100]) {
		let leaves = leaves(count);
		let indices = (0..count).collect::<Vec<_>>();
		let (root, proofs) = NftCredits::credit_tree_proofs(&leaves, indices.iter().copied());

		assert_eq!(
			root,
			CreditProofNode::from(binary_merkle_tree::merkle_root::<BlakeTwo256, _>(
				leaves.clone()
			)),
			"root over {count} leaves"
		);
		for leaf_index in indices.iter().copied() {
			let expected =
				binary_merkle_tree::merkle_proof::<BlakeTwo256, _, _>(leaves.clone(), leaf_index);
			assert_eq!(
				proofs[leaf_index as usize],
				expected.proof.into_iter().map(CreditProofNode::from).collect::<Vec<_>>(),
				"siblings of leaf {leaf_index} of {count}"
			);
			assert!(
				binary_merkle_tree::verify_proof::<BlakeTwo256, _, _>(
					&root.into(),
					proofs[leaf_index as usize].iter().copied().map(H256::from),
					count,
					leaf_index,
					&leaves[leaf_index as usize]
				),
				"leaf {leaf_index} of {count} verifies"
			);
		}
	}
}

#[test]
fn credit_tree_proofs_serve_a_subset_of_indices_unchanged() {
	// A claimant asks about their own leaves only, so the proofs must not depend on which other
	// indices were asked for in the same call.
	let leaves = leaves(13);
	let (all_root, all_proofs) = NftCredits::credit_tree_proofs(&leaves, 0..13);

	let subset = [0, 5, 12];
	let (subset_root, subset_proofs) =
		NftCredits::credit_tree_proofs(&leaves, subset.iter().copied());
	assert_eq!(subset_root, all_root);
	for (proof, leaf_index) in subset_proofs.iter().zip(subset) {
		assert_eq!(proof, &all_proofs[leaf_index as usize]);
	}

	let (empty_root, empty_proofs) = NftCredits::credit_tree_proofs(&leaves, core::iter::empty());
	assert_eq!(empty_root, all_root);
	assert!(empty_proofs.is_empty());
}

#[test]
fn credit_proofs_api_serves_every_credit_of_a_claimant_that_holds_several() {
	// Each proof for the claimant with the most credits must be the one a single-index query
	// returns.
	new_test_ext().execute_with(|| {
		let (award_block, awards) = award_credits_in_one_block();
		let credit_root =
			NftClaimCreditRoots::<Test>::get(award_block).expect("the block awarded credits");
		let claimant = awards
			.iter()
			.max_by_key(|award| {
				awards.iter().filter(|other| other.claimant == award.claimant).count()
			})
			.expect("the block awarded credits")
			.claimant
			.clone();
		let expected = awards
			.iter()
			.enumerate()
			.filter(|(_, award)| award.claimant == claimant)
			.collect::<Vec<_>>();
		assert!(expected.len() > 1, "the claimant must hold several credits in the block");

		let proofs = NftCredits::nft_claim_credit_proofs(award_block, &claimant)
			.expect("the block's awards are retained");
		assert_eq!(proofs.len(), expected.len());
		for (proof, (leaf_index, award)) in proofs.iter().zip(expected) {
			assert_credit_proof(proof, &credit_root, award, leaf_index as u32);
			// The same proof the per-index path builds, which rebuilds the tree for the one leaf.
			let single = NftCredits::nft_claim_credit_proof_from_awards(
				award_block,
				awards.clone(),
				leaf_index as u32,
			)
			.expect("the block's awards rehash to its root");
			assert_eq!(proof, &single);
		}
	});
}

#[test]
fn credit_roots_api_lists_each_award_block_with_its_root() {
	// A wallet's first query: the claimant's award blocks resolved against the roots recorded
	// for them, so it knows which blocks to ask for proofs of and what each commits to.
	new_test_ext().execute_with(|| {
		let (award_block, awards) = award_credits_in_one_block();
		let credit_root =
			NftClaimCreditRoots::<Test>::get(award_block).expect("the block awarded credits");

		assert_eq!(
			NftCredits::nft_claim_credit_roots(&awards[0].claimant),
			vec![(award_block, credit_root)],
			"the claimant's only award block, with the root committing to their leaf",
		);
		assert_eq!(
			NftCredits::nft_claim_credit_roots(&AccountOrPerson::Account(EVE)),
			vec![],
			"a claimant with no credit has no root to mint against",
		);
	});
}

#[test]
fn credit_roots_api_omits_an_award_block_before_its_root_is_recorded() {
	// The root of an award block is only recorded in the next block, so a wallet must not be
	// pointed at a block it cannot yet prove anything against.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 100,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		start_game_in_reporting_phase(&schedule, &[ALICE, BOB]);
		let award_block = System::block_number();
		let report: FullReport<Test> =
			vec![vec![Report::Person].try_into().unwrap()].try_into().unwrap();
		assert_ok!(Game::report(RuntimeOrigin::signed(ALICE), report));

		let bob = AccountOrPerson::Account(BOB);
		assert_eq!(NftClaimCreditBlocks::<Test>::get(&bob).to_vec(), vec![award_block]);
		assert_eq!(NftCredits::nft_claim_credit_roots(&bob), vec![]);

		advance_process();
		let credit_root =
			NftClaimCreditRoots::<Test>::get(award_block).expect("the block awarded credits");
		assert_eq!(NftCredits::nft_claim_credit_roots(&bob), vec![(award_block, credit_root)]);
	});
}

#[test]
fn credit_apis_walk_a_wallet_from_claimant_to_verified_proof() {
	// The whole off-chain path in two queries: the roots API names the award blocks, the proofs
	// API turns one of them into proofs that verify against the root it named.
	new_test_ext().execute_with(|| {
		let (_, awards) = award_credits_in_one_block();
		let claimant = awards[0].claimant.clone();

		let roots = NftCredits::nft_claim_credit_roots(&claimant);
		let (award_block, credit_root) = roots.first().expect("the claimant holds a credit");

		let proofs = NftCredits::nft_claim_credit_proofs(*award_block, &claimant)
			.expect("the block's awards are retained");
		let expected = awards
			.iter()
			.enumerate()
			.filter(|(_, award)| award.claimant == claimant)
			.collect::<Vec<_>>();
		assert_eq!(proofs.len(), expected.len(), "one proof per credit of the claimant");
		for (proof, (leaf_index, award)) in proofs.iter().zip(expected) {
			assert_credit_proof(proof, credit_root, award, leaf_index as u32);
		}
	});
}

#[test]
#[should_panic(expected = "block must have room for the awarded credit")]
fn a_full_block_reports_the_lost_credit_defensively() {
	// The `integrity_test` holds `MaxCreditsPerBlock` to what a block of `report`s awards, so a
	// full block means the bound is too small and the credit is lost: committed to no root and
	// unmintable. The mock shrinks the bound to nothing to reach it.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 100,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		start_game_in_reporting_phase(&schedule, &[ALICE, BOB]);
		MaxCreditsPerBlock::set(&0);

		let report: FullReport<Test> =
			vec![vec![Report::Person].try_into().unwrap()].try_into().unwrap();
		assert_ok!(Game::report(RuntimeOrigin::signed(ALICE), report));
	});
}

#[test]
fn block_awarding_no_credit_records_no_root() {
	new_test_ext().execute_with(|| {
		let idle_block = System::block_number();
		advance_process();

		assert_eq!(NftClaimCreditRoots::<Test>::get(idle_block), None);
		assert_eq!(NftClaimCreditRoots::<Test>::iter().count(), 0);
	});
}

#[test]
fn every_credit_of_a_multi_round_game_is_awarded_exactly_once() {
	// The credit slot has to name the same bit whether `report` or the attendance backfill
	// reaches it, across every round and every position in a group. Two co-players sharing a
	// slot would silently swallow one of their credits, and a slot that differs between the
	// two paths would award one credit twice.
	new_test_ext().execute_with(|| {
		let players = [ALICE, BOB, CHARLIE, DAVE]
			.map(AccountOrPerson::Account)
			.into_iter()
			.collect::<Vec<_>>();
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 100,
			rounds: 2,
			max_group_size: 4,
			..Default::default()
		};

		run_game_scenario(schedule.clone(), &players, |player| {
			Some(build_report_with_opinion(player, |_| Report::Person))
		});

		// Every player attends and plays both rounds in a group of four, so each earns one
		// credit per co-player per round.
		let expected = schedule.rounds as usize * (schedule.max_group_size as usize - 1);
		for player in &players {
			let credits = awarded_credits(player);
			assert_eq!(credits.len(), expected, "{player:?} should earn every credit once");
			let mut deduplicated = credits.clone();
			deduplicated.sort();
			deduplicated.dedup();
			assert_eq!(deduplicated.len(), credits.len(), "{player:?} has a credit twice");
		}
		assert_eq!(awarded_credit_count(), players.len() * expected);
		assert_eq!(committed_leaf_count() as usize, players.len() * expected);
	});
}

#[test]
fn credit_slots_are_dropped_when_the_game_ends() {
	// The mask names group positions, which only the per-game index maps resolve, so it is
	// drained with them rather than left behind for every game the chain ever runs.
	new_test_ext().execute_with(|| {
		let players = [ALICE, BOB].map(AccountOrPerson::Account).into_iter().collect::<Vec<_>>();
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 100,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};

		run_game_scenario(schedule, &players, |player| {
			Some(build_report_with_opinion(player, |_| Report::Person))
		});

		assert_eq!(awarded_credit_count(), 2, "the game did award credits");
		assert_eq!(AwardedNftClaimCredits::<Test>::iter().count(), 0);
		// What outlives the game is the commitment, and where to find it.
		assert_eq!(committed_leaf_count(), 2);
		for player in &players {
			assert!(!NftClaimCreditBlocks::<Test>::get(player).is_empty());
		}
	});
}

/// The place `player` holds in their group in `round`, as [`Pallet::credit_slot`] counts it.
fn group_position(player: &AccountOrPerson<AccountId32>, round: RoundIndex) -> AttesterPosition {
	let game = indiv_pallet_game::Game::<Test>::get().expect("game exists");
	let player_count = match game.state {
		GameState::Reporting { player_count } => player_count,
		_ => panic!("game must be in Reporting state"),
	};
	let index = PlayerToIndex::<Test>::get(player).expect("player has indices")[round as usize];
	let groups = GroupsSetting { max_per_group: game.max_group_size, player_count };
	let group_index = groups.group_index_from_player_index(index);
	groups
		.group_members(group_index)
		.position(|member| member == index)
		.expect("a player is a member of their own group") as AttesterPosition
}

#[test]
fn award_marks_the_attester_position_of_the_reporting_co_player() {
	// The stored mask must mark the slot of the co-player that awarded the credit, and only
	// that one: a wrong slot would let the attendance backfill award the same credit again.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 100,
			rounds: 1,
			max_group_size: 4,
			..Default::default()
		};
		start_game_in_reporting_phase(&schedule, &[ALICE, BOB, CHARLIE, DAVE]);
		MOCK_UNIX_TIME
			.with(|t| *t.borrow_mut() = Duration::from_secs(schedule.game_play_time as u64));

		let alice = AccountOrPerson::Account(ALICE);
		let bob = AccountOrPerson::Account(BOB);
		let charlie = AccountOrPerson::Account(CHARLIE);
		let (alice_slot, bob_slot, charlie_slot) =
			(group_position(&alice, 0), group_position(&bob, 0), group_position(&charlie, 0));

		assert_ok!(Game::report(
			RuntimeOrigin::signed(ALICE),
			build_report_with_opinion(&alice, |_| Report::Person),
		));

		// BOB holds ALICE's credit, in ALICE's slot: not his own, and not a silent co-player's.
		let awarded = AwardedNftClaimCredits::<Test>::get(1, &bob);
		assert!(awarded.contains(NftCredits::credit_slot(0, alice_slot)));
		assert!(!awarded.contains(NftCredits::credit_slot(0, bob_slot)));
		assert!(!awarded.contains(NftCredits::credit_slot(0, charlie_slot)));
		assert_eq!(awarded.count(), 1);

		// ALICE reported on others and nobody on her, so her own mask stays empty.
		assert_eq!(AwardedNftClaimCredits::<Test>::get(1, &alice), AwardedCredits::default());

		// A second award of the same credit records nothing further: the mask refuses it.
		let credit = NftCredits::compute_nft_claim_credit(1, 0, &alice, &bob);
		let awards_recorded = NftCredits::award_nft_claim_credit(
			1,
			&bob,
			credit,
			NftCredits::credit_slot(0, alice_slot),
			0,
		);
		assert_eq!(awards_recorded, 0);
	});
}

#[test]
fn awarded_credits_records_slots_within_capacity_only() {
	let mut awarded = AwardedCredits::default();
	assert_eq!(awarded.count(), 0);

	awarded.insert(0);
	awarded.insert(AwardedCredits::CAPACITY - 1);
	assert!(awarded.contains(0));
	assert!(awarded.contains(AwardedCredits::CAPACITY - 1));
	assert!(!awarded.contains(1));
	assert_eq!(awarded.count(), 2);

	// A slot the set cannot hold is neither recorded nor reported, rather than wrapping onto
	// another slot and swallowing that credit.
	assert!(!AwardedCredits::within_capacity(AwardedCredits::CAPACITY));
	awarded.insert(AwardedCredits::CAPACITY);
	assert!(!awarded.contains(AwardedCredits::CAPACITY));
	assert_eq!(awarded.count(), 2);
}

#[test]
fn credit_blocks_index_records_each_award_block_once() {
	// The index maps a claimant to the blocks whose tree holds a credit of theirs. A block is
	// recorded once however many credits it awards the claimant, and blocks accumulate in
	// ascending order across the game.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 100,
			rounds: 1,
			max_group_size: 4,
			..Default::default()
		};
		start_game_in_reporting_phase(&schedule, &[ALICE, BOB, CHARLIE, DAVE]);
		MOCK_UNIX_TIME
			.with(|t| *t.borrow_mut() = Duration::from_secs(schedule.game_play_time as u64));

		// ALICE and BOB report in the same block, so CHARLIE earns two credits in it.
		let first_block = System::block_number();
		for acc in [ALICE, BOB] {
			let who = AccountOrPerson::Account(acc.clone());
			assert_ok!(Game::report(
				RuntimeOrigin::signed(acc),
				build_report_with_opinion(&who, |_| Report::Person),
			));
		}

		advance_process();

		// CHARLIE reports in the next block, awarding ALICE a credit there.
		let second_block = System::block_number();
		let charlie = AccountOrPerson::Account(CHARLIE);
		assert_ok!(Game::report(
			RuntimeOrigin::signed(CHARLIE),
			build_report_with_opinion(&charlie, |_| Report::Person),
		));

		let alice = AccountOrPerson::Account(ALICE);
		assert_eq!(
			NftClaimCreditBlocks::<Test>::get(&alice).to_vec(),
			vec![first_block, second_block]
		);
		// Two credits in one block, one entry, and none for the block CHARLIE only reported in.
		assert_eq!(awarded_credits(&charlie).len(), 2, "CHARLIE is reported on by ALICE and BOB");
		assert_eq!(NftClaimCreditBlocks::<Test>::get(&charlie).to_vec(), vec![first_block]);

		// Every indexed block commits a tree the claimant can prove inclusion against.
		advance_process();
		for block in NftClaimCreditBlocks::<Test>::get(&alice) {
			assert!(NftClaimCreditRoots::<Test>::contains_key(block));
		}
	});
}

#[test]
fn credit_blocks_index_drops_its_oldest_block_when_full() {
	// The index is a bounded ring: a claimant with more award blocks than it holds loses the
	// oldest, while the credit itself is still awarded and committed.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 100,
			rounds: 1,
			max_group_size: 4,
			..Default::default()
		};
		start_game_in_reporting_phase(&schedule, &[ALICE, BOB, CHARLIE, DAVE]);
		MOCK_UNIX_TIME
			.with(|t| *t.borrow_mut() = Duration::from_secs(schedule.game_play_time as u64));
		MaxCreditBlocksPerClaimant::set(&1);

		let alice = AccountOrPerson::Account(ALICE);
		let bob = AccountOrPerson::Account(BOB);
		assert_ok!(Game::report(
			RuntimeOrigin::signed(BOB),
			build_report_with_opinion(&bob, |_| Report::Person),
		));

		advance_process();

		let last_block = System::block_number();
		let charlie = AccountOrPerson::Account(CHARLIE);
		assert_ok!(Game::report(
			RuntimeOrigin::signed(CHARLIE),
			build_report_with_opinion(&charlie, |_| Report::Person),
		));

		assert_eq!(NftClaimCreditBlocks::<Test>::get(&alice).to_vec(), vec![last_block]);
		assert_eq!(
			awarded_credits(&alice).len(),
			2,
			"both credits are awarded, only the older block falls out of the index"
		);
	});
}

#[test]
fn credit_is_committed_once_and_keeps_its_first_award_block() {
	// Both players report `Person`, so every credit of the game is awarded during
	// reporting. The attendance backfill walks the very same credits again: it must leave
	// their award block alone and contribute no second leaf, otherwise the claimant could
	// mint twice from one credit, or look for their leaf in the wrong block's tree.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 100,
			rounds: 1,
			max_group_size: 2,
			..Default::default()
		};
		start_game_in_reporting_phase(&schedule, &[ALICE, BOB]);

		let report_open = schedule.game_play_time;
		MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs(report_open as u64));

		let report_block = System::block_number();
		let report: FullReport<Test> =
			vec![vec![Report::Person].try_into().unwrap()].try_into().unwrap();
		assert_ok!(Game::report(RuntimeOrigin::signed(ALICE), report.clone()));
		assert_ok!(Game::report(RuntimeOrigin::signed(BOB), report));

		let alice = AccountOrPerson::Account(ALICE);
		let bob = AccountOrPerson::Account(BOB);
		let credit_for_alice = NftCredits::compute_nft_claim_credit(1, 0, &bob, &alice);

		// Run the backfill in a later block than the reports.
		let backfill_time = GameTimes::<Test>::reporting_end(&schedule) + 1;
		MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs(backfill_time as u64));
		while indiv_pallet_game::Game::<Test>::get().is_some() {
			advance_process();
		}
		advance_process(); // commit the last block's leaves, if any

		// The credit is awarded once, in the reporting block, and the backfill adds neither a
		// second leaf nor a second block to look for one in.
		assert!(awarded_credits(&alice).contains(&credit_for_alice));
		assert_eq!(awarded_credit_count(), 2);
		assert_eq!(committed_leaf_count(), 2);
		assert_eq!(NftClaimCreditBlocks::<Test>::get(&alice).to_vec(), vec![report_block]);

		// The root info is written after the award, so the backfill blocks, whose awards were all
		// refused, leave none behind. Were the order reversed, the info would be set over a block
		// with no awards and that block would record a root over no leaves.
		assert_eq!(PendingNftClaimCreditRootInfo::<Test>::get(), None);
		assert_eq!(NftClaimCreditRoots::<Test>::iter().count(), 1);
		assert!(NftClaimCreditRoots::<Test>::get(report_block).is_some());
	});
}

/// A delivery queue wider than the retained-awards ring is rejected: the tail of such a queue
/// holds trees whose awards the ring has already pruned.
#[test]
#[should_panic(expected = "MaxRetainedAwardBlocks (8) must be >= MaxQueuedCreditTrees (9)")]
fn integrity_test_rejects_queue_wider_than_retained_awards() {
	new_test_ext().execute_with(|| {
		MaxRetainedAwardBlocks::set(&8);
		MaxQueuedCreditTrees::set(&9);
		<Pallet<Test> as Hooks<u64>>::integrity_test();
	});
}

/// The credit tree delivery to the NFT claims chain: the queue the `on_initialize` commit feeds,
/// the offchain-worker-driven `send_credit_trees` and the `replay_credit_trees` repair.
mod credit_tree_delivery {
	use super::*;
	use frame_support::{dispatch::GetDispatchInfo, weights::Weight};
	use sp_runtime::{
		transaction_validity::{TransactionSource, TransactionValidityError},
		DispatchError,
	};

	/// Records the credit tree of `block` and queues it for delivery, as a block that awarded
	/// credits does at the following block's `on_initialize`.
	fn queue_credit_tree(block: u64) -> NftClaimCreditTree {
		let tree = NftClaimCreditTree {
			game_index: 7,
			root: CreditProofNode([block as u8; 32]),
			leaf_count: 3,
			timestamp: 1_000 + block as u32,
		};
		NftClaimCreditRoots::<Test>::insert(block, tree);
		NftCredits::queue_credit_tree_delivery(block);
		tree
	}

	/// The blocks to replay, as `replay_credit_trees` takes them.
	fn replay_blocks(blocks: Vec<u64>) -> BoundedVec<u64, MaxCreditTreesPerMessage> {
		BoundedVec::try_from(blocks).expect("the list fits MaxCreditTreesPerMessage")
	}

	fn authorized_origin() -> RuntimeOrigin {
		frame_system::RawOrigin::Authorized.into()
	}

	fn set_time(secs: u64) {
		MOCK_UNIX_TIME.with(|time| *time.borrow_mut() = Duration::from_secs(secs));
	}

	#[test]
	fn send_credit_trees_delivers_the_queue_and_clears_it() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			let first = queue_credit_tree(10);
			let second = queue_credit_tree(12);

			assert_ok!(NftCredits::send_credit_trees(authorized_origin(), 0, 0));

			let batch = last_sent_credit_tree_batch();
			assert_eq!(batch.trees.len(), 2);
			assert_eq!(batch.trees[0].sequence, Some(0));
			assert_eq!(batch.trees[0].block, 10);
			assert_eq!(batch.trees[0].tree, first);
			assert_eq!(batch.trees[1].sequence, Some(1));
			assert_eq!(batch.trees[1].block, 12);
			assert_eq!(batch.trees[1].tree, second);

			assert!(CreditTreeDeliveryQueue::<Test>::get().is_empty());
			assert!(recorded_events().contains(&RuntimeEvent::NftCredits(
				Event::CreditTreesSent { trees: bounded_vec![10, 12] }
			)));
			// The trees themselves stay: a claimant still needs them to build a proof.
			assert_eq!(NftClaimCreditRoots::<Test>::get(10), Some(first));
		});
	}

	#[test]
	fn send_credit_trees_sends_at_most_one_message_worth() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			// One tree more than a message can carry.
			for block in 1..=MaxCreditTreesPerMessage::get() as u64 + 1 {
				queue_credit_tree(block);
			}

			assert_ok!(NftCredits::send_credit_trees(authorized_origin(), 0, 0));

			assert_eq!(
				last_sent_credit_tree_batch().trees.len(),
				MaxCreditTreesPerMessage::get() as usize
			);
			let leftover_sequence = MaxCreditTreesPerMessage::get() as u64;
			assert_eq!(
				CreditTreeDeliveryQueue::<Test>::get().to_vec(),
				vec![(leftover_sequence, leftover_sequence + 1)]
			);

			// The leftover goes out in the next round.
			assert_ok!(NftCredits::send_credit_trees(authorized_origin(), leftover_sequence, 0));
			assert_eq!(last_sent_credit_tree_batch().trees.len(), 1);
			assert!(CreditTreeDeliveryQueue::<Test>::get().is_empty());
		});
	}

	/// A channel sized for `n` trees must carry `n` trees: the room the pallet asks for and the
	/// capacity it derives back from the channel are the same computation, and the message it then
	/// builds has to fit what the router takes. This is the setup `send_credit_trees`' benchmark
	/// runs, which only fails once the message goes out over a real router.
	#[test]
	fn a_channel_sized_for_a_message_carries_exactly_that_message() {
		// `MaxCreditTreesPerMessage` is a `pub storage` value, so reading it needs externalities.
		let max = new_test_ext().execute_with(MaxCreditTreesPerMessage::get);

		for trees in 1..=max {
			new_test_ext().execute_with(|| {
				// `frame_system` drops events deposited at block zero.
				System::set_block_number(1);
				set_claims_max_message_size(NftCredits::credit_tree_channel_size(trees));
				for block in 1..=max as u64 {
					queue_credit_tree(block);
				}

				assert_eq!(NftCredits::max_credit_trees_per_message(), Some(trees));
				assert_ok!(NftCredits::send_credit_trees(authorized_origin(), 0, 0));

				assert_eq!(last_sent_credit_tree_batch().trees.len(), trees as usize);
				assert_eq!(CreditTreeDeliveryQueue::<Test>::get().len() as u32, max - trees);
			});
		}
	}

	#[test]
	fn send_credit_trees_keeps_the_queue_without_a_channel() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			queue_credit_tree(10);
			close_claims_channel();

			assert_ok!(NftCredits::send_credit_trees(authorized_origin(), 0, 0));

			assert!(sent_credit_tree_xcms().is_empty());
			assert_eq!(CreditTreeDeliveryQueue::<Test>::get().to_vec(), vec![(0, 10)]);
			assert!(
				recorded_events().contains(&RuntimeEvent::NftCredits(Event::CreditTreeSendFailed))
			);
		});
	}

	#[test]
	fn send_credit_trees_keeps_the_queue_when_the_xcm_fails() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			queue_credit_tree(10);
			fail_credit_tree_xcms(true);

			assert_ok!(NftCredits::send_credit_trees(authorized_origin(), 0, 0));

			assert!(sent_credit_tree_xcms().is_empty());
			assert_eq!(CreditTreeDeliveryQueue::<Test>::get().to_vec(), vec![(0, 10)]);
			assert!(
				recorded_events().contains(&RuntimeEvent::NftCredits(Event::CreditTreeSendFailed))
			);

			// The next cycle's retry goes through and drains the same tree.
			fail_credit_tree_xcms(false);
			assert_ok!(NftCredits::send_credit_trees(authorized_origin(), 0, 0));
			assert_eq!(last_sent_credit_tree_batch().trees.len(), 1);
			assert!(CreditTreeDeliveryQueue::<Test>::get().is_empty());
		});
	}

	#[test]
	fn send_credit_trees_drops_a_queued_block_whose_tree_is_gone() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			queue_credit_tree(10);
			queue_credit_tree(12);
			NftClaimCreditRoots::<Test>::remove(10);

			assert_ok!(NftCredits::send_credit_trees(authorized_origin(), 0, 0));

			let batch = last_sent_credit_tree_batch();
			assert_eq!(batch.trees.len(), 1);
			assert_eq!(batch.trees[0].block, 12);
			assert!(CreditTreeDeliveryQueue::<Test>::get().is_empty());
			// Only the tree that was sent is listed, and the sequence spent on the one that was
			// not is reported on its own, which is what keeps the run alignable.
			assert!(recorded_events().contains(&RuntimeEvent::NftCredits(
				Event::CreditTreesSent { trees: bounded_vec![12] }
			)));
			assert!(recorded_events().contains(&RuntimeEvent::NftCredits(
				Event::CreditTreeDeliverySkipped { sequence: 0, block: 10 }
			)));
		});
	}

	#[test]
	fn send_credit_trees_clears_the_queue_with_nothing_left_to_send() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			queue_credit_tree(10);
			NftClaimCreditRoots::<Test>::remove(10);

			assert_ok!(NftCredits::send_credit_trees(authorized_origin(), 0, 0));

			assert!(sent_credit_tree_xcms().is_empty());
			assert!(CreditTreeDeliveryQueue::<Test>::get().is_empty());
			assert!(recorded_events().contains(&RuntimeEvent::NftCredits(
				Event::CreditTreesSent { trees: bounded_vec![] }
			)));
			// The spent sequence is reported, because the gap it leaves on the claims chain is
			// the one a replay cannot fill.
			assert!(recorded_events().contains(&RuntimeEvent::NftCredits(
				Event::CreditTreeDeliverySkipped { sequence: 0, block: 10 }
			)));
		});
	}

	#[test]
	fn send_credit_trees_refunds_down_to_the_trees_it_sent() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			queue_credit_tree(10);

			let pre_dispatch =
				Call::<Test>::send_credit_trees { first_sequence: 0, discriminator: 0 }
					.get_dispatch_info()
					.call_weight;
			let post =
				NftCredits::send_credit_trees(authorized_origin(), 0, 0).expect("sending succeeds");

			let actual = post.actual_weight.expect("the weight is refunded");
			assert_eq!(actual, <MockWeightInfo as WeightInfo>::send_credit_trees(1));
			assert!(actual.all_lt(pre_dispatch), "{actual:?} must be below {pre_dispatch:?}");
		});
	}

	#[test]
	fn send_credit_trees_rejects_any_other_origin() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			queue_credit_tree(10);

			assert_noop!(
				NftCredits::send_credit_trees(RuntimeOrigin::signed(ALICE), 0, 0),
				DispatchError::BadOrigin
			);
			assert_noop!(
				NftCredits::send_credit_trees(RuntimeOrigin::root(), 0, 0),
				DispatchError::BadOrigin
			);
		});
	}

	#[test]
	fn a_full_delivery_queue_reports_the_tree_it_could_not_take() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			for block in 1..=MaxQueuedCreditTrees::get() as u64 {
				queue_credit_tree(block);
			}

			queue_credit_tree(100);

			// The overflowing tree consumes no sequence number, so the claims chain never sees a
			// gap for a message that was never sent.
			assert_eq!(NextCreditTreeSequence::<Test>::get(), MaxQueuedCreditTrees::get() as u64);
			assert_eq!(
				CreditTreeDeliveryQueue::<Test>::get().len(),
				MaxQueuedCreditTrees::get() as usize
			);
			assert!(recorded_events().contains(&RuntimeEvent::NftCredits(
				Event::CreditTreeDeliveryDropped { block: 100 }
			)));
			// The tree stays, so a replay can still deliver it.
			assert!(NftClaimCreditRoots::<Test>::contains_key(100));
		});
	}

	#[test]
	fn authorize_send_credit_trees_accepts_a_local_transaction() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			queue_credit_tree(10);

			let (validity, weight) =
				NftCredits::authorize_send_credit_trees(TransactionSource::Local, &0)
					.expect("valid");

			assert_eq!(weight, Weight::zero());
			assert!(!validity.propagate);
			assert!(validity.priority > 0);
		});
	}

	#[test]
	fn authorize_send_credit_trees_rejects_an_external_transaction() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			queue_credit_tree(10);

			assert_eq!(
				NftCredits::authorize_send_credit_trees(TransactionSource::External, &0),
				Err(AuthorizeInvalidity::TransactionNotLocal.into())
			);
		});
	}

	#[test]
	fn authorize_send_credit_trees_rejects_an_empty_queue() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			assert_eq!(
				NftCredits::authorize_send_credit_trees(TransactionSource::Local, &0),
				Err(AuthorizeInvalidity::NoQueuedCreditTrees.into())
			);
		});
	}

	#[test]
	fn authorize_send_credit_trees_orders_by_the_queued_sequence() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			queue_credit_tree(10);
			queue_credit_tree(12);
			// Delivering the tree of sequence 0 leaves sequence 1 at the front of the queue.
			CreditTreeDeliveryQueue::<Test>::mutate(|queued| {
				queued.remove(0);
			});

			assert_eq!(
				NftCredits::authorize_send_credit_trees(TransactionSource::Local, &0),
				Err(TransactionValidityError::Invalid(InvalidTransaction::Stale))
			);
			assert_eq!(
				NftCredits::authorize_send_credit_trees(TransactionSource::Local, &2),
				Err(TransactionValidityError::Invalid(InvalidTransaction::Future))
			);
			assert!(NftCredits::authorize_send_credit_trees(TransactionSource::Local, &1).is_ok());
		});
	}

	#[test]
	fn the_offchain_worker_submits_a_delivery_for_the_queued_trees() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			clear_pool();
			queue_credit_tree(10);

			AllPalletsWithSystem::offchain_worker(System::block_number());

			assert_eq!(
				submitted_calls(),
				vec![RuntimeCall::NftCredits(Call::send_credit_trees {
					first_sequence: 0,
					discriminator: System::block_number() / 8,
				})]
			);
		});
	}

	#[test]
	fn the_offchain_worker_submits_nothing_with_an_empty_queue() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			clear_pool();

			AllPalletsWithSystem::offchain_worker(System::block_number());

			assert!(submitted_calls().is_empty());
		});
	}

	#[test]
	fn replay_credit_trees_resends_the_named_trees_without_a_sequence() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			set_time(1_000);
			let first = queue_credit_tree(10);
			let second = queue_credit_tree(12);

			assert_ok!(NftCredits::replay_credit_trees(
				RuntimeOrigin::signed(ALICE),
				replay_blocks(vec![10, 12])
			));

			let batch = last_sent_credit_tree_batch();
			assert_eq!(batch.trees.len(), 2);
			assert_eq!(batch.trees[0].sequence, None);
			assert_eq!(batch.trees[0].tree, first);
			assert_eq!(batch.trees[1].sequence, None);
			assert_eq!(batch.trees[1].tree, second);
			assert!(recorded_events()
				.contains(&RuntimeEvent::NftCredits(Event::CreditTreesReplayed { count: 2 })));
			// A replay is not a delivery: the queue is untouched.
			assert_eq!(CreditTreeDeliveryQueue::<Test>::get().to_vec(), vec![(0, 10), (1, 12)]);
		});
	}

	#[test]
	fn replay_credit_trees_skips_blocks_without_a_tree() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			set_time(1_000);
			queue_credit_tree(12);

			assert_ok!(NftCredits::replay_credit_trees(
				RuntimeOrigin::signed(ALICE),
				replay_blocks(vec![10, 12])
			));

			let batch = last_sent_credit_tree_batch();
			assert_eq!(batch.trees.len(), 1);
			assert_eq!(batch.trees[0].block, 12);
			// A replayed block spends no sequence, so its skip leaves no gap to report.
			assert!(!recorded_events().iter().any(|event| matches!(
				event,
				RuntimeEvent::NftCredits(Event::CreditTreeDeliverySkipped { .. })
			)));
		});
	}

	#[test]
	fn replay_credit_trees_rejects_a_list_without_any_tree() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			set_time(1_000);

			assert_noop!(
				NftCredits::replay_credit_trees(
					RuntimeOrigin::signed(ALICE),
					replay_blocks(vec![10])
				),
				Error::<Test>::NoCreditTreeForBlock
			);
		});
	}

	#[test]
	fn replay_credit_trees_rejects_an_empty_or_unsorted_list() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			set_time(1_000);
			queue_credit_tree(10);
			queue_credit_tree(12);

			assert_noop!(
				NftCredits::replay_credit_trees(
					RuntimeOrigin::signed(ALICE),
					replay_blocks(vec![])
				),
				Error::<Test>::NoBlocksToReplay
			);
			assert_noop!(
				NftCredits::replay_credit_trees(
					RuntimeOrigin::signed(ALICE),
					replay_blocks(vec![12, 10])
				),
				Error::<Test>::UnsortedReplayBlocks
			);
			assert_noop!(
				NftCredits::replay_credit_trees(
					RuntimeOrigin::signed(ALICE),
					replay_blocks(vec![10, 10])
				),
				Error::<Test>::UnsortedReplayBlocks
			);
		});
	}

	#[test]
	fn replay_credit_trees_rejects_an_unsigned_origin() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			set_time(1_000);
			queue_credit_tree(10);

			assert_noop!(
				NftCredits::replay_credit_trees(RuntimeOrigin::root(), replay_blocks(vec![10])),
				DispatchError::BadOrigin
			);
		});
	}

	#[test]
	fn rejected_replay_starts_no_cooldown() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			set_time(1_000);
			queue_credit_tree(10);

			assert_noop!(
				NftCredits::replay_credit_trees(
					RuntimeOrigin::signed(ALICE),
					replay_blocks(vec![])
				),
				Error::<Test>::NoBlocksToReplay
			);
			assert_eq!(LastReplayTime::<Test>::get(), None);
		});
	}

	#[test]
	fn replay_credit_trees_rejects_a_second_replay_in_the_window() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			set_time(1_000);
			queue_credit_tree(10);
			queue_credit_tree(12);

			assert_ok!(NftCredits::replay_credit_trees(
				RuntimeOrigin::signed(BOB),
				replay_blocks(vec![10])
			));

			// Another account, another tree, one second later.
			set_time(1_001);
			assert_noop!(
				NftCredits::replay_credit_trees(
					RuntimeOrigin::signed(ALICE),
					replay_blocks(vec![12])
				),
				Error::<Test>::ReplayCooldownActive
			);
			assert_eq!(sent_credit_tree_xcms().len(), 1);
		});
	}

	#[test]
	fn replay_credit_trees_serves_a_repair_once_the_window_has_passed() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			set_time(1_000);
			queue_credit_tree(10);
			let missing = queue_credit_tree(12);

			assert_ok!(NftCredits::replay_credit_trees(
				RuntimeOrigin::signed(BOB),
				replay_blocks(vec![10])
			));

			set_time(1_000 + ReplayCooldownSeconds::get());
			assert_ok!(NftCredits::replay_credit_trees(
				RuntimeOrigin::signed(ALICE),
				replay_blocks(vec![12])
			));
			let batch = last_sent_credit_tree_batch();
			assert_eq!(batch.trees.len(), 1);
			assert_eq!(batch.trees[0].tree, missing);
		});
	}

	#[test]
	fn the_replay_cooldown_does_not_hold_up_the_delivery_stream() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			set_time(1_000);
			queue_credit_tree(10);
			queue_credit_tree(12);

			assert_ok!(NftCredits::replay_credit_trees(
				RuntimeOrigin::signed(BOB),
				replay_blocks(vec![10])
			));

			// Still inside the window.
			assert_ok!(NftCredits::send_credit_trees(authorized_origin(), 0, 0));
			assert!(CreditTreeDeliveryQueue::<Test>::get().is_empty());
			assert_eq!(sent_credit_tree_xcms().len(), 2);
		});
	}

	#[test]
	fn an_invalid_replay_reports_itself_inside_the_window() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			set_time(1_000);
			queue_credit_tree(10);
			queue_credit_tree(12);

			assert_ok!(NftCredits::replay_credit_trees(
				RuntimeOrigin::signed(BOB),
				replay_blocks(vec![10])
			));

			assert_noop!(
				NftCredits::replay_credit_trees(
					RuntimeOrigin::signed(BOB),
					replay_blocks(vec![12, 10])
				),
				Error::<Test>::UnsortedReplayBlocks
			);
		});
	}

	#[test]
	fn replay_credit_trees_fails_without_a_channel() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			set_time(1_000);
			queue_credit_tree(10);
			close_claims_channel();

			assert_noop!(
				NftCredits::replay_credit_trees(
					RuntimeOrigin::signed(ALICE),
					replay_blocks(vec![10])
				),
				Error::<Test>::ExceedsClaimsChannelCapacity
			);
		});
	}

	#[test]
	fn replay_credit_trees_fails_when_the_message_would_not_fit() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			set_time(1_000);
			queue_credit_tree(10);
			queue_credit_tree(12);
			// Room for exactly one tree.
			set_claims_max_message_size(NftCredits::credit_tree_channel_size(1));

			assert_noop!(
				NftCredits::replay_credit_trees(
					RuntimeOrigin::signed(ALICE),
					replay_blocks(vec![10, 12])
				),
				Error::<Test>::ExceedsClaimsChannelCapacity
			);
			assert_ok!(NftCredits::replay_credit_trees(
				RuntimeOrigin::signed(ALICE),
				replay_blocks(vec![10])
			));
		});
	}

	#[test]
	fn replay_credit_trees_charges_the_remote_execution_weight_per_tree() {
		new_test_ext().execute_with(|| {
			// `frame_system` drops events deposited at block zero.
			System::set_block_number(1);
			let one = Call::<Test>::replay_credit_trees { blocks: replay_blocks(vec![10]) }
				.get_dispatch_info()
				.call_weight;
			let two = Call::<Test>::replay_credit_trees { blocks: replay_blocks(vec![10, 12]) }
				.get_dispatch_info()
				.call_weight;

			assert_eq!(
				one,
				<MockWeightInfo as WeightInfo>::replay_credit_trees(1) +
					NftClaimsRemoteWeight::get()
			);
			assert_eq!(
				two,
				<MockWeightInfo as WeightInfo>::replay_credit_trees(2) +
					NftClaimsRemoteWeight::get() * 2
			);
		});
	}
}

#[cfg(feature = "testnet")]
mod testnet_granted_credits {
	use super::*;
	use frame_support::traits::Get;
	use indiv_pallet_game::Config as GameConfig;
	use sp_runtime::DispatchError;

	const GAME: GameIdx = 7;

	fn grant(
		claimant: &AccountOrPerson<AccountId32>,
		attester: &AccountOrPerson<AccountId32>,
		round: RoundIndex,
		attester_position: AttesterPosition,
	) -> frame_support::pallet_prelude::DispatchResult {
		NftCredits::testnet_grant_nft_claim_credit(
			RuntimeOrigin::root(),
			claimant.clone(),
			attester.clone(),
			GAME,
			round,
			attester_position,
		)
	}

	#[test]
	fn a_granted_credit_is_committed_to_the_blocks_tree() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			MOCK_UNIX_TIME.with(|t| *t.borrow_mut() = Duration::from_secs(5_000));

			let alice = AccountOrPerson::Account(ALICE);
			let bob = AccountOrPerson::Account(BOB);
			let award_block = System::block_number();

			// No game is ongoing, and neither identity has ever signed up for one.
			assert!(indiv_pallet_game::Game::<Test>::get().is_none());
			assert_ok!(grant(&alice, &bob, 0, 0));

			let credit = NftCredits::compute_nft_claim_credit(GAME, 0, &bob, &alice);
			let leaf = NftCredits::compute_nft_claim_credit_leaf(&alice, &credit);
			assert_eq!(
				NftClaimCreditAwards::<Test>::get(award_block).to_vec(),
				vec![NftClaimCreditAward { claimant: alice.clone(), credit }]
			);
			assert_eq!(NftClaimCreditBlocks::<Test>::get(&alice).to_vec(), vec![award_block]);
			assert!(System::events().iter().any(|record| record.event ==
				RuntimeEvent::NftCredits(Event::<Test>::NftClaimCreditAwarded {
					claimant: alice.clone(),
					credit,
					leaf_index: 0,
				})));

			advance_process();

			// The tree the claim chain verifies a proof against is built over the grant like
			// over any other award.
			assert_eq!(
				NftClaimCreditRoots::<Test>::get(award_block),
				Some(NftClaimCreditTree {
					game_index: GAME,
					root: binary_merkle_tree::merkle_root::<BlakeTwo256, _>(vec![leaf]).into(),
					leaf_count: 1,
					timestamp: 5_000,
				})
			);
		});
	}

	#[test]
	fn varying_the_attester_position_grants_a_second_credit() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);

			let alice = AccountOrPerson::Account(ALICE);
			let bob = AccountOrPerson::Account(BOB);

			assert_ok!(grant(&alice, &bob, 0, 0));
			assert_ok!(grant(&alice, &bob, 0, 1));

			assert_eq!(NftClaimCreditAwards::<Test>::decode_len(System::block_number()), Some(2));
		});
	}

	#[test]
	fn a_repeated_credit_slot_grants_nothing() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);

			let alice = AccountOrPerson::Account(ALICE);
			let bob = AccountOrPerson::Account(BOB);
			let charlie = AccountOrPerson::Account(CHARLIE);

			assert_ok!(grant(&alice, &bob, 0, 0));
			// The slot is what a game spends, so a different attester on the same slot is the
			// same credit as far as the mask is concerned and mints nothing further.
			assert_noop!(grant(&alice, &charlie, 0, 0), Error::<Test>::CreditNotAwarded);
			assert_eq!(NftClaimCreditAwards::<Test>::decode_len(System::block_number()), Some(1));
		});
	}

	#[test]
	fn a_round_or_slot_beyond_a_games_bounds_is_rejected() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);

			let alice = AccountOrPerson::Account(ALICE);
			let bob = AccountOrPerson::Account(BOB);
			let rounds = <<Test as GameConfig>::MaxRounds as Get<u32>>::get() as RoundIndex;
			let group = <<Test as GameConfig>::MaxGroupSize as Get<u32>>::get();

			assert_noop!(grant(&alice, &bob, rounds, 0), Error::<Test>::CreditSlotOutOfBounds);
			assert_noop!(grant(&alice, &bob, 0, group), Error::<Test>::CreditSlotOutOfBounds);
			assert_ok!(grant(&alice, &bob, rounds - 1, group - 1));
		});
	}

	#[test]
	fn only_root_can_grant_a_credit() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);

			let alice = AccountOrPerson::Account(ALICE);
			assert_noop!(
				NftCredits::testnet_grant_nft_claim_credit(
					RuntimeOrigin::signed(ALICE),
					alice.clone(),
					AccountOrPerson::Account(BOB),
					GAME,
					0,
					0
				),
				DispatchError::BadOrigin
			);
			assert!(NftClaimCreditAwards::<Test>::decode_len(System::block_number()).is_none());
		});
	}
}

#[test]
fn attended_player_gets_credits_from_non_reporting_co_members() {
	// Regression test for the "award all unawarded NFT claim credits on attendance" feature.
	//
	// Setup: a single 3-player group, single round. Alice is the only one who
	// reports (she calls everyone a `Person`). Bob and Charlie never report.
	//
	// Under the old contract, Alice — despite being attended — would receive no
	// credits at all because nobody reported `Person` on her. Under the new contract,
	// the moment Alice's attendance is enacted, the pallet must backfill a credit for
	// every group co-member (Bob and Charlie) ⇒ exactly 2 credits for Alice.
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

		// The new contract: Alice has 2 credits — one keyed by (game, round, Bob,
		// Alice), one by (game, round, Charlie, Alice) — even though neither Bob
		// nor Charlie ever submitted a report.
		let alice_credits = awarded_credits(&alice);
		assert_eq!(alice_credits.len(), 2);
		assert!(alice_credits.contains(&NftCredits::compute_nft_claim_credit(1, 0, &bob, &alice)));
		assert!(
			alice_credits.contains(&NftCredits::compute_nft_claim_credit(1, 0, &charlie, &alice))
		);
	});
}

#[test]
fn player_process_step1_defers_players_that_do_not_fit_the_block_awards() {
	// ALICE, BOB and CHARLIE report all-`Person`; DAVE never reports. The three reporters
	// attend, and each is backfilled the one credit DAVE never awarded them. With room for a
	// single player's worst case per block, step1 processes one player per block and carries
	// the rest over, instead of dropping their credits.
	new_test_ext().execute_with(|| {
		let schedule = GameSchedule::<u32, u128> {
			game_play_time: 100,
			rounds: 1,
			max_group_size: 4,
			..Default::default()
		};
		start_game_in_reporting_phase(&schedule, &[ALICE, BOB, CHARLIE, DAVE]);

		for acc in [ALICE, BOB, CHARLIE] {
			let who = AccountOrPerson::Account(acc.clone());
			assert_ok!(Game::report(
				RuntimeOrigin::signed(acc),
				build_report_with_opinion(&who, |_| Report::Person),
			));
		}
		// 3 reporters × 3 co-players, all awarded during reporting.
		assert_eq!(awarded_credit_count(), 9);
		advance_process(); // commit the reporting block's leaves

		// One attendee's worst case is `rounds * (max_group_size - 1)` credits. Only that
		// many fit per block from here on, which is the bound `integrity_test` guarantees.
		MaxCreditsPerBlock::set(&3);

		MOCK_UNIX_TIME.with(|t| {
			*t.borrow_mut() =
				Duration::from_secs(GameTimes::<Test>::reporting_end(&schedule) as u64 + 1)
		});
		advance_process(); // reporting -> player process step1
		advance_process(); // step1: one player fits, the rest are deferred

		assert!(
			matches!(
				GameStore::<Test>::get().unwrap().state,
				GameState::PlayerProcess {
					step: PlayerProcessStep::Step1ProcessPlayers { last_iteration: Some(_), .. },
				}
			),
			"step1 must stop at the awards bound and resume in a later block",
		);

		while GameStore::<Test>::get().is_some() {
			advance_process();
		}
		advance_process(); // commit the last block's leaves

		// Each of the three attendees earned DAVE's credit on top of the nine awarded while
		// reporting, and every credit was committed to exactly one tree.
		assert_eq!(awarded_credit_count(), 12);
		assert_eq!(committed_leaf_count(), 12);
	});
}
