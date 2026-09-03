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

//! DotNS gateway transaction extension.

use crate::{
	pallet::{self, Call, Config, Pallet},
	types::{Link, ProofOf},
	weights::WeightInfo as _,
	AccountAlias, AliasRegistration, Collection, LiteLabelOwner,
};
use codec::{Decode, DecodeWithMemTracking, Encode};
use core::fmt;
use frame_support::{
	ensure,
	pallet_prelude::TransactionSource,
	traits::{IsSubType, OriginTrait},
	weights::Weight,
	CloneNoBound, DefaultNoBound, EqNoBound, PartialEqNoBound,
};
use indiv_support::traits::RingIndex;
use scale_info::TypeInfo;
use sp_io::hashing::twox_64;
use sp_runtime::{
	traits::{DispatchInfoOf, Implication, TransactionExtension, ValidateResult, Verify},
	transaction_validity::{InvalidTransaction, TransactionValidityError, ValidTransaction},
};

/// Custom invalidity reasons surfaced by [`AsDotnsGateway`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CustomValidity {
	/// The origin must be `None` when the extension authenticates a call.
	OriginNotNone = 180,
	/// The label or link payload does not match the required format.
	InvalidName = 181,
	/// The `Link::LiteUsername` target is not owned by the `who` provided in
	/// the call.
	NotLiteLabelOwner = 182,
	/// The off-chain signature does not verify against `who` over the
	/// inherited implication.
	InvalidOffchainSignature = 183,
	/// The alias derived from the ring proof has already registered a name.
	AliasAlreadyRegistered = 184,
	/// The `who` provided in the call is already bound to a full-person
	/// registration.
	AccountAlreadyRegistered = 185,
}

impl From<CustomValidity> for TransactionValidityError {
	fn from(e: CustomValidity) -> Self {
		InvalidTransaction::Custom(e as u8).into()
	}
}

/// Authentication payload for [`AsDotnsGateway`].
#[derive(
	Encode, Decode, TypeInfo, EqNoBound, CloneNoBound, PartialEqNoBound, DecodeWithMemTracking,
)]
#[scale_info(skip_type_params(T))]
pub enum AsDotnsGatewayInfo<T: Config> {
	/// Authenticate a full-person username registration.
	///
	/// Authenticates an origin for `Call::register_name { who, label, link }` and carries:
	/// - `proof`: ring-VRF membership proof for the `People` collection. Must be generated against
	///   the message returned by `Pallet::construct_register_proof_message`.
	/// - `ring_index`: index of the ring of the person.
	/// - `signature`: signature produced by `who` over the SCALE-encoded inherited implication.
	RegisterFullName {
		proof: ProofOf<T>,
		ring_index: RingIndex,
		signature: T::AttestationSignature,
	},
}

/// Transaction extension for interacting with dotNS gateway personhood calls.
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
pub struct AsDotnsGateway<T: Config>(pub Option<AsDotnsGatewayInfo<T>>);

impl<T: Config> fmt::Debug for AsDotnsGateway<T> {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "AsDotnsGateway")
	}
}

impl<T: Config> AsDotnsGateway<T> {
	pub fn new(info: Option<AsDotnsGatewayInfo<T>>) -> Self {
		Self(info)
	}

	fn authenticate_register_full_name(
		call: &<T as frame_system::Config>::RuntimeCall,
		inherited_implication: &impl Implication,
		proof: &ProofOf<T>,
		ring_index: RingIndex,
		signature: &T::AttestationSignature,
	) -> Result<(indiv_support::traits::Alias, T::AccountId), TransactionValidityError>
	where
		<T as frame_system::Config>::RuntimeCall: IsSubType<Call<T>>,
	{
		let Some(Call::<T>::register_name { who, label, link }) = call.is_sub_type() else {
			return Err(InvalidTransaction::Call.into());
		};

		ensure!(label.is_valid_person(), CustomValidity::InvalidName);
		if let Link::LiteUsername(ref lite_label) = link {
			ensure!(lite_label.is_valid_lite(), CustomValidity::InvalidName);
			ensure!(
				LiteLabelOwner::<T>::get(lite_label).as_ref() == Some(who),
				CustomValidity::NotLiteLabelOwner,
			);
		}

		// Make sure the account isn't already used.
		ensure!(!AccountAlias::<T>::contains_key(who), CustomValidity::AccountAlreadyRegistered,);

		let sig_msg = inherited_implication.using_encoded(sp_io::hashing::blake2_256);
		ensure!(signature.verify(&sig_msg[..], who), CustomValidity::InvalidOffchainSignature,);

		// NOTE: we don't include the `inherited_implication` in the proof message because it is
		// included in the signature above, and the signer must match with `who`, and `who` is
		// included in the proof message.
		let proof_msg = Pallet::<T>::construct_register_proof_message(who, label.as_slice(), link);
		let alias = Pallet::<T>::verify_proof(&Collection::People, ring_index, proof, &proof_msg)
			.map_err(|_| InvalidTransaction::BadProof)?;

		// An alias can only register once.
		ensure!(
			!AliasRegistration::<T>::contains_key(alias),
			CustomValidity::AliasAlreadyRegistered,
		);

		Ok((alias, who.clone()))
	}
}

impl<T: Config + Send + Sync> TransactionExtension<<T as frame_system::Config>::RuntimeCall>
	for AsDotnsGateway<T>
where
	<T as frame_system::Config>::RuntimeCall: IsSubType<Call<T>>,
{
	const IDENTIFIER: &'static str = "AsDotnsGateway";
	type Implicit = ();
	type Val = ();
	type Pre = ();

	fn weight(&self, _call: &<T as frame_system::Config>::RuntimeCall) -> Weight {
		match self.0 {
			Some(AsDotnsGatewayInfo::RegisterFullName { .. }) =>
				<T as Config>::WeightInfo::as_register_full_name_tx_ext(),
			None => Weight::zero(),
		}
	}

	fn validate(
		&self,
		mut origin: <T as frame_system::Config>::RuntimeOrigin,
		call: &<T as frame_system::Config>::RuntimeCall,
		_info: &DispatchInfoOf<<T as frame_system::Config>::RuntimeCall>,
		_len: usize,
		_self_implicit: Self::Implicit,
		inherited_implication: &impl Implication,
		_source: TransactionSource,
	) -> ValidateResult<Self::Val, <T as frame_system::Config>::RuntimeCall> {
		match &self.0 {
			Some(AsDotnsGatewayInfo::RegisterFullName { proof, ring_index, signature }) => {
				ensure!(
					matches!(origin.as_system_ref(), Some(frame_system::RawOrigin::None)),
					CustomValidity::OriginNotNone,
				);

				let (alias, who) = Self::authenticate_register_full_name(
					call,
					inherited_implication,
					proof,
					*ring_index,
					signature,
				)?;

				let provides = twox_64(&("DotnsGateway:Register", alias, &who).encode());
				let validity = ValidTransaction::with_tag_prefix("DotnsGateway:Register")
					.and_provides(provides);

				origin.set_caller_from(pallet::Origin::PersonRegistration(alias));

				Ok((validity.into(), (), origin))
			},
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
