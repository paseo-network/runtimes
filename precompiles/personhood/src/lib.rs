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

//! Pallet-revive precompile for proof of personhood status verification.
//!
//! Exposes `personhoodStatus(address, bytes32) -> PersonhoodInfo` that queries a
//! [`PersonhoodLookup`] implementation to return the personhood tier and context alias
//! of a given account.
//!
//! # Precompile Address
//!
//! Fixed at `0x000000000000000000000000000000000a010000`
//!
//! # Solidity Usage
//!
//! ```solidity
//! IPersonhood personhood = IPersonhood(0x000000000000000000000000000000000A010000);
//! IPersonhood.PersonhoodInfo memory info = personhood.personhoodStatus(addr, bytes32("dotns"));
//! if (info.status == 2) { /* full person */ }
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use core::marker::PhantomData;

use codec::{Decode, MaxEncodedLen};
use pallet_revive::{
	precompiles::{
		alloy::{self, sol_types::SolCall},
		AddressMapper, AddressMatcher, Error, Ext, H160,
	},
	sp_runtime::Weight,
};

use indiv_support::traits::{Context, PersonhoodLookup, PEOPLE_IDENTIFIER, PEOPLE_LITE_IDENTIFIER};

#[cfg(test)]
mod integration_tests;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

alloy::sol!("sol/IPersonhood.sol");

const LOG_TARGET: &str = "runtime::personhood-precompile";

pub const DEFAULT_CONTEXT_ALIAS: [u8; 32] = [0u8; 32];
pub const LITE_STATUS: u8 = 1;
pub const FULL_STATUS: u8 = 2;
pub const NO_STATUS: u8 = 0;

/// Fixed address index for this precompile.
///
/// `pallet-revive` left-shifts user-defined `AddressMatcher::Fixed(u16)` values by 16 bits
/// when placing them in the H160 suffix, so this resolves to the full address
/// `0x000000000000000000000000000000000A010000`. User-precompile addresses are structurally
/// disjoint from `pallet-revive`'s built-in precompiles (`0x1`–`0x901`), which occupy a
/// different range of the suffix bytes; no collision is possible by construction.
pub const PERSONHOOD_ADDRESS_INDEX: u16 = 0x0A01;
/// Configuration trait for the personhood precompile.
pub trait Config: frame_system::Config + pallet_revive::Config {
	/// Membership proof type accepted by the personhood resolver.
	///
	/// `MaxEncodedLen` is required so the precompile can reject oversized calldata
	/// before SCALE-decoding.
	type Proof: Decode + MaxEncodedLen;
	/// The personhood lookup implementation.
	type PersonhoodResolver: PersonhoodLookup<
		<Self as frame_system::Config>::AccountId,
		Self::Proof,
	>;
}

/// Maps a collection [`Identifier`] to the corresponding status byte.
///
/// Only [`PEOPLE_IDENTIFIER`] and [`PEOPLE_LITE_IDENTIFIER`] are valid; receiving any other
/// identifier indicates a misalignment between this precompile and [`PersonhoodLookup`].
/// We log an error for visibility but do not revert — this is a view call and callers
/// receive `NO_STATUS` instead of an error variant.
fn collection_to_status(collection: &[u8; 32]) -> u8 {
	if collection == PEOPLE_IDENTIFIER {
		FULL_STATUS
	} else if collection == PEOPLE_LITE_IDENTIFIER {
		LITE_STATUS
	} else {
		log::error!(
			target: LOG_TARGET,
			"unknown collection identifier from PersonhoodLookup: {collection:?}",
		);
		NO_STATUS
	}
}

/// Precompile exposing proof of personhood status verification.
pub struct PersonhoodCheck<T>(PhantomData<T>);

impl<T> pallet_revive::precompiles::Precompile for PersonhoodCheck<T>
where
	T: Config,
{
	type T = T;
	type Interface = IPersonhood::IPersonhoodCalls;
	const MATCHER: AddressMatcher =
		AddressMatcher::Fixed(core::num::NonZero::new(PERSONHOOD_ADDRESS_INDEX).unwrap());
	const HAS_CONTRACT_INFO: bool = false;

	fn call(
		_address: &[u8; 20],
		input: &Self::Interface,
		env: &mut impl Ext<T = Self::T>,
	) -> Result<Vec<u8>, Error> {
		match input {
			IPersonhood::IPersonhoodCalls::personhoodStatus(call) => {
				let charged = env.charge(T::PersonhoodResolver::personhood_info_weight())?;

				let account = H160(call.account.into_array());
				let account_id =
					<T as pallet_revive::Config>::AddressMapper::to_account_id(&account);
				let context: Context = call.context.0;

				let (result, actual_weight) =
					T::PersonhoodResolver::personhood_info(&account_id, &context);
				env.adjust_gas(charged, actual_weight);

				let info = match result {
					None => IPersonhood::PersonhoodInfo {
						status: NO_STATUS,
						contextAlias: DEFAULT_CONTEXT_ALIAS.into(),
					},
					Some((collection, alias)) => {
						let status = collection_to_status(&collection);
						// For consistency if the user has no status we return the default context
						// alias, even if they have an alias for an unknown collection
						let final_alias =
							if status == NO_STATUS { DEFAULT_CONTEXT_ALIAS } else { alias };
						IPersonhood::PersonhoodInfo { status, contextAlias: final_alias.into() }
					},
				};

				Ok(IPersonhood::personhoodStatusCall::abi_encode_returns(&info))
			},
			IPersonhood::IPersonhoodCalls::personhoodInfoByProof(call) => {
				let charged =
					env.charge(T::PersonhoodResolver::personhood_info_by_proof_weight())?;

				let identifier = match call.request.expectedStatus {
					FULL_STATUS => *PEOPLE_IDENTIFIER,
					LITE_STATUS => *PEOPLE_LITE_IDENTIFIER,
					other => {
						log::error!(
							target: LOG_TARGET,
							"personhoodInfoByProof: unsupported expectedStatus {other}",
						);
						env.adjust_gas(charged, Weight::zero());
						return Ok(IPersonhood::personhoodInfoByProofCall::abi_encode_returns(
							&false,
						));
					},
				};

				// Rejecting oversized proof calldata before SCALE-decode so attackers
				// cannot force unbounded decode work by submitting arbitrarily large payloads. The
				// bound is the configured proof type's max encoded length.
				let max_proof_len = <T::Proof as MaxEncodedLen>::max_encoded_len();
				if call.request.proof.len() > max_proof_len {
					log::error!(
						target: LOG_TARGET,
						"personhoodInfoByProof: proof bytes ({}) exceed max-encoded bound ({})",
						call.request.proof.len(),
						max_proof_len,
					);
					env.adjust_gas(charged, Weight::zero());
					return Ok(IPersonhood::personhoodInfoByProofCall::abi_encode_returns(&false));
				}

				let proof = match T::Proof::decode(&mut &call.request.proof[..]) {
					Ok(p) => p,
					Err(_) => {
						log::error!(
							target: LOG_TARGET,
							"personhoodInfoByProof: failed to SCALE-decode proof bytes",
						);
						return Ok(IPersonhood::personhoodInfoByProofCall::abi_encode_returns(
							&false,
						));
					},
				};

				let request = indiv_support::traits::PersonhoodProofRequest {
					identifier,
					proof,
					alias: call.request.expectedAlias.0,
					ring_index: call.request.ringIndex,
					context: call.request.context.0,
					revision: call.request.revision,
					message: &call.request.message,
				};

				let (matched, actual_weight) =
					T::PersonhoodResolver::personhood_info_by_proof(request);
				env.adjust_gas(charged, actual_weight);

				Ok(IPersonhood::personhoodInfoByProofCall::abi_encode_returns(&matched))
			},
		}
	}
}
