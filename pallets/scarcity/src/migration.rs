// This file is part of Substrate.

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

//! Storage migrations for the Scarcity pallet.

use crate::{BalanceOf, Config, ItemDefinition, ItemDefs, Pallet, Transferability};
use frame_support::{
	migrations::VersionedMigration, pallet_prelude::*, traits::UncheckedOnRuntimeUpgrade,
};
use sp_runtime::Saturating;

const LOG_TARGET: &str = "runtime::pallet-scarcity::migration";

/// Adds [`Transferability`] to every stored item definition, as [`Transferability::Transferable`].
///
/// `transferability` is a trailing field, so a definition written before it existed decodes to
/// `None` under the current type rather than failing loudly. Every read of that item then reports
/// `UnknownItem`, which takes minting, both transfer paths, burning and deletion with it, and a
/// collection whose items cannot be deleted cannot be deleted either, so its deposit stays held.
pub type MigrateV0ToV1<T> = VersionedMigration<
	0,
	1,
	v1::MigrateToTransferability<T>,
	Pallet<T>,
	<T as frame_system::Config>::DbWeight,
>;

pub mod v1 {
	use super::*;

	/// An item definition as stored before item transferability existed.
	#[derive(Decode)]
	pub struct OldItemDefinition<Balance> {
		pub supply: u32,
		pub live_supply: u32,
		pub metadata_count: u32,
		pub deposit: Balance,
	}

	/// Use [`MigrateV0ToV1`](super::MigrateV0ToV1) rather than this directly.
	///
	/// Running this twice would reset a soulbound item to transferable: the new encoding is the
	/// old one plus a trailing byte, and `Decode` reads a prefix, so a migrated definition still
	/// decodes as [`OldItemDefinition`] with the flag left over and ignored. The version gate is
	/// what makes that unreachable.
	pub struct MigrateToTransferability<T>(PhantomData<T>);

	impl<T: Config> UncheckedOnRuntimeUpgrade for MigrateToTransferability<T> {
		fn on_runtime_upgrade() -> Weight {
			let mut translated = 0u64;
			ItemDefs::<T>::translate_values(|old: OldItemDefinition<BalanceOf<T>>| {
				translated.saturating_inc();
				Some(ItemDefinition {
					supply: old.supply,
					live_supply: old.live_supply,
					metadata_count: old.metadata_count,
					// Deliberately the stored value, not a recomputed one. The new field widens
					// the definition's `MaxEncodedLen` by a byte, but deposits are released from
					// what was charged, and the collection's aggregate is checked against the
					// same figures, so recomputing here would desync both.
					deposit: old.deposit,
					// The only value that preserves behaviour: nothing was bindable before.
					transferability: Transferability::Transferable,
				})
			});
			log::info!(target: LOG_TARGET, "translated {translated} item definitions");
			T::DbWeight::get().reads_writes(translated.saturating_add(1), translated)
		}

		#[cfg(feature = "try-runtime")]
		fn pre_upgrade() -> Result<alloc::vec::Vec<u8>, sp_runtime::TryRuntimeError> {
			// Keys, not entries: the values do not decode under the current type yet, and
			// `iter` would skip every one of them and report an empty map.
			Ok((ItemDefs::<T>::iter_keys().count() as u32).encode())
		}

		#[cfg(feature = "try-runtime")]
		fn post_upgrade(state: alloc::vec::Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
			let before = u32::decode(&mut &state[..]).map_err(|_| {
				sp_runtime::TryRuntimeError::Other("pre_upgrade state is not a u32")
			})?;
			// `iter` now, so an entry that failed to translate is counted as missing rather than
			// silently skipped.
			let after = ItemDefs::<T>::iter().count() as u32;
			ensure!(before == after, "an item definition did not survive the migration");
			Ok(())
		}
	}
}
