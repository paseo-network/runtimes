// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Drives the coinage migration to completion against a real People snapshot.
//!
//! # Why this exists
//!
//! Units B and C are multi-block migrations, and **nothing else can execute them**:
//!
//! - `try-runtime on-runtime-upgrade` runs single-block migrations only. It verifies unit A and
//!   then reports the ~6,575 still-undecodable entries that B and C exist to fix.
//! - **chopsticks cannot run People at all**: `Unresolved function
//!   env:ext_statement_store_remove_by_version_1`.
//! - **zombie-bite cannot either**: People warp-syncs, then doppelganger's state-override path
//!   fails to instantiate the runtime with the same missing host function, never writes `head.txt`,
//!   and the bite dies. `--enable-statement-store` does not help — host-function registration is a
//!   compile-time type parameter on the executor, not a node flag. The gap is inside Parity's
//!   `jv-doppelganger-node` SDK branch.
//!
//! So this steps the migrations directly, which is also deterministic and reproducible in a way a
//! forked network is not.
//!
//! # Running it
//!
//! Needs a `try-runtime create-snapshot` of People and the `try-runtime` feature (the
//! `pre_upgrade`/`post_upgrade` hooks are gated on it):
//!
//! ```text
//! try-runtime create-snapshot --uri wss://people-paseo.rotko.net \
//!   -p System -p ParachainSystem -p Timestamp -p Balances -p Assets -p AssetsHolder \
//!   -p Coinage -p Members -p MembersNotifier -p Score -p OriginRestriction -p ChunksManager \
//!   -p PeopleLite -p People -p NetworkSuffix -p Airdrop -p Resources -p Honour \
//!   people.snap
//!
//! PEOPLE_SNAPSHOT=/abs/path/people.snap \
//!   cargo test -p people-paseo-runtime --features try-runtime --test coinage_migration -- --nocapture
//! ```
//!
//! Without `PEOPLE_SNAPSHOT` the test **skips** rather than fails, so CI stays green on a machine
//! that has no snapshot. A skip is logged loudly; do not mistake it for a pass.

#![cfg(feature = "try-runtime")]

use frame_remote_externalities::{Builder, Mode, OfflineConfig, SnapshotConfig};
use frame_support::{
	traits::OnRuntimeUpgrade,
	weights::{Weight, WeightMeter},
};
use sp_runtime::AccountId32 as AccountId;
extern crate alloc;

use people_paseo_runtime::{
	migrations::coinage::{
		old, MigrateCoinageToInstances, RelocateCoinageRecyclerCollections,
		SeedCoinageInstanceZero, LEGACY_INSTANCE_ID,
	},
	Runtime,
};

/// The MBM service budget the live runtime grants per block: 80% of `max_block`. Stepping with
/// exactly this is what makes the block count below meaningful rather than arbitrary.
fn mbm_budget_per_block() -> Weight {
	sp_runtime::Perbill::from_percent(80) *
		<Runtime as frame_system::Config>::BlockWeights::get().max_block
}

/// A hard ceiling on simulated blocks, so a cursor that fails to advance fails the test instead of
/// hanging it.
const MAX_SIMULATED_BLOCKS: u32 = 500;

/// Step one `SteppedMigration` to completion, one simulated block at a time, and report how many
/// blocks it took.
macro_rules! drive_to_completion {
	($m:ty, $label:expr) => {{
		use frame_support::migrations::SteppedMigration;
		let mut cursor = None;
		let mut blocks = 0u32;
		loop {
			assert!(
				blocks < MAX_SIMULATED_BLOCKS,
				"{} did not finish within {} blocks — the cursor is probably not advancing",
				$label,
				MAX_SIMULATED_BLOCKS,
			);
			let mut meter = WeightMeter::with_limit(mbm_budget_per_block());
			cursor = <$m as SteppedMigration>::step(cursor, &mut meter)
				.unwrap_or_else(|e| panic!("{} step failed: {:?}", $label, e));
			blocks += 1;
			if cursor.is_none() {
				break;
			}
		}
		println!("  {} completed in {} simulated block(s)", $label, blocks);
		blocks
	}};
}

/// Baseline identifier: `b"coinage/recycler" ++ [denomination] ++ [0u8; 15]`.
fn legacy_recycler_identifier_for_test(denomination: i8) -> [u8; 32] {
	let mut id = [0u8; 32];
	id[0..16].copy_from_slice(&indiv_pallet_coinage::RECYCLER_COLLECTION_PREFIX);
	id[16] = denomination as u8;
	id
}

/// v0.3.1 identifier: instance id at `[16..20]`, denomination at `[20]`.
fn instanced_recycler_identifier_for_test(denomination: i8) -> [u8; 32] {
	let mut id = [0u8; 32];
	id[0..16].copy_from_slice(&indiv_pallet_coinage::RECYCLER_COLLECTION_PREFIX);
	id[16..20].copy_from_slice(&LEGACY_INSTANCE_ID.to_le_bytes());
	id[20] = denomination as u8;
	id
}

#[tokio::test(flavor = "multi_thread")]
async fn coinage_migration_completes_against_live_state() {
	sp_tracing::try_init_simple();

	let Some(path) = std::env::var("PEOPLE_SNAPSHOT").ok().filter(|p| !p.is_empty()) else {
		println!(
			"SKIPPED: set PEOPLE_SNAPSHOT to a `try-runtime create-snapshot` of People. \
			 This is a SKIP, not a pass — units B and C were not executed."
		);
		return;
	};

	let mut ext = Builder::<people_paseo_runtime::Block>::default()
		.mode(Mode::Offline(OfflineConfig { state_snapshot: SnapshotConfig::new(path) }))
		.build()
		.await
		.expect("could not load the snapshot");

	ext.execute_with(|| {
		use indiv_pallet_coinage::{CoinsByOwner, Instances, LockedCoins};

		// ---- before ------------------------------------------------------------------------
		// Every figure comes from the snapshot. Nothing is compared against a documented count:
		// the design's `RecyclersCoinToRecycler` number was already stale by 24 when re-read.
		// Read through the LEGACY aliases: the live keys are `Twox64Concat`-hashed, so reading
		// them through the new `Blake2_128Concat` types logs a decode failure per key and yields
		// nothing. (That mistake is what the first run of this test made.)
		// Counted through the migration's own shape-based counter for the three maps whose
		// destination shares their prefix; the typed alias would over-count by reading the
		// destination keys back as legacy ones.
		let coins_before = old::CoinsByOwner::<Runtime>::iter_keys().count();
		let locked_before = old::LockedCoins::<Runtime>::iter_keys().count();
		let unloaded_before = old::RecyclersUnloaded::<Runtime>::iter_keys().count();
		let c2r_before = old::RecyclersCoinToRecycler::<Runtime>::iter_keys().count();
		// Pre-migration there are no destination keys yet, so the typed counts above are exact.
		// Asserted so a re-run against an already-migrated snapshot cannot silently pass.
		println!("\n=== snapshot state before (legacy shapes) ===");
		println!("  Coinage::CoinsByOwner             : {coins_before}");
		println!("  Coinage::LockedCoins              : {locked_before}");
		println!("  Coinage::RecyclersCoinToRecycler  : {c2r_before}");
		println!("  Coinage::RecyclersUnloaded        : {unloaded_before}  (anti-replay set)");
		assert!(
			coins_before > 0,
			"the snapshot holds no coinage state — is `-p Coinage` in the scrape?"
		);
		assert!(
			Instances::<Runtime>::iter().next().is_none(),
			"the snapshot is already migrated; take a fresh one from a pre-upgrade block",
		);

		// 🔴 The actual question this test exists to answer: does every holder keep their coin,
		// unchanged? A count alone would pass a migration that shuffled coins between accounts or
		// zeroed every value, so capture the full (account -> value, age) map and compare it
		// entry by entry afterwards.
		let coins_map_before: alloc::collections::BTreeMap<AccountId, (i8, u16)> =
			old::CoinsByOwner::<Runtime>::iter()
				.map(|(who, c)| (who, (c.value, c.age)))
				.collect();
		assert_eq!(coins_map_before.len(), coins_before, "duplicate account in the legacy map");

		let locks_before: alloc::collections::BTreeMap<AccountId, alloc::vec::Vec<u8>> =
			old::LockedCoins::<Runtime>::iter_keys()
				.map(|who| {
					let raw = old::LockedCoins::<Runtime>::hashed_key_for(&who);
					(who, sp_io::storage::get(&raw).map(|v| v.to_vec()).unwrap_or_default())
				})
				.collect();

		// Recycler ring membership is the other half of a holder's position: a coin is unloaded
		// from a ring, so a stranded ring is as bad as a lost balance.
		let members_before = indiv_pallet_members::Members::<Runtime>::iter().count();
		let c2r_map_before: alloc::collections::BTreeMap<_, _> =
			old::RecyclersCoinToRecycler::<Runtime>::iter().collect();

		// ---- unit A ------------------------------------------------------------------------
		let a_state = SeedCoinageInstanceZero::pre_upgrade().expect("unit A pre_upgrade");
		SeedCoinageInstanceZero::on_runtime_upgrade();
		SeedCoinageInstanceZero::post_upgrade(a_state).expect("unit A post_upgrade");
		println!("\n=== unit A: instance seeded ===");
		assert!(Instances::<Runtime>::contains_key(LEGACY_INSTANCE_ID));

		// ---- unit B ------------------------------------------------------------------------
		// This is the first time unit B has ever executed.
		let b_state = MigrateCoinageToInstances::pre_upgrade_state().expect("unit B pre_upgrade");
		println!("\n=== unit B: stepping ===");
		let b_blocks = drive_to_completion!(MigrateCoinageToInstances, "unit B");
		MigrateCoinageToInstances::post_upgrade_state(b_state).expect("unit B post_upgrade");

		// ---- unit C ------------------------------------------------------------------------
		println!("\n=== unit C: stepping ===");
		let c_blocks = drive_to_completion!(RelocateCoinageRecyclerCollections, "unit C");

		// ---- after -------------------------------------------------------------------------
		// The migration must conserve balances: every coin that existed still exists, now
		// readable under the new hasher and the widened `Coin`.
		let coins_after = CoinsByOwner::<Runtime>::iter().count();
		println!("\n=== state after ===");
		println!("  Coinage::CoinsByOwner (new hasher)    : {coins_after}");
		assert_eq!(
			coins_after, coins_before,
			"coin count changed across the migration — balances were lost or duplicated",
		);
		// 🔴 The anti-replay rebuild: every previously-consumed alias must now read `Unloaded`.
		// One that reads `None` is spendable a second time.
		let states_after = indiv_pallet_coinage::RecyclerAliasStates::<Runtime>::iter().count();
		println!("  Coinage::RecyclerAliasStates         : {states_after}");
		assert_eq!(
			states_after, unloaded_before,
			"the anti-replay set was not fully rebuilt — a consumed alias is spendable again",
		);
		let c2r_after = indiv_pallet_coinage::RecyclersCoinToRecycler::<Runtime>::iter().count();
		assert_eq!(c2r_after, c2r_before, "RecyclersCoinToRecycler lost or gained entries");
		// 🔴 Value and ownership preservation, entry by entry.
		let coins_map_after: alloc::collections::BTreeMap<AccountId, (i8, u16)> =
			CoinsByOwner::<Runtime>::iter()
				.map(|(who, c)| {
					assert_eq!(
						c.instance_id, LEGACY_INSTANCE_ID,
						"a migrated coin is not in the seeded instance, so it is unspendable",
					);
					(who, (c.value, c.age))
				})
				.collect();
		assert_eq!(
			coins_map_after, coins_map_before,
			"a holder's coin changed across the migration — value, age or ownership moved",
		);

		// Locks: value bytes are copied verbatim, so they must be byte-identical.
		for (who, before) in &locks_before {
			let raw = indiv_pallet_coinage::LockedCoins::<Runtime>::hashed_key_for(who);
			let after = sp_io::storage::get(&raw).map(|v| v.to_vec()).unwrap_or_default();
			assert_eq!(after, *before, "a coin lock changed across the migration");
		}

		// Recycler mapping: same members, same denomination, now instance-qualified.
		let c2r_map_after: alloc::collections::BTreeMap<_, _> =
			indiv_pallet_coinage::RecyclersCoinToRecycler::<Runtime>::iter().collect();
		assert_eq!(
			c2r_map_after.len(),
			c2r_map_before.len(),
			"RecyclersCoinToRecycler lost or gained members",
		);
		for (member, denomination) in &c2r_map_before {
			assert_eq!(
				c2r_map_after.get(member),
				Some(&(LEGACY_INSTANCE_ID, *denomination)),
				"a recycler member lost its ring or changed denomination",
			);
		}

		// 🔴 Unit C: the ring membership must have MOVED, not been dropped. Same total rows, and
		// nothing left under a legacy identifier.
		let members_after = indiv_pallet_members::Members::<Runtime>::iter().count();
		assert_eq!(
			members_after, members_before,
			"pallet-members rows were lost or duplicated by the relocation",
		);
		for denomination in
			indiv_pallet_coinage::RecyclerCollectionCreated::<Runtime>::iter_key_prefix(
				LEGACY_INSTANCE_ID,
			) {
			let legacy = legacy_recycler_identifier_for_test(denomination);
			let instanced = instanced_recycler_identifier_for_test(denomination);
			// Denomination 0's two identifiers are byte-identical, so it is already at its
			// destination and must NOT be expected to have vacated the legacy address.
			if legacy != instanced {
				assert!(
					indiv_pallet_members::Collections::<Runtime>::get(legacy).is_none(),
					"a recycler collection is still at its legacy identifier — the ring is \
					 stranded",
				);
			}
			assert!(
				indiv_pallet_members::Collections::<Runtime>::get(instanced).is_some(),
				"a recycler collection did not arrive at its instanced identifier",
			);
		}
		// `LockedCoins` values are copied verbatim, so they must all still decode.
		let locked_after = LockedCoins::<Runtime>::iter().count();
		println!("  Coinage::LockedCoins                 : {locked_after}");
		assert_eq!(locked_after, locked_before, "LockedCoins lost or gained entries");

		println!(
			"\n=== weight budgeting ===\n  \
			 unit B took {b_blocks} block(s), unit C took {c_blocks} block(s) at the runtime's \
			 real MBM budget (80% of max_block)."
		);
		assert!(
			b_blocks + c_blocks < MAX_SIMULATED_BLOCKS,
			"the migration needs an implausible number of blocks",
		);
	});
}
