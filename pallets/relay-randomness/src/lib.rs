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

//! # Relay Randomness Pallet
//!
//! This pallet stores relay chain randomness read from the relay chain state proof.
//!
//! It hooks into `cumulus_pallet_parachain_system` via
//! [`OnSystemEvent::on_relay_state_proof`](cumulus_pallet_parachain_system::OnSystemEvent) and
//! retrieves the relay chain randomness from the validation data: `CURRENT_BLOCK_RANDOMNESS`
//! and `ONE_EPOCH_AGO_RANDOMNESS`.
//!
//! The relay chain also exposes `TWO_EPOCHS_AGO_RANDOMNESS`, it is not stored: it is the
//! previous one-epoch-ago value, so it never offers fresher randomness than the one-epoch-ago
//! entry.
//!
//! Each stored value carries a `moment`: the relay chain block number by which the value was
//! knowable by everybody. Consumers compare it against a moment they committed to earlier and
//! accept the value only once its moment is greater, which guarantees the randomness could not
//! have been known when they committed.
//!
//! The moment is derived from the first relay parent whose state served the value:
//!
//! - Per-block randomness: that relay parent's block number minus the chain's relay parent offset
//!   ([`cumulus_pallet_parachain_system::Config::RelayParentOffset`]), because the value was
//!   already public for that many blocks before the parachain observed it.
//! - One-epoch-ago randomness: one block earlier still. It was already fully determined by the
//!   relay parent preceding the first one to serve it, since its last input is the final block VRF
//!   of the previous epoch.
//!
//!
//! So consumers can require randomness produced after a commitment of theirs.
//!
//! [`RelayBlockRandomness`] and [`RelayOneEpochAgoRandomness`] expose the stored values
//! through [`indiv_support::traits::MomentRandomness`].
//!
//! # Example
//!
//! A relay chain with epoch length 4.
//! A parachain with relay parent offset 2, and 1 parachain block per slot.
//!
//! `vN` is the block VRF of relay block `RN`, publicly known when `RN` is published. `eX` is
//! the epoch randomness accumulated from the block VRFs of epoch `X`, determinable by everybody
//! once the last block of epoch `X` is published and served by the relay chain as the
//! one-epoch-ago randomness during epoch `X+1`. The parachain rows show the pallet storage
//! after each parachain block.
//!
//! ```text
//!                     ┌────── epoch 5 ─────┬────── epoch 6 ─────┬────── epoch 7 ─────┐
//! time                │t1   t2   t3   t4   │t5   t6   t7   t8   │t9   t10  t11  t12  │
//!                     ├────────────────────┼────────────────────┼────────────────────┤
//! relay chain         │R10  R11  R12  R13  │R14  R15  R16  R17  │R18  R19  R20  R21  │
//!   block VRF         │v10  v11  v12  v13  │v14  v15  v16  v17  │v18  v19  v20  v21  │
//!   1-epoch-ago       │e4   e4   e4   e4   │e5   e5   e5   e5   │e6   e6   e6   e6   │
//!                     ├────────────────────┼────────────────────┼────────────────────┤
//! parachain           │P8   P9   P10  P11  │P12  P13  P14  P15  │P16  P17  P18  P19  │
//!   relay parent      │R8   R9   R10  R11  │R12  R13  R14  R15  │R16  R17  R18  R19  │
//!   block VRF         │v8   v9   v10  v11  │v12  v13  v14  v15  │v16  v17  v18  v19  │
//!   block VRF moment  │R6   R7   R8   R9   │R10  R11  R12  R13  │R14  R15  R16  R17  │
//!   1-epoch-ago       │e3   e3   e4   e4   │e4   e4   e5   e5   │e5   e5   e6   e6   │
//!   1-epoch-ago moment│R3   R3   R7   R7   │R7   R7   R11  R11  │R11  R11  R15  R15  │
//!                     └────────────────────┴────────────────────┴────────────────────┘
//! ```
//!
//! Scenario:
//!
//! * Alice registers a key in P13 (relay parent R13). The chain records the current moment: 13. P13
//!   is authored at t6, when R15 is already public, so Alice may already know the block VRFs up to
//!   `v15` and the epoch randomness up to `e5` when she submits her transaction. If a lottery draws
//!   over Alice's key, it needs to wait for a randomness with its moment past 13, so for the block
//!   randomness: P16 (`v16`, moment 14) and for the 1-epoch-ago randomness: P18 (`e6`, moment 15,
//!   determinable only at t8). In particular `e5` never qualifies: its moment 11 does not pass
//!   Alice's commitment moment, as she knows it since t4.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
pub mod weights;

#[cfg(any(test, feature = "runtime-benchmarks"))]
pub mod testing_utils;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub use pallet::*;
pub use weights::WeightInfo;

use codec::{Decode, Encode, MaxEncodedLen};
use cumulus_pallet_parachain_system::RelaychainDataProvider;
use cumulus_primitives_core::relay_chain;
use frame_support::traits::Get;
use indiv_support::traits::MomentRandomness;
use scale_info::TypeInfo;
use sp_runtime::traits::BlockNumberProvider;

/// A relay chain randomness value with the moment it became known to everybody.
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, Copy, PartialEq, Eq, Debug)]
pub struct RandomnessEntry {
	/// The relay chain randomness value.
	pub randomness: [u8; 32],
	/// The relay chain block number by which it was known to everybody.
	pub moment: relay_chain::BlockNumber,
}

/// The relay chain randomness values stored in [`Randomness`].
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct RandomnessValues {
	/// The distinct relay chain per-block VRF output.
	/// `None` until a VRF output is first observed.
	pub block: Option<RandomnessEntry>,
	/// The last distinct relay chain one-epoch-ago randomness.
	/// `None` until a value is first observed.
	pub one_epoch_ago: Option<RandomnessEntry>,
}

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use cumulus_pallet_parachain_system::{OnSystemEvent, RelayChainStateProof};
	use cumulus_primitives_core::{relay_chain::well_known_keys, PersistedValidationData};
	use frame_support::pallet_prelude::*;

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + cumulus_pallet_parachain_system::Config {
		/// Weight information for this pallet.
		type WeightInfo: WeightInfo;
	}

	/// The last distinct relay chain randomness values, refreshed from the relay chain
	/// state proof.
	///
	/// The values persist between blocks: they reflect the relay parent of the last
	/// block whose inherent ran. Code running before the inherent (e.g. `on_initialize`)
	/// sees the previous block's values.
	#[pallet::storage]
	pub type Randomness<T: Config> = StorageValue<_, RandomnessValues, ValueQuery>;

	impl<T: Config> Pallet<T> {
		/// Refresh a stored randomness entry with an observed value: when the value
		/// differs from the stored one, rewrite the entry with the given moment.
		///
		/// The relay chain keeps each value in state until it rotates, so an unchanged
		/// value keeps the moment it was first observed at. The moment thus only
		/// advances when the randomness itself changes.
		fn refresh_entry(
			entry: &mut Option<RandomnessEntry>,
			observed: Option<[u8; 32]>,
			moment: relay_chain::BlockNumber,
		) {
			let Some(randomness) = observed else { return };
			if entry.is_none_or(|entry| entry.randomness != randomness) {
				*entry = Some(RandomnessEntry { randomness, moment });
			}
		}
	}

	impl<T: Config> OnSystemEvent for Pallet<T> {
		fn on_validation_data(_data: &PersistedValidationData) {}

		fn on_validation_code_applied() {}

		fn on_relay_state_proof(relay_state_proof: &RelayChainStateProof) -> Weight {
			// `CURRENT_BLOCK_RANDOMNESS` is stored as an encoded `Option`: the outer
			// `Option` is `None` when the key is absent from the relay chain state, the
			// inner one when the relay block author claimed a slot without a VRF.
			let vrf = relay_state_proof
				.read_optional_entry::<Option<[u8; 32]>>(well_known_keys::CURRENT_BLOCK_RANDOMNESS)
				.expect("Invalid current block randomness in relay chain state proof")
				.flatten();

			let one_epoch_ago = relay_state_proof
				.read_optional_entry::<[u8; 32]>(well_known_keys::ONE_EPOCH_AGO_RANDOMNESS)
				.expect("Invalid one epoch ago randomness in relay chain state proof");

			// A newly observed block VRF was created in the relay parent itself, which
			// was already public for the relay parent offset when this block was
			// authored.
			let vrf_moment = RelaychainDataProvider::<T>::current_block_number().saturating_sub(
				<T as cumulus_pallet_parachain_system::Config>::RelayParentOffset::get(),
			);
			// A newly observed epoch randomness was additionally determined
			// one relay block before entering the state: its last input is the final
			// block VRF of the previous epoch.
			let epoch_moment = vrf_moment.saturating_sub(1);

			Randomness::<T>::mutate(|values| {
				Self::refresh_entry(&mut values.block, vrf, vrf_moment);
				Self::refresh_entry(&mut values.one_epoch_ago, one_epoch_ago, epoch_moment);
			});

			<T as Config>::WeightInfo::on_relay_state_proof()
		}
	}
}

/// [`MomentRandomness`] implementation over the relay chain block randomness.
///
/// It rotates every relay chain block. It is known by everybody
/// [`cumulus_pallet_parachain_system::Config::RelayParentOffset`] relay chain blocks before the
/// relay chain block is observed in the parachain.
///
/// The relay block producer already learned this randomness at the beginning of epoch
/// `current_epoch - 1`, when the seed of this VRF (the relay chain BABE epoch randomness)
/// became determinable.
///
/// Refer to the relay chain documentation to know the exact properties.
pub struct RelayBlockRandomness<T>(core::marker::PhantomData<T>);

impl<T: Config> MomentRandomness<relay_chain::BlockNumber> for RelayBlockRandomness<T> {
	fn randomness() -> Option<([u8; 32], relay_chain::BlockNumber)> {
		Randomness::<T>::get().block.map(|entry| (entry.randomness, entry.moment))
	}

	fn current_moment() -> relay_chain::BlockNumber {
		RelaychainDataProvider::<T>::current_block_number()
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn set_randomness(randomness: [u8; 32], moment: relay_chain::BlockNumber) {
		Randomness::<T>::mutate(|values| {
			values.block = Some(RandomnessEntry { randomness, moment })
		});
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn set_current_moment(moment: relay_chain::BlockNumber) {
		RelaychainDataProvider::<T>::set_block_number(moment);
	}
}

/// [`MomentRandomness`] implementation over the relay chain one-epoch-ago randomness.
///
/// It rotates every relay epoch. It is fully determined by the relay chain block preceding
/// the one that first serves it, itself known by everybody
/// [`cumulus_pallet_parachain_system::Config::RelayParentOffset`] relay chain blocks before being
/// observed in the parachain. Waiting for a moment past a commitment can take up to a bit more
/// than one full relay epochs.
///
/// Refer to the relay chain documentation to know the exact properties.
pub struct RelayOneEpochAgoRandomness<T>(core::marker::PhantomData<T>);

impl<T: Config> MomentRandomness<relay_chain::BlockNumber> for RelayOneEpochAgoRandomness<T> {
	fn randomness() -> Option<([u8; 32], relay_chain::BlockNumber)> {
		Randomness::<T>::get()
			.one_epoch_ago
			.map(|entry| (entry.randomness, entry.moment))
	}

	fn current_moment() -> relay_chain::BlockNumber {
		RelaychainDataProvider::<T>::current_block_number()
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn set_randomness(randomness: [u8; 32], moment: relay_chain::BlockNumber) {
		Randomness::<T>::mutate(|values| {
			values.one_epoch_ago = Some(RandomnessEntry { randomness, moment })
		});
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn set_current_moment(moment: relay_chain::BlockNumber) {
		RelaychainDataProvider::<T>::set_block_number(moment);
	}
}
