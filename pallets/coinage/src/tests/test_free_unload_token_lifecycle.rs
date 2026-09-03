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

use crate::{
	extension::{AsCoinage, AsCoinageInfo},
	mock::*,
	*,
};
use codec::Encode;
use frame_support::BoundedVec;
use frame_system::AuthorizeCall;
use sp_runtime::{
	bounded_vec,
	testing::UintAuthorityId,
	transaction_validity::{InvalidTransaction, TransactionSource, TransactionValidityError},
};
use verifiable::GenerateVerifiable;

/// Type of free unload token (people or lite people).
#[derive(Clone, Copy)]
pub(super) enum FreeTokenType {
	People,
	LitePeople,
}

/// Builds the unload extrinsic for people or lite-people
///
/// The counter is local per person per period, ranging from 0..limit
/// with limit = Coinage::free_unload_token_limit_for_people().
///
/// The counter resets each period.
///
/// Each free unload token is identified by the tuple `(period, counter)`
/// and cannot be reused.
pub(super) fn build_unload_free_token_ext(
	call: RuntimeCall,
	period: u32,
	counter: u32,
	recycler_secrets: &[Secret],
	value: Denomination,
	index: u32,
	alias: [u8; 32],
	token_type: FreeTokenType,
) -> Extrinsic {
	let inherited_implication = ((0u8, &call), (), ());
	let proven_msg = sp_io::hashing::blake2_256(&inherited_implication.encode());

	let context = crate::pallet::free_unload_token_context(period, counter);

	// Alias proofs must be created BEFORE the people/lite proof because the people/lite
	// proof signs intent_msg which depends on alias_proofs.
	let mut alias_proofs_vec = Vec::new();
	let ring_members = Coinage::get_recycler_members(TEST_INSTANCE_ID, value, index);

	for secret in recycler_secrets {
		let member = CryptoOf::<Test>::member_from_secret(secret);
		let commitment =
			CryptoOf::<Test>::open(recycler_ring_size(), &member, ring_members.clone().into_iter())
				.unwrap();
		let (proof, _) = CryptoOf::<Test>::create(
			commitment,
			secret,
			UNLOADING_RECYCLER_CONTEXT.as_ref(),
			&proven_msg,
		)
		.unwrap();
		alias_proofs_vec.push(proof);
	}

	let alias_proofs = BoundedVec::try_from(alias_proofs_vec).expect("Too many proofs");

	// The people/lite proof signs intent_msg = blake2_256(alias_proofs ++ inherited_implication).
	let intent_msg = sp_io::hashing::blake2_256(
		&[alias_proofs.encode(), inherited_implication.encode()].concat(),
	);

	let info = match token_type {
		FreeTokenType::People => {
			let proof =
				MembershipProof { context: context.to_vec(), msg: intent_msg.to_vec(), alias };
			AsCoinageInfo::AsUnloadTokenPeople { proof, period, counter, alias_proofs }
		},
		FreeTokenType::LitePeople => {
			let proof =
				MembershipProof { context: context.to_vec(), msg: intent_msg.to_vec(), alias };
			AsCoinageInfo::AsUnloadTokenLitePeople { proof, period, counter, alias_proofs }
		},
	};

	let extension = (AuthorizeCall::<Test>::new(), AsCoinage::<Test>::new(Some(info)));
	Extrinsic::new_signed(call, 0, UintAuthorityId(0), extension)
}

/// Helper to validate whether a free unload token counter is accepted for a token type.
pub(super) fn test_free_unload_counter_validity(
	token_type: FreeTokenType,
	value: Denomination,
	current_period: u32,
	counter: u32,
	seed_offset: u8,
	alias: [u8; 32],
	dest_base: u64,
) -> bool {
	let (secrets, index, revision) = setup_recycler(value, 1, seed_offset);
	let dest = dest_base + seed_offset as u64;
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
		to: dest,
	});
	let ext = build_unload_free_token_ext(
		call,
		current_period,
		counter,
		&secrets,
		value,
		index,
		alias,
		token_type,
	);

	Executive::validate_transaction(TransactionSource::External, ext, Default::default()).is_ok()
}

#[test]
fn free_unload_token_lifecycle() {
	new_test_ext().execute_with(|| {
		// 1. Setup: Create asset, fund recycler
		setup_asset();

		let value = 0;
		let (secrets, index, revision) = setup_recycler(value, 1, 0);
		let dest = 1u64;

		// Set a specific start time to make period calculations deterministic
		let target_time = 10000;
		advance_until_time(target_time as u32);

		let period_duration = get_u32::<<Test as Config>::UnloadTokenTimePeriodPeopleLitePeople>();
		let current_period = (target_time as u32) / period_duration;
		let counter = 0;

		// 2. Consume Free Token: Unload recycler using People token
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
			to: dest,
		});

		let ext = build_unload_free_token_ext(
			call,
			current_period,
			counter,
			&secrets,
			value,
			index,
			[1u8; 32],
			FreeTokenType::People,
		);

		Executive::apply_extrinsic(ext).unwrap().unwrap();

		// Verify token is consumed in storage
		let user_alias = [1u8; 32]; // Matches alias used above
		assert!(ConsumedFreeUnloadTokens::<Test>::contains_key(current_period, user_alias));

		// 3. Advance time just before the end of the grace period
		let grace_period: u32 = FREE_UNLOAD_TOKEN_GRACE_WINDOW;
		let expiry_time = (current_period + 1) * period_duration + grace_period;
		advance_until_time(expiry_time - 2);

		// Verify clean fails before expiration (one block before)
		let clean_call = crate::Call::clean_consumed_free_token { period: current_period };
		let clean_ext_early = build_authorized_ext(clean_call.clone());
		assert_eq!(
			Executive::validate_transaction(
				TransactionSource::Local,
				clean_ext_early,
				Default::default(),
			),
			Err(TransactionValidityError::Invalid(InvalidTransaction::Future))
		);

		// 4. Advance past expiration; the offchain worker is triggered and creates transactions
		//    that
		// are included in the next block.
		advance_block(); // triggers offchain worker
		advance_block(); // include the created transaction.

		// 5. Verify cleanup happened
		assert!(!ConsumedFreeUnloadTokens::<Test>::contains_key(current_period, user_alias));
		System::assert_has_event(
			crate::Event::<Test>::ConsumedFreeTokensCleaned { period: current_period }.into(),
		);
	});
}

#[test]
fn free_unload_token_consumption_events_emit_for_people_and_lite_people() {
	new_test_ext().execute_with(|| {
		setup_asset();

		let value = 0;
		let target_time = 10000;
		advance_until_time(target_time as u32);

		let period_duration = get_u32::<<Test as Config>::UnloadTokenTimePeriodPeopleLitePeople>();
		let current_period = (target_time as u32) / period_duration;

		let (people_secrets, people_index, people_revision) = setup_recycler(value, 1, 11);
		let people_dest = 101u64;
		let people_call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![CryptoOf::<Test>::alias_in_context(
				&people_secrets[0],
				UNLOADING_RECYCLER_CONTEXT.as_ref(),
			)
			.unwrap()],
			value,
			index: people_index,
			revision: people_revision,
			to: people_dest,
		});
		let people_ext = build_unload_free_token_ext(
			people_call,
			current_period,
			0,
			&people_secrets,
			value,
			people_index,
			[9u8; 32],
			FreeTokenType::People,
		);
		Executive::apply_extrinsic(people_ext).unwrap().unwrap();
		System::assert_has_event(
			crate::Event::<Test>::PeopleFreeUnloadTokenConsumed { period: current_period }.into(),
		);

		let (lite_secrets, lite_index, lite_revision) = setup_recycler(value, 1, 12);
		let lite_dest = 102u64;
		let lite_call = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![CryptoOf::<Test>::alias_in_context(
				&lite_secrets[0],
				UNLOADING_RECYCLER_CONTEXT.as_ref(),
			)
			.unwrap()],
			value,
			index: lite_index,
			revision: lite_revision,
			to: lite_dest,
		});
		let lite_ext = build_unload_free_token_ext(
			lite_call,
			current_period,
			0,
			&lite_secrets,
			value,
			lite_index,
			[10u8; 32],
			FreeTokenType::LitePeople,
		);
		Executive::apply_extrinsic(lite_ext).unwrap().unwrap();
		System::assert_has_event(
			crate::Event::<Test>::LitePeopleFreeUnloadTokenConsumed { period: current_period }
				.into(),
		);
	});
}

// This test verifies that a free unload token proof is:
// 1. Invalid before the period starts
// 2. Valid during the period
// 3. Valid during the grace window (when period is exactly 1 hour old)
// 4. Invalid after the grace window
#[test]
fn free_unload_token_period_validity() {
	new_test_ext().execute_with(|| {
		setup_asset();

		let value = 0;
		let period_duration = get_u32::<<Test as Config>::UnloadTokenTimePeriodPeopleLitePeople>();
		let grace_period: u32 = FREE_UNLOAD_TOKEN_GRACE_WINDOW;

		let target_period: u32 = 100;
		let period_start = target_period * period_duration;
		let period_end = (target_period + 1) * period_duration;
		let grace_window_end = period_end + grace_period;

		// ====================
		// 1. Before the period - proof should be INVALID
		// ====================
		advance_until_time(period_start - 2);

		let (secrets_1, index_1, revision_1) = setup_recycler(value, 1, 10);
		let dest_1 = 1001u64;
		let call_1 = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![CryptoOf::<Test>::alias_in_context(
				&secrets_1[0],
				UNLOADING_RECYCLER_CONTEXT.as_ref(),
			)
			.unwrap()],
			value,
			index: index_1,
			revision: revision_1,
			to: dest_1,
		});
		let ext_before = build_unload_free_token_ext(
			call_1.clone(),
			target_period,
			0,
			&secrets_1,
			value,
			index_1,
			[1u8; 32],
			FreeTokenType::People,
		);

		assert_invalid(ext_before, CustomInvalidity::InvalidUnloadTokenPeriod);

		// ====================
		// 2. During the period - proof should be VALID
		// ====================
		advance_until_time(period_start);

		let (secrets_2, index_2, revision_2) = setup_recycler(value, 1, 20);
		let dest_2 = 1002u64;
		let call_2 = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![CryptoOf::<Test>::alias_in_context(
				&secrets_2[0],
				UNLOADING_RECYCLER_CONTEXT.as_ref(),
			)
			.unwrap()],
			value,
			index: index_2,
			revision: revision_2,
			to: dest_2,
		});
		let ext_during = build_unload_free_token_ext(
			call_2.clone(),
			target_period,
			0,
			&secrets_2,
			value,
			index_2,
			[1u8; 32],
			FreeTokenType::People,
		);

		Executive::apply_extrinsic(ext_during).unwrap().unwrap();

		// ====================
		// 3. During grace window - proof should be VALID
		// ====================
		advance_until_time(grace_window_end - 2);

		let (secrets_3, index_3, revision_3) = setup_recycler(value, 1, 30);
		let dest_3 = 1003u64;
		let call_3 = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![CryptoOf::<Test>::alias_in_context(
				&secrets_3[0],
				UNLOADING_RECYCLER_CONTEXT.as_ref(),
			)
			.unwrap()],
			value,
			index: index_3,
			revision: revision_3,
			to: dest_3,
		});
		// Use different alias [2u8; 32] since [1u8; 32] already consumed for this period
		let ext_grace = build_unload_free_token_ext(
			call_3.clone(),
			target_period,
			0,
			&secrets_3,
			value,
			index_3,
			[2u8; 32],
			FreeTokenType::People,
		);

		Executive::apply_extrinsic(ext_grace).unwrap().unwrap();

		// ====================
		// 4. After grace window - proof should be INVALID
		// ====================
		advance_until_time(grace_window_end);

		let (secrets_4, index_4, revision_4) = setup_recycler(value, 1, 40);
		let dest_4 = 1004u64;
		let call_4 = RuntimeCall::Coinage(crate::Call::unload_recycler_into_coin {
			instance_id: TEST_INSTANCE_ID,
			aliases: bounded_vec![CryptoOf::<Test>::alias_in_context(
				&secrets_4[0],
				UNLOADING_RECYCLER_CONTEXT.as_ref(),
			)
			.unwrap()],
			value,
			index: index_4,
			revision: revision_4,
			to: dest_4,
		});
		// Use different alias [3u8; 32]
		let ext_after = build_unload_free_token_ext(
			call_4.clone(),
			target_period,
			0,
			&secrets_4,
			value,
			index_4,
			[3u8; 32],
			FreeTokenType::People,
		);

		assert_invalid(ext_after, CustomInvalidity::InvalidUnloadTokenPeriod);
	});
}

/// Test that the free unload token counter limit for people changes dynamically based on
/// the unload token fee (controlled by MockPaidUnloadTokenFeeOverride).
#[test]
fn free_unload_token_counter_limit_people_changes_with_fee() {
	new_test_ext().execute_with(|| {
		setup_asset();

		let value = 0;
		let period_duration = get_u32::<<Test as Config>::UnloadTokenTimePeriodPeopleLitePeople>();

		// Advance to a known period
		let target_time = 10000;
		advance_until_time(target_time);
		let current_period = target_time / period_duration;

		// ============================================================
		// Phase 1: Default fee (MockPaidUnloadTokenFeeOverride = Some(2))
		// People allowance = 10 (UNLOAD_TOKEN_ALLOWANCE_PER_TIME_PERIOD_FOR_PEOPLE)
		// Fee = 2 (MockPaidUnloadTokenFeeOverride)
		// Limit = 10 / 2 = 5
		// Valid counters: 0, 1, 2, 3, 4 (counter < 5)
		// ============================================================
		assert_eq!(Coinage::free_unload_token_limit_for_people(), 5);

		// Counter 4 (max valid) should be valid
		assert!(
			test_free_unload_counter_validity(
				FreeTokenType::People,
				value,
				current_period,
				4,
				0,
				[10u8; 32],
				10000,
			),
			"Counter 4 should be valid with fee=2"
		);
		// Counter 5 (limit) should be invalid
		assert!(
			!test_free_unload_counter_validity(
				FreeTokenType::People,
				value,
				current_period,
				5,
				1,
				[11u8; 32],
				10000,
			),
			"Counter 5 should be invalid with fee=2"
		);

		// ============================================================
		// Phase 2: Lower fee (MockPaidUnloadTokenFeeOverride = Some(1))
		// People allowance = 10, fee = 1, dynamic limit = 10.
		// Hard cap = 8, effective limit = min(10, 8) = 8.
		// Valid counters: 0..7 (counter < 8)
		// ============================================================
		MockPaidUnloadTokenFeeOverride::set(&Some(1));
		assert_eq!(Coinage::free_unload_token_limit_for_people(), 8);

		// Counter 7 (max valid) should be valid
		assert!(
			test_free_unload_counter_validity(
				FreeTokenType::People,
				value,
				current_period,
				7,
				2,
				[20u8; 32],
				10000,
			),
			"Counter 7 should be valid with fee=1"
		);
		// Counter 8 (limit) should be invalid
		assert!(
			!test_free_unload_counter_validity(
				FreeTokenType::People,
				value,
				current_period,
				8,
				3,
				[21u8; 32],
				10000,
			),
			"Counter 8 should be invalid with fee=1"
		);

		// ============================================================
		// Phase 3: Higher fee (MockPaidUnloadTokenFeeOverride = Some(5))
		// People allowance = 10, fee = 5, limit = 10 / 5 = 2
		// Valid counters: 0, 1 (counter < 2)
		// ============================================================
		MockPaidUnloadTokenFeeOverride::set(&Some(5));
		assert_eq!(Coinage::free_unload_token_limit_for_people(), 2);

		// Counter 1 (max valid) should be valid
		assert!(
			test_free_unload_counter_validity(
				FreeTokenType::People,
				value,
				current_period,
				1,
				4,
				[30u8; 32],
				10000,
			),
			"Counter 1 should be valid with fee=5"
		);
		// Counter 2 (limit) should be invalid
		assert!(
			!test_free_unload_counter_validity(
				FreeTokenType::People,
				value,
				current_period,
				2,
				5,
				[31u8; 32],
				10000,
			),
			"Counter 2 should be invalid with fee=5"
		);
	});
}

/// Test that the free unload token counter limit for lite people changes dynamically based on
/// the unload token fee (controlled by MockPaidUnloadTokenFeeOverride).
#[test]
fn free_unload_token_counter_limit_lite_people_changes_with_fee() {
	new_test_ext().execute_with(|| {
		setup_asset();

		let value = 0;
		let period_duration = get_u32::<<Test as Config>::UnloadTokenTimePeriodPeopleLitePeople>();

		// Advance to a known period
		let target_time = 10000;
		advance_until_time(target_time);
		let current_period = target_time / period_duration;

		// ============================================================
		// Phase 1: Default fee (MockPaidUnloadTokenFeeOverride = Some(2))
		// Lite people allowance = 4, fee = 2, limit = 4 / 2 = 2
		// Valid counters: 0, 1 (counter < 2)
		// ============================================================
		assert_eq!(Coinage::free_unload_token_limit_for_lite_people(), 2);

		// Counter 1 (max valid) should be valid
		assert!(
			test_free_unload_counter_validity(
				FreeTokenType::LitePeople,
				value,
				current_period,
				1,
				0,
				[40u8; 32],
				20000,
			),
			"Counter 1 should be valid with fee=2"
		);
		// Counter 2 (limit) should be invalid
		assert!(
			!test_free_unload_counter_validity(
				FreeTokenType::LitePeople,
				value,
				current_period,
				2,
				1,
				[41u8; 32],
				20000,
			),
			"Counter 2 should be invalid with fee=2"
		);

		// ============================================================
		// Phase 2: Lower fee (MockPaidUnloadTokenFeeOverride = Some(1))
		// Lite people allowance = 4, fee = 1, limit = 4 / 1 = 4
		// Valid counters: 0, 1, 2, 3 (counter < 4)
		// ============================================================
		MockPaidUnloadTokenFeeOverride::set(&Some(1));
		assert_eq!(Coinage::free_unload_token_limit_for_lite_people(), 4);

		// Counter 3 (max valid) should be valid
		assert!(
			test_free_unload_counter_validity(
				FreeTokenType::LitePeople,
				value,
				current_period,
				3,
				2,
				[50u8; 32],
				20000,
			),
			"Counter 3 should be valid with fee=1"
		);
		// Counter 4 (limit) should be invalid
		assert!(
			!test_free_unload_counter_validity(
				FreeTokenType::LitePeople,
				value,
				current_period,
				4,
				3,
				[51u8; 32],
				20000,
			),
			"Counter 4 should be invalid with fee=1"
		);

		// ============================================================
		// Phase 3: Higher fee (MockPaidUnloadTokenFeeOverride = Some(4))
		// Lite people allowance = 4, fee = 4, limit = 4 / 4 = 1
		// Valid counters: 0 (counter < 1)
		// ============================================================
		MockPaidUnloadTokenFeeOverride::set(&Some(4));
		assert_eq!(Coinage::free_unload_token_limit_for_lite_people(), 1);

		// Counter 0 (max valid) should be valid
		assert!(
			test_free_unload_counter_validity(
				FreeTokenType::LitePeople,
				value,
				current_period,
				0,
				4,
				[60u8; 32],
				20000,
			),
			"Counter 0 should be valid with fee=4"
		);
		// Counter 1 (limit) should be invalid
		assert!(
			!test_free_unload_counter_validity(
				FreeTokenType::LitePeople,
				value,
				current_period,
				1,
				5,
				[61u8; 32],
				20000,
			),
			"Counter 1 should be invalid with fee=4"
		);
	});
}

#[test]
fn free_unload_token_limit_returns_zero_when_fee_is_zero() {
	new_test_ext().execute_with(|| {
		setup_asset();
		MockPaidUnloadTokenFeeOverride::set(&Some(0));

		assert_eq!(Coinage::free_unload_token_limit_for_people(), 0);
		assert_eq!(Coinage::free_unload_token_limit_for_lite_people(), 0);
	});
}

#[test]
fn free_unload_token_counter_is_rejected_when_fee_is_zero() {
	new_test_ext().execute_with(|| {
		setup_asset();

		let value = 0;
		let period_duration = get_u32::<<Test as Config>::UnloadTokenTimePeriodPeopleLitePeople>();

		// Advance to a known period
		let target_time = 10000;
		advance_until_time(target_time);
		let current_period = target_time / period_duration;

		MockPaidUnloadTokenFeeOverride::set(&Some(0));

		assert_eq!(Coinage::free_unload_token_limit_for_people(), 0);
		assert_eq!(Coinage::free_unload_token_limit_for_lite_people(), 0);

		// With zero limit, even counter=0 must be rejected for both token types.
		assert!(
			!test_free_unload_counter_validity(
				FreeTokenType::People,
				value,
				current_period,
				0,
				6,
				[70u8; 32],
				30000,
			),
			"Counter 0 should be invalid for people with fee=0"
		);
		assert!(
			!test_free_unload_counter_validity(
				FreeTokenType::LitePeople,
				value,
				current_period,
				0,
				7,
				[71u8; 32],
				30000,
			),
			"Counter 0 should be invalid for lite people with fee=0"
		);
	});
}

#[test]
fn free_unload_token_limit_handles_flooring_and_fee_above_allowance() {
	new_test_ext().execute_with(|| {
		setup_asset();

		// Integer division should floor: people=10/3=3, lite=4/3=1.
		MockPaidUnloadTokenFeeOverride::set(&Some(3));
		assert_eq!(Coinage::free_unload_token_limit_for_people(), 3);
		assert_eq!(Coinage::free_unload_token_limit_for_lite_people(), 1);

		// A non-zero fee above allowance should still yield zero free tokens.
		MockPaidUnloadTokenFeeOverride::set(&Some(11));
		assert_eq!(Coinage::free_unload_token_limit_for_people(), 0);
		assert_eq!(Coinage::free_unload_token_limit_for_lite_people(), 0);
	});
}
