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
use asset_hub_paseo_runtime::xcm_config::{
	bridging::to_ethereum::{BridgeHubEthereumBaseFee, EthereumNetwork},
	RelayTreasuryPalletAccount,
};
use bp_bridge_hub_paseo::snowbridge::CreateAssetCall;
use bridge_hub_paseo_runtime::{
	bridge_to_ethereum_config::EthereumGatewayAddress, EthereumBeaconClient, EthereumInboundQueue,
	Runtime, RuntimeOrigin,
};
use codec::{Decode, Encode};
use emulated_integration_tests_common::{xcm_emulator::ConvertLocation, RESERVABLE_ASSET_ID};
use frame_support::pallet_prelude::TypeInfo;
use hex_literal::hex;
use paseo_system_emulated_network::{
	asset_hub_paseo_emulated_chain::genesis::AssetHubPaseoAssetOwner,
	penpal_emulated_chain::CustomizableAssetFromSystemAssetHub,
	BridgeHubPaseoParaSender as BridgeHubPaseoSender,
};
use snowbridge_beacon_primitives::{
	types::deneb, AncestryProof, BeaconHeader, ExecutionProof, VersionedExecutionPayloadHeader,
};
use snowbridge_core::{
	gwei,
	inbound::{InboundQueueFixture, Log, Message, Proof},
	meth,
	outbound::OperatingMode,
	AssetMetadata, Rewards, TokenIdOf,
};
use snowbridge_pallet_system::PricingParametersOf;
use snowbridge_router_primitives::inbound::{
	Command, Destination, GlobalConsensusEthereumConvertsFor, MessageV1, VersionedMessage,
};
use sp_core::{H160, H256, U256};
use sp_runtime::{DispatchError::Token, FixedU128, TokenError::FundsUnavailable};
use system_parachains_constants::paseo::currency::UNITS;

const INITIAL_FUND: u128 = 5_000_000_000 * POLKADOT_ED;
pub const CHAIN_ID: u64 = 11155111;
pub const WETH: [u8; 20] = hex!("87d1f7fdfEe7f651FaBc8bFCB6E086C278b77A7d");
pub const ETHEREUM_DESTINATION_ADDRESS: [u8; 20] = hex!("44a57ee2f2FCcb85FDa2B0B18EBD0D8D2333700e");
pub const GATEWAY_ADDRESS: [u8; 20] = hex!("EDa338E4dC46038493b885327842fD3E301CaB39");

const INSUFFICIENT_XCM_FEE: u128 = 1000;
const XCM_FEE: u128 = 4_000_000_000;
const TOKEN_AMOUNT: u128 = 100_000_000_000;
const AH_BASE_FEE: u128 = 2_750_872_500_000u128;
const MIN_ETHER_BALANCE: u128 = 15_000_000_000_000;
const ETHER_TOKEN_ADDRESS: [u8; 20] = [0; 20];

#[derive(Encode, Decode, Debug, PartialEq, Eq, Clone, TypeInfo)]
pub enum ControlCall {
	#[codec(index = 3)]
	CreateAgent,
	#[codec(index = 4)]
	CreateChannel { mode: OperatingMode },
}

#[allow(clippy::large_enum_variant)]
#[derive(Encode, Decode, Debug, PartialEq, Eq, Clone, TypeInfo)]
pub enum SnowbridgeControl {
	#[codec(index = 83)]
	Control(ControlCall),
}

pub fn send_inbound_message(fixture: InboundQueueFixture) -> DispatchResult {
	EthereumBeaconClient::store_finalized_header(
		fixture.finalized_header,
		fixture.block_roots_root,
	)
	.unwrap();

	EthereumInboundQueue::submit(
		RuntimeOrigin::signed(BridgeHubPaseoSender::get()),
		fixture.message,
	)
}

/// Tests the registering of a token as an asset on AssetHub.
#[test]
fn register_weth_token_from_ethereum_to_asset_hub() {
	// Fund AH sovereign account on BH so that it can pay execution fees.
	BridgeHubPaseo::fund_para_sovereign(AssetHubPaseo::para_id(), INITIAL_FUND);
	// Fund ethereum sovereign account on AssetHub.
	AssetHubPaseo::fund_accounts(vec![(ethereum_sovereign_account(), INITIAL_FUND)]);

	BridgeHubPaseo::execute_with(|| {
		type RuntimeEvent = <BridgeHubPaseo as Chain>::RuntimeEvent;

		assert_ok!(<BridgeHubPaseo as Chain>::System::set_storage(
			<BridgeHubPaseo as Chain>::RuntimeOrigin::root(),
			vec![(EthereumGatewayAddress::key().to_vec(), H160(GATEWAY_ADDRESS).encode())],
		));
		// Construct RegisterToken message and sent to inbound queue
		let message = VersionedMessage::V1(MessageV1 {
			chain_id: CHAIN_ID,
			command: Command::RegisterToken { token: WETH.into(), fee: XCM_FEE },
		});
		// Convert the message to XCM
		let (xcm, _) = EthereumInboundQueue::do_convert([0; 32].into(), message).unwrap();
		let _ = EthereumInboundQueue::send_xcm(xcm, AssetHubPaseo::para_id()).unwrap();

		assert_expected_events!(
			BridgeHubPaseo,
			vec![
				RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},
			]
		);
	});

	AssetHubPaseo::execute_with(|| {
		type RuntimeEvent = <AssetHubPaseo as Chain>::RuntimeEvent;

		assert_expected_events!(
			AssetHubPaseo,
			vec![
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Created { .. }) => {},
			]
		);
	});
}

/// Tests sending a token to a 3rd party parachain, called PenPal. The token reserve is
/// still located on AssetHub.
#[test]
fn send_token_from_ethereum_to_penpal() {
	let asset_hub_sovereign = BridgeHubPaseo::sovereign_account_id_of(Location::new(
		1,
		[Parachain(AssetHubPaseo::para_id().into())],
	));

	// The Weth asset location, identified by the contract address on Ethereum
	let weth_asset_location: Location =
		(Parent, Parent, EthereumNetwork::get(), AccountKey20 { network: None, key: WETH }).into();
	// Converts the Weth asset location into an asset ID
	let weth_asset_id = weth_asset_location.clone();

	// Fund ethereum sovereign on AssetHub
	AssetHubPaseo::fund_accounts(vec![(ethereum_sovereign_account(), INITIAL_FUND)]);

	// Create asset on the Penpal parachain.
	PenpalB::execute_with(|| {
		// Set the trusted asset location from AH, in this case, Ethereum.
		assert_ok!(<PenpalB as Chain>::System::set_storage(
			<PenpalB as Chain>::RuntimeOrigin::root(),
			vec![(
				CustomizableAssetFromSystemAssetHub::key().to_vec(),
				Location::new(2, [GlobalConsensus(Ethereum { chain_id: CHAIN_ID })]).encode(),
			)],
		));

		assert_ok!(<PenpalB as PenpalBPallet>::ForeignAssets::create(
			<PenpalB as Chain>::RuntimeOrigin::signed(PenpalBSender::get()),
			weth_asset_location.clone(),
			asset_hub_sovereign.clone().into(),
			1000,
		));

		assert!(<PenpalB as PenpalBPallet>::ForeignAssets::asset_exists(weth_asset_location));
	});

	AssetHubPaseo::execute_with(|| {
		assert_ok!(<AssetHubPaseo as AssetHubPaseoPallet>::ForeignAssets::force_create(
			<AssetHubPaseo as Chain>::RuntimeOrigin::root(),
			weth_asset_id.clone(),
			asset_hub_sovereign.clone().into(),
			true,
			1000,
		));

		assert!(<AssetHubPaseo as AssetHubPaseoPallet>::ForeignAssets::asset_exists(
			weth_asset_id
		));
	});

	BridgeHubPaseo::execute_with(|| {
		type RuntimeEvent = <BridgeHubPaseo as Chain>::RuntimeEvent;

		type RuntimeOrigin = <BridgeHubPaseo as Chain>::RuntimeOrigin;

		// Fund AssetHub sovereign account so it can pay execution fees for the asset transfer
		assert_ok!(<BridgeHubPaseo as BridgeHubPaseoPallet>::Balances::force_set_balance(
			RuntimeOrigin::root(),
			asset_hub_sovereign.clone().into(),
			INITIAL_FUND,
		));

		let message_id: H256 = [1; 32].into();
		let message = VersionedMessage::V1(MessageV1 {
			chain_id: CHAIN_ID,
			command: Command::SendToken {
				token: WETH.into(),
				destination: Destination::ForeignAccountId32 {
					para_id: PenpalB::para_id().into(),
					id: PenpalBReceiver::get().into(),
					fee: 40_000_000_000,
				},
				amount: 1_000_000,
				fee: 40_000_000_000,
			},
		});
		// Convert the message to XCM
		let (xcm, _) = EthereumInboundQueue::do_convert(message_id, message).unwrap();
		// Send the XCM
		let _ = EthereumInboundQueue::send_xcm(xcm, AssetHubPaseo::para_id()).unwrap();

		assert_expected_events!(
			BridgeHubPaseo,
			vec![
				RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},
			]
		);
	});

	AssetHubPaseo::execute_with(|| {
		type RuntimeEvent = <AssetHubPaseo as Chain>::RuntimeEvent;
		// Check that the assets were issued on AssetHub
		assert_expected_events!(
			AssetHubPaseo,
			vec![
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { .. }) => {},
				RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},
			]
		);
	});

	PenpalB::execute_with(|| {
		type RuntimeEvent = <PenpalB as Chain>::RuntimeEvent;
		// Check that the assets were issued on PenPal
		assert_expected_events!(
			PenpalB,
			vec![
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { .. }) => {},
			]
		);
	});
}

/// Tests the registering of a token as an asset on AssetHub, and then subsequently sending
/// a token from Ethereum to AssetHub.
#[test]
fn send_weth_from_ethereum_to_asset_hub() {
	BridgeHubPaseo::fund_para_sovereign(AssetHubPaseo::para_id(), INITIAL_FUND);
	// Fund ethereum sovereign account on AssetHub.
	AssetHubPaseo::fund_accounts(vec![(ethereum_sovereign_account(), INITIAL_FUND)]);

	BridgeHubPaseo::execute_with(|| {
		type RuntimeEvent = <BridgeHubPaseo as Chain>::RuntimeEvent;

		assert_ok!(<BridgeHubPaseo as Chain>::System::set_storage(
			<BridgeHubPaseo as Chain>::RuntimeOrigin::root(),
			vec![(EthereumGatewayAddress::key().to_vec(), H160(GATEWAY_ADDRESS).encode())],
		));

		// Construct RegisterToken message and sent to inbound queue
		let message_id: H256 = [1; 32].into();
		let message = VersionedMessage::V1(MessageV1 {
			chain_id: CHAIN_ID,
			command: Command::RegisterToken { token: WETH.into(), fee: XCM_FEE },
		});
		// Convert the message to XCM
		let (xcm, _) = EthereumInboundQueue::do_convert(message_id, message).unwrap();
		// Send the XCM
		let _ = EthereumInboundQueue::send_xcm(xcm, AssetHubPaseo::para_id()).unwrap();

		assert_expected_events!(
			BridgeHubPaseo,
			vec![
				RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},
			]
		);

		// Construct SendToken message and sent to inbound queue
		let message = VersionedMessage::V1(MessageV1 {
			chain_id: CHAIN_ID,
			command: Command::SendToken {
				token: WETH.into(),
				destination: Destination::AccountId32 {
					id: AssetHubPaseoReceiver::get().into(),
				},
				amount: TOKEN_AMOUNT,
				fee: XCM_FEE,
			},
		});
		// Convert the message to XCM
		let (xcm, _) = EthereumInboundQueue::do_convert(message_id, message).unwrap();
		// Send the XCM
		let _ = EthereumInboundQueue::send_xcm(xcm, AssetHubPaseo::para_id()).unwrap();

		// Check that the message was sent
		assert_expected_events!(
			BridgeHubPaseo,
			vec![
				RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},
			]
		);
	});

	AssetHubPaseo::execute_with(|| {
		type RuntimeEvent = <AssetHubPaseo as Chain>::RuntimeEvent;

		// Check that the token was received and issued as a foreign asset on AssetHub
		assert_expected_events!(
			AssetHubPaseo,
			vec![
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { .. }) => {},
			]
		);
	});
}

// Performs a round trip tansfer of a token, asseting success.
fn send_token_from_ethereum_to_asset_hub_and_back_works(
	token_address: H160,
	amount: u128,
	asset_location: Location,
) {
	let assethub_sovereign = BridgeHubPaseo::sovereign_account_id_of(
		BridgeHubPaseo::sibling_location_of(AssetHubPaseo::para_id()),
	);

	BridgeHubPaseo::fund_accounts(vec![
		(assethub_sovereign.clone(), INITIAL_FUND),
		(RelayTreasuryPalletAccount::get(), INITIAL_FUND),
	]);
	AssetHubPaseo::fund_accounts(vec![
		(AssetHubPaseoReceiver::get(), INITIAL_FUND),
		(ethereum_sovereign_account(), INITIAL_FUND),
	]);

	// Set base transfer fee to Ethereum on AH.
	AssetHubPaseo::execute_with(|| {
		type RuntimeOrigin = <AssetHubPaseo as Chain>::RuntimeOrigin;

		assert_ok!(<AssetHubPaseo as Chain>::System::set_storage(
			RuntimeOrigin::root(),
			vec![(BridgeHubEthereumBaseFee::key().to_vec(), AH_BASE_FEE.encode())],
		));
	});

	// Send Token from Bridge Hub (simulates received Command from Ethereum)
	BridgeHubPaseo::execute_with(|| {
		type RuntimeEvent = <BridgeHubPaseo as Chain>::RuntimeEvent;

		assert_ok!(
			<BridgeHubPaseo as BridgeHubPaseoPallet>::EthereumSystem::set_pricing_parameters(
			<BridgeHubPaseo as Chain>::RuntimeOrigin::root(),
				PricingParametersOf::<Runtime> {
					exchange_rate: FixedU128::from_rational(1, 75),
					fee_per_gas: gwei(20),
					rewards: Rewards {
						local: (UNITS / 100), // 0.01 DOT
						remote: meth(1),
					},
					multiplier: FixedU128::from_rational(1, 1),
				}
			)
		);

		assert_ok!(<BridgeHubPaseo as Chain>::System::set_storage(
			<BridgeHubPaseo as Chain>::RuntimeOrigin::root(),
			vec![(EthereumGatewayAddress::key().to_vec(), H160(GATEWAY_ADDRESS).encode())],
		));

		let message_id: H256 = [1; 32].into();
		// Construct SendToken message and sent to inbound queue
		let message = VersionedMessage::V1(MessageV1 {
			chain_id: CHAIN_ID,
			command: Command::SendToken {
				token: token_address,
				destination: Destination::AccountId32 {
					id: AssetHubPaseoReceiver::get().into(),
				},
				amount,
				fee: XCM_FEE,
			},
		});
		// Convert the message to XCM
		let (xcm, _) = EthereumInboundQueue::do_convert(message_id, message).unwrap();
		// Send the XCM
		let _ = EthereumInboundQueue::send_xcm(xcm, AssetHubPaseo::para_id()).unwrap();

		// Check that the message was sent
		assert_expected_events!(
			BridgeHubPaseo,
			vec![
				RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},
			]
		);
	});

	// Receive Token on Asset Hub.
	AssetHubPaseo::execute_with(|| {
		type RuntimeEvent = <AssetHubPaseo as Chain>::RuntimeEvent;

		// Check that the token was received and issued as a foreign asset on AssetHub
		assert_expected_events!(
			AssetHubPaseo,
			vec![
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { asset_id, .. }) => {
					asset_id: *asset_id == asset_location,
				},
			]
		);
	});

	let treasury_account_before = BridgeHubPaseo::execute_with(|| {
		<<BridgeHubPaseo as BridgeHubPaseoPallet>::Balances as frame_support::traits::fungible::Inspect<_>>::balance(&RelayTreasuryPalletAccount::get())
	});

	// Send Token from Asset Hub back to Ethereum.
	AssetHubPaseo::execute_with(|| {
		type RuntimeOrigin = <AssetHubPaseo as Chain>::RuntimeOrigin;

		let assets = vec![Asset { id: AssetId(asset_location), fun: Fungible(amount) }];
		let versioned_assets = VersionedAssets::from(Assets::from(assets));

		let destination = VersionedLocation::from(Location::new(
			2,
			[GlobalConsensus(Ethereum { chain_id: CHAIN_ID })],
		));

		let beneficiary = VersionedLocation::from(Location::new(
			0,
			[AccountKey20 { network: None, key: ETHEREUM_DESTINATION_ADDRESS }],
		));

		let free_balance_before =
			<AssetHubPaseo as AssetHubPaseoPallet>::Balances::free_balance(
				AssetHubPaseoReceiver::get(),
			);
		// Send the Token back to Ethereum
		assert_ok!(
			<AssetHubPaseo as AssetHubPaseoPallet>::PolkadotXcm::limited_reserve_transfer_assets(
				RuntimeOrigin::signed(AssetHubPaseoReceiver::get()),
				Box::new(destination),
				Box::new(beneficiary),
				Box::new(versioned_assets),
				0,
				Unlimited,
			)
		);

		let free_balance_after =
			<AssetHubPaseo as AssetHubPaseoPallet>::Balances::free_balance(
				AssetHubPaseoReceiver::get(),
			);
		// Assert at least DefaultBridgeHubEthereumBaseFee charged from the sender
		let free_balance_diff = free_balance_before - free_balance_after;
		assert!(free_balance_diff > AH_BASE_FEE);
	});

	// Check that message with Token was queued on the BridgeHub
	BridgeHubPaseo::execute_with(|| {
		type RuntimeEvent = <BridgeHubPaseo as Chain>::RuntimeEvent;
		// check the outbound queue
		assert_expected_events!(
			BridgeHubPaseo,
			vec![
				RuntimeEvent::EthereumOutboundQueue(snowbridge_pallet_outbound_queue::Event::MessageQueued {..}) => {},
			]
		);

		// check treasury account balance on BH after (should receive some fees)
		let treasury_account_after = <<BridgeHubPaseo as BridgeHubPaseoPallet>::Balances as frame_support::traits::fungible::Inspect<_>>::balance(&RelayTreasuryPalletAccount::get());
		let local_fee = treasury_account_after - treasury_account_before;

		let events = BridgeHubPaseo::events();
		// Check that the local fee was credited to the Snowbridge sovereign account
		assert!(
			events.iter().any(|event| matches!(
				event,
				RuntimeEvent::Balances(pallet_balances::Event::Minted { who, amount: fee_minted })
					if *who == RelayTreasuryPalletAccount::get() && *fee_minted == local_fee
			)),
			"Snowbridge sovereign takes local fee."
		);
		// Check that the remote delivery fee was credited to the AssetHub sovereign account
		assert!(
			events.iter().any(|event| matches!(
				event,
				RuntimeEvent::Balances(pallet_balances::Event::Minted { who, .. })
					if *who == assethub_sovereign,
			)),
			"AssetHub sovereign takes remote fee."
		);
	});
}

/// Tests sending Ether from Ethereum to Asset Hub and back to Ethereum
#[test]
fn send_eth_asset_from_asset_hub_to_ethereum() {
	let ether_location: Location = (Parent, Parent, EthereumNetwork::get()).into();

	// Register Ether as foreign asset on AH.
	AssetHubPaseo::execute_with(|| {
		type RuntimeEvent = <AssetHubPaseo as Chain>::RuntimeEvent;
		type RuntimeOrigin = <AssetHubPaseo as Chain>::RuntimeOrigin;

		assert_ok!(<AssetHubPaseo as AssetHubPaseoPallet>::ForeignAssets::force_create(
			RuntimeOrigin::root(),
			ether_location.clone(),
			ethereum_sovereign_account().into(),
			true,
			MIN_ETHER_BALANCE,
		));

		assert_expected_events!(
			AssetHubPaseo,
			vec![
				RuntimeEvent::ForeignAssets(pallet_assets::Event::ForceCreated { .. }) => {},
			]
		);
	});

	// Perform a roundtrip transfer of Ether
	send_token_from_ethereum_to_asset_hub_and_back_works(
		ETHER_TOKEN_ADDRESS.into(),
		MIN_ETHER_BALANCE + TOKEN_AMOUNT,
		ether_location,
	);
}

/// Tests the full cycle of token transfers:
/// - registering a token on AssetHub
/// - sending a token to AssetHub
/// - returning the token to Ethereum
#[test]
fn send_weth_asset_from_asset_hub_to_ethereum() {
	// Register WETH on Asset Hub
	register_weth_token_from_ethereum_to_asset_hub();

	let weth_location: Location =
		(Parent, Parent, EthereumNetwork::get(), AccountKey20 { network: None, key: WETH }).into();
	// Perform a roundtrip transfer of WETH
	send_token_from_ethereum_to_asset_hub_and_back_works(WETH.into(), TOKEN_AMOUNT, weth_location);
}

#[test]
fn register_weth_token_in_asset_hub_fail_for_insufficient_fee() {
	BridgeHubPaseo::fund_para_sovereign(AssetHubPaseo::para_id(), INITIAL_FUND);

	BridgeHubPaseo::execute_with(|| {
		type RuntimeEvent = <BridgeHubPaseo as Chain>::RuntimeEvent;

		assert_ok!(<BridgeHubPaseo as Chain>::System::set_storage(
			<BridgeHubPaseo as Chain>::RuntimeOrigin::root(),
			vec![(EthereumGatewayAddress::key().to_vec(), H160(GATEWAY_ADDRESS).encode())],
		));

		let message_id: H256 = [1; 32].into();
		let message = VersionedMessage::V1(MessageV1 {
			chain_id: CHAIN_ID,
			command: Command::RegisterToken { token: WETH.into(), fee: INSUFFICIENT_XCM_FEE },
		});
		// Convert the message to XCM
		let (xcm, _) = EthereumInboundQueue::do_convert(message_id, message).unwrap();
		// Send the XCM
		let _ = EthereumInboundQueue::send_xcm(xcm, AssetHubPaseo::para_id()).unwrap();

		assert_expected_events!(
			BridgeHubPaseo,
			vec![
				RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},
			]
		);
	});

	AssetHubPaseo::execute_with(|| {
		type RuntimeEvent = <AssetHubPaseo as Chain>::RuntimeEvent;

		assert_expected_events!(
			AssetHubPaseo,
			vec![
				RuntimeEvent::MessageQueue(pallet_message_queue::Event::Processed { success:false, .. }) => {},
			]
		);
	});
}

#[test]
fn send_token_from_ethereum_to_asset_hub_fail_for_insufficient_fund() {
	// Insufficient fund
	BridgeHubPaseo::fund_para_sovereign(AssetHubPaseo::para_id(), 1_000);

	BridgeHubPaseo::execute_with(|| {
		assert_ok!(<BridgeHubPaseo as Chain>::System::set_storage(
			<BridgeHubPaseo as Chain>::RuntimeOrigin::root(),
			vec![(EthereumGatewayAddress::key().to_vec(), H160(GATEWAY_ADDRESS).encode())],
		));

		assert_err!(send_inbound_message(make_register_token_message()), Token(FundsUnavailable));
	});
}

/// Tests that the EthereumInboundQueue CreateAssetCall parameter on BridgeHub matches
/// the ForeignAssets::create call on AssetHub.
#[test]
fn asset_hub_foreign_assets_pallet_is_configured_correctly_in_bridge_hub() {
	let assethub_sovereign = BridgeHubPaseo::sovereign_account_id_of(Location::new(
		1,
		[Parachain(AssetHubPaseo::para_id().into())],
	));

	let call_create_foreign_assets =
		<AssetHubPaseo as Chain>::RuntimeCall::ForeignAssets(pallet_assets::Call::<
			<AssetHubPaseo as Chain>::Runtime,
			pallet_assets::Instance2,
		>::create {
			id: v4::Location::default(),
			min_balance: ASSET_MIN_BALANCE,
			admin: assethub_sovereign.into(),
		})
		.encode();

	let bridge_hub_inbound_queue_assets_pallet_call_index = CreateAssetCall::get();

	assert!(
		call_create_foreign_assets.starts_with(&bridge_hub_inbound_queue_assets_pallet_call_index)
	);
}

fn ethereum_sovereign_account() -> AccountId {
	let origin_location = (Parent, Parent, EthereumNetwork::get()).into();
	GlobalConsensusEthereumConvertsFor::<AccountId>::convert_location(&origin_location).unwrap()
}

fn make_register_token_message() -> InboundQueueFixture {
	InboundQueueFixture{
		message: Message {
			event_log: Log{
				address: hex!("eda338e4dc46038493b885327842fd3e301cab39").into(),
				topics: vec![
					hex!("7153f9357c8ea496bba60bf82e67143e27b64462b49041f8e689e1b05728f84f").into(),
					hex!("c173fac324158e77fb5840738a1a541f633cbec8884c6a601c567d2b376a0539").into(),
					hex!("5f7060e971b0dc81e63f0aa41831091847d97c1a4693ac450cc128c7214e65e0").into(),
				],
				data: hex!("00000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000002e0001000000000000000087d1f7fdfee7f651fabc8bfcb6e086c278b77a7d00e40b54020000000000000000000000000000000000000000000000000000000000").into(),
			},
			proof: Proof {
				receipt_proof: (vec![
					hex!("4a98e45a319168b0fc6005ce6b744ee9bf54338e2c0784b976a8578d241ced0f").to_vec(),
				], vec![
					hex!("f9028c30b9028802f90284018301d205b9010000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000080000000000000000000000000000004000000000080000000000000000000000000000000000010100000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000000040004000000000000002000002000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100000000000000000200000000000010f90179f85894eda338e4dc46038493b885327842fd3e301cab39e1a0f78bb28d4b1d7da699e5c0bc2be29c2b04b5aab6aacf6298fe5304f9db9c6d7ea000000000000000000000000087d1f7fdfee7f651fabc8bfcb6e086c278b77a7df9011c94eda338e4dc46038493b885327842fd3e301cab39f863a07153f9357c8ea496bba60bf82e67143e27b64462b49041f8e689e1b05728f84fa0c173fac324158e77fb5840738a1a541f633cbec8884c6a601c567d2b376a0539a05f7060e971b0dc81e63f0aa41831091847d97c1a4693ac450cc128c7214e65e0b8a000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000002e0001000000000000000087d1f7fdfee7f651fabc8bfcb6e086c278b77a7d00e40b54020000000000000000000000000000000000000000000000000000000000").to_vec(),
				]),
				execution_proof: ExecutionProof {
					header: BeaconHeader {
						slot: 393,
						proposer_index: 4,
						parent_root: hex!("6545b47a614a1dd4cad042a0cdbbf5be347e8ffcdc02c6c64540d5153acebeef").into(),
						state_root: hex!("b62ac34a8cb82497be9542fe2114410c9f6021855b766015406101a1f3d86434").into(),
						body_root: hex!("308e4c20194c0c77155c65a2d2c7dcd0ec6a7b20bdeb002c065932149fe0aa1b").into(),
					},
					ancestry_proof: Some(AncestryProof {
						header_branch: vec![
							hex!("6545b47a614a1dd4cad042a0cdbbf5be347e8ffcdc02c6c64540d5153acebeef").into(),
							hex!("fa84cc88ca53a72181599ff4eb07d8b444bce023fe2347c3b4f51004c43439d3").into(),
							hex!("cadc8ae211c6f2221c9138e829249adf902419c78eb4727a150baa4d9a02cc9d").into(),
							hex!("33a89962df08a35c52bd7e1d887cd71fa7803e68787d05c714036f6edf75947c").into(),
							hex!("2c9760fce5c2829ef3f25595a703c21eb22d0186ce223295556ed5da663a82cf").into(),
							hex!("e1aa87654db79c8a0ecd6c89726bb662fcb1684badaef5cd5256f479e3c622e1").into(),
							hex!("aa70d5f314e4a1fbb9c362f3db79b21bf68b328887248651fbd29fc501d0ca97").into(),
							hex!("160b6c235b3a1ed4ef5f80b03ee1c76f7bf3f591c92fca9d8663e9221b9f9f0f").into(),
							hex!("f68d7dcd6a07a18e9de7b5d2aa1980eb962e11d7dcb584c96e81a7635c8d2535").into(),
							hex!("1d5f912dfd6697110dd1ecb5cb8e77952eef57d85deb373572572df62bb157fc").into(),
							hex!("ffff0ad7e659772f9534c195c815efc4014ef1e1daed4404c06385d11192e92b").into(),
							hex!("6cf04127db05441cd833107a52be852868890e4317e6a02ab47683aa75964220").into(),
							hex!("b7d05f875f140027ef5118a2247bbb84ce8f2f0f1123623085daf7960c329f5f").into(),
						],
						finalized_block_root: hex!("751414cd97c0624f922b3e80285e9f776b08fa22fd5f87391f2ed7ef571a8d46").into(),
					}),
					execution_header: VersionedExecutionPayloadHeader::Deneb(deneb::ExecutionPayloadHeader {
						parent_hash: hex!("8092290aa21b7751576440f77edd02a94058429ce50e63a92d620951fb25eda2").into(),
						fee_recipient: hex!("0000000000000000000000000000000000000000").into(),
						state_root: hex!("96a83e9ddf745346fafcb0b03d57314623df669ed543c110662b21302a0fae8b").into(),
						receipts_root: hex!("62d13e9a073dc7cf609005b5531bb208c8686f18f7c8ae02d76232d83ae41a21").into(),
						logs_bloom: hex!("00000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000080000000400000000000000000000004000000000080000000000000000000000000000000000010100000000000000000000000000000000020000000000000000000000000000000000080000000000000000000000000000040004000000000000002002002000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000080000000000000000000000000000000000100000000000000000200000200000010").into(),
						prev_randao: hex!("62e309d4f5119d1f5c783abc20fc1a549efbab546d8d0b25ff1cfd58be524e67").into(),
						block_number: 393,
						gas_limit: 54492273,
						gas_used: 199644,
						timestamp: 1710552813,
						extra_data: hex!("d983010d0b846765746888676f312e32312e368664617277696e").into(),
						base_fee_per_gas: U256::from(7u64),
						block_hash: hex!("6a9810efb9581d30c1a5c9074f27c68ea779a8c1ae31c213241df16225f4e131").into(),
						transactions_root: hex!("2cfa6ed7327e8807c7973516c5c32a68ef2459e586e8067e113d081c3bd8c07d").into(),
						withdrawals_root: hex!("792930bbd5baac43bcc798ee49aa8185ef76bb3b44ba62b91d86ae569e4bb535").into(),
						blob_gas_used: 0,
						excess_blob_gas: 0,
					}),
					execution_branch: vec![
						hex!("a6833fa629f3286b6916c6e50b8bf089fc9126bee6f64d0413b4e59c1265834d").into(),
						hex!("b46f0c01805fe212e15907981b757e6c496b0cb06664224655613dcec82505bb").into(),
						hex!("db56114e00fdd4c1f85c892bf35ac9a89289aaecb1ebd0a96cde606a748b5d71").into(),
						hex!("d3af7c05c516726be7505239e0b9c7cb53d24abce6b91cdb3b3995f0164a75da").into(),
					],
				}
			}
		},
		finalized_header: BeaconHeader {
			slot: 864,
			proposer_index: 4,
			parent_root: hex!("614e7672f991ac268cd841055973f55e1e42228831a211adef207bb7329be614").into(),
			state_root: hex!("5fa8dfca3d760e4242ab46d529144627aa85348a19173b6e081172c701197a4a").into(),
			body_root: hex!("0f34c083b1803666bb1ac5e73fa71582731a2cf37d279ff0a3b0cad5a2ff371e").into(),
		},
		block_roots_root: hex!("3adb5c78afd49ef17160ca7fc38b47228cbb13a317709c86bb6f51d799ba9ab6").into(),
	}
}

fn send_token_from_ethereum_to_asset_hub_with_fee(account_id: [u8; 32], fee: u128) {
	let weth_asset_location: Location = Location::new(
		2,
		[EthereumNetwork::get().into(), AccountKey20 { network: None, key: WETH }],
	);
	// Fund asset hub sovereign on bridge hub
	let asset_hub_sovereign = BridgeHubPaseo::sovereign_account_id_of(Location::new(
		1,
		[Parachain(AssetHubPaseo::para_id().into())],
	));
	BridgeHubPaseo::fund_accounts(vec![(asset_hub_sovereign.clone(), INITIAL_FUND)]);

	// Register WETH
	AssetHubPaseo::execute_with(|| {
		type RuntimeOrigin = <AssetHubPaseo as Chain>::RuntimeOrigin;

		assert_ok!(<AssetHubPaseo as AssetHubPaseoPallet>::ForeignAssets::force_create(
			RuntimeOrigin::root(),
			weth_asset_location.clone(),
			asset_hub_sovereign.into(),
			false,
			1,
		));

		assert!(<AssetHubPaseo as AssetHubPaseoPallet>::ForeignAssets::asset_exists(
			weth_asset_location.clone(),
		));
	});

	// Send WETH to an existent account on asset hub
	BridgeHubPaseo::execute_with(|| {
		type RuntimeEvent = <BridgeHubPaseo as Chain>::RuntimeEvent;

		type EthereumInboundQueue =
			<BridgeHubPaseo as BridgeHubPaseoPallet>::EthereumInboundQueue;
		let message_id: H256 = [0; 32].into();
		let message = VersionedMessage::V1(MessageV1 {
			chain_id: CHAIN_ID,
			command: Command::SendToken {
				token: WETH.into(),
				destination: Destination::AccountId32 { id: account_id },
				amount: 1_000_000,
				fee,
			},
		});
		let (xcm, _) = EthereumInboundQueue::do_convert(message_id, message).unwrap();
		assert_ok!(EthereumInboundQueue::send_xcm(xcm, AssetHubPaseo::para_id()));

		// Check that the message was sent
		assert_expected_events!(
			BridgeHubPaseo,
			vec![
				RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},
			]
		);
	});
}

#[test]
fn send_token_from_ethereum_to_existent_account_on_asset_hub() {
	send_token_from_ethereum_to_asset_hub_with_fee(AssetHubPaseoSender::get().into(), XCM_FEE);

	AssetHubPaseo::execute_with(|| {
		type RuntimeEvent = <AssetHubPaseo as Chain>::RuntimeEvent;

		// Check that the token was received and issued as a foreign asset on AssetHub
		assert_expected_events!(
			AssetHubPaseo,
			vec![
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { .. }) => {},
			]
		);
	});
}

#[test]
fn send_token_from_ethereum_to_non_existent_account_on_asset_hub() {
	send_token_from_ethereum_to_asset_hub_with_fee([1; 32], XCM_FEE);

	AssetHubPaseo::execute_with(|| {
		type RuntimeEvent = <AssetHubPaseo as Chain>::RuntimeEvent;

		// Check that the token was received and issued as a foreign asset on AssetHub
		assert_expected_events!(
			AssetHubPaseo,
			vec![
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { .. }) => {},
			]
		);
	});
}

#[test]
fn send_token_from_ethereum_to_non_existent_account_on_asset_hub_with_insufficient_fee() {
	send_token_from_ethereum_to_asset_hub_with_fee([1; 32], INSUFFICIENT_XCM_FEE);

	AssetHubPaseo::execute_with(|| {
		type RuntimeEvent = <AssetHubPaseo as Chain>::RuntimeEvent;

		// Check that the message was not processed successfully due to insufficient fee

		assert_expected_events!(
			AssetHubPaseo,
			vec![
				RuntimeEvent::MessageQueue(pallet_message_queue::Event::Processed { success:false, .. }) => {},
			]
		);
	});
}

#[test]
fn send_token_from_ethereum_to_non_existent_account_on_asset_hub_with_sufficient_fee_but_do_not_satisfy_ed(
) {
	// On AH the xcm fee is 26_789_690 and the ED is 3_300_000
	send_token_from_ethereum_to_asset_hub_with_fee([1; 32], 30_000_000);

	AssetHubPaseo::execute_with(|| {
		type RuntimeEvent = <AssetHubPaseo as Chain>::RuntimeEvent;

		// Check that the message was not processed successfully due to insufficient ED
		assert_expected_events!(
			AssetHubPaseo,
			vec![
				RuntimeEvent::MessageQueue(pallet_message_queue::Event::Processed { success:false, .. }) => {},
			]
		);
	});
}

#[test]
fn transfer_relay_token() {
	let assethub_sovereign = BridgeHubPaseo::sovereign_account_id_of(
		BridgeHubPaseo::sibling_location_of(AssetHubPaseo::para_id()),
	);
	BridgeHubPaseo::fund_accounts(vec![(assethub_sovereign.clone(), INITIAL_FUND)]);

	let asset_id: Location = Location { parents: 1, interior: [].into() };
	let expected_asset_id: Location =
		Location { parents: 1, interior: [GlobalConsensus(Polkadot)].into() };

	let expected_token_id = TokenIdOf::convert_location(&expected_asset_id).unwrap();

	let ethereum_sovereign: AccountId =
		GlobalConsensusEthereumConvertsFor::<[u8; 32]>::convert_location(&Location::new(
			2,
			[GlobalConsensus(EthereumNetwork::get())],
		))
		.unwrap()
		.into();

	// Register token
	BridgeHubPaseo::execute_with(|| {
		type RuntimeOrigin = <BridgeHubPaseo as Chain>::RuntimeOrigin;
		type RuntimeEvent = <BridgeHubPaseo as Chain>::RuntimeEvent;

		assert_ok!(<BridgeHubPaseo as BridgeHubPaseoPallet>::Balances::force_set_balance(
			RuntimeOrigin::root(),
			sp_runtime::MultiAddress::Id(BridgeHubPaseoSender::get()),
			INITIAL_FUND * 10,
		));

		assert_ok!(<BridgeHubPaseo as BridgeHubPaseoPallet>::EthereumSystem::register_token(
			RuntimeOrigin::root(),
			Box::new(VersionedLocation::V4(asset_id.clone())),
			AssetMetadata {
				name: "wnd".as_bytes().to_vec().try_into().unwrap(),
				symbol: "wnd".as_bytes().to_vec().try_into().unwrap(),
				decimals: 12,
			},
		));
		// Check that a message was sent to Ethereum to create the agent
		assert_expected_events!(
			BridgeHubPaseo,
			vec![RuntimeEvent::EthereumSystem(snowbridge_pallet_system::Event::RegisterToken { .. }) => {},]
		);
	});

	// Send token to Ethereum
	AssetHubPaseo::execute_with(|| {
		type RuntimeOrigin = <AssetHubPaseo as Chain>::RuntimeOrigin;
		type RuntimeEvent = <AssetHubPaseo as Chain>::RuntimeEvent;

		// Set base transfer fee to Ethereum on AH.
		assert_ok!(<AssetHubPaseo as Chain>::System::set_storage(
			<AssetHubPaseo as Chain>::RuntimeOrigin::root(),
			vec![(BridgeHubEthereumBaseFee::key().to_vec(), AH_BASE_FEE.encode())],
		));

		let assets = vec![Asset { id: AssetId(Location::parent()), fun: Fungible(TOKEN_AMOUNT) }];
		let versioned_assets = VersionedAssets::V4(Assets::from(assets));

		let destination = VersionedLocation::V4(Location::new(
			2,
			[GlobalConsensus(Ethereum { chain_id: CHAIN_ID })],
		));

		let beneficiary = VersionedLocation::V4(Location::new(
			0,
			[AccountKey20 { network: None, key: ETHEREUM_DESTINATION_ADDRESS }],
		));

		assert_ok!(<AssetHubPaseo as AssetHubPaseoPallet>::PolkadotXcm::limited_reserve_transfer_assets(
			RuntimeOrigin::signed(AssetHubPaseoSender::get()),
			Box::new(destination),
			Box::new(beneficiary),
			Box::new(versioned_assets),
			0,
			Unlimited,
		));

		let events = AssetHubPaseo::events();
		// Check that the native asset transferred to some reserved account(sovereign of Ethereum)
		assert!(
			events.iter().any(|event| matches!(
				event,
				RuntimeEvent::Balances(pallet_balances::Event::Transfer { amount, to, ..})
					if *amount == TOKEN_AMOUNT && *to == ethereum_sovereign.clone(),
			)),
			"native token reserved to Ethereum sovereign account."
		);
	});

	// Send token back from ethereum
	BridgeHubPaseo::execute_with(|| {
		type RuntimeEvent = <BridgeHubPaseo as Chain>::RuntimeEvent;

		// Check that the transfer token back to Ethereum message was queue in the Ethereum
		// Outbound Queue
		assert_expected_events!(
			BridgeHubPaseo,
			vec![RuntimeEvent::EthereumOutboundQueue(snowbridge_pallet_outbound_queue::Event::MessageQueued{ .. }) => {},]
		);

		// Send relay token back to AH
		let message_id: H256 = [0; 32].into();
		let message = VersionedMessage::V1(MessageV1 {
			chain_id: CHAIN_ID,
			command: Command::SendNativeToken {
				token_id: expected_token_id,
				destination: Destination::AccountId32 {
					id: AssetHubPaseoReceiver::get().into(),
				},
				amount: TOKEN_AMOUNT,
				fee: XCM_FEE,
			},
		});
		// Convert the message to XCM
		let (xcm, _) = EthereumInboundQueue::do_convert(message_id, message).unwrap();
		// Send the XCM
		let _ = EthereumInboundQueue::send_xcm(xcm, AssetHubPaseo::para_id()).unwrap();

		assert_expected_events!(
			BridgeHubPaseo,
			vec![RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},]
		);
	});

	AssetHubPaseo::execute_with(|| {
		type RuntimeEvent = <AssetHubPaseo as Chain>::RuntimeEvent;

		assert_expected_events!(
			AssetHubPaseo,
			vec![RuntimeEvent::Balances(pallet_balances::Event::Burned{ .. }) => {},]
		);

		let events = AssetHubPaseo::events();

		// Check that the native token burnt from some reserved account
		assert!(
			events.iter().any(|event| matches!(
				event,
				RuntimeEvent::Balances(pallet_balances::Event::Burned { who, ..})
					if *who == ethereum_sovereign.clone(),
			)),
			"native token burnt from Ethereum sovereign account."
		);

		// Check that the token was minted to beneficiary
		assert!(
			events.iter().any(|event| matches!(
				event,
				RuntimeEvent::Balances(pallet_balances::Event::Minted { who, amount })
					if *amount >= TOKEN_AMOUNT && *who == AssetHubPaseoReceiver::get()
			)),
			"Token minted to beneficiary."
		);
	});
}

#[test]
fn transfer_ah_token() {
	let assethub_sovereign = BridgeHubPaseo::sovereign_account_id_of(
		BridgeHubPaseo::sibling_location_of(AssetHubPaseo::para_id()),
	);
	BridgeHubPaseo::fund_accounts(vec![(assethub_sovereign.clone(), INITIAL_FUND)]);

	let ethereum_destination = Location::new(2, [GlobalConsensus(Ethereum { chain_id: CHAIN_ID })]);

	let ethereum_sovereign: AccountId =
		GlobalConsensusEthereumConvertsFor::<[u8; 32]>::convert_location(&ethereum_destination)
			.unwrap()
			.into();
	AssetHubPaseo::fund_accounts(vec![(ethereum_sovereign.clone(), INITIAL_FUND)]);

	let asset_id: Location =
		[PalletInstance(ASSETS_PALLET_ID), GeneralIndex(RESERVABLE_ASSET_ID.into())].into();

	let asset_id_in_bh: Location = Location::new(
		1,
		[
			Parachain(AssetHubPaseo::para_id().into()),
			PalletInstance(ASSETS_PALLET_ID),
			GeneralIndex(RESERVABLE_ASSET_ID.into()),
		],
	);

	let asset_id_after_reanchored = Location::new(
		1,
		[GlobalConsensus(Polkadot), Parachain(AssetHubPaseo::para_id().into())],
	)
	.appended_with(asset_id.clone().interior)
	.unwrap();

	let token_id = TokenIdOf::convert_location(&asset_id_after_reanchored).unwrap();

	// Register token
	BridgeHubPaseo::execute_with(|| {
		type RuntimeOrigin = <BridgeHubPaseo as Chain>::RuntimeOrigin;

		assert_ok!(<BridgeHubPaseo as BridgeHubPaseoPallet>::EthereumSystem::register_token(
			RuntimeOrigin::root(),
			Box::new(VersionedLocation::V4(asset_id_in_bh.clone())),
			AssetMetadata {
				name: "ah_asset".as_bytes().to_vec().try_into().unwrap(),
				symbol: "ah_asset".as_bytes().to_vec().try_into().unwrap(),
				decimals: 12,
			},
		));
	});

	// Mint some token
	AssetHubPaseo::mint_asset(
		<AssetHubPaseo as Chain>::RuntimeOrigin::signed(AssetHubPaseoAssetOwner::get()),
		RESERVABLE_ASSET_ID,
		AssetHubPaseoSender::get(),
		TOKEN_AMOUNT,
	);

	// Send token to Ethereum
	AssetHubPaseo::execute_with(|| {
		type RuntimeOrigin = <AssetHubPaseo as Chain>::RuntimeOrigin;
		type RuntimeEvent = <AssetHubPaseo as Chain>::RuntimeEvent;

		// Set base transfer fee to Ethereum on AH.
		assert_ok!(<AssetHubPaseo as Chain>::System::set_storage(
			<AssetHubPaseo as Chain>::RuntimeOrigin::root(),
			vec![(BridgeHubEthereumBaseFee::key().to_vec(), AH_BASE_FEE.encode())],
		));

		// Send partial of the token, will fail if send all
		let assets = vec![Asset { id: AssetId(asset_id.clone()), fun: Fungible(TOKEN_AMOUNT / 2) }];
		let versioned_assets = VersionedAssets::V4(Assets::from(assets));

		let beneficiary = VersionedLocation::V4(Location::new(
			0,
			[AccountKey20 { network: None, key: ETHEREUM_DESTINATION_ADDRESS }],
		));

		assert_ok!(<AssetHubPaseo as AssetHubPaseoPallet>::PolkadotXcm::limited_reserve_transfer_assets(
			RuntimeOrigin::signed(AssetHubPaseoSender::get()),
			Box::new(VersionedLocation::from(ethereum_destination)),
			Box::new(beneficiary),
			Box::new(versioned_assets),
			0,
			Unlimited,
		));

		assert_expected_events!(
			AssetHubPaseo,
			vec![RuntimeEvent::Assets(pallet_assets::Event::Transferred{ .. }) => {},]
		);

		let events = AssetHubPaseo::events();
		// Check that the native asset transferred to some reserved account(sovereign of Ethereum)
		assert!(
			events.iter().any(|event| matches!(
				event,
				RuntimeEvent::Assets(pallet_assets::Event::Transferred { asset_id, to, ..})
					if *asset_id == RESERVABLE_ASSET_ID && *to == ethereum_sovereign.clone()
			)),
			"native token reserved to Ethereum sovereign account."
		);
	});

	// Send token back from Ethereum
	BridgeHubPaseo::execute_with(|| {
		type RuntimeEvent = <BridgeHubPaseo as Chain>::RuntimeEvent;

		// Check that the transfer token back to Ethereum message was queue in the Ethereum
		// Outbound Queue
		assert_expected_events!(
			BridgeHubPaseo,
			vec![RuntimeEvent::EthereumOutboundQueue(snowbridge_pallet_outbound_queue::Event::MessageQueued{ .. }) => {},]
		);

		let message = VersionedMessage::V1(MessageV1 {
			chain_id: CHAIN_ID,
			command: Command::SendNativeToken {
				token_id,
				destination: Destination::AccountId32 {
					id: AssetHubPaseoReceiver::get().into(),
				},
				amount: TOKEN_AMOUNT / 10,
				fee: XCM_FEE,
			},
		});
		// Convert the message to XCM
		let (xcm, _) = EthereumInboundQueue::do_convert([0; 32].into(), message).unwrap();
		// Send the XCM
		let _ = EthereumInboundQueue::send_xcm(xcm, AssetHubPaseo::para_id()).unwrap();

		assert_expected_events!(
			BridgeHubPaseo,
			vec![RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},]
		);
	});

	AssetHubPaseo::execute_with(|| {
		type RuntimeEvent = <AssetHubPaseo as Chain>::RuntimeEvent;

		assert_expected_events!(
			AssetHubPaseo,
			vec![RuntimeEvent::Assets(pallet_assets::Event::Burned{..}) => {},]
		);

		let events = AssetHubPaseo::events();

		// Check that the native token burnt from some reserved account
		assert!(
			events.iter().any(|event| matches!(
				event,
				RuntimeEvent::Assets(pallet_assets::Event::Burned { owner, .. })
					if *owner == ethereum_sovereign.clone(),
			)),
			"token burnt from Ethereum sovereign account."
		);

		// Check that the token was minted to beneficiary
		assert!(
			events.iter().any(|event| matches!(
				event,
				RuntimeEvent::Assets(pallet_assets::Event::Issued { owner, .. })
					if *owner == AssetHubPaseoReceiver::get()
			)),
			"Token minted to beneficiary."
		);
	});
}
