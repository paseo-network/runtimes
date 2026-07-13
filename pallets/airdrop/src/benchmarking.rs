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

//! Benchmarks for the airdrop pallet.

use super::*;
use crate::{
	pallet::{ActionSchedule, Events, Registrations, Winners},
	Pallet as Airdrop,
};
use core::time::Duration;
use frame_benchmarking::{v2::*, BenchmarkError};
use frame_support::traits::{
	fungibles::{Create as _, Inspect, InspectHold, Mutate as _, MutateHold as _},
	tokens::fungibles,
};
use indiv_support::{
	traits::{Alias, Context},
	utils::{BigEndianU256, BigEndianU64},
};
use sp_runtime::{transaction_validity::TransactionSource, Permill};

/// Build a sr25519 VRF signature without going through `sr25519::Pair::vrf_sign`, which is gated on
/// sp-core's `full_crypto` feature and doesn't compile into the wasm runtime build.
///
/// Two adjustments vs. what sp-core's `VrfSecret::vrf_sign` produces:
///  * Rebuild the `schnorrkel::Keypair` from the pair's raw secret bytes (sp-core's `From<Pair> for
///    schnorrkel::Keypair` is `full_crypto`-only).
///  * Replace the default `getrandom`-backed RNG used by `dleq_proove` for nonce derivation with
///    [`ZeroRng`], a deterministic stand-in. In the wasm runtime there is no `getrandom`, so
///    `getrandom_or_panic` returns a `PanicRng` and the benchmark traps the moment a nonce is
///    needed. The resulting signature still verifies — the DLEQ proof is sound regardless of how
///    its nonce was sampled — but it would leak the secret outside benches; this helper is
///    bench-only by design.
pub fn vrf_sign_via_schnorrkel(
	pair: &sp_core::sr25519::Pair,
	transcript: sp_core::sr25519::vrf::VrfTranscript,
) -> sp_core::sr25519::vrf::VrfSignature {
	use sp_core::Pair as _;
	let bytes = pair.to_raw_vec();
	let secret = schnorrkel::SecretKey::from_bytes(&bytes)
		.expect("sr25519::Pair::to_raw_vec yields a valid schnorrkel SecretKey; qed");
	let keypair = secret.to_keypair();
	let inout = keypair.vrf_create_hash(transcript.0);
	let extra = merlin::Transcript::new(b"VRF");
	let (proof, _) =
		keypair.dleq_proove(schnorrkel::context::attach_rng(extra, ZeroRng), &inout, true);
	sp_core::sr25519::vrf::VrfSignature {
		pre_output: sp_core::sr25519::vrf::VrfPreOutput(inout.to_preout()),
		proof: sp_core::sr25519::vrf::VrfProof(proof),
	}
}

/// Deterministic stand-in for the system RNG in the wasm runtime, where `getrandom_or_panic`
/// otherwise panics. Always returns zeroes. **Bench-only**: deterministic nonces leak the
/// signing key in real use.
struct ZeroRng;
impl rand_core::RngCore for ZeroRng {
	fn next_u32(&mut self) -> u32 {
		0
	}
	fn next_u64(&mut self) -> u64 {
		0
	}
	fn fill_bytes(&mut self, dest: &mut [u8]) {
		for b in dest {
			*b = 0;
		}
	}
	fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
		self.fill_bytes(dest);
		Ok(())
	}
}
impl rand_core::CryptoRng for ZeroRng {}

/// Benchmark setup.
pub trait BenchmarkHelper<T: Config> {
	/// Set the timestamp in the `Clock`.
	///
	/// Must be called with `now > Duration::ZERO` before any code path that
	/// reads `UnixTime::now()`; otherwise the runtime logs a "called at genesis"
	/// error.
	fn set_unix_time(now: Duration);
	/// Construct a deterministic `AssetId` from an integer.
	fn create_asset_id_parameter(id: u32) -> AssetIdOf<T>;
	/// Build a ring-membership proof signed by `member_seed`'s key over
	/// `(context, message)`. Returns the proof and the alias that
	/// `T::MemberService::verify_membership_at_rev` will recover for it
	/// (the alias is deterministic in `(member_secret, context)` — the
	/// caller can't pick it).
	fn build_membership_proof(
		context: &Context,
		message: &[u8],
		member_seed: u32,
	) -> (ProofOf<T>, Alias);
	/// Return an `AccountId` whose sr25519 public key is the one in the
	/// returned `Pair`. It must work with `T::AccountIdToPublic`. The pair is used in the VRF
	/// signature generation and verification.
	fn account_keypair_for(seed: u32) -> (T::AccountId, sp_core::sr25519::Pair);
}

impl<T: Config> BenchmarkHelper<T> for () {
	fn set_unix_time(_now: Duration) {
		unimplemented!()
	}
	fn create_asset_id_parameter(_id: u32) -> AssetIdOf<T> {
		unimplemented!()
	}
	fn build_membership_proof(
		_context: &Context,
		_message: &[u8],
		_member_seed: u32,
	) -> (ProofOf<T>, Alias) {
		unimplemented!()
	}
	fn account_keypair_for(_seed: u32) -> (T::AccountId, sp_core::sr25519::Pair) {
		unimplemented!()
	}
}

/// All benchmark events sit at the same phase boundaries.
const REGISTRATION_STARTS: u64 = 100;
const DRAW_TIME: u64 = 200;
const END_TIME: u64 = 300;

/// Prize value used throughout the benchmarks. Derived per-asset as
/// `5 * min_balance`, so the value scales with whatever asset the runtime
/// supplies instead of being a hardcoded constant that might fall below
/// the asset's ED.
fn prize_value<T: Config>(asset_id: &AssetIdOf<T>) -> AssetBalanceOf<T> {
	let min = <T as Config>::Fungibles::minimum_balance(asset_id.clone());
	min.saturating_mul(5u32.into())
}

fn event_id(byte: u8) -> EventId {
	[byte; 32]
}

fn alias_with(byte0: u8, idx: u32) -> Alias {
	let mut a = [0u8; 32];
	a[0] = byte0;
	a[28..32].copy_from_slice(&idx.to_le_bytes());
	a
}

fn default_prize<T: Config>(
	asset_id: AssetIdOf<T>,
	max_winners: u32,
	winner_cap: Permill,
) -> AirdropPrize<AssetIdOf<T>, AssetBalanceOf<T>> {
	let asset_amount = prize_value::<T>(&asset_id);
	AirdropPrize { asset_id, asset_amount, max_winners, winner_cap }
}

fn default_info<T: Config>(
	asset_id: AssetIdOf<T>,
	max_winners: u32,
	winner_cap: Permill,
) -> EventInfoOf<T> {
	EventInfo {
		prize: default_prize::<T>(asset_id, max_winners, winner_cap),
		registration_starts: REGISTRATION_STARTS,
		draw_time: DRAW_TIME,
		end_time: END_TIME,
	}
}

/// Make sure the prize asset exists, is enabled for airdrop scheduling, and that the pot holds
/// enough free balance to cover the prize allocation. On first enable, the asset's ED is also
/// minted into the pot to mirror the real `enable_asset` flow.
fn fund_pot_for<T: Config>(asset_id: AssetIdOf<T>, max_winners: u32) -> Result<(), BenchmarkError>
where
	<T as Config>::Fungibles: fungibles::Create<AccountIdOf<T>>,
{
	let pot = Airdrop::<T>::airdrop_pot_id();
	if !<T as Config>::Fungibles::asset_exists(asset_id.clone()) {
		<T as Config>::Fungibles::create(asset_id.clone(), pot.clone(), true, 1u32.into())
			.map_err(|_| BenchmarkError::Stop("asset create"))?;
	}
	let prize = prize_value::<T>(&asset_id);
	let to_hold = prize.saturating_mul(max_winners.into());
	let mut funding = to_hold;
	if !crate::pallet::SupportedAssets::<T>::contains_key(&asset_id) {
		let min = <T as Config>::Fungibles::minimum_balance(asset_id.clone());
		funding = funding.saturating_add(min);
		crate::pallet::SupportedAssets::<T>::insert(&asset_id, min);
	}
	<T as Config>::Fungibles::mint_into(asset_id, &pot, funding)
		.map_err(|_| BenchmarkError::Stop("mint pot"))?;
	Ok(())
}

/// Insert an event directly into storage in the given status. Use this to
/// land at a specific lifecycle phase.
fn setup_event<T: Config>(asset_id: AssetIdOf<T>, id: EventId, max_winners: u32, status: Status) {
	let info = default_info::<T>(asset_id, max_winners, Permill::one());
	let event = ActiveEvent { id, info: info.clone(), status: status.clone() };
	let timestamp = Airdrop::<T>::next_action_scheduled_at(&status, &info);
	Events::<T>::insert(id, event);
	ActionSchedule::<T>::insert(BigEndianU64(timestamp), id, ());
}

/// Pre-fill `n` `Registrations` entries for `event_id`. The slot keys are mocked and concentrated
/// on the lower part of the spectrum.
fn fill_registrations<T: Config>(id: EventId, n: u32) {
	for i in 0..n {
		let slot_alias = alias_with(0x01, i);
		Registrations::<T>::insert(
			id,
			BigEndianU256::from(slot_alias),
			RegistrationEntry::<T::AccountId>::Alias { alias: slot_alias },
		);
	}
}

/// Pre-fill `n` `Winners` entries for `event_id`.
fn fill_winners<T: Config>(id: EventId, n: u32) {
	for i in 0..n {
		let slot_alias = alias_with(0xA0, i);
		Winners::<T>::insert(
			id,
			RegistrationEntry::<T::AccountId>::Alias { alias: slot_alias },
			BigEndianU256::from(slot_alias),
		);
	}
}

#[benchmarks(
	where
		<T as Config>::Fungibles: fungibles::Create<AccountIdOf<T>>,
)]
mod benches {
	use super::*;

	#[benchmark]
	fn schedule_event() -> Result<(), BenchmarkError> {
		let asset_id = T::BenchmarkHelper::create_asset_id_parameter(0);
		fund_pot_for::<T>(asset_id.clone(), 1)?;
		T::BenchmarkHelper::set_unix_time(Duration::from_secs(1));

		let id = event_id(1);
		let info = default_info::<T>(asset_id, 1, Permill::one());
		let origin = T::ManagerOrigin::try_successful_origin()
			.map_err(|_| BenchmarkError::Stop("manager origin"))?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, id, info);

		assert!(Events::<T>::contains_key(id));
		assert!(ActionSchedule::<T>::contains_key(BigEndianU64(REGISTRATION_STARTS), id));
		Ok(())
	}

	#[benchmark]
	fn enable_asset() -> Result<(), BenchmarkError> {
		let asset_id = T::BenchmarkHelper::create_asset_id_parameter(50);
		let pot = Airdrop::<T>::airdrop_pot_id();
		if !<T as Config>::Fungibles::asset_exists(asset_id.clone()) {
			<T as Config>::Fungibles::create(asset_id.clone(), pot, true, 1u32.into())
				.map_err(|_| BenchmarkError::Stop("asset create"))?;
		}
		let min = <T as Config>::Fungibles::minimum_balance(asset_id.clone());
		let source: T::AccountId = account("enable-source", 0, 0);
		<T as Config>::Fungibles::mint_into(
			asset_id.clone(),
			&source,
			min.saturating_mul(2u32.into()),
		)
		.map_err(|_| BenchmarkError::Stop("mint source"))?;
		let origin = T::ManagerOrigin::try_successful_origin()
			.map_err(|_| BenchmarkError::Stop("manager origin"))?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, asset_id.clone(), source);

		assert!(crate::pallet::SupportedAssets::<T>::contains_key(&asset_id));
		Ok(())
	}

	#[benchmark]
	fn disable_asset() -> Result<(), BenchmarkError> {
		let asset_id = T::BenchmarkHelper::create_asset_id_parameter(51);
		fund_pot_for::<T>(asset_id.clone(), 0)?;
		assert!(crate::pallet::SupportedAssets::<T>::contains_key(&asset_id));
		let beneficiary: T::AccountId = account("disable-beneficiary", 0, 0);
		let origin = T::ManagerOrigin::try_successful_origin()
			.map_err(|_| BenchmarkError::Stop("manager origin"))?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, asset_id.clone(), beneficiary);

		assert!(!crate::pallet::SupportedAssets::<T>::contains_key(&asset_id));
		Ok(())
	}

	#[benchmark]
	fn remove_scheduled_event() -> Result<(), BenchmarkError> {
		let asset_id = T::BenchmarkHelper::create_asset_id_parameter(0);
		fund_pot_for::<T>(asset_id.clone(), 1)?;
		T::BenchmarkHelper::set_unix_time(Duration::from_secs(1));

		let id = event_id(2);
		let info = default_info::<T>(asset_id, 1, Permill::one());
		let manager = T::ManagerOrigin::try_successful_origin()
			.map_err(|_| BenchmarkError::Stop("manager origin"))?;
		Airdrop::<T>::schedule_event(manager.clone(), id, info)?;

		#[extrinsic_call]
		_(manager as T::RuntimeOrigin, id);

		assert!(!Events::<T>::contains_key(id));
		assert!(!ActionSchedule::<T>::contains_key(BigEndianU64(REGISTRATION_STARTS), id));
		Ok(())
	}

	#[benchmark]
	fn participate_with_alias() -> Result<(), BenchmarkError> {
		let asset_id = T::BenchmarkHelper::create_asset_id_parameter(0);
		fund_pot_for::<T>(asset_id.clone(), 1)?;
		T::BenchmarkHelper::set_unix_time(Duration::from_secs(REGISTRATION_STARTS as u64));

		let id = event_id(3);
		setup_event::<T>(asset_id, id, 1, Status::Registering { total_participants: 0 });

		let participant_alias: Alias = alias_with(0x11, 0);
		let participant_origin =
			RegistrationEntry::<T::AccountId>::Alias { alias: participant_alias };
		let context = crate::context_for_event(&id);
		let message = codec::Encode::encode(&participant_origin);
		let (proof, alias) = T::BenchmarkHelper::build_membership_proof(&context, &message, 0);

		#[block]
		{
			Airdrop::<T>::do_participate_with_alias(id, participant_origin, proof, 0, 0)?;
		}

		assert!(Registrations::<T>::contains_key(id, BigEndianU256::from(alias)));
		Ok(())
	}

	// When participating using Bandersnatch VRF the cost is way more.
	#[benchmark]
	fn participate_with_account_via_schnorrkel_vrf() -> Result<(), BenchmarkError> {
		let asset_id = T::BenchmarkHelper::create_asset_id_parameter(0);
		fund_pot_for::<T>(asset_id.clone(), 1)?;
		T::BenchmarkHelper::set_unix_time(Duration::from_secs(REGISTRATION_STARTS as u64));

		let id = event_id(4);
		setup_event::<T>(asset_id, id, 1, Status::Registering { total_participants: 0 });

		let (account_id, pair) = T::BenchmarkHelper::account_keypair_for(0);
		let public = T::AccountIdToPublic::try_convert(account_id.clone())
			.map_err(|_| BenchmarkError::Stop("account-to-public mapping"))?;
		let transcript = crate::vrf::transcript_for_event(&id, &public);
		let signature = vrf_sign_via_schnorrkel(&pair, transcript);
		let entropy = crate::vrf::verify_and_extract_entropy(&public, &id, &signature)
			.expect("benchmark signature must verify");
		let slot = BigEndianU256::from(entropy);

		#[block]
		{
			Airdrop::<T>::do_participate_with_account(account_id, id, signature)?;
		}

		assert!(Registrations::<T>::contains_key(id, slot));

		Ok(())
	}

	#[benchmark]
	fn claim() -> Result<(), BenchmarkError> {
		let asset_id = T::BenchmarkHelper::create_asset_id_parameter(0);
		fund_pot_for::<T>(asset_id.clone(), 1)?;
		// `do_schedule` would `hold` for us.
		let pot = Airdrop::<T>::airdrop_pot_id();
		<T as Config>::Fungibles::hold(
			asset_id.clone(),
			&HoldReason::Airdrop.into(),
			&pot,
			prize_value::<T>(&asset_id),
		)?;
		T::BenchmarkHelper::set_unix_time(Duration::from_secs(DRAW_TIME as u64));

		let id = event_id(5);
		setup_event::<T>(
			asset_id,
			id,
			1,
			Status::Claiming { total_participants: 1, effective_winners: 1, claimed: 0 },
		);

		let registrant_alias: Alias = alias_with(0xC0, 0);
		let registrant = RegistrationEntry::<T::AccountId>::Alias { alias: registrant_alias };
		Winners::<T>::insert(id, registrant.clone(), BigEndianU256::from(registrant_alias));

		let beneficiary: T::AccountId = account("beneficiary", 0, 0);

		#[block]
		{
			Airdrop::<T>::do_claim(id, registrant.clone(), beneficiary)?;
		}

		assert!(!Winners::<T>::contains_key(id, &registrant));
		Ok(())
	}

	#[benchmark]
	fn start_registration() -> Result<(), BenchmarkError> {
		let asset_id = T::BenchmarkHelper::create_asset_id_parameter(0);
		fund_pot_for::<T>(asset_id.clone(), 1)?;
		T::BenchmarkHelper::set_unix_time(Duration::from_secs(REGISTRATION_STARTS as u64));

		let id = event_id(6);
		setup_event::<T>(asset_id, id, 1, Status::Scheduled);

		#[extrinsic_call]
		start_registration_authorized(frame_system::RawOrigin::Authorized, id, 0);

		assert!(matches!(
			Events::<T>::get(id).expect("event still present").status,
			Status::Registering { .. }
		));
		Ok(())
	}

	#[benchmark]
	fn close_registration() -> Result<(), BenchmarkError> {
		// Worst case for `close_registration_authorized` is the draw-initiation path:
		// randomness is consumed (hence `setup_randomness`) and the excess prize hold is
		// released because `effective_winners < max_winners`.
		let asset_id = T::BenchmarkHelper::create_asset_id_parameter(0);
		fund_pot_for::<T>(asset_id.clone(), MAX_WINNERS)?;
		let pot = Airdrop::<T>::airdrop_pot_id();
		<T as Config>::Fungibles::hold(
			asset_id.clone(),
			&HoldReason::Airdrop.into(),
			&pot,
			prize_value::<T>(&asset_id).saturating_mul(MAX_WINNERS.into()),
		)?;
		T::BenchmarkHelper::set_unix_time(Duration::from_secs(DRAW_TIME as u64));
		T::Randomness::setup_randomness();

		let id = event_id(7);
		setup_event::<T>(
			asset_id.clone(),
			id,
			MAX_WINNERS,
			Status::Registering { total_participants: 1 },
		);
		// One real registration so `total_participants > 0`.
		Registrations::<T>::insert(
			id,
			BigEndianU256::from(alias_with(0x01, 0)),
			RegistrationEntry::<T::AccountId>::Alias { alias: alias_with(0x01, 0) },
		);
		let expected_kept_hold = prize_value::<T>(&asset_id);

		#[extrinsic_call]
		close_registration_authorized(frame_system::RawOrigin::Authorized, id, 0);

		let event = Events::<T>::get(id).expect("event still present");
		assert!(matches!(
			event.status,
			Status::DrawWinners {
				effective_winners: 1,
				winners_added: 0,
				total_participants: 1,
				..
			}
		));
		assert!(EventEntropy::<T>::contains_key(id));
		let held_after =
			<T as Config>::Fungibles::balance_on_hold(asset_id, &HoldReason::Airdrop.into(), &pot);
		assert_eq!(held_after, expected_kept_hold);
		Ok(())
	}

	#[benchmark]
	fn draw_winners(n: Linear<1, { T::DrawLimit::get() }>) -> Result<(), BenchmarkError> {
		// Worst case: the draw walk reads `n` `Registrations` entries and writes the same number of
		// `Winners` entries with a wrap around the end of the map.
		let asset_id = T::BenchmarkHelper::create_asset_id_parameter(0);
		let max_winners = T::DrawLimit::get();
		fund_pot_for::<T>(asset_id.clone(), max_winners)?;
		T::BenchmarkHelper::set_unix_time(Duration::from_secs(DRAW_TIME as u64));

		let id = event_id(8);
		// Cursor sits at first-byte 0x80 (middle of the keyspace). Slot keys
		// use first-byte 0x40 (below cursor) and 0xC0 (above cursor),
		// alternating so the chain walks both arms.
		let cursor = {
			let mut a = [0u8; 32];
			a[0] = 0x80;
			BigEndianU256::from(a)
		};
		for i in 0..n {
			let first_byte = if i % 2 == 0 { 0xC0 } else { 0x40 };
			let slot_alias = alias_with(first_byte, i / 2);
			Registrations::<T>::insert(
				id,
				BigEndianU256::from(slot_alias),
				RegistrationEntry::<T::AccountId>::Alias { alias: slot_alias },
			);
		}
		setup_event::<T>(
			asset_id,
			id,
			max_winners,
			Status::DrawWinners {
				total_participants: max_winners,
				effective_winners: max_winners,
				winners_added: max_winners.saturating_sub(n),
				from_winner_key: cursor,
			},
		);

		#[extrinsic_call]
		draw_winners_authorized(frame_system::RawOrigin::Authorized, id, 0);

		assert_eq!(Winners::<T>::iter_prefix(id).count(), n as usize);
		Ok(())
	}

	#[benchmark]
	fn close_drawing() -> Result<(), BenchmarkError> {
		let asset_id = T::BenchmarkHelper::create_asset_id_parameter(0);
		fund_pot_for::<T>(asset_id.clone(), 1)?;
		T::BenchmarkHelper::set_unix_time(Duration::from_secs(DRAW_TIME as u64));

		let id = event_id(9);
		setup_event::<T>(
			asset_id,
			id,
			1,
			Status::DrawWinners {
				total_participants: 1,
				effective_winners: 1,
				winners_added: 1,
				from_winner_key: BigEndianU256::default(),
			},
		);

		#[extrinsic_call]
		close_drawing_authorized(frame_system::RawOrigin::Authorized, id, 0);

		assert!(matches!(
			Events::<T>::get(id).expect("event still present").status,
			Status::Claiming { .. }
		));
		Ok(())
	}

	#[benchmark]
	fn close_claiming() -> Result<(), BenchmarkError> {
		let asset_id = T::BenchmarkHelper::create_asset_id_parameter(0);
		fund_pot_for::<T>(asset_id.clone(), 1)?;
		T::BenchmarkHelper::set_unix_time(Duration::from_secs(END_TIME as u64));

		let id = event_id(10);
		setup_event::<T>(
			asset_id,
			id,
			1,
			Status::Claiming { total_participants: 1, effective_winners: 1, claimed: 0 },
		);

		#[extrinsic_call]
		close_claiming_authorized(frame_system::RawOrigin::Authorized, id, 0);

		assert!(matches!(
			Events::<T>::get(id).expect("event still present").status,
			Status::ClearingRegistrations { .. }
		));
		Ok(())
	}

	#[benchmark]
	fn clean_up_registrations(
		n: Linear<1, { T::ClearLimit::get() }>,
	) -> Result<(), BenchmarkError> {
		// Fill `n + 1` entries so `clean_up_registrations_inner(_, n)` clears
		// exactly `n` and leaves a leftover under a real backend (the
		// `BasicExternalities` used by the in-test `impl_benchmark_test_suite!`
		// ignores the limit and drains everything, but production
		// benchmarking honours it).
		let asset_id = T::BenchmarkHelper::create_asset_id_parameter(0);
		fund_pot_for::<T>(asset_id.clone(), 1)?;
		T::BenchmarkHelper::set_unix_time(Duration::from_secs(END_TIME as u64));

		let id = event_id(11);
		setup_event::<T>(
			asset_id,
			id,
			1,
			Status::ClearingRegistrations {
				total_participants: n.saturating_add(1),
				effective_winners: 1,
				claimed: 0,
				cleaned_registrations: 0,
			},
		);
		fill_registrations::<T>(id, n.saturating_add(1));

		#[block]
		{
			Airdrop::<T>::clean_up_registrations_inner(id, n)?;
		}

		#[cfg(not(test))]
		assert!(matches!(
			Events::<T>::get(id).expect("event still present").status,
			Status::ClearingRegistrations { .. }
		));

		Ok(())
	}

	#[benchmark]
	fn clean_up_winners(n: Linear<1, { T::ClearLimit::get() }>) -> Result<(), BenchmarkError> {
		// Same shape as `clean_up_registrations`; see the comment there.
		let asset_id = T::BenchmarkHelper::create_asset_id_parameter(0);
		fund_pot_for::<T>(asset_id.clone(), 1)?;
		T::BenchmarkHelper::set_unix_time(Duration::from_secs(END_TIME as u64));

		let id = event_id(12);
		setup_event::<T>(
			asset_id,
			id,
			1,
			Status::ClearingWinners {
				total_participants: 1,
				effective_winners: n.saturating_add(1),
				claimed: 0,
				cleaned_winners: 0,
			},
		);
		fill_winners::<T>(id, n.saturating_add(1));

		#[block]
		{
			Airdrop::<T>::clean_up_winners_inner(id, n)?;
		}

		#[cfg(not(test))]
		assert!(matches!(
			Events::<T>::get(id).expect("event still present").status,
			Status::ClearingWinners { .. }
		));

		Ok(())
	}

	#[benchmark]
	fn finalize() -> Result<(), BenchmarkError> {
		// Worst case: `unclaimed > 0`, so `release_remaining_prizes` runs.
		let asset_id = T::BenchmarkHelper::create_asset_id_parameter(0);
		fund_pot_for::<T>(asset_id.clone(), 1)?;
		let pot = Airdrop::<T>::airdrop_pot_id();
		<T as Config>::Fungibles::hold(
			asset_id.clone(),
			&HoldReason::Airdrop.into(),
			&pot,
			prize_value::<T>(&asset_id),
		)?;
		T::BenchmarkHelper::set_unix_time(Duration::from_secs(END_TIME as u64));

		let id = event_id(13);
		setup_event::<T>(
			asset_id.clone(),
			id,
			1,
			Status::Finalizing { effective_winners: 1, claimed: 0 },
		);

		#[extrinsic_call]
		finalize_authorized(frame_system::RawOrigin::Authorized, id, 0);

		assert!(!Events::<T>::contains_key(id));
		let held_after =
			<T as Config>::Fungibles::balance_on_hold(asset_id, &HoldReason::Airdrop.into(), &pot);
		assert_eq!(held_after, 0u32.into());
		Ok(())
	}

	#[benchmark]
	fn transition_clean_up_phase() -> Result<(), BenchmarkError> {
		// The transition step that fires when a clean-up phase drains its
		// backing storage. We measure the helper directly so the weight
		// can be added separately from the per-entry clear cost.
		let asset_id = T::BenchmarkHelper::create_asset_id_parameter(0);
		fund_pot_for::<T>(asset_id.clone(), 1)?;

		let id = event_id(14);
		setup_event::<T>(
			asset_id,
			id,
			1,
			Status::ClearingRegistrations {
				total_participants: 0,
				effective_winners: 1,
				claimed: 0,
				cleaned_registrations: 0,
			},
		);
		let event = Events::<T>::get(id).expect("installed");
		let next_status = Status::ClearingWinners {
			total_participants: 0,
			effective_winners: 1,
			claimed: 0,
			cleaned_winners: 0,
		};

		#[block]
		{
			Airdrop::<T>::transition_clean_up_phase(event, next_status);
		}

		assert!(matches!(
			Events::<T>::get(id).expect("event still present").status,
			Status::ClearingWinners { .. }
		));
		Ok(())
	}

	#[benchmark]
	fn authorize_lifecycle_call() -> Result<(), BenchmarkError> {
		// The authorize closures share a common shape: `ensure_local`,
		// `Events::get`, status destructure, optional time check, `valid_for`.
		// `draw_winners_authorized`'s closure is the heaviest: it reads the
		// biggest `Status` variant (`DrawWinners`, 3×u32 + BigEndianU256)
		// and builds a multi-call tag `(event_id, winners_added)` rather
		// than the single-`event_id` tag the other closures use.
		let asset_id = T::BenchmarkHelper::create_asset_id_parameter(0);
		fund_pot_for::<T>(asset_id.clone(), 1)?;
		T::BenchmarkHelper::set_unix_time(Duration::from_secs(DRAW_TIME as u64));

		let id = event_id(15);
		setup_event::<T>(
			asset_id,
			id,
			1,
			Status::DrawWinners {
				total_participants: 1,
				effective_winners: 1,
				winners_added: 0,
				from_winner_key: BigEndianU256::default(),
			},
		);

		let call = Call::<T>::draw_winners_authorized { event_id: id, discriminator: 0 };
		#[block]
		{
			call.authorize(TransactionSource::Local).unwrap().unwrap();
		}

		Ok(())
	}

	impl_benchmark_test_suite!(Airdrop, crate::mock::new_test_ext(), crate::mock::Test,);
}
