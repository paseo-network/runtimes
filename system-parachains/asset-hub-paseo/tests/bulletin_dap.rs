// Copyright (C) Parity Technologies and the various Paseo contributors.
// SPDX-License-Identifier: Apache-2.0

//! Exercise the inbound message produced by Bulletin's accumulation forwarder.

use asset_hub_paseo_runtime::{
	xcm_config::{CheckingAccount, DotLocation, XcmConfig},
	Balances, Dap, Runtime, RuntimeGenesisConfig, System,
};
use frame_support::{
	assert_ok,
	traits::fungible::{Inspect, Mutate},
};
use parachains_common::AccountId;
use sp_runtime::{traits::AccountIdConversion, BuildStorage};
use xcm::latest::prelude::*;
use xcm_executor::XcmExecutor;

#[test]
fn bulletin_accumulation_teleport_reaches_dap_staging() {
	let mut ext =
		sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
	ext.execute_with(|| {
		System::set_block_number(1);
		parachain_info::ParachainId::<Runtime>::put(cumulus_primitives_core::ParaId::new(1000));
		let amount = 10_000_000_000u128;
		let checking = CheckingAccount::get();
		let staging = Dap::staging_account();
		let ed = <Balances as Inspect<AccountId>>::minimum_balance();
		assert_ok!(Balances::mint_into(&checking, amount + ed));
		assert_ok!(Balances::mint_into(&staging, ed));
		let accumulation: AccountId =
			frame_support::PalletId(*b"acf/dott").into_account_truncating();
		let message = Xcm(vec![
			ReceiveTeleportedAsset((DotLocation::get(), amount).into()),
			AliasOrigin(Location::new(
				1,
				[Parachain(1010), AccountId32 { network: None, id: accumulation.into() }],
			)),
			UnpaidExecution { weight_limit: Unlimited, check_origin: None },
			DepositAsset {
				assets: Wild(AllCounted(1)),
				beneficiary: Location::new(
					0,
					[AccountId32 { network: None, id: staging.clone().into() }],
				),
			},
		]);
		let outcome = XcmExecutor::<XcmConfig>::prepare_and_execute(
			Location::new(1, [Parachain(1010)]),
			message,
			&mut [0; 32],
			Weight::MAX,
			Weight::zero(),
		);
		assert!(matches!(outcome, Outcome::Complete { .. }), "{outcome:?}");
		assert_eq!(Balances::balance(&staging), ed + amount);
		assert_eq!(Balances::balance(&checking), ed);
	});
}
