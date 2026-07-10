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

//! Alias Accounts pallet benchmarks.

use super::*;
use crate::{
	extension::{AsRingAlias, AsRingAliasInfo},
	pallet::{AccountToAlias, AliasFee, AliasToAccount, BalanceOf},
	types::{AliasAccountInfo, ContextualAlias, ProofOf},
};
use frame_benchmarking::{account, v2::*, BenchmarkError};
use frame_support::{
	dispatch::{DispatchInfo, PostDispatchInfo},
	traits::{fungibles, EnsureOrigin, Get},
};
use frame_system::RawOrigin as SystemOrigin;
use indiv_support::traits::{
	Alias, Context, Identifier, RevisionIndex, RingIndex, PEOPLE_IDENTIFIER, PEOPLE_LITE_IDENTIFIER,
};
use sp_runtime::{
	traits::{DispatchTransaction, Dispatchable},
	Saturating,
};

// ============================================================================
// Local BenchmarkHelper
// ============================================================================

pub trait BenchmarkHelper<T: Config> {
	/// Advance the runtime `UnixTime` source to `seconds` since epoch.
	///
	/// Must be called with `seconds > 0` before any code path that reads
	/// `UnixTime::now()`; otherwise the runtime logs a "called at genesis" error.
	fn set_time(seconds: u64);

	/// A `Context` to use for proof generation in benchmarks.
	fn allowed_context() -> Context;

	/// Returns `(proof, alias)` such that proof verification through
	/// [`Config::MemberService`] succeeds against a ring seeded via
	/// [`Self::seed_ring`] for the given `context` and `msg`.
	fn mock_proof(seed: u32, context: Context, msg: &[u8]) -> (ProofOf<T>, Alias);

	/// Return a ring-VRF proof that verifies via [`Config::MemberService`] for the
	/// tuple `(identifier, ring_index, revision, context, message)`.
	///
	/// **Precondition:** [`Self::seed_ring`] must have set up the ring window at
	/// `(identifier, ring_index)`, and `revision` must be one of the revisions it
	/// inserted.
	fn create_proof_for_revision(
		identifier: &Identifier,
		ring_index: RingIndex,
		revision: RevisionIndex,
		context: &Context,
		message: &[u8],
	) -> ProofOf<T>;

	/// Ensure the PGAS asset exists. Idempotent — must be a no-op when the asset
	/// already exists.
	fn setup_pgas_asset();

	/// Maximum number of revisions that [`Config::MemberService`] retains per ring.
	/// Used as the worst-case `Linear` upper bound in benchmarks.
	fn max_ring_revisions() -> u32;

	/// Populate [`Config::MemberService`] with `revisions` records (numbered `0..=revisions`)
	/// at `(collection, ring)`, all sharing `source_time`. Also seeds whatever per-collection
	/// metadata (e.g. ring exponent) the underlying service requires for proof verification.
	fn seed_ring(collection: Identifier, ring: RingIndex, revisions: u32, source_time: u64);
}

// ============================================================================
// Setup helpers
// ============================================================================

/// Build an `AliasAccountInfo` with arbitrary alias bytes.
fn make_alias_info<T: Config + BenchmarkHelper<T>>(
	collection: Identifier,
	ring: RingIndex,
	revision: RevisionIndex,
	alias_seed: u8,
) -> AliasAccountInfo {
	let alias: Alias = [alias_seed; 32];
	let context = <T as BenchmarkHelper<T>>::allowed_context();
	AliasAccountInfo { collection, revision, ring, ca: ContextualAlias { alias, context } }
}

/// Pre-populate a full alias <-> account mapping so the "replace existing" / "clean up stale"
/// branches can run without going through the extension.
fn insert_mapping<T: Config>(account: &T::AccountId, info: &AliasAccountInfo) {
	AccountToAlias::<T>::insert(account, info);
	AliasToAccount::<T>::insert(info.collection, &info.ca, account);
	frame_system::Pallet::<T>::inc_sufficients(account);
}

/// Seed a full `RingRoots` window and return the revision that drives
/// `is_revision_in_grace` to its worst-case path for the current config:
/// - `max_ring_revisions() >= 2` → a non-latest revision (slow path).
/// - `max_ring_revisions() == 1` → the only revision (slow path is unreachable)
///
/// Returns `(target_revision, source_time)`. The caller must call `set_time(...)`:
/// - `set_time(source_time)` → target revision is in grace.
/// - `set_time(source_time + CleanupGracePeriod + 1)` → target revision is past grace.
fn setup_full_window_worst_case<T: Config + BenchmarkHelper<T>>(
	collection: Identifier,
	ring: RingIndex,
) -> (RevisionIndex, u64) {
	let max_roots = <T as BenchmarkHelper<T>>::max_ring_revisions();
	let source_time: u64 = 1;
	<T as BenchmarkHelper<T>>::seed_ring(collection, ring, max_roots, source_time);
	// Latest root is `max_roots - 1`
	let target_revision = max_roots.saturating_sub(2);
	(target_revision, source_time)
}

// ============================================================================
// Benchmarks
// ============================================================================

#[benchmarks(where
	T: BenchmarkHelper<T> + Send + Sync,
	<T as frame_system::Config>::RuntimeCall:
		Dispatchable<Info = DispatchInfo, PostInfo = PostDispatchInfo> + From<Call<T>>,
)]
mod benches {
	use super::*;

	/// Worst case: replace an existing alias with a different account, with a fee burn.
	///
	/// The proof targets a *non-latest* revision so `verify_proof` traverses the slow
	/// path through `is_revision_in_grace` (i.e. `revision_source_time` + a `Timestamp::Now`
	/// read + the grace-window arithmetic).
	#[benchmark]
	fn set_alias_account() -> Result<(), BenchmarkError> {
		let collection: Identifier = *PEOPLE_IDENTIFIER;
		let ring: RingIndex = 0;
		let context: Context = <T as BenchmarkHelper<T>>::allowed_context();

		let (target_revision, source_time) = setup_full_window_worst_case::<T>(collection, ring);
		// Clock is `source_time` so the revision is in grace.
		<T as BenchmarkHelper<T>>::set_time(source_time);

		// Fee equals one PGAS ED; fund the caller with a small multiple of ED so
		// the burn leaves the account comfortably above existential.
		<T as BenchmarkHelper<T>>::setup_pgas_asset();
		let pgas_ed: BalanceOf<T> =
			<T::Fungibles as fungibles::Inspect<T::AccountId>>::minimum_balance(
				T::PgasAssetId::get(),
			);
		let fee: BalanceOf<T> = pgas_ed;
		AliasFee::<T>::put(fee);
		let new_account: T::AccountId = account("paid_new", 0, 0);
		<T::Fungibles as fungibles::Mutate<T::AccountId>>::mint_into(
			T::PgasAssetId::get(),
			&new_account,
			pgas_ed.saturating_mul(10u32.into()),
		)
		.map_err(|_| BenchmarkError::Stop("PGAS mint should succeed"))?;

		let proof_valid_at = source_time;
		let msg = Pallet::<T>::proof_message(&new_account, proof_valid_at);
		let proof = <T as BenchmarkHelper<T>>::create_proof_for_revision(
			&collection,
			ring,
			target_revision,
			&context,
			&msg,
		);
		let validated =
			Pallet::<T>::verify_proof(&proof, &collection, ring, target_revision, &context, &msg)
				.map_err(|_| BenchmarkError::Stop("helper must produce a valid proof"))?;
		let alias = validated.ca.alias;

		// Pre-existing mapping for the old account so the dispatch hits the
		// worst-case "swap" branch.
		let old_account: T::AccountId = account("paid_old", 0, 0);
		let existing_info = AliasAccountInfo {
			collection,
			revision: target_revision,
			ring,
			ca: ContextualAlias { alias, context },
		};
		insert_mapping::<T>(&old_account, &existing_info);

		#[extrinsic_call]
		_(
			SystemOrigin::Signed(new_account.clone()),
			proof,
			collection,
			ring,
			target_revision,
			context,
			proof_valid_at,
		);

		assert_eq!(AccountToAlias::<T>::get(&old_account), None);
		let info = AccountToAlias::<T>::get(&new_account).expect("new mapping exists");
		assert_eq!(info.ca.alias, alias);
		assert_eq!(info.revision, target_revision);
		assert_eq!(
			AliasToAccount::<T>::get(collection, &ContextualAlias { alias, context }),
			Some(new_account.clone()),
		);

		frame_system::Pallet::<T>::assert_last_event(
			Event::<T>::AliasAccountSet { account: new_account, collection, alias }.into(),
		);

		Ok(())
	}

	/// Single path: take AccountToAlias + remove AliasToAccount + dec_sufficients.
	#[benchmark]
	fn unset_alias_account() -> Result<(), BenchmarkError> {
		let collection: Identifier = *PEOPLE_IDENTIFIER;
		let alias_info = make_alias_info::<T>(collection, 0, 1, 2);
		let caller_account: T::AccountId = account("alias_holder", 0, 0);

		insert_mapping::<T>(&caller_account, &alias_info);

		#[extrinsic_call]
		_(SystemOrigin::Signed(caller_account.clone()));

		assert_eq!(AliasToAccount::<T>::get(collection, &alias_info.ca), None);
		assert_eq!(AccountToAlias::<T>::get(&caller_account), None);

		frame_system::Pallet::<T>::assert_last_event(
			Event::<T>::AliasAccountUnset { account: caller_account }.into(),
		);

		Ok(())
	}

	/// The alias is stored under a revision one behind the latest. `find_valid_record` locates it
	/// but flags it stale, which triggers a `UnixTime::now()` read and a grace-period check — more
	/// work than the single extra iteration a hash miss would have cost.
	///
	/// `r` sweeps how many records live in `RingRoots`. A ring with just one
	/// record has no stale path to take (the only record is always the latest),
	/// so at `r = 1` we fall through to the cheap early-exit.
	#[benchmark]
	fn clean_up_stale_alias() -> Result<(), BenchmarkError> {
		let collection: Identifier = *PEOPLE_IDENTIFIER;
		let ring: RingIndex = 0;

		let (stored_revision, source_time) = setup_full_window_worst_case::<T>(collection, ring);
		// Advance the clock past the grace window so the alias is flagged as stale and cleanup is
		// allowed.
		<T as BenchmarkHelper<T>>::set_time(
			source_time.saturating_add(T::CleanupGracePeriod::get()).saturating_add(1),
		);

		let alias_info = make_alias_info::<T>(collection, ring, stored_revision, 3);
		let holder: T::AccountId = account("stale_holder", 0, 0);
		insert_mapping::<T>(&holder, &alias_info);

		let caller: T::AccountId = account("cleaner", 0, 0);

		#[extrinsic_call]
		_(SystemOrigin::Signed(caller), collection, alias_info.ca.clone());

		assert_eq!(AliasToAccount::<T>::get(collection, &alias_info.ca), None);
		assert_eq!(AccountToAlias::<T>::get(&holder), None);

		frame_system::Pallet::<T>::assert_last_event(
			Event::<T>::StaleAliasCleanedUp {
				account: holder,
				collection,
				alias: alias_info.ca.alias,
			}
			.into(),
		);

		Ok(())
	}

	#[benchmark]
	fn personhood_info() -> Result<(), BenchmarkError> {
		let collection: Identifier = *PEOPLE_IDENTIFIER;
		let ring: RingIndex = 0;

		let (stored_revision, source_time) = setup_full_window_worst_case::<T>(collection, ring);
		// Clock is `source_time` so the revision is still in grace and the lookup returns `Some`.
		<T as BenchmarkHelper<T>>::set_time(source_time);

		let alias_info = make_alias_info::<T>(collection, ring, stored_revision, 4);
		let holder: T::AccountId = account("personhood_holder", 0, 0);
		insert_mapping::<T>(&holder, &alias_info);

		let context = alias_info.ca.context;

		#[block]
		{
			<Pallet<T> as indiv_support::traits::PersonhoodLookup<
				T::AccountId,
				ProofOf<T>,
			>>::personhood_info(&holder, &context)
				.0
				.ok_or("personhood lookup must hit worst-case path")?;
		}

		Ok(())
	}

	#[benchmark]
	fn personhood_info_by_proof() -> Result<(), BenchmarkError> {
		// Worst-case: largest supported ring exponent on a successful match.
		let identifier: Identifier = *PEOPLE_LITE_IDENTIFIER;
		let ring: RingIndex = 0;
		let revision: RevisionIndex = 0;
		let context = <T as BenchmarkHelper<T>>::allowed_context();

		<T as BenchmarkHelper<T>>::seed_ring(identifier, ring, 1, 0);
		<T as BenchmarkHelper<T>>::set_time(1);

		let msg: &[u8] = b"bench";
		let (proof, alias) = <T as BenchmarkHelper<T>>::mock_proof(0, context, msg);
		let request = indiv_support::traits::PersonhoodProofRequest {
			identifier,
			proof,
			alias,
			ring_index: ring,
			context,
			revision,
			message: msg,
		};

		#[block]
		{
			let (matched, _) = <Pallet<T> as indiv_support::traits::PersonhoodLookup<
				T::AccountId,
				ProofOf<T>,
			>>::personhood_info_by_proof(request);
			if !matched {
				return Err("personhood lookup must hit worst-case path".into());
			}
		}

		Ok(())
	}

	/// The proof targets a *non-latest* revision so `verify_proof` exercises the slow
	/// path through `is_revision_in_grace`, matching the worst case in
	/// [`set_alias_account`].
	#[benchmark]
	fn reprove_alias_account() -> Result<(), BenchmarkError> {
		let collection: Identifier = *PEOPLE_IDENTIFIER;
		let ring: RingIndex = 0;
		let context: Context = <T as BenchmarkHelper<T>>::allowed_context();

		let (new_revision, source_time) = setup_full_window_worst_case::<T>(collection, ring);
		<T as BenchmarkHelper<T>>::set_time(source_time);
		let old_revision: RevisionIndex = new_revision.saturating_sub(1);

		let holder: T::AccountId = account("reprove_holder", 0, 0);
		let proof_valid_at = source_time;
		let msg = Pallet::<T>::proof_message(&holder, source_time);
		let proof = <T as BenchmarkHelper<T>>::create_proof_for_revision(
			&collection,
			ring,
			new_revision,
			&context,
			&msg,
		);
		let validated =
			Pallet::<T>::verify_proof(&proof, &collection, ring, new_revision, &context, &msg)
				.map_err(|_| BenchmarkError::Stop("helper must produce a valid proof"))?;
		let alias = validated.ca.alias;

		let stored = AliasAccountInfo {
			collection,
			revision: old_revision,
			ring,
			ca: ContextualAlias { alias, context },
		};
		insert_mapping::<T>(&holder, &stored);

		#[extrinsic_call]
		_(SystemOrigin::Signed(holder.clone()), proof, ring, new_revision, proof_valid_at);

		let info = AccountToAlias::<T>::get(&holder).expect("mapping retained");
		assert_eq!(info.revision, new_revision);
		assert_eq!(info.ring, ring);

		frame_system::Pallet::<T>::assert_last_event(
			Event::<T>::AliasAccountSet { account: holder, collection, alias }.into(),
		);

		Ok(())
	}

	#[benchmark]
	fn set_alias_fee() -> Result<(), BenchmarkError> {
		let origin =
			T::FeeManagerOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;
		let fee: BalanceOf<T> = 42u32.into();

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, fee);

		assert_eq!(AliasFee::<T>::get(), Some(fee));

		frame_system::Pallet::<T>::assert_last_event(Event::<T>::AliasFeeSet { fee }.into());

		Ok(())
	}

	// ==================== Transaction extension benchmarks ====================

	#[benchmark]
	fn as_ring_alias_info_with_account() -> Result<(), BenchmarkError> {
		let collection: Identifier = *PEOPLE_IDENTIFIER;
		let ring: RingIndex = 0;

		let (stored_revision, source_time) = setup_full_window_worst_case::<T>(collection, ring);
		// Clock is `source_time` so the revision is still in grace and the
		// extension's validate succeeds.
		<T as BenchmarkHelper<T>>::set_time(source_time);

		let alias_info = make_alias_info::<T>(collection, ring, stored_revision, 5);
		let holder: T::AccountId = account("with_account_holder", 0, 0);
		insert_mapping::<T>(&holder, &alias_info);

		let nonce: T::Nonce = Default::default();
		let tx_ext = AsRingAlias::<T>::new(Some(AsRingAliasInfo::WithAccount(nonce)));

		let call: <T as frame_system::Config>::RuntimeCall =
			frame_system::Call::<T>::remark { remark: alloc::vec![] }.into();

		let origin: <T as frame_system::Config>::RuntimeOrigin =
			frame_system::RawOrigin::Signed(holder).into();

		#[block]
		{
			tx_ext
				.test_run(origin, &call, &Default::default(), 0, 0, |_| Ok(Default::default()))
				.expect("validate must succeed for the WithAccount path")
				.map_err(|_| BenchmarkError::Stop("inner call failed"))?;
		}

		Ok(())
	}

	impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
