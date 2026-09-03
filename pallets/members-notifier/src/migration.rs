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

//! Storage migrations for the members notifier pallet.

use crate::{Config, GenesisWhitelistEntry, Pallet, SubscriptionWhitelist, LOG_TARGET};
use alloc::vec::Vec;
use core::marker::PhantomData;
use frame_support::{
	traits::{Get, OnRuntimeUpgrade},
	weights::Weight,
};

/// Seeds [`SubscriptionWhitelist`]
///
/// Every entry in `Entries` is written unconditionally, overwriting whatever is in storage.
/// Malformed entries are logged and skipped.
///
/// # Single use!
///
/// Remove this from the runtime's `Migrations` tuple once the upgrade carrying it is live.
pub struct SeedSubscriptionWhitelist<T, Entries>(PhantomData<(T, Entries)>);

impl<T: Config, Entries: Get<Vec<GenesisWhitelistEntry>>> OnRuntimeUpgrade
	for SeedSubscriptionWhitelist<T, Entries>
{
	fn on_runtime_upgrade() -> Weight {
		let mut writes: u64 = 0;

		for entry in Entries::get() {
			let para_id = entry.para_id;

			let subscription = match Pallet::<T>::resolve_whitelist_entry(&entry) {
				Ok(subscription) => subscription,
				Err(error) => {
					// Skipped rather than brick the upgrade. Gov can always subscribe
					log::error!(
						target: LOG_TARGET,
						"skipping malformed whitelist entry for {para_id:?}: {error:?}",
					);
					continue;
				},
			};

			SubscriptionWhitelist::<T>::insert(para_id, subscription);
			writes = writes.saturating_add(1);

			log::info!(target: LOG_TARGET, "whitelisted {para_id:?}");
		}

		T::DbWeight::get().reads_writes(0, writes)
	}

	#[cfg(feature = "try-runtime")]
	fn post_upgrade(_state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
		// All entries must be present and match their source.
		for entry in Entries::get() {
			let stored = SubscriptionWhitelist::<T>::get(entry.para_id)
				.ok_or("members-notifier: entry was not whitelisted")?;
			let expected = Pallet::<T>::resolve_whitelist_entry(&entry)
				.map_err(|_| "members-notifier: malformed source entry")?;
			frame_support::ensure!(
				stored == expected,
				"members-notifier: whitelisted subscription does not match the source entry"
			);
		}

		Ok(())
	}
}
