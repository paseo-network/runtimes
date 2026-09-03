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

//! Benchmarks for the people-airdrops pallet.

use super::*;
use crate::Pallet as PeopleAirdrops;
use alloc::vec::Vec;
use frame_benchmarking::{account, v2::*, BenchmarkError};
use frame_support::traits::EnsureOriginWithArg;

/// Benchmark setup hooks.
///
/// The pallet reaches its airdrop implementation only through the [`Airdrop`] trait, which
/// cannot place a draw into an arbitrary lifecycle phase or inspect its state. The runtime
/// implements these hooks against its concrete airdrop pallet.
pub trait BenchmarkHelper<T: Config> {
	/// Fund `source` to cover `draws` draws and return the event info to schedule for each of
	/// them. Every info must use a distinct, enabled prize asset, so a batch schedule touches
	/// distinct asset storage per draw and the benchmark charges the worst case instead of
	/// hitting keys cached by the first draw.
	fn fund_prize_source(source: &T::AccountId, draws: u32) -> Vec<AirdropEventInfoOf<T>>;
	/// Open registration for a scheduled draw.
	fn open_registration(event_id: &EventId);
	/// Turn every registered participant of an open draw into a winner and start its claiming
	/// phase.
	fn start_claiming(event_id: &EventId);
	/// Count the draw's registrations.
	fn count_registrations(event_id: &EventId) -> u32;
	/// Count the draw's unclaimed winners.
	fn count_winners(event_id: &EventId) -> u32;
	/// Set the unix time reported by [`Config::UnixTime`].
	fn set_unix_time(now_secs: u64);
}

impl<T: Config> BenchmarkHelper<T> for () {
	fn fund_prize_source(_source: &T::AccountId, _draws: u32) -> Vec<AirdropEventInfoOf<T>> {
		unimplemented!()
	}
	fn open_registration(_event_id: &EventId) {
		unimplemented!()
	}
	fn start_claiming(_event_id: &EventId) {
		unimplemented!()
	}
	fn count_registrations(_event_id: &EventId) -> u32 {
		unimplemented!()
	}
	fn count_winners(_event_id: &EventId) -> u32 {
		unimplemented!()
	}
	fn set_unix_time(_now_secs: u64) {
		unimplemented!()
	}
}

fn assert_last_event<T: Config>(generic_event: <T as frame_system::Config>::RuntimeEvent) {
	frame_system::Pallet::<T>::assert_last_event(generic_event);
}

fn manager_origin<T: Config>() -> Result<T::RuntimeOrigin, BenchmarkError> {
	T::ManagerOrigin::try_successful_origin().map_err(|_| BenchmarkError::Stop("manager origin"))
}

fn person_origin<T: Config>() -> Result<T::RuntimeOrigin, BenchmarkError> {
	T::EnsurePerson::try_successful_origin(&PeopleAirdrops::<T>::people_airdrops_context())
		.map_err(|_| BenchmarkError::Stop("person origin"))
}

/// Schedule `n` draws through the extrinsic and return their event ids.
///
/// One call per draw, so `n` may exceed `MaxScheduleBatch` (`register` needs up to
/// `MaxRegisterBatch` draws and the two bounds are independent).
fn schedule_n_draws<T: Config>(n: u32) -> Result<Vec<EventId>, BenchmarkError> {
	// The runtime randomness source is empty at genesis; scheduling requires a value.
	T::Randomness::set_randomness([42u8; 32], 1);
	let source = T::PrizeSource::get();
	let infos = T::BenchmarkHelper::fund_prize_source(&source, n);
	if infos.len() != n as usize {
		return Err(BenchmarkError::Stop("helper must return one info per draw"));
	}
	let first = NextDrawIndex::<T>::get();
	for info in infos {
		let mut draws: BoundedVec<_, T::MaxScheduleBatch> = BoundedVec::new();
		draws
			.try_push(info)
			.map_err(|_| BenchmarkError::Stop("MaxScheduleBatch is zero"))?;
		PeopleAirdrops::<T>::schedule_draws(manager_origin::<T>()?, draws)
			.map_err(|_| BenchmarkError::Stop("schedule draws"))?;
	}
	Ok((first..first.saturating_add(n as u64))
		.map(PeopleAirdrops::<T>::draw_event_id)
		.collect())
}

/// Schedule `n` draws and open their registration.
fn schedule_and_open_n_draws<T: Config>(n: u32) -> Result<Vec<EventId>, BenchmarkError> {
	let event_ids = schedule_n_draws::<T>(n)?;
	for event_id in &event_ids {
		T::BenchmarkHelper::open_registration(event_id);
	}
	Ok(event_ids)
}

fn bounded_ids<T: Config>(
	event_ids: &[EventId],
) -> Result<BoundedVec<EventId, T::MaxRegisterBatch>, BenchmarkError> {
	event_ids
		.to_vec()
		.try_into()
		.map_err(|_| BenchmarkError::Stop("ids exceed MaxRegisterBatch"))
}

#[benchmarks]
mod benches {
	use super::*;

	#[benchmark]
	fn schedule_draws(n: Linear<1, { T::MaxScheduleBatch::get() }>) -> Result<(), BenchmarkError> {
		// The runtime randomness source is empty at genesis; scheduling requires a value.
		T::Randomness::set_randomness([42u8; 32], 1);
		let source = T::PrizeSource::get();
		// One distinct prize asset per draw (see the `BenchmarkHelper` trait doc).
		let infos = T::BenchmarkHelper::fund_prize_source(&source, n);
		let draws: BoundedVec<_, T::MaxScheduleBatch> = infos
			.try_into()
			.map_err(|_| BenchmarkError::Stop("n exceeds MaxScheduleBatch"))?;
		let origin = manager_origin::<T>()?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, draws);

		assert_eq!(NextDrawIndex::<T>::get(), n as u64);
		Ok(())
	}

	#[benchmark]
	fn remove_scheduled_draw() -> Result<(), BenchmarkError> {
		let event_id = schedule_n_draws::<T>(1)?[0];
		let origin = manager_origin::<T>()?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, event_id);

		assert_last_event::<T>(Event::<T>::DrawRemoved { event_id }.into());
		Ok(())
	}

	#[benchmark]
	fn cancel_draw() -> Result<(), BenchmarkError> {
		// Worst case: the draw is open with a registered participant, so the cancel releases the
		// full prize allocation and transitions into the clean-up pipeline.
		let event_id = schedule_and_open_n_draws::<T>(1)?[0];
		PeopleAirdrops::<T>::register(person_origin::<T>()?, bounded_ids::<T>(&[event_id])?)
			.map_err(|_| BenchmarkError::Stop("register"))?;
		let origin = manager_origin::<T>()?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, event_id);

		assert_last_event::<T>(Event::<T>::DrawCancelled { event_id }.into());
		Ok(())
	}

	#[benchmark]
	fn register(n: Linear<1, { T::MaxRegisterBatch::get() }>) -> Result<(), BenchmarkError> {
		let event_ids = schedule_and_open_n_draws::<T>(n)?;
		let ids = bounded_ids::<T>(&event_ids)?;
		let origin = person_origin::<T>()?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, ids);

		for event_id in &event_ids {
			assert_eq!(T::BenchmarkHelper::count_registrations(event_id), 1);
		}
		Ok(())
	}

	#[benchmark]
	fn claim() -> Result<(), BenchmarkError> {
		let event_id = schedule_and_open_n_draws::<T>(1)?[0];
		PeopleAirdrops::<T>::register(person_origin::<T>()?, bounded_ids::<T>(&[event_id])?)
			.map_err(|_| BenchmarkError::Stop("register"))?;
		T::BenchmarkHelper::start_claiming(&event_id);
		let destination: T::AccountId = account("destination", 0, 0);
		let origin = person_origin::<T>()?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, event_id, destination);

		assert_eq!(T::BenchmarkHelper::count_winners(&event_id), 0);
		Ok(())
	}

	#[benchmark]
	fn clean_up_draw_salt() -> Result<(), BenchmarkError> {
		let event_id = schedule_n_draws::<T>(1)?[0];
		let (_salt, end_time) =
			DrawSalts::<T>::get(event_id).ok_or(BenchmarkError::Stop("salt stored"))?;
		T::BenchmarkHelper::set_unix_time(end_time.saturating_add(1));

		#[extrinsic_call]
		_(frame_system::RawOrigin::Authorized, event_id);

		assert!(DrawSalts::<T>::get(event_id).is_none());
		Ok(())
	}

	#[benchmark]
	fn authorize_clean_up_draw_salt() -> Result<(), BenchmarkError> {
		use sp_runtime::transaction_validity::TransactionSource;
		let event_id = schedule_n_draws::<T>(1)?[0];
		let (_salt, end_time) =
			DrawSalts::<T>::get(event_id).ok_or(BenchmarkError::Stop("salt stored"))?;
		T::BenchmarkHelper::set_unix_time(end_time.saturating_add(1));

		#[block]
		{
			PeopleAirdrops::<T>::authorize_clean_up_draw_salt(TransactionSource::Local, &event_id)
				.expect("authorize succeeds");
		}
		Ok(())
	}

	impl_benchmark_test_suite!(PeopleAirdrops, crate::mock::new_test_ext(), crate::mock::Test);
}
