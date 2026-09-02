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

//! Types for Proof-of-Personhood system.

#![allow(clippy::result_unit_err)]

use super::*;
use frame_support::pallet_prelude::*;
use indiv_support::traits::ValidateProof;

pub type RevisionIndex = u32;
pub type PageIndex = u32;
pub type KeyCount = u64;

pub type CryptoOf<T> = <<T as Config>::MemberService as MembershipProver>::Crypto;
pub type MemberOf<T> = <CryptoOf<T> as GenerateVerifiable>::Member;
pub type ProofOf<T> = <CryptoOf<T> as GenerateVerifiable>::Proof;
pub type MembersOf<T> = <CryptoOf<T> as GenerateVerifiable>::Members;
pub type IntermediateOf<T> = <CryptoOf<T> as GenerateVerifiable>::Intermediate;
pub type SecretOf<T> = <CryptoOf<T> as GenerateVerifiable>::Secret;
pub type SignatureOf<T> = <CryptoOf<T> as GenerateVerifiable>::Signature;

/// A membership proof in a collection: a ring-VRF proof bundled with the ring index and root
/// revision it was produced against.
///
/// This is the type runtimes wire into consumers such as `pallet-coinage` (as
/// `<Pallet<T> as ValidateProof>::Proof`), so they no longer need to re-define an equivalent
/// wrapper locally.
#[derive(
	Encode,
	Decode,
	DecodeWithMemTracking,
	CloneNoBound,
	PartialEqNoBound,
	EqNoBound,
	DebugNoBound,
	TypeInfo,
	MaxEncodedLen,
)]
#[scale_info(skip_type_params(T))]
pub struct MembershipProof<T: Config> {
	/// The ring-VRF membership proof.
	pub proof: ProofOf<T>,
	/// The ring index the proof was produced against.
	pub ring: RingIndex,
	/// The revision of the ring root the proof was produced against.
	pub revision: RevisionIndex,
}

impl<T: Config> ValidateProof for Pallet<T> {
	type Proof = MembershipProof<T>;

	fn validate_proof(
		identifier: &Identifier,
		proof: &Self::Proof,
		context: &Context,
		msg: &[u8],
	) -> Result<Alias, ()> {
		let contextual_alias = T::MemberService::verify_membership(
			identifier,
			&proof.proof,
			proof.ring,
			proof.revision,
			*context,
			msg,
		)
		.inspect_err(|e| log::debug!("validate proof fail: verify membership: {e:?}"))
		.map_err(|_| ())?;
		Ok(contextual_alias.alias)
	}
}

/// Record of personhood.
#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub struct PersonRecord<Member, AccountId> {
	// The key used for the person.
	pub key: Member,
	/// An optional privileged account that can send transactions on behalf of the person.
	///
	/// Invariant: the account holds one sufficient reference tied to this field. Every write
	/// that sets the field must `inc_sufficients` on the account and every write that clears
	/// it must `dec_sufficients`.
	pub account: Option<AccountId>,
}
