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

//! A list of the different weight modules for our runtime.

pub mod frame_election_provider_support;
pub mod frame_system;
pub mod frame_system_extensions;
pub mod pallet_asset_rate;
pub mod pallet_bags_list;
pub mod pallet_balances;
pub mod pallet_beefy_mmr;
pub mod pallet_bounties;
pub mod pallet_child_bounties;
pub mod pallet_conviction_voting;
pub mod pallet_election_provider_multi_phase;
pub mod pallet_fast_unstake;
pub mod pallet_indices;
pub mod pallet_message_queue;
pub mod pallet_multisig;
pub mod pallet_nomination_pools;
pub mod pallet_preimage;
pub mod pallet_proxy;
pub mod pallet_referenda;
pub mod pallet_scheduler;
pub mod pallet_session;
pub mod pallet_staking;
pub mod pallet_sudo;
pub mod pallet_timestamp;
pub mod pallet_transaction_payment;
pub mod pallet_treasury;
pub mod pallet_utility;
pub mod pallet_vesting;
pub mod pallet_whitelist;
pub mod pallet_xcm;
pub mod polkadot_runtime_common_auctions;
pub mod polkadot_runtime_common_claims;
pub mod polkadot_runtime_common_crowdloan;
pub mod polkadot_runtime_common_paras_registrar;
pub mod polkadot_runtime_common_slots;
pub mod runtime_parachains_configuration;
pub mod runtime_parachains_coretime;
pub mod runtime_parachains_disputes;
pub mod runtime_parachains_disputes_slashing;
pub mod runtime_parachains_hrmp;
pub mod runtime_parachains_inclusion;
pub mod runtime_parachains_initializer;
pub mod runtime_parachains_on_demand;
pub mod runtime_parachains_paras;
pub mod runtime_parachains_paras_inherent;
pub mod xcm;
