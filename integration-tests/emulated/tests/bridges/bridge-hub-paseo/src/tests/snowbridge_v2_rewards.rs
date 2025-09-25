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

use crate::{
	tests::snowbridge_common::{
		eth_location, set_up_eth_and_pas_pool_on_paseo_asset_hub, INITIAL_FUND,
	},
	*,
};
use bridge_hub_paseo_runtime::bridge_common_config::{BridgeReward, BridgeRewardBeneficiaries};
use pallet_bridge_relayers::{Error::FailedToPayReward, RewardLedger};

#[test]
fn claim_rewards_works() {
	let assethub_location = BridgeHubPaseo::sibling_location_of(AssetHubPaseo::para_id());
	let assethub_sovereign = BridgeHubPaseo::sovereign_account_id_of(assethub_location);

	let relayer_account = BridgeHubPaseoSender::get();
	let reward_address = AssetHubPaseoReceiver::get();

	BridgeHubPaseo::fund_accounts(vec![
		(assethub_sovereign.clone(), INITIAL_FUND),
		(relayer_account.clone(), INITIAL_FUND),
	]);
	set_up_eth_and_pas_pool_on_paseo_asset_hub();

	BridgeHubPaseo::execute_with(|| {
		type RuntimeEvent = <BridgeHubPaseo as Chain>::RuntimeEvent;
		type RuntimeOrigin = <BridgeHubPaseo as Chain>::RuntimeOrigin;
		let reward_amount = MIN_ETHER_BALANCE * 2; // Reward should be more than Ether min balance

		type BridgeRelayers = <BridgeHubPaseo as BridgeHubPaseoPallet>::BridgeRelayers;
		BridgeRelayers::register_reward(
			&relayer_account.clone(),
			BridgeReward::Snowbridge,
			reward_amount,
		);

		// Check that the reward was registered.
		assert_expected_events!(
			BridgeHubPaseo,
			vec![
				RuntimeEvent::BridgeRelayers(pallet_bridge_relayers::Event::RewardRegistered { relayer, reward_kind, reward_balance }) => {
					relayer: *relayer == relayer_account,
					reward_kind: *reward_kind == BridgeReward::Snowbridge,
					reward_balance: *reward_balance == reward_amount,
				},
			]
		);

		let relayer_location = Location::new(
			0,
			[Junction::AccountId32 { id: reward_address.clone().into(), network: None }],
		);
		let reward_beneficiary = BridgeRewardBeneficiaries::AssetHubLocation(Box::new(
			VersionedLocation::V5(relayer_location),
		));
		let result = BridgeRelayers::claim_rewards_to(
			RuntimeOrigin::signed(relayer_account.clone()),
			BridgeReward::Snowbridge,
			reward_beneficiary.clone(),
		);
		assert_ok!(result);

		assert_expected_events!(
			BridgeHubPaseo,
			vec![
				// Check that the pay reward event was emitted on BH
				RuntimeEvent::BridgeRelayers(pallet_bridge_relayers::Event::RewardPaid { relayer, reward_kind, reward_balance, beneficiary }) => {
					relayer: *relayer == relayer_account,
					reward_kind: *reward_kind == BridgeReward::Snowbridge,
					reward_balance: *reward_balance == reward_amount,
					beneficiary: *beneficiary == reward_beneficiary,
				},
			]
		);
	});

	AssetHubPaseo::execute_with(|| {
		type RuntimeEvent = <AssetHubPaseo as Chain>::RuntimeEvent;
		assert_expected_events!(
			AssetHubPaseo,
			vec![
				// Check that the reward was paid on AH
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { asset_id, owner, .. }) => {
					asset_id: *asset_id == eth_location(),
					owner: *owner == reward_address.clone(),
				},
			]
		);
	})
}

#[test]
fn claim_snowbridge_rewards_to_local_account_fails() {
	let assethub_location = BridgeHubPaseo::sibling_location_of(AssetHubPaseo::para_id());
	let assethub_sovereign = BridgeHubPaseo::sovereign_account_id_of(assethub_location);

	let relayer_account = BridgeHubPaseoSender::get();
	let reward_address = AssetHubPaseoReceiver::get();

	BridgeHubPaseo::fund_accounts(vec![
		(assethub_sovereign.clone(), INITIAL_FUND),
		(relayer_account.clone(), INITIAL_FUND),
	]);
	set_up_eth_and_pas_pool_on_paseo_asset_hub();

	BridgeHubPaseo::execute_with(|| {
		type Runtime = <BridgeHubPaseo as Chain>::Runtime;
		type RuntimeEvent = <BridgeHubPaseo as Chain>::RuntimeEvent;
		type RuntimeOrigin = <BridgeHubPaseo as Chain>::RuntimeOrigin;
		let reward_amount = MIN_ETHER_BALANCE * 2; // Reward should be more than Ether min balance

		type BridgeRelayers = <BridgeHubPaseo as BridgeHubPaseoPallet>::BridgeRelayers;
		BridgeRelayers::register_reward(
			&relayer_account.clone(),
			BridgeReward::Snowbridge,
			reward_amount,
		);

		// Check that the reward was registered.
		assert_expected_events!(
			BridgeHubPaseo,
			vec![
				RuntimeEvent::BridgeRelayers(pallet_bridge_relayers::Event::RewardRegistered { relayer, reward_kind, reward_balance }) => {
					relayer: *relayer == relayer_account,
					reward_kind: *reward_kind == BridgeReward::Snowbridge,
					reward_balance: *reward_balance == reward_amount,
				},
			]
		);

		let reward_beneficiary = BridgeRewardBeneficiaries::LocalAccount(reward_address);
		let result = BridgeRelayers::claim_rewards_to(
			RuntimeOrigin::signed(relayer_account.clone()),
			BridgeReward::Snowbridge,
			reward_beneficiary.clone(),
		);
		assert_err!(result, FailedToPayReward::<Runtime, ()>);
	})
}
