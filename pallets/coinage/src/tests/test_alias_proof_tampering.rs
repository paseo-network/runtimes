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

//! Tests that verify the unload token proof binds over the alias proofs, preventing an attacker
//! from swapping alias proofs after the main proof was signed.
//!
//! The unload token proof (people, lite people, paid, or from-output first alias) signs over
//! `alias_proofs ++ inherited_implication` rather than just `inherited_implication`. This makes
//! the alias proofs part of the signed intent: if any alias proof is swapped, the recomputed
//! intent message no longer matches what the main proof signed and the extension rejects the
//! transaction.
//!
//! Every test follows the same pattern:
//! 1. Build a completely **valid** extrinsic (2 aliases, all proofs correct).
//! 2. An attacker replaces the **second** alias proof with a proof from a third member.
//! 3. The extension rejects because the intent message no longer matches.

use crate::{
	extension::{AsCoinage, AsCoinageInfo},
	mock::*,
	pallet::CustomInvalidity,
	*,
};
use codec::Encode;
use frame_support::{traits::fungible::Mutate as _, BoundedVec};
use frame_system::AuthorizeCall;
use sp_runtime::testing::UintAuthorityId;
use verifiable::GenerateVerifiable;

/// Common state produced by [`setup_tamper_scenario`].
struct TamperScenario {
	/// The runtime call (`unload_recycler_into_external_asset` with 3 aliases).
	call: RuntimeCall,
	/// SCALE-encoded `inherited_implication`, needed for `intent_msg` computation.
	encoded_implication: Vec<u8>,
	/// Valid alias proofs from `secrets[0..3]`.
	legit_proofs: Vec<Proof>,
	/// The 4 recycler secrets.
	secrets: Vec<Secret>,
	/// The recycler ring members.
	ring_members: Vec<MemberOf<Test>>,
	value: Denomination,
	index: RingIndex,
	revision: RevisionIndex,
}

/// Set up the common scenario for alias-proof tampering tests.
///
/// Creates a recycler with **4** members and an `unload_recycler_into_external_asset` call
/// carrying **3** aliases (`secrets[0..3]`). Returns three legitimate alias proofs plus a
/// swapped proof from the fourth member (`secrets[3]`).
///
/// `max_fee` has to match how the extrinsic pays: it is ignored by a prepaid token, and has to
/// cover the converted fee when the fee comes out of the output.
fn setup_tamper_scenario(max_fee: u64) -> TamperScenario {
	setup_balances();

	let value: Denomination = 0;
	let (secrets, index, revision) = setup_recycler(value, 4, 0);

	let aliases: Vec<_> = secrets[..3]
		.iter()
		.map(|s| {
			CryptoOf::<Test>::alias_in_context(s, UNLOADING_RECYCLER_CONTEXT.as_ref()).unwrap()
		})
		.collect();

	let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_external_asset {
		instance_id: TEST_INSTANCE_ID,
		aliases: BoundedVec::truncate_from(aliases),
		value,
		index,
		revision,
		to: CHARLIE,
		max_fee,
	});

	let inherited_implication = ((0u8, &call), (), ());
	let encoded_implication = inherited_implication.encode();
	let proven_msg = sp_io::hashing::blake2_256(&encoded_implication);

	let ring_members = Coinage::get_recycler_members(TEST_INSTANCE_ID, value, index);
	let legit_proofs: Vec<_> = secrets[..3]
		.iter()
		.map(|s| create_unload_proof(s, &ring_members, &proven_msg).0)
		.collect();

	TamperScenario {
		call,
		encoded_implication,
		legit_proofs,
		secrets,
		ring_members,
		value,
		index,
		revision,
	}
}

/// A valid people-token extrinsic with 3 alias proofs becomes invalid when an attacker
/// replaces the last alias proof.
///
/// The people proof signs `blake2_256(alias_proofs.encode() ++ inherited_implication.encode())`.
/// Replacing `alias_proofs[2]` changes the hash, so `validate_proof` fails.
#[test]
fn people_swapped_alias_proofs_is_invalid() {
	new_test_ext().execute_with(|| {
		let s = setup_tamper_scenario(0);

		// People proof signs over the LEGITIMATE alias proofs.
		let legit_alias_proofs: BoundedVec<_, <Test as Config>::MaxConsolidation> =
			s.legit_proofs.clone().try_into().unwrap();
		let intent_msg = sp_io::hashing::blake2_256(
			&[legit_alias_proofs.encode(), s.encoded_implication].concat(),
		);
		let context = crate::pallet::free_unload_token_context(0, 0);
		let people_proof = MembershipProof {
			context: context.to_vec(),
			msg: intent_msg.to_vec(),
			alias: [0u8; 32],
		};

		// Attacker keeps legit_proofs[0..2] but swaps item 1 and 2.
		let tampered: BoundedVec<_, <Test as Config>::MaxConsolidation> =
			vec![s.legit_proofs[0].clone(), s.legit_proofs[2].clone(), s.legit_proofs[1].clone()]
				.try_into()
				.unwrap();

		// Tampered extrinsic is invalid
		let info = AsCoinageInfo::AsUnloadTokenPeople {
			proof: people_proof.clone(),
			period: 0,
			counter: 0,
			alias_proofs: tampered,
		};
		let ext = Extrinsic::new_transaction(
			s.call.clone(),
			(AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(info))),
		);
		assert_invalid(ext, CustomInvalidity::InvalidUnloadTokenProof);

		// Legit extrinsic is valid
		let info = AsCoinageInfo::AsUnloadTokenPeople {
			proof: people_proof,
			period: 0,
			counter: 0,
			alias_proofs: s.legit_proofs.try_into().unwrap(),
		};
		let ext = Extrinsic::new_transaction(
			s.call,
			(AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(info))),
		);
		Executive::apply_extrinsic(ext).unwrap().unwrap();
	});
}

/// Same as [`people_swapped_alias_proofs_is_invalid`] but for the lite people proof variant.
#[test]
fn lite_people_swapped_alias_proofs_is_invalid() {
	new_test_ext().execute_with(|| {
		let s = setup_tamper_scenario(0);

		// Lite people proof signs over the LEGITIMATE alias proofs.
		let legit_alias_proofs: BoundedVec<_, <Test as Config>::MaxConsolidation> =
			s.legit_proofs.clone().try_into().unwrap();
		let intent_msg = sp_io::hashing::blake2_256(
			&[legit_alias_proofs.encode(), s.encoded_implication].concat(),
		);
		let context = crate::pallet::free_unload_token_context(0, 0);
		let lite_people_proof = MembershipProof {
			context: context.to_vec(),
			msg: intent_msg.to_vec(),
			alias: [0u8; 32],
		};

		// Attacker swaps items 1 and 2.
		let tampered: BoundedVec<_, <Test as Config>::MaxConsolidation> =
			vec![s.legit_proofs[0].clone(), s.legit_proofs[2].clone(), s.legit_proofs[1].clone()]
				.try_into()
				.unwrap();

		// Tampered extrinsic is invalid
		let info = AsCoinageInfo::AsUnloadTokenLitePeople {
			proof: lite_people_proof.clone(),
			period: 0,
			counter: 0,
			alias_proofs: tampered,
		};
		let ext = Extrinsic::new_transaction(
			s.call.clone(),
			(AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(info))),
		);
		assert_invalid(ext, CustomInvalidity::InvalidUnloadTokenProof);

		// Legit extrinsic is valid
		let info = AsCoinageInfo::AsUnloadTokenLitePeople {
			proof: lite_people_proof,
			period: 0,
			counter: 0,
			alias_proofs: s.legit_proofs.try_into().unwrap(),
		};
		let ext = Extrinsic::new_transaction(
			s.call,
			(AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(info))),
		);
		Executive::apply_extrinsic(ext).unwrap().unwrap();
	});
}

/// A valid paid-token extrinsic with 3 alias proofs becomes invalid when an attacker swaps
/// items 1 and 2 inside `alias_proofs`.
///
/// The paid token proof signs `blake2_256(alias_proofs.encode() ++ inherited.encode())`.
/// Swapping two alias proofs changes the encoding, so `validate_token_consumption_proof`
/// fails.
#[test]
fn paid_swapped_alias_proofs_is_invalid() {
	new_test_ext().execute_with(|| {
		// Extra setup: pay for a token and build its ring.
		let signer = 1;
		let _ = Balances::mint_into(&signer, 1000);

		let secret = get_secret(1);
		let member = CryptoOf::<Test>::member_from_secret(&secret);
		let sig = CryptoOf::<Test>::sign(&secret, &signer.encode()).unwrap();

		let pay_ext = Extrinsic::new_signed(
			crate::Call::pay_for_recycler_unload_fee_token_with_native {
				member_key: member,
				proof_of_ownership: sig,
			}
			.into(),
			signer,
			UintAuthorityId(signer),
			(AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(None)),
		);
		assert_eq!(Executive::apply_extrinsic(pay_ext), Ok(Ok(())));
		Members::process_maintenance();

		// Now reuse the common tamper scenario for the recycler + call.
		let s = setup_tamper_scenario(0);
		let period: u32 = 0;
		let paid_ring_index: u32 = 0;

		// Paid proof signs over the LEGITIMATE alias proofs.
		let legit_alias_proofs: BoundedVec<_, <Test as Config>::MaxConsolidation> =
			s.legit_proofs.clone().try_into().unwrap();
		let intent_msg = sp_io::hashing::blake2_256(
			&[legit_alias_proofs.encode(), s.encoded_implication.clone()].concat(),
		);

		let mut ctx = [0u8; 32];
		ctx[..28].copy_from_slice(PAID_UNLOAD_TOKEN_CONTEXT_BASE.as_ref());
		ctx[28..32].copy_from_slice(&period.to_le_bytes());
		let paid_members = Coinage::get_paid_token_ring_members(period, paid_ring_index);
		let com = CryptoOf::<Test>::open(paid_token_ring_size(), &member, paid_members.into_iter())
			.unwrap();
		let (p_proof, _) = CryptoOf::<Test>::create(com, &secret, &ctx, &intent_msg).unwrap();

		let id = Coinage::paid_token_collection_identifier(period);
		let revision =
			<Test as Config>::MemberService::ring_revision(&id, paid_ring_index).unwrap();

		// Attacker swaps items 1 and 2.
		let tampered: BoundedVec<_, <Test as Config>::MaxConsolidation> =
			vec![s.legit_proofs[0].clone(), s.legit_proofs[2].clone(), s.legit_proofs[1].clone()]
				.try_into()
				.unwrap();

		// Tampered extrinsic is invalid
		let info = AsCoinageInfo::AsUnloadTokenPaid {
			proof: p_proof.clone(),
			period,
			paid_token_ring_index: paid_ring_index,
			paid_token_ring_revision: revision,
			alias_proofs: tampered,
		};
		let uxt = Extrinsic::new_signed(
			s.call.clone(),
			0,
			UintAuthorityId(0),
			(AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(info))),
		);
		assert_invalid(uxt, CustomInvalidity::InvalidUnloadTokenProof);

		// Legit extrinsic is valid
		let info = AsCoinageInfo::AsUnloadTokenPaid {
			proof: p_proof,
			period,
			paid_token_ring_index: paid_ring_index,
			paid_token_ring_revision: revision,
			alias_proofs: s.legit_proofs.try_into().unwrap(),
		};
		let uxt = Extrinsic::new_signed(
			s.call,
			0,
			UintAuthorityId(0),
			(AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(info))),
		);
		Executive::apply_extrinsic(uxt).unwrap().unwrap();
	});
}

/// A valid from-output extrinsic with 3 alias proofs becomes invalid when an attacker swaps
/// items 1 and 2 inside `alias_proofs`.
///
/// The first alias proof signs `blake2_256(alias_proofs[1..].encode() ++ retry_counter ++
/// inherited_implication)`. Swapping two proofs in the tail changes the encoding, so
/// `validate_alias_proof` fails for the first proof.
#[test]
fn from_output_swapped_alias_proofs_is_invalid() {
	new_test_ext().execute_with(|| {
		let s = setup_tamper_scenario(unload_token_fee_in_asset());
		let inherited_implication = ((0u8, &s.call), (), ());

		// First proof signs over the LEGITIMATE tail: [legit_proofs[1], legit_proofs[2]].
		let legit_tail = vec![s.legit_proofs[1].clone(), s.legit_proofs[2].clone()];
		let intent_msg =
			(&legit_tail, 0u8, &inherited_implication).using_encoded(sp_io::hashing::blake2_256);
		let (first_proof, _) = create_unload_proof(&s.secrets[0], &s.ring_members, &intent_msg);

		// Attacker swaps items 1 and 2.
		let tampered: BoundedVec<_, <Test as Config>::MaxConsolidation> =
			vec![first_proof.clone(), s.legit_proofs[2].clone(), s.legit_proofs[1].clone()]
				.try_into()
				.unwrap();

		// Tampered extrinsic is invalid
		let info = Some(AsCoinageInfo::AsUnloadTokenFromOutput {
			fee_recycler_value: s.value,
			fee_recycler_index: s.index,
			fee_recycler_revision: s.revision,
			retry_counter: 0u8,
			alias_proofs: tampered,
		});
		let ext = Extrinsic::new_transaction(
			s.call.clone(),
			(AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(info)),
		);
		assert_invalid(ext, CustomInvalidity::InvalidAliasProof);

		// Legit extrinsic is valid
		let legit: BoundedVec<_, <Test as Config>::MaxConsolidation> =
			vec![first_proof, s.legit_proofs[1].clone(), s.legit_proofs[2].clone()]
				.try_into()
				.unwrap();
		let info = Some(AsCoinageInfo::AsUnloadTokenFromOutput {
			fee_recycler_value: s.value,
			fee_recycler_index: s.index,
			fee_recycler_revision: s.revision,
			retry_counter: 0u8,
			alias_proofs: legit,
		});
		let ext = Extrinsic::new_transaction(
			s.call,
			(AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(info)),
		);
		Executive::apply_extrinsic(ext).unwrap().unwrap();
	});
}
