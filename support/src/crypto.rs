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

//! Cryptographic type definitions for the Individuality project.
//!
//! Defines the concrete ring VRF suite and verifiable type used across the workspace.
//!
//! The suite has two interchangeable backends with identical outputs. By default the
//! upstream arkworks suite computes everything in-runtime and runs on any validator.
//! The `ec-crypto-hostcalls` feature switches both `std` and `no_std` builds to a
//! backend that offloads elliptic curve operations to the RFC-163 host functions of
//! `sp-crypto-ec-utils`; a runtime built with it only runs on validators that expose
//! those host functions.

#[cfg(feature = "ec-crypto-hostcalls")]
mod host_hooks;

#[cfg(not(feature = "ec-crypto-hostcalls"))]
mod bandersnatch {
	pub use verifiable::ring::ark_vrf::suites::bandersnatch::BandersnatchSha512Ell2 as BandersnatchSuite;
}

#[cfg(feature = "ec-crypto-hostcalls")]
mod bandersnatch {
	use alloc::borrow::Cow;
	use spin::Once;
	#[cfg(not(feature = "prover"))]
	use verifiable::ring::make_ring_context;
	use verifiable::ring::{
		ark_vrf::{
			self, pedersen::PedersenSuite, ring::RingSuite,
			suites::bandersnatch::BandersnatchSha512Ell2, Suite,
		},
		make_canonical_pcs_vk, make_empty_members_set, Bls12_381Params, MembersSet, RingDomainSize,
		RingSuiteExt, VerifierCache,
	};
	#[cfg(feature = "prover")]
	use verifiable::ring::{make_ring_setup, ProverCache};

	/// The upstream bandersnatch suite from the `ark-vrf` crate.
	type Upstream = BandersnatchSha512Ell2;

	#[derive(Debug, Copy, Clone, PartialEq, Eq)]
	pub struct BandersnatchSuite;

	ark_vrf::suite_types!(BandersnatchSuite);
	ark_vrf::ring_suite_types!(BandersnatchSuite);

	impl Suite for BandersnatchSuite {
		const SUITE_ID: &'static [u8] = <Upstream as Suite>::SUITE_ID;
		// Host-accelerated elliptic curve type backed by the `sp-crypto-ec-utils`
		// host functions through the local `host_hooks`.
		type Affine = super::host_hooks::EdwardsAffine;
		type Transcript = <Upstream as Suite>::Transcript;

		// Bandersnatch hashes to curve with Elligator2, not the trait's try-and-increment
		// default. Delegate to the upstream construction and re-project onto the host-accelerated
		// affine type (same base field), so VRF outputs stay identical to the standard suite.
		fn data_to_point(data: &[u8]) -> Option<AffinePoint> {
			let native_pt = Upstream::data_to_point(data)?;
			Some(AffinePoint::new_unchecked(native_pt.x, native_pt.y))
		}
	}

	impl PedersenSuite for BandersnatchSuite {
		const BLINDING_BASE: AffinePoint = AffinePoint::new_unchecked(
			<Upstream as PedersenSuite>::BLINDING_BASE.x,
			<Upstream as PedersenSuite>::BLINDING_BASE.y,
		);
	}

	impl RingSuite for BandersnatchSuite {
		// Host-accelerated pairing engine backed by the `sp-crypto-ec-utils`
		// host functions through the local `host_hooks`.
		type Pairing = super::host_hooks::Bls12_381;
		const ACCUMULATOR_BASE: AffinePoint = AffinePoint::new_unchecked(
			<Upstream as RingSuite>::ACCUMULATOR_BASE.x,
			<Upstream as RingSuite>::ACCUMULATOR_BASE.y,
		);
		const PADDING: AffinePoint = AffinePoint::new_unchecked(
			<Upstream as RingSuite>::PADDING.x,
			<Upstream as RingSuite>::PADDING.y,
		);
	}

	const fn cache_index(domain_size: RingDomainSize) -> usize {
		match domain_size {
			RingDomainSize::Domain11 => 0,
			RingDomainSize::Domain12 => 1,
			RingDomainSize::Domain16 => 2,
		}
	}

	/// Lazy-static cache for the ring verifier context, one cell per domain size.
	///
	/// Computed once per domain size on first access and reused thereafter.
	/// When the `prover` feature is enabled, the context is extracted from the
	/// cached `RingSetup`.
	pub struct BandersnatchVerifierCache;

	impl VerifierCache<BandersnatchSuite> for BandersnatchVerifierCache {
		#[cfg(feature = "prover")]
		fn ring_context(
			domain_size: RingDomainSize,
		) -> Cow<'static, ark_vrf::ring::RingContext<BandersnatchSuite>> {
			Cow::Borrowed(BandersnatchProverCache::setup(domain_size).ring_context())
		}

		#[cfg(not(feature = "prover"))]
		fn ring_context(
			domain_size: RingDomainSize,
		) -> Cow<'static, ark_vrf::ring::RingContext<BandersnatchSuite>> {
			type Ctx = ark_vrf::ring::RingContext<BandersnatchSuite>;
			static CELLS: [Once<Ctx>; 3] = [const { Once::new() }; 3];
			Cow::Borrowed(
				CELLS[cache_index(domain_size)].call_once(|| make_ring_context(domain_size)),
			)
		}

		fn verifier_params() -> ark_vrf::ring::PcsVerifierParams<BandersnatchSuite> {
			static CELL: Once<ark_vrf::ring::PcsVerifierParams<BandersnatchSuite>> = Once::new();
			CELL.call_once(make_canonical_pcs_vk::<BandersnatchSuite>).clone()
		}

		fn empty_members_set(domain_size: RingDomainSize) -> MembersSet<BandersnatchSuite> {
			type M = MembersSet<BandersnatchSuite>;
			static CELLS: [Once<M>; 3] = [const { Once::new() }; 3];
			CELLS[cache_index(domain_size)]
				.call_once(|| make_empty_members_set(domain_size))
				.clone()
		}
	}

	/// Lazy-static cache for the ring prover setup, one cell per domain size.
	///
	/// Computed once per domain size on first access and reused thereafter.
	#[cfg(feature = "prover")]
	pub struct BandersnatchProverCache;

	#[cfg(feature = "prover")]
	impl BandersnatchProverCache {
		/// Get or construct the cached ring setup for the given domain size.
		fn setup(
			domain_size: RingDomainSize,
		) -> &'static ark_vrf::ring::RingSetup<BandersnatchSuite> {
			type Setup = ark_vrf::ring::RingSetup<BandersnatchSuite>;
			static CELLS: [Once<Setup>; 3] = [const { Once::new() }; 3];
			CELLS[cache_index(domain_size)].call_once(|| make_ring_setup(domain_size))
		}
	}

	#[cfg(feature = "prover")]
	impl ProverCache<BandersnatchSuite> for BandersnatchProverCache {
		fn ring_setup(
			domain_size: RingDomainSize,
		) -> Cow<'static, ark_vrf::ring::RingSetup<BandersnatchSuite>> {
			Cow::Borrowed(Self::setup(domain_size))
		}
	}

	impl RingSuiteExt for BandersnatchSuite {
		const VRF_INPUT_DOMAIN: &[u8] = <Upstream as RingSuiteExt>::VRF_INPUT_DOMAIN;

		const PUBLIC_KEY_SIZE: usize = <Upstream as RingSuiteExt>::PUBLIC_KEY_SIZE;
		const MEMBERS_SET_SIZE: usize = <Upstream as RingSuiteExt>::MEMBERS_SET_SIZE;
		const MEMBERS_COMMITMENT_SIZE: usize = <Upstream as RingSuiteExt>::MEMBERS_COMMITMENT_SIZE;
		const STATIC_CHUNK_SIZE: usize = <Upstream as RingSuiteExt>::STATIC_CHUNK_SIZE;
		const SIGNATURE_SIZE: usize = <Upstream as RingSuiteExt>::SIGNATURE_SIZE;
		const RING_PROOF_SIZE: usize = <Upstream as RingSuiteExt>::RING_PROOF_SIZE;
		const VRF_OUTPUT_SIZE: usize = <Upstream as RingSuiteExt>::VRF_OUTPUT_SIZE;

		type CurveParams = Bls12_381Params;

		type PublicKeyBytes = <Upstream as RingSuiteExt>::PublicKeyBytes;
		type SignatureBytes = <Upstream as RingSuiteExt>::SignatureBytes;

		type VerifierCache = BandersnatchVerifierCache;

		#[cfg(feature = "prover")]
		type ProverCache = BandersnatchProverCache;
	}
}

pub use bandersnatch::*;
pub use verifiable::GenerateVerifiable;

/// The ring VRF verifiable type used across the project.
pub type BandersnatchVrfVerifiable = verifiable::ring::RingVrfVerifiable<BandersnatchSuite>;

/// Entropy backend for `getrandom` on targets without an OS source, i.e. the
/// wasm benchmarking runtime. Ring proof blinding draws from it when benchmark
/// setup creates proofs on-chain (`verifiable/no-std-prover`). Deterministic on
/// purpose: benchmark runs stay reproducible, and predictable blinding is
/// harmless outside production. Production runtimes are built without
/// `runtime-benchmarks` and have no in-runtime prover at all.
#[cfg(feature = "runtime-benchmarks")]
mod benchmark_entropy {
	use core::sync::atomic::{AtomicU64, Ordering};

	getrandom::register_custom_getrandom!(fill_from_counter);

	fn fill_from_counter(dest: &mut [u8]) -> Result<(), getrandom::Error> {
		static COUNTER: AtomicU64 = AtomicU64::new(0);
		for chunk in dest.chunks_mut(8) {
			let word = split_mix_64(COUNTER.fetch_add(1, Ordering::Relaxed));
			chunk.copy_from_slice(&word.to_le_bytes()[..chunk.len()]);
		}
		Ok(())
	}

	fn split_mix_64(counter: u64) -> u64 {
		let mut word = counter.wrapping_add(1).wrapping_mul(0x9E3779B97F4A7C15);
		word = (word ^ (word >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
		word = (word ^ (word >> 27)).wrapping_mul(0x94D049BB133111EB);
		word ^ (word >> 31)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use alloc::vec::Vec;
	use verifiable::{
		ring::{ark_vrf::ring::SrsLookup, RingDomainSize},
		GenerateVerifiable,
	};

	#[test]
	fn plain_signature() {
		let msg = b"test message";
		let secret = BandersnatchVrfVerifiable::new_secret([0u8; 32]);
		let public = BandersnatchVrfVerifiable::member_from_secret(&secret);
		let signature = BandersnatchVrfVerifiable::sign(&secret, msg).unwrap();
		assert!(BandersnatchVrfVerifiable::verify_signature(&signature, msg, &public));
	}

	#[test]
	fn signature_wrong_message_fails() {
		let secret = BandersnatchVrfVerifiable::new_secret([1u8; 32]);
		let public = BandersnatchVrfVerifiable::member_from_secret(&secret);
		let signature = BandersnatchVrfVerifiable::sign(&secret, b"correct").unwrap();
		assert!(!BandersnatchVrfVerifiable::verify_signature(&signature, b"wrong", &public));
	}

	#[test]
	fn signature_wrong_key_fails() {
		let secret1 = BandersnatchVrfVerifiable::new_secret([1u8; 32]);
		let secret2 = BandersnatchVrfVerifiable::new_secret([2u8; 32]);
		let public2 = BandersnatchVrfVerifiable::member_from_secret(&secret2);
		let signature = BandersnatchVrfVerifiable::sign(&secret1, b"msg").unwrap();
		assert!(!BandersnatchVrfVerifiable::verify_signature(&signature, b"msg", &public2));
	}

	#[test]
	fn member_validity() {
		let secret = BandersnatchVrfVerifiable::new_secret([42u8; 32]);
		let public = BandersnatchVrfVerifiable::member_from_secret(&secret);
		assert!(BandersnatchVrfVerifiable::is_member_valid(&public));
		assert!(!BandersnatchVrfVerifiable::is_member_valid(&[0xff; 32]));
	}

	#[test]
	fn different_entropy_yields_different_keys() {
		let s1 = BandersnatchVrfVerifiable::new_secret([1u8; 32]);
		let s2 = BandersnatchVrfVerifiable::new_secret([2u8; 32]);
		let p1 = BandersnatchVrfVerifiable::member_from_secret(&s1);
		let p2 = BandersnatchVrfVerifiable::member_from_secret(&s2);
		assert_ne!(p1, p2);
	}

	#[test]
	fn alias_in_context_deterministic() {
		let secret = BandersnatchVrfVerifiable::new_secret([7u8; 32]);
		let alias1 = BandersnatchVrfVerifiable::alias_in_context(&secret, b"ctx").unwrap();
		let alias2 = BandersnatchVrfVerifiable::alias_in_context(&secret, b"ctx").unwrap();
		assert_eq!(alias1, alias2);
	}

	#[test]
	fn alias_differs_across_contexts() {
		let secret = BandersnatchVrfVerifiable::new_secret([7u8; 32]);
		let alias1 = BandersnatchVrfVerifiable::alias_in_context(&secret, b"ctx_a").unwrap();
		let alias2 = BandersnatchVrfVerifiable::alias_in_context(&secret, b"ctx_b").unwrap();
		assert_ne!(alias1, alias2);
	}

	fn build_members_commitment(
		domain_size: RingDomainSize,
		members: &[[u8; 32]],
	) -> <BandersnatchVrfVerifiable as GenerateVerifiable>::Members {
		let builder_pcs_params =
			verifiable::ring::ring_verifier_builder_params::<BandersnatchSuite>(domain_size);

		let get_chunks =
			|range: core::ops::Range<usize>| -> Result<
				Vec<verifiable::ring::StaticChunk<BandersnatchSuite>>,
				(),
			> {
				(&builder_pcs_params)
					.lookup(range)
					.map(|v: Vec<_>| v.into_iter().map(verifiable::ring::StaticChunk).collect())
					.ok_or(())
			};

		let mut inter = BandersnatchVrfVerifiable::start_members(domain_size);
		BandersnatchVrfVerifiable::push_members(&mut inter, members.iter().copied(), get_chunks)
			.unwrap();
		BandersnatchVrfVerifiable::finish_members(inter)
	}

	#[test]
	fn ring_proof_create_and_validate() {
		let domain_size = RingDomainSize::Domain11;
		let context = b"test-context";
		let message = b"test-message";
		let num_members = 3;

		let secrets: Vec<_> = (0..num_members)
			.map(|i| BandersnatchVrfVerifiable::new_secret([i as u8; 32]))
			.collect();
		let members: Vec<_> =
			secrets.iter().map(BandersnatchVrfVerifiable::member_from_secret).collect();

		let members_commitment = build_members_commitment(domain_size, &members);

		// Prove as member 0.
		let prover_idx = 0;
		let proof_commitment = BandersnatchVrfVerifiable::open(
			domain_size,
			&members[prover_idx],
			members.clone().into_iter(),
		)
		.unwrap();
		let (proof, alias) = BandersnatchVrfVerifiable::create(
			proof_commitment,
			&secrets[prover_idx],
			context,
			message,
		)
		.unwrap();

		// Validate.
		let recovered_alias = BandersnatchVrfVerifiable::validate(
			domain_size,
			&proof,
			&members_commitment,
			context,
			message,
		)
		.unwrap();
		assert_eq!(alias, recovered_alias);

		// Cross-check with alias_in_context.
		let expected_alias =
			BandersnatchVrfVerifiable::alias_in_context(&secrets[prover_idx], context).unwrap();
		assert_eq!(alias, expected_alias);
	}

	#[test]
	fn ring_proof_wrong_context_fails() {
		let domain_size = RingDomainSize::Domain11;
		let message = b"msg";

		let secret = BandersnatchVrfVerifiable::new_secret([0u8; 32]);
		let member = BandersnatchVrfVerifiable::member_from_secret(&secret);

		let members_commitment = build_members_commitment(domain_size, &[member]);

		let commitment =
			BandersnatchVrfVerifiable::open(domain_size, &member, [member].into_iter()).unwrap();
		let (proof, _alias) =
			BandersnatchVrfVerifiable::create(commitment, &secret, b"ctx_a", message).unwrap();

		// Validate with wrong context must fail.
		assert!(BandersnatchVrfVerifiable::validate(
			domain_size,
			&proof,
			&members_commitment,
			b"ctx_b",
			message,
		)
		.is_err());
	}
}
