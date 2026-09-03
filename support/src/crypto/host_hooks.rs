// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Individuality.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License.

// Individuality is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Individuality.  If not, see <http://www.gnu.org/licenses/>.

//! Workaround: arkworks 0.6 runtime types on the arkworks 0.5 host functions.
//!
//! `verifiable` depends on `ark-vrf` 0.5.2, which uses arkworks 0.6. The
//! runtime type aliases of `sp-crypto-ec-utils` bind to the arkworks 0.5
//! `-ext` crates and cannot back an arkworks 0.6 VRF suite, and
//! `sp-crypto-ec-utils` cannot move to arkworks 0.6 at this time.
//!
//! The bridge is sound because only opaque SCALE bytes cross the
//! host-function boundary: affine points and field elements in arkworks
//! canonical form, uncompressed and unvalidated. This wire format is
//! byte-identical in arkworks 0.5 and 0.6. Prepared pairing points put no
//! line coefficients on the wire, since `G2Prepared` in `ark-models-ext` is a
//! newtype that serializes as the affine point. So this module encodes
//! arkworks 0.6 types while the host side keeps arkworks 0.5, and deployed
//! node binaries need no change.
//!
//! The hook bodies mirror the `HostHooks` impls of `sp-crypto-ec-utils`
//! 0.21.1. Two contracts to keep in mind:
//!
//! - Degenerate points (bandersnatch, `z = 0`, reachable only with non-subgroup inputs): the host
//!   returns error code 4 and the hooks substitute the all-zero projective, which every downstream
//!   validity check rejects.
//! - The `Error` enum of `sp-crypto-ec-utils` is private. Its values still cross the `host_calls`
//!   API inside a `Result`, and a fieldless enum casts to `u32` without its type name, so the hooks
//!   detect code 4 with `err as u32 == 4`.
//!
//! A wire mismatch would surface as a decode panic in wasm at run time, not
//! at compile time. The tests in this module (compiled with the
//! `ec-crypto-hostcalls` feature) run the real host functions natively and
//! compare every hook against pure arkworks 0.6; they are the check that
//! closes this risk.
//!
//! # Removal
//!
//! Remove this workaround when `sp-crypto-ec-utils` moves to the arkworks 0.6
//! ext line (watch <https://github.com/paritytech/polkadot-sdk>). Then delete
//! this module and the direct `ark-*` dependencies in `Cargo.toml`, and bind
//! `Suite::Affine` and `RingSuite::Pairing` in `crypto.rs` to the
//! `sp_crypto_ec_utils` type aliases again.

use alloc::{vec, vec::Vec};
use ark_ec::{pairing::Pairing, AffineRepr, CurveConfig, CurveGroup};
use ark_ff::Zero;
use ark_scale::{
	ark_serialize::{CanonicalDeserialize, CanonicalSerialize, Compress, Validate},
	scale::{Decode, Encode},
	ArkScaleMaxEncodedLen, MaxEncodedLen,
};
use sp_crypto_ec_utils::{
	bls12_381::host_calls as bls12_381_host_calls,
	ed_on_bls12_381_bandersnatch::host_calls as bandersnatch_host_calls,
};

/// *BLS12-381* pairing engine with operations offloaded to the host.
pub type Bls12_381 = ark_bls12_381_ext::Bls12_381<HostHooks>;

/// *Ed-on-BLS12-381-Bandersnatch* affine point with operations offloaded to the host.
pub type EdwardsAffine = ark_ed_on_bls12_381_bandersnatch_ext::EdwardsAffine<HostHooks>;

type EdwardsProjective = ark_ed_on_bls12_381_bandersnatch_ext::EdwardsProjective<HostHooks>;
type BandersnatchConfig = ark_ed_on_bls12_381_bandersnatch_ext::BandersnatchConfig<HostHooks>;
type BandersnatchScalarField = <BandersnatchConfig as CurveConfig>::ScalarField;
type BandersnatchBaseField = <BandersnatchConfig as CurveConfig>::BaseField;

type G1Affine = ark_bls12_381_ext::g1::G1Affine<HostHooks>;
type G1Projective = ark_bls12_381_ext::g1::G1Projective<HostHooks>;
type G2Affine = ark_bls12_381_ext::g2::G2Affine<HostHooks>;
type G2Projective = ark_bls12_381_ext::g2::G2Projective<HostHooks>;
type G1Prepared = <Bls12_381 as Pairing>::G1Prepared;
type G2Prepared = <Bls12_381 as Pairing>::G2Prepared;
type TargetField = <Bls12_381 as Pairing>::TargetField;
type BlsScalarField = <Bls12_381 as Pairing>::ScalarField;

const FAIL_MSG: &str = "Unexpected failure, bad arguments, broken host/runtime contract; qed";

// Code 4 is `Error::DegeneratePoint`; the enum is private, see the module doc.
const DEGENERATE_POINT: u32 = 4;

const SCALE_USAGE: ark_scale::Usage = ark_scale::make_usage(Compress::No, Validate::No);
type ArkScale<T> = ark_scale::ArkScale<T, SCALE_USAGE>;

fn encode<T: CanonicalSerialize>(value: T) -> Vec<u8> {
	ArkScale::from(value).encode()
}

fn encode_iter<T: CanonicalSerialize>(iter: impl Iterator<Item = T>) -> Vec<u8> {
	encode(iter.collect::<Vec<_>>())
}

fn decode<T: CanonicalDeserialize>(mut buf: &[u8]) -> T {
	ArkScale::<T>::decode(&mut buf).map(|value| value.0).expect(FAIL_MSG)
}

fn buffer_for<T: CanonicalSerialize + ArkScaleMaxEncodedLen>() -> Vec<u8> {
	vec![0; <ArkScale<T> as MaxEncodedLen>::max_encoded_len()]
}

fn degenerate_fallback() -> EdwardsProjective {
	let zero = BandersnatchBaseField::zero();
	EdwardsProjective::new_unchecked(zero, zero, zero, zero)
}

/// Curve hooks jumping into the `sp-crypto-ec-utils` host functions.
#[derive(Copy, Clone)]
pub struct HostHooks;

impl ark_bls12_381_ext::CurveHooks for HostHooks {
	fn multi_miller_loop(
		g1: impl Iterator<Item = G1Prepared>,
		g2: impl Iterator<Item = G2Prepared>,
	) -> TargetField {
		let mut out = buffer_for::<TargetField>();
		bls12_381_host_calls::bls12_381_multi_miller_loop(
			&encode_iter(g1),
			&encode_iter(g2),
			&mut out,
		)
		.expect(FAIL_MSG);
		decode::<TargetField>(&out)
	}

	fn final_exponentiation(target: TargetField) -> TargetField {
		let mut in_out = encode(target);
		bls12_381_host_calls::bls12_381_final_exponentiation(&mut in_out).expect(FAIL_MSG);
		decode::<TargetField>(&in_out)
	}

	fn msm_g1(bases: &[G1Affine], scalars: &[BlsScalarField]) -> G1Projective {
		let mut out = buffer_for::<G1Affine>();
		bls12_381_host_calls::bls12_381_msm_g1(&encode(bases), &encode(scalars), &mut out)
			.expect(FAIL_MSG);
		decode::<G1Affine>(&out).into_group()
	}

	fn msm_g2(bases: &[G2Affine], scalars: &[BlsScalarField]) -> G2Projective {
		let mut out = buffer_for::<G2Affine>();
		bls12_381_host_calls::bls12_381_msm_g2(&encode(bases), &encode(scalars), &mut out)
			.expect(FAIL_MSG);
		decode::<G2Affine>(&out).into_group()
	}

	fn mul_projective_g1(base: &G1Projective, scalar: &[u64]) -> G1Projective {
		let mut out = buffer_for::<G1Affine>();
		bls12_381_host_calls::bls12_381_mul_g1(
			&encode(base.into_affine()),
			&encode(scalar),
			&mut out,
		)
		.expect(FAIL_MSG);
		decode::<G1Affine>(&out).into_group()
	}

	fn mul_projective_g2(base: &G2Projective, scalar: &[u64]) -> G2Projective {
		let mut out = buffer_for::<G2Affine>();
		bls12_381_host_calls::bls12_381_mul_g2(
			&encode(base.into_affine()),
			&encode(scalar),
			&mut out,
		)
		.expect(FAIL_MSG);
		decode::<G2Affine>(&out).into_group()
	}
}

impl ark_ed_on_bls12_381_bandersnatch_ext::CurveHooks for HostHooks {
	fn msm_te(bases: &[EdwardsAffine], scalars: &[BandersnatchScalarField]) -> EdwardsProjective {
		let mut out = buffer_for::<EdwardsAffine>();
		match bandersnatch_host_calls::ed_on_bls12_381_bandersnatch_msm(
			&encode(bases),
			&encode(scalars),
			&mut out,
		) {
			Ok(()) => decode::<EdwardsAffine>(&out).into_group(),
			Err(error) if error as u32 == DEGENERATE_POINT => degenerate_fallback(),
			Err(_) => panic!("{FAIL_MSG}"),
		}
	}

	fn mul_projective_te(base: &EdwardsProjective, scalar: &[u64]) -> EdwardsProjective {
		// A `z = 0` projective has no affine representative for the FFI
		// channel; apply the host-side fallback locally.
		if base.z.is_zero() {
			return degenerate_fallback();
		}
		let mut out = buffer_for::<EdwardsAffine>();
		match bandersnatch_host_calls::ed_on_bls12_381_bandersnatch_mul(
			&encode(base.into_affine()),
			&encode(scalar),
			&mut out,
		) {
			Ok(()) => decode::<EdwardsAffine>(&out).into_group(),
			Err(error) if error as u32 == DEGENERATE_POINT => degenerate_fallback(),
			Err(_) => panic!("{FAIL_MSG}"),
		}
	}
}

#[cfg(test)]
mod tests {
	//! Alignment checks between the hooks above and `sp-crypto-ec-utils`.
	//!
	//! In native builds the `host_calls` invocations run the real
	//! `sp-crypto-ec-utils` host-side code, built on arkworks 0.5, so these
	//! tests exercise the actual 0.6-to-0.5 wire bridge. `NativeHooks`
	//! computes the same operations with pure arkworks 0.6 via the default
	//! `CurveHooks` implementations and serves as the reference.

	use super::*;
	use alloc::format;
	use ark_bls12_381_ext::CurveHooks as BlsCurveHooks;
	use ark_ed_on_bls12_381_bandersnatch_ext::CurveHooks as BandersnatchCurveHooks;
	use ark_ff::{MontFp, PrimeField};

	struct NativeHooks;

	impl BlsCurveHooks for NativeHooks {}
	impl BandersnatchCurveHooks for NativeHooks {}

	type NativeEdwardsAffine = ark_ed_on_bls12_381_bandersnatch_ext::EdwardsAffine<NativeHooks>;
	type NativeBls12_381 = ark_bls12_381_ext::Bls12_381<NativeHooks>;
	type NativeG1Affine = ark_bls12_381_ext::g1::G1Affine<NativeHooks>;
	type NativeG2Affine = ark_bls12_381_ext::g2::G2Affine<NativeHooks>;

	fn wide_scalar<F: PrimeField>(seed: u8) -> F {
		F::from_le_bytes_mod_order(&[seed; 32])
	}

	// The hook parameter is a phantom type: points with equal coordinates
	// are the same curve point, so transplant coordinates between the
	// host-backed and the native-backed type.
	fn host_te_point(point: NativeEdwardsAffine) -> EdwardsAffine {
		EdwardsAffine::new_unchecked(point.x, point.y)
	}

	fn host_g1_point(point: NativeG1Affine) -> G1Affine {
		G1Affine::new_unchecked(point.x, point.y)
	}

	fn host_g2_point(point: NativeG2Affine) -> G2Affine {
		G2Affine::new_unchecked(point.x, point.y)
	}

	#[test]
	fn mul_te_matches_arkworks() {
		let scalar = wide_scalar::<BandersnatchScalarField>(0xa5).into_bigint();
		let base = NativeEdwardsAffine::generator();
		let host = <HostHooks as BandersnatchCurveHooks>::mul_projective_te(
			&host_te_point(base).into_group(),
			scalar.0.as_ref(),
		);
		let native = <NativeHooks as BandersnatchCurveHooks>::mul_projective_te(
			&base.into_group(),
			scalar.0.as_ref(),
		);
		assert_eq!(encode(host.into_affine()), encode(native.into_affine()));
	}

	#[test]
	fn msm_te_matches_arkworks() {
		let native_bases = [1u64, 2, 3].map(|multiplier| {
			(NativeEdwardsAffine::generator() * BandersnatchScalarField::from(multiplier))
				.into_affine()
		});
		let host_bases = native_bases.map(host_te_point);
		let scalars = [4u8, 5, 6].map(wide_scalar::<BandersnatchScalarField>);
		let host = <HostHooks as BandersnatchCurveHooks>::msm_te(&host_bases, &scalars);
		let native = <NativeHooks as BandersnatchCurveHooks>::msm_te(&native_bases, &scalars);
		assert_eq!(encode(host.into_affine()), encode(native.into_affine()));
	}

	#[test]
	fn mul_g1_matches_arkworks() {
		let scalar = wide_scalar::<BlsScalarField>(0x31).into_bigint();
		let base = NativeG1Affine::generator();
		let host = <HostHooks as BlsCurveHooks>::mul_projective_g1(
			&host_g1_point(base).into_group(),
			scalar.0.as_ref(),
		);
		let native = <NativeHooks as BlsCurveHooks>::mul_projective_g1(
			&base.into_group(),
			scalar.0.as_ref(),
		);
		assert_eq!(encode(host.into_affine()), encode(native.into_affine()));
	}

	#[test]
	fn mul_g2_matches_arkworks() {
		let scalar = wide_scalar::<BlsScalarField>(0x32).into_bigint();
		let base = NativeG2Affine::generator();
		let host = <HostHooks as BlsCurveHooks>::mul_projective_g2(
			&host_g2_point(base).into_group(),
			scalar.0.as_ref(),
		);
		let native = <NativeHooks as BlsCurveHooks>::mul_projective_g2(
			&base.into_group(),
			scalar.0.as_ref(),
		);
		assert_eq!(encode(host.into_affine()), encode(native.into_affine()));
	}

	#[test]
	fn msm_g1_matches_arkworks() {
		let native_bases = [1u64, 2, 3].map(|multiplier| {
			(NativeG1Affine::generator() * BlsScalarField::from(multiplier)).into_affine()
		});
		let host_bases = native_bases.map(host_g1_point);
		let scalars = [4u8, 5, 6].map(wide_scalar::<BlsScalarField>);
		let host = <HostHooks as BlsCurveHooks>::msm_g1(&host_bases, &scalars);
		let native = <NativeHooks as BlsCurveHooks>::msm_g1(&native_bases, &scalars);
		assert_eq!(encode(host.into_affine()), encode(native.into_affine()));
	}

	#[test]
	fn msm_g2_matches_arkworks() {
		let native_bases = [1u64, 2, 3].map(|multiplier| {
			(NativeG2Affine::generator() * BlsScalarField::from(multiplier)).into_affine()
		});
		let host_bases = native_bases.map(host_g2_point);
		let scalars = [4u8, 5, 6].map(wide_scalar::<BlsScalarField>);
		let host = <HostHooks as BlsCurveHooks>::msm_g2(&host_bases, &scalars);
		let native = <NativeHooks as BlsCurveHooks>::msm_g2(&native_bases, &scalars);
		assert_eq!(encode(host.into_affine()), encode(native.into_affine()));
	}

	// Covers `multi_miller_loop` and `final_exponentiation`, including the
	// prepared-point encoding on the wire.
	#[test]
	fn pairing_matches_arkworks() {
		let g1 = (NativeG1Affine::generator() * wide_scalar::<BlsScalarField>(7)).into_affine();
		let g2 = (NativeG2Affine::generator() * wide_scalar::<BlsScalarField>(9)).into_affine();
		let host = Bls12_381::pairing(host_g1_point(g1), host_g2_point(g2));
		let native = NativeBls12_381::pairing(g1, g2);
		assert_eq!(host.0, native.0);
	}

	// `multi_miller_loop` puts `Vec<G1Prepared>`/`Vec<G2Prepared>` on the
	// wire while the host decodes plain affine points, so the prepared
	// types must serialize as bare affine points.
	#[test]
	fn prepared_points_encode_as_affine_points() {
		let g1 = G1Affine::generator();
		let g2 = G2Affine::generator();
		assert_eq!(encode(G1Prepared::from(g1)), encode(g1));
		assert_eq!(encode(G2Prepared::from(g2)), encode(g2));
	}

	/// Non-subgroup point: multiplication by the scalar field modulus
	/// drives the HWCD arithmetic into a `z = 0` projective.
	fn y2_non_subgroup_point() -> NativeEdwardsAffine {
		let point = NativeEdwardsAffine::get_point_from_y_unchecked(
			BandersnatchBaseField::from(2u64),
			false,
		)
		.expect("y = 2 yields a curve point");
		assert!(point.is_on_curve());
		assert!(!point.is_in_correct_subgroup_assuming_on_curve());
		point
	}

	// Projective equality treats any two `z = 0` points as equal, so
	// compare raw coordinates.
	fn assert_degenerate_fallback(result: EdwardsProjective) {
		let expected = degenerate_fallback();
		assert_eq!(
			(result.x, result.y, result.t, result.z),
			(expected.x, expected.y, expected.t, expected.z)
		);
	}

	// The hooks detect the degenerate case with `error as u32 == 4`; the
	// `Error` enum of `sp-crypto-ec-utils` is unnameable, so pin the value
	// and the variant name through a live host call.
	#[test]
	fn host_reports_degenerate_point_as_code_4() {
		let point = y2_non_subgroup_point();
		let modulus = <BandersnatchScalarField as PrimeField>::MODULUS;

		let native = <NativeHooks as BandersnatchCurveHooks>::mul_projective_te(
			&point.into_group(),
			modulus.0.as_ref(),
		);
		assert!(native.z.is_zero(), "precondition: the input must drive the result to z = 0");

		let mut out = buffer_for::<EdwardsAffine>();
		let error = bandersnatch_host_calls::ed_on_bls12_381_bandersnatch_mul(
			&encode(host_te_point(point)),
			&encode(modulus.0.as_ref()),
			&mut out,
		)
		.expect_err("the host must report the degenerate result as an error");
		assert_eq!(error as u32, DEGENERATE_POINT);
		assert_eq!(format!("{error:?}"), "DegeneratePoint");
	}

	#[test]
	fn mul_projective_te_falls_back_on_degenerate_result() {
		let base = host_te_point(y2_non_subgroup_point());
		let modulus = <BandersnatchScalarField as PrimeField>::MODULUS;
		let result = <HostHooks as BandersnatchCurveHooks>::mul_projective_te(
			&base.into_group(),
			modulus.0.as_ref(),
		);
		assert_degenerate_fallback(result);
	}

	#[test]
	fn mul_projective_te_falls_back_on_z_zero_input() {
		let zero = BandersnatchBaseField::zero();
		let base = EdwardsProjective::new_unchecked(
			zero,
			BandersnatchBaseField::from(7u64),
			BandersnatchBaseField::from(11u64),
			zero,
		);
		let result = <HostHooks as BandersnatchCurveHooks>::mul_projective_te(&base, &[7, 0, 0, 0]);
		assert_degenerate_fallback(result);
	}

	/// Valid curve points with `d * x_a * x_b * y_a * y_b = 1`, which
	/// forces the HWCD addition to `z = 0`. Values from the
	/// `sp-crypto-ec-utils` test suite.
	fn exceptional_pair() -> (EdwardsAffine, EdwardsAffine) {
		let x_first: BandersnatchBaseField = MontFp!(
			"12611587488970178020234800979835231446181428428390492190317266241455236381927"
		);
		let y_first: BandersnatchBaseField =
			MontFp!("8625363597705895091270672088731506059935752500467284843225771956507605756711");
		let x_second: BandersnatchBaseField =
			MontFp!("5253339395048946693631279295832797565125937378490576959411837397991361739535");
		let y_second: BandersnatchBaseField = MontFp!(
			"24752777243643877000069062635360441442644758493268974317933177186378585499408"
		);
		(
			EdwardsAffine::new_unchecked(x_first, y_first),
			EdwardsAffine::new_unchecked(x_second, y_second),
		)
	}

	#[test]
	fn msm_te_falls_back_on_degenerate_result() {
		let (first, second) = exceptional_pair();
		assert!(first.is_on_curve() && second.is_on_curve());
		let scalars = [BandersnatchScalarField::from(2u64), BandersnatchScalarField::from(1u64)];
		let result = <HostHooks as BandersnatchCurveHooks>::msm_te(&[first, second], &scalars);
		assert_degenerate_fallback(result);
	}

	// The hooks translate only code 4 into the fallback and panic on every
	// other code. Pin the neighbouring codes so that 4 cannot change
	// meaning without this test failing.
	#[test]
	fn host_error_codes_match_hook_assumptions() {
		let generator = EdwardsAffine::generator();
		let scalar: &[u64] = &[7, 0, 0, 0];

		let mut small_out = [0u8; 1];
		let error = bandersnatch_host_calls::ed_on_bls12_381_bandersnatch_mul(
			&encode(generator),
			&encode(scalar),
			&mut small_out,
		)
		.expect_err("an undersized output buffer must fail");
		assert_eq!(error as u32, 1);
		assert_eq!(format!("{error:?}"), "Encode");

		let mut out = buffer_for::<EdwardsAffine>();
		let error = bandersnatch_host_calls::ed_on_bls12_381_bandersnatch_mul(
			&[0xff; 4],
			&encode(scalar),
			&mut out,
		)
		.expect_err("a garbage point encoding must fail");
		assert_eq!(error as u32, 2);
		assert_eq!(format!("{error:?}"), "Decode");

		let bases = [generator, generator];
		let scalars = [BandersnatchScalarField::from(2u64)];
		let error = bandersnatch_host_calls::ed_on_bls12_381_bandersnatch_msm(
			&encode(bases.as_ref()),
			&encode(scalars.as_ref()),
			&mut out,
		)
		.expect_err("mismatched input lengths must fail");
		assert_eq!(error as u32, 3);
		assert_eq!(format!("{error:?}"), "LengthMismatch");
	}
}
