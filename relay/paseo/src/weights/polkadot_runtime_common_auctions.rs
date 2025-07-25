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

//! Autogenerated weights for `polkadot_runtime_common::auctions`
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

/// Weight functions for `paseo_runtime_common :: auctions`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> polkadot_runtime_common::auctions::WeightInfo for WeightInfo<T> {
	/// Storage: `Auctions::AuctionInfo` (r:1 w:1)
	/// Proof: `Auctions::AuctionInfo` (`max_values`: Some(1), `max_size`: Some(8), added: 503, mode: `MaxEncodedLen`)
	/// Storage: `Auctions::AuctionCounter` (r:1 w:1)
	/// Proof: `Auctions::AuctionCounter` (`max_values`: Some(1), `max_size`: Some(4), added: 499, mode: `MaxEncodedLen`)
	fn new_auction() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `4`
		//  Estimated: `1493`
		// Minimum execution time: 13_030_000 picoseconds.
		Weight::from_parts(13_360_000, 0)
			.saturating_add(Weight::from_parts(0, 1493))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: `Paras::ParaLifecycles` (r:1 w:0)
	/// Proof: `Paras::ParaLifecycles` (`max_values`: None, `max_size`: None, mode: `Measured`)
	/// Storage: `Auctions::AuctionCounter` (r:1 w:0)
	/// Proof: `Auctions::AuctionCounter` (`max_values`: Some(1), `max_size`: Some(4), added: 499, mode: `MaxEncodedLen`)
	/// Storage: `Auctions::AuctionInfo` (r:1 w:0)
	/// Proof: `Auctions::AuctionInfo` (`max_values`: Some(1), `max_size`: Some(8), added: 503, mode: `MaxEncodedLen`)
	/// Storage: `Slots::Leases` (r:1 w:0)
	/// Proof: `Slots::Leases` (`max_values`: None, `max_size`: None, mode: `Measured`)
	/// Storage: `Auctions::Winning` (r:1 w:1)
	/// Proof: `Auctions::Winning` (`max_values`: None, `max_size`: Some(1920), added: 4395, mode: `MaxEncodedLen`)
	/// Storage: `Auctions::ReservedAmounts` (r:2 w:2)
	/// Proof: `Auctions::ReservedAmounts` (`max_values`: None, `max_size`: Some(60), added: 2535, mode: `MaxEncodedLen`)
	/// Storage: `System::Account` (r:1 w:1)
	/// Proof: `System::Account` (`max_values`: None, `max_size`: Some(128), added: 2603, mode: `MaxEncodedLen`)
	fn bid() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `695`
		//  Estimated: `6060`
		// Minimum execution time: 126_870_000 picoseconds.
		Weight::from_parts(129_801_000, 0)
			.saturating_add(Weight::from_parts(0, 6060))
			.saturating_add(T::DbWeight::get().reads(8))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	/// Storage: `Auctions::AuctionInfo` (r:1 w:1)
	/// Proof: `Auctions::AuctionInfo` (`max_values`: Some(1), `max_size`: Some(8), added: 503, mode: `MaxEncodedLen`)
	/// Storage: `Babe::NextRandomness` (r:1 w:0)
	/// Proof: `Babe::NextRandomness` (`max_values`: Some(1), `max_size`: Some(32), added: 527, mode: `MaxEncodedLen`)
	/// Storage: `Babe::EpochStart` (r:1 w:0)
	/// Proof: `Babe::EpochStart` (`max_values`: Some(1), `max_size`: Some(8), added: 503, mode: `MaxEncodedLen`)
	/// Storage: `Auctions::AuctionCounter` (r:1 w:0)
	/// Proof: `Auctions::AuctionCounter` (`max_values`: Some(1), `max_size`: Some(4), added: 499, mode: `MaxEncodedLen`)
	/// Storage: `Auctions::Winning` (r:3600 w:3600)
	/// Proof: `Auctions::Winning` (`max_values`: None, `max_size`: Some(1920), added: 4395, mode: `MaxEncodedLen`)
	/// Storage: `Auctions::ReservedAmounts` (r:37 w:36)
	/// Proof: `Auctions::ReservedAmounts` (`max_values`: None, `max_size`: Some(60), added: 2535, mode: `MaxEncodedLen`)
	/// Storage: `System::Account` (r:36 w:36)
	/// Proof: `System::Account` (`max_values`: None, `max_size`: Some(128), added: 2603, mode: `MaxEncodedLen`)
	/// Storage: `Slots::Leases` (r:7 w:7)
	/// Proof: `Slots::Leases` (`max_values`: None, `max_size`: None, mode: `Measured`)
	/// Storage: `Paras::ParaLifecycles` (r:1 w:1)
	/// Proof: `Paras::ParaLifecycles` (`max_values`: None, `max_size`: None, mode: `Measured`)
	/// Storage: `ParasShared::CurrentSessionIndex` (r:1 w:0)
	/// Proof: `ParasShared::CurrentSessionIndex` (`max_values`: Some(1), `max_size`: None, mode: `Measured`)
	/// Storage: `Paras::ActionsQueue` (r:1 w:1)
	/// Proof: `Paras::ActionsQueue` (`max_values`: None, `max_size`: None, mode: `Measured`)
	fn on_initialize() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `6946951`
		//  Estimated: `15822990`
		// Minimum execution time: 11_356_863_000 picoseconds.
		Weight::from_parts(11_708_724_000, 0)
			.saturating_add(Weight::from_parts(0, 15822990))
			.saturating_add(T::DbWeight::get().reads(3687))
			.saturating_add(T::DbWeight::get().writes(3682))
	}
	/// Storage: `Auctions::ReservedAmounts` (r:37 w:36)
	/// Proof: `Auctions::ReservedAmounts` (`max_values`: None, `max_size`: Some(60), added: 2535, mode: `MaxEncodedLen`)
	/// Storage: `System::Account` (r:36 w:36)
	/// Proof: `System::Account` (`max_values`: None, `max_size`: Some(128), added: 2603, mode: `MaxEncodedLen`)
	/// Storage: `Auctions::Winning` (r:3600 w:3600)
	/// Proof: `Auctions::Winning` (`max_values`: None, `max_size`: Some(1920), added: 4395, mode: `MaxEncodedLen`)
	/// Storage: `Auctions::AuctionInfo` (r:0 w:1)
	/// Proof: `Auctions::AuctionInfo` (`max_values`: Some(1), `max_size`: Some(8), added: 503, mode: `MaxEncodedLen`)
	fn cancel_auction() -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `177732`
		//  Estimated: `15822990`
		// Minimum execution time: 9_413_267_000 picoseconds.
		Weight::from_parts(9_533_138_000, 0)
			.saturating_add(Weight::from_parts(0, 15822990))
			.saturating_add(T::DbWeight::get().reads(3673))
			.saturating_add(T::DbWeight::get().writes(3673))
	}
}
