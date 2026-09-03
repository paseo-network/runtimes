// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Individuality.
// SPDX-License-Identifier: Apache-2.0
//
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

//! PASEO-LOCAL. This module has no upstream counterpart in `individuality-community`.
//!
//! Upstream never needs it: its chains carry this pallet from genesis, where
//! [`GenesisConfig::build`](crate::pallet::GenesisConfig) writes [`NetworkSuffix`] into state.
//! Paseo adds the pallet by runtime upgrade to chains that are already live, so the value is
//! only ever produced by the `ValueQuery` default — and a `ValueQuery` default lives in the
//! runtime, not in state.
//!
//! That distinction is invisible to a Rust caller and fatal to an RPC caller:
//! `state_getStorage` over the `NetworkSuffix::NetworkSuffix` key returns `null`, because
//! nothing was ever written. The Android client reads that key directly and throws on `null`;
//! it does not (and cannot) know the runtime's default. [`SeedNetworkSuffix`] therefore writes
//! the default into state on the upgrade that introduces the pallet, making the on-chain value
//! observable to every client, not just to on-chain code.
//!
//! # The suffix value is `dot`, not `.dot` — do not "fix" this
//!
//! Whatever a runtime binds to [`Config::DefaultSuffix`] must NOT carry a leading dot.
//! [`indiv_support::context::build_product_context`] pushes `b'.'` itself and then splices the
//! suffix verbatim, so the separator is already accounted for. The three clients disagree about
//! the rest:
//!
//! * Rust (this runtime) — emits `<product>` `.` `<suffix>`.
//! * Android — concatenates the fetched suffix without stripping a leading dot.
//! * iOS — strips one leading dot from the fetched suffix before concatenating.
//!
//! With `dot` all three agree on `peopl.dot`. With `.dot`, Rust and Android would agree on
//! `peopl..dot` while iOS alone produced `peopl.dot` — a silent, total failure of every alias
//! on iOS with two of the three implementations "agreeing" and therefore looking correct.
//! This is a settled decision, recorded here because the failure mode is asymmetric and quiet.

use super::*;
use frame_support::{
	traits::{Get, OnRuntimeUpgrade},
	weights::Weight,
};

/// Write [`Config::DefaultSuffix`] into [`NetworkSuffix`] if, and only if, the key is absent
/// from state.
///
/// Idempotent, and safe to leave in a runtime's migration tuple across later upgrades: once the
/// key exists — whether written here, at genesis, or by
/// [`set_network_suffix`](crate::pallet::Pallet::set_network_suffix) — this is a single read and
/// nothing else. In particular it will never clobber a suffix that governance has changed.
pub struct SeedNetworkSuffix<T>(core::marker::PhantomData<T>);

impl<T: Config> OnRuntimeUpgrade for SeedNetworkSuffix<T> {
	fn on_runtime_upgrade() -> Weight {
		if NetworkSuffix::<T>::exists() {
			return T::DbWeight::get().reads(1);
		}

		NetworkSuffix::<T>::put(T::DefaultSuffix::get());
		T::DbWeight::get().reads_writes(1, 1)
	}

	#[cfg(feature = "try-runtime")]
	fn pre_upgrade() -> Result<alloc::vec::Vec<u8>, frame_support::sp_runtime::TryRuntimeError> {
		use codec::Encode;
		Ok(NetworkSuffix::<T>::exists().encode())
	}

	#[cfg(feature = "try-runtime")]
	fn post_upgrade(
		state: alloc::vec::Vec<u8>,
	) -> Result<(), frame_support::sp_runtime::TryRuntimeError> {
		use codec::Decode;

		let existed = bool::decode(&mut &state[..]).map_err(|_| {
			frame_support::sp_runtime::TryRuntimeError::Other("failed to decode pre_upgrade state")
		})?;

		frame_support::ensure!(
			NetworkSuffix::<T>::exists(),
			"SeedNetworkSuffix: the network suffix key is still absent from state"
		);

		let suffix = NetworkSuffix::<T>::get();
		frame_support::ensure!(
			!suffix.is_empty(),
			"SeedNetworkSuffix: the stored network suffix is empty"
		);
		frame_support::ensure!(
			suffix.first() != Some(&b'.'),
			"SeedNetworkSuffix: the network suffix must not start with a dot; the context \
			 builder supplies the separator itself"
		);

		if !existed {
			frame_support::ensure!(
				suffix == T::DefaultSuffix::get(),
				"SeedNetworkSuffix: the seeded suffix does not match the configured default"
			);
		}

		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	// Imported by name, not by glob: the mock's `construct_runtime!` also defines a
	// `NetworkSuffix` (the pallet alias), which would shadow the storage item ambiguously.
	use crate::mock::{new_test_ext, new_test_ext_with_suffix, Test};

	#[test]
	fn seeds_the_default_when_the_key_is_absent() {
		new_test_ext().execute_with(|| {
			// Reproduce a chain that upgraded into this pallet: the `ValueQuery` default
			// answers on-chain reads, but nothing was ever written to state.
			NetworkSuffix::<Test>::kill();
			assert!(!NetworkSuffix::<Test>::exists());
			assert_eq!(NetworkSuffix::<Test>::get().as_slice(), b"paseo");

			SeedNetworkSuffix::<Test>::on_runtime_upgrade();

			assert!(NetworkSuffix::<Test>::exists());
			assert_eq!(NetworkSuffix::<Test>::get().as_slice(), b"paseo");
		});
	}

	#[test]
	fn leaves_an_existing_suffix_untouched() {
		new_test_ext_with_suffix(b"test").execute_with(|| {
			SeedNetworkSuffix::<Test>::on_runtime_upgrade();

			assert_eq!(NetworkSuffix::<Test>::get().as_slice(), b"test");
		});
	}

	#[test]
	fn is_idempotent() {
		new_test_ext().execute_with(|| {
			NetworkSuffix::<Test>::kill();

			SeedNetworkSuffix::<Test>::on_runtime_upgrade();
			SeedNetworkSuffix::<Test>::on_runtime_upgrade();

			assert_eq!(NetworkSuffix::<Test>::get().as_slice(), b"paseo");
		});
	}
}
