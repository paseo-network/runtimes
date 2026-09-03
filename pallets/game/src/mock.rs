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

//! The game's mock runtime: the runtime shared with the credits pallet, without the credits.
//!
//! Everything but the pallet list and this pallet's own [`Config`] comes from [`runtime`], which
//! `indiv-pallet-nft-credits` includes as well. See its module documentation.

#[path = "mock_runtime.rs"]
mod runtime;

pub use runtime::*;

// `mock_runtime.rs` is written against the game pallet by name, so that the credits mock can
// include the same file; here that name is this crate.
use crate as indiv_pallet_game;

/// This runtime plays games without minting anything from them; the pair is tested in
/// `indiv-pallet-nft-credits`, whose mock builds on the same runtime.
type MockNftClaimCredits = ();

// Configure a mock runtime to test the pallet.
frame_support::construct_runtime!(
	pub enum Test
	{
		System: frame_system,
		ChunksManager: indiv_pallet_chunks_manager,
		Members: indiv_pallet_members,
		Game: crate,
		Score: indiv_pallet_score,
		Balances: pallet_balances,
		Assets: pallet_assets,
		AssetsHolder: pallet_assets_holder,
		Airdrop: indiv_pallet_airdrop,
		People: indiv_pallet_people,
		PeopleLite: indiv_pallet_people_lite,
		Deposit: deposit,
	}
);
