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

//! PASEO-LOCAL storage migration for `indiv-pallet-score`.
//!
//! This file does not exist upstream. Upstream's `next-people-paseo` is launched from a genesis
//! preset that writes the v0.3.x shape directly, so it never has to migrate into it.
//! `people-paseo` does: it has been running the pre-v0.3 shape live since spec `2_004_003`, with
//! four `Participants` entries.
//!
//! # The bug this exists to prevent
//!
//! v0.3.1 narrows two fields at the FRONT of [`Participant`](crate::Participant):
//!
//! ```text
//!  pub struct Participant<Balance> {
//! -    pub score: u32,                  field 0
//! +    pub score: u8,
//!      pub streak: Streak,              field 1  -- Attended(u32)/Absent(u32) -> (u8)/(u8)
//!      pub attendance_history: AttendanceHistory,   //  u8
//!      pub credit: Balance,             //  u128 on people-paseo
//!      ... three bools, Recognition, Option<u32>
//!  }
//! ```
//!
//! Six bytes disappear from the front of the struct, so every later field shifts. The stored
//! bytes still DECODE — they just decode into different values. `unhashed::get` calls
//! `Decode::decode`, not `decode_all`, so the six trailing bytes are discarded without an error
//! (`frame-support`'s `storage/unhashed.rs`). There is no log line, no `Default` substitution, and
//! no corrupted-state event. The result is a plausible-looking wrong value.
//!
//! Measured on the four live entries, all of which hold the identical 31 bytes
//! `0x000000000000000000ff000000000000000000000000000000000000000100`:
//!
//! | Field | pre-v0.3 (correct) | v0.3.1 read of the SAME bytes |
//! |---|---|---|
//! | `score` | 0 | 0 |
//! | `streak` | `Attended(0)` | `Attended(0)` |
//! | `attendance_history` | `0xFF` (attended all 8) | **`0x00`** (missed all 8) |
//! | `credit` | 0 | **280_375_465_082_880** (~28,037 PAS at 10 decimals) |
//! | `recognition` | `NotRecognized` | **`ExternallyRecognized`** |
//!
//! 🔴 `credit` is HOLD-BACKED: `cash_out` releases it from a hold on the score pot that was never
//! taken. A phantom credit is not a cosmetic misreading, it is an attempt to move funds that do
//! not exist. `recognition` flipping to `ExternallyRecognized` additionally makes
//! `Recognition::is_recognized()` return `true` for an unrecognised account.
//!
//! `migration_reproduces_the_silent_misdecode` in this file's tests asserts every row of that
//! table against the real live bytes, so the failure mode stays pinned rather than described.
//!
//! # Keys are NOT affected
//!
//! `AccountOrPerson` moved from this pallet to `indiv_support::identity` but kept its variant
//! order (`Account` = 0, `Person` = 1) and its encoding. All four live keys are `Account(..)`;
//! there is no `Person(Alias)` participant today. Nothing is re-keyed here.

extern crate alloc;

use crate::{Config, Pallet};
use frame_support::{
	migrations::VersionedMigration, pallet_prelude::*, traits::UncheckedOnRuntimeUpgrade,
};

const LOG_TARGET: &str = "runtime::indiv-pallet-score::migration";

/// Re-encodes `Participants`, `PersonhoodThreshold` and `PersonhoodThresholdSchedule` for the
/// individuality v0.3.1 storage shape.
///
/// `VersionedMigration` is mandatory rather than stylistic: [`v1::MigrateScoreWidthsToU8`] is
/// **not idempotent**. Run twice, the second pass would read already-narrowed 25-byte values with
/// the 31-byte pre-v0.3 type. `Participant` has a trailing `Option<u32>`, so a short buffer makes
/// that decode fail rather than silently succeed — the migration would abort loudly instead of
/// corrupting — but the storage version is the real protection and must not be removed.
pub type MigrateV0ToV1<T> = VersionedMigration<
	0,
	1,
	v1::MigrateScoreWidthsToU8<T>,
	Pallet<T>,
	<T as frame_system::Config>::DbWeight,
>;

pub mod v1 {
	use super::*;
	#[cfg(feature = "try-runtime")]
	use crate::AbsenceGraceSchedule;
	use crate::{
		pallet::{BalanceOf, MAX_PERSONHOOD_THRESHOLD_TIERS},
		types::{
			AccountOrPerson, AttendanceHistory, Participant, PersonhoodThresholdTier,
			PersonhoodThresholdTiers, Recognition, Streak,
		},
		Participants, PersonhoodThreshold, PersonhoodThresholdSchedule,
	};
	use alloc::vec::Vec;
	use sp_core::ConstU32;

	// ===================================================================================
	//  🔴 THE u32 -> u8 NARROWING RULE. Read this before changing anything below.
	// ===================================================================================
	//
	// `score`, `Streak::Attended(_)`, `Streak::Absent(_)` and
	// `PersonhoodThresholdTier::score_threshold` all go from `u32` to `u8`. Values above 255
	// have no representation in the new type and SOMETHING has to be chosen.
	//
	// This is a human decision that production data cannot settle: all four live `Participants`
	// hold `score = 0` and `Streak::Attended(0)`, and `PersonhoodThreshold` /
	// `PersonhoodThresholdSchedule` are both ABSENT from live storage (read read-only from
	// people-paseo). Every path below is therefore exercised only by the tests in this file.
	//
	// THE RULE SHIPPED HERE: saturate at `u8::MAX`, never truncate, and shout.
	//
	//   * Saturating cannot silently turn a large value into a small one. `as u8` can, and that is
	//     the dangerous direction: `score = 256` truncates to `0`, which SILENTLY REVOKES
	//     personhood, while saturating to 255 preserves it. Every one of these fields is monotone
	//     in "privilege the account already had", so rounding UP preserves the status quo and
	//     rounding DOWN removes it without anyone noticing.
	//   * It matches what the new runtime does anyway. `Streak`'s own doc-comment says the count
	//     "saturates at u8::MAX ... so saturation is behaviourally transparent", and `score` is
	//     re-clamped on the next update by `score.min(MAX_PERSONHOOD_THRESHOLD)` (= 21). A
	//     saturated value self-heals; a truncated one does not.
	//   * Under the pre-v0.3 runtime `score` was ALREADY clamped to 21 on every write (`score.score
	//     = uncapped_score.min(MAX_PERSONHOOD_THRESHOLD)`, with `MAX_PERSONHOOD_THRESHOLD: u32 =
	//     21`) and `score_threshold` was validated `<= 21`. So a stored value above 255 is not
	//     merely unlikely, it is UNREACHABLE by the old code. If one shows up, the state is not
	//     what this migration was written against.
	//
	// Hence the split behaviour, which is the important half of the rule:
	//
	//   * [`pre_upgrade`] treats ANY saturation as a HARD FAILURE. Under try-runtime a human sees
	//     it before enactment and decides, which is the correct place for a decision this code is
	//     not entitled to make.
	//   * [`on_runtime_upgrade`] saturates and logs at `error!` instead of aborting. Aborting at
	//     enactment would leave the pre-v0.3 bytes in place under the v0.3.1 type — i.e. it would
	//     leave the phantom-credit bug switched on, which is strictly worse than a saturated streak
	//     counter.
	//
	// ALTERNATIVES CONSIDERED AND REJECTED, for the record:
	//   (a) `value as u8` (truncate)  -- rejected: silently maps 256 -> 0, the one direction
	//       that revokes privilege with no trace. Never acceptable here.
	//   (b) clamp to the semantic cap 21 -- rejected: indistinguishable from saturation for
	//       `score` (both are >= any threshold, both self-heal to <= 21 on the next write) but
	//       WRONG for `Streak`, whose counter is not bounded by 21, and it hard-codes a runtime
	//       constant into a storage transform.
	//   (c) abort the whole migration on any out-of-range value -- rejected as the ENACTMENT
	//       behaviour for the reason above; adopted as the PRE-UPGRADE behaviour, which is where
	//       it does good instead of harm.
	//
	// To change the rule, change [`narrow`] and nothing else. It is the only place a `u32`
	// becomes a `u8` in this file.

	/// The one and only `u32` -> `u8` conversion in this migration.
	///
	/// Returns the narrowed value and whether it had to saturate. See the block comment above
	/// for why saturation, and not truncation, is the rule.
	pub fn narrow(value: u32) -> (u8, bool) {
		if value > u8::MAX as u32 {
			(u8::MAX, true)
		} else {
			(value as u8, false)
		}
	}

	/// Encoded-length delta of one `Participant`: the migration removes exactly six bytes.
	///
	/// `score` loses 3 (`u32` -> `u8`) and the `Streak` payload loses 3 (the variant tag stays).
	/// Nothing else in the struct changes width, so this holds for every entry regardless of
	/// content. Live: 31 bytes -> 25 bytes. `post_upgrade` asserts it per entry, which is a
	/// cheap, content-independent proof that the value really was re-encoded and not merely
	/// re-written.
	pub const PARTICIPANT_BYTES_DROPPED: usize = 6;

	/// Sanity bound on `Participants` for a single-block migration.
	///
	/// Live count is **4**. The bound exists so a chain that grew unexpectedly between design and
	/// enactment surfaces as a loud `pre_upgrade` failure under try-runtime instead of as an
	/// oversized block. It is not a correctness limit — see the single-block justification on
	/// [`MigrateScoreWidthsToU8`].
	pub const EXPECTED_MAX_PARTICIPANTS: u32 = 4_096;

	/// `Streak` as stored before v0.3.1.
	///
	/// Variant order and indices are identical to the new [`Streak`]; only the payload width
	/// changes. That is precisely why the old bytes still decode as the new type.
	#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Eq, PartialEq, Debug, Clone)]
	pub enum OldStreak {
		Attended(u32),
		Absent(u32),
	}

	/// `Participant` as stored before v0.3.1.
	///
	/// Every field after `streak` is byte-identical to the new type; they are listed here only so
	/// that the decoder consumes the record in the right order.
	#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Eq, PartialEq, Debug)]
	pub struct OldParticipant<Balance> {
		pub score: u32,
		pub streak: OldStreak,
		pub attendance_history: AttendanceHistory,
		pub credit: Balance,
		pub cashed_out: bool,
		pub reached_personhood: bool,
		pub has_ever_reached_personhood: bool,
		pub recognition: Recognition,
		pub last_attended_game: Option<u32>,
	}

	/// `PersonhoodThresholdTier` as stored before v0.3.1.
	#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Eq, PartialEq, Debug, Clone)]
	pub struct OldPersonhoodThresholdTier {
		pub population_size_threshold: u32,
		pub score_threshold: u32,
	}

	/// The bounded tier list as stored before v0.3.1.
	pub type OldPersonhoodThresholdTiers =
		BoundedVec<OldPersonhoodThresholdTier, ConstU32<MAX_PERSONHOOD_THRESHOLD_TIERS>>;

	/// The pre-v0.3 layouts, at their original (and unchanged) storage prefixes.
	///
	/// The two `StorageValue` aliases deliberately use `OptionQuery` where the live pallet uses
	/// `ValueQuery`. Both live items are ABSENT on people-paseo, and `ValueQuery` cannot tell
	/// "absent, so the default applies" from "present and equal to the default". The migration
	/// must only rewrite a key that actually exists — writing a defaulted value would turn an
	/// invisible governance-tunable back into a stored one.
	pub mod old {
		use super::*;

		#[frame_support::storage_alias]
		pub type Participants<T: Config> = StorageMap<
			Pallet<T>,
			Blake2_128Concat,
			AccountOrPerson<<T as frame_system::Config>::AccountId>,
			OldParticipant<BalanceOf<T>>,
			OptionQuery,
		>;

		#[frame_support::storage_alias]
		pub type PersonhoodThreshold<T: Config> = StorageValue<Pallet<T>, u32, OptionQuery>;

		#[frame_support::storage_alias]
		pub type PersonhoodThresholdSchedule<T: Config> =
			StorageValue<Pallet<T>, OldPersonhoodThresholdTiers, OptionQuery>;
	}

	/// Narrows one participant record. Returns the new record and the number of fields that had
	/// to saturate.
	pub fn convert_participant<Balance>(
		old: OldParticipant<Balance>,
	) -> (Participant<Balance>, u32) {
		let (score, score_saturated) = narrow(old.score);
		let (streak, streak_saturated) = match old.streak {
			OldStreak::Attended(n) => {
				let (n, s) = narrow(n);
				(Streak::Attended(n), s)
			},
			OldStreak::Absent(n) => {
				let (n, s) = narrow(n);
				(Streak::Absent(n), s)
			},
		};
		let saturations = u32::from(score_saturated) + u32::from(streak_saturated);
		(
			Participant {
				score,
				streak,
				// Everything below is carried verbatim. These are exactly the fields the
				// unmigrated read corrupts, so they must survive bit-for-bit.
				attendance_history: old.attendance_history,
				credit: old.credit,
				cashed_out: old.cashed_out,
				reached_personhood: old.reached_personhood,
				has_ever_reached_personhood: old.has_ever_reached_personhood,
				recognition: old.recognition,
				last_attended_game: old.last_attended_game,
			},
			saturations,
		)
	}

	/// Use [`super::MigrateV0ToV1`] rather than this directly.
	///
	/// # Behaviour
	///
	/// - Every `Participants` value is re-encoded in place (same key) from the pre-v0.3 layout.
	/// - `PersonhoodThreshold` and `PersonhoodThresholdSchedule` are re-encoded **only if they
	///   exist**. Both are absent on people-paseo today, so this is a no-op there; it is here so
	///   that a value set by governance between now and enactment is migrated instead of misread.
	///   See the note on [`old`] for why they are not simply killed: killing would discard a
	///   governance decision, and `PersonhoodThresholdSchedule`'s stored default is not the same
	///   thing as its absence.
	/// - `AbsenceGraceSchedule`, `AbsenceGraceRatio`, `CurrentRoundPoints`, `CurrentRoundIndex`,
	///   `RoundsPointsForParticipant`, `RoundPayouts`, `RoundPlanning` and `RoundSchedules` are NOT
	///   touched — their types are byte-identical across the version.
	///
	/// # Single-block, not multi-block — on correctness, not weight
	///
	/// Weight is not the argument: 4 entries of 31 bytes is roughly 4 reads and 4 writes, noise
	/// against people-paseo's PoV budget, and even a 1,000x surprise stays comfortably in one
	/// block (the [`EXPECTED_MAX_PARTICIPANTS`] guard turns that surprise into a loud
	/// `pre_upgrade` failure rather than a heavy block).
	///
	/// The argument is that a HALF-MIGRATED `Participants` map is the worst state reachable
	/// here. Old and new values live at the same keys and both decode, so a partially converted
	/// map is indistinguishable from a fully converted one by inspection: some accounts would
	/// carry a real credit and others a phantom ~28,037 PAS credit, with no error, no event and
	/// no way to tell which is which except by knowing when each entry was written. A
	/// single-block migration cannot produce that state. A multi-block one can, and people-paseo
	/// runs `FreezeChainOnFailedMigration`, which freezes the chain but does not repair the
	/// half-converted map.
	///
	/// # Failure policy: all-or-nothing
	///
	/// Phase 1 reads and converts everything into memory and writes nothing. Phase 2 writes only
	/// if every conversion succeeded. A value that will not decode as the pre-v0.3 type aborts
	/// the run with an `error!` log and NOTHING written, leaving storage recoverable.
	///
	/// Saturation is the deliberate exception: it does not abort. See the rule block above.
	///
	/// An aborted run still bumps the storage version — that is `VersionedMigration`'s
	/// unconditional behaviour — and will not retry. Deliberate: an abort means the live bytes
	/// were not what this code was written against, and the right response is a human reading
	/// the chain, not an automatic retry.
	pub struct MigrateScoreWidthsToU8<T>(PhantomData<T>);

	impl<T: Config> UncheckedOnRuntimeUpgrade for MigrateScoreWidthsToU8<T> {
		fn on_runtime_upgrade() -> Weight {
			let mut reads = 0u64;
			let mut writes = 0u64;

			// ---- Phase 1: read and convert. No writes. ----
			let mut converted: Vec<(AccountOrPerson<T::AccountId>, Participant<BalanceOf<T>>)> =
				Vec::new();
			let mut saturations = 0u32;

			// `iter()` over the old alias silently SKIPS a value that fails to decode, so the
			// keys are enumerated separately and each value is fetched explicitly. Otherwise an
			// undecodable entry would be dropped from the migration without a word and left
			// behind in the pre-v0.3 encoding.
			for key in old::Participants::<T>::iter_keys() {
				reads = reads.saturating_add(1);
				let Some(old_value) = old::Participants::<T>::get(&key) else {
					log::error!(
						target: LOG_TARGET,
						"ABORTING: a Participants value does not decode as the pre-v0.3 layout. \
						 NOTHING has been written; storage is untouched. This means live state is \
						 not what this migration was written against — inspect the chain.",
					);
					return T::DbWeight::get().reads(reads);
				};
				let (new_value, saturated) = convert_participant(old_value);
				if saturated > 0 {
					// Not fatal at enactment (aborting here would leave the phantom-credit
					// misdecode in place), but it must be impossible to miss in the logs.
					log::error!(
						target: LOG_TARGET,
						"SATURATED {saturated} field(s) of a Participant while narrowing u32 -> \
						 u8. pre_upgrade should have rejected this before enactment. Review the \
						 affected account.",
					);
				}
				saturations = saturations.saturating_add(saturated);
				converted.push((key, new_value));
			}

			let count = converted.len() as u32;
			if count > EXPECTED_MAX_PARTICIPANTS {
				log::error!(
					target: LOG_TARGET,
					"Participants holds {count} entries, above the {EXPECTED_MAX_PARTICIPANTS} \
					 this single-block migration was sized for; the block may be heavy.",
				);
			}

			// The two governance-tunables, only if a key really exists.
			let threshold = old::PersonhoodThreshold::<T>::get().map(|v| {
				reads = reads.saturating_add(1);
				let (narrowed, saturated) = narrow(v);
				if saturated {
					log::error!(
						target: LOG_TARGET,
						"SATURATED PersonhoodThreshold {v} -> {narrowed} while narrowing u32 -> u8",
					);
				}
				(narrowed, u32::from(saturated))
			});

			let mut schedule: Option<PersonhoodThresholdTiers> = None;
			if let Some(old_tiers) = old::PersonhoodThresholdSchedule::<T>::get() {
				reads = reads.saturating_add(1);
				let mut tiers: PersonhoodThresholdTiers = BoundedVec::new();
				for tier in old_tiers.into_iter() {
					let (score_threshold, saturated) = narrow(tier.score_threshold);
					if saturated {
						log::error!(
							target: LOG_TARGET,
							"SATURATED a PersonhoodThresholdTier score_threshold while narrowing \
							 u32 -> u8",
						);
						saturations = saturations.saturating_add(1);
					}
					if tiers
						.try_push(PersonhoodThresholdTier {
							population_size_threshold: tier.population_size_threshold,
							score_threshold,
						})
						.is_err()
					{
						// Unreachable: the source BoundedVec carries the same bound. Handled
						// rather than `expect`ed, because a panic in a single-block migration
						// takes the block with it.
						log::error!(
							target: LOG_TARGET,
							"ABORTING: PersonhoodThresholdSchedule overflowed its bound while \
							 being re-encoded. NOTHING has been written.",
						);
						return T::DbWeight::get().reads(reads);
					}
				}
				schedule = Some(tiers);
			}

			// ---- Phase 2: write. ----
			for (key, value) in converted {
				// Same key, new value encoding: an overwrite in place. No key is created or
				// removed, which is why `post_upgrade` can assert the count is unchanged.
				Participants::<T>::insert(&key, value);
				writes = writes.saturating_add(1);
			}
			if let Some((value, saturated)) = threshold {
				saturations = saturations.saturating_add(saturated);
				PersonhoodThreshold::<T>::put(value);
				writes = writes.saturating_add(1);
			}
			if let Some(tiers) = schedule {
				PersonhoodThresholdSchedule::<T>::put(tiers);
				writes = writes.saturating_add(1);
			}

			log::info!(
				target: LOG_TARGET,
				"re-encoded {count} Participants from the pre-v0.3 u32 layout to the v0.3.1 u8 \
				 layout ({saturations} field(s) saturated at u8::MAX)",
			);

			T::DbWeight::get().reads_writes(reads, writes)
		}

		#[cfg(feature = "try-runtime")]
		fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
			use frame_support::storage::unhashed;

			// Capture RAW BYTES, not decoded values. `post_upgrade` re-derives the expectation
			// from these, so every assertion there is against what was actually on chain at the
			// upgrade block — never against a literal from a design document. Live counts have
			// already drifted during this work, and the same is assumed of these.
			let mut captured: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
			let mut would_saturate = 0u32;

			for key in old::Participants::<T>::iter_keys() {
				let full_key = old::Participants::<T>::hashed_key_for(&key);
				let raw =
					unhashed::get_raw(&full_key).ok_or(sp_runtime::TryRuntimeError::Other(
						"score: a Participants key enumerated but its value could not be read",
					))?;
				// Must decode as the OLD layout, consuming every byte. `decode_all` is used
				// deliberately: `Decode::decode` would accept a value that is already in the new
				// 25-byte shape padded with junk, and this check exists to prove the layout.
				let old_value =
					<OldParticipant<BalanceOf<T>> as codec::DecodeAll>::decode_all(&mut &raw[..])
						.map_err(|_| {
						sp_runtime::TryRuntimeError::Other(
							"score: a live Participant does not decode as the pre-v0.3 layout \
								 — live state is not what this migration was written against",
						)
					})?;
				if narrow(old_value.score).1 {
					would_saturate = would_saturate.saturating_add(1);
				}
				let streak_inner = match old_value.streak {
					OldStreak::Attended(n) | OldStreak::Absent(n) => n,
				};
				if narrow(streak_inner).1 {
					would_saturate = would_saturate.saturating_add(1);
				}
				captured.push((full_key, raw));
			}

			ensure!(
				captured.len() as u32 <= EXPECTED_MAX_PARTICIPANTS,
				"score: Participants is larger than this single-block migration was sized for — \
				 re-cost it before enacting"
			);

			// 🔴 The saturation gate. This is the human decision point: every live value is 0,
			// and the pre-v0.3 runtime clamped `score` to 21 on every write, so an out-of-range
			// value means the state is not what anyone believes it is. Refuse to proceed and let
			// a person look, rather than silently applying a rule nobody signed off on.
			ensure!(
				would_saturate == 0,
				"score: at least one live value exceeds u8::MAX and would SATURATE. The u32 -> u8 \
				 narrowing rule has never been exercised on production data. Do not enact: read \
				 the affected Participants entries and confirm the rule in migration.rs::v1::narrow"
			);

			// The two governance-tunables: capture presence AND raw bytes.
			let threshold_raw = unhashed::get_raw(&old::PersonhoodThreshold::<T>::hashed_key());
			let schedule_raw =
				unhashed::get_raw(&old::PersonhoodThresholdSchedule::<T>::hashed_key());
			if let Some(raw) = &threshold_raw {
				let v = u32::decode(&mut &raw[..]).map_err(|_| {
					sp_runtime::TryRuntimeError::Other(
						"score: PersonhoodThreshold does not decode as the pre-v0.3 u32",
					)
				})?;
				ensure!(
					!narrow(v).1,
					"score: PersonhoodThreshold exceeds u8::MAX and would SATURATE — see \
					 migration.rs::v1::narrow"
				);
			}
			if let Some(raw) = &schedule_raw {
				let tiers = OldPersonhoodThresholdTiers::decode(&mut &raw[..]).map_err(|_| {
					sp_runtime::TryRuntimeError::Other(
						"score: PersonhoodThresholdSchedule does not decode as the pre-v0.3 layout",
					)
				})?;
				for t in tiers.iter() {
					ensure!(
						!narrow(t.score_threshold).1,
						"score: a PersonhoodThresholdTier score_threshold exceeds u8::MAX and \
						 would SATURATE — see migration.rs::v1::narrow"
					);
				}
			}

			// An item whose type did NOT change, captured so `post_upgrade` can prove the
			// migration did not reach outside its own three items (e.g. a stray `kill_prefix`).
			let grace_raw = unhashed::get_raw(&AbsenceGraceSchedule::<T>::hashed_key());

			log::info!(
				target: LOG_TARGET,
				"pre_upgrade: {} Participants, PersonhoodThreshold present={}, \
				 PersonhoodThresholdSchedule present={}",
				captured.len(),
				threshold_raw.is_some(),
				schedule_raw.is_some(),
			);

			Ok((captured, threshold_raw, schedule_raw, grace_raw).encode())
		}

		#[cfg(feature = "try-runtime")]
		fn post_upgrade(state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
			use frame_support::storage::unhashed;

			type Captured =
				(Vec<(Vec<u8>, Vec<u8>)>, Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>);
			let (captured, threshold_raw, schedule_raw, grace_raw) =
				<Captured>::decode(&mut &state[..]).map_err(|_| {
					sp_runtime::TryRuntimeError::Other("score: pre_upgrade state failed to decode")
				})?;

			// 1. Nothing was created or destroyed. This migration only overwrites values.
			ensure!(
				old::Participants::<T>::iter_keys().count() == captured.len(),
				"score: the Participants entry count changed"
			);

			for (full_key, old_raw) in &captured {
				let old_value = <OldParticipant<BalanceOf<T>> as codec::DecodeAll>::decode_all(
					&mut &old_raw[..],
				)
				.map_err(|_| {
					sp_runtime::TryRuntimeError::Other(
						"score: captured pre_upgrade bytes no longer decode",
					)
				})?;
				let (expected, _) = convert_participant(old_value);

				let new_raw =
					unhashed::get_raw(full_key).ok_or(sp_runtime::TryRuntimeError::Other(
						"score: a Participants key disappeared during the migration",
					))?;

				// 2. The stored bytes are EXACTLY the re-encoded expectation derived from
				//    `pre_upgrade`'s capture. Byte comparison, not structural: a structural
				//    comparison of the new type against itself would also pass on unmigrated bytes,
				//    because unmigrated bytes decode.
				ensure!(
					new_raw == expected.encode(),
					"score: a migrated Participant is not the re-encoding of the value captured \
					 in pre_upgrade"
				);

				// 3. Six bytes shorter, always, whatever the content. This is the assertion that
				//    fails if the migration silently did nothing: unmigrated bytes decode fine as
				//    the new type, so length is what distinguishes them.
				ensure!(
					new_raw.len() + PARTICIPANT_BYTES_DROPPED == old_raw.len(),
					"score: a migrated Participant is not exactly 6 bytes shorter than the value \
					 captured in pre_upgrade — it was probably not re-encoded at all"
				);

				// 4. The fields the unmigrated read corrupts must be byte-identical to what was
				//    there before. Stated explicitly rather than left implicit in (2), because
				//    these three ARE the bug.
				let stored = <Participant<BalanceOf<T>> as Decode>::decode(&mut &new_raw[..])
					.map_err(|_| {
						sp_runtime::TryRuntimeError::Other(
							"score: a migrated Participant does not decode as the v0.3.1 layout",
						)
					})?;
				let reference = expected;
				ensure!(
					stored.credit == reference.credit,
					"score: credit changed — this is the hold-backed field, do not enact"
				);
				ensure!(
					stored.attendance_history.encode() == reference.attendance_history.encode(),
					"score: attendance_history changed"
				);
				ensure!(
					stored.recognition.encode() == reference.recognition.encode(),
					"score: recognition changed"
				);
			}

			// 5. The two governance-tunables kept their presence, and their narrowed value.
			let now_threshold = unhashed::get_raw(&PersonhoodThreshold::<T>::hashed_key());
			ensure!(
				now_threshold.is_some() == threshold_raw.is_some(),
				"score: PersonhoodThreshold appeared or disappeared"
			);
			if let (Some(before), Some(after)) = (&threshold_raw, &now_threshold) {
				let v = u32::decode(&mut &before[..]).map_err(|_| {
					sp_runtime::TryRuntimeError::Other("score: captured PersonhoodThreshold bytes")
				})?;
				ensure!(
					*after == narrow(v).0.encode(),
					"score: PersonhoodThreshold is not the narrowed pre_upgrade value"
				);
			}

			let now_schedule = unhashed::get_raw(&PersonhoodThresholdSchedule::<T>::hashed_key());
			ensure!(
				now_schedule.is_some() == schedule_raw.is_some(),
				"score: PersonhoodThresholdSchedule appeared or disappeared"
			);
			if let (Some(before), Some(after)) = (&schedule_raw, &now_schedule) {
				let tiers =
					OldPersonhoodThresholdTiers::decode(&mut &before[..]).map_err(|_| {
						sp_runtime::TryRuntimeError::Other(
							"score: captured PersonhoodThresholdSchedule bytes",
						)
					})?;
				let expected: Vec<PersonhoodThresholdTier> = tiers
					.iter()
					.map(|t| PersonhoodThresholdTier {
						population_size_threshold: t.population_size_threshold,
						score_threshold: narrow(t.score_threshold).0,
					})
					.collect();
				let expected: PersonhoodThresholdTiers = BoundedVec::truncate_from(expected);
				ensure!(
					*after == expected.encode(),
					"score: PersonhoodThresholdSchedule is not the narrowed pre_upgrade value"
				);
			}

			// 6. An item this migration must not touch really was not touched. Catches a stray
			//    `kill_prefix` or a mis-scoped alias.
			ensure!(
				unhashed::get_raw(&AbsenceGraceSchedule::<T>::hashed_key()) == grace_raw,
				"score: AbsenceGraceSchedule changed — the migration reached outside its scope"
			);

			log::info!(
				target: LOG_TARGET,
				"post_upgrade: {} Participants verified against the pre_upgrade capture",
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
		mock::{new_test_ext, Test},
		types::{AccountOrPerson, Participant, Recognition, Streak},
		Participants, PersonhoodThreshold, PersonhoodThresholdSchedule,
	};
	use codec::{Decode, DecodeAll, Encode};
	use frame_support::{pallet_prelude::*, traits::OnRuntimeUpgrade};

	/// The exact 31 bytes held by all four live `Score::Participants` entries on people-paseo,
	/// read read-only at block 6,443,886 / spec 2004003. Identical across all four keys.
	const LIVE_PARTICIPANT_HEX: &str =
		"000000000000000000ff000000000000000000000000000000000000000100";

	/// people-paseo's `Balance`. The mock uses `u64`, so the live-bytes tests below work on the
	/// types directly rather than through the mock runtime — exactly as the `credit` field
	/// requires, since the phantom value only appears at `u128` width.
	type LiveBalance = u128;

	fn unhex(s: &str) -> Vec<u8> {
		(0..s.len())
			.step_by(2)
			.map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
			.collect()
	}

	// ===================================================================================
	// 1. The failure this migration prevents, reproduced against production bytes.
	// ===================================================================================

	#[test]
	fn migration_reproduces_the_silent_misdecode() {
		let raw = unhex(LIVE_PARTICIPANT_HEX);
		assert_eq!(raw.len(), 31, "the live value is 31 bytes");

		// (a) Under the PRE-v0.3 layout the value decodes, consuming every byte, into the
		//     values the chain actually means.
		let old = OldParticipant::<LiveBalance>::decode_all(&mut &raw[..])
			.expect("live bytes are the pre-v0.3 layout");
		assert_eq!(old.score, 0);
		assert_eq!(old.streak, OldStreak::Attended(0));
		assert_eq!(old.attendance_history.encode(), vec![0xFF], "attended all 8 recent games");
		assert_eq!(old.credit, 0, "no credit, so nothing is held");
		assert_eq!(old.recognition, Recognition::NotRecognized);
		assert_eq!(old.last_attended_game, None);

		// (b) 🔴 THE BUG. The SAME bytes also decode under the v0.3.1 layout — successfully,
		//     with no error — into different values. `Decode::decode` (what `unhashed::get`
		//     calls) stops after 25 bytes and discards the remaining 6.
		let mut cursor = &raw[..];
		let misread = Participant::<LiveBalance>::decode(&mut cursor)
			.expect("this is the whole problem: it does NOT fail");
		assert_eq!(cursor.len(), 6, "6 bytes left over, silently discarded");
		assert_eq!(
			misread.attendance_history.encode(),
			vec![0x00],
			"0xFF -> 0x00: reads as 'missed all 8 recent games'"
		);
		assert_eq!(
			misread.credit, 280_375_465_082_880,
			"phantom hold-backed credit, ~28,037 PAS at 10 decimals"
		);
		assert_eq!(
			misread.recognition,
			Recognition::ExternallyRecognized,
			"NotRecognized -> ExternallyRecognized: is_recognized() flips to true"
		);
		// And `decode_all` — which storage does NOT use — would have caught it.
		assert!(Participant::<LiveBalance>::decode_all(&mut &raw[..]).is_err());

		// (c) After conversion the stored bytes decode under v0.3.1 into the CORRECT values.
		let (new, saturations) = convert_participant(old);
		assert_eq!(saturations, 0, "no live value saturates");
		let new_raw = new.encode();
		assert_eq!(new_raw.len(), 25, "31 - 6");
		assert_eq!(new_raw.len() + PARTICIPANT_BYTES_DROPPED, raw.len());

		let fixed = Participant::<LiveBalance>::decode_all(&mut &new_raw[..])
			.expect("the migrated value decodes exactly");
		assert_eq!(fixed.attendance_history.encode(), vec![0xFF]);
		assert_eq!(fixed.credit, 0, "the phantom credit is gone");
		assert_eq!(fixed.recognition, Recognition::NotRecognized);
		assert_eq!(fixed.score, 0);
		assert_eq!(fixed.streak, Streak::Attended(0));
	}

	// ===================================================================================
	// 2. The narrowing rule.
	// ===================================================================================

	#[test]
	fn narrow_saturates_and_never_truncates() {
		assert_eq!(narrow(0), (0, false));
		assert_eq!(narrow(21), (21, false));
		assert_eq!(narrow(255), (255, false));
		assert_eq!(narrow(256), (255, true), "saturates, and says so");
		assert_eq!(narrow(u32::MAX), (255, true));
		// The rejected alternative, pinned so a future edit to `narrow` shows up here: `as u8`
		// would map 256 to 0, which silently REVOKES personhood instead of preserving it.
		assert_eq!(256u32 as u8, 0);
		assert_ne!(narrow(256).0, 256u32 as u8);
	}

	#[test]
	fn saturation_is_reported_per_field() {
		let p = OldParticipant::<LiveBalance> {
			score: 1_000,
			streak: OldStreak::Absent(70_000),
			attendance_history: Default::default(),
			credit: 7,
			cashed_out: false,
			reached_personhood: true,
			has_ever_reached_personhood: true,
			recognition: Recognition::NotRecognized,
			last_attended_game: Some(9),
		};
		let (new, saturations) = convert_participant(p);
		assert_eq!(saturations, 2, "score and streak both saturated");
		assert_eq!(new.score, u8::MAX);
		assert_eq!(new.streak, Streak::Absent(u8::MAX));
		// Everything else is carried verbatim.
		assert_eq!(new.credit, 7);
		assert!(new.reached_personhood);
		assert_eq!(new.last_attended_game, Some(9));
	}

	#[test]
	fn conversion_preserves_every_unnarrowed_field() {
		for (history_byte, recognition) in [
			(0xFFu8, Recognition::NotRecognized),
			(0x00, Recognition::ExternallyRecognized),
			(0xA5, Recognition::Suspended(3)),
			(0x01, Recognition::Recognized(u64::MAX)),
		] {
			let history =
				crate::types::AttendanceHistory::decode(&mut &[history_byte][..]).unwrap();
			let old = OldParticipant::<LiveBalance> {
				score: 21,
				streak: OldStreak::Attended(5),
				attendance_history: history,
				credit: u128::MAX,
				cashed_out: true,
				reached_personhood: false,
				has_ever_reached_personhood: true,
				recognition,
				last_attended_game: None,
			};
			let recognition_bytes = old.recognition.encode();
			let (new, saturations) = convert_participant(old);
			assert_eq!(saturations, 0);
			assert_eq!(new.score, 21);
			assert_eq!(new.streak, Streak::Attended(5));
			assert_eq!(new.attendance_history.encode(), vec![history_byte]);
			assert_eq!(new.credit, u128::MAX);
			assert!(new.cashed_out);
			assert!(!new.reached_personhood);
			assert!(new.has_ever_reached_personhood);
			assert_eq!(new.recognition.encode(), recognition_bytes);
		}
	}

	// ===================================================================================
	// 3. End-to-end through `VersionedMigration`, in the mock runtime.
	// ===================================================================================

	fn old_zero_participant() -> OldParticipant<u64> {
		OldParticipant {
			score: 0,
			streak: OldStreak::Attended(0),
			attendance_history: Default::default(),
			credit: 0,
			cashed_out: false,
			reached_personhood: false,
			has_ever_reached_personhood: false,
			recognition: Recognition::NotRecognized,
			last_attended_game: None,
		}
	}

	#[test]
	fn migration_reencodes_participants_in_place() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<Test>>();

			let a = AccountOrPerson::Account(1u64);
			let b = AccountOrPerson::Account(2u64);
			old::Participants::<Test>::insert(&a, old_zero_participant());
			old::Participants::<Test>::insert(
				&b,
				OldParticipant {
					score: 21,
					streak: OldStreak::Absent(3),
					credit: 12_345,
					cashed_out: true,
					recognition: Recognition::Recognized(77),
					last_attended_game: Some(4),
					..old_zero_participant()
				},
			);
			let key_a = old::Participants::<Test>::hashed_key_for(&a);
			let before_a =
				frame_support::storage::unhashed::get_raw(&key_a).expect("value was written");

			MigrateV0ToV1::<Test>::on_runtime_upgrade();

			assert_eq!(StorageVersion::get::<crate::Pallet<Test>>(), 1);
			// Keys unchanged, count unchanged.
			assert_eq!(Participants::<Test>::iter_keys().count(), 2);

			let after_a = frame_support::storage::unhashed::get_raw(&key_a).unwrap();
			assert_eq!(after_a.len() + PARTICIPANT_BYTES_DROPPED, before_a.len());

			let pa = Participants::<Test>::get(&a).unwrap();
			assert_eq!(pa.score, 0);
			assert_eq!(pa.streak, Streak::Attended(0));
			assert_eq!(pa.attendance_history.encode(), vec![0xFF]);
			assert_eq!(pa.credit, 0);
			assert_eq!(pa.recognition, Recognition::NotRecognized);

			let pb = Participants::<Test>::get(&b).unwrap();
			assert_eq!(pb.score, 21);
			assert_eq!(pb.streak, Streak::Absent(3));
			assert_eq!(pb.credit, 12_345);
			assert!(pb.cashed_out);
			assert_eq!(pb.recognition, Recognition::Recognized(77));
			assert_eq!(pb.last_attended_game, Some(4));
		});
	}

	#[test]
	fn migration_narrows_the_governance_tunables_only_when_they_exist() {
		// Absent: must stay absent. Writing a defaulted value would turn an invisible tunable
		// into a stored one, which `state_getStorage` consumers can observe.
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<Test>>();
			// The mock seeds `PersonhoodThreshold` at genesis; the live chain does not, so kill
			// it to reproduce people-paseo, where both keys are ABSENT.
			PersonhoodThreshold::<Test>::kill();
			assert!(!old::PersonhoodThreshold::<Test>::exists());
			assert!(!old::PersonhoodThresholdSchedule::<Test>::exists());

			MigrateV0ToV1::<Test>::on_runtime_upgrade();

			assert!(!PersonhoodThreshold::<Test>::exists(), "absent must stay absent");
			assert!(!PersonhoodThresholdSchedule::<Test>::exists());
		});

		// Present: must be narrowed in place.
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<Test>>();
			PersonhoodThreshold::<Test>::kill();
			old::PersonhoodThreshold::<Test>::put(17u32);
			old::PersonhoodThresholdSchedule::<Test>::put(
				OldPersonhoodThresholdTiers::truncate_from(vec![
					OldPersonhoodThresholdTier {
						population_size_threshold: 5_000,
						score_threshold: 1,
					},
					OldPersonhoodThresholdTier {
						population_size_threshold: 10_000,
						score_threshold: 3,
					},
				]),
			);

			MigrateV0ToV1::<Test>::on_runtime_upgrade();

			assert_eq!(PersonhoodThreshold::<Test>::get(), 17);
			let s = PersonhoodThresholdSchedule::<Test>::get();
			assert_eq!(s.len(), 2);
			assert_eq!(s[0].population_size_threshold, 5_000);
			assert_eq!(s[0].score_threshold, 1);
			assert_eq!(s[1].population_size_threshold, 10_000);
			assert_eq!(s[1].score_threshold, 3);
		});
	}

	#[test]
	fn a_two_tier_schedule_would_be_corrupted_without_the_migration() {
		// The single-tier case survives an unmigrated read by luck (trailing bytes discarded);
		// two tiers do not, because each element shrinks by 3 bytes and the second element
		// starts at the wrong offset. This is why the schedule is migrated rather than left.
		let old = OldPersonhoodThresholdTiers::truncate_from(vec![
			OldPersonhoodThresholdTier { population_size_threshold: 5_000, score_threshold: 1 },
			OldPersonhoodThresholdTier { population_size_threshold: 10_000, score_threshold: 3 },
		]);
		let raw = old.encode();
		let misread = crate::types::PersonhoodThresholdTiers::decode(&mut &raw[..])
			.expect("it decodes; that is the problem");
		assert_eq!(misread.len(), 2);
		assert_eq!(misread[0].population_size_threshold, 5_000);
		assert_eq!(misread[0].score_threshold, 1);
		// The second tier is garbage: it picks up the first tier's trailing zero bytes.
		assert_ne!(misread[1].population_size_threshold, 10_000);
	}

	#[test]
	fn migration_is_a_no_op_when_already_at_version_one() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(1).put::<crate::Pallet<Test>>();
			let a = AccountOrPerson::Account(1u64);
			// A value already in the NEW shape.
			Participants::<Test>::insert(
				&a,
				Participant::<u64> {
					score: 9,
					streak: Streak::Attended(2),
					attendance_history: Default::default(),
					credit: 5,
					cashed_out: false,
					reached_personhood: true,
					has_ever_reached_personhood: true,
					recognition: Recognition::NotRecognized,
					last_attended_game: Some(1),
				},
			);
			let before = frame_support::storage::unhashed::get_raw(
				&Participants::<Test>::hashed_key_for(&a),
			)
			.unwrap();

			MigrateV0ToV1::<Test>::on_runtime_upgrade();

			let after = frame_support::storage::unhashed::get_raw(
				&Participants::<Test>::hashed_key_for(&a),
			)
			.unwrap();
			assert_eq!(before, after, "a second run must not re-narrow an already-narrow value");
		});
	}

	// ===================================================================================
	// 4. The try-runtime hooks, actually executed (not merely compiled).
	// ===================================================================================

	#[cfg(feature = "try-runtime")]
	#[test]
	fn pre_and_post_upgrade_agree_on_a_realistic_state() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<Test>>();
			PersonhoodThreshold::<Test>::kill();
			for who in 1u64..=4 {
				old::Participants::<Test>::insert(
					&AccountOrPerson::Account(who),
					old_zero_participant(),
				);
			}

			let state = MigrateV0ToV1::<Test>::pre_upgrade().expect("pre_upgrade");
			MigrateV0ToV1::<Test>::on_runtime_upgrade();
			MigrateV0ToV1::<Test>::post_upgrade(state).expect("post_upgrade");
		});
	}

	#[cfg(feature = "try-runtime")]
	#[test]
	fn pre_upgrade_refuses_a_value_that_would_saturate() {
		// 🔴 The narrowing rule is a human decision. If production data ever needs it, the
		// try-runtime run must stop rather than apply a default nobody signed off on.
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<Test>>();
			PersonhoodThreshold::<Test>::kill();
			old::Participants::<Test>::insert(
				&AccountOrPerson::Account(1u64),
				OldParticipant { score: 300, ..old_zero_participant() },
			);

			assert!(MigrateV0ToV1::<Test>::pre_upgrade().is_err());
		});

		// ...and the same for a saturating streak.
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<Test>>();
			PersonhoodThreshold::<Test>::kill();
			old::Participants::<Test>::insert(
				&AccountOrPerson::Account(1u64),
				OldParticipant { streak: OldStreak::Absent(9_000), ..old_zero_participant() },
			);

			assert!(MigrateV0ToV1::<Test>::pre_upgrade().is_err());
		});
	}

	#[cfg(feature = "try-runtime")]
	#[test]
	fn post_upgrade_catches_a_value_the_migration_failed_to_reencode() {
		// Proves the post-check is load-bearing. Unmigrated bytes DECODE as the new type, so a
		// naive structural check would pass here; the length and byte-equality checks do not.
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<Test>>();
			PersonhoodThreshold::<Test>::kill();
			old::Participants::<Test>::insert(
				&AccountOrPerson::Account(1u64),
				old_zero_participant(),
			);
			let state = MigrateV0ToV1::<Test>::pre_upgrade().expect("pre_upgrade");

			// Deliberately do NOT run the migration.
			assert!(
				MigrateV0ToV1::<Test>::post_upgrade(state).is_err(),
				"post_upgrade must not pass on an unmigrated Participant"
			);
		});
	}

	#[test]
	fn migration_writes_nothing_when_a_participant_cannot_be_decoded() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<crate::Pallet<Test>>();
			let a = AccountOrPerson::Account(1u64);
			let b = AccountOrPerson::Account(2u64);
			old::Participants::<Test>::insert(&a, old_zero_participant());
			// A truncated value at a well-formed key: cannot decode as either layout.
			frame_support::storage::unhashed::put_raw(
				&old::Participants::<Test>::hashed_key_for(&b),
				&[0x00, 0x01],
			);
			let before_a = frame_support::storage::unhashed::get_raw(
				&old::Participants::<Test>::hashed_key_for(&a),
			)
			.unwrap();

			MigrateV0ToV1::<Test>::on_runtime_upgrade();

			// All-or-nothing: the abort happens in phase 1, before any write, so the GOOD entry
			// is left in the pre-v0.3 encoding too.
			let after_a = frame_support::storage::unhashed::get_raw(
				&old::Participants::<Test>::hashed_key_for(&a),
			)
			.unwrap();
			assert_eq!(before_a, after_a, "nothing was written");
		});
	}
}
