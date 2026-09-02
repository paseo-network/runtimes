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

//! The per-collection ERC-721 precompile.

use super::*;
use frame_system::RawOrigin;
use pallet_revive::{codec::DecodeAll, sp_runtime::traits::UniqueSaturatedInto};
use pallet_scarcity::{
	weights::WeightInfo as _, ItemDefs, ItemIndex, OnCollectionDeleted, OnPurseOccupied,
	Pallet as Scarcity, ValidateMetadata,
};
use IScarcityCollection::IScarcityCollectionCalls;

/// ERC-721 precompile answering for every collection under the address prefix `PREFIX`.
///
/// The collection id is taken from the first four bytes of the called address, so each
/// collection appears as its own contract. See the crate documentation for the address
/// layout and the supported surface.
pub struct ScarcityCollection<T, const PREFIX: u16>(PhantomData<T>);

impl<T, const PREFIX: u16> pallet_revive::precompiles::Precompile for ScarcityCollection<T, PREFIX>
where
	T: pallet_scarcity::Config + pallet_revive::Config,
{
	type T = T;
	type Interface = IScarcityCollectionCalls;
	const MATCHER: AddressMatcher = AddressMatcher::Prefix(NonZero::new(PREFIX).unwrap());
	const HAS_CONTRACT_INFO: bool = false;

	fn call(
		address: &[u8; 20],
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

		let collection = collection_id_of(address);
		// The prefix matcher answers for every id, so without this gate an address whose
		// collection does not exist reads as a live empty ERC-721 contract that
		// `supportsInterface` vouches for, while `collectionOwner` reverts.
		charge_reads(env, 1)?;
		if !Collections::<T>::contains_key(collection) {
			return Err(revert(ERR_UNKNOWN_COLLECTION));
		}
		match input {
			// ERC-165
			IScarcityCollectionCalls::supportsInterface(call) =>
				Self::supports_interface(call, env),
			// ERC-721 core reads
			IScarcityCollectionCalls::balanceOf(call) => Self::balance_of(collection, call, env),
			IScarcityCollectionCalls::ownerOf(call) => Self::owner_of(collection, call, env),
			IScarcityCollectionCalls::tokenOfOwnerByIndex(call) =>
				Self::token_of_owner_by_index(collection, call, env),
			IScarcityCollectionCalls::getApproved(call) =>
				Self::get_approved(collection, call, env),
			IScarcityCollectionCalls::isApprovedForAll(_) => {
				charge_reads(env, 1)?;
				Ok(IScarcityCollection::isApprovedForAllCall::abi_encode_returns(&false))
			},
			// ERC-5192
			IScarcityCollectionCalls::locked(call) => Self::locked(collection, call, env),
			// ERC-2981
			IScarcityCollectionCalls::royaltyInfo(call) =>
				Self::royalty_info(collection, call, env),
			// ERC-7572
			IScarcityCollectionCalls::contractURI(_) => Self::contract_uri(collection, env),
			// ERC-721 transfers
			IScarcityCollectionCalls::transferFrom(call) => Self::transfer_from(
				collection,
				Transfer { from: &call.from, to: &call.to, token: &call.tokenId },
				Safety::Unchecked,
				env,
			),
			IScarcityCollectionCalls::safeTransferFrom_0(call) => Self::transfer_from(
				collection,
				Transfer { from: &call.from, to: &call.to, token: &call.tokenId },
				Safety::Checked { data: call.data.as_ref() },
				env,
			),
			IScarcityCollectionCalls::safeTransferFrom_1(call) => Self::transfer_from(
				collection,
				Transfer { from: &call.from, to: &call.to, token: &call.tokenId },
				Safety::Checked { data: &[] },
				env,
			),
			// Approvals: unsupported by the purse model, see the crate documentation.
			IScarcityCollectionCalls::approve(_) |
			IScarcityCollectionCalls::setApprovalForAll(_) => {
				charge_reads(env, 1)?;
				Err(revert(ERR_APPROVALS_UNSUPPORTED))
			},
			// ERC-721 metadata
			IScarcityCollectionCalls::name(_) => Self::name(collection, env),
			IScarcityCollectionCalls::symbol(_) => Self::symbol(collection, env),
			IScarcityCollectionCalls::tokenURI(call) => Self::token_uri(collection, call, env),
			// Admin surface
			IScarcityCollectionCalls::defineItem(call) => Self::define_item(collection, call, env),
			IScarcityCollectionCalls::mint(call) => Self::mint(collection, call, env),
			IScarcityCollectionCalls::forceTransfer(call) =>
				Self::force_transfer(collection, call, env),
			IScarcityCollectionCalls::forceBurn(call) => Self::force_burn(collection, call, env),
			IScarcityCollectionCalls::setCollectionMetadata(call) => {
				let value = bounded_value::<T>(&call.value)?;
				Self::set_collection_metadata(collection, &call.key, Some(value), env)
			},
			IScarcityCollectionCalls::removeCollectionMetadata(call) =>
				Self::set_collection_metadata(collection, &call.key, None, env),
			IScarcityCollectionCalls::setItemMetadata(call) => {
				let value = bounded_value::<T>(&call.value)?;
				Self::set_item_metadata(collection, call.item, &call.key, Some(value), env)
			},
			IScarcityCollectionCalls::removeItemMetadata(call) =>
				Self::set_item_metadata(collection, call.item, &call.key, None, env),
			IScarcityCollectionCalls::setInstanceMetadata(call) => {
				let value = bounded_value::<T>(&call.value)?;
				Self::set_instance_metadata(collection, &call.tokenId, &call.key, Some(value), env)
			},
			IScarcityCollectionCalls::removeInstanceMetadata(call) =>
				Self::set_instance_metadata(collection, &call.tokenId, &call.key, None, env),
			IScarcityCollectionCalls::nominateCollectionOwner(call) =>
				Self::nominate_collection_owner(collection, Some(&call.successor), env),
			IScarcityCollectionCalls::clearCollectionOwnerNomination(_) =>
				Self::nominate_collection_owner(collection, None, env),
			IScarcityCollectionCalls::claimCollectionOwnership(_) =>
				Self::claim_collection_ownership(collection, env),
			IScarcityCollectionCalls::deleteItem(call) => Self::delete_item(collection, call, env),
			IScarcityCollectionCalls::deleteCollection(_) =>
				Self::delete_collection(collection, env),
			// Scarcity reads
			IScarcityCollectionCalls::collectionOwner(_) | IScarcityCollectionCalls::owner(_) =>
				Self::collection_owner(collection, env),
			IScarcityCollectionCalls::pendingCollectionOwner(_) =>
				Self::pending_collection_owner(collection, env),
			IScarcityCollectionCalls::collectionOwnerDeposit(_) =>
				Self::collection_owner_deposit(collection, env),
			IScarcityCollectionCalls::hasCollectionMetadata(call) =>
				Self::has_collection_metadata(collection, call, env),
			IScarcityCollectionCalls::hasItemMetadata(call) =>
				Self::has_item_metadata(collection, call, env),
			IScarcityCollectionCalls::hasInstanceMetadata(call) =>
				Self::has_instance_metadata(collection, call, env),
			IScarcityCollectionCalls::itemSupply(call) => Self::item_supply(collection, call, env),
			IScarcityCollectionCalls::instanceInfo(call) =>
				Self::instance_info(collection, call, env),
			IScarcityCollectionCalls::collectionMetadata(call) =>
				Self::collection_metadata(collection, call, env),
			IScarcityCollectionCalls::itemMetadata(call) =>
				Self::item_metadata(collection, call, env),
			IScarcityCollectionCalls::instanceMetadata(call) =>
				Self::instance_metadata(collection, call, env),
		}
	}
}

/// The three address and token arguments every ERC-721 transfer overload carries.
struct Transfer<'a> {
	from: &'a Address,
	to: &'a Address,
	token: &'a U256,
}

/// Whether the ERC-721 receiver guarantee applies to a transfer.
enum Safety<'a> {
	/// `transferFrom`, whose caller accepts a destination that cannot acknowledge the token.
	Unchecked,
	/// `safeTransferFrom`, whose destination must acknowledge the token if it carries code.
	/// `data` reaches the destination unread, and is empty for the three-argument overload.
	Checked { data: &'a [u8] },
}

/// The metadata scope a write landed in, for deciding what it must announce.
enum Scope {
	Collection,
	Item,
	Instance(U256),
}

/// Whether the destination of a `safeTransferFrom` has acknowledged the token.
enum Acknowledgement {
	/// The destination answered for the token, or carries no code and has nothing to answer.
	Acknowledged,
	/// The destination carries code and cannot be asked, so the transfer must not stand.
	Unavailable,
}

/// Exhaustive on purpose: a method added to the interface must be classified here before
/// the crate compiles again.
///
/// Visible to the crate's tests, which drive every selector through a read-only frame and use this
/// to say which of them must be refused. Nothing else stops a `STATICCALL` from reaching a pallet
/// write, so a misclassification here fails open.
pub(crate) fn is_mutating(input: &IScarcityCollectionCalls) -> bool {
	match input {
		IScarcityCollectionCalls::transferFrom(_) |
		IScarcityCollectionCalls::safeTransferFrom_0(_) |
		IScarcityCollectionCalls::safeTransferFrom_1(_) |
		IScarcityCollectionCalls::approve(_) |
		IScarcityCollectionCalls::setApprovalForAll(_) |
		IScarcityCollectionCalls::defineItem(_) |
		IScarcityCollectionCalls::mint(_) |
		IScarcityCollectionCalls::forceTransfer(_) |
		IScarcityCollectionCalls::forceBurn(_) |
		IScarcityCollectionCalls::setCollectionMetadata(_) |
		IScarcityCollectionCalls::removeCollectionMetadata(_) |
		IScarcityCollectionCalls::setItemMetadata(_) |
		IScarcityCollectionCalls::removeItemMetadata(_) |
		IScarcityCollectionCalls::setInstanceMetadata(_) |
		IScarcityCollectionCalls::removeInstanceMetadata(_) |
		IScarcityCollectionCalls::nominateCollectionOwner(_) |
		IScarcityCollectionCalls::clearCollectionOwnerNomination(_) |
		IScarcityCollectionCalls::claimCollectionOwnership(_) |
		IScarcityCollectionCalls::deleteItem(_) |
		IScarcityCollectionCalls::deleteCollection(_) => true,
		IScarcityCollectionCalls::supportsInterface(_) |
		IScarcityCollectionCalls::balanceOf(_) |
		IScarcityCollectionCalls::ownerOf(_) |
		IScarcityCollectionCalls::tokenOfOwnerByIndex(_) |
		IScarcityCollectionCalls::getApproved(_) |
		IScarcityCollectionCalls::isApprovedForAll(_) |
		IScarcityCollectionCalls::locked(_) |
		IScarcityCollectionCalls::royaltyInfo(_) |
		IScarcityCollectionCalls::contractURI(_) |
		IScarcityCollectionCalls::name(_) |
		IScarcityCollectionCalls::symbol(_) |
		IScarcityCollectionCalls::tokenURI(_) |
		IScarcityCollectionCalls::collectionOwner(_) |
		IScarcityCollectionCalls::owner(_) |
		IScarcityCollectionCalls::pendingCollectionOwner(_) |
		IScarcityCollectionCalls::collectionOwnerDeposit(_) |
		IScarcityCollectionCalls::itemSupply(_) |
		IScarcityCollectionCalls::instanceInfo(_) |
		IScarcityCollectionCalls::collectionMetadata(_) |
		IScarcityCollectionCalls::itemMetadata(_) |
		IScarcityCollectionCalls::instanceMetadata(_) |
		IScarcityCollectionCalls::hasCollectionMetadata(_) |
		IScarcityCollectionCalls::hasItemMetadata(_) |
		IScarcityCollectionCalls::hasInstanceMetadata(_) => false,
	}
}

impl<T, const PREFIX: u16> ScarcityCollection<T, PREFIX>
where
	T: pallet_scarcity::Config + pallet_revive::Config,
{
	fn supports_interface(
		call: &IScarcityCollection::supportsInterfaceCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		charge_reads(env, 1)?;
		let id = call.interfaceId.0;
		let supported = id == ERC165_INTERFACE_ID ||
			id == ERC721_INTERFACE_ID ||
			id == ERC721_METADATA_INTERFACE_ID ||
			id == ERC5192_INTERFACE_ID ||
			id == ERC2981_INTERFACE_ID ||
			id == ERC4906_INTERFACE_ID;
		Ok(IScarcityCollection::supportsInterfaceCall::abi_encode_returns(&supported))
	}

	fn balance_of(
		collection: CollectionId,
		call: &IScarcityCollection::balanceOfCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		// Reverse address mapping plus the purse read.
		charge_reads(env, 2)?;
		if call.owner == Address::ZERO {
			return Err(revert(ERR_ZERO_OWNER));
		}
		let holds = NftsByOwner::<T>::get(account_of::<T>(&call.owner))
			.is_some_and(|nft| nft.collection == collection);
		let balance = if holds { U256::ONE } else { U256::ZERO };
		Ok(IScarcityCollection::balanceOfCall::abi_encode_returns(&balance))
	}

	fn owner_of(
		collection: CollectionId,
		call: &IScarcityCollection::ownerOfCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		charge_reads(env, 2)?;
		let (purse, _) = live_instance::<T>(collection, &call.tokenId)?;
		Ok(IScarcityCollection::ownerOfCall::abi_encode_returns(&address_of::<T>(&purse)))
	}

	/// The single token a purse holds, at index 0.
	///
	/// A purse holds at most one instance, so this is `balanceOf` with the answer in place of the
	/// count, and every index above 0 is out of range. It does not make the collection ERC-721
	/// Enumerable, which also needs `totalSupply` and `tokenByIndex`.
	fn token_of_owner_by_index(
		collection: CollectionId,
		call: &IScarcityCollection::tokenOfOwnerByIndexCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		// Reverse address mapping plus the purse read, as in `balance_of`.
		charge_reads(env, 2)?;
		if call.owner == Address::ZERO {
			return Err(revert(ERR_ZERO_OWNER));
		}
		// EIP-721 requires a throw for any index at or above the holder's balance, which is 1 for
		// a holder of this collection and 0 for everyone else.
		let nft = NftsByOwner::<T>::get(account_of::<T>(&call.owner))
			.filter(|nft| nft.collection == collection)
			.ok_or_else(|| revert(ERR_INDEX_OUT_OF_RANGE))?;
		if !call.index.is_zero() {
			return Err(revert(ERR_INDEX_OUT_OF_RANGE));
		}
		Ok(IScarcityCollection::tokenOfOwnerByIndexCall::abi_encode_returns(&U256::from(
			nft.instance,
		)))
	}

	fn get_approved(
		collection: CollectionId,
		call: &IScarcityCollection::getApprovedCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		charge_reads(env, 2)?;
		live_instance::<T>(collection, &call.tokenId)?;
		Ok(IScarcityCollection::getApprovedCall::abi_encode_returns(&Address::ZERO))
	}

	fn locked(
		collection: CollectionId,
		call: &IScarcityCollection::lockedCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		// The instance lookup, then the definition carrying the flag.
		charge_reads(env, 3)?;
		let (_, nft) = live_instance::<T>(collection, &call.tokenId)?;
		let transferability =
			Scarcity::<T>::transferability_of(&nft).map_err(revert_scarcity::<T>)?;
		let locked = transferability == Transferability::Soulbound;
		Ok(IScarcityCollection::lockedCall::abi_encode_returns(&locked))
	}

	fn name(collection: CollectionId, env: &mut impl Ext<T = T>) -> Result<Vec<u8>, Error> {
		let name = Self::collection_string(collection, NAME_KEY, env)?;
		Ok(IScarcityCollection::nameCall::abi_encode_returns(&name))
	}

	fn symbol(collection: CollectionId, env: &mut impl Ext<T = T>) -> Result<Vec<u8>, Error> {
		let symbol = Self::collection_string(collection, SYMBOL_KEY, env)?;
		Ok(IScarcityCollection::symbolCall::abi_encode_returns(&symbol))
	}

	/// ERC-2981 royalty for a sale of one instance.
	fn royalty_info(
		collection: CollectionId,
		call: &IScarcityCollection::royaltyInfoCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		// The instance lookup, then the item and collection scopes of both keys.
		charge_reads(env, 6)?;
		let (_, nft) = live_instance::<T>(collection, &call.tokenId)?;
		let Some((receiver, basis_points)) = Self::royalty_terms(&nft)? else {
			return Ok(IScarcityCollection::royaltyInfoCall::abi_encode_returns(
				&IScarcityCollection::royaltyInfoReturn {
					receiver: Address::ZERO,
					royaltyAmount: U256::ZERO,
				},
			));
		};
		let amount = call
			.salePrice
			.checked_mul(U256::from(basis_points))
			.map(|scaled| scaled / U256::from(BASIS_POINTS_DENOMINATOR))
			.ok_or_else(|| revert(ERR_ROYALTY_OVERFLOW))?;
		Ok(IScarcityCollection::royaltyInfoCall::abi_encode_returns(
			&IScarcityCollection::royaltyInfoReturn {
				receiver: Address::from(receiver),
				royaltyAmount: amount,
			},
		))
	}

	/// Royalty terms a sale can actually be priced against, or `None`.
	///
	/// Every way a collection can fail to configure a royalty answers `None` rather than
	/// reverting: an unset key, a receiver that is not an address or is the zero address,
	/// basis points that do not decode, and a share above 100%. A marketplace that calls this
	/// without catching reverts would otherwise fail the whole sale, which makes one bad
	/// metadata value cost far more than the royalty it was meant to collect. Implementations
	/// backed by typed storage reject these when the royalty is set; this one stores raw bytes
	/// under a metadata key, so the read is the only place left to reject them.
	fn royalty_terms(nft: &Nft) -> Result<Option<([u8; 20], u128)>, Error> {
		let (Some(receiver), Some(basis_points)) = (
			Self::item_bytes(nft, ROYALTY_RECEIVER_KEY)?,
			Self::item_bytes(nft, ROYALTY_BASIS_POINTS_KEY)?,
		) else {
			return Ok(None);
		};
		let Ok(receiver) = <[u8; 20]>::try_from(receiver) else {
			return Ok(None);
		};
		// Quoting an amount payable to the zero address would have a marketplace burn it.
		// Note this is not the fallback sentinel it is elsewhere: implementations that store
		// the receiver and the share together read a zero item-level receiver as "use the
		// collection default", whereas resolution here has already chosen the item-level value,
		// so a zero receiver means no royalty and discards the collection's terms.
		if receiver == [0u8; 20] {
			return Ok(None);
		}
		// `decode_all` rejects a value with bytes to spare, where `decode` would read a `u128` off
		// the front of a longer one and ignore the rest. An ABI-encoded `uint256` is the mistake
		// that guards against: its leading 16 bytes are the high half of a big-endian word, so a
		// share small enough to be valid would read back as zero.
		Ok(u128::decode_all(&mut basis_points.as_slice())
			.ok()
			.filter(|points| *points <= BASIS_POINTS_DENOMINATOR)
			.map(|points| (receiver, points)))
	}

	/// A metadata value for `key` resolved item, then collection scope.
	///
	/// The reserved keys are shorter than any sane `MaxKeyLen`, so the revert here reports a
	/// runtime misconfiguration rather than anything a collection can cause.
	fn item_bytes(nft: &Nft, key: &[u8]) -> Result<Option<Vec<u8>>, Error> {
		let key =
			MetadataKeyOf::<T>::try_from(key.to_vec()).map_err(|_| revert(ERR_KEY_TOO_LONG))?;
		Ok(Scarcity::<T>::item_metadata_of(nft.collection, nft.item, &key)
			.map(|value| value.into_inner()))
	}

	fn contract_uri(collection: CollectionId, env: &mut impl Ext<T = T>) -> Result<Vec<u8>, Error> {
		let uri = Self::collection_string(collection, CONTRACT_URI_KEY, env)?;
		Ok(IScarcityCollection::contractURICall::abi_encode_returns(&uri))
	}

	/// A collection metadata value under a reserved key, or the empty string.
	///
	/// Decoded lossily. [`Erc721MetadataPolicy`](crate::Erc721MetadataPolicy) keeps these keys
	/// UTF-8 only for a runtime that wires it, and only from the moment it does, so the read
	/// cannot depend on it. Replacement characters keep a collection holding anything else
	/// readable, where a revert would make every ERC-721 consumer treat it as broken.
	fn collection_string(
		collection: CollectionId,
		key: &[u8],
		env: &mut impl Ext<T = T>,
	) -> Result<alloc::string::String, Error> {
		charge_reads(env, 1)?;
		let key =
			MetadataKeyOf::<T>::try_from(key.to_vec()).map_err(|_| revert(ERR_KEY_TOO_LONG))?;
		match Scarcity::<T>::collection_metadata_of(collection, &key) {
			None => Ok(alloc::string::String::new()),
			Some(value) => {
				let value = value.into_inner();
				Ok(alloc::string::String::from_utf8_lossy(&value).into_owned())
			},
		}
	}

	fn token_uri(
		collection: CollectionId,
		call: &IScarcityCollection::tokenURICall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		// Instance lookup plus the three metadata scopes. Resolved from the already-loaded
		// `Nft` rather than through `instance_metadata_of`, which would repeat the instance
		// lookup.
		charge_reads(env, 5)?;
		let (_, nft) = live_instance::<T>(collection, &call.tokenId)?;
		let key = MetadataKeyOf::<T>::try_from(TOKEN_URI_KEY.to_vec())
			.map_err(|_| revert(ERR_KEY_TOO_LONG))?;
		let value = InstanceMetadata::<T>::get(nft.instance, &key)
			.map(|entry| entry.value)
			.or_else(|| Scarcity::<T>::item_metadata_of(nft.collection, nft.item, &key));
		// Decoded lossily, for the reason given on `collection_string`.
		let uri = match value {
			None => alloc::string::String::new(),
			Some(value) => {
				let value = value.into_inner();
				alloc::string::String::from_utf8_lossy(&value).into_owned()
			},
		};
		Ok(IScarcityCollection::tokenURICall::abi_encode_returns(&uri))
	}

	fn define_item(
		collection: CollectionId,
		call: &IScarcityCollection::defineItemCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		// The pallet call plus the runtime's metadata policy, which validates every pair inside
		// it, mirroring the pallet's own weight annotation.
		env.charge(
			<T as pallet_scarcity::Config>::WeightInfo::define_item(call.keys.len() as u32)
				.saturating_add(<T as pallet_scarcity::Config>::MetadataPolicy::validate_weight(
					call.keys.len() as u32,
				)),
		)?;
		let metadata = bounded_metadata::<T>(&call.keys, &call.values)?;
		let who = caller_account::<T>(env)?;
		let transferability =
			if call.soulbound { Transferability::Soulbound } else { Transferability::Transferable };
		let item = Scarcity::<T>::do_define_item(who, collection, transferability, metadata)
			.map_err(revert_scarcity::<T>)?;
		Ok(IScarcityCollection::defineItemCall::abi_encode_returns(&item))
	}

	fn mint(
		collection: CollectionId,
		call: &IScarcityCollection::mintCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		// The destination's reverse address mapping and the item definition read for ERC-5192,
		// then the pallet call and the runtime's metadata policy and purse hook behind it.
		charge_reads(env, 2)?;
		env.charge(
			<T as pallet_scarcity::Config>::WeightInfo::mint(call.keys.len() as u32)
				.saturating_add(<T as pallet_scarcity::Config>::MetadataPolicy::validate_weight(
					call.keys.len() as u32,
				)),
		)?;
		env.charge(<<T as pallet_scarcity::Config>::OnPurseOccupied as OnPurseOccupied<
			T::AccountId,
		>>::on_purse_occupied_weight())?;
		// The zero address maps to a real fallback purse; minting there would strand the
		// instance behind a burn-shaped `Transfer` log.
		if call.to == Address::ZERO {
			return Err(revert(ERR_ZERO_DESTINATION));
		}
		let metadata = bounded_metadata::<T>(&call.keys, &call.values)?;
		let who = caller_account::<T>(env)?;
		let to = account_of::<T>(&call.to);
		let instance = Scarcity::<T>::do_mint(who, collection, call.item, to, metadata)
			.map_err(revert_scarcity::<T>)?;
		let token = U256::from(instance);
		deposit_event(
			env,
			IScarcityCollection::Transfer { from: Address::ZERO, to: call.to, tokenId: token },
		)?;
		// ERC-5192 asks for the status of a minted token, and transferability is fixed at
		// definition, so this is the only point at which either event can ever be emitted. The
		// mint resolved this definition, so the read cannot miss.
		let soulbound = ItemDefs::<T>::get(collection, call.item)
			.is_some_and(|definition| definition.transferability == Transferability::Soulbound);
		if soulbound {
			deposit_event(env, IScarcityCollection::Locked { tokenId: token })?;
		} else {
			deposit_event(env, IScarcityCollection::Unlocked { tokenId: token })?;
		}
		Ok(IScarcityCollection::mintCall::abi_encode_returns(&token))
	}

	/// Move a token on its holder's own authority.
	///
	/// The caller must be the holder. Without an approval mechanism there is no third party to
	/// resolve, so the ERC-721 spender cases all collapse to this one.
	fn transfer_from(
		collection: CollectionId,
		transfer: Transfer<'_>,
		safety: Safety<'_>,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		// See `mint` on why the zero address is rejected.
		if *transfer.to == Address::ZERO {
			return Err(revert(ERR_ZERO_DESTINATION));
		}
		// Collection-membership validation reads and both reverse address mappings.
		charge_reads(env, 4)?;
		let (holder, nft) = live_instance::<T>(collection, transfer.token)?;
		if account_of::<T>(transfer.from) != holder {
			return Err(revert(ERR_WRONG_HOLDER));
		}
		if caller_account::<T>(env)? != holder {
			return Err(revert(ERR_NOT_HOLDER));
		}
		env.charge(<T as pallet_scarcity::Config>::WeightInfo::transfer_by_holder())?;
		// The move registers its destination. `WeightInfo::transfer_by_holder` does not cover that:
		// the extrinsic adds the same term to its own annotation, and the benchmark runtime wires
		// no handler.
		env.charge(<<T as pallet_scarcity::Config>::OnPurseOccupied as OnPurseOccupied<
			T::AccountId,
		>>::on_purse_occupied_weight())?;
		let to = account_of::<T>(transfer.to);
		Scarcity::<T>::do_transfer_by_holder(&holder, nft.instance, to)
			.map_err(revert_scarcity::<T>)?;
		deposit_event(
			env,
			IScarcityCollection::Transfer {
				from: *transfer.from,
				to: *transfer.to,
				tokenId: *transfer.token,
			},
		)?;
		if let Safety::Checked { data } = safety {
			// A revert here unwinds the move above with the rest of the frame.
			if let Acknowledgement::Unavailable = Self::acknowledge_receipt(&transfer, data, env)? {
				return Err(revert(ERR_CONTRACT_RECEIVER));
			}
		}
		Ok(Vec::new())
	}

	/// Ask the destination of a `safeTransferFrom` to acknowledge the token it now holds.
	///
	/// Answers [`Acknowledgement::Unavailable`] for every destination carrying code: the
	/// `IERC721Receiver::onERC721Received` call this owes them needs `Ext::call`, whose
	/// reentrancy argument `pallet-revive` does not export. See the crate documentation for
	/// what the call does once it can be made, and why it will not permit reentry.
	fn acknowledge_receipt(
		transfer: &Transfer<'_>,
		_data: &[u8],
		env: &mut impl Ext<T = T>,
	) -> Result<Acknowledgement, Error> {
		charge_reads(env, 1)?;
		if env.code_size(&H160(transfer.to.into_array())) == 0 {
			return Ok(Acknowledgement::Acknowledged);
		}
		// TODO(<https://github.com/paritytech/individuality/issues/1298>): once that lands, call
		// `IERC721Receiver::onERC721Received` here with `ReentrancyProtection::Strict` and
		// answer `Acknowledged` only for its own selector, forwarding `data` unread.
		Ok(Acknowledgement::Unavailable)
	}

	fn force_transfer(
		collection: CollectionId,
		call: &IScarcityCollection::forceTransferCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		// Collection-membership validation reads, the destination's reverse address mapping and
		// the pallet call.
		charge_reads(env, 3)?;
		env.charge(<T as pallet_scarcity::Config>::WeightInfo::force_transfer())?;
		// See `mint` on why the zero address is rejected.
		if call.to == Address::ZERO {
			return Err(revert(ERR_ZERO_DESTINATION));
		}
		let (from, nft) = live_instance::<T>(collection, &call.tokenId)?;
		let who = caller_account::<T>(env)?;
		let to = account_of::<T>(&call.to);
		let from_address = address_of::<T>(&from);
		// The pallet rejects a move to the current holder, but it compares accounts and this
		// call carries an address. `ownerOf` reports an unregistered purse key as a truncated
		// hash, and resolving that hash back yields the fallback account instead of the key, so
		// the two accounts differ and the pallet's check passes. The instance would then leave a
		// holder that asked for nothing under a log naming one address for both ends.
		if call.to == from_address && to != from {
			return Err(revert(ERR_SELF_TRANSFER));
		}
		// The move registers its destination, which the benchmark does not cover. See
		// `transfer_from`. Charged past the rejections above so a revert does not pay for a
		// registration that never happens.
		env.charge(<<T as pallet_scarcity::Config>::OnPurseOccupied as OnPurseOccupied<
			T::AccountId,
		>>::on_purse_occupied_weight())?;
		Scarcity::<T>::force_transfer(RawOrigin::Signed(who).into(), nft.instance, to)
			.map_err(revert_scarcity::<T>)?;
		deposit_event(
			env,
			IScarcityCollection::Transfer {
				from: from_address,
				to: call.to,
				tokenId: call.tokenId,
			},
		)?;
		Ok(Vec::new())
	}

	fn force_burn(
		collection: CollectionId,
		call: &IScarcityCollection::forceBurnCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		// Collection-membership validation reads, then the pallet call at its worst-case
		// weight, refunded below to the actual metadata count removed. Unlike the other
		// mutators the worst case waits for the lookups, because `adjust_gas` refunds only on
		// the path that reaches it: charging first would make an unknown token keep the full
		// charge of the heaviest call in the crate, which
		// `force_burn_refunds_unused_weight` pins against.
		charge_reads(env, 2)?;
		let (purse, nft) = live_instance::<T>(collection, &call.tokenId)?;
		let who = caller_account::<T>(env)?;
		let worst = <T as pallet_scarcity::Config>::WeightInfo::force_burn(
			<T as pallet_scarcity::Config>::MaxInstanceMetadata::get(),
		);
		let charged = env.charge(worst)?;
		let post = Scarcity::<T>::force_burn(RawOrigin::Signed(who).into(), nft.instance)
			.map_err(|e| revert_scarcity::<T>(e.error))?;
		env.adjust_gas(charged, post.actual_weight.unwrap_or(worst));
		deposit_event(
			env,
			IScarcityCollection::Transfer {
				from: address_of::<T>(&purse),
				to: Address::ZERO,
				tokenId: call.tokenId,
			},
		)?;
		Ok(Vec::new())
	}

	/// Announce a metadata write that changes what a standard read returns.
	///
	/// Only the reserved keys behind `tokenURI` and `contractURI` have a standard event. Every
	/// other key emits nothing, `name` and `symbol` included: no standard defines an event for
	/// them, and announcing a change that no standard read reflects would have consumers refetch a
	/// document that did not move.
	///
	/// A removal announces even when the key was absent. Telling them apart costs a read, and a
	/// consumer refetching once too often is cheaper than one that never learns.
	fn announce_metadata_write(
		key: &[u8],
		scope: Scope,
		env: &mut impl Ext<T = T>,
	) -> Result<(), Error> {
		if key == TOKEN_URI_KEY {
			return match scope {
				Scope::Instance(token) =>
					deposit_event(env, IScarcityCollection::MetadataUpdate { tokenId: token }),
				// The affected instances cannot be enumerated, so the range covers every id and
				// the log's source address carries the collection scope.
				Scope::Collection | Scope::Item => deposit_event(
					env,
					IScarcityCollection::BatchMetadataUpdate {
						fromTokenId: U256::ZERO,
						toTokenId: U256::MAX,
					},
				),
			};
		}
		// `contractURI` resolves collection scope only, so a write to it anywhere else changes
		// nothing a read can see.
		if key == CONTRACT_URI_KEY && matches!(scope, Scope::Collection) {
			return deposit_event(env, IScarcityCollection::ContractURIUpdated {});
		}
		Ok(())
	}

	/// Serves `setCollectionMetadata` with `Some` and `removeCollectionMetadata` with `None`,
	/// mirroring the pallet call's optional value.
	fn set_collection_metadata(
		collection: CollectionId,
		key: &alloy::primitives::Bytes,
		value: Option<MetadataValueOf<T>>,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		env.charge(
			<T as pallet_scarcity::Config>::WeightInfo::set_collection_metadata()
				.saturating_add(<T as pallet_scarcity::Config>::MetadataPolicy::validate_weight(1)),
		)?;
		let bounded = bounded_key::<T>(key)?;
		let who = caller_account::<T>(env)?;
		Scarcity::<T>::set_collection_metadata(
			RawOrigin::Signed(who).into(),
			collection,
			bounded,
			value,
		)
		.map_err(revert_scarcity::<T>)?;
		Self::announce_metadata_write(key, Scope::Collection, env)?;
		Ok(Vec::new())
	}

	/// Serves `setItemMetadata` with `Some` and `removeItemMetadata` with `None`, mirroring
	/// the pallet call's optional value.
	fn set_item_metadata(
		collection: CollectionId,
		item: ItemIndex,
		key: &alloy::primitives::Bytes,
		value: Option<MetadataValueOf<T>>,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		env.charge(
			<T as pallet_scarcity::Config>::WeightInfo::set_item_metadata()
				.saturating_add(<T as pallet_scarcity::Config>::MetadataPolicy::validate_weight(1)),
		)?;
		let bounded = bounded_key::<T>(key)?;
		let who = caller_account::<T>(env)?;
		Scarcity::<T>::set_item_metadata(
			RawOrigin::Signed(who).into(),
			collection,
			item,
			bounded,
			value,
		)
		.map_err(revert_scarcity::<T>)?;
		Self::announce_metadata_write(key, Scope::Item, env)?;
		Ok(Vec::new())
	}

	/// Serves `setInstanceMetadata` with `Some` and `removeInstanceMetadata` with `None`,
	/// mirroring the pallet call's optional value.
	fn set_instance_metadata(
		collection: CollectionId,
		token: &U256,
		key: &alloy::primitives::Bytes,
		value: Option<MetadataValueOf<T>>,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		// Collection-membership validation reads, then the pallet call. The pallet call takes
		// the global instance id, so the membership check keeps this address answering only
		// for its own collection.
		charge_reads(env, 2)?;
		env.charge(
			<T as pallet_scarcity::Config>::WeightInfo::set_instance_metadata()
				.saturating_add(<T as pallet_scarcity::Config>::MetadataPolicy::validate_weight(1)),
		)?;
		let (_, nft) = live_instance::<T>(collection, token)?;
		let bounded = bounded_key::<T>(key)?;
		let who = caller_account::<T>(env)?;
		Scarcity::<T>::set_instance_metadata(
			RawOrigin::Signed(who).into(),
			nft.instance,
			bounded,
			value,
		)
		.map_err(revert_scarcity::<T>)?;
		Self::announce_metadata_write(key, Scope::Instance(*token), env)?;
		Ok(Vec::new())
	}

	/// Serves `nominateCollectionOwner` with `Some` and `clearCollectionOwnerNomination` with
	/// `None`, mirroring the pallet call's optional successor.
	fn nominate_collection_owner(
		collection: CollectionId,
		successor: Option<&Address>,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		// The pallet call, plus the reverse address mapping a nomination resolves.
		env.charge(<T as pallet_scarcity::Config>::WeightInfo::nominate_collection_owner())?;
		if successor.is_some() {
			charge_reads(env, 1)?;
		}
		let pending_owner = match successor {
			Some(address) => {
				// The zero address maps to a real fallback purse; `pendingCollectionOwner` reports
				// a cleared nomination as zero, so nominating it would be unreadable.
				if *address == Address::ZERO {
					return Err(revert(ERR_ZERO_SUCCESSOR));
				}
				Some(account_of::<T>(address))
			},
			None => None,
		};
		let who = caller_account::<T>(env)?;
		Scarcity::<T>::nominate_collection_owner(
			RawOrigin::Signed(who).into(),
			collection,
			pending_owner,
		)
		.map_err(revert_scarcity::<T>)?;
		Ok(Vec::new())
	}

	fn claim_collection_ownership(
		collection: CollectionId,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		env.charge(<T as pallet_scarcity::Config>::WeightInfo::claim_collection_ownership())?;
		// The outgoing owner, read before the call replaces it, for the event's `previousOwner`.
		charge_reads(env, 1)?;
		let previous = Collections::<T>::get(collection)
			.ok_or_else(|| revert(ERR_UNKNOWN_COLLECTION))?
			.owner;
		let who = caller_account::<T>(env)?;
		Scarcity::<T>::claim_collection_ownership(
			RawOrigin::Signed(who.clone()).into(),
			collection,
		)
		.map_err(revert_scarcity::<T>)?;
		deposit_event(
			env,
			IScarcityCollection::OwnershipTransferred {
				previousOwner: address_of::<T>(&previous),
				newOwner: address_of::<T>(&who),
			},
		)?;
		Ok(Vec::new())
	}

	fn delete_item(
		collection: CollectionId,
		call: &IScarcityCollection::deleteItemCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		env.charge(<T as pallet_scarcity::Config>::WeightInfo::delete_item())?;
		let who = caller_account::<T>(env)?;
		Scarcity::<T>::delete_item(RawOrigin::Signed(who).into(), collection, call.item)
			.map_err(revert_scarcity::<T>)?;
		deposit_event(env, IScarcityCollection::ItemDeleted { item: call.item })?;
		Ok(Vec::new())
	}

	fn delete_collection(
		collection: CollectionId,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		// The runtime's cross-pallet cleanup hook runs inside the pallet call, so its weight
		// is charged with it, mirroring the pallet's own weight annotation.
		env.charge(
			<T as pallet_scarcity::Config>::WeightInfo::delete_collection().saturating_add(
				<T as pallet_scarcity::Config>::OnCollectionDeleted::on_delete_weight(),
			),
		)?;
		let who = caller_account::<T>(env)?;
		Scarcity::<T>::delete_collection(RawOrigin::Signed(who).into(), collection)
			.map_err(revert_scarcity::<T>)?;
		deposit_event(env, IScarcityCollection::CollectionDeleted {})?;
		Ok(Vec::new())
	}

	fn collection_owner(
		collection: CollectionId,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		charge_reads(env, 1)?;
		let info =
			Collections::<T>::get(collection).ok_or_else(|| revert(ERR_UNKNOWN_COLLECTION))?;
		Ok(IScarcityCollection::collectionOwnerCall::abi_encode_returns(&address_of::<T>(
			&info.owner,
		)))
	}

	fn pending_collection_owner(
		collection: CollectionId,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		charge_reads(env, 1)?;
		let info =
			Collections::<T>::get(collection).ok_or_else(|| revert(ERR_UNKNOWN_COLLECTION))?;
		let pending = info.pending_owner.as_ref().map(address_of::<T>).unwrap_or(Address::ZERO);
		Ok(IScarcityCollection::pendingCollectionOwnerCall::abi_encode_returns(&pending))
	}

	fn collection_owner_deposit(
		collection: CollectionId,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		charge_reads(env, 1)?;
		let info =
			Collections::<T>::get(collection).ok_or_else(|| revert(ERR_UNKNOWN_COLLECTION))?;
		let deposit: u128 = info.owner_deposit.unique_saturated_into();
		Ok(IScarcityCollection::collectionOwnerDepositCall::abi_encode_returns(&U256::from(
			deposit,
		)))
	}

	fn has_collection_metadata(
		collection: CollectionId,
		call: &IScarcityCollection::hasCollectionMetadataCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		charge_reads(env, 1)?;
		let key = bounded_key::<T>(&call.key)?;
		let present = CollectionMetadata::<T>::contains_key(collection, &key);
		Ok(IScarcityCollection::hasCollectionMetadataCall::abi_encode_returns(&present))
	}

	fn has_item_metadata(
		collection: CollectionId,
		call: &IScarcityCollection::hasItemMetadataCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		charge_reads(env, 1)?;
		let key = bounded_key::<T>(&call.key)?;
		let present = ItemMetadata::<T>::contains_key((collection, call.item, key));
		Ok(IScarcityCollection::hasItemMetadataCall::abi_encode_returns(&present))
	}

	fn has_instance_metadata(
		collection: CollectionId,
		call: &IScarcityCollection::hasInstanceMetadataCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		// Instance lookup plus the presence check.
		charge_reads(env, 3)?;
		let (_, nft) = live_instance::<T>(collection, &call.tokenId)?;
		let key = bounded_key::<T>(&call.key)?;
		let present = InstanceMetadata::<T>::contains_key(nft.instance, &key);
		Ok(IScarcityCollection::hasInstanceMetadataCall::abi_encode_returns(&present))
	}

	fn item_supply(
		collection: CollectionId,
		call: &IScarcityCollection::itemSupplyCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		charge_reads(env, 1)?;
		let definition =
			ItemDefs::<T>::get(collection, call.item).ok_or_else(|| revert(ERR_UNKNOWN_ITEM))?;
		Ok(IScarcityCollection::itemSupplyCall::abi_encode_returns(
			&IScarcityCollection::itemSupplyReturn {
				supply: definition.supply,
				liveSupply: definition.live_supply,
			},
		))
	}

	fn instance_info(
		collection: CollectionId,
		call: &IScarcityCollection::instanceInfoCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		charge_reads(env, 2)?;
		let (_, nft) = live_instance::<T>(collection, &call.tokenId)?;
		Ok(IScarcityCollection::instanceInfoCall::abi_encode_returns(
			&IScarcityCollection::instanceInfoReturn {
				item: nft.item,
				mintedAt: nft.minted_at,
				lastMoved: nft.last_moved,
				stateNonce: nft.state_nonce,
			},
		))
	}

	fn collection_metadata(
		collection: CollectionId,
		call: &IScarcityCollection::collectionMetadataCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		charge_reads(env, 1)?;
		let key = MetadataKeyOf::<T>::try_from(call.key.to_vec())
			.map_err(|_| revert(ERR_KEY_TOO_LONG))?;
		let value = Scarcity::<T>::collection_metadata_of(collection, &key)
			.map(|value| value.into_inner())
			.unwrap_or_default();
		Ok(IScarcityCollection::collectionMetadataCall::abi_encode_returns(
			&alloy::primitives::Bytes::from(value),
		))
	}

	fn item_metadata(
		collection: CollectionId,
		call: &IScarcityCollection::itemMetadataCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		charge_reads(env, 2)?;
		let key = MetadataKeyOf::<T>::try_from(call.key.to_vec())
			.map_err(|_| revert(ERR_KEY_TOO_LONG))?;
		let value = Scarcity::<T>::item_metadata_of(collection, call.item, &key)
			.map(|value| value.into_inner())
			.unwrap_or_default();
		Ok(IScarcityCollection::itemMetadataCall::abi_encode_returns(
			&alloy::primitives::Bytes::from(value),
		))
	}

	fn instance_metadata(
		collection: CollectionId,
		call: &IScarcityCollection::instanceMetadataCall,
		env: &mut impl Ext<T = T>,
	) -> Result<Vec<u8>, Error> {
		// Instance lookup plus the three metadata scopes; see `token_uri` on the direct
		// resolution.
		charge_reads(env, 5)?;
		let (_, nft) = live_instance::<T>(collection, &call.tokenId)?;
		let key = MetadataKeyOf::<T>::try_from(call.key.to_vec())
			.map_err(|_| revert(ERR_KEY_TOO_LONG))?;
		let value = InstanceMetadata::<T>::get(nft.instance, &key)
			.map(|entry| entry.value)
			.or_else(|| Scarcity::<T>::item_metadata_of(nft.collection, nft.item, &key))
			.map(|value| value.into_inner())
			.unwrap_or_default();
		Ok(IScarcityCollection::instanceMetadataCall::abi_encode_returns(
			&alloy::primitives::Bytes::from(value),
		))
	}
}
