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

//! Relay randomness pallet benchmarking.

use super::*;
use crate::testing_utils::relay_state_proof;
use codec::Encode;
use cumulus_pallet_parachain_system::OnSystemEvent;
use cumulus_primitives_core::relay_chain::well_known_keys;
use frame_benchmarking::v2::*;
use sp_runtime::traits::BlockNumberProvider;

#[benchmarks]
mod benches {
	use super::*;

	/// Worst case: both stored values are present and both change, so the hook reads the
	/// relay parent number from `ParachainSystem::ValidationData` and rewrites every
	/// entry.
	#[benchmark]
	fn on_relay_state_proof() {
		Randomness::<T>::put(RandomnessValues {
			block: Some(RandomnessEntry { randomness: [9u8; 32], moment: 41 }),
			one_epoch_ago: Some(RandomnessEntry { randomness: [8u8; 32], moment: 30 }),
		});
		RelaychainDataProvider::<T>::set_block_number(100);
		let moment = 100u32.saturating_sub(
			<T as cumulus_pallet_parachain_system::Config>::RelayParentOffset::get(),
		);
		let proof = relay_state_proof(&[
			(well_known_keys::CURRENT_BLOCK_RANDOMNESS, Some([1u8; 32]).encode()),
			(well_known_keys::ONE_EPOCH_AGO_RANDOMNESS, [2u8; 32].encode()),
		]);

		#[block]
		{
			<Pallet<T> as OnSystemEvent>::on_relay_state_proof(&proof);
		}

		assert_eq!(
			Randomness::<T>::get(),
			RandomnessValues {
				block: Some(RandomnessEntry { randomness: [1u8; 32], moment }),
				one_epoch_ago: Some(RandomnessEntry {
					randomness: [2u8; 32],
					moment: moment.saturating_sub(1)
				})
			}
		);
	}

	impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
