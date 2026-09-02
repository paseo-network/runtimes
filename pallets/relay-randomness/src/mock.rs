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

//! Mock runtime for testing the relay-randomness pallet.

use cumulus_pallet_parachain_system::{
	consensus_hook::ExpectParentIncluded, AnyRelayNumber, ParachainSetCode, RelaychainDataProvider,
};
use cumulus_primitives_core::{relay_chain, ParaId};
use frame_support::{derive_impl, parameter_types, traits::ConstU32};
use sp_runtime::{traits::BlockNumberProvider, BuildStorage};

pub type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
	pub enum Test
	{
		System: frame_system,
		ParachainSystem: cumulus_pallet_parachain_system,
		RelayRandomness: crate,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Block = Block;
	type OnSetCode = ParachainSetCode<Self>;
}

parameter_types! {
	pub const ParachainId: ParaId = ParaId::new(2000);
}

/// The tests never build a parachain block, they only drive
/// [`OnSystemEvent::on_relay_state_proof`](cumulus_pallet_parachain_system::OnSystemEvent) and
/// read the relay parent number back, so the message handling associated types are no-ops.
impl cumulus_pallet_parachain_system::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type OnSystemEvent = RelayRandomness;
	type SelfParaId = ParachainId;
	type OutboundXcmpMessageSource = ();
	type DmpQueue = ();
	type ReservedDmpWeight = ();
	type XcmpMessageHandler = ();
	type ReservedXcmpWeight = ();
	type CheckAssociatedRelayNumber = AnyRelayNumber;
	type ConsensusHook = ExpectParentIncluded;
	type WeightInfo = ();
	type RelayParentOffset = ConstU32<2>;
	// PASEO DELTA: upstream also binds `type SchedulingSignatureVerifier = ();` here. That
	// associated type does not exist in `cumulus-pallet-parachain-system` 0.28.0, the version
	// this workspace pins; it arrives in a later polkadot-sdk train. Nothing else in the mock
	// or in the pallet depends on it.
}

impl crate::Config for Test {
	type WeightInfo = ();
}

/// Set the relay parent number the pallet reads, as the `set_validation_data` inherent
/// would.
pub fn set_relay_parent_number(number: relay_chain::BlockNumber) {
	RelaychainDataProvider::<Test>::set_block_number(number);
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	RuntimeGenesisConfig::default().build_storage().unwrap().into()
}
