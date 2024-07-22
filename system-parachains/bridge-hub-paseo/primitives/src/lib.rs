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

//! Module with configuration which reflects BridgeHubPolkadot runtime setup
//! (AccountId, Headers, Hashes...)

#![cfg_attr(not(feature = "std"), no_std)]

pub use bp_bridge_hub_cumulus::*;

pub const BRIDGE_HUB_PASEO_PARACHAIN_ID: u32 = 1002;
pub mod snowbridge {
    use crate::Balance;
    use frame_support::parameter_types;
    use snowbridge_core::{PricingParameters, Rewards, U256};
    use sp_runtime::FixedU128;
    use xcm::latest::NetworkId;

    parameter_types! {
		/// Should match the `ForeignAssets::create` index on Asset Hub.
		pub const CreateAssetCall: [u8;2] = [53, 0];
		/// The pallet index of the Ethereum inbound queue pallet in the Bridge Hub runtime.
		pub const InboundQueuePalletInstance: u8 = 80;
		/// Default pricing parameters used to calculate bridging fees. Initialized to unit values,
        /// as it is intended that these parameters should be updated with more
        /// accurate values prior to bridge activation. This can be performed
        /// using the `EthereumSystem::set_pricing_parameters` governance extrinsic.
		pub Parameters: PricingParameters<Balance> = PricingParameters {
			// ETH/DOT exchange rate
			exchange_rate: FixedU128::from_rational(1, 1),
			// Ether fee per gas unit
			fee_per_gas: U256::one(),
			// Relayer rewards
			rewards: Rewards {
				// Reward for submitting a message to BridgeHub
				local: 1,
				// Reward for submitting a message to the Gateway contract on Ethereum
				remote: U256::one(),
			},
			// Safety factor to cover unfavourable fluctuations in the ETH/DOT exchange rate.
			multiplier: FixedU128::from_rational(1, 1),
		};
		/// Network and location for the Ethereum chain. On Polkadot, the Ethereum chain bridged
        /// to is the Ethereum Main network, with chain ID 1.
        /// <https://chainlist.org/chain/1>
        /// <https://ethereum.org/en/developers/docs/apis/json-rpc/#net_version>
		pub EthereumNetwork: NetworkId = NetworkId::Ethereum { chain_id: 11155111 };
	}
}
