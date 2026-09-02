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
	pallet::{AccountToAlias, AliasToAccount, BalanceOf, StaleSince},
	types::{AliasAccountInfo, ContextualAlias, ProofOf},
};
use alloc::vec::Vec;
use frame_benchmarking::{account, v2::*, BenchmarkError};
use frame_support::{
	dispatch::{DispatchInfo, PostDispatchInfo},
	pallet_prelude::{Authorize, BoundedVec, TransactionSource},
	traits::{fungibles, Get},
};
use frame_system::RawOrigin as SystemOrigin;
use indiv_support::traits::{
	Alias, Context, Identifier, RevisionIndex, RingIndex, PEOPLE_IDENTIFIER, PEOPLE_LITE_IDENTIFIER,
};
use sp_runtime::{traits::Dispatchable, Saturating};

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

	/// Make [`Config::AliasFee`] return `Some(fee)`.
	fn set_alias_fee(fee: BalanceOf<T>);

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

/// Build an `AliasAccountInfo` whose alias is `alias_seed`, so distinct seeds give distinct
/// aliases however many a batch needs.
fn make_alias_info<T: Config + BenchmarkHelper<T>>(
	collection: Identifier,
	ring: RingIndex,
	revision: RevisionIndex,
	alias_seed: u32,
) -> AliasAccountInfo {
	let mut alias: Alias = [0u8; 32];
	alias[..4].copy_from_slice(&alias_seed.to_le_bytes());
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

/// Second every seeded ring root is dated, and the [`StaleSince`] every stamped mapping carries.
///
/// The member service reads it as its newest root's time, so a benchmark holds the clock here to
/// keep a revision retained and moves it past [`Config::MappingRetention`] to expire one.
const RING_SOURCE_TIME: u64 = 1;

/// Seed a full `RingRoots` window and return the revision that drives the member service's
/// revision lookup to its worst-case path for the current config:
/// - `max_ring_revisions() >= 2` → a non-latest revision (full window scan).
/// - `max_ring_revisions() == 1` → the only revision.
///
/// Returns `(target_revision, source_time)`, where `source_time` is [`RING_SOURCE_TIME`], so the
/// caller must call `set_time(...)`:
/// - `set_time(source_time)` → target revision is still retained.
/// - `set_time(source_time + MappingRetention + 1)` → the revision has expired at the member
///   service, whose retention is below `MappingRetention`, and the mapping is eligible for cleanup.
fn setup_full_window_worst_case<T: Config + BenchmarkHelper<T>>(
	collection: Identifier,
	ring: RingIndex,
) -> (RevisionIndex, u64) {
	let max_roots = <T as BenchmarkHelper<T>>::max_ring_revisions();
	<T as BenchmarkHelper<T>>::seed_ring(collection, ring, max_roots, RING_SOURCE_TIME);
	// Latest root is `max_roots - 1`
	let target_revision = max_roots.saturating_sub(2);
	(target_revision, RING_SOURCE_TIME)
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
	/// The proof targets a *non-latest* revision so the member service's revision validity
	/// check and root lookup both scan the full window.
	#[benchmark]
	fn set_alias_account() -> Result<(), BenchmarkError> {
		let collection: Identifier = *PEOPLE_IDENTIFIER;
		let ring: RingIndex = 0;
		let context: Context = <T as BenchmarkHelper<T>>::allowed_context();

		let (target_revision, source_time) = setup_full_window_worst_case::<T>(collection, ring);
		// Clock is `source_time` so the revision is still retained.
		<T as BenchmarkHelper<T>>::set_time(source_time);

		// Fee equals one PGAS ED; fund the caller with a small multiple of ED so
		// the burn leaves the account comfortably above existential.
		<T as BenchmarkHelper<T>>::setup_pgas_asset();
		let pgas_ed: BalanceOf<T> =
			<T::Fungibles as fungibles::Inspect<T::AccountId>>::minimum_balance(
				T::PgasAssetId::get(),
			);
		<T as BenchmarkHelper<T>>::set_alias_fee(pgas_ed);
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

	/// Seeds `n` mappings in the state `action` applies to, each in a ring of its own carrying a
	/// full revision window, and returns them in the ascending order a sweep carries them.
	///
	/// A batch is one pass over [`AccountToAlias`], which is keyed by account, so its mappings
	/// span as many rings as it has accounts. One ring each is what makes every ring-root read a
	/// separate key, and those reads are most of the batch's proof size.
	///
	/// Every seeded root is dated [`RING_SOURCE_TIME`], and a stamped mapping carries that same
	/// second, so a caller that moves the clock past [`Config::MappingRetention`] from there has
	/// both an expired revision and an expired stamp.
	fn seed_sweep_batch<T: Config + BenchmarkHelper<T>>(
		collection: Identifier,
		n: u32,
		action: StaleAliasAction,
	) -> BoundedVec<T::AccountId, T::MaxStaleAliasBatch> {
		let max_roots = <T as BenchmarkHelper<T>>::max_ring_revisions();
		// The latest revision never expires, so a mapping that verifies again is stored under it.
		// The one behind it has expired once the clock moves past the member service's retention,
		// and is the entry a full-window scan reaches last but one.
		let revision = match action {
			StaleAliasAction::Report | StaleAliasAction::Retire => max_roots.saturating_sub(2),
			StaleAliasAction::ClearReport => max_roots.saturating_sub(1),
		};
		// `Report` is the state before the first stamp; the other two act on a stamped mapping.
		let stamped = matches!(action, StaleAliasAction::Retire | StaleAliasAction::ClearReport);

		let mut accounts = (0..n)
			.map(|i| {
				let ring = i as RingIndex;
				<T as BenchmarkHelper<T>>::seed_ring(collection, ring, max_roots, RING_SOURCE_TIME);
				let holder: T::AccountId = account("sweep_holder", i, 0);
				// One alias per holder: the alias keys the reverse mapping, so a shared one would
				// leave every holder but the last unreachable through it.
				let info = make_alias_info::<T>(collection, ring, revision, i);
				insert_mapping::<T>(&holder, &info);
				if stamped {
					StaleSince::<T>::insert(&holder, RING_SOURCE_TIME);
				}
				holder
			})
			.collect::<Vec<_>>();
		accounts.sort();
		BoundedVec::try_from(accounts).expect("n is bounded by MaxStaleAliasBatch")
	}

	/// Moves the clock past [`Config::MappingRetention`] counted from [`RING_SOURCE_TIME`], which
	/// expires both a seeded revision and a seeded stamp.
	fn set_time_past_retention<T: Config + BenchmarkHelper<T>>() {
		<T as BenchmarkHelper<T>>::set_time(
			RING_SOURCE_TIME.saturating_add(T::MappingRetention::get()).saturating_add(1),
		);
	}

	/// The reporting sweep: `n` mappings stored under a revision one behind the latest and expired
	/// at the member service, none stamped yet, so the call reads each ring's newest root to date
	/// it and writes the stamp.
	#[benchmark]
	fn report_stale_aliases(
		n: Linear<1, { T::MaxStaleAliasBatch::get() }>,
	) -> Result<(), BenchmarkError> {
		let collection: Identifier = *PEOPLE_IDENTIFIER;

		let accounts = seed_sweep_batch::<T>(collection, n, StaleAliasAction::Report);
		set_time_past_retention::<T>();

		#[extrinsic_call]
		_(SystemOrigin::Authorized, accounts.clone());

		for holder in &accounts {
			assert_eq!(StaleSince::<T>::get(holder), Some(RING_SOURCE_TIME));
			assert!(AccountToAlias::<T>::get(holder).is_some());
		}

		Ok(())
	}

	/// `authorize` for the reporting sweep, which reads every mapping in the batch and runs the
	/// member service's validity check over a full revision window for each.
	#[benchmark]
	fn authorize_report_stale_aliases(
		n: Linear<1, { T::MaxStaleAliasBatch::get() }>,
	) -> Result<(), BenchmarkError> {
		let collection: Identifier = *PEOPLE_IDENTIFIER;

		let accounts = seed_sweep_batch::<T>(collection, n, StaleAliasAction::Report);
		set_time_past_retention::<T>();
		let call = Call::<T>::report_stale_aliases { accounts };

		#[block]
		{
			call.authorize(TransactionSource::InBlock)
				.ok_or("Call must give some authorization")??;
		}

		Ok(())
	}

	/// The removal sweep: `n` stamped mappings whose retention has run out, so the call removes
	/// both directions of each and drops its sufficient reference.
	#[benchmark]
	fn retire_stale_aliases(
		n: Linear<1, { T::MaxStaleAliasBatch::get() }>,
	) -> Result<(), BenchmarkError> {
		let collection: Identifier = *PEOPLE_IDENTIFIER;

		let accounts = seed_sweep_batch::<T>(collection, n, StaleAliasAction::Retire);
		// Past the retention counted from the stamp every seeded mapping carries.
		set_time_past_retention::<T>();

		#[extrinsic_call]
		_(SystemOrigin::Authorized, accounts.clone());

		for holder in &accounts {
			assert_eq!(AccountToAlias::<T>::get(holder), None);
			assert_eq!(StaleSince::<T>::get(holder), None);
			// The mapping's sufficient reference was the only one, so dropping it reaps the
			// account. A retirement that leaves it behind has not paid for the removal.
			assert!(!frame_system::Account::<T>::contains_key(holder));
		}

		Ok(())
	}

	/// `authorize` for the removal sweep, which additionally compares each stamp against the
	/// retention.
	#[benchmark]
	fn authorize_retire_stale_aliases(
		n: Linear<1, { T::MaxStaleAliasBatch::get() }>,
	) -> Result<(), BenchmarkError> {
		let collection: Identifier = *PEOPLE_IDENTIFIER;

		let accounts = seed_sweep_batch::<T>(collection, n, StaleAliasAction::Retire);
		set_time_past_retention::<T>();
		let call = Call::<T>::retire_stale_aliases { accounts };

		#[block]
		{
			call.authorize(TransactionSource::InBlock)
				.ok_or("Call must give some authorization")??;
		}

		Ok(())
	}

	/// The clearing sweep: `n` stamped mappings whose revision verifies again, which a collection
	/// re-created under the same identifier produces. Each is stored under the newest revision, the
	/// last entry of a full window, so the validity check scans all of it.
	#[benchmark]
	fn clear_stale_alias_reports(
		n: Linear<1, { T::MaxStaleAliasBatch::get() }>,
	) -> Result<(), BenchmarkError> {
		let collection: Identifier = *PEOPLE_IDENTIFIER;

		let accounts = seed_sweep_batch::<T>(collection, n, StaleAliasAction::ClearReport);
		set_time_past_retention::<T>();

		#[extrinsic_call]
		_(SystemOrigin::Authorized, accounts.clone());

		for holder in &accounts {
			assert_eq!(StaleSince::<T>::get(holder), None);
			assert!(AccountToAlias::<T>::get(holder).is_some());
		}

		Ok(())
	}

	/// `authorize` for the clearing sweep, whose validity check is the one that scans a full
	/// revision window and finds the revision.
	#[benchmark]
	fn authorize_clear_stale_alias_reports(
		n: Linear<1, { T::MaxStaleAliasBatch::get() }>,
	) -> Result<(), BenchmarkError> {
		let collection: Identifier = *PEOPLE_IDENTIFIER;

		let accounts = seed_sweep_batch::<T>(collection, n, StaleAliasAction::ClearReport);
		set_time_past_retention::<T>();
		let call = Call::<T>::clear_stale_alias_reports { accounts };

		#[block]
		{
			call.authorize(TransactionSource::InBlock)
				.ok_or("Call must give some authorization")??;
		}

		Ok(())
	}

	#[benchmark]
	fn personhood_info() -> Result<(), BenchmarkError> {
		let collection: Identifier = *PEOPLE_IDENTIFIER;
		let ring: RingIndex = 0;

		let (stored_revision, source_time) = setup_full_window_worst_case::<T>(collection, ring);
		// Clock is `source_time` so the revision is still retained and the lookup returns `Some`.
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

	/// The proof targets a *non-latest* revision so the member service scans the full window,
	/// matching the worst case in [`set_alias_account`].
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

	impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
