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

//! Transaction extensions for indiv-pallet-game.

use crate::*;
use codec::{Decode, DecodeWithMemTracking, Encode};
use frame_support::{
	defensive, ensure,
	pallet_prelude::Weight,
	traits::{IsSubType, UnixTime},
	CloneNoBound, DebugNoBound, EqNoBound, PartialEqNoBound,
};
use frame_system::{CheckNonce, ValidNonceInfo};
use scale_info::TypeInfo;
use sp_runtime::{
	traits::{DispatchInfoOf, PostDispatchInfoOf, TransactionExtension, ValidateResult},
	transaction_validity::{
		InvalidTransaction, TransactionSource, TransactionValidityError, ValidTransaction,
	},
};

/// A type alias to easily access system runtime call.
type RuntimeCallOf<T> = <T as frame_system::Config>::RuntimeCall;

/// The data for the transaction extension [`GameAsInvited`].
#[derive(
	CloneNoBound,
	EqNoBound,
	PartialEqNoBound,
	Encode,
	Decode,
	TypeInfo,
	DecodeWithMemTracking,
	DebugNoBound,
)]
#[scale_info(skip_type_params(T))]
pub struct GameAsInvitedData<T: Config> {
	/// The nonce of the account.
	pub nonce: T::Nonce,
	/// The account that invited the player.
	pub inviter: T::AccountId,
	/// The ticket of the player.
	pub ticket: TicketOf<T>,
	/// The signature from the ticket
	///
	/// It is the signature by ticket of the message: signer account encoded (The signer that is
	/// passed to this transaction extension, usually the signer of the extrinsic or for a general
	/// extrinsic the signer in `VerifySignature`.)
	pub signature: T::TicketSignature,
}

/// Transaction extension to validate and transmute to `Invited` origin and eventually provide for
/// the invited player.
///
/// If `None` the extension is not used, if `Some`, the extension is used, will validate and
/// transmute to `Invited` origin.
#[derive(
	CloneNoBound,
	EqNoBound,
	PartialEqNoBound,
	Encode,
	Decode,
	TypeInfo,
	DecodeWithMemTracking,
	DebugNoBound,
)]
#[scale_info(skip_type_params(T))]
pub struct GameAsInvited<T: Config + Send + Sync>(Option<GameAsInvitedData<T>>);

impl<T: Config + Send + Sync> GameAsInvited<T> {
	/// Creates a new `GameAsInvited` transaction extension.
	pub fn new(data: Option<GameAsInvitedData<T>>) -> Self {
		Self(data)
	}
}

/// Custom error for invalid transactions in the `GameAsInvited` transaction extension.
#[repr(u8)]
pub enum CustomError {
	/// The origin of the transaction is not signed.
	OriginNotSigned = 140,
	/// The call is not a `sign_up_with_invite` call.
	CallNotSignUpWithInvite = 141,
	/// The player is not new.
	NotNewPlayer = 142,
	/// There is no game.
	NoGame = 143,
	/// The game registration has ended.
	GameRegistrationEnded = 144,
	/// The game is not in registration phase.
	GameNotInRegistration = 145,
	/// The account is already used for a statement account.
	AccountAlreadyUsedForStmtAccount = 146,
	/// The player cannot onboard because they are already onboarded.
	CannotOnboardAlreadyOnboarded = 147,
	/// There is no pending invite for the given inviter and ticket.
	NoPendingInvite = 148,
	/// The signature provided by the player is invalid.
	InvalidSignature = 149,
	/// The airdrop VRF variant check is invalid.
	InvalidAirdropVrfVariant = 150,
	/// The airdrop VRF verification is invalid.
	InvalidAirdropRegistration = 151,
	/// The transaction is invalid for an unexpected reason.
	UnexpectedInvalidity = 152,
}
use CustomError::*;

/// The value passed from validate to prepare and prepare to post dispatch in the [`GameAsInvited`]
/// transaction extension.
pub enum GameAsInvitedValPre<AccountId> {
	UsingInvite(AccountId),
	None,
}

impl<T: Config + Send + Sync> TransactionExtension<RuntimeCallOf<T>> for GameAsInvited<T> {
	const IDENTIFIER: &'static str = "GameAsInvited";
	type Implicit = ();

	type Val = GameAsInvitedValPre<T::AccountId>;
	type Pre = GameAsInvitedValPre<T::AccountId>;

	fn weight(&self, _call: &RuntimeCallOf<T>) -> Weight {
		match self.0 {
			Some(_) => <T as Config>::WeightInfo::as_invited_tx_ext(),
			None => Weight::zero(),
		}
	}

	fn validate(
		&self,
		origin: T::RuntimeOrigin,
		call: &RuntimeCallOf<T>,
		_info: &DispatchInfoOf<RuntimeCallOf<T>>,
		_len: usize,
		_self_implicit: Self::Implicit,
		_inherited_implication: &impl Encode,
		_source: TransactionSource,
	) -> ValidateResult<Self::Val, RuntimeCallOf<T>> {
		match &self.0 {
			Some(GameAsInvitedData { nonce, inviter, ticket, signature }) => {
				// Origin must be a signed origin.
				let Some(frame_system::Origin::<T>::Signed(who)) = origin.as_system_ref() else {
					return Err(InvalidTransaction::Custom(OriginNotSigned as u8).into());
				};

				// Call must be a `sign_up_with_invite` call.
				let Some(Call::sign_up_with_invite { airdrop: call_airdrop, .. }) =
					call.is_sub_type()
				else {
					return Err(InvalidTransaction::Custom(CallNotSignUpWithInvite as u8).into());
				};

				let who_account_or_person = AccountOrPerson::Account(who.clone());

				// Player must be new.
				ensure!(
					!Players::<T>::contains_key(&who_account_or_person),
					InvalidTransaction::Custom(NotNewPlayer as u8)
				);

				// Game must be in registration phase.
				let game = Game::<T>::get().ok_or(InvalidTransaction::Custom(NoGame as u8))?;
				ensure!(
					T::UnixTime::now() < Duration::from_secs(game.registration_ends as u64),
					InvalidTransaction::Custom(GameRegistrationEnded as u8)
				);
				ensure!(
					matches!(game.state, GameState::Registration { .. }),
					InvalidTransaction::Custom(GameNotInRegistration as u8)
				);

				// We prevent any overlap between statement accounts and player.
				ensure!(
					!StmtAccountToAlias::<T>::contains_key(who),
					InvalidTransaction::Custom(AccountAlreadyUsedForStmtAccount as u8)
				);

				let maybe_archived = ArchivedPlayers::<T>::get(&who_account_or_person);

				// Player must onboard smoothly if new.
				if maybe_archived.is_none() {
					ensure!(
						indiv_pallet_score::Pallet::<T>::can_onboard_for_recognition(who).is_ok(),
						InvalidTransaction::Custom(CannotOnboardAlreadyOnboarded as u8)
					);
				}

				// Validate the invite.
				PendingInvites::<T>::get(inviter, ticket)
					.ok_or(InvalidTransaction::Custom(NoPendingInvite as u8))?;
				ensure!(
					who.using_encoded(|bytes| signature.verify(bytes, ticket)),
					InvalidTransaction::Custom(InvalidSignature as u8)
				);

				// Immutably increment the sufficient reference, just for validation purposes.
				frame_system::Pallet::<T>::inc_sufficients(who);

				Pallet::<T>::validate_register_for_airdrop(
					call_airdrop,
					&who_account_or_person,
					game.index,
					game.airdrop_scheduled,
				)
				.map_err(|e| InvalidTransaction::Custom(e as u8))?;

				// Validate the nonce.
				let ValidNonceInfo { requires, provides } =
					CheckNonce::<T>::validate_nonce_for_account(who, *nonce)?;
				let validity = ValidTransaction { requires, provides, ..Default::default() };

				Ok((
					validity,
					GameAsInvitedValPre::UsingInvite(who.clone()),
					Origin::Invited(who.clone()).into(),
				))
			},
			None => Ok((ValidTransaction::default(), GameAsInvitedValPre::None, origin)),
		}
	}

	fn prepare(
		self,
		val: Self::Val,
		_origin: &T::RuntimeOrigin,
		_call: &RuntimeCallOf<T>,
		_info: &DispatchInfoOf<RuntimeCallOf<T>>,
		_len: usize,
	) -> Result<Self::Pre, TransactionValidityError> {
		match val {
			GameAsInvitedValPre::UsingInvite(account) => {
				let Some(GameAsInvitedData { nonce, ticket, inviter, .. }) = self.0 else {
					// defensive, should already been checked in `validate`.
					defensive!();
					return Err(InvalidTransaction::Call.into());
				};

				// Prepare the nonce.
				CheckNonce::<T>::prepare_nonce_for_account(&account, nonce)?;

				// Consume the invite.
				// Note: if the transaction fails, the invite is still consumed here.
				// TODO: https://github.com/paritytech/individuality/issues/230
				// refactor to make signing up with invite 100% fail-free guaranteed.
				PendingInvites::<T>::remove(inviter, ticket);

				Ok(GameAsInvitedValPre::UsingInvite(account))
			},
			GameAsInvitedValPre::None => Ok(GameAsInvitedValPre::None),
		}
	}

	fn post_dispatch_details(
		pre: Self::Pre,
		_info: &DispatchInfoOf<RuntimeCallOf<T>>,
		_post_info: &PostDispatchInfoOf<RuntimeCallOf<T>>,
		_len: usize,
		_result: &sp_runtime::DispatchResult,
	) -> Result<Weight, TransactionValidityError> {
		match pre {
			GameAsInvitedValPre::UsingInvite(account) => {
				// Take back the sufficient reference. It's fine because the actual call dispatch,
				// in case it was successful, will do an `inc_sufficients` for the caller, which
				// will keep the account alive, and otherwise, if the call failed, the invite is
				// consumed.
				frame_system::Pallet::<T>::dec_sufficients(&account);
			},
			GameAsInvitedValPre::None => (),
		}

		Ok(Weight::zero())
	}
}
