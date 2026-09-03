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

//! Coinage system.
//!
//! Allows assets to be represented as fungible coins that can be transferred between peers,
//! split, and consolidated using recyclers. Each instance wraps one asset at one coin unit; the
//! same asset can be wrapped by several instances, one per unit.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
pub mod extension;
pub mod paid_tkn_manager;
pub mod pot;
pub mod recycler_manager;
#[cfg(any(test, feature = "runtime-benchmarks"))]
mod testing_utils;
pub mod weights;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub use indiv_support::traits::ValidateProof;
pub use paid_tkn_manager::*;
pub use pallet::*;
pub use pot::*;
pub use recycler_manager::*;
pub use weights::WeightInfo;

use alloc::{collections::BTreeSet, vec::Vec};
use codec::Encode;
use frame_support::{
	dispatch::{DispatchErrorWithPostInfo, PostDispatchInfo},
	pallet_prelude::*,
	storage::types::{Key as NMapKey, StorageNMap},
	traits::{
		fungible::{self, Mutate as _},
		fungibles::{self, Inspect, Mutate as _, MutateHold as _},
		tokens::{Fortitude, Precision, Preservation, Restriction},
		AccountTouch, Consideration, Defensive, Footprint, IsSubType, UnixTime,
	},
	PalletId,
};
use frame_system::{
	offchain::{CreateAuthorizedTransaction, SubmitTransaction},
	pallet_prelude::*,
};
use indiv_support::{
	traits::{
		Alias, AppendOnlyMembers, AppendOnlyMembersWeightInfo, Context, Identifier,
		MembershipProver, RevisionIndex, RingExponent, RingIndex, RingRootsProvider,
	},
	tx_priority,
	weight_budget::OcwWeightBudget,
};
use pallet_asset_conversion::{QuotePrice, Swap};
use sp_core::H256;
use sp_crypto_hashing::blake2_256;
use sp_runtime::{
	traits::{AccountIdConversion, CheckedAdd, CheckedMul, Convert, Zero},
	ArithmeticError, SaturatedConversion, Saturating,
};
use verifiable::GenerateVerifiable;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	/// The ring-vrf context for interactions with the recycler.
	pub const UNLOADING_RECYCLER_CONTEXT: Context = *b"pop:polkadot.network/coinrecyclr";
	/// Message prefix bound into the ring-VRF proof when recovering a coin from an archived
	/// recycler.
	pub const UNLOAD_ARCHIVED_MSG_PREFIX: &[u8] = b"pop:polkadot.network/coin-unload-archived";
	/// The base for the ring-vrf context for people and lite people free unload token.
	pub const FREE_UNLOAD_TOKEN_CONTEXT_BASE: [u8; 24] = *b"pop:polkadot.net/coinftk";
	/// The base for the ring-vrf context for paid unload token.
	pub const PAID_UNLOAD_TOKEN_CONTEXT_BASE: [u8; 28] = *b"pop:polkadot.net/coinpaidtok";

	/// Base prefix for recycler collection identifiers (one per denomination).
	pub const RECYCLER_COLLECTION_PREFIX: [u8; 16] = *b"coinage/recycler";

	/// Base prefix for paid token collection identifiers (one per period).
	pub const PAID_TOKEN_COLLECTION_PREFIX: [u8; 16] = *b"coinage/paidtkn!";

	/// The maximum number of trie nodes in the non-inclusion proof passed to
	/// [`Call::unload_archived_recycler_into_external_asset`]. Bounds the proof for weight
	/// determinism.
	///
	/// The unloaded-aliases trie is a Substrate 16-ary (nibble) Patricia trie keyed by the 32-byte
	/// [`Alias`], so the deepest possible path is 64 branch nodes plus one leaf node, i.e. 65
	/// nodes.
	/// (In practice the number of key is limited and pseudorandom (but grindable), so this is a
	/// very large estimate).
	pub const MAX_TRIE_PROOF_NODES: u32 = 65;

	/// The maximum encoded length of a single trie node in the non-inclusion proof passed to
	/// [`Call::unload_archived_recycler_into_external_asset`]. Bounds the proof for weight
	/// determinism.
	///
	/// The unloaded-aliases trie is a Substrate 16-ary (nibble) Patricia trie (`LayoutV1`) keyed
	/// by [`Alias`] (a 32-byte key) with empty values, so the largest possible node is a
	/// branch-without-value node: header (≤2 bytes) + partial key (≤32 bytes for a 63-nibble
	/// prefix) + the 16-child bitmap (2 bytes) + 16 child references, each a 32-byte hash encoded
	/// as `Compact(32) ++ hash` (33 bytes). So 564 bytes. We take 1KiB as an upper bound.
	pub const MAX_TRIE_NODE_LEN: u32 = 1024;

	/// The number of seconds after the period where free unload tokens are still accepted.
	///
	/// This allows for a transaction sent at the limit to still have time to be included.
	pub(crate) const FREE_UNLOAD_TOKEN_GRACE_WINDOW: u32 = 3600;

	/// Maximum number of storage entries to remove per dust cleanup call.
	pub(crate) const DUST_CLEANUP_BATCH_SIZE: u32 = 1000;

	/// Tx validity tag prefix for cleaning paid unload token dust.
	pub(crate) const CLEAN_PAID_UNLOAD_TOKEN_DUST_TX_TAG_PREFIX: &str =
		"coinage:clean-paid-unload-token-dust";

	/// Fixed onboarding size for recycler collections.
	///
	/// Must stay at one to prevent coins from being locked for an indeterminate time.
	pub(crate) const RECYCLER_ONBOARDING_SIZE: u32 = 1;

	/// The onboarding size for paid unload token collections.
	///
	/// Must stay at one to prevent paid unload tokens from being locked for an indeterminate time.
	pub(crate) const PAID_UNLOAD_TOKEN_ONBOARDING_SIZE: u32 = 1;

	/// Time period index used for unload token collections and context derivation.
	pub type Period = u32;

	/// Big-endian encoded period for storage keys where iteration order must match
	/// numeric order. SCALE encodes `u32` as little-endian, which breaks lexicographic
	/// ordering under the `Identity` hasher.
	pub type BigEndianPeriod = indiv_support::utils::BigEndianU32;

	/// Generate the complete context for free unload tokens:
	/// `FREE_UNLOAD_TOKEN_CONTEXT_BASE || period (u32 le-encoded) || counter (u32 le-encoded)`.
	pub fn free_unload_token_context(period: Period, counter: u32) -> [u8; 32] {
		let mut c = [0u8; 32];
		c[..24].copy_from_slice(FREE_UNLOAD_TOKEN_CONTEXT_BASE.as_ref());
		c[24..28].copy_from_slice(&period.to_le_bytes());
		c[28..32].copy_from_slice(&counter.to_le_bytes());
		c
	}

	/// The log target for the pallet.
	pub(crate) const LOG_TARGET: &str = "runtime::indiv-pallet-coinage";

	/// Maximum number of consumed free unload tokens removed per `clean_consumed_free_token` call.
	///
	/// This bounds the call's worst-case proof size so one invocation fits a single block's
	/// `Normal` extrinsic budget. This property is asserted in the `integrity_test`.
	pub(crate) const CLEAN_CONSUMED_FREE_TOKEN_LIMIT: u32 = 1000;

	pub type CryptoOf<T> = <<T as Config>::MemberService as MembershipProver>::Crypto;
	pub type MemberOf<T> = <CryptoOf<T> as GenerateVerifiable>::Member;
	/// The ring-VRF membership root commitment (the "recycler root").
	pub type MembersOf<T> = <CryptoOf<T> as GenerateVerifiable>::Members;
	pub type ProofOf<T> = <CryptoOf<T> as GenerateVerifiable>::Proof;
	pub type SignatureOf<T> = <CryptoOf<T> as GenerateVerifiable>::Signature;
	pub type FungiblesBalanceOf<T> = <<T as Config>::Fungibles as fungibles::Inspect<
		<T as frame_system::Config>::AccountId,
	>>::Balance;
	pub type FungiblesAssetIdOf<T> = <<T as Config>::Fungibles as fungibles::Inspect<
		<T as frame_system::Config>::AccountId,
	>>::AssetId;
	pub type NativeBalanceOf<T> = <<T as Config>::NativeFungible as fungible::Inspect<
		<T as frame_system::Config>::AccountId,
	>>::Balance;

	/// The denomination is represented as an exponent of 2 multiplied to its instance's
	/// [`InstanceRecord::asset_unit`], i.e., value = 2^(Denomination) * asset_unit.
	pub type Denomination = i8;

	/// Identifier of a coinage instance. Each instance wraps one underlying asset.
	///
	/// Instances are allocated sequentially by [`Pallet::create_sufficient_instance`] and recorded
	/// in [`Instances`].
	pub type InstanceId = u32;

	/// Whether the instance's minimum coin is deemed valuable enough by the admin to cover
	/// the cost of loading a coin into a recycler until it is unloaded or let there forever.
	/// This is because the the coin's lifecycle is paid at the unloading.
	#[derive(
		Copy,
		Clone,
		PartialEq,
		Eq,
		Debug,
		Encode,
		Decode,
		TypeInfo,
		MaxEncodedLen,
		DecodeWithMemTracking,
	)]
	pub enum InstanceMode {
		/// The sufficiently valuable instance: no pot, loads take no deposit.
		Sufficient,
		/// Load-side costs are underwritten by the instance's pot.
		Sponsored,
	}

	/// The configuration and load-deposit ledger of one coinage instance.
	///
	/// Carries only what differs per underlying asset. The denomination range and the values
	/// feeding the unload token fee stay [`Config`] constants, because a paid unload token can be
	/// consumed to unload any instance.
	#[derive(Encode, Decode, TypeInfo, MaxEncodedLen)]
	#[scale_info(skip_type_params(T))]
	pub struct InstanceRecord<T: Config> {
		/// The underlying asset backing every coin of this instance.
		pub asset_id: FungiblesAssetIdOf<T>,
		/// The asset amount of a coin of denomination zero.
		pub asset_unit: FungiblesBalanceOf<T>,
		/// Whether the instance's loads take a pot deposit.
		pub mode: InstanceMode,
		/// The tier the instance last loaded at, `None` until its first sponsored load.
		///
		/// Only ever `Some` with a non-zero count: a drained one is dropped.
		pub current_load_deposit: Option<DepositTier<FungiblesAssetIdOf<T>, FungiblesBalanceOf<T>>>,
		/// The superseded tier still holding deposits, `None` when there is none.
		///
		/// Only ever `Some` with a non-zero count: a drained one is dropped, which is what frees
		/// the slot the next rotation needs.
		///
		/// (Only one old tier, because the deposit value won't change often and can always be
		/// collapsed with [`Pallet::collapse_load_deposits`]).
		pub old_load_deposit: Option<DepositTier<FungiblesAssetIdOf<T>, FungiblesBalanceOf<T>>>,
		/// The account that created the instance and the [`Config::InstanceCreationDeposit`]
		/// ticket it paid for this instance's permanent footprint, `None` when the instance was
		/// created sufficient or made sufficient.
		pub creator: Option<(T::AccountId, T::InstanceCreationDeposit)>,
	}

	impl<T: Config> InstanceRecord<T> {
		/// The number of redeemable keys backed by the ledger, both tiers summed.
		///
		/// This may be lower than the number of keys that have been loaded, in case the instance
		/// moved from sufficient to sponsored. Keys loaded while sufficient are not backed by
		/// the ledger.
		pub(crate) fn load_deposit_key_count(&self) -> u32 {
			let old = self.old_load_deposit.as_ref().map_or(0, |tier| tier.count);
			let current = self.current_load_deposit.as_ref().map_or(0, |tier| tier.count);
			old.saturating_add(current)
		}
	}

	/// One tier of live load deposits: `count` keys, each backed by `price` of `asset_id`.
	#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, TypeInfo, MaxEncodedLen)]
	pub struct DepositTier<AssetId, Balance> {
		/// The asset id the deposits of this tier are held in.
		pub asset_id: AssetId,
		/// The deposit held per key of this tier.
		pub price: Balance,
		/// The number of redeemable keys backed at this tier.
		pub count: u32,
	}

	impl<AssetId: PartialEq, Balance: PartialEq> DepositTier<AssetId, Balance> {
		/// Whether this tier's deposits are held in `asset_id` at `price`.
		pub(crate) fn is_priced_at(&self, asset_id: &AssetId, price: &Balance) -> bool {
			self.asset_id == *asset_id && self.price == *price
		}
	}

	/// Invalidity reasons for the transaction extension validation.
	#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, TypeInfo)]
	pub enum CustomInvalidity {
		NoCoin = 35,
		CoinTooOld = 36,
		SplitIntoNotSorted = 37,
		SplitExponentTooSmall = 38,
		InternalError = 39,
		SplitTooBig = 40,
		InvalidSplit = 41,
		EmptySplit = 42,
		TooManySplits = 43,
		MemberKeyAlreadyUsed = 45,
		RecyclerAlreadyUnloaded = 46,
		DenominationTooBig = 47,
		DenominationTooSmall = 48,
		DenominationOutOfBound = 49,
		NoUnloadingRecycler = 50,
		InvalidMemberKey = 51,
		InvalidCall = 52,
		AddressAlreadyHasCoin = 53,
		OriginToAsCoinMustBeSigned = 54,
		InvalidUnloadTokenProof = 55,
		InvalidUnloadTokenPeriod = 56,
		UnloadTokenAlreadyConsumed = 57,
		/// The free unload token counter has reached the limit for the period.
		UnloadTokenCounterOutOfRange = 58,
		/// The recycler revision does not match (recycler may not exist or has been rebuilt).
		InvalidRecyclerRevision = 59,
		NoRecycler = 60,
		TransactionNotLocal = 61,
		NothingToBuild = 62,
		SplitExponentTooBig = 63,
		InvalidUnloadTokenPeriodOrRingIndex = 64,
		DuplicateDestinationsInSplit = 65,
		CoinAmountBelowFee = 66,
		InvalidProofOfOwnership = 67,
		/// The fee is below `MinimumExponentForOutputUnloadFee`.
		FeeCoinBelowMinimum = 68,
		/// The output-fee extension (`AsUnloadTokenFromOutput`)
		/// can only be used with external asset and coin unload calls.
		FromOutputFeeNotAllowed = 69,
		/// The alias proofs array is empty.
		EmptyAliasProofs = 70,
		/// The paid token ring revision does not match (ring may not exist or has been rebuilt).
		InvalidPaidTokenRingRevision = 71,
		/// The underlying asset cannot be converted into the native currency to pay the fee, so it
		/// cannot be used as the fee currency for now.
		CannotConvertAssetToNative = 73,
		/// The coin cannot be used yet because it is temporarily locked after a failed dispatch.
		/// The lock duration grows exponentially with each consecutive failure.
		CoinTemporarilyLocked = 74,
		/// One of the alias proofs failed verification.
		InvalidAliasProof = 75,
		/// The max_fee parameter is insufficient to cover the unload fee.
		MaxFeeInsufficientForUnload = 76,
		/// [`Call::unload_recycler_into_coins`] with [`UnloadFee::Prepaid`] requires `max_fee` to
		/// be 0.
		MaxFeeNotAllowedForPrepaid = 77,
		/// The denomination cannot be losslessly converted to an asset amount because the
		/// instance's `asset_unit` is not evenly divisible by `2^|value|`.
		LossyDenominationConversion = 78,
		/// The first alias in the call does not match the alias derived from the first proof
		/// validated in the extension.
		FirstCallAliasMismatch = 79,
		/// The `InfallibleUnpaidSigned` extension requires a signed origin.
		InfallibleUnpaidSignedOriginMustBeSigned = 80,
		/// The caller does not have enough of the underlying asset to cover the load amount
		/// required by the `InfallibleUnpaidSigned` extension.
		InfallibleUnpaidSignedInsufficientBalance = 81,
		/// The mixed-output unload call is missing aliases or loaded-coin outputs.
		EmptyMixedOutput = 83,
		/// A batched `load_recycler_with_external_asset_unpaid` call has no inner items.
		EmptyUnpaidLoadBatch = 84,
		/// No coinage instance exists for the given [`InstanceId`].
		InstanceNotFound = 85,
		/// The recycler alias cannot be used yet because it is temporarily locked after a failed
		/// dispatch.
		AliasTemporarilyLocked = 86,
		/// The sponsored instance's pot cannot fund this load's deposit.
		PotCannotCoverLoadDeposit = 87,
		/// The load deposit changed while the sponsored instance's old tier still holds deposits,
		/// so the instance needs [`Pallet::collapse_load_deposits`] before it can load again.
		LoadDepositOldTierOccupied = 88,
		/// The unload call has no inputs, so it can never succeed.
		EmptyInputs = 89,
		/// The value being unloaded does not cover the network unload fee taken out of it.
		UnloadedValueBelowFee = 90,
	}

	impl From<CustomInvalidity> for TransactionValidityError {
		fn from(e: CustomInvalidity) -> Self {
			InvalidTransaction::Custom(e as u8).into()
		}
	}

	pub(crate) enum MixedOutputValidationError {
		EmptyAliases,
		EmptyLoadedCoins,
		InvalidSplit,
		MemberKeyAlreadyUsed,
		InvalidMemberKey,
		Denomination(DenominationToAssetAmountError),
	}

	impl MixedOutputValidationError {
		fn into_pallet_error<T: Config>(self) -> Error<T> {
			match self {
				MixedOutputValidationError::EmptyAliases => Error::<T>::EmptyInputs,
				MixedOutputValidationError::EmptyLoadedCoins |
				MixedOutputValidationError::InvalidSplit => Error::<T>::InvalidSplit,
				MixedOutputValidationError::MemberKeyAlreadyUsed =>
					Error::<T>::MemberKeyAlreadyUsed,
				MixedOutputValidationError::InvalidMemberKey => Error::<T>::InvalidMemberKey,
				MixedOutputValidationError::Denomination(e) => e.into_pallet_error::<T>(),
			}
		}

		fn into_custom_invalidity(self) -> CustomInvalidity {
			match self {
				MixedOutputValidationError::EmptyAliases |
				MixedOutputValidationError::EmptyLoadedCoins => CustomInvalidity::EmptyMixedOutput,
				MixedOutputValidationError::InvalidSplit => CustomInvalidity::InvalidSplit,
				MixedOutputValidationError::MemberKeyAlreadyUsed =>
					CustomInvalidity::MemberKeyAlreadyUsed,
				MixedOutputValidationError::InvalidMemberKey => CustomInvalidity::InvalidMemberKey,
				MixedOutputValidationError::Denomination(e) => e.into_custom_invalidity(),
			}
		}
	}

	/// The token that is necessary to unload a recycler.
	#[derive(
		Clone, PartialEq, Eq, Debug, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen,
	)]
	pub enum UnloadToken {
		/// A free token used by a person, for a period at a count.
		///
		/// A person may use up to a number of tokens based on their allowance per period and the
		/// current price of the unload token.
		FreePeople { period: Period, counter: u32 },
		/// A free token used by a lite person, for a period at a count.
		///
		/// A lite person may use up to a number of tokens based on their allowance per period and
		/// the current price of the unload token.
		FreePeopleLite { period: Period, counter: u32 },
		/// A paid token used by anyone.
		Paid,
	}

	/// A coin with a value and an age.
	///
	/// The age tracks how many times the coin has been transferred or split. After a certain age,
	/// the coin can't be transferred or split. It can be recycled or directly offboarded into the
	/// underlying external asset, with the latter revealing its transfer chain.
	#[derive(
		Copy,
		Clone,
		PartialEq,
		Eq,
		Debug,
		Encode,
		Decode,
		DecodeWithMemTracking,
		TypeInfo,
		MaxEncodedLen,
	)]
	pub struct Coin {
		/// The instance the coin belongs to.
		pub instance_id: InstanceId,
		/// The value of the coin.
		pub value: Denomination,
		/// The age of the coin. The age increases by one on each transfer or split. After a
		/// certain age, the coin can't be transferred or split. It can either be recycled or
		/// directly offboarded, with the latter revealing its transfer chain.
		pub age: u16,
	}

	/// The reason why a coin is temporarily locked.
	#[derive(
		Copy,
		Clone,
		PartialEq,
		Eq,
		Debug,
		Encode,
		Decode,
		DecodeWithMemTracking,
		TypeInfo,
		MaxEncodedLen,
	)]
	pub enum LockReason {
		/// The previous dispatch using this coin failed after prepare.
		FailedDispatch {
			/// The number of times this coin has been retried after a failed dispatch.
			/// Starts at 0 on the first failure.
			retries: u8,
		},
	}

	/// Metadata for a temporarily locked coin or recycler alias.
	#[derive(
		Copy,
		Clone,
		PartialEq,
		Eq,
		Debug,
		Encode,
		Decode,
		DecodeWithMemTracking,
		TypeInfo,
		MaxEncodedLen,
	)]
	pub struct LockInfo {
		/// Why the coin is locked.
		pub reason: LockReason,
		/// Unix timestamp (seconds) at which the lock expires.
		pub until: u64,
	}

	/// State of a recycler alias in [`RecyclerAliasStates`].
	///
	/// An alias is either temporarily locked after a failed dispatch or permanently consumed
	/// by a successful unload. When the entry is absent, the alias is available again.
	#[derive(
		Copy,
		Clone,
		PartialEq,
		Eq,
		Debug,
		Encode,
		Decode,
		DecodeWithMemTracking,
		TypeInfo,
		MaxEncodedLen,
	)]
	pub enum AliasState {
		/// Temporarily locked after a failed dispatch; reusable once `LockInfo::until` passes.
		Locked(LockInfo),
		/// Permanently consumed by a successful unload.
		Unloaded,
	}

	/// Input for unloading a recycler.
	#[derive(
		CloneNoBound,
		PartialEqNoBound,
		EqNoBound,
		DebugNoBound,
		Encode,
		Decode,
		DecodeWithMemTracking,
		TypeInfo,
	)]
	#[scale_info(skip_type_params(AliasesBound))]
	pub struct UnloadRecyclerInput<AliasesBound: Get<u32>> {
		/// The value of the recycler.
		pub value: Denomination,
		/// The recycler's ring index.
		pub index: RingIndex,
		/// The revision of the recycler ring.
		pub revision: RevisionIndex,
		/// The aliases to be unloaded.
		pub aliases: BoundedVec<Alias, AliasesBound>,
	}

	/// A single inner item of [`Call::load_recycler_with_external_asset_unpaid_batch`].
	#[derive(
		CloneNoBound,
		PartialEqNoBound,
		EqNoBound,
		DebugNoBound,
		Encode,
		Decode,
		DecodeWithMemTracking,
		TypeInfo,
	)]
	#[scale_info(skip_type_params(T))]
	pub struct UnpaidLoadInput<T: Config> {
		/// Whether to preserve the signer's account when transferring the underlying asset.
		pub preservation: CodecPreservation,
		/// The denomination of the recycler the member key is being loaded into.
		pub value: Denomination,
		/// The new member key being loaded.
		pub member_key: MemberOf<T>,
		/// Signature of the signer's account id by `member_key`.
		pub proof_of_ownership: SignatureOf<T>,
	}

	/// The mode by which we describe whether an operation should keep an account alive.
	///
	/// This is just a version of [Preservation] that implements codec.
	#[derive(
		Copy,
		Clone,
		Debug,
		Eq,
		PartialEq,
		Encode,
		Decode,
		DecodeWithMemTracking,
		TypeInfo,
		MaxEncodedLen,
	)]
	pub enum CodecPreservation {
		/// We don't care if the account gets killed by this operation.
		Expendable,
		/// The account may not be killed, but we don't care if the balance gets dusted.
		Protect,
		/// The account may not be killed and our provider reference must remain (in the context of
		/// tokens, this means that the account may not be dusted).
		Preserve,
	}

	impl From<CodecPreservation> for Preservation {
		fn from(x: CodecPreservation) -> Self {
			match x {
				CodecPreservation::Protect => Self::Protect,
				CodecPreservation::Preserve => Self::Preserve,
				CodecPreservation::Expendable => Self::Expendable,
			}
		}
	}

	impl CodecPreservation {
		/// Returns whichever of `self` and `other` is the more restrictive preservation, ordered
		/// `Preserve` > `Protect` > `Expendable`.
		pub fn strictest(self, other: Self) -> Self {
			match (self, other) {
				(Self::Preserve, _) | (_, Self::Preserve) => Self::Preserve,
				(Self::Protect, _) | (_, Self::Protect) => Self::Protect,
				_ => Self::Expendable,
			}
		}
	}

	/// The currency to use for paying fees.
	#[derive(
		Copy,
		Clone,
		Debug,
		Eq,
		PartialEq,
		Encode,
		Decode,
		DecodeWithMemTracking,
		TypeInfo,
		MaxEncodedLen,
	)]
	pub enum FeeCurrency {
		/// Pay the fee with the native currency.
		Native,
		/// Pay the fee with the underlying external asset.
		ExternalAsset,
	}

	/// Archival commitment for a cleaned recycler ring that still has recoverable coins.
	#[derive(Clone, Debug, Eq, PartialEq, Encode, Decode, TypeInfo, MaxEncodedLen)]
	pub struct ArchivedRecycler {
		/// `blake2_256(unloaded_aliases_root ++ recycler_root)`, see [`archive_commitment`].
		pub commitment: H256,
		/// Number of not-yet-recovered coins still backed by this archive.
		pub remaining: u32,
	}

	/// Compute the archival commitment binding the unloaded-aliases trie root and the ring-VRF
	/// recycler root: `blake2_256((unloaded_root, recycler_root).encode())`.
	///
	/// The single definition of the commitment formula stored in [`ArchivedRecycler`]: used when
	/// archiving ([`RecyclerManager::clean_unchecked`]), when verifying and updating an archive
	/// ([`RecyclerManager::unload_archived`]), and by tests/benchmarks.
	pub fn archive_commitment(unloaded_root: H256, recycler_root: &impl Encode) -> H256 {
		H256::from((unloaded_root, recycler_root).using_encoded(blake2_256))
	}

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	/// All the coins in all instances currently circulating, keyed by owner.
	///
	/// A coin is minted when unloaded from a recycler, and destroyed when loaded into one.
	#[pallet::storage]
	pub type CoinsByOwner<T: Config> =
		StorageMap<_, Blake2_128Concat, T::AccountId, Coin, OptionQuery>;

	/// Temporary lock expiry for coins that previously failed dispatch, keyed by owner.
	///
	/// An entry is locked until the stored Unix timestamp, preventing repeated failed dispatch
	/// attempts in a short period.
	#[pallet::storage]
	pub type LockedCoins<T: Config> =
		StorageMap<_, Blake2_128Concat, T::AccountId, LockInfo, OptionQuery>;

	/// The total value of coins that were burnt, keyed by instance.
	///
	/// This tracks value that is intentionally destroyed as part of protocol flows (for example:
	/// recycler expiration cleanup and fee remainder burning). This storage item keeps track of
	/// the total value of such destroyed coins.
	#[pallet::storage]
	pub type TotalValueOfDestroyedCoins<T> =
		StorageMap<_, Twox64Concat, InstanceId, FungiblesBalanceOf<T>, ValueQuery>;

	/// Consumed free unload tokens by period and alias.
	///
	/// This storage keeps track of the free unload tokens that have been consumed by people
	/// and lite people, to avoid double spending.
	///
	/// It is cleared periodically.
	#[pallet::storage]
	pub type ConsumedFreeUnloadTokens<T: Config> =
		StorageDoubleMap<_, Twox64Concat, Period, Twox64Concat, Alias, ()>;

	// By convention, all storage items handled by [`RecyclerManager`] starts with `Recycler` or
	// `Recyclers`.

	/// Tracks whether a recycler collection exists for a given instance and denomination.
	///
	/// [`Pallet::create_sufficient_instance`] creates one recycler collection per denomination in
	/// `[MinimumExponent, MaximumExponent]`.
	///
	/// **WARNING**: Do not use this storage directly, use [`RecyclerManager`] type instead.
	///
	/// This storage item is managed by [`RecyclerManager`] and is part of a consistent set:
	/// * [RecyclerCollectionCreated] - whether the collection exists for an instance and
	///   denomination.
	/// * [RecyclersLastRemovedRingIndex] - the last removed ring index for each instance and
	///   denomination.
	/// * [RecyclersCoinToRecycler] - the mapping from member key to the instance and denomination
	///   it is in.
	/// * [RecyclerAliasStates] - per-alias lock/unloaded state, indexed by instance, denomination
	///   and ring index.
	/// * [RecyclersUnloadedCount] - the number of unloaded aliases of each ring.
	/// * [RecyclersDusting] - marks rings with deferred recycler dust pending removal.
	/// * [RecyclersArchives] - archival commitments for cleaned rings that still hold recoverable
	///   coins.
	///
	/// Ring members, pending members, and ring state are managed by [`Config::MemberService`].
	#[pallet::storage]
	pub type RecyclerCollectionCreated<T> =
		StorageDoubleMap<_, Twox64Concat, InstanceId, Twox64Concat, Denomination, (), OptionQuery>;

	/// Last removed ring index per instance and recycler denomination.
	///
	/// Rings are removed sequentially starting from index 0. The next ring to check for
	/// expiration is `last_removed + 1` (or `0` if nothing has been removed yet).
	///
	/// **WARNING**: Do not use this storage directly, use [`RecyclerManager`] type instead.
	///
	/// This storage item is managed by [`RecyclerManager`] and is part of a consistent set:
	/// * [RecyclerCollectionCreated] - whether the collection exists for an instance and
	///   denomination.
	/// * [RecyclersLastRemovedRingIndex] - the last removed ring index for each instance and
	///   denomination.
	/// * [RecyclersCoinToRecycler] - the mapping from member key to the instance and denomination
	///   it is in.
	/// * [RecyclerAliasStates] - per-alias lock/unloaded state, indexed by instance, denomination
	///   and ring index.
	/// * [RecyclersUnloadedCount] - the number of unloaded aliases of each ring.
	/// * [RecyclersDusting] - marks rings with deferred recycler dust pending removal.
	/// * [RecyclersArchives] - archival commitments for cleaned rings that still hold recoverable
	///   coins.
	///
	/// Ring members, pending members, and ring state are managed by [`Config::MemberService`].
	#[pallet::storage]
	pub type RecyclersLastRemovedRingIndex<T> = StorageDoubleMap<
		_,
		Twox64Concat,
		InstanceId,
		Twox64Concat,
		Denomination,
		RingIndex,
		OptionQuery,
	>;

	/// Mapping from a recycler member key to the instance and denomination it belongs to.
	///
	/// When a coin is loaded into a recycler, the member key is recorded here so that the
	/// pallet can look up which instance and denomination the member key corresponds to.
	///
	/// **WARNING**: Do not use this storage directly, use [`RecyclerManager`] type instead.
	///
	/// This storage item is managed by [`RecyclerManager`] and is part of a consistent set:
	/// * [RecyclerCollectionCreated] - whether the collection exists for an instance and
	///   denomination.
	/// * [RecyclersLastRemovedRingIndex] - the last removed ring index for each instance and
	///   denomination.
	/// * [RecyclersCoinToRecycler] - the mapping from member key to the instance and denomination
	///   it is in.
	/// * [RecyclerAliasStates] - per-alias lock/unloaded state, indexed by instance, denomination
	///   and ring index.
	/// * [RecyclersUnloadedCount] - the number of unloaded aliases of each ring.
	/// * [RecyclersDusting] - marks rings with deferred recycler dust pending removal.
	/// * [RecyclersArchives] - archival commitments for cleaned rings that still hold recoverable
	///   coins.
	///
	/// Ring members, pending members, and ring state are managed by [`Config::MemberService`].
	#[pallet::storage]
	pub type RecyclersCoinToRecycler<T> =
		StorageMap<_, Blake2_128Concat, MemberOf<T>, (InstanceId, Denomination), OptionQuery>;

	/// State of recycler aliases, indexed by `(instance, denomination, ring index, alias)`.
	///
	/// Each entry records either a temporary failed-dispatch lock or a permanently consumed
	/// alias. Absence from the map means the alias is available.
	///
	/// **WARNING**: Do not use this storage directly, use [`RecyclerManager`] type instead.
	///
	/// This storage item is managed by [`RecyclerManager`] and is part of a consistent set:
	/// * [RecyclerCollectionCreated] - whether the collection exists for an instance and
	///   denomination.
	/// * [RecyclersLastRemovedRingIndex] - the last removed ring index for each instance and
	///   denomination.
	/// * [RecyclersCoinToRecycler] - the mapping from member key to the instance and denomination
	///   it is in.
	/// * [RecyclerAliasStates] - per-alias lock/unloaded state, indexed by instance, denomination
	///   and ring index.
	/// * [RecyclersUnloadedCount] - the number of unloaded aliases of each ring.
	/// * [RecyclersDusting] - marks rings with deferred recycler dust pending removal.
	/// * [RecyclersArchives] - archival commitments for cleaned rings that still hold recoverable
	///   coins.
	///
	/// Ring members, pending members, and ring state are managed by [`Config::MemberService`].
	#[pallet::storage]
	pub type RecyclerAliasStates<T> = StorageNMap<
		_,
		(
			NMapKey<Twox64Concat, InstanceId>,
			NMapKey<Twox64Concat, Denomination>,
			NMapKey<Twox64Concat, RingIndex>,
			NMapKey<Twox64Concat, Alias>,
		),
		AliasState,
		OptionQuery,
	>;

	/// Number of aliases unloaded from each recycler ring.
	///
	/// Equals the number of [RecyclerAliasStates] entries of the ring in state
	/// [`AliasState::Unloaded`], so `RingStatus::total` minus this value is the number of coins the
	/// ring still holds. Absent for a ring that already had alias states when the count was
	/// introduced, because recovering its number needs a scan; such a ring is never counted.
	///
	/// **WARNING**: Do not use this storage directly, use [`RecyclerManager`] type instead.
	///
	/// This storage item is managed by [`RecyclerManager`] and is part of a consistent set:
	/// * [RecyclerCollectionCreated] - whether the collection exists for an instance and
	///   denomination.
	/// * [RecyclersLastRemovedRingIndex] - the last removed ring index for each instance and
	///   denomination.
	/// * [RecyclersCoinToRecycler] - the mapping from member key to the instance and denomination
	///   it is in.
	/// * [RecyclerAliasStates] - per-alias lock/unloaded state, indexed by instance, denomination
	///   and ring index.
	/// * [RecyclersUnloadedCount] - the number of unloaded aliases of each ring.
	/// * [RecyclersDusting] - marks rings with deferred recycler dust pending removal.
	/// * [RecyclersArchives] - archival commitments for cleaned rings that still hold recoverable
	///   coins.
	///
	/// Ring members, pending members, and ring state are managed by [`Config::MemberService`].
	#[pallet::storage]
	pub type RecyclersUnloadedCount<T> =
		StorageMap<_, Twox64Concat, (InstanceId, Denomination, RingIndex), u32, OptionQuery>;

	/// Marks recycler rings that have deferred recycler dust pending removal.
	///
	/// When a recycler ring is removed, the cleanup of its leftover alias states in
	/// [RecyclerAliasStates] is performed gradually through this storage item. An entry here
	/// indicates that entries in [RecyclerAliasStates] for the given instance, denomination and
	/// ring index still exist and should be dusted.
	///
	/// **WARNING**: Do not use this storage directly, use [`RecyclerManager`] type instead.
	///
	/// This storage item is managed by [`RecyclerManager`] and is part of a consistent set:
	/// * [RecyclerCollectionCreated] - whether the collection exists for an instance and
	///   denomination.
	/// * [RecyclersLastRemovedRingIndex] - the last removed ring index for each instance and
	///   denomination.
	/// * [RecyclersCoinToRecycler] - the mapping from member key to the instance and denomination
	///   it is in.
	/// * [RecyclerAliasStates] - per-alias lock/unloaded state, indexed by instance, denomination
	///   and ring index.
	/// * [RecyclersUnloadedCount] - the number of unloaded aliases of each ring.
	/// * [RecyclersDusting] - marks rings with deferred recycler dust pending removal.
	/// * [RecyclersArchives] - archival commitments for cleaned rings that still hold recoverable
	///   coins.
	///
	/// Ring members, pending members, and ring state are managed by [`Config::MemberService`].
	#[pallet::storage]
	pub type RecyclersDusting<T> =
		StorageMap<_, Twox64Concat, (InstanceId, Denomination, RingIndex), (), OptionQuery>;

	/// Archival commitments for cleaned recycler rings that still hold recoverable coins.
	///
	/// When a recycler ring is cleaned (see [`RecyclerManager::clean_unchecked`]) while it still
	/// has at least one not-unloaded alias, an [`ArchivedRecycler`] is recorded here keyed by
	/// `(instance, denomination, ring index)`. It commits to the trie of unloaded aliases and
	/// the recycler root. Not-unloaded coins can still be unloaded with
	/// [`Pallet::unload_archived_recycler_into_external_asset`], which updates the archive. The
	/// archive is removed once all coins have been unloaded.
	///
	/// Only the commitments are stored on-chain. The unloaded-aliases trie and the ring can be
	/// reconstructed offchain to build the recovery proofs by listening to the
	/// [`Event::RecyclerAliasUnloaded`], [`Event::RecyclerArchived`] and
	/// [`Event::ArchivedRecyclerUnloadedIntoExternalAsset`] events.
	///
	/// **WARNING**: Do not use this storage directly, use [`RecyclerManager`] type instead.
	///
	/// This storage item is managed by [`RecyclerManager`] and is part of a consistent set:
	/// * [RecyclerCollectionCreated] - whether the collection exists for an instance and
	///   denomination.
	/// * [RecyclersLastRemovedRingIndex] - the last removed ring index for each instance and
	///   denomination.
	/// * [RecyclersCoinToRecycler] - the mapping from member key to the instance and denomination
	///   it is in.
	/// * [RecyclerAliasStates] - per-alias lock/unloaded state, indexed by instance, denomination
	///   and ring index.
	/// * [RecyclersUnloadedCount] - the number of unloaded aliases of each ring.
	/// * [RecyclersDusting] - marks rings with deferred recycler dust pending removal.
	/// * [RecyclersArchives] - archival commitments for cleaned rings that still hold recoverable
	///   coins.
	///
	/// Ring members, pending members, and ring state are managed by [`Config::MemberService`].
	#[pallet::storage]
	pub type RecyclersArchives<T> = StorageMap<
		_,
		Twox64Concat,
		(InstanceId, Denomination, RingIndex),
		ArchivedRecycler,
		OptionQuery,
	>;

	// By convention, all storage items handled by [`PaidTknManager`] starts with `PaidUnloadToken`
	// or `PaidToken`.

	/// Mapping from a paid token member key to the period it belongs to.
	///
	/// When a user pays for a recycler unload token, the member key is recorded here so
	/// that the pallet can look up which period the member key corresponds to.
	///
	/// **WARNING**: Do not use this storage directly, use [`PaidTknManager`] type instead.
	///
	/// This storage item is managed by [`PaidTknManager`] and is part of a consistent set:
	/// * [PaidUnloadTokenMembers] - tracks registered member keys.
	/// * [PaidUnloadTokenConsumed] - the consumed paid unload token aliases.
	/// * [PaidTokenCollectionsCreated] - whether the collection exists for a period.
	/// * [PaidUnloadTokenDusting] - marks periods with consumed tokens pending removal.
	///
	/// Ring members, pending members, and ring state are managed by [`Config::MemberService`].
	#[pallet::storage]
	pub type PaidUnloadTokenMembers<T> = StorageMap<_, Blake2_128Concat, MemberOf<T>, ()>;

	/// Consumed paid unload tokens by period, ring index and alias.
	///
	/// When a paid unload token is consumed, the alias produced by the ring-VRF proof is
	/// stored here to prevent double-spending within the same ring.
	///
	/// **WARNING**: Do not use this storage directly, use [`PaidTknManager`] type instead.
	///
	/// This storage item is managed by [`PaidTknManager`] and is part of a consistent set:
	/// * [PaidUnloadTokenMembers] - tracks registered member keys.
	/// * [PaidUnloadTokenConsumed] - the consumed paid unload token aliases.
	/// * [PaidTokenCollectionsCreated] - whether the collection exists for a period.
	/// * [PaidUnloadTokenDusting] - marks periods with consumed tokens pending removal.
	///
	/// Ring members, pending members, and ring state are managed by [`Config::MemberService`].
	#[pallet::storage]
	pub type PaidUnloadTokenConsumed<T> = StorageNMap<
		_,
		(
			NMapKey<Identity, BigEndianPeriod>,
			NMapKey<Twox64Concat, RingIndex>,
			NMapKey<Twox64Concat, Alias>,
		),
		(),
		OptionQuery,
	>;

	/// Tracks whether a paid token collection exists for a given period.
	///
	/// Uses `Identity` hasher so that iteration yields periods in order, enabling efficient
	/// cleanup of expired periods.
	///
	/// **WARNING**: Do not use this storage directly, use [`PaidTknManager`] type instead.
	///
	/// This storage item is managed by [`PaidTknManager`] and is part of a consistent set:
	/// * [PaidUnloadTokenMembers] - tracks registered member keys.
	/// * [PaidUnloadTokenConsumed] - the consumed paid unload token aliases.
	/// * [PaidTokenCollectionsCreated] - whether the collection exists for a period.
	/// * [PaidUnloadTokenDusting] - marks periods with consumed tokens pending removal.
	///
	/// Ring members, pending members, and ring state are managed by [`Config::MemberService`].
	#[pallet::storage]
	pub type PaidTokenCollectionsCreated<T> =
		StorageMap<_, Identity, BigEndianPeriod, (), OptionQuery>;

	/// Marks paid unload token periods that have consumed tokens pending removal.
	///
	/// When a paid unload token collection is removed, the cleanup of its consumed tokens in
	/// [PaidUnloadTokenConsumed] is performed gradually through this storage item. An entry
	/// here indicates that consumed tokens for the given period still exist and should be
	/// dusted.
	///
	/// **WARNING**: Do not use this storage directly, use [`PaidTknManager`] type instead.
	///
	/// This storage item is managed by [`PaidTknManager`] and is part of a consistent set:
	/// * [PaidUnloadTokenMembers] - tracks registered member keys.
	/// * [PaidUnloadTokenConsumed] - the consumed paid unload token aliases.
	/// * [PaidTokenCollectionsCreated] - whether the collection exists for a period.
	/// * [PaidUnloadTokenDusting] - marks periods with consumed tokens pending removal.
	///
	/// Ring members, pending members, and ring state are managed by [`Config::MemberService`].
	#[pallet::storage]
	pub type PaidUnloadTokenDusting<T> = StorageMap<_, Identity, BigEndianPeriod, (), OptionQuery>;

	/// Tracks the next ring index to clean for each expired period.
	///
	/// Used by the OCW to determine cleanup progress and by the collection deletion
	/// extrinsic to verify all rings have been cleaned.
	///
	/// Rings are cleaned sequentially (one per OCW interval) rather than all at once.
	/// This is intentional: a single storage cursor enables O(1) completion checks in
	/// both [`PaidTknManager::ensure_can_clean_ring`] and
	/// [`PaidTknManager::ensure_can_delete_collection`]. The alternative — submitting
	/// all ring cleans in parallel — would require fetching ring members to check whether
	/// each ring was already cleaned (since `ring_status` still reports `total > 0` after
	/// cleanup because rings are not removed until collection deletion). Cleanup of expired
	/// collections is not time-critical, so the simpler sequential approach is preferred.
	///
	/// **WARNING**: Do not use this storage directly, use [`PaidTknManager`] type instead.
	///
	/// This storage item is managed by [`PaidTknManager`] and is part of a consistent set:
	/// * [PaidUnloadTokenMembers] - tracks registered member keys.
	/// * [PaidUnloadTokenConsumed] - the consumed paid unload token aliases.
	/// * [PaidTokenCollectionsCreated] - whether the collection exists for a period.
	/// * [PaidUnloadTokenDusting] - marks periods with consumed tokens pending removal.
	/// * [PaidUnloadTokenNextRingToClean] - sequential ring cleanup progress.
	///
	/// Ring members, pending members, and ring state are managed by [`Config::MemberService`].
	#[pallet::storage]
	pub type PaidUnloadTokenNextRingToClean<T> =
		StorageMap<_, Identity, BigEndianPeriod, RingIndex, OptionQuery>;

	/// The coinage instances, keyed by [`InstanceId`].
	///
	/// Created by [`Pallet::create_sufficient_instance`]. Entries are never removed: the coins and
	/// recyclers of an instance can outlive any single operation.
	#[pallet::storage]
	pub type Instances<T: Config> = StorageMap<_, Twox64Concat, InstanceId, InstanceRecord<T>>;

	/// The [`InstanceId`] that [`Pallet::create_sufficient_instance`] allocates next.
	#[pallet::storage]
	pub type NextInstanceId<T> = StorageValue<_, InstanceId, ValueQuery>;

	/// Reverse lookup from an underlying asset to the instances wrapping it, as a set.
	///
	/// An asset may be wrapped by multiple instances with different
	/// [`InstanceRecord::asset_unit`]s.
	#[pallet::storage]
	pub type AssetToInstance<T: Config> =
		StorageDoubleMap<_, Blake2_128Concat, FungiblesAssetIdOf<T>, Twox64Concat, InstanceId, ()>;

	/// What each funder has put into an instance's pot through [`Pallet::fund_pot`].
	#[pallet::storage]
	pub type PotContributions<T: Config> = StorageNMap<
		_,
		(
			NMapKey<Twox64Concat, InstanceId>,
			NMapKey<Blake2_128Concat, <T as frame_system::Config>::AccountId>,
			NMapKey<Blake2_128Concat, FungiblesAssetIdOf<T>>,
		),
		FungiblesBalanceOf<T>,
		ValueQuery,
	>;

	#[pallet::config]
	pub trait Config:
		frame_system::Config<
			RuntimeOrigin: Into<Result<Origin<Self>, Self::RuntimeOrigin>> + From<Origin<Self>>,
			RuntimeCall: IsSubType<Call<Self>>,
		> + CreateAuthorizedTransaction<Call<Self>>
		+ Send
		+ Sync
	{
		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;

		/// The pallet id for deriving the pallet account.
		type PalletId: Get<PalletId>;

		/// The time provider.
		type UnixTime: UnixTime;

		/// Service for managing member collections (ring-VRF rings).
		type MemberService: AppendOnlyMembers<Location = xcm::v5::Location>
			+ MembershipProver<
				Crypto: GenerateVerifiable<
					Proof: Send + Sync + DecodeWithMemTracking,
					Signature: Send + Sync + DecodeWithMemTracking,
					Member: DecodeWithMemTracking,
					Members: Parameter + DecodeWithMemTracking,
					Config: TryFrom<RingExponent>,
				>,
			> + RingRootsProvider<MembersOf<Self>>
			+ AppendOnlyMembersWeightInfo;

		/// The ring exponent for recycler collections.
		///
		/// NOTE: Changing this value on a live chain requires substantial migration work.
		#[pallet::constant]
		type RecyclerRingExponent: Get<RingExponent>;

		/// The ring exponent for paid unload token collections.
		#[pallet::constant]
		type PaidUnloadTokenRingExponent: Get<RingExponent>;

		/// The native fungible of the chain.
		type NativeFungible: fungible::Mutate<Self::AccountId, Balance = FungiblesBalanceOf<Self>>;

		/// The fungibles an instance's coins can wrap, and the load deposit and the instance
		/// creation deposit can be denominated in.
		///
		/// Expected to cover the native token as well as the chain's assets, so that coins can
		/// wrap the native token and a pot can be funded in it.
		///
		/// We intentionally keep this without `fungibles::Create`: normal pallet usage does not
		/// require it. Benchmarks that need asset setup go through
		/// `T::BenchmarkHelper::setup_assets()`.
		type Fungibles: fungibles::MutateHold<Self::AccountId, Reason: From<HoldReason>>
			+ fungibles::Mutate<Self::AccountId>
			+ AccountTouch<FungiblesAssetIdOf<Self>, Self::AccountId>;

		/// Origin allowed to create a sufficient coinage instance via
		/// [`Pallet::create_sufficient_instance`] and to switch an instance's mode via
		/// [`Pallet::make_instance_sufficient`] and [`Pallet::make_instance_sponsored`].
		type AdminOrigin: EnsureOrigin<Self::RuntimeOrigin>;

		/// Origin allowed to create a sponsored coinage instance via
		/// [`Pallet::create_sponsored_instance`], yielding the account that provides the
		/// creation deposit and the pallet account's minimum balance.
		type SponsorOrigin: EnsureOrigin<Self::RuntimeOrigin, Success = Self::AccountId>;

		/// Whether sponsored instances can be created at all.
		type EnablePermissionless: Get<bool>;

		/// The currency the load deposit is denominated in and the deposit held per recycler
		/// member key loaded on a sponsored instance.
		///
		/// The deposit is held from the time the coin is loaded until the coin is unloaded or the
		/// coin's recycler is archived.
		/// It accounts for the cost of loading the coin until it is unloaded or archived. When the
		/// coin is unloaded the user pay the unload fee which cover this cost and the load deposit
		/// is released. When the coin is archived, its footprint on the chain's state becomes
		/// minimal.
		///
		/// Not a constant: it is expected to be revisited regularly as the price of the asset
		/// moves, and can be repointed at another asset id at any time. Changing either half does
		/// not touch deposits already held, which keep the asset id and price they were taken at
		/// until they are settled or [`Pallet::collapse_load_deposits`] re-prices them.
		type LoadDeposit: Get<(FungiblesAssetIdOf<Self>, FungiblesBalanceOf<Self>)>;

		/// The creation deposit of a sponsored instance.
		///
		/// The ticket is kept in [`InstanceRecord::creator`] for as long as the instance is
		/// sponsored: instances are never removed, so it is dropped only by
		/// [`Pallet::make_instance_sufficient`].
		type InstanceCreationDeposit: Consideration<Self::AccountId, Footprint>;

		/// The validator for membership proofs used by the people and lite-people collections.
		type MembershipProof: ValidateProof<Proof: Parameter + Send + Sync>;

		/// The minimum exponent for the denomination.
		#[pallet::constant]
		type MinimumExponent: Get<i8>;

		/// The maximum exponent for the denomination.
		#[pallet::constant]
		type MaximumExponent: Get<i8>;

		/// The minimum coin exponent that can be used to dispatch a call `unload_recycler_*` with
		/// the transaction extension `AsUnloadTokenFromOutput`.
		///
		/// This ensures the fee coin is large enough to penalize failing transactions, but it does
		/// not need to cover the whole unload token fee.
		///
		/// The exponent is global, so the coin value it represents scales with each instance's
		/// [`InstanceRecord::asset_unit`]. An instance whose unit is worth less therefore penalizes
		/// failing transactions less.
		///
		/// The helper function `weight_for_unload_recycler_paying_using_output` can be used to
		/// evaluate the worst-case weight for this operation.
		#[pallet::constant]
		type MinimumExponentForOutputUnloadFee: Get<i8>;

		/// The maximum number of outputs for a split.
		#[pallet::constant]
		type MaxSplitOutputs: Get<u32>;

		/// The maximum number of alias proofs in a consolidation.
		#[pallet::constant]
		type MaxConsolidation: Get<u32> + Send + Sync;

		/// The maximum number of inner calls in a single
		/// [`Call::load_recycler_with_external_asset_unpaid_batch`] dispatch.
		#[pallet::constant]
		type MaxBatchUnpaidLoad: Get<u32> + Send + Sync;

		/// The maximum age a coin can have before it must be recycled.
		///
		/// At maximum age, the coin can no longer be transferred or split.
		///
		/// This parameter can be changed at any time.
		type MaximumAge: Get<u16>;

		/// The time period duration for unload tokens, in seconds.
		#[pallet::constant]
		type UnloadTokenTimePeriodPeopleLitePeople: Get<u32>;

		/// The allowance of unload tokens that a person can use per time period, expressed in the
		/// native currency.
		///
		/// The allowance is native rather than per-asset because the budget is shared across all
		/// instances, so there is no single underlying asset to denominate it in.
		///
		/// Use pallet view to fetch the corresponding number of unload tokens given the current
		/// price for unload tokens.
		///
		/// This parameter can be changed at any time.
		type UnloadTokenAllowancePerTimePeriodForPeople: Get<NativeBalanceOf<Self>>;

		/// The allowance of unload tokens that a lite person can use per time period, expressed in
		/// the native currency.
		///
		/// Use pallet's get_free_unload_token_info() to fetch the corresponding number of unload
		/// tokens given the current price for unload tokens.
		///
		/// This parameter can be changed at any time.
		type UnloadTokenAllowancePerTimePeriodForLitePeople: Get<NativeBalanceOf<Self>>;

		/// Hard upper bound on the number of free unload tokens per time period.
		///
		/// The effective free token limit is:
		/// `min(allowance / current_fee, MaxFreeUnloadTokensPerTimePeriod)`.
		///
		/// This parameter can be changed at any time.
		type MaxFreeUnloadTokensPerTimePeriod: Get<u32>;

		/// The expiration time for a recycler ring, in seconds, after it is full.
		type RecyclerExpirationTime: Get<u32>;

		/// The expiration time for a paid unload token ring, in seconds, after its period is over.
		type PaidUnloadTokenRingExpirationTime: Get<u32>;

		/// The time period duration for paid unload tokens, in seconds.
		type PaidUnloadTokenTimePeriod: Get<u32>;

		/// The market between different assets. Used for fees in various calls.
		type FeeConversion: Swap<
				Self::AccountId,
				AssetKind = FungiblesAssetIdOf<Self>,
				Balance = FungiblesBalanceOf<Self>,
			> + QuotePrice<AssetKind = FungiblesAssetIdOf<Self>, Balance = FungiblesBalanceOf<Self>>;

		/// The asset kind [`Config::FeeConversion`] knows the native currency by.
		type NativeAssetKind: Get<FungiblesAssetIdOf<Self>>;

		/// The account to which the unload token fees are paid.
		type FeeDestination: Get<Self::AccountId>;

		/// The weight-to-fee conversion. Includes the fee multiplier, taking into
		/// network congestion.
		type WeightToFee: Convert<Weight, NativeBalanceOf<Self>>;

		/// The number of blocks between offchain worker executions.
		#[pallet::constant]
		type OffchainWorkerInterval: Get<BlockNumberFor<Self>>;

		/// The base number of seconds to lock a coin after a failed dispatch in `AsCoin` flow.
		///
		/// The actual lock duration is exponential: `2^retries * base` where `retries` is
		/// the number of consecutive failures.
		#[pallet::constant]
		type CoinFailureLockPeriod: Get<u64>;

		/// Helper for runtime benchmarks.
		#[cfg(feature = "runtime-benchmarks")]
		type BenchmarkHelper: crate::BenchmarkHelper<Self>;
	}

	#[pallet::extra_constants]
	impl<T: Config> Pallet<T> {
		/// The account id of the pallet.
		pub fn pallet_account() -> T::AccountId {
			T::PalletId::get().into_account_truncating()
		}
	}

	#[derive(
		Copy,
		Clone,
		PartialEq,
		Eq,
		Debug,
		Encode,
		Decode,
		MaxEncodedLen,
		TypeInfo,
		DecodeWithMemTracking,
	)]
	pub enum UnloadFee {
		/// Fee is settled by the unload token, so no fee is deducted from unloaded assets.
		/// For free tokens (people/lite people), consuming a token in the current period covers
		/// the unload fee. For paid tokens, the fee was paid upfront when entering the paid ring.
		/// All alias proofs are validated in the call.
		///
		/// The fee is settled independently of the unload operation.
		Prepaid,
		/// Fee is deducted from the unloaded assets.
		/// The first alias (of the first input) was pre-validated and marked as unloaded
		/// in the extension for spam protection. The call verifies the first alias is marked
		/// as unloaded and skips re-validation for it.
		///
		/// The fees are converted into the native currency with [`Config::FeeConversion`].
		/// This feature is not available if the fee conversion market does not support the asset
		/// or the amount. Calls expose `max_fee` to allow the caller to limit slippage.
		FromOutput {
			/// The denomination of the fee recycler validated in extension.
			/// Must match `inputs[0].value` in the call.
			fee_recycler_value: Denomination,
			/// The index of the fee recycler validated in extension.
			/// Must match `inputs[0].index` in the call.
			fee_recycler_index: RingIndex,
		},
	}

	#[pallet::origin]
	#[derive(
		CloneNoBound,
		PartialEqNoBound,
		EqNoBound,
		DebugNoBound,
		Encode,
		Decode,
		MaxEncodedLen,
		TypeInfo,
		DecodeWithMemTracking,
	)]
	pub enum Origin<T: Config> {
		/// A coin as origin. The coin is removed by the transaction extension when creating this
		/// origin. The origin effectively holds the coin.
		Coin {
			/// The id of the coin owner.
			coin_id: T::AccountId,
			/// The coin held by the origin.
			coin: Coin,
		},
		/// An unload token as origin. The unload token is marked as consumed by the transaction
		/// extension when creating this origin.
		UnloadToken {
			/// All the alias proofs used alongside this token. The proofs are included in the
			/// origin as they sign for the whole inherited implications of the transaction
			/// extension.
			///
			/// When `fee` is [`UnloadFee::FromOutput`], the first proof proved a different message
			/// and MUST be skipped during call-level validation as it is already validated.
			alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation>,
			/// The proven message: `inherited_implication` hashed with blake2_256.
			///
			/// When `fee` is [`UnloadFee::FromOutput`], the first proof in `alias_proofs` proved a
			/// different message and MUST be skipped during call-level validation as it is already
			/// validated.
			proven_msg: [u8; 32],
			/// How the unload fee is handled.
			fee: UnloadFee,
		},
		/// A signed account as origin for the infallible `load_recycler_with_external_asset`
		/// call, authenticated by the `InfallibleUnpaidSigned` transaction extension.
		///
		/// All validation is performed by the transaction extension in the validation phase.
		/// The asset transfer is committed in the prepare phase. The dispatch only loads the
		/// recycler and emits the event.
		InfallibleUnpaidSigned {
			/// The account id of the signer.
			who: T::AccountId,
		},
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		CoinSplit {
			instance_id: InstanceId,
			output_count: u32,
		},
		CoinTransferred {
			instance_id: InstanceId,
			to: T::AccountId,
			value: Denomination,
			new_age: u16,
		},
		RecyclerLoadedWithCoin {
			instance_id: InstanceId,
			value: Denomination,
		},
		RecyclerLoadedWithExternalAsset {
			instance_id: InstanceId,
			who: T::AccountId,
			value: Denomination,
			amount: FungiblesBalanceOf<T>,
		},
		RecyclerUnloadedIntoCoin {
			instance_id: InstanceId,
			to: T::AccountId,
			input_value: Denomination,
			output_value: Denomination,
			input_count: u32,
		},
		RecyclerUnloadedIntoExternalAsset {
			instance_id: InstanceId,
			to: T::AccountId,
			value: Denomination,
			input_count: u32,
			amount: FungiblesBalanceOf<T>,
		},
		RecyclerUnloadedIntoExternalAssetAndLoadedCoins {
			instance_id: InstanceId,
			to: T::AccountId,
			value: Denomination,
			input_count: u32,
			external_asset_amount: FungiblesBalanceOf<T>,
			loaded_coin_count: u32,
		},
		/// An alias was permanently marked as unloaded from a live recycler ring.
		///
		/// Emitted once per alias on every unload from a live (not yet archived) ring;
		/// recoveries from an archived ring emit
		/// [`Event::ArchivedRecyclerUnloadedIntoExternalAsset`] instead. Together, these events
		/// let an offchain service reconstruct the unloaded-aliases trie committed to by
		/// [`Event::RecyclerArchived`], and hence build the proofs needed by
		/// [`Pallet::unload_archived_recycler_into_external_asset`].
		RecyclerAliasUnloaded {
			instance_id: InstanceId,
			value: Denomination,
			ring_index: RingIndex,
			alias: Alias,
		},
		PaidUnloadTokenRegisteredWithCoin {
			instance_id: InstanceId,
			fee: FungiblesBalanceOf<T>,
			destroyed: FungiblesBalanceOf<T>,
		},
		PaidUnloadTokenRegisteredWithNative {
			who: T::AccountId,
			fee: NativeBalanceOf<T>,
		},
		PaidUnloadTokenRegisteredWithExternalAsset {
			instance_id: InstanceId,
			who: T::AccountId,
			fee: FungiblesBalanceOf<T>,
		},
		PeopleFreeUnloadTokenConsumed {
			period: Period,
		},
		LitePeopleFreeUnloadTokenConsumed {
			period: Period,
		},
		RecyclersUnloadedIntoCoin {
			instance_id: InstanceId,
			to: T::AccountId,
			output_value: Denomination,
			input_count: u32,
		},
		RecyclersUnloadedIntoExternalAsset {
			instance_id: InstanceId,
			to: T::AccountId,
			input_count: u32,
			amount: FungiblesBalanceOf<T>,
		},
		RecyclersUnloadedIntoExternalAssetNonAnonymous {
			instance_id: InstanceId,
			who: T::AccountId,
			to: T::AccountId,
			input_count: u32,
			amount: FungiblesBalanceOf<T>,
			fee_currency: FeeCurrency,
		},
		RecyclerUnloadedIntoCoins {
			instance_id: InstanceId,
			output_count: u32,
		},
		CoinOffboardedIntoExternalAsset {
			instance_id: InstanceId,
			to: T::AccountId,
			value: Denomination,
			amount: FungiblesBalanceOf<T>,
		},
		/// A recycler ring was cleaned. `remaining_coins` is the number of not-yet-unloaded coins;
		/// when non-zero the ring is archived (see [Event::RecyclerArchived]) and that value is
		/// retained for recovery rather than destroyed.
		RecyclerCleaned {
			instance_id: InstanceId,
			value: Denomination,
			remaining_coins: u32,
		},
		/// A cleaned recycler ring with recoverable coins was archived: its archival commitment
		/// (see [`archive_commitment`]) was recorded in [RecyclersArchives].
		///
		/// The ring-VRF `recycler_root` is emitted here because the ring is removed from storage
		/// in the same operation: this event is the last on-chain source of the root, which
		/// offchain services must retain to build the recovery proofs for
		/// [`Pallet::unload_archived_recycler_into_external_asset`].
		RecyclerArchived {
			instance_id: InstanceId,
			value: Denomination,
			ring_index: RingIndex,
			recycler_root: MembersOf<T>,
		},
		/// A coin was recovered from an archived recycler ring into the external asset.
		ArchivedRecyclerUnloadedIntoExternalAsset {
			instance_id: InstanceId,
			who: T::AccountId,
			to: T::AccountId,
			value: Denomination,
			ring_index: RingIndex,
			amount: FungiblesBalanceOf<T>,
			fee_currency: FeeCurrency,
			alias: Alias,
		},
		ConsumedFreeTokensCleaned {
			period: Period,
		},
		PaidUnloadTokenRingCleaned {
			period: Period,
			ring_index: RingIndex,
		},
		RecyclerDustCleaned,
		PaidUnloadTokenDustCleaned,
		ExpiredPaidUnloadTokenCollectionDeleted {
			period: Period,
		},
		/// A coinage instance was created for an underlying asset.
		InstanceCreated {
			instance_id: InstanceId,
			asset_id: FungiblesAssetIdOf<T>,
			asset_unit: FungiblesBalanceOf<T>,
			mode: InstanceMode,
		},
		/// A tracked contribution was added to a sponsored instance's pot.
		PotFunded {
			instance_id: InstanceId,
			funder: T::AccountId,
			currency: FungiblesAssetIdOf<T>,
			amount: FungiblesBalanceOf<T>,
		},
		/// A funder took back part of their recorded pot contribution.
		PotFundsWithdrawn {
			instance_id: InstanceId,
			funder: T::AccountId,
			currency: FungiblesAssetIdOf<T>,
			amount: FungiblesBalanceOf<T>,
		},
		/// `count` load deposits of `price` each were taken from a sponsored instance's pot.
		LoadDepositsHeld {
			instance_id: InstanceId,
			currency: FungiblesAssetIdOf<T>,
			price: FungiblesBalanceOf<T>,
			count: u32,
		},
		/// `count` settled keys released `amount` of `currency` to the pot's free balance.
		LoadDepositsReleased {
			instance_id: InstanceId,
			currency: FungiblesAssetIdOf<T>,
			amount: FungiblesBalanceOf<T>,
			count: u32,
		},
		/// Every live load deposit of the instance was re-priced to the current
		/// [`Config::LoadDeposit`].
		LoadDepositsCollapsed {
			instance_id: InstanceId,
			currency: FungiblesAssetIdOf<T>,
			price: FungiblesBalanceOf<T>,
			count: u32,
		},
		/// Governance switched the instance's mode.
		InstanceModeSet {
			instance_id: InstanceId,
			mode: InstanceMode,
		},
	}

	#[pallet::error]
	pub enum Error<T> {
		MemberKeyAlreadyUsed,
		InvalidMemberKey,
		InternalError,
		RecyclerAlreadyUnloaded,
		InvalidConsolidation,
		ConsolidationTooBig,
		DenominationTooBig,
		DenominationTooSmall,
		CoinAmountBelowFee,
		DenominationOutOfBound,
		/// The denomination cannot be losslessly converted to an asset amount because the
		/// instance's `asset_unit` is not evenly divisible by `2^|value|`.
		LossyDenominationConversion,
		InvalidAliasProof,
		NoUnloadingRecycler,
		ProofAndAliasMismatch,
		NothingToBuild,
		TooManyRings,
		AddressAlreadyHasCoin,
		InvalidProofOfOwnership,
		EmptyInputs,
		/// The fee recycler in the origin does not match the call's recycler.
		RecyclerMismatch,
		/// The total unloaded amount is less than the fee.
		InsufficientUnloadForFee,
		/// The first alias was not pre-marked by extension (required for FromOutput fee).
		AliasNotPremarked,
		/// The recycler revision does not match (recycler may not exist or has been rebuilt).
		InvalidRecyclerRevision,
		InvalidSplit,
		/// The asset cannot be converted into the native currency to pay the fee.
		CannotConvertAssetToNative,
		AliasTemporarilyLocked,
		/// [`Call::unload_recycler_into_coins`] with [`UnloadFee::Prepaid`] requires `max_fee` to
		/// be 0.
		MaxFeeNotAllowedForPrepaid,
		/// The max_fee exceeds the total input value.
		MaxFeeExceedsInput,
		/// The max fee argument doesn't satisfy the requirements.
		InvalidMaxFee,
		/// The underlying asset id does not exist in [`Config::Fungibles`].
		UnknownAsset,
		/// No coinage instance exists for the given [`InstanceId`].
		InstanceNotFound,
		/// The asset unit is zero, or cannot represent every denomination in
		/// `[MinimumExponent, MaximumExponent]` without truncation.
		InvalidAssetUnit,
		/// No archived recycler exists for the given `(instance, denomination, ring index)`.
		ArchivedRecyclerNotFound,
		/// The supplied `recycler_root`/`unloaded_root` do not match the stored archival
		/// commitment.
		InvalidArchivedRoots,
		/// The recycler ring exponent could not be converted to the crypto config.
		InvalidRingExponent,
		/// The alias was already unloaded, or the supplied non-inclusion proof is invalid.
		AliasWasUnloadedOrInvalidProof,
		/// A `fund_pot` or `withdraw_pot_funds` amount of zero.
		ZeroAmount,
		/// The instance is not sponsored, so it has no pot.
		InstanceNotSponsored,
		/// The withdrawal exceeds the caller's recorded pot contribution in that currency.
		WithdrawExceedsContribution,
		/// The sponsored instance's pot cannot fund this load's deposit.
		PotCannotCoverLoadDeposit,
		/// The load deposit changed while the sponsored instance's old tier still holds deposits,
		/// so the instance needs [`Pallet::collapse_load_deposits`] before it can load again.
		LoadDepositOldTierOccupied,
		/// The ledger is already a single tier at the current [`Config::LoadDeposit`], so there is
		/// nothing to collapse.
		NothingToCollapse,
		/// The instance is already sponsored.
		InstanceAlreadySponsored,
		/// [`Config::EnablePermissionless`] is false, so no sponsored instance can be created.
		SponsoredInstancesDisabled,
		/// The pallet account cannot receive the underlying asset because it has not been
		/// touched for it, which [`Pallet::create_sufficient_instance`] expects to have happened
		/// already.
		PalletAccountNotTouched,
		/// The pallet account holds less than the underlying asset's minimum balance, which
		/// [`Pallet::create_sufficient_instance`] expects as a buffer against the account being
		/// dusted.
		PalletAccountBelowMinimumBalance,
		/// A `fund_pot` amount below the currency's minimum balance, which the transfer could
		/// dust right away.
		FundingBelowMinimumBalance,
		/// Paying the fee would cost more than the caller's `max_fee`, in the currency paying it.
		FeeExceedsMaxFee,
	}

	/// A reason for the pallet placing a hold on funds.
	#[pallet::composite_enum]
	pub enum HoldReason {
		/// Hold for wrapped coins in the coinage system.
		Wrapped,
		/// The load deposits held on a sponsored instance's pot, one per redeemable member key.
		LoadDeposit,
		/// Available to runtimes backing [`Config::InstanceCreationDeposit`] with a fungible
		/// hold on the creator of a sponsored instance.
		InstanceCreationDeposit,
	}

	#[pallet::view_functions]
	impl<T: Config> Pallet<T> {
		/// Get every instance wrapping the given underlying asset.
		///
		/// An asset can be wrapped by several instances, each with its own coin unit; read the
		/// unit of one from [`Instances`].
		pub fn get_instance_ids(asset_id: FungiblesAssetIdOf<T>) -> Vec<InstanceId> {
			AssetToInstance::<T>::iter_key_prefix(asset_id).collect()
		}

		/// Get the load deposit currency and price.
		pub fn get_load_deposit() -> (FungiblesAssetIdOf<T>, FungiblesBalanceOf<T>) {
			T::LoadDeposit::get()
		}

		/// Get the pot account of a sponsored instance, `None` for a sufficient or missing one.
		pub fn get_pot_account(instance_id: InstanceId) -> Option<T::AccountId> {
			let record = Instances::<T>::get(instance_id)?;
			(record.mode == InstanceMode::Sponsored).then(|| Self::pot_account(instance_id))
		}

		/// Get the status of a sponsored instance's pot, `None` for a sufficient or missing
		/// instance.
		pub fn get_pot_status(
			instance_id: InstanceId,
		) -> Option<PotView<FungiblesAssetIdOf<T>, FungiblesBalanceOf<T>>> {
			Self::pot_status(instance_id)
		}

		/// Per asset id: the funder's recorded pot contribution and how much of it is
		/// withdrawable right now, which is the contribution capped by what
		/// [`Pallet::withdraw_pot_funds`] could actually move out of the pot in that currency.
		pub fn get_pot_contributions(
			instance_id: InstanceId,
			funder: T::AccountId,
		) -> Vec<(FungiblesAssetIdOf<T>, FungiblesBalanceOf<T>, FungiblesBalanceOf<T>)> {
			Self::pot_contributions(instance_id, funder)
		}

		/// Get the current number of free unload tokens distributed to people and lite people
		/// given the current price for unload tokens.
		///
		/// Returns: `(limit_people, limit_lite_people)`.
		pub fn get_free_unload_token_info() -> (u32, u32) {
			(
				Self::free_unload_token_limit_for_people(),
				Self::free_unload_token_limit_for_lite_people(),
			)
		}

		/// Returns the current value of [`Config::MaximumAge`].
		pub fn get_maximum_age() -> u16 {
			T::MaximumAge::get()
		}

		/// Returns the current value of [`Config::UnloadTokenAllowancePerTimePeriodForPeople`].
		pub fn get_unload_token_allowance_per_time_period_for_people() -> NativeBalanceOf<T> {
			T::UnloadTokenAllowancePerTimePeriodForPeople::get()
		}

		/// Returns the current value of
		/// [`Config::UnloadTokenAllowancePerTimePeriodForLitePeople`].
		pub fn get_unload_token_allowance_per_time_period_for_lite_people() -> NativeBalanceOf<T> {
			T::UnloadTokenAllowancePerTimePeriodForLitePeople::get()
		}

		/// Returns the current value of [`Config::MaxFreeUnloadTokensPerTimePeriod`].
		pub fn get_max_free_unload_tokens_per_time_period() -> u32 {
			T::MaxFreeUnloadTokensPerTimePeriod::get()
		}

		/// Get the ring status for a recycler at a given ring index.
		pub fn get_recycler_ring_status(
			instance_id: InstanceId,
			value: Denomination,
			index: RingIndex,
		) -> Option<indiv_support::traits::RingStatus> {
			let identifier = Self::recycler_collection_identifier(instance_id, value);
			T::MemberService::ring_status(&identifier, index)
		}

		/// Get the ring revision for a recycler at a given ring index.
		pub fn get_recycler_ring_revision(
			instance_id: InstanceId,
			value: Denomination,
			index: RingIndex,
		) -> Option<RevisionIndex> {
			let identifier = Self::recycler_collection_identifier(instance_id, value);
			T::MemberService::ring_revision(&identifier, index)
		}

		/// Get the ring status for a paid token at a given period and ring index.
		pub fn get_paid_token_ring_status(
			period: Period,
			index: RingIndex,
		) -> Option<indiv_support::traits::RingStatus> {
			let identifier = Self::paid_token_collection_identifier(period);
			T::MemberService::ring_status(&identifier, index)
		}

		/// Get the ring revision for a paid token at a given period and ring index.
		pub fn get_paid_token_ring_revision(
			period: Period,
			index: RingIndex,
		) -> Option<RevisionIndex> {
			let identifier = Self::paid_token_collection_identifier(period);
			T::MemberService::ring_revision(&identifier, index)
		}

		/// Get the amount of an instance's underlying asset that currently pays for one paid unload
		/// token.
		///
		/// Returns `None` if the instance does not exist, or if its asset cannot currently be
		/// converted into the native currency, in which case only native fee payment is available.
		pub fn get_paid_unload_token_fee_in_asset(
			instance_id: InstanceId,
		) -> Option<FungiblesBalanceOf<T>> {
			Self::quote_paid_unload_token_fee_in_asset(instance_id).ok()
		}

		/// Get the amount of an instance's underlying asset that currently pays for `count` paid
		/// unload tokens.
		///
		/// The batch is quoted as a single swap into the native currency, so pool slippage makes
		/// the result differ from `count` times the single-token quote. Returns `None` if the
		/// instance does not exist, or if its asset cannot currently be converted into the native
		/// currency, in which case only native fee payment is available.
		pub fn get_paid_unload_token_fee_quote_in_asset(
			instance_id: InstanceId,
			count: u32,
		) -> Option<FungiblesBalanceOf<T>> {
			Self::quote_paid_unload_token_fees_in_asset(instance_id, count).ok()
		}

		/// Get the current fee in the native currency for paid unload tokens.
		pub fn get_paid_unload_token_fee_in_native() -> NativeBalanceOf<T> {
			Self::paid_unload_token_fee_in_native()
		}

		/// Get coin details for an account.
		pub fn get_coin_by_owner(owner: T::AccountId) -> Option<Coin> {
			CoinsByOwner::<T>::get(owner)
		}

		/// Get the Unix timestamp until which a coin is currently locked after failed dispatch.
		///
		/// Returns `None` when there is no lock or when the stored lock has already expired.
		pub fn get_coin_lock_until(owner: T::AccountId) -> Option<u64> {
			let current_time = T::UnixTime::now().as_secs();
			LockedCoins::<T>::get(owner)
				.and_then(|locked| (current_time < locked.until).then_some(locked.until))
		}

		/// Get the instance and denomination for a specific recycler member key.
		pub fn get_recycler_member_info(member: MemberOf<T>) -> Option<(InstanceId, Denomination)> {
			RecyclersCoinToRecycler::<T>::get(member)
		}

		/// Check whether a paid token member key is registered.
		pub fn is_paid_token_member(member: MemberOf<T>) -> bool {
			PaidUnloadTokenMembers::<T>::contains_key(member)
		}

		/// Get the members of a recycler ring.
		/// Required to build the ring commitment (accumulator) for the proof.
		pub fn get_recycler_members(
			instance_id: InstanceId,
			value: Denomination,
			index: RingIndex,
		) -> Vec<MemberOf<T>> {
			let identifier = Self::recycler_collection_identifier(instance_id, value);
			T::MemberService::ring_members(&identifier, index)
		}

		/// Get the ring-VRF root (the "recycler root") of a recycler ring, if it has been built.
		pub fn recycler_ring_root(
			instance_id: InstanceId,
			value: Denomination,
			index: RingIndex,
		) -> Option<MembersOf<T>> {
			let identifier = Self::recycler_collection_identifier(instance_id, value);
			T::MemberService::get_ring_roots(identifier, &[index])
				.into_iter()
				.find_map(|(idx, root, _revision)| (idx == index).then_some(root))
		}

		/// Get the members of a paid token ring.
		/// Required to build the ring commitment (accumulator) for the proof.
		pub fn get_paid_token_ring_members(period: Period, index: RingIndex) -> Vec<MemberOf<T>> {
			let identifier = Self::paid_token_collection_identifier(period);
			T::MemberService::ring_members(&identifier, index)
		}

		/// Check if a recycler alias has already been unloaded (spent).
		///
		/// If the recycler is not live, the result is not significant.
		pub fn is_recycler_alias_unloaded(
			instance_id: InstanceId,
			value: Denomination,
			index: RingIndex,
			alias: Alias,
		) -> bool {
			matches!(
				RecyclerAliasStates::<T>::get((instance_id, value, index, alias)),
				Some(AliasState::Unloaded),
			)
		}

		/// Check if a paid unload token has been consumed.
		///
		/// If the period is not current, the result is not significant.
		pub fn is_paid_token_alias_consumed(period: Period, index: u32, alias: Alias) -> bool {
			PaidUnloadTokenConsumed::<T>::contains_key((
				BigEndianPeriod::from(period),
				index,
				alias,
			))
		}

		/// Check if a free unload token has been consumed.
		///
		/// If the period is not current, the result is not significant.
		pub fn is_free_token_alias_consumed(period: Period, alias: Alias) -> bool {
			ConsumedFreeUnloadTokens::<T>::contains_key(period, alias)
		}

		/// Get the worst-case weight for a transaction using output-based fee payment.
		///
		/// This function returns the maximum weight for transactions dispatched with
		/// `AsUnloadTokenFromOutput`, useful for runtime configuration to find a good value for
		/// `MinimumExponentForOutputUnloadFee`.
		pub fn weight_for_unload_recycler_paying_using_output() -> Weight {
			// Transaction extension weight (includes validate_unload_calls for r=1)
			let ext_weight = T::WeightInfo::as_unload_token_from_output_tx_ext()
				.saturating_add(T::WeightInfo::validate_unload_calls(1, T::MaxSplitOutputs::get()));

			// Maximum of the possible unload call weights in `FromOutput` mode: the worst-case
			// `FromOutput` benchmarked path, matching what a `FromOutput` transaction pays after
			// its `PostDispatchInfo` refund.
			let call_weight = Pallet::<T>::unload_recycler_into_external_asset_and_loaded_coins_from_output_weight(
				T::MaxConsolidation::get() as usize,
				T::MaxSplitOutputs::get() as usize,
			)
			.max(Pallet::<T>::unload_recycler_into_external_asset_from_output_weight(
				T::MaxConsolidation::get() as usize,
			))
			.max(Pallet::<T>::unload_recycler_into_coins_from_output_weight(
				T::MaxConsolidation::get() as usize,
				T::MaxSplitOutputs::get(),
			))
			// On a sponsored instance the unload additionally settles the load deposits, and
			// the voucher variant charges deposits for its fresh keys.
			.saturating_add(T::WeightInfo::settle_load_deposits())
			.saturating_add(T::WeightInfo::charge_load_deposit());

			ext_weight.saturating_add(call_weight)
		}
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn integrity_test() {
			assert!(
				T::MinimumExponent::get() <= T::MaximumExponent::get(),
				"MinimumExponent must be <= MaximumExponent",
			);

			// Ensure that the maximum denomination in unit of minimum denomination can be
			// represented in u32.
			// This property is used by `validate_split`.
			let msg =
				"exponent range is too big, the maximum denomination in unit of minimum coin \
				value must be represented in u32.";
			assert!(
				1u32.checked_shl(
					(i32::from(T::MaximumExponent::get()))
						.checked_sub(i32::from(T::MinimumExponent::get()))
						.expect(msg)
						.try_into()
						.expect(msg)
				)
				.is_some(),
				"{}",
				msg,
			);

			assert!(
				T::MinimumExponentForOutputUnloadFee::get() >= T::MinimumExponent::get(),
				"MinimumExponentForOutputUnloadFee should be >= to MinimumExponent",
			);
			assert!(
				T::MinimumExponentForOutputUnloadFee::get() <= T::MaximumExponent::get(),
				"MinimumExponentForOutputUnloadFee should be <= to MaximumExponent",
			);

			assert!(T::MaxConsolidation::get() > 0, "MaxConsolidation must be greater than zero",);
			assert!(T::MaxSplitOutputs::get() >= 2, "MaxSplitOutputs must be at least 2",);

			let budget = OcwWeightBudget::from_normal_max::<T>();

			let recycler_ring_capacity = T::RecyclerRingExponent::get().ring_capacity();
			budget.assert_fits(
				"clean_recycler",
				T::WeightInfo::clean_recycler(recycler_ring_capacity, recycler_ring_capacity)
					.saturating_add(T::WeightInfo::authorize_clean_recycler())
					.saturating_add(T::WeightInfo::settle_load_deposits()),
			);
			budget.assert_fits(
				"clean_consumed_free_token",
				T::WeightInfo::clean_consumed_free_token(CLEAN_CONSUMED_FREE_TOKEN_LIMIT)
					.saturating_add(T::WeightInfo::authorize_clean_consumed_free_token()),
			);
			budget.assert_fits(
				"clean_paid_unload_token_ring",
				T::WeightInfo::clean_paid_unload_token_ring(
					T::PaidUnloadTokenRingExponent::get().ring_capacity(),
				)
				.saturating_add(T::WeightInfo::authorize_clean_paid_unload_token_ring()),
			);
			budget.assert_fits(
				"clean_recycler_dust",
				T::WeightInfo::clean_recycler_dust(DUST_CLEANUP_BATCH_SIZE)
					.saturating_add(T::WeightInfo::authorize_clean_recycler_dust()),
			);
			budget.assert_fits(
				"clean_paid_unload_token_dust",
				T::WeightInfo::clean_paid_unload_token_dust(DUST_CLEANUP_BATCH_SIZE)
					.saturating_add(T::WeightInfo::authorize_clean_paid_unload_token_dust()),
			);
			budget.assert_fits(
				"delete_expired_paid_unload_token_collection",
				T::WeightInfo::delete_expired_paid_unload_token_collection().saturating_add(
					T::WeightInfo::authorize_delete_expired_paid_unload_token_collection(),
				),
			);

			assert!(
				!T::LoadDeposit::get().1.is_zero(),
				"the LoadDeposit price must be non-zero, otherwise sponsored loads take no \
				collateral at all",
			);
		}

		fn on_poll(_n: BlockNumberFor<T>, weight: &mut frame_support::weights::WeightMeter) {
			// Use at most 50% of available weight to be cautious about weight
			// underestimates.
			let budget = weight.remaining() / 2;
			let mut meter = frame_support::weights::WeightMeter::with_limit(budget);
			Self::do_on_poll(&mut meter);
			weight.consume(meter.consumed());
		}

		fn offchain_worker(block_number: BlockNumberFor<T>) {
			let interval = T::OffchainWorkerInterval::get();
			if interval == 0u32.into() || !(block_number % interval).is_zero() {
				return;
			}

			// 1. Clean Expired Recyclers
			for (instance_id, value, ()) in RecyclerCollectionCreated::<T>::iter() {
				if RecyclerManager::<T>::ensure_can_clean(instance_id, value).is_ok() {
					let call = Call::clean_recycler { instance_id, value };
					Self::submit_authorized_transaction(call, "Clean Recycler");
				}
			}

			// 2. Clean Expired Paid Unload Token Rings (one ring at a time).
			// Process only the oldest period (BigEndianPeriod ensures numeric ordering).
			// Sequential cleanup is deliberate — see `PaidUnloadTokenNextRingToClean` docs.
			if let Some((period, ())) = PaidTokenCollectionsCreated::<T>::iter().next() {
				let identifier = Self::paid_token_collection_identifier(period.0);
				let next_ring = PaidUnloadTokenNextRingToClean::<T>::get(period).unwrap_or(0);
				// Since we don't call `remove_ring`, a ring with `total > 0` means it
				// was populated and still needs its PaidUnloadTokenMembers cleaned.
				// `total == 0` means we've passed the last ring.
				let more_rings_to_clean = T::MemberService::ring_status(&identifier, next_ring)
					.is_some_and(|s| s.total > 0);

				if more_rings_to_clean {
					if PaidTknManager::<T>::ensure_can_clean_ring(period.0, next_ring).is_ok() {
						let call = Call::clean_paid_unload_token_ring {
							period: period.0,
							ring_index: next_ring,
						};
						Self::submit_authorized_transaction(call, "Clean Paid Unload Token Ring");
					}
				} else if PaidTknManager::<T>::ensure_can_delete_collection(period.0).is_ok() {
					let call =
						Call::delete_expired_paid_unload_token_collection { period: period.0 };
					Self::submit_authorized_transaction(
						call,
						"Delete Expired Paid Token Collection",
					);
				}
			}

			// 3. Clean Consumed Free Unload Tokens
			let current_periods = Self::current_free_unload_token_periods();
			let mut period_to_check = current_periods[0];
			// Check the last 10 expired periods
			for _ in 0..10 {
				if let Some(p) = period_to_check.checked_sub(1) {
					period_to_check = p;
					// Check if there is anything to clean to avoid spamming empty calls
					if ConsumedFreeUnloadTokens::<T>::iter_prefix_values(p).next().is_some() {
						let call = Call::clean_consumed_free_token { period: p };
						Self::submit_authorized_transaction(
							call,
							"Clean Consumed Free Unload Tokens",
						);
					}
				} else {
					break;
				}
			}

			// 4. Clean Recycler Dust
			if RecyclersDusting::<T>::iter_keys().next().is_some() {
				let call = Call::clean_recycler_dust {};
				Self::submit_authorized_transaction(call, "Clean Recycler Dust");
			}

			// 5. Clean Paid Unload Token Dust
			if PaidUnloadTokenDusting::<T>::iter_keys().next().is_some() {
				let call = Call::clean_paid_unload_token_dust {};
				Self::submit_authorized_transaction(call, "Clean Paid Unload Token Dust");
			}
		}
	}

	impl<T: Config> Pallet<T> {
		/// Message bound into the ring-VRF proof for recovering a coin from an archived recycler.
		///
		/// Binding the signer prevents a stolen proof from being reused by a different signer.
		pub fn unload_archived_proof_message(signer: &T::AccountId) -> [u8; 32] {
			signer.using_encoded(|bytes| blake2_256(&[UNLOAD_ARCHIVED_MSG_PREFIX, bytes].concat()))
		}

		fn do_on_poll(weight: &mut frame_support::weights::WeightMeter) {
			// Ensure paid token collection exists for the current period.
			// Most blocks this is a no-op (contains_key), on period boundaries it creates
			// a new collection.
			let create_paid_weight = T::WeightInfo::on_poll_create_paid_token_collection();
			if weight.can_consume(create_paid_weight) {
				if let Err(e) = PaidTknManager::<T>::ensure_current_period_collection_exists() {
					log::warn!(
						target: LOG_TARGET,
						"failed to ensure paid unload token collection exists for current period: {e:?}"
					);
				}
				weight.consume(create_paid_weight);
			}
		}

		pub(crate) fn unload_recycler_into_coin_weight(alias_count: usize) -> Weight {
			let n = alias_count as u32;
			if n <= 2 {
				T::WeightInfo::unload_recycler_into_coin_1_2(n)
			} else if n <= 8 {
				// `unload_recycler_into_coin` is only valid and benchmarked for power of twos,
				// so we round up to the closest one.
				T::WeightInfo::unload_recycler_into_coin_4_8(n.next_power_of_two())
			} else {
				// `unload_recycler_into_coin` is only valid and benchmarked for power of twos,
				// so we round up to the closest one.
				T::WeightInfo::unload_recycler_into_coin_8_max(n.next_power_of_two())
			}
		}

		pub(crate) fn unload_recycler_into_external_asset_prepaid_weight(
			alias_count: usize,
		) -> Weight {
			let n = alias_count as u32;
			if n <= 2 {
				T::WeightInfo::unload_recycler_into_external_asset_prepaid_1_2(n)
			} else if n <= 8 {
				T::WeightInfo::unload_recycler_into_external_asset_prepaid_3_8(n)
			} else {
				T::WeightInfo::unload_recycler_into_external_asset_prepaid_9_max(n)
			}
		}

		pub(crate) fn unload_recycler_into_external_asset_and_loaded_coins_prepaid_weight(
			alias_count: usize,
			loaded_coin_count: usize,
		) -> Weight {
			let a = alias_count as u32;
			let d = loaded_coin_count as u32;
			if a <= 2 {
				T::WeightInfo::unload_recycler_into_external_asset_and_loaded_coins_prepaid_1_2(
					a, d,
				)
			} else if a <= 8 {
				T::WeightInfo::unload_recycler_into_external_asset_and_loaded_coins_prepaid_3_8(
					a, d,
				)
			} else {
				T::WeightInfo::unload_recycler_into_external_asset_and_loaded_coins_prepaid_9_max(
					a, d,
				)
			}
		}

		/// Weight of the `FromOutput` fee path of [`Call::unload_recycler_into_external_asset`].
		///
		/// Dual-mode unload calls charge the component-wise maximum of their fee-mode paths in
		/// `#[pallet::weight]` and refund down to the mode actually run via `PostDispatchInfo`.
		pub(crate) fn unload_recycler_into_external_asset_from_output_weight(
			alias_count: usize,
		) -> Weight {
			let n = alias_count as u32;
			if n <= 2 {
				T::WeightInfo::unload_recycler_into_external_asset_from_output_1_2(n)
			} else if n <= 8 {
				T::WeightInfo::unload_recycler_into_external_asset_from_output_3_8(n)
			} else {
				T::WeightInfo::unload_recycler_into_external_asset_from_output_9_max(n)
			}
		}

		/// Weight of the `FromOutput` fee path of
		/// [`Call::unload_recycler_into_external_asset_and_loaded_coins`].
		///
		/// Dual-mode unload calls charge the component-wise maximum of their fee-mode paths in
		/// `#[pallet::weight]` and refund down to the mode actually run via `PostDispatchInfo`.
		pub(crate) fn unload_recycler_into_external_asset_and_loaded_coins_from_output_weight(
			alias_count: usize,
			loaded_coin_count: usize,
		) -> Weight {
			let a = alias_count as u32;
			let d = loaded_coin_count as u32;
			if a <= 2 {
				T::WeightInfo::unload_recycler_into_external_asset_and_loaded_coins_from_output_1_2(
					a, d,
				)
			} else if a <= 8 {
				T::WeightInfo::unload_recycler_into_external_asset_and_loaded_coins_from_output_3_8(
					a, d,
				)
			} else {
				T::WeightInfo::unload_recycler_into_external_asset_and_loaded_coins_from_output_9_max(a, d)
			}
		}

		/// Worst-case weight charged up front by the `#[pallet::weight]` annotation: the
		/// component-wise maximum of the two fee-mode paths. The fee mode is only known at
		/// dispatch (it lives in the `UnloadToken` origin), so the call refunds down to the
		/// mode actually run via `PostDispatchInfo`, and a call is billed exactly the mode it
		/// runs, never more.
		pub(crate) fn unload_recycler_into_external_asset_max_weight(alias_count: usize) -> Weight {
			Self::unload_recycler_into_external_asset_prepaid_weight(alias_count)
				.max(Self::unload_recycler_into_external_asset_from_output_weight(alias_count))
		}

		/// Mode-independent base weight for
		/// [`Call::unload_recycler_into_external_asset_and_loaded_coins`].
		/// See [`Self::unload_recycler_into_external_asset_max_weight`].
		pub(crate) fn unload_recycler_into_external_asset_and_loaded_coins_max_weight(
			alias_count: usize,
			loaded_coin_count: usize,
		) -> Weight {
			Self::unload_recycler_into_external_asset_and_loaded_coins_prepaid_weight(
				alias_count,
				loaded_coin_count,
			)
			.max(Self::unload_recycler_into_external_asset_and_loaded_coins_from_output_weight(
				alias_count,
				loaded_coin_count,
			))
		}

		pub(crate) fn unload_recycler_into_external_asset_non_anonymous_weight(
			alias_count: usize,
		) -> Weight {
			let n = alias_count as u32;
			if n <= 2 {
				T::WeightInfo::unload_recycler_into_external_asset_non_anonymous_1_2(n)
			} else if n <= 8 {
				T::WeightInfo::unload_recycler_into_external_asset_non_anonymous_3_8(n)
			} else {
				T::WeightInfo::unload_recycler_into_external_asset_non_anonymous_9_max(n)
			}
		}

		pub(crate) fn unload_recyclers_into_external_asset_non_anonymous_weight(
			alias_count: u32,
		) -> Weight {
			if alias_count <= 2 {
				T::WeightInfo::unload_recyclers_into_external_asset_non_anonymous_1_2(alias_count)
			} else if alias_count <= 8 {
				T::WeightInfo::unload_recyclers_into_external_asset_non_anonymous_3_8(alias_count)
			} else {
				T::WeightInfo::unload_recyclers_into_external_asset_non_anonymous_9_max(alias_count)
			}
		}

		pub(crate) fn unload_recycler_into_coins_from_output_weight(
			alias_count: usize,
			destination_count: u32,
		) -> Weight {
			let a = alias_count as u32;
			if a <= 2 {
				T::WeightInfo::unload_recycler_into_coins_from_output_1_2(a, destination_count)
			} else if a <= 8 {
				T::WeightInfo::unload_recycler_into_coins_from_output_3_8(a, destination_count)
			} else {
				T::WeightInfo::unload_recycler_into_coins_from_output_9_max(a, destination_count)
			}
		}

		/// Weight of the `Prepaid` fee path of [`Call::unload_recycler_into_coins`].
		///
		/// Dual-mode unload calls charge the component-wise maximum of their fee-mode paths in
		/// `#[pallet::weight]` and refund down to the mode actually run via `PostDispatchInfo`.
		pub(crate) fn unload_recycler_into_coins_prepaid_weight(
			alias_count: usize,
			destination_count: u32,
		) -> Weight {
			let a = alias_count as u32;
			if a <= 2 {
				T::WeightInfo::unload_recycler_into_coins_prepaid_1_2(a, destination_count)
			} else if a <= 8 {
				T::WeightInfo::unload_recycler_into_coins_prepaid_3_8(a, destination_count)
			} else {
				T::WeightInfo::unload_recycler_into_coins_prepaid_9_max(a, destination_count)
			}
		}

		/// Worst-case weight for [`Call::unload_recycler_into_coins`]. See
		/// [`Self::unload_recycler_into_external_asset_max_weight`].
		pub(crate) fn unload_recycler_into_coins_max_weight(
			alias_count: usize,
			destination_count: u32,
		) -> Weight {
			Self::unload_recycler_into_coins_prepaid_weight(alias_count, destination_count).max(
				Self::unload_recycler_into_coins_from_output_weight(alias_count, destination_count),
			)
		}

		/// Shared dispatch body for [`Call::load_recycler_with_external_asset_unpaid`] and
		/// [`Call::load_recycler_with_external_asset_unpaid_batch`].
		///
		/// All preconditions (member-key validity, signature, balance, collection existence) are
		/// checked in the `AsCoinage` transaction extension; the inner calls here re-run them
		/// defensively and are expected never to fail.
		fn do_unpaid_load(
			who: &T::AccountId,
			instance_id: InstanceId,
			preservation: CodecPreservation,
			value: Denomination,
			member_key: MemberOf<T>,
		) -> DispatchResult {
			let record = Self::instance(instance_id)
				.defensive_proof("coinage: instance checked in validate")?;
			let asset_amount = Self::denomination_to_asset_amount(record.asset_unit, value)
				.defensive_proof("coinage: denomination conversion checked in validate")
				.map_err(|e| e.into_pallet_error::<T>())?;

			T::Fungibles::transfer_and_hold(
				record.asset_id,
				&HoldReason::Wrapped.into(),
				who,
				&Self::pallet_account(),
				asset_amount,
				Precision::Exact,
				preservation.into(),
				Fortitude::Polite,
			)
			.defensive_proof("coinage: transfer_and_hold balance checked in validate")
			.map_err(|_| Error::<T>::InternalError)?;

			RecyclerManager::<T>::load(instance_id, value, member_key)
				.inspect_err(|e| match e {
					RecyclerLoadError::MemberKeyAlreadyUsed => {
						defensive!("coinage: member key duplicate checked in validate");
					},
					RecyclerLoadError::InvalidMemberKey => {
						defensive!("coinage: invalid member key checked in validate");
					},
					RecyclerLoadError::InternalError => {
						defensive!("coinage: internal error");
					},
				})
				.map_err(|e| e.into_pallet_error::<T>())?;

			Self::deposit_event(Event::RecyclerLoadedWithExternalAsset {
				instance_id,
				who: who.clone(),
				value,
				amount: asset_amount,
			});

			Ok(())
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Split a coin into multiple coins.
		///
		/// The origin must be a [Origin::Coin], which can be obtained from the transaction
		/// extension [`AsCoinage`](crate::extension::AsCoinage).
		///
		/// The call is free and ages the resulting coins by one.
		///
		/// The `split_into` parameter contains a vector of pairs, each pair containing a coin
		/// value and a list of destination account ids. For each pair, a new coin with the given
		/// value is created for each destination account id.
		///
		/// Validity requirements:
		/// (an invalid transaction won't be included in a block, the coin is not consumed)
		/// * The coin's age must be less than [Config::MaximumAge].
		/// * The denomination must be within the bounds defined by [Config::MinimumExponent] and
		///   [Config::MaximumExponent].
		/// * The total value of the new coins must equal the value of the origin coin.
		/// * The number of outputs must not exceed [Config::MaxSplitOutputs].
		/// * The age of the new coins is set to the age of the origin coin plus one.
		/// * Each destination account must not already have a coin.
		#[pallet::call_index(0)]
		#[pallet::weight(T::WeightInfo::split(split_into.iter().map(|(_, d)| d.len() as u32).sum()))]
		pub fn split(
			origin: OriginFor<T>,
			split_into: BoundedVec<
				(Denomination, BoundedVec<T::AccountId, T::MaxSplitOutputs>),
				T::MaxSplitOutputs,
			>,
		) -> DispatchResultWithPostInfo {
			let Ok(Origin::Coin { coin_id: _, coin }) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};
			let instance_id = coin.instance_id;
			let output_count = split_into.iter().map(|(_, dests)| dests.len() as u32).sum();

			// This call should not fail; the origin's coin is already consumed by the transaction
			// extension before dispatch. Validation ensures all preconditions are met.

			for (value, dests) in split_into {
				for dest in dests {
					let new_coin = Coin { instance_id, value, age: coin.age.saturating_add(1) };

					// The destination has no coin, as verified during validation.
					CoinsByOwner::<T>::insert(&dest, new_coin);
				}
			}
			Self::deposit_event(Event::CoinSplit { instance_id, output_count });

			Ok(Pays::No.into())
		}

		/// Transfer a coin to another account.
		///
		/// The origin must be a [Origin::Coin], which can be obtained from the transaction
		/// extension [`AsCoinage`](crate::extension::AsCoinage).
		///
		/// The call is free and ages the resulting coin by one.
		///
		/// Validity requirements:
		/// (an invalid transaction won't be included in a block, the coin is not consumed)
		/// * The destination account must not already have a coin.
		/// * The coin's age must be less than [Config::MaximumAge].
		#[pallet::call_index(1)]
		#[pallet::weight(T::WeightInfo::transfer())]
		pub fn transfer(origin: OriginFor<T>, to: T::AccountId) -> DispatchResultWithPostInfo {
			let Ok(Origin::Coin { coin_id: _, coin }) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};
			let instance_id = coin.instance_id;
			let value = coin.value;
			let new_age = coin.age.saturating_add(1);

			// This call should not fail; the origin's coin is already consumed by the transaction
			// extension before dispatch. Validation ensures all preconditions are met.

			// The destination has no coin, as verified during validation.
			CoinsByOwner::<T>::insert(&to, Coin { instance_id, value, age: new_age });
			Self::deposit_event(Event::CoinTransferred { instance_id, to, value, new_age });

			Ok(Pays::No.into())
		}

		/// Load coin into a recycler.
		///
		/// The origin must be a [Origin::Coin], which can be obtained from the transaction
		/// extension [`AsCoinage`](crate::extension::AsCoinage).
		///
		/// The call is free.
		///
		/// The `member_key` parameter is the member key to be included in the recycler, and whose
		/// alias is used to unload from the recycler.
		///
		/// Validity requirements:
		/// (an invalid transaction won't be included in a block, the coin is not consumed)
		/// * The `member_key` must not already be used in another recycler.
		/// * The `member_key` must be valid (i.e. well formed).
		/// * The `proof_of_ownership` must be a valid signature of the coin's account id by the
		///   `member_key`.
		/// * The recycler collection for the coin's value must already exist
		/// * On a sponsored instance, the pot's free balance must cover the loaded key's deposit.
		#[pallet::call_index(2)]
		#[pallet::weight(T::WeightInfo::load_recycler_with_coin()
			.saturating_add(T::WeightInfo::charge_load_deposit()))]
		pub fn load_recycler_with_coin(
			origin: OriginFor<T>,
			member_key: MemberOf<T>,
			_proof_of_ownership: SignatureOf<T>,
		) -> DispatchResultWithPostInfo {
			let Ok(Origin::Coin { coin_id: _, coin }) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};
			let instance_id = coin.instance_id;

			// This call should not fail; the origin's coin is already consumed by the transaction
			// extension before dispatch. Validation ensures all preconditions are met.

			RecyclerManager::<T>::load(instance_id, coin.value, member_key)
				.inspect_err(|e| match e {
					RecyclerLoadError::MemberKeyAlreadyUsed => {
						defensive!("coinage: member key duplicate checked in validate");
					},
					RecyclerLoadError::InvalidMemberKey => {
						defensive!("coinage: invalid member key checked in validate");
					},
					RecyclerLoadError::InternalError => {
						defensive!("coinage: internal error");
					},
				})
				.map_err(|e| e.into_pallet_error::<T>())?;

			Self::charge_load_deposit(instance_id, 1)
				.defensive_proof("coinage: load deposit checked in validate")?;

			Self::deposit_event(Event::RecyclerLoadedWithCoin { instance_id, value: coin.value });

			let actual_weight = T::WeightInfo::load_recycler_with_coin()
				.saturating_add(Self::charge_load_deposit_weight(instance_id));
			Ok((Some(actual_weight), Pays::No).into())
		}

		/// Load external asset into a recycler.
		///
		/// The origin must be a signed origin.
		///
		/// The transaction fee is refunded.
		///
		/// The `preservation` parameter indicates how the asset transfer should preserve the
		/// signer's account.
		///
		/// The `instance_id` parameter indicates which coinage instance to load into, and hence
		/// which underlying asset is transferred.
		///
		/// The `value` parameter indicates the denomination to be loaded into the recycler.
		/// The equivalent amount of the underlying asset is transferred from the signer to
		/// the pallet account.
		///
		/// The `member_key` parameter is the member key to be included in the recycler, and whose
		/// alias is used to unload from the recycler.
		///
		/// The `proof_of_ownership` parameter is the signature of the signer's account id by the
		/// `member_key`.
		///
		/// Requirements:
		/// * The `instance_id` must refer to an existing instance.
		/// * The `member_key` must not already be used in another recycler.
		/// * The `member_key` must be valid (i.e. well formed).
		/// * The `value` must be within the bounds defined by [Config::MinimumExponent] and
		///   [Config::MaximumExponent].
		/// * The signer must have enough balance of the underlying asset to cover the equivalent
		///   amount for the given denomination.
		/// * The `proof_of_ownership` must be a valid signature of the signer's account id by the
		/// `member_key`.
		/// * On a sponsored instance, the pot's free balance must cover the loaded key's deposit.
		#[pallet::call_index(3)]
		#[pallet::weight(T::WeightInfo::load_recycler_with_external_asset()
			.saturating_add(Pallet::<T>::charge_load_deposit_weight(*instance_id)))]
		pub fn load_recycler_with_external_asset(
			origin: OriginFor<T>,
			instance_id: InstanceId,
			preservation: CodecPreservation,
			value: Denomination,
			member_key: MemberOf<T>,
			proof_of_ownership: SignatureOf<T>,
		) -> DispatchResultWithPostInfo {
			let who = ensure_signed(origin)?;

			ensure!(
				CryptoOf::<T>::verify_signature(
					&proof_of_ownership,
					&who.encode()[..],
					&member_key
				),
				Error::<T>::InvalidProofOfOwnership
			);

			let record = Self::instance(instance_id)?;
			let asset_amount = Self::denomination_to_asset_amount(record.asset_unit, value)
				.map_err(|e| e.into_pallet_error::<T>())?;

			T::Fungibles::transfer_and_hold(
				record.asset_id,
				&HoldReason::Wrapped.into(),
				&who,
				&Self::pallet_account(),
				asset_amount,
				Precision::Exact,
				preservation.into(),
				Fortitude::Polite,
			)?;
			RecyclerManager::<T>::load(instance_id, value, member_key)
				.map_err(|e| e.into_pallet_error::<T>())?;
			Self::charge_load_deposit(instance_id, 1)?;
			Self::deposit_event(Event::RecyclerLoadedWithExternalAsset {
				instance_id,
				who,
				value,
				amount: asset_amount,
			});
			Ok(Pays::No.into())
		}

		/// Load external asset into a recycler (infallible, validated unpaid variant).
		///
		/// The origin must be [Origin::InfallibleUnpaidSigned], which can be obtained from the
		/// transaction extension variant
		/// [`AsCoinageInfo::InfallibleUnpaidSigned`](crate::extension::AsCoinageInfo::InfallibleUnpaidSigned).
		///
		/// The transaction extension validation phase must ensure:
		/// - The `instance_id` refers to an existing instance.
		/// - The `member_key` is valid and not already used in another recycler.
		/// - The `proof_of_ownership` is a valid signature of the signer's account id by the
		///   `member_key`.
		/// - The `value` is within the bounds defined by [Config::MinimumExponent] and
		///   [Config::MaximumExponent], and can be losslessly converted to an asset amount.
		/// - The signer has enough balance of the underlying asset to cover the equivalent amount
		///   for the given denomination (respecting `preservation`).
		/// - The nonce is valid for replay protection.
		/// - The recycler collection for `instance_id` and `value` already exists.
		/// - On a sponsored instance, the pot's free balance covers the loaded key's deposit.
		///
		/// The call is free.
		#[pallet::call_index(15)]
		#[pallet::weight(T::WeightInfo::load_recycler_with_external_asset_unpaid()
			.saturating_add(Pallet::<T>::charge_load_deposit_weight(*instance_id)))]
		pub fn load_recycler_with_external_asset_unpaid(
			origin: OriginFor<T>,
			instance_id: InstanceId,
			preservation: CodecPreservation,
			value: Denomination,
			member_key: MemberOf<T>,
			_proof_of_ownership: SignatureOf<T>,
		) -> DispatchResultWithPostInfo {
			let Ok(Origin::<T>::InfallibleUnpaidSigned { who }) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};

			Self::do_unpaid_load(&who, instance_id, preservation, value, member_key)?;
			Self::charge_load_deposit(instance_id, 1)
				.defensive_proof("coinage: load deposit checked in validate")?;

			Ok(Pays::No.into())
		}

		/// Batched variant of [`Self::load_recycler_with_external_asset_unpaid`].
		///
		/// The origin must be [Origin::InfallibleUnpaidSigned], which can be obtained from the
		/// transaction extension variant
		/// [`AsCoinageInfo::InfallibleUnpaidSigned`](crate::extension::AsCoinageInfo::InfallibleUnpaidSigned).
		/// The extension validates each inner item and additionally checks within-batch
		/// member-key uniqueness and that the signer's balance covers the sum of all inner asset
		/// amounts.
		///
		/// This call dispatches each inner load by re-running the same checks the extension
		/// just performed (see [`RecyclerManager::load`]). The redundancy matches the defensive
		/// pattern used by [`Self::load_recycler_with_external_asset_unpaid`]: a dispatch path
		/// that fails any of these checks is a logic bug in the extension, not a user error.
		///
		/// The instance is fixed for the whole batch rather than per item, because the extension
		/// checks the signer's balance once against the summed cost of every item, which is only
		/// meaningful within one underlying asset.
		///
		/// On a sponsored instance, the pot's free balance must cover the deposits of every key
		/// loaded here, the batch being charged as one.
		///
		/// The call is free.
		#[pallet::call_index(16)]
		#[pallet::weight(T::WeightInfo::load_recycler_with_external_asset_unpaid()
			.saturating_mul(items.len() as u64)
			.saturating_add(Pallet::<T>::charge_load_deposit_weight(*instance_id)))]
		pub fn load_recycler_with_external_asset_unpaid_batch(
			origin: OriginFor<T>,
			instance_id: InstanceId,
			items: BoundedVec<UnpaidLoadInput<T>, T::MaxBatchUnpaidLoad>,
		) -> DispatchResultWithPostInfo {
			let Ok(Origin::<T>::InfallibleUnpaidSigned { who }) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};

			let n = items.len() as u32;
			for item in items.into_iter() {
				Self::do_unpaid_load(
					&who,
					instance_id,
					item.preservation,
					item.value,
					item.member_key,
				)?;
			}

			Self::charge_load_deposit(instance_id, n)
				.defensive_proof("coinage: load deposit checked in validate")?;

			Ok(Pays::No.into())
		}

		/// Unload a recycler to mint a new coin.
		///
		/// The origin must be a [Origin::UnloadToken] with `fee: UnloadFee::Prepaid`, which can be
		/// obtained from the transaction extension [`AsCoinage`](crate::extension::AsCoinage) using
		/// `AsUnloadTokenPeople`,
		/// `AsUnloadTokenLitePeople`, or `AsUnloadTokenPaid` variants.
		///
		/// This function allows a user to prove they own one or more coins in a recycler ring
		/// without revealing which specific coins they own. It consolidates one or multiple inputs
		/// into a single output coin.
		///
		/// Parameters:
		/// * `aliases`: the list of aliases corresponding to the member keys included in the
		///   recycler. The proofs for these aliases are contained in the origin.
		/// * `instance_id`, `value` and `index`: identifies the recycler being unloaded.
		/// * `_revision`: the recycler revision used for the alias_proofs.
		/// * `to`: the destination account for the new coin.
		///
		/// Requirements:
		/// * The origin must be [Origin::UnloadToken] with `fee: UnloadFee::Prepaid`.
		/// * The recycler identified by `instance_id`, `value` and `index` must exist.
		/// * The alias proofs provided in the origin must be valid for the recycler's revision.
		/// * The `aliases` provided must match the aliases derived from the proofs.
		/// * The aliases must not have been already unloaded from this recycler.
		/// * The number of aliases must be a power of two.
		/// * The resulting consolidated value must not exceed [Config::MaximumExponent].
		// The `MaxConsolidation` is enforced through the origin `UnloadToken`.
		#[pallet::call_index(4)]
		#[pallet::weight(Pallet::<T>::unload_recycler_into_coin_weight(aliases.len())
			.saturating_add(Pallet::<T>::settle_load_deposit_weight(*instance_id)))]
		pub fn unload_recycler_into_coin(
			origin: OriginFor<T>,
			instance_id: InstanceId,
			aliases: BoundedVec<Alias, T::MaxConsolidation>,
			value: Denomination,
			index: RingIndex,
			revision: RevisionIndex,
			to: T::AccountId,
		) -> DispatchResult {
			// Unloading into coin requires Prepaid fee (via free or paid unload token)
			let Ok(Origin::UnloadToken { alias_proofs, proven_msg, fee: UnloadFee::Prepaid }) =
				origin.into()
			else {
				return Err(DispatchError::BadOrigin.into());
			};
			ensure!(!aliases.is_empty(), Error::<T>::EmptyInputs);
			ensure!(aliases.len().is_power_of_two(), Error::<T>::InvalidConsolidation);
			let increment = aliases
				.len()
				.trailing_zeros()
				.try_into()
				.map_err(|_| Error::<T>::ConsolidationTooBig)?;
			let new_value = value.checked_add(increment).ok_or(Error::<T>::ConsolidationTooBig)?;
			ensure!(new_value <= T::MaximumExponent::get(), Error::<T>::ConsolidationTooBig);
			RecyclerManager::<T>::unload(
				instance_id,
				value,
				index,
				revision,
				&aliases,
				&alias_proofs,
				&proven_msg,
			)?;
			Self::settle_load_deposits(instance_id, aliases.len() as u32);
			let input_count = aliases.len() as u32;

			// The destination has no coin, as verified during validation.
			CoinsByOwner::<T>::insert(&to, Coin { instance_id, value: new_value, age: 0 });
			Self::deposit_event(Event::RecyclerUnloadedIntoCoin {
				instance_id,
				to,
				input_value: value,
				output_value: new_value,
				input_count,
			});

			Ok(())
		}

		/// Unload a recycler to withdraw the underlying external asset.
		///
		/// The origin must be [Origin::UnloadToken], which can be obtained from the transaction
		/// extension [`AsCoinage`](crate::extension::AsCoinage).
		///
		/// When `fee` is [UnloadFee::Prepaid] (via free or paid unload token), no fee is deducted.
		/// When `fee` is [UnloadFee::FromOutput], the fee is deducted from the unloaded assets:
		/// the asset is converted into the native currency, so the amount deducted depends on the
		/// market and is bounded by `max_fee`.
		///
		/// This function allows a user to withdraw their coins back into the underlying
		/// asset (e.g., an external asset).
		///
		/// Parameters:
		/// * `aliases`: the list of aliases corresponding to the member keys included in the
		///   recycler. The proofs for these aliases are contained in the origin.
		/// * `instance_id`, `value` and `index`: identifies the recycler being unloaded.
		/// * `_revision`: the recycler revision used for the alias_proofs.
		/// * `to`: the destination account for the underlying asset.
		/// * `max_fee`: the maximum amount of the unloaded asset the fee may consume. Whatever the
		///   fee does not consume goes to `to`. It is ignored for [UnloadFee::Prepaid], which takes
		///   no fee out of the output.
		///
		/// Requirements:
		/// * The origin must be [Origin::UnloadToken].
		/// * The recycler identified by `instance_id`, `value` and `index` must exist.
		/// * The alias proofs provided in the origin must be valid for the recycler's revision.
		/// * The aliases must not have been already unloaded (except for the first one when `fee`
		///   is [UnloadFee::FromOutput], which was pre-marked in the extension).
		// The `MaxConsolidation` is enforced through the origin `UnloadToken`.
		#[pallet::call_index(5)]
		#[pallet::weight(Pallet::<T>::unload_recycler_into_external_asset_max_weight(aliases.len())
			.saturating_add(Pallet::<T>::settle_load_deposit_weight(*instance_id)))]
		pub fn unload_recycler_into_external_asset(
			origin: OriginFor<T>,
			instance_id: InstanceId,
			aliases: BoundedVec<Alias, T::MaxConsolidation>,
			value: Denomination,
			index: RingIndex,
			revision: RevisionIndex,
			to: T::AccountId,
			max_fee: FungiblesBalanceOf<T>,
		) -> DispatchResultWithPostInfo {
			// Convert to single-element input for unified processing
			let input = UnloadRecyclerInput { value, index, revision, aliases };
			let inputs = [input];

			let asset_unit = Self::instance(instance_id)?.asset_unit;
			let amount_for_value = Self::denomination_to_asset_amount(asset_unit, value)
				.map_err(|e| e.into_pallet_error::<T>())?;
			let total_amount =
				amount_for_value.saturating_mul((inputs[0].aliases.len() as u32).into());

			let Ok(Origin::UnloadToken { alias_proofs, proven_msg, fee }) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};
			let alias_count = inputs[0].aliases.len();

			let actual_weight = match fee {
				UnloadFee::Prepaid =>
					Self::unload_recycler_into_external_asset_prepaid_weight(alias_count),
				UnloadFee::FromOutput { .. } =>
					Self::unload_recycler_into_external_asset_from_output_weight(alias_count),
			}
			.saturating_add(Self::settle_load_deposit_weight(instance_id));

			let result: DispatchResult = (|| {
				Self::process_unload_inputs_with_fee(
					instance_id,
					&inputs,
					&alias_proofs,
					&proven_msg,
					fee,
				)?;
				Self::settle_load_deposits(instance_id, alias_count as u32);

				let transfer_amount = match fee {
					// The fee is already paid, so `max_fee` bounds nothing and is ignored.
					UnloadFee::Prepaid => total_amount,
					UnloadFee::FromOutput { .. } => {
						// The fee cannot take more than the caller allowed, nor more than what is
						// being unloaded. The rest of the output goes to `to` untouched.
						let spent = Self::charge_unload_token_fee_from_hold(
							instance_id,
							max_fee,
							total_amount,
						)?;
						total_amount.saturating_sub(spent)
					},
				};

				Self::transfer_external_asset(instance_id, &to, transfer_amount)?;
				Self::deposit_event(Event::RecyclerUnloadedIntoExternalAsset {
					instance_id,
					to,
					value,
					input_count: inputs[0].aliases.len() as u32,
					amount: transfer_amount,
				});

				Ok(())
			})();

			result.map_err(|e| DispatchErrorWithPostInfo {
				post_info: PostDispatchInfo {
					actual_weight: Some(actual_weight),
					pays_fee: Pays::Yes,
				},
				error: e,
			})?;

			Ok(Some(actual_weight).into())
		}

		/// Pay the fee to register a member key for a paid unload token using a coin.
		///
		/// The origin must be a [Origin::Coin], which can be obtained from the transaction
		/// extension [`AsCoinage`](crate::extension::AsCoinage).
		///
		/// The coin is consumed. The fee is deducted from the coin's value: the asset is converted
		/// into the native currency, which is transferred to [Config::FeeDestination]. The
		/// remaining value of the coin is destroyed.
		///
		/// If the call fails, the origin coin is still consumed.
		///
		/// To protect the user against varying fees, if the coin's value is less than the fee, the
		/// call is invalid (an invalid call never goes into a block).
		///
		/// This is the one asset-paying call with no caller-settable bound on the fee, and it needs
		/// none: the coin is consumed whole either way, so its value is the bound, and whatever the
		/// conversion does not take is destroyed rather than returned. A caller who wants to bound
		/// what the fee costs pays for the token with
		/// [`Self::pay_for_recycler_unload_fee_token_with_external_asset`] instead.
		///
		/// The `proof_of_ownership` is a signature of the caller's account ID by the `member_key`.
		/// This ensures the caller controls the member key to prevent front-running.
		///
		/// Requirements:
		/// * The coin's age must be less than [Config::MaximumAge].
		/// * The denomination must be sufficient to cover the fee.
		/// * The `member_key` must be valid and not already used.
		/// * The `proof_of_ownership` must be valid.
		#[pallet::call_index(6)]
		#[pallet::weight(T::WeightInfo::pay_for_recycler_unload_fee_token_with_coin())]
		pub fn pay_for_recycler_unload_fee_token_with_coin(
			origin: OriginFor<T>,
			member_key: MemberOf<T>,
			proof_of_ownership: <CryptoOf<T> as GenerateVerifiable>::Signature,
		) -> DispatchResult {
			let Ok(Origin::Coin { coin_id, coin }) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};
			let instance_id = coin.instance_id;

			let record = Self::instance(instance_id)
				.defensive_proof("coinage: instance checked in validate")?;
			let amount = Self::denomination_to_asset_amount(record.asset_unit, coin.value)
				.map_err(|e| e.into_pallet_error::<T>())?;
			let fee = Self::quote_paid_unload_token_fee_in_asset(instance_id).map_err(|e| {
				log::error!(
					target: LOG_TARGET,
					"coinage: fee conversion became unavailable after validation",
				);
				e.into_pallet_error::<T>()
			})?;
			// Validation quoted the fee against this same state and found the coin worth at least
			// as much, and the coin's value comes from the origin, so the caller cannot have
			// changed it in between: the price moving is the only way to get here.
			if amount < fee {
				log::error!(
					target: LOG_TARGET,
					"coinage: fee conversion moved above the coin's value after validation",
				);
				return Err(Error::<T>::CoinAmountBelowFee.into());
			}

			// The coin's whole value is consumed either way, so its value is the bound on what the
			// conversion may take. The part the fee does not take is destroyed.
			// `CoinAmountBelowFee` above is the bound that can trip here, so neither of the
			// helper's own bounds can.
			let spent = Self::charge_unload_token_fee_from_hold(instance_id, amount, amount)?;
			let remaining = amount.saturating_sub(spent);
			TotalValueOfDestroyedCoins::<T>::mutate(instance_id, |v| {
				*v = v.saturating_add(remaining)
			});

			PaidTknManager::<T>::add_member(coin_id, member_key, proof_of_ownership)?;
			Self::deposit_event(Event::PaidUnloadTokenRegisteredWithCoin {
				instance_id,
				fee: spent,
				destroyed: remaining,
			});

			Ok(())
		}

		/// Pay the fee to register a member key for a paid unload token using the native currency.
		///
		/// The origin must be Signed.
		///
		/// This adds the `member_key` to a "paid unload token ring". Being part of this ring
		/// allows the user to later generate an `UnloadToken` to unload a recycler.
		///
		/// The fee is transferred from the caller to [Config::FeeDestination].
		///
		/// The `proof_of_ownership` is a signature of the caller's account ID by the `member_key`.
		/// This ensures the caller controls the member key to prevent front-running.
		///
		/// Requirements:
		/// * The `member_key` must be valid and not already used.
		/// * The `proof_of_ownership` must be valid.
		#[pallet::call_index(7)]
		#[pallet::weight(T::WeightInfo::pay_for_recycler_unload_fee_token_with_native())]
		pub fn pay_for_recycler_unload_fee_token_with_native(
			origin: OriginFor<T>,
			member_key: MemberOf<T>,
			proof_of_ownership: <CryptoOf<T> as GenerateVerifiable>::Signature,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;

			let fee_native = Self::paid_unload_token_fee_in_native();

			T::NativeFungible::transfer(
				&who,
				&T::FeeDestination::get(),
				fee_native,
				Preservation::Protect,
			)?;

			PaidTknManager::<T>::add_member(who.clone(), member_key, proof_of_ownership)?;
			Self::deposit_event(Event::PaidUnloadTokenRegisteredWithNative {
				who,
				fee: fee_native,
			});
			Ok(())
		}

		/// Pay the fee to register a member key for a paid unload token using the underlying
		/// asset of the given instance.
		///
		/// The origin must be Signed.
		///
		/// This adds the `member_key` to a "paid unload token ring". Being part of this ring
		/// allows the user to later generate an `UnloadToken` to unload a recycler.
		///
		/// The fee are charged in the underlying asset of the specified instance, and converted
		/// into the native currency to be transferred to the fee destination.
		/// `max_fee` bounds how much of the asset the conversion may take.
		///
		/// The `proof_of_ownership` is a signature of the caller's account ID by the `member_key`.
		/// This ensures the caller controls the member key to prevent front-running.
		///
		/// The `instance_id` only selects which instance's underlying asset the fee is paid in.
		/// The resulting token is not bound to that instance and can be consumed to unload any
		/// instance's recycler, which is why the fee is the same for all of them.
		///
		/// Unlike the signed unload calls, this one is not pre-checked against `max_fee` during
		/// transaction validation and does not refund the weight it did not use: a conversion that
		/// moved past `max_fee` between the caller quoting it and the transaction being included
		/// fails the dispatch at the full benchmarked weight. Callers should leave headroom in
		/// `max_fee`.
		///
		/// Requirements:
		/// * The `instance_id` must refer to an existing instance.
		/// * The `member_key` must be valid and not already used.
		/// * The `proof_of_ownership` must be valid.
		/// * `max_fee` must cover the converted fee.
		#[pallet::call_index(8)]
		#[pallet::weight(T::WeightInfo::pay_for_recycler_unload_fee_token_with_external_asset())]
		pub fn pay_for_recycler_unload_fee_token_with_external_asset(
			origin: OriginFor<T>,
			instance_id: InstanceId,
			member_key: MemberOf<T>,
			proof_of_ownership: <CryptoOf<T> as GenerateVerifiable>::Signature,
			max_fee: FungiblesBalanceOf<T>,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;

			let fee_native = Self::paid_unload_token_fee_in_native();
			let quoted_fee = Self::quote_asset_for_native_fee(instance_id, fee_native)
				.map_err(|e| e.into_pallet_error::<T>())?;
			ensure!(quoted_fee <= max_fee, Error::<T>::FeeExceedsMaxFee);
			let asset_id = Self::instance(instance_id)
				.defensive_proof("coinage: instance present after quote_asset_for_native_fee")?
				.asset_id;

			let fee = Self::charge_asset_and_transfer_native(
				asset_id,
				&who,
				fee_native,
				quoted_fee,
				&T::FeeDestination::get(),
			)?;

			PaidTknManager::<T>::add_member(who.clone(), member_key, proof_of_ownership)?;
			Self::deposit_event(Event::PaidUnloadTokenRegisteredWithExternalAsset {
				instance_id,
				who,
				fee,
			});

			Ok(())
		}

		// NOTE: we may want to add pay_for_recycler_unload_fee_token_with_any_asset
		// and deprecate both pay_for_recycler_unload_fee_token_with_external_asset
		// and pay_for_recycler_unload_fee_token_with_native.

		/// Unload a recycler into a mixed output of external asset and freshly loaded coins.
		///
		/// The origin must be [Origin::UnloadToken], which can be obtained from the transaction
		/// extension [`AsCoinage`](crate::extension::AsCoinage).
		///
		/// This function allows a user to offboard part of the unloaded value into the underlying
		/// asset while reminting the rest as freshly loaded recycler coins.
		///
		/// When `fee` is [UnloadFee::Prepaid], `external_asset_amount` is transferred as-is.
		/// When `fee` is [UnloadFee::FromOutput], the fee is deducted from the specified
		/// `external_asset_amount`, so the recipient receives the remainder. The asset is
		/// converted into the native currency to pay the fee, so the amount deducted depends on
		/// the market and is bounded by `max_fee`.
		///
		/// Parameters:
		/// * `aliases`: the list of aliases corresponding to the member keys included in the
		///   recycler. The proofs for these aliases are contained in the origin.
		/// * `instance_id`, `value` and `index`: identifies the recycler being unloaded.
		/// * `revision`: the recycler revision used for the alias proofs.
		/// * `to`: the destination account for the external asset portion.
		/// * `external_asset_amount`: the gross asset portion to offboard from the unloaded value.
		/// * `loaded_coins`: the freshly loaded recycler coins to mint from the remaining unloaded
		///   value.
		/// * `max_fee`: the maximum amount of `external_asset_amount` the fee may consume. Whatever
		///   the fee does not consume goes to `to`. It is ignored for [UnloadFee::Prepaid], which
		///   takes no fee out of the output.
		///
		/// The total unloaded value must always equal the asset portion plus the loaded-coin
		/// portion. In `FromOutput` mode, the asset portion must be large enough to cover the
		/// unload fee.
		///
		/// Requirements:
		/// * The origin must be [Origin::UnloadToken].
		/// * The recycler identified by `instance_id`, `value` and `index` must exist.
		/// * The alias proofs provided in the origin must be valid for the recycler's revision.
		/// * The aliases must not have been already unloaded (except for the first one when `fee`
		///   is [UnloadFee::FromOutput], which was pre-marked in the extension).
		/// * `loaded_coins` must not be empty, and all loaded-coin member keys must be valid and
		///   unused.
		/// * The total unloaded value must equal `external_asset_amount` plus the total asset value
		///   of `loaded_coins`.
		/// * When using [UnloadFee::FromOutput], `external_asset_amount` must cover the fee.
		/// * On a sponsored instance, the pot's free balance must cover the deposits of the
		///   `loaded_coins` keys, without crediting the deposits this unload releases.
		#[pallet::call_index(9)]
		#[pallet::weight(
			Pallet::<T>::unload_recycler_into_external_asset_and_loaded_coins_max_weight(
				aliases.len(),
				loaded_coins.len()
			)
			.saturating_add(Pallet::<T>::settle_load_deposit_weight(*instance_id))
			.saturating_add(Pallet::<T>::charge_load_deposit_weight(*instance_id))
		)]
		pub fn unload_recycler_into_external_asset_and_loaded_coins(
			origin: OriginFor<T>,
			instance_id: InstanceId,
			aliases: BoundedVec<Alias, T::MaxConsolidation>,
			value: Denomination,
			index: RingIndex,
			revision: RevisionIndex,
			to: T::AccountId,
			external_asset_amount: FungiblesBalanceOf<T>,
			loaded_coins: BoundedVec<(Denomination, MemberOf<T>), T::MaxSplitOutputs>,
			max_fee: FungiblesBalanceOf<T>,
		) -> DispatchResultWithPostInfo {
			let Ok(Origin::UnloadToken { alias_proofs, proven_msg, fee }) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};
			let alias_count = aliases.len();
			let loaded_coin_count = loaded_coins.len();

			let actual_weight = match fee {
				UnloadFee::Prepaid =>
					Self::unload_recycler_into_external_asset_and_loaded_coins_prepaid_weight(
						alias_count,
						loaded_coin_count,
					),
				UnloadFee::FromOutput { .. } =>
					Self::unload_recycler_into_external_asset_and_loaded_coins_from_output_weight(
						alias_count,
						loaded_coin_count,
					),
			}
			.saturating_add(Self::settle_load_deposit_weight(instance_id))
			.saturating_add(Self::charge_load_deposit_weight(instance_id));

			let result: DispatchResult = (|| {
				// Keep the mixed-output invariant check in dispatch as the final safety
				// guard for the pallet logic itself, even though the extension validates
				// the same shape earlier.
				Self::validate_mixed_output_outputs(
					Self::instance(instance_id)?.asset_unit,
					value,
					aliases.len() as u32,
					external_asset_amount,
					&loaded_coins,
				)
				.map_err(MixedOutputValidationError::into_pallet_error::<T>)?;

				let input = UnloadRecyclerInput { value, index, revision, aliases };
				let inputs = [input];

				Self::process_unload_inputs_with_fee(
					instance_id,
					&inputs,
					&alias_proofs,
					&proven_msg,
					fee,
				)?;
				// Settle before charging: the two are independent (the solvency check in
				// validation does not credit the release), but the settled units must come
				// off the ledger before the fresh voucher keys go on.
				Self::settle_load_deposits(instance_id, alias_count as u32);
				RecyclerManager::<T>::load_batch_grouped(instance_id, &loaded_coins)
					.map_err(|e| e.into_pallet_error::<T>())?;
				Self::charge_load_deposit(instance_id, loaded_coin_count as u32)?;

				let transfer_amount = match fee {
					UnloadFee::Prepaid => external_asset_amount,
					UnloadFee::FromOutput { .. } => {
						let spent = Self::charge_unload_token_fee_from_hold(
							instance_id,
							max_fee,
							external_asset_amount,
						)?;
						external_asset_amount.saturating_sub(spent)
					},
				};

				Self::transfer_external_asset(instance_id, &to, transfer_amount)?;
				Self::deposit_event(Event::RecyclerUnloadedIntoExternalAssetAndLoadedCoins {
					instance_id,
					to,
					value,
					input_count: inputs[0].aliases.len() as u32,
					external_asset_amount: transfer_amount,
					loaded_coin_count: loaded_coins.len() as u32,
				});

				Ok(())
			})();

			result.map_err(|e| DispatchErrorWithPostInfo {
				post_info: PostDispatchInfo {
					actual_weight: Some(actual_weight),
					pays_fee: Pays::Yes,
				},
				error: e,
			})?;

			Ok(Some(actual_weight).into())
		}

		/// Unload a recycler to withdraw the underlying external asset (non-anonymous).
		///
		/// Convenience wrapper around [Self::unload_recyclers_into_external_asset_non_anonymous]
		/// for the single-recycler case.
		///
		/// See [Self::unload_recyclers_into_external_asset_non_anonymous] for full documentation.
		#[pallet::call_index(11)]
		#[pallet::weight(
			Pallet::<T>::unload_recycler_into_external_asset_non_anonymous_weight(
				alias_proofs.len()
			)
			.saturating_add(Pallet::<T>::settle_load_deposit_weight(*instance_id))
		)]
		pub fn unload_recycler_into_external_asset_non_anonymous(
			origin: OriginFor<T>,
			instance_id: InstanceId,
			input: UnloadRecyclerInput<T::MaxConsolidation>,
			alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation>,
			to: T::AccountId,
			fee_currency: FeeCurrency,
			max_fee: FungiblesBalanceOf<T>,
		) -> DispatchResultWithPostInfo {
			Self::unload_recyclers_into_external_asset_non_anonymous(
				origin,
				instance_id,
				// `MaxConsolidation` is at least one, asserted by `integrity_test`, so the single
				// input always fits.
				BoundedVec::truncate_from(alloc::vec![input]),
				alias_proofs,
				to,
				fee_currency,
				max_fee,
			)
		}

		/// Unload multiple recyclers to withdraw the underlying external asset (non-anonymous).
		///
		/// This is a signed-origin version of [`Self::unload_recycler_into_external_asset`]
		/// where the fee is paid explicitly by the signer rather than through the
		/// ring-authenticated unload token, and for multiple recyclers.
		///
		/// The fee charged is one unload token fee per recycler (i.e., `inputs.len()`).
		///
		/// Every input unloads from `instance_id`: one call cannot span instances, because the
		/// unloaded value is summed and paid out as a single transfer of one underlying asset.
		///
		/// Parameters:
		/// * `instance_id`: the instance every input unloads from.
		/// * `inputs`: A list of inputs, specifying the recycler and aliases to unload. At most
		///   [`Config::MaxConsolidation`] inputs, one alias of one input per proof.
		/// * `alias_proofs`: the proofs for all aliases across all inputs, signed over a message
		///   that includes the signer. The proofs must correspond sequentially to the aliases in
		///   `inputs`.
		/// * `to`: the destination account for the asset.
		/// * `fee_currency`: whether to pay the fee in native currency or external asset.
		/// * `max_fee`: the most the fee may cost the signer, in `fee_currency`: the native fee for
		///   [FeeCurrency::Native], and the amount of the signer's asset the conversion into the
		///   native fee may take for [FeeCurrency::ExternalAsset].
		///
		/// Requirements:
		/// * The origin must be Signed.
		/// * The `instance_id` must refer to an existing instance.
		/// * All specified recyclers must exist.
		/// * The alias proofs must correspond sequentially to the aliases in `inputs`.
		/// * `inputs` must not be empty and each element must contain at least one alias.
		/// * `alias_proofs` must hold exactly one proof per alias across all `inputs`.
		/// * The signer must have sufficient balance to pay the fee (one fee per recycler).
		/// * `max_fee` must cover the fee in `fee_currency`.
		#[pallet::call_index(12)]
		#[pallet::weight(Pallet::<T>::unload_recyclers_into_external_asset_non_anonymous_weight(
			alias_proofs.len() as u32
		).saturating_add(Pallet::<T>::settle_load_deposit_weight(*instance_id)))]
		pub fn unload_recyclers_into_external_asset_non_anonymous(
			origin: OriginFor<T>,
			instance_id: InstanceId,
			inputs: BoundedVec<UnloadRecyclerInput<T::MaxConsolidation>, T::MaxConsolidation>,
			alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation>,
			to: T::AccountId,
			fee_currency: FeeCurrency,
			max_fee: FungiblesBalanceOf<T>,
		) -> DispatchResultWithPostInfo {
			let signer = ensure_signed(origin)?;

			ensure!(!inputs.is_empty(), Error::<T>::EmptyInputs);

			// ensure lengths are a match and fail early if otherwise.
			let mut input_count: u32 = 0;
			for input in &inputs {
				ensure!(!input.aliases.is_empty(), Error::<T>::EmptyInputs);
				input_count = input_count.saturating_add(input.aliases.len() as u32);
			}
			ensure!(input_count as usize == alias_proofs.len(), Error::<T>::ProofAndAliasMismatch);

			// Charge the fee first, before any proof is verified, and early exit with a refund if
			// the signer cannot pay it or `max_fee` exceeded.
			Self::charge_fees_from_signer(
				&signer,
				instance_id,
				fee_currency,
				inputs.len() as u32,
				max_fee,
			)
			.map_err(|error| DispatchErrorWithPostInfo {
				post_info: PostDispatchInfo {
					actual_weight: Some(
						T::WeightInfo::unload_recyclers_into_external_asset_non_anonymous_fee_fail(
						),
					),
					pays_fee: Pays::Yes,
				},
				error,
			})?;

			let asset_unit = Self::instance(instance_id)?.asset_unit;

			// Calculate total amount
			let mut total_amount: FungiblesBalanceOf<T> = Zero::zero();
			for input in &inputs {
				let amount_per_coin = Self::denomination_to_asset_amount(asset_unit, input.value)
					.map_err(|e| e.into_pallet_error::<T>())?;
				let amount_for_input =
					amount_per_coin.saturating_mul((input.aliases.len() as u32).into());
				total_amount = total_amount.saturating_add(amount_for_input);
			}

			// Construct proven_msg including the signer for non-anonymous proof binding.
			// The instance is part of the intent because a denomination and ring index alone do
			// not identify a recycler.
			let proven_msg = blake2_256(&(instance_id, &inputs, &to, &signer).encode());

			Self::process_unload_inputs(instance_id, &inputs, &alias_proofs, &proven_msg, false)?;
			Self::settle_load_deposits(instance_id, input_count);
			Self::transfer_external_asset(instance_id, &to, total_amount)?;
			Self::deposit_event(Event::RecyclersUnloadedIntoExternalAssetNonAnonymous {
				instance_id,
				who: signer,
				to,
				input_count,
				amount: total_amount,
				fee_currency,
			});

			// Refund the transaction weight fee since we charged explicitly
			Ok(Pays::No.into())
		}

		/// Recover a coin from an archived recycler into the external asset.
		///
		/// This is a signed call.
		///
		/// It allows a user to unload a coin from an archived recycler.
		/// The unload token fee is charged to the signer, and the call is not refunded, as it
		/// accounts for the extra proof verification and archived recycler update.
		///
		/// Parameters:
		/// * `instance_id`, `value` and `index`: identify the archived recycler ring.
		/// * `recycler_root`: the deleted ring's ring-VRF root; validated together with
		///   `unloaded_root` against the stored archival commitment.
		/// * `unloaded_root`: the current root of the unloaded-aliases trie.
		/// * `alias_proof`: a ring-VRF membership proof, created over the message binding
		///   `blake2_256(UNLOAD_ARCHIVED_MSG_PREFIX ++ signer)` in the recycler unloading context
		///   UNLOADING_RECYCLER_CONTEXT.
		/// * `non_inclusion_proof`: trie nodes proving the caller's alias is absent from
		///   `unloaded_root` (i.e. it was never unloaded); must also cover the insertion path of
		///   the alias so the new root can be recomputed.
		/// * `to`: the account receiving the recovered denomination.
		/// * `fee_currency`: whether the unload fee is paid in native currency or external asset.
		/// * `max_fee`: the most the fee may cost the signer, in `fee_currency`: the native fee for
		///   [FeeCurrency::Native], and the amount of the signer's asset the conversion into the
		///   native fee may take for [FeeCurrency::ExternalAsset].
		///
		/// On success the full denomination is released to `to`, the alias is added to the unloaded
		/// set (so it cannot be recovered again), and the archive's recoverable count is
		/// decremented (the entry is removed once drained).
		/// (No [`Config::LoadDeposit`] is settled here, it was already settled when the recycler
		/// was archived.)
		///
		/// The unloaded-aliases trie needed for `unloaded_root` and `non_inclusion_proof` can be
		/// reconstructed offchain by listening to the [`Event::RecyclerAliasUnloaded`],
		/// [`Event::RecyclerArchived`] and [`Event::ArchivedRecyclerUnloadedIntoExternalAsset`]
		/// events.
		///
		/// This call conflicts with any other call that unloads from the same archived recycler:
		/// each unload updates the commitment, so the proofs in the competing call become outdated.
		/// `recycler_root` and `unloaded_root` are checked against the stored commitment at
		/// transaction validation, therefore resolving such conflicts without charging fees by
		/// marking outdated proofs as invalid.
		#[pallet::call_index(17)]
		#[pallet::weight(T::WeightInfo::unload_archived_recycler_into_external_asset())]
		pub fn unload_archived_recycler_into_external_asset(
			origin: OriginFor<T>,
			instance_id: InstanceId,
			value: Denomination,
			index: RingIndex,
			recycler_root: MembersOf<T>,
			unloaded_root: H256,
			alias_proof: ProofOf<T>,
			non_inclusion_proof: BoundedVec<
				BoundedVec<u8, ConstU32<MAX_TRIE_NODE_LEN>>,
				ConstU32<MAX_TRIE_PROOF_NODES>,
			>,
			to: T::AccountId,
			fee_currency: FeeCurrency,
			max_fee: FungiblesBalanceOf<T>,
		) -> DispatchResultWithPostInfo {
			let signer = ensure_signed(origin)?;

			// Charge the fee first, before any proof is verified, and early exit with a refund if
			// the signer cannot pay it or `max_fee` exceeded.
			Self::charge_fees_from_signer(&signer, instance_id, fee_currency, 1, max_fee).map_err(
				|error| DispatchErrorWithPostInfo {
					post_info: PostDispatchInfo {
						actual_weight: Some(
							T::WeightInfo::unload_archived_recycler_into_external_asset_fee_fail(),
						),
						pays_fee: Pays::Yes,
					},
					error,
				},
			)?;

			let asset_unit = Self::instance(instance_id)?.asset_unit;
			let proven_msg = Self::unload_archived_proof_message(&signer);

			let alias = RecyclerManager::<T>::unload_archived(
				instance_id,
				value,
				index,
				&recycler_root,
				unloaded_root,
				&alias_proof,
				non_inclusion_proof.into_iter().map(BoundedVec::into_inner),
				&proven_msg,
			)?;

			// Release the recovered coin's value. It was retained (not destroyed) at cleanup, so no
			// destroyed-coin accounting changes.
			let amount = Self::denomination_to_asset_amount(asset_unit, value)
				.map_err(|e| e.into_pallet_error::<T>())?;
			Self::transfer_external_asset(instance_id, &to, amount)?;

			Self::deposit_event(Event::ArchivedRecyclerUnloadedIntoExternalAsset {
				instance_id,
				who: signer,
				to,
				value,
				ring_index: index,
				amount,
				fee_currency,
				alias,
			});

			// The full weight is charged: unlike the anonymous unloads this call is not refunded,
			// it accounts for the extra proof verification and archive update.
			Ok(().into())
		}

		/// Unload a recycler to mint multiple new coins (split).
		///
		/// The origin must be a [Origin::UnloadToken] with `fee: UnloadFee::Prepaid`.
		///
		/// This function combines the functionality of [Self::unload_recycler_into_coin] and
		/// [Self::split] in a single atomic operation. The resulting coins' age is 1 because
		/// the action of splitting age coins. This is also important because resulting coins
		/// are not entirely fresh, they can be linked to other coins.
		///
		/// Unlike [Self::unload_recycler_into_coin], this call does **not** require the number of
		/// aliases to be a power of two.
		///
		/// Parameters:
		/// * `aliases`: the list of aliases corresponding to the member keys included in the
		///   recycler. The proofs for these aliases are contained in the origin.
		/// * `instance_id`, `value` and `index`: identifies the recycler being unloaded.
		/// * `revision`: the recycler revision used for the alias_proofs.
		/// * `split_into`: a vector of pairs, each pair containing a denomination and a list of
		///   destination account ids.
		/// * `max_fee`: the maximum fee the caller is willing to pay, expressed in the underlying
		///   asset balance. It must be equal to the difference between the total value of the
		///   unloaded coins and the total value of the new coins defined in `split_into`.
		///
		///   When using [UnloadFee::Prepaid], it must be zero: nothing is set aside for the fee, so
		///   `split_into` takes the whole unloaded value.
		///   When using [UnloadFee::FromOutput], this amount is deducted from the input: the asset
		///   is converted into the native network fee, which is transferred to
		///   [Config::FeeDestination], and any remainder is burned. The caller can query
		///   `get_paid_unload_token_fee_in_asset` to estimate the fee.
		///
		///   This parameter serves as a safeguard: the transaction is rejected at validation if the
		///   actual network fee exceeds `max_fee`, protecting the caller from excessive fee
		///   increases that would render the argument `split_into` invalid (unloaded funds must be
		///   higher than the split plus the fee).
		///
		/// Requirements:
		/// * The origin must be [Origin::UnloadToken].
		/// * The recycler identified by `instance_id`, `value` and `index` must exist.
		/// * The alias proofs provided in the origin must be valid for the recycler's revision.
		/// * The `aliases` provided must match the aliases derived from the proofs.
		/// * Each destination account must not already have a coin.
		/// * The total value of the new coins defined in `split_into` plus `max_fee` must equal the
		///   total value of the unloaded coins.
		/// * `max_fee` must be a multiple of the minimum coin. (This is implied by the condition
		///   above).
		/// * When using [UnloadFee::Prepaid], `max_fee` must be 0.
		/// * When using [UnloadFee::FromOutput], `max_fee` must cover the network fee.
		#[pallet::call_index(13)]
		#[pallet::weight(Pallet::<T>::unload_recycler_into_coins_max_weight(
			aliases.len(),
			split_into.iter().map(|(_, dests)| dests.len() as u32).sum::<u32>().max(1),
		).saturating_add(Pallet::<T>::settle_load_deposit_weight(*instance_id)))]
		pub fn unload_recycler_into_coins(
			origin: OriginFor<T>,
			instance_id: InstanceId,
			aliases: BoundedVec<Alias, T::MaxConsolidation>,
			value: Denomination,
			index: RingIndex,
			revision: RevisionIndex,
			split_into: BoundedVec<
				(Denomination, BoundedVec<T::AccountId, T::MaxSplitOutputs>),
				T::MaxSplitOutputs,
			>,
			max_fee: FungiblesBalanceOf<T>,
		) -> DispatchResultWithPostInfo {
			let Ok(Origin::UnloadToken { alias_proofs, proven_msg, fee }) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};
			let alias_count = aliases.len();
			let output_count: u32 = split_into.iter().map(|(_, dests)| dests.len() as u32).sum();

			let actual_weight = match fee {
				UnloadFee::Prepaid => Self::unload_recycler_into_coins_prepaid_weight(
					alias_count,
					output_count.max(1),
				),
				UnloadFee::FromOutput { .. } =>
					Self::unload_recycler_into_coins_from_output_weight(
						alias_count,
						output_count.max(1),
					),
			}
			.saturating_add(Self::settle_load_deposit_weight(instance_id));

			let result: DispatchResult = (|| {
				ensure!(!aliases.is_empty(), Error::<T>::EmptyInputs);

				let unit_per_input =
					Self::denomination_to_base_units(value).ok_or(Error::<T>::InternalError)?;

				let total_input_units = unit_per_input
					.checked_mul(aliases.len() as u32)
					.ok_or(Error::<T>::InternalError)?;

				let asset_unit = Self::instance(instance_id)?.asset_unit;
				let amount_per_unit =
					Self::denomination_to_asset_amount(asset_unit, T::MinimumExponent::get())
						.map_err(|e| e.into_pallet_error::<T>())?;
				// Get the expected total units of the split while ensuring
				// `max_fee + split == total_input_units`.
				let expected_total_units = match fee {
					UnloadFee::Prepaid => {
						ensure!(max_fee.is_zero(), Error::<T>::MaxFeeNotAllowedForPrepaid);
						total_input_units
					},
					UnloadFee::FromOutput { .. } => {
						ensure!(
							max_fee % amount_per_unit == Zero::zero(),
							Error::<T>::InvalidMaxFee
						);
						let max_fee_units: u32 = max_fee
							.checked_div(&amount_per_unit)
							.ok_or(Error::<T>::InternalError)? // `amount_per_unit` cannot be 0.
							.saturated_into();

						total_input_units
							.checked_sub(max_fee_units)
							.ok_or(Error::<T>::MaxFeeExceedsInput)?
					},
				};

				Self::validate_split_params(expected_total_units, &split_into)
					.map_err(|_| Error::<T>::InvalidSplit)?;

				let input = UnloadRecyclerInput { value, index, revision, aliases };
				let inputs = [input];

				Self::process_unload_inputs_with_fee(
					instance_id,
					&inputs,
					&alias_proofs,
					&proven_msg,
					fee,
				)?;
				Self::settle_load_deposits(instance_id, alias_count as u32);

				if let UnloadFee::FromOutput { .. } = fee {
					// Transfer the network fee to the fee destination and burn the
					// remainder. Burning is preferred over transferring the surplus to
					// the fee destination as it benefits all holders equally by reducing
					// supply, and avoids overfunding.
					//
					// The remainder exists because max_fee is the difference between the
					// split and the unloaded amount. So it is unlikely to exactly match
					// the unload token fee.
					// `max_fee` is both the bound and the whole amount set aside for the fee: it
					// is the difference between the unloaded value and the split, so there is
					// nothing else the fee could draw on.
					let spent =
						Self::charge_unload_token_fee_from_hold(instance_id, max_fee, max_fee)?;
					let remaining = max_fee.saturating_sub(spent);
					if remaining > Zero::zero() {
						TotalValueOfDestroyedCoins::<T>::mutate(instance_id, |v| {
							*v = v.saturating_add(remaining)
						});
					}
				}

				for (v, dests) in split_into {
					for dest in dests {
						// The destination has no coin as checked in validation.
						CoinsByOwner::<T>::insert(&dest, Coin { instance_id, value: v, age: 1 });
					}
				}
				Self::deposit_event(Event::RecyclerUnloadedIntoCoins { instance_id, output_count });

				Ok(())
			})();

			result.map_err(|e| DispatchErrorWithPostInfo {
				post_info: PostDispatchInfo {
					actual_weight: Some(actual_weight),
					pays_fee: Pays::Yes,
				},
				error: e,
			})?;

			Ok(Some(actual_weight).into())
		}

		/// Directly offboard a coin into the underlying external asset.
		///
		/// The origin must be a [Origin::Coin], obtained through
		/// [`AsCoinage`](crate::extension::AsCoinage) using `AsCoin`.
		///
		/// This call bypasses the recycler/unload-token offboarding flow and releases the
		/// underlying asset directly, whatever the coin's age.
		///
		/// # Privacy warning
		///
		/// Directly offboarding a coin with non-zero age publicly links the coin's transfer
		/// chain to the destination account, which compromises, to some extent, the anonymity of
		/// every previous holder in the chain: if Alice sends a coin to Bob and Bob to Charlie
		/// and Charlie directly offboards it, Alice may deduce what Bob did with the coin. A
		/// fresh coin (`age == 0`) has just been unloaded from a recycler and carries no transfer
		/// history, so it can be offboarded directly without this privacy loss. For maximum
		/// privacy, offboard coins with non-zero age through the recycler instead.
		///
		/// Parameters:
		/// * `to`: destination account that receives the released underlying asset amount.
		///
		/// Requirements:
		/// * The origin must be [Origin::Coin].
		/// * The denomination must be representable as underlying-asset amount.
		#[pallet::call_index(14)]
		#[pallet::weight(T::WeightInfo::direct_offboard_coin_into_external_asset())]
		pub fn direct_offboard_coin_into_external_asset(
			origin: OriginFor<T>,
			to: T::AccountId,
		) -> DispatchResultWithPostInfo {
			let Ok(Origin::Coin { coin_id: _, coin }) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};
			let instance_id = coin.instance_id;

			let asset_unit = Self::instance(instance_id)?.asset_unit;
			let amount = Self::denomination_to_asset_amount(asset_unit, coin.value)
				.map_err(|e| e.into_pallet_error::<T>())?;

			Self::transfer_external_asset(instance_id, &to, amount)?;
			Self::deposit_event(Event::CoinOffboardedIntoExternalAsset {
				instance_id,
				to,
				value: coin.value,
				amount,
			});

			Ok(Pays::No.into())
		}

		/// Create a sufficient coinage instance for an underlying asset.
		///
		/// The origin must satisfy [`Config::AdminOrigin`]. The asset must exist in
		/// [`Config::Fungibles`]; it may already be wrapped by other instances, so admin can
		/// always wrap it at the granularity it wants, whatever was created before.
		///
		/// The instance's recycler collections are created within this call. The pallet account
		/// must already be able to receive the underlying asset: for a non-sufficient asset it
		/// must have been touched beforehand. It must also already hold the asset's minimum
		/// balance as a buffer to avoid dustings.
		///
		/// Parameters:
		/// * `asset_id`: the underlying asset backing this instance's coins.
		/// * `asset_unit`: the asset amount of a coin of denomination zero. Must be non-zero, and
		///   must represent every denomination in `[MinimumExponent, MaximumExponent]` without
		///   truncation.
		#[pallet::call_index(18)]
		#[pallet::weight(T::WeightInfo::create_sufficient_instance())]
		pub fn create_sufficient_instance(
			origin: OriginFor<T>,
			asset_id: FungiblesAssetIdOf<T>,
			asset_unit: FungiblesBalanceOf<T>,
		) -> DispatchResult {
			T::AdminOrigin::ensure_origin(origin)?;

			// Nothing is minted or transferred here: admin is expected to have provisioned the
			// pallet account beforehand, which `do_create_instance` checks.
			Self::do_create_instance(asset_id, asset_unit, InstanceMode::Sufficient, None)?;

			Ok(())
		}

		/// Create a sponsored coinage instance wrapping `asset_id`.
		///
		/// The origin must satisfy [`Config::SponsorOrigin`], which yields the paying account;
		/// with `EnsureSigned` anyone can call this. [`Config::EnablePermissionless`] must be
		/// true, otherwise no sponsored instance can be created at all. The instance's load-side
		/// costs are underwritten by a pot account derived from the instance id (see
		/// [`Pallet::pot_account`]), kept funded by sponsors through [`Pallet::fund_pot`].
		///
		/// The caller provides:
		/// - the instance creation deposit ([`Config::InstanceCreationDeposit`], taken from the
		///   caller and kept for as long as the instance is sponsored, since instances are never
		///   removed; the caller and its ticket are recorded in [`InstanceRecord::creator`]),
		/// - the pallet account's minimum balance of the underlying asset (transferred rather than
		///   minted, so a permissionless call cannot create unbacked funds),
		/// - optionally `initial_funding`, recorded as the caller's pot contribution exactly as
		///   [`Pallet::fund_pot`] would. Bundled here because the instance id is only assigned
		///   inside the call, so a separate `fund_pot` cannot be batched with the creation
		///   race-free.
		///
		/// `asset_unit` is fixed at creation and instances are never removed, but an asset is not
		/// first-come: anyone, admin included, can wrap the same asset again in its own
		/// instance at another unit, so one creator cannot fix the coin granularity of an asset
		/// for everybody else. What stops a flood of near-duplicate instances is
		/// [`Config::InstanceCreationDeposit`], held on each creator for as long as its instance
		/// is sponsored.
		///
		/// Parameters:
		/// * `asset_id`: the underlying asset backing this instance's coins.
		/// * `asset_unit`: the asset amount of a coin of denomination zero. Must be non-zero, and
		///   must represent every denomination in `[MinimumExponent, MaximumExponent]` without
		///   truncation.
		/// * `initial_funding`: an optional `(currency, amount)` pot contribution.
		#[pallet::call_index(19)]
		#[pallet::weight(T::WeightInfo::create_sponsored_instance())]
		pub fn create_sponsored_instance(
			origin: OriginFor<T>,
			asset_id: FungiblesAssetIdOf<T>,
			asset_unit: FungiblesBalanceOf<T>,
			initial_funding: Option<(FungiblesAssetIdOf<T>, FungiblesBalanceOf<T>)>,
		) -> DispatchResult {
			let creator = T::SponsorOrigin::ensure_origin(origin)?;
			ensure!(T::EnablePermissionless::get(), Error::<T>::SponsoredInstancesDisabled);

			// Requires the pallet account to be able to receive the asset and also hold its minimum
			// balance to avoid dustings.
			let pallet_acc = Self::pallet_account();
			if T::Fungibles::should_touch(asset_id.clone(), &pallet_acc) {
				T::Fungibles::touch(asset_id.clone(), &pallet_acc, &creator)?;
			}
			let minimum_balance = T::Fungibles::minimum_balance(asset_id.clone());
			let balance = T::Fungibles::balance(asset_id.clone(), &pallet_acc);
			if balance < minimum_balance {
				T::Fungibles::transfer(
					asset_id.clone(),
					&creator,
					&pallet_acc,
					minimum_balance.saturating_sub(balance),
					Preservation::Expendable,
				)?;
			}

			let instance_id = Self::do_create_instance(
				asset_id,
				asset_unit,
				InstanceMode::Sponsored,
				Some(creator.clone()),
			)?;

			if let Some((currency, amount)) = initial_funding {
				Self::do_fund_pot(&creator, instance_id, currency, amount)?;
			}

			Ok(())
		}

		/// Fund the pot of the sponsored instance `instance_id` with `amount` of `currency`.
		///
		/// The contribution is recorded per funder and per currency: the part of it not
		/// currently held as load-deposit collateral can be taken back with
		/// [`Pallet::withdraw_pot_funds`]. A plain transfer to the pot account backs loads all
		/// the same but is a donation, not withdrawable.
		///
		/// Any existing `currency` is accepted, not just the current deposit currency, so a
		/// sponsor can prefund ahead of an admin currency switch.
		///
		/// A pot with no account for `currency` has one created for it first, at the caller's
		/// expense. Whatever that costs is not part of the recorded contribution and is never
		/// refunded. The `amount` must be at least the currency's minimum balance, so a
		/// funding cannot be dusted on arrival; the pot's account then survives any withdrawal
		/// or hold.
		#[pallet::call_index(20)]
		#[pallet::weight(T::WeightInfo::fund_pot())]
		pub fn fund_pot(
			origin: OriginFor<T>,
			instance_id: InstanceId,
			currency: FungiblesAssetIdOf<T>,
			amount: FungiblesBalanceOf<T>,
		) -> DispatchResult {
			let funder = ensure_signed(origin)?;
			Self::do_fund_pot(&funder, instance_id, currency, amount)
		}

		/// Take back up to the caller's recorded contribution to the pot of `instance_id` in
		/// `currency`.
		///
		/// Only the pot's free balance can be withdrawn: held collateral is out of reach.
		#[pallet::call_index(21)]
		#[pallet::weight(T::WeightInfo::withdraw_pot_funds())]
		pub fn withdraw_pot_funds(
			origin: OriginFor<T>,
			instance_id: InstanceId,
			currency: FungiblesAssetIdOf<T>,
			amount: FungiblesBalanceOf<T>,
		) -> DispatchResult {
			let funder = ensure_signed(origin)?;
			Self::do_withdraw_pot_funds(&funder, instance_id, currency, amount)
		}

		/// Re-price every live load deposit of `instance_id` to the current
		/// [`Config::LoadDeposit`], converting the collateral to the current currency.
		///
		/// Anybody may call this. It is the only operation that changes how much collateral
		/// backs already-loaded keys, in either direction: the pot is charged the shortfall when
		/// admin has raised the price since those loads, and refunded the excess when it
		/// has lowered it. Nothing is converted between currencies: old collateral is released
		/// to the pot's free balance and the new requirement is taken fresh, so no rate feed is
		/// involved.
		///
		/// This is the companion to an admin change of [`Config::LoadDeposit`], and the
		/// permissionless remedy for an instance whose loads are refused because its old tier is
		/// still occupied.
		#[pallet::call_index(23)]
		#[pallet::weight(T::WeightInfo::collapse_load_deposits())]
		pub fn collapse_load_deposits(
			origin: OriginFor<T>,
			instance_id: InstanceId,
		) -> DispatchResult {
			ensure_signed(origin)?;
			Self::do_collapse_load_deposits(instance_id)
		}

		/// Switch a sponsored instance to `InstanceMode::Sufficient`.
		///
		/// The origin must satisfy [`Config::AdminOrigin`]: this is admin blessing
		/// an instance into the stranded-value economics. Every load deposit is released to the
		/// pot's free balance, where funders reclaim their contributions through
		/// [`Pallet::withdraw_pot_funds`] (withdrawal does not require the instance to be
		/// sponsored); only donations stay stranded. The ledger is removed, and from here on
		/// loads take no deposit and unloads release none.
		///
		/// The instance creation deposit is released if some.
		#[pallet::call_index(24)]
		#[pallet::weight(T::WeightInfo::make_instance_sufficient())]
		pub fn make_instance_sufficient(
			origin: OriginFor<T>,
			instance_id: InstanceId,
		) -> DispatchResult {
			T::AdminOrigin::ensure_origin(origin)?;

			let record = Self::instance(instance_id)?;
			ensure!(record.mode == InstanceMode::Sponsored, Error::<T>::InstanceNotSponsored);

			// Release every hold to the pot's free balance, both tiers.
			let pot = Self::pot_account(instance_id);
			for tier in record.old_load_deposit.iter().chain(record.current_load_deposit.iter()) {
				let total =
					tier.price.checked_mul(&tier.count.into()).ok_or(ArithmeticError::Overflow)?;
				let _ = T::Fungibles::release(
					tier.asset_id.clone(),
					&HoldReason::LoadDeposit.into(),
					&pot,
					total,
					Precision::Exact,
				)
				.defensive_proof("coinage: the load deposit hold covers every ledger unit");
			}

			if let Some((creator, ticket)) = record.creator {
				ticket.drop(&creator)?;
			}

			Instances::<T>::mutate(instance_id, |maybe_record| {
				if let Some(record) = maybe_record {
					record.mode = InstanceMode::Sufficient;
					record.current_load_deposit = None;
					record.old_load_deposit = None;
					record.creator = None;
				}
			});

			Self::deposit_event(Event::InstanceModeSet {
				instance_id,
				mode: InstanceMode::Sufficient,
			});
			Ok(())
		}

		/// Switch a sufficient instance to `InstanceMode::Sponsored`.
		///
		/// The origin must satisfy [`Config::AdminOrigin`]. The deposit ledger restarts
		/// from zero: keys loaded while the instance was sufficient carry no deposit, so their
		/// unloads settle against whatever the ledger holds at the time, possibly releasing
		/// deposits taken for keys loaded after the switch, or nothing once the ledger is
		/// drained. The instance therefore runs under-collateralized until its pre-switch keys
		/// stop resolving, which admin accepts by making the switch.
		///
		/// Loads stay invalid until [`Config::LoadDeposit`] is set and the pot is funded through
		/// [`Pallet::fund_pot`].
		///
		/// No [`Config::InstanceCreationDeposit`] is taken, and
		/// [`InstanceRecord::creator`] stays as it is, so an instance that went through
		/// [`Pallet::make_instance_sufficient`] comes back with no creator and no deposit, the
		/// same as one admin created.
		#[pallet::call_index(25)]
		#[pallet::weight(T::WeightInfo::make_instance_sponsored())]
		pub fn make_instance_sponsored(
			origin: OriginFor<T>,
			instance_id: InstanceId,
		) -> DispatchResult {
			T::AdminOrigin::ensure_origin(origin)?;

			let record = Self::instance(instance_id)?;
			ensure!(record.mode == InstanceMode::Sufficient, Error::<T>::InstanceAlreadySponsored);
			debug_assert!(
				record.current_load_deposit.is_none() && record.old_load_deposit.is_none(),
				"a sufficient instance carries no ledger"
			);

			Instances::<T>::mutate(instance_id, |maybe_record| {
				if let Some(record) = maybe_record {
					record.mode = InstanceMode::Sponsored;
				}
			});

			Self::deposit_event(Event::InstanceModeSet {
				instance_id,
				mode: InstanceMode::Sponsored,
			});
			Ok(())
		}

		/// Clean up an expired recycler.
		///
		/// This is a maintenance call. The origin must be authorized and from local source.
		///
		/// This removes an old recycler that has exceeded its expiration time.
		/// Any remaining (not-yet-unloaded) coins are not destroyed: the ring is archived and their
		/// backing asset stays held in the pallet account, recoverable via
		/// [`Pallet::unload_archived_recycler_into_external_asset`].
		///
		/// On a sponsored instance this settles the [`Config::LoadDeposit`] of every remaining
		/// key.
		#[pallet::authorize(|source, instance_id, value| {
			if !matches!(source, TransactionSource::InBlock | TransactionSource::Local) {
				return Err(CustomInvalidity::TransactionNotLocal.into());
			}
			let (validity, weight) =
				RecyclerManager::<T>::ensure_can_clean(*instance_id, *value)?;
			Ok((validity, weight))
		})]
		#[pallet::call_index(101)]
		#[pallet::weight(T::WeightInfo::clean_recycler(
			T::RecyclerRingExponent::get().ring_capacity(),
			T::RecyclerRingExponent::get().ring_capacity(),
		).saturating_add(Pallet::<T>::settle_load_deposit_weight(*instance_id)))]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_clean_recycler())]
		pub fn clean_recycler(
			origin: OriginFor<T>,
			instance_id: InstanceId,
			value: Denomination,
		) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;
			let (remaining_coins, member_count, archived) =
				RecyclerManager::<T>::clean_unchecked(instance_id, value)?;

			// Unloaded keys settled their deposit at unload; the archived remainder settles here.
			Self::settle_load_deposits(instance_id, remaining_coins);

			// The remaining (not-yet-unloaded) coins are not destroyed: when `remaining_coins > 0`
			// the ring is archived and their backing asset stays held in the pallet account, to be
			// recovered via `unload_archived_recycler_into_external_asset`. So
			// `TotalValueOfDestroyedCoins` is left untouched here.
			Self::deposit_event(Event::RecyclerCleaned { instance_id, value, remaining_coins });
			if let Some((ring_index, recycler_root)) = archived {
				Self::deposit_event(Event::RecyclerArchived {
					instance_id,
					value,
					ring_index,
					recycler_root,
				});
			}

			let unloaded_count = member_count.saturating_sub(remaining_coins);
			Ok(Some(
				T::WeightInfo::clean_recycler(member_count, unloaded_count)
					.saturating_add(Self::settle_load_deposit_weight(instance_id)),
			)
			.into())
		}

		/// Cleanup storage for consumed free unload tokens of old periods.
		///
		/// This is a maintenance call. The origin must be authorized and from local source.
		#[pallet::authorize(|source, period| {
			if !matches!(source, TransactionSource::InBlock | TransactionSource::Local) {
				return Err(CustomInvalidity::TransactionNotLocal.into());
			}
			let current_periods = Self::current_free_unload_token_periods();
			if current_periods.into_iter().any(|current_period| current_period <= *period) {
				return Err(InvalidTransaction::Future.into());
			}
			if ConsumedFreeUnloadTokens::<T>::iter_prefix_values(period).next().is_none() {
				return Err(InvalidTransaction::Stale.into());
			}

			let validity = ValidTransaction::with_tag_prefix("coinage:remove-consumed-free-token")
				.and_provides(period)
				.priority(tx_priority::CLEANUP)
				.into();
			Ok((validity, Weight::zero()))
		})]
		#[pallet::call_index(102)]
		#[pallet::weight(T::WeightInfo::clean_consumed_free_token(
			CLEAN_CONSUMED_FREE_TOKEN_LIMIT
		))]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_clean_consumed_free_token())]
		pub fn clean_consumed_free_token(
			origin: OriginFor<T>,
			period: Period,
		) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;
			let result = ConsumedFreeUnloadTokens::<T>::clear_prefix(
				period,
				CLEAN_CONSUMED_FREE_TOKEN_LIMIT,
				None,
			);
			Self::deposit_event(Event::ConsumedFreeTokensCleaned { period });
			Ok(Some(T::WeightInfo::clean_consumed_free_token(result.unique)).into())
		}

		/// Clean up a single ring in an expired paid unload token collection.
		///
		/// This is a maintenance call. The origin must be authorized and from local source.
		/// Rings must be cleaned sequentially (ring 0 first, then 1, etc.) before the
		/// collection can be deleted via
		/// [`delete_expired_paid_unload_token_collection`](Self::delete_expired_paid_unload_token_collection).
		#[pallet::authorize(|source, period, ring_index| {
			if !matches!(source, TransactionSource::InBlock | TransactionSource::Local) {
				return Err(CustomInvalidity::TransactionNotLocal.into());
			}
			let (validity, weight) =
				PaidTknManager::<T>::ensure_can_clean_ring(*period, *ring_index)?;
			Ok((validity, weight))
		})]
		#[pallet::call_index(104)]
		#[pallet::weight(T::WeightInfo::clean_paid_unload_token_ring(
			T::PaidUnloadTokenRingExponent::get().ring_capacity()
		))]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_clean_paid_unload_token_ring())]
		pub fn clean_paid_unload_token_ring(
			origin: OriginFor<T>,
			period: Period,
			ring_index: RingIndex,
		) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;
			let count = PaidTknManager::<T>::clean_ring_unchecked(period, ring_index)?;
			Self::deposit_event(Event::PaidUnloadTokenRingCleaned { period, ring_index });
			Ok(Some(T::WeightInfo::clean_paid_unload_token_ring(count)).into())
		}

		/// Clean up dust for recyclers.
		///
		/// This is a maintenance call. The origin must be authorized and from local source.
		/// Removes up to DUST_CLEANUP_BATCH_SIZE entries from [RecyclerAliasStates] per call to
		/// bound the operation.
		#[pallet::authorize(|source| {
			if !matches!(source, TransactionSource::InBlock | TransactionSource::Local) {
				return Err(CustomInvalidity::TransactionNotLocal.into());
			}
			RecyclerManager::<T>::ensure_can_clean_dust()
		})]
		#[pallet::call_index(105)]
		#[pallet::weight(T::WeightInfo::clean_recycler_dust(DUST_CLEANUP_BATCH_SIZE))]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_clean_recycler_dust())]
		pub fn clean_recycler_dust(origin: OriginFor<T>) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;
			let removed = RecyclerManager::<T>::clean_dust_unchecked();
			Self::deposit_event(Event::RecyclerDustCleaned);
			Ok(Some(T::WeightInfo::clean_recycler_dust(removed)).into())
		}

		/// Clean up dust for paid unload tokens.
		///
		/// This is a maintenance call. The origin must be authorized and from local source.
		#[pallet::authorize(|source| {
			if !matches!(source, TransactionSource::InBlock | TransactionSource::Local) {
				return Err(CustomInvalidity::TransactionNotLocal.into());
			}
			PaidTknManager::<T>::ensure_can_clean_dust()
		})]
		#[pallet::call_index(106)]
		#[pallet::weight(T::WeightInfo::clean_paid_unload_token_dust(DUST_CLEANUP_BATCH_SIZE))]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_clean_paid_unload_token_dust())]
		pub fn clean_paid_unload_token_dust(origin: OriginFor<T>) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;
			let removed = PaidTknManager::<T>::clean_dust_unchecked();
			Self::deposit_event(Event::PaidUnloadTokenDustCleaned);
			Ok(Some(T::WeightInfo::clean_paid_unload_token_dust(removed)).into())
		}

		/// Delete an expired paid unload token collection after all rings have been cleaned.
		///
		/// This is a maintenance call. The origin must be authorized and from local source.
		/// All rings must have been cleaned via
		/// [`clean_paid_unload_token_ring`](Self::clean_paid_unload_token_ring) before this
		/// can be called.
		#[pallet::authorize(|source, period| {
			if !matches!(source, TransactionSource::InBlock | TransactionSource::Local) {
				return Err(CustomInvalidity::TransactionNotLocal.into());
			}
			let (validity, weight) =
				PaidTknManager::<T>::ensure_can_delete_collection(*period)?;
			Ok((validity, weight))
		})]
		#[pallet::call_index(107)]
		#[pallet::weight(T::WeightInfo::delete_expired_paid_unload_token_collection())]
		#[pallet::weight_of_authorize(
			T::WeightInfo::authorize_delete_expired_paid_unload_token_collection()
		)]
		pub fn delete_expired_paid_unload_token_collection(
			origin: OriginFor<T>,
			period: Period,
		) -> DispatchResult {
			ensure_authorized(origin)?;
			PaidTknManager::<T>::delete_collection_unchecked(period)?;
			Self::deposit_event(Event::ExpiredPaidUnloadTokenCollectionDeleted { period });
			Ok(())
		}
	}

	#[derive(Debug, PartialEq)]
	#[allow(clippy::enum_variant_names)]
	pub(crate) enum DenominationToAssetAmountError {
		DenominationOutOfBound,
		DenominationTooSmall,
		DenominationTooBig,
		LossyDenominationConversion,
	}

	impl DenominationToAssetAmountError {
		pub(crate) fn into_pallet_error<T: Config>(self) -> pallet::Error<T> {
			match self {
				DenominationToAssetAmountError::DenominationOutOfBound =>
					Error::<T>::DenominationOutOfBound,
				DenominationToAssetAmountError::DenominationTooSmall =>
					Error::<T>::DenominationTooSmall,
				DenominationToAssetAmountError::DenominationTooBig =>
					Error::<T>::DenominationTooBig,
				DenominationToAssetAmountError::LossyDenominationConversion =>
					Error::<T>::LossyDenominationConversion,
			}
		}
		pub(crate) fn into_custom_invalidity(self) -> CustomInvalidity {
			match self {
				DenominationToAssetAmountError::DenominationOutOfBound =>
					CustomInvalidity::DenominationOutOfBound,
				DenominationToAssetAmountError::DenominationTooSmall =>
					CustomInvalidity::DenominationTooSmall,
				DenominationToAssetAmountError::DenominationTooBig =>
					CustomInvalidity::DenominationTooBig,
				DenominationToAssetAmountError::LossyDenominationConversion =>
					CustomInvalidity::LossyDenominationConversion,
			}
		}
	}

	/// An error indicating that a fee denominated in the native currency cannot be paid with an
	/// instance's underlying asset.
	#[derive(Debug, PartialEq, Eq)]
	pub enum FeeConversionError {
		/// No conversion is available for the asset, for instance because no market exists for it
		/// or it cannot provide that much of the native currency.
		Unavailable,
		/// No coinage instance exists for the given [`InstanceId`].
		InstanceNotFound,
	}

	impl FeeConversionError {
		pub(crate) fn into_pallet_error<T: Config>(self) -> pallet::Error<T> {
			match self {
				Self::Unavailable => Error::<T>::CannotConvertAssetToNative,
				Self::InstanceNotFound => Error::<T>::InstanceNotFound,
			}
		}
	}

	impl From<FeeConversionError> for CustomInvalidity {
		fn from(e: FeeConversionError) -> Self {
			match e {
				FeeConversionError::Unavailable => CustomInvalidity::CannotConvertAssetToNative,
				FeeConversionError::InstanceNotFound => CustomInvalidity::InstanceNotFound,
			}
		}
	}

	impl From<FeeConversionError> for TransactionValidityError {
		fn from(e: FeeConversionError) -> Self {
			CustomInvalidity::from(e).into()
		}
	}

	impl<T: Config> Pallet<T> {
		/// Returns the record of the given instance, or [`Error::InstanceNotFound`].
		pub fn instance(instance_id: InstanceId) -> Result<InstanceRecord<T>, Error<T>> {
			Instances::<T>::get(instance_id).ok_or(Error::<T>::InstanceNotFound)
		}

		/// Validation and registry work shared by [`Pallet::create_sufficient_instance`] and
		/// [`Pallet::create_sponsored_instance`]: checks the asset, the unit and the pallet
		/// account's provisioning, creates the recycler collections and writes the registry
		/// entries.
		///
		/// The pallet account is never provisioned here, only checked. The callers differ on where
		/// the provisioning comes from: admin does it by hand before a sufficient creation, while
		/// a sponsored creation takes it from the creator right before calling this.
		///
		/// `creator` is the account the creation deposit is taken from, `None` for an admin
		/// creation, which takes none.
		pub(crate) fn do_create_instance(
			asset_id: FungiblesAssetIdOf<T>,
			asset_unit: FungiblesBalanceOf<T>,
			mode: InstanceMode,
			creator: Option<T::AccountId>,
		) -> Result<InstanceId, DispatchError> {
			ensure!(T::Fungibles::asset_exists(asset_id.clone()), Error::<T>::UnknownAsset);

			// A zero unit would make every coin worth nothing. The loop below cannot catch that,
			// because converting zero succeeds at every denomination.
			ensure!(!asset_unit.is_zero(), Error::<T>::InvalidAssetUnit);

			// Reject a unit that any denomination in the range cannot represent exactly.
			for value in T::MinimumExponent::get()..=T::MaximumExponent::get() {
				ensure!(
					Self::denomination_to_asset_amount(asset_unit, value).is_ok(),
					Error::<T>::InvalidAssetUnit
				);
			}

			let instance_id = NextInstanceId::<T>::get();
			let next_instance_id = instance_id.checked_add(1).ok_or(Error::<T>::InternalError)?;

			// The pallet account must be able to receive and hold any amount of the underlying
			// asset.
			let pallet_account = Self::pallet_account();
			ensure!(
				!T::Fungibles::should_touch(asset_id.clone(), &pallet_account),
				Error::<T>::PalletAccountNotTouched
			);
			// It must hold the asset's minimum balance, to avoid dustings.
			ensure!(
				T::Fungibles::balance(asset_id.clone(), &pallet_account) >=
					T::Fungibles::minimum_balance(asset_id.clone()),
				Error::<T>::PalletAccountBelowMinimumBalance
			);

			let creator = creator
				.map(|acc| {
					let deposit =
						T::InstanceCreationDeposit::new(&acc, Self::instance_creation_footprint())?;
					Ok::<_, DispatchError>((acc, deposit))
				})
				.transpose()?;

			for value in T::MinimumExponent::get()..=T::MaximumExponent::get() {
				RecyclerManager::<T>::create_collection(instance_id, value)?;
			}

			Instances::<T>::insert(
				instance_id,
				InstanceRecord::<T> {
					asset_id: asset_id.clone(),
					asset_unit,
					mode,
					current_load_deposit: None,
					old_load_deposit: None,
					creator,
				},
			);
			AssetToInstance::<T>::insert(&asset_id, instance_id, ());
			NextInstanceId::<T>::put(next_instance_id);

			Self::deposit_event(Event::InstanceCreated { instance_id, asset_id, asset_unit, mode });
			Ok(instance_id)
		}

		/// The permanent storage footprint of one instance, which
		/// [`Config::InstanceCreationDeposit`] prices.
		///
		/// Counts the two registry entries ([`Instances`] and [`AssetToInstance`]) plus one
		/// recycler collection per denomination in `[MinimumExponent, MaximumExponent]`. The
		/// collections live in the members pallet, so their cost is an estimate rather than a
		/// measurement; they also occupy the offchain worker's iteration slot for good.
		pub fn instance_creation_footprint() -> Footprint {
			/// Storage entries one recycler collection permanently occupies in the members
			/// pallet.
			const COLLECTION_ITEMS: u64 = 4;
			/// Bytes one recycler collection permanently occupies in the members pallet.
			const COLLECTION_BYTES: u64 = 300;

			let denominations = u64::from(
				i16::from(T::MaximumExponent::get())
					.saturating_sub(i16::from(T::MinimumExponent::get()))
					.saturating_add(1)
					.max(0) as u16,
			);
			let registry_bytes = (InstanceRecord::<T>::max_encoded_len() as u64)
				.saturating_add(FungiblesAssetIdOf::<T>::max_encoded_len() as u64);

			Footprint {
				count: denominations.saturating_mul(COLLECTION_ITEMS).saturating_add(2),
				size: denominations.saturating_mul(COLLECTION_BYTES).saturating_add(registry_bytes),
			}
		}

		/// The owner location of an instance's recycler collections: this pallet's own
		/// location with the instance id as an inner junction.
		///
		/// Distinct per instance, so the members pallet's per-owner collection bound
		/// (`indiv_pallet_members::Config::MaxCollections`) bounds the collections of one
		/// instance rather than capping how many instances can be created.
		pub fn recycler_collection_owner(instance_id: InstanceId) -> xcm::v5::Location {
			use xcm::v5::Junction::{GeneralIndex, PalletInstance};
			let pallet_index = <Self as frame_support::traits::PalletInfoAccess>::index() as u8;
			xcm::v5::Location::new(
				0,
				[PalletInstance(pallet_index), GeneralIndex(instance_id.into())],
			)
		}

		/// The owner location of the paid unload token collections (one per period): this
		/// pallet's own location.
		///
		/// Shared across periods: the collections are deleted after their period expires, so
		/// only the few live periods count against the members pallet's per-owner bound.
		pub fn paid_token_collection_owner() -> xcm::v5::Location {
			use xcm::v5::Junction::PalletInstance;
			let pallet_index = <Self as frame_support::traits::PalletInfoAccess>::index() as u8;
			xcm::v5::Location::new(0, [PalletInstance(pallet_index)])
		}

		/// Generate a collection identifier for a recycler of a given instance and denomination.
		pub fn recycler_collection_identifier(
			instance_id: InstanceId,
			value: Denomination,
		) -> Identifier {
			let mut id = [0u8; 32];
			id[0..16].copy_from_slice(&RECYCLER_COLLECTION_PREFIX);
			id[16..20].copy_from_slice(&instance_id.to_le_bytes());
			id[20] = value as u8;
			id
		}

		/// Generate a collection identifier for paid tokens of a given period.
		pub fn paid_token_collection_identifier(period: Period) -> Identifier {
			let mut id = [0u8; 32];
			id[0..16].copy_from_slice(&PAID_TOKEN_COLLECTION_PREFIX);
			id[16..20].copy_from_slice(&period.to_le_bytes());
			id
		}

		/// Get the maximum number of free unload tokens a person can use per time period.
		///
		/// The `allowance` is a fixed budget (in native currency) that each person/lite person is
		/// entitled to unload for free per time period. It is configured as:
		/// - People: `UnloadTokenAllowancePerTimePeriodForPeople`
		/// - Lite people: `UnloadTokenAllowancePerTimePeriodForLitePeople`
		///
		/// This function uses the people allowance.
		///
		/// This is calculated as:
		/// `min(allowance / current unload token fee, MaxFreeUnloadTokensPerTimePeriod)`.
		///
		/// This keeps the number of free unload tokens dynamic:
		/// - If the unload token fee rises, each person gets fewer free unloads.
		/// - If the unload token fee falls, each person gets more free unloads.
		///
		/// The result is capped by `MaxFreeUnloadTokensPerTimePeriod` to prevent
		/// excessively many free unloads when the fee becomes very small.
		pub fn free_unload_token_limit_for_people() -> u32 {
			let allowance = T::UnloadTokenAllowancePerTimePeriodForPeople::get();
			Self::compute_free_unload_token_limit(allowance)
		}

		/// Get the maximum number of free unload tokens a lite person can use per time period.
		///
		/// The `allowance` is the same fixed budget concept documented in
		/// `free_unload_token_limit_for_people`.
		///
		/// This function uses the lite-people allowance:
		/// `UnloadTokenAllowancePerTimePeriodForLitePeople`.
		///
		/// This is calculated as:
		/// `min(allowance / current unload token fee, MaxFreeUnloadTokensPerTimePeriod)`.
		///
		/// The same dynamic and capping rules as for people apply.
		pub fn free_unload_token_limit_for_lite_people() -> u32 {
			let allowance = T::UnloadTokenAllowancePerTimePeriodForLitePeople::get();
			Self::compute_free_unload_token_limit(allowance)
		}

		fn compute_free_unload_token_limit(allowance: NativeBalanceOf<T>) -> u32 {
			let fee: u128 = Self::paid_unload_token_fee_in_native().saturated_into();
			let allowance: u128 = allowance.saturated_into();

			let limit = match fee {
				0 => 0,
				_ => allowance / fee,
			};

			let limit = limit.saturated_into::<u32>();
			limit.min(T::MaxFreeUnloadTokensPerTimePeriod::get())
		}

		/// Convert a denomination (exponent) to the corresponding amount of the underlying asset.
		///
		/// Each coin's value represents a power-of-2 exponent relative to its instance's
		/// [`InstanceRecord::asset_unit`]. The returned asset amount is computed as:
		///
		/// - Negative values: `asset_unit >> |value|` (fractional denominations)
		/// - Non-negative values: `asset_unit << value` (whole/multiple denominations)
		///
		/// Example for `asset_unit = 1000`:
		///
		/// | Denomination | Calculation      | Asset amount      |
		/// |-----------|------------------|-------------------|
		/// | -2        | 2^(-2) × 1000    | 250 (= unit / 4)  |
		/// | -1        | 2^(-1) × 1000    | 500 (= unit / 2)  |
		/// | 0         | 2^0 × 1000       | 1000 (= unit)     |
		/// | 1         | 2^1 × 1000       | 2000 (= unit * 2) |
		/// | 2         | 2^2 × 1000       | 4000 (= unit * 4) |
		///
		/// # Errors
		///
		/// - [`DenominationToAssetAmountError::DenominationOutOfBound`] — `value` falls outside
		///   `[MinimumExponent, MaximumExponent]`.
		/// - [`DenominationToAssetAmountError::LossyDenominationConversion`] — right-shifting
		///   `asset_unit` would truncate bits (unit not divisible by `2^|value|`).
		/// - [`DenominationToAssetAmountError::DenominationTooSmall`] /
		///   [`DenominationToAssetAmountError::DenominationTooBig`] — shift exceeds the bit width
		///   of the balance type (unreachable with valid `MinimumExponent` / `MaximumExponent`
		///   configuration).
		pub(crate) fn denomination_to_asset_amount(
			asset_unit: FungiblesBalanceOf<T>,
			value: Denomination,
		) -> Result<FungiblesBalanceOf<T>, DenominationToAssetAmountError> {
			if value < T::MinimumExponent::get() || value > T::MaximumExponent::get() {
				return Err(DenominationToAssetAmountError::DenominationOutOfBound);
			}

			// Note: DenominationTooSmall/DenominationTooBig are unreachable given a valid
			// MinimumExponent/MaximumExponent configuration, since the bounds check
			// above rejects values before shifts can overflow.

			if value < 0 {
				let shift = value.unsigned_abs() as u32;
				let shifted = asset_unit
					.checked_shr(shift)
					.ok_or(DenominationToAssetAmountError::DenominationTooSmall)?;

				// Verify the division was exact: 1000 / 4 (2^2) = 250 is fine,
				// but 1000 / 16 (2^4) = 62.5 truncates to 62 which is lossy.
				if shifted.checked_shl(shift) != Some(asset_unit) {
					return Err(DenominationToAssetAmountError::LossyDenominationConversion);
				}

				Ok(shifted)
			} else {
				let shift = value as u32;
				let shifted = asset_unit
					.checked_shl(shift)
					.ok_or(DenominationToAssetAmountError::DenominationTooBig)?;

				// checked_shl won't catch overflow within the u64 range,
				// e.g. 1000 * 2^55 needs 65 bits but silently truncates.
				// The round-trip verifies nothing was lost.
				if shifted.checked_shr(shift) != Some(asset_unit) {
					return Err(DenominationToAssetAmountError::DenominationTooBig);
				}

				Ok(shifted)
			}
		}

		/// Pay one paid unload token fee out of the wrapped asset held by the pallet account for
		/// `instance_id`.
		///
		/// The fee arrives at [Config::FeeDestination] in the native currency: the asset is
		/// converted, so the amount of asset it costs depends on the market and is not known
		/// before the call. It is bounded twice, and the two bounds mean different things:
		/// `max_fee` is what the caller allowed, and `available` is what is actually being
		/// unloaded and so can back the fee.
		///
		/// Returns the amount of the asset the fee consumed.
		fn charge_unload_token_fee_from_hold(
			instance_id: InstanceId,
			max_fee: FungiblesBalanceOf<T>,
			available: FungiblesBalanceOf<T>,
		) -> Result<FungiblesBalanceOf<T>, DispatchError> {
			let fee_native = Self::paid_unload_token_fee_in_native();
			let asset_id = Self::instance(instance_id)?.asset_id;
			let asset_in =
				Self::quote_asset_for_native_fee(instance_id, fee_native).map_err(|e| {
					log::error!(
						target: LOG_TARGET,
						"coinage: fee conversion became unavailable after validation",
					);
					e.into_pallet_error::<T>()
				})?;
			if asset_in > max_fee {
				log::error!(
					target: LOG_TARGET,
					"coinage: fee conversion moved above `max_fee` after validation",
				);
				return Err(Error::<T>::FeeExceedsMaxFee.into());
			}
			ensure!(asset_in <= available, Error::<T>::InsufficientUnloadForFee);

			// The conversion spends free balance, so release exactly what the quote says it takes
			// and leave the rest of the wrapped asset held.
			//
			// (The instance creation ensured the pallet account has minimum balance for the asset).
			T::Fungibles::release(
				asset_id.clone(),
				&HoldReason::Wrapped.into(),
				&Self::pallet_account(),
				asset_in,
				Precision::Exact,
			)?;
			let spent = Self::charge_asset_and_transfer_native(
				asset_id.clone(),
				&Self::pallet_account(),
				fee_native,
				asset_in,
				&T::FeeDestination::get(),
			)?;

			// Safety operation if some unspent amount.
			let unspent = asset_in.saturating_sub(spent);
			if !unspent.is_zero() {
				log::error!(
					target: LOG_TARGET,
					"coinage: fee conversion spent less than quoted, re-holding the difference",
				);
				T::Fungibles::hold(
					asset_id,
					&HoldReason::Wrapped.into(),
					&Self::pallet_account(),
					unspent,
				)?;
			}

			Ok(spent)
		}

		/// Transfer external asset from pallet account to recipient.
		///
		/// Skips transfer if amount is zero.
		fn transfer_external_asset(
			instance_id: InstanceId,
			to: &T::AccountId,
			amount: FungiblesBalanceOf<T>,
		) -> DispatchResult {
			if amount > Zero::zero() {
				let asset_id = Self::instance(instance_id)?.asset_id;
				T::Fungibles::transfer_on_hold(
					asset_id,
					&HoldReason::Wrapped.into(),
					&Self::pallet_account(),
					to,
					amount,
					Precision::Exact,
					Restriction::Free,
					Fortitude::Polite,
				)?;
			}
			Ok(())
		}

		/// Process unload inputs with their corresponding proofs.
		///
		/// When `skip_first_alias` is true, the first alias of the first input is skipped
		/// (it was pre-validated and marked in the extension for FromOutput fee mode).
		fn process_unload_inputs(
			instance_id: InstanceId,
			inputs: &[UnloadRecyclerInput<T::MaxConsolidation>],
			alias_proofs: &[ProofOf<T>],
			proven_msg: &[u8; 32],
			skip_first_alias: bool,
		) -> DispatchResult {
			// Proofs are laid out contiguously in the same order as `inputs`, so each input's
			// proofs are a subslice of `alias_proofs` tracked by a running offset. This avoids
			// cloning proofs into a per-input `Vec`.
			let mut offset = 0usize;

			for (idx, input) in inputs.iter().enumerate() {
				let count = input.aliases.len();
				let current_proofs = alias_proofs
					.get(offset..offset + count)
					.ok_or(Error::<T>::ProofAndAliasMismatch)?;
				offset += count;

				let skip_first = skip_first_alias && idx == 0;
				if skip_first {
					// First input with premarked first alias: skip first, validate rest
					if input.aliases.len() > 1 {
						RecyclerManager::<T>::unload(
							instance_id,
							input.value,
							input.index,
							input.revision,
							&input.aliases[1..],
							&current_proofs[1..],
							proven_msg,
						)?;
					}
				} else {
					// Normal case: validate all aliases
					RecyclerManager::<T>::unload(
						instance_id,
						input.value,
						input.index,
						input.revision,
						&input.aliases,
						current_proofs,
						proven_msg,
					)?;
				}
			}

			// Ensure all proofs in the origin were consumed
			ensure!(offset == alias_proofs.len(), Error::<T>::ProofAndAliasMismatch);

			Ok(())
		}

		/// Validate the FromOutput fee invariants and process unload inputs.
		///
		/// For [UnloadFee::FromOutput], verifies that the first input matches the fee recycler
		/// validated in the extension and that its first alias was pre-marked, then processes
		/// the remaining aliases. For [UnloadFee::Prepaid], processes all aliases directly.
		fn process_unload_inputs_with_fee(
			instance_id: InstanceId,
			inputs: &[UnloadRecyclerInput<T::MaxConsolidation>],
			alias_proofs: &[ProofOf<T>],
			proven_msg: &[u8; 32],
			fee: UnloadFee,
		) -> DispatchResult {
			match fee {
				UnloadFee::Prepaid => Self::process_unload_inputs(
					instance_id,
					inputs,
					alias_proofs,
					proven_msg,
					false,
				),
				UnloadFee::FromOutput { fee_recycler_value, fee_recycler_index } => {
					let first_input = inputs.first().ok_or(Error::<T>::EmptyInputs)?;
					ensure!(
						first_input.value == fee_recycler_value &&
							first_input.index == fee_recycler_index,
						Error::<T>::RecyclerMismatch
					);
					// Note: Double-spend protection for FromOutput mode is in the extension's
					// validate_alias_proof(), which rejects already-unloaded aliases. This check
					// only verifies the extension did its job.
					let first_alias = first_input.aliases.first().ok_or(Error::<T>::EmptyInputs)?;
					ensure!(
						RecyclerAliasStates::<T>::get((
							instance_id,
							fee_recycler_value,
							fee_recycler_index,
							first_alias,
						)) == Some(AliasState::Unloaded),
						Error::<T>::AliasNotPremarked
					);
					Self::process_unload_inputs(instance_id, inputs, alias_proofs, proven_msg, true)
				},
			}
		}

		/// Charge `count` unload fees to `signer`, for [FeeCurrency::Native] the amount in native
		/// is charged, for [FeeCurrency::ExternalAsset] the amount in the instance's underlying
		/// asset is converted into native and charged.
		///
		/// `max_fee` bounds what the fee may cost the signer, in the currency they chose to pay it
		/// in. Neither price is fixed: the native fee follows [Config::WeightToFee], which a
		/// runtime can make depend on chain state, and the asset fee follows the market on top of
		/// that.
		fn charge_fees_from_signer(
			signer: &T::AccountId,
			instance_id: InstanceId,
			fee_currency: FeeCurrency,
			count: u32,
			max_fee: FungiblesBalanceOf<T>,
		) -> DispatchResult {
			let native = Self::paid_unload_token_fees_in_native(count);
			match fee_currency {
				FeeCurrency::Native => {
					ensure!(native <= max_fee, Error::<T>::FeeExceedsMaxFee);
					T::NativeFungible::transfer(
						signer,
						&T::FeeDestination::get(),
						native,
						Preservation::Protect,
					)?;
				},
				FeeCurrency::ExternalAsset => {
					let asset_in = Self::quote_asset_for_native_fee(instance_id, native)
						.map_err(|e| e.into_pallet_error::<T>())?;
					ensure!(asset_in <= max_fee, Error::<T>::FeeExceedsMaxFee);
					let asset_id = Self::instance(instance_id)?.asset_id;
					Self::charge_asset_and_transfer_native(
						asset_id,
						signer,
						native,
						asset_in,
						&T::FeeDestination::get(),
					)?;
				},
			}
			Ok(())
		}

		/// Convert a denomination to its number of base units relative to the minimum exponent.
		///
		/// One base unit is 2^minimum_exponent.
		///
		/// The denomination must have been checked to be within bounds prior to calling this
		/// function. But in case it is not then none is returned.
		///
		/// Note that an integrity test ensures the maximum denomination can be represented in u32
		/// base units.
		pub(crate) fn denomination_to_base_units(value: Denomination) -> Option<u32> {
			let min_exp = T::MinimumExponent::get();
			let exponent_offset: u32 =
				(i32::from(value)).checked_sub(i32::from(min_exp))?.try_into().ok()?;

			1u32.checked_shl(exponent_offset)
		}

		/// Validate split parameters against an expected total unit value.
		///
		/// Checks:
		/// * Destinations must not already have a coin.
		/// * Destinations within one value group must not be empty.
		/// * Value bounds.
		/// * Sorted order of values (strictly ascending).
		/// * Sum of outputs equals `expected_total_units`.
		/// * Max output count.
		pub(crate) fn validate_split_params(
			expected_total_units: u32,
			split_into: &[(Denomination, BoundedVec<T::AccountId, T::MaxSplitOutputs>)],
		) -> Result<(), CustomInvalidity> {
			use CustomInvalidity::*;

			let max_split_outputs = T::MaxSplitOutputs::get();
			let minimum_exponent = T::MinimumExponent::get();
			let maximum_exponent = T::MaximumExponent::get();

			let mut previous_denomination: Option<Denomination> = None;
			let mut split_output_count: u32 = 0;
			// This is the total value expressed as a number of the minimum denomination.
			// The integrity test ensures that the maximum denomination can be represented in u32.
			let mut total_unit: u32 = 0;

			for (value, dest) in split_into {
				ensure!(!dest.is_empty(), EmptySplit);
				ensure!(*value >= minimum_exponent, SplitExponentTooSmall);
				ensure!(*value <= maximum_exponent, SplitExponentTooBig);
				ensure!(
					// ensure split_into is sorted by value strictly ascending.
					previous_denomination
						.is_none_or(|previous_denomination| previous_denomination < *value),
					SplitIntoNotSorted,
				);
				previous_denomination = Some(*value);

				// Check destination count BEFORE performing storage reads to bound validation work.
				let split_output_count_for_value =
					u32::try_from(dest.len()).map_err(|_| TooManySplits)?;
				split_output_count = split_output_count
					.checked_add(split_output_count_for_value)
					.ok_or(TooManySplits)?;
				ensure!(split_output_count <= max_split_outputs, TooManySplits);

				ensure!(
					dest.iter().all(|d| !CoinsByOwner::<T>::contains_key(d)),
					AddressAlreadyHasCoin
				);

				let value_unit = Self::denomination_to_base_units(*value).ok_or(InternalError)?;

				let additional_unit_for_value =
					split_output_count_for_value.checked_mul(value_unit).ok_or(InvalidSplit)?;

				total_unit =
					total_unit.checked_add(additional_unit_for_value).ok_or(InvalidSplit)?;
			}

			ensure!(split_output_count <= max_split_outputs, TooManySplits);
			ensure!(total_unit == expected_total_units, InvalidSplit);

			let mut all_dests = split_into.iter().flat_map(|(_, dest)| dest).collect::<Vec<_>>();
			all_dests.sort();
			let duplicates = all_dests.windows(2).any(|w| w[0] == w[1]);
			ensure!(!duplicates, DuplicateDestinationsInSplit);

			Ok(())
		}

		/// Validate a coin split operation.
		pub(crate) fn validate_split(
			coin: &Coin,
			split_into: &[(Denomination, BoundedVec<T::AccountId, T::MaxSplitOutputs>)],
		) -> Result<(), CustomInvalidity> {
			if coin.age >= T::MaximumAge::get() {
				return Err(CustomInvalidity::CoinTooOld);
			}

			let expected_total = Self::denomination_to_base_units(coin.value)
				.ok_or(CustomInvalidity::InternalError)?;

			Self::validate_split_params(expected_total, split_into)
		}

		pub(crate) fn validate_mixed_output_outputs(
			asset_unit: FungiblesBalanceOf<T>,
			value: Denomination,
			alias_count: u32,
			external_asset_amount: FungiblesBalanceOf<T>,
			loaded_coins: &[(Denomination, MemberOf<T>)],
		) -> Result<(), MixedOutputValidationError> {
			if alias_count == 0 {
				return Err(MixedOutputValidationError::EmptyAliases);
			}
			if loaded_coins.is_empty() {
				return Err(MixedOutputValidationError::EmptyLoadedCoins);
			}

			let amount_per_input = Self::denomination_to_asset_amount(asset_unit, value)
				.map_err(MixedOutputValidationError::Denomination)?;
			let alias_count: FungiblesBalanceOf<T> = alias_count.into();
			let total_input_amount = amount_per_input
				.checked_mul(&alias_count)
				.ok_or(MixedOutputValidationError::InvalidSplit)?;

			let mut loaded_coin_amount: FungiblesBalanceOf<T> = Zero::zero();
			let mut seen = BTreeSet::new();

			for (loaded_coin_value, member_key) in loaded_coins {
				let amount = Self::denomination_to_asset_amount(asset_unit, *loaded_coin_value)
					.map_err(MixedOutputValidationError::Denomination)?;

				let encoded_key = member_key.encode();
				if !seen.insert(encoded_key) || RecyclerManager::<T>::is_member_key_used(member_key)
				{
					return Err(MixedOutputValidationError::MemberKeyAlreadyUsed);
				}

				if !CryptoOf::<T>::is_member_valid(member_key) {
					return Err(MixedOutputValidationError::InvalidMemberKey);
				}

				loaded_coin_amount = loaded_coin_amount
					.checked_add(&amount)
					.ok_or(MixedOutputValidationError::InvalidSplit)?;
			}

			let total_expected_amount = external_asset_amount
				.checked_add(&loaded_coin_amount)
				.ok_or(MixedOutputValidationError::InvalidSplit)?;

			if total_expected_amount != total_input_amount {
				return Err(MixedOutputValidationError::InvalidSplit);
			}

			Ok(())
		}

		/// Validate a coin transfer operation.
		pub(crate) fn validate_transfer(
			coin: &Coin,
			to: &T::AccountId,
		) -> Result<(), CustomInvalidity> {
			if coin.age >= T::MaximumAge::get() {
				return Err(CustomInvalidity::CoinTooOld);
			}
			if CoinsByOwner::<T>::contains_key(to) {
				return Err(CustomInvalidity::AddressAlreadyHasCoin);
			}
			Ok(())
		}

		/// Validate direct offboarding from coin origin into external asset.
		pub(crate) fn validate_direct_offboard_coin_into_external_asset(
			coin: &Coin,
			_to: &T::AccountId,
		) -> Result<(), CustomInvalidity> {
			let asset_unit = Instances::<T>::get(coin.instance_id)
				.ok_or(CustomInvalidity::InstanceNotFound)?
				.asset_unit;
			Self::denomination_to_asset_amount(asset_unit, coin.value)
				.map_err(|e| e.into_custom_invalidity())?;

			Ok(())
		}

		/// Validate loading a recycler with a coin.
		pub(crate) fn validate_load_recycler_with_coin(
			instance_id: InstanceId,
			coin_id: &T::AccountId,
			member_key: &MemberOf<T>,
			proof_of_ownership: &SignatureOf<T>,
		) -> Result<(), CustomInvalidity> {
			if RecyclerManager::<T>::is_member_key_used(member_key) {
				return Err(CustomInvalidity::MemberKeyAlreadyUsed);
			}

			if !CryptoOf::<T>::is_member_valid(member_key) {
				return Err(CustomInvalidity::InvalidMemberKey);
			}

			if !CryptoOf::<T>::verify_signature(
				proof_of_ownership,
				&coin_id.encode()[..],
				member_key,
			) {
				return Err(CustomInvalidity::InvalidProofOfOwnership);
			}

			Self::ensure_can_charge_load_deposit(instance_id, 1)?;

			Ok(())
		}

		/// Validate loading a recycler with an external asset (infallible path).
		///
		/// Performs the following checks:
		/// - The `member_key` is not already used in another recycler.
		/// - The `member_key` is valid (well-formed).
		/// - The `proof_of_ownership` is a valid signature of `who`'s account id by the
		///   `member_key`.
		/// - The `instance_id` refers to an existing instance.
		/// - The `value` can be losslessly converted to an asset amount (implying it is within the
		///   bounds defined by [Config::MinimumExponent] and [Config::MaximumExponent]).
		/// - `who` has enough reducible balance of the underlying asset to cover the equivalent
		///   amount for the given denomination (respecting `preservation`).
		pub(crate) fn validate_load_recycler_with_external_asset_unpaid(
			who: &T::AccountId,
			instance_id: InstanceId,
			preservation: CodecPreservation,
			value: Denomination,
			member_key: &MemberOf<T>,
			proof_of_ownership: &SignatureOf<T>,
		) -> Result<(), CustomInvalidity> {
			let load_cost = Self::validate_unpaid_load_item(
				who,
				instance_id,
				value,
				member_key,
				proof_of_ownership,
			)?;
			Self::check_unpaid_load_balance(who, instance_id, preservation, load_cost)?;

			Self::ensure_can_charge_load_deposit(instance_id, 1)
		}

		/// Per-item checks for an `InfallibleUnpaidSigned` load. Returns the asset amount required
		/// for this single item so callers can either check it directly or aggregate it.
		fn validate_unpaid_load_item(
			who: &T::AccountId,
			instance_id: InstanceId,
			value: Denomination,
			member_key: &MemberOf<T>,
			proof_of_ownership: &SignatureOf<T>,
		) -> Result<FungiblesBalanceOf<T>, CustomInvalidity> {
			if RecyclerManager::<T>::is_member_key_used(member_key) {
				return Err(CustomInvalidity::MemberKeyAlreadyUsed);
			}

			if !CryptoOf::<T>::is_member_valid(member_key) {
				return Err(CustomInvalidity::InvalidMemberKey);
			}

			if !CryptoOf::<T>::verify_signature(proof_of_ownership, &who.encode()[..], member_key) {
				return Err(CustomInvalidity::InvalidProofOfOwnership);
			}

			let asset_unit = Instances::<T>::get(instance_id)
				.ok_or(CustomInvalidity::InstanceNotFound)?
				.asset_unit;
			let load_cost = Self::denomination_to_asset_amount(asset_unit, value)
				.map_err(|e| e.into_custom_invalidity())?;

			Ok(load_cost)
		}

		fn check_unpaid_load_balance(
			who: &T::AccountId,
			instance_id: InstanceId,
			preservation: CodecPreservation,
			required: FungiblesBalanceOf<T>,
		) -> Result<(), CustomInvalidity> {
			let asset_id = Instances::<T>::get(instance_id)
				.defensive_proof("coinage: instance checked in validate_unpaid_load_item")
				.ok_or(CustomInvalidity::InstanceNotFound)?
				.asset_id;
			let balance = <T::Fungibles as Inspect<T::AccountId>>::reducible_balance(
				asset_id,
				who,
				preservation.into(),
				Fortitude::Polite,
			);
			if balance < required {
				return Err(CustomInvalidity::InfallibleUnpaidSignedInsufficientBalance);
			}
			Ok(())
		}

		/// Validate a batched `load_recycler_with_external_asset_unpaid` call.
		///
		/// Validates each inner item, checks that no two items share a member key, and that the
		/// signer's reducible balance covers the sum of all per-item asset amounts (using the
		/// strictest preservation mode of any item, which is sufficient for the worst case
		/// regardless of dispatch order).
		///
		/// Returns the list of member keys (in batch order) so the caller can add a
		/// transaction-pool `provides` tag per key.
		pub(crate) fn validate_load_recycler_with_external_asset_unpaid_batch<'a>(
			who: &T::AccountId,
			instance_id: InstanceId,
			items: &'a [UnpaidLoadInput<T>],
		) -> Result<Vec<&'a MemberOf<T>>, CustomInvalidity> {
			if items.is_empty() {
				return Err(CustomInvalidity::EmptyUnpaidLoadBatch);
			}

			let mut seen = BTreeSet::new();
			let mut total = FungiblesBalanceOf::<T>::zero();
			let mut strictest = CodecPreservation::Expendable;
			let mut member_keys = Vec::with_capacity(items.len());

			for item in items {
				if !seen.insert(item.member_key.encode()) {
					return Err(CustomInvalidity::MemberKeyAlreadyUsed);
				}

				let load_cost = Self::validate_unpaid_load_item(
					who,
					instance_id,
					item.value,
					&item.member_key,
					&item.proof_of_ownership,
				)?;

				total = total
					.checked_add(&load_cost)
					.ok_or(CustomInvalidity::InfallibleUnpaidSignedInsufficientBalance)?;

				strictest = strictest.strictest(item.preservation);

				member_keys.push(&item.member_key);
			}

			Self::check_unpaid_load_balance(who, instance_id, strictest, total)?;

			Self::ensure_can_charge_load_deposit(instance_id, items.len() as u32)?;

			Ok(member_keys)
		}

		/// Validate paying for recycler unload fee token with a coin.
		pub(crate) fn validate_pay_for_recycler_unload_fee_token_with_coin(
			coin: &Coin,
			coin_id: &T::AccountId,
			member_key: &MemberOf<T>,
			proof_of_ownership: &SignatureOf<T>,
		) -> Result<(), CustomInvalidity> {
			if coin.age >= T::MaximumAge::get() {
				return Err(CustomInvalidity::CoinTooOld);
			}

			let asset_unit = Instances::<T>::get(coin.instance_id)
				.ok_or(CustomInvalidity::InstanceNotFound)?
				.asset_unit;
			let amount = Self::denomination_to_asset_amount(asset_unit, coin.value)
				.map_err(|e| e.into_custom_invalidity())?;

			let fee = Self::quote_paid_unload_token_fee_in_asset(coin.instance_id)?;

			if amount < fee {
				return Err(CustomInvalidity::CoinAmountBelowFee);
			}

			if PaidUnloadTokenMembers::<T>::contains_key(member_key) {
				return Err(CustomInvalidity::MemberKeyAlreadyUsed);
			}

			if !CryptoOf::<T>::is_member_valid(member_key) {
				return Err(CustomInvalidity::InvalidMemberKey);
			}

			if !CryptoOf::<T>::verify_signature(
				proof_of_ownership,
				&coin_id.encode()[..],
				member_key,
			) {
				return Err(CustomInvalidity::InvalidProofOfOwnership);
			}

			Ok(())
		}

		/// Period are valid up to 1 hour in the future to let transactions propagate.
		pub(crate) fn current_free_unload_token_periods() -> [Period; 2] {
			let now_secs = T::UnixTime::now().as_secs() as u32;
			let free_token_period = now_secs
				.checked_div(T::UnloadTokenTimePeriodPeopleLitePeople::get())
				.unwrap_or(0);

			let now_secs_minus_hour = (T::UnixTime::now().as_secs() as u32)
				.saturating_sub(FREE_UNLOAD_TOKEN_GRACE_WINDOW);
			let free_token_period_minus_hour = now_secs_minus_hour
				.checked_div(T::UnloadTokenTimePeriodPeopleLitePeople::get())
				.unwrap_or(0);

			[free_token_period_minus_hour, free_token_period]
		}

		/// Submit an authorized transaction from the offchain worker and log the result.
		pub(crate) fn submit_authorized_transaction(call: Call<T>, description: &str) {
			let tx = T::create_authorized_transaction(call.into());
			match SubmitTransaction::<T, _>::submit_transaction(tx) {
				Ok(()) => log::debug!(
					target: LOG_TARGET,
					"offchain worker: submitted authorized transaction successfully for `{description}`",
				),
				Err(()) => log::warn!(
					target: LOG_TARGET,
					"offchain worker: failed to submit authorized transaction for `{description}`",
				),
			}
		}

		/// Validate that a call is a valid unload recycler call.
		///
		/// This performs the following validations:
		/// 1. Call variant: Verifies the call is one of the allowed unload calls.
		///    - [UnloadFee::Prepaid]: accepts `unload_recycler_into_coin`,
		///      `unload_recycler_into_coins`, `unload_recycler_into_external_asset`, and
		///      `unload_recycler_into_external_asset_and_loaded_coins`.
		///    - [UnloadFee::FromOutput]: only accepts `unload_recycler_into_coins`,
		///      `unload_recycler_into_external_asset`, and
		///      `unload_recycler_into_external_asset_and_loaded_coins`.
		/// 2. Recycler revision: Validates the recycler revision to prevent stale extrinsics from
		///    being included and consuming the unload fee token.
		/// 3. Destination ownership (for coin outputs): Validates that destination addresses don't
		///    already have a coin to prevent erasing another person's coin. (A malicious user could
		///    send to one of the destination just before this extrinsic, making this extrinsic fail
		///    and the sender pay despite not being their fault.)
		/// 4. Fee availability ([UnloadFee::FromOutput] only): Validates that the paid unload token
		///    fee in asset is available.
		/// 5. Max fee coverage ([UnloadFee::FromOutput], `unload_recycler_into_coins` only):
		///    Validates that `max_fee` covers the required network fee.
		/// 6. Output coverage ([UnloadFee::FromOutput] only): validates that the part of the output
		///    the fee is taken from covers the required network fee.
		/// 7. Empty max fee ([UnloadFee::Prepaid], `unload_recycler_into_coins` only): validates
		///    that `max_fee` is zero, since the split takes the whole unloaded value.
		///
		/// Returns the instance the call unloads from.
		pub(crate) fn validate_unload_calls(
			call: &<T as frame_system::Config>::RuntimeCall,
			fee: UnloadFee,
		) -> Result<InstanceId, CustomInvalidity> {
			let (
				instance_id,
				value,
				index,
				revision,
				coin_destination,
				split_into,
				max_fee,
				mixed_output,
				asset_output_alias_count,
			) = match call.is_sub_type() {
				Some(Call::<T>::unload_recycler_into_coin {
					revision,
					index,
					instance_id,
					value,
					to,
					..
				}) => {
					match fee {
						UnloadFee::FromOutput { .. } =>
							return Err(CustomInvalidity::FromOutputFeeNotAllowed),
						UnloadFee::Prepaid => {},
					}
					(*instance_id, *value, *index, *revision, Some(to), None, None, None, None)
				},
				Some(Call::<T>::unload_recycler_into_coins {
					revision,
					index,
					instance_id,
					value,
					max_fee,
					split_into,
					..
				}) => {
					// For this call for Prepaid, `max_fee` must be zero.
					match fee {
						UnloadFee::Prepaid =>
							ensure!(max_fee.is_zero(), CustomInvalidity::MaxFeeNotAllowedForPrepaid),
						UnloadFee::FromOutput { .. } => {},
					}
					(
						*instance_id,
						*value,
						*index,
						*revision,
						None,
						Some(split_into.as_slice()),
						Some(*max_fee),
						None,
						None,
					)
				},
				Some(Call::<T>::unload_recycler_into_external_asset {
					revision,
					index,
					instance_id,
					value,
					aliases,
					max_fee,
					..
				}) => (
					*instance_id,
					*value,
					*index,
					*revision,
					None,
					None,
					Some(*max_fee),
					None,
					Some(aliases.len() as u32),
				),
				Some(Call::<T>::unload_recycler_into_external_asset_and_loaded_coins {
					revision,
					index,
					instance_id,
					value,
					aliases,
					external_asset_amount,
					loaded_coins,
					max_fee,
					..
				}) => (
					*instance_id,
					*value,
					*index,
					*revision,
					None,
					None,
					Some(*max_fee),
					Some((aliases.len() as u32, *external_asset_amount, loaded_coins.as_slice())),
					None,
				),
				_ => return Err(CustomInvalidity::InvalidCall),
			};

			if !RecyclerManager::<T>::validate_recycler_revision(
				instance_id,
				value,
				index,
				revision,
			) {
				return Err(CustomInvalidity::InvalidRecyclerRevision);
			}

			if let Some(to) = coin_destination {
				if CoinsByOwner::<T>::contains_key(to) {
					return Err(CustomInvalidity::AddressAlreadyHasCoin);
				}
			}

			if let Some(split_into) = split_into {
				let max_split_outputs = T::MaxSplitOutputs::get();
				let mut split_output_count: u32 = 0;
				for (_value, dests) in split_into {
					// Reject empty destinations early to prevent CPU DoS via many empty arrays.
					ensure!(!dests.is_empty(), CustomInvalidity::EmptySplit);
					let output_count_for_value =
						u32::try_from(dests.len()).map_err(|_| CustomInvalidity::TooManySplits)?;
					split_output_count = split_output_count
						.checked_add(output_count_for_value)
						.ok_or(CustomInvalidity::TooManySplits)?;
					ensure!(
						split_output_count <= max_split_outputs,
						CustomInvalidity::TooManySplits
					);

					for dest in dests {
						if CoinsByOwner::<T>::contains_key(dest) {
							return Err(CustomInvalidity::AddressAlreadyHasCoin);
						}
					}
				}
			}

			if let Some((alias_count, external_asset_amount, loaded_coins)) = mixed_output.as_ref()
			{
				let asset_unit = Instances::<T>::get(instance_id)
					.ok_or(CustomInvalidity::InstanceNotFound)?
					.asset_unit;
				// Validate mixed-output structure during extension validation so invalid calls are
				// rejected before free tokens are consumed or FromOutput aliases are premarked.
				Self::validate_mixed_output_outputs(
					asset_unit,
					value,
					*alias_count,
					*external_asset_amount,
					loaded_coins,
				)
				.map_err(MixedOutputValidationError::into_custom_invalidity)?;

				// The new loaded coins deposit charging must be checked.
				// The unloaded coins may be fewer or may not even be backed by the pot (e.g. if
				// they were loaded while the instance was sufficient).
				Self::ensure_can_charge_load_deposit(instance_id, loaded_coins.len() as u32)?;
			}

			match fee {
				// The fee is already paid, so `max_fee` bounds nothing and is ignored.
				UnloadFee::Prepaid => {},
				UnloadFee::FromOutput { .. } => {
					let required_unload_fee =
						Self::quote_paid_unload_token_fee_in_asset(instance_id)?;
					if let Some((_, external_asset_amount, _)) = mixed_output.as_ref() {
						if *external_asset_amount < required_unload_fee {
							return Err(CustomInvalidity::UnloadedValueBelowFee);
						}
					}
					// The whole output of `unload_recycler_into_external_asset` is the asset, so
					// that is what its fee draws on. A quote above it is rejected here, the way
					// the mixed-output call's own portion is above: the dispatch would fail with
					// `Error::InsufficientUnloadForFee`.
					if let Some(alias_count) = asset_output_alias_count {
						let asset_unit = Instances::<T>::get(instance_id)
							.ok_or(CustomInvalidity::InstanceNotFound)?
							.asset_unit;
						let unloaded = Self::denomination_to_asset_amount(asset_unit, value)
							.map_err(|e| e.into_custom_invalidity())?
							.saturating_mul(alias_count.into());
						if unloaded < required_unload_fee {
							return Err(CustomInvalidity::UnloadedValueBelowFee);
						}
					}
					// Every call accepting `FromOutput` bounds the fee with `max_fee`.
					let max_fee = max_fee.ok_or(CustomInvalidity::InvalidCall)?;
					if max_fee < required_unload_fee {
						return Err(CustomInvalidity::MaxFeeInsufficientForUnload);
					}
				},
			}

			Ok(instance_id)
		}

		/// Extracts the first alias from a FromOutput-eligible unload call.
		///
		/// This is used by the
		/// [`AsUnloadTokenFromOutput`](crate::extension::AsCoinageInfo::AsUnloadTokenFromOutput)
		/// extension to verify that the first alias in the call matches the one derived from the
		/// first proof.
		pub(crate) fn first_alias_in_from_output_unload_call(
			call: &<T as frame_system::Config>::RuntimeCall,
		) -> Option<Alias> {
			match call.is_sub_type() {
				Some(Call::<T>::unload_recycler_into_coins { aliases, .. }) |
				Some(Call::<T>::unload_recycler_into_external_asset { aliases, .. }) |
				Some(Call::<T>::unload_recycler_into_external_asset_and_loaded_coins {
					aliases,
					..
				}) => aliases.first().copied(),
				_ => None,
			}
		}

		/// Validates that the signer's chosen fee currency can pay `count` unload token fees
		/// without exceeding `max_fee`.
		///
		/// Note that this is best-effort: a signed call's own transaction withdraw transaction fee,
		/// and may be wrapped in a batch etc...
		fn validate_asset_fee_payment(
			instance_id: InstanceId,
			fee_currency: FeeCurrency,
			count: u32,
			max_fee: FungiblesBalanceOf<T>,
		) -> Result<(), TransactionValidityError> {
			let required = match fee_currency {
				FeeCurrency::Native => Self::paid_unload_token_fees_in_native(count),
				FeeCurrency::ExternalAsset =>
					Self::quote_paid_unload_token_fees_in_asset(instance_id, count)?,
			};
			ensure!(required <= max_fee, CustomInvalidity::MaxFeeInsufficientForUnload);
			Ok(())
		}

		/// Validates state-dependent arguments of signed unload calls if the call is one of them.
		/// This is called from the extension to fail early (before fee payment) when the
		/// transaction was built against outdated chain state: recycler revisions for the
		/// non-anonymous unload calls and the archive commitment roots for archived recycler
		/// unloads.
		///
		/// For any other call, this is a no-op (returns Ok).
		pub(crate) fn validate_signed_unload_calls(
			call: &<T as frame_system::Config>::RuntimeCall,
		) -> Result<(), TransactionValidityError> {
			// Only validate for signed unload calls
			match call.is_sub_type() {
				Some(Call::<T>::unload_recycler_into_external_asset_non_anonymous {
					instance_id,
					input,
					fee_currency,
					max_fee,
					..
				}) => {
					if !RecyclerManager::<T>::validate_recycler_revision(
						*instance_id,
						input.value,
						input.index,
						input.revision,
					) {
						return Err(CustomInvalidity::InvalidRecyclerRevision.into());
					}

					Self::validate_asset_fee_payment(*instance_id, *fee_currency, 1, *max_fee)?;
				},
				Some(Call::<T>::unload_recyclers_into_external_asset_non_anonymous {
					instance_id,
					inputs,
					fee_currency,
					max_fee,
					..
				}) => {
					// (only for more accurate error reporting).
					if inputs.is_empty() {
						return Err(CustomInvalidity::EmptyInputs.into());
					}

					for input in inputs {
						if !RecyclerManager::<T>::validate_recycler_revision(
							*instance_id,
							input.value,
							input.index,
							input.revision,
						) {
							return Err(CustomInvalidity::InvalidRecyclerRevision.into());
						}
					}

					Self::validate_asset_fee_payment(
						*instance_id,
						*fee_currency,
						inputs.len() as u32,
						*max_fee,
					)?;
				},
				Some(Call::<T>::unload_archived_recycler_into_external_asset {
					instance_id,
					value,
					index,
					recycler_root,
					unloaded_root,
					fee_currency,
					max_fee,
					..
				}) => {
					// Each unload from an archive updates its commitment, so a transaction whose
					// roots do not match the stored commitment was built against a superseded
					// archive state (a competing unload was included first) and can never become
					// valid again: it is stale. A drained archive is removed from storage, so a
					// missing entry also means the transaction is stale.
					let Some(archive) = RecyclersArchives::<T>::get((*instance_id, *value, *index))
					else {
						return Err(InvalidTransaction::Stale.into());
					};
					if archive_commitment(*unloaded_root, recycler_root) != archive.commitment {
						return Err(InvalidTransaction::Stale.into());
					}

					Self::validate_asset_fee_payment(*instance_id, *fee_currency, 1, *max_fee)?;
				},
				Some(Call::<T>::load_recycler_with_external_asset { instance_id, .. }) => {
					// Advisory, unlike the unpaid variants whose dispatch relies on this check:
					// here dispatch would fail and the signer would pay for that failure, which is
					// no spam vector. Rejecting saves that fee and gives wallets a clean answer.
					Self::ensure_can_charge_load_deposit(*instance_id, 1)?;
				},
				// For any other call, do nothing
				_ => (),
			}

			Ok(())
		}

		/// Computes the average weight of a coin's full lifecycle.
		///
		/// Used to derive the upfront unload token fee via
		/// [`Self::paid_unload_token_fee_in_native`]. Because the fee is determined before the
		/// user chooses a specific payment method or unload path, each phase takes the worst-case
		/// weight across all method variants (e.g. pay-with-coin vs pay-with-native). Variable
		/// counts like the number of consolidation inputs or transfers are estimated at half the
		/// configured maximum, since charging for the full maximum would assume every coin is
		/// consolidated at capacity and transferred until exhaustion, which is unrealistic.
		/// If this underestimate the actual average weight, it can be adjusted, the worst case is
		/// tested to not be order of magnitude higher than this estimate.
		///
		/// The average-based counts knowingly undercharge heavy usage. For a sufficient instance
		/// the stranded-value economics finance that average-vs-worst-case gap; a sponsored
		/// instance has no stranded value, so if the undercharge ever matters the fee for
		/// sponsored instances can be switched to worst-case counts.
		pub(crate) fn coin_lifecycle_weight() -> Weight {
			// Maximum number of aliases (coins) that can be consolidated in one operation.
			let max_aliases = T::MaxConsolidation::get().max(1);

			// Single-ring operations (unload_recycler_into_{coin,coins}) are
			// bounded by the ring's capacity since all aliases must fit in one ring.
			let max_ring_capacity = T::RecyclerRingExponent::get().ring_capacity();
			let max_aliases_single_ring = max_aliases.min(max_ring_capacity);

			// Average case: assume users consolidate roughly half the maximum.
			let avg_aliases_single_ring = max_aliases_single_ring.saturating_div(2);

			// Split outputs: how many coins a split can produce.
			let max_split_outputs = T::MaxSplitOutputs::get();
			let avg_split_outputs = max_split_outputs.saturating_div(2);

			// Coin age: affects how many transactions a coin participates in.
			let max_age = u32::from(T::MaximumAge::get());
			let half_age = max_age.saturating_div(2);

			// Per-key background cost from member insertion.
			let bg_per_key = T::MemberService::add_member_background_weight();

			// === Phase 1: Acquiring an unload token by paying for it ===
			// Pay for a recycler unload fee token using any of the 3 methods.
			let pay_fee = T::WeightInfo::pay_for_recycler_unload_fee_token_with_coin()
				.max(T::WeightInfo::pay_for_recycler_unload_fee_token_with_native())
				.max(T::WeightInfo::pay_for_recycler_unload_fee_token_with_external_asset());

			// Background operation: inserting key into paid unload token ring.
			let bg_paid_token = bg_per_key;

			// === Phase 2: Loading ===
			// Load coins into recycler. On a sponsored instance each load call also charges
			// the load deposit; the fee is universal across instances, so every coin is priced
			// for it.
			let load_one = T::WeightInfo::load_recycler_with_coin()
				.max(T::WeightInfo::load_recycler_with_external_asset())
				.saturating_add(T::WeightInfo::charge_load_deposit());
			let load_avg = load_one.saturating_mul(avg_aliases_single_ring.into());

			// Background operation: pushing keys into recycler's ring.
			let bg_recycler = bg_per_key.saturating_mul(avg_aliases_single_ring.into());

			// === Phase 3: Unloading ===
			// Unload recycler with average number of items.
			let unload_avg = Self::unload_recycler_into_coin_weight(
				avg_aliases_single_ring as usize,
			)
			.max(Self::unload_recycler_into_external_asset_and_loaded_coins_prepaid_weight(
				avg_aliases_single_ring as usize,
				avg_split_outputs as usize,
			))
			.max(Self::unload_recycler_into_external_asset_and_loaded_coins_from_output_weight(
				avg_aliases_single_ring as usize,
				avg_split_outputs as usize,
			))
			.max(Self::unload_recycler_into_external_asset_prepaid_weight(
				avg_aliases_single_ring as usize,
			))
			.max(Self::unload_recycler_into_external_asset_from_output_weight(
				avg_aliases_single_ring as usize,
			))
			.max(Self::unload_recycler_into_external_asset_non_anonymous_weight(
				avg_aliases_single_ring as usize,
			))
			.max(Self::unload_recycler_into_coins_from_output_weight(
				avg_aliases_single_ring as usize,
				avg_split_outputs,
			))
			.max(Self::unload_recycler_into_coins_prepaid_weight(
				avg_aliases_single_ring as usize,
				avg_split_outputs,
			))
			// On a sponsored instance the unload also settles the load deposits.
			.saturating_add(T::WeightInfo::settle_load_deposits());

			// === Phase 4: Transfers and Splits ===
			// Post-unload usage: a coin is transferred or split until it reaches
			// MaximumAge. These operations are not charged individually, so their
			// cost is amortised into the upfront fee.
			let tx_split_max =
				T::WeightInfo::transfer().max(T::WeightInfo::split(avg_split_outputs));
			let tx_split_avg = tx_split_max.saturating_mul(half_age.into());

			// Sum all phases for total average lifecycle weight.
			pay_fee
				.saturating_add(bg_paid_token)
				.saturating_add(load_avg)
				.saturating_add(bg_recycler)
				.saturating_add(unload_avg)
				.saturating_add(tx_split_avg)
		}

		/// Returns the fee (in native currency) charged for a paid unload token.
		///
		/// This fee covers the estimated weight of a coin's *entire* lifecycle, not just the unload
		/// itself. Coins are transferred and split without charging until the maximum age. The
		/// unload token fee is the only time a fee is collected, accounting for all future network
		/// cost the coin will incur.
		pub(crate) fn paid_unload_token_fee_in_native() -> NativeBalanceOf<T> {
			let weight = Self::coin_lifecycle_weight();
			T::WeightToFee::convert(weight)
		}

		/// Returns the amount of the instance's underlying asset that buys `fee_native` of the
		/// native currency right now.
		///
		/// This is a quote and varies if operations on the market happen.
		pub(crate) fn quote_asset_for_native_fee(
			instance_id: InstanceId,
			fee_native: NativeBalanceOf<T>,
		) -> Result<FungiblesBalanceOf<T>, FeeConversionError> {
			let asset_id = Instances::<T>::get(instance_id)
				.ok_or(FeeConversionError::InstanceNotFound)?
				.asset_id;
			// Special case: no conversion is needed for the native currency, and zero is zero.
			if fee_native.is_zero() || asset_id == T::NativeAssetKind::get() {
				return Ok(fee_native);
			}
			T::FeeConversion::quote_price_tokens_for_exact_tokens(
				asset_id,
				T::NativeAssetKind::get(),
				fee_native,
				// The market's own fee is part of what the payer has to provide.
				true,
			)
			.ok_or(FeeConversionError::Unavailable)
		}

		/// Take at most `asset_in_max` of `asset` from `payer` and deposit exactly `native_amount`
		/// of the native currency into `beneficiary`, returning what the asset side actually cost.
		///
		/// The counterpart of [`Self::quote_asset_for_native_fee`].
		fn charge_asset_and_transfer_native(
			asset: FungiblesAssetIdOf<T>,
			payer: &T::AccountId,
			native_amount: FungiblesBalanceOf<T>,
			asset_in_max: FungiblesBalanceOf<T>,
			beneficiary: &T::AccountId,
		) -> Result<FungiblesBalanceOf<T>, DispatchError> {
			if native_amount.is_zero() {
				return Ok(Zero::zero());
			}
			if asset == T::NativeAssetKind::get() {
				ensure!(native_amount <= asset_in_max, Error::<T>::FeeExceedsMaxFee);
				T::Fungibles::transfer(
					asset,
					payer,
					beneficiary,
					native_amount,
					Preservation::Preserve,
				)?;
				return Ok(native_amount);
			}
			T::FeeConversion::swap_tokens_for_exact_tokens(
				payer.clone(),
				alloc::vec![asset, T::NativeAssetKind::get()],
				native_amount,
				Some(asset_in_max),
				beneficiary.clone(),
				true,
			)
		}

		/// Returns the amount of the native currency that pays `count` paid unload token fees.
		pub(crate) fn paid_unload_token_fees_in_native(count: u32) -> NativeBalanceOf<T> {
			Self::paid_unload_token_fee_in_native().saturating_mul(count.into())
		}

		/// Returns the amount of the instance's underlying asset that pays `count` paid unload
		/// token fees.
		pub(crate) fn quote_paid_unload_token_fees_in_asset(
			instance_id: InstanceId,
			count: u32,
		) -> Result<FungiblesBalanceOf<T>, FeeConversionError> {
			let fee_native = Self::paid_unload_token_fees_in_native(count);
			Self::quote_asset_for_native_fee(instance_id, fee_native)
		}

		/// Returns the amount of the instance's underlying asset that pays one paid unload token
		/// fee.
		pub(crate) fn quote_paid_unload_token_fee_in_asset(
			instance_id: InstanceId,
		) -> Result<FungiblesBalanceOf<T>, FeeConversionError> {
			Self::quote_asset_for_native_fee(instance_id, Self::paid_unload_token_fee_in_native())
		}
	}
}

/// Helper trait for runtime benchmarks.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper<T: Config> {
	/// Setup the underlying external asset used by benchmarks, and the instance wrapping it.
	fn setup_assets();
	/// Setup the underlying external asset without an instance wrapping it, returning its id.
	/// Used by the [`Pallet::create_sufficient_instance`] benchmark.
	///
	/// Must give the pallet account the asset's minimum balance, which the measured call
	/// requires admin to have provided beforehand.
	fn setup_asset_without_instance() -> FungiblesAssetIdOf<T>;
	/// Fund an account with fungibles balance.
	fn fund_account(who: &T::AccountId, amount: FungiblesBalanceOf<T>);
	/// Create (if it does not exist yet) an extra sufficient asset distinct per `seed`, mint a
	/// large balance of it to `who` and return its id.
	///
	/// Extra assets are distinct from the asset `setup_assets` wraps. Used as underlying assets
	/// of sponsored instances and as deposit currencies.
	fn create_extra_asset(seed: u32, who: &T::AccountId) -> FungiblesAssetIdOf<T>;
	/// The id of the asset [`BenchmarkHelper::create_extra_asset`] creates for the same `seed`.
	///
	/// A pure mapping: it does not create the asset.
	fn extra_asset_id(seed: u32) -> FungiblesAssetIdOf<T>;
	/// Set the current time (needed because benchmarks run at genesis where timestamp is 0).
	fn set_time(now: core::time::Duration);
	/// Set up the market that converts an instance's underlying asset into the native currency, so
	/// that fees can be paid with the asset. It must be deep enough for the benchmarked
	/// conversions.
	fn setup_fee_conversion();
	/// Create a people proof for the given context, message, and alias.
	fn create_people_proof(
		context: &[u8],
		msg: &[u8],
		alias: Alias,
	) -> <T::MembershipProof as ValidateProof>::Proof;
	/// Create a lite people proof for the given context, message, and alias.
	fn create_lite_people_proof(
		context: &[u8],
		msg: &[u8],
		alias: Alias,
	) -> <T::MembershipProof as ValidateProof>::Proof;

	/// Set up a recycler ring with `count` members and create valid alias proofs
	/// for extra batch verification benchmarks.
	///
	/// Returns `(value, ring_index, aliases, alias_proofs, proven_msg)` ready for
	/// `RecyclerManager::unload()`.
	///
	/// This helper is optional for runtimes. The default implementation returns
	/// `Err(Weightless)`, so runtimes that do not run these extra benchmarks do
	/// not need to implement it.
	fn setup_batch_verify(
		count: u32,
	) -> Result<
		(Denomination, RingIndex, Vec<Alias>, Vec<ProofOf<T>>, [u8; 32]),
		frame_benchmarking::BenchmarkError,
	> {
		let _ = count;
		Err(frame_benchmarking::BenchmarkError::Weightless)
	}
}
