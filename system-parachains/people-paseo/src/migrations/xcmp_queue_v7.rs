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

//! PASEO-LOCAL. Bumps `XcmpQueue`'s storage version from 6 to 7 WITHOUT touching
//! `OutboundXcmpStatus`.
//!
//! # Why this exists instead of upstream's `MigrateV6ToV7`
//!
//! `cumulus-pallet-xcmp-queue` 0.31.0 adds a `queued_bytes: u32` field to
//! `OutboundChannelDetails` (14 -> 18 bytes per channel) and ships
//! `migration::v7::MigrateV6ToV7`, which decodes the value in the v6 shape and re-encodes it in
//! the v7 shape, bumping the storage version from 6 to 7.
//!
//! On this chain the on-chain storage version is still 6, but the value is already in the v7
//! shape: the runtime has been built against `cumulus-pallet-xcmp-queue` >= 0.30 (v7 in-code
//! layout) since v2.5.0, and every write to `OutboundXcmpStatus` since then has encoded the new
//! struct, while nothing ever bumped the version marker. Verified against live state: the raw
//! value is `1 + n * 18` bytes (People: 2 channels, Asset Hub: 3).
//!
//! Running upstream's `MigrateV6ToV7` on that state would make `translate` fail to decode the
//! v6 shape (it would read 18-byte items as 14-byte ones and hit trailing bytes), return `Err`
//! before writing anything, and `defensive!` an `ERROR` log at enactment (a panic under debug
//! assertions). `VersionedMigration` would then still bump the version to 7, so the end state
//! is correct, but the log is noisy and misrepresents what happened.
//!
//! This migration replaces it: it writes nothing, only the `VersionedMigration` wrapper bumps
//! the version marker. `on_runtime_upgrade` reads the value once to put the channel count into
//! the enactment log. Under `try-runtime` the hooks read the raw value and assert it decodes
//! as the v7 layout, before and after, and that the bytes are unchanged.
//!
//! # Lifecycle
//!
//! Self-guarding through `VersionedMigration`: once the on-chain version is 7 it is a single
//! read. MUST be dropped from `Unreleased` once this runtime is enacted; it must never coexist
//! with upstream's `MigrateV6ToV7` in the same tuple.

#[cfg(feature = "try-runtime")]
use codec::Decode;
use codec::DecodeAll;
#[cfg(any(feature = "try-runtime", test))]
use codec::Encode;
use core::marker::PhantomData;
use cumulus_pallet_xcmp_queue::{Config, OutboundChannelDetails, Pallet};
use frame_support::{
	migrations::VersionedMigration,
	storage::unhashed,
	traits::{Get, PalletInfoAccess, UncheckedOnRuntimeUpgrade},
	weights::Weight,
	BoundedVec,
};
use sp_io::hashing::twox_128;

const LOG_TARGET: &str = "runtime::xcmp-queue-migration";

/// The v7 (in-code) shape of `OutboundXcmpStatus`.
type OutboundXcmpStatusV7<T> =
	BoundedVec<OutboundChannelDetails, <T as Config>::MaxActiveOutboundChannels>;

/// Raw storage key of `XcmpQueue::OutboundXcmpStatus`.
///
/// The storage item is `pub(super)` in the pallet, so the value is read raw:
/// `twox_128(pallet_name) ++ twox_128(b"OutboundXcmpStatus")`.
pub fn outbound_xcmp_status_key<T: Config>() -> alloc::vec::Vec<u8> {
	[twox_128(Pallet::<T>::name().as_bytes()), twox_128(b"OutboundXcmpStatus")].concat()
}

/// Reads the raw `OutboundXcmpStatus` value and checks it decodes as the v7 layout.
///
/// Returns the raw bytes (`None` when the key is absent, which the pallet reads as an empty
/// vector) and the number of channels. One storage read.
fn read_v7_status<T: Config>() -> Result<(Option<alloc::vec::Vec<u8>>, usize), &'static str> {
	let raw = unhashed::get_raw(&outbound_xcmp_status_key::<T>());
	let channels = match &raw {
		Some(bytes) => OutboundXcmpStatusV7::<T>::decode_all(&mut &bytes[..])
			.map_err(|_| "xcmp-queue: OutboundXcmpStatus does not decode as the v7 layout")?
			.len(),
		None => 0,
	};
	Ok((raw, channels))
}

/// Version-only migration: writes nothing. Wrapped by [`XcmpQueueSetStorageVersion7`].
pub struct UncheckedSetStorageVersion7<T>(PhantomData<T>);

impl<T: Config> UncheckedOnRuntimeUpgrade for UncheckedSetStorageVersion7<T> {
	fn on_runtime_upgrade() -> Weight {
		// One read, no write: the channel count goes into the enactment log so that the
		// collator output states what was found. The try-runtime hooks below cannot serve that
		// purpose: `try-runtime-cli` runs the first (version 6 -> 7) pass without checks and
		// the checked pass on already-migrated state, where `VersionedMigration` skips them.
		match read_v7_status::<T>() {
			Ok((raw, channels)) => log::info!(
				target: LOG_TARGET,
				"XcmpQueue: OutboundXcmpStatus holds {channels} channel(s) ({} raw byte(s)), \
				already in the v7 layout; bumping storage version 6 -> 7 without rewriting it",
				raw.as_ref().map_or(0, |b| b.len()),
			),
			// The state this migration is written NOT to be in. Reported, not touched: rewriting
			// it is upstream's `MigrateV6ToV7` job, and the two must never be swapped blindly.
			Err(e) => log::warn!(
				target: LOG_TARGET,
				"XcmpQueue: {e}; bumping storage version 6 -> 7 anyway, nothing rewritten",
			),
		}
		// The `VersionedMigration` wrapper accounts for the version read and write.
		T::DbWeight::get().reads(1)
	}

	#[cfg(feature = "try-runtime")]
	fn pre_upgrade() -> Result<alloc::vec::Vec<u8>, sp_runtime::TryRuntimeError> {
		let (raw, channels) = read_v7_status::<T>()?;
		log::info!(
			target: LOG_TARGET,
			"pre_upgrade: OutboundXcmpStatus decodes as v7 with {channels} channel(s) \
			({} raw byte(s)); storage version 6 -> 7 will be the only write",
			raw.as_ref().map_or(0, |b| b.len()),
		);
		Ok(raw.encode())
	}

	#[cfg(feature = "try-runtime")]
	fn post_upgrade(state: alloc::vec::Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
		let expected: Option<alloc::vec::Vec<u8>> = Decode::decode(&mut &state[..])
			.map_err(|_| "xcmp-queue: could not decode the pre_upgrade capture")?;
		let (raw, channels) = read_v7_status::<T>()?;
		frame_support::ensure!(
			raw == expected,
			"xcmp-queue: OutboundXcmpStatus bytes changed across the version bump"
		);
		log::info!(
			target: LOG_TARGET,
			"post_upgrade: OutboundXcmpStatus unchanged, still v7 with {channels} channel(s)",
		);
		Ok(())
	}
}

/// [`UncheckedSetStorageVersion7`] wrapped in a `VersionedMigration`: runs only when the
/// on-chain `XcmpQueue` storage version is 6, and leaves it at 7.
pub type XcmpQueueSetStorageVersion7<T> = VersionedMigration<
	6,
	7,
	UncheckedSetStorageVersion7<T>,
	Pallet<T>,
	<T as frame_system::Config>::DbWeight,
>;

#[cfg(test)]
mod tests {
	use super::*;
	use crate::Runtime;
	use cumulus_primitives_core::ParaId;
	use frame_support::traits::{GetStorageVersion, OnRuntimeUpgrade, StorageVersion};

	type Migration = XcmpQueueSetStorageVersion7<Runtime>;

	/// A v7-shaped value with the same channel count as live People (2), one suspended and
	/// carrying signals so that no field is at its default.
	fn v7_value() -> alloc::vec::Vec<u8> {
		let channels: OutboundXcmpStatusV7<Runtime> = BoundedVec::truncate_from(alloc::vec![
			OutboundChannelDetails::new(ParaId::from(1000)),
			OutboundChannelDetails::new(ParaId::from(1004))
				.with_signals()
				.with_suspended_state(),
		]);
		channels.encode()
	}

	fn ext_at_version_6() -> sp_io::TestExternalities {
		let mut ext = sp_io::TestExternalities::default();
		ext.execute_with(|| {
			StorageVersion::new(6).put::<Pallet<Runtime>>();
			unhashed::put_raw(&outbound_xcmp_status_key::<Runtime>(), &v7_value());
		});
		ext
	}

	#[test]
	fn key_matches_the_pallet_name_in_construct_runtime() {
		assert_eq!(Pallet::<Runtime>::name(), "XcmpQueue");
		assert_eq!(
			outbound_xcmp_status_key::<Runtime>(),
			[twox_128(b"XcmpQueue"), twox_128(b"OutboundXcmpStatus")].concat()
		);
	}

	/// The live value is `1 + n * 18` bytes; the synthetic one must match that layout.
	#[test]
	fn synthetic_value_has_the_live_v7_shape() {
		assert_eq!(v7_value().len(), 1 + 2 * 18);
		let (raw, channels) = ext_at_version_6().execute_with(read_v7_status::<Runtime>).unwrap();
		assert_eq!(raw.as_deref(), Some(&v7_value()[..]));
		assert_eq!(channels, 2);
	}

	#[test]
	fn bumps_version_and_leaves_bytes_unchanged() {
		ext_at_version_6().execute_with(|| {
			let before = v7_value();
			assert_eq!(Pallet::<Runtime>::on_chain_storage_version(), StorageVersion::new(6));

			#[cfg(feature = "try-runtime")]
			let state = Migration::pre_upgrade().unwrap();
			let weight = Migration::on_runtime_upgrade();
			#[cfg(feature = "try-runtime")]
			Migration::post_upgrade(state).unwrap();

			assert_eq!(Pallet::<Runtime>::on_chain_storage_version(), StorageVersion::new(7));
			assert_eq!(
				unhashed::get_raw(&outbound_xcmp_status_key::<Runtime>()).as_deref(),
				Some(&before[..])
			);
			// One value read, plus the wrapper's version read + write.
			assert_eq!(
				weight,
				<Runtime as frame_system::Config>::DbWeight::get().reads_writes(2, 1)
			);
		});
	}

	#[test]
	fn is_a_no_op_once_at_version_7() {
		ext_at_version_6().execute_with(|| {
			Migration::on_runtime_upgrade();
			let before = unhashed::get_raw(&outbound_xcmp_status_key::<Runtime>());
			let weight = Migration::on_runtime_upgrade();
			assert_eq!(Pallet::<Runtime>::on_chain_storage_version(), StorageVersion::new(7));
			assert_eq!(unhashed::get_raw(&outbound_xcmp_status_key::<Runtime>()), before);
			assert_eq!(weight, <Runtime as frame_system::Config>::DbWeight::get().reads(1));
		});
	}

	/// A v6-shaped value (14-byte items) must be rejected by the checks, which is exactly the
	/// state upstream's `MigrateV6ToV7` is written for and this chain is not in.
	#[test]
	fn v6_shape_is_rejected_by_the_checks() {
		let mut ext = sp_io::TestExternalities::default();
		ext.execute_with(|| {
			// Two v7 items with the trailing `queued_bytes: u32` stripped from each.
			let v7 = v7_value();
			let mut v6 = alloc::vec![v7[0]];
			v6.extend_from_slice(&v7[1..15]);
			v6.extend_from_slice(&v7[19..33]);
			assert_eq!(v6.len(), 1 + 2 * 14);
			unhashed::put_raw(&outbound_xcmp_status_key::<Runtime>(), &v6);
			assert!(read_v7_status::<Runtime>().is_err());
		});
	}
}
