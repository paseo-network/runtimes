// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-only

//! Compatibility with individuality-community v0.3.0 and the current Bulletin state.

use bulletin_paseo_runtime::{
	xcm_config::XcmConfig, DataRenewal, Executive, Runtime, RuntimeCall, RuntimeGenesisConfig,
	System, TransactionStorage, XcmpQueue,
};
use codec::Encode;
use frame_support::{assert_ok, storage::storage_prefix, traits::StorageVersion};
use pallet_bulletin_data_renewal::{PermanentStorageUsed, RenewalData, Renewals};
use pallet_bulletin_transaction_storage::{AuthorizationScope, Authorizations};
use parachains_common::AccountId;
use paseo_runtime_constants::system_parachain::{ASSET_HUB_ID, PEOPLE_ID};
use sp_runtime::BuildStorage;
use xcm::latest::prelude::*;

fn externalities() -> sp_io::TestExternalities {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
}

// Wire layout from individuality-community v0.3.0:
// runtimes/next-people-paseo/src/people.rs, BulletinPallets and TransactionStorageCalls.
// Encode independently of RuntimeCall so a Bulletin pallet/call index or argument change
// cannot silently change both the sender fixture and receiver together.
fn people_authorize(who: &AccountId) -> Vec<u8> {
	(40u8, 3u8, who, 7u32, 4096u64).encode()
}

fn people_refresh(who: &AccountId) -> Vec<u8> {
	(40u8, 7u8, who).encode()
}

fn execute_people_message(para_id: u32, call: Vec<u8>) -> Outcome {
	let message: Xcm<RuntimeCall> = Xcm(vec![
		UnpaidExecution { weight_limit: Unlimited, check_origin: None },
		Transact { origin_kind: OriginKind::Xcm, fallback_max_weight: None, call: call.into() },
		// Transact may finish XCM execution with a dispatch error in its status register.
		ExpectTransactStatus(MaybeErrorCode::Success),
	]);
	xcm_executor::XcmExecutor::<XcmConfig>::prepare_and_execute(
		Location::new(1, [Parachain(para_id)]),
		message,
		&mut [0u8; 32],
		Weight::MAX,
		Weight::zero(),
	)
}

#[test]
fn individuality_v0_3_0_can_allocate_and_refresh_without_resetting_usage() {
	externalities().execute_with(|| {
		System::set_block_number(1);
		let who = AccountId::new([0x11; 32]);
		let scope = AuthorizationScope::Account(who.clone());
		assert_ok!(execute_people_message(PEOPLE_ID, people_authorize(&who)).ensure_complete());
		let granted = Authorizations::<Runtime>::get(&scope).unwrap();
		assert_eq!(granted.extent.transactions_allowance, 7);
		assert_eq!(granted.extent.bytes_allowance, 4096);

		// Seed an already-used allowance, including permanent bytes from DataRenewal.
		Authorizations::<Runtime>::mutate(&scope, |authorization| {
			let extent = &mut authorization.as_mut().unwrap().extent;
			extent.transactions = 2;
			extent.bytes = 512;
			extent.extra.bytes_permanent = 1024;
		});
		let used = TransactionStorage::account_authorization_extent(who.clone());
		System::set_block_number(11);
		assert_ok!(execute_people_message(PEOPLE_ID, people_refresh(&who)).ensure_complete());
		let refreshed = Authorizations::<Runtime>::get(&scope).unwrap();
		assert_eq!(refreshed.expiration, granted.expiration + 10);
		assert_eq!(refreshed.extent, used);
	});
}

#[test]
fn individuality_messages_from_an_unlisted_sibling_cannot_change_authorizations() {
	externalities().execute_with(|| {
		System::set_block_number(1);
		let who = AccountId::new([0x11; 32]);
		let scope = AuthorizationScope::Account(who.clone());
		assert!(execute_people_message(2000, people_authorize(&who)).ensure_complete().is_err());
		assert!(!Authorizations::<Runtime>::contains_key(&scope));

		assert_ok!(execute_people_message(PEOPLE_ID, people_authorize(&who)).ensure_complete());
		let before = Authorizations::<Runtime>::get(&scope).unwrap().encode();
		System::set_block_number(11);
		assert!(execute_people_message(2000, people_refresh(&who)).ensure_complete().is_err());
		assert_eq!(Authorizations::<Runtime>::get(&scope).unwrap().encode(), before);
	});
}

#[test]
fn runtime_upgrade_preserves_current_storage() {
	externalities().execute_with(|| {
		System::set_block_number(1);
		let who = AccountId::new([0x11; 32]);
		let scope = AuthorizationScope::Account(who.clone());
		assert_ok!(execute_people_message(PEOPLE_ID, people_authorize(&who)).ensure_complete());
		Authorizations::<Runtime>::mutate(&scope, |authorization| {
			let extent = &mut authorization.as_mut().unwrap().extent;
			extent.transactions = 2;
			extent.bytes = 512;
			extent.extra.bytes_permanent = 1024;
		});
		let authorization = Authorizations::<Runtime>::get(&scope).unwrap().encode();

		// Para 1010 has already completed the renewal split and XCMP v7 migration.
		StorageVersion::new(5).put::<TransactionStorage>();
		StorageVersion::new(1).put::<DataRenewal>();
		StorageVersion::new(7).put::<XcmpQueue>();
		let renewal = RenewalData { account: who, recurring: true, paid: false };
		Renewals::<Runtime>::insert([0x22; 32], &renewal);
		PermanentStorageUsed::<Runtime>::put(1024);

		// V7: recipient, state, signals_exist, first_index, last_index, flags, queued_bytes.
		let channels = vec![
			(ASSET_HUB_ID, 0u8, false, 0u16, 0u16, 0u32, 0u32),
			(PEOPLE_ID, 1u8, true, 0u16, 0u16, 3u32, 0u32),
		]
		.encode();
		let channel_key = storage_prefix(b"XcmpQueue", b"OutboundXcmpStatus");
		sp_io::storage::set(&channel_key, &channels);

		for _ in 0..2 {
			Executive::execute_on_runtime_upgrade();
			assert_eq!(StorageVersion::get::<TransactionStorage>(), StorageVersion::new(5));
			assert_eq!(StorageVersion::get::<DataRenewal>(), StorageVersion::new(1));
			assert_eq!(StorageVersion::get::<XcmpQueue>(), StorageVersion::new(7));
			assert_eq!(Renewals::<Runtime>::get([0x22; 32]), Some(renewal.clone()));
			assert_eq!(PermanentStorageUsed::<Runtime>::get(), 1024);
			assert_eq!(Authorizations::<Runtime>::get(&scope).unwrap().encode(), authorization);
			assert_eq!(sp_io::storage::get(&channel_key).unwrap().to_vec(), channels);
		}
	});
}

#[cfg(feature = "runtime-benchmarks")]
#[test]
fn xcm_benchmark_batch_dispatches_every_nested_call() {
	use bulletin_paseo_runtime::{RuntimeEvent, RuntimeOrigin};
	use sp_runtime::traits::Dispatchable;

	externalities().execute_with(|| {
		System::set_block_number(1);
		let calls = (0..3u8)
			.map(|i| frame_system::Call::remark_with_event { remark: vec![i] }.into())
			.collect();
		let batch = <Runtime as pallet_xcm::benchmarking::Config>::batch_call(calls)
			.expect("Transact benchmarks must weigh all nested calls");
		assert_ok!(batch.dispatch(RuntimeOrigin::signed(AccountId::new([0x11; 32]))));
		let remarked: Vec<_> = System::events()
			.into_iter()
			.filter_map(|record| match record.event {
				RuntimeEvent::System(frame_system::Event::Remarked { hash, .. }) => Some(hash),
				_ => None,
			})
			.collect();
		let expected: Vec<_> = (0..3u8)
			.map(|i| sp_core::H256::from(sp_io::hashing::blake2_256(&[i])))
			.collect();
		assert_eq!(remarked, expected);
	});
}

#[cfg(feature = "runtime-benchmarks")]
#[test]
fn xcm_alias_benchmark_exercises_authorized_aliasers() {
	use frame_support::traits::ContainsPair;

	externalities().execute_with(|| {
		System::set_block_number(1);
		let (origin, target) =
			<Runtime as pallet_xcm_benchmarks::generic::Config>::alias_origin().unwrap();
		assert!(pallet_xcm::AuthorizedAliasers::<Runtime>::contains(&origin, &target));
		assert!(<XcmConfig as xcm_executor::Config>::Aliasers::contains(&origin, &target));
	});
}
