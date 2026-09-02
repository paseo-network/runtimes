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

//! Storage migrations for the dotNS gateway pallet.

use crate::{AccountNameRecord, AccountNames, Config, LiteLabelOwner, NameEntry, Pallet};
use frame_support::{
	migrations::VersionedMigration, pallet_prelude::*, traits::UncheckedOnRuntimeUpgrade,
};
use sp_runtime::Saturating;

const LOG_TARGET: &str = "runtime::indiv-pallet-dotns-gateway::migration";

pub type MigrateV0ToV1<T> = VersionedMigration<
	0,
	1,
	v1::BackfillAccountNames<T>,
	Pallet<T>,
	<T as frame_system::Config>::DbWeight,
>;

pub mod v1 {
	use super::*;
	use crate::BaseLabel;

	/// An account name record as stored before chat keys existed.
	#[derive(Decode)]
	pub struct OldAccountNameRecord {
		pub lite: Option<BaseLabel>,
		pub full: Option<BaseLabel>,
	}

	fn entry_without_chat(label: BaseLabel) -> NameEntry {
		NameEntry { label, chat: None }
	}

	/// Use [`super::MigrateV0ToV1`] rather than this directly.
	///
	/// Translates any record stored in the two-field shape, then fills [`AccountNames`] from
	/// [`LiteLabelOwner`] for lite labels registered before the map existed. Only the label is
	/// recoverable from pallet storage: the chat key stays `None` for these accounts. Full
	/// labels need no backfill: no full registration happened on any deployment before the map
	/// existed.
	///
	/// A backfilled entry carries no ordering guarantee. [`Pallet::reserve_name`] overwrites
	/// `lite` on every reservation, so a live record holds the most recent label, while the
	/// backfill takes whichever label storage iteration yields first. Two accounts with the same
	/// history can therefore show different labels, depending on whether the entry was
	/// backfilled or written after the upgrade.
	pub struct BackfillAccountNames<T>(PhantomData<T>);

	impl<T: Config> UncheckedOnRuntimeUpgrade for BackfillAccountNames<T> {
		fn on_runtime_upgrade() -> Weight {
			let mut translated = 0u64;
			AccountNames::<T>::translate_values(|old: OldAccountNameRecord| {
				translated.saturating_inc();
				Some(AccountNameRecord {
					lite: old.lite.map(entry_without_chat),
					full: old.full.map(entry_without_chat),
				})
			});

			let mut reads = 1u64;
			let mut writes = 0u64;
			for (label, owner) in LiteLabelOwner::<T>::iter() {
				// One read for the `LiteLabelOwner` entry, one for the record.
				reads = reads.saturating_add(2);
				let mut record = AccountNames::<T>::get(&owner).unwrap_or_default();
				if record.lite.is_none() {
					record.lite = Some(entry_without_chat(label));
					AccountNames::<T>::insert(&owner, record);
					writes.saturating_inc();
				}
			}
			log::info!(
				target: LOG_TARGET,
				"translated {translated} records, backfilled {writes} lite labels into AccountNames"
			);
			T::DbWeight::get().reads_writes(
				reads.saturating_add(translated.saturating_add(1)),
				writes.saturating_add(translated),
			)
		}

		#[cfg(feature = "try-runtime")]
		fn pre_upgrade() -> Result<alloc::vec::Vec<u8>, sp_runtime::TryRuntimeError> {
			// Every account that holds a record now, plus every account the backfill adds one
			// for. Keys, not entries: a two-field record does not decode under the current type,
			// so `iter` would skip it and report it as missing.
			let mut accounts = alloc::collections::BTreeSet::new();
			for account in AccountNames::<T>::iter_keys() {
				accounts.insert(account);
			}
			for (_, owner) in LiteLabelOwner::<T>::iter() {
				accounts.insert(owner);
			}
			let owners = LiteLabelOwner::<T>::iter().count() as u32;
			Ok((accounts.len() as u32, owners).encode())
		}

		#[cfg(feature = "try-runtime")]
		fn post_upgrade(state: alloc::vec::Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
			let (accounts, owners) = <(u32, u32)>::decode(&mut &state[..]).map_err(|_| {
				sp_runtime::TryRuntimeError::Other("pre_upgrade state is not (u32, u32)")
			})?;
			// `iter` now, so a record that failed to translate counts as missing rather than
			// being skipped.
			ensure!(
				AccountNames::<T>::iter().count() as u32 == accounts,
				"a record did not survive the migration"
			);
			ensure!(
				LiteLabelOwner::<T>::iter().count() as u32 == owners,
				"LiteLabelOwner changed during the migration"
			);
			for (_, owner) in LiteLabelOwner::<T>::iter() {
				ensure!(
					AccountNames::<T>::get(&owner).is_some_and(|record| record.lite.is_some()),
					"a lite label owner has no AccountNames record"
				);
			}
			Ok(())
		}
	}
}
