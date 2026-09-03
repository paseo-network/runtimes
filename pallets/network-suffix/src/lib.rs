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

//! Stores the network suffix used to derive product contexts.
//!
//! This pallet is intended for test networks and must not be used in production.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
// PASEO-LOCAL: no upstream counterpart. See the module docs for why a `ValueQuery` default is
// not enough on a chain that gains this pallet by runtime upgrade.
pub mod migration;
pub mod weights;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub use pallet::*;
pub use weights::WeightInfo;

use frame_support::{pallet_prelude::*, traits::Get};
use indiv_support::context::ProductContextNetworkSuffix;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_system::pallet_prelude::*;

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config<RuntimeEvent: From<Event<Self>>> {
		/// Origin allowed to update the suffix.
		type UpdateOrigin: EnsureOrigin<Self::RuntimeOrigin>;

		/// Suffix used when storage has not been initialized, including runtime upgrades from a
		/// version without this pallet.
		type DefaultSuffix: Get<ProductContextNetworkSuffix>;

		/// Weight information for this pallet.
		type WeightInfo: WeightInfo;
	}

	#[pallet::type_value]
	pub fn DefaultNetworkSuffix<T: Config>() -> ProductContextNetworkSuffix {
		T::DefaultSuffix::get()
	}

	/// Network suffix appended to product names when deriving product contexts.
	#[pallet::storage]
	#[pallet::getter(fn network_suffix)]
	pub type NetworkSuffix<T: Config> =
		StorageValue<_, ProductContextNetworkSuffix, ValueQuery, DefaultNetworkSuffix<T>>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// The network suffix changed.
		NetworkSuffixSet { old: ProductContextNetworkSuffix, new: ProductContextNetworkSuffix },
	}

	#[pallet::error]
	pub enum Error<T> {
		/// A network suffix cannot be empty.
		EmptySuffix,
	}

	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config> {
		pub network_suffix: ProductContextNetworkSuffix,
		#[serde(skip)]
		pub _phantom: PhantomData<T>,
	}

	impl<T: Config> Default for GenesisConfig<T> {
		fn default() -> Self {
			Self { network_suffix: T::DefaultSuffix::get(), _phantom: PhantomData }
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
		fn build(&self) {
			assert!(!self.network_suffix.is_empty(), "network suffix cannot be empty at genesis");
			NetworkSuffix::<T>::put(&self.network_suffix);
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Set the network suffix used by all product-context derivations.
		#[pallet::call_index(0)]
		#[pallet::weight(T::WeightInfo::set_network_suffix(network_suffix.len() as u32))]
		pub fn set_network_suffix(
			origin: OriginFor<T>,
			network_suffix: ProductContextNetworkSuffix,
		) -> DispatchResult {
			T::UpdateOrigin::ensure_origin(origin)?;
			ensure!(!network_suffix.is_empty(), Error::<T>::EmptySuffix);

			let old = NetworkSuffix::<T>::get();
			NetworkSuffix::<T>::put(&network_suffix);
			Self::deposit_event(Event::NetworkSuffixSet { old, new: network_suffix });
			Ok(())
		}
	}
}

impl<T: Config> Get<ProductContextNetworkSuffix> for Pallet<T> {
	fn get() -> ProductContextNetworkSuffix {
		NetworkSuffix::<T>::get()
	}
}
