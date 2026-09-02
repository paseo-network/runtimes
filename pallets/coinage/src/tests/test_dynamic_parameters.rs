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

//! The pallet reads these parameters from the runtime on every use, so a runtime may change them
//! without a migration. Each test changes one parameter while state created under the old value
//! exists and checks that both a raise and a lowering apply to the next call.

use super::{
	test_free_unload_token_lifecycle::{
		build_unload_free_token_ext, test_free_unload_counter_validity, FreeTokenType,
	},
	test_transfer::build_transfer_ext,
};
use crate::{mock::*, pallet::CustomInvalidity, *};
use sp_runtime::bounded_vec;

#[test]
fn maximum_age_applies_to_next_transfer() {
	new_test_ext().execute_with(|| {
		System::set_block_number(1);
		let signer = 1;
		let dest = 2;
		let age = MAXIMUM_AGE - 1;
		// A coin whose age is valid under the default maximum age.
		CoinsByOwner::<Test>::insert(signer, Coin { instance_id: TEST_INSTANCE_ID, value: 1, age });

		// Lowering the maximum to the coin's age rejects the transfer.
		MaximumAge::set(&age);
		assert_invalid(build_transfer_ext(signer, dest, true), CustomInvalidity::CoinTooOld);

		// Raising it back above the coin's age accepts the same transfer.
		MaximumAge::set(&MAXIMUM_AGE);
		assert_eq!(Executive::apply_extrinsic(build_transfer_ext(signer, dest, true)), Ok(Ok(())));

		// The transfer aged the coin to `MAXIMUM_AGE`. Lowering the maximum well below that age
		// makes the coin too old again.
		MaximumAge::set(&(MAXIMUM_AGE / 2));
		assert_invalid(build_transfer_ext(dest, 3, true), CustomInvalidity::CoinTooOld);
	});
}

#[test]
fn people_allowance_counter_bound_follows_parameter() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let value = 0;
		let target_time = 10000;
		advance_until_time(target_time);
		let period_duration = get_u32::<<Test as Config>::UnloadTokenTimePeriodPeopleLitePeople>();
		let period = target_time / period_duration;

		// Default: allowance 10, fee 2, limit 5.
		assert_eq!(Coinage::free_unload_token_limit_for_people(), 5);
		assert!(test_free_unload_counter_validity(
			FreeTokenType::People,
			value,
			period,
			4,
			0,
			[10u8; 32],
			10000
		));
		assert!(!test_free_unload_counter_validity(
			FreeTokenType::People,
			value,
			period,
			5,
			1,
			[11u8; 32],
			10000
		));

		// Raising the allowance raises the counter bound for the next call.
		UnloadTokenAllowancePerTimePeriodForPeople::set(&12);
		assert_eq!(Coinage::free_unload_token_limit_for_people(), 6);
		assert!(test_free_unload_counter_validity(
			FreeTokenType::People,
			value,
			period,
			5,
			2,
			[12u8; 32],
			10000
		));

		// Lowering it lowers the bound for the next call.
		UnloadTokenAllowancePerTimePeriodForPeople::set(&8);
		assert_eq!(Coinage::free_unload_token_limit_for_people(), 4);
		assert!(!test_free_unload_counter_validity(
			FreeTokenType::People,
			value,
			period,
			4,
			3,
			[13u8; 32],
			10000
		));
		assert!(test_free_unload_counter_validity(
			FreeTokenType::People,
			value,
			period,
			3,
			4,
			[14u8; 32],
			10000
		));
	});
}

#[test]
fn lite_people_allowance_counter_bound_follows_parameter() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let value = 0;
		let target_time = 10000;
		advance_until_time(target_time);
		let period_duration = get_u32::<<Test as Config>::UnloadTokenTimePeriodPeopleLitePeople>();
		let period = target_time / period_duration;

		// Default: allowance 4, fee 2, limit 2.
		assert_eq!(Coinage::free_unload_token_limit_for_lite_people(), 2);
		assert!(test_free_unload_counter_validity(
			FreeTokenType::LitePeople,
			value,
			period,
			1,
			0,
			[20u8; 32],
			20000
		));
		assert!(!test_free_unload_counter_validity(
			FreeTokenType::LitePeople,
			value,
			period,
			2,
			1,
			[21u8; 32],
			20000
		));

		// Raising the allowance raises the counter bound for the next call.
		UnloadTokenAllowancePerTimePeriodForLitePeople::set(&8);
		assert_eq!(Coinage::free_unload_token_limit_for_lite_people(), 4);
		assert!(test_free_unload_counter_validity(
			FreeTokenType::LitePeople,
			value,
			period,
			3,
			2,
			[22u8; 32],
			20000
		));

		// Lowering it lowers the bound for the next call.
		UnloadTokenAllowancePerTimePeriodForLitePeople::set(&2);
		assert_eq!(Coinage::free_unload_token_limit_for_lite_people(), 1);
		assert!(!test_free_unload_counter_validity(
			FreeTokenType::LitePeople,
			value,
			period,
			1,
			3,
			[23u8; 32],
			20000
		));
		assert!(test_free_unload_counter_validity(
			FreeTokenType::LitePeople,
			value,
			period,
			0,
			4,
			[24u8; 32],
			20000
		));
	});
}

#[test]
fn max_free_unload_tokens_cap_follows_parameter() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let value = 0;
		let target_time = 10000;
		advance_until_time(target_time);
		let period_duration = get_u32::<<Test as Config>::UnloadTokenTimePeriodPeopleLitePeople>();
		let period = target_time / period_duration;

		// With fee 1 the people allowance permits 10 tokens, so the default cap of 8 binds.
		MockPaidUnloadTokenFeeOverride::set(&Some(1));
		assert_eq!(Coinage::free_unload_token_limit_for_people(), 8);
		assert!(test_free_unload_counter_validity(
			FreeTokenType::People,
			value,
			period,
			7,
			0,
			[30u8; 32],
			30000
		));
		assert!(!test_free_unload_counter_validity(
			FreeTokenType::People,
			value,
			period,
			8,
			1,
			[31u8; 32],
			30000
		));

		// Raising the cap raises the counter bound up to the allowance.
		MaxFreeUnloadTokensPerTimePeriod::set(&10);
		assert_eq!(Coinage::free_unload_token_limit_for_people(), 10);
		assert!(test_free_unload_counter_validity(
			FreeTokenType::People,
			value,
			period,
			9,
			2,
			[32u8; 32],
			30000
		));
		assert!(!test_free_unload_counter_validity(
			FreeTokenType::People,
			value,
			period,
			10,
			3,
			[33u8; 32],
			30000
		));

		// Lowering the cap lowers the counter bound for the next call.
		MaxFreeUnloadTokensPerTimePeriod::set(&3);
		assert_eq!(Coinage::free_unload_token_limit_for_people(), 3);
		assert!(!test_free_unload_counter_validity(
			FreeTokenType::People,
			value,
			period,
			3,
			4,
			[34u8; 32],
			30000
		));
		assert!(test_free_unload_counter_validity(
			FreeTokenType::People,
			value,
			period,
			2,
			5,
			[35u8; 32],
			30000
		));
	});
}

#[test]
fn lowering_people_allowance_keeps_consumed_tokens_clearable() {
	new_test_ext().execute_with(|| {
		setup_asset();
		let value = 0;
		let target_time = 10000;
		advance_until_time(target_time);
		let period_duration = get_u32::<<Test as Config>::UnloadTokenTimePeriodPeopleLitePeople>();
		let period = target_time / period_duration;

		// Consume the token with the highest counter the default allowance permits.
		let (secrets, index, revision) = setup_recycler(value, 1, 0);
		let call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![CryptoOf::<Test>::alias_in_context(
				&secrets[0],
				UNLOADING_RECYCLER_CONTEXT.as_ref(),
			)
			.unwrap()],
			value,
			index,
			revision,
			to: 1,
		});
		let alias = [1u8; 32];
		let counter = Coinage::free_unload_token_limit_for_people() - 1;
		let ext = build_unload_free_token_ext(
			call,
			period,
			counter,
			&secrets,
			value,
			index,
			alias,
			FreeTokenType::People,
		);
		Executive::apply_extrinsic(ext).unwrap().unwrap();
		assert!(ConsumedFreeUnloadTokens::<Test>::contains_key(period, alias));

		// The consumed counter is now above the bound.
		UnloadTokenAllowancePerTimePeriodForPeople::set(&2);
		assert_eq!(Coinage::free_unload_token_limit_for_people(), 1);

		// Cleanup after the grace window still removes the consumed entry.
		let expiry_time = (period + 1) * period_duration + FREE_UNLOAD_TOKEN_GRACE_WINDOW;
		advance_until_time(expiry_time - 2);
		advance_block(); // triggers the offchain worker
		advance_block(); // includes the created transaction
		assert!(!ConsumedFreeUnloadTokens::<Test>::contains_key(period, alias));
		System::assert_has_event(crate::Event::<Test>::ConsumedFreeTokensCleaned { period }.into());
	});
}
