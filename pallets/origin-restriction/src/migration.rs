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

//! PASEO-LOCAL storage migration for `indiv-pallet-origin-restriction`.
//!
//! This file does not exist upstream, and it must run on **both** `people-paseo` and
//! `asset-hub-paseo`.
//!
//! # The bug this exists to prevent
//!
//! v0.3.1 does not change [`Usage`](crate::pallet::Usage)'s SCALE type. It changes what the
//! number MEANS:
//!
//! ```text
//! -let now = frame_system::Pallet::<T>::block_number();      // local parachain block
//! +let now = T::BlockNumberProvider::current_block_number();  // relay block, upstream binds
//!                                                            // RelaychainDataProvider
//! ```
//!
//! `at_block` keeps type `u32`, so every stored value still decodes. It is simply now read on a
//! different clock — and on Paseo that clock is FAR BEHIND the one that wrote the values. Read
//! read-only during this work:
//!
//! | Chain | own height | its `ParachainSystem::LastRelayChainBlockNumber` |
//! |---|---|---|
//! | people-paseo | 6,443,886 | **891,974** |
//! | asset-hub-paseo | 12,983,559 | **891,978** |
//!
//! Recovery is `elapsed = now.saturating_sub(usage.at_block)`. With `now` on the relay clock and
//! `at_block` a parachain block number roughly 6.4 million blocks ahead, `elapsed` saturates to
//! **0, and stays 0** until the relay chain catches up — about **337 days**.
//!
//! It does not self-heal. v0.3.1 also moved the `Usages::insert` out of `validate` and into
//! `prepare` (deliberately, so the pool cannot be made to write). A transaction that fails in
//! `validate` never reaches `prepare`, so `at_block` is never re-stamped. `clean_usage` cannot
//! rescue it either: it recomputes recovery on the same stalled clock and then requires
//! `used == 0`, so it returns `Error::NotZero`.
//!
//! The one live entry, on people-paseo, is `RestrictedEntity::AccountParticipant`, whose
//! `Allowance` is `{ max: 0, recovery_per_block: CENTS }`. With `max = 0` the only way such an
//! origin transacts at all is `allowed_one_time_excess()`, which requires
//! `usage_without_new_xt == 0` — i.e. it requires `used` to have fully recovered. Frozen
//! `elapsed` therefore means a total, self-perpetuating lockout, not a slower allowance.
//!
//! `usage_never_recovers_when_the_clock_stalls` in this file's tests reproduces exactly that.
//!
//! # asset-hub-paseo needs this even though its map is empty today
//!
//! `Usages` was empty on asset-hub-paseo when this was written (the pallet prefix held only the
//! storage-version key). That is not a reason to skip it: the map is written by `prepare` on
//! every restricted-origin transaction and is only ever removed on full recovery or an explicit
//! `clean_usage`. It need only be non-empty at the upgrade block for the same lockout to apply,
//! and asset-hub-paseo is further ahead of the relay than people-paseo is. Ship it on both.

extern crate alloc;

use crate::{Config, Pallet};
use frame_support::{
	migrations::VersionedMigration, pallet_prelude::*, traits::UncheckedOnRuntimeUpgrade,
};

const LOG_TARGET: &str = "runtime::indiv-pallet-origin-restriction::migration";

/// Rebases every `Usages` entry's `at_block` onto the clock v0.3.1 reads it on.
///
/// `VersionedMigration` is mandatory rather than stylistic. [`v1::RebaseUsageClock`] is
/// **destructive and not idempotent**: a second run would re-stamp `at_block` to the then-current
/// block and throw away every block of recovery accrued since the first run. Under the shipped
/// [`v1::USED_POLICY`] that would push an entity's recovery back to the start, repeatedly. The
/// storage version is the only thing preventing it.
pub type MigrateV0ToV1<T> = VersionedMigration<
	0,
	1,
	v1::RebaseUsageClock<T>,
	Pallet<T>,
	<T as frame_system::Config>::DbWeight,
>;

pub mod v1 {
	use super::*;
	use crate::{
		pallet::{BalanceOf, ProviderBlockNumberFor, Usage, Usages},
		RestrictedEntity,
	};
	use alloc::vec::Vec;
	use sp_runtime::traits::{BlockNumberProvider, SaturatedConversion, Zero};

	// ===================================================================================
	//  🔴 THE `used` POLICY. This is a governance-flavoured decision, not a mechanical one.
	// ===================================================================================
	//
	// Rebasing `at_block` onto the relay clock is FORCED — without it the entity is locked out
	// for ~337 days, as described at the top of this file. What happens to `used` is NOT forced,
	// and upstream does not settle it because upstream has no legacy `Usages` to settle.
	//
	// THE POLICY SHIPPED HERE: [`UsedPolicy::Keep`]. `used` is carried across untouched, and
	// only `at_block` moves. To change it, change the ONE constant [`USED_POLICY`] below;
	// nothing else in this file branches on the policy.
	//
	// Why `Keep` is the conservative choice AND an affordable one:
	//
	//   * It grants nothing. The migration's job is to repair a broken clock, not to hand out
	//     allowance. `Zero` writes a balance-shaped value the chain never earned; if the policy is
	//     ever wrong, `Keep` is wrong in the direction of "the user waits", and `Zero` is wrong in
	//     the direction of "the restriction did not apply".
	//   * On the one live entry the wait is **two relay blocks**, not a year. Measured, not
	//     assumed: `used = 155,781,116` planck and `recovery_per_block = CENTS = 100,000,000`
	//     planck (`ACCOUNT_PARTICIPANT_RECOVERY`, `system-parachains/people-paseo/src/people.rs`),
	//     so `ceil(155,781,116 / 100,000,000) = 2` relay blocks ~ 12 seconds after the upgrade
	//     block. The "banned for ~337 days" outcome belongs to shipping NO migration; it is not the
	//     cost of declining to zero `used`.
	//   * `pre_upgrade` turns that from a happy accident into an enforced precondition: it FAILS if
	//     any entry would need more than [`MAX_ACCEPTABLE_RECOVERY_BLOCKS`] to recover under the
	//     shipped policy. If a bigger `used` ever shows up, a human is forced to look and choose
	//     rather than inheriting this file's default.
	//
	// THE ALTERNATIVES, for whoever has to sign this off:
	//
	//   (a) `UsedPolicy::Zero` — one-line change: set `USED_POLICY` to `UsedPolicy::Zero`.
	//       Argument for it: under the OLD clock these entities would long since have recovered
	//       (the live entry has 700,261 local blocks of unclaimed recovery against a debt of
	//       1.56 CENTS), so zeroing reproduces the behaviour the chain would have had. Argument
	//       against: it is a fiat write of a value nobody computed, and it is indistinguishable
	//       from "the restriction was lifted" for any entity whose `used` had NOT in fact
	//       recovered.
	//
	//   (b) Settle under the old clock, then rebase — `used' = used - recovery_per_block *
	//       (local_now - at_block)`, then `at_block = relay_now`. Arithmetically the most
	//       faithful of the three: it is exactly what the pre-v0.3 code would have computed on
	//       the entity's next transaction. NOT IMPLEMENTED here, deliberately: it requires
	//       arithmetic across two unrelated `BlockNumber` associated types, and on every live
	//       entry it produces the identical result to (a) while being harder to review. If (a)
	//       is chosen, prefer (a); (b) only differs for an entity that had NOT yet fully
	//       recovered under the old clock, of which there are none today.
	//
	// Whatever is chosen, `post_upgrade` re-derives its expectation from `pre_upgrade`'s capture
	// and from `USED_POLICY`, so it checks the policy that was actually compiled in.

	/// What the migration does with `Usage::used` while it rebases `Usage::at_block`.
	#[derive(Debug, Clone, Copy, PartialEq, Eq)]
	pub enum UsedPolicy {
		/// Carry `used` across untouched. Only the clock is repaired. **Shipped.**
		Keep,
		/// Reset `used` to zero, unblocking every entity immediately.
		Zero,
	}

	/// 🔴 The `used` policy this runtime enacts. See the block comment above before changing it.
	pub const USED_POLICY: UsedPolicy = UsedPolicy::Keep;

	/// Sanity bound on `Usages` for a single-block migration.
	///
	/// Live counts when this was written: people-paseo **1**, asset-hub-paseo **0**. The bound
	/// exists so a chain that grew between design and enactment surfaces as a loud `pre_upgrade`
	/// failure under try-runtime instead of as an oversized block.
	pub const EXPECTED_MAX_USAGES: u32 = 4_096;

	/// Ceiling, in blocks of `T::BlockNumberProvider`, on how long any single entity may need to
	/// recover after the migration under the shipped [`USED_POLICY`].
	///
	/// 14,400 relay blocks is 24 hours at 6 s. The live entry needs **2**. This is not a
	/// correctness bound — it is the tripwire that stops `UsedPolicy::Keep` from quietly becoming
	/// a long ban if `used` grows before enactment. Tripping it means the policy decision above
	/// has to be made again, by a person, against the numbers of the day.
	pub const MAX_ACCEPTABLE_RECOVERY_BLOCKS: u128 = 14_400;

	/// How many provider blocks an entity needs before `used` recovers to at most `max`.
	///
	/// Returns `None` if it never will — which happens when `recovery_per_block` is zero and the
	/// debt is non-zero. That case is a permanent lockout by construction and `pre_upgrade`
	/// refuses to proceed on it.
	/// Takes plain `u128` rather than `BalanceOf<T>` so it can be unit-tested directly and so
	/// that the arithmetic is the same in `on_runtime_upgrade`, `pre_upgrade` and `post_upgrade`.
	/// Every caller converts with `saturated_into::<u128>()`; both runtimes' `Balance` IS `u128`,
	/// so nothing is lost there.
	pub fn blocks_to_recover(used: u128, max: u128, recovery_per_block: u128) -> Option<u128> {
		let debt = used.saturating_sub(max);
		if debt == 0 {
			return Some(0);
		}
		if recovery_per_block == 0 {
			return None;
		}
		Some(debt.div_ceil(recovery_per_block))
	}

	/// [`blocks_to_recover`] for a `BalanceOf<T>`-typed usage and allowance.
	pub fn blocks_to_recover_for<T: Config>(
		used: BalanceOf<T>,
		allowance: &crate::Allowance<BalanceOf<T>>,
	) -> Option<u128> {
		blocks_to_recover(
			used.saturated_into::<u128>(),
			allowance.max.saturated_into::<u128>(),
			allowance.recovery_per_block.saturated_into::<u128>(),
		)
	}

	/// Use [`super::MigrateV0ToV1`] rather than this directly.
	///
	/// # Behaviour
	///
	/// For every entry in `Usages`: `at_block` is set to
	/// `T::BlockNumberProvider::current_block_number()` — i.e. to a value on the clock the
	/// v0.3.1 code reads it on — and `used` is handled per [`USED_POLICY`]. No key is added or
	/// removed. `Usages` is the pallet's only storage item, so nothing else is touched.
	///
	/// # Single-block, not multi-block — on correctness, not weight
	///
	/// Weight is not the argument: one entry on people-paseo, zero on asset-hub-paseo.
	///
	/// The argument is that a half-rebased `Usages` is the worst state reachable here, and it is
	/// invisible. Old and new `at_block` values are the same SCALE type, so a converted entry and
	/// an unconverted one are indistinguishable except by comparing the number against two chain
	/// heights. Some entities would transact normally and others would be silently frozen for
	/// months, with no event and nothing in the map to say which is which. A single-block
	/// migration cannot produce that state. A multi-block one can, and neither runtime's
	/// `FailedMigrationHandler` repairs data — people-paseo freezes the chain,
	/// asset-hub-paseo force-unsticks it.
	///
	/// # Failure policy
	///
	/// There is no conversion that can fail: the value type is unchanged and the transform is a
	/// field assignment. The migration therefore writes as it iterates rather than buffering.
	/// The guard against acting on unexpected state lives in `pre_upgrade`, which is where it
	/// can still stop an enactment.
	pub struct RebaseUsageClock<T>(PhantomData<T>);

	impl<T: Config> UncheckedOnRuntimeUpgrade for RebaseUsageClock<T> {
		fn on_runtime_upgrade() -> Weight {
			let now: ProviderBlockNumberFor<T> = T::BlockNumberProvider::current_block_number();
			let local = frame_system::Pallet::<T>::block_number();
			let mut count = 0u64;

			log::info!(
				target: LOG_TARGET,
				"rebasing Usages onto the BlockNumberProvider clock: provider now = {now:?}, \
				 local frame_system now = {local:?}, policy = {USED_POLICY:?}",
			);

			let entries: Vec<(
				T::RestrictedEntity,
				Usage<BalanceOf<T>, ProviderBlockNumberFor<T>>,
			)> = Usages::<T>::iter().collect();

			for (entity, usage) in entries {
				let allowance = entity.allowance();
				let used = match USED_POLICY {
					UsedPolicy::Keep => usage.used,
					UsedPolicy::Zero => Zero::zero(),
				};

				// Report, per entry, exactly what the shipped policy costs this entity. This is
				// the line an operator reads to see whether "conservative" meant twelve seconds
				// or twelve months.
				match blocks_to_recover_for::<T>(used, &allowance) {
					Some(0) => log::info!(
						target: LOG_TARGET,
						"{entity:?}: within allowance, no wait",
					),
					Some(blocks) => log::warn!(
						target: LOG_TARGET,
						"{entity:?}: used is over its allowance; it recovers {blocks} provider \
						 block(s) after this upgrade",
					),
					None => log::error!(
						target: LOG_TARGET,
						"{entity:?}: recovery_per_block is zero and used exceeds max — this \
						 entity can NEVER recover on its own and needs governance. pre_upgrade \
						 should have rejected this before enactment.",
					),
				}

				Usages::<T>::insert(&entity, Usage { used, at_block: now });
				count = count.saturating_add(1);
			}

			log::info!(target: LOG_TARGET, "rebased {count} Usages entries to {now:?}");

			T::DbWeight::get().reads_writes(count.saturating_add(1), count)
		}

		#[cfg(feature = "try-runtime")]
		fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
			let now = T::BlockNumberProvider::current_block_number();
			let local = frame_system::Pallet::<T>::block_number();

			// Captured so that `post_upgrade` asserts against what was on chain at the upgrade
			// block, never against a documented literal. The live figures moved while this was
			// being written (`used` 155,976,700 -> 155,781,116, `at_block` 5,742,601 ->
			// 5,743,625), which is exactly why nothing here is hard-coded.
			// A zero provider block number on a live chain means the STATE THIS RAN AGAINST is
			// missing `ParachainSystem::LastRelayChainBlockNumber`, not that the relay is at
			// genesis. Both chains read ~892,000 during this work. If a try-runtime snapshot was
			// scraped without the `ParachainSystem` prefix, the migration would rebase every
			// stamp to 0 and `post_upgrade` would happily agree, because it checks against the
			// same zero. Fail here instead — this is a harness problem, and it is invisible
			// downstream.
			ensure!(
				!now.is_zero(),
				"origin-restriction: BlockNumberProvider returned 0 — the state under test is \
				 almost certainly missing ParachainSystem::LastRelayChainBlockNumber. Re-scrape \
				 with the ParachainSystem prefix included."
			);

			let mut captured: Vec<(Vec<u8>, u128, u128)> = Vec::new();
			let mut stale_clock = 0u32;
			let mut already_plausible = 0u32;

			for (entity, usage) in Usages::<T>::iter() {
				let allowance = entity.allowance();

				// The lockout tripwire. Under the shipped policy, can this entity get out?
				let used_after: BalanceOf<T> = match USED_POLICY {
					UsedPolicy::Keep => usage.used,
					UsedPolicy::Zero => Zero::zero(),
				};
				let wait = blocks_to_recover_for::<T>(used_after, &allowance).ok_or(
					sp_runtime::TryRuntimeError::Other(
						"origin-restriction: an entity has recovery_per_block == 0 and \
							 used > max — after this migration it could NEVER recover. Do not \
							 enact; this needs governance, not a storage migration.",
					),
				)?;
				ensure!(
					wait <= MAX_ACCEPTABLE_RECOVERY_BLOCKS,
					"origin-restriction: under USED_POLICY an entity would stay blocked for more \
					 than MAX_ACCEPTABLE_RECOVERY_BLOCKS provider blocks. Re-take the `used` \
					 decision in migration.rs::v1 against today's numbers before enacting."
				);

				// Diagnostic only: is this entry really a stale local-clock stamp?
				if usage.at_block > now {
					stale_clock = stale_clock.saturating_add(1);
				} else {
					already_plausible = already_plausible.saturating_add(1);
				}

				captured.push((entity.encode(), usage.used.saturated_into::<u128>(), wait));
			}

			ensure!(
				captured.len() as u32 <= EXPECTED_MAX_USAGES,
				"origin-restriction: Usages is larger than this single-block migration was sized \
				 for — re-cost it before enacting"
			);

			log::info!(
				target: LOG_TARGET,
				"pre_upgrade: {} Usages entries ({stale_clock} carrying a stamp ahead of the \
				 provider clock, {already_plausible} already at or below it); provider now = \
				 {now:?}, local now = {local:?}, policy = {USED_POLICY:?}",
				captured.len(),
			);

			Ok((captured, now).encode())
		}

		#[cfg(feature = "try-runtime")]
		fn post_upgrade(state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
			type CapturedFor<B> = (Vec<(Vec<u8>, u128, u128)>, B);
			let (captured, captured_now) = <CapturedFor<ProviderBlockNumberFor<T>>>::decode(
				&mut &state[..],
			)
			.map_err(|_| {
				sp_runtime::TryRuntimeError::Other(
					"origin-restriction: pre_upgrade state failed to decode",
				)
			})?;

			// 1. Nothing was created or destroyed.
			ensure!(
				Usages::<T>::iter().count() == captured.len(),
				"origin-restriction: the Usages entry count changed"
			);

			for (entity_bytes, used_before, expected_wait) in &captured {
				let entity = T::RestrictedEntity::decode(&mut &entity_bytes[..]).map_err(|_| {
					sp_runtime::TryRuntimeError::Other(
						"origin-restriction: a captured entity key no longer decodes",
					)
				})?;
				let usage = Usages::<T>::get(&entity).ok_or(sp_runtime::TryRuntimeError::Other(
					"origin-restriction: a captured Usages entry disappeared",
				))?;

				// 2. THE POINT OF THE MIGRATION: every stamp is now on the provider's clock.
				ensure!(
					usage.at_block == captured_now,
					"origin-restriction: at_block was not rebased onto the BlockNumberProvider \
					 clock"
				);

				// 3. `used` matches the compiled-in policy applied to the captured value.
				let expected_used: u128 = match USED_POLICY {
					UsedPolicy::Keep => *used_before,
					UsedPolicy::Zero => 0,
				};
				ensure!(
					usage.used.saturated_into::<u128>() == expected_used,
					"origin-restriction: used does not match USED_POLICY applied to the value \
					 captured in pre_upgrade"
				);

				// 4. The lockout is actually gone: recovery from here is finite, and no worse than
				//    pre_upgrade predicted. This is the assertion that fails if the clock was left
				//    stale.
				let allowance = entity.allowance();
				let wait = blocks_to_recover_for::<T>(usage.used, &allowance).ok_or(
					sp_runtime::TryRuntimeError::Other(
						"origin-restriction: an entity is left unable to ever recover",
					),
				)?;
				ensure!(
					wait == *expected_wait && wait <= MAX_ACCEPTABLE_RECOVERY_BLOCKS,
					"origin-restriction: an entity's recovery time is not what pre_upgrade \
					 computed"
				);
			}

			log::info!(
				target: LOG_TARGET,
				"post_upgrade: {} Usages entries rebased to {captured_now:?} and verified",
				captured.len(),
			);
			Ok(())
		}
	}
}

#[cfg(test)]
mod tests {
	use super::{v1::*, MigrateV0ToV1};
	use crate::{
		mock::{
			advance_para_by, advance_relay_by, exec_signed_tx, new_test_ext, MockPalletCall,
			MockRelayBlockNumberProvider, RuntimeRestrictedEntity, ALLOWANCE_RECOVERY_PER_BLOCK,
			MAX_ALLOWANCE, RELAY_BLOCK_GENESIS, RESTRICTED_ORIGIN_1,
		},
		pallet::{Usage, Usages},
	};
	use frame_support::{assert_err, pallet_prelude::*, traits::OnRuntimeUpgrade};
	use sp_runtime::{traits::BlockNumberProvider, transaction_validity::InvalidTransaction};

	/// The live people-paseo numbers, read read-only at People block 6,443,886 / relay 891,974.
	/// Kept here as documentation of the shape of the problem, not as an assertion input — the
	/// migration itself never sees a literal.
	const LIVE_USED: u128 = 155_781_116;
	const LIVE_AT_BLOCK: u32 = 5_743_625;
	const LIVE_RELAY_NOW: u32 = 891_974;
	/// `ACCOUNT_PARTICIPANT_RECOVERY = CENTS` on people-paseo.
	const LIVE_RECOVERY_PER_BLOCK: u128 = 100_000_000;

	// ===================================================================================
	// 1. The failure this migration prevents.
	// ===================================================================================

	#[test]
	fn live_entry_is_locked_out_for_about_a_year_without_the_migration() {
		// Pure arithmetic on the real live values, no runtime needed. `max = 0` for
		// `AccountParticipant`.
		let elapsed = LIVE_RELAY_NOW.saturating_sub(LIVE_AT_BLOCK);
		assert_eq!(elapsed, 0, "the relay clock is behind the stamp, so nothing ever elapses");

		// ...and it stays 0 until the relay reaches the stamp.
		let relay_blocks_until_unstuck = LIVE_AT_BLOCK - LIVE_RELAY_NOW;
		assert_eq!(relay_blocks_until_unstuck, 4_851_651);
		let days = (relay_blocks_until_unstuck as u64 * 6) / 86_400;
		assert_eq!(days, 336, "~337 days of lockout");

		// With the migration and the SHIPPED `Keep` policy, the same entity waits two blocks.
		assert_eq!(
			blocks_to_recover(LIVE_USED, 0, LIVE_RECOVERY_PER_BLOCK),
			Some(2),
			"ceil(155,781,116 / 100,000,000)"
		);
	}

	#[test]
	fn usage_never_recovers_when_the_clock_stalls() {
		// The same failure, executed end to end through the real transaction extension.
		new_test_ext().execute_with(|| {
			let entity = RuntimeRestrictedEntity::A;
			// A stamp written by the OLD code on the parachain clock, far ahead of the relay.
			let stale_stamp = RELAY_BLOCK_GENESIS + 1_000_000;
			Usages::<crate::mock::Test>::insert(
				&entity,
				Usage { used: MAX_ALLOWANCE + 100, at_block: stale_stamp },
			);

			// Over allowance, so the origin is refused.
			assert_err!(
				exec_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something {}),
				InvalidTransaction::Payment
			);

			// Let a LOT of relay time pass — far more than would be needed to recover
			// `used` at `ALLOWANCE_RECOVERY_PER_BLOCK` if the clock were sane.
			advance_relay_by(10_000);
			advance_para_by(10_000);

			// Still refused, and `used` has not moved: `saturating_sub` is still yielding 0.
			assert_err!(
				exec_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something {}),
				InvalidTransaction::Payment
			);
			let usage = Usages::<crate::mock::Test>::get(&entity).unwrap();
			assert_eq!(usage.used, MAX_ALLOWANCE + 100, "no recovery at all");
			assert_eq!(usage.at_block, stale_stamp, "and the stamp is never re-written");

			// `clean_usage` cannot rescue it either: it recomputes on the same stalled clock.
			assert_err!(
				crate::Pallet::<crate::mock::Test>::clean_usage(
					frame_system::RawOrigin::Signed(RESTRICTED_ORIGIN_1).into(),
					entity.clone(),
				),
				crate::Error::<crate::mock::Test>::NotZero
			);

			// ---- now run the migration ----
			StorageVersion::new(0).put::<crate::Pallet<crate::mock::Test>>();
			MigrateV0ToV1::<crate::mock::Test>::on_runtime_upgrade();

			let now = MockRelayBlockNumberProvider::current_block_number();
			let usage = Usages::<crate::mock::Test>::get(&entity).unwrap();
			assert_eq!(usage.at_block, now, "the stamp is on the provider's clock now");
			assert_eq!(usage.used, MAX_ALLOWANCE + 100, "USED_POLICY::Keep touched nothing else");

			// The migration handed out nothing: still over allowance, still refused.
			assert_err!(
				exec_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something {}),
				InvalidTransaction::Payment
			);

			// But recovery has RESTARTED, which is the whole point. 100 over allowance at 5 per
			// block is 20 blocks to get back inside `max`; the transaction additionally has to
			// fit its own fee under `max`, so give it headroom rather than asserting on the
			// exact fee the mock's `IdentityFee` happens to produce.
			let wait = blocks_to_recover(
				usage.used as u128,
				MAX_ALLOWANCE as u128,
				ALLOWANCE_RECOVERY_PER_BLOCK as u128,
			)
			.unwrap();
			assert_eq!(wait, 20);
			advance_relay_by(wait as u64 + 100);
			advance_para_by(wait as u64 + 100);

			// Unblocked — the outcome the stalled clock made unreachable for ~337 days.
			assert!(exec_signed_tx(RESTRICTED_ORIGIN_1, MockPalletCall::do_something {}).is_ok());
		});
	}

	// ===================================================================================
	// 2. The transform itself.
	// ===================================================================================

	#[test]
	fn migration_rebases_every_entry_and_keeps_used() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<crate::mock::Test>>();
			advance_relay_by(500);

			Usages::<crate::mock::Test>::insert(
				&RuntimeRestrictedEntity::A,
				Usage { used: 7, at_block: 9_999_999 },
			);
			Usages::<crate::mock::Test>::insert(
				&RuntimeRestrictedEntity::B,
				Usage { used: MAX_ALLOWANCE, at_block: 1 },
			);

			MigrateV0ToV1::<crate::mock::Test>::on_runtime_upgrade();

			let now = MockRelayBlockNumberProvider::current_block_number();
			assert_eq!(StorageVersion::get::<crate::Pallet<crate::mock::Test>>(), 1);
			assert_eq!(Usages::<crate::mock::Test>::iter().count(), 2, "no entry added or lost");

			let a = Usages::<crate::mock::Test>::get(&RuntimeRestrictedEntity::A).unwrap();
			assert_eq!(a.at_block, now);
			assert_eq!(a.used, 7, "USED_POLICY::Keep");

			let b = Usages::<crate::mock::Test>::get(&RuntimeRestrictedEntity::B).unwrap();
			assert_eq!(b.at_block, now);
			assert_eq!(b.used, MAX_ALLOWANCE);
		});
	}

	#[test]
	fn migration_is_a_no_op_when_already_at_version_one() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(1).put::<crate::Pallet<crate::mock::Test>>();
			Usages::<crate::mock::Test>::insert(
				&RuntimeRestrictedEntity::A,
				Usage { used: 7, at_block: RELAY_BLOCK_GENESIS },
			);
			advance_relay_by(50);

			MigrateV0ToV1::<crate::mock::Test>::on_runtime_upgrade();

			// A second run must NOT re-stamp: that would throw away 50 blocks of recovery.
			let a = Usages::<crate::mock::Test>::get(&RuntimeRestrictedEntity::A).unwrap();
			assert_eq!(a.at_block, RELAY_BLOCK_GENESIS);
		});
	}

	#[test]
	fn migration_on_an_empty_map_is_harmless() {
		// asset-hub-paseo's case today.
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<crate::mock::Test>>();
			MigrateV0ToV1::<crate::mock::Test>::on_runtime_upgrade();
			assert_eq!(Usages::<crate::mock::Test>::iter().count(), 0);
			assert_eq!(StorageVersion::get::<crate::Pallet<crate::mock::Test>>(), 1);
		});
	}

	// ===================================================================================
	// 3. The recovery arithmetic that both `pre_upgrade` and `post_upgrade` rely on.
	// ===================================================================================

	#[test]
	fn blocks_to_recover_is_a_ceiling_and_reports_the_impossible_case() {
		assert_eq!(blocks_to_recover(0, 0, 5), Some(0));
		assert_eq!(blocks_to_recover(5, 10, 5), Some(0), "already within allowance");
		assert_eq!(blocks_to_recover(10, 0, 5), Some(2));
		assert_eq!(blocks_to_recover(11, 0, 5), Some(3), "ceiling, not floor");
		// The permanent-lockout case `pre_upgrade` refuses to enact on.
		assert_eq!(blocks_to_recover(1, 0, 0), None);
		// ...but a zero recovery rate is fine when there is no debt.
		assert_eq!(blocks_to_recover(0, 0, 0), Some(0));
	}

	// ===================================================================================
	// 4. The try-runtime hooks, actually executed (not merely compiled).
	// ===================================================================================

	#[cfg(feature = "try-runtime")]
	#[test]
	fn pre_and_post_upgrade_agree_on_a_realistic_state() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<crate::mock::Test>>();
			advance_relay_by(200);
			// Two entries with stamps ahead of the provider clock, as on people-paseo.
			Usages::<crate::mock::Test>::insert(
				&RuntimeRestrictedEntity::A,
				Usage { used: MAX_ALLOWANCE + 50, at_block: 9_000_000 },
			);
			Usages::<crate::mock::Test>::insert(
				&RuntimeRestrictedEntity::B,
				Usage { used: 1, at_block: 9_000_001 },
			);

			let state = MigrateV0ToV1::<crate::mock::Test>::pre_upgrade().expect("pre_upgrade");
			MigrateV0ToV1::<crate::mock::Test>::on_runtime_upgrade();
			MigrateV0ToV1::<crate::mock::Test>::post_upgrade(state).expect("post_upgrade");
		});
	}

	#[cfg(feature = "try-runtime")]
	#[test]
	fn pre_upgrade_refuses_a_state_with_no_relay_block_number() {
		// Guards against a try-runtime snapshot scraped without the `ParachainSystem` prefix:
		// every stamp would be rebased to 0 and post_upgrade would agree, silently.
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<crate::mock::Test>>();
			MockRelayBlockNumberProvider::set_block_number(0);
			Usages::<crate::mock::Test>::insert(
				&RuntimeRestrictedEntity::A,
				Usage { used: 1, at_block: 9_000_000 },
			);

			assert!(MigrateV0ToV1::<crate::mock::Test>::pre_upgrade().is_err());
		});
	}

	#[cfg(feature = "try-runtime")]
	#[test]
	fn pre_upgrade_refuses_an_entity_that_would_stay_blocked_too_long() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<crate::mock::Test>>();
			// A debt that needs more than MAX_ACCEPTABLE_RECOVERY_BLOCKS at 5/block.
			let debt = MAX_ALLOWANCE +
				(MAX_ACCEPTABLE_RECOVERY_BLOCKS as u64 + 1) * ALLOWANCE_RECOVERY_PER_BLOCK;
			Usages::<crate::mock::Test>::insert(
				&RuntimeRestrictedEntity::A,
				Usage { used: debt, at_block: 9_000_000 },
			);

			assert!(
				MigrateV0ToV1::<crate::mock::Test>::pre_upgrade().is_err(),
				"the `used` policy must be re-taken by a human, not inherited"
			);
		});
	}

	#[cfg(feature = "try-runtime")]
	#[test]
	fn post_upgrade_catches_a_stamp_the_migration_failed_to_rebase() {
		// Proves the post-check is load-bearing: if `at_block` is left stale, it fails.
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<crate::mock::Test>>();
			Usages::<crate::mock::Test>::insert(
				&RuntimeRestrictedEntity::A,
				Usage { used: 1, at_block: 9_000_000 },
			);
			let state = MigrateV0ToV1::<crate::mock::Test>::pre_upgrade().expect("pre_upgrade");

			// Deliberately do NOT run the migration — simulate a no-op / half-applied run.
			assert!(
				MigrateV0ToV1::<crate::mock::Test>::post_upgrade(state).is_err(),
				"post_upgrade must not pass on an unrebased Usages entry"
			);
		});
	}

	#[test]
	fn the_shipped_policy_is_keep() {
		// A deliberate tripwire. If someone flips `USED_POLICY`, this test fails and the note in
		// guides/_recon/REQUIRED_MIGRATIONS_RESULT.md has to be updated with it.
		assert_eq!(USED_POLICY, UsedPolicy::Keep);
	}

	#[test]
	fn zero_policy_would_unblock_immediately() {
		// Documents what the one-line alternative buys, without shipping it.
		let used = LIVE_USED;
		assert_eq!(blocks_to_recover(used, 0, LIVE_RECOVERY_PER_BLOCK), Some(2));
		assert_eq!(blocks_to_recover(0, 0, LIVE_RECOVERY_PER_BLOCK), Some(0));
	}
}
