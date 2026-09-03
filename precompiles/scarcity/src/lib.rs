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

//! Pallet-revive precompiles exposing `pallet-scarcity` collections as ERC-721 contracts.
//!
//! Two precompiles share this crate:
//!
//! - [`ScarcityCollection`] answers at one address per collection. The collection id is encoded
//!   big-endian in the first four bytes of the address, so collection `7` under prefix `0x0520`
//!   lives at `0x00000007000000000000000000000000_0520_0000`. It serves the ERC-721 core and
//!   metadata read surface with the exact standard selectors, the collection owner's admin surface
//!   (define, mint, force-transfer, force-burn, metadata mutation at all three scopes, ownership
//!   handover, deletion) and Scarcity-specific reads (supply, instance info, raw metadata and its
//!   presence, pending owner, aggregate deposit).
//! - [`ScarcityFactory`] answers at one fixed address and creates collections. Creation cannot live
//!   on the per-collection addresses because the collection id does not exist until creation
//!   allocates it.
//!
//! `transferFrom` and `safeTransferFrom` move a token on its holder's own authority, through
//! `pallet-scarcity::do_transfer_by_holder`. The caller must be the holder: the pallet has no
//! approval mechanism to resolve a spender against, so `approve` and `setApprovalForAll` revert
//! and their getters answer with the empty values (zero address and `false`) that keep
//! read-driven indexers working. Only an eth-derived purse can be an EVM caller at all, so this
//! path serves EVM-native holders; wallet users move tokens over the fee-less native extrinsic.
//!
//! `safeTransferFrom` owes a destination that carries code the ERC-721 acknowledgement: after
//! the move, the destination is asked through `IERC721Receiver::onERC721Received` whether it
//! keeps the token. Making that call needs `Ext::call`, whose `ReentrancyProtection` argument
//! `pallet-revive` does not export, so this crate cannot make it. `acknowledge_receipt` stands
//! in and answers that a code-carrying destination cannot be asked, which reverts the transfer.
//! Skipping the acknowledgement instead would defeat the one guarantee distinguishing the safe
//! variant. Destinations without code have nothing to answer and transfer normally, so this
//! affects contract destinations only.
//!
//! Reaching the acknowledgement will not open this precompile to reentry. The destination is
//! told it already holds the token, which is the whole input to deciding whether to keep it, so
//! it has no reason to read this collection mid-transfer; the call therefore passes
//! `ReentrancyProtection::Strict` and accepts only the `onERC721Received` selector in return.
//! Reentry would let a destination re-enter the transfer path it is standing in, at the risk of
//! every other caller, to save itself a read it can make once the transfer has returned. What
//! the acknowledgement buys the destination is the ability to refuse a token by reverting, and
//! nothing besides.
//!
//! An item definition may be declared soulbound, binding every instance minted from it to the
//! purse key it lands in. Both holder paths then revert, `locked()` answers true and ERC-5192
//! is claimed. The collection owner's `forceTransfer` and `forceBurn` are a separate authority and
//! ignore the flag, so a misdirected mint still has a remedy and a locked token can still be
//! destroyed. Soulbound binds the holder, not the issuer; EIP-5192 reads `locked` as
//! non-transferable outright, and this is the documented deviation from it. A mint announces the
//! status it settled on, `Locked` or `Unlocked`, which is the only point either can be emitted
//! because the flag is fixed when the item is defined.
//!
//! `name()`, `symbol()`, `tokenURI()`, ERC-7572 `contractURI()` and ERC-2981 `royaltyInfo()`
//! read reserved metadata keys rather than dedicated pallet fields, so a collection opts into
//! each by writing the key. A key that is unset, or set to something the method cannot use,
//! answers with an empty value rather than reverting. Collections write these keys by hand and
//! a reverting read is a heavier failure than a missing one: a marketplace that settles a sale
//! through `royaltyInfo` would abort the sale over a mistyped royalty, which costs the seller
//! far more than the royalty was worth.
//!
//! ERC-165 claims the full ERC-721 interface, following the soulbound-token convention of
//! compliant-but-reverting methods for the parts that revert. ERC-7572 defines no interface id,
//! so `contractURI()` is discovered by calling it.
//!
//! ERC-4906 is claimed, and the metadata writers announce a change to what a standard read
//! returns: an instance-scope write to `tokenURI` emits `MetadataUpdate` for that token, a
//! collection- or item-scope write emits `BatchMetadataUpdate` over every id, because the
//! instances a fallback reaches cannot be enumerated. A write to `contractURI` emits ERC-7572's
//! `ContractURIUpdated`. Writes to any other key are silent, since no standard defines an event
//! for them. Only this precompile announces: the pallet's metadata extrinsics change the same
//! state without a log, as native moves do for `Transfer`, so a consumer that follows logs alone
//! misses both.
//! Two interfaces are served but not claimed, because an id covers every function of its
//! interface. `tokenOfOwnerByIndex` answers for wallets that call it, while ERC-721 Enumerable's
//! id also covers `totalSupply` and `tokenByIndex`, which need an instance counter the pallet does
//! not keep. `owner()` answers under ERC-173's name, while that id also covers
//! `transferOwnership`, which cannot exist here: a handover carries the collection's storage
//! deposit, so the successor has to accept and fund it, which is why the handover is a nomination
//! the successor claims.
//!
//! ERC-5192 is claimed, and a mint announces its token's status: `Locked` for an instance of a
//! soulbound item, `Unlocked` otherwise. Transferability is fixed when the item is defined, so a
//! mint is the only point at which either can be emitted.
//!
//! No function of either interface is payable, and both reject a call that attaches value
//! before doing anything else. Accepting it would strand it: a precompile address has no code,
//! no owner and no withdrawal path, so value sent there is unrecoverable by anyone. Rejecting
//! is a revert rather than a trap, because attaching value is a caller mistake the caller can
//! correct.
//!
//! `balanceOf` has to resolve an address back to a purse key, which works only while that key
//! is registered with `AddressMapper`: `to_address` of a native key is a truncated keccak hash
//! and cannot be inverted. Runtimes register the accounts frame-system creates, but a purse key
//! needs no `system::Account`, so a zero-balance holder would have no registration and would
//! read as holding nothing. [`MapPurseKey`] closes that by registering every key an instance
//! lands on, mints and both moves alike, and a runtime wires it through `pallet-scarcity`'s
//! `OnPurseOccupied`. Registration also makes `ownerOf` invertible: an address derived from a
//! registered key resolves back to that key, so a caller can pass a holder address to `mint`,
//! `forceTransfer` or `nominateCollectionOwner` and reach the account it read. That holds for
//! every live holder except one whose account was reaped, which is the case below.
//!
//! Registration costs a permanent entry per key, on the fee-less transfer as well, so a token
//! walking through fresh keys writes one unpaid entry per hop. The alternative is worse: an
//! unregistered holder reads as holding nothing under `balanceOf`, and its address resolves to
//! a different account, which silently misdirects any call that takes an address.
//!
//! Reaping a holder's account still removes its registration while the instance stays put. For
//! that holder `balanceOf` reads 0 unless the key is registered for another reason, while
//! `ownerOf` always reports the key's address, so `ownerOf` is the read to trust for ownership.
//!
//! `InstanceId`s are global across collections while every address names one collection, so
//! every token lookup checks that the instance actually belongs to the address's collection
//! and reverts as unknown otherwise. The prefix also matches addresses whose collection was
//! never created or was deleted; every selector on such an address reverts as an unknown
//! collection, so no unallocated address answers as a live contract.
//!
//! Weights: state-changing calls charge the corresponding `pallet-scarcity` benchmark
//! weights. Read calls charge one database read per worst-case storage access, which
//! over-charges pure computation but never under-charges; a crate benchmark can refine this
//! later.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use core::{marker::PhantomData, num::NonZero};

use frame_support::traits::Get;
use pallet_revive::{
	precompiles::{
		alloy::{
			self,
			primitives::{Address, IntoLogData, U256},
			sol_types::{Revert, SolCall},
		},
		AddressMapper, AddressMatcher, Error, Ext, RuntimeCosts, H160, H256,
	},
	sp_runtime::{DispatchError, Weight},
};
use pallet_scarcity::{
	CollectionId, CollectionMetadata, Collections, Error as ScarcityError, InstanceId,
	InstanceMetadata, Instances, ItemMetadata, MetadataKeyOf, MetadataValueOf, Nft, NftsByOwner,
	Transferability,
};

mod collection;
mod factory;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub use collection::ScarcityCollection;
pub use factory::ScarcityFactory;

alloy::sol!("sol/IScarcity.sol");

/// Reserved collection metadata key backing ERC-721 `name()`.
pub const NAME_KEY: &[u8] = b"name";
/// Reserved collection metadata key backing ERC-721 `symbol()`.
pub const SYMBOL_KEY: &[u8] = b"symbol";
/// Reserved metadata key backing ERC-721 `tokenURI()`, resolved instance, then item, then
/// collection scope.
pub const TOKEN_URI_KEY: &[u8] = b"tokenURI";
/// Reserved collection metadata key backing ERC-7572 `contractURI()`.
pub const CONTRACT_URI_KEY: &[u8] = b"contractURI";
/// Reserved metadata key naming the ERC-2981 royalty recipient as 20 raw address bytes,
/// resolved item, then collection scope.
pub const ROYALTY_RECEIVER_KEY: &[u8] = b"royaltyReceiver";
/// Reserved metadata key holding the ERC-2981 royalty share in basis points, as a
/// SCALE-encoded `u128`, resolved item, then collection scope.
pub const ROYALTY_BASIS_POINTS_KEY: &[u8] = b"royaltyBasisPoints";

/// Denominator of an ERC-2981 royalty fraction expressed in basis points.
pub const BASIS_POINTS_DENOMINATOR: u128 = 10_000;

/// ERC-165 interface identifier of ERC-165 itself.
pub const ERC165_INTERFACE_ID: [u8; 4] = [0x01, 0xff, 0xc9, 0xa7];
/// ERC-165 interface identifier of the ERC-721 core interface.
pub const ERC721_INTERFACE_ID: [u8; 4] = [0x80, 0xac, 0x58, 0xcd];
/// ERC-165 interface identifier of the ERC-721 metadata extension.
pub const ERC721_METADATA_INTERFACE_ID: [u8; 4] = [0x5b, 0x5e, 0x13, 0x9f];
/// ERC-165 interface identifier of ERC-5192, the soulbound extension.
pub const ERC5192_INTERFACE_ID: [u8; 4] = [0xb4, 0x5a, 0x3c, 0x0e];
/// ERC-165 interface identifier of ERC-2981, the royalty standard.
pub const ERC2981_INTERFACE_ID: [u8; 4] = [0x2a, 0x55, 0x20, 0x5a];
/// ERC-165 interface identifier of ERC-4906, the metadata update extension.
///
/// ERC-4906 chose this value rather than deriving it from its event signatures, so it is not the
/// selector XOR the other identifiers here are.
pub const ERC4906_INTERFACE_ID: [u8; 4] = [0x49, 0x06, 0x49, 0x06];

const ERR_UNKNOWN_TOKEN: &str = "unknown token";
const ERR_ZERO_DESTINATION: &str = "destination is the zero address";
const ERR_ZERO_OWNER: &str = "balance query for the zero address";
const ERR_INDEX_OUT_OF_RANGE: &str = "token index out of range for this owner";
const ERR_ZERO_SUCCESSOR: &str = "successor is the zero address";
const ERR_WRONG_HOLDER: &str = "transfer from the wrong holder";
const ERR_SELF_TRANSFER: &str = "destination already holds this instance";
const ERR_NOT_HOLDER: &str =
	"caller does not hold this token: transfers on another holder's authority need approvals, \
	 which are not supported yet";
const ERR_CONTRACT_RECEIVER: &str =
	"safe transfer to a contract is not supported yet: the receiver callback is unavailable";
const ERR_APPROVALS_UNSUPPORTED: &str =
	"approvals are not supported yet: the purse model has no approval mechanism";
const ERR_INVALID_CALLER: &str = "invalid caller";
const ERR_KEY_TOO_LONG: &str = "metadata key too long";
const ERR_VALUE_TOO_LONG: &str = "metadata value too long";
const ERR_KEY_VALUE_MISMATCH: &str = "metadata keys and values differ in length";
const ERR_UNKNOWN_COLLECTION: &str = "unknown collection";
const ERR_UNKNOWN_ITEM: &str = "unknown item";
const ERR_ROYALTY_OVERFLOW: &str = "royalty exceeds the representable range";
const ERR_VALUE_NOT_ACCEPTED: &str = "this precompile does not accept value";
const ERR_RESERVED_NOT_UTF8: &str = "reserved metadata value is not valid UTF-8";

/// Registers a purse key's address when a mint occupies it, for `pallet-scarcity`'s
/// [`OnPurseOccupied`](pallet_scarcity::OnPurseOccupied) hook.
///
/// `pallet-revive` registers addresses when `frame-system` creates an account, but a purse key
/// needs no account: holders pay no deposits and can hold an instance at zero balance. Without
/// this, `balanceOf` cannot resolve such a key back from its address and answers zero for a key
/// that holds a token.
///
/// The entry this writes is permanent and unbacked, and nothing reclaims it: burning or moving
/// the instance leaves it, and `AutoMapper` only unregisters keys that have an account to kill.
/// This is deliberately weaker than `AutoMapper`, whose entries are reclaimed on reap, and is
/// the cost of addressing a key the account system never sees. `map_no_deposit_unchecked`
/// documents this outcome for exactly this case.
///
/// Growth is one entry per occupation that reaches a fresh key, over mints and both moves. A mint
/// is gated by the instance deposit, which is refunded on burn, so mint-and-burn cycles
/// accumulate entries against one recycled deposit; the claims path is bounded by its tree
/// leaves, each claimable once. The holder move is gated by neither, because it is fee-less, so a
/// token walking through fresh keys writes an unpaid entry at every hop. That is the price of the
/// alternative: an unregistered holder reads as holding nothing under `balanceOf`, and its
/// address resolves to a different account, so any call taking a holder address is misdirected.
///
/// A runtime opts out by wiring `()`; this deliberately does not consult `AutoMap`, which
/// governs account-driven registration rather than this.
pub struct MapPurseKey<T>(PhantomData<T>);

impl<T> pallet_scarcity::OnPurseOccupied<T::AccountId> for MapPurseKey<T>
where
	T: pallet_revive::Config,
{
	fn on_purse_occupied(purse: &T::AccountId) {
		// Fails only when the key is already addressable, which is the desired end state.
		let _ = <T as pallet_revive::Config>::AddressMapper::map_no_deposit_unchecked(purse);
	}

	fn on_purse_occupied_weight() -> Weight {
		// The mapped check reads one entry, and registering an unmapped key writes one.
		// `DbWeight` carries only `ref_time`, so the proof both touches is priced here on the
		// same estimate the crate's reads use.
		let ref_time = <T as frame_system::Config>::DbWeight::get().reads_writes(1, 1).ref_time();
		Weight::from_parts(ref_time, 2 * PROOF_SIZE_PER_READ)
	}
}

/// Extract the collection id from a per-collection precompile address.
fn collection_id_of(address: &[u8; 20]) -> CollectionId {
	let bytes: [u8; 4] = address[0..4].try_into().expect("slice is 4 bytes; qed");
	CollectionId::from_be_bytes(bytes)
}

fn revert(reason: &str) -> Error {
	Error::Revert(Revert { reason: reason.into() })
}

/// Map the `pallet-scarcity` errors a caller can trigger to catchable reverts.
///
/// Every error variant the pallet entries called from this crate can return is listed, so a
/// caller never sees a trapped frame for a condition it could have handled. Adding a variant
/// to the pallet does not break this list at compile time; `mapped_scarcity_errors_are_exhaustive`
/// in the tests fails instead.
///
/// Anything else propagates as a plain error, which traps the frame rather than reverting.
fn revert_scarcity<T: pallet_scarcity::Config>(e: DispatchError) -> Error {
	let cases: [(ScarcityError<T>, &str); 20] = [
		(ScarcityError::NoPermission, "caller is not the collection owner"),
		(ScarcityError::UnknownCollection, ERR_UNKNOWN_COLLECTION),
		(ScarcityError::UnknownItem, ERR_UNKNOWN_ITEM),
		(ScarcityError::UnknownInstance, ERR_UNKNOWN_TOKEN),
		(ScarcityError::AddressOccupied, "destination purse already holds an instance"),
		(ScarcityError::SelfTransfer, ERR_SELF_TRANSFER),
		(ScarcityError::Soulbound, "token is soulbound to its purse key"),
		(ScarcityError::SupplyOverflow, "item supply exhausted"),
		(ScarcityError::TooManyInstanceMetadata, "too many instance metadata entries"),
		(ScarcityError::TooManyItems, "item index space exhausted"),
		(ScarcityError::TooManyCollections, "collection id space exhausted"),
		(ScarcityError::TooManyInstances, "instance id space exhausted"),
		(ScarcityError::StateNonceOverflow, "instance state nonce exhausted"),
		(ScarcityError::AlreadyCollectionOwner, "successor is already the collection owner"),
		(ScarcityError::NotPendingCollectionOwner, "caller is not the nominated successor"),
		(ScarcityError::ItemInUse, "item still has live instances"),
		(ScarcityError::ItemMetadataNotEmpty, "item metadata must be removed first"),
		(ScarcityError::CollectionItemsNotEmpty, "item definitions must be deleted first"),
		(ScarcityError::CollectionMetadataNotEmpty, "collection metadata must be removed first"),
		(ScarcityError::DeletionInvariant, "stored counters or deposits do not permit deletion"),
	];
	for (error, reason) in cases {
		if e == DispatchError::from(error) {
			return revert(reason);
		}
	}
	match e {
		// Deposit charges surface as token errors from the collection's consideration.
		DispatchError::Token(_) => revert("collection owner cannot pay the storage deposit"),
		// Deposit and dependency-counter accounting saturate the collection's aggregates.
		DispatchError::Arithmetic(_) => revert("collection accounting overflowed"),
		// The runtime's metadata policy reports why it refused a value as a string, so it is
		// forwarded rather than trapping the frame on a condition the caller could have handled.
		DispatchError::Other(reason) => revert(reason),
		other => other.into(),
	}
}

/// Reject a call carrying native value.
///
/// Every function of both interfaces is ABI-`nonpayable`, so a caller attaching value has made
/// a mistake, and the precompile has no way to make good on it: the address it would land on has
/// no owner, no code and no withdrawal path.
fn ensure_no_value<T: pallet_revive::Config>(env: &impl Ext<T = T>) -> Result<(), Error> {
	if env.value_transferred().is_zero() {
		return Ok(());
	}
	Err(revert(ERR_VALUE_NOT_ACCEPTED))
}

/// The signing account behind the EVM caller.
fn caller_account<T: pallet_revive::Config>(
	env: &mut impl Ext<T = T>,
) -> Result<T::AccountId, Error> {
	env.caller().account_id().cloned().map_err(|_| revert(ERR_INVALID_CALLER))
}

fn account_of<T: pallet_revive::Config>(address: &Address) -> T::AccountId {
	<T as pallet_revive::Config>::AddressMapper::to_account_id(&H160(address.into_array()))
}

fn address_of<T: pallet_revive::Config>(account: &T::AccountId) -> Address {
	Address::from(<T as pallet_revive::Config>::AddressMapper::to_address(account).0)
}

/// Proof size charged per storage read.
///
/// `DbWeight` carries only `ref_time`, but on a parachain every read also pulls trie nodes
/// into the proof. Every value this crate reads is bounded far below this headroom; a crate
/// benchmark can replace the estimate.
const PROOF_SIZE_PER_READ: u64 = 4 * 1024;

/// Ref time charged per metadata byte a policy validation scans.
///
/// One nanosecond per byte, which a UTF-8 scan comfortably beats. Like [`PROOF_SIZE_PER_READ`]
/// this is an estimate, replaceable by a crate benchmark.
const REF_TIME_PER_METADATA_BYTE: u64 = 1_000;

/// Charge `n` worst-case database reads before performing them.
fn charge_reads<T: frame_system::Config + pallet_revive::Config>(
	env: &mut impl Ext<T = T>,
	n: u64,
) -> Result<(), Error> {
	let ref_time = <T as frame_system::Config>::DbWeight::get().reads(n).ref_time();
	env.charge(Weight::from_parts(ref_time, n.saturating_mul(PROOF_SIZE_PER_READ)))?;
	Ok(())
}

/// Look up a live instance and check it belongs to `collection`.
///
/// `InstanceId`s are global, so an instance of another collection must answer as unknown on
/// this collection's address.
fn live_instance<T: pallet_scarcity::Config>(
	collection: CollectionId,
	token: &U256,
) -> Result<(T::AccountId, Nft), Error> {
	let id: InstanceId = u64::try_from(*token).map_err(|_| revert(ERR_UNKNOWN_TOKEN))?;
	let purse = Instances::<T>::get(id).ok_or_else(|| revert(ERR_UNKNOWN_TOKEN))?;
	let nft = NftsByOwner::<T>::get(&purse).ok_or_else(|| revert(ERR_UNKNOWN_TOKEN))?;
	if nft.instance != id || nft.collection != collection {
		return Err(revert(ERR_UNKNOWN_TOKEN));
	}
	Ok((purse, nft))
}

/// Convert one raw key to the pallet's bounded metadata key.
fn bounded_key<T: pallet_scarcity::Config>(
	key: &alloy::primitives::Bytes,
) -> Result<MetadataKeyOf<T>, Error> {
	MetadataKeyOf::<T>::try_from(key.to_vec()).map_err(|_| revert(ERR_KEY_TOO_LONG))
}

/// Convert one raw value to the pallet's bounded metadata value.
fn bounded_value<T: pallet_scarcity::Config>(
	value: &alloy::primitives::Bytes,
) -> Result<MetadataValueOf<T>, Error> {
	MetadataValueOf::<T>::try_from(value.to_vec()).map_err(|_| revert(ERR_VALUE_TOO_LONG))
}

/// Requires UTF-8 under the keys this precompile reads as Solidity `string`s, for
/// `pallet-scarcity`'s [`ValidateMetadata`](pallet_scarcity::ValidateMetadata) policy.
///
/// The pallet stores opaque bytes and reserves no key, so nothing there would stop `name` being
/// set to something no `string` can represent. Wiring this makes all four keys hold their type
/// on every write path the pallet has, extrinsics included, rather than only on the calls that
/// come through here.
///
/// The reads still decode lossily. This is a per-runtime rule, so it says nothing about a runtime
/// that leaves the policy at `()`, and nothing about entries written before it was wired.
pub struct Erc721MetadataPolicy<T>(PhantomData<T>);

impl<T, Key, Value> pallet_scarcity::ValidateMetadata<Key, Value> for Erc721MetadataPolicy<T>
where
	T: pallet_scarcity::Config,
	Key: AsRef<[u8]>,
	Value: AsRef<[u8]>,
{
	fn validate(key: &Key, value: &Value) -> Result<(), DispatchError> {
		let key = key.as_ref();
		let reserved =
			key == NAME_KEY || key == SYMBOL_KEY || key == TOKEN_URI_KEY || key == CONTRACT_URI_KEY;
		if reserved && core::str::from_utf8(value.as_ref()).is_err() {
			return Err(DispatchError::Other(ERR_RESERVED_NOT_UTF8));
		}
		Ok(())
	}

	fn validate_weight(pairs: u32) -> Weight {
		// One pass over a value bounded by `MaxValueLen`, touching no storage.
		let per_value = REF_TIME_PER_METADATA_BYTE
			.saturating_mul(<T as pallet_scarcity::Config>::MaxValueLen::get() as u64);
		Weight::from_parts(per_value.saturating_mul(pairs as u64), 0)
	}
}

/// Convert parallel key and value arrays to the pallet's bounded metadata pairs.
fn bounded_metadata<T: pallet_scarcity::Config>(
	keys: &[alloy::primitives::Bytes],
	values: &[alloy::primitives::Bytes],
) -> Result<Vec<(MetadataKeyOf<T>, MetadataValueOf<T>)>, Error> {
	if keys.len() != values.len() {
		return Err(revert(ERR_KEY_VALUE_MISMATCH));
	}
	keys.iter()
		.zip(values)
		.map(|(key, value)| {
			let key =
				MetadataKeyOf::<T>::try_from(key.to_vec()).map_err(|_| revert(ERR_KEY_TOO_LONG))?;
			let value = MetadataValueOf::<T>::try_from(value.to_vec())
				.map_err(|_| revert(ERR_VALUE_TOO_LONG))?;
			Ok((key, value))
		})
		.collect()
}

/// Deposit an EVM event, charging its weight.
fn deposit_event<T: pallet_revive::Config>(
	env: &mut impl Ext<T = T>,
	event: impl IntoLogData,
) -> Result<(), Error> {
	let (topics, data) = event.into_log_data().split();
	let topics = topics.into_iter().map(|topic| H256(topic.0)).collect::<Vec<_>>();
	env.frame_meter_mut().charge_weight_token(RuntimeCosts::DepositEvent {
		num_topic: topics.len() as u32,
		len: data.len() as u32,
	})?;
	env.deposit_event(topics, data.to_vec());
	Ok(())
}
