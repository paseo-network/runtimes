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

//! Coinage pallet benchmarks.

#[cfg(not(feature = "benchmark-proof-cache-regenerate"))]
mod proof_cache;

use super::*;
use crate::extension::{AsCoinage, AsCoinageInfo};
use alloc::vec;
use frame_benchmarking::{account, v2::*, BenchmarkError};
use frame_support::{
	dispatch::{DispatchInfo, PostDispatchInfo},
	pallet_prelude::Authorize,
	traits::{
		fungible::{Inspect as _, Mutate as _},
		fungibles::{Mutate as _, MutateHold as _},
		Consideration, Get, UnixTime,
	},
	BoundedVec,
};
use frame_system::RawOrigin as SystemOrigin;
use sp_runtime::{
	traits::{AppendZerosInput, DispatchTransaction, Dispatchable, One, SaturatedConversion},
	transaction_validity::TransactionSource,
	Saturating,
};
use verifiable::GenerateVerifiable;

type SecretOf<T> = <CryptoOf<T> as GenerateVerifiable>::Secret;
type ProofOf<T> = <CryptoOf<T> as GenerateVerifiable>::Proof;
type SignatureOf<T> = <CryptoOf<T> as GenerateVerifiable>::Signature;
type BoundedProofsOf<T> = BoundedVec<ProofOf<T>, <T as Config>::MaxConsolidation>;
type BoundedInputsOf<T> = BoundedVec<
	UnloadRecyclerInput<<T as Config>::MaxConsolidation>,
	<T as Config>::MaxConsolidation,
>;
type BoundedAliasesOf<T> = BoundedVec<Alias, <T as Config>::MaxConsolidation>;

struct MixedOutputScenario<T: Config> {
	aliases: BoundedAliasesOf<T>,
	alias_proofs: BoundedProofsOf<T>,
	input_value: Denomination,
	index: RingIndex,
	revision: RevisionIndex,
	dest: T::AccountId,
	external_asset_amount: FungiblesBalanceOf<T>,
	loaded_coins: BoundedVec<(Denomination, MemberOf<T>), T::MaxSplitOutputs>,
}

/// Common benchmark setup: set time, assets, fee conversion, and initialize chunks.
fn common_setup<T: Config>() {
	// Set a non-zero time (benchmarks run at genesis where time is 0)
	T::BenchmarkHelper::set_time(core::time::Duration::from_secs(3600));
	T::BenchmarkHelper::setup_assets();
	T::BenchmarkHelper::setup_fee_conversion();

	// Initialize chunks for ring-VRF operations via pallet-members.
	// Initializes both recycler and paid token ring exponents (or just one if they're equal).
	let recycler_exp = T::RecyclerRingExponent::get();
	let paid_exp = T::PaidUnloadTokenRingExponent::get();
	T::MemberService::initialize_chunks(recycler_exp);
	if paid_exp != recycler_exp {
		T::MemberService::initialize_chunks(paid_exp);
	}

	// `setup_assets` creates the instance, and with it the recycler collections.
	// Paid token collections are per period and created on demand, so pre-create
	// the current one to keep that cost out of the benchmarks.
	PaidTknManager::<T>::ensure_current_period_collection_exists()
		.expect("paid token collection creation should succeed");
}

/// The instance the benchmarks operate on.
///
/// `common_setup` creates it via `T::BenchmarkHelper::setup_assets()`. In the
/// `create_sufficient_instance` benchmark, the measured call creates it.
const INSTANCE_ID: InstanceId = 0;

/// The record of the instance the benchmarks operate on.
///
/// Any benchmark that performs `common_setup` first can call this to unwrap without noise.
fn instance<T: Config>() -> InstanceRecord<T> {
	Instances::<T>::get(INSTANCE_ID).expect("created by setup_assets in common_setup")
}

/// Underlying asset id helper for benchmarks.
fn asset_id<T: Config>() -> FungiblesAssetIdOf<T> {
	instance::<T>().asset_id
}

/// Asset unit helper for benchmarks.
fn asset_unit<T: Config>() -> FungiblesBalanceOf<T> {
	instance::<T>().asset_unit
}

/// Seed for the sponsored asset in benchmarks.
const SPONSORED_ASSET_SEED: u32 = 1_000_000;

/// A [`Config::SponsorOrigin`] origin and its account, the account is funded.
fn funded_sponsor<T: Config>() -> (<T as frame_system::Config>::RuntimeOrigin, T::AccountId) {
	let origin = T::SponsorOrigin::try_successful_origin().unwrap();
	let creator = T::SponsorOrigin::ensure_origin(origin.clone()).unwrap();
	T::InstanceCreationDeposit::ensure_successful(
		&creator,
		Pallet::<T>::instance_creation_footprint(),
	);
	// Native asset to touch the pallet account and the pot.
	let native_minimum = <T::NativeFungible as fungible::Inspect<_>>::minimum_balance();
	T::NativeFungible::mint_into(&creator, native_minimum.saturating_mul(1_000_000u32.into()))
		.unwrap();
	(origin, creator)
}

/// Mint `price * 1_000` of the configured load deposit asset amount to `who`.
fn fund_load_deposit_asset<T: Config>(who: &T::AccountId) {
	let (asset_id, price) = T::LoadDeposit::get();
	T::Fungibles::mint_into(asset_id, who, price.saturating_mul(1_000u32.into())).unwrap();
}

/// Create a sponsored instance over a fresh extra asset, returning its id and funded creator.
fn setup_sponsored_instance<T: Config>() -> (InstanceId, T::AccountId) {
	let (origin, creator) = funded_sponsor::<T>();
	let asset_id = T::BenchmarkHelper::create_extra_asset(SPONSORED_ASSET_SEED, &creator);
	let asset_unit: FungiblesBalanceOf<T> =
		(1u32 << T::MinimumExponent::get().unsigned_abs() as u32).into();
	let instance_id = NextInstanceId::<T>::get();
	Pallet::<T>::create_sponsored_instance(origin, asset_id, asset_unit, None).unwrap();
	(instance_id, creator)
}

/// A tier at the `seed`-th deposit asset id, holding one unit at a fixed price.
fn deposit_tier<T: Config>(seed: u32) -> DepositTier<FungiblesAssetIdOf<T>, FungiblesBalanceOf<T>> {
	DepositTier {
		asset_id: T::BenchmarkHelper::extra_asset_id(seed),
		price: 1_000u32.into(),
		count: 1,
	}
}

/// Put a privileged `instance_id` into the worst state a sponsored load validation can hit
/// while staying valid: mode switched to sponsored, deposit set, pot funded, and a current tier
/// priced away from the governance price so the next load rotates.
///
/// The inserted tier carries no matching hold, which is fine for validation-only benchmarks:
/// nothing here settles.
fn worst_case_sponsored_validation<T: Config>(instance_id: InstanceId) {
	let origin = T::AdminOrigin::try_successful_origin().unwrap();
	Pallet::<T>::make_instance_sponsored(origin, instance_id).unwrap();

	let (asset_id, price) = T::LoadDeposit::get();
	let pot = Pallet::<T>::pot_account(instance_id);
	fund_load_deposit_asset::<T>(&pot);

	// A price the configured one has superseded, so the next load rotates this tier out.
	let superseded_price = price.saturating_add(One::one());
	set_load_deposit_ledger::<T>(
		instance_id,
		Some(DepositTier { asset_id, price: superseded_price, count: 1 }),
		None,
	);
}

/// Overwrite the deposit ledger of `instance_id`, bypassing the load path.
fn set_load_deposit_ledger<T: Config>(
	instance_id: InstanceId,
	current: Option<DepositTier<FungiblesAssetIdOf<T>, FungiblesBalanceOf<T>>>,
	old: Option<DepositTier<FungiblesAssetIdOf<T>, FungiblesBalanceOf<T>>>,
) {
	Instances::<T>::mutate(instance_id, |maybe_record| {
		let record = maybe_record.as_mut().expect("instance exists");
		record.current_load_deposit = current;
		record.old_load_deposit = old;
	});
}

/// A [`deposit_tier`] whose asset_id exists and whose single unit is actually held on the
/// instance's pot.
fn funded_held_tier<T: Config>(
	instance_id: InstanceId,
	seed: u32,
) -> DepositTier<FungiblesAssetIdOf<T>, FungiblesBalanceOf<T>> {
	let pot = Pallet::<T>::pot_account(instance_id);
	T::BenchmarkHelper::create_extra_asset(seed, &pot);
	let tier = deposit_tier::<T>(seed);
	T::Fungibles::hold(tier.asset_id.clone(), &HoldReason::LoadDeposit.into(), &pot, tier.price)
		.unwrap();
	tier
}

/// Create a new secret key and public key from indices.
fn new_member_from<T: Config>(i: u32, seed: u32) -> (SecretOf<T>, MemberOf<T>) {
	let mut entropy = &(i, seed).encode()[..];
	let mut entropy = AppendZerosInput::new(&mut entropy);
	let secret = CryptoOf::<T>::new_secret(Decode::decode(&mut entropy).unwrap());
	let public = CryptoOf::<T>::member_from_secret(&secret);
	(secret, public)
}

/// Create a coin at a fresh address.
fn create_coin<T: Config>(value: Denomination, age: u16, seed: u32) -> T::AccountId {
	let owner: T::AccountId = account("coin_owner", seed, 0);
	let coin = Coin { instance_id: INSTANCE_ID, value, age };
	CoinsByOwner::<T>::insert(&owner, coin);
	owner
}

/// Create a signature for proof of ownership.
fn create_proof_of_ownership<T: Config>(
	secret: &SecretOf<T>,
	account_id: &T::AccountId,
) -> SignatureOf<T> {
	CryptoOf::<T>::sign(secret, &account_id.encode()[..]).expect("signing should not fail")
}

/// Setup recycler with n pending members for the given value.
/// Returns the secrets and their corresponding members.
fn setup_recycler_with_pending<T: Config>(
	value: Denomination,
	count: u32,
	seed: u32,
) -> Vec<(SecretOf<T>, MemberOf<T>)> {
	let mut members = Vec::new();
	for i in 0..count {
		let (secret, member) = new_member_from::<T>(i, seed);
		members.push((secret, member.clone()));
		RecyclerManager::<T>::load(INSTANCE_ID, value, member).expect("should load");
	}
	members
}

/// Setup a built recycler (ring completed) with n members.
/// Returns the ring index, revision, and secrets/members.
///
/// The member count is padded to fill the ring so the onboarding cohort constraint is
/// satisfied. Callers should use `members[..n]` for their actual proofs and
/// `members.iter()` for the full ring member list.
fn setup_built_recycler<T: Config>(
	value: Denomination,
	count: u32,
	seed: u32,
) -> (RingIndex, RevisionIndex, Vec<(SecretOf<T>, MemberOf<T>)>) {
	let padded_count = count.max(T::RecyclerRingExponent::get().ring_capacity());
	let members = setup_recycler_with_pending::<T>(value, padded_count, seed);
	let identifier = Pallet::<T>::recycler_collection_identifier(INSTANCE_ID, value);
	let ring_index = 0u32;
	T::MemberService::onboard_all_and_build_ring(&identifier, ring_index)
		.expect("should build ring");
	let revision =
		T::MemberService::ring_revision(&identifier, ring_index).expect("ring should be built");
	(ring_index, revision, members)
}

#[cfg(feature = "benchmark-proof-cache-regenerate")]
use alloc::string::String;
#[cfg(feature = "benchmark-proof-cache-regenerate")]
use core::fmt::Write;

#[cfg(feature = "benchmark-proof-cache-regenerate")]
fn to_hex(bytes: &[u8]) -> String {
	let mut out = String::with_capacity(bytes.len() * 2);
	for byte in bytes {
		write!(&mut out, "{byte:02x}").expect("writing to string should not fail");
	}
	out
}

#[cfg(feature = "benchmark-proof-cache-regenerate")]
fn emit_cache_entry(cache_key: &[u8; 32], proof: &[u8], alias: &Alias) {
	let entry = alloc::format!(
		"CACHE_ENTRY: (hex!(\"{}\"), &hex!(\"{}\"), hex!(\"{}\")),",
		to_hex(cache_key),
		to_hex(proof),
		to_hex(alias),
	);

	#[cfg(feature = "std")]
	eprintln!("{entry}");

	#[cfg(not(feature = "std"))]
	log::error!("{entry}");
}

/// Generate alias proof for recycler unload.
/// First checks the proof cache, falls back to computing the proof if not cached.
///
/// Proof generation takes several minutes during benchmarks, so the cache is important.
/// To regenerate the cache, build with the `benchmark-proof-cache-regenerate` feature
/// (or `coinage-benchmark-proof-cache-regenerate` on the runtime); see
/// `pallets/coinage/src/benchmarking/README.md` for the full procedure.
fn generate_alias_proof<T: Config>(
	secret: &SecretOf<T>,
	all_members: &[MemberOf<T>],
	msg: &[u8; 32],
) -> (ProofOf<T>, Alias) {
	let member = CryptoOf::<T>::member_from_secret(secret);

	#[cfg(not(feature = "benchmark-proof-cache-regenerate"))]
	if let Some(cached) = proof_cache::lookup_alias_proof::<_, ProofOf<T>>(
		T::RecyclerRingExponent::get(),
		&member,
		all_members,
		msg,
	) {
		return cached;
	}

	// Cache miss: compute the proof
	let domain_size: <CryptoOf<T> as GenerateVerifiable>::Config =
		T::RecyclerRingExponent::get().try_into().ok().expect("valid ring exponent");
	let commitment = CryptoOf::<T>::open(domain_size, &member, all_members.iter().cloned())
		.expect("should open commitment");
	let (proof, alias) = CryptoOf::<T>::create(
		commitment,
		secret,
		pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		msg.as_ref(),
	)
	.expect("should create proof");

	#[cfg(feature = "benchmark-proof-cache-regenerate")]
	{
		let cache_key = sp_crypto_hashing::blake2_256(&(&member, all_members, msg).encode());
		let encoded_proof = proof.encode();
		emit_cache_entry(&cache_key, &encoded_proof, &alias);
	}

	(proof, alias)
}

/// Setup paid unload token ring with n pending members.
/// Returns the period, ring index, and secrets/members.
fn setup_paid_token_ring_pending<T: Config>(
	count: u32,
	seed: u32,
) -> (u32, u32, Vec<(SecretOf<T>, MemberOf<T>)>) {
	let now = T::UnixTime::now().as_secs() as u32;
	let period = now.checked_div(T::PaidUnloadTokenTimePeriod::get()).unwrap_or(0);
	let index = 0u32;

	let mut members = Vec::new();
	for i in 0..count {
		let caller: T::AccountId = account("paid_caller", seed + i, 0);
		let (secret, member) = new_member_from::<T>(i, seed);
		let sig = create_proof_of_ownership::<T>(&secret, &caller);
		members.push((secret, member.clone()));
		PaidTknManager::<T>::add_member(caller, member, sig).expect("should add member");
	}

	(period, index, members)
}

/// Setup a built paid token ring with n members.
///
/// The member count is padded to fill the ring so the onboarding cohort constraint is
/// satisfied.
fn setup_built_paid_token_ring<T: Config>(
	count: u32,
	seed: u32,
) -> (u32, u32, Vec<(SecretOf<T>, MemberOf<T>)>) {
	let padded_count = count.max(T::PaidUnloadTokenRingExponent::get().ring_capacity());
	let (period, index, members) = setup_paid_token_ring_pending::<T>(padded_count, seed);
	let identifier = Pallet::<T>::paid_token_collection_identifier(period);
	T::MemberService::onboard_all_and_build_ring(&identifier, index).expect("should build ring");
	(period, index, members)
}

/// Fund the pallet account with held assets for unloading.
fn fund_pallet_account<T: Config>(amount: FungiblesBalanceOf<T>) {
	use frame_support::traits::fungibles::MutateHold;

	let pallet_account = Pallet::<T>::pallet_account();

	// Mint extra to cover existential deposit requirements
	let extra: FungiblesBalanceOf<T> = 1000u32.into();
	let total = amount.saturating_add(extra);

	// First mint to pallet account
	T::Fungibles::mint_into(asset_id::<T>(), &pallet_account, total).expect("should mint");

	// Then hold the amount (not the extra)
	T::Fungibles::hold(asset_id::<T>(), &HoldReason::Wrapped.into(), &pallet_account, amount)
		.expect("should hold");
}

/// Sets up `n` recycler rings, one per denomination, each with minimal members.
/// - Ring 0: value = min_exp
/// - Ring 1: value = min_exp + 1
/// - Ring i: value = min_exp + i
///
/// Each ring has only `RecyclerOnboardingSize` members (almost empty).
fn setup_multi_recyclers<T: Config>(
	n: u32,
	seed: u32,
) -> (
	Vec<UnloadRecyclerInput<T::MaxConsolidation>>,
	Vec<(SecretOf<T>, Vec<MemberOf<T>>)>,
	FungiblesBalanceOf<T>,
) {
	let min_exp = T::MinimumExponent::get();
	let onboarding_size = pallet::RECYCLER_ONBOARDING_SIZE;

	let mut inputs = Vec::new();
	let mut sign_data = Vec::new();
	let mut total_asset_amount: FungiblesBalanceOf<T> = 0u32.into();

	let mut seed_offset = seed;

	for i in 0..n {
		// One ring per denomination, all at ring index 0
		let value = min_exp.saturating_add(i as i8);
		let ring_index = 0u32;

		let identifier = Pallet::<T>::recycler_collection_identifier(INSTANCE_ID, value);
		let members = setup_recycler_with_pending::<T>(value, onboarding_size, seed_offset);
		seed_offset += onboarding_size;

		T::MemberService::onboard_all_and_build_ring(&identifier, ring_index)
			.expect("should build ring");
		let revision =
			T::MemberService::ring_revision(&identifier, ring_index).expect("ring should be built");

		let actual_ring_members = T::MemberService::ring_members(&identifier, ring_index);

		let (secret, _) = &members[0];
		let alias =
			CryptoOf::<T>::alias_in_context(secret, pallet::UNLOADING_RECYCLER_CONTEXT.as_ref())
				.expect("alias should be valid");

		inputs.push(UnloadRecyclerInput {
			value,
			index: ring_index,
			revision,
			aliases: vec![alias].try_into().unwrap(),
		});
		sign_data.push((secret.clone(), actual_ring_members));

		let asset_amount = Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), value)
			.expect("denomination should be in range");
		total_asset_amount = total_asset_amount.saturating_add(asset_amount);
	}

	(inputs, sign_data, total_asset_amount)
}

#[benchmarks(
	where
		<T as frame_system::Config>::RuntimeCall: From<Call<T>> +
		Dispatchable<Info = DispatchInfo, PostInfo = PostDispatchInfo>,
)]
mod benches {
	use super::*;
	use indiv_support::traits::RingMembershipProof;

	fn setup_single_recycler_unload<T: Config>(
		n: u32,
		fund_multiplier: u32,
		mode: UnloadFeeBenchMode,
	) -> (
		BoundedAliasesOf<T>,
		BoundedProofsOf<T>,
		Denomination,
		RingIndex,
		RevisionIndex,
		T::AccountId,
		FungiblesBalanceOf<T>,
	) {
		common_setup::<T>();

		// In `FromOutput` mode the unload fee is deducted from the output and the remainder is
		// transferred to a fresh destination account. A single alias's output must therefore cover
		// both the fee and the destination's minimum balance, otherwise the `n = 1` sample would
		// fail (the remainder transfer drops below the existential deposit) and get skipped,
		// leaving too few points to fit a slope.
		let mut value = T::MinimumExponent::get();
		if mode == UnloadFeeBenchMode::FromOutput {
			let fee = Pallet::<T>::quote_paid_unload_token_fee_in_asset(INSTANCE_ID)
				.expect("fee should be available after setup");
			let required = fee.saturating_add(T::Fungibles::minimum_balance(asset_id::<T>()));
			while Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), value)
				.unwrap_or_default() <
				required
			{
				assert!(
					value < T::MaximumExponent::get(),
					"no denomination up to the maximum exponent covers the unload fee and minimum balance"
				);
				value = value.saturating_add(1);
			}
		}
		let (index, revision, members) = setup_built_recycler::<T>(value, n, 0);
		let asset_amount = Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), value)
			.expect("denomination should be in range");
		let funded_amount =
			asset_amount.saturating_mul(n.into()).saturating_mul(fund_multiplier.into());
		fund_pallet_account::<T>(funded_amount);

		let msg: [u8; 32] = [0u8; 32];
		let members_only: Vec<MemberOf<T>> =
			members.iter().map(|(_, member)| member.clone()).collect();
		let aliases: BoundedAliasesOf<T> = members[..n as usize]
			.iter()
			.map(|(secret, _)| {
				CryptoOf::<T>::alias_in_context(secret, UNLOADING_RECYCLER_CONTEXT.as_ref())
					.expect("alias should be valid")
			})
			.collect::<Vec<_>>()
			.try_into()
			.unwrap();
		let bounded_proofs: BoundedProofsOf<T> = members[..n as usize]
			.iter()
			.map(|(secret, _)| generate_alias_proof::<T>(secret, &members_only, &msg).0)
			.collect::<Vec<_>>()
			.try_into()
			.unwrap();
		let dest: T::AccountId = account("dest", 0, 0);

		// `FromOutput` pre-marks the first alias in the extension before dispatch; the call then
		// verifies only the remaining `n - 1` proofs and deducts the fee from the output.
		if mode == UnloadFeeBenchMode::FromOutput {
			RecyclerManager::<T>::mark_alias_unloaded(INSTANCE_ID, value, index, aliases[0]);
		}

		(aliases, bounded_proofs, value, index, revision, dest, asset_amount)
	}

	fn setup_single_recycler_unload_non_anonymous<T: Config>(
		n: u32,
	) -> (
		UnloadRecyclerInput<T::MaxConsolidation>,
		BoundedProofsOf<T>,
		T::AccountId,
		T::AccountId,
		FungiblesBalanceOf<T>,
	) {
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		let (index, revision, members) = setup_built_recycler::<T>(value, n, 0);
		let asset_amount = Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), value)
			.expect("denomination should be in range");
		fund_pallet_account::<T>(asset_amount.saturating_mul(n.into()).saturating_mul(2u32.into()));

		let aliases: Vec<Alias> = members[..n as usize]
			.iter()
			.map(|(secret, _)| {
				CryptoOf::<T>::alias_in_context(secret, UNLOADING_RECYCLER_CONTEXT.as_ref())
					.expect("alias should be valid")
			})
			.collect();
		let input =
			UnloadRecyclerInput { value, index, revision, aliases: aliases.try_into().unwrap() };
		let inputs = vec![input.clone()];

		let caller: T::AccountId = account("caller", 0, 0);
		let dest: T::AccountId = account("dest", 0, 0);
		T::BenchmarkHelper::fund_account(
			&caller,
			asset_amount.saturating_mul(n.into()).saturating_mul(10u32.into()),
		);

		let proven_msg =
			sp_crypto_hashing::blake2_256(&(INSTANCE_ID, &inputs, &dest, &caller).encode());
		let members_only: Vec<MemberOf<T>> =
			members.iter().map(|(_, member)| member.clone()).collect();
		let bounded_proofs: BoundedProofsOf<T> = members[..n as usize]
			.iter()
			.map(|(secret, _)| generate_alias_proof::<T>(secret, &members_only, &proven_msg).0)
			.collect::<Vec<_>>()
			.try_into()
			.unwrap();

		(input, bounded_proofs, caller, dest, asset_amount)
	}

	fn setup_multi_recycler_unload_non_anonymous<T: Config>(
		n: u32,
	) -> (BoundedInputsOf<T>, BoundedProofsOf<T>, T::AccountId, T::AccountId, FungiblesBalanceOf<T>)
	{
		common_setup::<T>();

		let (inputs, sign_data, total_asset_amount) = setup_multi_recyclers::<T>(n, 0);
		let caller: T::AccountId = account("caller", 0, 0);
		let dest: T::AccountId = account("dest", 0, 0);

		fund_pallet_account::<T>(total_asset_amount);
		T::BenchmarkHelper::fund_account(&caller, total_asset_amount.saturating_mul(10u32.into()));

		let proven_msg =
			sp_crypto_hashing::blake2_256(&(INSTANCE_ID, &inputs, &dest, &caller).encode());
		let mut alias_proofs = Vec::new();
		for (secret, actual_ring_members) in &sign_data {
			let (proof, _) = generate_alias_proof::<T>(secret, actual_ring_members, &proven_msg);
			alias_proofs.push(proof);
		}
		let bounded_proofs: BoundedProofsOf<T> = alias_proofs.try_into().unwrap();
		let bounded_inputs: BoundedInputsOf<T> = inputs.try_into().unwrap();

		(bounded_inputs, bounded_proofs, caller, dest, total_asset_amount)
	}

	fn split_units_into_exact_output_pieces(total_units: u64, d: u32) -> Option<Vec<i8>> {
		if total_units == 0 || d == 0 || d < total_units.count_ones() {
			return None;
		}

		// 2. Decompose `total_units` into powers of 2 (binary representation).
		// Each set bit at position `i` becomes an output piece with exponent offset `i`.
		let mut pieces = Vec::new();
		for i in 0..64 {
			if (total_units & (1u64 << i)) != 0 {
				pieces.push(i as i8);
			}
		}

		// 3. Split pieces until we have exactly `d` outputs.
		// Splitting: one piece of 2^n becomes two pieces of 2^(n-1).
		while pieces.len() < d as usize {
			pieces.sort_unstable();
			let largest = pieces.pop()?;
			if largest == 0 {
				// Can't split a piece of value 2^0 = 1.
				return None;
			}
			// Split: 2^largest = 2^(largest-1) + 2^(largest-1)
			pieces.push(largest - 1);
			pieces.push(largest - 1);
		}

		pieces.sort_unstable();
		Some(pieces)
	}

	fn prepare_from_output_unload_recycler_into_coins_bench<T: Config>(
		input_value: Denomination,
		index: RingIndex,
		aliases: &BoundedAliasesOf<T>,
	) -> (T::AccountId, NativeBalanceOf<T>, FungiblesBalanceOf<T>) {
		let destroyed_before = TotalValueOfDestroyedCoins::<T>::get(INSTANCE_ID);
		let fee_dest = T::FeeDestination::get();
		let fee_dest_before = T::NativeFungible::balance(&fee_dest);

		// The extension premarks the first alias before dispatch in `FromOutput` mode.
		RecyclerManager::<T>::mark_alias_unloaded(INSTANCE_ID, input_value, index, aliases[0]);

		(fee_dest, fee_dest_before, destroyed_before)
	}

	/// Fee mode an unload benchmark scenario is set up for.
	#[derive(Clone, Copy, PartialEq)]
	enum UnloadFeeBenchMode {
		/// The unload fee is settled by the token; no fee is reserved from the output. All `a`
		/// proofs are verified in the call.
		Prepaid,
		/// The unload fee is deducted from the output; a fee reserve is carved out of the output.
		/// The first alias is pre-marked by the extension, so the call verifies the remaining
		/// `a - 1` proofs and additionally transfers the fee.
		FromOutput,
	}

	/// Sets up a benchmark scenario for unloading a recycler into coins.
	/// - `a`: number of input aliases (coins consumed from the recycler)
	/// - `d`: number of destination output coins to produce
	/// - `mode`: whether the scenario reserves a fee from the output (`FromOutput`) or not
	///   (`Prepaid`)
	fn setup_unload_recycler_into_coins<T: Config>(
		a: u32,
		d: u32,
		mode: UnloadFeeBenchMode,
	) -> Result<
		(
			BoundedAliasesOf<T>,
			BoundedProofsOf<T>,
			Denomination,
			RingIndex,
			RevisionIndex,
			BoundedVec<
				(Denomination, BoundedVec<T::AccountId, T::MaxSplitOutputs>),
				T::MaxSplitOutputs,
			>,
			FungiblesBalanceOf<T>,
		),
		BenchmarkError,
	> {
		common_setup::<T>();

		let min_exp = T::MinimumExponent::get();
		let max_exp = T::MaximumExponent::get();
		let amount_per_unit = Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), min_exp)
			.expect("minimum exponent should be in range");
		let required_fee = Pallet::<T>::quote_paid_unload_token_fee_in_asset(INSTANCE_ID)
			.expect("fee should be available after setup");
		let amount_per_unit_u128: u128 = amount_per_unit.saturated_into();
		let required_fee_u128: u128 = required_fee.saturated_into();
		let min_fee_units_u128 = required_fee_u128
			.saturating_add(amount_per_unit_u128.saturating_sub(1)) /
			amount_per_unit_u128;
		// `Prepaid` reserves no fee from the output (`max_fee` must be zero), so the whole input
		// value is split into the `d` outputs. `FromOutput` reserves strictly more than the
		// required unload fee so the benchmark also exercises the remainder-burn branch.
		let min_fee_units: u32 = match mode {
			UnloadFeeBenchMode::Prepaid => 0,
			UnloadFeeBenchMode::FromOutput => min_fee_units_u128
				.saturating_add(1)
				.try_into()
				.map_err(|_| BenchmarkError::Skip)?,
		};

		// Since denominations are powers of 2, the minimum number of outputs needed to represent a
		// value is the popcount of its minimum-denomination units.
		// After reserving fee units for `FromOutput`, each candidate remaining value must therefore
		// satisfy that lower bound before it can be split into exactly `d` outputs.
		// Determines the minimum input denomination needed so that `a` coins can be split into
		// exactly `d` output coins while still reserving enough value for the `FromOutput` fee
		// path. We reserve strictly more than the required unload fee so the benchmark also
		// measures the remainder burn accounting branch.
		//
		// It works by:
		// 1. Finding the smallest exponent offset `k` such that `a * 2^k` leaves enough units for
		//    `d` outputs plus at least `min_fee_units` (each input coin is worth 2^k
		//    minimum-denomination units).
		// 2. Trying fee reserves from `min_fee_units` upward until the remaining value can be
		//    decomposed and split into exactly `d` output coins.
		// 3. Grouping pieces by denomination and assigning destination accounts.
		//
		// After a fee reserve is chosen, the remaining units go through the same decomposition and
		// split process as before.
		//
		// Example: if the remaining units are 6 and `d=5`
		// - binary 110 gives initial pieces at bit positions 1 and 2: [1, 2]
		// - split pieces by halving the largest until we have 5 pieces: [1, 2] -> [1, 1, 1] -> [1,
		//   1, 0, 0] -> [1, 0, 0, 0, 0]
		// - final output: 4 coins of value 2^0=1 and 1 coin of value 2^1=2
		let mut k = 0i8;
		let (input_value, pieces, max_fee) = loop {
			// 1. Find the smallest exponent offset `k` that can fund `d` outputs and the fee
			//    reserve.
			let total_units = (a as u64).saturating_mul(1u64 << (k as u32));
			let input_value = min_exp.saturating_add(k);
			if input_value > max_exp {
				return Err(BenchmarkError::Skip);
			}
			if total_units <= (d as u64).saturating_add(u64::from(min_fee_units)) {
				k = k.saturating_add(1);
				continue;
			}
			let max_fee_units = total_units.saturating_sub(d as u64);
			// `Prepaid` reserves nothing for the fee, so only fee_units == 0 is admissible.
			let fee_units_end = match mode {
				UnloadFeeBenchMode::Prepaid => 0,
				UnloadFeeBenchMode::FromOutput => max_fee_units,
			};
			let mut found = None;
			for fee_units in u64::from(min_fee_units)..=fee_units_end {
				let available_units = total_units.saturating_sub(fee_units);
				let Some(pieces) = split_units_into_exact_output_pieces(available_units, d) else {
					continue;
				};

				let highest_bit = *pieces.last().ok_or(BenchmarkError::Skip)?;
				if min_exp.saturating_add(highest_bit) > max_exp {
					continue;
				}

				let max_fee: FungiblesBalanceOf<T> =
					(u128::from(fee_units).saturating_mul(amount_per_unit_u128)).saturated_into();
				found = Some((input_value, pieces, max_fee));
				break;
			}
			if let Some(found) = found {
				break found;
			}
			k = k.saturating_add(1);
		};

		// 4. Group pieces by denomination and assign destination accounts.
		// Result: Vec of (denomination, destinations) for the split_into parameter.
		let mut dest_idx = 0u32;
		let mut split_into: Vec<(Denomination, BoundedVec<T::AccountId, T::MaxSplitOutputs>)> =
			Vec::new();

		let mut current_val = pieces[0];
		let mut current_dests: Vec<T::AccountId> = Vec::new();

		for val in pieces {
			if val == current_val {
				current_dests.push(account("dest", dest_idx, 0));
				dest_idx += 1;
			} else {
				split_into
					.push((min_exp.saturating_add(current_val), current_dests.try_into().unwrap()));
				current_val = val;
				current_dests = vec![account("dest", dest_idx, 0)];
				dest_idx += 1;
			}
		}
		if !current_dests.is_empty() {
			split_into
				.push((min_exp.saturating_add(current_val), current_dests.try_into().unwrap()));
		}
		let split_into: BoundedVec<_, T::MaxSplitOutputs> = split_into.try_into().unwrap();

		let (index, revision, members) = setup_built_recycler::<T>(input_value, a, 50_000);
		let asset_amount =
			Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), input_value)
				.expect("denomination should be in range");
		fund_pallet_account::<T>(asset_amount.saturating_mul(a.into()));

		let msg: [u8; 32] = [0u8; 32];
		let members_only: Vec<MemberOf<T>> =
			members.iter().map(|(_, member)| member.clone()).collect();
		let aliases: BoundedAliasesOf<T> = members[..a as usize]
			.iter()
			.map(|(secret, _)| {
				CryptoOf::<T>::alias_in_context(secret, UNLOADING_RECYCLER_CONTEXT.as_ref())
					.expect("alias should be valid")
			})
			.collect::<Vec<_>>()
			.try_into()
			.unwrap();
		let bounded_proofs: BoundedProofsOf<T> = members[..a as usize]
			.iter()
			.map(|(secret, _)| generate_alias_proof::<T>(secret, &members_only, &msg).0)
			.collect::<Vec<_>>()
			.try_into()
			.unwrap();

		Ok((aliases, bounded_proofs, input_value, index, revision, split_into, max_fee))
	}

	/// Sets up a benchmark scenario for unloading a recycler into external asset and loaded coins.
	/// - `a`: number of input aliases (coins consumed from the recycler)
	/// - `d`: number of loaded-coin outputs to produce
	fn select_mixed_output_units<T: Config>(
		a: u32,
		d: u32,
		min_external_units: u64,
	) -> Result<(Denomination, u64), BenchmarkError> {
		let min_exp = T::MinimumExponent::get();
		let max_exp = T::MaximumExponent::get();

		let mut extra_exp = 0i8;
		loop {
			let input_value = min_exp.saturating_add(extra_exp);
			if input_value > max_exp {
				return Err(BenchmarkError::Skip);
			}

			let total_units =
				(a as u64).checked_shl(extra_exp as u32).ok_or(BenchmarkError::Skip)?;
			// The external portion (`total_units - loaded_coin_units`) must cover
			// `min_external_units` (the reserved unload fee in `FromOutput` mode; zero in
			// `Prepaid` mode).
			if total_units <= (d as u64).saturating_add(min_external_units) {
				extra_exp = extra_exp.saturating_add(1);
				continue;
			}

			let max_loaded_coin_units = total_units.saturating_sub(min_external_units);
			for loaded_coin_units in (d as u64..max_loaded_coin_units).rev() {
				let highest_piece_exp = if loaded_coin_units == 0 {
					0
				} else {
					63 - loaded_coin_units.leading_zeros() as i8
				};
				if d >= loaded_coin_units.count_ones() &&
					min_exp.saturating_add(highest_piece_exp) <= max_exp
				{
					return Ok((input_value, loaded_coin_units));
				}
			}

			extra_exp = extra_exp.saturating_add(1);
		}
	}

	fn decompose_loaded_coin_units(
		loaded_coin_units: u64,
		d: u32,
	) -> Result<Vec<i8>, BenchmarkError> {
		let mut loaded_coin_piece_exponents = Vec::new();
		for bit in 0..64 {
			if (loaded_coin_units & (1u64 << bit)) != 0 {
				loaded_coin_piece_exponents.push(bit as i8);
			}
		}

		while loaded_coin_piece_exponents.len() < d as usize {
			loaded_coin_piece_exponents.sort_unstable();
			let Some(largest_piece_exp) = loaded_coin_piece_exponents.pop() else {
				return Err(BenchmarkError::Skip);
			};
			if largest_piece_exp == 0 {
				return Err(BenchmarkError::Skip);
			}
			loaded_coin_piece_exponents.push(largest_piece_exp - 1);
			loaded_coin_piece_exponents.push(largest_piece_exp - 1);
		}

		loaded_coin_piece_exponents.sort_unstable();
		Ok(loaded_coin_piece_exponents)
	}

	fn setup_unload_recycler_into_external_asset_and_loaded_coins<T: Config>(
		a: u32,
		d: u32,
		mode: UnloadFeeBenchMode,
	) -> Result<MixedOutputScenario<T>, BenchmarkError> {
		common_setup::<T>();

		let min_exp = T::MinimumExponent::get();
		let amount_per_unit = Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), min_exp)
			.expect("minimum exponent should be in range");

		// `FromOutput` deducts the unload fee from the external-asset portion, so reserve strictly
		// more than the required fee there (the extra also exercises the remainder-burn branch).
		// `Prepaid` reserves nothing.
		let min_external_units: u64 = match mode {
			UnloadFeeBenchMode::Prepaid => 0,
			UnloadFeeBenchMode::FromOutput => {
				let required_fee = Pallet::<T>::quote_paid_unload_token_fee_in_asset(INSTANCE_ID)
					.expect("fee should be available after setup");
				let amount_per_unit_u128: u128 = amount_per_unit.saturated_into();
				let required_fee_u128: u128 = required_fee.saturated_into();
				let fee_units = required_fee_u128
					.saturating_add(amount_per_unit_u128.saturating_sub(1)) /
					amount_per_unit_u128;
				u64::try_from(fee_units.saturating_add(1)).map_err(|_| BenchmarkError::Skip)?
			},
		};

		let (input_value, loaded_coin_units) =
			select_mixed_output_units::<T>(a, d, min_external_units)?;

		let total_units = Pallet::<T>::denomination_to_base_units(input_value)
			.ok_or(BenchmarkError::Skip)?
			.checked_mul(a)
			.ok_or(BenchmarkError::Skip)? as u64;
		let external_asset_units =
			total_units.checked_sub(loaded_coin_units).ok_or(BenchmarkError::Skip)?;

		let loaded_coin_piece_exponents = decompose_loaded_coin_units(loaded_coin_units, d)?;

		let loaded_coins: BoundedVec<_, T::MaxSplitOutputs> = loaded_coin_piece_exponents
			.into_iter()
			.enumerate()
			.map(|(i, piece_exp)| {
				let (_secret, member_key) = new_member_from::<T>(i as u32, 10_000);
				(min_exp.saturating_add(piece_exp), member_key)
			})
			.collect::<Vec<_>>()
			.try_into()
			.map_err(|_| BenchmarkError::Skip)?;

		let (index, revision, members) = setup_built_recycler::<T>(input_value, a, 60_000);
		let asset_amount =
			Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), input_value)
				.expect("denomination should be in range");
		fund_pallet_account::<T>(asset_amount.saturating_mul(a.into()));

		let msg: [u8; 32] = [0u8; 32];
		let members_only: Vec<MemberOf<T>> =
			members.iter().map(|(_, member)| member.clone()).collect();
		let aliases: BoundedAliasesOf<T> = members[..a as usize]
			.iter()
			.map(|(secret, _)| {
				CryptoOf::<T>::alias_in_context(secret, UNLOADING_RECYCLER_CONTEXT.as_ref())
					.expect("alias should be valid")
			})
			.collect::<Vec<_>>()
			.try_into()
			.unwrap();
		let bounded_proofs: BoundedProofsOf<T> = members[..a as usize]
			.iter()
			.map(|(secret, _)| generate_alias_proof::<T>(secret, &members_only, &msg).0)
			.collect::<Vec<_>>()
			.try_into()
			.unwrap();

		let dest: T::AccountId = account("dest", 0, 0);

		// `FromOutput` pre-marks the first alias in the extension before dispatch.
		if mode == UnloadFeeBenchMode::FromOutput {
			RecyclerManager::<T>::mark_alias_unloaded(INSTANCE_ID, input_value, index, aliases[0]);
		}

		Ok(MixedOutputScenario {
			aliases,
			alias_proofs: bounded_proofs,
			input_value,
			index,
			revision,
			dest,
			external_asset_amount: amount_per_unit
				.checked_mul(&external_asset_units.saturated_into::<FungiblesBalanceOf<T>>())
				.ok_or(BenchmarkError::Skip)?,
			loaded_coins,
		})
	}

	// ==================== Coin-origin extrinsics ====================

	#[benchmark]
	fn split(n: Linear<1, { T::MaxSplitOutputs::get() }>) -> Result<(), BenchmarkError> {
		common_setup::<T>();

		// Create a coin with value that can be split into n coins
		// We need to find a value such that n coins of minimum value sum to that value
		// n = 2^k means value = min_exp + k
		let min_exp = T::MinimumExponent::get();
		// ceil(log2(n)) using integer math (no_std compatible)
		let k = if n <= 1 { 0i8 } else { (32 - (n - 1).leading_zeros()) as i8 };
		let value = min_exp.saturating_add(k);

		let age = 0u16;
		let coin_owner = create_coin::<T>(value, age, 0);

		// Create n destinations
		let mut destinations: Vec<T::AccountId> = Vec::new();
		for i in 0..n {
			destinations.push(account("dest", i, 0));
		}

		// Build split_into: all coins go to minimum value
		let split_into = vec![(min_exp, destinations.try_into().unwrap())].try_into().unwrap();

		#[extrinsic_call]
		_(
			Origin::Coin {
				coin_id: coin_owner.clone(),
				coin: Coin { instance_id: INSTANCE_ID, value, age },
			},
			split_into,
		);

		// Verify coins were created at destinations
		for i in 0..n {
			let dest: T::AccountId = account("dest", i, 0);
			assert!(CoinsByOwner::<T>::contains_key(&dest));
		}

		Ok(())
	}

	#[benchmark]
	fn transfer() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		let age = 0u16;
		let coin_owner = create_coin::<T>(value, age, 0);
		let dest: T::AccountId = account("dest", 0, 0);

		#[extrinsic_call]
		_(
			Origin::Coin {
				coin_id: coin_owner.clone(),
				coin: Coin { instance_id: INSTANCE_ID, value, age },
			},
			dest.clone(),
		);

		assert!(CoinsByOwner::<T>::contains_key(&dest));

		Ok(())
	}

	#[benchmark]
	fn load_recycler_with_coin() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		let age = 0u16;
		let coin_owner = create_coin::<T>(value, age, 0);

		let (secret, member_key) = new_member_from::<T>(0, 0);
		let proof_of_ownership = create_proof_of_ownership::<T>(&secret, &coin_owner);

		// Fund pallet account for holding
		let asset_amount = Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), value)
			.expect("denomination should be in range");
		fund_pallet_account::<T>(asset_amount);

		#[extrinsic_call]
		_(
			Origin::Coin {
				coin_id: coin_owner.clone(),
				coin: Coin { instance_id: INSTANCE_ID, value, age },
			},
			member_key.clone(),
			proof_of_ownership,
		);

		assert!(RecyclersCoinToRecycler::<T>::contains_key(&member_key));

		Ok(())
	}

	#[benchmark]
	fn pay_for_recycler_unload_fee_token_with_coin() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		// Create a coin with value >= fee
		let fee = Pallet::<T>::quote_paid_unload_token_fee_in_asset(INSTANCE_ID)
			.expect("fee should be available after setup");
		let mut value = T::MinimumExponent::get();
		while Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), value)
			.unwrap_or_default() <
			fee
		{
			value = value.saturating_add(1);
		}

		let age = 0u16;
		let coin_owner = create_coin::<T>(value, age, 0);

		let (secret, member_key) = new_member_from::<T>(0, 0);
		let proof_of_ownership = create_proof_of_ownership::<T>(&secret, &coin_owner);

		// Fund pallet account
		let asset_amount = Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), value)
			.expect("denomination should be in range");
		fund_pallet_account::<T>(asset_amount);

		// Fund fee destination with minimum balance
		let fee_dest = T::FeeDestination::get();
		T::Fungibles::mint_into(asset_id::<T>(), &fee_dest, fee).expect("should mint to fee dest");

		#[extrinsic_call]
		_(
			Origin::Coin {
				coin_id: coin_owner.clone(),
				coin: Coin { instance_id: INSTANCE_ID, value, age },
			},
			member_key.clone(),
			proof_of_ownership,
		);

		assert!(PaidUnloadTokenMembers::<T>::contains_key(&member_key));

		Ok(())
	}

	// ==================== Signed-origin extrinsics ====================

	#[benchmark]
	fn load_recycler_with_external_asset() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let caller: T::AccountId = account("caller", 0, 0);
		let value = T::MinimumExponent::get();
		let asset_amount = Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), value)
			.expect("denomination should be in range");

		// Fund caller with the asset
		T::BenchmarkHelper::fund_account(&caller, asset_amount * 2u32.into());

		let (secret, member_key) = new_member_from::<T>(0, 0);
		let proof_of_ownership = create_proof_of_ownership::<T>(&secret, &caller);

		#[extrinsic_call]
		_(
			SystemOrigin::Signed(caller.clone()),
			INSTANCE_ID,
			CodecPreservation::Protect,
			value,
			member_key.clone(),
			proof_of_ownership,
		);

		assert!(RecyclersCoinToRecycler::<T>::contains_key(&member_key));

		Ok(())
	}

	#[benchmark]
	fn pay_for_recycler_unload_fee_token_with_native() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let caller: T::AccountId = account("caller", 0, 0);

		// Fund caller with native asset_id (fee + ED so account survives the transfer)
		let fee_native = Pallet::<T>::paid_unload_token_fee_in_native();
		let ed = T::NativeFungible::minimum_balance();
		T::NativeFungible::mint_into(&caller, fee_native + ed).expect("should mint native");

		// Fund fee destination with ED so it exists before receiving the fee
		let fee_dest = T::FeeDestination::get();
		T::NativeFungible::mint_into(&fee_dest, ed).expect("should mint to fee dest");

		let (secret, member_key) = new_member_from::<T>(0, 0);
		let proof_of_ownership = create_proof_of_ownership::<T>(&secret, &caller);

		#[extrinsic_call]
		_(SystemOrigin::Signed(caller.clone()), member_key.clone(), proof_of_ownership);

		assert!(PaidUnloadTokenMembers::<T>::contains_key(&member_key));

		Ok(())
	}

	#[benchmark]
	fn pay_for_recycler_unload_fee_token_with_external_asset() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let caller: T::AccountId = account("caller", 0, 0);

		// Fund caller with external asset
		let fee = Pallet::<T>::quote_paid_unload_token_fee_in_asset(INSTANCE_ID)
			.expect("fee should be available after setup");
		T::BenchmarkHelper::fund_account(&caller, fee * 2u32.into());

		// Fund fee destination
		let fee_dest = T::FeeDestination::get();
		T::BenchmarkHelper::fund_account(&fee_dest, fee);

		let (secret, member_key) = new_member_from::<T>(0, 0);
		let proof_of_ownership = create_proof_of_ownership::<T>(&secret, &caller);
		let max_fee = Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, 1)
			.expect("fee conversion is set up by `common_setup`");

		#[extrinsic_call]
		_(
			SystemOrigin::Signed(caller.clone()),
			INSTANCE_ID,
			member_key.clone(),
			proof_of_ownership,
			max_fee,
		);

		assert!(PaidUnloadTokenMembers::<T>::contains_key(&member_key));

		Ok(())
	}

	// ==================== Root/OCW extrinsics ====================

	/// The cost scales with the number of denominations, since each gets a recycler collection,
	/// but that count comes from [`Config::MinimumExponent`] and [`Config::MaximumExponent`], so
	/// it needs no component.
	#[benchmark]
	fn create_sufficient_instance() -> Result<(), BenchmarkError> {
		// `common_setup` would create the instance, so set up only the asset.
		let asset_id = T::BenchmarkHelper::setup_asset_without_instance();

		// `2^|MinimumExponent|`, which converts losslessly at every denomination in the range.
		// Its magnitude does not affect the weight.
		let asset_unit: FungiblesBalanceOf<T> =
			(1u32 << T::MinimumExponent::get().unsigned_abs() as u32).into();

		let origin = T::AdminOrigin::try_successful_origin().unwrap();

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, asset_id.clone(), asset_unit);

		assert_eq!(Pallet::<T>::get_instance_ids(asset_id.clone()), vec![INSTANCE_ID]);
		for value in T::MinimumExponent::get()..=T::MaximumExponent::get() {
			assert!(RecyclerCollectionCreated::<T>::contains_key(INSTANCE_ID, value));
		}

		Ok(())
	}

	/// The cost scales with the number of denominations, since each gets a recycler collection,
	/// but that count comes from [`Config::MinimumExponent`] and [`Config::MaximumExponent`], so
	/// it needs no component.
	#[benchmark]
	fn create_sponsored_instance() -> Result<(), BenchmarkError> {
		let (origin, creator) = funded_sponsor::<T>();
		let asset_id = T::BenchmarkHelper::create_extra_asset(SPONSORED_ASSET_SEED, &creator);
		let asset_unit: FungiblesBalanceOf<T> =
			(1u32 << T::MinimumExponent::get().unsigned_abs() as u32).into();

		T::BenchmarkHelper::create_extra_asset(0, &creator);
		let deposit_asset_id = T::BenchmarkHelper::extra_asset_id(0);
		let funding: FungiblesBalanceOf<T> = 1_000u32.into();

		#[extrinsic_call]
		_(
			origin as T::RuntimeOrigin,
			asset_id.clone(),
			asset_unit,
			// Some initial funding is the most expensive path.
			Some((deposit_asset_id.clone(), funding)),
		);

		let instance_id =
			*Pallet::<T>::get_instance_ids(asset_id).first().expect("instance was created");
		assert_eq!(
			Instances::<T>::get(instance_id).expect("instance exists").mode,
			InstanceMode::Sponsored
		);
		assert_eq!(PotContributions::<T>::get((instance_id, &creator, &deposit_asset_id)), funding);
		Ok(())
	}

	/// The worst case funds a pot with no account for the asset_id yet: the call touches it
	/// before the transfer, in an asset asset_id (the more expensive side of the union). Only
	/// an instance switched to sponsored has such a pot, one created sponsored can be funded
	/// at creation.
	#[benchmark]
	fn fund_pot() -> Result<(), BenchmarkError> {
		common_setup::<T>();
		let admin = T::AdminOrigin::try_successful_origin().unwrap();
		Pallet::<T>::make_instance_sponsored(admin, INSTANCE_ID).unwrap();

		let (_, funder) = funded_sponsor::<T>();
		T::BenchmarkHelper::create_extra_asset(0, &funder);
		let asset_id = T::BenchmarkHelper::extra_asset_id(0);
		let amount: FungiblesBalanceOf<T> = 1_000u32.into();

		#[extrinsic_call]
		_(SystemOrigin::Signed(funder.clone()), INSTANCE_ID, asset_id.clone(), amount);

		assert_eq!(PotContributions::<T>::get((INSTANCE_ID, &funder, &asset_id)), amount);
		Ok(())
	}

	/// Worst case: the withdrawal leaves a record behind rather than removing it, the pot's
	/// account for the asset_id survives and the funder's account for it does not, so the
	/// transfer has to create it.
	#[benchmark]
	fn withdraw_pot_funds() -> Result<(), BenchmarkError> {
		let (instance_id, creator) = setup_sponsored_instance::<T>();
		T::BenchmarkHelper::create_extra_asset(0, &creator);
		let asset_id = T::BenchmarkHelper::extra_asset_id(0);
		let amount: FungiblesBalanceOf<T> = 1_000u32.into();
		Pallet::<T>::do_fund_pot(
			&creator,
			instance_id,
			asset_id.clone(),
			amount.saturating_mul(2u32.into()),
		)
		.unwrap();

		// Reap the creator's account for the currency, so the withdrawal has to create it back.
		T::Fungibles::burn_from(
			asset_id.clone(),
			&creator,
			T::Fungibles::balance(asset_id.clone(), &creator),
			Preservation::Expendable,
			Precision::Exact,
			Fortitude::Polite,
		)
		.unwrap();
		assert!(T::Fungibles::balance(asset_id.clone(), &creator).is_zero());

		#[extrinsic_call]
		_(SystemOrigin::Signed(creator.clone()), instance_id, asset_id.clone(), amount);

		assert_eq!(PotContributions::<T>::get((instance_id, &creator, &asset_id)), amount);
		assert_eq!(T::Fungibles::balance(asset_id, &creator), amount);
		Ok(())
	}

	/// Worst case: the load rotates, which writes both ledger slots instead of bumping one count.
	#[benchmark]
	fn charge_load_deposit() -> Result<(), BenchmarkError> {
		let (instance_id, creator) = setup_sponsored_instance::<T>();
		let (asset_id, price) = T::LoadDeposit::get();
		fund_load_deposit_asset::<T>(&creator);
		// Twice the price so the hold leaves the pot's account above its minimum balance.
		Pallet::<T>::do_fund_pot(
			&creator,
			instance_id,
			asset_id.clone(),
			price.saturating_mul(2u32.into()),
		)
		.unwrap();
		// A tier the configured price has superseded, with the old slot free for it to rotate
		// into. Its collateral is not held, which the charge does not look at.
		set_load_deposit_ledger::<T>(
			instance_id,
			Some(DepositTier {
				asset_id: asset_id.clone(),
				price: price.saturating_add(One::one()),
				count: 1,
			}),
			None,
		);

		#[block]
		{
			Pallet::<T>::charge_load_deposit(instance_id, 1)
				.expect("pot funded and the old slot is free");
		}

		let record = Instances::<T>::get(instance_id).expect("instance exists");
		assert_eq!(record.current_load_deposit.map(|tier| tier.count), Some(1));
		assert!(record.old_load_deposit.is_some());
		Ok(())
	}

	/// Worst case: both tiers are walked, in distinct currencies, so the settlement releases
	/// twice and drops both slots.
	#[benchmark]
	fn settle_load_deposits() -> Result<(), BenchmarkError> {
		let (instance_id, _creator) = setup_sponsored_instance::<T>();
		let old = funded_held_tier::<T>(instance_id, 0);
		let current = funded_held_tier::<T>(instance_id, 1);
		set_load_deposit_ledger::<T>(instance_id, Some(current), Some(old));

		#[block]
		{
			Pallet::<T>::settle_load_deposits(instance_id, 2);
		}

		let record = Instances::<T>::get(instance_id).expect("instance exists");
		assert!(record.old_load_deposit.is_none());
		assert!(record.current_load_deposit.is_none());
		Ok(())
	}

	/// The benchmark of reading the storage item `Instances`.
	#[benchmark]
	fn read_instance() -> Result<(), BenchmarkError> {
		let (instance_id, _creator) = setup_sponsored_instance::<T>();
		set_load_deposit_ledger::<T>(
			instance_id,
			Some(deposit_tier::<T>(1)),
			Some(deposit_tier::<T>(0)),
		);

		#[block]
		{
			Instances::<T>::get(instance_id);
		}

		Ok(())
	}

	/// Worst case: both tiers are in distinct currencies, neither of them the new one, so the
	/// collapse releases twice and takes the whole new requirement fresh (the top-up branch).
	#[benchmark]
	fn collapse_load_deposits() -> Result<(), BenchmarkError> {
		let (instance_id, creator) = setup_sponsored_instance::<T>();
		let old = funded_held_tier::<T>(instance_id, 0);
		let current = funded_held_tier::<T>(instance_id, 1);
		set_load_deposit_ledger::<T>(instance_id, Some(current), Some(old));

		// The configured deposit is in yet another asset_id, so the collapse releases both tiers
		// and takes the whole new requirement fresh out of the pot's free balance.
		let (new_asset_id, new_price) = T::LoadDeposit::get();
		let pot = Pallet::<T>::pot_account(instance_id);
		fund_load_deposit_asset::<T>(&pot);
		let _ = &creator;

		let caller: T::AccountId = account("collapser", 0, 0);

		#[extrinsic_call]
		_(SystemOrigin::Signed(caller), instance_id);

		let record = Instances::<T>::get(instance_id).expect("instance exists");
		let collapsed = record.current_load_deposit.expect("ledger collapsed");
		assert_eq!(collapsed.asset_id, new_asset_id);
		assert_eq!(collapsed.price, new_price);
		assert_eq!(collapsed.count, 2);
		assert!(record.old_load_deposit.is_none());
		Ok(())
	}

	/// Worst case: both tiers are in distinct currencies, so the release walks two of them.
	#[benchmark]
	fn make_instance_sufficient() -> Result<(), BenchmarkError> {
		let (instance_id, _creator) = setup_sponsored_instance::<T>();
		let old = funded_held_tier::<T>(instance_id, 0);
		let current = funded_held_tier::<T>(instance_id, 1);
		set_load_deposit_ledger::<T>(instance_id, Some(current), Some(old));
		let origin = T::AdminOrigin::try_successful_origin().unwrap();

		assert!(
			Instances::<T>::get(instance_id).expect("instance exists").creator.is_some(),
			"the measured worst case releases a creation deposit"
		);

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, instance_id);

		let record = Instances::<T>::get(instance_id).expect("instance exists");
		assert_eq!(record.mode, InstanceMode::Sufficient);
		assert!(record.current_load_deposit.is_none());
		assert!(record.old_load_deposit.is_none());
		// The creation deposit is returned, which is what the measured worst case includes.
		assert!(record.creator.is_none());
		Ok(())
	}

	#[benchmark]
	fn make_instance_sponsored() -> Result<(), BenchmarkError> {
		let asset_id = T::BenchmarkHelper::setup_asset_without_instance();
		let asset_unit: FungiblesBalanceOf<T> =
			(1u32 << T::MinimumExponent::get().unsigned_abs() as u32).into();
		let create_origin = T::AdminOrigin::try_successful_origin().unwrap();
		Pallet::<T>::create_sufficient_instance(create_origin.clone(), asset_id, asset_unit)
			.unwrap();

		#[extrinsic_call]
		_(create_origin as T::RuntimeOrigin, INSTANCE_ID);

		assert_eq!(
			Instances::<T>::get(INSTANCE_ID).expect("instance exists").mode,
			InstanceMode::Sponsored
		);
		Ok(())
	}

	#[benchmark]
	fn clean_recycler(
		// Number of members in the ring.
		n: Linear<1, { T::RecyclerRingExponent::get().ring_capacity() }>,
		// Number of unloaded aliases.
		m: Linear<0, { T::RecyclerRingExponent::get().ring_capacity() }>,
	) -> Result<(), BenchmarkError> {
		// Note: m > n is impossible in practice (can't have more unloaded than members), but the
		// benchmark is still valid because the cost depends on iterating over the unloaded
		// entries and, whenever coins remain recoverable, building the unloaded-aliases trie
		// root over the m entries and recording the archival commitment, independent of the
		// actual member count. The setup pads the ring to full capacity, so the archival branch
		// (including `unloaded_aliases_root`) runs for every point of the `m` sweep except
		// m == capacity (where nothing remains recoverable and it is legitimately skipped), and
		// its cost is captured by the `m` slope.
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		// Fill ring 0 with n members. This advances CurrentRingIndex to 1 and sets
		// immutable_since on ring 0.
		let (_index, _revision, _members) = setup_built_recycler::<T>(value, n, 0);
		let identifier = Pallet::<T>::recycler_collection_identifier(INSTANCE_ID, value);

		// Insert m unloaded alias-state entries to simulate consumed aliases.
		// This captures the cost of iterating over unloaded entries in `clean_unchecked`.
		let ring_index: RingIndex = 0;
		for i in 0..m {
			let mut alias: Alias = [0u8; 32];
			alias[0..4].copy_from_slice(&i.to_le_bytes());
			RecyclerAliasStates::<T>::insert(
				(INSTANCE_ID, value, ring_index, alias),
				AliasState::Unloaded,
			);
		}
		// Keep the counter in step with the entries written above; `clean_unchecked` compares the
		// two.
		RecyclersUnloadedCount::<T>::insert((INSTANCE_ID, value, ring_index), m);

		// Advance time past expiration
		let status = T::MemberService::ring_status(&identifier, 0).expect("ring exists");
		let immutable_since = status.immutable_since.expect("ring should be immutable") as u32;
		let expiration = T::RecyclerExpirationTime::get();
		T::BenchmarkHelper::set_time(core::time::Duration::from_secs(
			(immutable_since + expiration + 1) as u64,
		));

		#[extrinsic_call]
		_(SystemOrigin::Authorized, INSTANCE_ID, value);

		assert_eq!(RecyclersLastRemovedRingIndex::<T>::get(INSTANCE_ID, value), Some(0));
		// If some coins weren't unloaded, the recycler is archived.
		assert_eq!(
			RecyclersArchives::<T>::contains_key((INSTANCE_ID, value, ring_index)),
			m < status.total,
		);

		Ok(())
	}

	#[benchmark]
	fn clean_consumed_free_token(
		n: Linear<0, { pallet::CLEAN_CONSUMED_FREE_TOKEN_LIMIT }>,
	) -> Result<(), BenchmarkError> {
		common_setup::<T>();

		// Compute an expired period (period 0 is expired when time is set to 3600 by common_setup)
		let expired_period = 0u32;

		// Insert n consumed tokens for the expired period
		for i in 0..n {
			// Create unique aliases by encoding i into different positions
			let mut alias: Alias = [0u8; 32];
			alias[0..4].copy_from_slice(&i.to_le_bytes());
			ConsumedFreeUnloadTokens::<T>::insert(expired_period, alias, ());
		}

		// Verify tokens are to be removed
		assert_eq!(ConsumedFreeUnloadTokens::<T>::iter_prefix(expired_period).count(), n as usize);

		#[extrinsic_call]
		_(SystemOrigin::Authorized, expired_period);

		// Verify tokens were cleaned
		assert!(ConsumedFreeUnloadTokens::<T>::iter_prefix(expired_period).next().is_none());

		Ok(())
	}

	#[benchmark]
	fn clean_paid_unload_token_ring(
		n: Linear<1, { T::PaidUnloadTokenRingExponent::get().ring_capacity() }>,
	) -> Result<(), BenchmarkError> {
		common_setup::<T>();

		// Setup and build a paid token ring with n members
		let (period, _index, _members) = setup_built_paid_token_ring::<T>(n, 0);

		// Advance time past expiration
		let expiration_time = (period + 1)
			.saturating_mul(T::PaidUnloadTokenTimePeriod::get())
			.saturating_add(T::PaidUnloadTokenRingExpirationTime::get());
		T::BenchmarkHelper::set_time(core::time::Duration::from_secs(expiration_time as u64 + 1));

		let ring_index: RingIndex = 0;

		#[extrinsic_call]
		_(SystemOrigin::Authorized, period, ring_index);

		assert_eq!(
			PaidUnloadTokenNextRingToClean::<T>::get(BigEndianPeriod::from(period)),
			Some(ring_index + 1)
		);

		Ok(())
	}

	#[benchmark]
	fn delete_expired_paid_unload_token_collection() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		// Setup and build a paid token ring
		let (period, _index, _members) = setup_built_paid_token_ring::<T>(4, 0);

		// Advance time past expiration
		let expiration_time = (period + 1)
			.saturating_mul(T::PaidUnloadTokenTimePeriod::get())
			.saturating_add(T::PaidUnloadTokenRingExpirationTime::get());
		T::BenchmarkHelper::set_time(core::time::Duration::from_secs(expiration_time as u64 + 1));

		let ring_index: RingIndex = 0;
		let alias: Alias = [1u8; 32];
		// Insert a consumed token to trigger the worst case: the dusting path.
		PaidUnloadTokenConsumed::<T>::insert(
			(BigEndianPeriod::from(period), ring_index, alias),
			(),
		);

		Pallet::<T>::clean_paid_unload_token_ring(
			SystemOrigin::Authorized.into(),
			period,
			ring_index,
		)?;

		#[extrinsic_call]
		_(SystemOrigin::Authorized, period);

		assert!(!PaidTokenCollectionsCreated::<T>::contains_key(BigEndianPeriod::from(period)));
		// Verify the dusting flag was set (worst case)
		assert!(PaidUnloadTokenDusting::<T>::contains_key(BigEndianPeriod::from(period)));

		Ok(())
	}

	#[benchmark]
	fn clean_recycler_dust(
		n: Linear<0, { pallet::DUST_CLEANUP_BATCH_SIZE }>,
	) -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		let ring_index: RingIndex = 0;

		// Insert n unloaded alias-state entries for this (value, ring_index)
		for i in 0..n {
			let mut alias: Alias = [0u8; 32];
			alias[0..4].copy_from_slice(&i.to_le_bytes());
			RecyclerAliasStates::<T>::insert(
				(INSTANCE_ID, value, ring_index, alias),
				AliasState::Unloaded,
			);
		}

		// Set the dusting flag so the extrinsic's authorize check passes
		RecyclersDusting::<T>::insert((INSTANCE_ID, value, ring_index), ());

		#[extrinsic_call]
		_(SystemOrigin::Authorized);

		// All entries should be removed (n <= DUST_CLEANUP_BATCH_SIZE)
		assert_eq!(
			RecyclerAliasStates::<T>::iter_prefix((INSTANCE_ID, value, ring_index))
				.filter(|(_, state)| matches!(state, AliasState::Unloaded))
				.count(),
			0
		);

		Ok(())
	}

	#[benchmark]
	fn clean_paid_unload_token_dust(
		n: Linear<0, { pallet::DUST_CLEANUP_BATCH_SIZE }>,
	) -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let period: Period = 0;
		let ring_index: RingIndex = 0;

		// Insert n entries into PaidUnloadTokenConsumed for this (period, ring_index)
		for i in 0..n {
			let mut alias: Alias = [0u8; 32];
			alias[0..4].copy_from_slice(&i.to_le_bytes());
			PaidUnloadTokenConsumed::<T>::insert(
				(BigEndianPeriod::from(period), ring_index, alias),
				(),
			);
		}

		// Set the dusting flag so the extrinsic's authorize check passes
		PaidUnloadTokenDusting::<T>::insert(BigEndianPeriod::from(period), ());

		#[extrinsic_call]
		_(SystemOrigin::Authorized);

		// All entries should be removed (n <= DUST_CLEANUP_BATCH_SIZE)
		assert_eq!(
			PaidUnloadTokenConsumed::<T>::iter_prefix((BigEndianPeriod::from(period),)).count(),
			0
		);

		Ok(())
	}

	// ==================== UnloadToken-origin extrinsics ====================

	/// The benchmark for `unload_recycler_into_coin` is split into three separate benchmarks
	/// for different range of `n` (number of input aliases).
	/// This is because the cost is not linear on `n`, there is a sublinear coefficient for the cost
	/// of the batch validation of the proofs.
	/// By benchmarking on a smaller range we approximate the sublinear cost over a range.
	///
	/// The ranges are `1..=2`, `4..=8` and `8..=max`, it uses powers of two because the call
	/// is valid only for powers of two.
	#[benchmark]
	fn unload_recycler_into_coin_1_2(n: Linear<1, 2>) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, value, index, revision, dest, _asset_amount) =
			setup_single_recycler_unload::<T>(n, 1, UnloadFeeBenchMode::Prepaid);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_coin(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				INSTANCE_ID,
				aliases,
				value,
				index,
				revision,
				dest.clone(),
			)?;
		}

		assert!(CoinsByOwner::<T>::contains_key(&dest));

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_coin_4_8(n: Linear<4, 8>) -> Result<(), BenchmarkError> {
		if !n.is_power_of_two() {
			return Err(BenchmarkError::Skip);
		}

		let (aliases, bounded_proofs, value, index, revision, dest, _asset_amount) =
			setup_single_recycler_unload::<T>(n, 1, UnloadFeeBenchMode::Prepaid);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_coin(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				INSTANCE_ID,
				aliases,
				value,
				index,
				revision,
				dest.clone(),
			)?;
		}

		assert!(CoinsByOwner::<T>::contains_key(&dest));

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_coin_8_max(
		n: Linear<
			8,
			{ T::MaxConsolidation::get().min(T::RecyclerRingExponent::get().ring_capacity()) },
		>,
	) -> Result<(), BenchmarkError> {
		if !n.is_power_of_two() {
			return Err(BenchmarkError::Skip);
		}

		let (aliases, bounded_proofs, value, index, revision, dest, _asset_amount) =
			setup_single_recycler_unload::<T>(n, 1, UnloadFeeBenchMode::Prepaid);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_coin(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				INSTANCE_ID,
				aliases,
				value,
				index,
				revision,
				dest.clone(),
			)?;
		}

		assert!(CoinsByOwner::<T>::contains_key(&dest));

		Ok(())
	}

	/// The benchmark for `unload_recycler_into_external_asset` is split into three separate
	/// benchmarks for different range of `n` (number of input aliases).
	/// This is because the cost is not linear on `n`, there is a sublinear coefficient for the cost
	/// of the batch validation of the proofs.
	/// By benchmarking on a smaller range we approximate the sublinear cost over a range.
	#[benchmark]
	fn unload_recycler_into_external_asset_prepaid_1_2(
		n: Linear<1, 2>,
	) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, value, index, revision, dest, asset_amount) =
			setup_single_recycler_unload::<T>(n, 2, UnloadFeeBenchMode::Prepaid);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				INSTANCE_ID,
				aliases,
				value,
				index,
				revision,
				dest.clone(),
				Zero::zero(),
			)?;
		}

		assert_eq!(T::Fungibles::balance(asset_id::<T>(), &dest), asset_amount * n.into(),);

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_external_asset_prepaid_3_8(
		n: Linear<3, 8>,
	) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, value, index, revision, dest, asset_amount) =
			setup_single_recycler_unload::<T>(n, 2, UnloadFeeBenchMode::Prepaid);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				INSTANCE_ID,
				aliases,
				value,
				index,
				revision,
				dest.clone(),
				Zero::zero(),
			)?;
		}

		assert_eq!(T::Fungibles::balance(asset_id::<T>(), &dest), asset_amount * n.into(),);

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_external_asset_prepaid_9_max(
		n: Linear<
			9,
			{ T::MaxConsolidation::get().min(T::RecyclerRingExponent::get().ring_capacity()) },
		>,
	) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, value, index, revision, dest, asset_amount) =
			setup_single_recycler_unload::<T>(n, 2, UnloadFeeBenchMode::Prepaid);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				INSTANCE_ID,
				aliases,
				value,
				index,
				revision,
				dest.clone(),
				Zero::zero(),
			)?;
		}

		assert_eq!(T::Fungibles::balance(asset_id::<T>(), &dest), asset_amount * n.into(),);

		Ok(())
	}

	/// `FromOutput` counterparts of the `unload_recycler_into_external_asset` buckets.
	///
	/// The `FromOutput` path verifies `n - 1` proofs (the first alias is pre-marked in the
	/// extension) and additionally deducts the unload fee from the output and burns the remainder,
	/// whereas the `Prepaid` buckets verify all `n` proofs and move no fee. Both are measured; the
	/// `AsCoinage` extension charges the difference for the mode it mints (see
	/// the `FromOutput` `PostDispatchInfo` refund).
	///
	/// The low bucket sweeps `n` over `1..=3` even though it only serves `n <= 2`. At `n = 1` the
	/// single alias is the pre-marked fee recycler, so no recycler ring is unloaded and the
	/// ring-unload storage keys are never touched. Those keys are first read at `n = 2`, so a
	/// `1..=2` sweep would leave them with a single data point and the per-prefix proof-size
	/// regression fails with "only 1 unique value". Extending to `n = 3` gives every prefix two
	/// distinct alias counts. The cost is `base + (n - 1) * per_unload`, linear in `n`, so the fit
	/// stays accurate for the `n <= 2` values actually served.
	#[benchmark]
	fn unload_recycler_into_external_asset_from_output_1_2(
		n: Linear<1, 3>,
	) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, value, index, revision, dest, _asset_amount) =
			setup_single_recycler_unload::<T>(n, 2, UnloadFeeBenchMode::FromOutput);
		let dest_before = T::Fungibles::balance(asset_id::<T>(), &dest);
		let fee_dest = T::FeeDestination::get();
		let fee_dest_before = T::NativeFungible::balance(&fee_dest);

		let max_fee = Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, 1)
			.expect("fee conversion is set up by `common_setup`");

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::FromOutput {
						fee_recycler_value: value,
						fee_recycler_index: index,
					},
				}
				.into(),
				INSTANCE_ID,
				aliases,
				value,
				index,
				revision,
				dest.clone(),
				max_fee,
			)?;
		}

		// The destination received the output net of the fee, which went to the fee destination.
		assert!(T::Fungibles::balance(asset_id::<T>(), &dest) > dest_before);
		assert!(T::NativeFungible::balance(&fee_dest) > fee_dest_before);

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_external_asset_from_output_3_8(
		n: Linear<3, 8>,
	) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, value, index, revision, dest, _asset_amount) =
			setup_single_recycler_unload::<T>(n, 2, UnloadFeeBenchMode::FromOutput);
		let dest_before = T::Fungibles::balance(asset_id::<T>(), &dest);
		let fee_dest = T::FeeDestination::get();
		let fee_dest_before = T::NativeFungible::balance(&fee_dest);

		let max_fee = Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, 1)
			.expect("fee conversion is set up by `common_setup`");

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::FromOutput {
						fee_recycler_value: value,
						fee_recycler_index: index,
					},
				}
				.into(),
				INSTANCE_ID,
				aliases,
				value,
				index,
				revision,
				dest.clone(),
				max_fee,
			)?;
		}

		assert!(T::Fungibles::balance(asset_id::<T>(), &dest) > dest_before);
		assert!(T::NativeFungible::balance(&fee_dest) > fee_dest_before);

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_external_asset_from_output_9_max(
		n: Linear<
			9,
			{ T::MaxConsolidation::get().min(T::RecyclerRingExponent::get().ring_capacity()) },
		>,
	) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, value, index, revision, dest, _asset_amount) =
			setup_single_recycler_unload::<T>(n, 2, UnloadFeeBenchMode::FromOutput);
		let dest_before = T::Fungibles::balance(asset_id::<T>(), &dest);
		let fee_dest = T::FeeDestination::get();
		let fee_dest_before = T::NativeFungible::balance(&fee_dest);

		let max_fee = Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, 1)
			.expect("fee conversion is set up by `common_setup`");

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::FromOutput {
						fee_recycler_value: value,
						fee_recycler_index: index,
					},
				}
				.into(),
				INSTANCE_ID,
				aliases,
				value,
				index,
				revision,
				dest.clone(),
				max_fee,
			)?;
		}

		assert!(T::Fungibles::balance(asset_id::<T>(), &dest) > dest_before);
		assert!(T::NativeFungible::balance(&fee_dest) > fee_dest_before);

		Ok(())
	}

	/// The benchmark for `unload_recycler_into_external_asset_and_loaded_coins` is split into three
	/// separate benchmarks for different range of `a` (number of input aliases).
	/// This is because the cost is not linear on `a`, there is a sublinear coefficient for the cost
	/// of the batch validation of the proofs.
	/// By benchmarking on a smaller range we approximate the sublinear cost over a range.
	///
	/// `d` scales the loaded-coin output count over the full `MaxSplitOutputs` range.
	#[benchmark]
	fn unload_recycler_into_external_asset_and_loaded_coins_prepaid_1_2(
		a: Linear<1, 2>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let scenario = setup_unload_recycler_into_external_asset_and_loaded_coins::<T>(
			a,
			d,
			UnloadFeeBenchMode::Prepaid,
		)?;
		let aliases_copy = scenario.aliases.clone();
		let loaded_coins_copy = scenario.loaded_coins.clone();

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset_and_loaded_coins(
				Origin::UnloadToken {
					alias_proofs: scenario.alias_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				INSTANCE_ID,
				scenario.aliases,
				scenario.input_value,
				scenario.index,
				scenario.revision,
				scenario.dest.clone(),
				scenario.external_asset_amount,
				scenario.loaded_coins,
				Zero::zero(),
			)?;
		}

		assert_eq!(
			T::Fungibles::balance(asset_id::<T>(), &scenario.dest),
			scenario.external_asset_amount,
		);
		for alias in &aliases_copy {
			assert!(matches!(
				RecyclerAliasStates::<T>::get((
					INSTANCE_ID,
					scenario.input_value,
					scenario.index,
					alias
				)),
				Some(AliasState::Unloaded),
			));
		}
		for (value, member_key) in &loaded_coins_copy {
			assert_eq!(RecyclersCoinToRecycler::<T>::get(member_key), Some((INSTANCE_ID, *value)));
		}

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_external_asset_and_loaded_coins_prepaid_3_8(
		a: Linear<3, 8>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let scenario = setup_unload_recycler_into_external_asset_and_loaded_coins::<T>(
			a,
			d,
			UnloadFeeBenchMode::Prepaid,
		)?;
		let aliases_copy = scenario.aliases.clone();
		let loaded_coins_copy = scenario.loaded_coins.clone();

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset_and_loaded_coins(
				Origin::UnloadToken {
					alias_proofs: scenario.alias_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				INSTANCE_ID,
				scenario.aliases,
				scenario.input_value,
				scenario.index,
				scenario.revision,
				scenario.dest.clone(),
				scenario.external_asset_amount,
				scenario.loaded_coins,
				Zero::zero(),
			)?;
		}

		assert_eq!(
			T::Fungibles::balance(asset_id::<T>(), &scenario.dest),
			scenario.external_asset_amount,
		);
		for alias in &aliases_copy {
			assert!(matches!(
				RecyclerAliasStates::<T>::get((
					INSTANCE_ID,
					scenario.input_value,
					scenario.index,
					alias
				)),
				Some(AliasState::Unloaded),
			));
		}
		for (value, member_key) in &loaded_coins_copy {
			assert_eq!(RecyclersCoinToRecycler::<T>::get(member_key), Some((INSTANCE_ID, *value)));
		}

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_external_asset_and_loaded_coins_prepaid_9_max(
		a: Linear<
			9,
			{ T::MaxConsolidation::get().min(T::RecyclerRingExponent::get().ring_capacity()) },
		>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let scenario = setup_unload_recycler_into_external_asset_and_loaded_coins::<T>(
			a,
			d,
			UnloadFeeBenchMode::Prepaid,
		)?;
		let aliases_copy = scenario.aliases.clone();
		let loaded_coins_copy = scenario.loaded_coins.clone();

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset_and_loaded_coins(
				Origin::UnloadToken {
					alias_proofs: scenario.alias_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				INSTANCE_ID,
				scenario.aliases,
				scenario.input_value,
				scenario.index,
				scenario.revision,
				scenario.dest.clone(),
				scenario.external_asset_amount,
				scenario.loaded_coins,
				Zero::zero(),
			)?;
		}

		assert_eq!(
			T::Fungibles::balance(asset_id::<T>(), &scenario.dest),
			scenario.external_asset_amount,
		);
		for alias in &aliases_copy {
			assert!(matches!(
				RecyclerAliasStates::<T>::get((
					INSTANCE_ID,
					scenario.input_value,
					scenario.index,
					alias
				)),
				Some(AliasState::Unloaded),
			));
		}
		for (value, member_key) in &loaded_coins_copy {
			assert_eq!(RecyclersCoinToRecycler::<T>::get(member_key), Some((INSTANCE_ID, *value)));
		}

		Ok(())
	}

	/// `FromOutput` counterparts of the `unload_recycler_into_external_asset_and_loaded_coins`
	/// buckets.
	///
	/// The `FromOutput` path verifies `a - 1` proofs (the first alias is pre-marked in the
	/// extension) and additionally deducts the unload fee from the external-asset portion and
	/// burns the remainder, whereas the `Prepaid` buckets verify all `a` proofs and move no fee.
	/// Both are measured; the `AsCoinage` extension charges the difference for the mode it mints
	/// (see the `FromOutput` `PostDispatchInfo` refund).
	///
	/// The low bucket sweeps `a` over `1..=3` even though it only serves `a <= 2`. At `a = 1` the
	/// single alias is the pre-marked fee recycler, so no recycler ring is unloaded and the
	/// ring-unload storage keys are never touched. Those keys are first read at `a = 2`, so a
	/// `1..=2` sweep would leave them with a single data point and the per-prefix proof-size
	/// regression fails with "only 1 unique value". Extending to `a = 3` gives every prefix two
	/// distinct alias counts. The cost is `base + (a - 1) * per_unload`, linear in `a`, so the fit
	/// stays accurate for the `a <= 2` values actually served.
	#[benchmark]
	fn unload_recycler_into_external_asset_and_loaded_coins_from_output_1_2(
		a: Linear<1, 3>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let scenario = setup_unload_recycler_into_external_asset_and_loaded_coins::<T>(
			a,
			d,
			UnloadFeeBenchMode::FromOutput,
		)?;
		let aliases_copy = scenario.aliases.clone();
		let loaded_coins_copy = scenario.loaded_coins.clone();
		let dest_before = T::Fungibles::balance(asset_id::<T>(), &scenario.dest);
		let fee_dest = T::FeeDestination::get();
		let fee_dest_before = T::NativeFungible::balance(&fee_dest);

		let max_fee = Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, 1)
			.expect("fee conversion is set up by `common_setup`");

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset_and_loaded_coins(
				Origin::UnloadToken {
					alias_proofs: scenario.alias_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::FromOutput {
						fee_recycler_value: scenario.input_value,
						fee_recycler_index: scenario.index,
					},
				}
				.into(),
				INSTANCE_ID,
				scenario.aliases,
				scenario.input_value,
				scenario.index,
				scenario.revision,
				scenario.dest.clone(),
				scenario.external_asset_amount,
				scenario.loaded_coins,
				max_fee,
			)?;
		}

		// The destination received the external portion net of the fee, which went to the fee
		// destination.
		let dest_balance = T::Fungibles::balance(asset_id::<T>(), &scenario.dest);
		assert!(dest_balance > dest_before && dest_balance < scenario.external_asset_amount);
		assert!(T::NativeFungible::balance(&fee_dest) > fee_dest_before);
		for alias in &aliases_copy {
			assert!(matches!(
				RecyclerAliasStates::<T>::get((
					INSTANCE_ID,
					scenario.input_value,
					scenario.index,
					alias
				)),
				Some(AliasState::Unloaded),
			));
		}
		for (value, member_key) in &loaded_coins_copy {
			assert_eq!(RecyclersCoinToRecycler::<T>::get(member_key), Some((INSTANCE_ID, *value)));
		}

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_external_asset_and_loaded_coins_from_output_3_8(
		a: Linear<3, 8>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let scenario = setup_unload_recycler_into_external_asset_and_loaded_coins::<T>(
			a,
			d,
			UnloadFeeBenchMode::FromOutput,
		)?;
		let aliases_copy = scenario.aliases.clone();
		let loaded_coins_copy = scenario.loaded_coins.clone();
		let dest_before = T::Fungibles::balance(asset_id::<T>(), &scenario.dest);
		let fee_dest = T::FeeDestination::get();
		let fee_dest_before = T::NativeFungible::balance(&fee_dest);

		let max_fee = Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, 1)
			.expect("fee conversion is set up by `common_setup`");

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset_and_loaded_coins(
				Origin::UnloadToken {
					alias_proofs: scenario.alias_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::FromOutput {
						fee_recycler_value: scenario.input_value,
						fee_recycler_index: scenario.index,
					},
				}
				.into(),
				INSTANCE_ID,
				scenario.aliases,
				scenario.input_value,
				scenario.index,
				scenario.revision,
				scenario.dest.clone(),
				scenario.external_asset_amount,
				scenario.loaded_coins,
				max_fee,
			)?;
		}

		let dest_balance = T::Fungibles::balance(asset_id::<T>(), &scenario.dest);
		assert!(dest_balance > dest_before && dest_balance < scenario.external_asset_amount);
		assert!(T::NativeFungible::balance(&fee_dest) > fee_dest_before);
		for alias in &aliases_copy {
			assert!(matches!(
				RecyclerAliasStates::<T>::get((
					INSTANCE_ID,
					scenario.input_value,
					scenario.index,
					alias
				)),
				Some(AliasState::Unloaded),
			));
		}
		for (value, member_key) in &loaded_coins_copy {
			assert_eq!(RecyclersCoinToRecycler::<T>::get(member_key), Some((INSTANCE_ID, *value)));
		}

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_external_asset_and_loaded_coins_from_output_9_max(
		a: Linear<
			9,
			{ T::MaxConsolidation::get().min(T::RecyclerRingExponent::get().ring_capacity()) },
		>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let scenario = setup_unload_recycler_into_external_asset_and_loaded_coins::<T>(
			a,
			d,
			UnloadFeeBenchMode::FromOutput,
		)?;
		let aliases_copy = scenario.aliases.clone();
		let loaded_coins_copy = scenario.loaded_coins.clone();
		let dest_before = T::Fungibles::balance(asset_id::<T>(), &scenario.dest);
		let fee_dest = T::FeeDestination::get();
		let fee_dest_before = T::NativeFungible::balance(&fee_dest);

		let max_fee = Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, 1)
			.expect("fee conversion is set up by `common_setup`");

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset_and_loaded_coins(
				Origin::UnloadToken {
					alias_proofs: scenario.alias_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::FromOutput {
						fee_recycler_value: scenario.input_value,
						fee_recycler_index: scenario.index,
					},
				}
				.into(),
				INSTANCE_ID,
				scenario.aliases,
				scenario.input_value,
				scenario.index,
				scenario.revision,
				scenario.dest.clone(),
				scenario.external_asset_amount,
				scenario.loaded_coins,
				max_fee,
			)?;
		}

		let dest_balance = T::Fungibles::balance(asset_id::<T>(), &scenario.dest);
		assert!(dest_balance > dest_before && dest_balance < scenario.external_asset_amount);
		assert!(T::NativeFungible::balance(&fee_dest) > fee_dest_before);
		for alias in &aliases_copy {
			assert!(matches!(
				RecyclerAliasStates::<T>::get((
					INSTANCE_ID,
					scenario.input_value,
					scenario.index,
					alias
				)),
				Some(AliasState::Unloaded),
			));
		}
		for (value, member_key) in &loaded_coins_copy {
			assert_eq!(RecyclersCoinToRecycler::<T>::get(member_key), Some((INSTANCE_ID, *value)));
		}

		Ok(())
	}

	/// The benchmark for `unload_recycler_into_external_asset_non_anonymous` is split into three
	/// separate benchmarks for different range of `n` (number of input aliases).
	/// This is because the cost is not linear on `n`, there is a sublinear coefficient for the cost
	/// of the batch validation of the proofs.
	/// By benchmarking on a smaller range we approximate the sublinear cost over a range.
	#[benchmark]
	fn unload_recycler_into_external_asset_non_anonymous_1_2(
		n: Linear<1, 2>,
	) -> Result<(), BenchmarkError> {
		let (input, bounded_proofs, caller, dest, asset_amount) =
			setup_single_recycler_unload_non_anonymous::<T>(n);

		let max_fee = Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, 1)
			.expect("fee conversion is set up by `common_setup`");

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset_non_anonymous(
				frame_system::RawOrigin::Signed(caller).into(),
				INSTANCE_ID,
				input,
				bounded_proofs,
				dest.clone(),
				FeeCurrency::ExternalAsset,
				max_fee,
			)?;
		}

		assert_eq!(T::Fungibles::balance(asset_id::<T>(), &dest), asset_amount * n.into(),);

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_external_asset_non_anonymous_3_8(
		n: Linear<3, 8>,
	) -> Result<(), BenchmarkError> {
		let (input, bounded_proofs, caller, dest, asset_amount) =
			setup_single_recycler_unload_non_anonymous::<T>(n);

		let max_fee = Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, 1)
			.expect("fee conversion is set up by `common_setup`");

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset_non_anonymous(
				frame_system::RawOrigin::Signed(caller).into(),
				INSTANCE_ID,
				input,
				bounded_proofs,
				dest.clone(),
				FeeCurrency::ExternalAsset,
				max_fee,
			)?;
		}

		assert_eq!(T::Fungibles::balance(asset_id::<T>(), &dest), asset_amount * n.into(),);

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_external_asset_non_anonymous_9_max(
		n: Linear<
			9,
			{ T::MaxConsolidation::get().min(T::RecyclerRingExponent::get().ring_capacity()) },
		>,
	) -> Result<(), BenchmarkError> {
		let (input, bounded_proofs, caller, dest, asset_amount) =
			setup_single_recycler_unload_non_anonymous::<T>(n);

		let max_fee = Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, 1)
			.expect("fee conversion is set up by `common_setup`");

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset_non_anonymous(
				frame_system::RawOrigin::Signed(caller).into(),
				INSTANCE_ID,
				input,
				bounded_proofs,
				dest.clone(),
				FeeCurrency::ExternalAsset,
				max_fee,
			)?;
		}

		assert_eq!(T::Fungibles::balance(asset_id::<T>(), &dest), asset_amount * n.into(),);

		Ok(())
	}

	/// The benchmark for `unload_recyclers_into_external_asset_non_anonymous` is split into three
	/// separate benchmarks for different range of `n` (number of input aliases).
	/// This is because the cost is not linear on `n`, there is a sublinear coefficient for the cost
	/// of the batch validation of the proofs.
	/// By benchmarking on a smaller range we approximate the sublinear cost over a range.
	///
	/// In the worst case we unload from different recyclers, in the benchmark we do one recycler
	/// per denomination.
	///
	/// `n` drives both dimensions: number of recyclers (one per coin denomination) and total alias
	/// proofs (one per recycler). Using a single parameter is valid because the worst case is one
	/// alias per recycler — multiple aliases within one recycler are cheaper (shared ring
	/// verification), so benchmarking one-alias-per-recycler captures the upper bound.
	#[benchmark]
	fn unload_recyclers_into_external_asset_non_anonymous_1_2(
		n: Linear<1, 2>,
	) -> Result<(), BenchmarkError> {
		let (inputs, bounded_proofs, caller, dest, total_asset_amount) =
			setup_multi_recycler_unload_non_anonymous::<T>(n);

		let max_fee = Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, n)
			.expect("fee conversion is set up by `common_setup`");

		#[block]
		{
			Pallet::<T>::unload_recyclers_into_external_asset_non_anonymous(
				frame_system::RawOrigin::Signed(caller).into(),
				INSTANCE_ID,
				inputs,
				bounded_proofs,
				dest.clone(),
				FeeCurrency::ExternalAsset,
				max_fee,
			)?;
		}

		assert_eq!(T::Fungibles::balance(asset_id::<T>(), &dest), total_asset_amount,);

		Ok(())
	}

	/// A `max_fee` one below what the conversion costs, so the bound is what rejects the call.
	///
	/// Quoted outside the measured block: the exit performs one quote of its own, and measuring
	/// this one too would charge callers for a quote that never happened.
	fn max_fee_below_the_conversion<T: Config>() -> FungiblesBalanceOf<T> {
		Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, 1)
			.expect("fee conversion is set up by `common_setup`") -
			1u32.into()
	}

	/// The early exit `unload_recyclers_into_external_asset_non_anonymous` takes when the fee
	/// conversion moved past the caller's `max_fee`, which it refunds down to.
	///
	/// To avoid paying the worst-case call fee if the fee changed suddently.
	#[benchmark]
	fn unload_recyclers_into_external_asset_non_anonymous_fee_fail() -> Result<(), BenchmarkError> {
		let (inputs, bounded_proofs, caller, dest, _total_asset_amount) =
			setup_multi_recycler_unload_non_anonymous::<T>(1);
		let max_fee = max_fee_below_the_conversion::<T>();

		let result;
		#[block]
		{
			result = Pallet::<T>::unload_recyclers_into_external_asset_non_anonymous(
				frame_system::RawOrigin::Signed(caller).into(),
				INSTANCE_ID,
				inputs,
				bounded_proofs,
				dest,
				FeeCurrency::ExternalAsset,
				max_fee,
			);
		}

		assert_eq!(
			result.map(|_| ()).map_err(|e| e.error),
			Err(Error::<T>::FeeExceedsMaxFee.into()),
			"the call must be rejected on the fee bound",
		);

		Ok(())
	}

	/// The early exit `unload_archived_recycler_into_external_asset` takes when the fee conversion
	/// moved past the caller's `max_fee`, which it refunds down to.
	///
	/// To avoid paying the worst-case call fee if the fee changed suddently.
	#[benchmark]
	fn unload_archived_recycler_into_external_asset_fee_fail() -> Result<(), BenchmarkError> {
		let (input, bounded_proofs, caller, dest, _asset_amount) =
			setup_single_recycler_unload_non_anonymous::<T>(1);
		let recycler_root = Pallet::<T>::recycler_ring_root(INSTANCE_ID, input.value, input.index)
			.expect("the ring root exists");
		let alias_proof = bounded_proofs.into_iter().next().expect("one proof was generated");
		let max_fee = max_fee_below_the_conversion::<T>();

		let result;
		#[block]
		{
			result = Pallet::<T>::unload_archived_recycler_into_external_asset(
				frame_system::RawOrigin::Signed(caller).into(),
				INSTANCE_ID,
				input.value,
				input.index,
				recycler_root,
				H256::zero(),
				alias_proof,
				Default::default(),
				dest,
				FeeCurrency::ExternalAsset,
				max_fee,
			);
		}

		assert_eq!(
			result.map(|_| ()).map_err(|e| e.error),
			Err(Error::<T>::FeeExceedsMaxFee.into()),
			"the call must be rejected on the fee bound",
		);

		Ok(())
	}

	#[benchmark]
	fn unload_recyclers_into_external_asset_non_anonymous_3_8(
		n: Linear<3, 8>,
	) -> Result<(), BenchmarkError> {
		let (inputs, bounded_proofs, caller, dest, total_asset_amount) =
			setup_multi_recycler_unload_non_anonymous::<T>(n);

		let max_fee = Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, n)
			.expect("fee conversion is set up by `common_setup`");

		#[block]
		{
			Pallet::<T>::unload_recyclers_into_external_asset_non_anonymous(
				frame_system::RawOrigin::Signed(caller).into(),
				INSTANCE_ID,
				inputs,
				bounded_proofs,
				dest.clone(),
				FeeCurrency::ExternalAsset,
				max_fee,
			)?;
		}

		assert_eq!(T::Fungibles::balance(asset_id::<T>(), &dest), total_asset_amount,);

		Ok(())
	}

	#[benchmark]
	fn unload_recyclers_into_external_asset_non_anonymous_9_max(
		n: Linear<
			9,
			{
				((T::MaximumExponent::get() as i32 - T::MinimumExponent::get() as i32 + 1) as u32)
					.min(T::MaxConsolidation::get())
			},
		>,
	) -> Result<(), BenchmarkError> {
		let (inputs, bounded_proofs, caller, dest, total_asset_amount) =
			setup_multi_recycler_unload_non_anonymous::<T>(n);

		let max_fee = Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, n)
			.expect("fee conversion is set up by `common_setup`");

		#[block]
		{
			Pallet::<T>::unload_recyclers_into_external_asset_non_anonymous(
				frame_system::RawOrigin::Signed(caller).into(),
				INSTANCE_ID,
				inputs,
				bounded_proofs,
				dest.clone(),
				FeeCurrency::ExternalAsset,
				max_fee,
			)?;
		}

		assert_eq!(T::Fungibles::balance(asset_id::<T>(), &dest), total_asset_amount,);

		Ok(())
	}

	/// Worst-case notes:
	/// - Ring at full capacity; unloaded trie holds all members but one (the max for a ring);
	///   `dest` is a fresh account (asset-account creation); `ExternalAsset` is the heavier fee
	///   branch. Ring-VRF validation is constant-cost and invalid proofs cost no more than the
	///   valid one benchmarked here. The trie traversal is hash-addressed from the committed root,
	///   so its depth cannot be inflated beyond the real trie's.
	/// - `into_memory_db` hashes every supplied proof node before validation and tolerates
	///   extraneous nodes, so an accepted proof can be padded up to `MAX_TRIE_PROOF_NODES` x
	///   `MAX_TRIE_NODE_LEN`. The measured block therefore dispatches with the honest proof and
	///   then builds a proof db padded to those bounds, so the weight upper-bounds that hashing for
	///   any accepted proof.
	#[benchmark]
	fn unload_archived_recycler_into_external_asset() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		// Build a ring; member 0 will recover, the rest form the committed unloaded set.
		let (index, _revision, members) = setup_built_recycler::<T>(value, 1, 0);
		let recycler_root =
			Pallet::<T>::recycler_ring_root(INSTANCE_ID, value, index).expect("ring root exists");

		let member_keys: Vec<MemberOf<T>> = members.iter().map(|(_, m)| m.clone()).collect();
		let unloaded: Vec<Alias> = members[1..]
			.iter()
			.map(|(s, _)| {
				CryptoOf::<T>::alias_in_context(s, pallet::UNLOADING_RECYCLER_CONTEXT.as_ref())
					.expect("alias")
			})
			.collect();

		let caller: T::AccountId = account("caller", 0, 0);
		let dest: T::AccountId = account("dest", 1, 0);
		let proven_msg = Pallet::<T>::unload_archived_proof_message(&caller);
		let (alias_proof, alias) =
			generate_alias_proof::<T>(&members[0].0, &member_keys, &proven_msg);

		// Build the unloaded-aliases trie and a recovery proof for `alias` (see the helper for why
		// the insert path is recorded rather than a plain lookup proof).
		let (unloaded_root, proof_nodes) =
			crate::testing_utils::unloaded_root_and_non_inclusion_proof(&unloaded, &alias);
		let bounded_proof = crate::testing_utils::to_bounded_proof(proof_nodes);

		// Archive the ring with all members recoverable.
		let commitment = archive_commitment(unloaded_root, &recycler_root);
		let member_count = members.len() as u32;
		RecyclersArchives::<T>::insert(
			(INSTANCE_ID, value, index),
			ArchivedRecycler { commitment, remaining: member_count },
		);

		// Back the recoverable value and fund the fee (paid in the external asset).
		let asset_amount = Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), value)
			.expect("denomination should be in range");
		fund_pallet_account::<T>(asset_amount);
		let fee = Pallet::<T>::quote_paid_unload_token_fee_in_asset(INSTANCE_ID).expect("fee");
		T::BenchmarkHelper::fund_account(&caller, fee.saturating_mul(2u32.into()));
		T::BenchmarkHelper::fund_account(&T::FeeDestination::get(), fee);

		// The dispatch above uses the honest few-node proof, but it accepts (and hashes, via
		// `into_memory_db`) a proof padded to its bounds. Reproduce that maximal hashing here so
		// the measured weight upper-bounds any accepted proof: `MAX_TRIE_PROOF_NODES` distinct
		// nodes of `MAX_TRIE_NODE_LEN` bytes each.
		let worst_case_proof: Vec<Vec<u8>> = (0..crate::MAX_TRIE_PROOF_NODES)
			.map(|i| {
				let mut node = alloc::vec![0u8; crate::MAX_TRIE_NODE_LEN as usize];
				node[..4].copy_from_slice(&i.to_le_bytes());
				node
			})
			.collect();

		let max_fee = Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, 1)
			.expect("fee conversion is set up by `common_setup`");

		#[block]
		{
			Pallet::<T>::unload_archived_recycler_into_external_asset(
				frame_system::RawOrigin::Signed(caller).into(),
				INSTANCE_ID,
				value,
				index,
				recycler_root,
				unloaded_root,
				alias_proof,
				bounded_proof,
				dest.clone(),
				FeeCurrency::ExternalAsset,
				max_fee,
			)?;

			// additionally benchmark the worst-case proof hashing.
			let db = sp_trie::StorageProof::new(worst_case_proof)
				.into_memory_db::<sp_runtime::traits::BlakeTwo256>();
			core::hint::black_box(&db);
		}

		assert_eq!(T::Fungibles::balance(asset_id::<T>(), &dest), asset_amount);
		assert_eq!(
			RecyclersArchives::<T>::get((INSTANCE_ID, value, index))
				.expect("still archived")
				.remaining,
			member_count - 1,
		);

		Ok(())
	}

	/// The benchmark for `unload_recycler_into_coins` is split into three
	/// separate benchmarks for different range of `n` (number of input aliases).
	/// This is because the cost is not linear on `n`, there is a sublinear coefficient for the cost
	/// of the batch validation of the proofs.
	/// By benchmarking on a smaller range we approximate the sublinear cost over a range.
	///
	/// Bucketed variants for alias-count ranges while preserving destination scaling with
	/// `d: Linear<1, MaxSplitOutputs>`.
	///
	/// - `a`: number of aliases (alias proofs to verify)
	/// - `d`: number of destinations (output coins to create)
	#[benchmark]
	fn unload_recycler_into_coins_from_output_1_2(
		a: Linear<1, 2>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, input_value, index, revision, split_into, max_fee) =
			setup_unload_recycler_into_coins::<T>(a, d, UnloadFeeBenchMode::FromOutput)?;
		let aliases_copy = aliases.clone();
		let split_into_copy = split_into.clone();
		let (fee_dest, fee_dest_before, destroyed_before) =
			prepare_from_output_unload_recycler_into_coins_bench::<T>(input_value, index, &aliases);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_coins(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::FromOutput {
						fee_recycler_value: input_value,
						fee_recycler_index: index,
					},
				}
				.into(),
				INSTANCE_ID,
				aliases,
				input_value,
				index,
				revision,
				split_into,
				max_fee,
			)?;
		}

		for (v, dests) in &split_into_copy {
			for dest in dests {
				let coin = CoinsByOwner::<T>::get(dest).expect("destination should have a coin");
				assert_eq!(coin.value, *v);
				assert_eq!(coin.age, 1);
			}
		}
		for alias in &aliases_copy {
			assert!(matches!(
				RecyclerAliasStates::<T>::get((INSTANCE_ID, input_value, index, alias)),
				Some(AliasState::Unloaded),
			));
		}
		assert!(T::NativeFungible::balance(&fee_dest) > fee_dest_before);
		assert!(TotalValueOfDestroyedCoins::<T>::get(INSTANCE_ID) > destroyed_before);

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_coins_from_output_3_8(
		a: Linear<3, 8>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, input_value, index, revision, split_into, max_fee) =
			setup_unload_recycler_into_coins::<T>(a, d, UnloadFeeBenchMode::FromOutput)?;
		let aliases_copy = aliases.clone();
		let split_into_copy = split_into.clone();
		let (fee_dest, fee_dest_before, destroyed_before) =
			prepare_from_output_unload_recycler_into_coins_bench::<T>(input_value, index, &aliases);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_coins(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::FromOutput {
						fee_recycler_value: input_value,
						fee_recycler_index: index,
					},
				}
				.into(),
				INSTANCE_ID,
				aliases,
				input_value,
				index,
				revision,
				split_into,
				max_fee,
			)?;
		}

		for (v, dests) in &split_into_copy {
			for dest in dests {
				let coin = CoinsByOwner::<T>::get(dest).expect("destination should have a coin");
				assert_eq!(coin.value, *v);
				assert_eq!(coin.age, 1);
			}
		}
		for alias in &aliases_copy {
			assert!(matches!(
				RecyclerAliasStates::<T>::get((INSTANCE_ID, input_value, index, alias)),
				Some(AliasState::Unloaded),
			));
		}
		assert!(T::NativeFungible::balance(&fee_dest) > fee_dest_before);
		assert!(TotalValueOfDestroyedCoins::<T>::get(INSTANCE_ID) > destroyed_before);

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_coins_from_output_9_max(
		a: Linear<
			9,
			{ T::MaxConsolidation::get().min(T::RecyclerRingExponent::get().ring_capacity()) },
		>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, input_value, index, revision, split_into, max_fee) =
			setup_unload_recycler_into_coins::<T>(a, d, UnloadFeeBenchMode::FromOutput)?;
		let aliases_copy = aliases.clone();
		let split_into_copy = split_into.clone();
		let (fee_dest, fee_dest_before, destroyed_before) =
			prepare_from_output_unload_recycler_into_coins_bench::<T>(input_value, index, &aliases);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_coins(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::FromOutput {
						fee_recycler_value: input_value,
						fee_recycler_index: index,
					},
				}
				.into(),
				INSTANCE_ID,
				aliases,
				input_value,
				index,
				revision,
				split_into,
				max_fee,
			)?;
		}

		for (v, dests) in &split_into_copy {
			for dest in dests {
				let coin = CoinsByOwner::<T>::get(dest).expect("destination should have a coin");
				assert_eq!(coin.value, *v);
				assert_eq!(coin.age, 1);
			}
		}
		for alias in &aliases_copy {
			assert!(matches!(
				RecyclerAliasStates::<T>::get((INSTANCE_ID, input_value, index, alias)),
				Some(AliasState::Unloaded),
			));
		}
		assert!(T::NativeFungible::balance(&fee_dest) > fee_dest_before);
		assert!(TotalValueOfDestroyedCoins::<T>::get(INSTANCE_ID) > destroyed_before);

		Ok(())
	}

	/// `Prepaid` counterparts of the `unload_recycler_into_coins` buckets.
	///
	/// The `Prepaid` path verifies all `a` alias proofs (the first alias is not pre-marked by the
	/// extension) and moves no fee, whereas the `FromOutput` buckets verify `a - 1` proofs and add
	/// a fee transfer and remainder burn. Neither path dominates the other, so both are measured;
	/// the `AsCoinage` extension charges the difference for the mode it mints (see
	/// the `Prepaid` `PostDispatchInfo` refund).
	#[benchmark]
	fn unload_recycler_into_coins_prepaid_1_2(
		a: Linear<1, 2>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, input_value, index, revision, split_into, max_fee) =
			setup_unload_recycler_into_coins::<T>(a, d, UnloadFeeBenchMode::Prepaid)?;
		let aliases_copy = aliases.clone();
		let split_into_copy = split_into.clone();
		let fee_dest = T::FeeDestination::get();
		let fee_dest_before = T::NativeFungible::balance(&fee_dest);
		let destroyed_before = TotalValueOfDestroyedCoins::<T>::get(INSTANCE_ID);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_coins(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				INSTANCE_ID,
				aliases,
				input_value,
				index,
				revision,
				split_into,
				max_fee,
			)?;
		}

		for (v, dests) in &split_into_copy {
			for dest in dests {
				let coin = CoinsByOwner::<T>::get(dest).expect("destination should have a coin");
				assert_eq!(coin.value, *v);
				assert_eq!(coin.age, 1);
			}
		}
		for alias in &aliases_copy {
			assert!(matches!(
				RecyclerAliasStates::<T>::get((INSTANCE_ID, input_value, index, alias)),
				Some(AliasState::Unloaded),
			));
		}
		// `Prepaid` moves no fee and burns nothing.
		assert_eq!(T::NativeFungible::balance(&fee_dest), fee_dest_before);
		assert_eq!(TotalValueOfDestroyedCoins::<T>::get(INSTANCE_ID), destroyed_before);

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_coins_prepaid_3_8(
		a: Linear<3, 8>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, input_value, index, revision, split_into, max_fee) =
			setup_unload_recycler_into_coins::<T>(a, d, UnloadFeeBenchMode::Prepaid)?;
		let aliases_copy = aliases.clone();
		let split_into_copy = split_into.clone();
		let fee_dest = T::FeeDestination::get();
		let fee_dest_before = T::NativeFungible::balance(&fee_dest);
		let destroyed_before = TotalValueOfDestroyedCoins::<T>::get(INSTANCE_ID);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_coins(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				INSTANCE_ID,
				aliases,
				input_value,
				index,
				revision,
				split_into,
				max_fee,
			)?;
		}

		for (v, dests) in &split_into_copy {
			for dest in dests {
				let coin = CoinsByOwner::<T>::get(dest).expect("destination should have a coin");
				assert_eq!(coin.value, *v);
				assert_eq!(coin.age, 1);
			}
		}
		for alias in &aliases_copy {
			assert!(matches!(
				RecyclerAliasStates::<T>::get((INSTANCE_ID, input_value, index, alias)),
				Some(AliasState::Unloaded),
			));
		}
		assert_eq!(T::NativeFungible::balance(&fee_dest), fee_dest_before);
		assert_eq!(TotalValueOfDestroyedCoins::<T>::get(INSTANCE_ID), destroyed_before);

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_coins_prepaid_9_max(
		a: Linear<
			9,
			{ T::MaxConsolidation::get().min(T::RecyclerRingExponent::get().ring_capacity()) },
		>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, input_value, index, revision, split_into, max_fee) =
			setup_unload_recycler_into_coins::<T>(a, d, UnloadFeeBenchMode::Prepaid)?;
		let aliases_copy = aliases.clone();
		let split_into_copy = split_into.clone();
		let fee_dest = T::FeeDestination::get();
		let fee_dest_before = T::NativeFungible::balance(&fee_dest);
		let destroyed_before = TotalValueOfDestroyedCoins::<T>::get(INSTANCE_ID);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_coins(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				INSTANCE_ID,
				aliases,
				input_value,
				index,
				revision,
				split_into,
				max_fee,
			)?;
		}

		for (v, dests) in &split_into_copy {
			for dest in dests {
				let coin = CoinsByOwner::<T>::get(dest).expect("destination should have a coin");
				assert_eq!(coin.value, *v);
				assert_eq!(coin.age, 1);
			}
		}
		for alias in &aliases_copy {
			assert!(matches!(
				RecyclerAliasStates::<T>::get((INSTANCE_ID, input_value, index, alias)),
				Some(AliasState::Unloaded),
			));
		}
		assert_eq!(T::NativeFungible::balance(&fee_dest), fee_dest_before);
		assert_eq!(TotalValueOfDestroyedCoins::<T>::get(INSTANCE_ID), destroyed_before);

		Ok(())
	}

	// ==================== Batch verification proof-count bucket benchmarks ====================
	//
	// These benchmarks encode piecewise proof-count buckets for batch verification
	// weight modeling. The intended fit is:
	//   time ~= K1 + K2*n + K3*sqrt(n)
	// and for n > 8 we can approximate with:
	//   time ~= M1 + M2*n
	//
	// Buckets:
	//   exact:    1        — isolate fixed setup/amortization effects
	//   low:      2..4     — low-n transitional range
	//   medium:   4..8     — still curved, but closer to linear
	//   high:     8..max   — approximately linear region

	#[benchmark(extra)]
	fn batch_verify_recycler_single() -> Result<(), BenchmarkError> {
		let (value, ring_index, _aliases, proofs, proven_msg) =
			T::BenchmarkHelper::setup_batch_verify(1)?;
		let identifier = Pallet::<T>::recycler_collection_identifier(INSTANCE_ID, value);
		let items: Vec<RingMembershipProof<ProofOf<T>>> = proofs
			.iter()
			.map(|proof| RingMembershipProof {
				proof: proof.clone(),
				message: proven_msg.to_vec(),
				context: UNLOADING_RECYCLER_CONTEXT.to_vec(),
			})
			.collect();
		let revision = T::MemberService::ring_revision(&identifier, ring_index)
			.expect("benchmark ring must have a current revision");

		#[block]
		{
			let results = T::MemberService::verify_memberships_in_ring(
				&identifier,
				ring_index,
				revision,
				&items,
			)
			.expect("batch verify: single proof failed");
			assert_eq!(results.len(), items.len());
		}

		Ok(())
	}

	// Benchmark with two independent parameters:
	// - `a`: number of aliases (alias proofs to verify)
	// - `d`: number of destinations (output coins to create)
	#[benchmark(extra)]
	fn batch_verify_recycler_small(n: Linear<2, 4>) -> Result<(), BenchmarkError> {
		let (value, ring_index, _aliases, proofs, proven_msg) =
			T::BenchmarkHelper::setup_batch_verify(n)?;
		let identifier = Pallet::<T>::recycler_collection_identifier(INSTANCE_ID, value);
		let items: Vec<RingMembershipProof<ProofOf<T>>> = proofs
			.iter()
			.map(|proof| RingMembershipProof {
				proof: proof.clone(),
				message: proven_msg.to_vec(),
				context: UNLOADING_RECYCLER_CONTEXT.to_vec(),
			})
			.collect();
		let revision = T::MemberService::ring_revision(&identifier, ring_index)
			.expect("benchmark ring must have a current revision");

		#[block]
		{
			let results = T::MemberService::verify_memberships_in_ring(
				&identifier,
				ring_index,
				revision,
				&items,
			)
			.expect("batch verify: small batch failed");
			assert_eq!(results.len(), items.len());
		}

		Ok(())
	}

	#[benchmark(extra)]
	fn batch_verify_recycler_medium(n: Linear<4, 8>) -> Result<(), BenchmarkError> {
		let (value, ring_index, _aliases, proofs, proven_msg) =
			T::BenchmarkHelper::setup_batch_verify(n)?;
		let identifier = Pallet::<T>::recycler_collection_identifier(INSTANCE_ID, value);
		let items: Vec<RingMembershipProof<ProofOf<T>>> = proofs
			.iter()
			.map(|proof| RingMembershipProof {
				proof: proof.clone(),
				message: proven_msg.to_vec(),
				context: UNLOADING_RECYCLER_CONTEXT.to_vec(),
			})
			.collect();
		let revision = T::MemberService::ring_revision(&identifier, ring_index)
			.expect("benchmark ring must have a current revision");

		#[block]
		{
			let results = T::MemberService::verify_memberships_in_ring(
				&identifier,
				ring_index,
				revision,
				&items,
			)
			.expect("batch verify: medium batch failed");
			assert_eq!(results.len(), items.len());
		}

		Ok(())
	}

	#[benchmark(extra)]
	fn batch_verify_recycler_large(
		n: Linear<
			8,
			{ T::MaxConsolidation::get().min(T::RecyclerRingExponent::get().ring_capacity()) },
		>,
	) -> Result<(), BenchmarkError> {
		let (value, ring_index, _aliases, proofs, proven_msg) =
			T::BenchmarkHelper::setup_batch_verify(n)?;
		let identifier = Pallet::<T>::recycler_collection_identifier(INSTANCE_ID, value);
		let items: Vec<RingMembershipProof<ProofOf<T>>> = proofs
			.iter()
			.map(|proof| RingMembershipProof {
				proof: proof.clone(),
				message: proven_msg.to_vec(),
				context: UNLOADING_RECYCLER_CONTEXT.to_vec(),
			})
			.collect();
		let revision = T::MemberService::ring_revision(&identifier, ring_index)
			.expect("benchmark ring must have a current revision");

		#[block]
		{
			let results = T::MemberService::verify_memberships_in_ring(
				&identifier,
				ring_index,
				revision,
				&items,
			)
			.expect("batch verify: large batch failed");
			assert_eq!(results.len(), items.len());
		}

		Ok(())
	}
	// ==================== Transaction extension benchmarks ====================

	/// Benchmark for AsCoinage(None) with calls outside the specifically weighted signed unload
	/// calls.
	///
	/// Uses `load_recycler_with_external_asset` on a sponsored instance in the worst validation
	/// state, the heaviest check `validate_signed_unload_calls` performs on this branch (the
	/// load-deposit pre-flight); the resulting weight conservatively covers the cheaper calls
	/// that also land here.
	#[benchmark]
	fn as_none_tx_ext_others() -> Result<(), BenchmarkError> {
		common_setup::<T>();
		worst_case_sponsored_validation::<T>(INSTANCE_ID);

		let caller: T::AccountId = account("caller", 0, 0);
		let value = T::MinimumExponent::get();
		let (secret, member_key) = new_member_from::<T>(0, 0);
		let proof_of_ownership = create_proof_of_ownership::<T>(&secret, &caller);

		let call = Call::<T>::load_recycler_with_external_asset {
			instance_id: INSTANCE_ID,
			preservation: CodecPreservation::Protect,
			value,
			member_key,
			proof_of_ownership,
		};

		let tx_ext = AsCoinage::<T>::new(None);
		let origin = SystemOrigin::Signed(caller);
		let len = call.encode().len();

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call.into(), &Default::default(), len, 0, |_| {
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	/// Benchmark for AsCoinage(None) with unload_recycler_into_external_asset_non_anonymous
	/// (single input, validates 1 recycler revision).
	#[benchmark]
	fn as_none_tx_ext_unload_recycler_into_external_asset_non_anonymous(
	) -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		let (index, revision, members) = setup_built_recycler::<T>(value, 1, 0);

		// Generate alias proof
		let msg: [u8; 32] = [0u8; 32];
		let members_only: Vec<MemberOf<T>> = members.iter().map(|(_, m)| m.clone()).collect();
		let (secret, _) = &members[0];
		let (proof, alias) = generate_alias_proof::<T>(secret, &members_only, &msg);

		let input = UnloadRecyclerInput {
			value,
			index,
			revision,
			aliases: vec![alias].try_into().unwrap(),
		};
		let alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation> =
			vec![proof].try_into().unwrap();

		let call = Call::<T>::unload_recycler_into_external_asset_non_anonymous {
			instance_id: INSTANCE_ID,
			input,
			alias_proofs,
			to: account("dest", 0, 0),
			fee_currency: FeeCurrency::ExternalAsset,
			max_fee: Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, 1)
				.expect("fee conversion is set up by `common_setup`"),
		};

		let caller: T::AccountId = whitelisted_caller();
		let tx_ext = AsCoinage::<T>::new(None);
		let origin = SystemOrigin::Signed(caller);
		let len = call.encode().len();

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call.into(), &Default::default(), len, 0, |_| {
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	/// Benchmark for AsCoinage(None) with unload_recyclers_into_external_asset_non_anonymous
	/// (multiple inputs, validates n recycler revisions).
	#[benchmark]
	fn as_none_tx_ext_unload_recyclers_into_external_asset_non_anonymous(
		n: Linear<
			1,
			{
				((T::MaximumExponent::get() as i32 - T::MinimumExponent::get() as i32 + 1) as u32)
					.min(T::MaxConsolidation::get())
			},
		>,
	) -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let (inputs, sign_data, _total_asset_amount) = setup_multi_recyclers::<T>(n, 0);

		let msg: [u8; 32] = [0u8; 32];
		let mut alias_proofs = Vec::new();

		for (secret, actual_ring_members) in &sign_data {
			let (proof, _) = generate_alias_proof::<T>(secret, actual_ring_members, &msg);
			alias_proofs.push(proof);
		}

		let bounded_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation> =
			alias_proofs.try_into().unwrap();

		let call = Call::<T>::unload_recyclers_into_external_asset_non_anonymous {
			instance_id: INSTANCE_ID,
			inputs: inputs.try_into().unwrap(),
			alias_proofs: bounded_proofs,
			to: account("dest", 0, 0),
			fee_currency: FeeCurrency::ExternalAsset,
			max_fee: Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, n)
				.expect("fee conversion is set up by `common_setup`"),
		};

		let caller: T::AccountId = whitelisted_caller();
		let tx_ext = AsCoinage::<T>::new(None);
		let origin = SystemOrigin::Signed(caller);
		let len = call.encode().len();

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call.into(), &Default::default(), len, 0, |_| {
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	/// Benchmark for AsCoinage(None) with unload_archived_recycler_into_external_asset
	/// (validates the supplied roots against the archive's stored commitment).
	#[benchmark]
	fn as_none_tx_ext_unload_archived_recycler_into_external_asset() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		// Build a ring; member 0 recovers, the rest form the committed unloaded set.
		let (index, _revision, members) = setup_built_recycler::<T>(value, 1, 0);
		let recycler_root =
			Pallet::<T>::recycler_ring_root(INSTANCE_ID, value, index).expect("ring root exists");

		let member_keys: Vec<MemberOf<T>> = members.iter().map(|(_, m)| m.clone()).collect();
		let unloaded: Vec<Alias> = members[1..]
			.iter()
			.map(|(s, _)| {
				CryptoOf::<T>::alias_in_context(s, pallet::UNLOADING_RECYCLER_CONTEXT.as_ref())
					.expect("alias")
			})
			.collect();

		let caller: T::AccountId = account("caller", 0, 0);
		let proven_msg = Pallet::<T>::unload_archived_proof_message(&caller);
		let (alias_proof, alias) =
			generate_alias_proof::<T>(&members[0].0, &member_keys, &proven_msg);
		let (unloaded_root, proof_nodes) =
			crate::testing_utils::unloaded_root_and_non_inclusion_proof(&unloaded, &alias);

		// Archive the ring; the call's roots must match this commitment at validation.
		let commitment = archive_commitment(unloaded_root, &recycler_root);
		let remaining = members.len() as u32;
		RecyclersArchives::<T>::insert(
			(INSTANCE_ID, value, index),
			ArchivedRecycler { commitment, remaining },
		);

		let call = Call::<T>::unload_archived_recycler_into_external_asset {
			instance_id: INSTANCE_ID,
			value,
			index,
			recycler_root,
			unloaded_root,
			alias_proof,
			non_inclusion_proof: crate::testing_utils::to_bounded_proof(proof_nodes),
			to: account("dest", 1, 0),
			fee_currency: FeeCurrency::ExternalAsset,
			max_fee: Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, 1)
				.expect("fee conversion is set up by `common_setup`"),
		};

		let tx_ext = AsCoinage::<T>::new(None);
		let origin = SystemOrigin::Signed(caller);
		let len = call.encode().len();

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call.into(), &Default::default(), len, 0, |_| {
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	/// Benchmark for AsCoin extension with split call.
	#[benchmark]
	fn as_coin_split(
		n: Linear<1, { (T::MaximumExponent::get() - T::MinimumExponent::get() + 1) as u32 }>,
	) -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let min_exp = T::MinimumExponent::get();

		// Create a coin and split into multiple distinct value groups to exercise the outer loop.
		// For n >= 2: 2 destinations at min_exp, 1 destination at each of min_exp+1..min_exp+n-2
		// This gives n destinations total and (n-1) value groups.
		// Denomination = 2^(min_exp + n - 1)
		let value = min_exp.saturating_add((n as i8 - 1).max(0));
		let age = 0u16;
		let coin_owner = create_coin::<T>(value, age, 0);

		// Build split_into with multiple value groups (must be strictly ascending)
		let mut split_into: Vec<(Denomination, BoundedVec<T::AccountId, T::MaxSplitOutputs>)> =
			Vec::new();
		let mut dest_idx = 0u32;

		if n == 1 {
			split_into.push((min_exp, vec![account("dest", 0, 0)].try_into().unwrap()));
		} else {
			// 2 at min_exp
			split_into.push((
				min_exp,
				vec![account("dest", dest_idx, 0), account("dest", dest_idx + 1, 0)]
					.try_into()
					.unwrap(),
			));
			dest_idx += 2;

			// 1 at each of min_exp+1 to min_exp+n-2
			for i in 1..(n - 1) {
				let denom = min_exp.saturating_add(i as i8);
				split_into.push((denom, vec![account("dest", dest_idx, 0)].try_into().unwrap()));
				dest_idx += 1;
			}
		}

		let split_into = split_into.try_into().unwrap();
		let call = Call::<T>::split { split_into };
		let tx_ext = AsCoinage::<T>::new(Some(AsCoinageInfo::AsCoin));
		let origin = SystemOrigin::Signed(coin_owner);
		let len = call.encode().len();

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call.into(), &Default::default(), len, 0, |_| {
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	/// Benchmark for AsCoin extension with transfer call.
	#[benchmark]
	fn as_coin_transfer() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		let age = 0u16;
		let coin_owner = create_coin::<T>(value, age, 0);
		let to: T::AccountId = account("dest", 0, 0);

		let call = Call::<T>::transfer { to };
		let tx_ext = AsCoinage::<T>::new(Some(AsCoinageInfo::AsCoin));
		let origin = SystemOrigin::Signed(coin_owner);
		let len = call.encode().len();

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call.into(), &Default::default(), len, 0, |_| {
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	/// Benchmark for AsCoin extension with load_recycler_with_coin call.
	#[benchmark]
	fn as_coin_load_recycler_with_coin() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		let age = 0u16;
		let coin_owner = create_coin::<T>(value, age, 0);
		// The validation's load-deposit pre-flight is heaviest on a sponsored instance.
		worst_case_sponsored_validation::<T>(INSTANCE_ID);

		let (secret, member_key) = new_member_from::<T>(0, 0);
		let proof_of_ownership = create_proof_of_ownership::<T>(&secret, &coin_owner);

		let call = Call::<T>::load_recycler_with_coin { member_key, proof_of_ownership };
		let tx_ext = AsCoinage::<T>::new(Some(AsCoinageInfo::AsCoin));
		let origin = SystemOrigin::Signed(coin_owner);
		let len = call.encode().len();

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call.into(), &Default::default(), len, 0, |_| {
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	/// Benchmark for AsCoin extension with pay_for_recycler_unload_fee_token_with_coin call.
	#[benchmark]
	fn as_coin_pay_for_recycler_unload_fee_token_with_coin() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		// Need a coin with value >= paid unload token fee
		let fee = Pallet::<T>::quote_paid_unload_token_fee_in_asset(INSTANCE_ID)
			.expect("fee should be available after setup");
		let mut value = T::MinimumExponent::get();
		while Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), value)
			.unwrap_or_default() <
			fee
		{
			value = value.saturating_add(1);
		}

		let age = 0u16;
		let coin_owner = create_coin::<T>(value, age, 0);

		let (secret, member_key) = new_member_from::<T>(0, 0);
		let proof_of_ownership = create_proof_of_ownership::<T>(&secret, &coin_owner);

		let call = Call::<T>::pay_for_recycler_unload_fee_token_with_coin {
			member_key,
			proof_of_ownership,
		};
		let tx_ext = AsCoinage::<T>::new(Some(AsCoinageInfo::AsCoin));
		let origin = SystemOrigin::Signed(coin_owner);
		let len = call.encode().len();

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call.into(), &Default::default(), len, 0, |_| {
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	#[benchmark]
	fn as_unload_token_people_tx_ext() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		let (index, revision, members) = setup_built_recycler::<T>(value, 1, 0);

		let period = T::UnixTime::now()
			.as_secs()
			.checked_div(T::UnloadTokenTimePeriodPeopleLitePeople::get() as u64)
			.unwrap_or(0) as u32;
		let counter = 0u32;

		// Get alias from secret
		let members_only: Vec<MemberOf<T>> = members.iter().map(|(_, m)| m.clone()).collect();
		let (secret, _) = &members[0];
		let alias =
			CryptoOf::<T>::alias_in_context(secret, pallet::UNLOADING_RECYCLER_CONTEXT.as_ref())
				.expect("alias should be valid");

		// Create the call first to compute inherited_implication
		let call = Call::<T>::unload_recycler_into_external_asset {
			instance_id: INSTANCE_ID,
			aliases: vec![alias].try_into().unwrap(),
			value,
			index,
			revision,
			to: account("dest", 0, 0),
			max_fee: Zero::zero(),
		};

		let runtime_call: <T as frame_system::Config>::RuntimeCall = call.clone().into();
		let inherited_implication = ((0u8, &runtime_call), (), ());
		let proven_msg = sp_crypto_hashing::blake2_256(&inherited_implication.encode());

		// Generate alias proof with proven_msg
		let (alias_proof, _) = generate_alias_proof::<T>(secret, &members_only, &proven_msg);
		let alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation> =
			vec![alias_proof].try_into().unwrap();

		// Create a people proof with intent message (alias_proofs ++ inherited_implication)
		let context = pallet::free_unload_token_context(period, counter);
		let intent_msg = sp_crypto_hashing::blake2_256(
			&[alias_proofs.encode(), inherited_implication.encode()].concat(),
		);
		let proof = T::BenchmarkHelper::create_people_proof(&context, &intent_msg, alias);

		let tx_ext = AsCoinage::<T>::new(Some(AsCoinageInfo::AsUnloadTokenPeople {
			proof,
			period,
			counter,
			alias_proofs,
		}));
		let origin = SystemOrigin::None;
		let len = call.encode().len();

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call.into(), &Default::default(), len, 0, |_| {
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	#[benchmark]
	fn as_unload_token_lite_people_tx_ext() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		let (index, revision, members) = setup_built_recycler::<T>(value, 1, 0);

		let period = T::UnixTime::now()
			.as_secs()
			.checked_div(T::UnloadTokenTimePeriodPeopleLitePeople::get() as u64)
			.unwrap_or(0) as u32;
		let counter = 0u32;

		// Get alias from secret
		let members_only: Vec<MemberOf<T>> = members.iter().map(|(_, m)| m.clone()).collect();
		let (secret, _) = &members[0];
		let alias =
			CryptoOf::<T>::alias_in_context(secret, pallet::UNLOADING_RECYCLER_CONTEXT.as_ref())
				.expect("alias should be valid");

		// Create the call first to compute inherited_implication
		let call = Call::<T>::unload_recycler_into_external_asset {
			instance_id: INSTANCE_ID,
			aliases: vec![alias].try_into().unwrap(),
			value,
			index,
			revision,
			to: account("dest", 0, 0),
			max_fee: Zero::zero(),
		};

		let runtime_call: <T as frame_system::Config>::RuntimeCall = call.clone().into();
		let inherited_implication = ((0u8, &runtime_call), (), ());
		let proven_msg = sp_crypto_hashing::blake2_256(&inherited_implication.encode());

		// Generate alias proof with proven_msg
		let (alias_proof, _) = generate_alias_proof::<T>(secret, &members_only, &proven_msg);
		let alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation> =
			vec![alias_proof].try_into().unwrap();

		// Create a lite people proof with intent message (alias_proofs ++ inherited_implication)
		let context = pallet::free_unload_token_context(period, counter);
		let intent_msg = sp_crypto_hashing::blake2_256(
			&[alias_proofs.encode(), inherited_implication.encode()].concat(),
		);
		let proof = T::BenchmarkHelper::create_lite_people_proof(&context, &intent_msg, alias);

		let tx_ext = AsCoinage::<T>::new(Some(AsCoinageInfo::AsUnloadTokenLitePeople {
			proof,
			period,
			counter,
			alias_proofs,
		}));
		let origin = SystemOrigin::None;
		let len = call.encode().len();

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call.into(), &Default::default(), len, 0, |_| {
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	#[benchmark]
	fn as_unload_token_paid_tx_ext() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		// Setup recycler for unloading
		let value = T::MinimumExponent::get();
		let (recycler_index, recycler_revision, recycler_members) =
			setup_built_recycler::<T>(value, 1, 0);

		// Setup paid token ring
		let (period, paid_ring_index, paid_members) = setup_built_paid_token_ring::<T>(4, 100);
		let identifier = Pallet::<T>::paid_token_collection_identifier(period);
		let paid_ring_revision =
			T::MemberService::ring_revision(&identifier, paid_ring_index).expect("ring built");

		// Get alias from recycler secret
		let recycler_members_only: Vec<MemberOf<T>> =
			recycler_members.iter().map(|(_, m)| m.clone()).collect();
		let (recycler_secret, _) = &recycler_members[0];
		let alias = CryptoOf::<T>::alias_in_context(
			recycler_secret,
			pallet::UNLOADING_RECYCLER_CONTEXT.as_ref(),
		)
		.expect("alias should be valid");

		// Create the call first to compute inherited_implication
		let call = Call::<T>::unload_recycler_into_external_asset {
			instance_id: INSTANCE_ID,
			aliases: vec![alias].try_into().unwrap(),
			value,
			index: recycler_index,
			revision: recycler_revision,
			to: account("dest", 0, 0),
			max_fee: Zero::zero(),
		};

		let runtime_call: <T as frame_system::Config>::RuntimeCall = call.clone().into();
		let inherited_implication = ((0u8, &runtime_call), (), ());
		let proven_msg = sp_crypto_hashing::blake2_256(&inherited_implication.encode());

		// Generate alias proof with proven_msg
		let (alias_proof, _) =
			generate_alias_proof::<T>(recycler_secret, &recycler_members_only, &proven_msg);
		let alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation> =
			vec![alias_proof].try_into().unwrap();

		// Generate paid token proof with intent message (alias_proofs ++ inherited_implication)
		let intent_msg = sp_crypto_hashing::blake2_256(
			&[alias_proofs.encode(), inherited_implication.encode()].concat(),
		);
		let (paid_secret, _) = &paid_members[0];
		let paid_members_only: Vec<MemberOf<T>> =
			paid_members.iter().map(|(_, m)| m.clone()).collect();
		let member = CryptoOf::<T>::member_from_secret(paid_secret);
		let domain_size: <CryptoOf<T> as GenerateVerifiable>::Config =
			T::PaidUnloadTokenRingExponent::get()
				.try_into()
				.ok()
				.expect("valid ring exponent");
		let commitment =
			CryptoOf::<T>::open(domain_size, &member, paid_members_only.iter().cloned())
				.expect("should open");
		let context = {
			let mut c = [0u8; 32];
			c[..28].copy_from_slice(pallet::PAID_UNLOAD_TOKEN_CONTEXT_BASE.as_ref());
			c[28..32].copy_from_slice(&period.to_le_bytes());
			c
		};
		let (proof, _) =
			CryptoOf::<T>::create(commitment, paid_secret, context.as_ref(), intent_msg.as_ref())
				.expect("should create proof");

		let tx_ext = AsCoinage::<T>::new(Some(AsCoinageInfo::AsUnloadTokenPaid {
			proof,
			period,
			paid_token_ring_index: paid_ring_index,
			paid_token_ring_revision: paid_ring_revision,
			alias_proofs,
		}));
		let origin = SystemOrigin::None;
		let len = call.encode().len();

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call.into(), &Default::default(), len, 0, |_| {
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	#[benchmark]
	fn as_unload_token_from_output_tx_ext() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		// Setup recycler with a denomination large enough for the penalty fee.
		let value = T::MinimumExponentForOutputUnloadFee::get();

		let (index, revision, members) = setup_built_recycler::<T>(value, 1, 0);

		// Get alias from secret
		let members_only: Vec<MemberOf<T>> = members.iter().map(|(_, m)| m.clone()).collect();
		let (secret, _) = &members[0];
		let alias =
			CryptoOf::<T>::alias_in_context(secret, pallet::UNLOADING_RECYCLER_CONTEXT.as_ref())
				.expect("alias should be valid");

		// Create the call first to compute inherited_implication
		let call = Call::<T>::unload_recycler_into_external_asset {
			instance_id: INSTANCE_ID,
			aliases: vec![alias].try_into().unwrap(),
			value,
			index,
			revision,
			to: account("dest", 0, 0),
			max_fee: Pallet::<T>::quote_paid_unload_token_fees_in_asset(INSTANCE_ID, 1)
				.expect("fee conversion is set up by `common_setup`"),
		};

		let runtime_call: <T as frame_system::Config>::RuntimeCall = call.clone().into();
		let inherited_implication = ((0u8, &runtime_call), (), ());

		// No other alias proofs (single alias benchmark)
		let other_proofs = Vec::<ProofOf<T>>::new();

		// Generate first alias proof signing the other proofs, retry counter, and inherited
		// implication.
		let retry_counter = 0u8;
		let intent_msg = (&other_proofs, retry_counter, &inherited_implication)
			.using_encoded(sp_crypto_hashing::blake2_256);
		let (first_alias_proof, _) = generate_alias_proof::<T>(secret, &members_only, &intent_msg);
		let alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation> =
			vec![first_alias_proof].try_into().unwrap();

		let tx_ext = AsCoinage::<T>::new(Some(AsCoinageInfo::AsUnloadTokenFromOutput {
			fee_recycler_value: value,
			fee_recycler_index: index,
			fee_recycler_revision: revision,
			retry_counter,
			alias_proofs,
		}));
		let origin = SystemOrigin::None;
		let len = call.encode().len();

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call.into(), &Default::default(), len, 0, |_| {
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	/// Benchmark for the `load_recycler_with_external_asset_unpaid` call dispatch.
	#[benchmark]
	fn load_recycler_with_external_asset_unpaid() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let caller: T::AccountId = account("caller", 0, 0);
		let value = T::MinimumExponent::get();
		let asset_amount = Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), value)
			.expect("denomination should be in range");

		T::BenchmarkHelper::fund_account(&caller, asset_amount * 2u32.into());

		let (secret, member_key) = new_member_from::<T>(0, 0);
		let proof_of_ownership = create_proof_of_ownership::<T>(&secret, &caller);

		let origin: T::RuntimeOrigin = Origin::<T>::InfallibleUnpaidSigned { who: caller }.into();

		#[extrinsic_call]
		_(
			origin as T::RuntimeOrigin,
			INSTANCE_ID,
			CodecPreservation::Protect,
			value,
			member_key.clone(),
			proof_of_ownership,
		);

		assert!(RecyclersCoinToRecycler::<T>::contains_key(&member_key));

		Ok(())
	}

	/// Benchmark for the `InfallibleUnpaidSigned` extension.
	#[benchmark]
	fn as_infallible_unpaid_tx_ext() -> Result<(), BenchmarkError> {
		common_setup::<T>();
		// The validation's load-deposit pre-flight is heaviest on a sponsored instance.
		worst_case_sponsored_validation::<T>(INSTANCE_ID);

		let caller: T::AccountId = account("caller", 0, 0);
		let value = T::MinimumExponent::get();
		let asset_amount = Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), value)
			.expect("denomination should be in range");

		// Fund caller with the external asset.
		T::BenchmarkHelper::fund_account(&caller, asset_amount * 2u32.into());

		let (secret, member_key) = new_member_from::<T>(0, 0);
		let proof_of_ownership = create_proof_of_ownership::<T>(&secret, &caller);

		let call = Call::<T>::load_recycler_with_external_asset_unpaid {
			instance_id: INSTANCE_ID,
			preservation: CodecPreservation::Protect,
			value,
			member_key,
			proof_of_ownership,
		};

		let nonce = frame_system::Pallet::<T>::account_nonce(&caller);
		let tx_ext = AsCoinage::<T>::new(Some(AsCoinageInfo::InfallibleUnpaidSigned { nonce }));
		let origin = SystemOrigin::Signed(caller);
		let len = call.encode().len();

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call.into(), &Default::default(), len, 0, |_| {
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	/// Benchmark for the `InfallibleUnpaidSigned` extension validating a batch of `n` items.
	///
	/// Sweeping `n` separates the once-per-transaction cost (the `reducible_balance` reads and the
	/// `CheckNonce` read/write) from the per-item cost (the member-key lookup and signature
	/// verification), so the weight can be modelled as `base + n * per_item` instead of the
	/// conservative `n * as_infallible_unpaid_tx_ext()` over-estimate.
	#[benchmark]
	fn as_infallible_unpaid_tx_ext_batch(
		n: Linear<1, { T::MaxBatchUnpaidLoad::get() }>,
	) -> Result<(), BenchmarkError> {
		common_setup::<T>();
		// The validation's load-deposit pre-flight is heaviest on a sponsored instance.
		worst_case_sponsored_validation::<T>(INSTANCE_ID);

		let caller: T::AccountId = account("caller", 0, 0);
		let value = T::MinimumExponent::get();
		let asset_amount = Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), value)
			.expect("denomination should be in range");

		// Fund the caller for the whole batch, with a buffer above the aggregate cost.
		T::BenchmarkHelper::fund_account(&caller, asset_amount * (n + 1).into());

		let items = (0..n)
			.map(|i| {
				let (secret, member_key) = new_member_from::<T>(i, 0);
				let proof_of_ownership = create_proof_of_ownership::<T>(&secret, &caller);
				UnpaidLoadInput {
					preservation: CodecPreservation::Protect,
					value,
					member_key,
					proof_of_ownership,
				}
			})
			.collect::<Vec<_>>()
			.try_into()
			.unwrap();

		let call = Call::<T>::load_recycler_with_external_asset_unpaid_batch {
			instance_id: INSTANCE_ID,
			items,
		};

		let nonce = frame_system::Pallet::<T>::account_nonce(&caller);
		let tx_ext = AsCoinage::<T>::new(Some(AsCoinageInfo::InfallibleUnpaidSigned { nonce }));
		let origin = SystemOrigin::Signed(caller);
		let len = call.encode().len();

		#[block]
		{
			tx_ext
				.test_run(origin.into(), &call.into(), &Default::default(), len, 0, |_| {
					Ok(Default::default())
				})
				.unwrap()?;
		}

		Ok(())
	}

	// ==================== Validation benchmarks ====================

	/// Benchmark `validate_unload_calls` operations.
	/// - `r`: number of recycler revision checks
	/// - `d`: number of output validations (coin destinations and loaded-coin outputs)
	// NOTE: we use one recycler per denomination to avoid the expensive setup of creating multiple
	// full recyclers.
	#[benchmark]
	fn validate_unload_calls(
		r: Linear<
			1,
			{
				((T::MaximumExponent::get() as i32 - T::MinimumExponent::get() as i32 + 1) as u32)
					.min(T::MaxConsolidation::get())
			},
		>,
		d: Linear<0, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let (inputs, _, _) = setup_multi_recyclers::<T>(r, 0);
		let mixed_output_validation = if d > 0 {
			let scenario = setup_unload_recycler_into_external_asset_and_loaded_coins::<T>(
				1,
				d,
				UnloadFeeBenchMode::Prepaid,
			)?;
			Some((scenario.input_value, scenario.external_asset_amount, scenario.loaded_coins))
		} else {
			None
		};

		// Setup `d` destination accounts (that don't have coins)
		let mut destinations: Vec<T::AccountId> = Vec::new();
		for i in 0..d {
			destinations.push(account("dest", i, 0));
		}

		// The mixed-output branch also pre-flights the voucher keys' load deposits, heaviest
		// on a sponsored instance. The conversion comes after the recycler setup, whose loads
		// must run while the instance is still privileged.
		worst_case_sponsored_validation::<T>(INSTANCE_ID);

		#[block]
		{
			// Revision checks (what validate_unload_calls does for each input)
			for input in &inputs {
				let _ = RecyclerManager::<T>::validate_recycler_revision(
					INSTANCE_ID,
					input.value,
					input.index,
					input.revision,
				);
			}
			// Destination checks (what validate_unload_calls does for coin unloads)
			for dest in &destinations {
				let _ = CoinsByOwner::<T>::contains_key(dest);
			}
			if let Some((value, external_asset_amount, loaded_coins)) = &mixed_output_validation {
				if Pallet::<T>::validate_mixed_output_outputs(
					asset_unit::<T>(),
					*value,
					1,
					*external_asset_amount,
					loaded_coins.as_slice(),
				)
				.is_err()
				{
					return Err(BenchmarkError::Skip);
				}
				if Pallet::<T>::ensure_can_charge_load_deposit(
					INSTANCE_ID,
					loaded_coins.len() as u32,
				)
				.is_err()
				{
					return Err(BenchmarkError::Skip);
				}
			}
		}

		Ok(())
	}

	#[benchmark]
	fn direct_offboard_coin_into_external_asset() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		let age = 0u16;
		let coin_owner = create_coin::<T>(value, age, 0);

		let asset_amount = Pallet::<T>::denomination_to_asset_amount(asset_unit::<T>(), value)
			.expect("denomination should be in range");
		fund_pallet_account::<T>(asset_amount);

		let to: T::AccountId = account("dest", 0, 0);

		#[extrinsic_call]
		_(
			Origin::Coin {
				coin_id: coin_owner,
				coin: Coin { instance_id: INSTANCE_ID, value, age },
			},
			to.clone(),
		);

		assert_eq!(T::Fungibles::balance(asset_id::<T>(), &to), asset_amount);

		Ok(())
	}

	// ==================== Authorize benchmarks ====================

	#[benchmark]
	fn authorize_clean_recycler() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		let (_index, _revision, _members) = setup_built_recycler::<T>(value, 1, 0);

		// Advance time past expiration
		let identifier = Pallet::<T>::recycler_collection_identifier(INSTANCE_ID, value);
		let status = T::MemberService::ring_status(&identifier, 0).expect("ring exists");
		let immutable_since = status.immutable_since.expect("ring should be immutable") as u32;
		let expiration = T::RecyclerExpirationTime::get();
		T::BenchmarkHelper::set_time(core::time::Duration::from_secs(
			(immutable_since + expiration + 1) as u64,
		));

		let call = Call::<T>::clean_recycler { instance_id: INSTANCE_ID, value };

		#[block]
		{
			call.authorize(TransactionSource::InBlock)
				.ok_or("Call must give some authorization")??;
		}

		Ok(())
	}

	#[benchmark]
	fn authorize_clean_consumed_free_token() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		// Advance time so period 0 is fully expired (past the grace window).
		let period_length = T::UnloadTokenTimePeriodPeopleLitePeople::get();
		let grace = pallet::FREE_UNLOAD_TOKEN_GRACE_WINDOW;
		T::BenchmarkHelper::set_time(core::time::Duration::from_secs(
			(period_length + grace + 1) as u64,
		));

		let expired_period = 0u32;
		let alias: Alias = [1u8; 32];
		ConsumedFreeUnloadTokens::<T>::insert(expired_period, alias, ());

		let call = Call::<T>::clean_consumed_free_token { period: expired_period };

		#[block]
		{
			call.authorize(TransactionSource::InBlock)
				.ok_or("Call must give some authorization")??;
		}

		Ok(())
	}

	#[benchmark]
	fn authorize_clean_paid_unload_token_ring() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let (period, _index, _members) = setup_built_paid_token_ring::<T>(4, 0);

		// Advance time past expiration
		let expiration_time = (period + 1)
			.saturating_mul(T::PaidUnloadTokenTimePeriod::get())
			.saturating_add(T::PaidUnloadTokenRingExpirationTime::get());
		T::BenchmarkHelper::set_time(core::time::Duration::from_secs(expiration_time as u64 + 1));

		let ring_index: RingIndex = 0;

		let call = Call::<T>::clean_paid_unload_token_ring { period, ring_index };

		#[block]
		{
			call.authorize(TransactionSource::InBlock)
				.ok_or("Call must give some authorization")??;
		}

		Ok(())
	}

	#[benchmark]
	fn authorize_clean_recycler_dust() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		let ring_index: RingIndex = 0;
		RecyclersDusting::<T>::insert((INSTANCE_ID, value, ring_index), ());

		let call = Call::<T>::clean_recycler_dust {};

		#[block]
		{
			call.authorize(TransactionSource::InBlock)
				.ok_or("Call must give some authorization")??;
		}

		Ok(())
	}

	#[benchmark]
	fn authorize_clean_paid_unload_token_dust() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let period: Period = 0;
		PaidUnloadTokenDusting::<T>::insert(BigEndianPeriod::from(period), ());

		let call = Call::<T>::clean_paid_unload_token_dust {};

		#[block]
		{
			call.authorize(TransactionSource::InBlock)
				.ok_or("Call must give some authorization")??;
		}

		Ok(())
	}

	#[benchmark]
	fn authorize_delete_expired_paid_unload_token_collection() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let (period, _index, _members) = setup_built_paid_token_ring::<T>(4, 0);

		// Advance time past expiration
		let expiration_time = (period + 1)
			.saturating_mul(T::PaidUnloadTokenTimePeriod::get())
			.saturating_add(T::PaidUnloadTokenRingExpirationTime::get());
		T::BenchmarkHelper::set_time(core::time::Duration::from_secs(expiration_time as u64 + 1));

		// Clean the ring first (required before deleting collection)
		let ring_index: RingIndex = 0;
		Pallet::<T>::clean_paid_unload_token_ring(
			SystemOrigin::Authorized.into(),
			period,
			ring_index,
		)?;

		let call = Call::<T>::delete_expired_paid_unload_token_collection { period };

		#[block]
		{
			call.authorize(TransactionSource::InBlock)
				.ok_or("Call must give some authorization")??;
		}

		Ok(())
	}

	// ==================== on_poll benchmarks ====================

	/// Benchmark creating a paid token collection for a new period.
	#[benchmark]
	fn on_poll_create_paid_token_collection() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		// common_setup created collection for period at time=3600. Advance to the next period
		// so the benchmark measures actual collection creation, not the no-op path.
		let period_duration = T::PaidUnloadTokenTimePeriod::get();
		let new_time = 3600u64 + period_duration as u64;
		T::BenchmarkHelper::set_time(core::time::Duration::from_secs(new_time));

		// Compute the new period and verify no collection exists yet.
		let period = (new_time as u32) / period_duration;
		assert!(!PaidTokenCollectionsCreated::<T>::contains_key(BigEndianPeriod::from(period)));

		#[block]
		{
			PaidTknManager::<T>::ensure_current_period_collection_exists()
				.expect("should create collection");
		}

		assert!(PaidTokenCollectionsCreated::<T>::contains_key(BigEndianPeriod::from(period)));

		Ok(())
	}

	impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext_bench(), crate::mock::Test);
}
