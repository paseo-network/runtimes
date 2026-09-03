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

//! Coinage transaction extension.

use crate::*;
use codec::{Decode, DecodeWithMemTracking, Encode};
use frame_support::{
	pallet_prelude::TransactionSource, weights::Weight, BoundedVec, CloneNoBound, DebugNoBound,
	DefaultNoBound, EqNoBound, PartialEqNoBound,
};
use frame_system::{CheckNonce, ValidNonceInfo};
use scale_info::TypeInfo;
use sp_io::hashing::{blake2_256, twox_64};
use sp_runtime::{
	traits::{
		DispatchInfoOf, PostDispatchInfoOf, Saturating, TransactionExtension, ValidateResult,
	},
	transaction_validity::{InvalidTransaction, TransactionValidityError, ValidTransaction},
};

/// The inner information of the coinage transaction extension. Allows authentication as
/// the origin [Origin::Coin] or [Origin::UnloadToken].
// For unloading variants the alias proofs are included in the transaction extension so that they
// sign for the specific sub-intent. But because they are not validated in the transaction
// extension itself, the proof that carries the complete intent and the payment must include
// the alias proofs in its signed message so they can't be tampered with.
// Having alias proofs in the transaction extension is also for future optimization when we
// validate proofs during the transaction validation and batch all proofs at the block level.
#[derive(
	Encode,
	Decode,
	TypeInfo,
	EqNoBound,
	CloneNoBound,
	PartialEqNoBound,
	DecodeWithMemTracking,
	DebugNoBound,
)]
#[scale_info(skip_type_params(T))]
pub enum AsCoinageInfo<T: Config + Send + Sync> {
	/// Transmute the signed origin into [Origin::Coin] for the calls: [Call::split],
	/// [Call::transfer], [Call::load_recycler_with_coin].
	AsCoin,
	/// Transmute the None origin into [Origin::UnloadToken] for the calls:
	/// [Call::unload_recycler_into_coin], [Call::unload_recycler_into_external_asset],
	/// [Call::unload_recycler_into_external_asset_and_vouchers], and
	/// [Call::unload_recycler_into_coins].
	AsUnloadTokenPeople {
		/// The proof of personhood, proving the message
		/// `alias_proofs.encode() ++ inherited_implication` hashed with blake2_256 in the
		/// context: `"pop:polkadot.net/coinftk" ++ period ++ counter` with period and counter
		/// as LE-encoded unsigned 32-bit integers.
		proof: <T::PeopleProof as ValidateProof>::Proof,
		/// The period of the unload token.
		period: Period,
		/// The counter of the unload token.
		counter: u32,
		/// The alias proofs for the aliases of the coins consolidated, proving the message
		/// `inherited_implication` hashed with blake2_256 in the context:
		/// `"pop:polkadot.network/coinrecyclr"`.
		///
		/// The maximum number of proofs is bounded by [Config::MaxConsolidation].
		alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation>,
	},
	/// Transmute the None origin into [Origin::UnloadToken] for the calls:
	/// [Call::unload_recycler_into_coin], [Call::unload_recycler_into_external_asset],
	/// [Call::unload_recycler_into_external_asset_and_vouchers], and
	/// [Call::unload_recycler_into_coins].
	AsUnloadTokenLitePeople {
		/// The proof of lite personhood, proving the message
		/// `alias_proofs.encode() ++ inherited_implication` hashed with blake2_256 in the
		/// context: `"pop:polkadot.net/coinftk" ++ period ++ counter` with period and counter
		/// as LE-encoded unsigned 32-bit integers.
		proof: <T::LitePeopleProof as ValidateProof>::Proof,
		/// The period of the unload token.
		period: Period,
		/// The counter of the unload token.
		counter: u32,
		/// The alias proofs for the aliases of the coins consolidated, proving the message
		/// `inherited_implication` hashed with blake2_256 in the context:
		/// `"pop:polkadot.network/coinrecyclr"`.
		///
		/// The maximum number of proofs is bounded by [Config::MaxConsolidation].
		alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation>,
	},
	/// Transmute the None origin into [Origin::UnloadToken] for the calls:
	/// [Call::unload_recycler_into_coin], [Call::unload_recycler_into_external_asset],
	/// [Call::unload_recycler_into_external_asset_and_vouchers], and
	/// [Call::unload_recycler_into_coins].
	AsUnloadTokenPaid {
		/// The proof from the paid unload token ring, proving the message
		/// `alias_proofs.encode() ++ inherited_implication` hashed with blake2_256 in the
		/// context: `"pop:polkadot.net/coinpaidtok" ++ period` with period as an LE-encoded
		/// unsigned 32-bit integer.
		proof: ProofOf<T>,
		/// The period of the unload token.
		period: Period,
		/// The index of the paid unload token ring.
		paid_token_ring_index: u32,
		/// The revision of the paid unload token ring.
		paid_token_ring_revision: u32,
		/// The alias proofs for the aliases of the coins consolidated, proving the message
		/// `inherited_implication` hashed with blake2_256 in the context:
		/// `"pop:polkadot.network/coinrecyclr"`.
		///
		/// The maximum number of proofs is bounded by [Config::MaxConsolidation].
		alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation>,
	},
	/// Transmute the None origin into [Origin::UnloadToken] with [UnloadFee::FromOutput] for the
	/// calls [Call::unload_recycler_into_external_asset],
	/// [Call::unload_recycler_into_external_asset_and_vouchers], and
	/// [Call::unload_recycler_into_coins].
	/// The fee is deducted from the unloaded assets.
	///
	/// If the transaction fails then the first alias is consumed as penalty.
	///
	/// The first alias proof is validated in the extension for spam protection and marked as
	/// unloaded (penalty if call fails). The coin value for this first alias
	/// (`fee_recycler_value`) must be at least [Config::MinimumExponentForOutputUnloadFee] to
	/// cover the weight of a failed unload, but doesn't need to cover the full unload token fee
	/// (which is deducted from the total output). The call skips re-validation for the first
	/// alias, only checking that it matches the `first_alias` from the origin.
	AsUnloadTokenFromOutput {
		/// The coin value of the recycler for the first alias (fee coin).
		fee_recycler_value: CoinValue,
		/// The recycler index for the first alias.
		fee_recycler_index: RingIndex,
		/// The recycler revision for the first alias.
		fee_recycler_revision: RevisionIndex,
		/// All alias proofs including the first one.
		/// The first proof signs `alias_proofs[1..].encode() ++ inherited_implication` hashed
		/// with blake2_256 in the context: `"pop:polkadot.network/coinrecyclr"`.
		/// It is validated in the extension for spam protection, ensuring the other proofs
		/// cannot be tampered with.
		/// The remaining proofs sign `inherited_implication` hashed with blake2_256 in the
		/// same context and are validated in the call.
		alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation>,
	},
	/// Transmute the signed origin into [Origin::InfallibleUnpaidSigned] for supported calls such
	/// as [Call::load_recycler_with_external_asset].
	///
	/// All the check asserting the call is successful are done during validation. And the call
	/// doesn't pay fee on success.
	InfallibleUnpaidSigned {
		/// The nonce of the signer's account, used for replay protection.
		nonce: T::Nonce,
	},
}

/// The coinage transaction extension. Allows authentication as the origin [Origin::Coin]
/// or [Origin::UnloadToken].
#[derive(
	Encode,
	Decode,
	TypeInfo,
	EqNoBound,
	CloneNoBound,
	PartialEqNoBound,
	DefaultNoBound,
	DecodeWithMemTracking,
	DebugNoBound,
)]
#[scale_info(skip_type_params(T))]
pub struct AsCoinage<T: Config + Send + Sync>(Option<AsCoinageInfo<T>>);

impl<T: Config + Send + Sync> AsCoinage<T> {
	pub fn new(explicit: Option<AsCoinageInfo<T>>) -> Self {
		Self(explicit)
	}

	/// Calculate validation parameters `(revision_checks, output_checks)` for unload calls.
	/// Used by `validate_unload_calls` weight calculation.
	fn unload_call_validation_params(
		call: &<T as frame_system::Config>::RuntimeCall,
	) -> (u32, u32) {
		match call.is_sub_type() {
			Some(Call::<T>::unload_recycler_into_coin { .. }) => (1, 1),
			Some(Call::<T>::unload_recycler_into_coins { split_into, .. }) => {
				let destinations: u32 =
					split_into.iter().map(|(_, dests)| dests.len() as u32).sum();
				(1, destinations)
			},
			Some(Call::<T>::unload_recycler_into_external_asset_and_vouchers {
				new_vouchers,
				..
			}) => (1, new_vouchers.len() as u32),
			Some(Call::<T>::unload_recycler_into_external_asset { .. }) => (1, 0),
			// Default for unknown/invalid calls - will fail validation anyway
			_ => (1, 0),
		}
	}
}

#[derive(Clone, Copy)]
pub enum FreeTokenKind {
	People,
	LitePeople,
}

/// The type used to transfer information from validation to the preparation phase in the coinage
/// transaction extension.
pub enum Val<T: Config + Send + Sync> {
	NotUsing,
	UsingCoin {
		coin_id: T::AccountId,
	},
	UsingFreeToken {
		token: Alias,
		period: Period,
		kind: FreeTokenKind,
	},
	UsingPaidToken {
		token: Alias,
		period: Period,
		ring_index: u32,
	},
	/// Using output token - first alias validated, fee coin covers fee.
	UsingOutputToken {
		first_alias: Alias,
		fee_recycler_value: CoinValue,
		fee_recycler_index: RingIndex,
	},
	/// Using the infallible unpaid extension. All checks passed in validate; prepare will
	/// commit the nonce.
	UsingInfallibleUnpaidSigned {
		who: T::AccountId,
		nonce: T::Nonce,
	},
}

/// The type used to transfer information from preparation to the post-dispatch phase in the coinage
/// transaction extension.
pub enum Pre<T: Config + Send + Sync> {
	NotUsing,
	UsingCoin {
		coin_id: T::AccountId,
		coin: Coin,
	},
	UsingFreeToken,
	UsingPaidToken,
	/// Using output token - first alias marked as unloaded in prepare (penalty if call fails).
	UsingOutputToken {
		fee_recycler_value: CoinValue,
	},
	/// Using the infallible unpaid extension.
	UsingInfallibleUnpaidSigned,
}

impl<T: Config + Send + Sync> TransactionExtension<<T as frame_system::Config>::RuntimeCall>
	for AsCoinage<T>
{
	const IDENTIFIER: &'static str = "AsCoinage";
	type Implicit = ();

	type Val = Val<T>;
	type Pre = Pre<T>;

	fn weight(&self, call: &<T as frame_system::Config>::RuntimeCall) -> Weight {
		match &self.0 {
			None => match call.is_sub_type() {
				Some(Call::<T>::unload_recycler_into_external_asset_non_anonymous { .. }) =>
					T::WeightInfo::as_none_tx_ext_unload_recycler_into_external_asset_non_anonymous(
					),
				Some(Call::<T>::unload_recyclers_into_external_asset_non_anonymous {
					inputs,
					..
				}) =>
					T::WeightInfo::as_none_tx_ext_unload_recyclers_into_external_asset_non_anonymous(
						inputs.len() as u32,
					),
				_ => T::WeightInfo::as_none_tx_ext_others(),
			},
			Some(AsCoinageInfo::AsCoin) => match call.is_sub_type() {
				Some(Call::<T>::split { split_into }) => {
					let n =
						split_into.iter().map(|(_, dests)| dests.len() as u32).sum::<u32>().max(1);
					T::WeightInfo::as_coin_split(n)
				},
				Some(Call::<T>::transfer { .. }) => T::WeightInfo::as_coin_transfer(),
				Some(Call::<T>::load_recycler_with_coin { .. }) =>
					T::WeightInfo::as_coin_load_recycler_with_coin(),
				Some(Call::<T>::pay_for_recycler_unload_fee_token_with_coin { .. }) =>
					T::WeightInfo::as_coin_pay_for_recycler_unload_fee_token_with_coin(),
				// Default for unknown/invalid calls - use max of other cases
				_ => T::WeightInfo::as_coin_transfer()
					.max(T::WeightInfo::as_coin_load_recycler_with_coin())
					.max(T::WeightInfo::as_coin_pay_for_recycler_unload_fee_token_with_coin()),
			},
			Some(AsCoinageInfo::AsUnloadTokenPeople { .. }) => {
				let (r, d) = Self::unload_call_validation_params(call);
				T::WeightInfo::as_unload_token_people_tx_ext()
					.saturating_add(T::WeightInfo::validate_unload_calls(r, d))
			},
			Some(AsCoinageInfo::AsUnloadTokenLitePeople { .. }) => {
				let (r, d) = Self::unload_call_validation_params(call);
				T::WeightInfo::as_unload_token_lite_people_tx_ext()
					.saturating_add(T::WeightInfo::validate_unload_calls(r, d))
			},
			Some(AsCoinageInfo::AsUnloadTokenPaid { .. }) => {
				let (r, d) = Self::unload_call_validation_params(call);
				T::WeightInfo::as_unload_token_paid_tx_ext()
					.saturating_add(T::WeightInfo::validate_unload_calls(r, d))
			},
			Some(AsCoinageInfo::AsUnloadTokenFromOutput { .. }) => {
				let (r, d) = Self::unload_call_validation_params(call);
				T::WeightInfo::as_unload_token_from_output_tx_ext()
					.saturating_add(T::WeightInfo::validate_unload_calls(r, d))
			},
			Some(AsCoinageInfo::InfallibleUnpaidSigned { .. }) => {
				// `n × as_infallible_unpaid_tx_ext()` is a safe over-estimate for the batch
				// case: the single-item benchmark folds in once-per-tx work (the
				// `reducible_balance` reads and the `CheckNonce` read/write) that the batch
				// validator only does once regardless of `n`. Multiplying by `n` therefore
				// charges those reads/writes `n` times when only one happens. Per-item work
				// (member-key lookup + signature verification) is correctly scaled. If this
				// over-charge ever matters, split the benchmark into a fixed overhead and a
				// per-item increment, or add a dedicated `_batch(n)` benchmark.
				let n = match call.is_sub_type() {
					Some(Call::<T>::load_recycler_with_external_asset_unpaid_batch { items }) =>
						(items.len() as u64).max(1),
					_ => 1,
				};
				T::WeightInfo::as_infallible_unpaid_tx_ext().saturating_mul(n)
			},
		}
	}

	fn validate(
		&self,
		mut origin: <T as frame_system::Config>::RuntimeOrigin,
		call: &<T as frame_system::Config>::RuntimeCall,
		_info: &DispatchInfoOf<<T as frame_system::Config>::RuntimeCall>,
		_len: usize,
		_self_implicit: Self::Implicit,
		inherited_implication: &impl Encode,
		_source: TransactionSource,
	) -> ValidateResult<Self::Val, <T as frame_system::Config>::RuntimeCall> {
		// Global gate: reject every Coinage call (top-level) when the underlying asset id
		// is unset. `set_underlying_asset_id` is exempt — otherwise pallet could never
		// be brought online. Nested calls dispatched via utility/proxy/multisig
		// bypass this and are caught by the dispatch-handler's `Self::underlying_asset_id()`
		// lookup as a fallback.
		if let Some(coinage_call) = call.is_sub_type() {
			if !matches!(coinage_call, Call::<T>::set_underlying_asset_id { .. }) &&
				!UnderlyingAssetId::<T>::exists()
			{
				return Err(CustomInvalidity::AssetIdNotSet.into());
			}
		}

		match &self.0 {
			None => {
				// For signed non-anonymous unload calls, validate recycler revisions early
				// to protect users from paying fees when proofs are outdated.
				Pallet::<T>::validate_non_anonymous_unload_calls_revisions(call)?;
				Ok((ValidTransaction::default(), Val::NotUsing, origin))
			},
			Some(AsCoinageInfo::AsCoin) => {
				let Some(frame_system::Origin::<T>::Signed(coin_id)) = origin.as_system_ref()
				else {
					return Err(CustomInvalidity::OriginToAsCoinMustBeSigned.into());
				};
				let coin_id = coin_id.clone();
				let current_time = T::UnixTime::now().as_secs();
				if let Some(locked_coin) = LockedCoins::<T>::get(&coin_id) {
					if current_time < locked_coin.until {
						return Err(CustomInvalidity::CoinTemporarilyLocked.into());
					}
				}

				let coin = CoinsByOwner::<T>::get(&coin_id).ok_or(CustomInvalidity::NoCoin)?;

				match call.is_sub_type() {
					Some(Call::<T>::split { split_into }) => {
						Pallet::<T>::validate_split(&coin, split_into)?;
					},
					Some(Call::<T>::transfer { to }) => {
						Pallet::<T>::validate_transfer(&coin, to)?;
					},
					Some(Call::<T>::direct_offboard_coin_into_external_asset { to }) => {
						Pallet::<T>::validate_direct_offboard_coin_into_external_asset(&coin, to)?;
					},
					Some(Call::<T>::load_recycler_with_coin { member_key, proof_of_ownership }) => {
						Pallet::<T>::validate_load_recycler_with_coin(
							&coin,
							&coin_id,
							member_key,
							proof_of_ownership,
						)?;
					},
					Some(Call::<T>::pay_for_recycler_unload_fee_token_with_coin {
						member_key,
						proof_of_ownership,
					}) => {
						Pallet::<T>::validate_pay_for_recycler_unload_fee_token_with_coin(
							&coin,
							&coin_id,
							member_key,
							proof_of_ownership,
						)?;
					},
					_ => {
						return Err(CustomInvalidity::InvalidCall.into());
					},
				}

				let provides = twox_64(&("operate", &coin_id).encode()[..]);
				let validity =
					ValidTransaction::with_tag_prefix("Coinage:coin").and_provides(provides).into();
				origin.set_caller_from(Origin::Coin { coin_id: coin_id.clone(), coin });
				Ok((validity, Val::UsingCoin { coin_id }, origin))
			},
			Some(AsCoinageInfo::AsUnloadTokenPeople { proof, period, counter, alias_proofs }) => {
				Pallet::<T>::validate_unload_calls(call, UnloadFee::Prepaid)?;

				let current_periods = Pallet::<T>::current_free_unload_token_periods();
				if !current_periods.contains(period) {
					return Err(CustomInvalidity::InvalidUnloadTokenPeriod.into());
				}
				let limit = Pallet::<T>::free_unload_token_limit_for_people()?;
				if *counter >= limit {
					return Err(CustomInvalidity::UnloadTokenCounterOutOfRange.into());
				}
				let context = free_unload_token_context(*period, *counter);
				let proven_msg = inherited_implication.using_encoded(blake2_256);
				let intent_msg = (alias_proofs, inherited_implication).using_encoded(blake2_256);
				let alias = T::PeopleProof::validate_proof(proof, &context, &intent_msg)
					.map_err(|_| CustomInvalidity::InvalidUnloadTokenProof)?;
				if ConsumedFreeUnloadTokens::<T>::contains_key(period, alias) {
					return Err(CustomInvalidity::UnloadTokenAlreadyConsumed.into());
				}

				let validity = ValidTransaction::with_tag_prefix("Coinage:ppl-unload-token")
					.and_provides(alias)
					.into();
				origin.set_caller_from(Origin::UnloadToken {
					alias_proofs: alias_proofs.clone(),
					proven_msg,
					fee: UnloadFee::Prepaid,
				});
				Ok((
					validity,
					Val::UsingFreeToken {
						token: alias,
						period: *period,
						kind: FreeTokenKind::People,
					},
					origin,
				))
			},
			Some(AsCoinageInfo::AsUnloadTokenLitePeople {
				proof,
				period,
				counter,
				alias_proofs,
			}) => {
				Pallet::<T>::validate_unload_calls(call, UnloadFee::Prepaid)?;

				let current_periods = Pallet::<T>::current_free_unload_token_periods();
				if !current_periods.contains(period) {
					return Err(CustomInvalidity::InvalidUnloadTokenPeriod.into());
				}
				let limit = Pallet::<T>::free_unload_token_limit_for_lite_people()?;
				if *counter >= limit {
					return Err(CustomInvalidity::UnloadTokenCounterOutOfRange.into());
				}
				let context = free_unload_token_context(*period, *counter);
				let proven_msg = inherited_implication.using_encoded(blake2_256);
				let intent_msg = (alias_proofs, inherited_implication).using_encoded(blake2_256);
				let alias = T::LitePeopleProof::validate_proof(proof, &context, &intent_msg)
					.map_err(|_| CustomInvalidity::InvalidUnloadTokenProof)?;
				if ConsumedFreeUnloadTokens::<T>::contains_key(period, alias) {
					return Err(CustomInvalidity::UnloadTokenAlreadyConsumed.into());
				}

				let validity = ValidTransaction::with_tag_prefix("Coinage:liteppl-unload-token")
					.and_provides(alias)
					.into();
				origin.set_caller_from(Origin::UnloadToken {
					alias_proofs: alias_proofs.clone(),
					proven_msg,
					fee: UnloadFee::Prepaid,
				});
				Ok((
					validity,
					Val::UsingFreeToken {
						token: alias,
						period: *period,
						kind: FreeTokenKind::LitePeople,
					},
					origin,
				))
			},
			Some(AsCoinageInfo::AsUnloadTokenPaid {
				proof,
				period,
				paid_token_ring_index,
				paid_token_ring_revision,
				alias_proofs,
			}) => {
				Pallet::<T>::validate_unload_calls(call, UnloadFee::Prepaid)?;

				let proven_msg = inherited_implication.using_encoded(blake2_256);
				let intent_msg = (alias_proofs, inherited_implication).using_encoded(blake2_256);
				let alias = PaidTknManager::<T>::validate_token_consumption_proof(
					*period,
					*paid_token_ring_index,
					*paid_token_ring_revision,
					proof,
					&intent_msg,
				)?;

				let validity = ValidTransaction::with_tag_prefix("Coinage:paid-unload-token")
					.and_provides((period, paid_token_ring_index, alias))
					.into();

				origin.set_caller_from(Origin::UnloadToken {
					alias_proofs: alias_proofs.clone(),
					proven_msg,
					fee: UnloadFee::Prepaid,
				});
				Ok((
					validity,
					Val::UsingPaidToken {
						token: alias,
						period: *period,
						ring_index: *paid_token_ring_index,
					},
					origin,
				))
			},
			Some(AsCoinageInfo::AsUnloadTokenFromOutput {
				fee_recycler_value,
				fee_recycler_index,
				fee_recycler_revision,
				alias_proofs,
			}) => {
				// Validate unload call (FromOutput mode: only allows supported output-bearing
				// unloads).
				Pallet::<T>::validate_unload_calls(
					call,
					UnloadFee::FromOutput {
						fee_recycler_value: *fee_recycler_value,
						fee_recycler_index: *fee_recycler_index,
					},
				)?;

				// Must have at least one proof
				let first_alias_proof =
					alias_proofs.first().ok_or(CustomInvalidity::EmptyAliasProofs)?;

				// Validate fee recycler revision
				if !RecyclerManager::<T>::validate_recycler_revision(
					*fee_recycler_value,
					*fee_recycler_index,
					*fee_recycler_revision,
				) {
					return Err(CustomInvalidity::InvalidRecyclerRevision.into());
				}

				// Validate fee coin is large enough to cover the minimum penalty.
				// It does not need to cover the whole unload token fee, it only needs to cover
				// the price for a failing `unload_recycler_*`
				// The full unload token fee will be deducted from the output if the transaction
				// succeeds.
				if *fee_recycler_value < T::MinimumExponentForOutputUnloadFee::get() {
					return Err(CustomInvalidity::FeeCoinBelowMinimum.into());
				}

				// Validate the first alias proof for spam protection. It proves the full intent.
				let proven_msg = inherited_implication.using_encoded(blake2_256);
				let intent_msg =
					(&alias_proofs[1..], inherited_implication).using_encoded(blake2_256);
				let first_alias = RecyclerManager::<T>::validate_alias_proof(
					*fee_recycler_value,
					*fee_recycler_index,
					*fee_recycler_revision,
					first_alias_proof,
					&intent_msg,
				)?;

				// Validate that the first alias in the call matches the one from the proof.
				//
				// Ideally this check would live in the call itself, alongside the recycler
				// index and value check, with the validated first alias propagated through the
				// origin. For now we do it here as a simpler implementation.
				let call_first_alias = Pallet::<T>::first_alias_in_from_output_unload_call(call)
					.ok_or(CustomInvalidity::InvalidCall)?;
				if call_first_alias != first_alias {
					return Err(CustomInvalidity::FirstCallAliasMismatch.into());
				}

				let validity = ValidTransaction::with_tag_prefix("Coinage:output-unload-token")
					.and_provides((fee_recycler_value, fee_recycler_index, first_alias))
					.into();

				origin.set_caller_from(Origin::UnloadToken {
					alias_proofs: alias_proofs.clone(),
					proven_msg,
					fee: UnloadFee::FromOutput {
						fee_recycler_value: *fee_recycler_value,
						fee_recycler_index: *fee_recycler_index,
					},
				});
				Ok((
					validity,
					Val::UsingOutputToken {
						first_alias,
						fee_recycler_value: *fee_recycler_value,
						fee_recycler_index: *fee_recycler_index,
					},
					origin,
				))
			},
			Some(AsCoinageInfo::InfallibleUnpaidSigned { nonce }) => {
				let Some(frame_system::Origin::<T>::Signed(who)) = origin.as_system_ref() else {
					return Err(CustomInvalidity::InfallibleUnpaidSignedOriginMustBeSigned.into());
				};
				let who = who.clone();

				// Only allowed for `load_recycler_with_external_asset_unpaid` and its
				// `load_recycler_with_external_asset_unpaid_batch`.
				let member_keys: Vec<&MemberOf<T>> = match call.is_sub_type() {
					Some(Call::<T>::load_recycler_with_external_asset_unpaid {
						preservation,
						value,
						member_key,
						proof_of_ownership,
					}) => {
						Pallet::<T>::validate_load_recycler_with_external_asset_unpaid(
							&who,
							*preservation,
							*value,
							member_key,
							proof_of_ownership,
						)?;
						alloc::vec![member_key]
					},
					Some(Call::<T>::load_recycler_with_external_asset_unpaid_batch { items }) =>
						Pallet::<T>::validate_load_recycler_with_external_asset_unpaid_batch(
							&who, items,
						)?,
					_ => return Err(CustomInvalidity::InvalidCall.into()),
				};

				// Validate the nonce for replay protection.
				let ValidNonceInfo { requires, mut provides } =
					CheckNonce::<T>::validate_nonce_for_account(&who, *nonce)?;
				// Also tag the transaction by each `member_key` so that the transaction pool
				// deduplicates concurrent submissions reusing any of these one-shot member keys.
				for member_key in &member_keys {
					provides.push(("Coinage:infallible-unpaid-load", member_key).encode());
				}
				let validity = ValidTransaction { requires, provides, ..Default::default() };

				origin.set_caller_from(Origin::InfallibleUnpaidSigned { who: who.clone() });
				Ok((validity, Val::UsingInfallibleUnpaidSigned { who, nonce: *nonce }, origin))
			},
		}
	}

	fn prepare(
		self,
		val: Self::Val,
		_origin: &<T as frame_system::Config>::RuntimeOrigin,
		_call: &<T as frame_system::Config>::RuntimeCall,
		_info: &DispatchInfoOf<<T as frame_system::Config>::RuntimeCall>,
		_len: usize,
	) -> Result<Self::Pre, TransactionValidityError> {
		let pre = match val {
			Val::NotUsing => Pre::NotUsing,
			Val::UsingCoin { coin_id } => {
				let coin = CoinsByOwner::<T>::take(&coin_id).ok_or(CustomInvalidity::NoCoin)?;

				// Coin is removed in prepare to prevent concurrent use in the same block.
				// On dispatch failure it is restored in post_dispatch.
				Pre::UsingCoin { coin_id, coin }
			},
			Val::UsingFreeToken { token, period, kind } => {
				ConsumedFreeUnloadTokens::<T>::insert(period, token, ());
				match kind {
					FreeTokenKind::People => {
						Pallet::<T>::deposit_event(Event::PeopleFreeUnloadTokenConsumed { period });
					},
					FreeTokenKind::LitePeople => {
						Pallet::<T>::deposit_event(Event::LitePeopleFreeUnloadTokenConsumed {
							period,
						});
					},
				}
				Pre::UsingFreeToken
			},
			Val::UsingPaidToken { token, period, ring_index } => {
				PaidTknManager::<T>::mark_token_consumed(period, ring_index, token);
				Pre::UsingPaidToken
			},
			Val::UsingOutputToken { first_alias, fee_recycler_value, fee_recycler_index } => {
				// Mark first alias as unloaded (penalty if call fails)
				RecyclerManager::<T>::mark_alias_unloaded(
					fee_recycler_value,
					fee_recycler_index,
					first_alias,
				);
				Pre::UsingOutputToken { fee_recycler_value }
			},
			Val::UsingInfallibleUnpaidSigned { who, nonce } => {
				CheckNonce::<T>::prepare_nonce_for_account(&who, nonce)?;
				Pre::UsingInfallibleUnpaidSigned
			},
		};

		Ok(pre)
	}

	fn post_dispatch_details(
		pre: Self::Pre,
		_info: &DispatchInfoOf<<T as frame_system::Config>::RuntimeCall>,
		_post_info: &PostDispatchInfoOf<<T as frame_system::Config>::RuntimeCall>,
		_len: usize,
		result: &sp_runtime::DispatchResult,
	) -> Result<Weight, TransactionValidityError> {
		match pre {
			Pre::NotUsing => Ok(Weight::zero()),
			Pre::UsingCoin { coin_id, coin } => {
				if result.is_err() {
					CoinsByOwner::<T>::insert(&coin_id, coin);

					// Get the new retries count: 0 on first failure, incremented on subsequent
					// failures.
					let new_retries = LockedCoins::<T>::get(&coin_id)
						.and_then(|locked| match locked.reason {
							LockReason::FailedDispatch { retries } =>
								Some(retries.saturating_add(1)),
						})
						.unwrap_or(0);

					// Calculate exponential lock time: 2^retries * lock_period.
					let base_lock = T::CoinFailureLockPeriod::get();
					let lock_duration =
						2u64.saturating_pow(u32::from(new_retries)).saturating_mul(base_lock);

					let current_time = T::UnixTime::now().as_secs();
					let lock_until = current_time.saturating_add(lock_duration);
					LockedCoins::<T>::insert(
						&coin_id,
						LockedCoin {
							reason: LockReason::FailedDispatch { retries: new_retries },
							until: lock_until,
						},
					);
				} else {
					LockedCoins::<T>::remove(&coin_id);
				}
				Ok(Weight::zero())
			},
			Pre::UsingFreeToken => Ok(Weight::zero()),
			Pre::UsingPaidToken => Ok(Weight::zero()),
			Pre::UsingOutputToken { fee_recycler_value } => {
				if result.is_err() {
					if let Ok(underlying_value) =
						Pallet::<T>::coin_value_to_asset_amount(fee_recycler_value)
							.defensive_proof("coinage: valid coin can always be converted")
					{
						TotalValueOfDestroyedCoins::<T>::mutate(|v| {
							*v = v.saturating_add(underlying_value)
						});
					}
				}
				Ok(Weight::zero())
			},
			Pre::UsingInfallibleUnpaidSigned => {
				// Dispatch should never fail: all preconditions are verified in
				// validate. If it does fail the transaction is not included in the
				// block.
				if result.is_err() {
					defensive!("coinage: infallible unpaid dispatch failed unexpectedly");
					return Err(
						InvalidTransaction::Custom(CustomInvalidity::InternalError as u8).into()
					);
				}
				Ok(Weight::zero())
			},
		}
	}
}
