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

//! Storage migrations for the PGAS pallet.

extern crate alloc;

use crate::{Config, Pallet, WeightInfo};
use frame_support::{
	pallet_prelude::*,
	traits::{fungibles, OnRuntimeUpgrade},
};
use fungibles::Inspect as _;

const LOG_TARGET: &str = "runtime::indiv-pallet-pgas::migration";

/// One-shot migration that creates the PGAS asset under `T::PgasAdmin` if it does not
/// already exist. Idempotent: re-running is a no-op.
pub struct CreatePgasAsset<T>(PhantomData<T>);

impl<T: Config> OnRuntimeUpgrade for CreatePgasAsset<T> {
	fn on_runtime_upgrade() -> Weight {
		let exists_check = <T as Config>::WeightInfo::authorize_create_pgas_asset();
		if T::Fungibles::asset_exists(T::PgasAssetId::get()) {
			log::info!(target: LOG_TARGET, "PGAS asset already exists; skipping.");
			return exists_check;
		}

		match Pallet::<T>::do_create_pgas_asset() {
			Ok(()) => log::info!(target: LOG_TARGET, "PGAS asset created."),
			Err(e) => log::error!(target: LOG_TARGET, "failed to create PGAS asset: {e:?}"),
		}

		exists_check.saturating_add(<T as Config>::WeightInfo::create_pgas_asset())
	}

	#[cfg(feature = "try-runtime")]
	fn post_upgrade(_state: alloc::vec::Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
		ensure!(
			T::Fungibles::asset_exists(T::PgasAssetId::get()),
			"PGAS asset must exist after migration"
		);
		Ok(())
	}
}
