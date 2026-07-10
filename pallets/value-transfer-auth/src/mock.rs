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

use frame_support::{construct_runtime, derive_impl, parameter_types};
use sp_core::{ed25519, Pair};
use sp_runtime::BuildStorage;

pub type Block = frame_system::mocking::MockBlock<Test>;

construct_runtime!(
	pub enum Test {
		System: frame_system,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Block = Block;
}

pub fn test_keypair() -> (ed25519::Pair, ed25519::Public) {
	let pair = ed25519::Pair::from_seed(&[0x42; 32]);
	let public = pair.public();
	(pair, public)
}

parameter_types! {
	pub TestAuthorizationPubkey: ed25519::Public = test_keypair().1;
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	frame_system::GenesisConfig::<Test>::default()
		.build_storage()
		.expect("genesis")
		.into()
}
