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

//! Scarcity purse transactions through the production `TxExtension` pipeline.

use frame_support::{
	assert_ok,
	dispatch::GetDispatchInfo,
	traits::{
		fungibles::{Create, Inspect, Mutate},
		Get,
	},
};
use next_asset_hub_paseo_runtime::{
	Assets, Balances, PgasAdmin, PgasAssetId, PgasMinBalance, Runtime, RuntimeCall, RuntimeOrigin,
	TxExtension,
};
use parachains_common::AccountId;
use sp_runtime::{
	generic::Era,
	traits::{DispatchTransaction, Zero},
	transaction_validity::{InvalidTransaction, TransactionValidityError},
	BuildStorage,
};

fn scarcity_tx_extension(nonce: u32, state_nonce: u64) -> TxExtension {
	purse_tx_extension(
		nonce,
		// Names the NFT `scarcity_purse_test_state` seeds, at the same state nonce.
		Some(indiv_pallet_scarcity::extension::AsScarcityInfo::AsNft { instance: 0, state_nonce }),
	)
}

/// A purse-signed transaction with `as_scarcity` in the origin slot.
///
/// `ChargePGAS` keeps its PGAS path, as it does in every decoded extrinsic.
fn purse_tx_extension(
	nonce: u32,
	as_scarcity: Option<indiv_pallet_scarcity::extension::AsScarcityInfo>,
) -> TxExtension {
	TxExtension::from((
		(
			(),
			indiv_pallet_scarcity::extension::AsScarcity::<Runtime>::new(as_scarcity),
			frame_system::AuthorizeCall::<Runtime>::new(),
			indiv_pallet_pgas::AsPgas::<Runtime>::new(None),
			indiv_pallet_dotns_gateway::AsDotnsGateway::<Runtime>::new(None),
		),
		indiv_pallet_origin_restriction::RestrictOrigin::<Runtime>::new(true),
		frame_system::CheckNonZeroSender::<Runtime>::new(),
		frame_system::CheckSpecVersion::<Runtime>::new(),
		frame_system::CheckTxVersion::<Runtime>::new(),
		frame_system::CheckGenesis::<Runtime>::new(),
		frame_system::CheckEra::<Runtime>::from(Era::Immortal),
		frame_system::CheckNonce::<Runtime>::from(nonce),
		frame_system::CheckWeight::<Runtime>::new(),
		pallet_pgas_allowance::ChargePGAS::<
			Runtime,
			pallet_asset_conversion_tx_payment::ChargeAssetTxPayment<Runtime>,
		>::from(pallet_asset_conversion_tx_payment::ChargeAssetTxPayment::<Runtime>::from(
			0, None,
		)),
		frame_metadata_hash_extension::CheckMetadataHash::<Runtime>::new(false),
		pallet_revive::evm::tx_extension::SetOrigin::<Runtime>::default(),
	))
}

fn scarcity_purse_test_state(state_nonce: u64) -> (sp_io::TestExternalities, AccountId) {
	let storage = frame_system::GenesisConfig::<Runtime>::default().build_storage().unwrap();
	let mut ext = sp_io::TestExternalities::from(storage);
	let from = AccountId::from([1u8; 32]);
	ext.execute_with(|| {
		frame_system::Pallet::<Runtime>::set_block_number(1);
		pallet_timestamp::Pallet::<Runtime>::set_timestamp(1_000);
		indiv_pallet_scarcity::NftsByOwner::<Runtime>::insert(
			&from,
			indiv_pallet_scarcity::Nft {
				instance: 0,
				collection: 0,
				item: 0,
				minted_at: 0,
				last_moved: 0,
				state_nonce,
				moves: 0,
			},
		);
		indiv_pallet_scarcity::Instances::<Runtime>::insert(0, &from);
		// The holder transfer resolves its item's transferability, so the definition the
		// instance names has to exist as it would on a chain that minted it.
		indiv_pallet_scarcity::ItemDefs::<Runtime>::insert(
			0,
			0,
			indiv_pallet_scarcity::ItemDefinition {
				supply: 1,
				live_supply: 1,
				metadata_count: 0,
				deposit: 0,
				transferability: indiv_pallet_scarcity::Transferability::Transferable,
			},
		);
	});
	(ext, from)
}

/// An NFT-only purse key — no balance, no System account — can send a feeless transfer
/// through the full extension pipeline. This pins the security-critical ordering of
/// `AsScarcity` within `TxExtension`.
#[test]
fn nft_only_purse_without_system_account_can_transfer() {
	let (mut ext, from) = scarcity_purse_test_state(0);
	ext.execute_with(|| {
		let to = AccountId::from([2u8; 32]);
		assert!(Balances::free_balance(&from).is_zero());
		assert_eq!(frame_system::Pallet::<Runtime>::account_nonce(&from), 0);

		let call = RuntimeCall::Scarcity(indiv_pallet_scarcity::Call::<Runtime>::transfer {
			to: to.clone(),
		});
		let info = call.get_dispatch_info();
		let result = scarcity_tx_extension(0, 0).dispatch_transaction(
			RuntimeOrigin::signed(from.clone()),
			call,
			&info,
			0,
			0,
		);
		assert!(matches!(result, Ok(Ok(_))), "transaction failed: {result:?}");

		assert!(Balances::free_balance(&from).is_zero());
		assert!(!indiv_pallet_scarcity::NftsByOwner::<Runtime>::contains_key(&from));
		assert_eq!(
			indiv_pallet_scarcity::NftsByOwner::<Runtime>::get(&to)
				.map(|nft| (nft.state_nonce, nft.moves)),
			Some((1, 1)),
			"the move lands and spends one of the instance's feeless moves",
		);
	});
}

/// An instance that has spent its budget buys no more block space: the move is refused at
/// validation, before it reaches a block (paritytech/individuality#1270).
///
/// This is what bounds the feeless block space one mint buys. Without it a holder of enough
/// instances fills whole blocks for free.
#[test]
fn a_spent_move_budget_stops_a_feeless_transfer() {
	let (mut ext, from) = scarcity_purse_test_state(0);
	ext.execute_with(|| {
		let spent = <Runtime as indiv_pallet_scarcity::Config>::MaximumMoves::get();
		indiv_pallet_scarcity::NftsByOwner::<Runtime>::mutate(&from, |maybe_nft| {
			maybe_nft.as_mut().expect("the purse holds the seeded NFT").moves = spent;
		});

		let to = AccountId::from([2u8; 32]);
		let call = RuntimeCall::Scarcity(indiv_pallet_scarcity::Call::<Runtime>::transfer {
			to: to.clone(),
		});
		let info = call.get_dispatch_info();
		let result = scarcity_tx_extension(0, 0).dispatch_transaction(
			RuntimeOrigin::signed(from.clone()),
			call,
			&info,
			0,
			0,
		);

		assert_eq!(
			result.unwrap_err(),
			TransactionValidityError::Invalid(InvalidTransaction::Custom(
				indiv_pallet_scarcity::extension::CustomInvalidity::MovesExhausted as u8
			)),
		);
		// Refused at the pool, so the NFT stays put and earns no backoff lock.
		assert!(indiv_pallet_scarcity::NftsByOwner::<Runtime>::contains_key(&from));
		assert!(!indiv_pallet_scarcity::Locked::<Runtime>::contains_key(&from));
	});
}

/// A purse that holds only PGAS pays for the move that refills a spent budget
/// (paritytech/individuality#1270). Without this path an instance is stuck once its feeless
/// moves are gone.
#[test]
fn a_pgas_funded_purse_pays_to_refill_the_move_budget() {
	let (mut ext, from) = scarcity_purse_test_state(0);
	ext.execute_with(|| {
		let spent = <Runtime as indiv_pallet_scarcity::Config>::MaximumMoves::get();
		indiv_pallet_scarcity::NftsByOwner::<Runtime>::mutate(&from, |maybe_nft| {
			maybe_nft.as_mut().expect("the purse holds the seeded NFT").moves = spent;
		});
		// A claim mints PGAS into the purse. It is sufficient, so the purse needs no DOT.
		assert_ok!(<Assets as Create<_>>::create(
			PgasAssetId::get(),
			PgasAdmin::get(),
			true,
			PgasMinBalance::get()
		));
		assert_ok!(<Assets as Mutate<_>>::mint_into(PgasAssetId::get(), &from, 1u128 << 60));
		let pgas_before = <Assets as Inspect<_>>::balance(PgasAssetId::get(), &from);

		let to = AccountId::from([2u8; 32]);
		let call =
			RuntimeCall::Scarcity(indiv_pallet_scarcity::Call::<Runtime>::transfer_by_holder {
				instance: 0,
				to: to.clone(),
			});
		let info = call.get_dispatch_info();
		let result = purse_tx_extension(0, None).dispatch_transaction(
			RuntimeOrigin::signed(from.clone()),
			call,
			&info,
			0,
			0,
		);
		assert!(matches!(result, Ok(Ok(_))), "transaction failed: {result:?}");

		assert!(Balances::free_balance(&from).is_zero());
		assert!(
			<Assets as Inspect<_>>::balance(PgasAssetId::get(), &from) < pgas_before,
			"the purse pays the fee in PGAS",
		);
		assert_eq!(
			indiv_pallet_scarcity::NftsByOwner::<Runtime>::get(&to)
				.map(|nft| (nft.state_nonce, nft.moves)),
			Some((1, 0)),
			"the paid move lands and refills the budget",
		);
	});
}

/// A purse with a spent budget and no funds cannot pay for the move that refills it. The NFT
/// stays put until someone funds the purse.
#[test]
fn an_unfunded_purse_cannot_refill_the_move_budget() {
	let (mut ext, from) = scarcity_purse_test_state(0);
	ext.execute_with(|| {
		let spent = <Runtime as indiv_pallet_scarcity::Config>::MaximumMoves::get();
		indiv_pallet_scarcity::NftsByOwner::<Runtime>::mutate(&from, |maybe_nft| {
			maybe_nft.as_mut().expect("the purse holds the seeded NFT").moves = spent;
		});
		// The PGAS asset exists, so `ChargePGAS` tries its PGAS path before it falls back.
		assert_ok!(<Assets as Create<_>>::create(
			PgasAssetId::get(),
			PgasAdmin::get(),
			true,
			PgasMinBalance::get()
		));

		let to = AccountId::from([2u8; 32]);
		let call =
			RuntimeCall::Scarcity(indiv_pallet_scarcity::Call::<Runtime>::transfer_by_holder {
				instance: 0,
				to: to.clone(),
			});
		let info = call.get_dispatch_info();
		let result = purse_tx_extension(0, None).dispatch_transaction(
			RuntimeOrigin::signed(from.clone()),
			call,
			&info,
			0,
			0,
		);

		assert_eq!(
			result.unwrap_err(),
			TransactionValidityError::Invalid(InvalidTransaction::Payment)
		);
		let nft = indiv_pallet_scarcity::NftsByOwner::<Runtime>::get(&from)
			.expect("the purse keeps the NFT");
		assert_eq!((nft.state_nonce, nft.moves), (0, spent));
		assert_eq!(indiv_pallet_scarcity::Instances::<Runtime>::get(0), Some(from));
		assert!(!indiv_pallet_scarcity::NftsByOwner::<Runtime>::contains_key(&to));
	});
}

/// A failed purse dispatch restores the NFT behind the backoff lock and charges no fee —
/// Coinage's retry model.
#[test]
fn failed_scarcity_transfer_is_feeless_and_retryable_after_lock() {
	// A state nonce at u64::MAX makes the dispatch (not validation) fail on overflow.
	let (mut ext, from) = scarcity_purse_test_state(u64::MAX);
	ext.execute_with(|| {
		let to = AccountId::from([2u8; 32]);
		let call = RuntimeCall::Scarcity(indiv_pallet_scarcity::Call::<Runtime>::transfer {
			to: to.clone(),
		});
		let info = call.get_dispatch_info();
		let result = scarcity_tx_extension(0, u64::MAX).dispatch_transaction(
			RuntimeOrigin::signed(from.clone()),
			call,
			&info,
			0,
			0,
		);
		assert!(matches!(result, Ok(Err(_))), "expected failed dispatch: {result:?}");

		assert!(Balances::free_balance(&from).is_zero());
		// The bumped state nonce saturates at the maximum this purse starts from.
		assert_eq!(
			indiv_pallet_scarcity::NftsByOwner::<Runtime>::get(&from).map(|nft| nft.state_nonce),
			Some(u64::MAX),
		);
		assert!(!indiv_pallet_scarcity::NftsByOwner::<Runtime>::contains_key(&to));
		assert!(indiv_pallet_scarcity::Locked::<Runtime>::contains_key(&from));
	});
}
