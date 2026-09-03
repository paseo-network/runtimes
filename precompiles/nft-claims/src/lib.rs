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

//! Pallet-revive precompile exposing `pallet-nft-claims` collection-minter registration.
//!
//! [`NftClaimsMinter`] answers at one fixed address and lets a collection's owner, contract
//! or externally owned, register the collection for deposit-free claim minting, pick the
//! [`ItemSelection`], withdraw it again and read the current registration back.
//!
//! This surface lives outside the Scarcity precompile crate on purpose: `pallet-scarcity` is
//! a standalone base layer, and not every runtime that has it also has `pallet-nft-claims`.
//! A runtime wires this precompile in only when it has both pallets.
//!
//! Authorization is the pallet's: every mutator dispatches as the caller, and
//! `set_collection_minter` enforces that the signer is the collection's current Scarcity
//! owner and validates a contract selection through the runtime's `CollectionSelector`.
//!
//! No function of the interface is payable, and a call that attaches value is rejected before
//! anything else. Accepting it would strand it: the precompile address has no code, no owner
//! and no withdrawal path, so value sent there is unrecoverable by anyone. Rejecting is a
//! revert rather than a trap, because attaching value is a caller mistake the caller can
//! correct.
//!
//! Weights: the mutators charge the `set_collection_minter` benchmark weight. The read
//! charges one database read, which over-charges pure computation but never under-charges.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use core::{marker::PhantomData, num::NonZero};

use frame_support::traits::Get;
use frame_system::RawOrigin;
use indiv_pallet_nft_claims::{
	CollectionMinters, Error as NftClaimsError, ItemSelection, Pallet as NftClaims, WeightInfo as _,
};
use pallet_revive::{
	precompiles::{
		alloy::{
			self,
			primitives::Address,
			sol_types::{Revert, SolCall},
		},
		AddressMapper, AddressMatcher, Error, Ext, H160,
	},
	sp_runtime::{DispatchError, Weight},
};
use pallet_scarcity::CollectionId;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

alloy::sol!("sol/INftClaimsMinter.sol");

use INftClaimsMinter::INftClaimsMinterCalls;

/// `collectionMinter` kind: the collection is not registered for claims.
pub const KIND_NONE: u8 = 0;
/// `collectionMinter` kind: claims mint a pseudo-random item.
pub const KIND_RANDOM: u8 = 1;
/// `collectionMinter` kind: the registered minter contract picks the item.
pub const KIND_CONTRACT: u8 = 2;

const ERR_INVALID_CALLER: &str = "invalid caller";
const ERR_VALUE_NOT_ACCEPTED: &str = "this precompile does not accept value";

fn revert(reason: &str) -> Error {
	Error::Revert(Revert { reason: reason.into() })
}

/// Map the `pallet-nft-claims` errors registration can trigger to catchable reverts.
///
/// Every error variant reachable through `set_collection_minter` is listed, so a caller
/// never sees a trapped frame for a condition it could have handled. Adding a variant to the
/// pallet does not break this list at compile time; `mapped_nft_claims_errors_are_exhaustive`
/// in the tests fails instead.
///
/// A rejected contract selection surfaces as the string error the runtime's
/// `CollectionSelector::validate` reports, which is forwarded as the revert reason. Anything
/// else propagates as a plain error, which traps the frame rather than reverting.
fn revert_nft_claims<T: indiv_pallet_nft_claims::Config>(e: DispatchError) -> Error {
	let cases: [(NftClaimsError<T>, &str); 2] = [
		(NftClaimsError::UnknownCollection, "unknown collection"),
		(NftClaimsError::NotCollectionOwner, "caller is not the collection owner"),
	];
	for (error, reason) in cases {
		if e == DispatchError::from(error) {
			return revert(reason);
		}
	}
	match e {
		DispatchError::Other(reason) => revert(reason),
		other => other.into(),
	}
}

/// The signing account behind the EVM caller.
fn caller_account<T: pallet_revive::Config>(
	env: &mut impl Ext<T = T>,
) -> Result<T::AccountId, Error> {
	env.caller().account_id().cloned().map_err(|_| revert(ERR_INVALID_CALLER))
}

/// Reject a call carrying native value.
///
/// Every function of the interface is ABI-`nonpayable`, so a caller attaching value has made
/// a mistake, and the precompile has no way to make good on it: the address it would land on
/// has no owner, no code and no withdrawal path.
fn ensure_no_value<T: pallet_revive::Config>(env: &impl Ext<T = T>) -> Result<(), Error> {
	if env.value_transferred().is_zero() {
		return Ok(());
	}
	Err(revert(ERR_VALUE_NOT_ACCEPTED))
}

fn address_of<T: pallet_revive::Config>(account: &T::AccountId) -> Address {
	Address::from(<T as pallet_revive::Config>::AddressMapper::to_address(account).0)
}

/// Proof size charged per storage read.
///
/// `DbWeight` carries only `ref_time`, but on a parachain every read also pulls trie nodes
/// into the proof. The registration entry is bounded far below this headroom; a crate
/// benchmark can replace the estimate.
const PROOF_SIZE_PER_READ: u64 = 4 * 1024;

/// Charge `n` worst-case database reads before performing them.
fn charge_reads<T: frame_system::Config + pallet_revive::Config>(
	env: &mut impl Ext<T = T>,
	n: u64,
) -> Result<(), Error> {
	let ref_time = <T as frame_system::Config>::DbWeight::get().reads(n).ref_time();
	env.charge(Weight::from_parts(ref_time, n.saturating_mul(PROOF_SIZE_PER_READ)))?;
	Ok(())
}

/// Collection-minter registration for NFT claims at the fixed address index `INDEX`.
///
/// Registers, withdraws and reads back the claim registration of Scarcity collections. The
/// mutators dispatch `set_collection_minter` as the caller, so the pallet's owner gating and
/// contract validation apply unchanged.
pub struct NftClaimsMinter<T, const INDEX: u16>(PhantomData<T>);

impl<T, const INDEX: u16> pallet_revive::precompiles::Precompile for NftClaimsMinter<T, INDEX>
where
	T: indiv_pallet_nft_claims::Config + pallet_revive::Config,
{
	type T = T;
	type Interface = INftClaimsMinterCalls;
	const MATCHER: AddressMatcher = AddressMatcher::Fixed(NonZero::new(INDEX).unwrap());
	const HAS_CONTRACT_INFO: bool = false;

	fn call(
		_address: &[u8; 20],
		input: &Self::Interface,
		env: &mut impl Ext<T = Self::T>,
	) -> Result<Vec<u8>, Error> {
		frame_support::ensure!(
			!env.is_delegate_call(),
			pallet_revive::Error::<T>::PrecompileDelegateDenied
		);
		ensure_no_value(env)?;
		if env.is_read_only() && is_mutating(input) {
			return Err(Error::Error(pallet_revive::Error::<T>::StateChangeDenied.into()));
		}

		match input {
			INftClaimsMinterCalls::setRandomMinter(call) =>
				Self::set_minter(call.collection, Some(ItemSelection::Random), env),
			INftClaimsMinterCalls::setContractMinter(call) => Self::set_minter(
				call.collection,
				Some(ItemSelection::Contract(H160(call.minter.into_array()))),
				env,
			),
			INftClaimsMinterCalls::clearMinter(call) =>
				Self::set_minter(call.collection, None, env),
			INftClaimsMinterCalls::collectionMinter(call) =>
				Self::collection_minter(call.collection, env),
		}
	}
}

/// Exhaustive on purpose: a method added to the interface must be classified here before
/// the crate compiles again.
fn is_mutating(input: &INftClaimsMinterCalls) -> bool {
	match input {
		INftClaimsMinterCalls::setRandomMinter(_) |
		INftClaimsMinterCalls::setContractMinter(_) |
		INftClaimsMinterCalls::clearMinter(_) => true,
		INftClaimsMinterCalls::collectionMinter(_) => false,
	}
}

impl<T, const INDEX: u16> NftClaimsMinter<T, INDEX>
where
	T: indiv_pallet_nft_claims::Config + pallet_revive::Config,
{
	/// Serves the three mutators, mirroring the pallet call's optional selection.
	fn set_minter(
		collection: CollectionId,
		selection: Option<ItemSelection>,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		env.charge(<T as indiv_pallet_nft_claims::Config>::WeightInfo::set_collection_minter())?;
		let who = caller_account::<T>(env)?;
		NftClaims::<T>::set_collection_minter(RawOrigin::Signed(who).into(), collection, selection)
			.map_err(revert_nft_claims::<T>)?;
		Ok(Vec::new())
	}

	fn collection_minter(
		collection: CollectionId,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		charge_reads(env, 1)?;
		let ret = match CollectionMinters::<T>::get(collection) {
			// An unknown collection has no registration either, so it answers the same way.
			None => INftClaimsMinter::collectionMinterReturn {
				kind: KIND_NONE,
				minter: Address::ZERO,
				owner: Address::ZERO,
			},
			Some(minter) => {
				let owner = address_of::<T>(&minter.owner);
				match minter.selection {
					ItemSelection::Random => INftClaimsMinter::collectionMinterReturn {
						kind: KIND_RANDOM,
						minter: Address::ZERO,
						owner,
					},
					ItemSelection::Contract(contract) => INftClaimsMinter::collectionMinterReturn {
						kind: KIND_CONTRACT,
						minter: Address::from(contract.0),
						owner,
					},
				}
			},
		};
		Ok(INftClaimsMinter::collectionMinterCall::abi_encode_returns(&ret))
	}
}
