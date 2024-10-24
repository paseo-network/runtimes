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


mod snowbridge;
mod teleport;
mod claim_assets;



// pub(crate) fn asset_hub_kusama_location() -> Location {
// 	Location::new(
// 		2,
// 		[GlobalConsensus(NetworkId::Kusama), Parachain(AssetHubKusama::para_id().into())],
// 	)
// }
//
// pub(crate) fn bridge_hub_kusama_location() -> Location {
// 	Location::new(
// 		2,
// 		[GlobalConsensus(NetworkId::Kusama), Parachain(BridgeHubKusama::para_id().into())],
// 	)
// }
//
// pub(crate) fn send_asset_from_asset_hub_paseo(
// 	destination: Location,
// 	(id, amount): (Location, u128),
// ) -> DispatchResult {
// 	let signed_origin =
// 		<AssetHubPaseo as Chain>::RuntimeOrigin::signed(AssetHubPaseoSender::get());
//
// 	let beneficiary: Location =
// 		AccountId32Junction { network: None, id: AssetHubKusamaReceiver::get().into() }.into();
//
// 	let assets: Assets = (id, amount).into();
// 	let fee_asset_item = 0;
//
// 	AssetHubPaseo::execute_with(|| {
// 		<AssetHubPaseo as AssetHubPaseoPallet>::PolkadotXcm::limited_reserve_transfer_assets(
// 			signed_origin,
// 			bx!(destination.into()),
// 			bx!(beneficiary.into()),
// 			bx!(assets.into()),
// 			fee_asset_item,
// 			WeightLimit::Unlimited,
// 		)
// 	})
// }
//
// pub(crate) fn assert_bridge_hub_paseo_message_accepted(expected_processed: bool) {
// 	BridgeHubPaseo::execute_with(|| {
// 		type RuntimeEvent = <BridgeHubPaseo as Chain>::RuntimeEvent;
//
// 		if expected_processed {
// 			assert_expected_events!(
// 				BridgeHubPaseo,
// 				vec![
// 					// pay for bridge fees
// 					RuntimeEvent::Balances(pallet_balances::Event::Burned { .. }) => {},
// 					// message exported
// 					RuntimeEvent::BridgeKusamaMessages(
// 						pallet_bridge_messages::Event::MessageAccepted { .. }
// 					) => {},
// 					// message processed successfully
// 					RuntimeEvent::MessageQueue(
// 						pallet_message_queue::Event::Processed { success: true, .. }
// 					) => {},
// 				]
// 			);
// 		} else {
// 			assert_expected_events!(
// 				BridgeHubPaseo,
// 				vec![
// 					RuntimeEvent::MessageQueue(pallet_message_queue::Event::Processed {
// 						success: false,
// 						..
// 					}) => {},
// 				]
// 			);
// 		}
// 	});
// }
//
// pub(crate) fn assert_bridge_hub_kusama_message_received() {
// 	BridgeHubKusama::execute_with(|| {
// 		type RuntimeEvent = <BridgeHubKusama as Chain>::RuntimeEvent;
// 		assert_expected_events!(
// 			BridgeHubKusama,
// 			vec![
// 				// message sent to destination
// 				RuntimeEvent::XcmpQueue(
// 					cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }
// 				) => {},
// 			]
// 		);
// 	})
// }
//
