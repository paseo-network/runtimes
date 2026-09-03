// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Reference weights for `pallet_parameters`.
//!
//! ⚠️ NOT BENCHMARKED ON THIS RUNTIME. These are the crate's own published reference weights
//! (`pallet_parameters::weights::SubstrateWeight`, which the crate does not re-export publicly),
//! restated here so the runtime binds a named weights module rather than `()` and so the
//! benchmark regeneration pass has a file to overwrite.
//!
//! The `Parameters::Parameters` proof size below is the crate's generic worst case
//! (`max_size: 11322`), not this runtime's `RuntimeParameters` encoding, so it over-estimates
//! rather than under-estimates. Replace with a generated file from
//! `frame-omni-bencher ... --pallet pallet_parameters` before enactment.

use core::marker::PhantomData;
use frame_support::{traits::Get, weights::Weight};

/// Weight functions for `pallet_parameters`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_parameters::WeightInfo for WeightInfo<T> {
	/// Storage: `Parameters::Parameters` (r:1 w:1)
	/// Proof: `Parameters::Parameters` (`max_values`: None, `max_size`: Some(11322), added: 13797,
	/// mode: `MaxEncodedLen`)
	fn set_parameter() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0`
		//  Estimated: `14787`
		// Minimum execution time: 5_884_000 picoseconds.
		Weight::from_parts(6_204_000, 14787)
			.saturating_add(T::DbWeight::get().reads(1_u64))
			.saturating_add(T::DbWeight::get().writes(1_u64))
	}
}
