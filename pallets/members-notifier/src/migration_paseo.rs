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

//! PASEO-LOCAL storage migration for `indiv-pallet-members-notifier`.
//!
//! Kept in its own file, beside upstream's `migration.rs` rather than inside it, so that a
//! reviewer can diff `migration.rs` against v0.3.1 and see it is untouched.
//!
//! # Upstream ships a migration for this pallet, and it is NOT sufficient
//!
//! v0.3.1 adds two storage maps. `next-people-paseo`'s `Migrations` tuple carries
//! [`SeedSubscriptionWhitelist`](crate::migration::SeedSubscriptionWhitelist), which seeds
//! `SubscriptionWhitelist`. **Nothing anywhere in v0.3.1 writes `SubscribedCollections`.**
//!
//! That is correct for upstream and wrong for Paseo, and the reason is structural.
//! `next-people-paseo` is launched from a genesis preset with NO subscribers: the whitelist is
//! the activation path, `do_subscribe` populates `SubscribedCollections` as a side effect, and an
//! empty map at genesis is therefore accurate. `people-paseo` is a live chain that already has a
//! subscriber. Read read-only at People block 6,443,886 / spec 2004003:
//!
//! ```text
//! MembersNotifier::Subscribers            1 key -- ParaId(1000) = AssetHub
//!     collections: "pop:polkadot.network/people     " (ring exponent 9)
//!                  "pop:polkadot.network/people-lite" (ring exponent 9)
//!     pallet_index: 97   (= MembersSubscriber in asset-hub-paseo's construct_runtime!)
//! MembersNotifier::SubscribedCollections  0 keys
//! MembersNotifier::SubscriptionWhitelist  0 keys
//! MembersNotifier::SealedBatchSequence    1467
//! ```
//!
//! # What happens if this migration is missing
//!
//! v0.3.1 opens `OnRingRootChange::on_ring_root_change` with
//!
//! ```ignore
//! // Not recording changes that no subscriber asked for.
//! if !SubscribedCollections::<T>::contains_key(identifier) {
//!     return;
//! }
//! ```
//!
//! and `enqueue_updates` drops any collection absent from the same map, then takes the
//! "no subscribers" branch and drains the send buffer **without sealing a batch**.
//!
//! So with the map empty: every ring-root change on People is silently discarded. AssetHub keeps
//! its subscription, keeps its last-known roots, and simply stops receiving updates —
//! permanently, with no error, no event and no log line. Every ring proof minted against a newer
//! People member set then fails to verify on AssetHub. The map is only ever written by
//! `rebuild_subscribed_collections()`, which runs on `do_subscribe` and `unsubscribe`, so the
//! outage lasts until somebody notices and re-subscribes AssetHub through governance.
//!
//! `ring_root_changes_are_dropped_without_the_seed` in this file's tests reproduces exactly that.
//!
//! # `try_state` catches it — and this migration is written so that check means something
//!
//! v0.3.1 adds `do_try_state()`, which asserts `SubscribedCollections` is EXACTLY the union of
//! every subscriber's collections, in both directions. A `try-runtime` run with `--checks all`
//! therefore fails loudly on an unmigrated chain. Two consequences shape this file:
//!
//! 1. The migration seeds the map by calling `Pallet::rebuild_subscribed_collections()` — the
//!    same function the runtime itself uses — rather than reimplementing the rule. A second
//!    implementation could drift from the invariant `do_try_state` enforces.
//! 2. `post_upgrade` calls `do_try_state()` itself, so the check runs even when someone forgets
//!    `--checks all`, and so a failure is attributed to this migration rather than to whatever
//!    ran after it.
//!
//! # 🔴 OPEN QUESTION FOR A HUMAN — deliberately not decided here
//!
//! Should `people-paseo` ALSO run upstream's `SeedSubscriptionWhitelist`?
//!
//! * **Running it** matches upstream's `Migrations` tuple exactly, but on Paseo it writes a
//!   whitelist entry for para 1000 which is **already subscribed**. `subscribe_whitelisted`
//!   rejects an already-subscribed parachain with `CustomInvalidity::AlreadySubscribed`, so the
//!   entry can only ever be consumed after an `unsubscribe`. It is harmless but it is state
//!   nobody asked for, and `do_try_state` does not police the whitelist, so nothing will ever
//!   flag it.
//! * **Skipping it** leaves `SubscriptionWhitelist` empty, which is what the chain looks like
//!   today, but diverges from upstream's tuple — and a future reader comparing the two tuples
//!   will find a missing entry with no explanation unless this note is carried with it.
//!
//! This migration does **not** touch `SubscriptionWhitelist` either way, and `post_upgrade`
//! asserts that it did not. Whichever way the question is answered, it is answered by adding or
//! not adding `SeedSubscriptionWhitelist` to `people-paseo`'s `Migrations` tuple — a runtime
//! decision, visible in the tuple, not buried in a pallet.

extern crate alloc;

use crate::{Config, Pallet};
use frame_support::{
	migrations::VersionedMigration, pallet_prelude::*, traits::UncheckedOnRuntimeUpgrade,
};

const LOG_TARGET: &str = "runtime::indiv-pallet-members-notifier::migration_paseo";

/// Seeds `SubscribedCollections` from the subscribers a live chain already has.
///
/// `VersionedMigration` is used for consistency with the other Paseo-local migrations and to make
/// the seeding a single, auditable event. The inner migration happens to be idempotent —
/// `rebuild_subscribed_collections()` clears and rebuilds — but re-running it would still be a
/// silent re-derivation of state the runtime maintains itself, so the version guard stays.
pub type MigrateV0ToV1<T> = VersionedMigration<
	0,
	1,
	v1::SeedSubscribedCollections<T>,
	Pallet<T>,
	<T as frame_system::Config>::DbWeight,
>;

pub mod v1 {
	use super::*;
	use crate::{SubscribedCollections, Subscribers};
	#[cfg(feature = "try-runtime")]
	use crate::SubscriptionWhitelist;
	use alloc::{collections::BTreeSet, vec::Vec};
	use indiv_support::traits::Identifier;

	/// Sanity bound on `Subscribers` for a single-block migration.
	///
	/// Live count is **1**, and `T::MaxSubscribers` is `10` on people-paseo, so this can never be
	/// approached — the bound is here so that a `Config` change between design and enactment
	/// surfaces as a loud `pre_upgrade` failure instead of as an oversized block.
	pub const EXPECTED_MAX_SUBSCRIBERS: u32 = 256;

	/// Use [`super::MigrateV0ToV1`] rather than this directly.
	///
	/// # Behaviour
	///
	/// Calls `Pallet::<T>::rebuild_subscribed_collections()`. That is the entire migration: it
	/// clears `SubscribedCollections` and re-inserts the union of every subscriber's
	/// `collections`, which is precisely the invariant `Pallet::do_try_state()` enforces. On
	/// people-paseo that writes two entries, one per collection of the single AssetHub
	/// subscription.
	///
	/// Nothing else is touched. In particular `SubscriptionWhitelist` is left exactly as it is —
	/// see the open question in this module's header — and `post_upgrade` asserts so.
	///
	/// # Single-block, not multi-block — on correctness, not weight
	///
	/// Weight is not the argument: the bound on the work is
	/// `MaxSubscribers * MaxCollectionsPerSubscriber` = 10 x 3 = 30 writes on people-paseo, and
	/// the live figure is 2.
	///
	/// The argument is that `SubscribedCollections` is a set with a whole-set invariant, not a
	/// collection of independent rows. A partially seeded map is not "some work still to do", it
	/// is a state in which SOME collections' ring-root changes propagate to AssetHub and others
	/// are silently dropped — with `do_try_state` failing and pointing at the map rather than at
	/// the cause. Worse, `rebuild_subscribed_collections()` begins by CLEARING the map, so a
	/// multi-block version of this would have to be written as a cursor-driven rebuild that
	/// cannot use the runtime's own function, which is the property that makes this migration
	/// trustworthy. Single block, one function call, whole invariant restored atomically.
	///
	/// # Failure policy
	///
	/// There is nothing to decode and nothing that can fail. The guard against acting on
	/// unexpected state is in `pre_upgrade`, which is where it can still stop an enactment.
	pub struct SeedSubscribedCollections<T>(PhantomData<T>);

	/// The collection identifiers implied by the current `Subscribers` map.
	///
	/// This is the expectation `do_try_state` checks against, computed the same way, and is used
	/// by both `pre_upgrade` and `post_upgrade` so that neither compares against a literal.
	pub fn expected_collections<T: Config>() -> BTreeSet<Identifier> {
		Subscribers::<T>::iter()
			.flat_map(|(_, info)| info.collections.iter().map(|(id, _)| *id).collect::<Vec<_>>())
			.collect()
	}

	impl<T: Config> UncheckedOnRuntimeUpgrade for SeedSubscribedCollections<T> {
		fn on_runtime_upgrade() -> Weight {
			let subscribers = Subscribers::<T>::iter().count() as u64;
			let before = SubscribedCollections::<T>::iter_keys().count();

			// The runtime's own rule, called rather than reimplemented.
			Pallet::<T>::rebuild_subscribed_collections();

			let after = SubscribedCollections::<T>::iter_keys().count();

			if subscribers > 0 && after == 0 {
				log::error!(
					target: LOG_TARGET,
					"{subscribers} subscriber(s) but SubscribedCollections is still EMPTY after \
					 seeding. Every ring-root change will be dropped silently. Do not proceed.",
				);
			} else {
				log::info!(
					target: LOG_TARGET,
					"seeded SubscribedCollections from {subscribers} subscriber(s): \
					 {before} -> {after} entries",
				);
			}

			// `rebuild_subscribed_collections` clears up to `max_subscribed_collections()` keys
			// and then writes one per collection, on top of one read per subscriber.
			let cleared = u64::from(Pallet::<T>::max_subscribed_collections());
			T::DbWeight::get()
				.reads_writes(subscribers.saturating_add(1), cleared.saturating_add(after as u64))
		}

		#[cfg(feature = "try-runtime")]
		fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
			let subscribers: Vec<(cumulus_primitives_core::ParaId, Vec<u8>)> = Subscribers::<T>::iter()
				.map(|(para_id, info)| (para_id, info.encode()))
				.collect();

			ensure!(
				subscribers.len() as u32 <= EXPECTED_MAX_SUBSCRIBERS,
				"members-notifier: Subscribers is larger than this single-block migration was \
				 sized for — re-cost it before enacting"
			);

			// The state this migration exists to repair. If it is already non-empty, something
			// else has written it and this migration's premise no longer holds.
			ensure!(
				SubscribedCollections::<T>::iter_keys().next().is_none(),
				"members-notifier: SubscribedCollections is already populated — this migration \
				 assumed it was empty, so live state is not what it was written against"
			);

			let expected: Vec<Identifier> = expected_collections::<T>().into_iter().collect();

			// A live chain with subscribers but nothing to subscribe them to would mean the
			// `Subscribers` values are not what this migration reads them as.
			if !subscribers.is_empty() {
				ensure!(
					!expected.is_empty(),
					"members-notifier: there are subscribers but none names a collection — \
					 seeding would leave SubscribedCollections empty and ring-root updates would \
					 stop"
				);
			}

			// Captured so `post_upgrade` can prove the migration stayed inside its one item.
			let whitelist: Vec<(cumulus_primitives_core::ParaId, Vec<u8>)> =
				SubscriptionWhitelist::<T>::iter().map(|(p, w)| (p, w.encode())).collect();
			let sealed = crate::SealedBatchSequence::<T>::get();
			let page_state = crate::PageState::<T>::get().encode();

			log::info!(
				target: LOG_TARGET,
				"pre_upgrade: {} subscriber(s) implying {} collection(s); \
				 SubscribedCollections empty; SubscriptionWhitelist holds {} entry/entries",
				subscribers.len(),
				expected.len(),
				whitelist.len(),
			);

			Ok((subscribers, expected, whitelist, sealed, page_state).encode())
		}

		#[cfg(feature = "try-runtime")]
		fn post_upgrade(state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
			use cumulus_primitives_core::ParaId;

			type Captured = (
				Vec<(ParaId, Vec<u8>)>,
				Vec<Identifier>,
				Vec<(ParaId, Vec<u8>)>,
				crate::types::SequenceNumber,
				Vec<u8>,
			);
			let (subscribers, expected, whitelist, sealed, page_state) =
				<Captured>::decode(&mut &state[..]).map_err(|_| {
					sp_runtime::TryRuntimeError::Other(
						"members-notifier: pre_upgrade state failed to decode",
					)
				})?;

			// 1. The subscribers themselves are untouched — this migration derives from them, it
			//    must not edit them.
			let now_subscribers: Vec<(ParaId, Vec<u8>)> =
				Subscribers::<T>::iter().map(|(p, i)| (p, i.encode())).collect();
			ensure!(
				now_subscribers == subscribers,
				"members-notifier: Subscribers changed during the migration"
			);

			// 2. `SubscribedCollections` is EXACTLY the captured expectation — both directions,
			//    so neither a missing collection nor an invented one passes.
			let got: alloc::collections::BTreeSet<Identifier> =
				SubscribedCollections::<T>::iter_keys().collect();
			let want: alloc::collections::BTreeSet<Identifier> =
				expected.iter().copied().collect();
			ensure!(
				got == want,
				"members-notifier: SubscribedCollections is not the union of the subscribers' \
				 collections captured in pre_upgrade"
			);
			ensure!(
				got.len() == expected.len(),
				"members-notifier: SubscribedCollections holds a duplicate or lost an entry"
			);

			// 3. 🔴 The open question, enforced: this migration did NOT seed the whitelist. If
			//    `SeedSubscriptionWhitelist` is added to the runtime's tuple, it runs as its own
			//    migration with its own post-check, and this assertion still holds because the
			//    tuple's entries are checked independently.
			let now_whitelist: Vec<(ParaId, Vec<u8>)> =
				SubscriptionWhitelist::<T>::iter().map(|(p, w)| (p, w.encode())).collect();
			ensure!(
				now_whitelist == whitelist,
				"members-notifier: SubscriptionWhitelist changed — this migration must not touch \
				 it; see the open question in migration_paseo.rs"
			);

			// 4. Distribution state is where it was. Catches a stray clear over the pallet.
			ensure!(
				crate::SealedBatchSequence::<T>::get() == sealed,
				"members-notifier: SealedBatchSequence changed"
			);
			ensure!(
				crate::PageState::<T>::get().encode() == page_state,
				"members-notifier: PageState changed"
			);

			// 5. The pallet's own invariant, run here so that it is checked even without
			//    `--checks all`, and so a failure is attributed to this migration.
			Pallet::<T>::do_try_state()?;

			log::info!(
				target: LOG_TARGET,
				"post_upgrade: SubscribedCollections seeded with {} collection(s) from {} \
				 subscriber(s); do_try_state passed",
				expected.len(),
				subscribers.len(),
			);
			Ok(())
		}
	}
}

#[cfg(test)]
mod tests {
	use super::{v1::*, MigrateV0ToV1};
	use crate::{
		mock::*, types::SubscriberInfo, PageState, PendingUpdates, SubscribedCollections,
		Subscribers, SubscriptionWhitelist,
	};
	use cumulus_primitives_core::ParaId;
	use frame_support::{pallet_prelude::*, traits::OnRuntimeUpgrade, BoundedVec};
	use indiv_support::traits::{Identifier, OnRingRootChange, RingExponent, RingRootOp};

	/// The two collection identifiers para 1000 (AssetHub) is subscribed to on people-paseo,
	/// read read-only from the live `Subscribers` value. 32-byte ASCII, space padded.
	const LIVE_PEOPLE: &[u8; 32] = b"pop:polkadot.network/people     ";
	const LIVE_PEOPLE_LITE: &[u8; 32] = b"pop:polkadot.network/people-lite";
	/// `MembersSubscriber` in asset-hub-paseo's `construct_runtime!`.
	const LIVE_PALLET_INDEX: u8 = 97;

	/// Recreates the single live `Subscribers` entry in the mock.
	fn insert_live_subscriber() {
		let collections: BoundedVec<_, <Test as crate::Config>::MaxCollectionsPerSubscriber> =
			BoundedVec::truncate_from(vec![
				(*LIVE_PEOPLE as Identifier, RingExponent::R2e9),
				(*LIVE_PEOPLE_LITE as Identifier, RingExponent::R2e9),
			]);
		Subscribers::<Test>::insert(
			ParaId::from(1000u32),
			SubscriberInfo::<Test> {
				collections,
				last_init_sequence: 0,
				pallet_index: LIVE_PALLET_INDEX,
			},
		);
	}

	// ===================================================================================
	// 1. The failure this migration prevents.
	// ===================================================================================

	#[test]
	fn ring_root_changes_are_dropped_without_the_seed() {
		new_test_ext().execute_with(|| {
			insert_live_subscriber();

			// This is the live state exactly: a subscriber, and an empty SubscribedCollections.
			assert_eq!(Subscribers::<Test>::iter().count(), 1);
			assert_eq!(SubscribedCollections::<Test>::iter_keys().count(), 0);

			// 🔴 The pallet's own invariant is already violated, which is what a try-runtime
			// `--checks all` run would report.
			assert!(
				crate::Pallet::<Test>::do_try_state().is_err(),
				"do_try_state must flag the unseeded map"
			);

			// A ring root changes on a collection AssetHub IS subscribed to...
            let page = PageState::<Test>::get().write_page;
			<crate::Pallet<Test> as OnRingRootChange<_>>::on_ring_root_change(
				*LIVE_PEOPLE,
				0,
				RingRootOp::Deleted,
			);

			// ...and it is silently discarded. No error, no event, nothing enqueued.
			assert_eq!(
				PendingUpdates::<Test>::iter_keys().count(),
				0,
				"the update was dropped by the SubscribedCollections gate"
			);
			assert!(!PendingUpdates::<Test>::contains_key((page, *LIVE_PEOPLE as Identifier, 0u32)));

			// ---- now run the migration ----
			StorageVersion::new(0).put::<crate::Pallet<Test>>();
			MigrateV0ToV1::<Test>::on_runtime_upgrade();

			assert!(crate::Pallet::<Test>::do_try_state().is_ok(), "invariant restored");
			assert_eq!(SubscribedCollections::<Test>::iter_keys().count(), 2);

			// The same change now propagates.
			let page = PageState::<Test>::get().write_page;
			<crate::Pallet<Test> as OnRingRootChange<_>>::on_ring_root_change(
				*LIVE_PEOPLE,
				0,
				RingRootOp::Deleted,
			);
			assert!(
				PendingUpdates::<Test>::contains_key((page, *LIVE_PEOPLE as Identifier, 0u32)),
				"the update reaches the send buffer once the collection is seeded"
			);
		});
	}

	// ===================================================================================
	// 2. The transform itself.
	// ===================================================================================

	#[test]
	fn migration_seeds_exactly_the_subscribers_collections() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<Test>>();
			insert_live_subscriber();

			MigrateV0ToV1::<Test>::on_runtime_upgrade();

			assert_eq!(StorageVersion::get::<crate::Pallet<Test>>(), 1);
			let got: alloc::collections::BTreeSet<Identifier> =
				SubscribedCollections::<Test>::iter_keys().collect();
			assert_eq!(got, expected_collections::<Test>());
			assert!(got.contains(&(*LIVE_PEOPLE as Identifier)));
			assert!(got.contains(&(*LIVE_PEOPLE_LITE as Identifier)));
			assert_eq!(got.len(), 2, "two collections, no duplicates and nothing invented");
		});
	}

	#[test]
	fn migration_does_not_touch_the_subscription_whitelist() {
		// The open question, pinned as a test: this migration answers it by NOT answering it.
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<Test>>();
			insert_live_subscriber();
			assert_eq!(SubscriptionWhitelist::<Test>::iter().count(), 0);

			MigrateV0ToV1::<Test>::on_runtime_upgrade();

			assert_eq!(
				SubscriptionWhitelist::<Test>::iter().count(),
				0,
				"seeding the whitelist is a runtime-tuple decision, not this migration's"
			);
		});
	}

	#[test]
	fn migration_deduplicates_collections_shared_by_two_subscribers() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<Test>>();
			insert_live_subscriber();
			// A second subscriber sharing one collection with the first.
			let collections: BoundedVec<_, <Test as crate::Config>::MaxCollectionsPerSubscriber> =
				BoundedVec::truncate_from(vec![(*LIVE_PEOPLE as Identifier, RingExponent::R2e9)]);
			Subscribers::<Test>::insert(
				ParaId::from(2000u32),
				SubscriberInfo::<Test> {
					collections,
					last_init_sequence: 0,
					pallet_index: LIVE_PALLET_INDEX,
				},
			);

			MigrateV0ToV1::<Test>::on_runtime_upgrade();

			assert_eq!(SubscribedCollections::<Test>::iter_keys().count(), 2);
			assert!(crate::Pallet::<Test>::do_try_state().is_ok());
		});
	}

	#[test]
	fn migration_on_a_chain_with_no_subscribers_seeds_nothing() {
		// Upstream's own situation: an empty map is CORRECT when there are no subscribers.
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<Test>>();
			assert_eq!(Subscribers::<Test>::iter().count(), 0);

			MigrateV0ToV1::<Test>::on_runtime_upgrade();

			assert_eq!(SubscribedCollections::<Test>::iter_keys().count(), 0);
			assert!(crate::Pallet::<Test>::do_try_state().is_ok());
		});
	}

	// ===================================================================================
	// 4. The try-runtime hooks, actually executed (not merely compiled).
	// ===================================================================================

	#[cfg(feature = "try-runtime")]
	#[test]
	fn pre_and_post_upgrade_agree_on_the_live_shape() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<Test>>();
			insert_live_subscriber();

			let state = MigrateV0ToV1::<Test>::pre_upgrade().expect("pre_upgrade");
			MigrateV0ToV1::<Test>::on_runtime_upgrade();
			MigrateV0ToV1::<Test>::post_upgrade(state).expect("post_upgrade");
		});
	}

	#[cfg(feature = "try-runtime")]
	#[test]
	fn post_upgrade_catches_a_map_the_migration_failed_to_seed() {
		// Proves the post-check is load-bearing — and that it is the same check
		// `do_try_state` makes, so a try-runtime `--checks all` run and this hook agree.
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<Test>>();
			insert_live_subscriber();
			let state = MigrateV0ToV1::<Test>::pre_upgrade().expect("pre_upgrade");

			// Deliberately do NOT run the migration.
			assert!(
				MigrateV0ToV1::<Test>::post_upgrade(state).is_err(),
				"post_upgrade must not pass on an unseeded SubscribedCollections"
			);
		});
	}

	#[cfg(feature = "try-runtime")]
	#[test]
	fn pre_upgrade_refuses_an_already_populated_map() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<Test>>();
			insert_live_subscriber();
			SubscribedCollections::<Test>::insert(*LIVE_PEOPLE as Identifier, ());

			assert!(
				MigrateV0ToV1::<Test>::pre_upgrade().is_err(),
				"a pre-seeded map means live state is not what this migration assumed"
			);
		});
	}

	#[test]
	fn migration_is_a_no_op_when_already_at_version_one() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(1).put::<crate::Pallet<Test>>();
			insert_live_subscriber();

			MigrateV0ToV1::<Test>::on_runtime_upgrade();

			assert_eq!(
				SubscribedCollections::<Test>::iter_keys().count(),
				0,
				"the version guard held; nothing was rebuilt"
			);
		});
	}
}
