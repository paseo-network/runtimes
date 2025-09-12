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

pub use asset_hub_paseo_emulated_chain;
pub use bridge_hub_paseo_emulated_chain;
pub use collectives_paseo_emulated_chain;
pub use coretime_paseo_emulated_chain;
pub use paseo_emulated_chain;
pub use penpal_emulated_chain;
pub use people_paseo_emulated_chain;

use asset_hub_paseo_emulated_chain::AssetHubPaseo;
use bridge_hub_paseo_emulated_chain::BridgeHubPaseo;
use collectives_paseo_emulated_chain::CollectivesPaseo;
use coretime_paseo_emulated_chain::CoretimePaseo;
use paseo_emulated_chain::Paseo;
use penpal_emulated_chain::{PenpalA, PenpalB};
use people_paseo_emulated_chain::PeoplePaseo;

// Cumulus
use emulated_integration_tests_common::{
	accounts::{ALICE, BOB},
	xcm_emulator::{decl_test_networks, decl_test_sender_receiver_accounts_parameter_types},
};

decl_test_networks! {
	pub struct PaseoMockNet {
		relay_chain = Paseo,
		parachains = vec![
			AssetHubPaseo,
			BridgeHubPaseo,
			CollectivesPaseo,
			CoretimePaseo,
			PenpalA,
			PenpalB,
			PeoplePaseo,
		],
		bridge = ()
	},
}

decl_test_sender_receiver_accounts_parameter_types! {
	PaseoRelay { sender: ALICE, receiver: BOB },
	AssetHubPaseoPara { sender: ALICE, receiver: BOB },
	BridgeHubPaseoPara { sender: ALICE, receiver: BOB },
	CollectivesPaseoPara { sender: ALICE, receiver: BOB },
	CoretimePaseoPara { sender: ALICE, receiver: BOB },
	PenpalAPara { sender: ALICE, receiver: BOB },
	PenpalBPara { sender: ALICE, receiver: BOB },
	PeoplePaseoPara { sender: ALICE, receiver: BOB }
}
