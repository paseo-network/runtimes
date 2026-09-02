// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Individuality.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;
use crate::mock::*;

use frame_support::{assert_ok, traits::Currency};
use pallet_revive::{
	precompiles::alloy::{
		primitives::Bytes,
		sol_types::{Revert, SolCall, SolError, SolInterface},
	},
	sp_runtime::Weight,
	ExecConfig, TransactionLimits,
};
use sp_runtime::AccountId32;
use IScarcityCollection::IScarcityCollectionCalls;
use IScarcityFactory::IScarcityFactoryCalls;

fn key(bytes: &[u8]) -> MetadataKeyOf<Test> {
	bytes.to_vec().try_into().unwrap()
}

fn value(bytes: &[u8]) -> MetadataValueOf<Test> {
	bytes.to_vec().try_into().unwrap()
}

/// Call `target` with `input` and return the full result including consumed weight.
fn call_full(
	caller: &AccountId32,
	target: H160,
	input: Vec<u8>,
) -> pallet_revive::ContractResult<pallet_revive::ExecReturnValue, u64> {
	pallet_revive::Pallet::<Test>::bare_call(
		RuntimeOrigin::signed(caller.clone()),
		target,
		0u32.into(),
		TransactionLimits::WeightAndDeposit { weight_limit: Weight::MAX, deposit_limit: u64::MAX },
		input,
		&ExecConfig::new_substrate_tx(),
	)
}

/// Call `target` with `input` and return the raw execution result.
fn call_precompile(
	caller: &AccountId32,
	target: H160,
	input: Vec<u8>,
) -> pallet_revive::ExecReturnValue {
	call_full(caller, target, input).result.expect("precompile call should execute")
}

/// Call `target` with `input`, attaching `value`, and return the raw execution result.
fn call_with_value(
	caller: &AccountId32,
	target: H160,
	input: Vec<u8>,
	value: u64,
) -> pallet_revive::ExecReturnValue {
	pallet_revive::Pallet::<Test>::bare_call(
		RuntimeOrigin::signed(caller.clone()),
		target,
		value.into(),
		TransactionLimits::WeightAndDeposit { weight_limit: Weight::MAX, deposit_limit: u64::MAX },
		input,
		&ExecConfig::new_substrate_tx(),
	)
	.result
	.expect("precompile call should execute")
}

/// The account a precompile address maps to, where stray value would land.
fn precompile_account(address: H160) -> AccountId32 {
	<Test as pallet_revive::Config>::AddressMapper::to_fallback_account_id(&address)
}

/// Call `target` with `input`, expecting success, and return the output data.
fn call_ok(caller: &AccountId32, target: H160, input: Vec<u8>) -> Vec<u8> {
	let output = call_precompile(caller, target, input);
	assert!(!output.did_revert(), "expected success, got revert: {output:?}");
	output.data
}

/// Call `target` with `input`, expecting a revert whose reason contains `reason`.
fn call_reverted_with(caller: &AccountId32, target: H160, input: Vec<u8>, reason: &str) {
	let output = call_precompile(caller, target, input);
	assert!(output.did_revert(), "expected revert, got success: {output:?}");
	let decoded = Revert::abi_decode(&output.data).expect("revert data decodes as Error(string)");
	assert!(
		decoded.reason.contains(reason),
		"revert reason {:?} does not contain {reason:?}",
		decoded.reason
	);
}

fn assert_contract_event(contract: H160, event: impl IntoLogData) {
	let (topics, data) = event.into_log_data().split();
	let topics = topics.into_iter().map(|topic| H256(topic.0)).collect::<Vec<_>>();
	System::assert_has_event(RuntimeEvent::Revive(pallet_revive::Event::ContractEmitted {
		contract,
		data: data.to_vec(),
		topics,
	}));
}

/// How many EVM logs were produced, whatever their shape.
fn contract_event_count() -> usize {
	System::events()
		.into_iter()
		.filter(|record| {
			matches!(
				record.event,
				RuntimeEvent::Revive(pallet_revive::Event::ContractEmitted { .. })
			)
		})
		.count()
}

/// Assert no EVM log was produced at all, whatever its shape.
fn assert_no_contract_event() {
	let emitted = contract_event_count();
	assert_eq!(emitted, 0, "expected no EVM log, found {emitted}");
}

/// Assert exactly one EVM log was produced, and that it is `event` from `contract`.
fn assert_only_contract_event(contract: H160, event: impl IntoLogData) {
	let emitted = contract_event_count();
	assert_eq!(emitted, 1, "expected exactly one EVM log, found {emitted}");
	assert_contract_event(contract, event);
}

/// An eth-derived purse key: the fallback account of an H160 round-trips to the same H160,
/// so ERC-721 answers can be compared against the address directly.
fn purse(byte: u8) -> (H160, AccountId32) {
	let address = H160([byte; 20]);
	let account = <Test as pallet_revive::Config>::AddressMapper::to_account_id(&address);
	(address, account)
}

fn setup_collection(owner: &AccountId32) -> CollectionId {
	map_account(owner);
	pallet_scarcity::Pallet::<Test>::do_create_collection(owner.clone()).unwrap()
}

fn setup_item(owner: &AccountId32, collection: CollectionId) -> pallet_scarcity::ItemIndex {
	setup_item_as(owner, collection, Transferability::Transferable)
}

fn setup_item_as(
	owner: &AccountId32,
	collection: CollectionId,
	transferability: Transferability,
) -> pallet_scarcity::ItemIndex {
	pallet_scarcity::Pallet::<Test>::do_define_item(
		owner.clone(),
		collection,
		transferability,
		alloc::vec![],
	)
	.unwrap()
}

#[test]
fn factory_creates_collection() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		map_account(&alice);

		let data = call_ok(
			&alice,
			factory_address(),
			IScarcityFactory::createCollectionCall {}.abi_encode(),
		);
		let collection = IScarcityFactory::createCollectionCall::abi_decode_returns(&data).unwrap();

		assert_eq!(collection, 0);
		assert_eq!(Collections::<Test>::get(0).unwrap().owner, alice);
		assert_contract_event(
			factory_address(),
			IScarcityFactory::CollectionCreated {
				collection: 0,
				owner: address_of::<Test>(&alice),
			},
		);
	});
}

#[test]
fn erc721_reads_work() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(alice.clone()),
			collection,
			key(NAME_KEY),
			Some(value(b"Duckies"))
		));
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(alice.clone()),
			collection,
			key(SYMBOL_KEY),
			Some(value(b"DUCK"))
		));
		let (holder_address, holder) = purse(0xBB);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder,
			alloc::vec![],
		)
		.unwrap();

		let target = collection_address(collection);
		let token = pallet_revive::precompiles::alloy::primitives::U256::from(instance);

		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::ownerOfCall { tokenId: token }.abi_encode(),
		);
		let owner = IScarcityCollection::ownerOfCall::abi_decode_returns(&data).unwrap();
		assert_eq!(owner.into_array(), holder_address.0);

		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::balanceOfCall { owner: holder_address.0.into() }.abi_encode(),
		);
		let balance = IScarcityCollection::balanceOfCall::abi_decode_returns(&data).unwrap();
		assert_eq!(balance, pallet_revive::precompiles::alloy::primitives::U256::ONE);

		let data = call_ok(&alice, target, IScarcityCollection::nameCall {}.abi_encode());
		let name = IScarcityCollection::nameCall::abi_decode_returns(&data).unwrap();
		assert_eq!(name, "Duckies");

		let data = call_ok(&alice, target, IScarcityCollection::symbolCall {}.abi_encode());
		let symbol = IScarcityCollection::symbolCall::abi_decode_returns(&data).unwrap();
		assert_eq!(symbol, "DUCK");

		let data =
			call_ok(&alice, target, IScarcityCollection::itemSupplyCall { item }.abi_encode());
		let supply = IScarcityCollection::itemSupplyCall::abi_decode_returns(&data).unwrap();
		assert_eq!(supply.supply, 1);
		assert_eq!(supply.liveSupply, 1);

		let data =
			call_ok(&alice, target, IScarcityCollection::collectionOwnerCall {}.abi_encode());
		let owner = IScarcityCollection::collectionOwnerCall::abi_decode_returns(&data).unwrap();
		assert_eq!(owner, address_of::<Test>(&alice));

		for (id, expected) in [
			(ERC165_INTERFACE_ID, true),
			(ERC721_INTERFACE_ID, true),
			(ERC721_METADATA_INTERFACE_ID, true),
			(ERC5192_INTERFACE_ID, true),
			(ERC2981_INTERFACE_ID, true),
			(ERC4906_INTERFACE_ID, true),
			// ERC-721 Enumerable, which `tokenOfOwnerByIndex` alone does not earn: the id also
			// covers `totalSupply` and `tokenByIndex`, and claiming it would promise both.
			([0x78, 0x0e, 0x9d, 0x63], false),
			([0xff, 0xff, 0xff, 0xff], false),
		] {
			let data = call_ok(
				&alice,
				target,
				IScarcityCollection::supportsInterfaceCall { interfaceId: id.into() }.abi_encode(),
			);
			let supported =
				IScarcityCollection::supportsInterfaceCall::abi_decode_returns(&data).unwrap();
			assert_eq!(supported, expected, "interface id {id:?}");
		}
	});
}

#[test]
fn token_uri_resolves_scopes() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(alice.clone()),
			collection,
			key(TOKEN_URI_KEY),
			Some(value(b"ipfs://collection-default"))
		));
		let (_, holder) = purse(0xBB);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder,
			alloc::vec![],
		)
		.unwrap();

		let target = collection_address(collection);
		let token = pallet_revive::precompiles::alloy::primitives::U256::from(instance);
		let uri_call = IScarcityCollection::tokenURICall { tokenId: token }.abi_encode();
		// The raw reader resolves the same three tiers as `tokenURI`, so both are asserted
		// at every tier.
		let raw_call = IScarcityCollection::instanceMetadataCall {
			tokenId: token,
			key: Bytes::from(TOKEN_URI_KEY.to_vec()),
		}
		.abi_encode();
		let resolved =
			|data: &[u8]| IScarcityCollection::tokenURICall::abi_decode_returns(data).unwrap();
		let resolved_raw = |data: &[u8]| {
			IScarcityCollection::instanceMetadataCall::abi_decode_returns(data).unwrap()
		};

		let data = call_ok(&alice, target, uri_call.clone());
		assert_eq!(resolved(&data), "ipfs://collection-default");
		let data = call_ok(&alice, target, raw_call.clone());
		assert_eq!(resolved_raw(&data).as_ref(), b"ipfs://collection-default");

		// The item tier: this is the only tier whose arguments the precompile chooses
		// (`nft.collection`/`nft.item`), and `item_metadata_of` falls back to collection
		// scope, so without this step a wrong item argument would still read as correct.
		assert_ok!(Scarcity::set_item_metadata(
			RuntimeOrigin::signed(alice.clone()),
			collection,
			item,
			key(TOKEN_URI_KEY),
			Some(value(b"ipfs://item-default"))
		));
		let data = call_ok(&alice, target, uri_call.clone());
		assert_eq!(resolved(&data), "ipfs://item-default");
		let data = call_ok(&alice, target, raw_call.clone());
		assert_eq!(resolved_raw(&data).as_ref(), b"ipfs://item-default");

		assert_ok!(Scarcity::set_instance_metadata(
			RuntimeOrigin::signed(alice.clone()),
			instance,
			key(TOKEN_URI_KEY),
			Some(value(b"ipfs://instance-override"))
		));
		let data = call_ok(&alice, target, uri_call);
		assert_eq!(resolved(&data), "ipfs://instance-override");
		let data = call_ok(&alice, target, raw_call);
		assert_eq!(resolved_raw(&data).as_ref(), b"ipfs://instance-override");
	});
}

#[test]
fn item_metadata_is_read_from_the_instance_own_item() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item_a = setup_item(&alice, collection);
		let item_b = setup_item(&alice, collection);
		// Distinct values per item, no collection default: reading the wrong item cannot be
		// masked by a fallback.
		for (item, uri) in [(item_a, b"ipfs://a".to_vec()), (item_b, b"ipfs://b".to_vec())] {
			assert_ok!(Scarcity::set_item_metadata(
				RuntimeOrigin::signed(alice.clone()),
				collection,
				item,
				key(TOKEN_URI_KEY),
				Some(value(&uri))
			));
		}
		let (_, holder) = purse(0xBB);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item_b,
			holder,
			alloc::vec![],
		)
		.unwrap();

		let data = call_ok(
			&alice,
			collection_address(collection),
			IScarcityCollection::tokenURICall { tokenId: U256::from(instance) }.abi_encode(),
		);
		let uri = IScarcityCollection::tokenURICall::abi_decode_returns(&data).unwrap();
		assert_eq!(uri, "ipfs://b");
	});
}

#[test]
fn collection_metadata_reads() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(alice.clone()),
			collection,
			key(b"website"),
			Some(value(b"https://example.test"))
		));
		let target = collection_address(collection);

		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::collectionMetadataCall { key: Bytes::from(b"website".to_vec()) }
				.abi_encode(),
		);
		let website =
			IScarcityCollection::collectionMetadataCall::abi_decode_returns(&data).unwrap();
		assert_eq!(website.as_ref(), b"https://example.test");

		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::collectionMetadataCall { key: Bytes::from(b"absent".to_vec()) }
				.abi_encode(),
		);
		let absent =
			IScarcityCollection::collectionMetadataCall::abi_decode_returns(&data).unwrap();
		assert!(absent.is_empty(), "an unset key reads as empty bytes, got {absent:?}");
	});
}

/// Write a royalty pair at collection scope: 20 raw address bytes and SCALE-encoded points.
fn set_collection_royalty(
	owner: &AccountId32,
	collection: CollectionId,
	receiver: H160,
	basis_points: u128,
) {
	assert_ok!(Scarcity::set_collection_metadata(
		RuntimeOrigin::signed(owner.clone()),
		collection,
		key(ROYALTY_RECEIVER_KEY),
		Some(value(&receiver.0))
	));
	assert_ok!(Scarcity::set_collection_metadata(
		RuntimeOrigin::signed(owner.clone()),
		collection,
		key(ROYALTY_BASIS_POINTS_KEY),
		Some(value(&codec::Encode::encode(&basis_points)))
	));
}

fn royalty_of(
	caller: &AccountId32,
	target: H160,
	token: U256,
	sale_price: u128,
) -> IScarcityCollection::royaltyInfoReturn {
	let data = call_ok(
		caller,
		target,
		IScarcityCollection::royaltyInfoCall { tokenId: token, salePrice: U256::from(sale_price) }
			.abi_encode(),
	);
	IScarcityCollection::royaltyInfoCall::abi_decode_returns(&data).unwrap()
}

/// Write one royalty key at item scope. `None` clears it, falling back to collection scope.
fn set_item_royalty_key(
	owner: &AccountId32,
	collection: CollectionId,
	item: pallet_scarcity::ItemIndex,
	metadata_key: &[u8],
	metadata_value: Option<&[u8]>,
) {
	assert_ok!(Scarcity::set_item_metadata(
		RuntimeOrigin::signed(owner.clone()),
		collection,
		item,
		key(metadata_key),
		metadata_value.map(value)
	));
}

/// ERC-2981 fixes the arithmetic but leaves the boundary cases to the implementation. These
/// follow the reference implementation's shape: floor division, and a zero on either side of
/// the product still naming the receiver.
#[test]
fn royalty_amount_floors_and_survives_zero_on_either_side() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (receiver_address, holder) = purse(0xBB);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder,
			alloc::vec![],
		)
		.unwrap();
		let target = collection_address(collection);
		let token = U256::from(instance);

		// A share that does not divide evenly truncates rather than rounding up, so a
		// marketplace never owes more than the exact fraction.
		set_collection_royalty(&alice, collection, receiver_address, 250);
		// 2.5% of 100 is 2.5, which floors to 2; below 40 the whole share rounds away.
		for (sale_price, expected) in [(1_000u128, 25u128), (100, 2), (10, 0), (0, 0)] {
			let quoted = royalty_of(&alice, target, token, sale_price);
			assert_eq!(quoted.royaltyAmount, U256::from(expected), "250 bps of {sale_price}");
			assert_eq!(
				quoted.receiver,
				Address::from(receiver_address.0),
				"a zero amount still names the receiver, at {sale_price}"
			);
		}

		// Zero basis points is a configured royalty of nothing, which is not the same as
		// having configured none: the receiver is still reported.
		set_collection_royalty(&alice, collection, receiver_address, 0);
		let quoted = royalty_of(&alice, target, token, 1_000);
		assert_eq!(quoted.receiver, Address::from(receiver_address.0));
		assert_eq!(quoted.royaltyAmount, U256::ZERO);

		// One basis point is the smallest configurable share and still floors.
		set_collection_royalty(&alice, collection, receiver_address, 1);
		assert_eq!(royalty_of(&alice, target, token, 10_000).royaltyAmount, U256::from(1u8));
		assert_eq!(royalty_of(&alice, target, token, 9_999).royaltyAmount, U256::ZERO);
	});
}

/// The reference implementation stores a receiver and a fraction together, so a token-level
/// override replaces both at once. Metadata keys resolve one at a time, so each key falls back
/// to the collection independently. That difference is deliberate and pinned here.
#[test]
fn royalty_item_scope_overrides_each_key_independently() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (collection_receiver, holder) = purse(0xBB);
		let (item_receiver, _) = purse(0xCC);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder,
			alloc::vec![],
		)
		.unwrap();
		let target = collection_address(collection);
		let token = U256::from(instance);
		set_collection_royalty(&alice, collection, collection_receiver, 250);

		// Overriding only the receiver keeps the collection's share.
		set_item_royalty_key(
			&alice,
			collection,
			item,
			ROYALTY_RECEIVER_KEY,
			Some(&item_receiver.0),
		);
		let quoted = royalty_of(&alice, target, token, 1_000);
		assert_eq!(quoted.receiver, Address::from(item_receiver.0));
		assert_eq!(quoted.royaltyAmount, U256::from(25u8));

		// Overriding only the share keeps the collection's receiver.
		set_item_royalty_key(&alice, collection, item, ROYALTY_RECEIVER_KEY, None);
		set_item_royalty_key(
			&alice,
			collection,
			item,
			ROYALTY_BASIS_POINTS_KEY,
			Some(&codec::Encode::encode(&1_000u128)),
		);
		let quoted = royalty_of(&alice, target, token, 1_000);
		assert_eq!(quoted.receiver, Address::from(collection_receiver.0));
		assert_eq!(quoted.royaltyAmount, U256::from(100u8));

		// An item-scope value that is unusable falls through to no royalty rather than to the
		// collection default, because resolution picks the value before judging it.
		set_item_royalty_key(&alice, collection, item, ROYALTY_BASIS_POINTS_KEY, Some(b"nope"));
		let quoted = royalty_of(&alice, target, token, 1_000);
		assert_eq!(quoted.receiver, Address::ZERO);
		assert_eq!(quoted.royaltyAmount, U256::ZERO);

		// Clearing the override restores the collection default.
		set_item_royalty_key(&alice, collection, item, ROYALTY_BASIS_POINTS_KEY, None);
		let quoted = royalty_of(&alice, target, token, 1_000);
		assert_eq!(quoted.receiver, Address::from(collection_receiver.0));
		assert_eq!(quoted.royaltyAmount, U256::from(25u8));

		// A zero receiver written at item scope means "no royalty", not "use the collection
		// default". Implementations that store the pair together read a zero token receiver as
		// the fallback sentinel, so this is the one place the two models mean opposite things:
		// here the collection's own receiver and share are both discarded.
		set_item_royalty_key(&alice, collection, item, ROYALTY_RECEIVER_KEY, Some(&[0u8; 20]));
		let quoted = royalty_of(&alice, target, token, 1_000);
		assert_eq!(quoted.receiver, Address::ZERO);
		assert_eq!(quoted.royaltyAmount, U256::ZERO);

		// Clearing it is what reaches the collection default, and separates that from the
		// zero-receiver case above.
		set_item_royalty_key(&alice, collection, item, ROYALTY_RECEIVER_KEY, None);
		let quoted = royalty_of(&alice, target, token, 1_000);
		assert_eq!(quoted.receiver, Address::from(collection_receiver.0));
		assert_eq!(quoted.royaltyAmount, U256::from(25u8));
	});
}

#[test]
fn royalty_info_resolves_item_over_collection_scope() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let other_item = setup_item(&alice, collection);
		let (holder_address, holder) = purse(0xBB);
		let (override_address, _) = purse(0xCC);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder.clone(),
			alloc::vec![],
		)
		.unwrap();
		let other_instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			other_item,
			purse(0xDD).1,
			alloc::vec![],
		)
		.unwrap();
		let target = collection_address(collection);
		let token = U256::from(instance);

		// Neither key set: no royalty, and no revert.
		let quoted = royalty_of(&alice, target, token, 1_000);
		assert_eq!(quoted.receiver, Address::ZERO);
		assert_eq!(quoted.royaltyAmount, U256::ZERO);

		set_collection_royalty(&alice, collection, holder_address, 250);
		let quoted = royalty_of(&alice, target, token, 1_000);
		assert_eq!(quoted.receiver, Address::from(holder_address.0));
		assert_eq!(quoted.royaltyAmount, U256::from(25u8), "250 bps of 1000");

		// An item-scope entry wins over the collection default for that item only.
		assert_ok!(Scarcity::set_item_metadata(
			RuntimeOrigin::signed(alice.clone()),
			collection,
			item,
			key(ROYALTY_RECEIVER_KEY),
			Some(value(&override_address.0))
		));
		let quoted = royalty_of(&alice, target, token, 1_000);
		assert_eq!(quoted.receiver, Address::from(override_address.0));
		assert_eq!(quoted.royaltyAmount, U256::from(25u8), "points still come from collection");

		let quoted = royalty_of(&alice, target, U256::from(other_instance), 1_000);
		assert_eq!(quoted.receiver, Address::from(holder_address.0), "sibling item unaffected");
	});
}

#[test]
fn royalty_info_tolerates_bad_terms_but_not_unknown_tokens() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (receiver_address, holder) = purse(0xBB);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder,
			alloc::vec![],
		)
		.unwrap();
		let target = collection_address(collection);
		let token = U256::from(instance);

		// A token that does not exist has no sale to price, and every other read on this
		// address answers unknown for it too.
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::royaltyInfoCall {
				tokenId: U256::from(instance + 1),
				salePrice: U256::from(1_000u16),
			}
			.abi_encode(),
			"unknown token",
		);

		// Exactly 100% is a valid fraction, and anchors the boundary the cases below cross.
		set_collection_royalty(&alice, collection, receiver_address, 10_000);
		let quoted = royalty_of(&alice, target, token, 1_000);
		assert_eq!(quoted.receiver, Address::from(receiver_address.0));
		assert_eq!(quoted.royaltyAmount, U256::from(1_000u16));

		// Every way a collection can misconfigure the keys answers "no royalty" rather than
		// reverting, so a marketplace settling the sale is never blocked by this call.
		for (name, receiver, basis_points) in [
			("receiver too short", value(b"short"), value(&codec::Encode::encode(&250u128))),
			("receiver too long", value(&[0x11; 21]), value(&codec::Encode::encode(&250u128))),
			("zero receiver", value(&[0u8; 20]), value(&codec::Encode::encode(&250u128))),
			("points not SCALE u128", value(&receiver_address.0), value(b"250")),
			(
				"points above 100%",
				value(&receiver_address.0),
				value(&codec::Encode::encode(&10_001u128)),
			),
		] {
			assert_ok!(Scarcity::set_collection_metadata(
				RuntimeOrigin::signed(alice.clone()),
				collection,
				key(ROYALTY_RECEIVER_KEY),
				Some(receiver)
			));
			assert_ok!(Scarcity::set_collection_metadata(
				RuntimeOrigin::signed(alice.clone()),
				collection,
				key(ROYALTY_BASIS_POINTS_KEY),
				Some(basis_points)
			));
			let quoted = royalty_of(&alice, target, token, 1_000);
			assert_eq!(quoted.receiver, Address::ZERO, "{name}");
			assert_eq!(quoted.royaltyAmount, U256::ZERO, "{name}");
		}

		// One key set and the other absent is the same answer.
		set_collection_royalty(&alice, collection, receiver_address, 250);
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(alice.clone()),
			collection,
			key(ROYALTY_RECEIVER_KEY),
			None
		));
		let quoted = royalty_of(&alice, target, token, 1_000);
		assert_eq!(quoted.receiver, Address::ZERO);
		assert_eq!(quoted.royaltyAmount, U256::ZERO);

		// Scaling before dividing keeps precision but can leave the range. This one still
		// reverts: a wrapped amount would quote a royalty unrelated to the sale.
		set_collection_royalty(&alice, collection, receiver_address, 10_000);
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::royaltyInfoCall { tokenId: token, salePrice: U256::MAX }
				.abi_encode(),
			"royalty exceeds the representable range",
		);

		// Burning is the other way a token stops being live, and settlement that quotes a
		// royalty after the fact hits it. The collection royalty is still configured, so this
		// asserts liveness rather than the terms.
		call_ok(&alice, target, IScarcityCollection::forceBurnCall { tokenId: token }.abi_encode());
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::royaltyInfoCall {
				tokenId: token,
				salePrice: U256::from(1_000u16),
			}
			.abi_encode(),
			"unknown token",
		);
	});
}

#[test]
fn contract_uri_reads_its_reserved_collection_key() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let sibling = pallet_scarcity::Pallet::<Test>::do_create_collection(alice.clone()).unwrap();
		let target = collection_address(collection);

		// A collection that has published nothing answers empty rather than reverting, so a
		// marketplace reader does not have to special-case it.
		let data = call_ok(&alice, target, IScarcityCollection::contractURICall {}.abi_encode());
		let unset = IScarcityCollection::contractURICall::abi_decode_returns(&data).unwrap();
		assert!(unset.is_empty(), "an unpublished contractURI reads as empty, got {unset:?}");

		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(alice.clone()),
			collection,
			key(CONTRACT_URI_KEY),
			Some(value(b"ipfs://collection.json"))
		));

		let data = call_ok(&alice, target, IScarcityCollection::contractURICall {}.abi_encode());
		let uri = IScarcityCollection::contractURICall::abi_decode_returns(&data).unwrap();
		assert_eq!(uri, "ipfs://collection.json");

		// The key is collection-scoped, so a sibling under the same prefix is unaffected.
		let data = call_ok(
			&alice,
			collection_address(sibling),
			IScarcityCollection::contractURICall {}.abi_encode(),
		);
		let sibling_uri = IScarcityCollection::contractURICall::abi_decode_returns(&data).unwrap();
		assert!(
			sibling_uri.is_empty(),
			"sibling collection reads its own key, got {sibling_uri:?}"
		);
	});
}

#[test]
fn cross_collection_lookups_answer_unknown() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection_a = setup_collection(&alice);
		let collection_b =
			pallet_scarcity::Pallet::<Test>::do_create_collection(alice.clone()).unwrap();
		let item = setup_item(&alice, collection_b);
		let (holder_address, holder) = purse(0xBB);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection_b,
			item,
			holder,
			alloc::vec![],
		)
		.unwrap();

		let token = pallet_revive::precompiles::alloy::primitives::U256::from(instance);

		// The instance lives in collection B, so collection A's address must not answer
		// for it.
		call_reverted_with(
			&alice,
			collection_address(collection_a),
			IScarcityCollection::ownerOfCall { tokenId: token }.abi_encode(),
			"unknown token",
		);
		let data = call_ok(
			&alice,
			collection_address(collection_a),
			IScarcityCollection::balanceOfCall { owner: holder_address.0.into() }.abi_encode(),
		);
		let balance = IScarcityCollection::balanceOfCall::abi_decode_returns(&data).unwrap();
		assert_eq!(balance, pallet_revive::precompiles::alloy::primitives::U256::ZERO);
	});
}

#[test]
fn mint_via_precompile_works() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (holder_address, holder) = purse(0xCC);
		let target = collection_address(collection);

		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::mintCall {
				item,
				to: holder_address.0.into(),
				keys: alloc::vec![Bytes::from(TOKEN_URI_KEY.to_vec())],
				values: alloc::vec![Bytes::from(b"ipfs://duck-1".to_vec())],
			}
			.abi_encode(),
		);
		let token = IScarcityCollection::mintCall::abi_decode_returns(&data).unwrap();

		let nft = NftsByOwner::<Test>::get(&holder).unwrap();
		assert_eq!(U256::from(nft.instance), token);
		assert_eq!(nft.collection, collection);
		assert_contract_event(
			target,
			IScarcityCollection::Transfer {
				from: Address::ZERO,
				to: holder_address.0.into(),
				tokenId: token,
			},
		);

		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::instanceMetadataCall {
				tokenId: token,
				key: Bytes::from(TOKEN_URI_KEY.to_vec()),
			}
			.abi_encode(),
		);
		let uri = IScarcityCollection::instanceMetadataCall::abi_decode_returns(&data).unwrap();
		assert_eq!(uri.as_ref(), b"ipfs://duck-1");
	});
}

#[test]
fn mint_requires_collection_owner() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let bob = id_to_account(2);
		map_account(&bob);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (holder_address, _) = purse(0xCC);

		call_reverted_with(
			&bob,
			collection_address(collection),
			IScarcityCollection::mintCall {
				item,
				to: holder_address.0.into(),
				keys: alloc::vec![],
				values: alloc::vec![],
			}
			.abi_encode(),
			"caller is not the collection owner",
		);
	});
}

#[test]
fn force_transfer_and_burn_work() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (from_address, from) = purse(0xBB);
		let (to_address, to) = purse(0xCC);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			from,
			alloc::vec![],
		)
		.unwrap();
		let target = collection_address(collection);
		let token = U256::from(instance);

		call_ok(
			&alice,
			target,
			IScarcityCollection::forceTransferCall { tokenId: token, to: to_address.0.into() }
				.abi_encode(),
		);
		assert_eq!(NftsByOwner::<Test>::get(&to).unwrap().instance, instance);
		assert_contract_event(
			target,
			IScarcityCollection::Transfer {
				from: from_address.0.into(),
				to: to_address.0.into(),
				tokenId: token,
			},
		);

		call_ok(&alice, target, IScarcityCollection::forceBurnCall { tokenId: token }.abi_encode());
		assert!(Instances::<Test>::get(instance).is_none());
		assert!(NftsByOwner::<Test>::get(&to).is_none());
		assert_contract_event(
			target,
			IScarcityCollection::Transfer {
				from: to_address.0.into(),
				to: Address::ZERO,
				tokenId: token,
			},
		);

		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::ownerOfCall { tokenId: token }.abi_encode(),
			"unknown token",
		);
	});
}

/// `forceTransfer` refuses the address `ownerOf` reports for the instance's own holder.
///
/// Occupying a key registers it, so the only holder left whose address does not resolve back is
/// one whose account was reaped afterwards. That address is a truncated hash resolving to the
/// fallback account, so the pallet compares two different accounts and its own self-transfer
/// check passes. Only the precompile can reject this.
#[test]
fn force_transfer_refuses_the_holders_own_address() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let target = collection_address(collection);

		// The mint registers the key, then funding and emptying it reaps the account and takes
		// that registration with it.
		let holder = id_to_account(9);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder.clone(),
			alloc::vec![],
		)
		.unwrap();
		assert!(<Test as pallet_revive::Config>::AddressMapper::is_mapped(&holder));
		Balances::make_free_balance_be(&holder, 1_000_000);
		Balances::make_free_balance_be(&holder, 0);
		assert!(!<Test as pallet_revive::Config>::AddressMapper::is_mapped(&holder));
		assert!(NftsByOwner::<Test>::contains_key(&holder));

		let token = U256::from(instance);
		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::ownerOfCall { tokenId: token }.abi_encode(),
		);
		let reported = IScarcityCollection::ownerOfCall::abi_decode_returns(&data).unwrap();
		assert_eq!(reported, address_of::<Test>(&holder));
		// The address does not resolve back to the holder, which is what makes the pallet's own
		// check miss it.
		assert_ne!(account_of::<Test>(&reported), holder);

		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::forceTransferCall { tokenId: token, to: reported }.abi_encode(),
			"destination already holds this instance",
		);
		assert_eq!(NftsByOwner::<Test>::get(&holder).unwrap().instance, instance);
	});
}

/// `mint` accepts the address `ownerOf` reports for a reaped holder and lands the instance on
/// the fallback account that address resolves to, not on the purse key.
///
/// A bare address carries nothing that tells a truncated purse key from an ordinary account no
/// one has mapped, so `mint` has no guard to apply and the interface documents the outcome
/// instead. This pins the blast radius: the purse key keeps what it holds, one address now
/// answers for two holders, and the collection owner moves the new instance back out.
#[test]
fn minting_to_a_reaped_holders_address_lands_on_the_fallback_account() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let target = collection_address(collection);

		// A mint destination is a key frame-system never created, so the hook is what registers
		// it. Funding that key and emptying it again is what drops the hook's own entry.
		let holder = id_to_account(9);
		assert!(!frame_system::Account::<Test>::contains_key(&holder));
		let held = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder.clone(),
			alloc::vec![],
		)
		.unwrap();
		assert!(<Test as pallet_revive::Config>::AddressMapper::is_mapped(&holder));
		Balances::make_free_balance_be(&holder, 1_000_000);
		Balances::make_free_balance_be(&holder, 0);
		assert!(!<Test as pallet_revive::Config>::AddressMapper::is_mapped(&holder));

		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::ownerOfCall { tokenId: U256::from(held) }.abi_encode(),
		);
		let reported = IScarcityCollection::ownerOfCall::abi_decode_returns(&data).unwrap();
		let fallback = account_of::<Test>(&reported);
		assert_ne!(fallback, holder);

		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::mintCall {
				item,
				to: reported,
				keys: alloc::vec![],
				values: alloc::vec![],
			}
			.abi_encode(),
		);
		let stranded = IScarcityCollection::mintCall::abi_decode_returns(&data).unwrap();

		assert_eq!(NftsByOwner::<Test>::get(&holder).unwrap().instance, held);
		assert_eq!(U256::from(NftsByOwner::<Test>::get(&fallback).unwrap().instance), stranded);

		// The fallback account is eth-derived, so `to_address` maps it back to the same address
		// the purse key reports. Both instances then name one address, and `balanceOf` sees only
		// the fallback.
		let owner_of = |token: U256| {
			let data = call_ok(
				&alice,
				target,
				IScarcityCollection::ownerOfCall { tokenId: token }.abi_encode(),
			);
			IScarcityCollection::ownerOfCall::abi_decode_returns(&data).unwrap()
		};
		assert_eq!(owner_of(U256::from(held)), reported);
		assert_eq!(owner_of(stranded), reported);
		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::balanceOfCall { owner: reported }.abi_encode(),
		);
		assert_eq!(
			IScarcityCollection::balanceOfCall::abi_decode_returns(&data).unwrap(),
			U256::ONE
		);

		// No key signs for the fallback account, but the collection owner keeps force authority
		// over the instance, so the stranding is recoverable without destroying it.
		let (rescue_address, rescue) = purse(0xDD);
		call_ok(
			&alice,
			target,
			IScarcityCollection::forceTransferCall {
				tokenId: stranded,
				to: rescue_address.0.into(),
			}
			.abi_encode(),
		);
		assert!(!NftsByOwner::<Test>::contains_key(&fallback));
		assert_eq!(U256::from(NftsByOwner::<Test>::get(&rescue).unwrap().instance), stranded);
		assert_eq!(NftsByOwner::<Test>::get(&holder).unwrap().instance, held);
	});
}

#[test]
fn approval_stubs_behave() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (holder_address, holder) = purse(0xBB);
		let (other_address, _) = purse(0xCC);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder.clone(),
			alloc::vec![],
		)
		.unwrap();
		let target = collection_address(collection);
		let token = U256::from(instance);

		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::approveCall { to: other_address.0.into(), tokenId: token }
				.abi_encode(),
			"approvals are not supported yet",
		);
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::setApprovalForAllCall {
				operator: other_address.0.into(),
				approved: true,
			}
			.abi_encode(),
			"approvals are not supported yet",
		);

		// The two argument shapes the ecosystem treats as safe no-ops revert like the rest.
		// Succeeding would owe an `Approval` or `ApprovalForAll` log for a mechanism that does not
		// exist, and inventing one is worse than refusing.
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::approveCall { to: Address::ZERO, tokenId: token }.abi_encode(),
			"approvals are not supported yet",
		);
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::setApprovalForAllCall {
				operator: other_address.0.into(),
				approved: false,
			}
			.abi_encode(),
			"approvals are not supported yet",
		);

		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::getApprovedCall { tokenId: token }.abi_encode(),
		);
		let approved = IScarcityCollection::getApprovedCall::abi_decode_returns(&data).unwrap();
		assert_eq!(approved, Address::ZERO);

		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::isApprovedForAllCall {
				owner: holder_address.0.into(),
				operator: other_address.0.into(),
			}
			.abi_encode(),
		);
		let is_operator =
			IScarcityCollection::isApprovedForAllCall::abi_decode_returns(&data).unwrap();
		assert!(!is_operator);

		// The instance is untouched by any of the above.
		assert_eq!(NftsByOwner::<Test>::get(&holder).unwrap().instance, instance);
	});
}

/// Mint one instance to an eth-derived purse and fund it so it can pay for its own EVM calls.
fn mint_to_funded_purse(
	owner: &AccountId32,
	collection: CollectionId,
	item: pallet_scarcity::ItemIndex,
	byte: u8,
) -> (H160, AccountId32, InstanceId) {
	let (address, account) = purse(byte);
	let instance = pallet_scarcity::Pallet::<Test>::do_mint(
		owner.clone(),
		collection,
		item,
		account.clone(),
		alloc::vec![],
	)
	.unwrap();
	Balances::make_free_balance_be(&account, u64::MAX / 2);
	(address, account, instance)
}

#[test]
fn holder_transfer_moves_the_token() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (holder_address, holder, instance) =
			mint_to_funded_purse(&alice, collection, item, 0xBB);
		let (to_address, to) = purse(0xCC);
		let target = collection_address(collection);
		let token = U256::from(instance);
		let nonce_before = NftsByOwner::<Test>::get(&holder).unwrap().state_nonce;

		call_ok(
			&holder,
			target,
			IScarcityCollection::transferFromCall {
				from: holder_address.0.into(),
				to: to_address.0.into(),
				tokenId: token,
			}
			.abi_encode(),
		);

		assert!(NftsByOwner::<Test>::get(&holder).is_none());
		let moved = NftsByOwner::<Test>::get(&to).expect("destination holds the token");
		assert_eq!(moved.instance, instance);
		assert_eq!(moved.collection, collection);
		// Any outstanding native authorization is invalidated by the move.
		assert_eq!(moved.state_nonce, nonce_before + 1);
		assert_eq!(Instances::<Test>::get(instance), Some(to));
		assert_contract_event(
			target,
			IScarcityCollection::Transfer {
				from: holder_address.0.into(),
				to: to_address.0.into(),
				tokenId: token,
			},
		);
	});
}

#[test]
fn holder_transfer_rejects_callers_that_are_not_the_holder() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (holder_address, holder, instance) =
			mint_to_funded_purse(&alice, collection, item, 0xBB);
		let (to_address, _) = purse(0xCC);
		let target = collection_address(collection);
		let token = U256::from(instance);
		let transfer = IScarcityCollection::transferFromCall {
			from: holder_address.0.into(),
			to: to_address.0.into(),
			tokenId: token,
		}
		.abi_encode();

		// The collection owner has `forceTransfer`, not the holder's authority, and no
		// approval mechanism exists to delegate that authority either.
		call_reverted_with(&alice, target, transfer.clone(), "caller does not hold this token");

		// A `from` that is not the current holder fails before authority is even considered.
		call_reverted_with(
			&holder,
			target,
			IScarcityCollection::transferFromCall {
				from: to_address.0.into(),
				to: holder_address.0.into(),
				tokenId: token,
			}
			.abi_encode(),
			"transfer from the wrong holder",
		);

		call_reverted_with(
			&holder,
			target,
			IScarcityCollection::transferFromCall {
				from: holder_address.0.into(),
				to: Address::ZERO,
				tokenId: token,
			}
			.abi_encode(),
			"destination is the zero address",
		);

		assert_eq!(NftsByOwner::<Test>::get(&holder).unwrap().instance, instance);
	});
}

#[test]
fn safe_transfer_reaches_purses_but_not_code() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (holder_address, holder, instance) =
			mint_to_funded_purse(&alice, collection, item, 0xBB);
		let (to_address, to) = purse(0xCC);
		let target = collection_address(collection);
		let token = U256::from(instance);

		// A precompile address carries code, so it stands in for any contract destination: the
		// acknowledgement cannot be asked for, so the move is undone rather than left to stand
		// unacknowledged. The holder assertion below covers that rollback.
		let with_code = Address::from(factory_address().0);
		call_reverted_with(
			&holder,
			target,
			IScarcityCollection::safeTransferFrom_1Call {
				from: holder_address.0.into(),
				to: with_code,
				tokenId: token,
			}
			.abi_encode(),
			"safe transfer to a contract is not supported yet",
		);
		call_reverted_with(
			&holder,
			target,
			IScarcityCollection::safeTransferFrom_0Call {
				from: holder_address.0.into(),
				to: with_code,
				tokenId: token,
				data: Bytes::from_static(b"payload"),
			}
			.abi_encode(),
			"safe transfer to a contract is not supported yet",
		);
		assert_eq!(NftsByOwner::<Test>::get(&holder).unwrap().instance, instance);

		// `transferFrom` makes no receiver guarantee, so the same destination is accepted
		// there. Moved straight back so the safe-path assertions below start from the holder.
		call_ok(
			&holder,
			target,
			IScarcityCollection::transferFromCall {
				from: holder_address.0.into(),
				to: with_code,
				tokenId: token,
			}
			.abi_encode(),
		);
		let code_purse = <Test as pallet_revive::Config>::AddressMapper::to_account_id(&H160(
			with_code.into_array(),
		));
		assert_eq!(NftsByOwner::<Test>::get(&code_purse).unwrap().instance, instance);
		assert_ok!(pallet_scarcity::Pallet::<Test>::do_transfer_by_holder(
			&code_purse,
			instance,
			holder.clone()
		));

		// A destination without code needs no callback, so it transfers like `transferFrom`.
		call_ok(
			&holder,
			target,
			IScarcityCollection::safeTransferFrom_0Call {
				from: holder_address.0.into(),
				to: to_address.0.into(),
				tokenId: token,
				data: Bytes::from_static(b"payload"),
			}
			.abi_encode(),
		);
		assert_eq!(NftsByOwner::<Test>::get(&to).unwrap().instance, instance);
		assert_contract_event(
			target,
			IScarcityCollection::Transfer {
				from: holder_address.0.into(),
				to: to_address.0.into(),
				tokenId: token,
			},
		);
	});
}

#[test]
fn holder_transfer_rejects_occupied_and_self_destinations() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (holder_address, holder, instance) =
			mint_to_funded_purse(&alice, collection, item, 0xBB);
		let (occupied_address, _, _) = mint_to_funded_purse(&alice, collection, item, 0xCC);
		let target = collection_address(collection);
		let token = U256::from(instance);

		call_reverted_with(
			&holder,
			target,
			IScarcityCollection::transferFromCall {
				from: holder_address.0.into(),
				to: occupied_address.0.into(),
				tokenId: token,
			}
			.abi_encode(),
			"destination purse already holds an instance",
		);
		call_reverted_with(
			&holder,
			target,
			IScarcityCollection::transferFromCall {
				from: holder_address.0.into(),
				to: holder_address.0.into(),
				tokenId: token,
			}
			.abi_encode(),
			"destination already holds this instance",
		);

		assert_eq!(NftsByOwner::<Test>::get(&holder).unwrap().instance, instance);
	});
}

/// A mint announces its token's ERC-5192 status, which is the only point it ever can.
///
/// Transferability is fixed when the item is defined, so neither event has a later emit site and
/// a consumer that misses this one never learns the status from a log.
#[test]
fn mint_announces_the_soulbound_status() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let bound_item = setup_item_as(&alice, collection, Transferability::Soulbound);
		let free_item = setup_item(&alice, collection);
		let target = collection_address(collection);

		let mint = |item, byte| {
			let (address, _) = purse(byte);
			let data = call_ok(
				&alice,
				target,
				IScarcityCollection::mintCall {
					item,
					to: address.0.into(),
					keys: alloc::vec![],
					values: alloc::vec![],
				}
				.abi_encode(),
			);
			IScarcityCollection::mintCall::abi_decode_returns(&data).unwrap()
		};

		let bound = mint(bound_item, 0xBB);
		assert_contract_event(target, IScarcityCollection::Locked { tokenId: bound });
		// The status rides alongside the mint's `Transfer` and nothing else.
		assert_eq!(contract_event_count(), 2);

		System::reset_events();
		let free = mint(free_item, 0xCC);
		assert_contract_event(target, IScarcityCollection::Unlocked { tokenId: free });
		assert_eq!(contract_event_count(), 2);
	});
}

#[test]
fn soulbound_items_report_locked_and_refuse_holder_transfers() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let bound_item = setup_item_as(&alice, collection, Transferability::Soulbound);
		let free_item = setup_item(&alice, collection);
		let (holder_address, holder, bound) =
			mint_to_funded_purse(&alice, collection, bound_item, 0xBB);
		let (_, _, free) = mint_to_funded_purse(&alice, collection, free_item, 0xDD);
		let (to_address, to) = purse(0xCC);
		let target = collection_address(collection);
		let bound_token = U256::from(bound);

		// Same collection, same address: the flag follows the item definition.
		for (token, expected) in [(bound, true), (free, false)] {
			let data = call_ok(
				&alice,
				target,
				IScarcityCollection::lockedCall { tokenId: U256::from(token) }.abi_encode(),
			);
			let locked = IScarcityCollection::lockedCall::abi_decode_returns(&data).unwrap();
			assert_eq!(locked, expected, "token {token}");
		}

		call_reverted_with(
			&holder,
			target,
			IScarcityCollection::transferFromCall {
				from: holder_address.0.into(),
				to: to_address.0.into(),
				tokenId: bound_token,
			}
			.abi_encode(),
			"token is soulbound to its purse key",
		);
		call_reverted_with(
			&holder,
			target,
			IScarcityCollection::safeTransferFrom_1Call {
				from: holder_address.0.into(),
				to: to_address.0.into(),
				tokenId: bound_token,
			}
			.abi_encode(),
			"token is soulbound to its purse key",
		);
		assert_eq!(NftsByOwner::<Test>::get(&holder).unwrap().instance, bound);
		assert!(NftsByOwner::<Test>::get(&to).is_none());

		// The collection owner keeps a remedy for a misdirected soulbound mint.
		call_ok(
			&alice,
			target,
			IScarcityCollection::forceTransferCall {
				tokenId: bound_token,
				to: to_address.0.into(),
			}
			.abi_encode(),
		);
		assert_eq!(NftsByOwner::<Test>::get(&to).unwrap().instance, bound);
	});
}

#[test]
fn holder_transfer_does_not_cross_collections() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let other_collection = setup_collection(&alice);
		let (holder_address, holder, instance) =
			mint_to_funded_purse(&alice, collection, item, 0xBB);
		let (to_address, _) = purse(0xCC);
		let token = U256::from(instance);

		// `InstanceId`s are global, so the sibling collection's address must not move a token
		// that does not belong to it.
		call_reverted_with(
			&holder,
			collection_address(other_collection),
			IScarcityCollection::transferFromCall {
				from: holder_address.0.into(),
				to: to_address.0.into(),
				tokenId: token,
			}
			.abi_encode(),
			"unknown token",
		);

		assert_eq!(NftsByOwner::<Test>::get(&holder).unwrap().instance, instance);
	});
}

/// `owner()` answers as `collectionOwner()`, without claiming ERC-173.
///
/// The id covers `transferOwnership`, which cannot exist while a handover carries a deposit the
/// successor has to fund, so serving the read alone is the honest half.
#[test]
fn owner_answers_without_claiming_erc173() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let target = collection_address(collection);

		let owner = call_ok(&alice, target, IScarcityCollection::ownerCall {}.abi_encode());
		let collection_owner =
			call_ok(&alice, target, IScarcityCollection::collectionOwnerCall {}.abi_encode());
		assert_eq!(owner, collection_owner, "the two names must not drift apart");

		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::supportsInterfaceCall {
				interfaceId: [0x7f, 0x58, 0x28, 0xd0].into(),
			}
			.abi_encode(),
		);
		assert!(
			!IScarcityCollection::supportsInterfaceCall::abi_decode_returns(&data).unwrap(),
			"ERC-173 must not be claimed while `transferOwnership` is absent"
		);
	});
}

/// The ABI flag reaches the pallet, in both positions.
///
/// The rest of the soulbound coverage defines its items through the pallet, so nothing else
/// fails if this mapping is inverted.
#[test]
fn define_item_maps_the_soulbound_flag() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let target = collection_address(collection);

		for (soulbound, expected) in
			[(false, Transferability::Transferable), (true, Transferability::Soulbound)]
		{
			let data = call_ok(
				&alice,
				target,
				IScarcityCollection::defineItemCall {
					soulbound,
					keys: alloc::vec![],
					values: alloc::vec![],
				}
				.abi_encode(),
			);
			let item = IScarcityCollection::defineItemCall::abi_decode_returns(&data).unwrap();
			let definition = pallet_scarcity::ItemDefs::<Test>::get(collection, item)
				.expect("the call defined the item");
			assert_eq!(definition.transferability, expected, "soulbound: {soulbound}");
		}
	});
}

#[test]
fn define_item_via_precompile_works() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let target = collection_address(collection);

		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::defineItemCall {
				soulbound: false,
				keys: alloc::vec![Bytes::from(b"rarity".to_vec())],
				values: alloc::vec![Bytes::from(b"legendary".to_vec())],
			}
			.abi_encode(),
		);
		let item = IScarcityCollection::defineItemCall::abi_decode_returns(&data).unwrap();
		assert_eq!(item, 0);
		assert!(pallet_scarcity::ItemDefs::<Test>::get(collection, item).is_some());

		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::itemMetadataCall { item, key: Bytes::from(b"rarity".to_vec()) }
				.abi_encode(),
		);
		let rarity = IScarcityCollection::itemMetadataCall::abi_decode_returns(&data).unwrap();
		assert_eq!(rarity.as_ref(), b"legendary");

		let data =
			call_ok(&alice, target, IScarcityCollection::itemSupplyCall { item }.abi_encode());
		let supply = IScarcityCollection::itemSupplyCall::abi_decode_returns(&data).unwrap();
		assert_eq!(supply.supply, 0);
		assert_eq!(supply.liveSupply, 0);
	});
}

#[test]
fn define_item_requires_collection_owner() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let bob = id_to_account(2);
		map_account(&bob);
		let collection = setup_collection(&alice);

		call_reverted_with(
			&bob,
			collection_address(collection),
			IScarcityCollection::defineItemCall {
				soulbound: false,
				keys: alloc::vec![],
				values: alloc::vec![],
			}
			.abi_encode(),
			"caller is not the collection owner",
		);
	});
}

#[test]
fn force_ops_require_collection_owner() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let bob = id_to_account(2);
		map_account(&bob);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (other_address, _) = purse(0xCC);
		let (_, holder) = purse(0xBB);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder,
			alloc::vec![],
		)
		.unwrap();
		let target = collection_address(collection);
		let token = U256::from(instance);

		call_reverted_with(
			&bob,
			target,
			IScarcityCollection::forceTransferCall { tokenId: token, to: other_address.0.into() }
				.abi_encode(),
			"caller is not the collection owner",
		);
		call_reverted_with(
			&bob,
			target,
			IScarcityCollection::forceBurnCall { tokenId: token }.abi_encode(),
			"caller is not the collection owner",
		);
	});
}

#[test]
fn zero_destination_reverts() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (_, holder) = purse(0xBB);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder,
			alloc::vec![],
		)
		.unwrap();
		let target = collection_address(collection);

		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::mintCall {
				item,
				to: Address::ZERO,
				keys: alloc::vec![],
				values: alloc::vec![],
			}
			.abi_encode(),
			"destination is the zero address",
		);
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::forceTransferCall {
				tokenId: U256::from(instance),
				to: Address::ZERO,
			}
			.abi_encode(),
			"destination is the zero address",
		);
	});
}

#[test]
fn mint_to_occupied_purse_reverts() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (holder_address, holder) = purse(0xBB);
		pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder,
			alloc::vec![],
		)
		.unwrap();

		call_reverted_with(
			&alice,
			collection_address(collection),
			IScarcityCollection::mintCall {
				item,
				to: holder_address.0.into(),
				keys: alloc::vec![],
				values: alloc::vec![],
			}
			.abi_encode(),
			"destination purse already holds an instance",
		);
	});
}

#[test]
fn oversized_token_id_answers_unknown() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		map_account(&alice);

		call_reverted_with(
			&alice,
			collection_address(collection),
			IScarcityCollection::ownerOfCall { tokenId: U256::MAX }.abi_encode(),
			"unknown token",
		);
	});
}

/// `tokenURI` rejects a token that names no live instance, rather than resolving the metadata
/// scopes and answering the empty string an instance with no URI set receives.
///
/// The id is in range and unallocated, so this reaches the liveness check rather than the
/// conversion `oversized_token_id_answers_unknown` stops at.
#[test]
fn token_uri_rejects_an_unknown_token() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);

		call_reverted_with(
			&alice,
			collection_address(collection),
			IScarcityCollection::tokenURICall { tokenId: U256::from(1u64) }.abi_encode(),
			"unknown token",
		);
	});
}

#[test]
fn balance_of_zero_address_reverts() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);

		call_reverted_with(
			&alice,
			collection_address(collection),
			IScarcityCollection::balanceOfCall { owner: Address::ZERO.0.into() }.abi_encode(),
			"balance query for the zero address",
		);
	});
}

#[test]
fn metadata_argument_validation_reverts() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (holder_address, _) = purse(0xBB);
		let target = collection_address(collection);
		let mint = |keys: Vec<Bytes>, values: Vec<Bytes>| {
			IScarcityCollection::mintCall { item, to: holder_address.0.into(), keys, values }
				.abi_encode()
		};

		call_reverted_with(
			&alice,
			target,
			mint(alloc::vec![Bytes::from(b"a".to_vec())], alloc::vec![]),
			"metadata keys and values differ in length",
		);
		// The mock bounds keys at 32 bytes and values at 256 bytes.
		call_reverted_with(
			&alice,
			target,
			mint(
				alloc::vec![Bytes::from(alloc::vec![0u8; 33])],
				alloc::vec![Bytes::from(b"v".to_vec())],
			),
			"metadata key too long",
		);
		call_reverted_with(
			&alice,
			target,
			mint(
				alloc::vec![Bytes::from(b"k".to_vec())],
				alloc::vec![Bytes::from(alloc::vec![0u8; 257])],
			),
			"metadata value too long",
		);
	});
}

#[test]
fn instance_info_reports_nft_fields() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (_, holder) = purse(0xBB);
		let (_, later_holder) = purse(0xCC);
		let minted_at = MockNow::get();
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder,
			alloc::vec![],
		)
		.unwrap();

		let moved_at = minted_at + 777;
		MockNow::set(moved_at);
		assert_ok!(Scarcity::force_transfer(
			RuntimeOrigin::signed(alice.clone()),
			instance,
			later_holder
		));

		let data = call_ok(
			&alice,
			collection_address(collection),
			IScarcityCollection::instanceInfoCall { tokenId: U256::from(instance) }.abi_encode(),
		);
		let info = IScarcityCollection::instanceInfoCall::abi_decode_returns(&data).unwrap();
		assert_eq!(info.item, item);
		assert_eq!(info.mintedAt, minted_at);
		assert_eq!(info.lastMoved, moved_at);
		assert_eq!(info.stateNonce, 1);
	});
}

#[test]
fn force_burn_refunds_unused_weight() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (_, heavy_holder) = purse(0xBB);
		let (_, light_holder) = purse(0xCC);
		// The mock allows at most 3 instance metadata entries; the heavy instance carries
		// the worst case, the light one none, so a refunded burn must consume less.
		let heavy = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			heavy_holder,
			alloc::vec![
				(key(b"a"), value(b"1")),
				(key(b"b"), value(b"2")),
				(key(b"c"), value(b"3"))
			],
		)
		.unwrap();
		let light = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			light_holder,
			alloc::vec![],
		)
		.unwrap();
		let target = collection_address(collection);

		let heavy_result = call_full(
			&alice,
			target,
			IScarcityCollection::forceBurnCall { tokenId: U256::from(heavy) }.abi_encode(),
		);
		assert!(!heavy_result.result.expect("burn succeeds").did_revert());
		let light_result = call_full(
			&alice,
			target,
			IScarcityCollection::forceBurnCall { tokenId: U256::from(light) }.abi_encode(),
		);
		assert!(!light_result.result.expect("burn succeeds").did_revert());

		assert!(
			light_result.weight_consumed.ref_time() < heavy_result.weight_consumed.ref_time(),
			"burning without metadata must be refunded below the worst case: light {:?}, heavy {:?}",
			light_result.weight_consumed,
			heavy_result.weight_consumed
		);

		// A burn that never reaches dispatch pays for its lookups, not for the worst-case
		// mutation: the worst case is charged only after the lookups succeed.
		let unknown = call_full(
			&alice,
			target,
			IScarcityCollection::forceBurnCall { tokenId: U256::from(u64::MAX) }.abi_encode(),
		);
		assert!(unknown.result.expect("burn executes").did_revert());
		assert!(
			unknown.weight_consumed.ref_time() < light_result.weight_consumed.ref_time(),
			"a reverting burn must not keep the worst-case charge: revert {:?}, burn {:?}",
			unknown.weight_consumed,
			light_result.weight_consumed
		);
	});
}

#[test]
fn unallocated_collection_and_item_revert() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (holder_address, _) = purse(0xBB);
		// The prefix matcher answers for every collection id, so "no such collection" is the
		// default state of the address space and the first thing a mistyped id reaches.
		let unallocated = collection_address(collection + 1);
		let mint_at = |item| {
			IScarcityCollection::mintCall {
				item,
				to: holder_address.0.into(),
				keys: alloc::vec![],
				values: alloc::vec![],
			}
			.abi_encode()
		};

		for input in [
			IScarcityCollection::collectionOwnerCall {}.abi_encode(),
			IScarcityCollection::defineItemCall {
				soulbound: false,
				keys: alloc::vec![],
				values: alloc::vec![],
			}
			.abi_encode(),
			mint_at(0),
			// The reads that answer without touching collection state must revert too, or the
			// address reads as a live empty contract that `supportsInterface` vouches for.
			IScarcityCollection::supportsInterfaceCall { interfaceId: ERC721_INTERFACE_ID.into() }
				.abi_encode(),
			IScarcityCollection::balanceOfCall { owner: holder_address.0.into() }.abi_encode(),
			IScarcityCollection::nameCall {}.abi_encode(),
		] {
			call_reverted_with(&alice, unallocated, input, "unknown collection");
		}

		let target = collection_address(collection);
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::itemSupplyCall { item: item + 1 }.abi_encode(),
			"unknown item",
		);
		call_reverted_with(&alice, target, mint_at(item + 1), "unknown item");
	});
}

#[test]
fn self_transfer_and_metadata_bound_revert() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (holder_address, holder) = purse(0xBB);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder,
			alloc::vec![],
		)
		.unwrap();
		let target = collection_address(collection);

		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::forceTransferCall {
				tokenId: U256::from(instance),
				to: holder_address.0.into(),
			}
			.abi_encode(),
			"destination already holds this instance",
		);

		// The mock caps instance metadata at three entries.
		let (fresh_address, _) = purse(0xCC);
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::mintCall {
				item,
				to: fresh_address.0.into(),
				keys: (0..4u8).map(|i| Bytes::from(alloc::vec![i])).collect(),
				values: (0..4u8).map(|i| Bytes::from(alloc::vec![i])).collect(),
			}
			.abi_encode(),
			"too many instance metadata entries",
		);
	});
}

#[test]
fn unpayable_storage_deposit_reverts() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (holder_address, _) = purse(0xDD);
		// Instance deposits are charged to the collection owner, so an owner that cannot
		// fund them fails the mint rather than trapping.
		Balances::make_free_balance_be(&alice, 1);

		call_reverted_with(
			&alice,
			collection_address(collection),
			IScarcityCollection::mintCall {
				item,
				to: holder_address.0.into(),
				keys: alloc::vec![],
				values: alloc::vec![],
			}
			.abi_encode(),
			"collection owner cannot pay the storage deposit",
		);
	});
}

/// Occupying a purse key registers it, so `balanceOf` answers for a zero-balance holder that
/// frame-system never created an account for, whether it was minted to or moved to.
///
/// Also pins the one state the hook does not reach, so it does not read as a fresh defect: a key
/// whose account is reaped afterwards keeps the instance but loses the registration.
#[test]
fn occupying_a_purse_key_makes_a_zero_balance_holder_addressable() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let target = collection_address(collection);

		let read = |holder: &AccountId32| {
			let instance = pallet_scarcity::Pallet::<Test>::do_mint(
				alice.clone(),
				collection,
				item,
				holder.clone(),
				alloc::vec![],
			)
			.unwrap();
			let token = U256::from(instance);
			let data = call_ok(
				&alice,
				target,
				IScarcityCollection::ownerOfCall { tokenId: token }.abi_encode(),
			);
			let reported = IScarcityCollection::ownerOfCall::abi_decode_returns(&data).unwrap();
			let data = call_ok(
				&alice,
				target,
				IScarcityCollection::balanceOfCall { owner: reported }.abi_encode(),
			);
			let balance = IScarcityCollection::balanceOfCall::abi_decode_returns(&data).unwrap();
			(reported, balance)
		};

		let balance_of = |holder: &AccountId32| {
			let data = call_ok(
				&alice,
				target,
				IScarcityCollection::balanceOfCall { owner: address_of::<Test>(holder) }
					.abi_encode(),
			);
			IScarcityCollection::balanceOfCall::abi_decode_returns(&data).unwrap()
		};

		// A zero-balance purse has no `system::Account`, so nothing else would register it, and
		// asserting that before the mint is what makes the mint the cause of the registration.
		let bare = id_to_account(9);
		assert!(!frame_system::Account::<Test>::contains_key(&bare));
		assert!(!<Test as pallet_revive::Config>::AddressMapper::is_mapped(&bare));
		let (_, bare_balance) = read(&bare);
		assert!(<Test as pallet_revive::Config>::AddressMapper::is_mapped(&bare));
		assert_eq!(bare_balance, U256::ONE);

		// A move registers its destination too, so a holder that never saw a mint still resolves.
		let moved_to = id_to_account(11);
		let instance = NftsByOwner::<Test>::get(&bare).unwrap().instance;
		assert_ok!(Scarcity::force_transfer(
			RuntimeOrigin::signed(alice.clone()),
			instance,
			moved_to.clone()
		));
		assert!(<Test as pallet_revive::Config>::AddressMapper::is_mapped(&moved_to));
		assert_eq!(balance_of(&moved_to), U256::ONE);

		// Not reached: reaping unmaps a key that still holds an instance. The mint here precedes
		// the funding, so the entry reaping removes is the one this hook wrote.
		let reaped = id_to_account(10);
		let (_, reaped_balance) = read(&reaped);
		assert_eq!(reaped_balance, U256::ONE);
		Balances::make_free_balance_be(&reaped, 1_000_000);
		Balances::make_free_balance_be(&reaped, 0);
		assert!(!<Test as pallet_revive::Config>::AddressMapper::is_mapped(&reaped));
		assert!(NftsByOwner::<Test>::contains_key(&reaped));
		assert_eq!(balance_of(&reaped), U256::ZERO);
	});
}

/// Assert that calling `address` with `input` and value attached is rejected and costs nothing.
fn assert_rejects_value(caller: &AccountId32, address: H160, input: Vec<u8>, method: &str) {
	let before = Balances::free_balance(caller);
	let output = call_with_value(caller, address, input, 1_000);
	assert!(output.did_revert(), "{method}: expected revert, got success: {output:?}");
	let decoded = Revert::abi_decode(&output.data).expect("revert data decodes as Error(string)");
	assert!(
		decoded.reason.contains("this precompile does not accept value"),
		"{method}: revert reason {:?}",
		decoded.reason
	);
	// The frame unwinds the transfer with the rest of its state changes.
	assert_eq!(Balances::free_balance(caller), before, "{method}: caller was charged");
	assert_eq!(
		Balances::free_balance(precompile_account(address)),
		0,
		"{method}: value stranded at the precompile"
	);
}

/// Every generated selector of the collection interface, once each.
///
/// Asserts its own exhaustiveness against the generated selector set, so a method added to
/// `IScarcity.sol` fails every caller of this until it is listed here. Callers that need a call to
/// reach its real path pass arguments that would otherwise succeed; callers testing a guard that
/// short-circuits before argument handling can pass anything.
fn all_collection_calls(
	token: U256,
	item: pallet_scarcity::ItemIndex,
	held: Address,
	empty: Address,
) -> Vec<IScarcityCollectionCalls> {
	let calls = alloc::vec![
		IScarcityCollectionCalls::supportsInterface(IScarcityCollection::supportsInterfaceCall {
			interfaceId: ERC721_INTERFACE_ID.into()
		}),
		IScarcityCollectionCalls::balanceOf(IScarcityCollection::balanceOfCall { owner: held }),
		IScarcityCollectionCalls::ownerOf(IScarcityCollection::ownerOfCall { tokenId: token }),
		IScarcityCollectionCalls::tokenOfOwnerByIndex(
			IScarcityCollection::tokenOfOwnerByIndexCall { owner: held, index: U256::ZERO }
		),
		IScarcityCollectionCalls::safeTransferFrom_0(IScarcityCollection::safeTransferFrom_0Call {
			from: held,
			to: empty,
			tokenId: token,
			data: Bytes::new(),
		}),
		IScarcityCollectionCalls::safeTransferFrom_1(IScarcityCollection::safeTransferFrom_1Call {
			from: held,
			to: empty,
			tokenId: token,
		}),
		IScarcityCollectionCalls::transferFrom(IScarcityCollection::transferFromCall {
			from: held,
			to: empty,
			tokenId: token,
		}),
		IScarcityCollectionCalls::approve(IScarcityCollection::approveCall {
			to: empty,
			tokenId: token,
		}),
		IScarcityCollectionCalls::setApprovalForAll(IScarcityCollection::setApprovalForAllCall {
			operator: empty,
			approved: true
		}),
		IScarcityCollectionCalls::getApproved(IScarcityCollection::getApprovedCall {
			tokenId: token,
		}),
		IScarcityCollectionCalls::isApprovedForAll(IScarcityCollection::isApprovedForAllCall {
			owner: held,
			operator: empty
		}),
		IScarcityCollectionCalls::name(IScarcityCollection::nameCall {}),
		IScarcityCollectionCalls::symbol(IScarcityCollection::symbolCall {}),
		IScarcityCollectionCalls::tokenURI(IScarcityCollection::tokenURICall { tokenId: token }),
		IScarcityCollectionCalls::royaltyInfo(IScarcityCollection::royaltyInfoCall {
			tokenId: token,
			salePrice: U256::from(10_000u64),
		}),
		IScarcityCollectionCalls::contractURI(IScarcityCollection::contractURICall {}),
		IScarcityCollectionCalls::locked(IScarcityCollection::lockedCall { tokenId: token }),
		IScarcityCollectionCalls::defineItem(IScarcityCollection::defineItemCall {
			soulbound: false,
			keys: alloc::vec![],
			values: alloc::vec![],
		}),
		IScarcityCollectionCalls::mint(IScarcityCollection::mintCall {
			item,
			to: empty,
			keys: alloc::vec![],
			values: alloc::vec![],
		}),
		IScarcityCollectionCalls::forceTransfer(IScarcityCollection::forceTransferCall {
			tokenId: token,
			to: empty,
		}),
		IScarcityCollectionCalls::forceBurn(IScarcityCollection::forceBurnCall { tokenId: token }),
		IScarcityCollectionCalls::collectionOwner(IScarcityCollection::collectionOwnerCall {}),
		IScarcityCollectionCalls::owner(IScarcityCollection::ownerCall {}),
		IScarcityCollectionCalls::itemSupply(IScarcityCollection::itemSupplyCall { item }),
		IScarcityCollectionCalls::instanceInfo(IScarcityCollection::instanceInfoCall {
			tokenId: token,
		}),
		IScarcityCollectionCalls::collectionMetadata(IScarcityCollection::collectionMetadataCall {
			key: Bytes::from_static(NAME_KEY)
		}),
		IScarcityCollectionCalls::itemMetadata(IScarcityCollection::itemMetadataCall {
			item,
			key: Bytes::from_static(NAME_KEY),
		}),
		IScarcityCollectionCalls::instanceMetadata(IScarcityCollection::instanceMetadataCall {
			tokenId: token,
			key: Bytes::from_static(NAME_KEY),
		}),
		IScarcityCollectionCalls::setCollectionMetadata(
			IScarcityCollection::setCollectionMetadataCall {
				key: Bytes::from_static(NAME_KEY),
				value: Bytes::from_static(b"v"),
			}
		),
		IScarcityCollectionCalls::removeCollectionMetadata(
			IScarcityCollection::removeCollectionMetadataCall { key: Bytes::from_static(NAME_KEY) }
		),
		IScarcityCollectionCalls::setItemMetadata(IScarcityCollection::setItemMetadataCall {
			item,
			key: Bytes::from_static(NAME_KEY),
			value: Bytes::from_static(b"v"),
		}),
		IScarcityCollectionCalls::removeItemMetadata(IScarcityCollection::removeItemMetadataCall {
			item,
			key: Bytes::from_static(NAME_KEY),
		}),
		IScarcityCollectionCalls::setInstanceMetadata(
			IScarcityCollection::setInstanceMetadataCall {
				tokenId: token,
				key: Bytes::from_static(NAME_KEY),
				value: Bytes::from_static(b"v"),
			}
		),
		IScarcityCollectionCalls::removeInstanceMetadata(
			IScarcityCollection::removeInstanceMetadataCall {
				tokenId: token,
				key: Bytes::from_static(NAME_KEY),
			}
		),
		IScarcityCollectionCalls::nominateCollectionOwner(
			IScarcityCollection::nominateCollectionOwnerCall { successor: empty }
		),
		IScarcityCollectionCalls::clearCollectionOwnerNomination(
			IScarcityCollection::clearCollectionOwnerNominationCall {}
		),
		IScarcityCollectionCalls::claimCollectionOwnership(
			IScarcityCollection::claimCollectionOwnershipCall {}
		),
		IScarcityCollectionCalls::deleteItem(IScarcityCollection::deleteItemCall { item }),
		IScarcityCollectionCalls::deleteCollection(IScarcityCollection::deleteCollectionCall {}),
		IScarcityCollectionCalls::pendingCollectionOwner(
			IScarcityCollection::pendingCollectionOwnerCall {}
		),
		IScarcityCollectionCalls::collectionOwnerDeposit(
			IScarcityCollection::collectionOwnerDepositCall {}
		),
		IScarcityCollectionCalls::hasCollectionMetadata(
			IScarcityCollection::hasCollectionMetadataCall { key: Bytes::from_static(NAME_KEY) }
		),
		IScarcityCollectionCalls::hasItemMetadata(IScarcityCollection::hasItemMetadataCall {
			item,
			key: Bytes::from_static(NAME_KEY),
		}),
		IScarcityCollectionCalls::hasInstanceMetadata(
			IScarcityCollection::hasInstanceMetadataCall {
				tokenId: token,
				key: Bytes::from_static(NAME_KEY),
			}
		),
	];

	let covered = calls.iter().map(|call| call.selector()).collect::<Vec<_>>();
	for selector in IScarcityCollectionCalls::selectors() {
		assert!(covered.contains(&selector), "no case for selector {selector:?}");
	}
	assert_eq!(covered.len(), IScarcityCollectionCalls::COUNT);
	calls
}

/// No function of either interface is payable, so every one of them must reject attached value.
///
/// Arguments are the ones that would otherwise succeed, which is what makes each case prove the
/// rejection wins over the real path rather than over some other revert.
#[test]
fn every_method_rejects_attached_value() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (holder_address, holder) = purse(0xDD);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder.clone(),
			alloc::vec![],
		)
		.unwrap();
		let target = collection_address(collection);
		let token = U256::from(instance);
		let held: Address = holder_address.0.into();
		let (empty_address, _) = purse(0xEE);
		let empty: Address = empty_address.0.into();

		let calls = all_collection_calls(token, item, held, empty);

		for call in &calls {
			let method = alloc::format!("selector {:02x?}", call.selector());
			assert_rejects_value(&alice, target, call.abi_encode(), &method);
		}

		let factory =
			IScarcityFactoryCalls::createCollection(IScarcityFactory::createCollectionCall {});
		assert_eq!(IScarcityFactoryCalls::COUNT, 1);
		assert_rejects_value(&alice, factory_address(), factory.abi_encode(), "createCollection");

		// None of the mutators above took effect.
		assert_eq!(NftsByOwner::<Test>::get(&holder).unwrap().instance, instance);
		assert!(NftsByOwner::<Test>::get(account_of::<Test>(&empty)).is_none());
		assert_eq!(Collections::<Test>::get(collection).unwrap().next_item_index, 1);
		assert!(Collections::<Test>::get(collection + 1).is_none());
	});
}

/// The keys the ERC-721 surface reads as strings only take UTF-8 through this precompile,
/// at every scope the read resolves. Other keys keep taking arbitrary bytes.
#[test]
fn reserved_metadata_values_must_be_utf8() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (_, holder) = purse(0xCC);
		let target = collection_address(collection);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder,
			vec![],
		)
		.unwrap();
		let bad = Bytes::from(alloc::vec![0xffu8, 0xfe]);
		const REASON: &str = "reserved metadata value is not valid UTF-8";

		// `name`, `symbol` and `contractURI` resolve at collection scope only; `tokenURI`
		// resolves at all three, so each of its scopes guards it.
		for reserved in [NAME_KEY, SYMBOL_KEY, TOKEN_URI_KEY, CONTRACT_URI_KEY] {
			call_reverted_with(
				&alice,
				target,
				IScarcityCollection::setCollectionMetadataCall {
					key: Bytes::from(reserved.to_vec()),
					value: bad.clone(),
				}
				.abi_encode(),
				REASON,
			);
		}
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::setItemMetadataCall {
				item,
				key: Bytes::from(TOKEN_URI_KEY.to_vec()),
				value: bad.clone(),
			}
			.abi_encode(),
			REASON,
		);
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::setInstanceMetadataCall {
				tokenId: U256::from(instance),
				key: Bytes::from(TOKEN_URI_KEY.to_vec()),
				value: bad.clone(),
			}
			.abi_encode(),
			REASON,
		);

		// `defineItem` and `mint` carry metadata too, so the same key is reachable without
		// touching a setter.
		let reserved_pair =
			|value: Bytes| (alloc::vec![Bytes::from(TOKEN_URI_KEY.to_vec())], alloc::vec![value]);
		let (keys, values) = reserved_pair(bad.clone());
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::defineItemCall { soulbound: false, keys, values }.abi_encode(),
			REASON,
		);
		let (keys, values) = reserved_pair(bad.clone());
		let (empty_address, _) = purse(0xDD);
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::mintCall { item, to: empty_address.0.into(), keys, values }
				.abi_encode(),
			REASON,
		);

		// The guard keys on the reserved names, not on the bytes: the same value goes through
		// under any other key.
		call_ok(
			&alice,
			target,
			IScarcityCollection::setCollectionMetadataCall {
				key: Bytes::from(b"policy".to_vec()),
				value: bad,
			}
			.abi_encode(),
		);
		assert!(CollectionMetadata::<Test>::contains_key(collection, key(b"policy")));
	});
}

/// The policy is per-runtime, so the reads must not depend on it: a chain wiring
/// `MetadataPolicy = ()` stores whatever it is given, and entries written before a policy was
/// wired keep whatever they hold. Decoding lossily keeps such a collection readable instead of
/// failing every ERC-721 consumer that asks.
///
/// Written straight to storage because this mock mirrors the runtime and its policy refuses these
/// values, which is what `reserved_metadata_values_must_be_utf8` covers.
#[test]
fn a_reserved_value_written_natively_reads_lossily() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (_, holder) = purse(0xCC);
		let target = collection_address(collection);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder,
			vec![],
		)
		.unwrap();

		for reserved in [NAME_KEY, SYMBOL_KEY, TOKEN_URI_KEY] {
			CollectionMetadata::<Test>::insert(
				collection,
				key(reserved),
				pallet_scarcity::MetadataEntry { value: value(&[0xff]), deposit: 0u64 },
			);
		}

		let data = call_ok(&alice, target, IScarcityCollection::nameCall {}.abi_encode());
		assert_eq!(IScarcityCollection::nameCall::abi_decode_returns(&data).unwrap(), "\u{fffd}");
		let data = call_ok(&alice, target, IScarcityCollection::symbolCall {}.abi_encode());
		assert_eq!(IScarcityCollection::symbolCall::abi_decode_returns(&data).unwrap(), "\u{fffd}");
		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::tokenURICall { tokenId: U256::from(instance) }.abi_encode(),
		);
		assert_eq!(
			IScarcityCollection::tokenURICall::abi_decode_returns(&data).unwrap(),
			"\u{fffd}"
		);
	});
}

#[test]
fn collection_metadata_set_and_remove_via_precompile() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let target = collection_address(collection);
		let k = Bytes::from(b"policy".to_vec());
		let read_value = |caller: &AccountId32| {
			let data = call_ok(
				caller,
				target,
				IScarcityCollection::collectionMetadataCall { key: k.clone() }.abi_encode(),
			);
			IScarcityCollection::collectionMetadataCall::abi_decode_returns(&data).unwrap()
		};
		let read_present = |caller: &AccountId32| {
			let data = call_ok(
				caller,
				target,
				IScarcityCollection::hasCollectionMetadataCall { key: k.clone() }.abi_encode(),
			);
			IScarcityCollection::hasCollectionMetadataCall::abi_decode_returns(&data).unwrap()
		};

		assert!(!read_present(&alice));
		call_ok(
			&alice,
			target,
			IScarcityCollection::setCollectionMetadataCall {
				key: k.clone(),
				value: Bytes::from(b"open".to_vec()),
			}
			.abi_encode(),
		);
		assert_eq!(read_value(&alice).as_ref(), b"open");
		assert!(read_present(&alice));

		// An empty value is a real entry: the raw getter cannot distinguish it from an
		// unset key, which is exactly what the presence getter is for.
		call_ok(
			&alice,
			target,
			IScarcityCollection::setCollectionMetadataCall { key: k.clone(), value: Bytes::new() }
				.abi_encode(),
		);
		assert!(read_value(&alice).is_empty());
		assert!(read_present(&alice));

		call_ok(
			&alice,
			target,
			IScarcityCollection::removeCollectionMetadataCall { key: k.clone() }.abi_encode(),
		);
		assert!(read_value(&alice).is_empty());
		assert!(!read_present(&alice));

		// Removing an absent key succeeds as a no-op, mirroring the pallet call.
		call_ok(
			&alice,
			target,
			IScarcityCollection::removeCollectionMetadataCall { key: k }.abi_encode(),
		);
	});
}

#[test]
fn item_metadata_set_and_remove_via_precompile() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let target = collection_address(collection);
		let k = Bytes::from(b"rarity".to_vec());
		let read_value = || {
			let data = call_ok(
				&alice,
				target,
				IScarcityCollection::itemMetadataCall { item, key: k.clone() }.abi_encode(),
			);
			IScarcityCollection::itemMetadataCall::abi_decode_returns(&data).unwrap()
		};
		let read_present = || {
			let data = call_ok(
				&alice,
				target,
				IScarcityCollection::hasItemMetadataCall { item, key: k.clone() }.abi_encode(),
			);
			IScarcityCollection::hasItemMetadataCall::abi_decode_returns(&data).unwrap()
		};

		call_ok(
			&alice,
			target,
			IScarcityCollection::setItemMetadataCall {
				item,
				key: k.clone(),
				value: Bytes::from(b"rare".to_vec()),
			}
			.abi_encode(),
		);
		assert_eq!(read_value().as_ref(), b"rare");
		assert!(read_present());

		call_ok(
			&alice,
			target,
			IScarcityCollection::removeItemMetadataCall { item, key: k.clone() }.abi_encode(),
		);
		assert!(read_value().is_empty());
		assert!(!read_present());

		// A collection default resolves through the raw getter but is not item-scope
		// presence, so a write-once policy at the item scope stays enforceable.
		call_ok(
			&alice,
			target,
			IScarcityCollection::setCollectionMetadataCall {
				key: k.clone(),
				value: Bytes::from(b"common".to_vec()),
			}
			.abi_encode(),
		);
		assert_eq!(read_value().as_ref(), b"common");
		assert!(!read_present());
	});
}

#[test]
fn instance_metadata_set_and_remove_via_precompile() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (_, holder) = purse(0xBB);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder,
			alloc::vec![],
		)
		.unwrap();
		let target = collection_address(collection);
		let token = U256::from(instance);
		let k = Bytes::from(b"level".to_vec());
		let read_value = || {
			let data = call_ok(
				&alice,
				target,
				IScarcityCollection::instanceMetadataCall { tokenId: token, key: k.clone() }
					.abi_encode(),
			);
			IScarcityCollection::instanceMetadataCall::abi_decode_returns(&data).unwrap()
		};
		let read_present = || {
			let data = call_ok(
				&alice,
				target,
				IScarcityCollection::hasInstanceMetadataCall { tokenId: token, key: k.clone() }
					.abi_encode(),
			);
			IScarcityCollection::hasInstanceMetadataCall::abi_decode_returns(&data).unwrap()
		};

		call_ok(
			&alice,
			target,
			IScarcityCollection::setInstanceMetadataCall {
				tokenId: token,
				key: k.clone(),
				value: Bytes::from(b"7".to_vec()),
			}
			.abi_encode(),
		);
		assert_eq!(read_value().as_ref(), b"7");
		assert!(read_present());

		call_ok(
			&alice,
			target,
			IScarcityCollection::removeInstanceMetadataCall { tokenId: token, key: k.clone() }
				.abi_encode(),
		);
		assert!(read_value().is_empty());
		assert!(!read_present());

		// An item default resolves through the raw getter but is not instance-scope
		// presence.
		call_ok(
			&alice,
			target,
			IScarcityCollection::setItemMetadataCall {
				item,
				key: k.clone(),
				value: Bytes::from(b"1".to_vec()),
			}
			.abi_encode(),
		);
		assert_eq!(read_value().as_ref(), b"1");
		assert!(!read_present());

		// An instance of another collection must not be mutable through this address.
		let other = pallet_scarcity::Pallet::<Test>::do_create_collection(alice.clone()).unwrap();
		let other_item = setup_item(&alice, other);
		let (_, other_holder) = purse(0xCC);
		let foreign = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			other,
			other_item,
			other_holder,
			alloc::vec![],
		)
		.unwrap();
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::setInstanceMetadataCall {
				tokenId: U256::from(foreign),
				key: k,
				value: Bytes::from(b"9".to_vec()),
			}
			.abi_encode(),
			"unknown token",
		);
	});
}

#[test]
fn metadata_mutators_require_collection_owner() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let bob = id_to_account(2);
		map_account(&bob);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (_, holder) = purse(0xBB);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder,
			alloc::vec![],
		)
		.unwrap();
		let target = collection_address(collection);
		let token = U256::from(instance);
		let k = Bytes::from(b"k".to_vec());
		let v = Bytes::from(b"v".to_vec());

		for input in [
			IScarcityCollection::setCollectionMetadataCall { key: k.clone(), value: v.clone() }
				.abi_encode(),
			IScarcityCollection::removeCollectionMetadataCall { key: k.clone() }.abi_encode(),
			IScarcityCollection::setItemMetadataCall { item, key: k.clone(), value: v.clone() }
				.abi_encode(),
			IScarcityCollection::removeItemMetadataCall { item, key: k.clone() }.abi_encode(),
			IScarcityCollection::setInstanceMetadataCall {
				tokenId: token,
				key: k.clone(),
				value: v,
			}
			.abi_encode(),
			IScarcityCollection::removeInstanceMetadataCall { tokenId: token, key: k }.abi_encode(),
		] {
			call_reverted_with(&bob, target, input, "caller is not the collection owner");
		}
	});
}

#[test]
fn ownership_handover_via_precompile() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let bob = id_to_account(2);
		map_account(&bob);
		let collection = setup_collection(&alice);
		let target = collection_address(collection);
		let bob_address = address_of::<Test>(&bob);
		let pending = |caller: &AccountId32| {
			let data = call_ok(
				caller,
				target,
				IScarcityCollection::pendingCollectionOwnerCall {}.abi_encode(),
			);
			IScarcityCollection::pendingCollectionOwnerCall::abi_decode_returns(&data).unwrap()
		};

		assert_eq!(pending(&alice), Address::ZERO);
		call_ok(
			&alice,
			target,
			IScarcityCollection::nominateCollectionOwnerCall { successor: bob_address }
				.abi_encode(),
		);
		assert_eq!(pending(&alice), bob_address);
		// Nomination alone moves no authority.
		assert_eq!(Collections::<Test>::get(collection).unwrap().owner, alice);

		call_ok(&bob, target, IScarcityCollection::claimCollectionOwnershipCall {}.abi_encode());
		let data =
			call_ok(&alice, target, IScarcityCollection::collectionOwnerCall {}.abi_encode());
		let owner = IScarcityCollection::collectionOwnerCall::abi_decode_returns(&data).unwrap();
		assert_eq!(owner, bob_address);
		assert_eq!(pending(&alice), Address::ZERO);

		// Authority moved with the claim: the former owner is locked out, the new owner
		// controls the collection.
		let define = IScarcityCollection::defineItemCall {
			soulbound: false,
			keys: alloc::vec![],
			values: alloc::vec![],
		}
		.abi_encode();
		call_reverted_with(&alice, target, define.clone(), "caller is not the collection owner");
		call_ok(&bob, target, define);
	});
}

#[test]
fn nomination_clears_via_precompile() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let bob = id_to_account(2);
		map_account(&bob);
		let collection = setup_collection(&alice);
		let target = collection_address(collection);

		call_ok(
			&alice,
			target,
			IScarcityCollection::nominateCollectionOwnerCall {
				successor: address_of::<Test>(&bob),
			}
			.abi_encode(),
		);
		call_ok(
			&alice,
			target,
			IScarcityCollection::clearCollectionOwnerNominationCall {}.abi_encode(),
		);

		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::pendingCollectionOwnerCall {}.abi_encode(),
		);
		let pending =
			IScarcityCollection::pendingCollectionOwnerCall::abi_decode_returns(&data).unwrap();
		assert_eq!(pending, Address::ZERO);
		call_reverted_with(
			&bob,
			target,
			IScarcityCollection::claimCollectionOwnershipCall {}.abi_encode(),
			"caller is not the nominated successor",
		);
	});
}

#[test]
fn ownership_handover_negative_cases() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let bob = id_to_account(2);
		let carol = id_to_account(3);
		map_account(&bob);
		map_account(&carol);
		let collection = setup_collection(&alice);
		let target = collection_address(collection);
		let nominate =
			|successor| IScarcityCollection::nominateCollectionOwnerCall { successor }.abi_encode();

		call_reverted_with(
			&bob,
			target,
			nominate(address_of::<Test>(&bob)),
			"caller is not the collection owner",
		);
		call_reverted_with(
			&bob,
			target,
			IScarcityCollection::clearCollectionOwnerNominationCall {}.abi_encode(),
			"caller is not the collection owner",
		);
		call_reverted_with(
			&alice,
			target,
			nominate(Address::ZERO),
			"successor is the zero address",
		);
		call_reverted_with(
			&alice,
			target,
			nominate(address_of::<Test>(&alice)),
			"successor is already the collection owner",
		);

		// A nomination for carol does not let bob claim.
		call_ok(&alice, target, nominate(address_of::<Test>(&carol)));
		call_reverted_with(
			&bob,
			target,
			IScarcityCollection::claimCollectionOwnershipCall {}.abi_encode(),
			"caller is not the nominated successor",
		);
		assert_eq!(Collections::<Test>::get(collection).unwrap().owner, alice);
	});
}

#[test]
fn delete_item_and_collection_via_precompile() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (_, holder) = purse(0xBB);
		let instance = pallet_scarcity::Pallet::<Test>::do_mint(
			alice.clone(),
			collection,
			item,
			holder,
			alloc::vec![],
		)
		.unwrap();
		let target = collection_address(collection);
		let k = Bytes::from(b"k".to_vec());
		call_ok(
			&alice,
			target,
			IScarcityCollection::setCollectionMetadataCall {
				key: k.clone(),
				value: Bytes::from(b"v".to_vec()),
			}
			.abi_encode(),
		);
		call_ok(
			&alice,
			target,
			IScarcityCollection::setItemMetadataCall {
				item,
				key: k.clone(),
				value: Bytes::from(b"v".to_vec()),
			}
			.abi_encode(),
		);
		let delete_item = IScarcityCollection::deleteItemCall { item }.abi_encode();
		let delete_collection = IScarcityCollection::deleteCollectionCall {}.abi_encode();

		// Cleanup runs leaves to roots: each precondition blocks until the previous step,
		// and the metadata-remove functions make each step reachable from the EVM.
		call_reverted_with(&alice, target, delete_item.clone(), "item still has live instances");
		call_ok(
			&alice,
			target,
			IScarcityCollection::forceBurnCall { tokenId: U256::from(instance) }.abi_encode(),
		);
		call_reverted_with(
			&alice,
			target,
			delete_item.clone(),
			"item metadata must be removed first",
		);
		call_ok(
			&alice,
			target,
			IScarcityCollection::removeItemMetadataCall { item, key: k.clone() }.abi_encode(),
		);
		call_reverted_with(
			&alice,
			target,
			delete_collection.clone(),
			"item definitions must be deleted first",
		);
		call_ok(&alice, target, delete_item);
		assert!(pallet_scarcity::ItemDefs::<Test>::get(collection, item).is_none());

		call_reverted_with(
			&alice,
			target,
			delete_collection.clone(),
			"collection metadata must be removed first",
		);
		call_ok(
			&alice,
			target,
			IScarcityCollection::removeCollectionMetadataCall { key: k }.abi_encode(),
		);
		call_ok(&alice, target, delete_collection);
		assert!(Collections::<Test>::get(collection).is_none());

		// The deleted collection's address stops answering.
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::collectionOwnerCall {}.abi_encode(),
			"unknown collection",
		);
	});
}

#[test]
fn delete_ops_require_collection_owner() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let bob = id_to_account(2);
		map_account(&bob);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let target = collection_address(collection);

		call_reverted_with(
			&bob,
			target,
			IScarcityCollection::deleteItemCall { item }.abi_encode(),
			"caller is not the collection owner",
		);
		call_reverted_with(
			&bob,
			target,
			IScarcityCollection::deleteCollectionCall {}.abi_encode(),
			"caller is not the collection owner",
		);
		assert!(Collections::<Test>::get(collection).is_some());
	});
}

#[test]
fn collection_owner_deposit_reports_aggregate() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let target = collection_address(collection);
		let read_deposit = || {
			let data = call_ok(
				&alice,
				target,
				IScarcityCollection::collectionOwnerDepositCall {}.abi_encode(),
			);
			IScarcityCollection::collectionOwnerDepositCall::abi_decode_returns(&data).unwrap()
		};

		let base = read_deposit();
		assert_eq!(base, U256::from(Collections::<Test>::get(collection).unwrap().owner_deposit));
		assert!(base > U256::ZERO, "the collection record itself carries a deposit");

		call_ok(
			&alice,
			target,
			IScarcityCollection::setCollectionMetadataCall {
				key: Bytes::from(b"k".to_vec()),
				value: Bytes::from(b"v".to_vec()),
			}
			.abi_encode(),
		);
		let grown = read_deposit();
		assert_eq!(grown, U256::from(Collections::<Test>::get(collection).unwrap().owner_deposit));
		assert!(grown > base, "a metadata entry grows the aggregate deposit");

		call_ok(
			&alice,
			target,
			IScarcityCollection::removeCollectionMetadataCall { key: Bytes::from(b"k".to_vec()) }
				.abi_encode(),
		);
		assert_eq!(read_deposit(), base, "removal releases the entry's exact deposit");
	});
}

/// Every pallet error variant the precompile can reach must map to a catchable revert.
///
/// The mapping in `revert_scarcity` is a runtime list, so the compiler cannot flag a variant
/// added to `pallet-scarcity` later. This test walks the variants from the error type's own
/// metadata and fails on any that starts trapping instead of reverting.
#[test]
fn mapped_scarcity_errors_are_exhaustive() {
	// The ABI covers the pallet's whole owner surface, so every error variant is reachable.
	const UNREACHABLE: [&str; 0] = [];

	let pallet_index = match DispatchError::from(pallet_scarcity::Error::<Test>::NoPermission) {
		DispatchError::Module(module) => module.index,
		other => panic!("pallet errors are module errors, got {other:?}"),
	};
	let variants =
		match <pallet_scarcity::Error<Test> as scale_info::TypeInfo>::type_info().type_def {
			scale_info::TypeDef::Variant(def) => def.variants,
			other => panic!("pallet errors are a variant type, got {other:?}"),
		};
	assert!(!variants.is_empty(), "error metadata carries no variants");

	for variant in &variants {
		let error = DispatchError::Module(sp_runtime::ModuleError {
			index: pallet_index,
			error: [variant.index, 0, 0, 0],
			message: None,
		});
		let reverts = matches!(revert_scarcity::<Test>(error), Error::Revert(_));
		let reachable = !UNREACHABLE.contains(&variant.name);
		assert_eq!(
			reverts,
			reachable,
			"{}: reverts={reverts}, but it is {} through this precompile. Map it in \
			 `revert_scarcity`, or add it to UNREACHABLE if the ABI cannot reach it.",
			variant.name,
			if reachable { "reachable" } else { "unreachable" }
		);
	}

	for name in UNREACHABLE {
		assert!(
			variants.iter().any(|variant| variant.name == name),
			"UNREACHABLE lists {name}, which no longer exists in the pallet"
		);
	}
}

/// An invalid token is reported before the receiver limitation, not after.
///
/// The two failures compete only on `safeTransferFrom` to a destination carrying code. EIP-721
/// orders "not a valid NFT" first, and the acknowledgement belongs after a completed transfer, so
/// the token lookup has to win. Pins the ordering the receiver stub was moved to establish.
#[test]
fn safe_transfer_reports_an_unknown_token_before_the_receiver_limit() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (holder_address, holder, _) = mint_to_funded_purse(&alice, collection, item, 0xBB);
		let target = collection_address(collection);
		// A precompile address carries code, as in `safe_transfer_reaches_purses_but_not_code`.
		let with_code = Address::from(factory_address().0);

		for input in [
			IScarcityCollection::safeTransferFrom_1Call {
				from: holder_address.0.into(),
				to: with_code,
				tokenId: U256::from(u64::MAX),
			}
			.abi_encode(),
			IScarcityCollection::safeTransferFrom_0Call {
				from: holder_address.0.into(),
				to: with_code,
				tokenId: U256::from(u64::MAX),
				data: Bytes::from_static(b"payload"),
			}
			.abi_encode(),
		] {
			call_reverted_with(&holder, target, input, "unknown token");
		}
	});
}

/// A caller that is neither the holder nor `from` is told about `from` first.
///
/// Each half is covered alone elsewhere; this pins which wins when both are wrong, so the
/// precedence cannot be swapped silently.
#[test]
fn holder_transfer_reports_the_wrong_holder_before_the_wrong_caller() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (_, _, instance) = mint_to_funded_purse(&alice, collection, item, 0xBB);
		let (stranger_address, stranger) = purse(0xCC);
		let (empty_address, _) = purse(0xEE);
		let target = collection_address(collection);

		// `from` names a purse that does not hold the token, and the caller is a third party.
		call_reverted_with(
			&stranger,
			target,
			IScarcityCollection::transferFromCall {
				from: stranger_address.0.into(),
				to: empty_address.0.into(),
				tokenId: U256::from(instance),
			}
			.abi_encode(),
			"transfer from the wrong holder",
		);
	});
}

/// A purse that has never held anything reads as holding nothing.
///
/// The other `balanceOf` answers are a holder, a holder of another collection and the zero
/// address; this names the plain empty case so the branch is covered by intent.
#[test]
fn balance_of_a_purse_that_never_held_anything_is_zero() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let (never_used, _) = purse(0xAF);
		let target = collection_address(collection);

		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::balanceOfCall { owner: never_used.0.into() }.abi_encode(),
		);
		let balance = IScarcityCollection::balanceOfCall::abi_decode_returns(&data).unwrap();
		assert_eq!(balance, U256::ZERO);
	});
}

/// `tokenOfOwnerByIndex` answers index 0 for a holder and refuses everything else.
#[test]
fn token_of_owner_by_index_serves_index_zero_only() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (holder_address, _, instance) = mint_to_funded_purse(&alice, collection, item, 0xBB);
		let (empty_address, _) = purse(0xEE);
		let target = collection_address(collection);
		let held: Address = holder_address.0.into();

		let data = call_ok(
			&alice,
			target,
			IScarcityCollection::tokenOfOwnerByIndexCall { owner: held, index: U256::ZERO }
				.abi_encode(),
		);
		let token =
			IScarcityCollection::tokenOfOwnerByIndexCall::abi_decode_returns(&data).unwrap();
		assert_eq!(token, U256::from(instance));

		// A purse holds at most one instance, so its balance is 1 and index 1 is out of range.
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::tokenOfOwnerByIndexCall { owner: held, index: U256::from(1u64) }
				.abi_encode(),
			"token index out of range",
		);
		// An owner holding nothing has balance 0, so even index 0 is out of range.
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::tokenOfOwnerByIndexCall {
				owner: empty_address.0.into(),
				index: U256::ZERO,
			}
			.abi_encode(),
			"token index out of range",
		);
		call_reverted_with(
			&alice,
			target,
			IScarcityCollection::tokenOfOwnerByIndexCall {
				owner: Address::ZERO,
				index: U256::ZERO,
			}
			.abi_encode(),
			"balance query for the zero address",
		);

		// A holder of another collection is not this collection's holder.
		let other = setup_collection(&alice);
		call_reverted_with(
			&alice,
			collection_address(other),
			IScarcityCollection::tokenOfOwnerByIndexCall { owner: held, index: U256::ZERO }
				.abi_encode(),
			"token index out of range",
		);
	});
}

/// Native operations produce no EVM log, which consumers would expect for every move under
/// ERC-721 and for every `tokenURI` change under ERC-4906, both of which this address claims.
///
/// Pins the gap deliberately rather than describing it: the planned reconstruction of these logs
/// inside `pallet-revive` will make this test fail, which is the point at which it should be
/// replaced by an assertion on the reconstructed log.
#[test]
fn native_operations_emit_no_evm_log() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (_, holder, instance) = mint_to_funded_purse(&alice, collection, item, 0xBB);
		let (_, to) = purse(0xCC);

		// The mint above went through the pallet, not the precompile.
		assert_no_contract_event();

		assert_ok!(Scarcity::force_transfer(
			RuntimeOrigin::signed(alice.clone()),
			instance,
			to.clone()
		));
		assert_no_contract_event();

		assert_ok!(pallet_scarcity::Pallet::<Test>::do_transfer_by_holder(
			&to,
			instance,
			holder.clone()
		));
		assert_no_contract_event();

		// The same `tokenURI` writes the precompile announces under ERC-4906, taken natively.
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(alice.clone()),
			collection,
			key(TOKEN_URI_KEY),
			Some(value(b"collection"))
		));
		assert_ok!(Scarcity::set_item_metadata(
			RuntimeOrigin::signed(alice.clone()),
			collection,
			item,
			key(TOKEN_URI_KEY),
			Some(value(b"item"))
		));
		assert_ok!(Scarcity::set_instance_metadata(
			RuntimeOrigin::signed(alice),
			instance,
			key(TOKEN_URI_KEY),
			Some(value(b"instance"))
		));
		assert_no_contract_event();
	});
}

/// Metadata writes announce a change only where a standard read would see one.
#[test]
fn metadata_writes_announce_only_standard_reads() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (_, _, instance) = mint_to_funded_purse(&alice, collection, item, 0xBB);
		let target = collection_address(collection);
		let token = U256::from(instance);
		let every_token = IScarcityCollection::BatchMetadataUpdate {
			fromTokenId: U256::ZERO,
			toTokenId: U256::MAX,
		};

		// Instance scope names the one token it changed.
		System::reset_events();
		call_ok(
			&alice,
			target,
			IScarcityCollection::setInstanceMetadataCall {
				tokenId: token,
				key: Bytes::from_static(TOKEN_URI_KEY),
				value: Bytes::from_static(b"ipfs://one"),
			}
			.abi_encode(),
		);
		assert_only_contract_event(target, IScarcityCollection::MetadataUpdate { tokenId: token });

		// Item and collection scope reach an unbounded set, so they announce every id.
		System::reset_events();
		call_ok(
			&alice,
			target,
			IScarcityCollection::setItemMetadataCall {
				item,
				key: Bytes::from_static(TOKEN_URI_KEY),
				value: Bytes::from_static(b"ipfs://item"),
			}
			.abi_encode(),
		);
		assert_only_contract_event(target, every_token.clone());

		System::reset_events();
		call_ok(
			&alice,
			target,
			IScarcityCollection::removeCollectionMetadataCall {
				key: Bytes::from_static(TOKEN_URI_KEY),
			}
			.abi_encode(),
		);
		assert_only_contract_event(target, every_token);

		// `contractURI` is ERC-7572's, and only at collection scope.
		System::reset_events();
		call_ok(
			&alice,
			target,
			IScarcityCollection::setCollectionMetadataCall {
				key: Bytes::from_static(CONTRACT_URI_KEY),
				value: Bytes::from_static(b"ipfs://collection"),
			}
			.abi_encode(),
		);
		assert_only_contract_event(target, IScarcityCollection::ContractURIUpdated {});

		System::reset_events();
		call_ok(
			&alice,
			target,
			IScarcityCollection::setItemMetadataCall {
				item,
				key: Bytes::from_static(CONTRACT_URI_KEY),
				value: Bytes::from_static(b"ignored"),
			}
			.abi_encode(),
		);
		assert_no_contract_event();

		// No standard read reflects these, so announcing them would have consumers refetch a
		// document that did not move.
		for (key, value) in [
			(NAME_KEY, b"Ducks".as_slice()),
			(SYMBOL_KEY, b"DUCK".as_slice()),
			(b"rarity".as_slice(), b"legendary".as_slice()),
		] {
			System::reset_events();
			call_ok(
				&alice,
				target,
				IScarcityCollection::setCollectionMetadataCall {
					key: Bytes::copy_from_slice(key),
					value: Bytes::copy_from_slice(value),
				}
				.abi_encode(),
			);
			assert_no_contract_event();
		}
	});
}

/// Handover and deletion are visible to a log-driven indexer.
#[test]
fn handover_and_deletion_emit_events() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let bob = id_to_account(2);
		map_account(&bob);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let target = collection_address(collection);

		// Nomination moves no authority, so it announces nothing.
		System::reset_events();
		call_ok(
			&alice,
			target,
			IScarcityCollection::nominateCollectionOwnerCall {
				successor: address_of::<Test>(&bob),
			}
			.abi_encode(),
		);
		assert_no_contract_event();

		System::reset_events();
		call_ok(&bob, target, IScarcityCollection::claimCollectionOwnershipCall {}.abi_encode());
		assert_only_contract_event(
			target,
			IScarcityCollection::OwnershipTransferred {
				previousOwner: address_of::<Test>(&alice),
				newOwner: address_of::<Test>(&bob),
			},
		);

		System::reset_events();
		call_ok(&bob, target, IScarcityCollection::deleteItemCall { item }.abi_encode());
		assert_only_contract_event(target, IScarcityCollection::ItemDeleted { item });

		System::reset_events();
		call_ok(&bob, target, IScarcityCollection::deleteCollectionCall {}.abi_encode());
		assert_only_contract_event(target, IScarcityCollection::CollectionDeleted {});
	});
}

/// A basis-points value that is not exactly a `u128` is refused, not read from its prefix.
///
/// `Decode` would take the first 16 bytes of a longer value and ignore the rest, so an
/// ABI-encoded `uint256` written here would quote whatever its low half happened to say. Both
/// orderings are covered because the mistake is plausible in either: a little-endian value pads on
/// the right, an ABI word on the left.
#[test]
fn royalty_basis_points_reject_a_value_that_is_not_exactly_u128() {
	new_test_ext().execute_with(|| {
		let alice = id_to_account(1);
		let collection = setup_collection(&alice);
		let item = setup_item(&alice, collection);
		let (_, _, instance) = mint_to_funded_purse(&alice, collection, item, 0xBB);
		let (receiver, _) = purse(0xCC);
		let target = collection_address(collection);
		let token = U256::from(instance);

		set_collection_royalty(&alice, collection, receiver, 250);
		let quoted = royalty_of(&alice, target, token, 10_000);
		assert_eq!(quoted.royaltyAmount, U256::from(250u64), "the well-formed pair must quote");

		// A 32-byte ABI word holding the same share, as an ABI encoder writes it: big-endian,
		// so its leading bytes are the zeros a `u128` decode would read instead.
		let mut abi_word = [0u8; 32];
		abi_word[16..].copy_from_slice(&250u128.to_be_bytes());
		// The same share with trailing padding, which `Decode` would also have accepted.
		let mut padded = codec::Encode::encode(&250u128);
		padded.extend_from_slice(&[0u8; 4]);

		for encoded in [abi_word.to_vec(), padded] {
			assert_ok!(Scarcity::set_collection_metadata(
				RuntimeOrigin::signed(alice.clone()),
				collection,
				key(ROYALTY_BASIS_POINTS_KEY),
				Some(value(&encoded))
			));
			let quoted = royalty_of(&alice, target, token, 10_000);
			assert_eq!(quoted.receiver, Address::ZERO, "over-long value must not quote");
			assert_eq!(quoted.royaltyAmount, U256::ZERO);
		}
	});
}

/// Frame-flag guards, driven through `pallet_revive::precompiles::run`, which executes a
/// precompile inside a frame with controlled read-only and delegate-call flags.
///
/// `pallet-revive` exports that harness only under `runtime-benchmarks`, and enabling the
/// feature unconditionally would grow the benchmark-only methods of the FRAME traits without
/// enabling the feature on the pallets that implement them, breaking any workspace-wide
/// build. The gate keeps it to feature-enabled runs, which is where CI exercises it.
#[cfg(feature = "runtime-benchmarks")]
mod guards {
	use super::*;
	use pallet_revive::precompiles::run::{
		precompile as run_precompile, CallSetup, VmBinaryModule,
	};
	use IScarcityCollection::IScarcityCollectionCalls;
	use IScarcityFactory::IScarcityFactoryCalls;

	fn collection_owner_call() -> IScarcityCollectionCalls {
		IScarcityCollectionCalls::collectionOwner(IScarcityCollection::collectionOwnerCall {})
	}

	/// Every selector, with arguments that need not resolve: both frame guards run ahead of the
	/// collection lookup, so a denial does not depend on the call describing anything live.
	fn every_call() -> Vec<IScarcityCollectionCalls> {
		let (held, _) = purse(0xDD);
		let (empty, _) = purse(0xEE);
		all_collection_calls(U256::from(1u64), 0, held.0.into(), empty.0.into())
	}

	fn assert_denied_with(result: Result<Vec<u8>, Error>, expected: pallet_revive::Error<Test>) {
		let expected: DispatchError = expected.into();
		match result {
			Err(Error::Error(e)) => assert_eq!(e.error, expected),
			other => panic!("expected {expected:?}, got {other:?}"),
		}
	}

	/// Whether a result is the frame-guard denial rather than any other outcome.
	fn is_denied_with(
		result: &Result<Vec<u8>, Error>,
		expected: pallet_revive::Error<Test>,
	) -> bool {
		let expected: DispatchError = expected.into();
		matches!(result, Err(Error::Error(e)) if e.error == expected)
	}

	#[test]
	fn delegate_call_is_denied() {
		new_test_ext().execute_with(|| {
			let alice = id_to_account(1);
			let collection = setup_collection(&alice);

			let mut setup = CallSetup::<Test>::new(VmBinaryModule::dummy());
			setup.set_delegate_call(true);
			let (mut ext, _) = setup.ext();

			// Reads and mutations alike, over the whole interface: a delegate call executes with
			// the delegator's address, so the collection id in the address is not the callee's and
			// no selector may be served.
			for call in every_call() {
				let result = run_precompile::<ScarcityCollection<Test, COLLECTION_PREFIX>, _>(
					&mut ext,
					&collection_address(collection).0,
					&call,
				);
				assert!(
					is_denied_with(&result, pallet_revive::Error::<Test>::PrecompileDelegateDenied),
					"selector {:02x?} was served in a delegate call: {result:?}",
					call.selector()
				);
			}

			let factory = run_precompile::<ScarcityFactory<Test, FACTORY_INDEX>, _>(
				&mut ext,
				&factory_address().0,
				&IScarcityFactoryCalls::createCollection(IScarcityFactory::createCollectionCall {}),
			);
			assert_denied_with(factory, pallet_revive::Error::<Test>::PrecompileDelegateDenied);
		});
	}

	/// Every mutating selector must be refused in a read-only frame, and no read may be.
	///
	/// Driven over the whole interface rather than a sample, because `is_mutating` is the only
	/// thing standing between a `STATICCALL` and a pallet write: the precompile calls
	/// `Scarcity::<T>::do_*` directly, so `pallet-revive` never sees the writes and cannot refuse
	/// them itself. A single arm classified as a read would fail open silently, which one sampled
	/// selector would not catch.
	#[test]
	fn read_only_frame_denies_every_mutation_and_serves_every_read() {
		new_test_ext().execute_with(|| {
			let alice = id_to_account(1);
			let collection = setup_collection(&alice);

			let mut setup = CallSetup::<Test>::new(VmBinaryModule::dummy());
			setup.set_read_only(true);
			let (mut ext, _) = setup.ext();

			let mut mutating = 0;
			for call in every_call() {
				let result = run_precompile::<ScarcityCollection<Test, COLLECTION_PREFIX>, _>(
					&mut ext,
					&collection_address(collection).0,
					&call,
				);
				let denied =
					is_denied_with(&result, pallet_revive::Error::<Test>::StateChangeDenied);
				let selector = call.selector();
				if crate::collection::is_mutating(&call) {
					mutating += 1;
					assert!(denied, "selector {selector:02x?} mutates but was served: {result:?}");
				} else {
					// A read may still revert on these arguments; what it must not do is trip the
					// state-change guard.
					assert!(
						!denied,
						"selector {selector:02x?} reads but was refused as a mutation"
					);
				}
			}
			// A tripwire, not a safety net: `all_collection_calls` already pins that every selector
			// is driven, and a new mutator classified as a read leaves this count untouched. That
			// one is caught by review or not at all.
			assert_eq!(mutating, 20, "mutating arm count changed; update this test deliberately");

			let factory = run_precompile::<ScarcityFactory<Test, FACTORY_INDEX>, _>(
				&mut ext,
				&factory_address().0,
				&IScarcityFactoryCalls::createCollection(IScarcityFactory::createCollectionCall {}),
			);
			assert_denied_with(factory, pallet_revive::Error::<Test>::StateChangeDenied);

			// Views keep answering inside a STATICCALL frame, with a real value.
			let read = run_precompile::<ScarcityCollection<Test, COLLECTION_PREFIX>, _>(
				&mut ext,
				&collection_address(collection).0,
				&collection_owner_call(),
			)
			.expect("reads must succeed in a read-only frame");
			let owner =
				IScarcityCollection::collectionOwnerCall::abi_decode_returns(&read).unwrap();
			assert_eq!(owner, address_of::<Test>(&alice));
		});
	}
}
