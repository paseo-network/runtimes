// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Coretime migration for Polkadot runtime

use crate::{
	coretime::{Config, WeightInfo},
	parachains_assigner_coretime,
	parachains_assigner_coretime::PartsOf57600,
	OriginKind,
};
use codec::{Decode, Encode};
use core::{iter, result};
#[cfg(feature = "try-runtime")]
use frame_support::ensure;
use frame_support::{
	traits::{OnRuntimeUpgrade, PalletInfoAccess, StorageVersion},
	weights::Weight,
};
use frame_system::pallet_prelude::BlockNumberFor;
use pallet_broker::{CoreAssignment, CoreMask, ScheduleItem};
use polkadot_parachain_primitives::primitives::IsSystem;
use polkadot_primitives::{Balance, BlockNumber, CoreIndex, Id as ParaId};
use paseo_runtime_constants::system_parachain::coretime::TIMESLICE_PERIOD;
use runtime_parachains::configuration;
#[cfg(feature = "try-runtime")]
use runtime_parachains::scheduler::common::AssignmentProvider;

use sp_arithmetic::traits::SaturatedConversion;
use sp_core::Get;
use sp_runtime::BoundedVec;
use sp_std::{vec, vec::Vec};
use xcm::prelude::{send_xcm, Instruction, Junction, Location, SendError, WeightLimit, Xcm};

/// Return information about a legacy lease of a parachain.
pub trait GetLegacyLease<N> {
	/// If parachain is a lease holding parachain, return the block at which the lease expires.
	fn get_parachain_lease_in_blocks(para: ParaId) -> Option<N>;
	// All parachains holding a lease, no matter if there are gaps in the slots or not.
	fn get_all_parachains_with_leases() -> Vec<ParaId>;
}

#[derive(Encode, Decode)]
enum CoretimeCalls {
	#[codec(index = 1)]
	Reserve(pallet_broker::Schedule),
	#[codec(index = 3)]
	SetLease(pallet_broker::TaskId, pallet_broker::Timeslice),
	#[codec(index = 19)]
	NotifyCoreCount(u16),
	#[codec(index = 20)]
	NotifyRevenue((BlockNumber, Balance)),
	#[codec(index = 99)]
	SwapLeases(ParaId, ParaId),
}

#[derive(Encode, Decode)]
enum BrokerRuntimePallets {
	#[codec(index = 50)]
	Broker(CoretimeCalls),
}

/// Migrate a chain to use coretime.
///
/// This assumes that the `Coretime` and the `AssignerCoretime` pallets are added at the same
/// time to a runtime.
pub struct MigrateToCoretime<T, SendXcm, LegacyLease>(
	core::marker::PhantomData<(T, SendXcm, LegacyLease)>,
);

impl<T: Config, SendXcm: xcm::v4::SendXcm, LegacyLease: GetLegacyLease<BlockNumberFor<T>>>
	MigrateToCoretime<T, SendXcm, LegacyLease>
{
	fn already_migrated() -> bool {
		// We are using the assigner coretime because the coretime pallet doesn't has any
		// storage data. But both pallets are introduced at the same time, so this is fine.
		let name_hash = parachains_assigner_coretime::Pallet::<T>::name_hash();
		let mut next_key = name_hash.to_vec();
		let storage_version_key =
			StorageVersion::storage_key::<parachains_assigner_coretime::Pallet<T>>();

		loop {
			match sp_io::storage::next_key(&next_key) {
				// StorageVersion is initialized before, so we need to ignore it.
				Some(key) if key == storage_version_key => {
					next_key = key;
				},
				// If there is any other key with the prefix of the pallet,
				// we already have executed the migration.
				Some(key) if key.starts_with(&name_hash) => {
					log::info!("`MigrateToCoretime` already executed!");
					return true
				},
				// Any other key/no key means that we did not yet have migrated.
				None | Some(_) => return false,
			}
		}
	}
}

impl<
		T: Config + runtime_parachains::dmp::Config,
		SendXcm: xcm::v4::SendXcm,
		LegacyLease: GetLegacyLease<BlockNumberFor<T>>,
	> OnRuntimeUpgrade for MigrateToCoretime<T, SendXcm, LegacyLease>
{
	fn on_runtime_upgrade() -> Weight {
		if Self::already_migrated() {
			return Weight::zero()
		}

		log::info!("Migrating existing parachains to coretime.");
		migrate_to_coretime::<T, SendXcm, LegacyLease>()
	}

	#[cfg(feature = "try-runtime")]
	fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::DispatchError> {
		if Self::already_migrated() {
			return Ok(Vec::new())
		}

		let legacy_paras = LegacyLease::get_all_parachains_with_leases();
		let config = configuration::ActiveConfig::<T>::get();
		let total_core_count = config.scheduler_params.num_cores + legacy_paras.len() as u32;

		Ok(total_core_count.encode())
	}

	#[cfg(feature = "try-runtime")]
	fn post_upgrade(state: Vec<u8>) -> Result<(), sp_runtime::DispatchError> {
		if state.is_empty() {
			return Ok(())
		}

		log::trace!("Running post_upgrade()");

		let prev_core_count = <u32>::decode(&mut &state[..]).unwrap();
		let new_core_count = parachains_assigner_coretime::Pallet::<T>::session_core_count();
		ensure!(new_core_count == prev_core_count, "Total number of cores need to not change.");

		Ok(())
	}
}

// Migrate to Coretime.
//
// NOTE: Also migrates `num_cores` config value in configuration::ActiveConfig.
fn migrate_to_coretime<
	T: Config,
	SendXcm: xcm::v4::SendXcm,
	LegacyLease: GetLegacyLease<BlockNumberFor<T>>,
>() -> Weight {
	let legacy_paras = LegacyLease::get_all_parachains_with_leases();
	let legacy_count = legacy_paras.len() as u32;
	let now = frame_system::Pallet::<T>::block_number();
	for (core, para_id) in legacy_paras.into_iter().enumerate() {
		let r = parachains_assigner_coretime::Pallet::<T>::assign_core(
			CoreIndex(core as u32),
			now,
			vec![(CoreAssignment::Task(para_id.into()), PartsOf57600::FULL)],
			None,
		);
		if let Err(err) = r {
			log::error!(
				"Creating assignment for existing para failed: {:?}, error: {:?}",
				para_id,
				err
			);
		}
	}

	let config = configuration::ActiveConfig::<T>::get();
	for on_demand in 0..config.scheduler_params.num_cores {
		let core = CoreIndex(legacy_count.saturating_add(on_demand as _));
		let r = parachains_assigner_coretime::Pallet::<T>::assign_core(
			core,
			now,
			vec![(CoreAssignment::Pool, PartsOf57600::FULL)],
			None,
		);
		if let Err(err) = r {
			log::error!("Creating assignment for existing on-demand core, failed: {:?}", err);
		}
	}
	let total_cores = config.scheduler_params.num_cores + legacy_count;
	configuration::ActiveConfig::<T>::mutate(|c| {
		c.scheduler_params.num_cores = total_cores;
	});

	if let Err(err) = migrate_send_assignments_to_coretime_chain::<T, SendXcm, LegacyLease>() {
		log::error!("Sending legacy chain data to coretime chain failed: {:?}", err);
	}

	let single_weight = <T as Config>::WeightInfo::assign_core(1);
	single_weight
		.saturating_mul(u64::from(legacy_count.saturating_add(config.scheduler_params.num_cores)))
		// Second read from sending assignments to the coretime chain.
		.saturating_add(T::DbWeight::get().reads_writes(2, 1))
}

fn migrate_send_assignments_to_coretime_chain<
	T: Config,
	SendXcm: xcm::v4::SendXcm,
	LegacyLease: GetLegacyLease<BlockNumberFor<T>>,
>() -> result::Result<(), SendError> {
	let legacy_paras = LegacyLease::get_all_parachains_with_leases();
	let legacy_paras_count = legacy_paras.len();
	let (system_chains, lease_holding): (Vec<_>, Vec<_>) =
		legacy_paras.into_iter().partition(IsSystem::is_system);

	let reservations = system_chains.into_iter().map(|p| {
		let schedule = BoundedVec::truncate_from(vec![ScheduleItem {
			mask: CoreMask::complete(),
			assignment: CoreAssignment::Task(p.into()),
		}]);
		mk_coretime_call::<T>(CoretimeCalls::Reserve(schedule))
	});

	let mut leases = lease_holding.into_iter().filter_map(|p| {
			log::trace!(target: "coretime-migration", "Preparing sending of lease holding para {:?}", p);
			let Some(valid_until) = LegacyLease::get_parachain_lease_in_blocks(p) else {
				log::error!("Lease holding chain with no lease information?!");
				return None
			};

			let valid_until: u32 = match valid_until.try_into() {
				Ok(val) => val,
				Err(_) => {
					log::error!("Converting block number to u32 failed!");
					return None
				},
			};

			let time_slice = (valid_until + TIMESLICE_PERIOD - 1).div_ceil(TIMESLICE_PERIOD);
			log::trace!(target: "coretime-migration", "Sending of lease holding para {:?}, valid_until: {:?}, time_slice: {:?}", p, valid_until, time_slice);
			Some(mk_coretime_call::<T>(CoretimeCalls::SetLease(p.into(), time_slice)))
		});

	let core_count: u16 = configuration::ActiveConfig::<T>::get()
		.scheduler_params
		.num_cores
		.saturated_into();
	let set_core_count =
		iter::once(mk_coretime_call::<T>(CoretimeCalls::NotifyCoreCount(core_count)));
	log::trace!(target: "coretime-migration", "Set core count to {:?}. legacy paras count is  {:?}",core_count, legacy_paras_count);

	let pool = (legacy_paras_count..core_count.into()).map(|_| {
		let schedule = BoundedVec::truncate_from(vec![ScheduleItem {
			mask: CoreMask::complete(),
			assignment: CoreAssignment::Pool,
		}]);
		// Reserved cores will come before lease cores, so cores will change their assignments
		// when coretime chain sends us their assign_core calls -> Good test.
		mk_coretime_call::<T>(CoretimeCalls::Reserve(schedule))
	});

	let message_content = iter::once(Instruction::UnpaidExecution {
		weight_limit: WeightLimit::Unlimited,
		check_origin: None,
	});

	let reservation_content = message_content.clone().chain(reservations).collect();
	let leases_content_1 = message_content
		.clone()
		.chain(leases.by_ref().take(legacy_paras_count / 2)) // split in two messages to avoid overweighted XCM
		.collect();
	let leases_content_2 = message_content.clone().chain(leases).collect();
	let set_core_count_content = message_content.clone().chain(set_core_count).collect();

	// If `pool_content` is empty don't send a blank XCM message
	let messages = if core_count as usize > legacy_paras_count {
		let pool_content = message_content.clone().chain(pool).collect();
		vec![
			Xcm(reservation_content),
			Xcm(pool_content),
			Xcm(leases_content_1),
			Xcm(leases_content_2),
			Xcm(set_core_count_content),
		]
	} else {
		vec![
			Xcm(reservation_content),
			Xcm(leases_content_1),
			Xcm(leases_content_2),
			Xcm(set_core_count_content),
		]
	};

	for message in messages {
		send_xcm::<SendXcm>(Location::new(0, Junction::Parachain(T::BrokerId::get())), message)?;
	}

	Ok(())
}

fn mk_coretime_call<T: Config>(call: CoretimeCalls) -> Instruction<()> {
	Instruction::Transact {
		origin_kind: OriginKind::Superuser,
		require_weight_at_most: T::MaxXcmTransactWeight::get(),
		call: BrokerRuntimePallets::Broker(call).encode().into(),
	}
}
