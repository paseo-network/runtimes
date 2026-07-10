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

//! Transaction extension for ring alias authentication.
//!
//! Provides a single variant:
//! - `WithAccount(nonce)`: signed call by an account already bound to an alias. The extension
//!   promotes the origin to `Origin::RingAlias` so downstream pallets can consume the alias
//!   identity.

use crate::*;
use codec::{Decode, DecodeWithMemTracking, Encode};
use core::fmt;
use frame_support::{
	ensure, pallet_prelude::TransactionSource, traits::OriginTrait, weights::Weight, CloneNoBound,
	DefaultNoBound, EqNoBound, PartialEqNoBound,
};
use frame_system::{CheckNonce, ValidNonceInfo};
use scale_info::TypeInfo;
use sp_runtime::{
	traits::{DispatchInfoOf, Implication, TransactionExtension, ValidateResult},
	transaction_validity::{InvalidTransaction, TransactionValidityError, ValidTransaction},
};

/// Custom invalidity reasons for the `AsRingAlias` transaction extension.
#[repr(u8)]
pub enum CustomValidity {
	/// The origin must be a signed origin.
	OriginNotSigned = 200,
	/// No alias-to-account mapping exists for this signer.
	NoAliasMapping = 202,
	/// The stored alias revision is no longer valid for the current ring state.
	StaleRevision = 206,
}

impl From<CustomValidity> for TransactionValidityError {
	fn from(e: CustomValidity) -> Self {
		InvalidTransaction::Custom(e as u8).into()
	}
}

/// Information required to authenticate as a ring alias.
#[derive(
	Encode, Decode, TypeInfo, EqNoBound, CloneNoBound, PartialEqNoBound, DecodeWithMemTracking,
)]
#[scale_info(skip_type_params(T))]
pub enum AsRingAliasInfo<T: Config + Send + Sync> {
	/// Signed origin with nonce replay protection.
	/// The signer must have an existing alias-to-account mapping.
	WithAccount(T::Nonce),
}

/// Transaction extension to transform a signed origin into a verified ring alias.
///
/// **WARNING:** This extension only handles authentication and replay protection. It does NOT
/// restrict what the resulting `RingAlias` origin can do or enforce any usage limits. It MUST
/// be paired with a restriction extension (e.g. `pallet-origin-restriction`) that limits the
/// calls and usage allowed for the `RingAlias` origin. Without this, there is no spam protection.
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
pub struct AsRingAlias<T: Config + Send + Sync>(pub Option<AsRingAliasInfo<T>>);

impl<T: Config + Send + Sync> fmt::Debug for AsRingAlias<T> {
	#[cfg(feature = "std")]
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "AsRingAlias")
	}

	#[cfg(not(feature = "std"))]
	fn fmt(&self, _: &mut fmt::Formatter) -> fmt::Result {
		Ok(())
	}
}

impl<T: Config + Send + Sync> AsRingAlias<T> {
	pub fn new(info: Option<AsRingAliasInfo<T>>) -> Self {
		Self(info)
	}

	pub fn none() -> Self {
		Self(None)
	}
}

/// Validation result passed from `validate` to `prepare`.
#[cfg_attr(feature = "std", derive(Debug))]
pub enum Val<T: Config + Send + Sync> {
	/// Extension not used, passthrough.
	NotUsing,
	/// Existing account was used.
	UsingAccount(T::AccountId, T::Nonce),
}

impl<T: Config + Send + Sync> TransactionExtension<<T as frame_system::Config>::RuntimeCall>
	for AsRingAlias<T>
{
	const IDENTIFIER: &'static str = "AsRingAlias";
	type Implicit = ();
	type Val = Val<T>;
	type Pre = ();

	fn weight(&self, _call: &<T as frame_system::Config>::RuntimeCall) -> Weight {
		match &self.0 {
			None => Weight::zero(),
			Some(AsRingAliasInfo::WithAccount(_)) =>
				<T as Config>::WeightInfo::as_ring_alias_info_with_account(),
		}
	}

	fn validate(
		&self,
		origin: <T as frame_system::Config>::RuntimeOrigin,
		_call: &<T as frame_system::Config>::RuntimeCall,
		_info: &DispatchInfoOf<<T as frame_system::Config>::RuntimeCall>,
		_len: usize,
		_self_implicit: Self::Implicit,
		_inherited_implication: &impl Implication,
		_source: TransactionSource,
	) -> ValidateResult<Self::Val, <T as frame_system::Config>::RuntimeCall> {
		match &self.0 {
			Some(AsRingAliasInfo::WithAccount(nonce)) => {
				let Some(frame_system::Origin::<T>::Signed(who)) = origin.as_system_ref() else {
					return Err(CustomValidity::OriginNotSigned.into());
				};
				let who = who.clone();

				let alias_info =
					AccountToAlias::<T>::get(&who).ok_or(CustomValidity::NoAliasMapping)?;

				ensure!(
					Pallet::<T>::is_revision_in_grace(
						&alias_info.collection,
						alias_info.ring,
						alias_info.revision,
					),
					CustomValidity::StaleRevision
				);

				let local_origin = pallet::Origin::RingAlias(alias_info);
				let mut origin = origin;
				origin.set_caller_from(local_origin);

				let ValidNonceInfo { requires, provides } =
					CheckNonce::<T>::validate_nonce_for_account(&who, *nonce)?;
				let validity = ValidTransaction { requires, provides, ..Default::default() };

				Ok((validity, Val::UsingAccount(who, *nonce), origin))
			},
			None => Ok((ValidTransaction::default(), Val::NotUsing, origin)),
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
		match val {
			Val::UsingAccount(who, nonce) =>
				CheckNonce::<T>::prepare_nonce_for_account(&who, nonce)?,
			Val::NotUsing => (),
		}

		Ok(())
	}
}
