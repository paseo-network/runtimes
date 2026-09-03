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

//! PGAS-origin transaction extension.
//!
//! [`AsPgas`] verifies a ring-VRF membership proof, records the (day, alias) for uniqueness,
//! and mutates the outer origin into [`Origin::ClaimAlias`] so that
//! [`Call::claim_pgas`](crate::Call::claim_pgas) can mint to the requested `target` without
//! requiring the caller to be a signed account on the chain hosting this pallet (typically
//! Asset Hub).

use crate::{
	pallet::Day, weights::WeightInfo as _, ClaimedGasAliases, Config, Origin, Pallet, ProofOf,
};
use codec::{Decode, DecodeWithMemTracking, Encode};
use core::fmt;
use frame_support::{
	ensure,
	pallet_prelude::TransactionSource,
	traits::{fungibles::Inspect, IsSubType, OriginTrait},
	weights::Weight,
	CloneNoBound, DefaultNoBound, EqNoBound, PartialEqNoBound,
};
use indiv_support::{
	traits::{
		MembershipProver, RevisionIndex, RingIndex, PEOPLE_IDENTIFIER, PEOPLE_LITE_IDENTIFIER,
	},
	tx_priority,
};
use scale_info::TypeInfo;
use sp_crypto_hashing::twox_64;
use sp_runtime::{
	traits::{DispatchInfoOf, Get, TransactionExtension, ValidateResult},
	transaction_validity::{InvalidTransaction, TransactionValidityError, ValidTransaction},
};

/// Custom invalidity reasons surfaced by this pallet's transaction extension and authorize
/// closures. Codes are kept pallet-unique so clients can distinguish each failure mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CustomValidity {
	/// `slot_index` is outside the collection-specific range.
	InvalidClaimSlot = 230,
	/// The `day` the caller built the proof for is neither the current day nor (while the grace
	/// window is open) the previous day.
	InvalidClaimDay = 231,
	/// The (day, alias) pair has already been used this period.
	AlreadyClaimed = 232,
	/// The PGAS asset has not been created yet — `create_pgas_asset` must be called first.
	PgasAssetNotCreated = 233,
	/// The `first_alias` passed to `clean_pgas_claim_records` does not match the first alias
	/// currently stored under the given `day_index` prefix — the prefix state has moved on
	/// since the caller constructed the call.
	FirstAliasMismatch = 234,
}

impl From<CustomValidity> for TransactionValidityError {
	fn from(e: CustomValidity) -> Self {
		InvalidTransaction::Custom(e as u8).into()
	}
}

/// Selects which ring collection a PGAS claim proof targets.
///
/// [`PgasCollection::People`] uses [`PEOPLE_IDENTIFIER`]; [`PgasCollection::LitePeople`] uses
/// [`PEOPLE_LITE_IDENTIFIER`]. Slot bounds differ per collection — see
/// [`Config::MaxClaimsPerPeriodPerPerson`] and [`Config::MaxClaimsPerPeriodPerLitePerson`].
#[derive(
	Encode,
	Decode,
	DecodeWithMemTracking,
	Clone,
	Copy,
	PartialEq,
	Eq,
	Debug,
	TypeInfo,
	codec::MaxEncodedLen,
)]
pub enum PgasCollection {
	/// Full personhood.
	People,
	/// Lite personhood.
	LitePeople,
}

impl PgasCollection {
	/// The collection's ring identifier.
	pub fn identifier(&self) -> indiv_support::traits::Identifier {
		match self {
			PgasCollection::People => *PEOPLE_IDENTIFIER,
			PgasCollection::LitePeople => *PEOPLE_LITE_IDENTIFIER,
		}
	}
}

/// Information required to validate a PGAS claim proof and mutate the origin.
///
/// Proof construction:
/// - `collection` selects which ring collection the proof targets.
/// - `ring_index` must point to the ring used to create the proof.
/// - `revision` must be the specific ring revision the proof was built against. The extension
///   verifies via [`MembershipProver::verify_membership`]. Callers are expected to know their
///   proof's revision (readable from the prover via [`MembershipProver::ring_revision`]).
/// - `day` is the day index the proof's context was built against; must equal either the current
///   day or (while within [`crate::pallet::PGAS_DAY_GRACE_WINDOW`]) the previous day.
/// - The proof context is derived on-chain from `day` and the call's `slot_index` via
///   [`Pallet::build_gas_context`].
/// - The proof message is `blake2_256(scale_encode(inherited_implication))`.
#[derive(
	Encode, Decode, TypeInfo, EqNoBound, CloneNoBound, PartialEqNoBound, DecodeWithMemTracking,
)]
#[scale_info(skip_type_params(T))]
pub enum AsPgasInfo<T: Config> {
	/// Claim PGAS for the specified ring collection.
	Claim {
		proof: ProofOf<T>,
		ring_index: RingIndex,
		revision: RevisionIndex,
		collection: PgasCollection,
		day: u32,
	},
}

/// Extension that validates a PGAS claim proof and replaces the outer origin with
/// [`Origin::ClaimAlias`].
#[derive(
	Encode,
	Decode,
	TypeInfo,
	EqNoBound,
	CloneNoBound,
	PartialEqNoBound,
	DefaultNoBound,
	DecodeWithMemTracking,
)]
#[scale_info(skip_type_params(T))]
pub struct AsPgas<T: Config>(Option<AsPgasInfo<T>>);

impl<T: Config> fmt::Debug for AsPgas<T> {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "AsPgas")
	}
}

impl<T: Config> AsPgas<T> {
	pub fn new(explicit: Option<AsPgasInfo<T>>) -> Self {
		Self(explicit)
	}

	fn validate_claim(
		origin: <T as frame_system::Config>::RuntimeOrigin,
		call: &<T as frame_system::Config>::RuntimeCall,
		inherited_implication: &impl Encode,
		proof: &ProofOf<T>,
		ring_index: RingIndex,
		revision: RevisionIndex,
		collection: PgasCollection,
		day: u32,
	) -> ValidateResult<(), <T as frame_system::Config>::RuntimeCall> {
		ensure!(
			matches!(origin.as_system_ref(), Some(frame_system::RawOrigin::None)),
			InvalidTransaction::BadSigner
		);

		let slot_index: u32 = match call.is_sub_type() {
			Some(crate::Call::<T>::claim_pgas { slot_index, .. }) => *slot_index,
			_ => return Err(InvalidTransaction::Call.into()),
		};

		ensure!(
			slot_index < Pallet::<T>::max_claims_for(collection),
			CustomValidity::InvalidClaimSlot
		);

		// Reject early if the PGAS asset has not been created yet — minting in dispatch would
		// fail anyway, and we'd rather not run proof verification for a claim that can't settle.
		ensure!(
			T::Fungibles::asset_exists(T::PgasAssetId::get()),
			CustomValidity::PgasAssetNotCreated
		);

		// Accept only the current day, or the previous day while still within the grace window.
		// When outside the grace window, `grace_day() == current_day()`.
		let current_day = Pallet::<T>::current_day();
		let grace_day = Pallet::<T>::grace_day();
		ensure!(day == current_day || day == grace_day, CustomValidity::InvalidClaimDay);

		let identifier = collection.identifier();
		let context = Pallet::<T>::build_gas_context(day, slot_index);
		let msg = inherited_implication.using_encoded(sp_io::hashing::blake2_256);

		let ca = T::MembershipProver::verify_membership(
			&identifier,
			proof,
			ring_index,
			revision,
			context,
			&msg[..],
		)
		.map_err(|_| InvalidTransaction::BadProof)?;
		let alias = ca.alias;
		let day_be = Day::from(day);

		// Pool-hygiene pre-check; dispatch re-checks authoritatively.
		ensure!(
			!ClaimedGasAliases::<T>::contains_key(day_be, alias),
			CustomValidity::AlreadyClaimed
		);

		let provides = twox_64(&("pgas-slot", identifier, alias, day, slot_index).encode());
		let validity = ValidTransaction::with_tag_prefix("Pgas:Claim")
			.and_provides(provides)
			.priority(tx_priority::USER_DEFAULT);

		let local_origin = Origin::ClaimAlias { alias, day: day_be, collection };
		let mut origin = origin;
		origin.set_caller_from(local_origin);

		Ok((validity.into(), (), origin))
	}
}

impl<T: Config> TransactionExtension<<T as frame_system::Config>::RuntimeCall> for AsPgas<T> {
	const IDENTIFIER: &'static str = "AsPgas";
	type Implicit = ();
	type Val = ();
	type Pre = ();

	fn weight(&self, _call: &<T as frame_system::Config>::RuntimeCall) -> Weight {
		match self.0 {
			Some(AsPgasInfo::Claim { .. }) => <T as Config>::WeightInfo::as_pgas_claim_tx_ext(),
			None => Weight::zero(),
		}
	}

	fn validate(
		&self,
		origin: <T as frame_system::Config>::RuntimeOrigin,
		call: &<T as frame_system::Config>::RuntimeCall,
		_info: &DispatchInfoOf<<T as frame_system::Config>::RuntimeCall>,
		_len: usize,
		_self_implicit: Self::Implicit,
		inherited_implication: &impl Encode,
		_source: TransactionSource,
	) -> ValidateResult<Self::Val, <T as frame_system::Config>::RuntimeCall> {
		match &self.0 {
			Some(AsPgasInfo::Claim { proof, ring_index, revision, collection, day }) =>
				Self::validate_claim(
					origin,
					call,
					inherited_implication,
					proof,
					*ring_index,
					*revision,
					*collection,
					*day,
				),
			// Extension not in use by this transaction: pass through with default validity. The
			// effective priority comes from whichever extension authorizes the call.
			None => Ok((ValidTransaction::default(), (), origin)),
		}
	}

	fn prepare(
		self,
		_val: Self::Val,
		_origin: &<T as frame_system::Config>::RuntimeOrigin,
		_call: &<T as frame_system::Config>::RuntimeCall,
		_info: &DispatchInfoOf<<T as frame_system::Config>::RuntimeCall>,
		_len: usize,
	) -> Result<Self::Pre, TransactionValidityError> {
		Ok(())
	}
}
