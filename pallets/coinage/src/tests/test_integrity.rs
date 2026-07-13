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

use crate::mock::*;
use frame_support::traits::Hooks;

/// Verify that the default mock configuration passes all integrity checks.
#[test]
fn integrity_test_passes() {
	new_test_ext().execute_with(|| {
		<crate::Pallet<Test> as Hooks<u64>>::integrity_test();
	});
}

/// Verify that `UnderlyingAssetUnit = 0` is rejected by the integrity test.
#[test]
#[should_panic(expected = "UnderlyingAssetUnit must be greater than zero")]
fn integrity_test_rejects_zero_underlying_asset_unit() {
	new_test_ext().execute_with(|| {
		TestUnderlyingAssetUnit::set(&0u64);
		<crate::Pallet<Test> as Hooks<u64>>::integrity_test();
	});
}
