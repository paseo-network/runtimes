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

//! `RestrictProtectedAssetErc20` — wraps an `ERC20` precompile and rejects every call targeting a
//! protected-asset id while the global value-transfer block flag is set.
//!
//! Metadata calls (`name`, `symbol`, `decimals`) are always allowed so wallets and indexers can
//! still render the token. Every other entry point (`balanceOf`, `totalSupply`, `allowance`,
//! `transfer`, `transferFrom`, `approve`, `permit`, `nonces`, `DOMAIN_SEPARATOR`) is rejected
//! when the block flag is set and the target is a protected asset.
//!
//! The wrapper reuses the inner precompile's `AssetIdExtractor` so the address-decoding scheme
//! stays in sync with the wrapped `ERC20` automatically.

use alloc::vec::Vec;
use core::marker::PhantomData;
use ethereum_standards::IERC20::IERC20Calls;
use indiv_pallet_value_transfer_auth::extension::block_flag;
use pallet_assets_precompiles::{AssetIdExtractor, AssetPrecompileConfig, ERC20};
use pallet_revive::precompiles::{
	alloy::sol_types::Revert, AddressMatcher, Error, Ext, Precompile,
};
use paseo_runtime_constants::PROTECTED_ASSET_ID;

/// `true` if the call is purely a metadata read that should remain allowed.
fn is_metadata_call(input: &IERC20Calls) -> bool {
	matches!(input, IERC20Calls::name(_) | IERC20Calls::symbol(_) | IERC20Calls::decimals(_))
}

/// `ERC20` precompile wrapper that gates access to a protected asset on the global block flag.
pub struct RestrictProtectedAssetErc20<Runtime, PrecompileConfig, Instance = ()>(
	PhantomData<(Runtime, PrecompileConfig, Instance)>,
);

impl<Runtime, PrecompileConfig, Instance: 'static> Precompile
	for RestrictProtectedAssetErc20<Runtime, PrecompileConfig, Instance>
where
	PrecompileConfig: AssetPrecompileConfig,
	<PrecompileConfig::AssetIdExtractor as AssetIdExtractor>::AssetId: PartialEq<u32>,
	ERC20<Runtime, PrecompileConfig, Instance>: Precompile<Interface = IERC20Calls>,
{
	type T = <ERC20<Runtime, PrecompileConfig, Instance> as Precompile>::T;
	type Interface = IERC20Calls;
	const MATCHER: AddressMatcher =
		<ERC20<Runtime, PrecompileConfig, Instance> as Precompile>::MATCHER;
	const HAS_CONTRACT_INFO: bool =
		<ERC20<Runtime, PrecompileConfig, Instance> as Precompile>::HAS_CONTRACT_INFO;

	fn call(
		address: &[u8; 20],
		input: &Self::Interface,
		env: &mut impl Ext<T = Self::T>,
	) -> Result<Vec<u8>, Error> {
		let asset_id = PrecompileConfig::AssetIdExtractor::asset_id_from_address(address)?;

		if asset_id == PROTECTED_ASSET_ID && block_flag::is_blocked() && !is_metadata_call(input) {
			return Err(Error::Revert(Revert {
				reason: "Protected asset access requires value-transfer authorization".into(),
			}));
		}

		ERC20::<Runtime, PrecompileConfig, Instance>::call(address, input, env)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use ethereum_standards::IERC20;

	#[test]
	fn is_metadata_call_accepts_only_name_symbol_decimals() {
		assert!(is_metadata_call(&IERC20Calls::name(IERC20::nameCall {})));
		assert!(is_metadata_call(&IERC20Calls::symbol(IERC20::symbolCall {})));
		assert!(is_metadata_call(&IERC20Calls::decimals(IERC20::decimalsCall {})));

		assert!(!is_metadata_call(&IERC20Calls::totalSupply(IERC20::totalSupplyCall {})));
		assert!(!is_metadata_call(&IERC20Calls::balanceOf(IERC20::balanceOfCall {
			account: Default::default(),
		})));
		assert!(!is_metadata_call(&IERC20Calls::allowance(IERC20::allowanceCall {
			owner: Default::default(),
			spender: Default::default(),
		})));
		assert!(!is_metadata_call(&IERC20Calls::nonces(IERC20::noncesCall {
			owner: Default::default(),
		})));
		assert!(!is_metadata_call(&IERC20Calls::DOMAIN_SEPARATOR(IERC20::DOMAIN_SEPARATORCall {})));
		assert!(!is_metadata_call(&IERC20Calls::transfer(IERC20::transferCall {
			to: Default::default(),
			value: Default::default(),
		})));
		assert!(!is_metadata_call(&IERC20Calls::transferFrom(IERC20::transferFromCall {
			from: Default::default(),
			to: Default::default(),
			value: Default::default(),
		})));
		assert!(!is_metadata_call(&IERC20Calls::approve(IERC20::approveCall {
			spender: Default::default(),
			value: Default::default(),
		})));
		assert!(!is_metadata_call(&IERC20Calls::permit(IERC20::permitCall {
			owner: Default::default(),
			spender: Default::default(),
			value: Default::default(),
			deadline: Default::default(),
			v: Default::default(),
			r: Default::default(),
			s: Default::default(),
		})));
	}
}
