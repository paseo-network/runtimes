// Copyright (C) Parity Technologies and the various Polkadot contributors, see Contributions.md
// for a list of specific contributors.
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

//! Autogenerated weights for `runtime_parachains :: configuration`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 47.2.0
//! DATE: 2025-06-16, STEPS: `50`, REPEAT: `20`, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! WORST CASE MAP SIZE: `1000000`
//! HOSTNAME: `ggwpez-ref-hw`, CPU: `AMD EPYC 7232P 8-Core Processor`
//! WASM-EXECUTION: `Compiled`, CHAIN: `None`, DB CACHE: 1024

// Executed Command:
// frame-omni-bencher
// v1
// benchmark
// pallet
// --runtime=target/production/wbuild/paseo-runtime/paseo_runtime.compact.compressed.wasm
// --header=.github/scripts/cmd/file_header.txt
// --output=./relay/paseo/src/weights/
// --all
// --quiet

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]
#![allow(missing_docs)]

use frame_support::{traits::Get, weights::Weight};
use core::marker::PhantomData;

/// Weight functions for `runtime_parachains :: configuration`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> runtime_parachains :: configuration::WeightInfo for WeightInfo<T> {
	/// Storage: `Configuration::PendingConfigs` (r:1 w:1)
	/// Proof: `Configuration::PendingConfigs` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	/// Storage: `Configuration::BypassConsistencyCheck` (r:1 w:0)
	/// Proof: `Configuration::BypassConsistencyCheck` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	/// Storage: `ParasShared::CurrentSessionIndex` (r:1 w:0)
	/// Proof: `ParasShared::CurrentSessionIndex` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	fn set_config_with_block_number() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `151`
		//  Estimated: `1636`
		// Minimum execution time: 14_920_000 picoseconds.
		Weight::from_parts(15_370_000, 0)
			.saturating_add(Weight::from_parts(0, 1636))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Configuration::PendingConfigs` (r:1 w:1)
	/// Proof: `Configuration::PendingConfigs` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	/// Storage: `Configuration::BypassConsistencyCheck` (r:1 w:0)
	/// Proof: `Configuration::BypassConsistencyCheck` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	/// Storage: `ParasShared::CurrentSessionIndex` (r:1 w:0)
	/// Proof: `ParasShared::CurrentSessionIndex` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	fn set_config_with_u32() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `151`
		//  Estimated: `1636`
		// Minimum execution time: 14_830_000 picoseconds.
		Weight::from_parts(15_410_000, 0)
			.saturating_add(Weight::from_parts(0, 1636))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Configuration::PendingConfigs` (r:1 w:1)
	/// Proof: `Configuration::PendingConfigs` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	/// Storage: `Configuration::BypassConsistencyCheck` (r:1 w:0)
	/// Proof: `Configuration::BypassConsistencyCheck` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	/// Storage: `ParasShared::CurrentSessionIndex` (r:1 w:0)
	/// Proof: `ParasShared::CurrentSessionIndex` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	fn set_config_with_option_u32() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `151`
		//  Estimated: `1636`
		// Minimum execution time: 15_110_000 picoseconds.
		Weight::from_parts(15_420_000, 0)
			.saturating_add(Weight::from_parts(0, 1636))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Benchmark::Override` (r:0 w:0)
	/// Proof: `Benchmark::Override` (`max_values`: None, `max_size`: None, mode: `Measured`)
	fn set_hrmp_open_request_ttl() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0`
		//  Estimated: `0`
		// Minimum execution time: 2_000_000_000_000 picoseconds.
		Weight::from_parts(2_000_000_000_000, 0)
			.saturating_add(Weight::from_parts(0, 0))
	}
	/// Storage: `Configuration::PendingConfigs` (r:1 w:1)
	/// Proof: `Configuration::PendingConfigs` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	/// Storage: `Configuration::BypassConsistencyCheck` (r:1 w:0)
	/// Proof: `Configuration::BypassConsistencyCheck` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	/// Storage: `ParasShared::CurrentSessionIndex` (r:1 w:0)
	/// Proof: `ParasShared::CurrentSessionIndex` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	fn set_config_with_balance() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `151`
		//  Estimated: `1636`
		// Minimum execution time: 15_021_000 picoseconds.
		Weight::from_parts(15_400_000, 0)
			.saturating_add(Weight::from_parts(0, 1636))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Configuration::PendingConfigs` (r:1 w:1)
	/// Proof: `Configuration::PendingConfigs` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	/// Storage: `Configuration::BypassConsistencyCheck` (r:1 w:0)
	/// Proof: `Configuration::BypassConsistencyCheck` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	/// Storage: `ParasShared::CurrentSessionIndex` (r:1 w:0)
	/// Proof: `ParasShared::CurrentSessionIndex` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	fn set_config_with_executor_params() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `151`
		//  Estimated: `1636`
		// Minimum execution time: 17_860_000 picoseconds.
		Weight::from_parts(18_230_000, 0)
			.saturating_add(Weight::from_parts(0, 1636))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Configuration::PendingConfigs` (r:1 w:1)
	/// Proof: `Configuration::PendingConfigs` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	/// Storage: `Configuration::BypassConsistencyCheck` (r:1 w:0)
	/// Proof: `Configuration::BypassConsistencyCheck` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	/// Storage: `ParasShared::CurrentSessionIndex` (r:1 w:0)
	/// Proof: `ParasShared::CurrentSessionIndex` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	fn set_config_with_perbill() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `151`
		//  Estimated: `1636`
		// Minimum execution time: 14_860_000 picoseconds.
		Weight::from_parts(15_430_000, 0)
			.saturating_add(Weight::from_parts(0, 1636))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Configuration::PendingConfigs` (r:1 w:1)
	/// Proof: `Configuration::PendingConfigs` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	/// Storage: `Configuration::BypassConsistencyCheck` (r:1 w:0)
	/// Proof: `Configuration::BypassConsistencyCheck` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	/// Storage: `ParasShared::CurrentSessionIndex` (r:1 w:0)
	/// Proof: `ParasShared::CurrentSessionIndex` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	fn set_node_feature() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `151`
		//  Estimated: `1636`
		// Minimum execution time: 18_550_000 picoseconds.
		Weight::from_parts(19_560_000, 0)
			.saturating_add(Weight::from_parts(0, 1636))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Configuration::PendingConfigs` (r:1 w:1)
	/// Proof: `Configuration::PendingConfigs` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	/// Storage: `Configuration::BypassConsistencyCheck` (r:1 w:0)
	/// Proof: `Configuration::BypassConsistencyCheck` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	/// Storage: `ParasShared::CurrentSessionIndex` (r:1 w:0)
	/// Proof: `ParasShared::CurrentSessionIndex` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	fn set_config_with_scheduler_params() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `151`
		//  Estimated: `1636`
		// Minimum execution time: 15_091_000 picoseconds.
		Weight::from_parts(15_490_000, 0)
			.saturating_add(Weight::from_parts(0, 1636))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(1))
	}
}
