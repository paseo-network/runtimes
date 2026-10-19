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

//! AHM v2 migration wiring: the receiving side of moving account, proxy, registrar and HRMP
//! state off the relay chain.
//!
//! Upstream this is the Coretime chain's job (`pallet-ct-migrator`). Paseo's Coretime chain is
//! dead, so the pallet runs on Asset Hub instead; the relay chain's `pallet-rc2-migrator` is
//! pointed at Asset Hub accordingly.
//!
//! Compiled only with the `ahm-v2` feature, which released runtimes do not enable. The
//! integration tests turn it on to drive the real runtime.

use crate::{xcm_config::XcmRouter, AccountId, Runtime, RuntimeEvent};
use frame_system::EnsureRoot;

impl pallet_ct_migrator::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type SendXcm = XcmRouter;
	type AdminOrigin = EnsureRoot<AccountId>;
}

#[cfg(test)]
mod tests {
	use crate::{Runtime, RuntimeCall};
	use codec::Encode;
	use pallet_rc2_migrator::{CtMigratorCall, CtRuntimeCall};

	/// Ensure the pallet + call index aligns.
	#[test]
	fn the_relay_chain_encodes_this_chains_calls_correctly() {
		assert_eq!(
			CtRuntimeCall::CtMigrator(CtMigratorCall::StartMigration).encode(),
			RuntimeCall::CtMigrator(pallet_ct_migrator::Call::<Runtime>::start_migration {})
				.encode(),
		);
		assert_eq!(
			CtRuntimeCall::CtMigrator(CtMigratorCall::EndLockdown).encode(),
			RuntimeCall::CtMigrator(pallet_ct_migrator::Call::<Runtime>::end_lockdown {}).encode(),
		);
	}
}
