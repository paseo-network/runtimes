// This file is part of Substrate.

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

//! Tests for the scarcity pallet's ownership and transaction-extension invariants.

use crate::{
	extension::{AsScarcity, AsScarcityInfo, CustomInvalidity, Pre, Val},
	mock::*,
	runtime_api::{
		BatchError, MetadataLayers, MetadataQuery, MetadataTarget, MAX_METADATA_QUERIES,
	},
	CollectionMetadata, Collections, Error, Event, InstanceDeposits, InstanceMetadata,
	InstanceMetadataCount, Instances, ItemDefs, ItemMetadata, LockInfo, Locked, MetadataKeyOf,
	MetadataValueOf, MintWithoutDeposit, NextCollectionId, NextInstanceId, Nft, NftsByOwner,
	OnCollectionDeleted, Origin, Transferability,
};
use codec::Encode;
#[cfg(feature = "try-runtime")]
use frame_support::traits::Hooks;
use frame_support::{assert_noop, assert_ok, dispatch::Pays, traits::OriginTrait};
use sp_runtime::{
	traits::{TransactionExtension, TxBaseImplication},
	transaction_validity::{
		InvalidTransaction, TransactionSource, TransactionValidityError, ValidTransaction,
	},
	DispatchResult, TryRuntimeError,
};

const OWNER: u64 = 1;
const OTHER: u64 = 2;
const RECIPIENT: u64 = 3;

fn key(value: &[u8]) -> MetadataKeyOf<Test> {
	value.to_vec().try_into().expect("metadata key fits the test bound")
}

fn value(value: &[u8]) -> MetadataValueOf<Test> {
	value.to_vec().try_into().expect("metadata value fits the test bound")
}

fn metadata(entries: &[(&[u8], &[u8])]) -> Vec<(MetadataKeyOf<Test>, MetadataValueOf<Test>)> {
	entries
		.iter()
		.map(|(key_bytes, value_bytes)| (key(key_bytes), value(value_bytes)))
		.collect::<Vec<_>>()
}

fn define(collection: u32) {
	define_as(collection, Transferability::Transferable);
}

fn define_as(collection: u32, transferability: Transferability) {
	assert_ok!(Scarcity::define_item(
		RuntimeOrigin::signed(OWNER),
		collection,
		transferability,
		metadata(&[])
	));
}

fn setup_item() {
	assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));
	define(0);
}

fn mint(item: u32, to: u64) {
	assert_ok!(Scarcity::mint(RuntimeOrigin::signed(OWNER), 0, item, to, metadata(&[])));
}

fn mint_with_metadata(item: u32, to: u64, entries: &[(&[u8], &[u8])]) {
	assert_ok!(Scarcity::mint(RuntimeOrigin::signed(OWNER), 0, item, to, metadata(entries),));
}

fn nft_origin(owner: u64, nft: Nft) -> RuntimeOrigin {
	RuntimeOrigin::from(Origin::<Test>::Nft { owner, nft })
}

fn transfer_call(to: u64) -> RuntimeCall {
	RuntimeCall::Scarcity(crate::Call::transfer { to })
}

fn burn_call() -> RuntimeCall {
	RuntimeCall::Scarcity(crate::Call::burn {})
}

fn authorization(instance: u64, state_nonce: u64) -> AsScarcityInfo {
	AsScarcityInfo::AsNft { instance, state_nonce }
}

fn current_authorization(owner: u64) -> AsScarcityInfo {
	let nft = NftsByOwner::<Test>::get(owner).expect("authorization requires an NFT");
	authorization(nft.instance, nft.state_nonce)
}

fn scarcity_extension(info: AsScarcityInfo) -> AsScarcity<Test> {
	AsScarcity::new(Some(info))
}

fn extension_for_val(val: &Val<Test>) -> AsScarcity<Test> {
	match val {
		Val::NotUsing => AsScarcity::new(None),
		Val::UsingNft { instance, state_nonce, .. } =>
			scarcity_extension(authorization(*instance, *state_nonce)),
	}
}

fn validate_transfer_as(
	signer: u64,
	to: u64,
	info: AsScarcityInfo,
) -> Result<(ValidTransaction, Val<Test>, RuntimeOrigin), TransactionValidityError> {
	let call = transfer_call(to);
	scarcity_extension(info).validate(
		RuntimeOrigin::signed(signer),
		&call,
		&Default::default(),
		0,
		(),
		&TxBaseImplication(()),
		TransactionSource::External,
	)
}

fn validate_transfer(
	signer: u64,
	to: u64,
) -> Result<(ValidTransaction, Val<Test>, RuntimeOrigin), TransactionValidityError> {
	validate_transfer_as(signer, to, current_authorization(signer))
}

fn prepare_transfer(val: Val<Test>, origin: &RuntimeOrigin, to: u64) -> Pre<Test> {
	let call = transfer_call(to);
	extension_for_val(&val)
		.prepare(val, origin, &call, &Default::default(), 0)
		.unwrap()
}

fn validate_burn_as(
	signer: u64,
	info: AsScarcityInfo,
) -> Result<(ValidTransaction, Val<Test>, RuntimeOrigin), TransactionValidityError> {
	let call = burn_call();
	scarcity_extension(info).validate(
		RuntimeOrigin::signed(signer),
		&call,
		&Default::default(),
		0,
		(),
		&TxBaseImplication(()),
		TransactionSource::External,
	)
}

fn validate_burn(
	signer: u64,
) -> Result<(ValidTransaction, Val<Test>, RuntimeOrigin), TransactionValidityError> {
	validate_burn_as(signer, current_authorization(signer))
}

fn prepare_burn(val: Val<Test>, origin: &RuntimeOrigin) -> Pre<Test> {
	let call = burn_call();
	extension_for_val(&val)
		.prepare(val, origin, &call, &Default::default(), 0)
		.unwrap()
}

fn assert_invalidity(error: TransactionValidityError, expected: CustomInvalidity) {
	let expected = expected as u8;
	match error {
		TransactionValidityError::Invalid(InvalidTransaction::Custom(code)) =>
			assert_eq!(code, expected, "wrong custom invalidity"),
		other => panic!("expected custom invalidity {expected}, got {other:?}"),
	}
}

fn assert_no_nft(error: TransactionValidityError) {
	assert_invalidity(error, CustomInvalidity::NoNft);
}

fn assert_state_mismatch(error: TransactionValidityError) {
	assert_invalidity(error, CustomInvalidity::NftStateMismatch);
}

fn post_dispatch(pre: Pre<Test>, result: DispatchResult) {
	assert_ok!(AsScarcity::<Test>::post_dispatch_details(
		pre,
		&Default::default(),
		&Default::default(),
		0,
		&result,
	));
}

fn assert_try_state_error(expected: &'static str) {
	match Scarcity::do_try_state() {
		Err(TryRuntimeError::Other(actual)) => assert_eq!(actual, expected),
		other => panic!("expected try-state error {expected:?}, got {other:?}"),
	}
}

#[test]
fn create_collection_assigns_incremental_ids_and_owner() {
	new_test_ext().execute_with(|| {
		assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));
		assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OTHER)));

		assert_eq!(Collections::<Test>::get(0).unwrap().owner, OWNER);
		assert_eq!(Collections::<Test>::get(1).unwrap().owner, OTHER);
		System::assert_has_event(
			Event::<Test>::CollectionCreated { collection: 0, owner: OWNER }.into(),
		);
		System::assert_has_event(
			Event::<Test>::CollectionCreated { collection: 1, owner: OTHER }.into(),
		);
	});
}

#[test]
fn collection_owner_can_nominate_or_clear_a_successor() {
	new_test_ext().execute_with(|| {
		assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));

		assert_noop!(
			Scarcity::nominate_collection_owner(RuntimeOrigin::signed(OTHER), 0, Some(RECIPIENT)),
			Error::<Test>::NoPermission
		);
		assert_noop!(
			Scarcity::nominate_collection_owner(RuntimeOrigin::signed(OWNER), 99, Some(OTHER)),
			Error::<Test>::UnknownCollection
		);
		assert_noop!(
			Scarcity::nominate_collection_owner(RuntimeOrigin::signed(OWNER), 0, Some(OWNER)),
			Error::<Test>::AlreadyCollectionOwner
		);

		assert_ok!(Scarcity::nominate_collection_owner(
			RuntimeOrigin::signed(OWNER),
			0,
			Some(OTHER),
		));
		let nominated = Collections::<Test>::get(0).expect("collection exists");
		assert_eq!(nominated.owner, OWNER);
		assert_eq!(nominated.pending_owner, Some(OTHER));
		System::assert_has_event(
			Event::<Test>::CollectionOwnerNominated { collection: 0, pending_owner: Some(OTHER) }
				.into(),
		);

		assert_ok!(Scarcity::nominate_collection_owner(RuntimeOrigin::signed(OWNER), 0, None,));
		assert_eq!(Collections::<Test>::get(0).unwrap().pending_owner, None);
	});
}

#[test]
fn only_the_current_nominee_can_claim_collection_ownership() {
	new_test_ext().execute_with(|| {
		assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));
		assert_noop!(
			Scarcity::claim_collection_ownership(RuntimeOrigin::signed(OTHER), 0),
			Error::<Test>::NotPendingCollectionOwner
		);

		assert_ok!(Scarcity::nominate_collection_owner(
			RuntimeOrigin::signed(OWNER),
			0,
			Some(OTHER),
		));
		assert_noop!(
			Scarcity::claim_collection_ownership(RuntimeOrigin::signed(RECIPIENT), 0),
			Error::<Test>::NotPendingCollectionOwner
		);
		assert_noop!(
			Scarcity::claim_collection_ownership(RuntimeOrigin::signed(OTHER), 99),
			Error::<Test>::UnknownCollection
		);
	});
}

#[test]
fn claim_fails_atomically_when_nominee_cannot_back_the_deposit() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			key(b"name"),
			Some(value(b"collection")),
		));
		let before = Collections::<Test>::get(0).expect("collection exists");
		let owner_hold = held(OWNER);
		assert_eq!(held(99), 0);

		assert_ok!(Scarcity::nominate_collection_owner(RuntimeOrigin::signed(OWNER), 0, Some(99),));
		assert!(
			Scarcity::claim_collection_ownership(RuntimeOrigin::signed(99), 0).is_err(),
			"an unfunded nominee cannot assume the collection deposit",
		);

		let after = Collections::<Test>::get(0).expect("collection remains");
		assert_eq!(after.owner, OWNER);
		assert_eq!(after.pending_owner, Some(99));
		assert_eq!(after.owner_deposit, before.owner_deposit);
		assert_eq!(held(OWNER), owner_hold);
		assert_eq!(held(99), 0);
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn claim_moves_exact_collection_deposit_and_authority() {
	new_test_ext().execute_with(|| {
		setup_item();
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			key(b"name"),
			Some(value(b"collection")),
		));
		mint_with_metadata(0, RECIPIENT, &[(b"unique", b"claimed")]);
		let instance_deposit =
			InstanceDeposits::<Test>::get(0).expect("ordinary mint has a deposit");
		let instance_metadata_deposit = InstanceMetadata::<Test>::get(0, key(b"unique"))
			.expect("metadata exists")
			.deposit;

		// A second collection proves that claiming one collection does not release the old
		// owner's unrelated holds.
		assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));
		let old_owner_hold = held(OWNER);
		let moved_deposit = Collections::<Test>::get(0).unwrap().owner_deposit;
		let remaining_deposit = Collections::<Test>::get(1).unwrap().owner_deposit;
		assert_eq!(old_owner_hold, moved_deposit + remaining_deposit);

		assert_ok!(Scarcity::nominate_collection_owner(
			RuntimeOrigin::signed(OWNER),
			0,
			Some(OTHER),
		));
		assert_ok!(Scarcity::claim_collection_ownership(RuntimeOrigin::signed(OTHER), 0));

		let claimed = Collections::<Test>::get(0).expect("claimed collection exists");
		assert_eq!(claimed.owner, OTHER);
		assert_eq!(claimed.pending_owner, None);
		assert_eq!(claimed.owner_deposit, moved_deposit);
		assert_eq!(held(OWNER), remaining_deposit);
		assert_eq!(held(OTHER), moved_deposit);
		assert_eq!(Collections::<Test>::get(1).unwrap().owner, OWNER);
		System::assert_has_event(
			Event::<Test>::CollectionOwnerChanged {
				collection: 0,
				old_owner: OWNER,
				new_owner: OTHER,
			}
			.into(),
		);

		assert_noop!(
			Scarcity::define_item(
				RuntimeOrigin::signed(OWNER),
				0,
				Transferability::Transferable,
				metadata(&[])
			),
			Error::<Test>::NoPermission
		);
		assert_ok!(Scarcity::define_item(
			RuntimeOrigin::signed(OTHER),
			0,
			Transferability::Transferable,
			metadata(&[])
		));
		assert_noop!(
			Scarcity::set_collection_metadata(
				RuntimeOrigin::signed(OWNER),
				0,
				key(b"name"),
				Some(value(b"old owner")),
			),
			Error::<Test>::NoPermission
		);
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OTHER),
			0,
			key(b"name"),
			Some(value(b"new owner")),
		));
		assert_noop!(
			Scarcity::set_instance_metadata(
				RuntimeOrigin::signed(OWNER),
				0,
				key(b"unique"),
				Some(value(b"claimed")),
			),
			Error::<Test>::NoPermission
		);
		assert_ok!(Scarcity::set_instance_metadata(
			RuntimeOrigin::signed(OTHER),
			0,
			key(b"unique"),
			Some(value(b"updated")),
		));

		let before_burn = held(OTHER);
		assert_noop!(
			Scarcity::force_burn(RuntimeOrigin::signed(OWNER), 0),
			Error::<Test>::NoPermission
		);
		assert_ok!(Scarcity::force_burn(RuntimeOrigin::signed(OTHER), 0));
		assert_eq!(held(OTHER), before_burn - instance_deposit - instance_metadata_deposit);
		assert_eq!(held(OWNER), remaining_deposit);
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn create_collection_holds_and_tracks_its_deposit() {
	new_test_ext().execute_with(|| {
		assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));

		let info = Collections::<Test>::get(0).expect("collection exists");
		assert!(info.collection_deposit > 0);
		assert_eq!(info.owner_deposit, info.collection_deposit);
		assert_eq!(info.pending_owner, None);
		assert_eq!(info.item_count, 0);
		assert_eq!(info.metadata_count, 0);
		assert_eq!(held(OWNER), info.owner_deposit);
	});
}

#[test]
fn define_item_holds_and_tracks_its_deposit() {
	new_test_ext().execute_with(|| {
		assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));
		let held_before = held(OWNER);

		define(0);
		let definition = ItemDefs::<Test>::get(0, 0).expect("item definition exists");
		assert!(definition.deposit > 0);
		assert_eq!(definition.supply, 0);
		assert_eq!(definition.live_supply, 0);
		assert_eq!(definition.metadata_count, 0);
		assert_eq!(Collections::<Test>::get(0).unwrap().item_count, 1);
		assert_eq!(held(OWNER), held_before + definition.deposit);
		assert_eq!(Collections::<Test>::get(0).unwrap().owner_deposit, held(OWNER),);
	});
}

#[test]
fn mint_charges_collection_owner_and_stores_instance_deposit() {
	new_test_ext().execute_with(|| {
		setup_item();
		let held_before = held(OWNER);

		mint(0, RECIPIENT);
		let deposit = InstanceDeposits::<Test>::get(0).expect("paid mint stores its deposit");
		assert!(deposit > 0);
		assert_eq!(held(OWNER), held_before + deposit);
		assert_eq!(Collections::<Test>::get(0).unwrap().owner_deposit, held(OWNER),);
		assert_eq!(frame_system::Account::<Test>::get(RECIPIENT).sufficients, 0);
	});
}

#[test]
fn define_item_requires_collection_owner() {
	new_test_ext().execute_with(|| {
		assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));
		assert_noop!(
			Scarcity::define_item(
				RuntimeOrigin::signed(OTHER),
				0,
				Transferability::Transferable,
				metadata(&[])
			),
			Error::<Test>::NoPermission
		);
		assert_noop!(
			Scarcity::define_item(
				RuntimeOrigin::signed(OWNER),
				99,
				Transferability::Transferable,
				metadata(&[])
			),
			Error::<Test>::UnknownCollection
		);
	});
}

#[test]
fn define_item_assigns_incremental_indexes() {
	new_test_ext().execute_with(|| {
		assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));
		define(0);
		define(0);
		define(0);

		assert!(ItemDefs::<Test>::contains_key(0, 0));
		assert!(ItemDefs::<Test>::contains_key(0, 1));
		assert!(ItemDefs::<Test>::contains_key(0, 2));
		assert_eq!(Collections::<Test>::get(0).unwrap().next_item_index, 3);
		assert_eq!(ItemDefs::<Test>::get(0, 0).unwrap().supply, 0);
		assert_eq!(ItemDefs::<Test>::get(0, 1).unwrap().supply, 0);
		assert_eq!(ItemDefs::<Test>::get(0, 2).unwrap().supply, 0);
		System::assert_has_event(Event::<Test>::ItemDefined { collection: 0, item: 2 }.into());
	});
}

#[test]
fn metadata_resolution_prefers_item_then_falls_back_to_collection() {
	new_test_ext().execute_with(|| {
		setup_item();
		let shared = key(b"shared");
		let inherited = key(b"inherited");
		let absent = key(b"absent");

		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			shared.clone(),
			Some(value(b"collection")),
		));
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			inherited.clone(),
			Some(value(b"default")),
		));
		assert_ok!(Scarcity::set_item_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			0,
			shared.clone(),
			Some(value(b"item")),
		));

		assert_eq!(Scarcity::collection_metadata_of(0, &shared), Some(value(b"collection")));
		assert_eq!(Scarcity::item_metadata_of(0, 0, &shared), Some(value(b"item")));
		assert_eq!(Scarcity::item_metadata_of(0, 0, &inherited), Some(value(b"default")));
		assert_eq!(Scarcity::item_metadata_of(0, 0, &absent), None);
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn instance_metadata_overrides_item_and_collection_without_affecting_other_mints() {
	new_test_ext().execute_with(|| {
		setup_item();
		let shared = key(b"shared");
		let inherited = key(b"inherited");
		let unique = key(b"unique");

		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			shared.clone(),
			Some(value(b"collection")),
		));
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			inherited.clone(),
			Some(value(b"default")),
		));
		assert_ok!(Scarcity::set_item_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			0,
			shared.clone(),
			Some(value(b"item")),
		));

		mint_with_metadata(0, RECIPIENT, &[(b"shared", b"instance"), (b"unique", b"first")]);
		mint(0, OTHER);

		assert_eq!(Scarcity::instance_metadata_of(0, &shared), Some(value(b"instance")));
		assert_eq!(Scarcity::instance_metadata_of(0, &inherited), Some(value(b"default")));
		assert_eq!(Scarcity::instance_metadata_of(0, &unique), Some(value(b"first")));
		assert_eq!(Scarcity::instance_metadata_of(1, &shared), Some(value(b"item")));
		assert_eq!(Scarcity::instance_metadata_of(1, &inherited), Some(value(b"default")));
		assert_eq!(Scarcity::instance_metadata_of(1, &unique), None);
		assert_eq!(Scarcity::item_metadata_of(0, 0, &shared), Some(value(b"item")));
		assert_eq!(InstanceMetadataCount::<Test>::get(0), 2);
		assert_eq!(InstanceMetadataCount::<Test>::get(1), 0);
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn metadata_batch_returns_positionally_aligned_stored_layers() {
	new_test_ext().execute_with(|| {
		setup_item();
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			key(b"collection"),
			Some(value(b"collection-value"))
		));
		assert_ok!(Scarcity::set_item_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			0,
			key(b"item"),
			Some(value(b"item-value"))
		));
		mint_with_metadata(0, RECIPIENT, &[(b"instance", b"instance-value")]);

		let empty = MetadataLayers::default();
		assert_eq!(
			Scarcity::metadata_batch(vec![
				MetadataQuery::Instance(0),
				MetadataQuery::Item { collection: 0, item: 0 },
				MetadataQuery::Collection(0),
				MetadataQuery::Instance(99),
				MetadataQuery::Item { collection: 0, item: 99 },
				MetadataQuery::Collection(99),
			]),
			Ok(vec![
				MetadataLayers {
					resolved: Some(MetadataTarget::Instance {
						instance: 0,
						collection: 0,
						item: 0,
					}),
					collection: vec![(b"collection".to_vec(), b"collection-value".to_vec())],
					item: vec![(b"item".to_vec(), b"item-value".to_vec())],
					instance: vec![(b"instance".to_vec(), b"instance-value".to_vec())],
				},
				MetadataLayers {
					resolved: Some(MetadataTarget::Item { collection: 0, item: 0 }),
					collection: vec![(b"collection".to_vec(), b"collection-value".to_vec())],
					item: vec![(b"item".to_vec(), b"item-value".to_vec())],
					instance: Vec::new(),
				},
				MetadataLayers {
					resolved: Some(MetadataTarget::Collection(0)),
					collection: vec![(b"collection".to_vec(), b"collection-value".to_vec())],
					item: Vec::new(),
					instance: Vec::new(),
				},
				empty.clone(),
				empty.clone(),
				empty,
			])
		);
	});
}

#[test]
fn metadata_batch_orders_entries_by_raw_key_bytes() {
	new_test_ext().execute_with(|| {
		setup_item();
		for (k, v) in [
			(&b"zebra"[..], &b"5"[..]),
			(&b"alpha"[..], &b"0"[..]),
			(&b"mid"[..], &b"2"[..]),
			(&b"beta"[..], &b"1"[..]),
			(&b"omega"[..], &b"4"[..]),
			(&b"nu"[..], &b"3"[..]),
		] {
			assert_ok!(Scarcity::set_collection_metadata(
				RuntimeOrigin::signed(OWNER),
				0,
				key(k),
				Some(value(v))
			));
		}

		// Storage iteration order is hash order; the API promises raw key byte order.
		let layers = Scarcity::metadata_batch(vec![MetadataQuery::Collection(0)])
			.expect("one query fits the cap");
		assert_eq!(
			layers[0].collection,
			vec![
				(b"alpha".to_vec(), b"0".to_vec()),
				(b"beta".to_vec(), b"1".to_vec()),
				(b"mid".to_vec(), b"2".to_vec()),
				(b"nu".to_vec(), b"3".to_vec()),
				(b"omega".to_vec(), b"4".to_vec()),
				(b"zebra".to_vec(), b"5".to_vec()),
			]
		);
	});
}

#[test]
fn metadata_batch_rejects_an_oversized_request() {
	new_test_ext().execute_with(|| {
		assert_eq!(
			Scarcity::metadata_batch(vec![
				MetadataQuery::Collection(0);
				MAX_METADATA_QUERIES as usize + 1
			]),
			Err(BatchError::TooLarge { max: MAX_METADATA_QUERIES })
		);
	});
}

#[test]
fn instance_metadata_mutation_is_collection_owner_only_and_updates_deposits() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		let metadata_key = key(b"instance");
		let held_before = held(OWNER);

		assert_noop!(
			Scarcity::set_instance_metadata(
				RuntimeOrigin::signed(OTHER),
				0,
				metadata_key.clone(),
				Some(value(b"denied")),
			),
			Error::<Test>::NoPermission
		);
		assert_noop!(
			Scarcity::set_instance_metadata(
				RuntimeOrigin::signed(OWNER),
				99,
				metadata_key.clone(),
				Some(value(b"missing")),
			),
			Error::<Test>::UnknownInstance
		);

		assert_ok!(Scarcity::set_instance_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			metadata_key.clone(),
			Some(value(b"one")),
		));
		let first_deposit =
			InstanceMetadata::<Test>::get(0, &metadata_key).expect("entry exists").deposit;
		assert!(first_deposit > 0);
		assert_eq!(InstanceMetadataCount::<Test>::get(0), 1);
		assert_eq!(held(OWNER), held_before + first_deposit);

		assert_ok!(Scarcity::set_instance_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			metadata_key.clone(),
			Some(value(b"longer")),
		));
		let replacement_deposit =
			InstanceMetadata::<Test>::get(0, &metadata_key).expect("entry exists").deposit;
		assert!(replacement_deposit > first_deposit);
		assert_eq!(InstanceMetadataCount::<Test>::get(0), 1);
		assert_eq!(held(OWNER), held_before + replacement_deposit);
		System::assert_has_event(
			Event::<Test>::InstanceMetadataSet { instance: 0, key: metadata_key.clone() }.into(),
		);

		assert_ok!(Scarcity::set_instance_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			metadata_key.clone(),
			None,
		));
		assert!(!InstanceMetadata::<Test>::contains_key(0, &metadata_key));
		assert_eq!(InstanceMetadataCount::<Test>::get(0), 0);
		assert_eq!(held(OWNER), held_before);
		System::assert_has_event(
			Event::<Test>::InstanceMetadataRemoved { instance: 0, key: metadata_key }.into(),
		);
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn metadata_mutation_is_owner_only_at_both_levels() {
	new_test_ext().execute_with(|| {
		setup_item();
		let collection_key = key(b"collection");
		let item_key = key(b"item");

		assert_noop!(
			Scarcity::set_collection_metadata(
				RuntimeOrigin::signed(OTHER),
				0,
				collection_key.clone(),
				Some(value(b"denied")),
			),
			Error::<Test>::NoPermission
		);
		assert_noop!(
			Scarcity::set_item_metadata(
				RuntimeOrigin::signed(OTHER),
				0,
				0,
				item_key.clone(),
				Some(value(b"denied")),
			),
			Error::<Test>::NoPermission
		);

		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			collection_key.clone(),
			Some(value(b"first")),
		));
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			collection_key.clone(),
			Some(value(b"second")),
		));
		assert_ok!(Scarcity::set_item_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			0,
			item_key.clone(),
			Some(value(b"first")),
		));
		assert_ok!(Scarcity::set_item_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			0,
			item_key.clone(),
			Some(value(b"second")),
		));
		System::assert_has_event(
			Event::<Test>::CollectionMetadataSet { collection: 0, key: collection_key.clone() }
				.into(),
		);
		System::assert_has_event(
			Event::<Test>::ItemMetadataSet { collection: 0, item: 0, key: item_key.clone() }.into(),
		);
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			collection_key.clone(),
			None,
		));
		assert_ok!(Scarcity::set_item_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			0,
			item_key.clone(),
			None,
		));
		System::assert_has_event(
			Event::<Test>::CollectionMetadataRemoved { collection: 0, key: collection_key.clone() }
				.into(),
		);
		System::assert_has_event(
			Event::<Test>::ItemMetadataRemoved { collection: 0, item: 0, key: item_key.clone() }
				.into(),
		);

		assert!(!CollectionMetadata::<Test>::contains_key(0, collection_key));
		assert!(!ItemMetadata::<Test>::contains_key((0, 0, item_key)));
		assert_ok!(Scarcity::do_try_state());
	});
}

/// The policy runs on every path that stores a value, which is the point of holding it here
/// rather than at an interface: the three setters, and the entries `define_item` and `mint`
/// carry. Removals have no value to judge, so they pass regardless.
#[test]
fn the_metadata_policy_refuses_a_value_on_every_write_path() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		let instance = NftsByOwner::<Test>::get(RECIPIENT).unwrap().instance;
		let refused = |result: DispatchResult| {
			assert_noop!(result, sp_runtime::DispatchError::Other("policed value"));
		};

		refused(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			key(POLICED_KEY),
			Some(value(REJECTED_VALUE)),
		));
		refused(Scarcity::set_item_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			0,
			key(POLICED_KEY),
			Some(value(REJECTED_VALUE)),
		));
		refused(Scarcity::set_instance_metadata(
			RuntimeOrigin::signed(OWNER),
			instance,
			key(POLICED_KEY),
			Some(value(REJECTED_VALUE)),
		));
		refused(Scarcity::define_item(
			RuntimeOrigin::signed(OWNER),
			0,
			Transferability::Transferable,
			metadata(&[(POLICED_KEY, REJECTED_VALUE)]),
		));
		refused(Scarcity::mint(
			RuntimeOrigin::signed(OWNER),
			0,
			0,
			OTHER,
			metadata(&[(POLICED_KEY, REJECTED_VALUE)]),
		));

		// The policy judges the value, not the key: the same key takes anything else, and
		// removing it needs no judgement at all.
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			key(POLICED_KEY),
			Some(value(b"yes")),
		));
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			key(POLICED_KEY),
			None,
		));

		// Nothing the refused calls touched was left behind.
		assert!(!ItemMetadata::<Test>::contains_key((0, 0, key(POLICED_KEY))));
		assert!(!InstanceMetadata::<Test>::contains_key(instance, key(POLICED_KEY)));
		assert_eq!(Collections::<Test>::get(0).unwrap().next_item_index, 1);
		assert!(NftsByOwner::<Test>::get(OTHER).is_none());
	});
}

/// The depositless mint's hook weight covers the metadata policy, which runs once per entry.
///
/// The only in-tree caller mints bare, so a policy charge missing here would cost nothing today
/// and undercharge the first caller that mints with metadata.
#[test]
fn the_depositless_mint_hook_weight_covers_the_policy() {
	use crate::{MintWithoutDeposit, OnPurseOccupied, ValidateMetadata};

	let policy_weight = |pairs| {
		<<Test as crate::Config>::MetadataPolicy as ValidateMetadata<
			MetadataKeyOf<Test>,
			MetadataValueOf<Test>,
		>>::validate_weight(pairs)
	};
	let hook_weight = |pairs| <Scarcity as MintWithoutDeposit<u64>>::mint_hook_weight(pairs);

	new_test_ext().execute_with(|| {
		assert_eq!(hook_weight(0), RecordPurseOccupancy::on_purse_occupied_weight());
		assert_eq!(
			hook_weight(3),
			RecordPurseOccupancy::on_purse_occupied_weight().saturating_add(policy_weight(3))
		);
		assert!(
			policy_weight(3).all_gt(frame_support::weights::Weight::zero()),
			"not tautological"
		);
	});
}

/// The policy's weight rides on every call that can write metadata, scaled by the pairs it
/// carries, so a runtime cannot wire an expensive rule the calls do not pay for.
#[test]
fn metadata_weights_include_the_policy() {
	use crate::{weights::WeightInfo, ValidateMetadata};
	use frame_support::dispatch::GetDispatchInfo;

	let policy = |pairs| {
		<<Test as crate::Config>::MetadataPolicy as ValidateMetadata<
			MetadataKeyOf<Test>,
			MetadataValueOf<Test>,
		>>::validate_weight(pairs)
	};

	new_test_ext().execute_with(|| {
		let declared = crate::Call::<Test>::set_collection_metadata {
			collection: 0,
			key: key(b"k"),
			value: None,
		}
		.get_dispatch_info()
		.call_weight;
		assert_eq!(
			declared,
			<() as WeightInfo>::set_collection_metadata().saturating_add(policy(1))
		);

		let declared = crate::Call::<Test>::define_item {
			collection: 0,
			transferability: Transferability::Transferable,
			metadata: metadata(&[(b"one", b"1"), (b"two", b"2")]),
		}
		.get_dispatch_info()
		.call_weight;
		assert_eq!(declared, <() as WeightInfo>::define_item(2).saturating_add(policy(2)));
	});
}

/// The `integrity_test` holds each call a runtime sizes to a share of a block.
///
/// The drivers are the metadata entries a call carries and the weight of the runtime hooks, so
/// these raise one of each. A runtime that overshoots produces a call that no block can hold.
mod integrity {
	use super::*;
	use frame_support::{traits::Hooks, weights::Weight};

	#[test]
	fn passes_with_the_default_configuration() {
		new_test_ext().execute_with(|| {
			<Scarcity as Hooks<u64>>::integrity_test();
		});
	}

	/// `mint` carries `MaxInstanceMetadata` entries and `burn` removes them, so the limit sets
	/// the worst case of both.
	#[test]
	#[should_panic = "`mint` worst-case weight"]
	fn rejects_an_oversized_instance_metadata_limit() {
		new_test_ext().execute_with(|| {
			MaxInstanceMetadata::set(&1_000_000);
			<Scarcity as Hooks<u64>>::integrity_test();
		});
	}

	/// The policy runs once per entry on every call that writes metadata, and `mint` is the one
	/// that carries the most.
	#[test]
	#[should_panic = "`mint` worst-case weight"]
	fn rejects_an_expensive_metadata_policy() {
		new_test_ext().execute_with(|| {
			PolicyWeightPerPair::set(&Weight::from_parts(u64::MAX / 100, 0));
			<Scarcity as Hooks<u64>>::integrity_test();
		});
	}

	/// The deletion hook runs on every `delete_collection`, which charges it up front.
	#[test]
	#[should_panic = "`delete_collection` worst-case weight"]
	fn rejects_an_expensive_deletion_hook() {
		new_test_ext().execute_with(|| {
			DeletionHookWeight::set(&Weight::from_parts(u64::MAX / 100, 0));
			<Scarcity as Hooks<u64>>::integrity_test();
		});
	}
}

#[test]
fn metadata_mutation_rejects_unknown_collection_and_item() {
	new_test_ext().execute_with(|| {
		assert_noop!(
			Scarcity::set_collection_metadata(
				RuntimeOrigin::signed(OWNER),
				99,
				key(b"key"),
				Some(value(b"value")),
			),
			Error::<Test>::UnknownCollection
		);
		assert_noop!(
			Scarcity::set_item_metadata(
				RuntimeOrigin::signed(OWNER),
				99,
				0,
				key(b"key"),
				Some(value(b"value")),
			),
			Error::<Test>::UnknownCollection
		);

		assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));
		assert_noop!(
			Scarcity::set_item_metadata(
				RuntimeOrigin::signed(OWNER),
				0,
				99,
				key(b"key"),
				Some(value(b"value")),
			),
			Error::<Test>::UnknownItem
		);
	});
}

#[test]
fn metadata_deposits_update_in_place_and_release() {
	new_test_ext().execute_with(|| {
		setup_item();
		let collection_key = key(b"c");
		let base_hold = held(OWNER);

		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			collection_key.clone(),
			Some(value(b"one")),
		));
		let first_deposit = CollectionMetadata::<Test>::get(0, &collection_key).unwrap().deposit;
		assert!(first_deposit > 0);
		assert_eq!(Collections::<Test>::get(0).unwrap().metadata_count, 1);
		assert_eq!(held(OWNER), base_hold + first_deposit);

		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			collection_key.clone(),
			Some(value(b"longer")),
		));
		let replacement_deposit =
			CollectionMetadata::<Test>::get(0, &collection_key).unwrap().deposit;
		assert!(replacement_deposit > first_deposit);
		assert_eq!(Collections::<Test>::get(0).unwrap().metadata_count, 1);
		assert_eq!(held(OWNER), base_hold + replacement_deposit);

		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			collection_key,
			None,
		));
		assert_eq!(Collections::<Test>::get(0).unwrap().metadata_count, 0);
		assert_eq!(held(OWNER), base_hold);

		let item_key = key(b"i");
		assert_ok!(Scarcity::set_item_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			0,
			item_key.clone(),
			Some(value(b"one")),
		));
		let first_deposit = ItemMetadata::<Test>::get((0, 0, item_key.clone())).unwrap().deposit;
		assert_eq!(ItemDefs::<Test>::get(0, 0).unwrap().metadata_count, 1);
		assert_eq!(held(OWNER), base_hold + first_deposit);
		assert_ok!(Scarcity::set_item_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			0,
			item_key.clone(),
			Some(value(b"longer")),
		));
		let replacement_deposit =
			ItemMetadata::<Test>::get((0, 0, item_key.clone())).unwrap().deposit;
		assert!(replacement_deposit > first_deposit);
		assert_eq!(ItemDefs::<Test>::get(0, 0).unwrap().metadata_count, 1);
		assert_eq!(held(OWNER), base_hold + replacement_deposit);
		assert_ok!(
			Scarcity::set_item_metadata(RuntimeOrigin::signed(OWNER), 0, 0, item_key, None,)
		);
		assert_eq!(ItemDefs::<Test>::get(0, 0).unwrap().metadata_count, 0);
		assert_eq!(held(OWNER), base_hold);
		assert_eq!(Collections::<Test>::get(0).unwrap().owner_deposit, base_hold);
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn removing_absent_metadata_is_a_no_op() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		let events_before = System::events().len();
		let held_before = held(OWNER);

		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			key(b"missing"),
			None,
		));
		assert_ok!(Scarcity::set_item_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			0,
			key(b"missing"),
			None,
		));
		assert_ok!(Scarcity::set_instance_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			key(b"missing"),
			None,
		));

		assert_eq!(held(OWNER), held_before);
		assert_eq!(System::events().len(), events_before);
		assert_eq!(InstanceMetadataCount::<Test>::get(0), 0);
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn define_item_accepts_more_than_old_cap_and_charges_each_metadata_entry() {
	new_test_ext().execute_with(|| {
		assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));
		let held_before = held(OWNER);
		let metadata = (0..41)
			.map(|index| {
				(key(format!("key-{index}").as_bytes()), value(format!("value-{index}").as_bytes()))
			})
			.collect::<Vec<_>>();
		assert_ok!(Scarcity::define_item(
			RuntimeOrigin::signed(OWNER),
			0,
			Transferability::Transferable,
			metadata
		));

		let definition = ItemDefs::<Test>::get(0, 0).expect("item definition exists");
		assert_eq!(ItemMetadata::<Test>::iter_prefix((0, 0)).count(), 41);
		assert_eq!(Scarcity::item_metadata_of(0, 0, &key(b"key-40")), Some(value(b"value-40")),);
		let metadata_deposit = ItemMetadata::<Test>::iter_prefix((0, 0))
			.map(|(_, entry)| entry.deposit)
			.sum::<u64>();
		assert_eq!(held(OWNER), held_before + definition.deposit + metadata_deposit);
		assert_eq!(Collections::<Test>::get(0).unwrap().owner_deposit, held(OWNER));
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn mint_requires_owner_and_existing_def() {
	new_test_ext().execute_with(|| {
		setup_item();
		assert_noop!(
			Scarcity::mint(RuntimeOrigin::signed(OTHER), 0, 0, RECIPIENT, metadata(&[])),
			Error::<Test>::NoPermission
		);
		assert_noop!(
			Scarcity::mint(RuntimeOrigin::signed(OWNER), 0, 1, RECIPIENT, metadata(&[])),
			Error::<Test>::UnknownItem
		);
	});
}

#[test]
fn mint_enforces_one_nft_per_key() {
	new_test_ext().execute_with(|| {
		setup_item();
		define(0);
		mint(0, RECIPIENT);
		assert_noop!(
			Scarcity::mint(RuntimeOrigin::signed(OWNER), 0, 1, RECIPIENT, metadata(&[])),
			Error::<Test>::AddressOccupied
		);
	});
}

#[test]
fn mint_enforces_instance_metadata_limit_atomically() {
	new_test_ext().execute_with(|| {
		setup_item();
		let too_many =
			metadata(&[(b"one", b"1"), (b"two", b"2"), (b"three", b"3"), (b"four", b"4")]);

		assert_noop!(
			Scarcity::mint(RuntimeOrigin::signed(OWNER), 0, 0, RECIPIENT, too_many),
			Error::<Test>::TooManyInstanceMetadata
		);
		assert_eq!(NextInstanceId::<Test>::get(), 0);
		assert_eq!(ItemDefs::<Test>::get(0, 0).unwrap().supply, 0);
		assert!(!NftsByOwner::<Test>::contains_key(RECIPIENT));
		assert!(!InstanceMetadataCount::<Test>::contains_key(0));

		mint_with_metadata(0, RECIPIENT, &[(b"one", b"1"), (b"two", b"2"), (b"three", b"3")]);
		assert_noop!(
			Scarcity::set_instance_metadata(
				RuntimeOrigin::signed(OWNER),
				0,
				key(b"four"),
				Some(value(b"4")),
			),
			Error::<Test>::TooManyInstanceMetadata
		);
		assert_ok!(Scarcity::set_instance_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			key(b"one"),
			Some(value(b"updated")),
		));
		assert_eq!(InstanceMetadataCount::<Test>::get(0), 3);
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn mint_writes_consistent_state() {
	new_test_ext().execute_with(|| {
		setup_item();
		define(0);
		MockNow::set(1_234);
		mint(0, RECIPIENT);

		let nft = NftsByOwner::<Test>::get(RECIPIENT).expect("minted NFT is stored by owner");
		assert_eq!(nft.instance, 0);
		assert_eq!(Instances::<Test>::get(0), Some(RECIPIENT));
		assert!(InstanceMetadataCount::<Test>::contains_key(0));
		assert_eq!(InstanceMetadataCount::<Test>::get(0), 0);
		let definition = ItemDefs::<Test>::get(0, 0).unwrap();
		assert_eq!(definition.supply, 1);
		assert_eq!(definition.live_supply, 1);
		assert_eq!(nft.minted_at, 1_234);
		assert_eq!(nft.last_moved, 1_234);
		System::assert_has_event(
			Event::<Test>::Minted { instance: 0, collection: 0, item: 0, owner: RECIPIENT }.into(),
		);

		MockNow::set(1_235);
		mint(1, 4);
		let second = NftsByOwner::<Test>::get(4).expect("second NFT is stored by owner");
		assert_eq!(second.instance, 1);
		assert_eq!(Instances::<Test>::get(1), Some(4));
		assert_eq!(second.minted_at, 1_235);
	});
}

#[test]
fn mint_charges_for_initial_instance_metadata() {
	new_test_ext().execute_with(|| {
		setup_item();
		let held_before = held(OWNER);

		mint_with_metadata(0, RECIPIENT, &[(b"unique", b"value")]);

		let instance_deposit =
			InstanceDeposits::<Test>::get(0).expect("ordinary mint has a deposit");
		let metadata_deposit = InstanceMetadata::<Test>::get(0, key(b"unique"))
			.expect("metadata exists")
			.deposit;
		assert!(metadata_deposit > 0);
		assert_eq!(InstanceMetadataCount::<Test>::get(0), 1);
		assert_eq!(held(OWNER), held_before + instance_deposit + metadata_deposit);
		assert_eq!(Collections::<Test>::get(0).unwrap().owner_deposit, held(OWNER));
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn mint_without_deposit_waives_deposit_and_increments_supply() {
	new_test_ext().execute_with(|| {
		setup_item();
		let held_before = held(OWNER);
		MockNow::set(1_234);

		assert_eq!(Scarcity::mint_without_deposit(0, 0, RECIPIENT, metadata(&[])), Ok(0));

		let nft = NftsByOwner::<Test>::get(RECIPIENT).expect("depositless NFT is stored by owner");
		assert_eq!(nft.instance, 0);
		assert_eq!(nft.collection, 0);
		assert_eq!(nft.item, 0);
		assert_eq!(nft.minted_at, 1_234);
		assert_eq!(Instances::<Test>::get(0), Some(RECIPIENT));
		assert_eq!(ItemDefs::<Test>::get(0, 0).unwrap().supply, 1);
		assert!(!InstanceDeposits::<Test>::contains_key(0));
		assert_eq!(held(OWNER), held_before);
		System::assert_has_event(
			Event::<Test>::Minted { instance: 0, collection: 0, item: 0, owner: RECIPIENT }.into(),
		);
	});
}

#[test]
fn mint_without_deposit_waives_initial_instance_metadata_deposits() {
	new_test_ext().execute_with(|| {
		setup_item();
		let held_before = held(OWNER);
		let metadata_key = key(b"unique");

		assert_eq!(
			Scarcity::mint_without_deposit(
				0,
				0,
				RECIPIENT,
				vec![(metadata_key.clone(), value(b"free"))],
			),
			Ok(0)
		);

		let entry = InstanceMetadata::<Test>::get(0, &metadata_key).expect("metadata exists");
		assert_eq!(entry.value, value(b"free"));
		assert_eq!(entry.deposit, 0);
		assert_eq!(InstanceMetadataCount::<Test>::get(0), 1);
		assert_eq!(held(OWNER), held_before);

		assert_ok!(Scarcity::set_instance_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			metadata_key.clone(),
			Some(value(b"now-deposited")),
		));
		let charged = InstanceMetadata::<Test>::get(0, &metadata_key).expect("metadata remains");
		assert!(charged.deposit > 0);
		assert_eq!(held(OWNER), held_before + charged.deposit);
		assert_eq!(Scarcity::instance_metadata_of(0, &metadata_key), Some(value(b"now-deposited")),);
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn mint_without_deposit_checks_collection_item_and_destination() {
	new_test_ext().execute_with(|| {
		setup_item();
		define(0);

		assert_noop!(
			Scarcity::mint_without_deposit(99, 0, RECIPIENT, metadata(&[])),
			Error::<Test>::UnknownCollection
		);
		assert_noop!(
			Scarcity::mint_without_deposit(0, 99, RECIPIENT, metadata(&[])),
			Error::<Test>::UnknownItem
		);
		assert_ok!(Scarcity::mint_without_deposit(0, 0, RECIPIENT, metadata(&[])));
		assert_noop!(
			Scarcity::mint_without_deposit(0, 1, RECIPIENT, metadata(&[])),
			Error::<Test>::AddressOccupied
		);
	});
}

#[test]
fn item_defs_are_immutable() {
	new_test_ext().execute_with(|| {
		setup_item();
		let before = ItemDefs::<Test>::get(0, 0).expect("first definition exists");

		define(0);
		assert_eq!(ItemDefs::<Test>::get(0, 0), Some(before));
	});
}

/// A failed `transfer` charges its submitter, unlike the success path.
///
/// The purse-key origin pays nothing when a move lands, which is what lets a holder with no
/// balance move an instance. A failure writes nothing, so the same transaction stays valid and
/// waiving its fee too would buy unlimited block weight for free.
#[test]
fn a_failed_transfer_pays_its_fee() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		mint(0, OTHER);

		let nft = NftsByOwner::<Test>::get(RECIPIENT).expect("the holder has an instance");
		let error = Scarcity::transfer(nft_origin(RECIPIENT, nft), OTHER)
			.expect_err("the destination already holds an instance");
		assert_eq!(error.error, Error::<Test>::AddressOccupied.into());
		assert_eq!(error.post_info.pays_fee, Pays::Yes);
	});
}

/// A failed `burn` charges its submitter, for the reason given on `a_failed_transfer_pays_its_fee`.
#[test]
fn a_failed_burn_pays_its_fee() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);

		let error = Scarcity::burn(RuntimeOrigin::signed(RECIPIENT))
			.expect_err("burning needs the purse-key origin");
		assert_eq!(error.error, sp_runtime::DispatchError::BadOrigin);
		assert_eq!(error.post_info.pays_fee, Pays::Yes);
	});
}

#[test]
fn transfer_moves_ownership_and_updates_reverse_index() {
	new_test_ext().execute_with(|| {
		setup_item();
		MockNow::set(10);
		mint_with_metadata(0, RECIPIENT, &[(b"unique", b"moves")]);
		assert_eq!(frame_system::Account::<Test>::get(RECIPIENT).sufficients, 0);

		MockNow::set(20);
		let (validity, val, origin) = validate_transfer(RECIPIENT, OTHER).unwrap();
		assert_eq!(validity.priority, 10);
		assert_eq!(validity.provides, vec![("Scarcity", (0u64, 0u64)).encode()]);
		let pre = prepare_transfer(val, &origin, OTHER);
		assert!(!NftsByOwner::<Test>::contains_key(RECIPIENT));
		let post_info = Scarcity::transfer(origin, OTHER).unwrap();
		post_dispatch(pre, Ok(()));
		assert_eq!(post_info.pays_fee, Pays::No);
		assert!(!NftsByOwner::<Test>::contains_key(RECIPIENT));
		let moved = NftsByOwner::<Test>::get(OTHER).expect("recipient has NFT");
		assert_eq!(moved.instance, 0);
		assert_eq!(moved.last_moved, 20);
		assert_eq!(moved.state_nonce, 1);
		assert_eq!(Instances::<Test>::get(0), Some(OTHER));
		assert_eq!(frame_system::Account::<Test>::get(RECIPIENT).sufficients, 0);
		assert_eq!(frame_system::Account::<Test>::get(OTHER).sufficients, 0);
		assert_eq!(Scarcity::instance_metadata_of(0, &key(b"unique")), Some(value(b"moves")),);
		System::assert_has_event(
			Event::<Test>::Transferred { instance: 0, collection: 0, from: RECIPIENT, to: OTHER }
				.into(),
		);
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn depositless_instance_transfers_through_extension_pipeline() {
	new_test_ext().execute_with(|| {
		setup_item();
		let held_before = held(OWNER);
		MockNow::set(10);
		assert_ok!(Scarcity::mint_without_deposit(0, 0, RECIPIENT, metadata(&[])));

		MockNow::set(20);
		let (validity, val, origin) = validate_transfer(RECIPIENT, OTHER).unwrap();
		assert_eq!(validity.priority, 10);
		let pre = prepare_transfer(val, &origin, OTHER);
		assert_ok!(Scarcity::transfer(origin, OTHER));
		post_dispatch(pre, Ok(()));

		assert!(!NftsByOwner::<Test>::contains_key(RECIPIENT));
		let moved = NftsByOwner::<Test>::get(OTHER).expect("recipient has depositless NFT");
		assert_eq!(moved.instance, 0);
		assert_eq!(moved.last_moved, 20);
		assert_eq!(moved.state_nonce, 1);
		assert_eq!(Instances::<Test>::get(0), Some(OTHER));
		assert!(!InstanceDeposits::<Test>::contains_key(0));
		assert_eq!(held(OWNER), held_before);
	});
}

#[test]
fn transfer_priority_scales_with_rest_time() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);

		MockNow::set(0);
		let (fresh, _, _) = validate_transfer(RECIPIENT, OTHER).unwrap();
		assert_eq!(fresh.priority, 0);

		MockNow::set(100);
		let (rested, _, _) = validate_transfer(RECIPIENT, OTHER).unwrap();
		assert!(rested.priority > fresh.priority);

		MockNow::set(2_000_000);
		let (capped, _, _) = validate_transfer(RECIPIENT, OTHER).unwrap();
		assert_eq!(capped.priority, 1_000_000);
	});
}

#[test]
fn transfer_requires_owner_signature() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);

		assert_no_nft(
			validate_transfer_as(OTHER, OWNER, current_authorization(RECIPIENT))
				.err()
				.expect("a key without an NFT cannot use another NFT's authorization"),
		);
	});
}

#[test]
fn stale_authorization_cannot_act_on_reused_purse() {
	new_test_ext().execute_with(|| {
		setup_item();
		define(0);
		mint(0, RECIPIENT);
		let stale_authorization = current_authorization(RECIPIENT);

		let (_, val, origin) = validate_burn_as(RECIPIENT, stale_authorization.clone()).unwrap();
		let pre = prepare_burn(val, &origin);
		assert_ok!(Scarcity::burn(origin));
		post_dispatch(pre, Ok(()));

		mint(1, RECIPIENT);
		let replacement =
			NftsByOwner::<Test>::get(RECIPIENT).expect("a different NFT reused the purse");
		assert_eq!(replacement.instance, 1);

		assert_state_mismatch(
			validate_transfer_as(RECIPIENT, OTHER, stale_authorization.clone())
				.err()
				.expect("the transfer authorization names the burned instance"),
		);
		assert_state_mismatch(
			validate_burn_as(RECIPIENT, stale_authorization)
				.err()
				.expect("the burn authorization names the burned instance"),
		);
		assert_eq!(NftsByOwner::<Test>::get(RECIPIENT), Some(replacement));
	});
}

#[test]
fn prepare_rechecks_authorized_state_before_consuming_nft() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		let (_, val, origin) = validate_transfer(RECIPIENT, OTHER).unwrap();

		NftsByOwner::<Test>::mutate(RECIPIENT, |maybe_nft| {
			maybe_nft.as_mut().expect("minted NFT exists").state_nonce = 1;
		});
		let call = transfer_call(OTHER);
		let error = extension_for_val(&val)
			.prepare(val, &origin, &call, &Default::default(), 0)
			.err()
			.expect("changed state must fail preparation");

		assert_state_mismatch(error);
		assert_eq!(
			NftsByOwner::<Test>::get(RECIPIENT).map(|nft| nft.state_nonce),
			Some(1),
			"a failed preparation must not consume the changed state",
		);
	});
}

#[test]
fn same_block_double_use_blocked() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		let authorization = current_authorization(RECIPIENT);

		let (_, val, origin) =
			validate_transfer_as(RECIPIENT, OTHER, authorization.clone()).unwrap();
		let _pre = prepare_transfer(val, &origin, OTHER);
		assert!(!NftsByOwner::<Test>::contains_key(RECIPIENT));
		assert_no_nft(
			validate_transfer_as(RECIPIENT, OTHER, authorization)
				.err()
				.expect("the NFT is held by the prepared transaction"),
		);
	});
}

#[test]
fn failed_dispatch_restores_and_locks() {
	new_test_ext().execute_with(|| {
		setup_item();
		define(0);
		mint(0, OWNER);

		// Race shape: the destination is empty at validation time and becomes occupied before
		// dispatch — the only failure path that still reaches dispatch now that validate
		// pre-checks the destination.
		let (_, val, origin) = validate_transfer(OWNER, 4).unwrap();
		mint(1, 4);
		let pre = prepare_transfer(val, &origin, 4);
		let dispatch = Scarcity::transfer(origin, 4);
		assert_noop!(dispatch, Error::<Test>::AddressOccupied);
		post_dispatch(pre, Err(Error::<Test>::AddressOccupied.into()));
		assert_eq!(NftsByOwner::<Test>::get(OWNER).map(|nft| nft.instance), Some(0));
		assert_eq!(Locked::<Test>::get(OWNER), Some(LockInfo { retries: 1, until: 60 }));
		assert_ok!(Scarcity::do_try_state());
		// While locked, even a fresh empty destination is rejected at the pool.
		assert!(validate_transfer(OWNER, 5).is_err());

		MockNow::set(60);
		let (_, val, origin) = validate_transfer(OWNER, 5).unwrap();
		mint(1, 5);
		let pre = prepare_transfer(val, &origin, 5);
		let dispatch = Scarcity::transfer(origin, 5);
		assert_noop!(dispatch, Error::<Test>::AddressOccupied);
		post_dispatch(pre, Err(Error::<Test>::AddressOccupied.into()));
		assert_eq!(Locked::<Test>::get(OWNER), Some(LockInfo { retries: 2, until: 180 }));
	});
}

#[test]
fn success_clears_lock() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		Locked::<Test>::insert(RECIPIENT, LockInfo { retries: 3, until: 10 });
		MockNow::set(10);

		let (_, val, origin) = validate_transfer(RECIPIENT, OTHER).unwrap();
		let pre = prepare_transfer(val, &origin, OTHER);
		assert_ok!(Scarcity::transfer(origin, OTHER));
		post_dispatch(pre, Ok(()));
		assert!(!Locked::<Test>::contains_key(RECIPIENT));
	});
}

#[test]
fn non_transfer_calls_pass_through() {
	new_test_ext().execute_with(|| {
		let call = RuntimeCall::Scarcity(crate::Call::create_collection {});
		let (validity, val, origin) = scarcity_extension(authorization(0, 0))
			.validate(
				RuntimeOrigin::signed(OWNER),
				&call,
				&Default::default(),
				0,
				(),
				&TxBaseImplication(()),
				TransactionSource::External,
			)
			.unwrap();

		assert_eq!(validity.priority, 0);
		assert!(matches!(val, Val::NotUsing));
		assert!(matches!(origin.as_system_ref(), Some(frame_system::Origin::<Test>::Signed(who)) if *who == OWNER));
	});
}

#[test]
fn pool_rejects_self_transfer_without_lock() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		assert!(validate_transfer(RECIPIENT, RECIPIENT).is_err());
		// Pool rejection is side-effect free: NFT untouched, no failure lock written.
		assert!(NftsByOwner::<Test>::contains_key(RECIPIENT));
		assert_eq!(Locked::<Test>::get(RECIPIENT), None);
	});
}

/// A soulbound transfer is refused at the pool rather than at dispatch.
///
/// Every other dispatch failure can come good, so paying for it with a backoff lock is a fair
/// trade. This one never can: transferability is fixed when the item is defined, so each retry
/// only lengthens a lock that also gates the holder's burn.
#[test]
fn pool_rejects_a_soulbound_transfer_without_lock() {
	new_test_ext().execute_with(|| {
		assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));
		define_as(0, Transferability::Soulbound);
		mint(0, RECIPIENT);

		assert_invalidity(
			validate_transfer(RECIPIENT, OTHER)
				.err()
				.expect("a soulbound transfer is refused"),
			CustomInvalidity::Soulbound,
		);
		assert!(NftsByOwner::<Test>::contains_key(RECIPIENT));
		assert_eq!(Locked::<Test>::get(RECIPIENT), None);

		// What the holder cannot move, they can still destroy.
		assert!(validate_burn(RECIPIENT).is_ok());
	});
}

/// An unreadable item definition is refused too, but under its own name.
///
/// Both are permanent, so both belong at the pool rather than at dispatch. Reporting this one as
/// soulbound would send an operator looking at the token instead of at the state: a chain whose
/// `ItemDefs` rows have not been migrated answers this way for every instance it holds.
#[test]
fn pool_rejects_a_transfer_whose_item_cannot_be_read() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		ItemDefs::<Test>::remove(0, 0);

		assert_invalidity(
			validate_transfer(RECIPIENT, OTHER)
				.err()
				.expect("an unreadable item is refused"),
			CustomInvalidity::UnknownItem,
		);
		assert_eq!(Locked::<Test>::get(RECIPIENT), None);
	});
}

#[test]
fn pool_rejects_occupied_destination_without_lock() {
	new_test_ext().execute_with(|| {
		setup_item();
		define(0);
		mint(0, RECIPIENT);
		mint(1, OTHER);
		assert!(validate_transfer(RECIPIENT, OTHER).is_err());
		assert!(NftsByOwner::<Test>::contains_key(RECIPIENT));
		assert_eq!(Locked::<Test>::get(RECIPIENT), None);
	});
}

#[test]
fn one_nft_per_key_on_transfer() {
	new_test_ext().execute_with(|| {
		setup_item();
		define(0);
		mint(0, RECIPIENT);
		mint(1, OTHER);
		let nft = NftsByOwner::<Test>::take(RECIPIENT).expect("minted NFT exists");

		assert_noop!(
			Scarcity::transfer(nft_origin(RECIPIENT, nft), OTHER),
			Error::<Test>::AddressOccupied
		);
	});
}

#[test]
fn transfer_to_self_rejected() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		let nft = NftsByOwner::<Test>::take(RECIPIENT).expect("minted NFT exists");

		assert_noop!(
			Scarcity::transfer(nft_origin(RECIPIENT, nft), RECIPIENT),
			Error::<Test>::SelfTransfer
		);
	});
}

#[test]
fn burn_releases_instance_deposit_and_preserves_supply_and_item_metadata() {
	new_test_ext().execute_with(|| {
		setup_item();
		let metadata_key = key(b"survives");
		assert_ok!(Scarcity::set_item_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			0,
			metadata_key.clone(),
			Some(value(b"burn")),
		));
		MockNow::set(10);
		mint_with_metadata(0, RECIPIENT, &[(b"unique", b"removed")]);
		let instance_deposit =
			InstanceDeposits::<Test>::get(0).expect("ordinary mint has a deposit");
		let instance_metadata_deposit = InstanceMetadata::<Test>::get(0, key(b"unique"))
			.expect("metadata exists")
			.deposit;
		let held_before = held(OWNER);
		let supply = ItemDefs::<Test>::get(0, 0).unwrap().supply;
		let burned_authorization = current_authorization(RECIPIENT);

		MockNow::set(25);
		let (validity, val, origin) =
			validate_burn_as(RECIPIENT, burned_authorization.clone()).unwrap();
		assert_eq!(validity.priority, 15);
		let pre = prepare_burn(val, &origin);
		assert!(!NftsByOwner::<Test>::contains_key(RECIPIENT));
		let post_info = Scarcity::burn(origin).unwrap();
		post_dispatch(pre, Ok(()));

		assert_eq!(post_info.pays_fee, Pays::No);
		assert_eq!(post_info.actual_weight, Some(<() as crate::weights::WeightInfo>::burn(1)),);
		assert!(!NftsByOwner::<Test>::contains_key(RECIPIENT));
		assert_eq!(frame_system::Account::<Test>::get(RECIPIENT).sufficients, 0);
		assert!(!Instances::<Test>::contains_key(0));
		assert!(!InstanceDeposits::<Test>::contains_key(0));
		assert!(!InstanceMetadataCount::<Test>::contains_key(0));
		assert_eq!(InstanceMetadata::<Test>::iter_prefix(0).count(), 0);
		assert_eq!(Scarcity::instance_metadata_of(0, &key(b"unique")), None);
		let definition = ItemDefs::<Test>::get(0, 0).unwrap();
		assert_eq!(definition.supply, supply);
		assert_eq!(definition.live_supply, 0);
		assert_eq!(
			Scarcity::item_metadata_of(0, 0, &metadata_key),
			Some(value(b"burn")),
			"item-definition metadata must outlive a burned instance",
		);
		assert_eq!(held(OWNER), held_before - instance_deposit - instance_metadata_deposit);
		System::assert_has_event(
			Event::<Test>::Burned { instance: 0, collection: 0, owner: RECIPIENT }.into(),
		);

		assert_no_nft(
			validate_transfer_as(RECIPIENT, OTHER, burned_authorization.clone())
				.err()
				.expect("burned purse has no NFT"),
		);
		assert_no_nft(
			validate_burn_as(RECIPIENT, burned_authorization)
				.err()
				.expect("burned purse has no NFT"),
		);
	});
}

#[test]
fn burn_of_depositless_instance_releases_nothing_and_cleans_indexes() {
	new_test_ext().execute_with(|| {
		setup_item();
		assert_ok!(Scarcity::mint_without_deposit(
			0,
			0,
			RECIPIENT,
			metadata(&[(b"unique", b"free")]),
		));
		assert!(!InstanceDeposits::<Test>::contains_key(0));
		let held_before = held(OWNER);

		let (_, val, origin) = validate_burn(RECIPIENT).unwrap();
		let pre = prepare_burn(val, &origin);
		assert_ok!(Scarcity::burn(origin));
		post_dispatch(pre, Ok(()));

		assert!(!NftsByOwner::<Test>::contains_key(RECIPIENT));
		assert!(!Instances::<Test>::contains_key(0));
		assert!(!InstanceDeposits::<Test>::contains_key(0));
		assert!(!InstanceMetadataCount::<Test>::contains_key(0));
		assert_eq!(InstanceMetadata::<Test>::iter_prefix(0).count(), 0);
		assert_eq!(held(OWNER), held_before);
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn collection_owner_can_force_burn_an_instance() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint_with_metadata(0, RECIPIENT, &[(b"effect", b"healing")]);
		Locked::<Test>::insert(RECIPIENT, LockInfo { retries: 1, until: 60 });
		let instance_deposit =
			InstanceDeposits::<Test>::get(0).expect("ordinary mint has a deposit");
		let metadata_deposit = InstanceMetadata::<Test>::get(0, key(b"effect"))
			.expect("instance metadata exists")
			.deposit;
		let held_before = held(OWNER);

		assert_noop!(
			Scarcity::force_burn(RuntimeOrigin::signed(OTHER), 0),
			Error::<Test>::NoPermission
		);
		assert_noop!(
			Scarcity::force_burn(RuntimeOrigin::signed(OWNER), 99),
			Error::<Test>::UnknownInstance
		);
		let post_info = Scarcity::force_burn(RuntimeOrigin::signed(OWNER), 0).unwrap();
		assert_eq!(post_info.pays_fee, Pays::Yes);
		assert_eq!(
			post_info.actual_weight,
			Some(<() as crate::weights::WeightInfo>::force_burn(1)),
		);

		assert!(!NftsByOwner::<Test>::contains_key(RECIPIENT));
		assert!(!Instances::<Test>::contains_key(0));
		assert!(!InstanceDeposits::<Test>::contains_key(0));
		assert!(!InstanceMetadataCount::<Test>::contains_key(0));
		assert_eq!(InstanceMetadata::<Test>::iter_prefix(0).count(), 0);
		assert!(!Locked::<Test>::contains_key(RECIPIENT));
		assert_eq!(frame_system::Account::<Test>::get(RECIPIENT).sufficients, 0);
		let definition = ItemDefs::<Test>::get(0, 0).expect("definition remains");
		assert_eq!(definition.supply, 1);
		assert_eq!(definition.live_supply, 0);
		assert_eq!(held(OWNER), held_before - instance_deposit - metadata_deposit);
		System::assert_has_event(
			Event::<Test>::Burned { instance: 0, collection: 0, owner: RECIPIENT }.into(),
		);
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn collection_owner_can_force_transfer_an_instance() {
	new_test_ext().execute_with(|| {
		setup_item();
		define(0);
		MockNow::set(10);
		mint_with_metadata(0, RECIPIENT, &[(b"effect", b"healing")]);
		mint(1, OTHER);
		Locked::<Test>::insert(RECIPIENT, LockInfo { retries: 1, until: 60 });
		let deposit = InstanceDeposits::<Test>::get(0).expect("ordinary mint has a deposit");
		let held_before = held(OWNER);
		let target = 4;

		assert_noop!(
			Scarcity::force_transfer(RuntimeOrigin::signed(OTHER), 0, target),
			Error::<Test>::NoPermission
		);
		assert_noop!(
			Scarcity::force_transfer(RuntimeOrigin::signed(OWNER), 99, target),
			Error::<Test>::UnknownInstance
		);
		assert_noop!(
			Scarcity::force_transfer(RuntimeOrigin::signed(OWNER), 0, RECIPIENT),
			Error::<Test>::SelfTransfer
		);
		assert_noop!(
			Scarcity::force_transfer(RuntimeOrigin::signed(OWNER), 0, OTHER),
			Error::<Test>::AddressOccupied
		);

		MockNow::set(25);
		assert_ok!(Scarcity::force_transfer(RuntimeOrigin::signed(OWNER), 0, target));

		assert!(!NftsByOwner::<Test>::contains_key(RECIPIENT));
		let moved = NftsByOwner::<Test>::get(target).expect("target owns the NFT");
		assert_eq!(moved.instance, 0);
		assert_eq!(moved.last_moved, 25);
		assert_eq!(moved.state_nonce, 1);
		assert_eq!(Instances::<Test>::get(0), Some(target));
		assert_eq!(frame_system::Account::<Test>::get(RECIPIENT).sufficients, 0);
		assert_eq!(frame_system::Account::<Test>::get(target).sufficients, 0);
		assert_eq!(InstanceDeposits::<Test>::get(0), Some(deposit));
		assert_eq!(Scarcity::instance_metadata_of(0, &key(b"effect")), Some(value(b"healing")),);
		assert_eq!(held(OWNER), held_before);
		assert!(!Locked::<Test>::contains_key(RECIPIENT));
		System::assert_has_event(
			Event::<Test>::ForceTransferred {
				instance: 0,
				collection: 0,
				from: RECIPIENT,
				to: target,
			}
			.into(),
		);
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn force_transfer_away_and_back_invalidates_old_authorization() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		let stale = current_authorization(RECIPIENT);

		assert_ok!(Scarcity::force_transfer(RuntimeOrigin::signed(OWNER), 0, OTHER));
		assert_ok!(Scarcity::force_transfer(RuntimeOrigin::signed(OWNER), 0, RECIPIENT));
		let returned = NftsByOwner::<Test>::get(RECIPIENT).expect("NFT returned to its purse");
		assert_eq!(returned.instance, 0);
		assert_eq!(returned.state_nonce, 2);

		assert_state_mismatch(
			validate_transfer_as(RECIPIENT, OTHER, stale.clone())
				.err()
				.expect("the transfer authorization names an old ownership state"),
		);
		assert_state_mismatch(
			validate_burn_as(RECIPIENT, stale)
				.err()
				.expect("the burn authorization names an old ownership state"),
		);
		assert!(validate_transfer(RECIPIENT, OTHER).is_ok());
	});
}

#[test]
fn soulbound_instances_reject_both_holder_paths() {
	new_test_ext().execute_with(|| {
		assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));
		define_as(0, Transferability::Soulbound);
		define_as(0, Transferability::Transferable);
		mint(0, RECIPIENT);

		// The fee-less native path.
		let nft = NftsByOwner::<Test>::get(RECIPIENT).expect("minted NFT exists");
		assert_noop!(
			Scarcity::transfer(nft_origin(RECIPIENT, nft), OTHER),
			Error::<Test>::Soulbound
		);
		// The paid path a contract environment reaches.
		assert_noop!(
			Scarcity::do_transfer_by_holder(&RECIPIENT, 0, OTHER),
			Error::<Test>::Soulbound
		);

		assert_eq!(Instances::<Test>::get(0), Some(RECIPIENT));
		assert_eq!(NftsByOwner::<Test>::get(RECIPIENT).expect("still held").state_nonce, 0);

		// The sibling definition in the same collection is unaffected, so the flag is
		// per-definition rather than per-collection.
		mint(1, OTHER);
		assert_ok!(Scarcity::do_transfer_by_holder(&OTHER, 1, 4));
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn soulbound_instances_still_answer_to_the_collection_owner_and_can_be_burned() {
	new_test_ext().execute_with(|| {
		assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));
		define_as(0, Transferability::Soulbound);
		mint(0, RECIPIENT);

		// Soulbound binds the holder, not the issuer: without this the collection owner has
		// no remedy for a misdirected mint.
		assert_ok!(Scarcity::force_transfer(RuntimeOrigin::signed(OWNER), 0, OTHER));
		assert_eq!(Instances::<Test>::get(0), Some(OTHER));

		// Disposing of an instance is not a transfer, so the holder keeps that right.
		let (_, val, origin) = validate_burn(OTHER).expect("holder may burn");
		let pre = prepare_burn(val, &origin);
		assert_ok!(Scarcity::burn(origin));
		post_dispatch(pre, Ok(()));
		assert!(!Instances::<Test>::contains_key(0));
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn transfer_by_holder_moves_instance_and_clears_lock() {
	new_test_ext().execute_with(|| {
		setup_item();
		MockNow::set(10);
		mint_with_metadata(0, RECIPIENT, &[(b"unique", b"moves")]);
		Locked::<Test>::insert(RECIPIENT, LockInfo { retries: 2, until: 120 });

		MockNow::set(20);
		assert_ok!(Scarcity::do_transfer_by_holder(&RECIPIENT, 0, OTHER));

		assert!(!NftsByOwner::<Test>::contains_key(RECIPIENT));
		let moved = NftsByOwner::<Test>::get(OTHER).expect("destination holds the NFT");
		assert_eq!(moved.instance, 0);
		assert_eq!(moved.last_moved, 20);
		assert_eq!(moved.state_nonce, 1);
		assert_eq!(Instances::<Test>::get(0), Some(OTHER));
		// Both other move paths clear the source lock, and `try_state` requires every
		// `Locked` entry to have a matching NFT.
		assert!(!Locked::<Test>::contains_key(RECIPIENT));
		assert_eq!(Scarcity::instance_metadata_of(0, &key(b"unique")), Some(value(b"moves")));
		System::assert_has_event(
			Event::<Test>::Transferred { instance: 0, collection: 0, from: RECIPIENT, to: OTHER }
				.into(),
		);
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn transfer_by_holder_requires_the_named_instance_and_holder() {
	new_test_ext().execute_with(|| {
		setup_item();
		define(0);
		mint(0, RECIPIENT);
		mint(1, OTHER);

		// A purse that does not hold the named instance cannot move it, whether it holds
		// nothing or holds a different one.
		assert_noop!(
			Scarcity::do_transfer_by_holder(&OWNER, 0, OTHER),
			Error::<Test>::UnknownInstance
		);
		assert_noop!(
			Scarcity::do_transfer_by_holder(&OTHER, 0, OWNER),
			Error::<Test>::UnknownInstance
		);

		assert_eq!(Instances::<Test>::get(0), Some(RECIPIENT));
		assert_eq!(Instances::<Test>::get(1), Some(OTHER));
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn transfer_by_holder_rejects_occupied_and_self_destinations() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		mint(0, OTHER);

		assert_noop!(
			Scarcity::do_transfer_by_holder(&RECIPIENT, 0, OTHER),
			Error::<Test>::AddressOccupied
		);
		assert_noop!(
			Scarcity::do_transfer_by_holder(&RECIPIENT, 0, RECIPIENT),
			Error::<Test>::SelfTransfer
		);

		assert_eq!(Instances::<Test>::get(0), Some(RECIPIENT));
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn transfer_by_holder_invalidates_prior_authorization() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		let stale = current_authorization(RECIPIENT);

		assert_ok!(Scarcity::do_transfer_by_holder(&RECIPIENT, 0, OTHER));

		// The paid path bumps the same state nonce the fee-less path binds to, so an
		// authorization signed before the move no longer validates.
		assert_state_mismatch(
			validate_transfer_as(OTHER, RECIPIENT, stale)
				.err()
				.expect("the authorization names the pre-transfer ownership state"),
		);
		assert!(validate_transfer(OTHER, RECIPIENT).is_ok());
	});
}

#[test]
fn transfer_by_holder_nonce_overflow_is_atomic() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		Locked::<Test>::insert(RECIPIENT, LockInfo { retries: 1, until: 60 });
		NftsByOwner::<Test>::mutate(RECIPIENT, |maybe_nft| {
			maybe_nft.as_mut().expect("minted NFT exists").state_nonce = u64::MAX;
		});
		let before = NftsByOwner::<Test>::get(RECIPIENT).expect("minted NFT exists");

		assert_noop!(
			Scarcity::do_transfer_by_holder(&RECIPIENT, 0, OTHER),
			Error::<Test>::StateNonceOverflow
		);

		assert_eq!(NftsByOwner::<Test>::get(RECIPIENT), Some(before));
		assert!(!NftsByOwner::<Test>::contains_key(OTHER));
		assert_eq!(Instances::<Test>::get(0), Some(RECIPIENT));
		assert_eq!(Locked::<Test>::get(RECIPIENT), Some(LockInfo { retries: 1, until: 60 }));
	});
}

#[test]
fn force_transfer_nonce_overflow_is_atomic() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		Locked::<Test>::insert(RECIPIENT, LockInfo { retries: 1, until: 60 });
		NftsByOwner::<Test>::mutate(RECIPIENT, |maybe_nft| {
			maybe_nft.as_mut().expect("minted NFT exists").state_nonce = u64::MAX;
		});
		let before = NftsByOwner::<Test>::get(RECIPIENT).expect("minted NFT exists");

		assert_noop!(
			Scarcity::force_transfer(RuntimeOrigin::signed(OWNER), 0, OTHER),
			Error::<Test>::StateNonceOverflow
		);

		assert_eq!(NftsByOwner::<Test>::get(RECIPIENT), Some(before));
		assert!(!NftsByOwner::<Test>::contains_key(OTHER));
		assert_eq!(Instances::<Test>::get(0), Some(RECIPIENT));
		assert_eq!(Locked::<Test>::get(RECIPIENT), Some(LockInfo { retries: 1, until: 60 }));
	});
}

#[test]
fn holder_transfer_nonce_overflow_restores_the_nft() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		NftsByOwner::<Test>::mutate(RECIPIENT, |maybe_nft| {
			maybe_nft.as_mut().expect("minted NFT exists").state_nonce = u64::MAX;
		});

		let (_, val, origin) = validate_transfer(RECIPIENT, OTHER).unwrap();
		let pre = prepare_transfer(val, &origin, OTHER);
		let dispatch = Scarcity::transfer(origin, OTHER);
		assert_noop!(dispatch, Error::<Test>::StateNonceOverflow);
		post_dispatch(pre, Err(Error::<Test>::StateNonceOverflow.into()));

		assert_eq!(NftsByOwner::<Test>::get(RECIPIENT).map(|nft| nft.state_nonce), Some(u64::MAX),);
		assert!(!NftsByOwner::<Test>::contains_key(OTHER));
		assert_eq!(Instances::<Test>::get(0), Some(RECIPIENT));
	});
}

#[test]
fn delete_item_requires_dependencies_to_be_removed_and_never_reuses_its_id() {
	new_test_ext().execute_with(|| {
		setup_item();
		assert_ok!(Scarcity::set_item_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			0,
			key(b"default"),
			Some(value(b"potion")),
		));
		mint(0, RECIPIENT);

		assert_noop!(
			Scarcity::delete_item(RuntimeOrigin::signed(OTHER), 0, 0),
			Error::<Test>::NoPermission
		);
		assert_noop!(
			Scarcity::delete_item(RuntimeOrigin::signed(OWNER), 0, 0),
			Error::<Test>::ItemInUse
		);
		assert_ok!(Scarcity::force_burn(RuntimeOrigin::signed(OWNER), 0));
		assert_noop!(
			Scarcity::delete_item(RuntimeOrigin::signed(OWNER), 0, 0),
			Error::<Test>::ItemMetadataNotEmpty
		);
		assert_ok!(Scarcity::set_item_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			0,
			key(b"default"),
			None,
		));

		let definition_deposit = ItemDefs::<Test>::get(0, 0).unwrap().deposit;
		let held_before = held(OWNER);
		assert_ok!(Scarcity::delete_item(RuntimeOrigin::signed(OWNER), 0, 0));
		assert!(!ItemDefs::<Test>::contains_key(0, 0));
		let info = Collections::<Test>::get(0).expect("collection remains");
		assert_eq!(info.item_count, 0);
		assert_eq!(info.next_item_index, 1);
		assert_eq!(held(OWNER), held_before - definition_deposit);
		System::assert_has_event(Event::<Test>::ItemDeleted { collection: 0, item: 0 }.into());

		define(0);
		assert!(!ItemDefs::<Test>::contains_key(0, 0));
		assert!(ItemDefs::<Test>::contains_key(0, 1));
		assert_eq!(Collections::<Test>::get(0).unwrap().item_count, 1);
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn delete_collection_requires_dependencies_and_releases_its_deposit() {
	new_test_ext().execute_with(|| {
		setup_item();
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			key(b"name"),
			Some(value(b"potions")),
		));

		assert_noop!(
			Scarcity::delete_collection(RuntimeOrigin::signed(OTHER), 0),
			Error::<Test>::NoPermission
		);
		assert_noop!(
			Scarcity::delete_collection(RuntimeOrigin::signed(OWNER), 0),
			Error::<Test>::CollectionItemsNotEmpty
		);
		assert_ok!(Scarcity::delete_item(RuntimeOrigin::signed(OWNER), 0, 0));
		assert_noop!(
			Scarcity::delete_collection(RuntimeOrigin::signed(OWNER), 0),
			Error::<Test>::CollectionMetadataNotEmpty
		);
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			key(b"name"),
			None,
		));

		let collection_deposit = Collections::<Test>::get(0).unwrap().collection_deposit;
		assert_eq!(held(OWNER), collection_deposit);
		assert_ok!(Scarcity::delete_collection(RuntimeOrigin::signed(OWNER), 0));
		assert!(!Collections::<Test>::contains_key(0));
		assert_eq!(held(OWNER), 0);
		System::assert_has_event(Event::<Test>::CollectionDeleted { collection: 0 }.into());
		// The deletion hook ran for this collection, so cross-pallet state keyed by it is cleared.
		assert_eq!(LastDeletedCollection::get(), Some(0));

		assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));
		assert!(!Collections::<Test>::contains_key(0));
		assert!(Collections::<Test>::contains_key(1));
		assert_eq!(NextCollectionId::<Test>::get(), 2);
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn delete_collection_weight_includes_the_deletion_hook() {
	use crate::weights::WeightInfo;
	use frame_support::dispatch::GetDispatchInfo;

	new_test_ext().execute_with(|| {
		let declared = crate::Call::<Test>::delete_collection { collection: 0 }
			.get_dispatch_info()
			.call_weight;
		assert_eq!(
			declared,
			<() as WeightInfo>::delete_collection()
				.saturating_add(RecordCollectionDeletion::on_delete_weight())
		);
	});
}

#[test]
fn mint_weight_includes_the_purse_occupancy_hook() {
	use crate::{weights::WeightInfo, OnPurseOccupied};
	use frame_support::dispatch::GetDispatchInfo;

	new_test_ext().execute_with(|| {
		let declared = crate::Call::<Test>::mint {
			collection: 0,
			item: 0,
			to: RECIPIENT,
			metadata: alloc::vec![],
		}
		.get_dispatch_info()
		.call_weight;
		assert_eq!(
			declared,
			<() as WeightInfo>::mint(0)
				.saturating_add(RecordPurseOccupancy::on_purse_occupied_weight())
		);
	});
}

/// Every path that gives a key an instance notifies, so a runtime hook cannot be reached by one
/// and missed by another. A holder the hook misses reads as holding nothing, and its address
/// resolves to a different account.
#[test]
fn every_occupying_path_notifies_the_purse_occupancy_hook() {
	new_test_ext().execute_with(|| {
		setup_item();
		assert!(OccupiedPurses::get().is_empty());

		mint(0, RECIPIENT);
		assert_eq!(OccupiedPurses::get(), alloc::vec![RECIPIENT]);

		assert_ok!(<Scarcity as crate::MintWithoutDeposit<u64>>::mint_without_deposit(
			0,
			0,
			OTHER,
			alloc::vec![]
		));
		assert_eq!(OccupiedPurses::get(), alloc::vec![RECIPIENT, OTHER]);

		assert_ok!(Scarcity::force_transfer(RuntimeOrigin::signed(OWNER), 0, 4));
		assert_eq!(OccupiedPurses::get(), alloc::vec![RECIPIENT, OTHER, 4]);

		assert_ok!(Scarcity::do_transfer_by_holder(&4, 0, 5));
		assert_eq!(OccupiedPurses::get(), alloc::vec![RECIPIENT, OTHER, 4, 5]);

		// The fee-less holder path has its own body rather than calling `do_transfer_by_holder`,
		// so reaching it through the extrinsic is what proves the hook is wired there too.
		let nft = NftsByOwner::<Test>::get(5).expect("the instance moved to 5");
		assert_ok!(Scarcity::transfer(nft_origin(5, nft), 6));
		assert_eq!(OccupiedPurses::get(), alloc::vec![RECIPIENT, OTHER, 4, 5, 6]);
	});
}

/// Both moves declare the hook's weight, as the mint paths do.
#[test]
fn transfer_weights_include_the_purse_occupancy_hook() {
	use crate::{weights::WeightInfo, OnPurseOccupied};
	use frame_support::dispatch::GetDispatchInfo;

	let hook = RecordPurseOccupancy::on_purse_occupied_weight();
	assert_eq!(
		crate::Call::<Test>::transfer { to: RECIPIENT }.get_dispatch_info().call_weight,
		<() as WeightInfo>::transfer().saturating_add(hook)
	);
	assert_eq!(
		crate::Call::<Test>::force_transfer { instance: 0, to: RECIPIENT }
			.get_dispatch_info()
			.call_weight,
		<() as WeightInfo>::force_transfer().saturating_add(hook)
	);
}

#[test]
fn burn_uses_rest_time_priority_and_rejects_locked_keys() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);

		MockNow::set(100);
		let (validity, _, _) = validate_burn(RECIPIENT).unwrap();
		assert_eq!(validity.priority, 100);

		Locked::<Test>::insert(RECIPIENT, LockInfo { retries: 1, until: 101 });
		assert!(validate_burn(RECIPIENT).is_err());
		assert!(NftsByOwner::<Test>::contains_key(RECIPIENT));

		MockNow::set(101);
		assert!(validate_burn(RECIPIENT).is_ok());
	});
}

#[test]
fn failed_burn_restores_nft_and_locks_purse_key() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		ItemDefs::<Test>::mutate(0, 0, |maybe_definition| {
			maybe_definition.as_mut().expect("item exists").live_supply = 0;
		});

		let (_, val, origin) = validate_burn(RECIPIENT).unwrap();
		let pre = prepare_burn(val, &origin);
		let dispatch = Scarcity::burn(origin);
		let dispatch_error =
			dispatch.expect_err("the inconsistent live supply must make burn fail").error;
		// The burn's storage transaction restores its reverse index and deposit. The extension
		// still owns the NFT until post-dispatch handles the failed capability call.
		assert!(!NftsByOwner::<Test>::contains_key(RECIPIENT));
		assert_eq!(Instances::<Test>::get(0), Some(RECIPIENT));
		assert!(InstanceDeposits::<Test>::contains_key(0));

		post_dispatch(pre, Err(dispatch_error));
		assert!(NftsByOwner::<Test>::contains_key(RECIPIENT));
		assert_eq!(Locked::<Test>::get(RECIPIENT), Some(LockInfo { retries: 1, until: 60 }));
	});
}

#[test]
fn try_state_rejects_collection_identifier_at_or_above_next() {
	new_test_ext().execute_with(|| {
		setup_item();
		NextCollectionId::<Test>::put(0);

		assert_try_state_error("collection identifier is not below NextCollectionId");
	});
}

#[test]
fn try_state_rejects_non_sequential_item_catalogue() {
	new_test_ext().execute_with(|| {
		setup_item();
		let definition = ItemDefs::<Test>::take(0, 0).expect("item definition exists");
		ItemDefs::<Test>::insert(0, 1, definition);

		assert_try_state_error("item index is not below the collection's next item index");
	});
}

#[test]
fn try_state_rejects_item_counter_mismatch() {
	new_test_ext().execute_with(|| {
		setup_item();
		Collections::<Test>::mutate(0, |maybe_info| {
			maybe_info.as_mut().expect("collection exists").item_count = 2;
		});

		assert_try_state_error("collection item count does not match stored definitions");
	});
}

#[test]
fn try_state_rejects_collection_metadata_counter_mismatch() {
	new_test_ext().execute_with(|| {
		assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));
		Collections::<Test>::mutate(0, |maybe_info| {
			maybe_info.as_mut().expect("collection exists").metadata_count = 1;
		});

		assert_try_state_error("collection metadata count does not match stored entries");
	});
}

#[test]
fn try_state_rejects_item_metadata_counter_mismatch() {
	new_test_ext().execute_with(|| {
		setup_item();
		ItemDefs::<Test>::mutate(0, 0, |maybe_definition| {
			maybe_definition.as_mut().expect("item definition exists").metadata_count = 1;
		});

		assert_try_state_error("item metadata count does not match stored entries");
	});
}

#[test]
fn try_state_rejects_orphaned_item_definition() {
	new_test_ext().execute_with(|| {
		setup_item();
		let definition = ItemDefs::<Test>::get(0, 0).expect("item definition exists");
		ItemDefs::<Test>::insert(99, 0, definition);

		assert_try_state_error("ItemDefs entry has no matching collection");
	});
}

#[test]
fn try_state_rejects_instance_identifier_at_or_above_next() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		NextInstanceId::<Test>::put(0);

		assert_try_state_error("NFT instance is not below NextInstanceId");
	});
}

#[test]
fn try_state_accepts_nft_owner_without_system_account() {
	new_test_ext().execute_with(|| {
		setup_item();
		let nft_only_purse = 99;
		mint(0, nft_only_purse);

		assert!(!frame_system::Account::<Test>::contains_key(nft_only_purse));
		assert_ok!(Scarcity::do_try_state());
	});
}

#[test]
fn try_state_rejects_instance_metadata_count_mismatch() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint_with_metadata(0, RECIPIENT, &[(b"unique", b"value")]);
		InstanceMetadataCount::<Test>::insert(0, 2);

		assert_try_state_error("instance metadata count does not match stored entries");
	});
}

#[test]
fn try_state_rejects_live_instance_without_metadata_count() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		InstanceMetadataCount::<Test>::remove(0);

		assert_try_state_error("live instance has no metadata count entry");
	});
}

#[test]
fn try_state_rejects_orphaned_instance_metadata() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint_with_metadata(0, RECIPIENT, &[(b"unique", b"value")]);
		let entry = InstanceMetadata::<Test>::take(0, key(b"unique")).expect("metadata exists");
		InstanceMetadata::<Test>::insert(99, key(b"unique"), entry);

		assert_try_state_error("InstanceMetadata identifier is not below NextInstanceId");
	});
}

#[test]
fn try_state_rejects_nft_without_item_definition() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		NftsByOwner::<Test>::mutate(RECIPIENT, |maybe_nft| {
			maybe_nft.as_mut().expect("minted NFT exists").item = 99;
		});

		assert_try_state_error("NFT has no matching item definition");
	});
}

#[test]
fn try_state_rejects_live_supply_below_stored_instances() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		ItemDefs::<Test>::mutate(0, 0, |maybe_definition| {
			maybe_definition.as_mut().expect("item definition exists").live_supply = 0;
		});

		assert_try_state_error("item live supply is below its stored instance count");
	});
}

#[test]
fn try_state_rejects_live_supply_above_minted_supply() {
	new_test_ext().execute_with(|| {
		setup_item();
		ItemDefs::<Test>::mutate(0, 0, |maybe_definition| {
			maybe_definition.as_mut().expect("item definition exists").live_supply = 1;
		});

		assert_try_state_error("item live supply exceeds its minted supply");
	});
}

#[test]
fn try_state_rejects_lock_without_nft() {
	new_test_ext().execute_with(|| {
		Locked::<Test>::insert(RECIPIENT, LockInfo { retries: 1, until: 60 });

		assert_try_state_error("Locked entry has no matching NFT");
	});
}

#[test]
fn try_state_rejects_zero_retry_count() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		Locked::<Test>::insert(RECIPIENT, LockInfo { retries: 0, until: 60 });

		assert_try_state_error("Locked retry count must begin at one");
	});
}

#[test]
fn try_state_rejects_incorrect_collection_deposit_aggregate() {
	new_test_ext().execute_with(|| {
		setup_item();
		mint(0, RECIPIENT);
		Collections::<Test>::mutate(0, |maybe_info| {
			maybe_info.as_mut().expect("collection exists").owner_deposit += 1;
		});

		assert_try_state_error("collection owner deposit does not match its stored components");
	});
}

#[test]
fn try_state_accepts_issuer_depositless_transferred_and_burned_states() {
	new_test_ext().execute_with(|| {
		setup_item();
		define(0);
		assert_ok!(Scarcity::set_collection_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			key(b"default"),
			Some(value(b"collection")),
		));
		assert_ok!(Scarcity::set_item_metadata(
			RuntimeOrigin::signed(OWNER),
			0,
			0,
			key(b"default"),
			Some(value(b"item")),
		));
		mint_with_metadata(0, RECIPIENT, &[(b"unique", b"ordinary")]);
		mint(1, OTHER);

		let (_, val, origin) = validate_transfer(RECIPIENT, 4).unwrap();
		let pre = prepare_transfer(val, &origin, 4);
		assert_ok!(Scarcity::transfer(origin, 4));
		post_dispatch(pre, Ok(()));

		assert_ok!(Scarcity::mint_without_deposit(0, 0, 5, metadata(&[(b"unique", b"free")]),));
		let (_, val, origin) = validate_burn(5).unwrap();
		let pre = prepare_burn(val, &origin);
		assert_ok!(Scarcity::burn(origin));
		post_dispatch(pre, Ok(()));
		assert_ok!(Scarcity::mint_without_deposit(0, 0, 6, metadata(&[(b"unique", b"live")]),));

		assert_ok!(Scarcity::do_try_state());
		#[cfg(feature = "try-runtime")]
		assert_ok!(<Scarcity as Hooks<u64>>::try_state(System::block_number()));
	});
}

mod migration {
	use super::*;
	use crate::migration::{v1::MigrateToTransferability, MigrateV0ToV1};
	#[cfg(feature = "try-runtime")]
	use codec::Decode;
	use frame_support::{
		storage::unhashed,
		traits::{GetStorageVersion, OnRuntimeUpgrade, StorageVersion, UncheckedOnRuntimeUpgrade},
	};

	/// Write an item definition in the shape stored before transferability existed.
	///
	/// Encoded as the bare field sequence rather than through the migration's own
	/// `OldItemDefinition`, so a mistake in that struct fails here instead of round-tripping
	/// through itself and passing.
	fn put_old_definition(collection: u32, item: u32, deposit: u64) {
		let old = (3u32, 2u32, 1u32, deposit);
		unhashed::put_raw(&ItemDefs::<Test>::hashed_key_for(collection, item), &old.encode());
	}

	/// Build a collection and item through the pallet, then downgrade only the stored definition.
	///
	/// Leaves every counter, deposit and index exactly as a chain running the old code would have
	/// them, which is what lets the state be checked as a whole after migrating.
	fn downgrade_a_real_definition() {
		assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));
		define(0);
		let definition = ItemDefs::<Test>::get(0, 0).expect("the item was defined");
		let old = (definition.supply, definition.live_supply, definition.metadata_count, {
			let deposit: u64 = definition.deposit;
			deposit
		});
		unhashed::put_raw(&ItemDefs::<Test>::hashed_key_for(0, 0), &old.encode());
		assert_eq!(ItemDefs::<Test>::get(0, 0), None, "the downgrade must be unreadable");
	}

	/// The gate runs the translation on a chain still at version 0, and closes behind it.
	///
	/// Every other translating case drives the inner migration, so without this nothing exercises
	/// the type the runtime actually wires.
	#[test]
	fn the_versioned_migration_translates_and_bumps_the_version() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<Scarcity>();
			put_old_definition(0, 0, 77);

			let weight = <MigrateV0ToV1<Test> as OnRuntimeUpgrade>::on_runtime_upgrade();

			assert_eq!(
				ItemDefs::<Test>::get(0, 0).expect("translated").transferability,
				Transferability::Transferable
			);
			assert_eq!(Scarcity::on_chain_storage_version(), 1, "the gate must close behind it");
			// One read and write for the one definition, one more read for the end of the
			// prefix iteration, and a read and write for the version the gate checks and
			// stamps.
			let db = <Test as frame_system::Config>::DbWeight::get();
			assert_eq!(weight, db.reads_writes(3, 2), "the translation must charge what it did");
		});
	}

	/// Running the wired migration twice is safe, which is the property the gate exists for.
	///
	/// Sequenced the way a chain would reach it: migrate the old rows, define a soulbound item
	/// against the new code, then upgrade again. The second run is the one that would decode that
	/// item as its own four-field prefix and clear the flag if the version did not gate it.
	#[test]
	fn the_versioned_migration_is_idempotent() {
		new_test_ext().execute_with(|| {
			downgrade_a_real_definition();
			StorageVersion::new(0).put::<Scarcity>();

			<MigrateV0ToV1<Test> as OnRuntimeUpgrade>::on_runtime_upgrade();
			assert_eq!(
				ItemDefs::<Test>::get(0, 0).expect("translated").transferability,
				Transferability::Transferable
			);

			define_as(0, Transferability::Soulbound);
			<MigrateV0ToV1<Test> as OnRuntimeUpgrade>::on_runtime_upgrade();

			assert_eq!(
				ItemDefs::<Test>::get(0, 1).expect("the soulbound item exists").transferability,
				Transferability::Soulbound,
				"the second run must not decode the migrated value as its own prefix"
			);
			assert_ok!(Scarcity::do_try_state());
		});
	}

	/// The translation leaves the pallet's own invariants satisfied, and unblocks what the
	/// undecodable definition blocked.
	///
	/// Deletion is the case with the worst tail: a definition that cannot be read cannot be
	/// deleted, so `item_count` never reaches zero, so the collection cannot be deleted and its
	/// deposit stays held.
	#[test]
	fn a_translated_definition_is_usable_again() {
		new_test_ext().execute_with(|| {
			downgrade_a_real_definition();
			assert_noop!(
				Scarcity::delete_item(RuntimeOrigin::signed(OWNER), 0, 0),
				Error::<Test>::UnknownItem
			);

			MigrateToTransferability::<Test>::on_runtime_upgrade();

			assert_ok!(Scarcity::do_try_state());
			assert_ok!(Scarcity::delete_item(RuntimeOrigin::signed(OWNER), 0, 0));
			assert_ok!(Scarcity::delete_collection(RuntimeOrigin::signed(OWNER), 0));
			assert_ok!(Scarcity::do_try_state());
		});
	}

	/// The try-runtime checks pass over the state the migration is meant for.
	///
	/// `pre_upgrade` has to count keys rather than entries, because the values do not decode
	/// until the migration has run. Counting entries would report an empty map and make
	/// `post_upgrade` fail an upgrade that had in fact succeeded.
	#[cfg(feature = "try-runtime")]
	#[test]
	fn the_try_runtime_checks_span_the_translation() {
		new_test_ext().execute_with(|| {
			for (collection, item) in [(0, 0), (0, 1), (1, 0)] {
				put_old_definition(collection, item, 5);
			}
			assert_eq!(ItemDefs::<Test>::iter().count(), 0, "nothing decodes before the migration");

			let state = MigrateToTransferability::<Test>::pre_upgrade().expect("counts the keys");
			assert_eq!(u32::decode(&mut &state[..]).unwrap(), 3, "keys are counted, not entries");

			MigrateToTransferability::<Test>::on_runtime_upgrade();

			assert_ok!(MigrateToTransferability::<Test>::post_upgrade(state));
		});
	}

	/// `post_upgrade` fails when a definition does not survive, rather than reporting success.
	#[cfg(feature = "try-runtime")]
	#[test]
	fn the_post_upgrade_check_catches_a_lost_definition() {
		new_test_ext().execute_with(|| {
			put_old_definition(0, 0, 5);
			let state = MigrateToTransferability::<Test>::pre_upgrade().expect("counts the keys");

			// A value that decodes as neither shape is dropped by `translate_values`, which is
			// the loss the count is there to notice.
			unhashed::put_raw(&ItemDefs::<Test>::hashed_key_for(0, 0), &[0xffu8]);
			MigrateToTransferability::<Test>::on_runtime_upgrade();

			assert!(MigrateToTransferability::<Test>::post_upgrade(state).is_err());
		});
	}

	/// The old encoding is unreadable under the current type, and the migration recovers it.
	///
	/// The first assertion is the whole reason this migration exists: a trailing field turns
	/// every pre-existing definition into `None`, which every caller reports as `UnknownItem`.
	#[test]
	fn translates_definitions_written_before_transferability() {
		new_test_ext().execute_with(|| {
			put_old_definition(0, 0, 77);
			assert_eq!(ItemDefs::<Test>::get(0, 0), None, "the old encoding must not decode");

			MigrateToTransferability::<Test>::on_runtime_upgrade();

			let definition = ItemDefs::<Test>::get(0, 0).expect("the migration restored the item");
			assert_eq!(definition.supply, 3);
			assert_eq!(definition.live_supply, 2);
			assert_eq!(definition.metadata_count, 1);
			// Recomputing from the widened `MaxEncodedLen` would desync the collection aggregate.
			assert_eq!(definition.deposit, 77);
			assert_eq!(definition.transferability, Transferability::Transferable);
		});
	}

	/// Every definition is translated, not just the first the iterator reaches.
	#[test]
	fn translates_every_definition() {
		new_test_ext().execute_with(|| {
			for (collection, item) in [(0, 0), (0, 1), (1, 0)] {
				put_old_definition(collection, item, 5);
			}

			MigrateToTransferability::<Test>::on_runtime_upgrade();

			assert_eq!(ItemDefs::<Test>::iter().count(), 3);
		});
	}

	/// An empty map is the case a chain that never stored a definition takes.
	#[test]
	fn an_empty_map_migrates_to_nothing() {
		new_test_ext().execute_with(|| {
			MigrateToTransferability::<Test>::on_runtime_upgrade();

			assert_eq!(ItemDefs::<Test>::iter().count(), 0);
		});
	}

	/// The version gate is what makes the migration single-shot.
	///
	/// Without it a second run would decode the migrated value as its own four-field prefix and
	/// write `Transferable` back over a soulbound flag, so this pins the reason the inner
	/// migration is not exported for direct use.
	#[test]
	fn a_second_unguarded_run_would_clear_the_flag() {
		new_test_ext().execute_with(|| {
			assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));
			define_as(0, Transferability::Soulbound);

			MigrateToTransferability::<Test>::on_runtime_upgrade();

			assert_eq!(
				ItemDefs::<Test>::get(0, 0).expect("item exists").transferability,
				Transferability::Transferable,
				"an unguarded rerun clears the flag, which is why `MigrateV0ToV1` gates on the \
				 storage version"
			);
		});
	}

	/// A chain built from this code is already at version 1, so the gate holds shut.
	///
	/// This is the assertion the safety of the whole migration rests on. Genesis stamps the
	/// in-code storage version, so a fresh chain never runs a translation whose input shape it
	/// never wrote, and the soulbound-clearing rerun above stays unreachable.
	#[test]
	fn a_fresh_chain_skips_the_migration() {
		new_test_ext().execute_with(|| {
			assert_eq!(
				Scarcity::on_chain_storage_version(),
				Scarcity::in_code_storage_version(),
				"genesis must stamp the in-code version"
			);

			assert_ok!(Scarcity::create_collection(RuntimeOrigin::signed(OWNER)));
			define_as(0, Transferability::Soulbound);

			let weight = <MigrateV0ToV1<Test> as OnRuntimeUpgrade>::on_runtime_upgrade();

			assert_eq!(
				ItemDefs::<Test>::get(0, 0).expect("item exists").transferability,
				Transferability::Soulbound,
				"the gate must leave a soulbound item alone"
			);
			assert_eq!(weight, <Test as frame_system::Config>::DbWeight::get().reads(1));
		});
	}
}
