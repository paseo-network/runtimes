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

//! Proof-of-Personhood Game pallet benchmarking.

use super::*;

use codec::Encode;
use frame_benchmarking::v2::{benchmarks, *};

pub trait BenchmarkHelper<AccountSignature, TicketSignature, Ticket, AccountId, AirdropAssetId> {
	/// Creates an account deterministically from seed.
	fn create_account(seed: u64) -> AccountId;
	/// Creates a signature for a given message using an account.
	fn sign_account(seed: u64, msg: &[u8]) -> AccountSignature;
	/// Creates a ticket given a seed.
	fn create_ticket(seed: u64) -> Ticket;
	/// Creates a signature for a given ticket.
	fn sign_ticket(seed: u64, msg: &[u8]) -> TicketSignature;
	/// Moves in time away from the genesis block.
	/// Depends on `UnixTime` implementation.
	/// Needed due to the fact that pallet_timestamp::now() fails at genesis block.
	fn set_valid_time();
	fn set_time(now: core::time::Duration);
	fn fund_account(acc: AccountId);
	/// An asset id for the airdrop.
	fn airdrop_asset_id() -> AirdropAssetId;
}

#[benchmarks(
	where T: indiv_pallet_airdrop::Config + core::marker::Send + core::marker::Sync
		+ Config<
			AirdropAssetId = indiv_pallet_airdrop::AssetIdOf<T>,
			AirdropAssetBalance = indiv_pallet_airdrop::AssetBalanceOf<T>,
			Airdrop = indiv_pallet_airdrop::Pallet<T>
		>,
	<T as indiv_pallet_airdrop::Config>::Fungibles:
		frame_support::traits::fungibles::Create<T::AccountId>,
	<T as frame_system::Config>::RuntimeCall:
		Dispatchable<
			Info = DispatchInfo,
			PostInfo = PostDispatchInfo
		>
		+ From<Call<T>>,
	<<T as frame_system::Config>::RuntimeCall as Dispatchable>::RuntimeOrigin:
		AsSystemOriginSigner<T::AccountId> + AsTransactionAuthorizedOrigin + Clone,
)]
mod benches {
	use super::*;

	/// Sampling range for `remove_available_and_pending_invites`'s Linear
	/// regression. `PendingInvites` is user-controlled and unbounded; the runtime
	/// charges based on the caller-supplied `limit`, extrapolated linearly from
	/// this fit.
	const REMOVE_INVITES_SAMPLE_CAP: u32 = 10_000;

	use alloc::vec::Vec;
	use frame_support::{
		assert_ok,
		dispatch::{DispatchInfo, PostDispatchInfo, RawOrigin},
		pallet_prelude::One,
		traits::{
			fungibles::{Create, Inspect, Mutate},
			Consideration, ConstU32, EnsureOriginWithArg, UnixTime,
		},
		BoundedVec,
	};
	use indiv_pallet_score::{
		AbsenceGraceSchedule, AbsenceGraceTier, AbsenceGraceTiers, PersonhoodThresholdSchedule,
		PersonhoodThresholdTier, MAX_PERSONHOOD_THRESHOLD_TIERS,
	};
	use indiv_support::traits::{AddOnlyPeopleTrait, CountedMembers, PersonalId};
	use sp_core::Get;
	use sp_runtime::{
		traits::{
			AsSystemOriginSigner, AsTransactionAuthorizedOrigin, DispatchTransaction, Dispatchable,
		},
		Saturating,
	};

	type Fungibles<T> = <T as indiv_pallet_airdrop::Config>::Fungibles;

	type PeopleOf<T> = <T as indiv_pallet_score::Config>::People;

	const DEFAULT_IDENTIFIER_KEY: CommunicationIdentifier = [42u8; 65];

	/// Make `who` a `Recognized` participant backed by a real person in
	/// [`indiv_pallet_score::Config::People`], so offboarding them exercises the personhood
	/// suspension. The participant entry must already exist. Returns the personal id.
	fn make_recognized_participant<T: Config>(
		who: &AccountOrPerson<T::AccountId>,
	) -> Result<PersonalId, BenchmarkError> {
		PeopleOf::<T>::initialize_people_collection();
		let id = PeopleOf::<T>::reserve_new_id();
		let (key, _) = PeopleOf::<T>::mock_key(id);
		PeopleOf::<T>::recognize_personhood(id, Some(key))?;
		indiv_pallet_score::Participants::<T>::mutate(who, |p| {
			p.as_mut().expect("participant entry exists").recognition =
				indiv_pallet_score::Recognition::Recognized(id);
		});
		Ok(id)
	}

	/// Valid prize for benchmark schedules.
	fn bench_airdrop_prize<T: Config>(
	) -> indiv_pallet_airdrop::types::AirdropPrize<T::AirdropAssetId, T::AirdropAssetBalance> {
		indiv_pallet_airdrop::types::AirdropPrize {
			asset_id: <T as Config>::BenchmarkHelper::airdrop_asset_id(),
			asset_amount: 1u32.into(),
			max_winners: 1,
			winner_cap: sp_runtime::Permill::one(),
		}
	}

	/// `n` valid airdrops for benchmark schedules, drawn one day apart.
	fn bench_airdrops<T: Config>(
		n: u32,
	) -> BoundedVec<
		GameAirdrop<T::AirdropAssetId, T::AirdropAssetBalance>,
		ConstU32<{ MAX_GAME_AIRDROPS as u32 }>,
	> {
		/// Seconds in a day (24 * 60 * 60), used to space benchmark airdrops one day apart.
		const SECONDS_PER_DAY: u32 = 24 * 60 * 60;

		(0..n)
			.map(|i| GameAirdrop {
				draw_offset: i.saturating_mul(SECONDS_PER_DAY),
				claim_window: SECONDS_PER_DAY,
				prize: bench_airdrop_prize::<T>(),
			})
			.collect::<Vec<_>>()
			.try_into()
			.expect("n is bounded by MAX_GAME_AIRDROPS")
	}

	/// Ensure the prize asset exists, is enabled in `indiv_pallet_airdrop::SupportedAssets`,
	/// and that `T::AirdropSource` holds enough of it to fund any schedule a benchmark may
	/// build. Idempotent.
	fn bench_setup_airdrop_funds<T>()
	where
		T: indiv_pallet_airdrop::Config
			+ Config<
				AirdropAssetId = indiv_pallet_airdrop::AssetIdOf<T>,
				AirdropAssetBalance = indiv_pallet_airdrop::AssetBalanceOf<T>,
				Airdrop = indiv_pallet_airdrop::Pallet<T>,
			>,
		<T as indiv_pallet_airdrop::Config>::Fungibles:
			frame_support::traits::fungibles::Create<T::AccountId>,
	{
		let asset_id = <T as Config>::BenchmarkHelper::airdrop_asset_id();
		let pot = indiv_pallet_airdrop::Pallet::<T>::airdrop_pot_id();

		if !<Fungibles<T> as Inspect<T::AccountId>>::asset_exists(asset_id.clone()) {
			<Fungibles<T> as Create<T::AccountId>>::create(
				asset_id.clone(),
				pot.clone(),
				true,
				1u32.into(),
			)
			.expect("create airdrop asset for bench");
		}

		let min = <Fungibles<T> as Inspect<T::AccountId>>::minimum_balance(asset_id.clone());
		if !indiv_pallet_airdrop::SupportedAssets::<T>::contains_key(&asset_id) {
			<Fungibles<T> as Mutate<T::AccountId>>::mint_into(asset_id.clone(), &pot, min)
				.expect("seed airdrop pot ED");
			indiv_pallet_airdrop::SupportedAssets::<T>::insert(&asset_id, min);
		}

		// Mint a generous multiple of the ED so any schedule a benchmark builds is fundable.
		let source = T::AirdropSource::get();
		<Fungibles<T> as Mutate<T::AccountId>>::mint_into(
			asset_id,
			&source,
			min.saturating_mul(1_000_000u32.into()),
		)
		.expect("fund AirdropSource");
	}

	/// Move the airdrop events for `game_index` from `Status::Scheduled` (where `new_game`
	/// leaves them) into `Status::Registering` so the participate paths accept the call.
	fn bench_open_airdrop_registration<T>(game_index: GameIdx, airdrop_count: u32)
	where
		T: indiv_pallet_airdrop::Config + Config,
	{
		for airdrop_index in 0..airdrop_count {
			let event_id = pallet::Pallet::<T>::airdrop_event_id(game_index, airdrop_index as u8);
			let mut event = indiv_pallet_airdrop::Events::<T>::get(event_id)
				.expect("airdrop event scheduled by new_game");
			event.status =
				indiv_pallet_airdrop::types::Status::Registering { total_participants: 0 };
			indiv_pallet_airdrop::Events::<T>::insert(event_id, event);
		}
	}

	/// Prepare the `airdrop_count` airdrop events for `game_index` to accept registrations and
	/// build the `AirdropVrfs::Alias` value (one valid proof per event) for the participant
	/// identified by `participant_origin`.
	///
	/// Alias proofs are the airdrop worst case for a sign-up: per-event ring-membership
	/// verification dominates the cheaper sr25519 VRF check used by `AirdropVrfs::Account`.
	fn bench_alias_vrfs<T>(
		game_index: GameIdx,
		airdrop_count: u32,
		participant_origin: &indiv_pallet_airdrop::types::RegistrationEntry<T::AccountId>,
	) -> AirdropVrfs<AirdropProofOf<T>>
	where
		T: indiv_pallet_airdrop::Config
			+ Config<
				AirdropAssetId = indiv_pallet_airdrop::AssetIdOf<T>,
				AirdropAssetBalance = indiv_pallet_airdrop::AssetBalanceOf<T>,
				Airdrop = indiv_pallet_airdrop::Pallet<T>,
			>,
	{
		bench_open_airdrop_registration::<T>(game_index, airdrop_count);
		let message = codec::Encode::encode(participant_origin);
		let proofs = (0..airdrop_count)
			.map(|airdrop_index| {
				let event_id =
					pallet::Pallet::<T>::airdrop_event_id(game_index, airdrop_index as u8);
				let context = indiv_pallet_airdrop::context_for_event(&event_id);
				let (proof, _alias) = <<T as indiv_pallet_airdrop::Config>::BenchmarkHelper
					as indiv_pallet_airdrop::benchmarking::BenchmarkHelper<T>>::build_membership_proof(
						&context, &message, 0,
					);
				proof
			})
			.collect::<Vec<_>>()
			.try_into()
			.expect("airdrop_count is bounded by MAX_GAME_AIRDROPS");
		AirdropVrfs::Alias { proofs, ring_index: 0, revision: 0 }
	}

	/// Open registration on the `airdrop_count` airdrop events for `game_index` and build the
	/// `AirdropVrfs::Account` value (one valid signature per event) using an sr25519 keypair
	/// sourced from airdrop's benchmark helper. Returns the matching `AccountId` (which the
	/// bench must use as the call's caller — the VRFs only verify against this specific
	/// account's pubkey).
	fn bench_account_vrfs<T>(
		game_index: GameIdx,
		airdrop_count: u32,
	) -> (T::AccountId, AirdropVrfs<AirdropProofOf<T>>)
	where
		T: indiv_pallet_airdrop::Config
			+ Config<
				AirdropAssetId = indiv_pallet_airdrop::AssetIdOf<T>,
				AirdropAssetBalance = indiv_pallet_airdrop::AssetBalanceOf<T>,
				Airdrop = indiv_pallet_airdrop::Pallet<T>,
			>,
	{
		use sp_runtime::traits::TryConvert;
		bench_open_airdrop_registration::<T>(game_index, airdrop_count);
		let (account_id, pair) =
			<<T as indiv_pallet_airdrop::Config>::BenchmarkHelper
				as indiv_pallet_airdrop::benchmarking::BenchmarkHelper<T>>::account_keypair_for(0);
		let public =
			<T as indiv_pallet_airdrop::Config>::AccountIdToPublic::try_convert(account_id.clone())
				.expect("benchmark account must map to an sr25519 public key");
		let vrfs = (0..airdrop_count)
			.map(|airdrop_index| {
				let event_id =
					pallet::Pallet::<T>::airdrop_event_id(game_index, airdrop_index as u8);
				let transcript =
					indiv_pallet_airdrop::vrf::transcript_for_event(&event_id, &public);
				indiv_pallet_airdrop::benchmarking::vrf_sign_via_schnorrkel(&pair, transcript)
			})
			.collect::<Vec<_>>()
			.try_into()
			.expect("airdrop_count is bounded by MAX_GAME_AIRDROPS");
		(account_id, AirdropVrfs::Account(vrfs))
	}

	// `n` is the number of airdrop events the schedule carries, each of which is scheduled in
	// the airdrop pallet.
	#[benchmark]
	fn new_game(n: Linear<0, { MAX_GAME_AIRDROPS as u32 }>) -> Result<(), BenchmarkError> {
		let schedule = GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds: T::MaxRounds::get() as u8,
			max_group_size: T::MaxGroupSize::get(),
			airdrops: bench_airdrops::<T>(n),
		};

		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		#[block]
		{
			assert_ok!(pallet::Pallet::<T>::new_game(&schedule));
		}

		let game = Game::<T>::get().expect("Game should exist");
		assert_eq!(game.state, GameState::Registration { next_player_index: 0 });
		assert_eq!(game.max_group_size, schedule.max_group_size);
		assert_eq!(game.rounds, schedule.rounds);
		assert_eq!(u32::from(game.airdrops_scheduled), n, "every airdrop should be scheduled");

		Ok(())
	}

	#[benchmark]
	fn get_game() -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		// One game exists
		let schedule = GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds: T::MaxRounds::get() as u8,
			max_group_size: T::MaxGroupSize::get(),
			airdrops: bench_airdrops::<T>(1),
		};
		assert_ok!(pallet::Pallet::<T>::new_game(&schedule));

		let game: Option<GameInfo<T::AccountId>>;

		#[block]
		{
			game = Game::<T>::get();
		}

		// `get` returns it
		assert!(game.is_some());

		Ok(())
	}

	#[benchmark]
	fn get_game_schedules(
		n: Linear<1, { T::MaxGameSchedules::get() }>,
	) -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		// No scheduled games
		assert_eq!(GameSchedules::<T>::get().len(), 0);

		// n games to schedule
		let mut games_schedules = Vec::new();
		let offset = 1000u32;
		let mut prev_game_end = 2000u32;

		for _ in 0..n {
			let schedule = GameScheduleOf::<T> {
				game_play_time: prev_game_end + offset,
				rounds: T::MaxRounds::get() as u8,
				max_group_size: T::MaxGroupSize::get(),
				airdrops: bench_airdrops::<T>(MAX_GAME_AIRDROPS.into()),
			};
			prev_game_end = GameTimes::<T>::player_process_end(&schedule);

			games_schedules.push(schedule);
		}

		assert_ok!(pallet::Pallet::<T>::schedule_games(RawOrigin::Root.into(), games_schedules));

		let schedules: BoundedVec<GameScheduleOf<T>, T::MaxGameSchedules>;

		#[block]
		{
			schedules = pallet::GameSchedules::<T>::get();
		}

		// All the n schedules were created successfully
		assert_eq!(schedules.len(), n as usize);

		Ok(())
	}

	#[benchmark]
	fn unix_time() -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		#[block]
		{
			<T as Config>::UnixTime::now();
		}

		Ok(())
	}

	#[benchmark]
	fn put_game() -> Result<(), BenchmarkError> {
		#[block]
		{
			Game::<T>::put(GameInfo {
				index: 0,
				registration_ends: 0,
				shuffle_deadline: 1,
				game_date: 0,
				report_ends: 0,
				state: GameState::Registration { next_player_index: 0 },
				max_group_size: T::MaxGroupSize::get(),
				rounds: T::MaxRounds::get() as u8,
				pending_attendance: 0,
				airdrops_scheduled: 0,
			})
		}
		Ok(())
	}

	#[benchmark]
	fn put_game_schedules() -> Result<(), BenchmarkError> {
		let mut schedules = BoundedVec::<GameScheduleOf<T>, T::MaxGameSchedules>::default();
		for _ in 0..T::MaxGameSchedules::get() {
			let _ = schedules.try_push(GameScheduleOf::<T> {
				game_play_time: 1000,
				rounds: T::MaxRounds::get() as u8,
				max_group_size: T::MaxGroupSize::get(),
				airdrops: bench_airdrops::<T>(MAX_GAME_AIRDROPS.into()),
			});
		}

		#[block]
		{
			GameSchedules::<T>::put(schedules);
		}
		Ok(())
	}

	#[benchmark]
	fn shuffles_base() -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		let game = GameInfo {
			index: 0,
			registration_ends: 0,
			shuffle_deadline: 1, // to not fail at deadline check
			game_date: 0,
			report_ends: 0,
			state: GameState::Shuffle { step: ShuffleStep::Step1Insert { last_iteration: None } },
			max_group_size: T::MaxGroupSize::get(),
			rounds: T::MaxRounds::get() as u8,
			pending_attendance: 0,
			airdrops_scheduled: 0,
		};

		let mut meter = WeightMeter::new();

		#[block]
		{
			pallet::Pallet::<T>::shuffles(&mut meter, game);
		}

		assert_eq!(Game::<T>::get().unwrap().state, GameState::Reporting { player_count: 0 });

		Ok(())
	}

	#[benchmark]
	fn shuffle_step_insert(n: Linear<1, { T::MaxRounds::get() }>) -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		let rounds = n.try_into().unwrap();

		// One game exists
		let schedule = crate::types::GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds,
			max_group_size: 2,
			airdrops: bench_airdrops::<T>(1),
		};
		assert_ok!(pallet::Pallet::<T>::new_game(&schedule));

		// Two users join the game
		let alice: T::AccountId = account("Alice", 0, 0);
		let bob: T::AccountId = account("Bob", 1, 0);

		<T as Config>::BenchmarkHelper::fund_account(alice.clone());
		<T as Config>::BenchmarkHelper::fund_account(bob.clone());

		let result = pallet::Pallet::<T>::sign_up_with_account(
			RawOrigin::Signed(alice.clone()).into(),
			DEFAULT_IDENTIFIER_KEY,
			None,
		);
		assert!(
			result.is_ok(),
			"sign_up_with_account failed for Alice: {:?}",
			result.err().unwrap().error
		);

		let result = pallet::Pallet::<T>::sign_up_with_account(
			RawOrigin::Signed(bob.clone()).into(),
			DEFAULT_IDENTIFIER_KEY,
			None,
		);
		assert!(
			result.is_ok(),
			"sign_up_with_account failed for Bob: {:?}",
			result.err().unwrap().error
		);

		let parent_hash = frame_system::Pallet::<T>::parent_hash();
		let rounds = n.try_into().unwrap();
		let mut last_key = None;
		let mut pending_attendance = 0u32;

		// Do one shuffle step insert.
		assert_eq!(
			pallet::Pallet::<T>::shuffle_step_insert(
				&mut last_key,
				&mut pending_attendance,
				rounds,
				&parent_hash
			),
			StepResult::Continue
		);

		// Benchmark the second one.
		#[block]
		{
			let _ = pallet::Pallet::<T>::shuffle_step_insert(
				&mut last_key,
				&mut pending_attendance,
				rounds,
				&parent_hash,
			);
		}

		for round in 0..rounds {
			assert_eq!(ShuffleNotRecognized::<T>::iter_prefix(round).count(), 2);
		}

		Ok(())
	}

	#[benchmark]
	fn shuffle_step_retrieve(n: Linear<1, { T::MaxRounds::get() }>) -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		let rounds = n.try_into().unwrap();

		// One game exists
		let schedule = crate::types::GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds,
			max_group_size: 2,
			airdrops: bench_airdrops::<T>(1),
		};
		assert_ok!(pallet::Pallet::<T>::new_game(&schedule));

		// Two users join the game
		let alice: T::AccountId = account("Alice", 0, 0);
		let bob: T::AccountId = account("Bob", 1, 0);

		<T as Config>::BenchmarkHelper::fund_account(alice.clone());
		<T as Config>::BenchmarkHelper::fund_account(bob.clone());

		let result = pallet::Pallet::<T>::sign_up_with_account(
			RawOrigin::Signed(alice.clone()).into(),
			DEFAULT_IDENTIFIER_KEY,
			None,
		);
		assert!(
			result.is_ok(),
			"sign_up_with_account failed for Alice: {:?}",
			result.err().unwrap().error
		);

		let result = pallet::Pallet::<T>::sign_up_with_account(
			RawOrigin::Signed(bob.clone()).into(),
			DEFAULT_IDENTIFIER_KEY,
			None,
		);
		assert!(
			result.is_ok(),
			"sign_up_with_account failed for Bob: {:?}",
			result.err().unwrap().error
		);

		let parent_hash = frame_system::Pallet::<T>::parent_hash();
		let rounds = n.try_into().unwrap();
		let mut last_key = None;
		let mut pending_attendance = 0u32;

		let _ = pallet::Pallet::<T>::shuffle_step_insert(
			&mut last_key,
			&mut pending_attendance,
			rounds,
			&parent_hash,
		);
		let _ = pallet::Pallet::<T>::shuffle_step_insert(
			&mut last_key,
			&mut pending_attendance,
			rounds,
			&parent_hash,
		);

		for round in 0..rounds {
			assert_eq!(ShuffleNotRecognized::<T>::iter_prefix(round).count(), 2);
		}

		let mut phase = ShuffleRetrievePhase::NotRecognized { recognized_count: 0 };
		let mut next_index = 0;
		let mut cached_last_keys = vec![None; usize::from(rounds)];

		// Do one shuffle step retrieve.
		let _ = pallet::Pallet::<T>::shuffle_step_retrieve(
			&mut next_index,
			&mut phase,
			&mut cached_last_keys,
			rounds,
		);

		// Benchmark the second one.
		#[block]
		{
			let _ = pallet::Pallet::<T>::shuffle_step_retrieve(
				&mut next_index,
				&mut phase,
				&mut cached_last_keys,
				rounds,
			);
		}

		// Both users get assigned different indexes and are stored in both mapping storage items
		let alice_aop = AccountOrPerson::Account(alice);
		let bob_aop = AccountOrPerson::Account(bob);

		for round in 0..rounds {
			assert_eq!(ShuffleNotRecognized::<T>::iter_prefix(round).count(), 0);

			let alice_index = PlayerToIndex::<T>::get(&alice_aop).unwrap()[round as usize];
			let bob_index = PlayerToIndex::<T>::get(&bob_aop).unwrap()[round as usize];

			assert_eq!(IndexToPlayer::<T>::get((round, alice_index)), Some(alice_aop.clone()));
			assert_eq!(IndexToPlayer::<T>::get((round, bob_index)), Some(bob_aop.clone()));
		}

		Ok(())
	}

	#[benchmark]
	fn shuffle_step_compute_weights(
		// `p` is the swept product `rounds * group_size`. The per-call cost scales with
		// `rounds * (group_size - 1)` inner iterations, a product that a separable
		// `f(group_size, rounds)` weight cannot model, so we sweep a single component over the
		// whole product range and derive the two dimensions from it below. (The benchmark macro
		// requires single-letter parameter names, hence `p` rather than a spelled-out name.)
		p: Linear<2, { T::MaxRounds::get() * T::MaxGroupSize::get() }>,
	) -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		// Split the swept product `p` back into a group size and a round count. Group size grows
		// first, up to `max_group_size`, then rounds take over for the rest of the range. This way
		// small games (a low product) are measured too, instead of only sweeping rounds at the
		// maximum group size. The runtime charges the worst case, `rounds * max_group_size`.
		let group_size = p.clamp(2, T::MaxGroupSize::get());
		let rounds = (p / group_size).max(1) as u8;
		let player_count = 2u32 * group_size;

		// One game exists with `max_group_size = group_size`.
		let schedule = crate::types::GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds,
			max_group_size: group_size,
			airdrops: bench_airdrops::<T>(1),
		};
		assert_ok!(pallet::Pallet::<T>::new_game(&schedule));

		// Sign up `2 * group_size` players so the resulting two groups are both full.
		for i in 0..player_count {
			let player: T::AccountId = account("player", i, i);
			<T as Config>::BenchmarkHelper::fund_account(player.clone());
			let result = pallet::Pallet::<T>::sign_up_with_account(
				RawOrigin::Signed(player.clone()).into(),
				DEFAULT_IDENTIFIER_KEY,
				None,
			);
			assert!(
				result.is_ok(),
				"sign_up_with_account failed for player {i}: {:?}",
				result.err().unwrap().error
			);
		}

		// Run Step1 to insert every player into the per-round shuffle storages.
		let parent_hash = frame_system::Pallet::<T>::parent_hash();
		let mut step1_last_key = None;
		let mut pending_attendance = 0u32;
		loop {
			let r = pallet::Pallet::<T>::shuffle_step_insert(
				&mut step1_last_key,
				&mut pending_attendance,
				rounds,
				&parent_hash,
			);
			if matches!(r, StepResult::Finished) {
				break;
			}
		}

		// Run Step2 to assign player indices in every round.
		let mut next_index = 0;
		let mut phase = ShuffleRetrievePhase::Recognized;
		let mut cached_last_keys = vec![None; usize::from(rounds)];
		loop {
			let r = pallet::Pallet::<T>::shuffle_step_retrieve(
				&mut next_index,
				&mut phase,
				&mut cached_last_keys,
				rounds,
			);
			if matches!(r, StepResult::Finished) {
				break;
			}
		}

		assert_eq!(next_index, player_count);

		let ShuffleRetrievePhase::NotRecognized { recognized_count } = phase else {
			panic!("unexpected: shuffle step retrieve did not reach the not-recognized phase");
		};

		// Warm up Step3 once so the benchmarked call uses `iter_from_key`.
		let mut last_iteration = None;
		let _ = pallet::Pallet::<T>::shuffle_step_compute_weights(
			&mut last_iteration,
			rounds,
			next_index,
			recognized_count,
			group_size,
		);
		let warmup_player = last_iteration.clone().expect("warmup processed one player");

		#[block]
		{
			let _ = pallet::Pallet::<T>::shuffle_step_compute_weights(
				&mut last_iteration,
				rounds,
				next_index,
				recognized_count,
				group_size,
			);
		}

		let processed_player = last_iteration.expect("benchmark processed one player");
		assert!(processed_player != warmup_player, "benchmark must advance past the warmup player");
		let player_info = Players::<T>::get(&processed_player).expect("player should still exist");
		// All co-players are candidates (not externally recognised), so weight per voter
		// is `CandidateVoteWeight`. The player's group has `group_size` members, hence
		// `group_size - 1` voters per round.
		let expected = (T::CandidateVoteWeight::get() as u32)
			.saturating_mul((group_size - 1).saturating_mul(rounds as u32));
		let expected_u16: u16 = expected.try_into().unwrap_or(u16::MAX);
		assert_eq!(player_info.expected_max_vote_weight, expected_u16);

		Ok(())
	}

	#[benchmark]
	fn shuffle_step_start_session() -> Result<(), BenchmarkError> {
		// The session must be startable at this point.
		assert!(indiv_pallet_score::Pallet::<T>::can_start_attendance_report_session());

		// Worst-case `update_thresholds`: fill both schedules to max and set `active_count` past
		// every threshold so both `find()`s walk fully and return `None`.
		PersonhoodThresholdSchedule::<T>::put(BoundedVec::truncate_from(
			(1..=MAX_PERSONHOOD_THRESHOLD_TIERS)
				.map(|t| PersonhoodThresholdTier {
					population_size_threshold: t,
					score_threshold: 100,
				})
				.collect(),
		));
		let absence_max = AbsenceGraceTiers::bound() as u32;
		AbsenceGraceSchedule::<T>::put(BoundedVec::truncate_from(
			(1..=absence_max)
				.map(|t| AbsenceGraceTier {
					population_size_threshold: t,
					window: 8,
					allowed_misses: 7,
				})
				.collect(),
		));
		// The largest non-`u32::MAX` threshold across both schedules is
		// `max(MAX_PERSONHOOD_THRESHOLD_TIERS, AbsenceGraceTiers::bound())` (the loops
		// above use `1..=N`). Picking any `active_count` strictly above that misses every
		// tier in both schedules, so each `find()` traverses its full length.
		let largest_non_last_threshold = MAX_PERSONHOOD_THRESHOLD_TIERS.max(absence_max);
		<T as indiv_pallet_score::Config>::EnsurePerson::set_active_count(
			largest_non_last_threshold + 1,
		);

		#[block]
		{
			// Mirror the logic inside `shuffles` for the `Step4AwaitSession` step:
			// both the `can_start` check and the `start` call are on the hot path,
			// so they must be included in the benchmarked block.
			assert!(indiv_pallet_score::Pallet::<T>::can_start_attendance_report_session());
			assert!(indiv_pallet_score::Pallet::<T>::start_attendance_report_session().is_ok());
		}

		// Clean up so the benchmark leaves the pallet state consistent for the test
		// harness.
		let _ = indiv_pallet_score::Pallet::<T>::end_attendance_report_session();

		Ok(())
	}

	// Base cost: load the game, observe that there are no players left to process,
	// end the attendance report session, and transition to Step2.
	#[benchmark]
	fn player_process_step1() -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		let max_group_size = T::MaxGroupSize::get();
		let rounds = T::MaxRounds::get() as u8;
		let game_index = 0u32;

		let game = GameInfo {
			index: game_index,
			registration_ends: 0,
			shuffle_deadline: 0,
			game_date: 0,
			report_ends: 0,
			state: GameState::PlayerProcess {
				step: PlayerProcessStep::Step1ProcessPlayers {
					last_iteration: None,
					player_count: 0,
				},
			},
			max_group_size,
			rounds,
			pending_attendance: 0,
			airdrops_scheduled: 0,
		};

		let mut meter = WeightMeter::new();

		indiv_pallet_score::Pallet::<T>::start_attendance_report_session().unwrap();
		Game::<T>::put(game);

		// All the players are processed for the given game
		#[block]
		{
			pallet::Pallet::<T>::player_process_step1(&mut meter);
		}

		// Step 1 finished and handed off to step 2.
		assert!(matches!(
			Game::<T>::get().unwrap().state,
			GameState::PlayerProcess { step: PlayerProcessStep::Step2ClearIndices },
		));

		Ok(())
	}

	// Per-player worst case: the target is freshly decided as Attended. The
	// iteration runs `apply_attendance` (including score payout writes), rotates a
	// full attendance history, awards all attendance NFT claim credits, promotes all staged
	// candidates, and drops a deposit after the target reaches personhood.
	#[benchmark]
	fn player_process_step1_inner_loop(
		r: Linear<1, { T::MaxRounds::get() }>,
	) -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();

		let max_group_size = T::MaxGroupSize::get();
		let rounds = r as u8;
		let game_index = 0u32;
		let player_count = max_group_size;
		let target_account: T::AccountId = account("target", 0, 0);
		let target_player = AccountOrPerson::Account(target_account.clone());
		<T as Config>::BenchmarkHelper::fund_account(target_account.clone());
		T::PlayDeposit::ensure_successful(&target_account, pallet::PlayDepositAmount::<T>::get());

		let mut game: GameInfo<T::AccountId> = GameInfo {
			index: game_index,
			registration_ends: 0,
			shuffle_deadline: 0,
			game_date: 0,
			report_ends: 0,
			state: GameState::PlayerProcess {
				step: PlayerProcessStep::Step1ProcessPlayers { last_iteration: None, player_count },
			},
			max_group_size,
			rounds,
			pending_attendance: player_count,
			airdrops_scheduled: 0,
		};

		for i in 0..player_count {
			let mut player = Player {
				first_game: 0,
				registered: true,
				sent_report: true,
				early_attendance_enactment: None,
				yes_person: max_group_size as u8 - 1,
				no_not_person: 0,
				expected_max_vote_weight: max_group_size as u16 - 1,
				vote_weight: T::CandidateVoteWeight::get(),
				credibility: PlayerCredibility::Recognized,
			};
			let account_or_person = if i == 0 {
				indiv_pallet_score::Pallet::<T>::onboard_for_recognition(&target_account)?;
				player.credibility = PlayerCredibility::Deposit(T::PlayDeposit::new(
					&target_account,
					pallet::PlayDepositAmount::<T>::get(),
				)?);
				target_player.clone()
			} else {
				let mut alias = [0u8; 32];
				let i_bytes = i.to_le_bytes();
				alias[..i_bytes.len()].copy_from_slice(&i_bytes);
				let account_or_person = AccountOrPerson::Person(alias);
				let stmt_acc: T::AccountId = account("stmt", i, i);
				AliasToStmtAccount::<T>::insert(alias, &stmt_acc);
				StmtAccountToAlias::<T>::insert(&stmt_acc, alias);
				sp_statement_store::increase_allowance_by(
					stmt_acc.clone().into(),
					T::PlayerStatementLimit::get(),
				);
				assert!(
					indiv_pallet_score::Pallet::<T>::onboard_externally_recognized(&alias).is_ok()
				);
				account_or_person
			};

			Players::<T>::insert(&account_or_person, player);

			let indices =
				BoundedVec::try_from(vec![i; rounds as usize]).expect("rounds within bound");
			PlayerToIndex::<T>::insert(&account_or_person, indices);
			for round in 0..rounds {
				IndexToPlayer::<T>::insert((round, i), &account_or_person);
			}
		}

		let mut attendance = PlayerAttendanceHistory::<T>::get(&target_player);
		for i in 0..T::MaxAttendanceHistoryDepth::get() {
			let _ = attendance.try_push(i);
		}
		PlayerAttendanceHistory::<T>::insert(&target_player, attendance);

		indiv_pallet_score::Pallet::<T>::start_attendance_report_session().unwrap();
		let personhood_threshold = indiv_pallet_score::PersonhoodThreshold::<T>::get();
		indiv_pallet_score::Participants::<T>::mutate(&target_player, |maybe_participant| {
			let participant =
				maybe_participant.as_mut().expect("benchmark seeded score participant");
			participant.score = personhood_threshold.saturating_sub(1);
			participant.streak = indiv_pallet_score::Streak::Attended(1u8);
			participant.reached_personhood = false;
			participant.has_ever_reached_personhood = false;
		});
		let award_time = <T as Config>::UnixTime::now().as_secs() as u32;
		let mut last_iteration = None;
		let next_player = (
			target_player.clone(),
			Players::<T>::get(&target_player).expect("benchmark seeded target player"),
		);
		let mut iterator = Players::<T>::iter_from_key(target_player.clone());
		let mut credits_awarded = 0;

		#[block]
		{
			pallet::Pallet::<T>::process_player_attendance_outcome(
				game.index,
				game.rounds,
				game.max_group_size,
				&mut game.pending_attendance,
				player_count,
				&mut last_iteration,
				next_player,
				&mut iterator,
				award_time,
				&mut credits_awarded,
			);
		}

		assert_eq!(last_iteration, Some(target_player.clone()));
		let stored = <Players<T>>::get(&target_player)
			.expect("attended player should remain in Players, registered = false");
		assert!(!stored.registered);
		assert!(<ArchivedPlayers<T>>::get(&target_player).is_none());
		assert!(matches!(stored.credibility, PlayerCredibility::Recognized));
		let attendance = PlayerAttendanceHistory::<T>::get(&target_player);
		assert_eq!(attendance.len() as u32, T::MaxAttendanceHistoryDepth::get());
		assert_eq!(attendance.last(), Some(&game_index));
		assert!(indiv_pallet_score::Pallet::<T>::reached_personhood(&target_player));
		assert_eq!(game.pending_attendance, player_count - 1);

		Ok(())
	}

	#[benchmark]
	fn player_process_step2() -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		let rounds = T::MaxRounds::get() as u8;
		Game::<T>::put(GameInfo {
			index: 0,
			registration_ends: 0,
			shuffle_deadline: 0,
			game_date: 0,
			report_ends: 0,
			state: GameState::PlayerProcess { step: PlayerProcessStep::Step2ClearIndices },
			max_group_size: T::MaxGroupSize::get(),
			rounds,
			pending_attendance: 0,
			airdrops_scheduled: 0,
		});

		let mut meter = WeightMeter::new();

		#[block]
		{
			pallet::Pallet::<T>::player_process_step2(&mut meter);
		}

		assert!(Game::<T>::get().is_none(), "game should be killed after player_process_step2");

		Ok(())
	}

	#[benchmark]
	fn player_process_step2_inner_loop() -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();

		let total_entries = PLAYER_PROCESS_STEP2_CHUNK.saturating_add(1);
		let rounds = T::MaxRounds::get() as u8;
		let game_index = 1u32;
		for i in 0..total_entries {
			let mut alias = [0u8; 32];
			let i_bytes = i.to_le_bytes();
			alias[..i_bytes.len()].copy_from_slice(&i_bytes);
			let account_or_person = AccountOrPerson::Person(alias);

			let indices =
				BoundedVec::try_from(vec![i; rounds as usize]).expect("rounds within bound");
			PlayerToIndex::<T>::insert(&account_or_person, indices);
			IndexToPlayer::<T>::insert((0u8, i), &account_or_person);
			T::NftClaimCredits::benchmark_award_every_slot(game_index, &account_or_person);
		}

		let mut cursor1: Option<Vec<u8>> = None;
		let mut cursor2: Option<Vec<u8>> = None;
		let mut cursor3: Option<Vec<u8>> = None;
		let mut done1 = false;
		let mut done2 = false;
		let mut done3 = false;

		#[block]
		{
			pallet::Pallet::<T>::player_process_step2_inner_loop(
				game_index,
				&mut cursor1,
				&mut cursor2,
				&mut cursor3,
				&mut done1,
				&mut done2,
				&mut done3,
			);
		}

		assert!(IndexToPlayer::<T>::iter().count() < total_entries as usize);
		assert!(PlayerToIndex::<T>::iter().count() < total_entries as usize);
		// The third map is the credits pallet's, reached through `T::NftClaimCredits`, so what it
		// costs is measured with whatever the runtime wires in and asserted in its own tests.

		Ok(())
	}

	#[benchmark]
	fn process_cancelling() -> Result<(), BenchmarkError> {
		// A game in the cancelling state exists, with the player-drain
		// sub-step already reached (so the benchmark exercises the path
		// that actually completes the cancellation).
		let game = GameInfo {
			index: 0,
			registration_ends: 0,
			shuffle_deadline: 0,
			game_date: 0,
			report_ends: 0,
			state: GameState::Cancelling {
				step: CancellingStep::Step2DrainPlayers { last_iteration: None },
			},
			max_group_size: T::MaxGroupSize::get(),
			rounds: T::MaxRounds::get() as u8,
			pending_attendance: 0,
			// `process_cancelling` never touches the airdrop — the refund happens once at the
			// transition in `on_game_cancelled` (benchmarked separately). `airdrops_scheduled` is
			// inert here; if refund logic ever moves into this path, set up a funded event.
			airdrops_scheduled: 0,
		};

		// No players exists for the game so `process_cancelling_step_player` should do minimal
		// computation

		let mut meter = WeightMeter::new();
		Game::<T>::put(game);

		#[block]
		{
			pallet::Pallet::<T>::process_cancelling(&mut meter);
		}

		assert!(Game::<T>::get().is_none(), "The game should be removed");

		Ok(())
	}

	#[benchmark]
	fn process_cancelling_step_shuffle() -> Result<(), BenchmarkError> {
		// Pre-populate more than one chunk worth of entries in each shuffle map so that a
		// single step does the worst-case work (a full `CANCELLING_SHUFFLE_CHUNK`-sized clear).
		let total_entries = CANCELLING_SHUFFLE_CHUNK.saturating_add(1);
		let aop: AccountOrPerson<T::AccountId> =
			AccountOrPerson::Account(<T as Config>::BenchmarkHelper::create_account(0));
		for i in 0..total_entries {
			let hash = sp_io::hashing::blake2_256(&i.to_le_bytes());
			ShuffleRecognized::<T>::insert(0u8, hash, &aop);
			ShuffleNotRecognized::<T>::insert(0u8, hash, &aop);
		}

		let mut cursor1: Option<Vec<u8>> = None;
		let mut cursor2: Option<Vec<u8>> = None;
		let mut done1 = false;
		let mut done2 = false;

		#[block]
		{
			pallet::Pallet::<T>::process_cancelling_step_shuffle(
				&mut cursor1,
				&mut cursor2,
				&mut done1,
				&mut done2,
			);
		}

		assert!(ShuffleRecognized::<T>::iter().count() < total_entries as usize);
		assert!(ShuffleNotRecognized::<T>::iter().count() < total_entries as usize);

		Ok(())
	}

	#[benchmark]
	fn process_cancelling_step_player(
		n: Linear<1, { T::MaxRounds::get() }>,
	) -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		let rounds = n.try_into().unwrap();

		// One game exists
		let schedule = crate::types::GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds,
			max_group_size: T::MaxGroupSize::get(),
			airdrops: bench_airdrops::<T>(1),
		};
		assert_ok!(pallet::Pallet::<T>::new_game(&schedule));

		// One user joins the game
		let seed = 1u64; // to match the alias in try_successful_origin
		let person: Alias = [seed as u8; 32];

		let statement_account = <T as Config>::BenchmarkHelper::create_account(seed);

		let account_or_person = AccountOrPerson::Person(person);

		let msg = (Pallet::<T>::proof_of_ownership_msg_base(), &person)
			.using_encoded(sp_io::hashing::blake2_256);
		let signature = <T as Config>::BenchmarkHelper::sign_account(seed, &msg[..]);

		let score_context = indiv_pallet_score::Pallet::<T>::score_context();
		let origin = T::EnsurePerson::try_successful_origin(&score_context)
			.map_err(|_| BenchmarkError::Weightless)?;

		let result = pallet::Pallet::<T>::sign_up_with_alias(
			origin,
			DEFAULT_IDENTIFIER_KEY,
			statement_account.clone(),
			signature,
			None,
		);
		assert!(result.is_ok(), "sign_up_with_alias failed: {:?}", result.err().unwrap().error);

		let indices: BoundedVec<PlayerIndex, T::MaxRounds> =
			BoundedVec::try_from(vec![0u32; rounds as usize])
				.expect("rounds <= MaxRounds by Linear bound");
		PlayerToIndex::<T>::insert(&account_or_person, indices);
		for round in 0..rounds {
			IndexToPlayer::<T>::insert((round, 0u32), &account_or_person);
		}
		assert!(PlayerToIndex::<T>::contains_key(&account_or_person));

		// The cancelled game's credit mask is dropped with the player's indices.
		let game_index = Game::<T>::get().expect("game was scheduled").index;
		T::NftClaimCredits::benchmark_award_every_slot(game_index, &account_or_person);

		#[block]
		{
			assert!(!pallet::Pallet::<T>::process_cancelling_step_player(
				game_index, &mut None, rounds
			));
		}

		// All the player's info is reset
		let player = Players::<T>::get(&account_or_person).unwrap();
		assert!(!player.registered, "Player should have registered set to false");
		assert!(!player.sent_report, "Player should have sent_report set to false");

		// All player's indices were removed
		assert!(
			!PlayerToIndex::<T>::contains_key(&account_or_person),
			"PlayerToIndex should not contain player "
		);

		// IndexToPlayer mappings were removed for all rounds
		for round in 0..rounds {
			assert!(
				IndexToPlayer::<T>::get((round, 0)).is_none(),
				"IndexToPlayer should not contain mapping for round {round}",
			);
		}

		Ok(())
	}

	// Invites are for brand-new players, so they're never recognized — only the
	// `AirdropVrf::Account` variant is possible here.
	// `n` is the number of scheduled airdrop events the sign-up registers into; zero skips
	// airdrop registration entirely.
	#[benchmark]
	fn sign_up_with_invite(
		n: Linear<0, { MAX_GAME_AIRDROPS as u32 }>,
	) -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		// A game exists and it's in registration state
		let schedule = GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds: T::MaxRounds::get() as u8,
			max_group_size: T::MaxGroupSize::get(),
			airdrops: bench_airdrops::<T>(n),
		};
		assert_ok!(pallet::Pallet::<T>::new_game(&schedule));
		let game_index = Game::<T>::get().expect("game exists").index;

		let (caller, vrfs) = bench_account_vrfs::<T>(game_index, n);

		#[extrinsic_call]
		_(Origin::Invited(caller.clone()), DEFAULT_IDENTIFIER_KEY, Some(vrfs));

		// The caller becomes a registered player
		let account_or_person: AccountOrPerson<T::AccountId> = AccountOrPerson::Account(caller);
		let player = <Players<T>>::get(account_or_person);
		assert!(player.is_some());
		assert!(player.unwrap().registered);

		Ok(())
	}

	// New (not playing, not archived) account signing up: `sign_up_inner` calls
	// `onboard_for_recognition` and takes a `PlayDeposit`. The account is `NotRecognized`
	// after onboarding, so the only possible airdrop variant is `AirdropVrf::Account`. At zero
	// airdrops VRF verification and airdrop registration are skipped entirely.
	#[benchmark]
	fn sign_up_with_account_new(
		n: Linear<0, { MAX_GAME_AIRDROPS as u32 }>,
	) -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		// A game exists and it's in registration state
		let schedule = GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds: T::MaxRounds::get() as u8,
			max_group_size: T::MaxGroupSize::get(),
			airdrops: bench_airdrops::<T>(n),
		};
		assert_ok!(pallet::Pallet::<T>::new_game(&schedule));
		let game_index = Game::<T>::get().expect("game exists").index;

		let (caller, vrfs) = bench_account_vrfs::<T>(game_index, n);

		// To make sure the deposit ticket creation for the game will succeed
		T::PlayDeposit::ensure_successful(&caller, pallet::PlayDepositAmount::<T>::get());

		#[extrinsic_call]
		sign_up_with_account(RawOrigin::Signed(caller.clone()), DEFAULT_IDENTIFIER_KEY, Some(vrfs));

		// New player + onboarding ran (caller wasn't in `Participants` beforehand).
		let aop: AccountOrPerson<T::AccountId> = AccountOrPerson::Account(caller);
		let player = <Players<T>>::get(&aop).expect("player inserted");
		assert!(player.registered);
		assert!(indiv_pallet_score::Participants::<T>::contains_key(&aop));
		Ok(())
	}

	// Recognized, currently-playing account signing up: onboarding is skipped, the airdrop path
	// now takes the worst case: `AirdropVrf::Alias` variant.
	#[benchmark]
	fn sign_up_with_account_recognized(
		n: Linear<0, { MAX_GAME_AIRDROPS as u32 }>,
	) -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		// A game exists and it's in registration state
		let schedule = GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds: T::MaxRounds::get() as u8,
			max_group_size: T::MaxGroupSize::get(),
			airdrops: bench_airdrops::<T>(n),
		};
		assert_ok!(pallet::Pallet::<T>::new_game(&schedule));
		let game_index = Game::<T>::get().expect("game exists").index;

		let caller: T::AccountId = whitelisted_caller();
		let account_or_person = AccountOrPerson::Account(caller.clone());

		// Fund the caller before seeding their `PlayerCredibility::Deposit`.
		<T as Config>::BenchmarkHelper::fund_account(caller.clone());

		// Seed the caller as a player who's already been onboarded (so the sign-up reuses the
		// existing record) and is recognized in pallet-score (so the alias airdrop variant is
		// the valid one).
		indiv_pallet_score::Participants::<T>::insert(
			&account_or_person,
			indiv_pallet_score::Participant {
				score: 0,
				streak: Default::default(),
				attendance_history: Default::default(),
				credit: 0u32.into(),
				cashed_out: false,
				reached_personhood: true,
				has_ever_reached_personhood: true,
				recognition: indiv_pallet_score::Recognition::ExternallyRecognized,
				last_attended_game: None,
			},
		);
		let deposit = T::PlayDeposit::new(&caller, pallet::PlayDepositAmount::<T>::get())?;
		Players::<T>::insert(
			&account_or_person,
			Player {
				first_game: 0,
				registered: false,
				sent_report: false,
				early_attendance_enactment: None,
				yes_person: 0,
				no_not_person: 0,
				expected_max_vote_weight: 0,
				vote_weight: 0,
				credibility: PlayerCredibility::Deposit(deposit),
			},
		);

		let participant_origin =
			indiv_pallet_airdrop::types::RegistrationEntry::Account { account_id: caller.clone() };
		let vrfs = bench_alias_vrfs::<T>(game_index, n, &participant_origin);

		#[extrinsic_call]
		sign_up_with_account(RawOrigin::Signed(caller.clone()), DEFAULT_IDENTIFIER_KEY, Some(vrfs));

		let player = <Players<T>>::get(&account_or_person).expect("existing player record");
		assert!(player.registered, "existing player marked registered for this game");
		Ok(())
	}

	#[benchmark]
	fn sign_up_with_alias(
		n: Linear<0, { MAX_GAME_AIRDROPS as u32 }>,
	) -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		// A game exists and it's in registration state
		let game_schedule = GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds: T::MaxRounds::get() as u8,
			max_group_size: T::MaxGroupSize::get(),
			airdrops: bench_airdrops::<T>(n),
		};
		Pallet::<T>::new_game(&game_schedule)?;
		let game_index = Game::<T>::get().expect("game exists").index;

		let seed = 1u64; // to match the alias in try_successful_origin
		let person: Alias = [seed as u8; 32];

		let statement_account = <T as Config>::BenchmarkHelper::create_account(seed);

		// Pre-seed with a different statement account so the measured call exercises
		// the heaviest Alias sub-branch: remove prev + insert new + allowance shuffle.
		let prev_stmt_account = <T as Config>::BenchmarkHelper::create_account(2u64);
		StmtAccountToAlias::<T>::insert(&prev_stmt_account, person);
		AliasToStmtAccount::<T>::insert(person, &prev_stmt_account);

		// Valid signature
		let msg = (Pallet::<T>::proof_of_ownership_msg_base(), &person)
			.using_encoded(sp_io::hashing::blake2_256);
		let signature = <T as Config>::BenchmarkHelper::sign_account(seed, &msg[..]);

		// Origin for the personal alias
		let score_context = indiv_pallet_score::Pallet::<T>::score_context();
		let origin = T::EnsurePerson::try_successful_origin(&score_context)
			.map_err(|_| BenchmarkError::Weightless)?;

		let participant_origin =
			indiv_pallet_airdrop::types::RegistrationEntry::Alias { alias: person };
		let vrfs = bench_alias_vrfs::<T>(game_index, n, &participant_origin);

		#[extrinsic_call]
		_(
			origin as T::RuntimeOrigin,
			DEFAULT_IDENTIFIER_KEY,
			statement_account.clone(),
			signature,
			Some(vrfs),
		);

		// The caller becomes a registered player
		let players_len: u32 = Players::<T>::iter().collect::<Vec<_>>().len().try_into().unwrap();
		assert_eq!(players_len, 1);

		let account_or_person: AccountOrPerson<T::AccountId> = AccountOrPerson::Person(person);
		let player = <Players<T>>::get(account_or_person);
		assert!(player.is_some());
		assert!(player.unwrap().registered);

		// Post-assertions confirm the replacement landed.
		assert!(!StmtAccountToAlias::<T>::contains_key(&prev_stmt_account));
		// With statement account mapping created
		assert_eq!(StmtAccountToAlias::<T>::get(&statement_account), Some(person));

		Ok(())
	}

	// `n` is the number of scheduled airdrop events the sign-up registers into
	#[benchmark]
	fn sign_up_with_account_lite_invite(
		n: Linear<0, { MAX_GAME_AIRDROPS as u32 }>,
	) -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		// A game exists and it's in registration state
		let schedule = GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds: T::MaxRounds::get() as u8,
			max_group_size: T::MaxGroupSize::get(),
			airdrops: bench_airdrops::<T>(n),
		};
		Pallet::<T>::new_game(&schedule)?;
		let game_index = Game::<T>::get().expect("game exists").index;

		let score_context = indiv_pallet_score::Pallet::<T>::score_context();
		let origin = T::EnsureLiteAlias::try_successful_origin(&score_context)
			.map_err(|_| BenchmarkError::Weightless)?;
		let alias = T::EnsureLiteAlias::try_origin(origin.clone(), &score_context)
			.map_err(|_| BenchmarkError::Weightless)?;

		let (account, vrfs) = bench_account_vrfs::<T>(game_index, n);
		LiteInvites::<T>::insert(alias, &account);

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, account.clone(), DEFAULT_IDENTIFIER_KEY, Some(vrfs));

		let who = AccountOrPerson::Account(account.clone());
		let player = Players::<T>::get(&who).expect("the invited account is a player");
		assert!(player.registered);
		assert!(matches!(player.credibility, PlayerCredibility::Invited));
		assert_eq!(LiteInvites::<T>::get(alias), Some(account));
		assert!(indiv_pallet_score::Participants::<T>::contains_key(&who));

		Ok(())
	}

	/// `report` has two independent cost drivers, swept as separate `Linear` components:
	///
	/// - `e`: the co-player entries the reporter submits across all rounds. Each drives one pass of
	///   the per-entry loop (one `IndexToPlayer` read, one `Players` tally mutation, one
	///   `AwardedNftClaimCredits` write). Bounded by `MaxRounds * (MaxGroupSize - 1)`.
	/// - `n`: the early-attendance enactments the call triggers. The enactment loop fires
	///   `try_early_attendance_enactment` per reported player plus the reporter, but only when a
	///   player crosses the attendance threshold, which depends on chain state, not on `e`. A fixed
	///   `e` can enact anywhere from 0 to `e + 1` players, so this work needs its own component
	///   rather than folding into the `e` slope.
	#[benchmark]
	fn report(
		e: Linear<
			{ T::MaxGroupSize::get() - 1 },
			{ T::MaxRounds::get() * (T::MaxGroupSize::get() - 1) },
		>,
		n: Linear<0, { T::MaxGroupSize::get() - 1 }>,
	) -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		// --- Scenario derived from the swept components (e, n) -------------------
		// `e`'s per-entry cost is constant, so its slope over `[max_group_size - 1,
		// MaxRounds * (max_group_size - 1)]` extrapolates exactly to production. We realise
		// `e` with full groups (`rounds = e / (max_group_size - 1)`); `div_ceil` keeps the
		// entry count `>= e` (conservative) on non-multiples.
		//
		// The weight has no rounds component. Full-group packing uses the fewest rounds for
		// a given `e`; production may use more (up to `MaxRounds`) as eliminations shrink
		// late groups. Safe, because the only round-dependent work is the outer loop
		// (`full_report`/`reporter_indices` indexing, `group_members` arithmetic, the
		// empty-report check): it reads no storage per round (`PlayerToIndex` is hoisted
		// before the loop) so adds nothing to the PoV, and its ref_time is `MaxRounds`
		// iterations of cheap arithmetic, negligible against the per-entry slope.
		//
		// `n` is swept up to `MaxGroupSize - 1` (the most feasible at the low end of `e`, a
		// single full round). Its per-enactment cost is constant (proof size exactly linear
		// in `n`), so the slope extrapolates to the production bound `e + 1`. The cap
		// keeps `n` orthogonal to `e` and every sample on the co-players-only path, so
		// frame-omni-bencher's rectangular sweep needs no custom `--low`/`--high` values.
		let max_group_size = T::MaxGroupSize::get();
		let rounds = e.div_ceil(max_group_size.saturating_sub(1)).clamp(1, T::MaxRounds::get());
		let coplayers_enacted = n;

		// A game exists
		let game_schedule = GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds: rounds as u8,
			max_group_size,
			airdrops: bench_airdrops::<T>(1),
		};
		assert_ok!(Pallet::<T>::new_game(&game_schedule));

		// The origin is a game participant that did not send a report yet
		let caller: T::AccountId = whitelisted_caller();

		// Two other players participate in the game
		let mut players = (0..T::MaxGroupSize::get().pow(2))
			.map(|i| account::<T::AccountId>("player", i, i))
			.collect::<Vec<_>>();
		players[0] = caller.clone();

		// To make sure all the accounts have enough funds to sign up
		for player in &players {
			<T as Config>::BenchmarkHelper::fund_account(player.clone());
		}

		for player in &players {
			let result = Pallet::<T>::sign_up_with_account(
				RawOrigin::Signed(player.clone()).into(),
				DEFAULT_IDENTIFIER_KEY,
				None,
			);
			assert!(
				result.is_ok(),
				"sign_up_with_account failed for player {:?}: {:?}",
				player,
				result.err().unwrap().error
			);
		}

		let shuffle_time = GameTimes::<T>::registration_end(&game_schedule);
		<T as Config>::BenchmarkHelper::set_time(Duration::from_secs(shuffle_time.into()));
		let game = Game::<T>::get().expect("Game should exist");
		Pallet::<T>::process_game(&mut WeightMeter::new(), 1u32.into(), game);
		let game = Game::<T>::get().expect("Game should exist");
		Pallet::<T>::process_game(&mut WeightMeter::new(), 1u32.into(), game);

		let game_time = GameTimes::<T>::game_play_time(&game_schedule);
		<T as Config>::BenchmarkHelper::set_time(Duration::from_secs(game_time.into()));

		// The actual shuffle gives pseudo-random groupings; the caller's co-player
		// count across rounds isn't guaranteed to reach the worst-case bound of
		// `rounds * (MaxGroupSize - 1)` unique players. Rewrite the caller's indices
		// and the other slots of the caller's group so each round exposes a disjoint
		// set of co-players, pushing `reported_players.len()` to the max.
		let num_groups = max_group_size; // player_count / max_per_group == MaxGroupSize

		let caller_aop = AccountOrPerson::Account(caller.clone());
		let caller_indices: BoundedVec<PlayerIndex, T::MaxRounds> =
			vec![0u32; rounds as usize].try_into().expect("fits MaxRounds");
		PlayerToIndex::<T>::insert(&caller_aop, caller_indices);

		// Group 0 slots other than the caller's slot 0.
		let group0_other_slots: Vec<u32> = (1..max_group_size).map(|k| k * num_groups).collect();
		for round in 0..rounds as u8 {
			for (k, &slot) in group0_other_slots.iter().enumerate() {
				let player_idx = 1 + (round as usize) * (max_group_size as usize - 1) + k;
				let coplayer = AccountOrPerson::Account(players[player_idx].clone());
				IndexToPlayer::<T>::insert((round, slot), &coplayer);
			}
		}

		// Realise the scenario: saturating `yes_person` blocks `NotAttended`;
		// `sent_report = true` flips the chosen co-players to the Attended (full)
		// path, the others bail cheaply as Pending.
		for round in 0..rounds as u8 {
			for (k, _) in group0_other_slots.iter().enumerate() {
				let player_idx = 1 + (round as usize) * (max_group_size as usize - 1) + k;
				let account_or_person = AccountOrPerson::Account(players[player_idx].clone());
				let goes_full = (player_idx as u32) <= coplayers_enacted;
				Players::<T>::mutate(&account_or_person, |player_info| {
					if let Some(player_info) = player_info {
						player_info.yes_person = u8::MAX;
						if goes_full {
							player_info.sent_report = true;
						}
					}
				});
			}
		}

		// `Person` awards one credit per attestee on the spot, marking its slot;
		// `NotPerson` awards nothing in `report` (it is backfilled later only if the
		// attestee attends), so an all-`Person` report is the heavier path to benchmark.
		let round_report: BoundedVec<Report, T::MaxGroupSize> = (0..(T::MaxGroupSize::get() - 1))
			.map(|_| Report::Person)
			.collect::<Vec<_>>()
			.try_into()
			.unwrap();

		let mut full_report: FullReport<T> = BoundedVec::new();
		for _ in 0..rounds {
			assert!(full_report.try_push(round_report.clone()).is_ok());
		}

		#[extrinsic_call]
		_(RawOrigin::Signed(caller.clone()), full_report);

		let actual_enacted = Players::<T>::iter()
			.filter(|(_, p)| p.early_attendance_enactment.is_some())
			.count() as u32;
		assert_eq!(
			actual_enacted, n,
			"benchmark must produce exactly `n` full early-attendance enactments",
		);

		Ok(())
	}

	#[benchmark]
	fn offboard_account() -> Result<(), BenchmarkError> {
		// A game does not exist
		assert!(Game::<T>::get().is_none());

		// The caller is stored as a player
		let caller: T::AccountId = whitelisted_caller();
		let caller_aop = AccountOrPerson::Account(caller.clone());

		// To make sure the caller has enough funds to pay for the call
		<T as Config>::BenchmarkHelper::fund_account(caller.clone());

		// Mirror what sign_up_with_account would have produced,
		// otherwise frame_system sufficients underflow).
		indiv_pallet_score::Pallet::<T>::onboard_for_recognition(&caller)?;
		sp_statement_store::increase_allowance_by(
			caller.clone().into(),
			T::PlayerStatementLimit::get(),
		);

		// Worst case: the caller is a recognized participant, so the offboard also suspends
		// their personhood.
		let id = make_recognized_participant::<T>(&caller_aop)?;

		let deposit = T::PlayDeposit::new(&caller, pallet::PlayDepositAmount::<T>::get())?;
		let player: Player<<T as Config>::PlayDeposit> = Player {
			first_game: 0,
			registered: false,
			sent_report: false,
			early_attendance_enactment: None,
			yes_person: 0,
			no_not_person: 0,
			expected_max_vote_weight: 0,
			vote_weight: 0,
			credibility: PlayerCredibility::Deposit(deposit),
		};

		Players::<T>::insert(&caller_aop, player);

		let mut attendance = PlayerAttendanceHistory::<T>::get(&caller_aop);
		for i in 0..T::MaxAttendanceHistoryDepth::get() {
			let _ = attendance.try_push(i);
		}
		PlayerAttendanceHistory::<T>::insert(&caller_aop, attendance);

		#[extrinsic_call]
		offboard(RawOrigin::Signed(caller.clone()));

		// The caller is removed from the list of players
		assert!(!Players::<T>::contains_key(&caller_aop), "Player should be removed");

		// And the caller is not stored in the list of archived players
		assert!(!ArchivedPlayers::<T>::contains_key(&caller_aop), "Player should not be archived");

		// Resuming succeeds only for a suspended person, which proves the offboard suspended
		// them.
		assert_ok!(PeopleOf::<T>::recognize_personhood(id, None));

		Ok(())
	}

	#[benchmark]
	fn offboard_person() -> Result<(), BenchmarkError> {
		// A game does not exist.
		assert!(Game::<T>::get().is_none());

		let seed = 1u64; // to match the alias in try_successful_origin
		let person: Alias = [seed as u8; 32];
		let person_aop = AccountOrPerson::Person(person);
		let stmt_account = <T as Config>::BenchmarkHelper::create_account(seed);

		// Mirror the state that sign_up_with_alias would have produced,
		// otherwise sufficients underflows.
		indiv_pallet_score::Pallet::<T>::onboard_externally_recognized(&person)?;
		AliasToStmtAccount::<T>::insert(person, &stmt_account);
		StmtAccountToAlias::<T>::insert(&stmt_account, person);
		sp_statement_store::increase_allowance_by(
			stmt_account.clone().into(),
			T::PlayerStatementLimit::get(),
		);

		let player: Player<<T as Config>::PlayDeposit> = Player {
			first_game: 0,
			registered: false,
			sent_report: false,
			early_attendance_enactment: Some(EarlyAttendanceEnactment {
				attendance: true,
				disposition: PlayerDisposition::ArchiveUnkickable,
			}),
			expected_max_vote_weight: 2u16,
			vote_weight: 5u8,
			yes_person: 0,
			no_not_person: 0,
			credibility: PlayerCredibility::Recognized,
		};
		Players::<T>::insert(&person_aop, player);

		ArchivedPlayers::<T>::insert(
			&person_aop,
			ArchivedPlayer::Kickable { first_game: 0, archived_since: 0u32.into() },
		);

		Game::<T>::put(GameInfo {
			index: 0,
			registration_ends: 0,
			shuffle_deadline: 1,
			game_date: 0,
			report_ends: 0,
			state: GameState::Registration { next_player_index: 0 },
			max_group_size: T::MaxGroupSize::get(),
			rounds: T::MaxRounds::get() as u8,
			pending_attendance: 0,
			airdrops_scheduled: 0,
		});

		// Seed attendance history at max depth so the removal proof is worst case.
		let mut attendance = PlayerAttendanceHistory::<T>::get(&person_aop);
		for i in 0..T::MaxAttendanceHistoryDepth::get() {
			let _ = attendance.try_push(i);
		}
		PlayerAttendanceHistory::<T>::insert(&person_aop, attendance);

		let score_context = indiv_pallet_score::Pallet::<T>::score_context();
		let origin = T::EnsurePerson::try_successful_origin(&score_context)
			.map_err(|_| BenchmarkError::Weightless)?;

		#[extrinsic_call]
		offboard(origin as T::RuntimeOrigin);

		assert!(!Players::<T>::contains_key(&person_aop), "Person should be removed");
		assert!(
			!AliasToStmtAccount::<T>::contains_key(person),
			"AliasToStmtAccount should be removed"
		);
		assert!(
			!StmtAccountToAlias::<T>::contains_key(&stmt_account),
			"StmtAccountToAlias should be removed"
		);

		Ok(())
	}

	#[benchmark]
	fn kickout() -> Result<(), BenchmarkError> {
		// The caller is simply a signed origin
		let caller: T::AccountId = whitelisted_caller();

		// The player to kickout is an existing, archived, kickable player
		let player_to_kickout: T::AccountId = account("kickout", 0, 0);
		let player_aop = AccountOrPerson::Account(player_to_kickout.clone());

		indiv_pallet_score::Pallet::<T>::onboard_for_recognition(&player_to_kickout)?;

		// Worst case: the player is a recognized participant, so the kickout also suspends
		// their personhood.
		let id = make_recognized_participant::<T>(&player_aop)?;

		ArchivedPlayers::<T>::insert(
			&player_aop,
			ArchivedPlayer::Kickable { archived_since: 0u32.into(), first_game: 0 },
		);

		let mut attendance = PlayerAttendanceHistory::<T>::get(&player_aop);
		for i in 0..T::MaxAttendanceHistoryDepth::get() {
			let _ = attendance.try_push(i);
		}
		PlayerAttendanceHistory::<T>::insert(&player_aop, attendance);

		// The time to kickout a player is respected
		frame_system::Pallet::<T>::set_block_number(
			frame_system::Pallet::<T>::block_number() +
				T::NonPlayingKickoutTime::get() +
				One::one(),
		);

		#[extrinsic_call]
		_(RawOrigin::Signed(caller), player_to_kickout.clone());

		// The player is offboarded in pallet score
		assert!(
			!indiv_pallet_score::Participants::<T>::contains_key(&player_aop),
			"Player should be offboarded from indiv_pallet_score"
		);

		// Resuming succeeds only for a suspended person, which proves the kickout suspended
		// them.
		assert_ok!(PeopleOf::<T>::recognize_personhood(id, None));

		Ok(())
	}

	#[benchmark]
	fn grant_invites() -> Result<(), BenchmarkError> {
		// Account to receive invites
		let receiver: T::AccountId = account("receiver", 0, 0);

		// Number of invites to give
		let count: u32 = 5;

		// The caller is of InviteIssuer origin, so Root should work in all cases
		#[extrinsic_call]
		_(RawOrigin::Root, receiver.clone(), count);

		// The receiver of invites successfully has them assigned to him
		let available_invites = AvailableInvites::<T>::get(&receiver);
		assert_eq!(available_invites, count, "Receiver should have the given number of invites");

		Ok(())
	}

	#[benchmark]
	fn remove_available_and_pending_invites(
		n: Linear<1, REMOVE_INVITES_SAMPLE_CAP>,
	) -> Result<(), BenchmarkError> {
		// The concerned account has a few available and pending invites
		let account_with_invites: T::AccountId = account("acc", 0, 0);
		AvailableInvites::<T>::insert(&account_with_invites, 10);
		for i in 0..n {
			let ticket = <T as Config>::BenchmarkHelper::create_ticket(i as u64);
			PendingInvites::<T>::insert(&account_with_invites, ticket, ());
		}

		// The caller is of InviteIssuer origin, so Root should work in all cases
		#[extrinsic_call]
		_(RawOrigin::Root, account_with_invites.clone(), n);

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

		let ticket = <T as Config>::BenchmarkHelper::create_ticket(1);

		#[extrinsic_call]
		_(RawOrigin::Signed(caller.clone()), ticket.clone());

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
		let ticket = <T as Config>::BenchmarkHelper::create_ticket(1);
		PendingInvites::<T>::insert(caller.clone(), ticket.clone(), ());

		#[extrinsic_call]
		_(RawOrigin::Signed(caller.clone()), ticket.clone());

		// The ticket is removed from pending tickets of the caller
		assert!(PendingInvites::<T>::get(&caller, &ticket).is_none());

		// The number of available tickets of the caller is increased
		assert_eq!(AvailableInvites::<T>::get(&caller), 1);

		Ok(())
	}

	#[benchmark]
	fn schedule_games(n: Linear<1, { T::MaxGameSchedules::get() }>) -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		// No scheduled games
		assert_eq!(GameSchedules::<T>::get().len(), 0);

		// One ongoing game
		let game_schedule = GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds: T::MaxRounds::get() as u8,
			max_group_size: T::MaxGroupSize::get(),
			airdrops: bench_airdrops::<T>(1),
		};
		assert_ok!(pallet::Pallet::<T>::new_game(&game_schedule));

		// n games to schedule
		let mut games_schedules = Vec::new();
		let offset = 1000u32;
		let mut prev_game_end = 2000u32;

		for _ in 0..n {
			let schedule = GameScheduleOf::<T> {
				game_play_time: prev_game_end + offset,
				rounds: T::MaxRounds::get() as u8,
				max_group_size: T::MaxGroupSize::get(),
				airdrops: bench_airdrops::<T>(MAX_GAME_AIRDROPS.into()),
			};
			prev_game_end = GameTimes::<T>::player_process_end(&schedule);

			games_schedules.push(schedule);
		}

		#[extrinsic_call]
		_(RawOrigin::Root, games_schedules);

		// All the n games were successfully scheduled
		assert_eq!(GameSchedules::<T>::get().len(), n as usize);

		Ok(())
	}

	#[benchmark]
	fn remove_scheduled_game() -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		// The maximum number of games is scheduled
		let max_schedules = T::MaxGameSchedules::get();
		let mut games_schedules = Vec::new();
		let offset = 1000u32;
		let mut prev_game_end = 2000u32;

		for _ in 0..max_schedules {
			let schedule = GameScheduleOf::<T> {
				game_play_time: prev_game_end + offset,
				rounds: T::MaxRounds::get() as u8,
				max_group_size: T::MaxGroupSize::get(),
				airdrops: bench_airdrops::<T>(MAX_GAME_AIRDROPS.into()),
			};
			prev_game_end = GameTimes::<T>::player_process_end(&schedule);

			games_schedules.push(schedule);
		}

		let first_game_time = games_schedules[0].game_play_time;
		GameSchedules::<T>::put(BoundedVec::try_from(games_schedules).unwrap());

		#[extrinsic_call]
		_(RawOrigin::Root, first_game_time);

		// The first schedule is gone.
		let remaining = GameSchedules::<T>::get();
		assert_eq!(remaining.len(), (max_schedules - 1) as usize);
		assert!(
			remaining.iter().all(|s| s.game_play_time != first_game_time),
			"the first schedule should have been removed"
		);

		Ok(())
	}

	#[benchmark]
	fn set_play_deposit() -> Result<(), BenchmarkError> {
		let amount: NativeBalanceOf<T> = One::one();

		#[extrinsic_call]
		_(RawOrigin::Root, amount);

		assert_eq!(PlayDepositAmount::<T>::get(), amount);

		Ok(())
	}

	// `n` is the number of scheduled airdrop events the validated registration covers.
	#[benchmark]
	fn as_invited_tx_ext(n: Linear<0, { MAX_GAME_AIRDROPS as u32 }>) -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		// A game exists and it's in registration state
		let schedule = GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds: T::MaxRounds::get() as u8,
			max_group_size: T::MaxGroupSize::get(),
			airdrops: bench_airdrops::<T>(n),
		};
		assert_ok!(pallet::Pallet::<T>::new_game(&schedule));
		let game_index = Game::<T>::get().expect("game exists").index;

		// A spare invitation ticket exists
		let inviter: T::AccountId = account("inviter", 0, 0);
		let ticket = <T as Config>::BenchmarkHelper::create_ticket(0);
		PendingInvites::<T>::insert(&inviter, &ticket, ());

		// The caller is new, it has an sr25519 keypair.
		let (caller, vrfs) = bench_account_vrfs::<T>(game_index, n);
		let origin = RawOrigin::Signed(caller.clone());

		// The call is `sign_up_with_invite` with a valid airdrop VRF.
		let call: <T as frame_system::Config>::RuntimeCall = Call::sign_up_with_invite {
			identifier_key: DEFAULT_IDENTIFIER_KEY,
			airdrops: Some(vrfs),
		}
		.into();
		let len = call.encode().len();

		let msg = caller.encode();
		let signature = <T as Config>::BenchmarkHelper::sign_ticket(0, &msg[..]);

		let tx_ext = crate::GameAsInvited::<T>::new(Some(crate::GameAsInvitedData {
			nonce: 0u32.into(),
			ticket,
			inviter,
			signature,
		}));

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call, &Default::default(), len, 0, |new_origin| {
					// The extension should have transformed the Signed origin into our
					// custom Origin::Invited.
					assert!(matches!(
						new_origin.into_caller().try_into(),
						Ok(crate::Origin::<T>::Invited(_))
					));
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	#[benchmark]
	fn process_reporting() -> Result<(), BenchmarkError> {
		// Ensure we have a valid (non-genesis) time API and move time past report_ends.
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();
		<T as Config>::BenchmarkHelper::set_time(core::time::Duration::from_secs(1));

		// Put a game in the Reporting phase whose reporting window has already ended so
		// `process_reporting` transitions the state to `PlayerProcess`.
		Game::<T>::put(GameInfo {
			index: 0,
			registration_ends: 0,
			shuffle_deadline: 0,
			game_date: 0,
			report_ends: 0, // now (1s) >= report_ends (0s) so reporting can close
			state: GameState::Reporting { player_count: 0 },
			max_group_size: T::MaxGroupSize::get(),
			rounds: T::MaxRounds::get() as u8,
			pending_attendance: 0,
			airdrops_scheduled: 0,
		});

		let mut meter = WeightMeter::new();

		#[block]
		{
			pallet::Pallet::<T>::process_reporting(&mut meter);
		}

		// The game should have transitioned to PlayerProcess.
		let game = Game::<T>::get().expect("game should exist after process_reporting");
		assert!(matches!(game.state, GameState::PlayerProcess { .. }));

		Ok(())
	}

	#[benchmark]
	fn insert_attendance_history() -> Result<(), BenchmarkError> {
		let game_index = 99;
		let account: T::AccountId = account("player", 0, 0);
		let player = AccountOrPerson::Account(account);

		// Pre-fill the history to MaxAttendanceHistoryDepth so `try_push`
		// inside `fn note_attendance` fails
		let mut attendance = PlayerAttendanceHistory::<T>::get(&player);
		for i in 0..T::MaxAttendanceHistoryDepth::get() {
			let _ = attendance.try_push(i);
		}
		PlayerAttendanceHistory::<T>::insert(&player, attendance);
		assert_eq!(
			PlayerAttendanceHistory::<T>::get(&player).len() as u32,
			T::MaxAttendanceHistoryDepth::get()
		);

		#[block]
		{
			pallet::Pallet::<T>::note_attendance(game_index, &player);
		}

		// History stays at max depth; oldest entry rotated out, new one appended.
		let history = PlayerAttendanceHistory::<T>::get(&player);
		assert_eq!(history.len() as u32, T::MaxAttendanceHistoryDepth::get());
		assert_eq!(history.last(), Some(&game_index));

		Ok(())
	}

	#[benchmark]
	fn cancel_game() -> Result<(), BenchmarkError> {
		// A game exists in Registration phase — the only state this call accepts.
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();
		let schedule = GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds: T::MaxRounds::get() as u8,
			max_group_size: T::MaxGroupSize::get(),
			airdrops: bench_airdrops::<T>(MAX_GAME_AIRDROPS.into()),
		};
		assert_ok!(pallet::Pallet::<T>::new_game(&schedule));
		assert!(matches!(
			Game::<T>::get().expect("game exists").state,
			GameState::Registration { .. }
		));
		assert_eq!(Game::<T>::get().expect("game exists").airdrops_scheduled, MAX_GAME_AIRDROPS);

		// Worst case: the airdrop events have already opened for registration
		let game_index = Game::<T>::get().expect("game exists").index;
		bench_open_airdrop_registration::<T>(game_index, MAX_GAME_AIRDROPS.into());

		// The caller is of ManagerOrigin, so Root should work in all cases.
		#[extrinsic_call]
		_(RawOrigin::Root);

		// The game has been transitioned to `Cancelling`; the per-player
		// cleanup is driven by `process_cancelling` on later blocks and is
		// not part of this bench.
		assert!(matches!(
			Game::<T>::get().expect("game still in storage").state,
			GameState::Cancelling { step: CancellingStep::Step1DrainShuffle }
		));

		Ok(())
	}

	#[benchmark]
	fn set_game_phases() -> Result<(), BenchmarkError> {
		// Worst case: a game exists and the extrinsic must read it to verify it is
		// still in its Registration phase. `new_game` leaves the game in
		// `GameState::Registration` by construction, which is the allowed branch.
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();
		let schedule = GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds: T::MaxRounds::get() as u8,
			max_group_size: T::MaxGroupSize::get(),
			airdrops: bench_airdrops::<T>(1),
		};
		assert_ok!(pallet::Pallet::<T>::new_game(&schedule));
		assert!(matches!(
			Game::<T>::get().expect("game exists").state,
			GameState::Registration { .. },
		));

		let phases = PhaseDurationValues {
			registration: 1,
			shuffle: 1,
			post_shuffle_margin: 1,
			reporting: 1,
			player_process: 1,
		};

		#[extrinsic_call]
		_(RawOrigin::Root, phases.clone());

		assert_eq!(StoredPhaseDurations::<T>::get(), Some(phases));
		Ok(())
	}

	// `n` is the number of scheduled airdrop events to cancel.
	#[benchmark]
	fn on_game_cancelled(n: Linear<0, { MAX_GAME_AIRDROPS as u32 }>) -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		let schedule = GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds: T::MaxRounds::get() as u8,
			max_group_size: T::MaxGroupSize::get(),
			airdrops: bench_airdrops::<T>(n),
		};
		assert_ok!(pallet::Pallet::<T>::new_game(&schedule));
		let game = Game::<T>::get().expect("game exists after new_game");

		#[block]
		{
			pallet::Pallet::<T>::on_game_cancelled(&game);
		}

		// Cancellation routes each airdrop into the airdrop pallet's clean-up pipeline (or drops
		// it outright if still `Scheduled`); the game pallet no longer keeps its own record.
		use indiv_pallet_airdrop::types::Status;
		for airdrop_index in 0..n {
			let event_id = pallet::Pallet::<T>::airdrop_event_id(game.index, airdrop_index as u8);
			let still_present = indiv_pallet_airdrop::Events::<T>::get(event_id);
			assert!(
				still_present.is_none() ||
					matches!(
						still_present.expect("checked").status,
						Status::ClearingRegistrations { .. } |
							Status::ClearingWinners { .. } |
							Status::Finalizing { .. },
					),
			);
		}

		Ok(())
	}

	#[benchmark]
	fn claim_airdrop() -> Result<(), BenchmarkError> {
		<T as Config>::BenchmarkHelper::set_valid_time();
		bench_setup_airdrop_funds::<T>();

		// Stand up a game so the airdrop event for `game.index` is scheduled in airdrop.
		let schedule = GameScheduleOf::<T> {
			game_play_time: 1000,
			rounds: T::MaxRounds::get() as u8,
			max_group_size: T::MaxGroupSize::get(),
			airdrops: bench_airdrops::<T>(1),
		};
		assert_ok!(pallet::Pallet::<T>::new_game(&schedule));
		let game = Game::<T>::get().expect("game exists after new_game");

		// Transition the airdrop event to `Claiming` so `do_claim` accepts the call, and
		// place time inside the claim window.
		let event_id = pallet::Pallet::<T>::airdrop_event_id(game.index, 0);
		let mut event = indiv_pallet_airdrop::Events::<T>::get(event_id)
			.expect("airdrop event scheduled by new_game");
		event.status = indiv_pallet_airdrop::types::Status::Claiming {
			total_participants: 1,
			effective_winners: 1,
			claimed: 0,
		};
		let end_time = event.info.end_time;
		indiv_pallet_airdrop::Events::<T>::insert(event_id, event);
		<T as Config>::BenchmarkHelper::set_time(core::time::Duration::from_secs(
			end_time.saturating_sub(1),
		));

		// Claimant: a signed account, made recognized in pallet-score so the eligibility
		// check in `claim_airdrop` passes.
		let claimant: T::AccountId = whitelisted_caller();
		let key = AccountOrPerson::Account(claimant.clone());
		indiv_pallet_score::Participants::<T>::insert(
			&key,
			indiv_pallet_score::Participant {
				score: 0,
				streak: Default::default(),
				attendance_history: Default::default(),
				credit: 0u32.into(),
				cashed_out: false,
				reached_personhood: true,
				has_ever_reached_personhood: true,
				recognition: indiv_pallet_score::Recognition::ExternallyRecognized,
				last_attended_game: Some(game.index),
			},
		);

		// Register the claimant as the sole winner.
		indiv_pallet_airdrop::Winners::<T>::insert(
			event_id,
			indiv_pallet_airdrop::types::RegistrationEntry::Account {
				account_id: claimant.clone(),
			},
			indiv_pallet_airdrop::BigEndianU256::from([0u8; 32]),
		);

		let beneficiary: T::AccountId = account("beneficiary", 0, 0);

		#[extrinsic_call]
		_(RawOrigin::Signed(claimant.clone()), game.index, 0, beneficiary);

		assert!(!indiv_pallet_airdrop::Winners::<T>::contains_key(
			event_id,
			indiv_pallet_airdrop::types::RegistrationEntry::Account { account_id: claimant },
		));

		Ok(())
	}

	// Implements a test for each benchmark. Execute with:
	// `cargo test -p pallet-people --features runtime-benchmarks`.
	impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
