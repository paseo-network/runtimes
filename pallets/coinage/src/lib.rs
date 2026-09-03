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

//! Coinage system for (lite) people.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
pub mod extension;
pub mod paid_tkn_manager;
pub mod recycler_manager;
pub mod weights;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub use paid_tkn_manager::*;
pub use pallet::*;
pub use recycler_manager::*;
pub use weights::WeightInfo;

use alloc::{collections::BTreeSet, vec::Vec};
use codec::Encode;
use frame_support::{
	pallet_prelude::*,
	storage::types::{Key as NMapKey, StorageNMap},
	traits::{
		fungible::{self, Mutate as _},
		fungibles::{self, Inspect, Mutate as _, MutateHold as _},
		tokens::{ConversionToAssetBalance, Fortitude, Precision, Preservation, Restriction},
		Defensive, IsSubType, UnixTime,
	},
	PalletId,
};
use frame_system::{
	offchain::{CreateAuthorizedTransaction, SubmitTransaction},
	pallet_prelude::*,
};
use indiv_support::traits::{
	Alias, AppendOnlyMembers, AppendOnlyMembersWeightInfo, Context, Identifier, MembershipProver,
	RevisionIndex, RingExponent, RingIndex,
};
use sp_io::hashing::blake2_256;
use sp_runtime::{
	traits::{AccountIdConversion, Convert, Zero},
	SaturatedConversion, Saturating,
};
use verifiable::GenerateVerifiable;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	/// The ring-vrf context for interactions with the recycler.
	pub const UNLOADING_RECYCLER_CONTEXT: Context = *b"pop:polkadot.network/coinrecyclr";
	/// The base for the ring-vrf context for people and lite people free unload token.
	pub const FREE_UNLOAD_TOKEN_CONTEXT_BASE: [u8; 24] = *b"pop:polkadot.net/coinftk";
	/// The base for the ring-vrf context for paid unload token.
	pub const PAID_UNLOAD_TOKEN_CONTEXT_BASE: [u8; 28] = *b"pop:polkadot.net/coinpaidtok";

	/// Base prefix for recycler collection identifiers (one per coin value).
	pub const RECYCLER_COLLECTION_PREFIX: [u8; 16] = *b"coinage/recycler";

	/// Base prefix for paid token collection identifiers (one per period).
	pub const PAID_TOKEN_COLLECTION_PREFIX: [u8; 16] = *b"coinage/paidtkn!";

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

	/// Maximum number of consumed free unload tokens to remove per call.
	pub(crate) const CLEAN_CONSUMED_FREE_TOKEN_LIMIT: u32 = 10_000;

	pub type CryptoOf<T> = <<T as Config>::MemberService as MembershipProver>::Crypto;
	pub type MemberOf<T> = <CryptoOf<T> as GenerateVerifiable>::Member;
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

	/// The coin value is represented as an exponent of 2 multiplied to
	/// [Config::UnderlyingAssetUnit], i.e., value = 2^(CoinValue) * UnderlyingAssetUnit.
	pub type CoinValue = i8;

	/// An abstract interface defining the proof type and the validation method for a proof, a
	/// context and a message, and resulting with the alias of the prover inside the context.
	///
	/// This can be implemented to validate a ring-vrf proof for people.
	pub trait ValidateProof {
		/// The type of the proof to be validated.
		type Proof;

		/// Validate the given proof against the context and the message.
		/// Return the alias of the prover if the proof is valid.
		#[allow(clippy::result_unit_err)]
		fn validate_proof(proof: &Self::Proof, context: &[u8], msg: &[u8]) -> Result<Alias, ()>;
	}

	/// Invalidity reasons for the transaction extension validation.
	#[derive(Clone)]
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
		CoinValueTooBig = 47,
		CoinValueTooSmall = 48,
		CoinValueOutOfBound = 49,
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
		CoinValueIsLessThanFee = 66,
		InvalidProofOfOwnership = 67,
		/// The fee coin value is below `MinimumExponentForOutputUnloadFee`.
		FeeCoinBelowMinimum = 68,
		/// The output-fee extension (`AsUnloadTokenFromOutput`)
		/// can only be used with external asset and coin unload calls.
		FromOutputFeeNotAllowed = 69,
		/// The alias proofs array is empty.
		EmptyAliasProofs = 70,
		/// The paid token ring revision does not match (ring may not exist or has been rebuilt).
		InvalidPaidTokenRingRevision = 71,
		/// This operation requires a fresh coin (`age == 0`).
		FreshCoinRequired = 72,
		/// The paid unload token fee cannot be resolved in the underlying asset balance.
		/// This is currently hit when validating free unload tokens and their coverage.
		CannotConvertNativeToAsset = 73,
		/// The coin cannot be used yet because it is temporarily locked after a failed dispatch.
		/// The lock duration grows exponentially with each consecutive failure.
		CoinTemporarilyLocked = 74,
		/// One of the alias proofs failed verification.
		InvalidAliasProof = 75,
		/// The max_fee parameter is insufficient to cover the network unload fee.
		MaxFeeInsufficientForUnload = 76,
		/// When using Prepaid fee mode, max_fee must be 0.
		MaxFeeNotAllowedForPrepaid = 77,
		/// The coin value cannot be losslessly converted to an asset amount because
		/// `UnderlyingAssetUnit` is not evenly divisible by `2^|value|`.
		LossyCoinValueConversion = 78,
		/// The first alias in the call does not match the alias derived from the first proof
		/// validated in the extension.
		FirstCallAliasMismatch = 79,
		/// The `InfallibleUnpaidSigned` extension requires a signed origin.
		InfallibleUnpaidSignedOriginMustBeSigned = 80,
		/// The caller does not have enough of the underlying asset to cover the load amount
		/// required by the `InfallibleUnpaidSigned` extension.
		InfallibleUnpaidSignedInsufficientBalance = 81,
		/// The recycler collection for the given coin value does not exist yet.
		RecyclerCollectionNotCreated = 82,
		/// The mixed-output unload call is missing aliases or voucher outputs.
		EmptyMixedOutput = 83,
		/// A batched `load_recycler_with_external_asset_unpaid` call has no inner items.
		EmptyUnpaidLoadBatch = 84,
		/// The underlying asset id has not been set yet.
		AssetIdNotSet = 85,
	}

	impl From<CustomInvalidity> for TransactionValidityError {
		fn from(e: CustomInvalidity) -> Self {
			InvalidTransaction::Custom(e as u8).into()
		}
	}

	pub(crate) enum MixedOutputValidationError {
		EmptyAliases,
		EmptyVouchers,
		InvalidSplit,
		MemberKeyAlreadyUsed,
		InvalidMemberKey,
		CoinValue(CoinValueToAssetAmountError),
		Fee(CannotConvertNativeToAssetError),
	}

	impl MixedOutputValidationError {
		fn into_pallet_error<T: Config>(self) -> Error<T> {
			match self {
				MixedOutputValidationError::EmptyAliases => Error::<T>::EmptyInputs,
				MixedOutputValidationError::EmptyVouchers |
				MixedOutputValidationError::InvalidSplit => Error::<T>::InvalidSplit,
				MixedOutputValidationError::MemberKeyAlreadyUsed =>
					Error::<T>::MemberKeyAlreadyUsed,
				MixedOutputValidationError::InvalidMemberKey => Error::<T>::InvalidMemberKey,
				MixedOutputValidationError::CoinValue(e) => e.into_pallet_error::<T>(),
				MixedOutputValidationError::Fee(e) => e.into_pallet_error::<T>(),
			}
		}

		fn into_custom_invalidity(self) -> CustomInvalidity {
			match self {
				MixedOutputValidationError::EmptyAliases |
				MixedOutputValidationError::EmptyVouchers => CustomInvalidity::EmptyMixedOutput,
				MixedOutputValidationError::InvalidSplit => CustomInvalidity::InvalidSplit,
				MixedOutputValidationError::MemberKeyAlreadyUsed =>
					CustomInvalidity::MemberKeyAlreadyUsed,
				MixedOutputValidationError::InvalidMemberKey => CustomInvalidity::InvalidMemberKey,
				MixedOutputValidationError::CoinValue(e) => e.into_custom_invalidity(),
				MixedOutputValidationError::Fee(e) => e.into(),
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
	/// the coin can't be transferred or split and must be recycled.
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
		/// The value of the coin.
		pub value: CoinValue,
		/// The age of the coin. The age increases by one on each transfer or split. After a
		/// certain age, the coin can't be transferred or split and must be recycled.
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

	/// Metadata for a temporarily locked coin.
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
	pub struct LockedCoin {
		/// Why the coin is locked.
		pub reason: LockReason,
		/// Unix timestamp (seconds) at which the lock expires.
		pub until: u64,
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
		pub value: CoinValue,
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
		/// The coin value of the recycler the member key is being loaded into.
		pub value: CoinValue,
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

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	/// Coins by owner.
	///
	/// This storage map contains all the coins currently circulating. The coin is minted when
	/// unloaded from the recycler, and destroyed when loaded into the recycler.
	#[pallet::storage]
	pub type CoinsByOwner<T: Config> = StorageMap<_, Twox64Concat, T::AccountId, Coin>;

	/// Temporary lock expiry for coins that previously failed dispatch.
	///
	/// A coin owner entry is locked until the stored Unix timestamp, preventing repeated failed
	/// dispatch attempts in a short period.
	#[pallet::storage]
	pub type LockedCoins<T: Config> =
		StorageMap<_, Twox64Concat, T::AccountId, LockedCoin, OptionQuery>;

	/// The total value of coins that were burnt.
	///
	/// This tracks value that is intentionally destroyed as part of protocol flows (for example:
	/// recycler expiration cleanup and output-token spam penalty path). This storage item keeps
	/// track of the total value of such destroyed coins.
	#[pallet::storage]
	pub type TotalValueOfDestroyedCoins<T> = StorageValue<_, FungiblesBalanceOf<T>, ValueQuery>;

	/// Consumed free unload tokens by period and alias.
	///
	/// This storage keeps track of the free unload tokens that have been consumed by people
	/// and lite people, to avoid double spending.
	///
	/// It is cleared periodically.
	#[pallet::storage]
	pub type ConsumedFreeUnloadTokens<T: Config> =
		StorageDoubleMap<_, Twox64Concat, Period, Twox64Concat, Alias, ()>;

	/// Tracks whether a recycler collection exists for a given coin value.
	///
	/// Recycler collections are normally created eagerly during one-time `on_poll`
	/// initialization after `UnderlyingAssetId` has been set.
	/// [`RecyclerManager::ensure_collection_exists`] remains the fallback for
	/// first-use or recovery paths when a collection is still missing.
	///
	/// **WARNING**: Do not use this storage directly, use [`RecyclerManager`] type instead.
	///
	/// This storage item is managed by [`RecyclerManager`] and is part of a consistent set:
	/// * [RecyclerCollectionCreated] - whether the collection exists for a coin value.
	/// * [RecyclersLastRemovedRingIndex] - the last removed ring index for each coin value.
	/// * [RecyclersCoinToRecycler] - the mapping from member key to the coin value it is in.
	/// * [RecyclersUnloaded] - the recyclers' unloaded aliases, indexed by coin value and ring
	///   index.
	/// * [RecyclersDusting] - marks rings with unloaded aliases pending removal.
	///
	/// Ring members, pending members, and ring state are managed by [`Config::MemberService`].
	#[pallet::storage]
	pub type RecyclerCollectionCreated<T> = StorageMap<_, Twox64Concat, CoinValue, (), OptionQuery>;

	/// Last removed ring index per recycler coin value.
	///
	/// Rings are removed sequentially starting from index 0. The next ring to check for
	/// expiration is `last_removed + 1` (or `0` if nothing has been removed yet).
	///
	/// **WARNING**: Do not use this storage directly, use [`RecyclerManager`] type instead.
	///
	/// This storage item is managed by [`RecyclerManager`] and is part of a consistent set:
	/// * [RecyclerCollectionCreated] - whether the collection exists for a coin value.
	/// * [RecyclersLastRemovedRingIndex] - the last removed ring index for each coin value.
	/// * [RecyclersCoinToRecycler] - the mapping from member key to the coin value it is in.
	/// * [RecyclersUnloaded] - the recyclers' unloaded aliases, indexed by coin value and ring
	///   index.
	/// * [RecyclersDusting] - marks rings with unloaded aliases pending removal.
	///
	/// Ring members, pending members, and ring state are managed by [`Config::MemberService`].
	#[pallet::storage]
	pub type RecyclersLastRemovedRingIndex<T> = StorageMap<_, Twox64Concat, CoinValue, RingIndex>;

	/// Mapping from a recycler member key to the coin value it belongs to.
	///
	/// When a coin is loaded into a recycler, the member key is recorded here so that the
	/// pallet can look up which coin value the member key corresponds to.
	///
	/// **WARNING**: Do not use this storage directly, use [`RecyclerManager`] type instead.
	///
	/// This storage item is managed by [`RecyclerManager`] and is part of a consistent set:
	/// * [RecyclerCollectionCreated] - whether the collection exists for a coin value.
	/// * [RecyclersLastRemovedRingIndex] - the last removed ring index for each coin value.
	/// * [RecyclersCoinToRecycler] - the mapping from member key to the coin value it is in.
	/// * [RecyclersUnloaded] - the recyclers' unloaded aliases, indexed by coin value and ring
	///   index.
	/// * [RecyclersDusting] - marks rings with unloaded aliases pending removal.
	///
	/// Ring members, pending members, and ring state are managed by [`Config::MemberService`].
	#[pallet::storage]
	pub type RecyclersCoinToRecycler<T> = StorageMap<_, Twox64Concat, MemberOf<T>, CoinValue>;

	/// The recyclers' unloaded aliases, indexed by (coin value, ring index, alias).
	///
	/// When a coin is unloaded from a recycler, the alias produced by the ring-VRF proof is
	/// stored here to prevent double-spending within the same recycler ring.
	///
	/// **WARNING**: Do not use this storage directly, use [`RecyclerManager`] type instead.
	///
	/// This storage item is managed by [`RecyclerManager`] and is part of a consistent set:
	/// * [RecyclerCollectionCreated] - whether the collection exists for a coin value.
	/// * [RecyclersLastRemovedRingIndex] - the last removed ring index for each coin value.
	/// * [RecyclersCoinToRecycler] - the mapping from member key to the coin value it is in.
	/// * [RecyclersUnloaded] - the recyclers' unloaded aliases, indexed by coin value and ring
	///   index.
	/// * [RecyclersDusting] - marks rings with unloaded aliases pending removal.
	///
	/// Ring members, pending members, and ring state are managed by [`Config::MemberService`].
	#[pallet::storage]
	pub type RecyclersUnloaded<T> = StorageNMap<
		_,
		(
			NMapKey<Twox64Concat, CoinValue>,
			NMapKey<Twox64Concat, RingIndex>,
			NMapKey<Twox64Concat, Alias>,
		),
		(),
		OptionQuery,
	>;

	/// Marks recycler rings that have unloaded aliases pending removal.
	///
	/// When a recycler ring is removed, the cleanup of its unloaded aliases in
	/// [RecyclersUnloaded] is performed gradually through this storage item. An entry here
	/// indicates that unloaded aliases for the given coin value and ring index still exist
	/// and should be dusted.
	///
	/// **WARNING**: Do not use this storage directly, use [`RecyclerManager`] type instead.
	///
	/// This storage item is managed by [`RecyclerManager`] and is part of a consistent set:
	/// * [RecyclerCollectionCreated] - whether the collection exists for a coin value.
	/// * [RecyclersLastRemovedRingIndex] - the last removed ring index for each coin value.
	/// * [RecyclersCoinToRecycler] - the mapping from member key to the coin value it is in.
	/// * [RecyclersUnloaded] - the recyclers' unloaded aliases, indexed by coin value and ring
	///   index.
	/// * [RecyclersDusting] - marks rings with unloaded aliases pending removal.
	///
	/// Ring members, pending members, and ring state are managed by [`Config::MemberService`].
	#[pallet::storage]
	pub type RecyclersDusting<T> =
		StorageMap<_, Twox64Concat, (CoinValue, RingIndex), (), OptionQuery>;

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
	pub type PaidUnloadTokenMembers<T> = StorageMap<_, Twox64Concat, MemberOf<T>, ()>;

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

	/// Whether the one-time pallet initialization has run.
	///
	/// Set by [`Pallet::do_initialize`] once recycler collections are created and the pallet
	/// account has been ensured to hold the minimum balance of [`UnderlyingAssetId`].
	/// Initialization is gated on `UnderlyingAssetId` being set, so this stays unset until
	/// governance has called [`Pallet::set_underlying_asset_id`].
	#[pallet::storage]
	pub type InitializePalletAccount<T: Config> = StorageValue<_, (), OptionQuery>;

	/// The underlying asset id for the coins.
	///
	/// Set once by [`Config::UnderlyingAssetIdManager`] via
	/// [`Pallet::set_underlying_asset_id`]. While unset, every coin/recycler operation that
	/// needs the underlying asset fails with [`Error::AssetIdNotSet`] (or
	/// [`CustomInvalidity::AssetIdNotSet`] in transaction extension validation).
	#[pallet::storage]
	pub type UnderlyingAssetId<T: Config> = StorageValue<_, FungiblesAssetIdOf<T>, OptionQuery>;

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
		type MemberService: AppendOnlyMembers
			+ MembershipProver<
				Crypto: GenerateVerifiable<
					Proof: Send + Sync + DecodeWithMemTracking,
					Signature: Send + Sync + DecodeWithMemTracking,
					Member: DecodeWithMemTracking,
					Config: TryFrom<RingExponent>,
				>,
			> + AppendOnlyMembersWeightInfo;

		/// The location of the owner for the coinage collections.
		type CollectionOwner: Get<<Self::MemberService as AppendOnlyMembers>::Location>;

		/// The ring exponent for recycler collections.
		#[pallet::constant]
		type RecyclerRingExponent: Get<RingExponent>;

		/// The ring exponent for paid unload token collections.
		#[pallet::constant]
		type PaidUnloadTokenRingExponent: Get<RingExponent>;

		/// The native fungible of the chain.
		type NativeFungible: fungible::Mutate<Self::AccountId>;

		/// The fungibles containing the underlying asset of the coins.
		///
		/// We intentionally keep this without `fungibles::Create`: normal pallet usage does not
		/// require it. Benchmarks that need asset setup will be enabled once benchmark helper
		/// support (`T::BenchmarkHelper::setup_assets()`) is merged.
		type Fungibles: fungibles::MutateHold<Self::AccountId, Reason: From<HoldReason>>
			+ fungibles::Mutate<Self::AccountId>;

		/// The unit of the underlying asset of the coins.
		#[pallet::constant]
		type UnderlyingAssetUnit: Get<FungiblesBalanceOf<Self>>;

		/// Origin allowed to set the underlying asset id once via
		/// [`Pallet::set_underlying_asset_id`].
		type UnderlyingAssetIdManager: EnsureOrigin<Self::RuntimeOrigin>;

		/// The validator for people proofs.
		type PeopleProof: ValidateProof<Proof: Parameter + Send + Sync>;

		/// The validator for lite people proofs.
		// TODO: Remove once lite people get ring membership via `pallet-members` and we can unify
		// with `PeopleProof`.
		type LitePeopleProof: ValidateProof<Proof: Parameter + Send + Sync>;

		/// The minimum exponent for the coin value.
		#[pallet::constant]
		type MinimumExponent: Get<i8>;

		/// The maximum exponent for the coin value.
		#[pallet::constant]
		type MaximumExponent: Get<i8>;

		/// The minimum coin exponent that can be used to dispatch a call `unload_recycler_*` with
		/// the transaction extension `AsUnloadTokenFromOutput`.
		///
		/// This ensures the fee coin is large enough to penalize failing transactions, but it does
		/// not need to cover the whole unload token fee.
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
		type MaximumAge: Get<u16>;

		/// The time period duration for unload tokens, in seconds.
		#[pallet::constant]
		type UnloadTokenTimePeriodPeopleLitePeople: Get<u32>;

		/// The allowance of unload tokens that a person can use per time period, expressed in the
		/// underlying asset.
		///
		/// Use pallet view to fetch the corresponding number of unload tokens given the current
		/// price for unload tokens.
		#[pallet::constant]
		type UnloadTokenAllowancePerTimePeriodForPeople: Get<FungiblesBalanceOf<Self>>;

		/// The allowance of unload tokens that a lite person can use per time period, expressed in
		/// the underlying asset.
		///
		/// Use pallet's get_free_unload_token_info() to fetch the corresponding number of unload
		/// tokens given the current price for unload tokens.
		#[pallet::constant]
		type UnloadTokenAllowancePerTimePeriodForLitePeople: Get<FungiblesBalanceOf<Self>>;

		/// Hard upper bound on the number of free unload tokens per time period.
		///
		/// The effective free token limit is:
		/// `min(allowance / current_fee, MaxFreeUnloadTokensPerTimePeriod)`.
		#[pallet::constant]
		type MaxFreeUnloadTokensPerTimePeriod: Get<u32>;

		/// The expiration time for a recycler ring, in seconds, after it is full.
		type RecyclerExpirationTime: Get<u32>;

		/// The expiration time for a paid unload token ring, in seconds, after its period is over.
		type PaidUnloadTokenRingExpirationTime: Get<u32>;

		/// The time period duration for paid unload tokens, in seconds.
		type PaidUnloadTokenTimePeriod: Get<u32>;

		/// The conversion from native balance to asset balance.
		type ConversionToAssetBalance: ConversionToAssetBalance<
			NativeBalanceOf<Self>,
			FungiblesAssetIdOf<Self>,
			FungiblesBalanceOf<Self>,
			Error: Into<DispatchError>,
		>;

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
		FromOutput {
			/// The coin value of the fee recycler validated in extension.
			/// Must match `inputs[0].value` in the call.
			fee_recycler_value: CoinValue,
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
			output_count: u32,
		},
		CoinTransferred {
			to: T::AccountId,
			value: CoinValue,
			new_age: u16,
		},
		RecyclerLoadedWithCoin {
			value: CoinValue,
		},
		RecyclerLoadedWithExternalAsset {
			who: T::AccountId,
			value: CoinValue,
			amount: FungiblesBalanceOf<T>,
		},
		RecyclerUnloadedIntoCoin {
			to: T::AccountId,
			input_value: CoinValue,
			output_value: CoinValue,
			input_count: u32,
		},
		RecyclerUnloadedIntoExternalAsset {
			to: T::AccountId,
			value: CoinValue,
			input_count: u32,
			amount: FungiblesBalanceOf<T>,
		},
		RecyclerUnloadedIntoExternalAssetAndVouchers {
			to: T::AccountId,
			value: CoinValue,
			input_count: u32,
			external_asset_amount: FungiblesBalanceOf<T>,
			voucher_count: u32,
		},
		PaidUnloadTokenRegisteredWithCoin {
			fee: FungiblesBalanceOf<T>,
			destroyed: FungiblesBalanceOf<T>,
		},
		PaidUnloadTokenRegisteredWithNative {
			who: T::AccountId,
			fee: NativeBalanceOf<T>,
		},
		PaidUnloadTokenRegisteredWithExternalAsset {
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
			to: T::AccountId,
			output_value: CoinValue,
			input_count: u32,
		},
		RecyclersUnloadedIntoExternalAsset {
			to: T::AccountId,
			input_count: u32,
			amount: FungiblesBalanceOf<T>,
		},
		RecyclersUnloadedIntoExternalAssetNonAnonymous {
			who: T::AccountId,
			to: T::AccountId,
			input_count: u32,
			amount: FungiblesBalanceOf<T>,
			fee_currency: FeeCurrency,
		},
		RecyclerUnloadedIntoCoins {
			output_count: u32,
		},
		CoinOffboardedIntoExternalAsset {
			to: T::AccountId,
			value: CoinValue,
			amount: FungiblesBalanceOf<T>,
		},
		RecyclerCleaned {
			value: CoinValue,
			remaining_coins: u32,
			destroyed_amount: FungiblesBalanceOf<T>,
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
		UnderlyingAssetIdSet {
			asset_id: FungiblesAssetIdOf<T>,
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
		CoinValueTooBig,
		CoinValueTooSmall,
		CoinValueIsLessThanFee,
		CoinValueOutOfBound,
		/// The coin value cannot be losslessly converted to an asset amount because
		/// `UnderlyingAssetUnit` is not evenly divisible by `2^|value|`.
		LossyCoinValueConversion,
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
		/// This operation requires a fresh coin (`age == 0`).
		FreshCoinRequired,
		CannotConvertNativeToAsset,
		/// When using Prepaid fee mode, max_fee must be 0.
		MaxFeeNotAllowedForPrepaid,
		/// The max_fee exceeds the total input value.
		MaxFeeExceedsInput,
		/// The max fee argument doesn't satisfy the requirements.
		InvalidMaxFee,
		/// The recycler collection does not exist and could not be created on-demand.
		CannotCreateRecyclerCollection,
		/// The underlying asset id has not been set yet.
		AssetIdNotSet,
		/// The underlying asset id has already been set and cannot be changed.
		AssetIdAlreadySet,
		/// The proposed underlying asset id does not exist in [`Config::Fungibles`].
		UnknownAsset,
	}

	/// A reason for the pallet placing a hold on funds.
	#[pallet::composite_enum]
	pub enum HoldReason {
		/// Hold for wrapped coins in the coinage system.
		Wrapped,
	}

	#[pallet::view_functions]
	impl<T: Config> Pallet<T> {
		/// Get the current number of free unload tokens distributed to people and lite people
		/// given the current price for unload tokens.
		///
		/// If an element is `None`, no price is currently available and conversion between native
		/// and the underlying asset needs to be configured.
		///
		/// Returns: `(limit_people, limit_lite_people)`.
		///
		/// Each element is `None` when its limit cannot be computed.
		pub fn get_free_unload_token_info() -> (Option<u32>, Option<u32>) {
			(
				Self::free_unload_token_limit_for_people().ok(),
				Self::free_unload_token_limit_for_lite_people().ok(),
			)
		}

		/// Get the ring status for a recycler at a given ring index.
		pub fn get_recycler_ring_status(
			value: CoinValue,
			index: RingIndex,
		) -> Option<indiv_support::traits::RingStatus> {
			let identifier = Self::recycler_collection_identifier(value);
			T::MemberService::ring_status(&identifier, index)
		}

		/// Get the ring revision for a recycler at a given ring index.
		pub fn get_recycler_ring_revision(
			value: CoinValue,
			index: RingIndex,
		) -> Option<RevisionIndex> {
			let identifier = Self::recycler_collection_identifier(value);
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

		/// Get the current fee in the underlying asset for paid unload tokens.
		///
		/// If none is returned it means that no price is currently available, and some conversion
		/// between native and the underlying asset needs to be configured.
		pub fn get_paid_unload_token_fee_in_asset() -> Option<FungiblesBalanceOf<T>> {
			Self::paid_unload_token_fee_in_asset().ok()
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

		/// Get the coin value for a specific recycler member key.
		pub fn get_recycler_member_info(member: MemberOf<T>) -> Option<CoinValue> {
			RecyclersCoinToRecycler::<T>::get(member)
		}

		/// Check whether a paid token member key is registered.
		pub fn is_paid_token_member(member: MemberOf<T>) -> bool {
			PaidUnloadTokenMembers::<T>::contains_key(member)
		}

		/// Get the members of a recycler ring.
		/// Required to build the ring commitment (accumulator) for the proof.
		pub fn get_recycler_members(value: CoinValue, index: RingIndex) -> Vec<MemberOf<T>> {
			let identifier = Self::recycler_collection_identifier(value);
			T::MemberService::ring_members(&identifier, index)
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
			value: CoinValue,
			index: RingIndex,
			alias: Alias,
		) -> bool {
			RecyclersUnloaded::<T>::contains_key((value, index, alias))
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

			// Maximum of the possible unload call weights.
			let call_weight = Pallet::<T>::unload_recycler_into_external_asset_and_vouchers_weight(
				T::MaxConsolidation::get() as usize,
				T::MaxSplitOutputs::get() as usize,
			)
			.max(Pallet::<T>::unload_recycler_into_external_asset_weight(
				T::MaxConsolidation::get() as usize,
			))
			.max(Pallet::<T>::unload_recycler_into_coins_weight(
				T::MaxConsolidation::get() as usize,
				T::MaxSplitOutputs::get(),
			));

			ext_weight.saturating_add(call_weight)
		}
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn integrity_test() {
			assert!(
				!T::UnderlyingAssetUnit::get().is_zero(),
				"UnderlyingAssetUnit must be greater than zero",
			);

			assert!(
				T::MinimumExponent::get() <= T::MaximumExponent::get(),
				"MinimumExponent must be <= MaximumExponent",
			);

			// Ensure that the maximum coin value in unit of minimum coin value can be represented
			// in u32.
			// This property is used by `validate_split`.
			let msg = "exponent range is too big, the maximum coin value in unit of minimum coin \
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

			// Ensure every valid coin value can be converted to an asset amount without error.
			for value in T::MinimumExponent::get()..=T::MaximumExponent::get() {
				assert!(
					Self::coin_value_to_asset_amount(value).is_ok(),
					"coin_value_to_asset_amount failed for value {value}",
				);
			}

			assert!(T::MaxConsolidation::get() > 0, "MaxConsolidation must be greater than zero",);
			assert!(T::MaxSplitOutputs::get() >= 2, "MaxSplitOutputs must be at least 2",);
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
			for (value, ()) in RecyclerCollectionCreated::<T>::iter() {
				if RecyclerManager::<T>::ensure_can_clean(value).is_ok() {
					let call = Call::clean_recycler { value };
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

			// One-time initialization: create recycler collections and fund the pallet account.
			// Gated on `UnderlyingAssetId` being set so we stay inert until governance has
			// called `set_underlying_asset_id`. Coins can only enter `CoinsByOwner` via paths
			// that already require an asset id, so we never need collections before that.
			let check_weight = T::WeightInfo::on_poll_initialize_check_condition();
			if weight.can_consume(check_weight) {
				let needs_init =
					!InitializePalletAccount::<T>::exists() && UnderlyingAssetId::<T>::exists();
				if needs_init {
					let init_weight = T::WeightInfo::on_poll_initialize();
					if weight.can_consume(check_weight.saturating_add(init_weight)) {
						if let Err(e) = Self::do_initialize() {
							log::warn!(
								target: LOG_TARGET,
								"failed to initialize pallet account: {e:?}"
							);
						}
						weight.consume(init_weight);
					}
				}
				weight.consume(check_weight);
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

		pub(crate) fn unload_recycler_into_external_asset_weight(alias_count: usize) -> Weight {
			let n = alias_count as u32;
			if n <= 2 {
				T::WeightInfo::unload_recycler_into_external_asset_1_2(n)
			} else if n <= 8 {
				T::WeightInfo::unload_recycler_into_external_asset_3_8(n)
			} else {
				T::WeightInfo::unload_recycler_into_external_asset_9_max(n)
			}
		}

		pub(crate) fn unload_recycler_into_external_asset_and_vouchers_weight(
			alias_count: usize,
			voucher_count: usize,
		) -> Weight {
			let a = alias_count as u32;
			let d = voucher_count as u32;
			if a <= 2 {
				T::WeightInfo::unload_recycler_into_external_asset_and_vouchers_1_2(a, d)
			} else if a <= 8 {
				T::WeightInfo::unload_recycler_into_external_asset_and_vouchers_3_8(a, d)
			} else {
				T::WeightInfo::unload_recycler_into_external_asset_and_vouchers_9_max(a, d)
			}
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

		pub(crate) fn unload_recycler_into_coins_weight(
			alias_count: usize,
			destination_count: u32,
		) -> Weight {
			let a = alias_count as u32;
			if a <= 2 {
				T::WeightInfo::unload_recycler_into_coins_1_2(a, destination_count)
			} else if a <= 8 {
				T::WeightInfo::unload_recycler_into_coins_3_8(a, destination_count)
			} else {
				T::WeightInfo::unload_recycler_into_coins_9_max(a, destination_count)
			}
		}

		/// Shared dispatch body for [`Call::load_recycler_with_external_asset_unpaid`] and
		/// [`Call::load_recycler_with_external_asset_unpaid_batch`].
		///
		/// All preconditions (member-key validity, signature, balance, collection existence) are
		/// checked in the `AsCoinage` transaction extension; the inner calls here re-run them
		/// defensively and are expected never to fail.
		fn do_unpaid_load(
			who: &T::AccountId,
			preservation: CodecPreservation,
			value: CoinValue,
			member_key: MemberOf<T>,
		) -> DispatchResult {
			let asset_amount = Self::coin_value_to_asset_amount(value)
				.defensive_proof("coinage: coin value conversion checked in validate")
				.map_err(|e| e.into_pallet_error::<T>())?;
			let asset_id = Self::underlying_asset_id()
				.defensive_proof("coinage: asset id checked in validate")?;

			T::Fungibles::transfer_and_hold(
				asset_id,
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

			RecyclerManager::<T>::load(value, member_key)
				.inspect_err(|e| match e {
					RecyclerLoadError::MemberKeyAlreadyUsed => {
						defensive!("coinage: member key duplicate checked in validate");
					},
					RecyclerLoadError::InvalidMemberKey => {
						defensive!("coinage: invalid member key checked in validate");
					},
					RecyclerLoadError::CannotCreateRecyclerCollection => {
						defensive!("coinage: collection existence checked in validate");
					},
					RecyclerLoadError::InternalError => {
						defensive!("coinage: internal error");
					},
				})
				.map_err(|e| e.into_pallet_error::<T>())?;

			Self::deposit_event(Event::RecyclerLoadedWithExternalAsset {
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
		/// * The coin value must be within the bounds defined by [Config::MinimumExponent] and
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
				(CoinValue, BoundedVec<T::AccountId, T::MaxSplitOutputs>),
				T::MaxSplitOutputs,
			>,
		) -> DispatchResultWithPostInfo {
			let Ok(Origin::Coin { coin_id: _, coin }) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};
			let output_count = split_into.iter().map(|(_, dests)| dests.len() as u32).sum();

			// This call should not fail; the origin's coin is already consumed by the transaction
			// extension before dispatch. Validation ensures all preconditions are met.

			for (value, dests) in split_into {
				for dest in dests {
					let new_coin = Coin { value, age: coin.age.saturating_add(1) };

					// The destination has no coin, as verified during validation.
					CoinsByOwner::<T>::insert(&dest, new_coin);
				}
			}
			Self::deposit_event(Event::CoinSplit { output_count });

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
			let value = coin.value;
			let new_age = coin.age.saturating_add(1);

			// This call should not fail; the origin's coin is already consumed by the transaction
			// extension before dispatch. Validation ensures all preconditions are met.

			// The destination has no coin, as verified during validation.
			CoinsByOwner::<T>::insert(&to, Coin { value, age: new_age });
			Self::deposit_event(Event::CoinTransferred { to, value, new_age });

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
		#[pallet::call_index(2)]
		#[pallet::weight(T::WeightInfo::load_recycler_with_coin())]
		pub fn load_recycler_with_coin(
			origin: OriginFor<T>,
			member_key: MemberOf<T>,
			_proof_of_ownership: SignatureOf<T>,
		) -> DispatchResultWithPostInfo {
			let Ok(Origin::Coin { coin_id: _, coin }) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};

			// This call should not fail; the origin's coin is already consumed by the transaction
			// extension before dispatch. Validation ensures all preconditions are met.

			RecyclerManager::<T>::load(coin.value, member_key)
				.inspect_err(|e| match e {
					RecyclerLoadError::MemberKeyAlreadyUsed => {
						defensive!("coinage: member key duplicate checked in validate");
					},
					RecyclerLoadError::InvalidMemberKey => {
						defensive!("coinage: invalid member key checked in validate");
					},
					RecyclerLoadError::CannotCreateRecyclerCollection => {
						defensive!("coinage: collection existence checked in validate");
					},
					RecyclerLoadError::InternalError => {
						defensive!("coinage: internal error");
					},
				})
				.map_err(|e| e.into_pallet_error::<T>())?;
			Self::deposit_event(Event::RecyclerLoadedWithCoin { value: coin.value });

			Ok(Pays::No.into())
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
		/// The `value` parameter indicates the coin value to be loaded into the recycler.
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
		/// * The `member_key` must not already be used in another recycler.
		/// * The `member_key` must be valid (i.e. well formed).
		/// * The `value` must be within the bounds defined by [Config::MinimumExponent] and
		///   [Config::MaximumExponent].
		/// * The signer must have enough balance of the underlying asset to cover the equivalent
		///   amount for the given coin value.
		/// * The `proof_of_ownership` must be a valid signature of the signer's account id by the
		/// `member_key`.
		#[pallet::call_index(3)]
		#[pallet::weight(T::WeightInfo::load_recycler_with_external_asset())]
		pub fn load_recycler_with_external_asset(
			origin: OriginFor<T>,
			preservation: CodecPreservation,
			value: CoinValue,
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

			let asset_amount =
				Self::coin_value_to_asset_amount(value).map_err(|e| e.into_pallet_error::<T>())?;
			let asset_id = Self::underlying_asset_id()?;

			T::Fungibles::transfer_and_hold(
				asset_id,
				&HoldReason::Wrapped.into(),
				&who,
				&Self::pallet_account(),
				asset_amount,
				Precision::Exact,
				preservation.into(),
				Fortitude::Polite,
			)?;
			RecyclerManager::<T>::load(value, member_key)
				.map_err(|e| e.into_pallet_error::<T>())?;
			Self::deposit_event(Event::RecyclerLoadedWithExternalAsset {
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
		///
		/// The transaction extension validation phase must ensure:
		/// - The `member_key` is valid and not already used in another recycler.
		/// - The `proof_of_ownership` is a valid signature of the signer's account id by the
		///   `member_key`.
		/// - The `value` is within the bounds defined by [Config::MinimumExponent] and
		///   [Config::MaximumExponent], and can be losslessly converted to an asset amount.
		/// - The signer has enough balance of the underlying asset to cover the equivalent amount
		///   for the given coin value (respecting `preservation`).
		/// - The nonce is valid for replay protection.
		/// - The recycler collection for `value` already exists.
		///
		/// The call is free.
		#[pallet::call_index(15)]
		#[pallet::weight(T::WeightInfo::load_recycler_with_external_asset_unpaid())]
		pub fn load_recycler_with_external_asset_unpaid(
			origin: OriginFor<T>,
			preservation: CodecPreservation,
			value: CoinValue,
			member_key: MemberOf<T>,
			_proof_of_ownership: SignatureOf<T>,
		) -> DispatchResultWithPostInfo {
			let Ok(Origin::<T>::InfallibleUnpaidSigned { who }) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};

			Self::do_unpaid_load(&who, preservation, value, member_key)?;

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
		/// The call is free.
		#[pallet::call_index(16)]
		#[pallet::weight(T::WeightInfo::load_recycler_with_external_asset_unpaid()
			.saturating_mul(items.len() as u64))]
		pub fn load_recycler_with_external_asset_unpaid_batch(
			origin: OriginFor<T>,
			items: BoundedVec<UnpaidLoadInput<T>, T::MaxBatchUnpaidLoad>,
		) -> DispatchResultWithPostInfo {
			let Ok(Origin::<T>::InfallibleUnpaidSigned { who }) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};

			for item in items.into_iter() {
				Self::do_unpaid_load(&who, item.preservation, item.value, item.member_key)?;
			}

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
		/// * `value` and `index`: identifies the recycler being unloaded.
		/// * `_revision`: the recycler revision used for the alias_proofs.
		/// * `to`: the destination account for the new coin.
		///
		/// Requirements:
		/// * The origin must be [Origin::UnloadToken] with `fee: UnloadFee::Prepaid`.
		/// * The recycler identified by `value` and `index` must exist.
		/// * The alias proofs provided in the origin must be valid for the recycler's revision.
		/// * The `aliases` provided must match the aliases derived from the proofs.
		/// * The aliases must not have been already unloaded from this recycler.
		/// * The number of aliases must be a power of two.
		/// * The resulting consolidated value must not exceed [Config::MaximumExponent].
		// The `MaxConsolidation` is enforced through the origin `UnloadToken`.
		#[pallet::call_index(4)]
		#[pallet::weight(Pallet::<T>::unload_recycler_into_coin_weight(aliases.len()))]
		pub fn unload_recycler_into_coin(
			origin: OriginFor<T>,
			aliases: BoundedVec<Alias, T::MaxConsolidation>,
			value: CoinValue,
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
			RecyclerManager::<T>::unload(
				value,
				index,
				revision,
				&aliases,
				&alias_proofs,
				&proven_msg,
			)?;
			let increment = aliases
				.len()
				.trailing_zeros()
				.try_into()
				.map_err(|_| Error::<T>::ConsolidationTooBig)?;
			let new_value = value.checked_add(increment).ok_or(Error::<T>::ConsolidationTooBig)?;
			ensure!(new_value <= T::MaximumExponent::get(), Error::<T>::ConsolidationTooBig);
			let input_count = aliases.len() as u32;

			// The destination has no coin, as verified during validation.
			CoinsByOwner::<T>::insert(&to, Coin { value: new_value, age: 0 });
			Self::deposit_event(Event::RecyclerUnloadedIntoCoin {
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
		/// When `fee` is [UnloadFee::FromOutput], the fee is deducted from the unloaded assets.
		///
		/// This function allows a user to withdraw their coins back into the underlying
		/// asset (e.g., an external asset).
		///
		/// Parameters:
		/// * `aliases`: the list of aliases corresponding to the member keys included in the
		///   recycler. The proofs for these aliases are contained in the origin.
		/// * `value` and `index`: identifies the recycler being unloaded.
		/// * `_revision`: the recycler revision used for the alias_proofs.
		/// * `to`: the destination account for the underlying asset.
		///
		/// Requirements:
		/// * The origin must be [Origin::UnloadToken].
		/// * The recycler identified by `value` and `index` must exist.
		/// * The alias proofs provided in the origin must be valid for the recycler's revision.
		/// * The aliases must not have been already unloaded (except for the first one when `fee`
		///   is [UnloadFee::FromOutput], which was pre-marked in the extension).
		// The `MaxConsolidation` is enforced through the origin `UnloadToken`.
		#[pallet::call_index(5)]
		#[pallet::weight(Pallet::<T>::unload_recycler_into_external_asset_weight(aliases.len()))]
		pub fn unload_recycler_into_external_asset(
			origin: OriginFor<T>,
			aliases: BoundedVec<Alias, T::MaxConsolidation>,
			value: CoinValue,
			index: RingIndex,
			revision: RevisionIndex,
			to: T::AccountId,
		) -> DispatchResult {
			// Convert to single-element input for unified processing
			let input = UnloadRecyclerInput { value, index, revision, aliases };
			let inputs = [input];

			let amount_for_value =
				Self::coin_value_to_asset_amount(value).map_err(|e| e.into_pallet_error::<T>())?;
			let total_amount =
				amount_for_value.saturating_mul((inputs[0].aliases.len() as u32).into());

			let Ok(Origin::UnloadToken { alias_proofs, proven_msg, fee }) = origin.into() else {
				return Err(DispatchError::BadOrigin);
			};

			Self::process_unload_inputs_with_fee(&inputs, &alias_proofs, &proven_msg, fee)?;

			let transfer_amount = match fee {
				UnloadFee::Prepaid => total_amount,
				UnloadFee::FromOutput { .. } => Self::deduct_and_transfer_fee(total_amount)?,
			};

			Self::transfer_external_asset(&to, transfer_amount)?;
			Self::deposit_event(Event::RecyclerUnloadedIntoExternalAsset {
				to,
				value,
				input_count: inputs[0].aliases.len() as u32,
				amount: transfer_amount,
			});

			Ok(())
		}

		/// Pay the fee to register a member key for a paid unload token using a coin.
		///
		/// The origin must be a [Origin::Coin], which can be obtained from the transaction
		/// extension [`AsCoinage`](crate::extension::AsCoinage).
		///
		/// The coin is consumed. The fee is deducted from the coin's value and transferred to
		/// [Config::FeeDestination]. The remaining value of the coin is destroyed.
		///
		/// If the call fails, the origin coin is still consumed.
		///
		/// To protect the user against varying fees, if the coin's value is less than the fee, the
		/// call is invalid (an invalid call never goes into a block).
		///
		/// The `proof_of_ownership` is a signature of the caller's account ID by the `member_key`.
		/// This ensures the caller controls the member key to prevent front-running.
		///
		/// Requirements:
		/// * The coin's age must be less than [Config::MaximumAge].
		/// * The coin value must be sufficient to cover the fee.
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

			let amount = Self::coin_value_to_asset_amount(coin.value)
				.map_err(|e| e.into_pallet_error::<T>())?;
			let fee =
				Self::paid_unload_token_fee_in_asset().map_err(|e| e.into_pallet_error::<T>())?;
			ensure!(amount >= fee, Error::<T>::CoinValueIsLessThanFee);
			let asset_id = Self::underlying_asset_id()
				.defensive_proof("coinage: asset id checked in validate")?;

			// Release the fee amount from the hold
			T::Fungibles::release(
				asset_id.clone(),
				&HoldReason::Wrapped.into(),
				&Self::pallet_account(),
				fee,
				Precision::Exact,
			)?;

			// Transfer the fee
			T::Fungibles::transfer(
				asset_id,
				&Self::pallet_account(),
				&T::FeeDestination::get(),
				fee,
				// This is an issue if the total amount held by the system goes below
				// the existential deposit of the underlying asset.
				// But we fixed it by initializing the pallet account with some funds.
				Preservation::Preserve,
			)?;

			// The remaining amount stays held (effectively destroyed/burnt)
			let remaining = amount.saturating_sub(fee);
			TotalValueOfDestroyedCoins::<T>::mutate(|v| *v = v.saturating_add(remaining));

			PaidTknManager::<T>::add_member(coin_id, member_key, proof_of_ownership)?;
			Self::deposit_event(Event::PaidUnloadTokenRegisteredWithCoin {
				fee,
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
		/// external asset.
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
		#[pallet::call_index(8)]
		#[pallet::weight(T::WeightInfo::pay_for_recycler_unload_fee_token_with_external_asset())]
		pub fn pay_for_recycler_unload_fee_token_with_external_asset(
			origin: OriginFor<T>,
			member_key: MemberOf<T>,
			proof_of_ownership: <CryptoOf<T> as GenerateVerifiable>::Signature,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;

			let fee =
				Self::paid_unload_token_fee_in_asset().map_err(|e| e.into_pallet_error::<T>())?;
			// `paid_unload_token_fee_in_asset` already returns `AssetIdNotSet` when storage
			// is unset, so reaching this line guarantees the asset id is set.
			let asset_id = Self::underlying_asset_id()
				.defensive_proof("coinage: asset id present after fee-in-asset call")?;

			T::Fungibles::transfer(
				asset_id,
				&who,
				&T::FeeDestination::get(),
				fee,
				Preservation::Protect,
			)?;

			PaidTknManager::<T>::add_member(who.clone(), member_key, proof_of_ownership)?;
			Self::deposit_event(Event::PaidUnloadTokenRegisteredWithExternalAsset { who, fee });

			Ok(())
		}

		/// Unload a recycler into a mix of external asset and fresh vouchers.
		///
		/// The origin must be [Origin::UnloadToken], which can be obtained from the transaction
		/// extension [`AsCoinage`](crate::extension::AsCoinage).
		///
		/// This function allows a user to offboard part of the unloaded value into the underlying
		/// asset while reminting the rest as fresh recycler vouchers.
		///
		/// When `fee` is [UnloadFee::Prepaid], `external_asset_amount` is transferred as-is.
		/// When `fee` is [UnloadFee::FromOutput], the fee is deducted from the specified
		/// `external_asset_amount`, so the recipient receives the remainder.
		///
		/// Parameters:
		/// * `aliases`: the list of aliases corresponding to the member keys included in the
		///   recycler. The proofs for these aliases are contained in the origin.
		/// * `value` and `index`: identifies the recycler being unloaded.
		/// * `revision`: the recycler revision used for the alias proofs.
		/// * `to`: the destination account for the external asset portion.
		/// * `external_asset_amount`: the gross asset portion to offboard from the unloaded value.
		/// * `new_vouchers`: the fresh recycler vouchers to mint from the remaining unloaded value.
		///
		/// The total unloaded value must always equal the asset portion plus the voucher portion.
		/// In `FromOutput` mode, the asset portion must be large enough to cover the unload fee.
		///
		/// Requirements:
		/// * The origin must be [Origin::UnloadToken].
		/// * The recycler identified by `value` and `index` must exist.
		/// * The alias proofs provided in the origin must be valid for the recycler's revision.
		/// * The aliases must not have been already unloaded (except for the first one when `fee`
		///   is [UnloadFee::FromOutput], which was pre-marked in the extension).
		/// * `new_vouchers` must not be empty, and all voucher member keys must be valid and
		///   unused.
		/// * The total unloaded value must equal `external_asset_amount` plus the total asset value
		///   of `new_vouchers`.
		/// * When using [UnloadFee::FromOutput], `external_asset_amount` must cover the fee.
		#[pallet::call_index(9)]
		#[pallet::weight(
			Pallet::<T>::unload_recycler_into_external_asset_and_vouchers_weight(
				aliases.len(),
				new_vouchers.len()
			)
		)]
		pub fn unload_recycler_into_external_asset_and_vouchers(
			origin: OriginFor<T>,
			aliases: BoundedVec<Alias, T::MaxConsolidation>,
			value: CoinValue,
			index: RingIndex,
			revision: RevisionIndex,
			to: T::AccountId,
			external_asset_amount: FungiblesBalanceOf<T>,
			new_vouchers: BoundedVec<(CoinValue, MemberOf<T>), T::MaxSplitOutputs>,
		) -> DispatchResult {
			let Ok(Origin::UnloadToken { alias_proofs, proven_msg, fee }) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};

			// Keep the mixed-output invariant check in dispatch as the final safety guard for the
			// pallet logic itself, even though the extension validates the same shape earlier.
			Self::validate_mixed_output_outputs(
				value,
				aliases.len() as u32,
				external_asset_amount,
				&new_vouchers,
			)
			.map_err(MixedOutputValidationError::into_pallet_error::<T>)?;

			match fee {
				UnloadFee::Prepaid => {},
				UnloadFee::FromOutput { .. } => {
					let required_unload_fee = Self::paid_unload_token_fee_in_asset()
						.map_err(MixedOutputValidationError::Fee)
						.map_err(MixedOutputValidationError::into_pallet_error::<T>)?;
					if external_asset_amount < required_unload_fee {
						return Err(Error::<T>::InsufficientUnloadForFee.into());
					}
				},
			}

			let input = UnloadRecyclerInput { value, index, revision, aliases };
			let inputs = [input];

			Self::process_unload_inputs_with_fee(&inputs, &alias_proofs, &proven_msg, fee)?;
			RecyclerManager::<T>::load_batch_grouped(&new_vouchers)
				.map_err(|e| e.into_pallet_error::<T>())?;

			let transfer_amount = match fee {
				UnloadFee::Prepaid => external_asset_amount,
				UnloadFee::FromOutput { .. } =>
					Self::deduct_and_transfer_fee(external_asset_amount)?,
			};

			Self::transfer_external_asset(&to, transfer_amount)?;
			Self::deposit_event(Event::RecyclerUnloadedIntoExternalAssetAndVouchers {
				to,
				value,
				input_count: inputs[0].aliases.len() as u32,
				external_asset_amount: transfer_amount,
				voucher_count: new_vouchers.len() as u32,
			});
			Ok(())
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
		)]
		pub fn unload_recycler_into_external_asset_non_anonymous(
			origin: OriginFor<T>,
			input: UnloadRecyclerInput<T::MaxConsolidation>,
			alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation>,
			to: T::AccountId,
			fee_currency: FeeCurrency,
		) -> DispatchResultWithPostInfo {
			Self::unload_recyclers_into_external_asset_non_anonymous(
				origin,
				alloc::vec![input],
				alias_proofs,
				to,
				fee_currency,
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
		/// Parameters:
		/// * `inputs`: A list of inputs, specifying the recycler and aliases to unload.
		/// * `alias_proofs`: the proofs for all aliases across all inputs, signed over a message
		///   that includes the signer. The proofs must correspond sequentially to the aliases in
		///   `inputs`.
		/// * `to`: the destination account for the asset.
		/// * `fee_currency`: whether to pay the fee in native currency or external asset.
		///
		/// Requirements:
		/// * The origin must be Signed.
		/// * All specified recyclers must exist.
		/// * The alias proofs must correspond sequentially to the aliases in `inputs`.
		/// * `inputs` must not be empty and each element must contain at least one alias.
		/// * The signer must have sufficient balance to pay the fee (one fee per recycler).
		#[pallet::call_index(12)]
		#[pallet::weight(Pallet::<T>::unload_recyclers_into_external_asset_non_anonymous_weight(
			alias_proofs.len() as u32
		))]
		pub fn unload_recyclers_into_external_asset_non_anonymous(
			origin: OriginFor<T>,
			// It could be better to have a bound on this vec, like we do for other unload calls,
			// but given the origin is signed, the cost for a failing transaction will include the
			// transaction length, and if the transaction is successful then it is bounded by
			// `MaxConsolidation` (empty inputs are rejected) and it is charged for each input.
			inputs: Vec<UnloadRecyclerInput<T::MaxConsolidation>>,
			alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation>,
			to: T::AccountId,
			fee_currency: FeeCurrency,
		) -> DispatchResultWithPostInfo {
			let signer = ensure_signed(origin)?;

			ensure!(!inputs.is_empty(), Error::<T>::EmptyInputs);
			let input_count = inputs.iter().map(|input| input.aliases.len() as u32).sum();

			// Calculate total amount
			let mut total_amount: FungiblesBalanceOf<T> = Zero::zero();
			for input in &inputs {
				ensure!(!input.aliases.is_empty(), Error::<T>::EmptyInputs);

				let amount_per_coin = Self::coin_value_to_asset_amount(input.value)
					.map_err(|e| e.into_pallet_error::<T>())?;
				let amount_for_input =
					amount_per_coin.saturating_mul((input.aliases.len() as u32).into());
				total_amount = total_amount.saturating_add(amount_for_input);
			}

			// Construct proven_msg including the signer for non-anonymous proof binding
			let proven_msg = blake2_256(&(&inputs, &to, &signer).encode());

			Self::process_unload_inputs(&inputs, &alias_proofs, &proven_msg, false)?;
			let fee_count = inputs.len() as u32; // We charge one fee per recycler.
			Self::charge_fees_from_signer(&signer, fee_currency, fee_count)?;
			Self::transfer_external_asset(&to, total_amount)?;
			Self::deposit_event(Event::RecyclersUnloadedIntoExternalAssetNonAnonymous {
				who: signer,
				to,
				input_count,
				amount: total_amount,
				fee_currency,
			});

			// Refund the transaction weight fee since we charged explicitly
			Ok(Pays::No.into())
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
		/// * `value` and `index`: identifies the recycler being unloaded.
		/// * `revision`: the recycler revision used for the alias_proofs.
		/// * `split_into`: a vector of pairs, each pair containing a coin value and a list of
		///   destination account ids.
		/// * `max_fee`: the maximum fee the caller is willing to pay, expressed in the underlying
		///   asset balance. It must be equal to the difference between the total value of the
		///   unloaded coins and the total value of the new coins defined in `split_into`.
		///
		///   When using [UnloadFee::Prepaid], this must be 0.
		///   When using [UnloadFee::FromOutput], this amount is deducted from the input: the
		///   network fee is transferred to [Config::FeeDestination] and any remainder is burned.
		///   The caller can query `get_paid_unload_token_fee_in_asset` to estimate the fee.
		///
		///   This parameter serves as a safeguard: the transaction is rejected at validation if the
		///   actual network fee exceeds `max_fee`, protecting the caller from excessive fee
		///   increases that would render the argument `split_into` invalid (unloaded funds must be
		///   higher than the split plus the fee).
		///
		/// Requirements:
		/// * The origin must be [Origin::UnloadToken].
		/// * The recycler identified by `value` and `index` must exist.
		/// * The alias proofs provided in the origin must be valid for the recycler's revision.
		/// * The `aliases` provided must match the aliases derived from the proofs.
		/// * The total value of the new coins defined in `split_into` plus `max_fee` must equal the
		///   total value of the unloaded coins.
		/// * `max_fee` must be a multiple of the minimum coin. (This is implied by the condition
		///   above).
		/// * Each destination account must not already have a coin.
		/// * When using [UnloadFee::Prepaid], `max_fee` must be 0.
		/// * When using [UnloadFee::FromOutput], `max_fee` must cover the network fee.
		#[pallet::call_index(13)]
		#[pallet::weight(Pallet::<T>::unload_recycler_into_coins_weight(
			aliases.len(),
			split_into.iter().map(|(_, dests)| dests.len() as u32).sum::<u32>().max(1),
		))]
		pub fn unload_recycler_into_coins(
			origin: OriginFor<T>,
			aliases: BoundedVec<Alias, T::MaxConsolidation>,
			value: CoinValue,
			index: RingIndex,
			revision: RevisionIndex,
			split_into: BoundedVec<
				(CoinValue, BoundedVec<T::AccountId, T::MaxSplitOutputs>),
				T::MaxSplitOutputs,
			>,
			max_fee: FungiblesBalanceOf<T>,
		) -> DispatchResult {
			let Ok(Origin::UnloadToken { alias_proofs, proven_msg, fee }) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};

			// Validate max_fee constraints based on fee mode early, before other validations
			// that could produce confusing errors.
			match fee {
				UnloadFee::Prepaid => {
					ensure!(max_fee.is_zero(), Error::<T>::MaxFeeNotAllowedForPrepaid);
				},
				UnloadFee::FromOutput { .. } => {},
			}

			let output_count = split_into.iter().map(|(_, dests)| dests.len() as u32).sum();

			ensure!(!aliases.is_empty(), Error::<T>::EmptyInputs);

			let unit_per_input =
				Self::coin_value_to_unit(value).ok_or(Error::<T>::InternalError)?;

			let total_input_units = unit_per_input
				.checked_mul(aliases.len() as u32)
				.ok_or(Error::<T>::InternalError)?;

			let amount_per_unit = Self::coin_value_to_asset_amount(T::MinimumExponent::get())
				.map_err(|e| e.into_pallet_error::<T>())?;
			// Ensure max_fee is an exact multiple of the minimum coin. This is required because
			// max_fee must equal unloaded value minus split value, and both are always exact
			// multiples of the minimum coin.
			ensure!(max_fee % amount_per_unit == Zero::zero(), Error::<T>::InvalidMaxFee);
			let max_fee_units: u32 = max_fee
				.checked_div(&amount_per_unit)
				// `amount_per_unit` cannot be 0.
				.ok_or(Error::<T>::InternalError)?
				.saturated_into();

			let expected_total_units = total_input_units
				.checked_sub(max_fee_units)
				.ok_or(Error::<T>::MaxFeeExceedsInput)?;

			Self::validate_split_params(expected_total_units, &split_into)
				.map_err(|_| Error::<T>::InvalidSplit)?;

			match fee {
				UnloadFee::Prepaid => {
					// max_fee.is_zero() is already validated at the start of the function.
					RecyclerManager::<T>::unload(
						value,
						index,
						revision,
						&aliases,
						&alias_proofs,
						&proven_msg,
					)?;
				},
				UnloadFee::FromOutput { fee_recycler_value, fee_recycler_index } => {
					// Validate that the call's recycler matches the fee recycler from the extension
					ensure!(
						value == fee_recycler_value && index == fee_recycler_index,
						Error::<T>::RecyclerMismatch
					);

					// Verify the first alias was pre-marked by extension.
					// Note: Double-spend protection for FromOutput mode is in the extension's
					// validate_alias_proof(), which rejects already-unloaded aliases. This check
					// only verifies the extension did its job.
					let (first_alias, remaining_aliases) =
						aliases.split_first().ok_or(Error::<T>::EmptyInputs)?;
					let remaining_proofs = alias_proofs.get(1..).ok_or(Error::<T>::EmptyInputs)?;
					ensure!(
						RecyclersUnloaded::<T>::contains_key((value, index, *first_alias)),
						Error::<T>::AliasNotPremarked
					);

					// Process the remaining inputs properly, verifying cryptographic proofs
					// and marking them as unloaded.
					if !remaining_aliases.is_empty() {
						RecyclerManager::<T>::unload(
							value,
							index,
							revision,
							remaining_aliases,
							remaining_proofs,
							&proven_msg,
						)?;
					}

					// Transfer the network fee to the fee destination and burn the
					// remainder. Burning is preferred over transferring the surplus
					// to the fee destination as it benefits all holders equally by reducing
					// supply, and avoids overfunding.
					//
					// The remainder exists because max_fee is the difference between the split and
					// the unloaded amount. So it is unlikely to exactly match the unload token fee.
					let remaining = Self::deduct_and_transfer_fee(max_fee)?;
					if remaining > Zero::zero() {
						TotalValueOfDestroyedCoins::<T>::mutate(|v| {
							*v = v.saturating_add(remaining)
						});
					}
				},
			}

			for (v, dests) in split_into {
				for dest in dests {
					// The destination has no coin as checked in validation.
					CoinsByOwner::<T>::insert(&dest, Coin { value: v, age: 1 });
				}
			}
			Self::deposit_event(Event::RecyclerUnloadedIntoCoins { output_count });

			Ok(())
		}

		/// Directly offboard a fresh, 0-age coin into the underlying external asset.
		///
		/// The origin must be a [Origin::Coin], obtained through
		/// [`AsCoinage`](crate::extension::AsCoinage) using `AsCoin`.
		///
		/// Because the coin must be fresh (`age == 0`), this call bypasses the
		/// recycler/unload-token offboarding flow and releases the underlying asset directly.
		///
		/// Parameters:
		/// * `to`: destination account that receives the released underlying asset amount.
		///
		/// Requirements:
		/// * The origin must be [Origin::Coin].
		/// * The coin must be fresh: `coin.age == 0`.
		/// * The coin value must be representable as underlying-asset amount.
		#[pallet::call_index(14)]
		#[pallet::weight(T::WeightInfo::direct_offboard_coin_into_external_asset())]
		pub fn direct_offboard_coin_into_external_asset(
			origin: OriginFor<T>,
			to: T::AccountId,
		) -> DispatchResultWithPostInfo {
			let Ok(Origin::Coin { coin_id: _, coin }) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};

			ensure!(coin.age == 0, Error::<T>::FreshCoinRequired);

			let amount = Self::coin_value_to_asset_amount(coin.value)
				.map_err(|e| e.into_pallet_error::<T>())?;

			Self::transfer_external_asset(&to, amount)?;
			Self::deposit_event(Event::CoinOffboardedIntoExternalAsset {
				to,
				value: coin.value,
				amount,
			});

			Ok(Pays::No.into())
		}

		/// Set the underlying asset id used by the pallet.
		///
		/// The origin must satisfy [`Config::UnderlyingAssetIdManager`]. The setter is
		/// **single-use**: calling it again after the asset id has been set returns
		/// [`Error::AssetIdAlreadySet`]. Changing the underlying asset after coins exist would
		/// orphan the held balances of every in-flight coin, so the on-chain decision is
		/// intentionally one-shot.
		///
		/// The asset id must already exist in [`Config::Fungibles`].
		#[pallet::call_index(10)]
		#[pallet::weight(T::WeightInfo::set_underlying_asset_id())]
		pub fn set_underlying_asset_id(
			origin: OriginFor<T>,
			asset_id: FungiblesAssetIdOf<T>,
		) -> DispatchResult {
			T::UnderlyingAssetIdManager::ensure_origin(origin)?;
			ensure!(!UnderlyingAssetId::<T>::exists(), Error::<T>::AssetIdAlreadySet);
			ensure!(T::Fungibles::asset_exists(asset_id.clone()), Error::<T>::UnknownAsset);
			UnderlyingAssetId::<T>::put(asset_id.clone());
			Self::deposit_event(Event::UnderlyingAssetIdSet { asset_id });
			Ok(())
		}

		/// Clean up an expired recycler.
		///
		/// This is a maintenance call. The origin must be authorized and from local source.
		///
		/// This removes an old recycler that has exceeded its expiration time.
		/// Any remaining (not unloaded) value in the recycler is considered lost and added to
		/// [TotalValueOfDestroyedCoins].
		#[pallet::authorize(|source, value| {
			if !matches!(source, TransactionSource::InBlock | TransactionSource::Local) {
				return Err(CustomInvalidity::TransactionNotLocal.into());
			}
			let (validity, weight) = RecyclerManager::<T>::ensure_can_clean(*value)?;
			Ok((validity, weight))
		})]
		#[pallet::call_index(101)]
		#[pallet::weight(T::WeightInfo::clean_recycler(
			T::RecyclerRingExponent::get().ring_capacity(),
			T::RecyclerRingExponent::get().ring_capacity(),
		))]
		#[pallet::weight_of_authorize(T::WeightInfo::authorize_clean_recycler())]
		pub fn clean_recycler(
			origin: OriginFor<T>,
			value: CoinValue,
		) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;
			let (remaining_coins, member_count) = RecyclerManager::<T>::clean_unchecked(value)?;

			let underlying_value = Self::coin_value_to_asset_amount(value).map_err(|e| {
				log::error!(
					target: LOG_TARGET,
					"clean_recycler: unexpected conversion error: {e:?}",
				);
				Error::<T>::InternalError
			})?;
			let remaining_underlying_value = underlying_value
				.checked_mul(&remaining_coins.into())
				.ok_or(Error::<T>::InternalError)?;

			TotalValueOfDestroyedCoins::<T>::try_mutate::<_, Error<T>, _>(|total| {
				*total = total
					.checked_add(&remaining_underlying_value)
					.ok_or(Error::<T>::InternalError)?;
				Ok(())
			})?;
			Self::deposit_event(Event::RecyclerCleaned {
				value,
				remaining_coins,
				destroyed_amount: remaining_underlying_value,
			});

			let unloaded_count = member_count.saturating_sub(remaining_coins);
			Ok(Some(T::WeightInfo::clean_recycler(member_count, unloaded_count)).into())
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
		/// Removes up to DUST_CLEANUP_BATCH_SIZE unloaded alias entries per call to bound the
		/// operation.
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
	pub(crate) enum CoinValueToAssetAmountError {
		CoinValueOutOfBound,
		CoinValueTooSmall,
		CoinValueTooBig,
		LossyCoinValueConversion,
	}

	impl CoinValueToAssetAmountError {
		pub(crate) fn into_pallet_error<T: Config>(self) -> pallet::Error<T> {
			match self {
				CoinValueToAssetAmountError::CoinValueOutOfBound => Error::<T>::CoinValueOutOfBound,
				CoinValueToAssetAmountError::CoinValueTooSmall => Error::<T>::CoinValueTooSmall,
				CoinValueToAssetAmountError::CoinValueTooBig => Error::<T>::CoinValueTooBig,
				CoinValueToAssetAmountError::LossyCoinValueConversion =>
					Error::<T>::LossyCoinValueConversion,
			}
		}
		pub(crate) fn into_custom_invalidity(self) -> CustomInvalidity {
			match self {
				CoinValueToAssetAmountError::CoinValueOutOfBound =>
					CustomInvalidity::CoinValueOutOfBound,
				CoinValueToAssetAmountError::CoinValueTooSmall =>
					CustomInvalidity::CoinValueTooSmall,
				CoinValueToAssetAmountError::CoinValueTooBig => CustomInvalidity::CoinValueTooBig,
				CoinValueToAssetAmountError::LossyCoinValueConversion =>
					CustomInvalidity::LossyCoinValueConversion,
			}
		}
	}

	/// An error indicating that the paid unload token fee cannot be expressed in the underlying
	/// asset balance because either conversion failed, or asset id has not been set yet.
	#[derive(Debug, PartialEq, Eq)]
	pub enum CannotConvertNativeToAssetError {
		/// The price feed / conversion provider failed (e.g., no price available).
		ConversionFailed,
		/// The underlying asset id has not been set by governance yet.
		AssetIdNotSet,
	}

	impl CannotConvertNativeToAssetError {
		pub(crate) fn into_pallet_error<T: Config>(self) -> pallet::Error<T> {
			match self {
				Self::ConversionFailed => Error::<T>::CannotConvertNativeToAsset,
				Self::AssetIdNotSet => Error::<T>::AssetIdNotSet,
			}
		}
	}

	impl From<CannotConvertNativeToAssetError> for CustomInvalidity {
		fn from(e: CannotConvertNativeToAssetError) -> Self {
			match e {
				CannotConvertNativeToAssetError::ConversionFailed =>
					CustomInvalidity::CannotConvertNativeToAsset,
				CannotConvertNativeToAssetError::AssetIdNotSet => CustomInvalidity::AssetIdNotSet,
			}
		}
	}

	impl From<CannotConvertNativeToAssetError> for TransactionValidityError {
		fn from(e: CannotConvertNativeToAssetError) -> Self {
			CustomInvalidity::from(e).into()
		}
	}

	impl<T: Config> Pallet<T> {
		/// Returns the configured underlying asset id, or [`Error::AssetIdNotSet`] if governance
		/// has not set it yet via [`Pallet::set_underlying_asset_id`].
		pub fn underlying_asset_id() -> Result<FungiblesAssetIdOf<T>, Error<T>> {
			UnderlyingAssetId::<T>::get().ok_or(Error::<T>::AssetIdNotSet)
		}

		/// One-time pallet initialization, safe to re-run on partial failure.
		///
		/// Creates the recycler collections and funds the pallet account with the minimum
		/// balance of [`UnderlyingAssetId`]. Sets [`InitializePalletAccount`] when complete.
		/// Callers must gate on [`UnderlyingAssetId`] being set.
		pub(crate) fn do_initialize() -> DispatchResult {
			// Callers gate on `UnderlyingAssetId::exists()`, so this should always succeed.
			let asset_id = Self::underlying_asset_id()?;

			for value in T::MinimumExponent::get()..=T::MaximumExponent::get() {
				// These calls are idempotent, so a later failure can re-run them.
				RecyclerManager::<T>::ensure_collection_exists(value)?;
			}

			// We need to make the account for the pallet exist so it can receive and hold
			// any amount of underlying asset. Unfortunately, the fungibles trait doesn't
			// provide a way to force the existence of an account. So instead we just mint
			// out of nowhere the minimum balance.
			// TODO(#745): Avoid minting unbacked funds to bootstrap the pallet account.
			let pallet_acc = Self::pallet_account();
			if T::Fungibles::balance(asset_id.clone(), &pallet_acc).is_zero() {
				T::Fungibles::mint_into(
					asset_id.clone(),
					&pallet_acc,
					T::Fungibles::minimum_balance(asset_id),
				)?;
			}

			// INITIALIZATION SHOULD NEVER FAIL AFTER THIS POINT.
			//
			// The mint above has succeeded and although it is guarded with a check, if it is
			// drained because of some bug, then we would keep minting.

			// Mark as initialized only after all work succeeds, so that on failure
			// the next block retries automatically.
			InitializePalletAccount::<T>::put(());
			Ok(())
		}

		/// Generate a collection identifier for a recycler of a given coin value.
		pub fn recycler_collection_identifier(value: CoinValue) -> Identifier {
			let mut id = [0u8; 32];
			id[0..16].copy_from_slice(&RECYCLER_COLLECTION_PREFIX);
			id[16] = value as u8;
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
		/// The `allowance` is a fixed budget (in asset units) that each person/lite person is
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
		/// Returns an error if the fee cannot be calculated.
		pub fn free_unload_token_limit_for_people() -> Result<u32, CannotConvertNativeToAssetError>
		{
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
		/// Returns an error if the fee cannot be calculated.
		pub fn free_unload_token_limit_for_lite_people(
		) -> Result<u32, CannotConvertNativeToAssetError> {
			let allowance = T::UnloadTokenAllowancePerTimePeriodForLitePeople::get();
			Self::compute_free_unload_token_limit(allowance)
		}

		fn compute_free_unload_token_limit(
			allowance: FungiblesBalanceOf<T>,
		) -> Result<u32, CannotConvertNativeToAssetError> {
			let fee: u128 = Self::paid_unload_token_fee_in_asset()?.saturated_into();
			let allowance: u128 = allowance.saturated_into();

			let limit = match fee {
				0 => 0,
				_ => allowance / fee,
			};

			let limit = limit.saturated_into::<u32>();
			Ok(limit.min(T::MaxFreeUnloadTokensPerTimePeriod::get()))
		}

		/// Convert a coin value (exponent) to the corresponding amount of the underlying asset.
		///
		/// Each coin's value represents a power-of-2 exponent relative to
		/// [`Config::UnderlyingAssetUnit`]. The returned asset amount is computed as:
		///
		/// - Negative values: `UnderlyingAssetUnit >> |value|` (fractional denominations)
		/// - Non-negative values: `UnderlyingAssetUnit << value` (whole/multiple denominations)
		///
		/// Example for `UnderlyingAssetUnit = 1000`:
		///
		/// | CoinValue | Calculation      | Asset amount      |
		/// |-----------|------------------|-------------------|
		/// | -2        | 2^(-2) × 1000    | 250 (= unit / 4)  |
		/// | -1        | 2^(-1) × 1000    | 500 (= unit / 2)  |
		/// | 0         | 2^0 × 1000       | 1000 (= unit)     |
		/// | 1         | 2^1 × 1000       | 2000 (= unit * 2) |
		/// | 2         | 2^2 × 1000       | 4000 (= unit * 4) |
		///
		/// # Errors
		///
		/// - [`CoinValueToAssetAmountError::CoinValueOutOfBound`] — `value` falls outside
		///   `[MinimumExponent, MaximumExponent]`.
		/// - [`CoinValueToAssetAmountError::LossyCoinValueConversion`] — right-shifting
		///   `UnderlyingAssetUnit` would truncate bits (unit not divisible by `2^|value|`).
		/// - [`CoinValueToAssetAmountError::CoinValueTooSmall`] /
		///   [`CoinValueToAssetAmountError::CoinValueTooBig`] — shift exceeds the bit width of the
		///   balance type (unreachable with valid `MinimumExponent` / `MaximumExponent`
		///   configuration).
		pub(crate) fn coin_value_to_asset_amount(
			value: CoinValue,
		) -> Result<
			<T::Fungibles as fungibles::Inspect<T::AccountId>>::Balance,
			CoinValueToAssetAmountError,
		> {
			if value < T::MinimumExponent::get() || value > T::MaximumExponent::get() {
				return Err(CoinValueToAssetAmountError::CoinValueOutOfBound);
			}

			// Note: CoinValueTooSmall/CoinValueTooBig are unreachable given a valid
			// MinimumExponent/MaximumExponent configuration, since the bounds check
			// above rejects values before shifts can overflow.
			let unit = T::UnderlyingAssetUnit::get();

			if value < 0 {
				let shift = value.unsigned_abs() as u32;
				let shifted = unit
					.checked_shr(shift)
					.ok_or(CoinValueToAssetAmountError::CoinValueTooSmall)?;

				// Verify the division was exact: 1000 / 4 (2^2) = 250 is fine,
				// but 1000 / 16 (2^4) = 62.5 truncates to 62 which is lossy.
				if shifted.checked_shl(shift) != Some(unit) {
					return Err(CoinValueToAssetAmountError::LossyCoinValueConversion);
				}

				Ok(shifted)
			} else {
				let shift = value as u32;
				let shifted =
					unit.checked_shl(shift).ok_or(CoinValueToAssetAmountError::CoinValueTooBig)?;

				// checked_shl won't catch overflow within the u64 range,
				// e.g. 1000 * 2^55 needs 65 bits but silently truncates.
				// The round-trip verifies nothing was lost.
				if shifted.checked_shr(shift) != Some(unit) {
					return Err(CoinValueToAssetAmountError::CoinValueTooBig);
				}

				Ok(shifted)
			}
		}

		/// Deduct fee from total amount and transfer to fee destination.
		///
		/// Returns the remaining amount after fee deduction.
		fn deduct_and_transfer_fee(
			total_amount: FungiblesBalanceOf<T>,
		) -> Result<FungiblesBalanceOf<T>, DispatchError> {
			let fee =
				Self::paid_unload_token_fee_in_asset().map_err(|e| e.into_pallet_error::<T>())?;
			let transfer_amount =
				total_amount.checked_sub(&fee).ok_or(Error::<T>::InsufficientUnloadForFee)?;
			let asset_id = Self::underlying_asset_id()?;

			T::Fungibles::transfer_on_hold(
				asset_id,
				&HoldReason::Wrapped.into(),
				&Self::pallet_account(),
				&T::FeeDestination::get(),
				fee,
				Precision::Exact,
				Restriction::Free,
				Fortitude::Polite,
			)?;

			Ok(transfer_amount)
		}

		/// Transfer external asset from pallet account to recipient.
		///
		/// Skips transfer if amount is zero.
		fn transfer_external_asset(
			to: &T::AccountId,
			amount: FungiblesBalanceOf<T>,
		) -> DispatchResult {
			if amount > Zero::zero() {
				let asset_id = Self::underlying_asset_id()?;
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
			inputs: &[UnloadRecyclerInput<T::MaxConsolidation>],
			alias_proofs: &[ProofOf<T>],
			proven_msg: &[u8; 32],
			skip_first_alias: bool,
		) -> DispatchResult {
			let mut proofs_iter = alias_proofs.iter();

			for (idx, input) in inputs.iter().enumerate() {
				let count = input.aliases.len();
				let mut current_proofs = Vec::with_capacity(count);
				for _ in 0..count {
					let p = proofs_iter.next().ok_or(Error::<T>::ProofAndAliasMismatch)?;
					current_proofs.push(p.clone());
				}

				let skip_first = skip_first_alias && idx == 0;
				if skip_first {
					// First input with premarked first alias: skip first, validate rest
					if input.aliases.len() > 1 {
						RecyclerManager::<T>::unload(
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
						input.value,
						input.index,
						input.revision,
						&input.aliases,
						&current_proofs,
						proven_msg,
					)?;
				}
			}

			// Ensure all proofs in the origin were consumed
			ensure!(proofs_iter.next().is_none(), Error::<T>::ProofAndAliasMismatch);

			Ok(())
		}

		/// Validate the FromOutput fee invariants and process unload inputs.
		///
		/// For [UnloadFee::FromOutput], verifies that the first input matches the fee recycler
		/// validated in the extension and that its first alias was pre-marked, then processes
		/// the remaining aliases. For [UnloadFee::Prepaid], processes all aliases directly.
		fn process_unload_inputs_with_fee(
			inputs: &[UnloadRecyclerInput<T::MaxConsolidation>],
			alias_proofs: &[ProofOf<T>],
			proven_msg: &[u8; 32],
			fee: UnloadFee,
		) -> DispatchResult {
			match fee {
				UnloadFee::Prepaid =>
					Self::process_unload_inputs(inputs, alias_proofs, proven_msg, false),
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
						RecyclersUnloaded::<T>::contains_key((
							fee_recycler_value,
							fee_recycler_index,
							first_alias
						)),
						Error::<T>::AliasNotPremarked
					);
					Self::process_unload_inputs(inputs, alias_proofs, proven_msg, true)
				},
			}
		}

		/// Charge `count` unload fees from the signer in the specified currency.
		///
		/// Used by non-anonymous unload methods where the signer pays the fee explicitly.
		/// The fee is multiplied by `count` (typically the number of recyclers being unloaded).
		fn charge_fees_from_signer(
			signer: &T::AccountId,
			fee_currency: FeeCurrency,
			count: u32,
		) -> DispatchResult {
			match fee_currency {
				FeeCurrency::Native => {
					let fee_native = Self::paid_unload_token_fee_in_native();
					let total_fee = fee_native.saturating_mul(count.into());
					T::NativeFungible::transfer(
						signer,
						&T::FeeDestination::get(),
						total_fee,
						Preservation::Protect,
					)?;
				},
				FeeCurrency::ExternalAsset => {
					let fee_external_asset = Self::paid_unload_token_fee_in_asset()
						.map_err(|e| e.into_pallet_error::<T>())?;
					let total_fee = fee_external_asset.saturating_mul(count.into());
					let asset_id = Self::underlying_asset_id()?;
					T::Fungibles::transfer(
						asset_id,
						signer,
						&T::FeeDestination::get(),
						total_fee,
						Preservation::Protect,
					)?;
				},
			}
			Ok(())
		}

		/// Convert a coin value to its unit representation relative to the minimum exponent.
		///
		/// One unit is 2^minimum_exponent.
		///
		/// The coin value must have been checked to be within bounds prior to calling this
		/// function. But in case it is not then none is returned.
		///
		/// Note that an integrity test ensures the maximum coin value can be represented in u32
		/// units.
		pub(crate) fn coin_value_to_unit(value: CoinValue) -> Option<u32> {
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
			split_into: &[(CoinValue, BoundedVec<T::AccountId, T::MaxSplitOutputs>)],
		) -> Result<(), CustomInvalidity> {
			use CustomInvalidity::*;

			let max_split_outputs = T::MaxSplitOutputs::get();
			let minimum_exponent = T::MinimumExponent::get();
			let maximum_exponent = T::MaximumExponent::get();

			let mut previous_coin_value: Option<CoinValue> = None;
			let mut split_output_count: u32 = 0;
			// This is the total value expressed as a number of the minimum coin value.
			// The integrity test ensures that the maximum coin value can be represented in u32.
			let mut total_unit: u32 = 0;

			for (value, dest) in split_into {
				ensure!(!dest.is_empty(), EmptySplit);
				ensure!(*value >= minimum_exponent, SplitExponentTooSmall);
				ensure!(*value <= maximum_exponent, SplitExponentTooBig);
				ensure!(
					// ensure split_into is sorted by value strictly ascending.
					previous_coin_value
						.is_none_or(|previous_coin_value| previous_coin_value < *value),
					SplitIntoNotSorted,
				);
				previous_coin_value = Some(*value);

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

				let value_unit = Self::coin_value_to_unit(*value).ok_or(InternalError)?;

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
			split_into: &[(CoinValue, BoundedVec<T::AccountId, T::MaxSplitOutputs>)],
		) -> Result<(), CustomInvalidity> {
			if coin.age >= T::MaximumAge::get() {
				return Err(CustomInvalidity::CoinTooOld);
			}

			let expected_total =
				Self::coin_value_to_unit(coin.value).ok_or(CustomInvalidity::InternalError)?;

			Self::validate_split_params(expected_total, split_into)
		}

		pub(crate) fn validate_mixed_output_outputs(
			value: CoinValue,
			alias_count: u32,
			external_asset_amount: FungiblesBalanceOf<T>,
			new_vouchers: &[(CoinValue, MemberOf<T>)],
		) -> Result<(), MixedOutputValidationError> {
			if alias_count == 0 {
				return Err(MixedOutputValidationError::EmptyAliases);
			}
			if new_vouchers.is_empty() {
				return Err(MixedOutputValidationError::EmptyVouchers);
			}

			let amount_per_input = Self::coin_value_to_asset_amount(value)
				.map_err(MixedOutputValidationError::CoinValue)?;
			let alias_count: FungiblesBalanceOf<T> = alias_count.into();
			let total_input_amount = amount_per_input
				.checked_mul(&alias_count)
				.ok_or(MixedOutputValidationError::InvalidSplit)?;

			let mut voucher_amount: FungiblesBalanceOf<T> = Zero::zero();
			let mut seen = BTreeSet::new();

			for (voucher_value, member_key) in new_vouchers {
				let amount = Self::coin_value_to_asset_amount(*voucher_value)
					.map_err(MixedOutputValidationError::CoinValue)?;

				let encoded_key = member_key.encode();
				if !seen.insert(encoded_key) || RecyclerManager::<T>::is_member_key_used(member_key)
				{
					return Err(MixedOutputValidationError::MemberKeyAlreadyUsed);
				}

				if !CryptoOf::<T>::is_member_valid(member_key) {
					return Err(MixedOutputValidationError::InvalidMemberKey);
				}

				voucher_amount = voucher_amount
					.checked_add(&amount)
					.ok_or(MixedOutputValidationError::InvalidSplit)?;
			}

			let total_expected_amount = external_asset_amount
				.checked_add(&voucher_amount)
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
			if coin.age != 0 {
				return Err(CustomInvalidity::FreshCoinRequired);
			}

			Self::coin_value_to_asset_amount(coin.value).map_err(|e| e.into_custom_invalidity())?;

			Ok(())
		}

		/// Validate loading a recycler with a coin.
		pub(crate) fn validate_load_recycler_with_coin(
			coin: &Coin,
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

			if !RecyclerManager::<T>::collection_exists(coin.value) {
				return Err(CustomInvalidity::RecyclerCollectionNotCreated);
			}

			Ok(())
		}

		/// Validate loading a recycler with an external asset (infallible path).
		///
		/// Performs the following checks:
		/// - The `member_key` is not already used in another recycler.
		/// - The `member_key` is valid (well-formed).
		/// - The `proof_of_ownership` is a valid signature of `who`'s account id by the
		///   `member_key`.
		/// - The `value` can be losslessly converted to an asset amount (implying it is within the
		///   bounds defined by [Config::MinimumExponent] and [Config::MaximumExponent]).
		/// - `who` has enough reducible balance of the underlying asset to cover the equivalent
		///   amount for the given coin value (respecting `preservation`).
		/// - The recycler collection for the given coin value exists.
		pub(crate) fn validate_load_recycler_with_external_asset_unpaid(
			who: &T::AccountId,
			preservation: CodecPreservation,
			value: CoinValue,
			member_key: &MemberOf<T>,
			proof_of_ownership: &SignatureOf<T>,
		) -> Result<(), CustomInvalidity> {
			let load_cost =
				Self::validate_unpaid_load_item(who, value, member_key, proof_of_ownership)?;
			Self::check_unpaid_load_balance(who, preservation, load_cost)
		}

		/// Per-item checks for an `InfallibleUnpaidSigned` load. Returns the asset amount required
		/// for this single item so callers can either check it directly or aggregate it.
		fn validate_unpaid_load_item(
			who: &T::AccountId,
			value: CoinValue,
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

			let load_cost =
				Self::coin_value_to_asset_amount(value).map_err(|e| e.into_custom_invalidity())?;

			if !RecyclerManager::<T>::collection_exists(value) {
				return Err(CustomInvalidity::RecyclerCollectionNotCreated);
			}

			Ok(load_cost)
		}

		fn check_unpaid_load_balance(
			who: &T::AccountId,
			preservation: CodecPreservation,
			required: FungiblesBalanceOf<T>,
		) -> Result<(), CustomInvalidity> {
			let asset_id = UnderlyingAssetId::<T>::get()
				.defensive_proof("coinage: asset id checked in AsCoinage::validate gate")
				.ok_or(CustomInvalidity::AssetIdNotSet)?;
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

			Self::check_unpaid_load_balance(who, strictest, total)?;

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

			let amount = Self::coin_value_to_asset_amount(coin.value)
				.map_err(|e| e.into_custom_invalidity())?;

			let fee = Self::paid_unload_token_fee_in_asset()?;

			if amount < fee {
				return Err(CustomInvalidity::CoinValueIsLessThanFee);
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
		///      `unload_recycler_into_external_asset_and_vouchers`.
		///    - [UnloadFee::FromOutput]: only accepts `unload_recycler_into_coins`,
		///      `unload_recycler_into_external_asset`, and
		///      `unload_recycler_into_external_asset_and_vouchers`.
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
		pub(crate) fn validate_unload_calls(
			call: &<T as frame_system::Config>::RuntimeCall,
			fee: UnloadFee,
		) -> Result<(), CustomInvalidity> {
			let (value, index, revision, coin_destination, split_into, max_fee, mixed_output) =
				match call.is_sub_type() {
					Some(Call::<T>::unload_recycler_into_coin {
						revision,
						index,
						value,
						to,
						..
					}) => {
						match fee {
							UnloadFee::FromOutput { .. } =>
								return Err(CustomInvalidity::FromOutputFeeNotAllowed),
							UnloadFee::Prepaid => {},
						}
						(*value, *index, *revision, Some(to), None, None, None)
					},
					Some(Call::<T>::unload_recycler_into_coins {
						revision,
						index,
						value,
						max_fee,
						split_into,
						..
					}) => (
						*value,
						*index,
						*revision,
						None,
						Some(split_into.as_slice()),
						Some(*max_fee),
						None,
					),
					Some(Call::<T>::unload_recycler_into_external_asset {
						revision,
						index,
						value,
						..
					}) => (*value, *index, *revision, None, None, None, None),
					Some(Call::<T>::unload_recycler_into_external_asset_and_vouchers {
						revision,
						index,
						value,
						aliases,
						external_asset_amount,
						new_vouchers,
						..
					}) => (
						*value,
						*index,
						*revision,
						None,
						None,
						None,
						Some((
							aliases.len() as u32,
							*external_asset_amount,
							new_vouchers.as_slice(),
						)),
					),
					_ => return Err(CustomInvalidity::InvalidCall),
				};

			if !RecyclerManager::<T>::validate_recycler_revision(value, index, revision) {
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

			if let Some((alias_count, external_asset_amount, new_vouchers)) = mixed_output.as_ref()
			{
				// Validate mixed-output structure during extension validation so invalid calls are
				// rejected before free tokens are consumed or FromOutput aliases are premarked.
				Self::validate_mixed_output_outputs(
					value,
					*alias_count,
					*external_asset_amount,
					new_vouchers,
				)
				.map_err(MixedOutputValidationError::into_custom_invalidity)?;
			}

			match fee {
				UnloadFee::Prepaid =>
					if let Some(max_fee) = max_fee {
						ensure!(max_fee.is_zero(), CustomInvalidity::MaxFeeNotAllowedForPrepaid);
					},
				UnloadFee::FromOutput { .. } => {
					let required_unload_fee = Self::paid_unload_token_fee_in_asset()?;
					if let Some((_, external_asset_amount, _)) = mixed_output.as_ref() {
						if *external_asset_amount < required_unload_fee {
							return Err(CustomInvalidity::InvalidSplit);
						}
					}
					if let Some(max_fee) = max_fee {
						if max_fee < required_unload_fee {
							return Err(CustomInvalidity::MaxFeeInsufficientForUnload);
						}
					}
				},
			}

			Ok(())
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
				Some(Call::<T>::unload_recycler_into_external_asset_and_vouchers {
					aliases,
					..
				}) => aliases.first().copied(),
				_ => None,
			}
		}

		/// Validates recycler revisions for non-anonymous unload calls if the call is one of them.
		/// This is called from the extension to fail early (before fee payment) when proofs are
		/// outdated due to ring revision changes.
		///
		/// For any other call, this is a no-op (returns Ok).
		pub(crate) fn validate_non_anonymous_unload_calls_revisions(
			call: &<T as frame_system::Config>::RuntimeCall,
		) -> Result<(), CustomInvalidity> {
			// Only validate for non-anonymous unload calls
			match call.is_sub_type() {
				Some(Call::<T>::unload_recycler_into_external_asset_non_anonymous {
					input,
					fee_currency,
					..
				}) => {
					if !RecyclerManager::<T>::validate_recycler_revision(
						input.value,
						input.index,
						input.revision,
					) {
						return Err(CustomInvalidity::InvalidRecyclerRevision);
					}

					// `ExternalAsset` additionally needs the native→asset conversion rate.
					if *fee_currency == FeeCurrency::ExternalAsset {
						Self::paid_unload_token_fee_in_asset()?;
					}
				},
				Some(Call::<T>::unload_recyclers_into_external_asset_non_anonymous {
					inputs,
					fee_currency,
					..
				}) => {
					for input in inputs {
						if !RecyclerManager::<T>::validate_recycler_revision(
							input.value,
							input.index,
							input.revision,
						) {
							return Err(CustomInvalidity::InvalidRecyclerRevision);
						}
					}

					if *fee_currency == FeeCurrency::ExternalAsset {
						Self::paid_unload_token_fee_in_asset()?;
					}
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
			// Load coins into recycler.
			let load_one = T::WeightInfo::load_recycler_with_coin()
				.max(T::WeightInfo::load_recycler_with_external_asset());
			let load_avg = load_one.saturating_mul(avg_aliases_single_ring.into());

			// Background operation: pushing keys into recycler's ring.
			let bg_recycler = bg_per_key.saturating_mul(avg_aliases_single_ring.into());

			// === Phase 3: Unloading ===
			// Unload recycler with average number of items.
			let unload_avg =
				Self::unload_recycler_into_coin_weight(avg_aliases_single_ring as usize)
					.max(Self::unload_recycler_into_external_asset_and_vouchers_weight(
						avg_aliases_single_ring as usize,
						avg_split_outputs as usize,
					))
					.max(Self::unload_recycler_into_external_asset_non_anonymous_weight(
						avg_aliases_single_ring as usize,
					))
					.max(Self::unload_recycler_into_coins_weight(
						avg_aliases_single_ring as usize,
						avg_split_outputs,
					));

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

		pub(crate) fn paid_unload_token_fee_in_asset(
		) -> Result<FungiblesBalanceOf<T>, CannotConvertNativeToAssetError> {
			let fee = Self::paid_unload_token_fee_in_native();
			let asset_id = UnderlyingAssetId::<T>::get()
				.ok_or(CannotConvertNativeToAssetError::AssetIdNotSet)?;
			T::ConversionToAssetBalance::to_asset_balance(fee, asset_id)
				.map_err(|_| CannotConvertNativeToAssetError::ConversionFailed)
		}
	}
}

/// Helper trait for runtime benchmarks.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper<T: Config> {
	/// Setup the underlying external asset and populate [`UnderlyingAssetId`] storage with it.
	///
	/// Benchmarks expect `Pallet::<T>::underlying_asset_id()` to succeed after this runs.
	fn setup_assets();
	/// Fund an account with fungibles balance.
	fn fund_account(who: &T::AccountId, amount: FungiblesBalanceOf<T>);
	/// Set the current time (needed because benchmarks run at genesis where timestamp is 0).
	fn set_time(now: core::time::Duration);
	/// Setup the conversion rate from underlying asset to native (for native fee payment).
	fn setup_conversion_rate();
	/// Create a people proof for the given context, message, and alias.
	fn create_people_proof(
		context: &[u8],
		msg: &[u8],
		alias: Alias,
	) -> <T::PeopleProof as ValidateProof>::Proof;
	/// Create a lite people proof for the given context, message, and alias.
	fn create_lite_people_proof(
		context: &[u8],
		msg: &[u8],
		alias: Alias,
	) -> <T::LitePeopleProof as ValidateProof>::Proof;

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
		(CoinValue, RingIndex, Vec<Alias>, Vec<ProofOf<T>>, [u8; 32]),
		frame_benchmarking::BenchmarkError,
	> {
		let _ = count;
		Err(frame_benchmarking::BenchmarkError::Weightless)
	}
}
