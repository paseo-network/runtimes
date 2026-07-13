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

//! Account-based VRF entropy derivation.
//!
//! The construction mirrors how `verifiable::ring::bandersnatch` shapes a
//! VRF signature, so the two backends look as similar as possible:
//!
//! - **Domain.** Per-event domain bytes = `VRF_TRANSCRIPT_LABEL ‖ event_id`. This parallels
//!   bandersnatch's `VRF_INPUT_DOMAIN ‖ context`, which is hashed-to-curve to derive the
//!   per-context VRF input point. Here we just feed the concatenation into the merlin transcript:
//!   the transcript length-prefixes its value internally, so the prefix on `event_id` is enough to
//!   keep these bytes separated from other uses of the same id, and we don't need to compress to a
//!   fixed size.
//! - **Data.** The signer's `sr25519::Public` plays the role of bandersnatch's aux_data / message —
//!   it's bound into the proof so a captured signature can't be replayed under a different signer.
//!
//! The returned 32 bytes are produced by schnorrkel's HKDF-style expansion
//! (`make_bytes`, keyed by `VRF_EXPAND_CONTEXT`) over the VRF input/output
//! pair, so the entropy is uniformly distributed. It is deterministic in
//! `(secret_key, event_id)` and verifiable from `(public_key, signature)`.

extern crate alloc;

use alloc::vec::Vec;
use frame_support::traits::Defensive;
use sp_core::{
	crypto::VrfPublic,
	sr25519::{
		self,
		vrf::{VrfSignature, VrfTranscript},
	},
};

/// Domain-separation prefix. The per-event domain is `VRF_TRANSCRIPT_LABEL ++ event_id` and gets
/// folded into the transcript as the `b"domain"` field.
pub const VRF_TRANSCRIPT_LABEL: &[u8] = b"pop:airdrop";

/// Context label for the HKDF-style expansion of the VRF output into uniform
/// entropy bytes. Domain-separated from [`VRF_TRANSCRIPT_LABEL`] so the
/// expansion can never collide with the transcript itself.
pub const VRF_EXPAND_CONTEXT: &[u8] = b"pop:airdrop:entropy";

/// Per-event domain bytes: `VRF_TRANSCRIPT_LABEL ++ event_id`. Exposed so off-chain code can
/// compute it.
pub fn domain_for_event(event_id: &[u8; 32]) -> Vec<u8> {
	[VRF_TRANSCRIPT_LABEL, event_id].concat()
}

/// Build the transcript that the participant signs with their sr25519 key.
/// Binds `(domain, signer)` so two different events produce distinct VRF
/// outputs and a captured signature can't be replayed for a different
/// signer.
pub fn transcript_for_event(event_id: &[u8; 32], public: &sr25519::Public) -> VrfTranscript {
	let domain = domain_for_event(event_id);
	VrfTranscript::new(VRF_TRANSCRIPT_LABEL, &[(b"domain", &domain), (b"signer", &public.0)])
}

/// Verify a sr25519 VRF signature over the event transcript and, on success, return the
/// deterministic 32 bytes of entropy bound to `(public, event_id)`.
///
/// The entropy is the schnorrkel HKDF-style expansion of the VRF input/output pair under
/// [`VRF_EXPAND_CONTEXT`], so the bytes are uniformly distributed. They are verifiably
/// derivable from `(public, transcript, signature.pre_output)` and deterministic in the secret
/// key plus transcript.
pub fn verify_and_extract_entropy(
	public: &sr25519::Public,
	event_id: &[u8; 32],
	signature: &VrfSignature,
) -> Option<[u8; 32]> {
	let transcript = transcript_for_event(event_id, public);
	if !public.vrf_verify(&transcript.clone().into_sign_data(), signature) {
		return None
	}
	public
		.make_bytes::<32>(VRF_EXPAND_CONTEXT, &transcript, &signature.pre_output)
		.defensive_proof("airdrop:vrf:make bytes from verified transcript is ok")
		.ok()
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_core::{crypto::VrfSecret, sr25519::Pair, Pair as _};

	#[test]
	fn sign_then_verify_extracts_entropy() {
		let pair = Pair::from_seed(b"12345678901234567890123456789012");
		let public = pair.public();
		let event_id = [7u8; 32];

		let sign_data = transcript_for_event(&event_id, &public).into_sign_data();
		let sig = pair.vrf_sign(&sign_data);

		let entropy = verify_and_extract_entropy(&public, &event_id, &sig)
			.expect("valid signature must yield entropy");
		assert_eq!(entropy.len(), 32);

		// Determinism: same input must give same entropy.
		let sig2 = pair.vrf_sign(&sign_data);
		let entropy2 = verify_and_extract_entropy(&public, &event_id, &sig2).unwrap();
		assert_eq!(entropy, entropy2);
	}

	#[test]
	fn wrong_event_id_fails_verification() {
		let pair = Pair::from_seed(b"12345678901234567890123456789012");
		let public = pair.public();

		let signed_event = [1u8; 32];
		let other_event = [2u8; 32];
		let sign_data = transcript_for_event(&signed_event, &public).into_sign_data();
		let sig = pair.vrf_sign(&sign_data);

		assert!(verify_and_extract_entropy(&public, &other_event, &sig).is_none());
	}

	#[test]
	fn wrong_public_key_fails_verification() {
		let pair_a = Pair::from_seed(b"12345678901234567890123456789012");
		let pair_b = Pair::from_seed(b"abcdefghijklmnopqrstuvwxyz123456");
		let event_id = [3u8; 32];

		// Signer A signs (and the transcript commits to A's public key).
		let sign_data = transcript_for_event(&event_id, &pair_a.public()).into_sign_data();
		let sig = pair_a.vrf_sign(&sign_data);

		// Verification against B's public key fails: VRF mismatch and
		// also the transcript "signer" field wouldn't match.
		assert!(verify_and_extract_entropy(&pair_b.public(), &event_id, &sig).is_none());
	}

	#[test]
	fn distinct_event_ids_yield_distinct_entropy() {
		let pair = Pair::from_seed(b"12345678901234567890123456789012");
		let public = pair.public();
		let event_a = [1u8; 32];
		let event_b = [2u8; 32];
		let sig_a = pair.vrf_sign(&transcript_for_event(&event_a, &public).into_sign_data());
		let sig_b = pair.vrf_sign(&transcript_for_event(&event_b, &public).into_sign_data());
		let entropy_a = verify_and_extract_entropy(&public, &event_a, &sig_a).unwrap();
		let entropy_b = verify_and_extract_entropy(&public, &event_b, &sig_b).unwrap();
		assert_ne!(entropy_a, entropy_b);
	}

	#[test]
	fn distinct_signers_yield_distinct_entropy() {
		let pair_a = Pair::from_seed(b"12345678901234567890123456789012");
		let pair_b = Pair::from_seed(b"abcdefghijklmnopqrstuvwxyz123456");
		let event_id = [9u8; 32];
		let sig_a =
			pair_a.vrf_sign(&transcript_for_event(&event_id, &pair_a.public()).into_sign_data());
		let sig_b =
			pair_b.vrf_sign(&transcript_for_event(&event_id, &pair_b.public()).into_sign_data());
		let entropy_a = verify_and_extract_entropy(&pair_a.public(), &event_id, &sig_a).unwrap();
		let entropy_b = verify_and_extract_entropy(&pair_b.public(), &event_id, &sig_b).unwrap();
		assert_ne!(entropy_a, entropy_b);
	}
}
