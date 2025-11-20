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

use crate::*;

mod aliases;
mod claim_assets;
mod snowbridge;
mod snowbridge_common;
mod snowbridge_v2_config;
mod snowbridge_v2_inbound;
mod snowbridge_v2_outbound;
mod snowbridge_v2_outbound_edge_case;
mod snowbridge_v2_rewards;
mod teleport;

// USDT and wUSDT
pub(crate) fn usdt_at_ah_paseo() -> Location {
	Location::new(0, [PalletInstance(ASSETS_PALLET_ID), GeneralIndex(USDT_ID.into())])
}
