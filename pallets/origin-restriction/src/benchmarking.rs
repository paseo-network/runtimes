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

//! Benchmarks for pallet origin restriction.

use super::*;
use frame_benchmarking::{v2::*, BenchmarkError};
use frame_support::dispatch::DispatchClass;
use sp_runtime::traits::{BlockNumberProvider, DispatchTransaction};

fn assert_last_event<T: Config>(generic_event: <T as frame_system::Config>::RuntimeEvent) {
	frame_system::Pallet::<T>::assert_last_event(generic_event.into());
}

#[benchmarks]
mod benches {
	use super::*;

	#[benchmark]
	fn clean_usage() -> Result<(), BenchmarkError> {
		let (origin, _) = T::BenchmarkHelper::excess_pair();
		let entity = T::RestrictedEntity::restricted_entity(&origin)
			.expect("The origin from `excess_pair` must be restricted");

		Usages::<T>::insert(&entity, Usage { used: 1u32.into(), at_block: 0u32.into() });

		T::BlockNumberProvider::set_block_number(1_000u32.into());

		#[extrinsic_call]
		_(frame_system::RawOrigin::Root, entity.clone());

		assert_last_event::<T>(Event::UsageCleaned { entity }.into());

		Ok(())
	}

	#[benchmark]
	fn restrict_origin_tx_ext() -> Result<(), BenchmarkError> {
		let tx_ext = RestrictOrigin::<T>::new(true);
		let (origin, call) = T::BenchmarkHelper::excess_pair();

		let entity = T::RestrictedEntity::restricted_entity(&origin)
			.expect("The origin from `excess_pair` must be restricted");

		// Set the block number, so that the extension measures the same read as in production.
		// A provider that keeps the value in storage writes it only when it is set.
		T::BlockNumberProvider::set_block_number(1_000u32.into());

		let now = T::BlockNumberProvider::current_block_number();
		Usages::<T>::insert(&entity, Usage { used: 0u32.into(), at_block: now });

		let info = DispatchInfo {
			call_weight: Weight::MAX,
			extension_weight: Weight::zero(),
			class: DispatchClass::Normal,
			pays_fee: Pays::Yes,
		};

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call, &info, 0, 0, |_| Ok(Default::default()))
				.expect("Failed to allow the excess call, benchmark needs to be improved")
				.expect("inner call successful");
		}

		Ok(())
	}

	impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
