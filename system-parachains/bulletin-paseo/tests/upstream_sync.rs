// Copyright (C) Parity Technologies and the various Paseo contributors.
// SPDX-License-Identifier: GPL-3.0-only

//! Regression coverage for the Bulletin upstream sync.

use bulletin_paseo_runtime::{
	xcm_config::{AssetHubLocation, TokenRelayLocation, XcmConfig},
	AccumulateForward, Balances, ExistentialDeposit, ForwardPeriod, MinForwardAmount,
	ParachainSystem, PolkadotXcm, Runtime, RuntimeCall, RuntimeEvent, RuntimeGenesisConfig,
	RuntimeOrigin, SessionKeys, System, TransactionStorage, XcmpQueue,
};
use codec::{Decode, Encode};
use cumulus_primitives_core::{ParaId, XcmpMessageFormat, XcmpMessageSource};
use frame_support::{
	assert_ok,
	traits::{
		fungible::{Inspect, Mutate},
		tokens::Preservation,
		Contains, Hooks,
	},
};
use pallet_bulletin_data_renewal::Call as RenewalCall;
use pallet_bulletin_transaction_storage::TransactionRef;
use parachains_common::AccountId;
use paseo_runtime_constants::system_parachain::ASSET_HUB_ID;
use sp_runtime::BuildStorage;
use xcm::latest::prelude::*;
use xcm_executor::{
	traits::{FeeManager, FeeReason},
	XcmExecutor,
};

fn externalities() -> sp_io::TestExternalities {
	sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
}

fn prepare_channel() {
	ParachainSystem::open_outbound_hrmp_channel_for_benchmarks_or_tests(ParaId::new(ASSET_HUB_ID));
	assert_ok!(PolkadotXcm::force_xcm_version(
		RuntimeOrigin::root(),
		Box::new(AssetHubLocation::get()),
		XCM_VERSION,
	));
}

#[test]
fn session_key_deposit_works() {
	system_parachains_test_utils::session_key_deposit_works::<Runtime>(|owner| {
		let generated = SessionKeys::generate(&owner.encode(), None);
		(generated.keys, generated.proof.encode())
	});
}

#[test]
fn xcm_rejects_committing_renewals_directly_and_inside_wrappers() {
	externalities().execute_with(|| {
		System::set_block_number(1);
		assert_ok!(TransactionStorage::store(RuntimeOrigin::root(), vec![42; 100]));
		<TransactionStorage as Hooks<_>>::on_finalize(1);
		System::set_block_number(2);
		let entry = TransactionRef::Position { block: 1, index: 0 };
		let hash = TransactionStorage::transactions_at(1).unwrap()[0].content_hash;
		let calls = vec![
			RuntimeCall::DataRenewal(RenewalCall::renew { entry: entry.clone() }),
			RuntimeCall::DataRenewal(RenewalCall::force_renew { entry }),
			RuntimeCall::DataRenewal(RenewalCall::enable_auto_renew { content_hash: hash }),
		];
		for call in calls {
			let wrapped = RuntimeCall::Utility(pallet_utility::Call::batch_all {
				calls: vec![RuntimeCall::Utility(pallet_utility::Call::batch {
					calls: vec![call.clone()],
				})],
			});
			for call in [call, wrapped] {
				let message = Xcm(vec![
					UnpaidExecution { weight_limit: Unlimited, check_origin: None },
					Transact {
						origin_kind: OriginKind::Superuser,
						fallback_max_weight: None,
						call: call.encode().into(),
					},
				]);
				let outcome = XcmExecutor::<XcmConfig>::prepare_and_execute(
					AssetHubLocation::get(),
					message,
					&mut [0; 32],
					Weight::MAX,
					Weight::MAX,
				);
				assert!(
					matches!(
						outcome,
						Outcome::Incomplete {
							error: InstructionError { error: XcmError::NoPermission, .. },
							..
						}
					),
					"renewal must be rejected by the call filter: {outcome:?}"
				);
			}
		}
		assert_eq!(pallet_bulletin_data_renewal::PermanentStorageUsed::<Runtime>::get(), 0);
		assert!(pallet_bulletin_data_renewal::Renewals::<Runtime>::iter().next().is_none());
		assert_eq!(TransactionStorage::transactions_at(1).unwrap().len(), 1);
	});
}

#[test]
fn xcm_filter_preserves_cleanup_and_inherent_calls() {
	type Filter = <XcmConfig as xcm_executor::Config>::SafeCallFilter;
	assert!(Filter::contains(&RuntimeCall::DataRenewal(RenewalCall::disable_auto_renew {
		content_hash: [7; 32],
	})));
	assert!(Filter::contains(&RuntimeCall::DataRenewal(RenewalCall::process_pending_renewals {})));
}

#[test]
fn dust_accumulates_without_reducing_issuance() {
	externalities().execute_with(|| {
		System::set_block_number(1);
		let ed = ExistentialDeposit::get();
		let account = AccumulateForward::accumulation_account();
		let alice = AccountId::new([1; 32]);
		let bob = AccountId::new([2; 32]);
		for who in [&account, &alice, &bob] {
			assert_ok!(Balances::mint_into(who, ed));
		}
		let issuance = Balances::total_issuance();
		let dust = ed / 2;
		assert_ok!(Balances::transfer(&alice, &bob, ed - dust, Preservation::Expendable));
		assert_eq!(Balances::balance(&alice), 0);
		assert_eq!(Balances::balance(&account), ed + dust);
		assert_eq!(Balances::total_issuance(), issuance);
	});
}

#[test]
fn signed_xcm_delivery_fees_reach_the_accumulation_account() {
	externalities().execute_with(|| {
		System::set_block_number(1);
		prepare_channel();
		let ed = ExistentialDeposit::get();
		let account = AccumulateForward::accumulation_account();
		let alice = AccountId::new([1; 32]);
		assert_ok!(Balances::mint_into(&account, ed));
		assert_ok!(Balances::mint_into(&alice, ed * 1000));
		let issuance = Balances::total_issuance();
		let before = Balances::balance(&alice);
		let amount = ed * 100;
		let beneficiary: Location = AccountId32 { network: None, id: alice.clone().into() }.into();
		assert_ok!(PolkadotXcm::limited_teleport_assets(
			RuntimeOrigin::signed(alice.clone()),
			Box::new(AssetHubLocation::get().into()),
			Box::new(beneficiary.into()),
			Box::new(Assets::from((TokenRelayLocation::get(), amount)).into()),
			0,
			Unlimited,
		));
		let fee = before - Balances::balance(&alice) - amount;
		assert!(fee > 0);
		assert_eq!(Balances::balance(&account), ed + fee);
		assert_eq!(Balances::total_issuance(), issuance - amount);
		type Manager = <XcmConfig as xcm_executor::Config>::FeeManager;
		assert!(Manager::is_waived(
			Some(&AccountId32 { network: None, id: account.into() }.into()),
			FeeReason::InitiateTransfer
		));
	});
}

#[test]
fn accumulated_funds_are_queued_for_the_dap_and_keep_the_ed() {
	externalities().execute_with(|| {
		System::set_block_number(1);
		prepare_channel();
		let ed = ExistentialDeposit::get();
		let amount = MinForwardAmount::get();
		let account = AccumulateForward::accumulation_account();
		assert_ok!(Balances::mint_into(&account, ed + amount));
		AccumulateForward::on_idle(1, Weight::MAX);
		assert_eq!(Balances::balance(&account), ed);
		System::assert_has_event(RuntimeEvent::AccumulateForward(
			pallet_accumulate_and_forward::Event::ForwardSucceeded { amount },
		));
		let messages = XcmpQueue::take_outbound_messages(1, &[]);
		assert_eq!(messages.len(), 1);
		assert_eq!(messages[0].0, ParaId::new(ASSET_HUB_ID));
		let mut bytes = messages[0].1.as_slice();
		assert_eq!(
			XcmpMessageFormat::decode(&mut bytes).unwrap(),
			XcmpMessageFormat::ConcatenatedVersionedXcm
		);
		let xcm: Xcm<()> = xcm::VersionedXcm::decode(&mut bytes).unwrap().try_into().unwrap();
		let beneficiary = system_parachains_constants::paseo::locations::DapStagingLocation::get()
			.into_location();
		assert!(xcm
			.0
			.iter()
			.any(|i| matches!(i, DepositAsset { beneficiary: b, .. } if b == &beneficiary)));
		assert!(xcm.0.iter().any(|i| matches!(i, ReceiveTeleportedAsset(assets)
			if assets == &Assets::from((TokenRelayLocation::get(), amount)))));
	});
}

#[test]
fn failed_forward_preserves_funds_and_retries_after_the_period() {
	externalities().execute_with(|| {
		System::set_block_number(1);
		let account = AccumulateForward::accumulation_account();
		let amount = MinForwardAmount::get();
		let funded = ExistentialDeposit::get() + amount;
		assert_ok!(Balances::mint_into(&account, funded));
		AccumulateForward::on_idle(1, Weight::MAX);
		assert_eq!(Balances::balance(&account), funded);
		System::assert_has_event(RuntimeEvent::AccumulateForward(
			pallet_accumulate_and_forward::Event::ForwardFailed { amount },
		));
		prepare_channel();
		System::set_block_number(ForwardPeriod::get());
		AccumulateForward::on_idle(ForwardPeriod::get(), Weight::MAX);
		assert_eq!(Balances::balance(&account), funded);
		System::set_block_number(1 + ForwardPeriod::get());
		AccumulateForward::on_idle(1 + ForwardPeriod::get(), Weight::MAX);
		assert_eq!(Balances::balance(&account), ExistentialDeposit::get());
	});
}

#[cfg(feature = "try-runtime")]
#[test]
fn upgrade_requires_accumulation_account_funding() {
	use bulletin_paseo_runtime::migrations::EnsureAccumulationAccountFunded;
	use frame_support::traits::OnRuntimeUpgrade;
	externalities().execute_with(|| {
		assert!(EnsureAccumulationAccountFunded::post_upgrade(vec![]).is_err());
		assert_ok!(Balances::mint_into(
			&AccumulateForward::accumulation_account(),
			ExistentialDeposit::get()
		));
		assert_ok!(EnsureAccumulationAccountFunded::post_upgrade(vec![]));
	});
}

#[cfg(feature = "runtime-benchmarks")]
#[test]
fn affected_benchmarks_are_registered() {
	use bulletin_paseo_runtime::Block;
	externalities().execute_with(|| {
		let (pallets, _) = <Runtime as frame_benchmarking::runtime_decl_for_benchmark::Benchmark<
			Block,
		>>::benchmark_metadata(false);
		for (pallet, benchmark) in [
			("pallet_session", "set_keys"),
			("pallet_session", "purge_keys"),
			("pallet_accumulate_and_forward", "send_native"),
		] {
			assert!(
				pallets.iter().any(|p| p.pallet == pallet.as_bytes() &&
					p.benchmarks.iter().any(|b| b.name == benchmark.as_bytes())),
				"{pallet}::{benchmark} must be exposed by the runtime benchmark API"
			);
		}
	});
}
