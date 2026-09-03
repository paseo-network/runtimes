// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Individuality.
// SPDX-License-Identifier: Apache-2.0
//
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

use frame_support::{derive_impl, parameter_types};
use indiv_support::context::ProductContextNetworkSuffix;
use sp_runtime::BuildStorage;

pub type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
	pub enum Test {
		System: frame_system,
		NetworkSuffix: crate,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Block = Block;
}

parameter_types! {
	pub DefaultNetworkSuffix: ProductContextNetworkSuffix =
		b"paseo".to_vec().try_into().expect("default suffix fits");
}

impl crate::Config for Test {
	type UpdateOrigin = frame_system::EnsureRoot<Self::AccountId>;
	type DefaultSuffix = DefaultNetworkSuffix;
	type WeightInfo = ();
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	let mut ext: sp_io::TestExternalities =
		RuntimeGenesisConfig::default().build_storage().unwrap().into();
	ext.execute_with(|| System::set_block_number(1));
	ext
}

pub fn new_test_ext_with_suffix(suffix: &[u8]) -> sp_io::TestExternalities {
	let mut config = RuntimeGenesisConfig::default();
	config.network_suffix.network_suffix = suffix.to_vec().try_into().unwrap();
	let mut ext: sp_io::TestExternalities = config.build_storage().unwrap().into();
	ext.execute_with(|| System::set_block_number(1));
	ext
}
