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

//! Shared helpers for weight-related integrity checks.

use frame_support::{dispatch::DispatchClass, traits::Get, weights::Weight};

/// Weight budget for offchain-worker-submitted authorized transactions.
///
/// An OCW-submitted authorized transaction whose worst-case weight exceeds
/// `Normal.max_extrinsic` is silently dropped at the transaction-pool level,
/// stalling the flow it drives. Pallet integrity tests assert every such call's
/// worst-case weight fits this budget, which is half of `Normal.max_extrinsic` to
/// leave headroom for transaction extensions and so that a single call does not
/// consume a majority of the block.
pub struct OcwWeightBudget {
	budget: Weight,
}

impl OcwWeightBudget {
	/// Build the budget from the runtime's `Normal.max_extrinsic`.
	pub fn from_normal_max<T: frame_system::Config>() -> Self {
		let normal_max = <T as frame_system::Config>::BlockWeights::get()
			.per_class
			.get(DispatchClass::Normal)
			.max_extrinsic
			.expect("Normal class must have max_extrinsic configured");
		Self { budget: normal_max.saturating_div(2) }
	}

	/// Panic if `weight` does not fit the budget on both weight dimensions.
	pub fn assert_fits(&self, name: &str, weight: Weight) {
		assert!(
			weight.all_lte(self.budget),
			"`{name}` worst-case weight {weight:?} exceeds the OCW budget {budget:?}",
			budget = self.budget,
		);
	}
}
