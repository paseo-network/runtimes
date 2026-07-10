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

//! Asset Hub leaf-call classifier for value transfers.
//!
//! Nested dispatch (Utility/Proxy/Multisig/as_derivative/XCM Transact) is gated by the
//! runtime's `BaseCallFilter` via `BlockValueTransfersWhenFlagSet`, so this matcher does NOT
//! recurse.

use crate::{Runtime, RuntimeCall};
use frame_support::traits::Contains;

pub struct AhValueTransferFilter;

impl Contains<RuntimeCall> for AhValueTransferFilter {
	fn contains(call: &RuntimeCall) -> bool {
		match call {
			RuntimeCall::Assets(inner) => assets_call_targets_protected_asset(inner),

			RuntimeCall::AssetConversion(inner) =>
				asset_conversion_call_touches_protected_asset(inner),

			_ => false,
		}
	}
}

fn assets_call_targets_protected_asset(
	call: &pallet_assets::Call<Runtime, pallet_assets::Instance1>,
) -> bool {
	let protected_asset = paseo_runtime_constants::ProtectedAssetId::get();
	match call {
		pallet_assets::Call::transfer { id, .. } |
		pallet_assets::Call::transfer_keep_alive { id, .. } |
		pallet_assets::Call::transfer_all { id, .. } |
		pallet_assets::Call::burn { id, .. } |
		pallet_assets::Call::transfer_approved { id, .. } |
		pallet_assets::Call::approve_transfer { id, .. } => id.0 == protected_asset,
		_ => false,
	}
}

fn asset_conversion_call_touches_protected_asset(
	call: &pallet_asset_conversion::Call<Runtime>,
) -> bool {
	let protected_asset = paseo_runtime_constants::ProtectedAssetLocation::get();
	match call {
		pallet_asset_conversion::Call::add_liquidity { asset1, asset2, .. } |
		pallet_asset_conversion::Call::remove_liquidity { asset1, asset2, .. } =>
			**asset1 == protected_asset || **asset2 == protected_asset,
		pallet_asset_conversion::Call::swap_exact_tokens_for_tokens { path, .. } |
		pallet_asset_conversion::Call::swap_tokens_for_exact_tokens { path, .. } =>
			path.iter().any(|a| **a == protected_asset),
		_ => false,
	}
}
