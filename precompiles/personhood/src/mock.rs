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

pub use super::*;

use core::cell::RefCell;
use frame_support::{derive_impl, traits::Currency};
use indiv_support::traits::{Alias, Context, Identifier};
use pallet_revive::precompiles::AddressMapper;
use sp_runtime::{AccountId32, BuildStorage, Weight};

type Block = frame_system::mocking::MockBlock<Test>;

pub const ALICE_ALIAS: Alias = [0xAA; 32];
pub const BOB_ALIAS: Alias = [0xBB; 32];
pub const CHARLIE_ALIAS: Alias = [0xCC; 32];
pub const DAVE_ALIAS: Alias = [0xDD; 32];
pub const EVE_ALIAS: Alias = [0xEE; 32];

frame_support::construct_runtime!(
	pub enum Test
	{
		System: frame_system,
		Balances: pallet_balances,
		Revive: pallet_revive,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Block = Block;
	type AccountId = AccountId32;
	type Lookup = sp_runtime::traits::IdentityLookup<Self::AccountId>;
	type AccountData = pallet_balances::AccountData<u64>;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
	type AccountStore = System;
	type MaxFreezes = frame_support::traits::VariantCountOf<RuntimeFreezeReason>;
}

#[derive_impl(pallet_revive::config_preludes::TestDefaultConfig)]
impl pallet_revive::Config for Test {
	type AddressMapper = pallet_revive::AccountId32Mapper<Self>;
	type Balance = u64;
	type Currency = Balances;
	type Precompiles = (PersonhoodCheck<Self>,);
	type UploadOrigin = frame_system::EnsureSigned<AccountId32>;
	type InstantiateOrigin = frame_system::EnsureSigned<AccountId32>;
}

impl Config for Test {
	type Proof = ();
	type PersonhoodResolver = MockPersonhoodLookup;
}

thread_local! {
	static MOCK_PERSONHOOD: RefCell<
		Vec<(AccountId32, Context, Identifier, Alias)>
	> = const { RefCell::new(Vec::new()) };

	static MOCK_PROOF_RESULTS: RefCell<
		Vec<(Alias, Context, Identifier)>
	> = const { RefCell::new(Vec::new()) };
}

pub struct MockPersonhoodLookup;

/// Synthetic ordering tokens, not real weights.
///
/// The early-`None` path must be cheaper than the `Some` / late-`None` path so the
/// `gas_refund` test asserts the precompile forwards the resolver's `actual_weight`.
const MOCK_WORST_CASE_WEIGHT: Weight = Weight::from_parts(2, 0);
const MOCK_EARLY_NONE_REFUND: Weight = Weight::from_parts(1, 0);

impl PersonhoodLookup<AccountId32, ()> for MockPersonhoodLookup {
	fn personhood_info_weight() -> Weight {
		MOCK_WORST_CASE_WEIGHT
	}

	fn personhood_info(
		account: &AccountId32,
		context: &Context,
	) -> (Option<(Identifier, Alias)>, Weight) {
		let result = MOCK_PERSONHOOD.with(|entries| {
			entries.borrow().iter().find_map(|(acc, ctx, id, alias)| {
				if acc == account && ctx == context {
					Some((*id, *alias))
				} else {
					None
				}
			})
		});
		let actual = if result.is_some() {
			MOCK_WORST_CASE_WEIGHT
		} else {
			MOCK_WORST_CASE_WEIGHT.saturating_sub(MOCK_EARLY_NONE_REFUND)
		};
		(result, actual)
	}

	fn personhood_info_by_proof_weight() -> Weight {
		MOCK_WORST_CASE_WEIGHT
	}

	fn personhood_info_by_proof(
		request: indiv_support::traits::PersonhoodProofRequest<'_, ()>,
	) -> (bool, Weight) {
		let matched = MOCK_PROOF_RESULTS.with(|entries| {
			entries.borrow().iter().any(|(stored_alias, stored_context, id)| {
				*stored_alias == request.alias &&
					*stored_context == request.context &&
					*id == request.identifier
			})
		});
		let actual = if matched {
			MOCK_WORST_CASE_WEIGHT
		} else {
			MOCK_WORST_CASE_WEIGHT.saturating_sub(MOCK_EARLY_NONE_REFUND)
		};
		(matched, actual)
	}
}

pub fn set_proof_result(alias: Alias, context: Context, collection: Identifier) {
	MOCK_PROOF_RESULTS.with(|entries| {
		let mut entries = entries.borrow_mut();
		entries.retain(|(a, c, _)| *a != alias || *c != context);
		entries.push((alias, context, collection));
	});
}

pub fn set_personhood(
	account: &AccountId32,
	context: &Context,
	collection: Identifier,
	alias: Alias,
) {
	MOCK_PERSONHOOD.with(|entries| {
		let mut entries = entries.borrow_mut();
		entries.retain(|(acc, ctx, _, _)| acc != account || ctx != context);
		entries.push((account.clone(), *context, collection, alias));
	});
}

/// The precompile's fixed address: `0x000000000000000000000000000000000a010000`.
pub const PRECOMPILE_ADDR: pallet_revive::precompiles::H160 =
	pallet_revive::precompiles::H160(hex_literal::hex!("000000000000000000000000000000000a010000"));

/// Fund `account` and register its H160↔AccountId32 mapping with `pallet-revive`.
///
/// Shared by the unit tests (`tests.rs`) and the integration tests (`integration_tests.rs`),
/// which run against different mock runtimes — hence the generic `T` bound.
pub fn map_account<T>(account: &AccountId32)
where
	T: pallet_revive::Config
		+ pallet_balances::Config
		+ frame_system::Config<AccountId = AccountId32>,
	pallet_balances::Pallet<T>: Currency<AccountId32, Balance = u64>,
{
	pallet_balances::Pallet::<T>::make_free_balance_be(account, u64::MAX / 2);
	let _ = <T as pallet_revive::Config>::AddressMapper::map(account);
}

pub fn id_to_account(id: u64) -> AccountId32 {
	let mut bytes = [0u8; 32];
	bytes[..8].copy_from_slice(&id.to_le_bytes());
	AccountId32::new(bytes)
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	let t = RuntimeGenesisConfig {
		system: Default::default(),
		balances: Default::default(),
		revive: Default::default(),
	}
	.build_storage()
	.unwrap();

	let mut ext: sp_io::TestExternalities = t.into();
	ext.execute_with(|| {
		System::set_block_number(1);
	});
	ext
}
