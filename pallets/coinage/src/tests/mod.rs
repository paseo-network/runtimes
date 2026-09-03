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

use crate::{
	mock::{MockTime, Test, TEST_INSTANCE_ID},
	*,
};
use frame_support::traits::UnixTime;

mod test_alias_proof_tampering;
mod test_archive_recycler;
mod test_big_endian_period_ordering;
mod test_bounded_vec_decoding;
mod test_coin_lifecycle_weight;
mod test_coinage_paid_full_story;
mod test_create_sufficient_instance;
mod test_direct_offboard_coin_into_external_asset;
mod test_dusting;
mod test_dynamic_parameters;
mod test_extension;
mod test_free_unload_token_lifecycle;
mod test_infallible_unpaid_ext;
mod test_integrity;
mod test_load_recycler;
mod test_load_recycler_with_external_asset;
mod test_minimum_fee_denomination_for_output_unload;
mod test_native_instance;
mod test_paid_ring_lifecycle;
mod test_pay_for_recycler_unload_fee_token_with_coin;
mod test_pay_for_recycler_unload_fee_token_with_external_asset;
mod test_pay_for_recycler_unload_fee_token_with_native;
mod test_recycler_lifecycle;
mod test_recycler_unloaded_count;
mod test_split;
mod test_sponsored_instance;
mod test_transfer;
mod test_unload_archived_recycler;
mod test_unload_recycler_into_coin;
mod test_unload_recycler_into_coins;
mod test_unload_recycler_into_external_asset;
mod test_unload_recycler_into_external_asset_and_loaded_coins;
mod test_unload_recycler_into_external_asset_fee_from_output;
mod test_unload_recycler_into_external_asset_non_anonymous_fee_from_signer;

pub(super) fn get_recycler_alias_lock_until(
	value: Denomination,
	index: RingIndex,
	alias: Alias,
) -> Option<u64> {
	let current_time = MockTime::now().as_secs();
	match RecyclerAliasStates::<Test>::get((TEST_INSTANCE_ID, value, index, alias))? {
		AliasState::Locked(locked) => (current_time < locked.until).then_some(locked.until),
		AliasState::Unloaded => None,
	}
}
