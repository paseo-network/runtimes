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

//! Placeholder weight info. Replace with benchmarked weights when this pallet
//! is ready to be wired into a runtime.

use core::marker::PhantomData;
use frame_support::{
	traits::Get,
	weights::{constants::RocksDbWeight, Weight},
};

pub trait WeightInfo {
	fn schedule_event() -> Weight;
	fn remove_scheduled_event() -> Weight;
	fn enable_asset() -> Weight;
	fn disable_asset() -> Weight;
	fn participate_with_alias() -> Weight;
	fn participate_with_account_via_schnorrkel_vrf() -> Weight;
	fn claim() -> Weight;
	fn start_registration() -> Weight;
	fn close_registration() -> Weight;
	fn draw_winners(n: u32) -> Weight;
	fn close_drawing() -> Weight;
	fn close_claiming() -> Weight;
	fn clean_up_registrations(n: u32) -> Weight;
	fn clean_up_winners(n: u32) -> Weight;
	fn finalize() -> Weight;
	/// Weight of the `(transition + Events::insert + deposit_event)` step that
	/// fires once a clean-up phase finishes and the next one is entered.
	/// Charged in addition to `clean_up_registrations(N)` / `clean_up_winners(N)`
	/// at pre-dispatch, refunded in the post-dispatch when the transition
	/// didn't run (the phase had more entries to clear and re-fires on the
	/// next OCW tick).
	fn transition_clean_up_phase() -> Weight;
	fn authorize_lifecycle_call() -> Weight;
}

/// Weights for `indiv_pallet_airdrop` using the Substrate node and recommended hardware.
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
	/// Storage: `Airdrop::SupportedAssets` (r:1 w:0)
	/// Proof: `Airdrop::SupportedAssets` (`max_values`: None, `max_size`: Some(626), added:
	/// 3101, mode: `MaxEncodedLen`) Storage: `Assets::Asset` (r:1 w:1)
	/// Proof: `Assets::Asset` (`max_values`: None, `max_size`: Some(808), added: 3283, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Account` (r:1 w:1)
	/// Proof: `Assets::Account` (`max_values`: None, `max_size`: Some(732), added: 3207, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::BalancesOnHold` (r:1 w:1)
	/// Proof: `AssetsHolder::BalancesOnHold` (`max_values`: None, `max_size`: Some(682), added:
	/// 3157, mode: `MaxEncodedLen`) Storage: `AssetsHolder::Holds` (r:1 w:1)
	/// Proof: `AssetsHolder::Holds` (`max_values`: None, `max_size`: Some(847), added: 3322, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::ActionSchedule` (r:0 w:1)
	/// Proof: `Airdrop::ActionSchedule` (`max_values`: None, `max_size`: Some(40), added: 2515,
	/// mode: `MaxEncodedLen`)
	fn schedule_event() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `640`
		//  Estimated: `4312`
		// Minimum execution time: 42_825_000 picoseconds.
		Weight::from_parts(44_648_000, 0)
			.saturating_add(Weight::from_parts(0, 4312))
			.saturating_add(T::DbWeight::get().reads(6))
			.saturating_add(T::DbWeight::get().writes(6))
	}
	/// Storage: `Airdrop::SupportedAssets` (r:1 w:1)
	/// Proof: `Airdrop::SupportedAssets` (`max_values`: None, `max_size`: Some(626), added:
	/// 3101, mode: `MaxEncodedLen`) Storage: `Assets::Asset` (r:1 w:1)
	/// Proof: `Assets::Asset` (`max_values`: None, `max_size`: Some(808), added: 3283, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Account` (r:2 w:2)
	/// Proof: `Assets::Account` (`max_values`: None, `max_size`: Some(732), added: 3207, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::BalancesOnHold` (r:1 w:0)
	/// Proof: `AssetsHolder::BalancesOnHold` (`max_values`: None, `max_size`: Some(682), added:
	/// 3157, mode: `MaxEncodedLen`) Storage: `System::Account` (r:1 w:1)
	/// Proof: `System::Account` (`max_values`: None, `max_size`: Some(128), added: 2603, mode:
	/// `MaxEncodedLen`)
	fn enable_asset() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `582`
		//  Estimated: `7404`
		// Minimum execution time: 43_146_000 picoseconds.
		Weight::from_parts(44_958_000, 0)
			.saturating_add(Weight::from_parts(0, 7404))
			.saturating_add(T::DbWeight::get().reads(6))
			.saturating_add(T::DbWeight::get().writes(5))
	}
	/// Storage: `Airdrop::SupportedAssets` (r:1 w:1)
	/// Proof: `Airdrop::SupportedAssets` (`max_values`: None, `max_size`: Some(626), added:
	/// 3101, mode: `MaxEncodedLen`) Storage: `Assets::Asset` (r:1 w:1)
	/// Proof: `Assets::Asset` (`max_values`: None, `max_size`: Some(808), added: 3283, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Account` (r:2 w:2)
	/// Proof: `Assets::Account` (`max_values`: None, `max_size`: Some(732), added: 3207, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::BalancesOnHold` (r:1 w:1)
	/// Proof: `AssetsHolder::BalancesOnHold` (`max_values`: None, `max_size`: Some(682), added:
	/// 3157, mode: `MaxEncodedLen`) Storage: `System::Account` (r:2 w:2)
	/// Proof: `System::Account` (`max_values`: None, `max_size`: Some(128), added: 2603, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::Holds` (r:1 w:1)
	/// Proof: `AssetsHolder::Holds` (`max_values`: None, `max_size`: Some(847), added: 3322, mode:
	/// `MaxEncodedLen`)
	fn disable_asset() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `743`
		//  Estimated: `7404`
		// Minimum execution time: 53_812_000 picoseconds.
		Weight::from_parts(55_975_000, 0)
			.saturating_add(Weight::from_parts(0, 7404))
			.saturating_add(T::DbWeight::get().reads(8))
			.saturating_add(T::DbWeight::get().writes(8))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Asset` (r:1 w:1)
	/// Proof: `Assets::Asset` (`max_values`: None, `max_size`: Some(808), added: 3283, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Account` (r:1 w:1)
	/// Proof: `Assets::Account` (`max_values`: None, `max_size`: Some(732), added: 3207, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::Holds` (r:1 w:1)
	/// Proof: `AssetsHolder::Holds` (`max_values`: None, `max_size`: Some(847), added: 3322, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::BalancesOnHold` (r:1 w:1)
	/// Proof: `AssetsHolder::BalancesOnHold` (`max_values`: None, `max_size`: Some(682), added:
	/// 3157, mode: `MaxEncodedLen`) Storage: `Airdrop::ActionSchedule` (r:0 w:1)
	/// Proof: `Airdrop::ActionSchedule` (`max_values`: None, `max_size`: Some(40), added: 2515,
	/// mode: `MaxEncodedLen`)
	fn remove_scheduled_event() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `1036`
		//  Estimated: `4312`
		// Minimum execution time: 34_542_000 picoseconds.
		Weight::from_parts(36_125_000, 0)
			.saturating_add(Weight::from_parts(0, 4312))
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(6))
	}
	/// Storage: `Members::Collections` (r:1 w:0)
	/// Proof: `Members::Collections` (`max_values`: None, `max_size`: Some(646), added: 3121, mode:
	/// `MaxEncodedLen`) Storage: `Timestamp::Now` (r:1 w:0)
	/// Proof: `Timestamp::Now` (`max_values`: Some(1), `max_size`: Some(8), added: 503, mode:
	/// `MaxEncodedLen`) Storage: `Members::Root` (r:1 w:0)
	/// Proof: `Members::Root` (`max_values`: None, `max_size`: Some(1672), added: 4147, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::Registrations` (r:1 w:1)
	/// Proof: `Airdrop::Registrations` (`max_values`: None, `max_size`: Some(105), added: 2580,
	/// mode: `MaxEncodedLen`)
	fn participate_with_alias() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `2258`
		//  Estimated: `5137`
		// Minimum execution time: 19_136_373_000 picoseconds.
		Weight::from_parts(19_150_996_000, 0)
			.saturating_add(Weight::from_parts(0, 5137))
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::Registrations` (r:1 w:1)
	/// Proof: `Airdrop::Registrations` (`max_values`: None, `max_size`: Some(105), added: 2580,
	/// mode: `MaxEncodedLen`)
	fn participate_with_account_via_schnorrkel_vrf() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `371`
		//  Estimated: `4232`
		// Minimum execution time: 361_259_000 picoseconds.
		Weight::from_parts(365_475_000, 0)
			.saturating_add(Weight::from_parts(0, 4232))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Timestamp::Now` (r:1 w:0)
	/// Proof: `Timestamp::Now` (`max_values`: Some(1), `max_size`: Some(8), added: 503, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::SupportedAssets` (r:1 w:0)
	/// Proof: `Airdrop::SupportedAssets` (`max_values`: None, `max_size`: Some(626), added:
	/// 3101, mode: `MaxEncodedLen`) Storage: `Airdrop::Winners` (r:1 w:1)
	/// Proof: `Airdrop::Winners` (`max_values`: None, `max_size`: Some(121), added: 2596, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Asset` (r:1 w:1)
	/// Proof: `Assets::Asset` (`max_values`: None, `max_size`: Some(808), added: 3283, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Account` (r:2 w:2)
	/// Proof: `Assets::Account` (`max_values`: None, `max_size`: Some(732), added: 3207, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::Holds` (r:1 w:1)
	/// Proof: `AssetsHolder::Holds` (`max_values`: None, `max_size`: Some(847), added: 3322, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::BalancesOnHold` (r:1 w:1)
	/// Proof: `AssetsHolder::BalancesOnHold` (`max_values`: None, `max_size`: Some(682), added:
	/// 3157, mode: `MaxEncodedLen`) Storage: `System::Account` (r:1 w:1)
	/// Proof: `System::Account` (`max_values`: None, `max_size`: Some(128), added: 2603, mode:
	/// `MaxEncodedLen`)
	fn claim() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `1286`
		//  Estimated: `7404`
		// Minimum execution time: 70_567_000 picoseconds.
		Weight::from_parts(73_001_000, 0)
			.saturating_add(Weight::from_parts(0, 7404))
			.saturating_add(T::DbWeight::get().reads(10))
			.saturating_add(T::DbWeight::get().writes(8))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::ActionSchedule` (r:0 w:2)
	/// Proof: `Airdrop::ActionSchedule` (`max_values`: None, `max_size`: Some(40), added: 2515,
	/// mode: `MaxEncodedLen`)
	fn start_registration() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `367`
		//  Estimated: `4232`
		// Minimum execution time: 9_514_000 picoseconds.
		Weight::from_parts(10_336_000, 0)
			.saturating_add(Weight::from_parts(0, 4232))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `System::ParentHash` (r:1 w:0)
	/// Proof: `System::ParentHash` (`max_values`: Some(1), `max_size`: Some(32), added: 527, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Asset` (r:1 w:1)
	/// Proof: `Assets::Asset` (`max_values`: None, `max_size`: Some(808), added: 3283, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Account` (r:1 w:1)
	/// Proof: `Assets::Account` (`max_values`: None, `max_size`: Some(732), added: 3207, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::Holds` (r:1 w:1)
	/// Proof: `AssetsHolder::Holds` (`max_values`: None, `max_size`: Some(847), added: 3322, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::BalancesOnHold` (r:1 w:1)
	/// Proof: `AssetsHolder::BalancesOnHold` (`max_values`: None, `max_size`: Some(682), added:
	/// 3157, mode: `MaxEncodedLen`) Storage: `Airdrop::EventEntropy` (r:0 w:1)
	/// Proof: `Airdrop::EventEntropy` (`max_values`: None, `max_size`: Some(72), added: 2547,
	/// mode: `MaxEncodedLen`)
	fn close_registration() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `1073`
		//  Estimated: `4312`
		// Minimum execution time: 36_745_000 picoseconds.
		Weight::from_parts(38_429_000, 0)
			.saturating_add(Weight::from_parts(0, 4312))
			.saturating_add(T::DbWeight::get().reads(6))
			.saturating_add(T::DbWeight::get().writes(6))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::Registrations` (r:102 w:0)
	/// Proof: `Airdrop::Registrations` (`max_values`: None, `max_size`: Some(105), added: 2580,
	/// mode: `MaxEncodedLen`) Storage: `Airdrop::Winners` (r:0 w:100)
	/// Proof: `Airdrop::Winners` (`max_values`: None, `max_size`: Some(121), added: 2596, mode:
	/// `MaxEncodedLen`) The range of component `n` is `[1, 100]`.
	fn draw_winners(n: u32) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `511 + n * (42 ±0)`
		//  Estimated: `4321 + n * (2603 ±0)`
		// Minimum execution time: 13_881_000 picoseconds.
		Weight::from_parts(10_218_790, 0)
			.saturating_add(Weight::from_parts(0, 4321))
			// Standard Error: 1_249
			.saturating_add(Weight::from_parts(4_101_120, 0).saturating_mul(n.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().reads((1_u64).saturating_mul(n.into())))
			.saturating_add(T::DbWeight::get().writes(1))
			.saturating_add(T::DbWeight::get().writes((1_u64).saturating_mul(n.into())))
			.saturating_add(Weight::from_parts(0, 2603).saturating_mul(n.into()))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::ActionSchedule` (r:0 w:2)
	/// Proof: `Airdrop::ActionSchedule` (`max_values`: None, `max_size`: Some(40), added: 2515,
	/// mode: `MaxEncodedLen`)
	fn close_drawing() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `411`
		//  Estimated: `4232`
		// Minimum execution time: 9_525_000 picoseconds.
		Weight::from_parts(10_386_000, 0)
			.saturating_add(Weight::from_parts(0, 4232))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`)
	fn close_claiming() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `379`
		//  Estimated: `4232`
		// Minimum execution time: 7_712_000 picoseconds.
		Weight::from_parts(8_523_000, 0)
			.saturating_add(Weight::from_parts(0, 4232))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::Registrations` (r:101 w:100)
	/// Proof: `Airdrop::Registrations` (`max_values`: None, `max_size`: Some(105), added: 2580,
	/// mode: `MaxEncodedLen`) The range of component `n` is `[1, 100]`.
	fn clean_up_registrations(n: u32) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `513 + n * (38 ±0)`
		//  Estimated: `4232 + n * (2580 ±0)`
		// Minimum execution time: 9_375_000 picoseconds.
		Weight::from_parts(9_177_018, 0)
			.saturating_add(Weight::from_parts(0, 4232))
			// Standard Error: 255
			.saturating_add(Weight::from_parts(437_758, 0).saturating_mul(n.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().reads((1_u64).saturating_mul(n.into())))
			.saturating_add(T::DbWeight::get().writes(1))
			.saturating_add(T::DbWeight::get().writes((1_u64).saturating_mul(n.into())))
			.saturating_add(Weight::from_parts(0, 2580).saturating_mul(n.into()))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::Winners` (r:101 w:100)
	/// Proof: `Airdrop::Winners` (`max_values`: None, `max_size`: Some(121), added: 2596, mode:
	/// `MaxEncodedLen`) The range of component `n` is `[1, 100]`.
	fn clean_up_winners(n: u32) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `549 + n * (87 ±0)`
		//  Estimated: `4232 + n * (2596 ±0)`
		// Minimum execution time: 8_814_000 picoseconds.
		Weight::from_parts(8_791_876, 0)
			.saturating_add(Weight::from_parts(0, 4232))
			// Standard Error: 197
			.saturating_add(Weight::from_parts(574_785, 0).saturating_mul(n.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().reads((1_u64).saturating_mul(n.into())))
			.saturating_add(T::DbWeight::get().writes(1))
			.saturating_add(T::DbWeight::get().writes((1_u64).saturating_mul(n.into())))
			.saturating_add(Weight::from_parts(0, 2596).saturating_mul(n.into()))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Asset` (r:1 w:1)
	/// Proof: `Assets::Asset` (`max_values`: None, `max_size`: Some(808), added: 3283, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Account` (r:1 w:1)
	/// Proof: `Assets::Account` (`max_values`: None, `max_size`: Some(732), added: 3207, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::Holds` (r:1 w:1)
	/// Proof: `AssetsHolder::Holds` (`max_values`: None, `max_size`: Some(847), added: 3322, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::BalancesOnHold` (r:1 w:1)
	/// Proof: `AssetsHolder::BalancesOnHold` (`max_values`: None, `max_size`: Some(682), added:
	/// 3157, mode: `MaxEncodedLen`) Storage: `Airdrop::ActionSchedule` (r:0 w:1)
	/// Proof: `Airdrop::ActionSchedule` (`max_values`: None, `max_size`: Some(40), added: 2515,
	/// mode: `MaxEncodedLen`) Storage: `Airdrop::EventEntropy` (r:0 w:1)
	/// Proof: `Airdrop::EventEntropy` (`max_values`: None, `max_size`: Some(72), added: 2547,
	/// mode: `MaxEncodedLen`)
	fn finalize() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `1044`
		//  Estimated: `4312`
		// Minimum execution time: 35_484_000 picoseconds.
		Weight::from_parts(37_187_000, 0)
			.saturating_add(Weight::from_parts(0, 4312))
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(7))
	}
	/// Storage: `Airdrop::Events` (r:0 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`)
	fn transition_clean_up_phase() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0`
		//  Estimated: `0`
		// Minimum execution time: 2_543_000 picoseconds.
		Weight::from_parts(2_865_000, 0)
			.saturating_add(Weight::from_parts(0, 0))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: `Airdrop::Events` (r:1 w:0)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`)
	fn authorize_lifecycle_call() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `411`
		//  Estimated: `4232`
		// Minimum execution time: 4_166_000 picoseconds.
		Weight::from_parts(4_577_000, 0)
			.saturating_add(Weight::from_parts(0, 4232))
			.saturating_add(T::DbWeight::get().reads(1))
	}
}

// For backwards compatibility and tests.
impl WeightInfo for () {
	/// Storage: `Airdrop::SupportedAssets` (r:1 w:0)
	/// Proof: `Airdrop::SupportedAssets` (`max_values`: None, `max_size`: Some(626), added:
	/// 3101, mode: `MaxEncodedLen`) Storage: `Assets::Asset` (r:1 w:1)
	/// Proof: `Assets::Asset` (`max_values`: None, `max_size`: Some(808), added: 3283, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Account` (r:1 w:1)
	/// Proof: `Assets::Account` (`max_values`: None, `max_size`: Some(732), added: 3207, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::BalancesOnHold` (r:1 w:1)
	/// Proof: `AssetsHolder::BalancesOnHold` (`max_values`: None, `max_size`: Some(682), added:
	/// 3157, mode: `MaxEncodedLen`) Storage: `AssetsHolder::Holds` (r:1 w:1)
	/// Proof: `AssetsHolder::Holds` (`max_values`: None, `max_size`: Some(847), added: 3322, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::ActionSchedule` (r:0 w:1)
	/// Proof: `Airdrop::ActionSchedule` (`max_values`: None, `max_size`: Some(40), added: 2515,
	/// mode: `MaxEncodedLen`)
	fn schedule_event() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `640`
		//  Estimated: `4312`
		// Minimum execution time: 42_825_000 picoseconds.
		Weight::from_parts(44_648_000, 0)
			.saturating_add(Weight::from_parts(0, 4312))
			.saturating_add(RocksDbWeight::get().reads(6))
			.saturating_add(RocksDbWeight::get().writes(6))
	}
	/// Storage: `Airdrop::SupportedAssets` (r:1 w:1)
	/// Proof: `Airdrop::SupportedAssets` (`max_values`: None, `max_size`: Some(626), added:
	/// 3101, mode: `MaxEncodedLen`) Storage: `Assets::Asset` (r:1 w:1)
	/// Proof: `Assets::Asset` (`max_values`: None, `max_size`: Some(808), added: 3283, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Account` (r:2 w:2)
	/// Proof: `Assets::Account` (`max_values`: None, `max_size`: Some(732), added: 3207, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::BalancesOnHold` (r:1 w:0)
	/// Proof: `AssetsHolder::BalancesOnHold` (`max_values`: None, `max_size`: Some(682), added:
	/// 3157, mode: `MaxEncodedLen`) Storage: `System::Account` (r:1 w:1)
	/// Proof: `System::Account` (`max_values`: None, `max_size`: Some(128), added: 2603, mode:
	/// `MaxEncodedLen`)
	fn enable_asset() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `582`
		//  Estimated: `7404`
		// Minimum execution time: 43_146_000 picoseconds.
		Weight::from_parts(44_958_000, 0)
			.saturating_add(Weight::from_parts(0, 7404))
			.saturating_add(RocksDbWeight::get().reads(6))
			.saturating_add(RocksDbWeight::get().writes(5))
	}
	/// Storage: `Airdrop::SupportedAssets` (r:1 w:1)
	/// Proof: `Airdrop::SupportedAssets` (`max_values`: None, `max_size`: Some(626), added:
	/// 3101, mode: `MaxEncodedLen`) Storage: `Assets::Asset` (r:1 w:1)
	/// Proof: `Assets::Asset` (`max_values`: None, `max_size`: Some(808), added: 3283, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Account` (r:2 w:2)
	/// Proof: `Assets::Account` (`max_values`: None, `max_size`: Some(732), added: 3207, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::BalancesOnHold` (r:1 w:1)
	/// Proof: `AssetsHolder::BalancesOnHold` (`max_values`: None, `max_size`: Some(682), added:
	/// 3157, mode: `MaxEncodedLen`) Storage: `System::Account` (r:2 w:2)
	/// Proof: `System::Account` (`max_values`: None, `max_size`: Some(128), added: 2603, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::Holds` (r:1 w:1)
	/// Proof: `AssetsHolder::Holds` (`max_values`: None, `max_size`: Some(847), added: 3322, mode:
	/// `MaxEncodedLen`)
	fn disable_asset() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `743`
		//  Estimated: `7404`
		// Minimum execution time: 53_812_000 picoseconds.
		Weight::from_parts(55_975_000, 0)
			.saturating_add(Weight::from_parts(0, 7404))
			.saturating_add(RocksDbWeight::get().reads(8))
			.saturating_add(RocksDbWeight::get().writes(8))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Asset` (r:1 w:1)
	/// Proof: `Assets::Asset` (`max_values`: None, `max_size`: Some(808), added: 3283, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Account` (r:1 w:1)
	/// Proof: `Assets::Account` (`max_values`: None, `max_size`: Some(732), added: 3207, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::Holds` (r:1 w:1)
	/// Proof: `AssetsHolder::Holds` (`max_values`: None, `max_size`: Some(847), added: 3322, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::BalancesOnHold` (r:1 w:1)
	/// Proof: `AssetsHolder::BalancesOnHold` (`max_values`: None, `max_size`: Some(682), added:
	/// 3157, mode: `MaxEncodedLen`) Storage: `Airdrop::ActionSchedule` (r:0 w:1)
	/// Proof: `Airdrop::ActionSchedule` (`max_values`: None, `max_size`: Some(40), added: 2515,
	/// mode: `MaxEncodedLen`)
	fn remove_scheduled_event() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `1036`
		//  Estimated: `4312`
		// Minimum execution time: 34_542_000 picoseconds.
		Weight::from_parts(36_125_000, 0)
			.saturating_add(Weight::from_parts(0, 4312))
			.saturating_add(RocksDbWeight::get().reads(5))
			.saturating_add(RocksDbWeight::get().writes(6))
	}
	/// Storage: `Members::Collections` (r:1 w:0)
	/// Proof: `Members::Collections` (`max_values`: None, `max_size`: Some(646), added: 3121, mode:
	/// `MaxEncodedLen`) Storage: `Timestamp::Now` (r:1 w:0)
	/// Proof: `Timestamp::Now` (`max_values`: Some(1), `max_size`: Some(8), added: 503, mode:
	/// `MaxEncodedLen`) Storage: `Members::Root` (r:1 w:0)
	/// Proof: `Members::Root` (`max_values`: None, `max_size`: Some(1672), added: 4147, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::Registrations` (r:1 w:1)
	/// Proof: `Airdrop::Registrations` (`max_values`: None, `max_size`: Some(105), added: 2580,
	/// mode: `MaxEncodedLen`)
	fn participate_with_alias() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `2258`
		//  Estimated: `5137`
		// Minimum execution time: 19_136_373_000 picoseconds.
		Weight::from_parts(19_150_996_000, 0)
			.saturating_add(Weight::from_parts(0, 5137))
			.saturating_add(RocksDbWeight::get().reads(5))
			.saturating_add(RocksDbWeight::get().writes(2))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::Registrations` (r:1 w:1)
	/// Proof: `Airdrop::Registrations` (`max_values`: None, `max_size`: Some(105), added: 2580,
	/// mode: `MaxEncodedLen`)
	fn participate_with_account_via_schnorrkel_vrf() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `371`
		//  Estimated: `4232`
		// Minimum execution time: 361_259_000 picoseconds.
		Weight::from_parts(365_475_000, 0)
			.saturating_add(Weight::from_parts(0, 4232))
			.saturating_add(RocksDbWeight::get().reads(2))
			.saturating_add(RocksDbWeight::get().writes(2))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Timestamp::Now` (r:1 w:0)
	/// Proof: `Timestamp::Now` (`max_values`: Some(1), `max_size`: Some(8), added: 503, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::SupportedAssets` (r:1 w:0)
	/// Proof: `Airdrop::SupportedAssets` (`max_values`: None, `max_size`: Some(626), added:
	/// 3101, mode: `MaxEncodedLen`) Storage: `Airdrop::Winners` (r:1 w:1)
	/// Proof: `Airdrop::Winners` (`max_values`: None, `max_size`: Some(121), added: 2596, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Asset` (r:1 w:1)
	/// Proof: `Assets::Asset` (`max_values`: None, `max_size`: Some(808), added: 3283, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Account` (r:2 w:2)
	/// Proof: `Assets::Account` (`max_values`: None, `max_size`: Some(732), added: 3207, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::Holds` (r:1 w:1)
	/// Proof: `AssetsHolder::Holds` (`max_values`: None, `max_size`: Some(847), added: 3322, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::BalancesOnHold` (r:1 w:1)
	/// Proof: `AssetsHolder::BalancesOnHold` (`max_values`: None, `max_size`: Some(682), added:
	/// 3157, mode: `MaxEncodedLen`) Storage: `System::Account` (r:1 w:1)
	/// Proof: `System::Account` (`max_values`: None, `max_size`: Some(128), added: 2603, mode:
	/// `MaxEncodedLen`)
	fn claim() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `1286`
		//  Estimated: `7404`
		// Minimum execution time: 70_567_000 picoseconds.
		Weight::from_parts(73_001_000, 0)
			.saturating_add(Weight::from_parts(0, 7404))
			.saturating_add(RocksDbWeight::get().reads(10))
			.saturating_add(RocksDbWeight::get().writes(8))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::ActionSchedule` (r:0 w:2)
	/// Proof: `Airdrop::ActionSchedule` (`max_values`: None, `max_size`: Some(40), added: 2515,
	/// mode: `MaxEncodedLen`)
	fn start_registration() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `367`
		//  Estimated: `4232`
		// Minimum execution time: 9_514_000 picoseconds.
		Weight::from_parts(10_336_000, 0)
			.saturating_add(Weight::from_parts(0, 4232))
			.saturating_add(RocksDbWeight::get().reads(1))
			.saturating_add(RocksDbWeight::get().writes(3))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `System::ParentHash` (r:1 w:0)
	/// Proof: `System::ParentHash` (`max_values`: Some(1), `max_size`: Some(32), added: 527, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Asset` (r:1 w:1)
	/// Proof: `Assets::Asset` (`max_values`: None, `max_size`: Some(808), added: 3283, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Account` (r:1 w:1)
	/// Proof: `Assets::Account` (`max_values`: None, `max_size`: Some(732), added: 3207, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::Holds` (r:1 w:1)
	/// Proof: `AssetsHolder::Holds` (`max_values`: None, `max_size`: Some(847), added: 3322, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::BalancesOnHold` (r:1 w:1)
	/// Proof: `AssetsHolder::BalancesOnHold` (`max_values`: None, `max_size`: Some(682), added:
	/// 3157, mode: `MaxEncodedLen`) Storage: `Airdrop::EventEntropy` (r:0 w:1)
	/// Proof: `Airdrop::EventEntropy` (`max_values`: None, `max_size`: Some(72), added: 2547,
	/// mode: `MaxEncodedLen`)
	fn close_registration() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `1073`
		//  Estimated: `4312`
		// Minimum execution time: 36_745_000 picoseconds.
		Weight::from_parts(38_429_000, 0)
			.saturating_add(Weight::from_parts(0, 4312))
			.saturating_add(RocksDbWeight::get().reads(6))
			.saturating_add(RocksDbWeight::get().writes(6))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::Registrations` (r:102 w:0)
	/// Proof: `Airdrop::Registrations` (`max_values`: None, `max_size`: Some(105), added: 2580,
	/// mode: `MaxEncodedLen`) Storage: `Airdrop::Winners` (r:0 w:100)
	/// Proof: `Airdrop::Winners` (`max_values`: None, `max_size`: Some(121), added: 2596, mode:
	/// `MaxEncodedLen`) The range of component `n` is `[1, 100]`.
	fn draw_winners(n: u32) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `511 + n * (42 ±0)`
		//  Estimated: `4321 + n * (2603 ±0)`
		// Minimum execution time: 13_881_000 picoseconds.
		Weight::from_parts(10_218_790, 0)
			.saturating_add(Weight::from_parts(0, 4321))
			// Standard Error: 1_249
			.saturating_add(Weight::from_parts(4_101_120, 0).saturating_mul(n.into()))
			.saturating_add(RocksDbWeight::get().reads(2))
			.saturating_add(RocksDbWeight::get().reads((1_u64).saturating_mul(n.into())))
			.saturating_add(RocksDbWeight::get().writes(1))
			.saturating_add(RocksDbWeight::get().writes((1_u64).saturating_mul(n.into())))
			.saturating_add(Weight::from_parts(0, 2603).saturating_mul(n.into()))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::ActionSchedule` (r:0 w:2)
	/// Proof: `Airdrop::ActionSchedule` (`max_values`: None, `max_size`: Some(40), added: 2515,
	/// mode: `MaxEncodedLen`)
	fn close_drawing() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `411`
		//  Estimated: `4232`
		// Minimum execution time: 9_525_000 picoseconds.
		Weight::from_parts(10_386_000, 0)
			.saturating_add(Weight::from_parts(0, 4232))
			.saturating_add(RocksDbWeight::get().reads(1))
			.saturating_add(RocksDbWeight::get().writes(3))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`)
	fn close_claiming() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `379`
		//  Estimated: `4232`
		// Minimum execution time: 7_712_000 picoseconds.
		Weight::from_parts(8_523_000, 0)
			.saturating_add(Weight::from_parts(0, 4232))
			.saturating_add(RocksDbWeight::get().reads(1))
			.saturating_add(RocksDbWeight::get().writes(1))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::Registrations` (r:101 w:100)
	/// Proof: `Airdrop::Registrations` (`max_values`: None, `max_size`: Some(105), added: 2580,
	/// mode: `MaxEncodedLen`) The range of component `n` is `[1, 100]`.
	fn clean_up_registrations(n: u32) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `513 + n * (38 ±0)`
		//  Estimated: `4232 + n * (2580 ±0)`
		// Minimum execution time: 9_375_000 picoseconds.
		Weight::from_parts(9_177_018, 0)
			.saturating_add(Weight::from_parts(0, 4232))
			// Standard Error: 255
			.saturating_add(Weight::from_parts(437_758, 0).saturating_mul(n.into()))
			.saturating_add(RocksDbWeight::get().reads(2))
			.saturating_add(RocksDbWeight::get().reads((1_u64).saturating_mul(n.into())))
			.saturating_add(RocksDbWeight::get().writes(1))
			.saturating_add(RocksDbWeight::get().writes((1_u64).saturating_mul(n.into())))
			.saturating_add(Weight::from_parts(0, 2580).saturating_mul(n.into()))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Airdrop::Winners` (r:101 w:100)
	/// Proof: `Airdrop::Winners` (`max_values`: None, `max_size`: Some(121), added: 2596, mode:
	/// `MaxEncodedLen`) The range of component `n` is `[1, 100]`.
	fn clean_up_winners(n: u32) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `549 + n * (87 ±0)`
		//  Estimated: `4232 + n * (2596 ±0)`
		// Minimum execution time: 8_814_000 picoseconds.
		Weight::from_parts(8_791_876, 0)
			.saturating_add(Weight::from_parts(0, 4232))
			// Standard Error: 197
			.saturating_add(Weight::from_parts(574_785, 0).saturating_mul(n.into()))
			.saturating_add(RocksDbWeight::get().reads(2))
			.saturating_add(RocksDbWeight::get().reads((1_u64).saturating_mul(n.into())))
			.saturating_add(RocksDbWeight::get().writes(1))
			.saturating_add(RocksDbWeight::get().writes((1_u64).saturating_mul(n.into())))
			.saturating_add(Weight::from_parts(0, 2596).saturating_mul(n.into()))
	}
	/// Storage: `Airdrop::Events` (r:1 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Asset` (r:1 w:1)
	/// Proof: `Assets::Asset` (`max_values`: None, `max_size`: Some(808), added: 3283, mode:
	/// `MaxEncodedLen`) Storage: `Assets::Account` (r:1 w:1)
	/// Proof: `Assets::Account` (`max_values`: None, `max_size`: Some(732), added: 3207, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::Holds` (r:1 w:1)
	/// Proof: `AssetsHolder::Holds` (`max_values`: None, `max_size`: Some(847), added: 3322, mode:
	/// `MaxEncodedLen`) Storage: `AssetsHolder::BalancesOnHold` (r:1 w:1)
	/// Proof: `AssetsHolder::BalancesOnHold` (`max_values`: None, `max_size`: Some(682), added:
	/// 3157, mode: `MaxEncodedLen`) Storage: `Airdrop::ActionSchedule` (r:0 w:1)
	/// Proof: `Airdrop::ActionSchedule` (`max_values`: None, `max_size`: Some(40), added: 2515,
	/// mode: `MaxEncodedLen`) Storage: `Airdrop::EventEntropy` (r:0 w:1)
	/// Proof: `Airdrop::EventEntropy` (`max_values`: None, `max_size`: Some(72), added: 2547,
	/// mode: `MaxEncodedLen`)
	fn finalize() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `1044`
		//  Estimated: `4312`
		// Minimum execution time: 35_484_000 picoseconds.
		Weight::from_parts(37_187_000, 0)
			.saturating_add(Weight::from_parts(0, 4312))
			.saturating_add(RocksDbWeight::get().reads(5))
			.saturating_add(RocksDbWeight::get().writes(7))
	}
	/// Storage: `Airdrop::Events` (r:0 w:1)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`)
	fn transition_clean_up_phase() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0`
		//  Estimated: `0`
		// Minimum execution time: 2_543_000 picoseconds.
		Weight::from_parts(2_865_000, 0)
			.saturating_add(Weight::from_parts(0, 0))
			.saturating_add(RocksDbWeight::get().writes(1))
	}
	/// Storage: `Airdrop::Events` (r:1 w:0)
	/// Proof: `Airdrop::Events` (`max_values`: None, `max_size`: Some(767), added: 3242, mode:
	/// `MaxEncodedLen`)
	fn authorize_lifecycle_call() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `411`
		//  Estimated: `4232`
		// Minimum execution time: 4_166_000 picoseconds.
		Weight::from_parts(4_577_000, 0)
			.saturating_add(Weight::from_parts(0, 4232))
			.saturating_add(RocksDbWeight::get().reads(1))
	}
}
