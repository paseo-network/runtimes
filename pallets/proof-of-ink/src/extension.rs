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

//! Proof-of-Ink signed extension.

use crate::*;
use codec::{Decode, DecodeWithMemTracking, Encode};
use core::fmt;
use frame_support::{
	ensure, pallet_prelude::Weight, traits::IsSubType, CloneNoBound, EqNoBound, PartialEqNoBound,
};
use frame_system::{CheckNonce, ValidNonceInfo};
use scale_info::TypeInfo;
use sp_runtime::{
	traits::{DispatchInfoOf, PostDispatchInfoOf, TransactionExtension, ValidateResult},
	transaction_validity::{
		InvalidTransaction, TransactionSource, TransactionValidityError, ValidTransaction,
	},
};

/// Information required to transform an origin into respective origins supported by the
/// `AsProofOfInkParticipant` extension:
/// - `AuthorizedApplyWithSig` - authorized to apply with signature,
/// - `ReferredCandidate` - an account that was referred,
/// - `InvitedCandidate` - an account that was invited.
#[derive(
	Encode, Decode, TypeInfo, EqNoBound, CloneNoBound, PartialEqNoBound, DecodeWithMemTracking,
)]
#[scale_info(skip_type_params(T))]
pub enum AsProofOfInkParticipantInfo<T: Config + Send + Sync> {
	/// The signed origin will be transformed to `AuthorizedApplyWithSig` origin.
	/// Works only for `apply_with_signature` call. Requires the nonce of the account.
	/// Also provides for the account.
	/// Transmutes to `AuthorizedApplyWithSig` origin when:
	/// * origin is signed.
	/// * nonce is valid.
	/// * call is `apply_with_signature`.
	/// * origin is not a candidate yet,
	/// * referrer is not banned,
	/// * referrer has some design,
	/// * referral ticket exists,
	/// * ticket signature is valid.
	AsApplyWithSig(T::Nonce),

	/// The signed origin will be transformed to `ReferredCandidate` origin.
	/// Requires the nonce of the account. Also provides for the account.
	/// Transmutes to `ReferredCandidate` origin when:
	/// * origin is signed.
	/// * nonce is correct.
	/// * origin is a referred candidate
	/// * call is a `proof_of_ink` call.
	AsReferred(T::Nonce),

	/// The signed origin will be transformed to `InvitedCandidate` origin.
	/// Requires the nonce of the account. Also provides for the account.
	/// Transmutes to `InvitedCandidate` origin when:
	/// * origin is signed.
	/// * nonce is correct.
	/// * origin is an invited candidate
	/// * call is a `proof_of_ink` call.
	AsInvited(T::Nonce),
}

/// Extension to validate the transaction and transform the origin to:
/// - `AuthorizedApplyWithSig` - authorized to apply with signature,
/// - `ReferredCandidate` - an account that was referred,
/// - `InvitedCandidate` - an account that was invited.
///
/// To disable this extension, use `None` as the explicit.
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
pub struct AsProofOfInkParticipant<T: Config + Send + Sync>(Option<AsProofOfInkParticipantInfo<T>>);

impl<T: Config + Send + Sync> fmt::Debug for AsProofOfInkParticipant<T> {
	#[cfg(feature = "std")]
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "AsProofOfInkParticipant")
	}

	#[cfg(not(feature = "std"))]
	fn fmt(&self, _: &mut fmt::Formatter) -> fmt::Result {
		Ok(())
	}
}

impl<T: Config + Send + Sync> AsProofOfInkParticipant<T> {
	pub fn new(explicit: Option<AsProofOfInkParticipantInfo<T>>) -> Self {
		Self(explicit)
	}
}

impl<T: Config + Send + Sync> TransactionExtension<<T as frame_system::Config>::RuntimeCall>
	for AsProofOfInkParticipant<T>
{
	const IDENTIFIER: &'static str = "AsProofOfInkParticipant";
	type Implicit = ();

	type Val = Option<T::AccountId>;
	type Pre = Option<T::AccountId>;

	fn weight(&self, _call: &<T as frame_system::Config>::RuntimeCall) -> Weight {
		match self.0 {
			Some(AsProofOfInkParticipantInfo::AsInvited(_)) =>
				<T as Config>::WeightInfo::as_invited_tx_ext(),
			Some(AsProofOfInkParticipantInfo::AsApplyWithSig(_)) =>
				<T as Config>::WeightInfo::as_apply_with_sig_tx_ext(),
			Some(AsProofOfInkParticipantInfo::AsReferred(_)) =>
				<T as Config>::WeightInfo::as_referred_tx_ext(),
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
		_inherited_implication: &impl Encode,
		_source: TransactionSource,
	) -> ValidateResult<Self::Val, <T as frame_system::Config>::RuntimeCall> {
		match &self.0 {
			Some(AsProofOfInkParticipantInfo::AsApplyWithSig(nonce)) => {
				let Some(frame_system::Origin::<T>::Signed(who)) = origin.as_system_ref() else {
					return Err(InvalidTransaction::Call.into());
				};

				let Some(Call::apply_with_signature { referrer, signature, ticket }) =
					call.is_sub_type()
				else {
					return Err(InvalidTransaction::Call.into());
				};

				ensure!(!Candidates::<T>::contains_key(who), InvalidTransaction::BadSigner);

				let referrer_record =
					People::<T>::get(referrer).ok_or(InvalidTransaction::BadProof)?;
				ensure!(!referrer_record.banned, InvalidTransaction::BadProof);

				// If the referral is successful, the ticket will be removed from storage during
				// call dispatch.
				let tickets =
					ReferralTickets::<T>::get(referrer).ok_or(InvalidTransaction::BadProof)?;

				let ticket_valid = tickets
					.iter()
					.find(|t| &t.ticket == ticket)
					.map(|t| who.using_encoded(|bytes| signature.verify(bytes, &t.ticket)));

				ensure!(ticket_valid.is_some_and(|v| v), InvalidTransaction::BadProof);

				// Immutably increment the provider reference, just for validation purposes.
				frame_system::Pallet::<T>::inc_providers(who);
				let ValidNonceInfo { requires, provides } =
					CheckNonce::<T>::validate_nonce_for_account(who, *nonce)?;
				let validity = ValidTransaction { requires, provides, ..Default::default() };
				Ok((
					validity,
					Some(who.clone()),
					Origin::AuthorizedApplyWithSig(who.clone()).into(),
				))
			},
			Some(AsProofOfInkParticipantInfo::AsReferred(nonce)) => {
				let Some(who) = origin.as_signer().cloned() else {
					return Err(InvalidTransaction::Call.into());
				};

				let Some(candidate) = Candidates::<T>::get(&who) else {
					return Err(InvalidTransaction::Call.into());
				};

				// Only allow for call into proof of ink
				ensure!(
					IsSubType::<Call<T>>::is_sub_type(call).is_some(),
					InvalidTransaction::Call
				);

				ensure!(
					matches!(
						candidate,
						Candidate::Applied { cred: Credibility::Referred(_), .. } |
							Candidate::Selected { cred: Credibility::Referred(_), .. } |
							Candidate::Proven { was_referred: true, .. }
					),
					InvalidTransaction::Call
				);

				let ValidNonceInfo { requires, provides } =
					CheckNonce::<T>::validate_nonce_for_account(&who, *nonce)?;
				let validity = ValidTransaction { requires, provides, ..Default::default() };
				origin.set_caller(Origin::ReferredCandidate(who.clone()).into());
				Ok((validity, Some(who), origin))
			},
			Some(AsProofOfInkParticipantInfo::AsInvited(nonce)) => {
				let Some(who) = origin.as_signer().cloned() else {
					return Err(InvalidTransaction::Call.into());
				};

				if let Some(Call::apply_with_invitation { inviter, ticket, signature }) =
					call.is_sub_type()
				{
					ensure!(
						!Candidates::<T>::contains_key(who.clone()),
						InvalidTransaction::BadSigner
					);
					ensure!(
						who.clone().using_encoded(|bytes| signature.verify(bytes, ticket)),
						InvalidTransaction::BadSigner,
					);
					ensure!(
						PendingInvites::<T>::get(inviter, ticket).is_some(),
						InvalidTransaction::Call
					);

					frame_system::Pallet::<T>::inc_providers(&who);
				} else {
					// Only allow for calls to proof of ink
					ensure!(
						IsSubType::<Call<T>>::is_sub_type(call).is_some(),
						InvalidTransaction::Call
					);

					ensure!(
						matches!(
							Candidates::<T>::get(who.clone()),
							Some(Candidate::Applied { cred: Credibility::Invited(_), .. }) |
								Some(Candidate::Selected { cred: Credibility::Invited(_), .. }) |
								Some(Candidate::Proven { was_invited: true, .. })
						),
						InvalidTransaction::BadSigner
					);
				};

				let ValidNonceInfo { requires, provides } =
					CheckNonce::<T>::validate_nonce_for_account(&who, *nonce)?;
				let validity = ValidTransaction { requires, provides, ..Default::default() };
				Ok((validity, Some(who.clone()), Origin::InvitedCandidate(who.clone()).into()))
			},
			None => Ok((ValidTransaction::default(), None, origin)),
		}
	}

	fn prepare(
		self,
		val: Self::Val,
		_origin: &<T as frame_system::Config>::RuntimeOrigin,
		call: &<T as frame_system::Config>::RuntimeCall,
		_info: &DispatchInfoOf<<T as frame_system::Config>::RuntimeCall>,
		_len: usize,
	) -> Result<Self::Pre, TransactionValidityError> {
		if let Some(account) = val {
			// defensive, should already been checked in `validate`.
			let nonce = match &self.0 {
				None => None,
				Some(AsProofOfInkParticipantInfo::AsApplyWithSig(nonce)) => Some(*nonce),
				Some(AsProofOfInkParticipantInfo::AsReferred(nonce)) => Some(*nonce),
				Some(AsProofOfInkParticipantInfo::AsInvited(nonce)) => Some(*nonce),
			};
			let nonce = nonce.ok_or(InvalidTransaction::BadProof)?;
			CheckNonce::<T>::prepare_nonce_for_account(&account, nonce)?;

			let should_dec_providers_post_dispatch: bool = match &self.0 {
				Some(AsProofOfInkParticipantInfo::AsApplyWithSig(_)) => true,
				Some(AsProofOfInkParticipantInfo::AsInvited(_)) =>
					matches!(call.is_sub_type(), Some(Call::apply_with_invitation { .. })),
				_ => false,
			};

			Ok(should_dec_providers_post_dispatch.then_some(account))
		} else {
			Ok(None)
		}
	}

	fn post_dispatch_details(
		pre: Self::Pre,
		_info: &DispatchInfoOf<T::RuntimeCall>,
		_post_info: &PostDispatchInfoOf<T::RuntimeCall>,
		_len: usize,
		_result: &sp_runtime::DispatchResult,
	) -> Result<Weight, TransactionValidityError> {
		if let Some(account) = pre {
			// Take back the provider reference. It's fine because the actual call dispatch, in case
			// it was successful, will do an `inc_sufficients` for the caller, which will keep
			// the account alive.
			let _ = frame_system::Pallet::<T>::dec_providers(&account);
		}

		Ok(Weight::zero())
	}
}
