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

//! The AHM v2 lockdown of the chain receiving the relay chain's state: which calls each migration
//! stage allows.
//!
//! Upstream this is the Coretime chain's lockdown. On Paseo `pallet-ct-migrator` runs on Asset
//! Hub, so it is Asset Hub's.

use crate::RuntimeCall;
use frame_support::traits::Contains;

/// Contains all calls that are enabled before the migration starts.
pub struct CallsEnabledBeforeMigration;
impl Contains<RuntimeCall> for CallsEnabledBeforeMigration {
	fn contains(call: &RuntimeCall) -> bool {
		call_allowed_status(call).0
	}
}

/// Contains all calls that are enabled during the migration.
pub struct CallsEnabledDuringMigration;
impl Contains<RuntimeCall> for CallsEnabledDuringMigration {
	fn contains(call: &RuntimeCall) -> bool {
		call_allowed_status(call).1
	}
}

/// Return whether a call is enabled before and during the migration. Every call is enabled after
/// it.
///
/// During is from the relay chain's start signal until it ends the lockdown.
///
/// Proxy definitions arrive from the relay chain and are merged into `pallet_proxy`, so during the
/// migration every proxy call that changes the proxy map, or the deposits recorded in it, is
/// disabled. Using a proxy and handling announcements stay enabled. Announcements are not
/// migrated: the relay chain releases their deposits and sends them to Asset Hub as free balance.
pub fn call_allowed_status(call: &RuntimeCall) -> (bool, bool) {
	use RuntimeCall::*;
	const ON: bool = true;
	const OFF: bool = false;

	match call {
		System(..) => (ON, ON),
		ParachainSystem(..) => (ON, ON),
		Timestamp(..) => (ON, ON),
		ParachainInfo(..) => (ON, ON),
		Preimage(..) => (ON, ON),
		Scheduler(..) => (ON, ON),
		Parameters(..) => (ON, ON),
		MultiBlockMigrations(..) => (ON, ON),
		Balances(..) => (ON, ON),
		Vesting(..) => (ON, ON),
		Claims(..) => (ON, ON),
		Dap(..) => (ON, ON),
		CollatorSelection(..) => (ON, ON),
		Session(..) => (ON, ON),
		XcmpQueue(..) => (ON, ON),
		PolkadotXcm(..) => (ON, ON),
		CumulusXcm(..) => (ON, ON),
		ToKusamaXcmRouter(..) => (ON, ON),
		MessageQueue(..) => (ON, ON),
		SnowbridgeSystemFrontend(..) => (ON, ON),
		Utility(..) => (ON, ON),
		Multisig(..) => (ON, ON),
		// Leave the proxy map untouched.
		Proxy(
			pallet_proxy::Call::proxy { .. } |
			pallet_proxy::Call::proxy_announced { .. } |
			pallet_proxy::Call::announce { .. } |
			pallet_proxy::Call::remove_announcement { .. } |
			pallet_proxy::Call::reject_announcement { .. },
		) => (ON, ON),
		Indices(..) => (ON, ON),
		Assets(..) => (ON, ON),
		Uniques(..) => (ON, ON),
		Nfts(..) => (ON, ON),
		ForeignAssets(..) => (ON, ON),
		PoolAssets(..) => (ON, ON),
		AssetConversion(..) => (ON, ON),
		Scarcity(..) => (ON, ON),
		Psm(..) => (ON, ON),
		Treasury(..) => (ON, ON),
		ConvictionVoting(..) => (ON, ON),
		Referenda(..) => (ON, ON),
		Whitelist(..) => (ON, ON),
		Bounties(..) => (ON, ON),
		ChildBounties(..) => (ON, ON),
		AssetRate(..) => (ON, ON),
		MultiAssetBounties(..) => (ON, ON),
		StateTrieMigration(..) => (ON, ON),
		NominationPools(..) => (ON, ON),
		VoterList(..) => (ON, ON),
		StakingRcClient(..) => (ON, ON),
		MultiBlockElection(..) => (ON, ON),
		MultiBlockElectionVerifier(..) => (ON, ON),
		MultiBlockElectionUnsigned(..) => (ON, ON),
		MultiBlockElectionSigned(..) => (ON, ON),
		Staking(..) => (ON, ON),
		NftClaims(..) => (ON, ON),
		MembersSubscriber(..) => (ON, ON),
		AliasAccounts(..) => (ON, ON),
		Pgas(..) => (ON, ON),
		Revive(..) => (ON, ON),
		DotnsGateway(..) => (ON, ON),
		OriginRestriction(..) => (ON, ON),
		NetworkSuffix(..) => (ON, ON),
		Sudo(..) => (ON, ON),
		AhOps(..) => (ON, ON),
		CtMigrator(..) => (ON, ON),

		// TODO(ahm-v2): `RegistrarPara` and `HrmpPara` join as `(OFF, OFF)`.
		Proxy(..) => (ON, OFF),
	}
}

#[cfg(test)]
mod tests {
	use crate::{AccountId, ProxyType, Runtime, RuntimeCall};
	use frame_support::traits::Contains;
	use pallet_ct_migrator::{CtMigrationStage, MigrationStage};
	use parachains_runtimes_test_utils::ExtBuilder;

	fn allowed_at(stage: &MigrationStage, call: &RuntimeCall) -> bool {
		ExtBuilder::<Runtime>::default().build().execute_with(|| {
			CtMigrationStage::<Runtime>::put(stage);
			<Runtime as frame_system::Config>::BaseCallFilter::contains(call)
		})
	}

	#[test]
	fn proxy_changes_are_refused_while_the_migration_runs() {
		let alice = AccountId::new([1; 32]); // any account
		let remark = RuntimeCall::System(frame_system::Call::remark { remark: vec![1] });
		let changes = [
			RuntimeCall::Proxy(pallet_proxy::Call::add_proxy {
				delegate: alice.clone().into(),
				proxy_type: ProxyType::Any,
				delay: 0,
			}),
			RuntimeCall::Proxy(pallet_proxy::Call::remove_proxy {
				delegate: alice.clone().into(),
				proxy_type: ProxyType::Any,
				delay: 0,
			}),
			RuntimeCall::Proxy(pallet_proxy::Call::remove_proxies {}),
			RuntimeCall::Proxy(pallet_proxy::Call::create_pure {
				proxy_type: ProxyType::Any,
				delay: 0,
				index: 0,
			}),
			RuntimeCall::Proxy(pallet_proxy::Call::kill_pure {
				spawner: alice.clone().into(),
				proxy_type: ProxyType::Any,
				index: 0,
				height: 0,
				ext_index: 0,
			}),
			RuntimeCall::Proxy(pallet_proxy::Call::poke_deposit {}),
		];
		let uses = [
			RuntimeCall::Proxy(pallet_proxy::Call::proxy {
				real: alice.clone().into(),
				force_proxy_type: None,
				call: Box::new(remark.clone()),
			}),
			RuntimeCall::Proxy(pallet_proxy::Call::proxy_announced {
				delegate: alice.clone().into(),
				real: alice.clone().into(),
				force_proxy_type: None,
				call: Box::new(remark.clone()),
			}),
			RuntimeCall::Proxy(pallet_proxy::Call::announce {
				real: alice.clone().into(),
				call_hash: Default::default(),
			}),
			RuntimeCall::Proxy(pallet_proxy::Call::remove_announcement {
				real: alice.clone().into(),
				call_hash: Default::default(),
			}),
			RuntimeCall::Proxy(pallet_proxy::Call::reject_announcement {
				delegate: alice.clone().into(),
				call_hash: Default::default(),
			}),
			remark,
		];

		for stage in [
			MigrationStage::Pending,
			MigrationStage::DataMigrationOngoing,
			MigrationStage::MigrationDone,
		] {
			// GIVEN the chain at `stage`.
			// THEN a proxy change is refused exactly while the migration runs.
			for call in &changes {
				assert_eq!(allowed_at(&stage, call), !stage.is_ongoing(), "{call:?} at {stage:?}");
			}
			// THEN using a proxy, handling announcements, and any other call, pass at every stage.
			for call in &uses {
				assert!(allowed_at(&stage, call), "{call:?} refused at {stage:?}");
			}
		}
	}
}
