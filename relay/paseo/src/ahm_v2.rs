// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Paseo.

// Paseo is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Paseo is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Paseo.  If not, see <http://www.gnu.org/licenses/>.

//! AHM v2 migration wiring: the relay-chain side of moving account, proxy, registrar and HRMP
//! state off the relay chain.
//!
//! Upstream this state goes to the Coretime chain. Paseo's Coretime chain is dead, so the
//! receiving side (`pallet-ct-migrator`) runs on Asset Hub and every "CT" item below points
//! there.
//!
//! Compiled only with the `ahm-v2` feature, which released runtimes do not enable. The
//! integration tests turn it on to drive the real runtime.

pub mod lockdown;

use crate::{
	xcm_config::{AssetHubLocation, XcmRouter},
	AccountId, CrowdloanId, DelegatedStakingPalletId, OnDemandPalletId, PoolsPalletId, ProxyType,
	Runtime, RuntimeEvent, Timestamp, TransparentProxyType, TreasuryPalletId,
};
use frame_support::{
	parameter_types,
	traits::{Contains, Equals},
};
use frame_system::EnsureRoot;
use migrator_types::PortableProxyType;
use pallet_xcm::EnsureXcm;
use paseo_runtime_constants::{currency::UNITS, system_parachain};
use sp_runtime::traits::AccountIdConversion;

parameter_types! {
	/// Asset Hub stands in for the Coretime chain on Paseo, and is also where free balances are
	/// teleported to. Upstream these are two chains; here both halves of an account land on
	/// Asset Hub.
	pub const AssetHubId: u32 = system_parachain::ASSET_HUB_ID;
	pub const CtFreeBuffer: u128 = UNITS;
}

/// The pots of this chain, left for Sweep.
///
/// Upstream also lists the broker pot (`BrokerPalletId`). Paseo has no Coretime chain and the
/// account does not exist on the Paseo relay, so it is left out.
pub struct SweptAccounts;
impl Contains<AccountId> for SweptAccounts {
	fn contains(who: &AccountId) -> bool {
		[
			TreasuryPalletId::get(),
			OnDemandPalletId::get(),
			CrowdloanId::get(),
			PoolsPalletId::get(),
			DelegatedStakingPalletId::get(),
		]
		.into_iter()
		.any(|id| AccountIdConversion::<AccountId>::into_account_truncating(&id) == *who)
	}
}

impl pallet_rc2_migrator::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type SendXcm = XcmRouter;
	type CtParaId = AssetHubId;
	type AhParaId = AssetHubId;
	type CtFreeBuffer = CtFreeBuffer;
	type TimeProvider = Timestamp;
	type CtOrigin = EnsureXcm<Equals<AssetHubLocation>>;
	type AdminOrigin = EnsureRoot<AccountId>;
	type PreMigrationCalls = frame_support::traits::Everything;
	type IntraMigrationCalls = lockdown::CallsEnabledDuringMigration;
	type PostMigrationCalls = lockdown::CallsEnabledAfterMigration;
	type SweptAccounts = SweptAccounts;
}

/// Which proxy permissions travel to Asset Hub (the Coretime chain upstream). The others have no
/// meaning there, so their definitions stay here.
impl TryFrom<TransparentProxyType<ProxyType>> for PortableProxyType {
	type Error = ();

	fn try_from(t: TransparentProxyType<ProxyType>) -> Result<Self, ()> {
		match t.0 {
			ProxyType::Any => Ok(PortableProxyType::Any),
			ProxyType::NonTransfer => Ok(PortableProxyType::NonTransfer),
			ProxyType::CancelProxy => Ok(PortableProxyType::CancelProxy),
			ProxyType::ParaRegistration => Ok(PortableProxyType::ParaRegistration),
			ProxyType::Governance |
			ProxyType::Staking |
			ProxyType::Auction |
			ProxyType::NominationPools => Err(()),
			// Paseo-only. `StakingOperator` manages relay session keys; `SafeSudo` is relay sudo.
			// Neither has a counterpart in `PortableProxyType`.
			ProxyType::StakingOperator | ProxyType::SafeSudo => Err(()),
		}
	}
}

#[cfg(test)]
mod tests {
	use crate::{ProxyType, Runtime, RuntimeCall, TransparentProxyType};
	use codec::Encode;
	use migrator_types::PortableProxyType;
	use pallet_ct_migrator::{Rc2MigratorCall, Rc2RuntimeCall};

	/// Which proxy permissions travel to Asset Hub, one assertion per variant so a new variant
	/// has to be classified here.
	#[test]
	fn proxy_permissions_that_travel() {
		for (proxy_type, portable) in [
			(ProxyType::Any, Some(PortableProxyType::Any)),
			(ProxyType::NonTransfer, Some(PortableProxyType::NonTransfer)),
			(ProxyType::CancelProxy, Some(PortableProxyType::CancelProxy)),
			(ProxyType::ParaRegistration, Some(PortableProxyType::ParaRegistration)),
			(ProxyType::Governance, None),
			(ProxyType::Staking, None),
			(ProxyType::Auction, None),
			(ProxyType::NominationPools, None),
			(ProxyType::StakingOperator, None),
			(ProxyType::SafeSudo, None),
		] {
			assert_eq!(
				PortableProxyType::try_from(TransparentProxyType(proxy_type)).ok(),
				portable,
				"{proxy_type:?}"
			);
		}
	}

	/// Ensure the pallet + call index aligns.
	#[test]
	fn asset_hub_encodes_this_chains_calls_correctly() {
		assert_eq!(
			Rc2RuntimeCall::Rc2Migrator(Rc2MigratorCall::CtReady).encode(),
			RuntimeCall::Rc2Migrator(pallet_rc2_migrator::Call::<Runtime>::ct_ready {}).encode(),
		);
	}
}
