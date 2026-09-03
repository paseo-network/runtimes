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

//! Tests for `coin_lifecycle_weight` function.

use crate::{mock::*, AppendOnlyMembersWeightInfo, Config, Pallet, WeightInfo};
use frame_support::{traits::Get, weights::Weight};

/// Calculates the worst case weight for a coin's lifecycle.
///
/// This mirrors the logic in `coin_lifecycle_weight` but uses maximum values
/// instead of averages for all variable parameters.
fn worst_case_lifecycle_weight<T: Config>() -> Weight {
	let max_aliases = T::MaxConsolidation::get().max(1);
	let max_ring_capacity = T::RecyclerRingExponent::get().ring_capacity();
	let max_aliases_single_ring = max_aliases.min(max_ring_capacity);

	let max_split_outputs = T::MaxSplitOutputs::get();
	let max_age = u32::from(T::MaximumAge::get());

	// Per-key background cost from MemberService
	let bg_per_key = T::MemberService::add_member_background_weight();

	// Phase 1: Fee payment (same as average case - no variable component)
	let pay_fee = T::WeightInfo::pay_for_recycler_unload_fee_token_with_coin()
		.max(T::WeightInfo::pay_for_recycler_unload_fee_token_with_native())
		.max(T::WeightInfo::pay_for_recycler_unload_fee_token_with_external_asset());

	// Background operation: 1 key insertion for paid token
	let bg_paid_token = bg_per_key;

	// Phase 2: Loading - worst case uses max consolidation. On a sponsored instance each load
	// call also charges the load deposit.
	let load_one = T::WeightInfo::load_recycler_with_coin()
		.max(T::WeightInfo::load_recycler_with_external_asset())
		.saturating_add(T::WeightInfo::charge_load_deposit());
	let load_worst = load_one.saturating_mul(max_aliases_single_ring.into());

	// Background operation: 1 key insertion per coin loaded (worst case)
	let bg_recycler = bg_per_key.saturating_mul(max_aliases_single_ring.into());

	// Phase 3: Unloading - worst case uses max values
	let unload_worst = Pallet::<T>::unload_recycler_into_coin_weight(
		max_aliases_single_ring as usize,
	)
	.max(Pallet::<T>::unload_recycler_into_external_asset_and_loaded_coins_prepaid_weight(
		max_aliases_single_ring as usize,
		max_split_outputs as usize,
	))
	.max(Pallet::<T>::unload_recycler_into_external_asset_and_loaded_coins_from_output_weight(
		max_aliases_single_ring as usize,
		max_split_outputs as usize,
	))
	.max(Pallet::<T>::unload_recycler_into_external_asset_prepaid_weight(
		max_aliases_single_ring as usize,
	))
	.max(Pallet::<T>::unload_recycler_into_external_asset_from_output_weight(
		max_aliases_single_ring as usize,
	))
	.max(Pallet::<T>::unload_recycler_into_external_asset_non_anonymous_weight(
		max_aliases_single_ring as usize,
	))
	.max(Pallet::<T>::unload_recycler_into_coins_from_output_weight(
		max_aliases_single_ring as usize,
		max_split_outputs,
	))
	.max(Pallet::<T>::unload_recycler_into_coins_prepaid_weight(
		max_aliases_single_ring as usize,
		max_split_outputs,
	))
	// On a sponsored instance the unload settles the load deposits, and the variants producing
	// loaded coins additionally charge deposits for their fresh keys.
	.saturating_add(T::WeightInfo::settle_load_deposits())
	.saturating_add(T::WeightInfo::charge_load_deposit());

	// Phase 4: Transfers/Splits - worst case uses max age
	let tx_split_max = T::WeightInfo::transfer().max(T::WeightInfo::split(max_split_outputs));
	let tx_split_worst = tx_split_max.saturating_mul(max_age.into());

	pay_fee
		.saturating_add(bg_paid_token)
		.saturating_add(load_worst)
		.saturating_add(bg_recycler)
		.saturating_add(unload_worst)
		.saturating_add(tx_split_worst)
}

#[test]
fn average_weight_is_reasonable_compared_to_worst_case() {
	new_test_ext().execute_with(|| {
		let avg_case = Pallet::<Test>::coin_lifecycle_weight();
		let worst_case = worst_case_lifecycle_weight::<Test>();

		// The average case should be at least 1/10th of the worst case.
		// If average is too low compared to worst case, fees would be drastically
		// underestimated for heavy users, leading to potential economic attacks.
		let min_reasonable = worst_case.saturating_div(10);

		assert!(
			avg_case.all_gte(min_reasonable),
			"Average case weight ({avg_case:?}) is less than 1/10th of worst case ({worst_case:?}). \
			 Minimum reasonable: {min_reasonable:?}. \
			 This indicates the weight formula may need adjustment."
		);
	});
}

#[test]
fn average_weight_is_less_than_worst_case() {
	new_test_ext().execute_with(|| {
		let avg_case = Pallet::<Test>::coin_lifecycle_weight();
		let worst_case = worst_case_lifecycle_weight::<Test>();

		// Sanity check: average should be less than or equal to worst case.
		// If average exceeds worst case, something is wrong with the formula.
		assert!(
			worst_case.all_gte(avg_case),
			"Average case weight ({avg_case:?}) exceeds worst case ({worst_case:?}). \
			 This should never happen - check the weight formula."
		);
	});
}

#[test]
fn coin_lifecycle_weight_is_within_expected_range() {
	new_test_ext().execute_with(|| {
		let weight = Pallet::<Test>::coin_lifecycle_weight();

		// Expected weight is approximately:
		// ref_time: ~122ms, proof_size: ~195 KB
		// We allow a range of value/2 to value*2 to tolerate weight changes.
		let expected_ref_time = 122_000_000_000u64; // 122 ms
		let expected_proof_size = 195_000u64; // 195 KB

		let min_ref_time = expected_ref_time / 2;
		let max_ref_time = expected_ref_time * 2;
		let min_proof_size = expected_proof_size / 2;
		let max_proof_size = expected_proof_size * 2;

		assert!(
			weight.ref_time() >= min_ref_time && weight.ref_time() <= max_ref_time,
			"ref_time {} is outside expected range [{}, {}]",
			weight.ref_time(),
			min_ref_time,
			max_ref_time
		);

		assert!(
			weight.proof_size() >= min_proof_size && weight.proof_size() <= max_proof_size,
			"proof_size {} is outside expected range [{}, {}]",
			weight.proof_size(),
			min_proof_size,
			max_proof_size
		);
	});
}
