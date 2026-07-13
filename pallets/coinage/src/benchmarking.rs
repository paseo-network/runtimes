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
		fungibles::Mutate as _,
		Get, UnixTime,
	},
	BoundedVec,
};
use frame_system::RawOrigin as SystemOrigin;
use sp_runtime::{
	traits::{AppendZerosInput, DispatchTransaction, Dispatchable, SaturatedConversion, Zero},
	transaction_validity::TransactionSource,
};
use verifiable::GenerateVerifiable;

type SecretOf<T> = <CryptoOf<T> as GenerateVerifiable>::Secret;
type ProofOf<T> = <CryptoOf<T> as GenerateVerifiable>::Proof;
type SignatureOf<T> = <CryptoOf<T> as GenerateVerifiable>::Signature;
type BoundedProofsOf<T> = BoundedVec<ProofOf<T>, <T as Config>::MaxConsolidation>;
type BoundedAliasesOf<T> = BoundedVec<Alias, <T as Config>::MaxConsolidation>;

struct MixedOutputScenario<T: Config> {
	aliases: BoundedAliasesOf<T>,
	alias_proofs: BoundedProofsOf<T>,
	input_value: CoinValue,
	index: RingIndex,
	revision: RevisionIndex,
	dest: T::AccountId,
	external_asset_amount: FungiblesBalanceOf<T>,
	new_vouchers: BoundedVec<(CoinValue, MemberOf<T>), T::MaxSplitOutputs>,
}

/// Common benchmark setup: set time, assets, conversion rate, and initialize chunks.
fn common_setup<T: Config>() {
	// Set a non-zero time (benchmarks run at genesis where time is 0)
	T::BenchmarkHelper::set_time(core::time::Duration::from_secs(3600));
	T::BenchmarkHelper::setup_assets();
	T::BenchmarkHelper::setup_conversion_rate();

	// Initialize chunks for ring-VRF operations via pallet-members.
	// Initializes both recycler and paid token ring exponents (or just one if they're equal).
	let recycler_exp = T::RecyclerRingExponent::get();
	let paid_exp = T::PaidUnloadTokenRingExponent::get();
	T::MemberService::initialize_chunks(recycler_exp);
	if paid_exp != recycler_exp {
		T::MemberService::initialize_chunks(paid_exp);
	}

	// Pre-create all recycler and current-period paid token collections.
	// This matches the steady-state production path after `on_poll` initialization
	// has already completed, so benchmarks do not include one-time collection
	// creation cost. Real first-use fallback paths may still create collections
	// on demand via `RecyclerManager::ensure_collection_exists`.
	for value in T::MinimumExponent::get()..=T::MaximumExponent::get() {
		RecyclerManager::<T>::ensure_collection_exists(value)
			.expect("recycler collection creation should succeed");
	}
	PaidTknManager::<T>::ensure_current_period_collection_exists()
		.expect("paid token collection creation should succeed");
}

/// Underlying asset id helper for benchmarks.
///
/// `common_setup` calls `T::BenchmarkHelper::setup_assets()`, which writes the storage. Any
/// benchmark that performs `common_setup` first can call this to unwrap without noise.
fn asset_id<T: Config>() -> FungiblesAssetIdOf<T> {
	Pallet::<T>::underlying_asset_id().expect("set by setup_assets in common_setup")
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
fn create_coin<T: Config>(value: CoinValue, age: u16, seed: u32) -> T::AccountId {
	let owner: T::AccountId = account("coin_owner", seed, 0);
	let coin = Coin { value, age };
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
	value: CoinValue,
	count: u32,
	seed: u32,
) -> Vec<(SecretOf<T>, MemberOf<T>)> {
	let mut members = Vec::new();
	for i in 0..count {
		let (secret, member) = new_member_from::<T>(i, seed);
		members.push((secret, member.clone()));
		RecyclerManager::<T>::load(value, member).expect("should load");
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
	value: CoinValue,
	count: u32,
	seed: u32,
) -> (RingIndex, RevisionIndex, Vec<(SecretOf<T>, MemberOf<T>)>) {
	let padded_count = count.max(T::RecyclerRingExponent::get().ring_capacity());
	let members = setup_recycler_with_pending::<T>(value, padded_count, seed);
	let identifier = Pallet::<T>::recycler_collection_identifier(value);
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
		let cache_key = sp_core::hashing::blake2_256(&(&member, all_members, msg).encode());
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

		let identifier = Pallet::<T>::recycler_collection_identifier(value);
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

		let asset_amount =
			Pallet::<T>::coin_value_to_asset_amount(value).expect("coin value should be in range");
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
	use verifiable::BatchProofItem;

	fn setup_single_recycler_unload_prepaid<T: Config>(
		n: u32,
		fund_multiplier: u32,
	) -> (
		BoundedAliasesOf<T>,
		BoundedProofsOf<T>,
		CoinValue,
		RingIndex,
		RevisionIndex,
		T::AccountId,
		FungiblesBalanceOf<T>,
	) {
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		let (index, revision, members) = setup_built_recycler::<T>(value, n, 0);
		let asset_amount =
			Pallet::<T>::coin_value_to_asset_amount(value).expect("coin value should be in range");
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
		let asset_amount =
			Pallet::<T>::coin_value_to_asset_amount(value).expect("coin value should be in range");
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

		let proven_msg = sp_core::hashing::blake2_256(&(&inputs, &dest, &caller).encode());
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
	) -> (
		Vec<UnloadRecyclerInput<T::MaxConsolidation>>,
		BoundedProofsOf<T>,
		T::AccountId,
		T::AccountId,
		FungiblesBalanceOf<T>,
	) {
		common_setup::<T>();

		let (inputs, sign_data, total_asset_amount) = setup_multi_recyclers::<T>(n, 0);
		let caller: T::AccountId = account("caller", 0, 0);
		let dest: T::AccountId = account("dest", 0, 0);

		fund_pallet_account::<T>(total_asset_amount);
		T::BenchmarkHelper::fund_account(&caller, total_asset_amount.saturating_mul(10u32.into()));

		let proven_msg = sp_core::hashing::blake2_256(&(&inputs, &dest, &caller).encode());
		let mut alias_proofs = Vec::new();
		for (secret, actual_ring_members) in &sign_data {
			let (proof, _) = generate_alias_proof::<T>(secret, actual_ring_members, &proven_msg);
			alias_proofs.push(proof);
		}
		let bounded_proofs: BoundedProofsOf<T> = alias_proofs.try_into().unwrap();

		(inputs, bounded_proofs, caller, dest, total_asset_amount)
	}

	/// Sets up a benchmark scenario for unloading a recycler into coins.
	/// - `a`: number of input aliases (coins consumed from the recycler)
	/// - `d`: number of destination output coins to produce
	fn setup_unload_recycler_into_coins<T: Config>(
		a: u32,
		d: u32,
	) -> Result<
		(
			BoundedAliasesOf<T>,
			BoundedProofsOf<T>,
			CoinValue,
			RingIndex,
			RevisionIndex,
			BoundedVec<
				(CoinValue, BoundedVec<T::AccountId, T::MaxSplitOutputs>),
				T::MaxSplitOutputs,
			>,
		),
		BenchmarkError,
	> {
		common_setup::<T>();

		let min_exp = T::MinimumExponent::get();
		let max_exp = T::MaximumExponent::get();

		// Since coin values are powers of 2, the minimum number of outputs needed to represent
		// `a` input coins is the popcount of `a` (each set bit = one output denomination).
		// Skip if we can't possibly produce `d` outputs from `a` inputs.
		if d < a.count_ones() {
			return Err(BenchmarkError::Skip);
		}

		// Determines the minimum input denomination needed so that `a` coins can be split into
		// exactly `d` output coins while preserving total value. It works by:
		// 1. Finding the smallest exponent offset `k` such that `a * 2^k >= d` (each input coin is
		//    worth 2^k minimum-denomination units, we want to produce `d` outputs of at least 1
		//    unit each)
		// 2. Computing total_units = `a * 2^k` and decomposing it into binary to get the initial
		//    split (minimum number of pieces that sum to total_units)
		// 3. Repeatedly splitting the largest piece (2^n → two 2^(n-1)) until we have `d` pieces
		// 4. Grouping pieces by denomination and assigning destination accounts
		//
		// Example: `a=3, d=5` (3 inputs → 5 outputs)
		// - k=1 because 3 * 2^1 = 6 >= 5
		// - Input coins have denomination `min_exp + 1`
		// - total_units = 6 = binary 110 → initial pieces at bit positions 1 and 2: [1, 2]
		// - Split pieces by halving the largest until we have 5 pieces (each step: 2^n →
		//   2×2^(n-1)): [1, 2] → [1, 1, 1] → [1, 1, 0, 0] → [1, 0, 0, 0, 0]
		// - Final output: 4 coins of value 2^0=1 and 1 coin of value 2^1=2 (total = 4×1 + 1×2 = 6
		//   ✓)

		// 1. Find the smallest exponent offset `k` such that `a * 2^k >= d`.
		let mut k = 0i8;
		while (a as u64).saturating_mul(1u64 << (k as u32)) < (d as u64) {
			k += 1;
		}

		let input_value = min_exp.saturating_add(k);
		if input_value > max_exp {
			return Err(BenchmarkError::Skip);
		}

		// Total value in minimum-denomination units (i.e., how many "base coins" worth).
		let total_units = (a as u64).saturating_mul(1u64 << (k as u32));
		// Check that the largest output denomination fits within allowed range.
		let highest_bit = 63 - total_units.leading_zeros() as i8;
		if min_exp.saturating_add(highest_bit) > max_exp {
			return Err(BenchmarkError::Skip);
		}

		// 2. Decompose total_units into powers of 2 (binary representation).
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
			if let Some(largest) = pieces.pop() {
				if largest > 0 {
					// Split: 2^largest = 2^(largest-1) + 2^(largest-1)
					pieces.push(largest - 1);
					pieces.push(largest - 1);
				} else {
					// Can't split a piece of value 2^0 = 1
					return Err(BenchmarkError::Skip);
				}
			} else {
				return Err(BenchmarkError::Skip);
			}
		}

		pieces.sort_unstable();

		// 4. Group pieces by denomination and assign destination accounts.
		// Result: Vec of (coin_value, destinations) for the split_into parameter.
		let mut dest_idx = 0u32;
		let mut split_into: Vec<(CoinValue, BoundedVec<T::AccountId, T::MaxSplitOutputs>)> =
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
		let asset_amount = Pallet::<T>::coin_value_to_asset_amount(input_value)
			.expect("coin value should be in range");
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

		Ok((aliases, bounded_proofs, input_value, index, revision, split_into))
	}

	/// Sets up a benchmark scenario for unloading a recycler into external asset and vouchers.
	/// - `a`: number of input aliases (coins consumed from the recycler)
	/// - `d`: number of voucher outputs to produce
	fn select_mixed_output_units<T: Config>(
		a: u32,
		d: u32,
	) -> Result<(CoinValue, u64), BenchmarkError> {
		let min_exp = T::MinimumExponent::get();
		let max_exp = T::MaximumExponent::get();

		let mut extra_exp = 0i8;
		loop {
			let input_value = min_exp.saturating_add(extra_exp);
			if input_value > max_exp {
				return Err(BenchmarkError::Skip)
			}

			let total_units =
				(a as u64).checked_shl(extra_exp as u32).ok_or(BenchmarkError::Skip)?;
			if total_units <= d as u64 {
				extra_exp = extra_exp.saturating_add(1);
				continue
			}

			for voucher_units in (d as u64..total_units).rev() {
				let highest_piece_exp =
					if voucher_units == 0 { 0 } else { 63 - voucher_units.leading_zeros() as i8 };
				if d >= voucher_units.count_ones() &&
					min_exp.saturating_add(highest_piece_exp) <= max_exp
				{
					return Ok((input_value, voucher_units))
				}
			}

			extra_exp = extra_exp.saturating_add(1);
		}
	}

	fn decompose_voucher_units(voucher_units: u64, d: u32) -> Result<Vec<i8>, BenchmarkError> {
		let mut voucher_piece_exponents = Vec::new();
		for bit in 0..64 {
			if (voucher_units & (1u64 << bit)) != 0 {
				voucher_piece_exponents.push(bit as i8);
			}
		}

		while voucher_piece_exponents.len() < d as usize {
			voucher_piece_exponents.sort_unstable();
			let Some(largest_piece_exp) = voucher_piece_exponents.pop() else {
				return Err(BenchmarkError::Skip)
			};
			if largest_piece_exp == 0 {
				return Err(BenchmarkError::Skip)
			}
			voucher_piece_exponents.push(largest_piece_exp - 1);
			voucher_piece_exponents.push(largest_piece_exp - 1);
		}

		voucher_piece_exponents.sort_unstable();
		Ok(voucher_piece_exponents)
	}

	fn setup_unload_recycler_into_external_asset_and_vouchers<T: Config>(
		a: u32,
		d: u32,
	) -> Result<MixedOutputScenario<T>, BenchmarkError> {
		common_setup::<T>();

		let min_exp = T::MinimumExponent::get();
		let amount_per_unit = Pallet::<T>::coin_value_to_asset_amount(min_exp)
			.expect("minimum exponent should be in range");

		let (input_value, voucher_units) = select_mixed_output_units::<T>(a, d)?;

		let total_units = Pallet::<T>::coin_value_to_unit(input_value)
			.ok_or(BenchmarkError::Skip)?
			.checked_mul(a)
			.ok_or(BenchmarkError::Skip)? as u64;
		let external_asset_units =
			total_units.checked_sub(voucher_units).ok_or(BenchmarkError::Skip)?;

		let voucher_piece_exponents = decompose_voucher_units(voucher_units, d)?;

		let new_vouchers: BoundedVec<_, T::MaxSplitOutputs> = voucher_piece_exponents
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
		let asset_amount = Pallet::<T>::coin_value_to_asset_amount(input_value)
			.expect("coin value should be in range");
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
			new_vouchers,
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
		_(Origin::Coin { coin_id: coin_owner.clone(), coin: Coin { value, age } }, split_into);

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
		_(Origin::Coin { coin_id: coin_owner.clone(), coin: Coin { value, age } }, dest.clone());

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
		let asset_amount =
			Pallet::<T>::coin_value_to_asset_amount(value).expect("coin value should be in range");
		fund_pallet_account::<T>(asset_amount);

		#[extrinsic_call]
		_(
			Origin::Coin { coin_id: coin_owner.clone(), coin: Coin { value, age } },
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
		let fee = Pallet::<T>::paid_unload_token_fee_in_asset()
			.expect("fee should be available after setup");
		let mut value = T::MinimumExponent::get();
		while Pallet::<T>::coin_value_to_asset_amount(value).unwrap_or_default() < fee {
			value = value.saturating_add(1);
		}

		let age = 0u16;
		let coin_owner = create_coin::<T>(value, age, 0);

		let (secret, member_key) = new_member_from::<T>(0, 0);
		let proof_of_ownership = create_proof_of_ownership::<T>(&secret, &coin_owner);

		// Fund pallet account
		let asset_amount =
			Pallet::<T>::coin_value_to_asset_amount(value).expect("coin value should be in range");
		fund_pallet_account::<T>(asset_amount);

		// Fund fee destination with minimum balance
		let fee_dest = T::FeeDestination::get();
		T::Fungibles::mint_into(asset_id::<T>(), &fee_dest, fee).expect("should mint to fee dest");

		#[extrinsic_call]
		_(
			Origin::Coin { coin_id: coin_owner.clone(), coin: Coin { value, age } },
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
		let asset_amount =
			Pallet::<T>::coin_value_to_asset_amount(value).expect("coin value should be in range");

		// Fund caller with the asset
		T::BenchmarkHelper::fund_account(&caller, asset_amount * 2u32.into());

		let (secret, member_key) = new_member_from::<T>(0, 0);
		let proof_of_ownership = create_proof_of_ownership::<T>(&secret, &caller);

		#[extrinsic_call]
		_(
			SystemOrigin::Signed(caller.clone()),
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

		// Fund caller with native currency (fee + ED so account survives the transfer)
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
		let fee = Pallet::<T>::paid_unload_token_fee_in_asset()
			.expect("fee should be available after setup");
		T::BenchmarkHelper::fund_account(&caller, fee * 2u32.into());

		// Fund fee destination
		let fee_dest = T::FeeDestination::get();
		T::BenchmarkHelper::fund_account(&fee_dest, fee);

		let (secret, member_key) = new_member_from::<T>(0, 0);
		let proof_of_ownership = create_proof_of_ownership::<T>(&secret, &caller);

		#[extrinsic_call]
		_(SystemOrigin::Signed(caller.clone()), member_key.clone(), proof_of_ownership);

		assert!(PaidUnloadTokenMembers::<T>::contains_key(&member_key));

		Ok(())
	}

	// ==================== Root/OCW extrinsics ====================

	#[benchmark]
	fn clean_recycler(
		// Number of members in the ring.
		n: Linear<1, { T::RecyclerRingExponent::get().ring_capacity() }>,
		// Number of unloaded aliases.
		m: Linear<0, { T::RecyclerRingExponent::get().ring_capacity() }>,
	) -> Result<(), BenchmarkError> {
		// Note: m > n is impossible in practice (can't have more unloaded than members), but the
		// benchmark is still valid because the cost depends on iterating over unloaded entries to
		// count them, independent of the actual member count.
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		// Fill ring 0 with n members. This advances CurrentRingIndex to 1 and sets
		// immutable_since on ring 0.
		let (_index, _revision, _members) = setup_built_recycler::<T>(value, n, 0);
		let identifier = Pallet::<T>::recycler_collection_identifier(value);

		// Insert m entries into RecyclersUnloaded to simulate unloaded aliases.
		// This captures the cost of iterating over unloaded entries in `clean_unchecked`.
		let ring_index: RingIndex = 0;
		for i in 0..m {
			let mut alias: Alias = [0u8; 32];
			alias[0..4].copy_from_slice(&i.to_le_bytes());
			RecyclersUnloaded::<T>::insert((value, ring_index, alias), ());
		}

		// Advance time past expiration
		let status = T::MemberService::ring_status(&identifier, 0).expect("ring exists");
		let immutable_since = status.immutable_since.expect("ring should be immutable") as u32;
		let expiration = T::RecyclerExpirationTime::get();
		T::BenchmarkHelper::set_time(core::time::Duration::from_secs(
			(immutable_since + expiration + 1) as u64,
		));

		#[extrinsic_call]
		_(SystemOrigin::Authorized, value);

		assert_eq!(RecyclersLastRemovedRingIndex::<T>::get(value), Some(0));

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

		// Insert n entries into RecyclersUnloaded for this (value, ring_index)
		for i in 0..n {
			let mut alias: Alias = [0u8; 32];
			alias[0..4].copy_from_slice(&i.to_le_bytes());
			RecyclersUnloaded::<T>::insert((value, ring_index, alias), ());
		}

		// Set the dusting flag so the extrinsic's authorize check passes
		RecyclersDusting::<T>::insert((value, ring_index), ());

		#[extrinsic_call]
		_(SystemOrigin::Authorized);

		// All entries should be removed (n <= DUST_CLEANUP_BATCH_SIZE)
		assert_eq!(RecyclersUnloaded::<T>::iter_prefix((value, ring_index)).count(), 0);

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
			setup_single_recycler_unload_prepaid::<T>(n, 1);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_coin(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
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
			setup_single_recycler_unload_prepaid::<T>(n, 1);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_coin(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
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
			setup_single_recycler_unload_prepaid::<T>(n, 1);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_coin(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
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
	fn unload_recycler_into_external_asset_1_2(n: Linear<1, 2>) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, value, index, revision, dest, asset_amount) =
			setup_single_recycler_unload_prepaid::<T>(n, 2);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				aliases,
				value,
				index,
				revision,
				dest.clone(),
			)?;
		}

		assert_eq!(T::Fungibles::balance(asset_id::<T>(), &dest), asset_amount * n.into(),);

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_external_asset_3_8(n: Linear<3, 8>) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, value, index, revision, dest, asset_amount) =
			setup_single_recycler_unload_prepaid::<T>(n, 2);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				aliases,
				value,
				index,
				revision,
				dest.clone(),
			)?;
		}

		assert_eq!(T::Fungibles::balance(asset_id::<T>(), &dest), asset_amount * n.into(),);

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_external_asset_9_max(
		n: Linear<
			9,
			{ T::MaxConsolidation::get().min(T::RecyclerRingExponent::get().ring_capacity()) },
		>,
	) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, value, index, revision, dest, asset_amount) =
			setup_single_recycler_unload_prepaid::<T>(n, 2);

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				aliases,
				value,
				index,
				revision,
				dest.clone(),
			)?;
		}

		assert_eq!(T::Fungibles::balance(asset_id::<T>(), &dest), asset_amount * n.into(),);

		Ok(())
	}

	/// The benchmark for `unload_recycler_into_external_asset_and_vouchers` is split into three
	/// separate benchmarks for different range of `a` (number of input aliases).
	/// This is because the cost is not linear on `a`, there is a sublinear coefficient for the cost
	/// of the batch validation of the proofs.
	/// By benchmarking on a smaller range we approximate the sublinear cost over a range.
	///
	/// `d` scales the voucher output count over the full `MaxSplitOutputs` range.
	#[benchmark]
	fn unload_recycler_into_external_asset_and_vouchers_1_2(
		a: Linear<1, 2>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let scenario = setup_unload_recycler_into_external_asset_and_vouchers::<T>(a, d)?;
		let aliases_copy = scenario.aliases.clone();
		let new_vouchers_copy = scenario.new_vouchers.clone();

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset_and_vouchers(
				Origin::UnloadToken {
					alias_proofs: scenario.alias_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				scenario.aliases,
				scenario.input_value,
				scenario.index,
				scenario.revision,
				scenario.dest.clone(),
				scenario.external_asset_amount,
				scenario.new_vouchers,
			)?;
		}

		assert_eq!(
			T::Fungibles::balance(asset_id::<T>(), &scenario.dest),
			scenario.external_asset_amount,
		);
		for alias in &aliases_copy {
			assert!(RecyclersUnloaded::<T>::contains_key((
				scenario.input_value,
				scenario.index,
				alias,
			)));
		}
		for (value, member_key) in &new_vouchers_copy {
			assert_eq!(RecyclersCoinToRecycler::<T>::get(member_key), Some(*value));
		}

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_external_asset_and_vouchers_3_8(
		a: Linear<3, 8>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let scenario = setup_unload_recycler_into_external_asset_and_vouchers::<T>(a, d)?;
		let aliases_copy = scenario.aliases.clone();
		let new_vouchers_copy = scenario.new_vouchers.clone();

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset_and_vouchers(
				Origin::UnloadToken {
					alias_proofs: scenario.alias_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				scenario.aliases,
				scenario.input_value,
				scenario.index,
				scenario.revision,
				scenario.dest.clone(),
				scenario.external_asset_amount,
				scenario.new_vouchers,
			)?;
		}

		assert_eq!(
			T::Fungibles::balance(asset_id::<T>(), &scenario.dest),
			scenario.external_asset_amount,
		);
		for alias in &aliases_copy {
			assert!(RecyclersUnloaded::<T>::contains_key((
				scenario.input_value,
				scenario.index,
				alias,
			)));
		}
		for (value, member_key) in &new_vouchers_copy {
			assert_eq!(RecyclersCoinToRecycler::<T>::get(member_key), Some(*value));
		}

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_external_asset_and_vouchers_9_max(
		a: Linear<
			9,
			{ T::MaxConsolidation::get().min(T::RecyclerRingExponent::get().ring_capacity()) },
		>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let scenario = setup_unload_recycler_into_external_asset_and_vouchers::<T>(a, d)?;
		let aliases_copy = scenario.aliases.clone();
		let new_vouchers_copy = scenario.new_vouchers.clone();

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset_and_vouchers(
				Origin::UnloadToken {
					alias_proofs: scenario.alias_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				scenario.aliases,
				scenario.input_value,
				scenario.index,
				scenario.revision,
				scenario.dest.clone(),
				scenario.external_asset_amount,
				scenario.new_vouchers,
			)?;
		}

		assert_eq!(
			T::Fungibles::balance(asset_id::<T>(), &scenario.dest),
			scenario.external_asset_amount,
		);
		for alias in &aliases_copy {
			assert!(RecyclersUnloaded::<T>::contains_key((
				scenario.input_value,
				scenario.index,
				alias,
			)));
		}
		for (value, member_key) in &new_vouchers_copy {
			assert_eq!(RecyclersCoinToRecycler::<T>::get(member_key), Some(*value));
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

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset_non_anonymous(
				frame_system::RawOrigin::Signed(caller).into(),
				input,
				bounded_proofs,
				dest.clone(),
				FeeCurrency::ExternalAsset,
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

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset_non_anonymous(
				frame_system::RawOrigin::Signed(caller).into(),
				input,
				bounded_proofs,
				dest.clone(),
				FeeCurrency::ExternalAsset,
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

		#[block]
		{
			Pallet::<T>::unload_recycler_into_external_asset_non_anonymous(
				frame_system::RawOrigin::Signed(caller).into(),
				input,
				bounded_proofs,
				dest.clone(),
				FeeCurrency::ExternalAsset,
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

		#[block]
		{
			Pallet::<T>::unload_recyclers_into_external_asset_non_anonymous(
				frame_system::RawOrigin::Signed(caller).into(),
				inputs,
				bounded_proofs,
				dest.clone(),
				FeeCurrency::ExternalAsset,
			)?;
		}

		assert_eq!(T::Fungibles::balance(asset_id::<T>(), &dest), total_asset_amount,);

		Ok(())
	}

	#[benchmark]
	fn unload_recyclers_into_external_asset_non_anonymous_3_8(
		n: Linear<3, 8>,
	) -> Result<(), BenchmarkError> {
		let (inputs, bounded_proofs, caller, dest, total_asset_amount) =
			setup_multi_recycler_unload_non_anonymous::<T>(n);

		#[block]
		{
			Pallet::<T>::unload_recyclers_into_external_asset_non_anonymous(
				frame_system::RawOrigin::Signed(caller).into(),
				inputs,
				bounded_proofs,
				dest.clone(),
				FeeCurrency::ExternalAsset,
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

		#[block]
		{
			Pallet::<T>::unload_recyclers_into_external_asset_non_anonymous(
				frame_system::RawOrigin::Signed(caller).into(),
				inputs,
				bounded_proofs,
				dest.clone(),
				FeeCurrency::ExternalAsset,
			)?;
		}

		assert_eq!(T::Fungibles::balance(asset_id::<T>(), &dest), total_asset_amount,);

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
	fn unload_recycler_into_coins_1_2(
		a: Linear<1, 2>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, input_value, index, revision, split_into) =
			setup_unload_recycler_into_coins::<T>(a, d)?;
		let aliases_copy = aliases.clone();
		let split_into_copy = split_into.clone();

		// TODO: This benchmark uses Prepaid fee mode, but the worst case is FromOutput which
		// includes additional fee deduction and burn operations.
		#[block]
		{
			Pallet::<T>::unload_recycler_into_coins(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				aliases,
				input_value,
				index,
				revision,
				split_into,
				Zero::zero(),
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
			assert!(RecyclersUnloaded::<T>::contains_key((input_value, index, alias)));
		}

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_coins_3_8(
		a: Linear<3, 8>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, input_value, index, revision, split_into) =
			setup_unload_recycler_into_coins::<T>(a, d)?;
		let aliases_copy = aliases.clone();
		let split_into_copy = split_into.clone();

		// TODO: This benchmark uses Prepaid fee mode, but the worst case is FromOutput which
		// includes additional fee deduction and burn operations.
		#[block]
		{
			Pallet::<T>::unload_recycler_into_coins(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				aliases,
				input_value,
				index,
				revision,
				split_into,
				Zero::zero(),
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
			assert!(RecyclersUnloaded::<T>::contains_key((input_value, index, alias)));
		}

		Ok(())
	}

	#[benchmark]
	fn unload_recycler_into_coins_9_max(
		a: Linear<
			9,
			{ T::MaxConsolidation::get().min(T::RecyclerRingExponent::get().ring_capacity()) },
		>,
		d: Linear<1, { T::MaxSplitOutputs::get() }>,
	) -> Result<(), BenchmarkError> {
		let (aliases, bounded_proofs, input_value, index, revision, split_into) =
			setup_unload_recycler_into_coins::<T>(a, d)?;
		let aliases_copy = aliases.clone();
		let split_into_copy = split_into.clone();

		// TODO: This benchmark uses Prepaid fee mode, but the worst case is FromOutput which
		// includes additional fee deduction and burn operations.
		#[block]
		{
			Pallet::<T>::unload_recycler_into_coins(
				Origin::UnloadToken {
					alias_proofs: bounded_proofs,
					proven_msg: [0u8; 32],
					fee: UnloadFee::Prepaid,
				}
				.into(),
				aliases,
				input_value,
				index,
				revision,
				split_into,
				Zero::zero(),
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
			assert!(RecyclersUnloaded::<T>::contains_key((input_value, index, alias)));
		}

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
		let identifier = Pallet::<T>::recycler_collection_identifier(value);
		let items: Vec<BatchProofItem<ProofOf<T>>> = proofs
			.iter()
			.map(|proof| BatchProofItem {
				proof: proof.clone(),
				message: proven_msg.to_vec(),
				context: UNLOADING_RECYCLER_CONTEXT.to_vec(),
			})
			.collect();

		#[block]
		{
			let results =
				T::MemberService::verify_memberships_in_ring(&identifier, ring_index, &items)
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
		let identifier = Pallet::<T>::recycler_collection_identifier(value);
		let items: Vec<BatchProofItem<ProofOf<T>>> = proofs
			.iter()
			.map(|proof| BatchProofItem {
				proof: proof.clone(),
				message: proven_msg.to_vec(),
				context: UNLOADING_RECYCLER_CONTEXT.to_vec(),
			})
			.collect();

		#[block]
		{
			let results =
				T::MemberService::verify_memberships_in_ring(&identifier, ring_index, &items)
					.expect("batch verify: small batch failed");
			assert_eq!(results.len(), items.len());
		}

		Ok(())
	}

	#[benchmark(extra)]
	fn batch_verify_recycler_medium(n: Linear<4, 8>) -> Result<(), BenchmarkError> {
		let (value, ring_index, _aliases, proofs, proven_msg) =
			T::BenchmarkHelper::setup_batch_verify(n)?;
		let identifier = Pallet::<T>::recycler_collection_identifier(value);
		let items: Vec<BatchProofItem<ProofOf<T>>> = proofs
			.iter()
			.map(|proof| BatchProofItem {
				proof: proof.clone(),
				message: proven_msg.to_vec(),
				context: UNLOADING_RECYCLER_CONTEXT.to_vec(),
			})
			.collect();

		#[block]
		{
			let results =
				T::MemberService::verify_memberships_in_ring(&identifier, ring_index, &items)
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
		let identifier = Pallet::<T>::recycler_collection_identifier(value);
		let items: Vec<BatchProofItem<ProofOf<T>>> = proofs
			.iter()
			.map(|proof| BatchProofItem {
				proof: proof.clone(),
				message: proven_msg.to_vec(),
				context: UNLOADING_RECYCLER_CONTEXT.to_vec(),
			})
			.collect();

		#[block]
		{
			let results =
				T::MemberService::verify_memberships_in_ring(&identifier, ring_index, &items)
					.expect("batch verify: large batch failed");
			assert_eq!(results.len(), items.len());
		}

		Ok(())
	}
	// ==================== Transaction extension benchmarks ====================

	/// Benchmark for AsCoinage(None) with calls that don't require revision validation.
	#[benchmark]
	fn as_none_tx_ext_others() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		// Use a call that doesn't trigger revision validation (e.g., transfer)
		let value = T::MinimumExponent::get();
		let age = 0u16;
		let coin_owner = create_coin::<T>(value, age, 0);
		let dest: T::AccountId = account("dest", 0, 0);

		let call = Call::<T>::transfer { to: dest };

		let tx_ext = AsCoinage::<T>::new(None);
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
			input,
			alias_proofs,
			to: account("dest", 0, 0),
			fee_currency: FeeCurrency::ExternalAsset,
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
			inputs,
			alias_proofs: bounded_proofs,
			to: account("dest", 0, 0),
			fee_currency: FeeCurrency::ExternalAsset,
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
		// Coin value = 2^(min_exp + n - 1)
		let value = min_exp.saturating_add((n as i8 - 1).max(0));
		let age = 0u16;
		let coin_owner = create_coin::<T>(value, age, 0);

		// Build split_into with multiple value groups (must be strictly ascending)
		let mut split_into: Vec<(CoinValue, BoundedVec<T::AccountId, T::MaxSplitOutputs>)> =
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
		let fee = Pallet::<T>::paid_unload_token_fee_in_asset()
			.expect("fee should be available after setup");
		let mut value = T::MinimumExponent::get();
		while Pallet::<T>::coin_value_to_asset_amount(value).unwrap_or_default() < fee {
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
			aliases: vec![alias].try_into().unwrap(),
			value,
			index,
			revision,
			to: account("dest", 0, 0),
		};

		let runtime_call: <T as frame_system::Config>::RuntimeCall = call.clone().into();
		let inherited_implication = ((0u8, &runtime_call), (), ());
		let proven_msg = sp_core::hashing::blake2_256(&inherited_implication.encode());

		// Generate alias proof with proven_msg
		let (alias_proof, _) = generate_alias_proof::<T>(secret, &members_only, &proven_msg);
		let alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation> =
			vec![alias_proof].try_into().unwrap();

		// Create a people proof with intent message (alias_proofs ++ inherited_implication)
		let context = pallet::free_unload_token_context(period, counter);
		let intent_msg = sp_core::hashing::blake2_256(
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
			aliases: vec![alias].try_into().unwrap(),
			value,
			index,
			revision,
			to: account("dest", 0, 0),
		};

		let runtime_call: <T as frame_system::Config>::RuntimeCall = call.clone().into();
		let inherited_implication = ((0u8, &runtime_call), (), ());
		let proven_msg = sp_core::hashing::blake2_256(&inherited_implication.encode());

		// Generate alias proof with proven_msg
		let (alias_proof, _) = generate_alias_proof::<T>(secret, &members_only, &proven_msg);
		let alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation> =
			vec![alias_proof].try_into().unwrap();

		// Create a lite people proof with intent message (alias_proofs ++ inherited_implication)
		let context = pallet::free_unload_token_context(period, counter);
		let intent_msg = sp_core::hashing::blake2_256(
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
			aliases: vec![alias].try_into().unwrap(),
			value,
			index: recycler_index,
			revision: recycler_revision,
			to: account("dest", 0, 0),
		};

		let runtime_call: <T as frame_system::Config>::RuntimeCall = call.clone().into();
		let inherited_implication = ((0u8, &runtime_call), (), ());
		let proven_msg = sp_core::hashing::blake2_256(&inherited_implication.encode());

		// Generate alias proof with proven_msg
		let (alias_proof, _) =
			generate_alias_proof::<T>(recycler_secret, &recycler_members_only, &proven_msg);
		let alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation> =
			vec![alias_proof].try_into().unwrap();

		// Generate paid token proof with intent message (alias_proofs ++ inherited_implication)
		let intent_msg = sp_core::hashing::blake2_256(
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

		// Setup recycler with a coin value large enough for the penalty fee.
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
			aliases: vec![alias].try_into().unwrap(),
			value,
			index,
			revision,
			to: account("dest", 0, 0),
		};

		let runtime_call: <T as frame_system::Config>::RuntimeCall = call.clone().into();
		let inherited_implication = ((0u8, &runtime_call), (), ());

		// No other alias proofs (single alias benchmark)
		let other_proofs = Vec::<ProofOf<T>>::new();

		// Generate first alias proof signing (other_proofs ++ inherited_implication)
		let intent_msg = sp_core::hashing::blake2_256(
			&[other_proofs.encode(), inherited_implication.encode()].concat(),
		);
		let (first_alias_proof, _) = generate_alias_proof::<T>(secret, &members_only, &intent_msg);
		let alias_proofs: BoundedVec<ProofOf<T>, T::MaxConsolidation> =
			vec![first_alias_proof].try_into().unwrap();

		let tx_ext = AsCoinage::<T>::new(Some(AsCoinageInfo::AsUnloadTokenFromOutput {
			fee_recycler_value: value,
			fee_recycler_index: index,
			fee_recycler_revision: revision,
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
		let asset_amount =
			Pallet::<T>::coin_value_to_asset_amount(value).expect("coin value should be in range");

		T::BenchmarkHelper::fund_account(&caller, asset_amount * 2u32.into());

		let (secret, member_key) = new_member_from::<T>(0, 0);
		let proof_of_ownership = create_proof_of_ownership::<T>(&secret, &caller);

		let origin: T::RuntimeOrigin = Origin::<T>::InfallibleUnpaidSigned { who: caller }.into();

		#[extrinsic_call]
		_(
			origin as T::RuntimeOrigin,
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

		let caller: T::AccountId = account("caller", 0, 0);
		let value = T::MinimumExponent::get();
		let asset_amount =
			Pallet::<T>::coin_value_to_asset_amount(value).expect("coin value should be in range");

		// Fund caller with the external asset.
		T::BenchmarkHelper::fund_account(&caller, asset_amount * 2u32.into());

		let (secret, member_key) = new_member_from::<T>(0, 0);
		let proof_of_ownership = create_proof_of_ownership::<T>(&secret, &caller);

		let call = Call::<T>::load_recycler_with_external_asset_unpaid {
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

	// ==================== Validation benchmarks ====================

	/// Benchmark `validate_unload_calls` operations.
	/// - `r`: number of recycler revision checks
	/// - `d`: number of output validations (coin destinations and voucher outputs)
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
			let scenario = setup_unload_recycler_into_external_asset_and_vouchers::<T>(1, d)?;
			Some((scenario.input_value, scenario.external_asset_amount, scenario.new_vouchers))
		} else {
			None
		};

		// Setup `d` destination accounts (that don't have coins)
		let mut destinations: Vec<T::AccountId> = Vec::new();
		for i in 0..d {
			destinations.push(account("dest", i, 0));
		}

		#[block]
		{
			// Revision checks (what validate_unload_calls does for each input)
			for input in &inputs {
				let _ = RecyclerManager::<T>::validate_recycler_revision(
					input.value,
					input.index,
					input.revision,
				);
			}
			// Destination checks (what validate_unload_calls does for coin unloads)
			for dest in &destinations {
				let _ = CoinsByOwner::<T>::contains_key(dest);
			}
			if let Some((value, external_asset_amount, new_vouchers)) = &mixed_output_validation {
				if Pallet::<T>::validate_mixed_output_outputs(
					*value,
					1,
					*external_asset_amount,
					new_vouchers.as_slice(),
				)
				.is_err()
				{
					return Err(BenchmarkError::Skip)
				}
			}
		}

		Ok(())
	}

	#[benchmark]
	fn set_underlying_asset_id() -> Result<(), BenchmarkError> {
		// Capture the asset id the helper would set, then clear storage so the setter
		// actually performs the write (single-set semantics reject a second call).
		T::BenchmarkHelper::setup_assets();
		let asset_id = Pallet::<T>::underlying_asset_id()
			.expect("BenchmarkHelper::setup_assets must populate UnderlyingAssetId");
		UnderlyingAssetId::<T>::kill();

		let origin = T::UnderlyingAssetIdManager::try_successful_origin()
			.map_err(|_| BenchmarkError::Weightless)?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, asset_id.clone());

		assert_eq!(UnderlyingAssetId::<T>::get(), Some(asset_id.clone()));
		frame_system::Pallet::<T>::assert_last_event(
			Event::UnderlyingAssetIdSet { asset_id }.into(),
		);
		Ok(())
	}

	#[benchmark]
	fn direct_offboard_coin_into_external_asset() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		let age = 0u16; // Must be fresh (age == 0) for direct offboard
		let coin_owner = create_coin::<T>(value, age, 0);

		let amount =
			Pallet::<T>::coin_value_to_asset_amount(value).expect("coin value should be in range");
		fund_pallet_account::<T>(amount);

		let to: T::AccountId = account("dest", 0, 0);

		#[extrinsic_call]
		_(Origin::Coin { coin_id: coin_owner, coin: Coin { value, age } }, to.clone());

		assert_eq!(T::Fungibles::balance(asset_id::<T>(), &to), amount);

		Ok(())
	}

	// ==================== Authorize benchmarks ====================

	#[benchmark]
	fn authorize_clean_recycler() -> Result<(), BenchmarkError> {
		common_setup::<T>();

		let value = T::MinimumExponent::get();
		let (_index, _revision, _members) = setup_built_recycler::<T>(value, 1, 0);

		// Advance time past expiration
		let identifier = Pallet::<T>::recycler_collection_identifier(value);
		let status = T::MemberService::ring_status(&identifier, 0).expect("ring exists");
		let immutable_since = status.immutable_since.expect("ring should be immutable") as u32;
		let expiration = T::RecyclerExpirationTime::get();
		T::BenchmarkHelper::set_time(core::time::Duration::from_secs(
			(immutable_since + expiration + 1) as u64,
		));

		let call = Call::<T>::clean_recycler { value };

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
		RecyclersDusting::<T>::insert((value, ring_index), ());

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

	/// Benchmark the condition check `on_poll` performs every block.
	///
	/// Mirrors the worst case of the gate: when `InitializePalletAccount` is
	/// unset, the `&&` does not short-circuit and both storage values are read.
	#[benchmark]
	fn on_poll_initialize_check_condition() -> Result<(), BenchmarkError> {
		assert!(!InitializePalletAccount::<T>::exists());

		#[block]
		{
			let _needs_init =
				!InitializePalletAccount::<T>::exists() && UnderlyingAssetId::<T>::exists();
		}

		Ok(())
	}

	/// Benchmark the one-time initialization: all recycler collections + pallet account.
	#[benchmark]
	fn on_poll_initialize() -> Result<(), BenchmarkError> {
		// Use a minimal setup without common_setup to avoid pre-creating collections.
		T::BenchmarkHelper::set_time(core::time::Duration::from_secs(3600));
		T::BenchmarkHelper::setup_assets();

		let recycler_exp = T::RecyclerRingExponent::get();
		T::MemberService::initialize_chunks(recycler_exp);

		assert!(InitializePalletAccount::<T>::get().is_none());

		#[block]
		{
			Pallet::<T>::do_initialize().expect("should initialize");
		}

		assert!(InitializePalletAccount::<T>::get().is_some());

		Ok(())
	}

	impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext_bench(), crate::mock::Test);
}
