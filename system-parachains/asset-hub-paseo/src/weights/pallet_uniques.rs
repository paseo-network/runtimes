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

//! Autogenerated weights for `pallet_uniques`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 47.0.0
//! DATE: 2025-05-07, STEPS: `50`, REPEAT: `20`, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! WORST CASE MAP SIZE: `1000000`
//! HOSTNAME: `2a28e941ef22`, CPU: `QEMU Virtual CPU version 2.5+`
//! WASM-EXECUTION: `Compiled`, CHAIN: `None`, DB CACHE: 1024

// Executed Command:
// frame-omni-bencher
// v1
// benchmark
// pallet
// --extrinsic=*
// --runtime=target/production/wbuild/asset-hub-paseo-runtime/asset_hub_paseo_runtime.wasm
// --pallet=pallet_uniques
// --header=/_work/fellowship-001/runtimes/runtimes/.github/scripts/cmd/file_header.txt
// --output=./system-parachains/asset-hubs/asset-hub-paseo/src/weights
// --wasm-execution=compiled
// --steps=50
// --repeat=20
// --heap-pages=4096

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]
#![allow(missing_docs)]

use frame_support::{traits::Get, weights::Weight};
use core::marker::PhantomData;

/// Weight functions for `pallet_uniques`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_uniques::WeightInfo for WeightInfo<T> {
	/// Storage: `Uniques::Class` (r:1 w:1)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::ClassAccount` (r:0 w:1)
	/// Proof: `Uniques::ClassAccount` (`max_values`: None, `max_size`: Some(68), added: 2543, mode: `MaxEncodedLen`)
	fn create() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `249`
		//  Estimated: `3643`
		// Minimum execution time: 30_200_000 picoseconds.
		Weight::from_parts(31_261_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: `Uniques::Class` (r:1 w:1)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::ClassAccount` (r:0 w:1)
	/// Proof: `Uniques::ClassAccount` (`max_values`: None, `max_size`: Some(68), added: 2543, mode: `MaxEncodedLen`)
	fn force_create() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `109`
		//  Estimated: `3643`
		// Minimum execution time: 12_480_000 picoseconds.
		Weight::from_parts(12_990_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: `Uniques::Class` (r:1 w:1)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Asset` (r:1001 w:1000)
	/// Proof: `Uniques::Asset` (`max_values`: None, `max_size`: Some(122), added: 2597, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::InstanceMetadataOf` (r:1000 w:1000)
	/// Proof: `Uniques::InstanceMetadataOf` (`max_values`: None, `max_size`: Some(187), added: 2662, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Attribute` (r:1000 w:1000)
	/// Proof: `Uniques::Attribute` (`max_values`: None, `max_size`: Some(172), added: 2647, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::ClassAccount` (r:0 w:1)
	/// Proof: `Uniques::ClassAccount` (`max_values`: None, `max_size`: Some(68), added: 2543, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::ClassMetadataOf` (r:0 w:1)
	/// Proof: `Uniques::ClassMetadataOf` (`max_values`: None, `max_size`: Some(167), added: 2642, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Account` (r:0 w:1000)
	/// Proof: `Uniques::Account` (`max_values`: None, `max_size`: Some(88), added: 2563, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::CollectionMaxSupply` (r:0 w:1)
	/// Proof: `Uniques::CollectionMaxSupply` (`max_values`: None, `max_size`: Some(24), added: 2499, mode: `MaxEncodedLen`)
	/// The range of component `n` is `[0, 1000]`.
	/// The range of component `m` is `[0, 1000]`.
	/// The range of component `a` is `[0, 1000]`.
	fn destroy(n: u32, m: u32, a: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `346 + a * (107 ±0) + m * (56 ±0) + n * (76 ±0)`
		//  Estimated: `3643 + a * (2647 ±0) + m * (2662 ±0) + n * (2597 ±0)`
		// Minimum execution time: 3_312_565_000 picoseconds.
		Weight::from_parts(3_339_444_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			// Standard Error: 31_674
			.saturating_add(Weight::from_parts(8_621_167, 0).saturating_mul(n.into()))
			// Standard Error: 31_674
			.saturating_add(Weight::from_parts(496_089, 0).saturating_mul(m.into()))
			// Standard Error: 31_674
			.saturating_add(Weight::from_parts(336_929, 0).saturating_mul(a.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().reads((1_u64).saturating_mul(n.into())))
			.saturating_add(T::DbWeight::get().reads((1_u64).saturating_mul(m.into())))
			.saturating_add(T::DbWeight::get().reads((1_u64).saturating_mul(a.into())))
			.saturating_add(T::DbWeight::get().writes(4))
			.saturating_add(T::DbWeight::get().writes((2_u64).saturating_mul(n.into())))
			.saturating_add(T::DbWeight::get().writes((1_u64).saturating_mul(m.into())))
			.saturating_add(T::DbWeight::get().writes((1_u64).saturating_mul(a.into())))
			.saturating_add(Weight::from_parts(0, 2647).saturating_mul(a.into()))
			.saturating_add(Weight::from_parts(0, 2662).saturating_mul(m.into()))
			.saturating_add(Weight::from_parts(0, 2597).saturating_mul(n.into()))
	}
	/// Storage: `Uniques::Asset` (r:1 w:1)
	/// Proof: `Uniques::Asset` (`max_values`: None, `max_size`: Some(122), added: 2597, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Class` (r:1 w:1)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::CollectionMaxSupply` (r:1 w:0)
	/// Proof: `Uniques::CollectionMaxSupply` (`max_values`: None, `max_size`: Some(24), added: 2499, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Account` (r:0 w:1)
	/// Proof: `Uniques::Account` (`max_values`: None, `max_size`: Some(88), added: 2563, mode: `MaxEncodedLen`)
	fn mint() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `349`
		//  Estimated: `3643`
		// Minimum execution time: 37_161_000 picoseconds.
		Weight::from_parts(37_790_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	/// Storage: `Uniques::Class` (r:1 w:1)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Asset` (r:1 w:1)
	/// Proof: `Uniques::Asset` (`max_values`: None, `max_size`: Some(122), added: 2597, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Account` (r:0 w:1)
	/// Proof: `Uniques::Account` (`max_values`: None, `max_size`: Some(88), added: 2563, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::ItemPriceOf` (r:0 w:1)
	/// Proof: `Uniques::ItemPriceOf` (`max_values`: None, `max_size`: Some(89), added: 2564, mode: `MaxEncodedLen`)
	fn burn() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `495`
		//  Estimated: `3643`
		// Minimum execution time: 37_711_000 picoseconds.
		Weight::from_parts(38_980_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	/// Storage: `Uniques::Class` (r:1 w:0)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Asset` (r:1 w:1)
	/// Proof: `Uniques::Asset` (`max_values`: None, `max_size`: Some(122), added: 2597, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Account` (r:0 w:2)
	/// Proof: `Uniques::Account` (`max_values`: None, `max_size`: Some(88), added: 2563, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::ItemPriceOf` (r:0 w:1)
	/// Proof: `Uniques::ItemPriceOf` (`max_values`: None, `max_size`: Some(89), added: 2564, mode: `MaxEncodedLen`)
	fn transfer() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `495`
		//  Estimated: `3643`
		// Minimum execution time: 27_710_000 picoseconds.
		Weight::from_parts(28_620_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	/// Storage: `Uniques::Class` (r:1 w:1)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Asset` (r:5000 w:5000)
	/// Proof: `Uniques::Asset` (`max_values`: None, `max_size`: Some(122), added: 2597, mode: `MaxEncodedLen`)
	/// The range of component `i` is `[0, 5000]`.
	fn redeposit(i: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `805 + i * (76 ±0)`
		//  Estimated: `3643 + i * (2597 ±0)`
		// Minimum execution time: 12_391_000 picoseconds.
		Weight::from_parts(12_770_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			// Standard Error: 14_684
			.saturating_add(Weight::from_parts(19_750_995, 0).saturating_mul(i.into()))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().reads((1_u64).saturating_mul(i.into())))
			.saturating_add(T::DbWeight::get().writes(1))
			.saturating_add(T::DbWeight::get().writes((1_u64).saturating_mul(i.into())))
			.saturating_add(Weight::from_parts(0, 2597).saturating_mul(i.into()))
	}
	/// Storage: `Uniques::Asset` (r:1 w:1)
	/// Proof: `Uniques::Asset` (`max_values`: None, `max_size`: Some(122), added: 2597, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Class` (r:1 w:0)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	fn freeze() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `495`
		//  Estimated: `3643`
		// Minimum execution time: 17_440_000 picoseconds.
		Weight::from_parts(18_150_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Uniques::Asset` (r:1 w:1)
	/// Proof: `Uniques::Asset` (`max_values`: None, `max_size`: Some(122), added: 2597, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Class` (r:1 w:0)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	fn thaw() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `495`
		//  Estimated: `3643`
		// Minimum execution time: 17_431_000 picoseconds.
		Weight::from_parts(18_790_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Uniques::Class` (r:1 w:1)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	fn freeze_collection() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `349`
		//  Estimated: `3643`
		// Minimum execution time: 11_360_000 picoseconds.
		Weight::from_parts(12_140_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Uniques::Class` (r:1 w:1)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	fn thaw_collection() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `349`
		//  Estimated: `3643`
		// Minimum execution time: 11_440_000 picoseconds.
		Weight::from_parts(12_080_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Uniques::OwnershipAcceptance` (r:1 w:1)
	/// Proof: `Uniques::OwnershipAcceptance` (`max_values`: None, `max_size`: Some(52), added: 2527, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Class` (r:1 w:1)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `System::Account` (r:1 w:1)
	/// Proof: `System::Account` (`max_values`: None, `max_size`: Some(128), added: 2603, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::ClassAccount` (r:0 w:2)
	/// Proof: `Uniques::ClassAccount` (`max_values`: None, `max_size`: Some(68), added: 2543, mode: `MaxEncodedLen`)
	fn transfer_ownership() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `597`
		//  Estimated: `3643`
		// Minimum execution time: 28_340_000 picoseconds.
		Weight::from_parts(29_710_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(5))
	}
	/// Storage: `Uniques::Class` (r:1 w:1)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	fn set_team() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `349`
		//  Estimated: `3643`
		// Minimum execution time: 11_480_000 picoseconds.
		Weight::from_parts(12_681_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Uniques::Class` (r:1 w:1)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::ClassAccount` (r:0 w:1)
	/// Proof: `Uniques::ClassAccount` (`max_values`: None, `max_size`: Some(68), added: 2543, mode: `MaxEncodedLen`)
	fn force_item_status() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `349`
		//  Estimated: `3643`
		// Minimum execution time: 15_150_000 picoseconds.
		Weight::from_parts(15_950_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: `Uniques::Class` (r:1 w:1)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::InstanceMetadataOf` (r:1 w:0)
	/// Proof: `Uniques::InstanceMetadataOf` (`max_values`: None, `max_size`: Some(187), added: 2662, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Attribute` (r:1 w:1)
	/// Proof: `Uniques::Attribute` (`max_values`: None, `max_size`: Some(172), added: 2647, mode: `MaxEncodedLen`)
	fn set_attribute() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `626`
		//  Estimated: `3652`
		// Minimum execution time: 41_440_000 picoseconds.
		Weight::from_parts(43_240_000, 0)
			.saturating_add(Weight::from_parts(0, 3652))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: `Uniques::Class` (r:1 w:1)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::InstanceMetadataOf` (r:1 w:0)
	/// Proof: `Uniques::InstanceMetadataOf` (`max_values`: None, `max_size`: Some(187), added: 2662, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Attribute` (r:1 w:1)
	/// Proof: `Uniques::Attribute` (`max_values`: None, `max_size`: Some(172), added: 2647, mode: `MaxEncodedLen`)
	fn clear_attribute() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `823`
		//  Estimated: `3652`
		// Minimum execution time: 41_170_000 picoseconds.
		Weight::from_parts(42_470_000, 0)
			.saturating_add(Weight::from_parts(0, 3652))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: `Uniques::Class` (r:1 w:1)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::InstanceMetadataOf` (r:1 w:1)
	/// Proof: `Uniques::InstanceMetadataOf` (`max_values`: None, `max_size`: Some(187), added: 2662, mode: `MaxEncodedLen`)
	fn set_metadata() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `415`
		//  Estimated: `3652`
		// Minimum execution time: 29_671_000 picoseconds.
		Weight::from_parts(30_901_000, 0)
			.saturating_add(Weight::from_parts(0, 3652))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: `Uniques::Class` (r:1 w:1)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::InstanceMetadataOf` (r:1 w:1)
	/// Proof: `Uniques::InstanceMetadataOf` (`max_values`: None, `max_size`: Some(187), added: 2662, mode: `MaxEncodedLen`)
	fn clear_metadata() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `626`
		//  Estimated: `3652`
		// Minimum execution time: 31_190_000 picoseconds.
		Weight::from_parts(31_951_000, 0)
			.saturating_add(Weight::from_parts(0, 3652))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: `Uniques::Class` (r:1 w:1)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::ClassMetadataOf` (r:1 w:1)
	/// Proof: `Uniques::ClassMetadataOf` (`max_values`: None, `max_size`: Some(167), added: 2642, mode: `MaxEncodedLen`)
	fn set_collection_metadata() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `349`
		//  Estimated: `3643`
		// Minimum execution time: 30_710_000 picoseconds.
		Weight::from_parts(31_340_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: `Uniques::Class` (r:1 w:1)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::ClassMetadataOf` (r:1 w:1)
	/// Proof: `Uniques::ClassMetadataOf` (`max_values`: None, `max_size`: Some(167), added: 2642, mode: `MaxEncodedLen`)
	fn clear_collection_metadata() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `540`
		//  Estimated: `3643`
		// Minimum execution time: 30_610_000 picoseconds.
		Weight::from_parts(31_730_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: `Uniques::Class` (r:1 w:0)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Asset` (r:1 w:1)
	/// Proof: `Uniques::Asset` (`max_values`: None, `max_size`: Some(122), added: 2597, mode: `MaxEncodedLen`)
	fn approve_transfer() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `495`
		//  Estimated: `3643`
		// Minimum execution time: 17_660_000 picoseconds.
		Weight::from_parts(18_650_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Uniques::Class` (r:1 w:0)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Asset` (r:1 w:1)
	/// Proof: `Uniques::Asset` (`max_values`: None, `max_size`: Some(122), added: 2597, mode: `MaxEncodedLen`)
	fn cancel_approval() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `528`
		//  Estimated: `3643`
		// Minimum execution time: 17_520_000 picoseconds.
		Weight::from_parts(18_600_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Uniques::OwnershipAcceptance` (r:1 w:1)
	/// Proof: `Uniques::OwnershipAcceptance` (`max_values`: None, `max_size`: Some(52), added: 2527, mode: `MaxEncodedLen`)
	fn set_accept_ownership() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `109`
		//  Estimated: `3517`
		// Minimum execution time: 13_099_000 picoseconds.
		Weight::from_parts(13_910_000, 0)
			.saturating_add(Weight::from_parts(0, 3517))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Uniques::CollectionMaxSupply` (r:1 w:1)
	/// Proof: `Uniques::CollectionMaxSupply` (`max_values`: None, `max_size`: Some(24), added: 2499, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Class` (r:1 w:0)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	fn set_collection_max_supply() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `349`
		//  Estimated: `3643`
		// Minimum execution time: 14_060_000 picoseconds.
		Weight::from_parts(15_151_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Uniques::Asset` (r:1 w:0)
	/// Proof: `Uniques::Asset` (`max_values`: None, `max_size`: Some(122), added: 2597, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::ItemPriceOf` (r:0 w:1)
	/// Proof: `Uniques::ItemPriceOf` (`max_values`: None, `max_size`: Some(89), added: 2564, mode: `MaxEncodedLen`)
	fn set_price() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `326`
		//  Estimated: `3587`
		// Minimum execution time: 14_180_000 picoseconds.
		Weight::from_parts(15_100_000, 0)
			.saturating_add(Weight::from_parts(0, 3587))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Uniques::Asset` (r:1 w:1)
	/// Proof: `Uniques::Asset` (`max_values`: None, `max_size`: Some(122), added: 2597, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::ItemPriceOf` (r:1 w:1)
	/// Proof: `Uniques::ItemPriceOf` (`max_values`: None, `max_size`: Some(89), added: 2564, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Class` (r:1 w:0)
	/// Proof: `Uniques::Class` (`max_values`: None, `max_size`: Some(178), added: 2653, mode: `MaxEncodedLen`)
	/// Storage: `Uniques::Account` (r:0 w:2)
	/// Proof: `Uniques::Account` (`max_values`: None, `max_size`: Some(88), added: 2563, mode: `MaxEncodedLen`)
	fn buy_item() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `607`
		//  Estimated: `3643`
		// Minimum execution time: 38_940_000 picoseconds.
		Weight::from_parts(40_050_000, 0)
			.saturating_add(Weight::from_parts(0, 3643))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(4))
	}
}
